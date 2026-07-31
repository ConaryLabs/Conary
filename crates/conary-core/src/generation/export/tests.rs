// conary-core/src/generation/export/tests.rs

use super::*;
use crate::generation::artifact::{
    ArtifactWriteInputs, BootAssetsManifest, CasObjectRef, write_generation_artifact,
};
use crate::generation::metadata::{GENERATION_FORMAT, GenerationMetadata};
use crate::generation::root_manifest::{
    GENERATION_ROOT_MANIFEST_VERSION, GenerationRootEntry, GenerationRootManifest,
    MutableStateManifest,
};
use crate::generation::test_support::write_root_manifests_with_objects;
use crate::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
    ResolvedPayloadNode,
};
use sha2::{Digest, Sha256};
use std::os::unix::fs::MetadataExt;
use tempfile::TempDir;

const TEST_METADATA_XATTR: &str = "user.conary-export-metadata";
const SOURCE_FSTAB: &[u8] =
    b"UUID=source-root / btrfs defaults,subvol=root 0 1\nUUID=source-root /var btrfs defaults,subvol=var 0 0\n";

struct Fixture {
    _tmp: TempDir,
    generation_dir: PathBuf,
    objects_dir: PathBuf,
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(unix)]
fn write_script(path: &Path, content: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, content).unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[cfg(unix)]
fn fake_tools(dir: &Path) -> GenerationExportTools {
    let repart = dir.join("systemd-repart");
    let qemu_img = dir.join("qemu-img");
    let xorriso = dir.join("xorriso");
    let mkfs_vfat = dir.join("mkfs.vfat");
    let mmd = dir.join("mmd");
    let mcopy = dir.join("mcopy");
    let repart_log = dir.join("repart.log");
    let qemu_log = dir.join("qemu.log");
    let xorriso_log = dir.join("xorriso.log");
    let mkfs_vfat_log = dir.join("mkfs-vfat.log");
    let mmd_log = dir.join("mmd.log");
    let mcopy_log = dir.join("mcopy.log");
    write_script(
        &repart,
        &format!(
            "#!/bin/sh\nlast=''\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\" >> '{}'; last=\"$arg\"; done\nprintf raw > \"$last\"\n",
            repart_log.display()
        ),
    );
    write_script(
        &qemu_img,
        &format!(
            "#!/bin/sh\nprev=''\nlast=''\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\" >> '{}'; prev=\"$last\"; last=\"$arg\"; done\nprintf qcow2 > \"$last\"\n",
            qemu_log.display()
        ),
    );
    write_script(
        &xorriso,
        &format!(
            "#!/bin/sh\nout=''\nprev=''\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\" >> '{}'; if [ \"$prev\" = '-o' ]; then out=\"$arg\"; fi; prev=\"$arg\"; done\nprintf iso > \"$out\"\n",
            xorriso_log.display()
        ),
    );
    write_script(
        &mkfs_vfat,
        &format!(
            "#!/bin/sh\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\" >> '{}'; done\n",
            mkfs_vfat_log.display()
        ),
    );
    write_script(
        &mmd,
        &format!(
            "#!/bin/sh\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\" >> '{}'; done\n",
            mmd_log.display()
        ),
    );
    write_script(
        &mcopy,
        &format!(
            "#!/bin/sh\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\" >> '{}'; done\n",
            mcopy_log.display()
        ),
    );
    GenerationExportTools {
        systemd_repart: repart,
        qemu_img,
        xorriso,
        mkfs_vfat,
        mmd,
        mcopy,
    }
}

fn write_cas_object(objects_dir: &Path, bytes: &[u8]) -> CasObjectRef {
    let sha256 = digest(bytes);
    let object_path = crate::filesystem::object_path(objects_dir, &sha256).unwrap();
    std::fs::create_dir_all(object_path.parent().unwrap()).unwrap();
    std::fs::write(object_path, bytes).unwrap();
    CasObjectRef {
        sha256,
        size: bytes.len() as u64,
    }
}

