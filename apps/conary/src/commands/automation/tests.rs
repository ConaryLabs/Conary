// apps/conary/src/commands/automation/tests.rs

use super::*;
use crate::commands::composefs_ops::test_mount_skip_guard;
use crate::commands::test_helpers::setup_command_test_db;
use conary_core::db::models::{InstalledRequirementGroup, Repository, RepositoryPackage, Trove};
use tempfile::tempdir;

#[test]
fn status_json_includes_major_upgrades() {
    let summary = AutomationSummary {
        total: 2,
        security_updates: 0,
        available_updates: 0,
        orphaned_packages: 0,
        major_upgrades: 2,
        integrity_issues: 0,
    };
    let config = AutomationConfig::default();

    let json = build_status_json(&summary, &config);
    assert_eq!(json["major_upgrades"], 2);
}

#[test]
fn automation_install_leaves_ownership_mode_model_derived() {
    let source = include_str!("../automation.rs");
    let model_derived_ownership = ["ownership: ", "None,"].concat();
    let hard_coded_default = ["ownership: Some(super::OwnershipMode", "::default()),"].concat();

    assert!(
        source.contains(&model_derived_ownership),
        "automation installs must leave ownership unset so install derives it from the model"
    );
    assert!(
        !source.contains(&hard_coded_default),
        "automation installs must not force an ownership default"
    );
}

#[tokio::test]
async fn cmd_automation_apply_yes_removes_orphans_and_records_history() {
    let (_tmp, db_path) = setup_command_test_db();
    let root = tempdir().unwrap();
    let _guard = test_mount_skip_guard();

    let conn = crate::commands::open_db(&db_path).unwrap();
    crate::commands::composefs_ops::rebuild_and_mount_from_installed_state(
        &conn,
        &db_path,
        "Initial automation cleanup generation",
    )
    .unwrap();
    conn.execute(
        "UPDATE troves
         SET name = 'orphan-cleanup-fixture',
             install_reason = 'dependency',
             selection_reason = 'Required by nginx',
             orphan_since = '2020-01-01T00:00:00Z'
         WHERE name = 'openssl'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE provides
         SET capability = 'orphan-cleanup-fixture'
         WHERE capability = 'openssl'",
        [],
    )
    .unwrap();
    let nginx_id = Trove::find_by_name(&conn, "nginx")
        .unwrap()
        .into_iter()
        .next()
        .and_then(|trove| trove.id)
        .unwrap();
    for group in InstalledRequirementGroup::find_by_trove(&conn, nginx_id).unwrap() {
        if group
            .requirement
            .expression
            .atoms()
            .iter()
            .any(|atom| atom.name == "openssl")
        {
            InstalledRequirementGroup::delete(&conn, group.id.unwrap()).unwrap();
        }
    }
    drop(conn);

    cmd_automation_apply(
        &db_path,
        root.path().to_str().unwrap(),
        true,
        Some(vec!["orphans".to_string()]),
        false,
    )
    .await
    .expect("orphan cleanup should succeed");

    let conn = crate::commands::open_db(&db_path).unwrap();
    assert!(
        Trove::find_one_by_name(&conn, "orphan-cleanup-fixture")
            .unwrap()
            .is_none()
    );

    let history: (String, String, String) = conn
        .query_row(
            "SELECT category, status, packages FROM automation_history LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(history.0, "orphans");
    assert_eq!(history.1, "applied");
    assert!(history.2.contains("orphan-cleanup-fixture"));
}

#[tokio::test]
async fn cmd_automation_apply_records_failed_history_for_unreachable_update() {
    let (_tmp, db_path) = setup_command_test_db();
    let root = tempdir().unwrap();

    let conn = crate::commands::open_db(&db_path).unwrap();
    let mut repo = Repository::new(
        "test-updates".to_string(),
        "http://127.0.0.1:9/repo".to_string(),
    );
    let repo_id = repo.insert(&conn).unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "nginx".to_string(),
        "1.24.1".to_string(),
        conary_core::repository::versioning::VersionScheme::Conary,
        "sha256:test-nginx".to_string(),
        1234,
        "http://127.0.0.1:9/nginx-1.24.1.ccs".to_string(),
    );
    pkg.architecture = Some("x86_64".to_string());
    pkg.insert(&conn).unwrap();
    drop(conn);

    let err = cmd_automation_apply(
        &db_path,
        root.path().to_str().unwrap(),
        true,
        Some(vec!["updates".to_string()]),
        false,
    )
    .await
    .expect_err("unreachable update should fail");

    let message = format!("{err:#}");
    assert!(
        message.contains("failed") || message.contains("Failed"),
        "expected failure summary, got: {message}"
    );

    let conn = crate::commands::open_db(&db_path).unwrap();
    let history: (String, String, Option<String>) = conn
        .query_row(
            "SELECT category, status, error_message
             FROM automation_history
             ORDER BY id DESC
             LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(history.0, "updates");
    assert_eq!(history.1, "failed");
    assert!(history.2.is_some());
}

#[test]
fn query_automation_history_returns_latest_first() {
    let (_tmp, db_path) = setup_command_test_db();
    let conn = crate::commands::open_db(&db_path).unwrap();
    conn.execute(
        "INSERT INTO automation_history (action_id, category, packages, status, applied_at)
         VALUES ('older', 'updates', '[\"nginx\"]', 'applied', '2026-04-08 10:00:00')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO automation_history (action_id, category, packages, status, applied_at)
         VALUES ('newer', 'orphans', '[\"openssl\"]', 'failed', '2026-04-08 11:00:00')",
        [],
    )
    .unwrap();

    let rows = query_automation_history(&conn, 10, None, None, None).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].action_id, "newer");
    assert_eq!(rows[1].action_id, "older");
}

