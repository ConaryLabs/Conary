// apps/remi/src/deployment/tests.rs

use super::*;
use std::os::unix::fs::symlink;

mod fixtures;
mod ownership;

use fixtures::{arrange, repository_manifest};

#[test]
fn prepare_hard_switches_config_and_initializes_fresh_database() {
    let (temp, options) = arrange();
    let manifest = prepare(&options).unwrap();

    let transition_json: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    assert_eq!(transition_json["schema_version"], TRANSITION_SCHEMA);
    assert_eq!(
        transition_json["runtime_root"],
        std::fs::canonicalize(temp.path())
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    let mut retired_shape = transition_json.clone();
    retired_shape
        .as_object_mut()
        .unwrap()
        .remove("runtime_root");
    assert!(serde_json::from_value::<TransitionManifest>(retired_shape).is_err());

    let config = fs::read_to_string(&options.config_path).unwrap();
    assert!(!config.contains("[upstream"));
    assert!(config.contains("max_concurrent = 32"));
    assert!(config.contains("repository_manifest"));
    assert!(config.contains("repository_keys_dir"));
    assert!(config.contains("convert_top_n = 1000"));
    let parsed_config: toml::Value = toml::from_str(&config).unwrap();
    assert_eq!(
        parsed_config["prewarm"]["distros"],
        toml::Value::Array(vec![
            toml::Value::String("arch".to_string()),
            toml::Value::String("fedora".to_string()),
            toml::Value::String("ubuntu".to_string()),
        ])
    );
    assert!(config.contains("enabled = true"));
    assert!(config.contains("endpoint = \"https://r2.example.test\""));
    for retired in [
        "eviction_threshold",
        "eviction_min_age",
        "account_id",
        "write_through",
        "r2_redirect",
    ] {
        assert!(!config.contains(retired), "retired key survived: {retired}");
    }
    assert_eq!(
        fs::read_to_string(&options.repository_manifest_target).unwrap(),
        repository_manifest()
    );

    fs::write(
        temp.path().join("metadata/conary.db"),
        b"new-current-database",
    )
    .unwrap();
    rollback(&manifest).unwrap();
    assert!(
        fs::read_to_string(&options.config_path)
            .unwrap()
            .contains("[upstream.old]")
    );
    assert!(!options.repository_manifest_target.exists());
    assert!(!temp.path().join("metadata/conary.db").exists());
}

#[test]
fn inspect_state_proves_exact_reconciled_source_authority() {
    let (_temp, options) = arrange();
    prepare(&options).unwrap();

    let config = RemiConfig::load(&options.config_path).unwrap();
    let db_path = config.storage_root().join("metadata/conary.db");
    let mut conn = rusqlite::Connection::open(&db_path).unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    RepositoryManifest::load(&options.repository_manifest_target)
        .unwrap()
        .reconcile(&mut conn)
        .unwrap();
    drop(conn);

    let state = inspect_state(&options.config_path).unwrap();
    assert_eq!(state.schema_epoch, SCHEMA_EPOCH);
    assert_eq!(state.configured_profiles, 3);
    assert_eq!(state.populated_profiles, 0);
    assert_eq!(state.catalog_packages, 0);
    assert_eq!(state.candidate_profiles, 0);
    assert_eq!(state.candidate_catalog_packages, 0);
    assert_eq!(
        state.signing_profiles,
        vec!["arch", "fedora-44", "solus", "ubuntu-26.04"]
    );
    assert_eq!(
        state
            .profiles
            .iter()
            .map(|profile| (profile.profile.as_str(), profile.configured_sources))
            .collect::<Vec<_>>(),
        vec![("fedora-44", 2), ("ubuntu-26.04", 16), ("arch", 3)]
    );
    assert!(
        state
            .profiles
            .iter()
            .all(|profile| profile.profile_revision_sha256.is_none())
    );
    assert_eq!(state.universe, None);
    assert!(!state.private_candidates_complete());
    assert!(!state.repopulation_complete());
    let json = serde_json::to_value(&state).unwrap();
    assert!(json.get("evidence_clusters").is_none());
    assert!(json.get("evidence_samples").is_none());
    assert!(json["profiles"][0].get("evidence_samples").is_none());
}

#[test]
fn deployment_population_comes_from_active_immutable_catalogs() {
    use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};

    let fixture = ActiveCatalogFixture::new();
    let revision = fixture.activate(
        "fedora-44",
        1,
        vec![package(
            "fedora-44",
            "bash",
            "5.3",
            "1",
            Some("x86_64"),
            42,
            "deployment-bash",
        )],
    );
    let conn = fixture.connection();
    let operational_packages: i64 = conn
        .query_row("SELECT COUNT(*) FROM repository_packages", [], |row| {
            row.get(0)
        })
        .unwrap();
    let configured = vec![("fedora-44".to_string(), 2)];

    let profiles = inspect_deployment_profiles(&conn, fixture.authority(), &configured)
        .expect("inspect immutable deployment population");

    assert_eq!(operational_packages, 0);
    assert_eq!(profiles.len(), 1);
    assert_eq!(
        profiles[0].profile_revision_sha256.as_deref(),
        Some(revision.as_str())
    );
    assert_eq!(profiles[0].packages, 1);
    assert_eq!(profiles[0].converted_packages, 0);
}