impl Fixture {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let artifact_root = tmp.path().join("artifact");
        let generation_dir = artifact_root.join("generations/7");
        let objects_dir = artifact_root.join("objects");
        let boot_assets_dir = generation_dir.join(BOOT_ASSETS_DIR);
        std::fs::create_dir_all(boot_assets_dir.join("EFI/BOOT")).unwrap();
        std::fs::create_dir_all(&objects_dir).unwrap();
        std::fs::write(generation_dir.join("root.erofs"), b"root-erofs").unwrap();
        std::fs::write(boot_assets_dir.join("vmlinuz"), b"kernel").unwrap();
        std::fs::write(boot_assets_dir.join("initramfs.img"), b"initramfs").unwrap();
        std::fs::write(boot_assets_dir.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();

        let cas_object = write_cas_object(&objects_dir, b"hello");
        write_root_manifests_with_objects(&generation_dir, std::slice::from_ref(&cas_object));
        let mut root_manifest = GenerationRootManifest::read_from(&generation_dir).unwrap();
        root_manifest.root = directory_node(0o751, 1, b"root");
        root_manifest.entries.extend([
            GenerationRootEntry {
                path: "/boot".to_string(),
                node: directory_node(0o752, 2, b"boot"),
                content: None,
            },
            GenerationRootEntry {
                path: "/usr".to_string(),
                node: directory_node(0o753, 3, b"usr"),
                content: None,
            },
        ]);
        root_manifest
            .entries
            .extend(
                ROOT_SYMLINKS
                    .iter()
                    .enumerate()
                    .map(|(index, (path, target))| {
                        root_symlink_entry(
                            &format!("/{path}"),
                            target,
                            u32::try_from(index).unwrap() + 10,
                        )
                    }),
            );
        root_manifest
            .entries
            .sort_by(|left, right| left.path.cmp(&right.path));
        root_manifest.write_to(&generation_dir).unwrap();

        let state_object = write_cas_object(&objects_dir, b"captured mutable state");
        let fstab_object = write_cas_object(&objects_dir, SOURCE_FSTAB);
        MutableStateManifest {
            version: GENERATION_ROOT_MANIFEST_VERSION,
            entries: vec![
                GenerationRootEntry {
                    path: "/etc".to_string(),
                    node: directory_node(0o754, 4, b"etc"),
                    content: None,
                },
                state_regular("/etc/conary-export.conf", &state_object),
                state_regular("/etc/fstab", &fstab_object),
                state_directory("/srv"),
                state_regular("/srv/conary-export-state", &state_object),
                state_directory("/var"),
                state_directory("/var/lib"),
                state_regular("/var/lib/conary-export-state", &state_object),
            ],
        }
        .write_to(&generation_dir)
        .unwrap();
        let boot_assets = BootAssetsManifest {
            version: 1,
            generation: 7,
            architecture: "x86_64".to_string(),
            kernel_version: "6.19.8-conary".to_string(),
            kernel: "vmlinuz".to_string(),
            kernel_sha256: digest(b"kernel"),
            initramfs: "initramfs.img".to_string(),
            initramfs_sha256: digest(b"initramfs"),
            efi_bootloader: "EFI/BOOT/BOOTX64.EFI".to_string(),
            efi_bootloader_sha256: digest(b"efi"),
            created_at: "2026-04-22T00:00:00Z".to_string(),
        };
        let artifact_digest = write_generation_artifact(ArtifactWriteInputs {
            generation_dir: &generation_dir,
            generation: 7,
            architecture: "x86_64",
            erofs_path: &generation_dir.join("root.erofs"),
            cas_base_rel: "../../objects",
            cas_verification: crate::generation::artifact::CasObjectVerification::Deep,
            boot_assets,
            carrier_capabilities: Default::default(),
        })
        .unwrap();
        GenerationMetadata {
            generation: 7,
            format: GENERATION_FORMAT.to_string(),
            erofs_size: Some(10),
            cas_objects_referenced: Some(3),
            fsverity_enabled: false,
            erofs_verity_digest: None,
            artifact_manifest_sha256: Some(artifact_digest),
            security_capability_xattr_count: None,
            created_at: "2026-04-22T00:00:00Z".to_string(),
            package_count: 1,
            kernel_version: Some("6.19.8-conary".to_string()),
            summary: "fixture".to_string(),
        }
        .write_to(&generation_dir)
        .unwrap();

        Self {
            _tmp: tmp,
            generation_dir,
            objects_dir,
        }
    }

