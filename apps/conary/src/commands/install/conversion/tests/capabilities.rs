// src/commands/install/conversion/tests/capabilities.rs

use super::*;

#[tokio::test]
async fn converted_ccs_install_rejects_unknown_target_syscall_before_db_mutation() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let mut manifest = CcsManifest::new_minimal("converted-unknown-syscall", "1.0.0");
    manifest.capabilities = Some(CapabilityDeclaration {
        version: 1,
        rationale: Some("declares a syscall outside the selected target ABI".to_string()),
        network: NetworkCapabilities {
            connect_tcp: Vec::new(),
            bind_tcp: vec![80],
            none: false,
        },
        filesystem: FilesystemCapabilities::default(),
        syscalls: SyscallCapabilities {
            allow: vec!["definitely_not_a_syscall".to_string()],
            deny: Vec::new(),
        },
        linux: conary_core::capability::LinuxCapabilities::default(),
    });
    let package_path =
        write_runtime_ccs_package(temp_dir.path(), "converted-unknown-syscall", manifest);

    let err = install_ccs_artifact(CcsArtifactInstallOptions {
        ccs_path: package_path.to_str().unwrap(),
        db_path: db_path_str,
        root: install_root.to_str().unwrap(),
        dry_run: false,
        sandbox_mode: SandboxMode::Always,
        no_deps: true,
        allow_downgrade: false,
        intent: InstallIntent::PackageChange,
        yes: true,
        envelope_authority: CcsEnvelopeAuthority::LocalDev,
        repository_provenance: None,
        resolution_policy: test_resolution_policy(),
    })
    .await
    .unwrap_err();

    assert!(
        format!("{err:?}").contains("is not an exact name"),
        "converted CCS install should fail closed for unknown target syscalls: {err:?}"
    );
    let conn = conary_core::db::open(db_path_str).unwrap();
    let trove_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM troves WHERE name = 'converted-unknown-syscall'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let changeset_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM changesets", [], |row| row.get(0))
        .unwrap();
    assert_eq!(trove_count, 0);
    assert_eq!(changeset_count, 0);
    assert!(
        std::fs::read_link(temp_dir.path().join("current")).is_err(),
        "capability rejection must happen before generation activation"
    );
}

#[tokio::test]
async fn converted_ccs_install_accepts_prompted_capabilities_when_allowed() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());

    let mut manifest = CcsManifest::new_minimal("converted-allowed-capability", "1.0.0");
    manifest.capabilities = Some(CapabilityDeclaration {
        version: 1,
        rationale: Some("binds a privileged test port".to_string()),
        network: NetworkCapabilities {
            connect_tcp: Vec::new(),
            bind_tcp: vec![80],
            none: false,
        },
        filesystem: FilesystemCapabilities::default(),
        syscalls: SyscallCapabilities::default(),
        linux: conary_core::capability::LinuxCapabilities {
            required: vec!["cap-net-bind-service".to_string()],
        },
    });
    let package_path =
        write_runtime_ccs_package(temp_dir.path(), "converted-allowed-capability", manifest);

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
        envelope_authority: CcsEnvelopeAuthority::LocalDev,
        repository_provenance: None,
        resolution_policy: test_resolution_policy(),
    })
    .await
    .unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let trove_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM troves WHERE name = 'converted-allowed-capability'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(trove_count, 1);
}
