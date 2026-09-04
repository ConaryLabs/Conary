// apps/conary/src/commands/export/tests.rs

use super::*;
use conary_core::generation::artifact::{
    ArtifactWriteInputs, BOOT_ASSETS_DIR, BootAssetsManifest, CasObjectVerification,
    GenerationArtifact, load_generation_artifact, write_generation_artifact,
};
use conary_core::generation::metadata::GENERATION_FORMAT;
use conary_core::generation::root_manifest::{
    GENERATION_ROOT_MANIFEST_VERSION, GenerationRootEntry, GenerationRootManifest,
    MutableStateManifest,
};
use conary_core::payload::{
    PayloadContentAuthority, PayloadNode, PayloadNodeKind, ResolvedPayloadNode,
};
use std::path::PathBuf;
use tempfile::TempDir;

fn write_cas_object(objects_dir: &Path, bytes: &[u8]) -> CasObjectRef {
    let sha256 = hex_digest(bytes);
    let object_path = object_path(objects_dir, &sha256).unwrap();
    fs::create_dir_all(object_path.parent().unwrap()).unwrap();
    fs::write(object_path, bytes).unwrap();
    CasObjectRef {
        sha256,
        size: bytes.len() as u64,
    }
}

fn write_test_root_manifests(generation_dir: &Path, objects: &[CasObjectRef]) {
    let mut root = PayloadNode::regular(0o755);
    root.kind = PayloadNodeKind::Directory;
    root.mode = libc::S_IFDIR | 0o755;
    let root = ResolvedPayloadNode::from_numeric_source(root).unwrap();
    let mut entries = vec![GenerationRootEntry {
        path: "/objects".to_string(),
        node: root.clone(),
        content: None,
    }];
    entries.extend(objects.iter().map(|object| GenerationRootEntry {
        path: format!("/objects/{}", object.sha256),
        node: ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o644)).unwrap(),
        content: Some(PayloadContentAuthority {
            sha256: object.sha256.clone(),
            size: object.size,
        }),
    }));
    entries.sort_by(|left, right| left.path.cmp(&right.path));

    GenerationRootManifest {
        version: GENERATION_ROOT_MANIFEST_VERSION,
        root,
        entries,
    }
    .write_to(generation_dir)
    .unwrap();
    MutableStateManifest::empty()
        .write_to(generation_dir)
        .unwrap();
}

