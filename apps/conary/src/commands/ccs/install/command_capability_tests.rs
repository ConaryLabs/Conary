// apps/conary/src/commands/ccs/install/command_capability_tests.rs

use std::collections::HashMap;

use super::command::cmd_ccs_install;
use super::test_support::{ccs_init_file, ccs_regular_file, stage_test_boot_assets};

#[tokio::test]
async fn ccs_install_persists_capability_declarations() {
    use conary_core::capability::{
        CapabilityDeclaration, FilesystemCapabilities, LinuxCapabilities, NetworkCapabilities,
        SyscallCapabilities, load_capabilities_by_name,
    };
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData};
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
        ccs_regular_file(
            "/usr/bin/cap-decl".to_string(),
            file_hash.clone(),
            content.len() as u64,
            0o100755,
            "runtime".to_string(),
        ),
        ccs_regular_file(
            "/sbin/init".to_string(),
            init_hash.clone(),
            init_content.len() as u64,
            0o100755,
            "runtime".to_string(),
        ),
    ];
    let mut manifest = CcsManifest::new_minimal("declared-capabilities", "1.0.0");
    manifest.capabilities = Some(CapabilityDeclaration {
        version: 1,
        rationale: Some("needs outbound TLS and read access".to_string()),
        network: NetworkCapabilities {
            connect_tcp: vec![443],
            bind_tcp: Vec::new(),
            none: false,
        },
        filesystem: FilesystemCapabilities {
            read: vec!["/etc/ssl/certs".to_string()],
            write: Vec::new(),
            execute: vec!["/usr/bin".to_string()],
            deny: Vec::new(),
        },
        syscalls: SyscallCapabilities::default(),
        linux: LinuxCapabilities::default(),
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
        files: files.clone(),
        payloads: conary_core::ccs::builder::payloads_from_bounded_memory_for_tests(
            &files,
            HashMap::from([(file_hash, content), (init_hash, init_content)]),
        )
        .unwrap(),
        total_size,
        chunked: false,
        chunk_stats: None,
    };
    let trust_policy_path = super::test_support::write_signed_test_package(&result, &package_path);

    cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        Some(trust_policy_path.to_string_lossy().into_owned()),
        None,
        crate::commands::SandboxMode::Always,
        true,
        false,
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
    assert_eq!(stored.network.connect_tcp, vec![443]);
    assert_eq!(stored.filesystem.read, vec!["/etc/ssl/certs"]);
    assert_eq!(stored.filesystem.execute, vec!["/usr/bin"]);
}

#[tokio::test]
async fn ccs_install_rejects_scriptlet_capabilities_without_enforcement_before_mutation() {
    use conary_core::ccs::manifest::ScriptletCapabilityDeclaration;
    use conary_core::ccs::{BuildResult, CcsManifest, ComponentData};
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
        ccs_regular_file(
            "/usr/bin/scriptlet-capability".to_string(),
            file_hash.clone(),
            content.len() as u64,
            0o100755,
            "runtime".to_string(),
        ),
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
        files: files.clone(),
        payloads: conary_core::ccs::builder::payloads_from_bounded_memory_for_tests(
            &files,
            HashMap::from([(file_hash, content), (init_hash, init_content)]),
        )
        .unwrap(),
        total_size,
        chunked: false,
        chunk_stats: None,
    };
    let trust_policy_path = super::test_support::write_signed_test_package(&result, &package_path);

    let err = cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        false,
        Some(trust_policy_path.to_string_lossy().into_owned()),
        None,
        crate::commands::SandboxMode::Always,
        true,
        false,
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