    fn artifact(&self) -> GenerationArtifact {
        crate::generation::artifact::load_generation_artifact(&self.generation_dir).unwrap()
    }
}

fn state_directory(path: &str) -> GenerationRootEntry {
    GenerationRootEntry {
        path: path.to_string(),
        node: directory_node(0o755, 5, path.as_bytes()),
        content: None,
    }
}

fn root_symlink_entry(
    path: &str,
    target: &str,
    timestamp_discriminator: u32,
) -> GenerationRootEntry {
    let mut node = PayloadNode::regular(0o777);
    node.kind = PayloadNodeKind::Symlink {
        target: target.to_string(),
    };
    node.mode = libc::S_IFLNK | 0o777;
    node.mtime = PayloadTimestamp {
        seconds: 1_700_000_000 + i64::from(timestamp_discriminator),
        nanoseconds: timestamp_discriminator,
    };
    set_test_process_ownership(&mut node);
    GenerationRootEntry {
        path: path.to_string(),
        node: ResolvedPayloadNode::from_numeric_source(node).unwrap(),
        content: None,
    }
}

fn directory_node(
    permissions: u32,
    timestamp_discriminator: u32,
    marker: &[u8],
) -> ResolvedPayloadNode {
    let mut node = PayloadNode::regular(permissions);
    node.kind = PayloadNodeKind::Directory;
    node.mode = libc::S_IFDIR | permissions;
    node.mtime = PayloadTimestamp {
        seconds: 1_700_000_000 + i64::from(timestamp_discriminator),
        nanoseconds: timestamp_discriminator,
    };
    node.xattrs
        .insert(TEST_METADATA_XATTR.to_string(), marker.to_vec());
    set_test_process_ownership(&mut node);
    ResolvedPayloadNode::from_numeric_source(node).unwrap()
}

fn state_regular(path: &str, object: &CasObjectRef) -> GenerationRootEntry {
    let mut node = PayloadNode::regular(0o640);
    set_test_process_ownership(&mut node);
    GenerationRootEntry {
        path: path.to_string(),
        node: ResolvedPayloadNode::from_numeric_source(node).unwrap(),
        content: Some(PayloadContentAuthority {
            sha256: object.sha256.clone(),
            size: object.size,
        }),
    }
}

fn set_test_process_ownership(node: &mut PayloadNode) {
    node.user = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::geteuid() }),
    };
    node.group = PayloadIdentity::Numeric {
        id: u64::from(unsafe { libc::getegid() }),
    };
}

