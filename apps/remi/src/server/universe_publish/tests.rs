// apps/remi/src/server/universe_publish/tests.rs

use std::os::unix::fs::DirBuilderExt;

use conary_core::db::models::{
    CanonicalMappingAuthority, CanonicalPackage, MetadataTable, PackageImplementation, set_metadata,
};

use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};
use crate::server::signing_authority::ensure_universe_authority;

use super::*;

struct PublicationFixture {
    catalogs: ActiveCatalogFixture,
    candidate_dir: PathBuf,
    keys_root: PathBuf,
    database_writer: DatabaseWriter,
}

impl PublicationFixture {
    fn new() -> Self {
        let catalogs = ActiveCatalogFixture::new();
        let root = catalogs
            .catalog_dir()
            .parent()
            .expect("fixture root")
            .to_path_buf();
        let candidate_dir = root.join("universe-candidates");
        fs::create_dir(&candidate_dir).expect("create universe candidate root");
        let keys_root = root.join("repository-keys");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&keys_root)
            .expect("create universe key root");
        ensure_universe_authority(&keys_root).expect("provision universe authority");
        Self {
            catalogs,
            candidate_dir,
            keys_root,
            database_writer: DatabaseWriter::default(),
        }
    }

    fn publish(&self) -> Result<UniversePublicationOutcome> {
        let outcome = publish_current_universe(
            self.catalogs.db_path(),
            self.catalogs.catalog_dir(),
            &self.candidate_dir,
            Some(&self.keys_root),
            &self.database_writer,
        )?;
        if outcome == UniversePublicationOutcome::Unavailable {
            publish_initial_universe_for_test(
                self.catalogs.db_path(),
                self.catalogs.catalog_dir(),
                &self.candidate_dir,
                &self.keys_root,
                &self.database_writer,
            )
        } else {
            Ok(outcome)
        }
    }

    fn publish_without_serving_cache_seed(&self) -> Result<UniversePublicationOutcome> {
        let outcome = publish_current_universe_from_roots(
            self.catalogs.db_path(),
            self.catalogs.catalog_dir(),
            &self.candidate_dir,
            Some(&self.keys_root),
            &self.database_writer,
        )?;
        if outcome == UniversePublicationOutcome::Unavailable {
            publish_initial_universe_for_test(
                self.catalogs.db_path(),
                self.catalogs.catalog_dir(),
                &self.candidate_dir,
                &self.keys_root,
                &self.database_writer,
            )
        } else {
            Ok(outcome)
        }
    }

    fn set_canonical_mapping(&self, canonical: &str, profile: &str, package: &str) {
        let conn = self.catalogs.connection();
        conn.execute("DELETE FROM package_implementations", [])
            .expect("clear canonical implementations");
        conn.execute("DELETE FROM canonical_packages", [])
            .expect("clear canonical packages");
        let mut canonical_package =
            CanonicalPackage::new(canonical.to_string(), "package".to_string());
        let canonical_id = canonical_package
            .insert(&conn)
            .expect("insert canonical package");
        let mut implementation = PackageImplementation::new(
            canonical_id,
            profile.to_string(),
            package.to_string(),
            CanonicalMappingAuthority::Contract,
        );
        implementation
            .insert(&conn)
            .expect("insert canonical implementation");
        set_metadata(&conn, MetadataTable::Server, "canonical_map_revision", "1")
            .expect("set canonical revision");
        set_metadata(
            &conn,
            MetadataTable::Server,
            "last_canonical_rebuild",
            "2026-08-23T00:00:00Z",
        )
        .expect("set canonical generation time");
    }
}

#[test]
fn duplicate_target_path_is_rejected() {
    let mut targets = BTreeMap::new();
    insert_target(
        &mut targets,
        "objects/sha256/a".to_string(),
        "a".repeat(64),
        1,
    )
    .unwrap();
    assert!(
        insert_target(
            &mut targets,
            "objects/sha256/a".to_string(),
            "b".repeat(64),
            1,
        )
        .is_err()
    );
}

