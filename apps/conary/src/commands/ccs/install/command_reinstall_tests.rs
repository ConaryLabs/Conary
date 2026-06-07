// src/commands/ccs/install/command_reinstall_tests.rs

use std::collections::HashMap;

use super::command::cmd_ccs_install;

#[tokio::test]
async fn ccs_install_reinstall_dry_run_does_not_mutate_db() {
    use conary_core::ccs::builder::write_ccs_package;
    use conary_core::ccs::{BuildResult, CcsManifest};

    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let package_path = temp_dir.path().join("reinstall-dry-run.ccs");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    let conn = conary_core::db::open(db_path_str).unwrap();
    let mut existing = conary_core::db::models::Trove::new(
        "reinstall-dry-run".to_string(),
        "1.0.0".to_string(),
        conary_core::db::models::TroveType::Package,
    );
    let existing_id = existing.insert(&conn).unwrap();
    drop(conn);

    let result = BuildResult {
        manifest: CcsManifest::new_minimal("reinstall-dry-run", "1.0.0"),
        components: HashMap::new(),
        files: Vec::new(),
        blobs: HashMap::new(),
        total_size: 0,
        chunked: false,
        chunk_stats: None,
    };
    write_ccs_package(&result, &package_path).unwrap();

    cmd_ccs_install(
        package_path.to_str().unwrap(),
        db_path_str,
        install_root.to_str().unwrap(),
        true,
        true,
        None,
        None,
        crate::commands::SandboxMode::None,
        true,
        true,
        false,
        None,
    )
    .await
    .unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let (trove_count, retained_id): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(MAX(id), -1) FROM troves WHERE name = 'reinstall-dry-run'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(trove_count, 1);
    assert_eq!(
        retained_id, existing_id,
        "dry-run reinstall must not delete the existing installed trove"
    );
}
