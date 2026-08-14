// apps/conary/src/commands/system/tests.rs

use super::{
    cmd_init, cmd_rebuild_database, cmd_rollback, cmd_rollback_with_forced_precommit_failure,
    paths_refer_to_same_location, restore_snapshot, restore_snapshots_to_live_root,
    validate_init_privileges,
};
use crate::commands::{
    FileSnapshot, NativeLifecycleSnapshot, RollbackSystemAuthority, TroveSnapshot,
    metadata_with_removed_troves,
};
use conary_core::ccs::native_lifecycle::{
    LifecyclePath, NATIVE_LIFECYCLE_SCHEMA_V1, NativeInvocation, NativeLifecycleBundle,
    NativeLifecycleEntry, NativeLifecycleEntryKind, RpmCriticality, RpmProgram, RpmRuntimeMetadata,
    ScriptletFidelity, SourceFormat, TransactionOrder, VersionScheme,
};
use conary_core::db::models::{
    Changeset, ChangesetStatus, FileEntry, InstallSource, InstalledNativeLifecycleBundle,
    PackageResolution, Repository, RepositoryPackage, RepositoryPackageKey,
    RepositoryPackageKeyStatus, Trove, TroveType,
};
use conary_core::db::paths::objects_dir;
use conary_core::filesystem::CasStore;
use conary_core::generation::root_manifest::{
    CapturedSelectedRoot, GENERATION_ROOT_MANIFEST_VERSION, GenerationRootEntry,
    GenerationRootManifest, MutableStateManifest,
};
use conary_core::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
    ResolvedPayloadNode,
};
use rusqlite::params;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[path = "tests/rollback.rs"]
mod rollback;

#[tokio::test]
async fn init_retired_schema_refusal_names_exact_rebuild_command_without_mutating() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        INSERT INTO schema_version (version) VALUES (66);",
    )
    .unwrap();
    drop(conn);

    let error = cmd_init(db_path_str).await.unwrap_err().to_string();

    assert!(
        error.contains("conary system rebuild-db --discard-state --yes"),
        "{error}"
    );
    assert_eq!(
        conary_core::db::schema::inspect(&db_path).unwrap(),
        conary_core::db::schema::SchemaCompatibility::RebuildRequired {
            observed: "retired migration-chain schema version 66".to_string()
        }
    );
}

#[tokio::test]
async fn rebuild_database_resolves_retired_schema_and_seeds_current_authority() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        INSERT INTO schema_version (version) VALUES (66);",
    )
    .unwrap();
    drop(conn);

    cmd_rebuild_database(db_path_str).await.unwrap();

    assert_eq!(
        conary_core::db::schema::inspect(&db_path).unwrap(),
        conary_core::db::schema::SchemaCompatibility::Current
    );
    let current = conary_core::db::open(&db_path).unwrap();
    conary_core::ccs::HostCapabilityInventory::load_required(&current).unwrap();
    assert!(
        Repository::find_by_name(&current, "remi-fedora-44")
            .unwrap()
            .is_some()
    );
    assert_eq!(
        std::fs::read_dir(temp_dir.path().join("backups"))
            .unwrap()
            .count(),
        1
    );
}

#[tokio::test]
async fn init_seeds_every_builtin_remi_feed_without_incomplete_native_enrollment() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    cmd_init(db_path_str).await.unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let host_capabilities =
        conary_core::ccs::HostCapabilityInventory::load_required(&conn).unwrap();
    assert_eq!(
        host_capabilities.schema_version,
        conary_core::ccs::HOST_CAPABILITY_INVENTORY_SCHEMA_VERSION
    );
    for feed in conary_core::repository::supported_profiles::public_profiles() {
        let name = format!("remi-{}", feed.id());
        let remi = Repository::find_by_name(&conn, &name).unwrap().unwrap();
        assert_eq!(remi.url, "https://remi.conary.io");
        assert_eq!(remi.default_strategy.as_deref(), Some("remi"));
        assert_eq!(
            remi.default_strategy_endpoint.as_deref(),
            Some("https://remi.conary.io")
        );
        assert_eq!(remi.source_profile.as_deref(), Some(feed.id()));
        let expected_keys = conary_core::repository::remi_authority::canonical_remi_package_keys(
            "https://remi.conary.io",
            feed.id(),
        )
        .unwrap()
        .iter()
        .filter(|key| {
            matches!(
                key.status,
                conary_core::repository::PackageKeyStatus::Active
            )
        })
        .map(|key| key.public_key.clone())
        .collect::<Vec<_>>();
        assert_eq!(
            RepositoryPackageKey::trusted_keys_for_repository(&conn, remi.id.unwrap()).unwrap(),
            expected_keys
        );
    }

    assert!(
        Repository::list_all(&conn)
            .unwrap()
            .iter()
            .all(|repository| !matches!(
                repository.package_format,
                conary_core::repository::RepositoryFormat::Arch
                    | conary_core::repository::RepositoryFormat::Debian
                    | conary_core::repository::RepositoryFormat::Fedora
            )),
        "system init must not persist native repositories before exact trust and policy enrollment"
    );
}

