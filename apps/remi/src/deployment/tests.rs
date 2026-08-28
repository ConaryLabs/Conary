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
    let mut retired_schema = transition_json.clone();
    retired_schema["schema_version"] = serde_json::Value::from(2);
    let retired_schema: TransitionManifest = serde_json::from_value(retired_schema).unwrap();
    assert!(
        validate_transition_manifest(&retired_schema)
            .unwrap_err()
            .to_string()
            .contains("unsupported transition manifest schema 2")
    );
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
fn deployment_baseline_is_typed_bounded_and_self_measuring() {
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

    let baseline = inspect_baseline(&options.config_path).unwrap();
    assert_eq!(baseline.baseline_schema_version, 1);
    assert_eq!(baseline.schema_epoch, SCHEMA_EPOCH);
    assert_eq!(baseline.schema_revision, SCHEMA_VERSION);
    assert_eq!(baseline.configured_profiles, 3);
    assert_eq!(baseline.candidate_profiles, 0);
    assert_eq!(
        baseline
            .candidates
            .iter()
            .map(|candidate| (candidate.profile.as_str(), candidate.configured_sources))
            .collect::<Vec<_>>(),
        vec![("fedora-44", 2), ("ubuntu-26.04", 16), ("arch", 3)]
    );
    assert!(
        baseline
            .candidates
            .iter()
            .all(|candidate| candidate.identity.is_none())
    );
    assert!(baseline.measurement.sqlite_statements > 0);
    assert_eq!(baseline.measurement.catalog_file_opens, 0);
    assert_eq!(baseline.measurement.catalog_bytes_read, 0);

    let rendered = baseline.into_pretty_json().unwrap();
    let json = serde_json::from_str::<serde_json::Value>(&rendered).unwrap();
    assert_eq!(
        json["measurement"]["output_bytes"].as_u64().unwrap(),
        u64::try_from(rendered.len() + 1).unwrap()
    );
    assert!(json.get("profiles").is_none());
    assert!(json.get("universe").is_none());
    assert!(json.get("signing_profiles").is_none());
}

#[test]
fn deployment_baseline_candidate_proof_never_opens_catalog_files() {
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
            "deployment-baseline-bash",
        )],
    );
    fs::remove_dir_all(fixture.catalog_dir()).unwrap();
    let conn = fixture.connection();

    let candidates = baseline::inspect_candidates(&conn, &[("fedora-44".to_string(), 2)])
        .expect("baseline must use durable relational identity only");
    let identity = candidates[0]
        .identity
        .as_ref()
        .expect("private candidate identity");
    assert_eq!(identity.profile_revision_sha256, revision);

    let error = baseline::inspect_candidates(&conn, &[("fedora-44".to_string(), 3)])
        .expect_err("changed configured-source count must fail");
    assert!(
        error
            .to_string()
            .contains("contains 2 sources; configured authority contains 3"),
        "unexpected source-count error: {error:#}"
    );
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
    let latest_refresh = candidates[0]
        .latest_refresh
        .as_ref()
        .expect("candidate owns an exact latest refresh run");
    assert_eq!(latest_refresh.state, DeploymentRefreshRunState::Candidate);
    assert_eq!(latest_refresh.run_members, 2);
    assert_eq!(latest_refresh.candidate_members, 2);
    assert_eq!(
        Some(latest_refresh.run_id.as_str()),
        candidates[0].run_id.as_deref()
    );
    assert!(
        conary_core::db::models::RemiActiveProfileRevision::find(&conn, "fedora-44")
            .unwrap()
            .is_none()
    );

    let catalog_path = fixture
        .catalog_dir()
        .join("profiles/fedora-44")
        .join(&revision)
        .join(conary_core::repository::catalog::CATALOG_FILE_NAME);
    let mut bytes = fs::read(&catalog_path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    fs::write(&catalog_path, bytes).unwrap();
    let error = inspect_deployment_candidates(&conn, fixture.authority(), &configured)
        .expect_err("same-size candidate catalog tamper must fail");
    assert!(
        format!("{error:#}").contains("inspect private immutable profile"),
        "unexpected candidate-tamper error: {error:#}"
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
            latest_refresh: None,
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
            latest_refresh: None,
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
fn current_database_is_not_copied_and_remains_compatible_on_rollback() {
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
    let transition: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    assert_eq!(transition["database"]["action"], "keep-current");
    assert_eq!(
        transition["database"]["target"],
        db_path.to_string_lossy().as_ref()
    );
    assert!(!manifest.parent().unwrap().join("conary.db").exists());
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute("UPDATE deployment_marker SET value = 'after'", [])
        .unwrap();
    drop(conn);

    rollback(&manifest).unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let marker: String = conn
        .query_row("SELECT value FROM deployment_marker", [], |row| row.get(0))
        .unwrap();
    assert_eq!(marker, "after");
    assert!(!manifest.parent().unwrap().join("failed-current").exists());
}

#[test]
fn same_schema_rollback_rejects_an_incompatible_live_database() {
    let (temp, options) = arrange();
    let db_path = temp.path().join("metadata/conary.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    drop(conn);

    let manifest = prepare(&options).unwrap();
    fs::remove_file(&db_path).unwrap();
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

    let error = rollback(&manifest).expect_err("incompatible live database must fail closed");
    assert!(
        format!("{error:#}").contains("same-schema rollback found an incompatible live database"),
        "unexpected rollback error: {error:#}"
    );
    assert!(
        !fs::read_to_string(&options.config_path)
            .unwrap()
            .contains("[upstream.old]")
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