#[test]
fn rootfs_projection_creates_runtime_tree() {
    let fixture = Fixture::new();
    let artifact = fixture.artifact();
    let staging = fixture._tmp.path().join("rootfs");

    project_generation_rootfs(&artifact, &staging).unwrap();

    let gen_dir = staging.join("conary/generations/7");
    assert!(gen_dir.join("root.erofs").is_file());
    assert!(gen_dir.join(".conary-gen.json").is_file());
    assert!(gen_dir.join(".conary-artifact.json").is_file());
    assert!(gen_dir.join("cas-manifest.json").is_file());
    assert!(gen_dir.join("generation-root.json").is_file());
    assert!(gen_dir.join("mutable-state.json").is_file());
    assert!(gen_dir.join("boot-assets/manifest.json").is_file());
    assert_eq!(
        std::fs::read(staging.join("conary/etc-state/7/conary-export.conf")).unwrap(),
        b"captured mutable state"
    );
    assert!(!staging.join("conary/etc-state/7/etc").exists());
    assert_eq!(
        std::fs::read(staging.join("var/lib/conary-export-state")).unwrap(),
        b"captured mutable state"
    );
    assert_eq!(
        std::fs::read(staging.join("srv/conary-export-state")).unwrap(),
        b"captured mutable state"
    );
    assert!(staging.join("conary/etc-lower").is_dir());
    assert!(staging.join("conary/mnt").is_dir());
    assert!(staging.join("usr").is_dir());
    assert!(staging.join("etc").is_dir());
    assert!(staging.join("boot").is_dir());
    assert_eq!(
        std::fs::read(staging.join("conary/etc-state/7/fstab")).unwrap(),
        SOURCE_FSTAB
    );
    let machine_id = staging.join("conary/etc-state/7/machine-id");
    assert!(machine_id.is_file());
    assert!(std::fs::read(machine_id).unwrap().is_empty());
    assert_projected_metadata(&staging, &artifact.generation_root.root);
    assert_projected_metadata(
        &staging.join("usr"),
        carrier_mountpoint_node(&artifact, "/usr").unwrap(),
    );
    assert_projected_metadata(
        &staging.join("boot"),
        carrier_mountpoint_node(&artifact, "/boot").unwrap(),
    );
    assert_projected_metadata(
        &staging.join("etc"),
        carrier_mountpoint_node(&artifact, "/etc").unwrap(),
    );
    assert_projected_metadata(
        &staging.join("conary/etc-state/7"),
        carrier_mountpoint_node(&artifact, "/etc").unwrap(),
    );
}

#[test]
fn rootfs_projection_preserves_state_owned_machine_id() {
    let fixture = Fixture::new();
    let machine_id = write_cas_object(&fixture.objects_dir, b"0123456789abcdef0123456789abcdef\n");
    let mut artifact = fixture.artifact();
    artifact
        .mutable_state
        .entries
        .push(state_regular("/etc/machine-id", &machine_id));
    artifact
        .mutable_state
        .entries
        .sort_by(|left, right| left.path.cmp(&right.path));
    artifact.cas_objects.push(machine_id);
    let staging = fixture._tmp.path().join("rootfs-machine-id");

    project_generation_rootfs(&artifact, &staging).unwrap();

    assert_eq!(
        std::fs::read(staging.join("conary/etc-state/7/machine-id")).unwrap(),
        b"0123456789abcdef0123456789abcdef\n"
    );
}

fn assert_projected_metadata(path: &Path, expected: &ResolvedPayloadNode) {
    let metadata = std::fs::symlink_metadata(path).unwrap();
    assert_eq!(metadata.mode(), expected.source.mode);
    assert_eq!(u64::from(metadata.uid()), expected.uid);
    assert_eq!(u64::from(metadata.gid()), expected.gid);
    assert_eq!(metadata.mtime(), expected.source.mtime.seconds);
    assert_eq!(
        u32::try_from(metadata.mtime_nsec()).unwrap(),
        expected.source.mtime.nanoseconds
    );
    assert_eq!(
        xattr::get(path, TEST_METADATA_XATTR).unwrap().as_deref(),
        expected
            .source
            .xattrs
            .get(TEST_METADATA_XATTR)
            .map(Vec::as_slice)
    );
}