/// Create a minimal artifact-backed generation directory for testing.
fn create_test_generation(tmp: &Path) -> GenerationArtifact {
    let gen_dir = tmp.join("generations/5");
    let objects_dir = tmp.join("objects");
    let boot_assets_dir = gen_dir.join(BOOT_ASSETS_DIR);
    fs::create_dir_all(boot_assets_dir.join("EFI/BOOT")).unwrap();
    fs::create_dir_all(&objects_dir).unwrap();

    // Write a fake EROFS image (just some bytes for testing)
    let erofs_data = b"EROFS-IMAGE-PLACEHOLDER-DATA-FOR-TESTING";
    fs::write(gen_dir.join(EROFS_IMAGE_NAME), erofs_data).unwrap();

    fs::write(boot_assets_dir.join("vmlinuz"), b"kernel").unwrap();
    fs::write(boot_assets_dir.join("initramfs.img"), b"initramfs").unwrap();
    fs::write(boot_assets_dir.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();

    let object_one = write_cas_object(&objects_dir, b"file-content-one");
    let object_two = write_cas_object(&objects_dir, b"file-content-two");
    write_test_root_manifests(&gen_dir, &[object_one, object_two]);

    let boot_assets = BootAssetsManifest {
        version: 1,
        generation: 5,
        architecture: "x86_64".to_string(),
        kernel_version: "6.12.1-conary".to_string(),
        kernel: "vmlinuz".to_string(),
        kernel_sha256: hex_digest(b"kernel"),
        initramfs: "initramfs.img".to_string(),
        initramfs_sha256: hex_digest(b"initramfs"),
        efi_bootloader: "EFI/BOOT/BOOTX64.EFI".to_string(),
        efi_bootloader_sha256: hex_digest(b"efi"),
        created_at: "2026-03-17T12:00:00Z".to_string(),
    };

    let artifact_digest = write_generation_artifact(ArtifactWriteInputs {
        generation_dir: &gen_dir,
        generation: 5,
        architecture: "x86_64",
        erofs_path: &gen_dir.join(EROFS_IMAGE_NAME),
        cas_base_rel: "../../objects",
        cas_verification: CasObjectVerification::Deep,
        boot_assets,
        carrier_capabilities: Default::default(),
    })
    .unwrap();

    // Write generation metadata
    let metadata = GenerationMetadata {
        generation: 5,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(erofs_data.len() as i64),
        cas_objects_referenced: Some(2),
        fsverity_enabled: false,
        erofs_verity_digest: None,
        artifact_manifest_sha256: Some(artifact_digest),
        security_capability_xattr_count: None,
        created_at: "2026-03-17T12:00:00Z".to_string(),
        package_count: 42,
        kernel_version: Some("6.12.1-conary".to_string()),
        summary: "test generation".to_string(),
    };
    metadata.write_to(&gen_dir).unwrap();

    load_generation_artifact(&gen_dir).unwrap()
}

/// Helper: export from a test generation artifact to OCI.
fn export_test_generation(artifact: &GenerationArtifact) -> (TempDir, PathBuf) {
    let output_tmp = TempDir::new().unwrap();
    let output_dir = output_tmp.path().join("oci-out");
    fs::create_dir_all(&output_dir).unwrap();

    export_oci_artifact(artifact, &artifact.cas_dir, &output_dir).unwrap();

    (output_tmp, output_dir)
}

#[test]
fn test_oci_layout_structure() {
    let tmp = TempDir::new().unwrap();
    let artifact = create_test_generation(tmp.path());
    let (_output_tmp, output_dir) = export_test_generation(&artifact);

    // Verify all expected files exist
    assert!(output_dir.join("oci-layout").exists(), "oci-layout missing");
    assert!(output_dir.join("index.json").exists(), "index.json missing");
    assert!(
        output_dir.join("blobs/sha256").exists(),
        "blobs/sha256 directory missing"
    );

    // Verify oci-layout content
    let layout = fs::read_to_string(output_dir.join("oci-layout")).unwrap();
    let layout_json: serde_json::Value = serde_json::from_str(&layout).unwrap();
    assert_eq!(layout_json["imageLayoutVersion"], "1.0.0");

    // Verify we have blobs (at least manifest, config, layer = 3)
    let blob_count = fs::read_dir(output_dir.join("blobs/sha256"))
        .unwrap()
        .count();
    assert!(
        blob_count >= 3,
        "Expected at least 3 blobs (manifest, config, layer), got {blob_count}"
    );
}

#[test]
fn test_oci_manifest_valid_json() {
    let tmp = TempDir::new().unwrap();
    let artifact = create_test_generation(tmp.path());
    let (_output_tmp, output_dir) = export_test_generation(&artifact);

    // Read index.json to find the manifest digest
    let index_str = fs::read_to_string(output_dir.join("index.json")).unwrap();
    let index: serde_json::Value = serde_json::from_str(&index_str).unwrap();

    let manifests = index["manifests"].as_array().unwrap();
    assert_eq!(manifests.len(), 1);

    let manifest_desc = &manifests[0];
    assert_eq!(manifest_desc["mediaType"], MANIFEST_MEDIA_TYPE);

    // Read the manifest blob
    let manifest_digest = manifest_desc["digest"]
        .as_str()
        .unwrap()
        .strip_prefix("sha256:")
        .unwrap();
    let manifest_path = output_dir.join("blobs/sha256").join(manifest_digest);
    assert!(manifest_path.exists(), "Manifest blob not found");

    let manifest_str = fs::read_to_string(&manifest_path).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest_str).unwrap();

    assert_eq!(manifest["schemaVersion"], 2);
    assert_eq!(manifest["mediaType"], MANIFEST_MEDIA_TYPE);
    assert_eq!(manifest["config"]["mediaType"], CONFIG_MEDIA_TYPE);

    let layers = manifest["layers"].as_array().unwrap();
    assert_eq!(layers.len(), 1);
    assert_eq!(layers[0]["mediaType"], LAYER_MEDIA_TYPE);

    // Verify the layer size is a positive number
    let layer_size = layers[0]["size"].as_u64().unwrap();
    assert!(layer_size > 0, "Layer size should be positive");
}

