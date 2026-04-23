// src/commands/export.rs
//! Export Conary generations as OCI container images.
//!
//! Produces a standards-compliant OCI Image Layout directory from a
//! generation's EROFS image and CAS objects.  The resulting directory
//! can be loaded directly by `podman load` or `docker load` (after
//! converting with `skopeo copy oci:dir docker-archive:file.tar`).
//!
//! OCI Image Layout Specification:
//!   https://github.com/opencontainers/image-spec/blob/main/image-layout.md

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use conary_core::filesystem::CasStore;
use flate2::Compression;
use flate2::write::GzEncoder;
use tracing::info;

use super::generation::metadata::{GenerationMetadata, generation_path};
use conary_core::db::models::SystemState;
use conary_core::generation::gc::live_cas_hashes;
use conary_core::generation::metadata::{EROFS_IMAGE_NAME, GENERATION_METADATA_FILE};
use conary_core::generation::mount::current_generation;

/// OCI media types used in the manifest and config.
const MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
const CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.image.config.v1+json";
const LAYER_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar+gzip";
const INDEX_MEDIA_TYPE: &str = "application/vnd.oci.image.index.v1+json";

/// Map the host architecture to the OCI platform architecture string.
fn oci_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "riscv64" => "riscv64",
        other => other,
    }
}

/// Export a generation as an OCI image directory layout.
///
/// The generation is identified by number.  Pass `None` to use the
/// currently active generation.  The `objects_dir` points to the CAS
/// store (typically `/conary/objects`).  The `db_path` is used to
/// scope the exported CAS objects to only those referenced by the
/// generation's system state.
pub async fn export_oci(
    generation: Option<i64>,
    objects_dir: &Path,
    output_dir: &Path,
    db_path: &str,
) -> Result<()> {
    // Resolve generation number
    let gen_number = match generation {
        Some(n) => n,
        None => current_generation(Path::new("/conary"))?
            .ok_or_else(|| anyhow!("No active generation found"))?,
    };

    let gen_dir = generation_path(gen_number);
    if !gen_dir.exists() {
        bail!(
            "Generation {gen_number} does not exist at {}",
            gen_dir.display()
        );
    }

    let metadata = GenerationMetadata::read_from(&gen_dir)
        .with_context(|| format!("Failed to read metadata for generation {gen_number}"))?;

    let erofs_path = gen_dir.join(EROFS_IMAGE_NAME);
    if !erofs_path.exists() {
        bail!(
            "Generation {gen_number} has no EROFS image at {}",
            erofs_path.display()
        );
    }

    info!(
        "Exporting generation {gen_number} as OCI image to {}",
        output_dir.display()
    );

    // Scope CAS objects to the generation by querying the database for
    // the generation's state_id and collecting only referenced hashes.
    let scoped_hashes = scope_cas_hashes_for_generation(db_path, gen_number, objects_dir)?;
    info!(
        "Generation {gen_number} references {} CAS objects",
        scoped_hashes.len()
    );

    // Create OCI directory structure
    let blobs_dir = output_dir.join("blobs/sha256");
    fs::create_dir_all(&blobs_dir).with_context(|| {
        format!(
            "Failed to create blobs directory at {}",
            blobs_dir.display()
        )
    })?;

    // Step 1: Build the layer tar.gz
    let (layer_digest, layer_size, diff_id) = build_layer_tar_gz(
        &erofs_path,
        objects_dir,
        &gen_dir,
        &blobs_dir,
        &scoped_hashes,
    )?;
    info!(
        "Layer: sha256:{} ({} bytes, diffID: sha256:{})",
        layer_digest, layer_size, diff_id
    );

    // Step 2: Build the config JSON
    let config_json = build_config_json(&metadata, gen_number, &diff_id);
    let (config_digest, config_size) = write_blob(&blobs_dir, config_json.as_bytes())?;
    info!("Config: sha256:{} ({} bytes)", config_digest, config_size);

    // Step 3: Build the manifest JSON
    let manifest_json = build_manifest_json(&config_digest, config_size, &layer_digest, layer_size);
    let (manifest_digest, manifest_size) = write_blob(&blobs_dir, manifest_json.as_bytes())?;
    info!(
        "Manifest: sha256:{} ({} bytes)",
        manifest_digest, manifest_size
    );

    // Step 4: Write index.json
    let index_json = build_index_json(&manifest_digest, manifest_size);
    fs::write(output_dir.join("index.json"), &index_json).context("Failed to write index.json")?;

    // Step 5: Write oci-layout
    let oci_layout = r#"{"imageLayoutVersion":"1.0.0"}"#;
    fs::write(output_dir.join("oci-layout"), oci_layout).context("Failed to write oci-layout")?;

    println!(
        "Exported generation {gen_number} to {}",
        output_dir.display()
    );
    println!("  Manifest: sha256:{manifest_digest}");
    println!("  Layer:    sha256:{layer_digest} ({layer_size} bytes)");
    println!();
    println!("Load with:");
    println!(
        "  podman load < $(skopeo copy oci:{} docker-archive:/dev/stdout)",
        output_dir.display()
    );
    println!(
        "  skopeo copy oci:{} docker://registry.example.com/conary-gen:{}",
        output_dir.display(),
        gen_number
    );

    Ok(())
}