#[test]
fn query_automation_history_rejects_corrupt_package_identity() {
    let (_tmp, db_path) = setup_command_test_db();
    let conn = crate::commands::open_db(&db_path).unwrap();
    conn.execute(
        "INSERT INTO automation_history (action_id, category, packages, status, applied_at)
         VALUES ('corrupt', 'updates', 'not-json', 'applied', '2026-04-08 10:00:00')",
        [],
    )
    .unwrap();

    let error = query_automation_history(&conn, 10, None, None, None)
        .expect_err("corrupt persisted package identity must not become an empty package set");
    assert!(error.to_string().contains("corrupt packages"));
}

#[test]
fn load_automation_config_from_path_reads_real_values() {
    let dir = tempdir().unwrap();
    let model_path = dir.path().join("system.toml");
    std::fs::write(
        &model_path,
        r#"
[model]
version = 1

[automation]
mode = "auto"
check_interval = "12h"

[automation.security]
mode = "disabled"
"#,
    )
    .unwrap();

    let config = load_automation_config_from_path(&model_path).unwrap();
    assert!(matches!(
        config.mode,
        conary_core::model::AutomationMode::Auto
    ));
    assert_eq!(config.check_interval, "12h");
    assert!(matches!(
        config.security.mode,
        Some(conary_core::model::AutomationMode::Disabled)
    ));
}

#[test]
fn update_automation_config_file_preserves_comments() {
    let dir = tempdir().unwrap();
    let model_path = dir.path().join("system.toml");
    std::fs::write(
        &model_path,
        r#"# keep me
[model]
version = 1

[system]
hostname = "demo"
"#,
    )
    .unwrap();

    update_automation_config_file(
        &model_path,
        None,
        Some("auto"),
        None,
        None,
        Some("8h"),
        false,
        false,
    )
    .unwrap();

    let updated = std::fs::read_to_string(&model_path).unwrap();
    assert!(updated.contains("# keep me"));
    assert!(updated.contains("[automation]"));
    assert!(updated.contains("mode = \"auto\""));
    assert!(updated.contains("check_interval = \"8h\""));
    assert!(updated.contains("[system]"));
}