#[test]
fn test_oci_layer_contains_erofs() {
    let tmp = TempDir::new().unwrap();
    let artifact = create_test_generation(tmp.path());
    let (_output_tmp, output_dir) = export_test_generation(&artifact);

    // Find the layer blob via manifest -> layers[0].digest
    let index_str = fs::read_to_string(output_dir.join("index.json")).unwrap();
    let index: serde_json::Value = serde_json::from_str(&index_str).unwrap();
    let manifest_digest = index["manifests"][0]["digest"]
        .as_str()
        .unwrap()
        .strip_prefix("sha256:")
        .unwrap();

    let manifest_str =
        fs::read_to_string(output_dir.join("blobs/sha256").join(manifest_digest)).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest_str).unwrap();
    let layer_digest = manifest["layers"][0]["digest"]
        .as_str()
        .unwrap()
        .strip_prefix("sha256:")
        .unwrap();

    let layer_path = output_dir.join("blobs/sha256").join(layer_digest);
    assert!(layer_path.exists(), "Layer blob not found");

    // Decompress and read the tar
    let compressed_data = fs::read(&layer_path).unwrap();
    let decoder = flate2::read::GzDecoder::new(compressed_data.as_slice());
    let mut archive = tar::Archive::new(decoder);

    let mut found_erofs = false;
    let mut found_objects = false;
    let mut found_metadata = false;
    let mut entry_names: Vec<String> = Vec::new();

    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().to_string();
        entry_names.push(path.clone());

        if path == EROFS_IMAGE_NAME {
            found_erofs = true;
            // Verify the content matches our test EROFS data
            let size = entry.header().size().unwrap();
            assert_eq!(
                size,
                b"EROFS-IMAGE-PLACEHOLDER-DATA-FOR-TESTING".len() as u64
            );
        }
        if path.starts_with("objects/") {
            found_objects = true;
        }
        if path == GENERATION_METADATA_FILE {
            found_metadata = true;
        }
    }

    assert!(
        found_erofs,
        "root.erofs not found in layer tar. Entries: {entry_names:?}"
    );
    assert!(
        found_objects,
        "No CAS objects found in layer tar. Entries: {entry_names:?}"
    );
    assert!(
        found_metadata,
        ".conary-gen.json not found in layer tar. Entries: {entry_names:?}"
    );
}

#[test]
fn test_oci_config_labels() {
    let tmp = TempDir::new().unwrap();
    let artifact = create_test_generation(tmp.path());
    let (_output_tmp, output_dir) = export_test_generation(&artifact);

    // Find config blob via manifest
    let index_str = fs::read_to_string(output_dir.join("index.json")).unwrap();
    let index: serde_json::Value = serde_json::from_str(&index_str).unwrap();
    let manifest_digest = index["manifests"][0]["digest"]
        .as_str()
        .unwrap()
        .strip_prefix("sha256:")
        .unwrap();

    let manifest_str =
        fs::read_to_string(output_dir.join("blobs/sha256").join(manifest_digest)).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest_str).unwrap();
    let config_digest = manifest["config"]["digest"]
        .as_str()
        .unwrap()
        .strip_prefix("sha256:")
        .unwrap();

    let config_str =
        fs::read_to_string(output_dir.join("blobs/sha256").join(config_digest)).unwrap();
    let config: serde_json::Value = serde_json::from_str(&config_str).unwrap();

    assert_eq!(config["architecture"], oci_arch());
    assert_eq!(config["os"], "linux");

    let labels = &config["config"]["Labels"];
    assert_eq!(labels["io.conary.generation"], "5");
    assert_eq!(labels["io.conary.package-count"], "42");
    assert_eq!(labels["io.conary.kernel"], "6.12.1-conary");
    assert_eq!(labels["io.conary.format"], "composefs-native");
}