#[cfg(unix)]
#[test]
fn rootfs_projection_creates_current_and_usr_merge_symlinks() {
    let fixture = Fixture::new();
    let artifact = fixture.artifact();
    let staging = fixture._tmp.path().join("rootfs-links");

    project_generation_rootfs(&artifact, &staging).unwrap();

    assert_eq!(
        std::fs::read_link(staging.join("conary/current")).unwrap(),
        PathBuf::from("generations/7")
    );
    for (link, target) in ROOT_SYMLINKS {
        assert_eq!(
            std::fs::read_link(staging.join(link)).unwrap(),
            PathBuf::from(target)
        );
        let expected = carrier_root_symlink_node(&artifact, &format!("/{link}"), target).unwrap();
        let metadata = std::fs::symlink_metadata(staging.join(link)).unwrap();
        assert_eq!(metadata.mode(), expected.source.mode);
        assert_eq!(u64::from(metadata.uid()), expected.uid);
        assert_eq!(u64::from(metadata.gid()), expected.gid);
        assert_eq!(metadata.mtime(), expected.source.mtime.seconds);
        assert_eq!(
            u32::try_from(metadata.mtime_nsec()).unwrap(),
            expected.source.mtime.nanoseconds
        );
    }
}

#[test]
fn rootfs_projection_applies_authenticated_immutable_backing_security() {
    let fixture = Fixture::new();
    let mut artifact = fixture.artifact();
    let capability = crate::ccs::ImmutableBackingSecurity {
        mechanism: crate::ccs::ImmutableBackingSecurityMechanism::Selinux,
        xattr_value: b"system_u:object_r:usr_t:s0\0".to_vec(),
    };
    artifact
        .artifact_manifest
        .carrier_capabilities
        .immutable_backing_security = Some(capability.clone());
    let staging = fixture._tmp.path().join("rootfs-security");
    let mut observed = None;

    project_generation_rootfs_with_security_apply(&artifact, &staging, |actual, objects_dest| {
        observed = Some((actual.clone(), objects_dest.to_path_buf()));
        Ok(())
    })
    .unwrap();

    assert_eq!(observed, Some((capability, staging.join("conary/objects"))));
}

#[test]
fn rootfs_projection_copies_only_manifest_listed_cas_objects() {
    let fixture = Fixture::new();
    let extra = write_cas_object(&fixture.objects_dir, b"extra");
    let artifact = fixture.artifact();
    let staging = fixture._tmp.path().join("rootfs-cas");

    project_generation_rootfs(&artifact, &staging).unwrap();

    let objects = staging.join("conary/objects");
    for object in &artifact.cas_objects {
        assert!(
            crate::filesystem::object_path(&objects, &object.sha256)
                .unwrap()
                .is_file()
        );
    }
    assert!(
        !crate::filesystem::object_path(&objects, &extra.sha256)
            .unwrap()
            .exists()
    );
}

#[test]
fn rootfs_projection_rejects_missing_mountpoint_metadata_before_staging() {
    let fixture = Fixture::new();
    let mut artifact = fixture.artifact();
    artifact
        .generation_root
        .entries
        .retain(|entry| entry.path != "/boot");
    let staging = fixture._tmp.path().join("rootfs-missing-boot-authority");

    let error = project_generation_rootfs(&artifact, &staging).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("missing exact carrier mountpoint metadata for /boot")
    );
    assert!(!staging.exists());
}

#[test]
fn rootfs_projection_rejects_missing_root_symlink_metadata_before_staging() {
    let fixture = Fixture::new();
    let mut artifact = fixture.artifact();
    artifact
        .generation_root
        .entries
        .retain(|entry| entry.path != "/lib64");
    let staging = fixture._tmp.path().join("rootfs-missing-lib64-authority");

    let error = project_generation_rootfs(&artifact, &staging).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("missing exact carrier root-symlink metadata for /lib64")
    );
    assert!(!staging.exists());
}