#[test]
fn evidence_free_publication_refuses_profile_authority_change() {
    let fixture = PublicationFixture::new();
    let fedora_v1 = fixture.catalogs.activate(
        "fedora-44",
        1,
        vec![package(
            "fedora-44",
            "bash",
            "5.3",
            "1.fc44",
            Some("x86_64"),
            100,
            "fedora-bash-v1",
        )],
    );
    let ubuntu_v1 = fixture.catalogs.activate(
        "ubuntu-26.04",
        1,
        vec![package(
            "ubuntu-26.04",
            "bash",
            "5.3",
            "1ubuntu1",
            Some("amd64"),
            101,
            "ubuntu-bash-v1",
        )],
    );

    let first = fixture.publish().expect("publish initial universe");
    let UniversePublicationOutcome::Activated {
        manifest_sha256: first_sha256,
        sequence: 1,
    } = first
    else {
        panic!("initial publication did not activate sequence 1");
    };
    let conn = fixture.catalogs.connection();
    let manifest_json = conn
        .query_row(
            "SELECT manifest_json FROM remi_universe_revisions
             WHERE manifest_sha256 = ?1",
            [&first_sha256],
            |row| row.get::<_, String>(0),
        )
        .expect("load universe manifest");
    let manifest: RemiUniverseManifestV2 =
        serde_json::from_str(&manifest_json).expect("parse universe manifest");
    assert_eq!(manifest.sequence, 1);
    assert_eq!(
        manifest
            .profiles
            .iter()
            .map(|profile| (
                profile.revision.profile.as_str(),
                profile.profile_revision_sha256.as_str(),
            ))
            .collect::<Vec<_>>(),
        vec![
            ("fedora-44", fedora_v1.as_str()),
            ("ubuntu-26.04", ubuntu_v1.as_str()),
        ]
    );
    assert_eq!(
        fixture.publish().expect("repeat publication"),
        UniversePublicationOutcome::Unchanged {
            manifest_sha256: first_sha256.clone(),
            sequence: 1,
        }
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM remi_universe_revisions", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        1
    );

    let fedora_v2 = fixture.catalogs.activate(
        "fedora-44",
        2,
        vec![package(
            "fedora-44",
            "bash",
            "5.3",
            "2.fc44",
            Some("x86_64"),
            102,
            "fedora-bash-v2",
        )],
    );
    let error = fixture
        .publish()
        .expect_err("evidence-free authority change must fail");
    assert!(
        format!("{error:#}")
            .contains("evidence-free universe publication cannot change active profile authority"),
        "{error:#}"
    );
    assert_ne!(fedora_v2, fedora_v1);
    assert_eq!(
        conn.query_row(
            "SELECT manifest_sha256 FROM remi_active_universe_revision WHERE singleton = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        first_sha256
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM remi_universe_revisions", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        1
    );
}

#[test]
fn obsolete_universe_schema_is_replaced_without_deserializing_it() {
    let fixture = PublicationFixture::new();
    fixture.catalogs.activate(
        "fedora-44",
        1,
        vec![package(
            "fedora-44",
            "bash",
            "5.3",
            "1.fc44",
            Some("x86_64"),
            100,
            "fedora-obsolete-universe",
        )],
    );
    fixture.publish().expect("publish initial universe");
    let obsolete_sha256 = fixture
        .catalogs
        .replace_active_universe_with_obsolete_schema();

    assert!(matches!(
        crate::server::public_universe::PublicUniverseSnapshot::load(fixture.catalogs.db_path())
            .expect("classify obsolete universe"),
        crate::server::public_universe::PublicUniverseLoadOutcome::ObsoleteUniverseSchema {
            found: 1,
            required: REMI_UNIVERSE_SCHEMA_V2,
        }
    ));

    let UniversePublicationOutcome::Activated {
        manifest_sha256,
        sequence: 3,
    } = fixture
        .publish()
        .expect("publish current-schema replacement")
    else {
        panic!("obsolete universe was not replaced")
    };
    assert_ne!(manifest_sha256, obsolete_sha256);
    assert!(matches!(
        crate::server::public_universe::PublicUniverseSnapshot::load(fixture.catalogs.db_path())
            .expect("load replacement universe"),
        crate::server::public_universe::PublicUniverseLoadOutcome::Current(_)
    ));
}

#[test]
fn obsolete_universe_replacement_requires_unchanged_evidenced_authority() {
    let fixture = PublicationFixture::new();
    fixture.catalogs.activate(
        "fedora-44",
        1,
        vec![package(
            "fedora-44",
            "bash",
            "5.3",
            "1.fc44",
            Some("x86_64"),
            100,
            "fedora-obsolete-evidence-v1",
        )],
    );
    fixture.publish().expect("publish initial universe");
    let obsolete_sha256 = fixture
        .catalogs
        .replace_active_universe_with_obsolete_schema();
    fixture.catalogs.activate(
        "fedora-44",
        2,
        vec![package(
            "fedora-44",
            "bash",
            "5.4",
            "1.fc44",
            Some("x86_64"),
            101,
            "fedora-obsolete-evidence-v2",
        )],
    );

    let error = fixture
        .publish()
        .expect_err("obsolete schema must not bypass promotion evidence");
    assert!(
        error
            .to_string()
            .contains("evidence-free universe publication cannot change active profile authority"),
        "{error:#}"
    );
    let conn = fixture.catalogs.connection();
    assert_eq!(
        conn.query_row(
            "SELECT manifest_sha256 FROM remi_active_universe_revision WHERE singleton = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        obsolete_sha256
    );
}

#[tokio::test]
async fn activated_outcome_survives_search_rebuild_failure() {
    let fixture = PublicationFixture::new();
    fixture.catalogs.activate(
        "fedora-44",
        1,
        vec![package(
            "fedora-44",
            "bash",
            "5.3",
            "1.fc44",
            Some("x86_64"),
            100,
            "fedora-search-failure",
        )],
    );
    fixture.publish().expect("publish initial universe");
    fixture
        .catalogs
        .replace_active_universe_with_obsolete_schema();

    let root = fixture
        .catalogs
        .catalog_dir()
        .parent()
        .expect("fixture root");
    let config = crate::server::ServerConfig {
        db_path: fixture.catalogs.db_path().to_path_buf(),
        catalog_dir: fixture.catalogs.catalog_dir().to_path_buf(),
        catalog_candidate_dir: fixture.candidate_dir.clone(),
        chunk_dir: root.join("chunks"),
        cache_dir: root.join("cache"),
        release_publish: crate::server::config::ReleasePublishSection {
            repository_keys_dir: Some(fixture.keys_root.clone()),
            ..Default::default()
        },
        ..Default::default()
    };
    fs::create_dir_all(&config.chunk_dir).expect("create chunk root");
    fs::create_dir_all(&config.cache_dir).expect("create cache root");

    let mut server_state = crate::server::ServerState::new(config).expect("server state");
    server_state.catalog_authority = crate::server::catalog_authority::CatalogAuthority::from_paths(
        fixture.catalogs.db_path().to_path_buf(),
        root.join("missing-search-catalogs"),
        server_state.database_writer.clone(),
    );
    let search_dir = root.join("search");
    let search_engine =
        Arc::new(crate::server::SearchEngine::new(&search_dir).expect("create search engine"));
    server_state.search_engine = Some(Arc::clone(&search_engine));
    let state = Arc::new(RwLock::new(server_state));

    let outcome = publish_current_universe_from_state(&state)
        .await
        .expect("activation outcome must not be replaced by search failure");
    let UniversePublicationOutcome::Activated { sequence: 3, .. } = outcome else {
        panic!("obsolete universe replacement did not activate")
    };
    let crate::server::public_universe::PublicUniverseLoadOutcome::Current(universe) =
        crate::server::public_universe::PublicUniverseSnapshot::load(fixture.catalogs.db_path())
            .expect("load activated universe")
    else {
        panic!("activated replacement is not current")
    };
    assert!(matches!(
        search_engine.search_public_universe(universe.identity(), "bash", None, 10),
        Err(crate::server::search::PublicSearchError::Unavailable)
    ));
}

#[tokio::test]
async fn unchanged_outcome_preserves_valid_search_authority_on_rebuild_failure() {
    let fixture = PublicationFixture::new();
    fixture.catalogs.activate(
        "fedora-44",
        1,
        vec![package(
            "fedora-44",
            "bash",
            "5.3",
            "1.fc44",
            Some("x86_64"),
            100,
            "fedora-unchanged-search-failure",
        )],
    );
    fixture.publish().expect("publish initial universe");
    let crate::server::public_universe::PublicUniverseLoadOutcome::Current(universe) =
        crate::server::public_universe::PublicUniverseSnapshot::load(fixture.catalogs.db_path())
            .expect("load active universe")
    else {
        panic!("initial universe is not current")
    };

    let root = fixture
        .catalogs
        .catalog_dir()
        .parent()
        .expect("fixture root");
    let search_engine = Arc::new(
        crate::server::SearchEngine::new(&root.join("search")).expect("create search engine"),
    );
    search_engine
        .rebuild_from_universe(
            fixture.catalogs.db_path(),
            fixture.catalogs.authority(),
            &universe,
        )
        .expect("seed current search authority");
    assert_eq!(
        search_engine
            .search_public_universe(universe.identity(), "bash", None, 10)
            .expect("search current universe")
            .len(),
        1
    );

    let config = crate::server::ServerConfig {
        db_path: fixture.catalogs.db_path().to_path_buf(),
        catalog_dir: fixture.catalogs.catalog_dir().to_path_buf(),
        catalog_candidate_dir: fixture.candidate_dir.clone(),
        chunk_dir: root.join("chunks"),
        cache_dir: root.join("cache"),
        release_publish: crate::server::config::ReleasePublishSection {
            repository_keys_dir: Some(fixture.keys_root.clone()),
            ..Default::default()
        },
        ..Default::default()
    };
    fs::create_dir_all(&config.chunk_dir).expect("create chunk root");
    fs::create_dir_all(&config.cache_dir).expect("create cache root");
    let mut server_state = crate::server::ServerState::new(config).expect("server state");
    server_state.catalog_authority = crate::server::catalog_authority::CatalogAuthority::from_paths(
        fixture.catalogs.db_path().to_path_buf(),
        root.join("missing-search-catalogs"),
        server_state.database_writer.clone(),
    );
    server_state.search_engine = Some(Arc::clone(&search_engine));
    let state = Arc::new(RwLock::new(server_state));

    let outcome = publish_current_universe_from_state(&state)
        .await
        .expect("unchanged publication must survive search refresh failure");
    assert!(matches!(
        outcome,
        UniversePublicationOutcome::Unchanged { sequence: 1, .. }
    ));
    assert_eq!(
        search_engine
            .search_public_universe(universe.identity(), "bash", None, 10)
            .expect("existing search authority remains valid")
            .len(),
        1
    );
}

#[test]
fn publication_validation_does_not_seed_the_serving_reader_cache() {
    let fixture = PublicationFixture::new();
    let revision = fixture.catalogs.activate(
        "fedora-44",
        1,
        vec![package(
            "fedora-44",
            "bash",
            "5.3",
            "1.fc44",
            Some("x86_64"),
            100,
            "fedora-cache-seed",
        )],
    );

    fixture
        .publish_without_serving_cache_seed()
        .expect("publish universe without seeding a serving reader");

    assert!(
        !fixture
            .catalogs
            .authority()
            .has_verified_profile_reader_for_test("fedora-44", &revision)
    );
}

#[test]
fn tampered_active_bundle_fails_closed_without_advancing_pointer() {
    let fixture = PublicationFixture::new();
    fixture.catalogs.activate(
        "fedora-44",
        1,
        vec![package(
            "fedora-44",
            "bash",
            "5.3",
            "1.fc44",
            Some("x86_64"),
            100,
            "fedora-bash",
        )],
    );
    let first = fixture.publish().expect("publish initial universe");
    let UniversePublicationOutcome::Activated {
        manifest_sha256,
        sequence: 1,
    } = first
    else {
        panic!("initial publication did not activate sequence 1");
    };
    let canonical_path = universe_bundle_path(fixture.catalogs.catalog_dir(), &manifest_sha256)
        .join(UNIVERSE_CANONICAL_MAP_FILE);
    fs::write(&canonical_path, b"{}\n").expect("tamper canonical map");

    let error = fixture.publish().expect_err("tampered bundle must fail");
    assert!(
        format!("{error:#}").contains("invalid canonical map JSON"),
        "{error:#}"
    );
    let conn = fixture.catalogs.connection();
    assert_eq!(
        conn.query_row(
            "SELECT manifest_sha256, sequence FROM remi_active_universe_revision
             WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap(),
        (manifest_sha256, 1)
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM remi_universe_revisions", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        1
    );
}

#[test]
fn evidence_free_canonical_change_preserves_the_active_universe() {
    let fixture = PublicationFixture::new();
    fixture.catalogs.activate(
        "fedora-44",
        1,
        vec![package(
            "fedora-44",
            "bash",
            "5.3",
            "1.fc44",
            Some("x86_64"),
            100,
            "fedora-bash",
        )],
    );
    let first = fixture.publish().expect("publish initial universe");
    let UniversePublicationOutcome::Activated {
        manifest_sha256,
        sequence: 1,
    } = first
    else {
        panic!("initial publication did not activate sequence 1");
    };
    fixture.set_canonical_mapping("shell", "fedora-44", "missing-shell");

    let error = fixture
        .publish()
        .expect_err("evidence-free canonical authority change must fail");
    assert!(
        format!("{error:#}")
            .contains("evidence-free universe publication cannot change active profile authority"),
        "{error:#}"
    );
    let conn = fixture.catalogs.connection();
    assert_eq!(
        conn.query_row(
            "SELECT manifest_sha256, sequence FROM remi_active_universe_revision
             WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap(),
        (manifest_sha256, 1)
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM remi_universe_revisions", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        1
    );
    assert_eq!(
        fs::read_dir(fixture.catalogs.catalog_dir().join("universes"))
            .expect("read durable universe bundles")
            .count(),
        1
    );
}

#[test]
fn unchanged_authority_renews_before_timestamp_expiry() {
    let now = "2026-08-22T12:00:00Z".parse().unwrap();
    assert!(!requires_renewal(
        now,
        now + Duration::days(7),
        now + Duration::hours(7),
    ));
    assert!(requires_renewal(
        now,
        now + Duration::days(7),
        now + Duration::hours(6),
    ));
    assert!(requires_renewal(
        now,
        now + Duration::hours(5),
        now + Duration::days(1),
    ));
}