#[tokio::test]
async fn init_repairs_one_source_contract_without_resetting_other_feeds() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    cmd_init(db_path_str).await.unwrap();
    {
        let conn = conary_core::db::open(db_path_str).unwrap();
        let mut ubuntu = Repository::find_by_name(&conn, "remi-ubuntu-26.04")
            .unwrap()
            .unwrap();
        ubuntu.last_sync = Some("2026-07-18T00:00:00Z".to_string());
        ubuntu.update(&conn).unwrap();
        let ubuntu_id = ubuntu.id.unwrap();
        conn.execute(
            "UPDATE repositories
             SET source_profile = 'arch'
             WHERE id = ?1",
            params![ubuntu_id],
        )
        .unwrap();
        let mut package = RepositoryPackage::new(
            ubuntu_id,
            "ubuntu-package".to_string(),
            "1".to_string(),
            conary_core::repository::versioning::VersionScheme::Debian,
            "sha256:test".to_string(),
            1,
            "https://archive.ubuntu.com/ubuntu/pool/ubuntu-package.deb".to_string(),
        );
        package.source_profile = Some("ubuntu-26.04".to_string());
        let package_id = package.insert(&conn).unwrap();
        let mut resolution = PackageResolution::repository_package(
            ubuntu_id,
            "ubuntu-package".to_string(),
            package_id,
        );
        resolution.insert(&conn).unwrap();

        let mut remi = Repository::find_by_name(&conn, "remi-fedora-44")
            .unwrap()
            .unwrap();
        remi.last_sync = Some("2026-07-17T00:00:00Z".to_string());
        remi.update(&conn).unwrap();
        let mut package = RepositoryPackage::new(
            remi.id.unwrap(),
            "fedora-only".to_string(),
            "1".to_string(),
            conary_core::repository::versioning::VersionScheme::Rpm,
            "sha256:test".to_string(),
            1,
            "https://example.invalid/fedora-only".to_string(),
        );
        package.source_profile = Some("fedora-44".to_string());
        package.insert(&conn).unwrap();
        let mut resolution = PackageResolution::remi(
            remi.id.unwrap(),
            "fedora-only".to_string(),
            "https://remi.conary.io".to_string(),
            "fedora-44".to_string(),
        );
        resolution.insert(&conn).unwrap();
        RepositoryPackageKey::replace_for_repository(
            &conn,
            remi.id.unwrap(),
            &[RepositoryPackageKey {
                repository_id: remi.id.unwrap(),
                public_key: "wrong-canonical-key".to_string(),
                key_id: Some("wrong".to_string()),
                status: RepositoryPackageKeyStatus::Active,
                synced_at: None,
            }],
        )
        .unwrap();
    }

    cmd_init(db_path_str).await.unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let remi = Repository::find_by_name(&conn, "remi-fedora-44")
        .unwrap()
        .unwrap();
    assert_eq!(remi.source_profile.as_deref(), Some("fedora-44"));
    assert_eq!(remi.last_sync.as_deref(), Some("2026-07-17T00:00:00Z"));
    assert_eq!(
        RepositoryPackage::find_by_repository(&conn, remi.id.unwrap())
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        PackageResolution::find_by_repository(&conn, remi.id.unwrap())
            .unwrap()
            .len(),
        1
    );
    let expected_key = conary_core::repository::remi_authority::canonical_remi_package_keys(
        "https://remi.conary.io",
        "fedora-44",
    )
    .unwrap()[0]
        .public_key
        .clone();
    assert_eq!(
        RepositoryPackageKey::trusted_keys_for_repository(&conn, remi.id.unwrap()).unwrap(),
        vec![expected_key]
    );

    let ubuntu = Repository::find_by_name(&conn, "remi-ubuntu-26.04")
        .unwrap()
        .unwrap();
    assert_eq!(ubuntu.source_profile.as_deref(), Some("ubuntu-26.04"));
    assert!(ubuntu.last_sync.is_none());
    assert!(
        RepositoryPackage::find_by_repository(&conn, ubuntu.id.unwrap())
            .unwrap()
            .is_empty()
    );
    assert!(
        PackageResolution::find_by_repository(&conn, ubuntu.id.unwrap())
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn init_rerun_preserves_operator_repository_choices() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    cmd_init(db_path_str).await.unwrap();
    {
        let conn = conary_core::db::open(db_path_str).unwrap();

        let mut remi = Repository::find_by_name(&conn, "remi-arch")
            .unwrap()
            .unwrap();
        remi.url = "https://mirror.example.invalid/remi".to_string();
        remi.default_strategy_endpoint = Some(remi.url.clone());
        remi.source_profile = Some("fedora-44".to_string());
        remi.update(&conn).unwrap();
        RepositoryPackageKey::replace_for_repository(
            &conn,
            remi.id.unwrap(),
            &[RepositoryPackageKey {
                repository_id: remi.id.unwrap(),
                public_key: "operator-managed-key".to_string(),
                key_id: Some("operator".to_string()),
                status: RepositoryPackageKeyStatus::Active,
                synced_at: Some("2026-07-29T00:00:00Z".to_string()),
            }],
        )
        .unwrap();
    }

    cmd_init(db_path_str).await.unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let remi = Repository::find_by_name(&conn, "remi-arch")
        .unwrap()
        .unwrap();
    assert_eq!(remi.url, "https://mirror.example.invalid/remi");
    assert_eq!(remi.source_profile.as_deref(), Some("fedora-44"));
    assert_eq!(
        RepositoryPackageKey::trusted_keys_for_repository(&conn, remi.id.unwrap()).unwrap(),
        vec!["operator-managed-key".to_string()]
    );
}