#[test]
fn test_oci_blob_integrity() {
    let tmp = TempDir::new().unwrap();
    let artifact = create_test_generation(tmp.path());
    let (_output_tmp, output_dir) = export_test_generation(&artifact);

    // Every blob file should have a name that matches its SHA-256 digest
    for entry in fs::read_dir(output_dir.join("blobs/sha256")).unwrap() {
        let entry = entry.unwrap();
        let filename = entry.file_name().to_string_lossy().to_string();

        // Skip temp files
        if filename.ends_with(".tmp") {
            continue;
        }

        let data = fs::read(entry.path()).unwrap();
        let actual_digest = hex_digest(&data);

        assert_eq!(
            filename, actual_digest,
            "Blob filename does not match its SHA-256 digest"
        );
    }
}

#[test]
fn test_hex_digest() {
    // Known SHA-256 of empty string
    let digest = hex_digest(b"");
    assert_eq!(
        digest,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn test_oci_layer_uses_manifest_cas_scope() {
    let tmp = TempDir::new().unwrap();
    let artifact = create_test_generation(tmp.path());
    let mut scoped_artifact = artifact.clone();
    scoped_artifact.cas_objects = vec![artifact.cas_objects[0].clone()];
    let (_output_tmp, output_dir) = export_test_generation(&scoped_artifact);

    let index_str = fs::read_to_string(output_dir.join("index.json")).unwrap();
    let index: serde_json::Value = serde_json::from_str(&index_str).unwrap();
    let manifest_digest = index["manifests"][0]["digest"]
        .as_str()
        .unwrap()
        .strip_prefix("sha256:")
        .unwrap();

    let manifest_str =
        fs::read_to_string(output_dir.join("blobs/sha256").join(manifest_digest)).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest_str).unwrap();
    let layer_digest = manifest["layers"][0]["digest"]
        .as_str()
        .unwrap()
        .strip_prefix("sha256:")
        .unwrap();

    let compressed_data = fs::read(output_dir.join("blobs/sha256").join(layer_digest)).unwrap();
    let decoder = flate2::read::GzDecoder::new(compressed_data.as_slice());
    let mut archive = tar::Archive::new(decoder);
    let mut object_entries = Vec::new();

    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().to_string();
        if path.starts_with("objects/") {
            object_entries.push(path);
        }
    }

    let included = format!(
        "objects/{}/{}",
        &artifact.cas_objects[0].sha256[..2],
        &artifact.cas_objects[0].sha256[2..]
    );
    let excluded = format!(
        "objects/{}/{}",
        &artifact.cas_objects[1].sha256[..2],
        &artifact.cas_objects[1].sha256[2..]
    );

    assert!(
        object_entries.contains(&included),
        "manifest-listed CAS object missing from layer: {object_entries:?}"
    );
    assert!(
        !object_entries.contains(&excluded),
        "unlisted CAS object must not be exported from the CAS directory walk"
    );
}

