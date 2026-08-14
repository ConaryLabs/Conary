// apps/conary/src/commands/install/ccs_transaction/converted_directory_tests.rs

use super::super::InstallIntent;
use super::super::conversion::{CcsArtifactInstallOptions, install_ccs_artifact};
use conary_core::ccs::builder::write_signed_current_ccs_package;
use conary_core::ccs::native_lifecycle::{
    NATIVE_LIFECYCLE_SCHEMA_REVISION, NATIVE_LIFECYCLE_SCHEMA_V1, NativeLifecycleBundle,
    ScriptletFidelity, SourceFormat, VersionScheme as NativeVersionScheme,
};
use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry as CcsFileEntry};
use conary_core::db::models::{
    FileEntry as DbFileEntry, GenerationPublication, PayloadClaim, PayloadClaimAnchorPolicy, Trove,
    TroveType,
};
use conary_core::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
    ResolvedPayloadNode,
};
use conary_core::repository::dependency_model::DebianMultiArch;
use conary_core::repository::versioning::VersionScheme;
use conary_core::scriptlet::SandboxMode;
use std::collections::HashMap;

fn payload_node(kind: PayloadNodeKind, permissions: u32) -> PayloadNode {
    let mode_type = match kind {
        PayloadNodeKind::Directory => libc::S_IFDIR,
        PayloadNodeKind::Regular { .. } => libc::S_IFREG,
        _ => unreachable!("converted directory test uses only regular files and directories"),
    };
    PayloadNode {
        kind,
        mode: mode_type | permissions,
        user: PayloadIdentity::Numeric {
            id: u64::from(unsafe { libc::geteuid() }),
        },
        group: PayloadIdentity::Numeric {
            id: u64::from(unsafe { libc::getegid() }),
        },
        mtime: PayloadTimestamp::UNIX_EPOCH,
        xattrs: Default::default(),
    }
}

fn directory_node(permissions: u32) -> PayloadNode {
    payload_node(PayloadNodeKind::Directory, permissions)
}

fn native_free_bundle(
    format: SourceFormat,
    package_name: &str,
    version: &str,
) -> NativeLifecycleBundle {
    NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: format,
        source_family: format.as_str().to_string(),
        source_profile: None,
        source_release: None,
        source_arch: Some("x86_64".to_string()),
        source_package: package_name.to_string(),
        source_version: version.to_string(),
        source_checksum: Some(format!("sha256:{}", "1".repeat(64))),
        version_scheme: match format {
            SourceFormat::Rpm => NativeVersionScheme::Rpm,
            SourceFormat::Deb => NativeVersionScheme::Deb,
            SourceFormat::Arch => NativeVersionScheme::Arch,
            SourceFormat::Eopkg => NativeVersionScheme::Eopkg,
        },
        conversion_tool: "conary-test".to_string(),
        conversion_tool_version: env!("CARGO_PKG_VERSION").to_string(),
        conversion_policy: "typed-directory-source-abi-test".to_string(),
        evidence_digest: Some(format!("sha256:{}", "2".repeat(64))),
        scriptlet_fidelity: ScriptletFidelity::NativeFree,
        entries: Vec::new(),
    }
}

fn write_converted_directory_package(
    temp_dir: &std::path::Path,
    format: SourceFormat,
    version_scheme: VersionScheme,
) -> (String, std::path::PathBuf) {
    let name = format!("converted-{}-directory", format.as_str());
    let version = "1.0-1";
    let mut manifest = CcsManifest::new_minimal(&name, version);
    manifest.package.version_scheme = version_scheme;
    manifest.package.debian_multi_arch = match format {
        SourceFormat::Deb => Some(DebianMultiArch::No),
        SourceFormat::Rpm | SourceFormat::Arch | SourceFormat::Eopkg => None,
    };
    manifest.native_lifecycle = Some(native_free_bundle(format, &name, version));
    manifest.components.default = vec!["runtime".to_string()];

    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = conary_core::hash::sha256(&init_content);
    let files = vec![
        CcsFileEntry {
            path: "/shared".to_string(),
            node: directory_node(0o700),
            content: None,
            component: "runtime".to_string(),
            chunks: None,
        },
        CcsFileEntry {
            path: "/usr/sbin/init".to_string(),
            node: payload_node(
                PayloadNodeKind::Regular {
                    hardlink_identity: None,
                },
                0o755,
            ),
            content: Some(PayloadContentAuthority {
                sha256: init_hash.clone(),
                size: init_content.len() as u64,
            }),
            component: "runtime".to_string(),
            chunks: None,
        },
    ];
    let package_path = temp_dir.join(format!("{name}.ccs"));
    let result = BuildResult {
        manifest,
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: init_content.len() as u64,
            },
        )]),
        files: files.clone(),
        payloads: conary_core::ccs::builder::payloads_from_bounded_memory_for_tests(
            &files,
            HashMap::from([(init_hash, init_content)]),
        )
        .unwrap(),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    let signing_key = crate::commands::ccs::load_or_create_local_dev_key().unwrap();
    write_signed_current_ccs_package(&result, &package_path, &signing_key, true).unwrap();
    (name, package_path)
}