/// Build a gzipped tar layer containing the EROFS image and referenced CAS objects.
///
/// Layout inside the tar:
///   root.erofs          -- the generation's EROFS image
///   objects/<prefix>/<suffix>  -- CAS objects referenced by the generation
///
/// Returns (compressed_digest, compressed_size, uncompressed_digest).
fn build_layer_tar_gz(
    erofs_path: &Path,
    objects_dir: &Path,
    gen_dir: &Path,
    blobs_dir: &Path,
    scoped_hashes: &HashSet<String>,
) -> Result<(String, u64, String)> {
    // We need both compressed digest (for the manifest) and uncompressed
    // digest (for the config's diff_id).  Build the tar in memory first
    // to compute the uncompressed hash, then gzip-compress and hash again.

    // Build uncompressed tar
    let tar_bytes = build_layer_tar(erofs_path, objects_dir, gen_dir, scoped_hashes)?;

    // Compute uncompressed (diffID) digest
    let diff_id = hex_digest(&tar_bytes);

    // Compress and write to a temp file, then compute compressed digest
    let temp_path = blobs_dir.join("layer.tmp");
    {
        let file = fs::File::create(&temp_path).context("Failed to create temporary layer file")?;
        let mut encoder = GzEncoder::new(file, Compression::default());
        encoder
            .write_all(&tar_bytes)
            .context("Failed to write compressed layer")?;
        encoder
            .finish()
            .context("Failed to finish gzip compression")?;
    }

    // Read back compressed bytes to compute digest
    let compressed_bytes = fs::read(&temp_path).context("Failed to read compressed layer")?;
    let compressed_digest = hex_digest(&compressed_bytes);
    let compressed_size = compressed_bytes.len() as u64;

    // Move to final location
    let final_path = blobs_dir.join(&compressed_digest);
    fs::rename(&temp_path, &final_path).context("Failed to move layer blob to final location")?;

    Ok((compressed_digest, compressed_size, diff_id))
}