#[test]
fn esp_projection_writes_systemd_boot_contract() {
    let fixture = Fixture::new();
    let artifact = fixture.artifact();
    let staging = fixture._tmp.path().join("esp");

    project_generation_esp(&artifact, &staging).unwrap();

    assert!(staging.join("EFI/BOOT/BOOTX64.EFI").is_file());
    assert!(staging.join("vmlinuz").is_file());
    assert!(staging.join("initramfs.img").is_file());

    let loader_conf = std::fs::read_to_string(staging.join("loader/loader.conf")).unwrap();
    assert!(loader_conf.contains("default conary-gen-7"));
    assert!(loader_conf.contains("timeout 3"));
    assert!(loader_conf.contains("console-mode max"));
    assert!(loader_conf.contains("editor no"));

    let bls = std::fs::read_to_string(staging.join("loader/entries/conary-gen-7.conf")).unwrap();
    assert!(bls.contains("root=PARTLABEL=CONARY_ROOT"));
    assert!(bls.contains("rootfstype=ext4"));
    assert!(bls.contains(" rw "));
    assert!(bls.contains(" fstab=no "));
    assert!(bls.contains(" systemd.gpt_auto=0 "));
    assert!(bls.contains("systemd.mount-extra=PARTLABEL=CONARY_ESP:/boot:vfat:defaults,noatime"));
    assert!(bls.contains("conary.generation=7"));
    assert!(bls.contains("console=tty0"));
    assert!(bls.contains("console=ttyS0"));
    assert!(bls.contains("sort-key   conary-7"));
}

#[test]
fn iso_esp_projection_writes_readonly_carrier_boot_contract() {
    let fixture = Fixture::new();
    let artifact = fixture.artifact();
    let staging = fixture._tmp.path().join("iso-esp");

    project_generation_iso_esp(&artifact, &staging).unwrap();

    assert!(staging.join("EFI/BOOT/BOOTX64.EFI").is_file());
    assert!(staging.join("vmlinuz").is_file());
    assert!(staging.join("initramfs.img").is_file());

    let bls = std::fs::read_to_string(staging.join("loader/entries/conary-gen-7.conf")).unwrap();
    assert!(bls.contains("root=LABEL=CONARY_ISO"));
    assert!(bls.contains("rootfstype=iso9660"));
    assert!(bls.contains(" ro "));
    assert!(bls.contains(" fstab=no "));
    assert!(!bls.contains("systemd.gpt_auto=0"));
    assert!(!bls.contains("systemd.mount-extra="));
    assert!(bls.contains("conary.generation=7"));
    assert!(bls.contains("conary.carrier=readonly"));
    assert!(bls.contains("systemd.mask=boot.mount"));
    assert!(bls.contains("console=tty0"));
    assert!(bls.contains("console=ttyS0"));
}

#[test]
fn esp_projection_rejects_unsupported_architectures() {
    let fixture = Fixture::new();
    let mut artifact = fixture.artifact();
    artifact.artifact_manifest.architecture = "aarch64".to_string();
    let staging = fixture._tmp.path().join("esp-unsupported");

    let err = project_generation_esp(&artifact, &staging).unwrap_err();

    assert!(err.to_string().contains("only supports x86_64"));
    assert!(!staging.join("EFI").exists());
}

#[test]
fn export_format_parsing_reports_allowed_values() {
    let err = GenerationExportFormat::from_str("vmdk").unwrap_err();
    assert!(err.to_string().contains("raw, qcow2, or iso"));
    assert_eq!(
        GenerationExportFormat::from_str("raw").unwrap(),
        GenerationExportFormat::Raw
    );
    assert_eq!(
        GenerationExportFormat::from_str("qcow2").unwrap(),
        GenerationExportFormat::Qcow2
    );
    assert_eq!(
        GenerationExportFormat::from_str("iso").unwrap(),
        GenerationExportFormat::Iso
    );
}