#[test]
fn test_oci_export_uses_artifact_cas_dir_when_cli_dir_differs() {
    let tmp = TempDir::new().unwrap();
    let artifact = create_test_generation(tmp.path());
    let requested_objects_dir = tmp.path().join("empty-requested-objects");
    fs::create_dir_all(&requested_objects_dir).unwrap();
    let output_tmp = TempDir::new().unwrap();
    let output_dir = output_tmp.path().join("oci-out");
    fs::create_dir_all(&output_dir).unwrap();

    export_oci_artifact(&artifact, &requested_objects_dir, &output_dir).unwrap();

    let index_str = fs::read_to_string(output_dir.join("index.json")).unwrap();
    let index: serde_json::Value = serde_json::from_str(&index_str).unwrap();
    let manifest_digest = index["manifests"][0]["digest"]
        .as_str()
        .unwrap()
        .strip_prefix("sha256:")
        .unwrap();
    let manifest_str =
        fs::read_to_string(output_dir.join("blobs/sha256").join(manifest_digest)).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest_str).unwrap();
    let layer_digest = manifest["layers"][0]["digest"]
        .as_str()
        .unwrap()
        .strip_prefix("sha256:")
        .unwrap();

    let compressed_data = fs::read(output_dir.join("blobs/sha256").join(layer_digest)).unwrap();
    let decoder = flate2::read::GzDecoder::new(compressed_data.as_slice());
    let mut archive = tar::Archive::new(decoder);
    let object_count = archive
        .entries()
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .map(|path| path.to_string_lossy().starts_with("objects/"))
                .unwrap_or(false)
        })
        .count();

    assert_eq!(
        object_count, 2,
        "artifact CAS directory must remain authoritative even when the CLI requested directory is empty"
    );
}

#[test]
fn test_oci_artifact_export_requires_generation_metadata_file() {
    let tmp = TempDir::new().unwrap();
    let artifact = create_test_generation(tmp.path());
    fs::remove_file(artifact.generation_dir.join(GENERATION_METADATA_FILE)).unwrap();
    let output_tmp = TempDir::new().unwrap();
    let output_dir = output_tmp.path().join("oci-out");
    fs::create_dir_all(&output_dir).unwrap();

    let err = export_oci_artifact(&artifact, &artifact.cas_dir, &output_dir).unwrap_err();

    assert!(
        err.to_string().contains("generation metadata"),
        "expected metadata failure, got {err:#}"
    );
}

#[cfg(unix)]
#[test]
fn test_current_oci_loader_follows_current_symlink_to_artifact() {
    let tmp = TempDir::new().unwrap();
    let artifact = create_test_generation(tmp.path());
    std::os::unix::fs::symlink("generations/5", tmp.path().join("current")).unwrap();

    let loaded = load_current_oci_generation_artifact(&tmp.path().join("current")).unwrap();

    assert_eq!(loaded.generation, artifact.generation);
    assert_eq!(loaded.generation_dir, tmp.path().join("current"));
    assert_eq!(loaded.cas_objects, artifact.cas_objects);
}

#[test]
fn test_build_config_json_valid() {
    let metadata = GenerationMetadata {
        generation: 3,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(100),
        cas_objects_referenced: Some(10),
        fsverity_enabled: false,
        erofs_verity_digest: None,
        artifact_manifest_sha256: None,
        security_capability_xattr_count: None,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        package_count: 50,
        kernel_version: None,
        summary: "test".to_string(),
    };

    let json_str = build_config_json(&metadata, 3, "abcdef1234567890");
    let config: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert_eq!(config["os"], "linux");
    assert_eq!(config["architecture"], oci_arch());
    assert_eq!(config["rootfs"]["diff_ids"][0], "sha256:abcdef1234567890");
    assert_eq!(config["rootfs"]["type"], "layers");
}

#[test]
fn test_build_manifest_json_valid() {
    let json_str = build_manifest_json("cfgdigest", 100, "layerdigest", 5000);
    let manifest: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert_eq!(manifest["schemaVersion"], 2);
    assert_eq!(manifest["config"]["digest"], "sha256:cfgdigest");
    assert_eq!(manifest["config"]["size"], 100);
    assert_eq!(manifest["layers"][0]["digest"], "sha256:layerdigest");
    assert_eq!(manifest["layers"][0]["size"], 5000);
}

#[test]
fn test_build_index_json_valid() {
    let json_str = build_index_json("manifestdigest", 200);
    let index: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert_eq!(index["schemaVersion"], 2);
    assert_eq!(index["manifests"][0]["digest"], "sha256:manifestdigest");
    assert_eq!(index["manifests"][0]["size"], 200);
    assert_eq!(
        index["manifests"][0]["platform"]["architecture"],
        oci_arch()
    );
    assert_eq!(index["manifests"][0]["platform"]["os"], "linux");
}