/// Build an uncompressed tar archive with the generation's content.
fn build_layer_tar(
    erofs_path: &Path,
    objects_dir: &Path,
    gen_dir: &Path,
    scoped_hashes: &HashSet<String>,
) -> Result<Vec<u8>> {
    let buf = Vec::new();
    let mut tar_builder = tar::Builder::new(buf);

    // Add root.erofs
    let erofs_data = fs::read(erofs_path)
        .with_context(|| format!("Failed to read EROFS image at {}", erofs_path.display()))?;
    let mut header = tar::Header::new_gnu();
    header.set_size(erofs_data.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar_builder
        .append_data(&mut header, EROFS_IMAGE_NAME, erofs_data.as_slice())
        .context("Failed to add EROFS image to tar")?;

    // Use the pre-scoped CAS hashes (from the generation's DB state) to include
    // only objects referenced by this generation, not the entire CAS store.
    let cas = CasStore::new(objects_dir)
        .with_context(|| format!("Failed to open CAS directory {}", objects_dir.display()))?;

    for hash in scoped_hashes {
        let obj_path = cas
            .hash_to_path(hash)
            .with_context(|| format!("Invalid CAS hash {hash}"))?;

        if !obj_path.exists() {
            continue;
        }

        let prefix = &hash[..2];
        let suffix = &hash[2..];

        let data =
            fs::read(&obj_path).with_context(|| format!("Failed to read CAS object {hash}"))?;
        let mut obj_header = tar::Header::new_gnu();
        obj_header.set_size(data.len() as u64);
        obj_header.set_mode(0o644);
        obj_header.set_cksum();
        let tar_path = format!("objects/{prefix}/{suffix}");
        tar_builder
            .append_data(&mut obj_header, &tar_path, data.as_slice())
            .with_context(|| format!("Failed to add CAS object {hash} to tar"))?;
    }

    // Also include the generation metadata
    let meta_path = gen_dir.join(GENERATION_METADATA_FILE);
    if meta_path.exists() {
        let meta_data = fs::read(&meta_path).context("Failed to read generation metadata")?;
        let mut meta_header = tar::Header::new_gnu();
        meta_header.set_size(meta_data.len() as u64);
        meta_header.set_mode(0o644);
        meta_header.set_cksum();
        tar_builder
            .append_data(
                &mut meta_header,
                GENERATION_METADATA_FILE,
                meta_data.as_slice(),
            )
            .context("Failed to add generation metadata to tar")?;
    }

    let bytes = tar_builder
        .into_inner()
        .context("Failed to finalize tar archive")?;

    Ok(bytes)
}

/// Get the set of CAS hashes referenced by a specific generation.
///
/// Opens the database, looks up the generation's `system_state` by
/// `state_number`, and calls `live_cas_hashes()` to get only the
/// hashes referenced by that state.  Falls back to walking all CAS
/// objects if the database is unavailable or the generation has no
/// matching state (e.g., bootstrapped generation without a DB).
fn scope_cas_hashes_for_generation(
    db_path: &str,
    gen_number: i64,
    objects_dir: &Path,
) -> Result<HashSet<String>> {
    // Try to open the database and look up the generation's state
    let conn = match conary_core::db::open(db_path) {
        Ok(c) => c,
        Err(e) => {
            info!(
                "Database not available at {db_path} ({e}), \
                 falling back to full CAS walk for export"
            );
            return fallback_all_cas_hashes(objects_dir);
        }
    };

    let state = match SystemState::find_by_number(&conn, gen_number) {
        Ok(Some(s)) => s,
        Ok(None) => {
            info!(
                "No system state found for generation {gen_number}, \
                 falling back to full CAS walk for export"
            );
            return fallback_all_cas_hashes(objects_dir);
        }
        Err(e) => {
            info!(
                "Failed to query system state for generation {gen_number}: {e}, \
                 falling back to full CAS walk for export"
            );
            return fallback_all_cas_hashes(objects_dir);
        }
    };

    let state_id = state.id.ok_or_else(|| anyhow!("System state has no ID"))?;
    let hashes = live_cas_hashes(&conn, &[state_id])
        .with_context(|| format!("Failed to query live CAS hashes for generation {gen_number}"))?;

    Ok(hashes)
}

/// Fallback: collect all CAS object hashes when the database is not available.
fn fallback_all_cas_hashes(objects_dir: &Path) -> Result<HashSet<String>> {
    if !objects_dir.exists() {
        return Ok(HashSet::new());
    }

    let cas = CasStore::new(objects_dir)
        .with_context(|| format!("Failed to open CAS directory {}", objects_dir.display()))?;

    let hashes: HashSet<String> = cas
        .iter_objects()
        .map(|r| r.map(|(hash, _path)| hash))
        .collect::<std::result::Result<HashSet<_>, _>>()
        .with_context(|| "Failed to iterate CAS objects")?;

    Ok(hashes)
}

/// Write a blob to the blobs directory, returning (hex_digest, size).
fn write_blob(blobs_dir: &Path, data: &[u8]) -> Result<(String, u64)> {
    let digest = hex_digest(data);
    let path = blobs_dir.join(&digest);
    fs::write(&path, data).with_context(|| format!("Failed to write blob {digest}"))?;
    Ok((digest, data.len() as u64))
}

/// Compute the hex-encoded SHA-256 digest of the given data.
fn hex_digest(data: &[u8]) -> String {
    conary_core::hash::sha256(data)
}

/// Build the OCI image config JSON.
///
/// Follows the OCI Image Configuration spec:
///   https://github.com/opencontainers/image-spec/blob/main/config.md
fn build_config_json(metadata: &GenerationMetadata, gen_number: i64, diff_id: &str) -> String {
    let created = &metadata.created_at;
    let pkg_count = metadata.package_count;
    let kernel = metadata.kernel_version.as_deref().unwrap_or("unknown");
    let summary = &metadata.summary;

    // Build the config as a JSON string to avoid pulling in a builder crate.
    // This is intentionally simple -- no entrypoint or cmd since this is a
    // system image, not an application container.
    serde_json::json!({
        "created": created,
        "architecture": oci_arch(),
        "os": "linux",
        "config": {
            "Labels": {
                "io.conary.generation": gen_number.to_string(),
                "io.conary.package-count": pkg_count.to_string(),
                "io.conary.kernel": kernel,
                "io.conary.summary": summary,
                "io.conary.format": "composefs-native"
            }
        },
        "rootfs": {
            "type": "layers",
            "diff_ids": [
                format!("sha256:{diff_id}")
            ]
        },
        "history": [
            {
                "created": created,
                "comment": format!("Conary generation {gen_number}: {summary}")
            }
        ]
    })
    .to_string()
}

/// Build the OCI image manifest JSON.
fn build_manifest_json(
    config_digest: &str,
    config_size: u64,
    layer_digest: &str,
    layer_size: u64,
) -> String {
    serde_json::json!({
        "schemaVersion": 2,
        "mediaType": MANIFEST_MEDIA_TYPE,
        "config": {
            "mediaType": CONFIG_MEDIA_TYPE,
            "digest": format!("sha256:{config_digest}"),
            "size": config_size
        },
        "layers": [
            {
                "mediaType": LAYER_MEDIA_TYPE,
                "digest": format!("sha256:{layer_digest}"),
                "size": layer_size
            }
        ]
    })
    .to_string()
}

/// Build the OCI index JSON (top-level entry point).
fn build_index_json(manifest_digest: &str, manifest_size: u64) -> String {
    serde_json::json!({
        "schemaVersion": 2,
        "mediaType": INDEX_MEDIA_TYPE,
        "manifests": [
            {
                "mediaType": MANIFEST_MEDIA_TYPE,
                "digest": format!("sha256:{manifest_digest}"),
                "size": manifest_size,
                "platform": {
                    "architecture": oci_arch(),
                    "os": "linux"
                }
            }
        ]
    })
    .to_string()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::generation::metadata::GENERATION_FORMAT;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Create a minimal generation directory for testing.
    fn create_test_generation(tmp: &Path) -> (PathBuf, PathBuf) {
        // Generation directory with EROFS image and metadata
        let gen_dir = tmp.join("generation");
        fs::create_dir_all(&gen_dir).unwrap();

        // Write a fake EROFS image (just some bytes for testing)
        let erofs_data = b"EROFS-IMAGE-PLACEHOLDER-DATA-FOR-TESTING";
        fs::write(gen_dir.join(EROFS_IMAGE_NAME), erofs_data).unwrap();

        // Write generation metadata
        let metadata = GenerationMetadata {
            generation: 5,
            format: GENERATION_FORMAT.to_string(),
            erofs_size: Some(erofs_data.len() as i64),
            cas_objects_referenced: Some(2),
            fsverity_enabled: false,
            erofs_verity_digest: None,
            artifact_manifest_sha256: None,
            created_at: "2026-03-17T12:00:00Z".to_string(),
            package_count: 42,
            kernel_version: Some("6.12.1-conary".to_string()),
            summary: "test generation".to_string(),
        };
        metadata.write_to(&gen_dir).unwrap();

        // CAS objects directory
        let objects_dir = tmp.join("objects");
        fs::create_dir_all(objects_dir.join("ab")).unwrap();
        fs::create_dir_all(objects_dir.join("cd")).unwrap();
        fs::write(
            objects_dir
                .join("ab")
                .join("cdef0123456789abcdef0123456789abcdef0123456789abcdef01234567"),
            b"file-content-one",
        )
        .unwrap();
        fs::write(
            objects_dir
                .join("cd")
                .join("ef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"),
            b"file-content-two",
        )
        .unwrap();

        (gen_dir, objects_dir)
    }

    /// Helper: export from a test generation directory to OCI.
    fn export_test_generation(gen_dir: &Path, objects_dir: &Path) -> (TempDir, PathBuf) {
        let output_tmp = TempDir::new().unwrap();
        let output_dir = output_tmp.path().join("oci-out");
        fs::create_dir_all(&output_dir).unwrap();

        // Build layer, config, manifest, index manually since export_oci
        // expects real /conary paths. Use fallback_all_cas_hashes since
        // these tests don't have a database.
        let all_hashes = fallback_all_cas_hashes(objects_dir).unwrap();
        let metadata = GenerationMetadata::read_from(gen_dir).unwrap();
        let erofs_path = gen_dir.join(EROFS_IMAGE_NAME);
        let blobs_dir = output_dir.join("blobs/sha256");
        fs::create_dir_all(&blobs_dir).unwrap();

        let (layer_digest, layer_size, diff_id) =
            build_layer_tar_gz(&erofs_path, objects_dir, gen_dir, &blobs_dir, &all_hashes).unwrap();

        let config_json = build_config_json(&metadata, 5, &diff_id);
        let (config_digest, config_size) = write_blob(&blobs_dir, config_json.as_bytes()).unwrap();

        let manifest_json =
            build_manifest_json(&config_digest, config_size, &layer_digest, layer_size);
        let (manifest_digest, manifest_size) =
            write_blob(&blobs_dir, manifest_json.as_bytes()).unwrap();

        let index_json = build_index_json(&manifest_digest, manifest_size);
        fs::write(output_dir.join("index.json"), &index_json).unwrap();

        let oci_layout = r#"{"imageLayoutVersion":"1.0.0"}"#;
        fs::write(output_dir.join("oci-layout"), oci_layout).unwrap();

        (output_tmp, output_dir)
    }

    #[test]
    fn test_oci_layout_structure() {
        let tmp = TempDir::new().unwrap();
        let (gen_dir, objects_dir) = create_test_generation(tmp.path());
        let (_output_tmp, output_dir) = export_test_generation(&gen_dir, &objects_dir);

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
        let (gen_dir, objects_dir) = create_test_generation(tmp.path());
        let (_output_tmp, output_dir) = export_test_generation(&gen_dir, &objects_dir);

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
        let (gen_dir, objects_dir) = create_test_generation(tmp.path());
        let (_output_tmp, output_dir) = export_test_generation(&gen_dir, &objects_dir);

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
        let (gen_dir, objects_dir) = create_test_generation(tmp.path());
        let (_output_tmp, output_dir) = export_test_generation(&gen_dir, &objects_dir);

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
        let (gen_dir, objects_dir) = create_test_generation(tmp.path());
        let (_output_tmp, output_dir) = export_test_generation(&gen_dir, &objects_dir);

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
    fn test_fallback_cas_hashes_empty() {
        let tmp = TempDir::new().unwrap();
        let objects_dir = tmp.path().join("objects");
        // objects_dir does not exist
        let hashes = fallback_all_cas_hashes(&objects_dir).unwrap();
        assert!(hashes.is_empty());
    }

    #[test]
    fn test_fallback_cas_hashes_with_objects() {
        let tmp = TempDir::new().unwrap();
        let objects_dir = tmp.path().join("objects");
        fs::create_dir_all(objects_dir.join("ab")).unwrap();
        fs::create_dir_all(objects_dir.join("ff")).unwrap();
        fs::write(
            objects_dir
                .join("ab")
                .join("1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcd"),
            b"data",
        )
        .unwrap();
        fs::write(
            objects_dir
                .join("ff")
                .join("0000000000000000000000000000000000000000000000000000000000000000"),
            b"data2",
        )
        .unwrap();

        // Also add a temp file that should be skipped
        fs::write(objects_dir.join("ab").join(".tmp_in_progress"), b"skip").unwrap();

        let hashes = fallback_all_cas_hashes(&objects_dir).unwrap();
        assert_eq!(hashes.len(), 2);
        let sorted: Vec<&String> = {
            let mut v: Vec<&String> = hashes.iter().collect();
            v.sort();
            v
        };
        assert!(sorted[0].starts_with("ab"));
        assert!(sorted[1].starts_with("ff"));
    }

    #[test]
    fn test_scope_cas_hashes_falls_back_on_missing_db() {
        let tmp = TempDir::new().unwrap();
        let objects_dir = tmp.path().join("objects");
        fs::create_dir_all(objects_dir.join("ab")).unwrap();
        fs::write(
            objects_dir
                .join("ab")
                .join("cdef0123456789abcdef0123456789abcdef0123456789abcdef01234567"),
            b"data",
        )
        .unwrap();

        // Non-existent DB path -- should fall back to full CAS walk
        let hashes =
            scope_cas_hashes_for_generation("/nonexistent/conary.db", 1, &objects_dir).unwrap();
        assert_eq!(hashes.len(), 1);
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
}