#[tokio::test]
async fn init_repeat_preserves_canonical_authority_timestamps() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    cmd_init(db_path_str).await.unwrap();
    let before = {
        let conn = conary_core::db::open(db_path_str).unwrap();
        conn.prepare(
            "SELECT r.name, k.public_key, k.synced_at
             FROM repository_package_keys k
             JOIN repositories r ON r.id = k.repository_id
             WHERE r.name LIKE 'remi-%'
             ORDER BY r.name, k.public_key",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap()
    };

    cmd_init(db_path_str).await.unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let after = conn
        .prepare(
            "SELECT r.name, k.public_key, k.synced_at
             FROM repository_package_keys k
             JOIN repositories r ON r.id = k.repository_id
             WHERE r.name LIKE 'remi-%'
             ORDER BY r.name, k.public_key",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(after, before);
}

#[test]
fn system_database_init_requires_root_but_custom_test_databases_do_not() {
    let default_db = Path::new("/var/lib/conary/conary.db");
    let error = validate_init_privileges(default_db, false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("requires root privileges"));
    assert!(error.contains("re-run with sudo"));
    assert!(validate_init_privileges(default_db, true).is_ok());
    assert!(
        validate_init_privileges(Path::new("/var/lib/conary/../conary/conary.db"), false).is_err()
    );
    assert!(validate_init_privileges(Path::new("/tmp/conary-test.db"), false).is_ok());
}

#[cfg(unix)]
#[test]
fn privilege_path_comparison_resolves_a_dangling_database_symlink() {
    let temp_dir = tempfile::tempdir().unwrap();
    let actual_dir = temp_dir.path().join("actual");
    std::fs::create_dir(&actual_dir).unwrap();
    let actual_db = actual_dir.join("conary.db");
    let alias_db = temp_dir.path().join("database-alias");
    std::os::unix::fs::symlink("actual/conary.db", &alias_db).unwrap();

    assert!(paths_refer_to_same_location(&alias_db, &actual_db).unwrap());
}

#[cfg(unix)]
#[test]
fn privilege_check_rejects_deep_aliases_and_root_runtime_aliases() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut target = PathBuf::from("/var/lib/conary/conary.db");
    for index in (0..10).rev() {
        let alias = temp_dir.path().join(format!("alias-{index}"));
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        target = alias;
    }

    let non_root_error = validate_init_privileges(&target, false)
        .unwrap_err()
        .to_string();
    assert!(non_root_error.contains("canonical path"));

    let root_error = validate_init_privileges(&target, true)
        .unwrap_err()
        .to_string();
    assert!(root_error.contains("runtime state cannot diverge"));
}

#[tokio::test]
async fn init_error_names_unusable_database_parent() {
    let temp_dir = tempfile::tempdir().unwrap();
    let parent_file = temp_dir.path().join("not-a-directory");
    std::fs::write(&parent_file, b"not a directory").unwrap();
    let db_path = parent_file.join("conary.db");
    let db_path_str = db_path.to_str().unwrap();

    let err = cmd_init(db_path_str).await.unwrap_err().to_string();

    assert!(err.contains(&parent_file.display().to_string()));
    assert!(err.contains("safe next step"));
}