#[cfg(unix)]
#[test]
fn iso_export_writes_bootable_generation_carrier() {
    let fixture = Fixture::new();
    let tools = fake_tools(fixture._tmp.path());
    let output = fixture._tmp.path().join("gen.iso");

    let result = export_generation_image_with_tools(
        GenerationExportOptions {
            generation: None,
            generation_path: Some(fixture.generation_dir.clone()),
            format: GenerationExportFormat::Iso,
            output: output.clone(),
            size_bytes: None,
        },
        &tools,
    )
    .unwrap();

    assert_eq!(result.path, output);
    assert_eq!(result.format, GenerationExportFormat::Iso);
    assert_eq!(result.size, 3);
    assert!(output.is_file());

    let manifest_path = output.with_extension("iso.conary-provenance.json");
    assert_eq!(
        result.provenance_path.as_deref(),
        Some(manifest_path.as_path())
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["format"], "iso");
    assert_eq!(manifest["output"]["sha256"], crate::hash::sha256(b"iso"));

    let xorriso_log = std::fs::read_to_string(fixture._tmp.path().join("xorriso.log")).unwrap();
    assert!(xorriso_log.contains("-V\nCONARY_ISO"));
    assert!(xorriso_log.contains("-e\nEFI/efiboot.img"));
    assert!(xorriso_log.contains(&output.display().to_string()));
}

#[test]
fn minimum_size_includes_fixed_overhead_and_margin() {
    let fixture = Fixture::new();
    let artifact = fixture.artifact();
    let staging = fixture._tmp.path().join("minimum-rootfs");
    project_generation_rootfs(&artifact, &staging).unwrap();

    let minimum = minimum_image_size_bytes(&staging).unwrap();

    assert!(minimum >= (ESP_SIZE_MB * 1024 * 1024) + GPT_OVERHEAD_BYTES + IMAGE_SIZE_MARGIN_BYTES);
    assert!(minimum > dir_size(&staging).unwrap());
}

#[test]
fn minimum_size_scales_for_ext4_minimize_headroom() {
    let tmp = TempDir::new().unwrap();
    let rootfs = tmp.path().join("large-rootfs");
    std::fs::create_dir_all(&rootfs).unwrap();
    let large_file = std::fs::File::create(rootfs.join("large-cas-object")).unwrap();
    large_file.set_len(7 * 1024 * 1024 * 1024).unwrap();

    let minimum = minimum_image_size_bytes(&rootfs).unwrap();

    assert!(
        minimum >= 11 * 1024 * 1024 * 1024,
        "7GiB rootfs should default to an image large enough for ext4 metadata; got {minimum}"
    );
}

#[cfg(unix)]
#[test]
fn undersized_export_reports_requested_and_minimum_sizes() {
    let fixture = Fixture::new();
    let tools = fake_tools(fixture._tmp.path());
    let output = fixture._tmp.path().join("undersized.raw");

    let err = export_generation_image_with_tools(
        GenerationExportOptions {
            generation: None,
            generation_path: Some(fixture.generation_dir.clone()),
            format: GenerationExportFormat::Raw,
            output,
            size_bytes: Some(1),
        },
        &tools,
    )
    .unwrap_err();

    assert!(err.to_string().contains("requested image size 1 bytes"));
    assert!(err.to_string().contains("minimum"));
}

#[cfg(unix)]
#[test]
fn raw_export_calls_shared_repart_backend_and_cleans_staging() {
    let fixture = Fixture::new();
    let tools = fake_tools(fixture._tmp.path());
    let output = fixture._tmp.path().join("gen.raw");

    let result = export_generation_image_with_tools(
        GenerationExportOptions {
            generation: None,
            generation_path: Some(fixture.generation_dir.clone()),
            format: GenerationExportFormat::Raw,
            output: output.clone(),
            size_bytes: Some(1024 * 1024 * 1024),
        },
        &tools,
    )
    .unwrap();

    assert_eq!(result.path, output);
    assert_eq!(result.format, GenerationExportFormat::Raw);
    assert!(result.size > 0);
    assert!(output.is_file());
    let repart_log = std::fs::read_to_string(fixture._tmp.path().join("repart.log")).unwrap();
    assert!(repart_log.contains("--root=/"));
    let output_path = output.to_string_lossy().into_owned();
    assert!(repart_log.contains(&output_path));
    assert!(
        !std::fs::read_dir(fixture._tmp.path())
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".conary-generation-export-"))
    );
}