fn stage_test_boot_assets(root: &std::path::Path) {
    let kernel_version = "test-kernel";
    let boot_root = root.join("boot");
    std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
    std::fs::write(
        boot_root.join(format!("vmlinuz-{kernel_version}")),
        b"test-kernel",
    )
    .unwrap();
    std::fs::write(
        boot_root.join(format!("initramfs-{kernel_version}.img")),
        b"test-initramfs",
    )
    .unwrap();
    std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"test-efi").unwrap();
}

#[tokio::test]
async fn reloaded_converted_formats_install_with_their_directory_contract() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    for (format, version_scheme, expected_mode, expected_policy) in [
        (
            SourceFormat::Rpm,
            VersionScheme::Rpm,
            0o700,
            PayloadClaimAnchorPolicy::DirectoryOrSymlinkToDirectory,
        ),
        (
            SourceFormat::Deb,
            VersionScheme::Debian,
            0o755,
            PayloadClaimAnchorPolicy::DirectoryOrSymlinkToDirectory,
        ),
        (
            SourceFormat::Arch,
            VersionScheme::Arch,
            0o755,
            PayloadClaimAnchorPolicy::Directory,
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();
        let install_root = temp.path().join("install-root");
        std::fs::create_dir_all(&install_root).unwrap();
        stage_test_boot_assets(temp.path());
        conary_core::db::init(db_path_str).unwrap();

        let conn = conary_core::db::open(db_path_str).unwrap();
        conary_core::ccs::HostCapabilityInventory::discover()
            .unwrap()
            .persist(&conn)
            .unwrap();
        let mut anchor_trove = Trove::new(
            format!("{}-anchor", format.as_str()),
            "1.0.0".to_string(),
            TroveType::Package,
            VersionScheme::Conary,
        );
        let anchor_trove_id = anchor_trove.insert(&conn).unwrap();
        let existing_node =
            ResolvedPayloadNode::from_numeric_source(directory_node(0o755)).unwrap();
        let mut existing =
            DbFileEntry::new("/shared".to_string(), existing_node, None, anchor_trove_id);
        existing.insert(&conn).unwrap();
        drop(conn);

        let (package_name, package_path) =
            write_converted_directory_package(temp.path(), format, version_scheme);
        install_ccs_artifact(CcsArtifactInstallOptions {
            ccs_path: package_path.to_str().unwrap(),
            db_path: db_path_str,
            root: install_root.to_str().unwrap(),
            dry_run: false,
            sandbox_mode: SandboxMode::Always,
            no_deps: true,
            allow_downgrade: false,
            intent: InstallIntent::PackageChange,
            yes: true,
            envelope_authority: super::super::CcsEnvelopeAuthority::LocalDev,
            repository_provenance: None,
            resolution_policy: conary_core::repository::resolution_policy::ResolutionPolicy::new()
                .with_mixing(
                    conary_core::repository::resolution_policy::DependencyMixingPolicy::Permissive,
                ),
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "install reloaded converted {} CCS: {error:#}",
                format.as_str()
            )
        });

        let conn = conary_core::db::open(db_path_str).unwrap();
        let installed = Trove::find_by_name(&conn, &package_name)
            .unwrap()
            .into_iter()
            .next()
            .expect("converted package trove");
        let installed_id = installed.id.unwrap();
        let materialized = DbFileEntry::find_by_path(&conn, "/shared")
            .unwrap()
            .unwrap();
        assert_eq!(materialized.trove_id, anchor_trove_id);
        assert_eq!(materialized.node.source.mode, libc::S_IFDIR | expected_mode);
        let claim = PayloadClaim::find_by_path(&conn, "/shared")
            .unwrap()
            .into_iter()
            .find(|claim| claim.trove_id == installed_id)
            .expect("converted package directory claim");
        assert_eq!(claim.node.source.mode, libc::S_IFDIR | 0o700);
        assert_eq!(claim.anchor_policy, expected_policy);

        let publication = GenerationPublication::pending_recoverable(&conn)
            .unwrap()
            .pop()
            .expect("converted install publication snapshot");
        let snapshot = crate::commands::generation::selected_root::load_publication_selected_root(
            &conn,
            &publication,
        )
        .unwrap();
        let selected_node = snapshot
            .generation
            .entries
            .iter()
            .chain(&snapshot.state.entries)
            .find(|entry| entry.path == "/shared")
            .expect("published selected root shared directory");
        assert_eq!(
            selected_node.node.source.mode,
            libc::S_IFDIR | expected_mode
        );
    }
}