#[test]
fn private_candidate_population_comes_from_the_exact_current_candidate() {
    use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};

    let fixture = ActiveCatalogFixture::new();
    let revision = fixture.candidate(
        "fedora-44",
        1,
        vec![package(
            "fedora-44",
            "bash",
            "5.3",
            "1",
            Some("x86_64"),
            42,
            "deployment-candidate-bash",
        )],
    );
    let conn = fixture.connection();
    let configured = vec![("fedora-44".to_string(), 2)];

    let candidates = inspect_deployment_candidates(&conn, fixture.authority(), &configured)
        .expect("inspect exact private deployment candidate");

    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].profile_revision_sha256.as_deref(),
        Some(revision.as_str())
    );
    assert!(candidates[0].run_id.is_some());
    assert!(candidates[0].completed_at.is_some());
    assert_eq!(candidates[0].packages, 1);
    assert!(
        conary_core::db::models::RemiActiveProfileRevision::find(&conn, "fedora-44")
            .unwrap()
            .is_none()
    );
}

#[test]
fn repopulation_requires_current_conversions_and_the_matching_signed_universe() {
    let profile = DeploymentProfileState {
        profile: "fedora-44".to_string(),
        configured_sources: 1,
        profile_revision_sha256: Some("a".repeat(64)),
        packages: 1,
        converted_packages: 1,
    };
    let universe = DeploymentUniverseState {
        manifest_sha256: "b".repeat(64),
        sequence: 1,
        profiles: 1,
        canonical_map_revision: 0,
        canonical_map_entries: 0,
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        matches_active_profiles: true,
        fresh: true,
    };
    let mut state = DeploymentState {
        schema_epoch: SCHEMA_EPOCH,
        schema_revision: SCHEMA_VERSION,
        configured_profiles: 1,
        populated_profiles: 1,
        catalog_packages: 1,
        converted_packages: 1,
        candidate_profiles: 0,
        candidate_catalog_packages: 0,
        signing_profiles: vec!["fedora-44".to_string()],
        universe: Some(universe),
        profiles: vec![profile],
        candidates: vec![DeploymentCandidateState {
            profile: "fedora-44".to_string(),
            configured_sources: 1,
            profile_revision_sha256: None,
            run_id: None,
            completed_at: None,
            packages: 0,
        }],
    };

    assert!(state.repopulation_complete());
    state
        .universe
        .as_mut()
        .expect("universe")
        .matches_active_profiles = false;
    assert!(!state.repopulation_complete());
}

#[test]
fn private_candidate_completion_rejects_active_only_and_empty_catalogs() {
    let mut state = DeploymentState {
        schema_epoch: SCHEMA_EPOCH,
        schema_revision: SCHEMA_VERSION,
        configured_profiles: 1,
        populated_profiles: 1,
        catalog_packages: 1,
        converted_packages: 1,
        candidate_profiles: 0,
        candidate_catalog_packages: 0,
        signing_profiles: vec!["fedora-44".to_string()],
        universe: None,
        profiles: vec![DeploymentProfileState {
            profile: "fedora-44".to_string(),
            configured_sources: 1,
            profile_revision_sha256: Some("a".repeat(64)),
            packages: 1,
            converted_packages: 1,
        }],
        candidates: vec![DeploymentCandidateState {
            profile: "fedora-44".to_string(),
            configured_sources: 1,
            profile_revision_sha256: None,
            run_id: None,
            completed_at: None,
            packages: 0,
        }],
    };

    assert!(!state.private_candidates_complete());
    state.candidate_profiles = 1;
    state.candidates[0].profile_revision_sha256 = Some("b".repeat(64));
    state.candidates[0].run_id = Some("run".to_string());
    state.candidates[0].completed_at = Some(1);
    assert!(!state.private_candidates_complete());
    state.candidate_catalog_packages = 1;
    state.candidates[0].packages = 1;
    assert!(state.private_candidates_complete());
}

#[test]
fn retired_database_is_moved_and_restored_on_rollback() {
    let (temp, options) = arrange();
    let db_path = temp.path().join("metadata/conary.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        INSERT INTO schema_version (version) VALUES (79);",
    )
    .unwrap();
    drop(conn);
    let original = fs::read(&db_path).unwrap();

    let manifest = prepare(&options).unwrap();
    assert!(!db_path.exists());
    conary_core::db::init(&db_path).unwrap();
    rollback(&manifest).unwrap();
    assert_eq!(fs::read(&db_path).unwrap(), original);
    assert!(
        manifest
            .parent()
            .unwrap()
            .join("failed-current/conary.db")
            .exists()
    );
}

#[test]
fn current_database_is_snapshotted_and_restored_on_rollback() {
    let (temp, options) = arrange();
    let db_path = temp.path().join("metadata/conary.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    conn.execute_batch(
        "CREATE TABLE deployment_marker (value TEXT NOT NULL);
         INSERT INTO deployment_marker (value) VALUES ('before');",
    )
    .unwrap();
    drop(conn);

    let manifest = prepare(&options).unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute("UPDATE deployment_marker SET value = 'after'", [])
        .unwrap();
    drop(conn);

    rollback(&manifest).unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let marker: String = conn
        .query_row("SELECT value FROM deployment_marker", [], |row| row.get(0))
        .unwrap();
    assert_eq!(marker, "before");
    assert!(
        manifest
            .parent()
            .unwrap()
            .join("failed-current/conary.db")
            .exists()
    );
}

#[test]
fn prepare_rejects_symlinked_authority_inputs() {
    let (temp, options) = arrange();
    let real = temp.path().join("real-manifest.toml");
    fs::rename(&options.repository_manifest_source, &real).unwrap();
    symlink(&real, &options.repository_manifest_source).unwrap();

    assert!(
        prepare(&options)
            .unwrap_err()
            .to_string()
            .contains("plain file")
    );
    assert!(
        fs::read_to_string(&options.config_path)
            .unwrap()
            .contains("[upstream.old]")
    );
}
