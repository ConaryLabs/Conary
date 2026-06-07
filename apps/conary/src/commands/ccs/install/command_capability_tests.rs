// src/commands/ccs/install/command_capability_tests.rs

use std::collections::HashMap;

use super::command::cmd_ccs_install;
use super::test_support::{ccs_init_file, stage_test_boot_assets};

#[tokio::test]
async fn ccs_install_persists_capability_declarations() {
    use conary_core::capability::{
        CapabilityDeclaration, FilesystemCapabilities, NetworkCapabilities, SyscallCapabilities,
        load_capabilities_by_name,
    };
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::hash;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("declared-capabilities.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let content = b"declared capabilities".to_vec();
    let file_hash = hash::sha256(&content);
    let init_content = b"#!/bin/sh\nexec true\n".to_vec();
    let init_hash = hash::sha256(&init_content);
    let total_size = (content.len() + init_content.len()) as u64;
    let files = vec![
        FileEntry {
            path: "/usr/bin/cap-decl".to_string(),
            hash: file_hash.clone(),
            size: content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
        FileEntry {
            path: "/usr/sbin/init".to_string(),
            hash: init_hash.clone(),
            size: init_content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
    ];
    let mut manifest = CcsManifest::new_minimal("declared-capabilities", "1.0.0");
    manifest.capabilities = Some(CapabilityDeclaration {
        version: 1,
        rationale: Some("needs outbound TLS and read access".to_string()),
        network: NetworkCapabilities {
            outbound: vec!["443".to_string()],
            listen: Vec::new(),
            none: false,
        },
        filesystem: FilesystemCapabilities {
            read: vec!["/etc/ssl/certs".to_string()],
            write: Vec::new(),
            execute: vec!["/usr/bin".to_string()],
            deny: Vec::new(),
        },
        syscalls: SyscallCapabilities::default(),
    });
    let result = BuildResult {
        manifest,
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: total_size,
            },
        )]),
        files,
        blobs: HashMap::from([(file_hash, content), (init_hash, init_content)]),
        total_size,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, &package_path).unwrap();

    cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        true,
        None,
        None,
        crate::commands::SandboxMode::None,
        true,
        false,
        false,
        None,
    )
    .await
    .unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let stored = load_capabilities_by_name(&conn, "declared-capabilities")
        .unwrap()
        .expect("declared CCS capabilities should be stored");
    assert_eq!(
        stored.rationale.as_deref(),
        Some("needs outbound TLS and read access")
    );
    assert_eq!(stored.network.outbound, vec!["443"]);
    assert_eq!(stored.filesystem.read, vec!["/etc/ssl/certs"]);
    assert_eq!(stored.filesystem.execute, vec!["/usr/bin"]);
}

#[tokio::test]
async fn ccs_install_rejects_scriptlet_capabilities_without_enforcement_before_mutation() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::manifest::ScriptletCapabilityDeclaration;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
    use conary_core::hash;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("scriptlet-capability.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let content = b"scriptlet capability".to_vec();
    let file_hash = hash::sha256(&content);
    let (init_file, init_content, init_hash) = ccs_init_file();
    let files = vec![
        FileEntry {
            path: "/usr/bin/scriptlet-capability".to_string(),
            hash: file_hash.clone(),
            size: content.len() as u64,
            mode: 0o100755,
            component: "runtime".to_string(),
            file_type: FileType::Regular,
            target: None,
            chunks: None,
        },
        init_file,
    ];
    let total_size = (content.len() + init_content.len()) as u64;
    let mut manifest = CcsManifest::new_minimal("scriptlet-capability", "1.0.0");
    manifest
        .scriptlets
        .capabilities
        .push(ScriptletCapabilityDeclaration {
            name: "systemd-service-registration".to_string(),
            paths: vec!["/etc/systemd/system".to_string()],
        });
    let result = BuildResult {
        manifest,
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: total_size,
            },
        )]),
        files,
        blobs: HashMap::from([(file_hash, content), (init_hash, init_content)]),
        total_size,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, &package_path).unwrap();

    let err = cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        true,
        None,
        None,
        crate::commands::SandboxMode::Always,
        true,
        false,
        false,
        None,
    )
    .await
    .unwrap_err();

    let message = err.to_string();
    assert!(
        message.contains(
            "scriptlet capability declarations are present but enforcement is not available"
        ),
        "unexpected error: {message}"
    );
    let conn = conary_core::db::open(db_path_str).unwrap();
    assert!(
        conary_core::db::models::Trove::find_by_name(&conn, "scriptlet-capability")
            .unwrap()
            .is_empty(),
        "scriptlet capability gate must fail before DB mutation"
    );
}
