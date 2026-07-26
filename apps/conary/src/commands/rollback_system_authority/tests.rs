// apps/conary/src/commands/rollback_system_authority/tests.rs

use super::*;
use conary_core::ccs::native_lifecycle::{
    NATIVE_LIFECYCLE_SCHEMA_REVISION, NATIVE_LIFECYCLE_SCHEMA_V1, NativeLifecycleBundle,
    ScriptletFidelity, SourceFormat, VersionScheme,
};
use conary_core::db::models::{ConfigStatus, InstalledNativeLifecycleBundle, Trove};

fn residual_bundle() -> NativeLifecycleBundle {
    NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: SourceFormat::Deb,
        source_family: "debian".to_string(),
        source_profile: Some("ubuntu-26.04".to_string()),
        source_release: Some("13".to_string()),
        source_arch: Some("amd64".to_string()),
        source_package: "residual-fixture".to_string(),
        source_version: "1.0-1".to_string(),
        source_checksum: None,
        version_scheme: VersionScheme::Deb,
        conversion_tool: "test".to_string(),
        conversion_tool_version: "1".to_string(),
        conversion_policy: "rollback-system-authority".to_string(),
        evidence_digest: None,
        scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
        entries: Vec::new(),
    }
}

fn seed_exact_system_authority(conn: &Connection) {
    conn.execute(
        "INSERT INTO debian_alternative_groups
         (name, mode, master_link, selected_path)
         VALUES ('editor', 'manual', '/usr/bin/editor', '/usr/bin/vim')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO debian_alternative_slaves
         (group_name, name, link) VALUES ('editor', 'editor.1', '/usr/share/man/man1/editor.1')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO debian_alternative_choices
         (group_name, path, priority) VALUES ('editor', '/usr/bin/vim', 90)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO debian_alternative_choice_slaves
         (group_name, choice_path, slave_name, target)
         VALUES ('editor', '/usr/bin/vim', 'editor.1', '/usr/share/man/man1/vim.1')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO debian_debconf_templates (name, template_type)
         VALUES ('fixture/enable', 'boolean')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO debian_debconf_template_fields (template_name, name, value)
         VALUES ('fixture/enable', 'Description', X'656e61626c653f')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO debian_debconf_questions (name, template_name, value, seen)
         VALUES ('fixture/enable', 'fixture/enable', X'74727565', 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO debian_debconf_question_owners (question_name, owner)
         VALUES ('fixture/enable', 'fixture')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO debian_debconf_flags (question_name, name, value)
         VALUES ('fixture/enable', 'seen', 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO debian_debconf_substitutions (question_name, name, value)
         VALUES ('fixture/enable', 'PACKAGE', X'66697874757265')",
        [],
    )
    .unwrap();

    let mut residual = NativeLifecycleResidualState::new(
        &residual_bundle(),
        DebPackageState::ConfigFiles,
        vec!["fixture-trigger".to_string()],
        vec![NativePackageIdentity::new(
            "trigger-owner",
            "2.0",
            Some("amd64"),
        )],
    )
    .unwrap();
    residual.upsert(conn).unwrap();

    let mut peer = Trove::new(
        "runtime-peer".to_string(),
        "1.0-1".to_string(),
        conary_core::db::models::TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Debian,
    );
    peer.debian_multi_arch = Some(conary_core::repository::dependency_model::DebianMultiArch::No);
    let peer_id = peer.insert(conn).unwrap();
    let mut installed =
        InstalledNativeLifecycleBundle::new(peer_id, None, &residual_bundle()).unwrap();
    installed.lifecycle_state = DebPackageState::TriggersAwaited;
    installed.pending_triggers = vec!["peer-trigger".to_string()];
    installed.awaited_packages = vec![NativePackageIdentity::new(
        "awaited-peer",
        "2.0",
        Some("amd64"),
    )];
    installed.insert_or_replace(conn).unwrap();

    let mut config = conary_core::db::models::ConfigFile::new(
        "/etc/runtime-peer.conf".to_string(),
        peer_id,
        "a".repeat(64),
    );
    config.current_hash = Some("b".repeat(64));
    config.status = ConfigStatus::Modified;
    config.modified_at = Some("2026-01-02T03:04:05Z".to_string());
    config.source = conary_core::db::models::ConfigSource::Deb;
    config.insert(conn).unwrap();

    conn.execute(
        "INSERT INTO derived_packages (
            name, parent_name, status, updated_at
         ) VALUES ('runtime-derived', 'runtime-peer', 'built', '2026-01-03T04:05:06Z')",
        [],
    )
    .unwrap();
}

fn test_db() -> (tempfile::TempDir, Connection) {
    let (temp, db_path) = crate::commands::test_helpers::create_test_db();
    (temp, conary_core::db::open(db_path).unwrap())
}

#[test]
fn normalized_native_system_authority_round_trips_exactly() {
    let (_temp, mut conn) = test_db();
    seed_exact_system_authority(&conn);
    let before = RollbackSystemAuthority::capture(&conn).unwrap();
    assert!(
        before.installed_native_runtime[0]
            .trove
            .architecture
            .is_none(),
        "rollback identity must retain an exact absent architecture"
    );

    conn.execute_batch(
        "DELETE FROM debian_alternative_groups;
         DELETE FROM debian_debconf_substitutions;
         DELETE FROM debian_debconf_flags;
         DELETE FROM debian_debconf_question_owners;
         DELETE FROM debian_debconf_questions;
         DELETE FROM debian_debconf_template_fields;
         DELETE FROM debian_debconf_templates;
         DELETE FROM native_lifecycle_residual_states;
         INSERT INTO debian_alternative_groups
         (name, mode, master_link, selected_path)
         VALUES ('pager', 'auto', '/usr/bin/pager', '/usr/bin/less');
         UPDATE installed_native_lifecycle_bundles
         SET lifecycle_state = 'installed',
             pending_triggers_json = '[]',
             awaited_packages_json = '[]';
         UPDATE config_files
         SET current_hash = NULL, status = 'missing', modified_at = NULL;
         UPDATE derived_packages
         SET status = 'stale', updated_at = '2030-01-01T00:00:00Z';",
    )
    .unwrap();
    let tx = conn.transaction().unwrap();
    before.restore(&tx).unwrap();
    tx.commit().unwrap();

    assert_eq!(RollbackSystemAuthority::capture(&conn).unwrap(), before);
}

#[test]
fn normalized_native_system_authority_rejects_broken_relations() {
    let (_temp, conn) = test_db();
    seed_exact_system_authority(&conn);
    let mut authority = RollbackSystemAuthority::capture(&conn).unwrap();
    authority.alternative_groups.clear();
    let error = authority.validate().unwrap_err().to_string();
    assert!(error.contains("references missing owner"));
}