#[cfg(unix)]
#[test]
fn raw_export_writes_output_provenance_manifest() {
    let fixture = Fixture::new();
    let tools = fake_tools(fixture._tmp.path());
    let output = fixture._tmp.path().join("gen.raw");

    let result = export_generation_image_with_tools(
        GenerationExportOptions {
            generation: None,
            generation_path: Some(fixture.generation_dir.clone()),
            format: GenerationExportFormat::Raw,
            output: output.clone(),
            size_bytes: Some(1024 * 1024 * 1024),
        },
        &tools,
    )
    .unwrap();

    let manifest_path = output.with_extension("raw.conary-provenance.json");
    assert_eq!(
        result.provenance_path.as_deref(),
        Some(manifest_path.as_path())
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["version"], 1);
    assert_eq!(manifest["generation"], 7);
    assert_eq!(manifest["architecture"], "x86_64");
    assert_eq!(manifest["format"], "raw");
    assert_eq!(manifest["output"]["path"], output.display().to_string());
    assert_eq!(manifest["output"]["size"], 3);
    assert_eq!(manifest["output"]["sha256"], crate::hash::sha256(b"raw"));
    assert_eq!(
        manifest["source"]["artifact_manifest_sha256"],
        fixture
            .artifact()
            .metadata
            .artifact_manifest_sha256
            .unwrap()
    );
}

#[cfg(unix)]
#[test]
fn raw_export_passes_4k_aligned_size_to_repart() {
    let fixture = Fixture::new();
    let tools = fake_tools(fixture._tmp.path());
    let output = fixture._tmp.path().join("aligned.raw");

    export_generation_image_with_tools(
        GenerationExportOptions {
            generation: None,
            generation_path: Some(fixture.generation_dir.clone()),
            format: GenerationExportFormat::Raw,
            output,
            size_bytes: Some(1024 * 1024 * 1024 + 1),
        },
        &tools,
    )
    .unwrap();

    let repart_log = std::fs::read_to_string(fixture._tmp.path().join("repart.log")).unwrap();
    assert!(repart_log.lines().any(|line| line == "--size=1073745920"));
}

#[cfg(unix)]
#[test]
fn qcow2_export_converts_raw_and_removes_temp_raw() {
    let fixture = Fixture::new();
    let tools = fake_tools(fixture._tmp.path());
    let output = fixture._tmp.path().join("gen.qcow2");
    let raw_tmp = raw_temp_path(&output);

    let result = export_generation_image_with_tools(
        GenerationExportOptions {
            generation: None,
            generation_path: Some(fixture.generation_dir.clone()),
            format: GenerationExportFormat::Qcow2,
            output: output.clone(),
            size_bytes: Some(1024 * 1024 * 1024),
        },
        &tools,
    )
    .unwrap();

    assert_eq!(result.path, output);
    assert_eq!(result.format, GenerationExportFormat::Qcow2);
    assert!(output.is_file());
    assert!(!raw_tmp.exists());
    assert!(!output_provenance_path(&raw_tmp, GenerationExportFormat::Raw).exists());
    let manifest_path = output.with_extension("qcow2.conary-provenance.json");
    assert_eq!(
        result.provenance_path.as_deref(),
        Some(manifest_path.as_path())
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["version"], 1);
    assert_eq!(manifest["generation"], 7);
    assert_eq!(manifest["architecture"], "x86_64");
    assert_eq!(manifest["format"], "qcow2");
    assert_eq!(manifest["output"]["path"], output.display().to_string());
    assert_eq!(manifest["output"]["size"], 5);
    assert_eq!(manifest["output"]["sha256"], crate::hash::sha256(b"qcow2"));
    let qemu_log = std::fs::read_to_string(fixture._tmp.path().join("qemu.log")).unwrap();
    assert!(qemu_log.contains("convert"));
    assert!(qemu_log.contains("-O"));
    assert!(qemu_log.contains("qcow2"));
}
