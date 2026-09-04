// apps/remi/src/server/conversion/persistence/tests.rs

use super::super::test_support::create_test_db;
use super::*;
use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};
use conary_core::db::models::{RemiCatalogResource, RemiCatalogResourceKind};
use conary_core::repository::versioning::VersionScheme;

const PROFILE_REVISION: &str = "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a";

#[test]
fn conversion_finishing_after_activation_keeps_its_exact_input_revision() {
    let fixture = ActiveCatalogFixture::new();
    let revision_a = fixture.activate(
        "fedora-44",
        1,
        vec![package(
            "fedora-44",
            "fixture",
            "1.0-1.fc44",
            "",
            Some("x86_64"),
            42,
            "revision-a",
        )],
    );
    let old = fixture
        .authority()
        .open_active_profile("fedora-44")
        .expect("open revision A");
    let package_a = old
        .reader()
        .find_packages_by_name("fixture")
        .expect("read package from revision A")
        .pop()
        .expect("revision A package");

    let revision_b = fixture.activate(
        "fedora-44",
        2,
        vec![package(
            "fedora-44",
            "fixture",
            "1.0-1.fc44",
            "",
            Some("x86_64"),
            42,
            "revision-b",
        )],
    );
    let new = fixture
        .authority()
        .open_active_profile("fedora-44")
        .expect("open revision B");

    let transport = crate::server::conversion::test_support::test_transport(&[]);
    let mut converted = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        revision_a.clone(),
        package_a.name.clone(),
        package_a.version.clone(),
        package_a.architecture.clone().expect("exact architecture"),
        "rpm".to_string(),
        package_a.checksum.clone(),
        &transport,
        42,
        "sha256:converted-a".to_string(),
        "/tmp/converted-a.ccs".to_string(),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    let conn = fixture.connection();
    let converted_id = converted
        .insert_with_conversion_pin(&conn, 2)
        .expect("persist conversion and exact pin for revision A");

    assert_eq!(old.profile_revision_sha256(), revision_a);
    assert_eq!(new.profile_revision_sha256(), revision_b);
    assert_ne!(
        package_a.checksum,
        new.reader()
            .find_packages_by_name("fixture")
            .expect("read package from revision B")[0]
            .checksum
    );
    let persisted =
        ConvertedPackage::find_repository_by_checksum(&conn, &revision_a, &package_a.checksum)
            .expect("query exact revision A conversion")
            .expect("revision A conversion remains present");
    assert_eq!(persisted.id, Some(converted_id));
    assert_eq!(
        persisted.profile_revision_sha256.as_deref(),
        Some(revision_a.as_str())
    );
    assert!(
        ConvertedPackage::find_repository_by_checksum(&conn, &revision_b, &package_a.checksum,)
            .expect("query revision B conversion")
            .is_none()
    );
    let conversion_pin = ConvertedPackage::require_conversion_pin(&conn, converted_id)
        .expect("exact revision A conversion pin");
    assert_eq!(conversion_pin.profile_revision_sha256, revision_a);
    assert_eq!(conversion_pin.owner_identity, converted_id.to_string());

    drop(old);
    let reader_a_count = conn
        .query_row(
            "SELECT COUNT(*) FROM remi_profile_revision_pins
             WHERE owner_kind = 'reader' AND profile_revision_sha256 = ?1",
            [&revision_a],
            |row| row.get::<_, i64>(0),
        )
        .expect("count revision A reader pins");
    let conversion_a_count = conn
        .query_row(
            "SELECT COUNT(*) FROM remi_profile_revision_pins
             WHERE owner_kind = 'conversion' AND profile_revision_sha256 = ?1",
            [&revision_a],
            |row| row.get::<_, i64>(0),
        )
        .expect("count revision A conversion pins");
    assert_eq!(reader_a_count, 0);
    assert_eq!(conversion_a_count, 1);
}

fn cache_fixture(
    bind_current_digest: bool,
) -> (
    tempfile::NamedTempFile,
    tempfile::TempDir,
    ConversionService,
    RepositoryPackage,
    String,
    String,
) {
    cache_fixture_for_source(
        bind_current_digest,
        "fedora-base",
        "fedora",
        "rpm",
        VersionScheme::Rpm,
        "sha256:systemd-udev-259.5-1.fc44",
    )
}

fn cache_fixture_for_source(
    bind_current_digest: bool,
    _repository_name: &str,
    profile: &str,
    format: &str,
    version_scheme: VersionScheme,
    source_checksum: &str,
) -> (
    tempfile::NamedTempFile,
    tempfile::TempDir,
    ConversionService,
    RepositoryPackage,
    String,
    String,
) {
    let (database, conn) = create_test_db();
    let exact_profile = conary_core::repository::supported_profiles::profile_by_id(profile)
        .or_else(|| conary_core::repository::supported_profiles::profile_for_remi_route(profile))
        .unwrap()
        .id();
    let mut package = RepositoryPackage::new(
        1,
        "systemd-udev".to_string(),
        "259.5-1.fc44".to_string(),
        version_scheme,
        source_checksum.to_string(),
        42,
        "https://example.com/systemd-udev.pkg".to_string(),
    );
    package.architecture = Some("x86_64".to_string());
    package.source_profile = Some(exact_profile.to_string());
    let source_checksum = package.checksum.clone();
    let current_digest = conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string();
    RemiCatalogResource {
        resource_sha256: PROFILE_REVISION.to_string(),
        kind: RemiCatalogResourceKind::ProfileRevision,
        source_profile: exact_profile.to_string(),
        artifact_sha256: "b".repeat(64),
        artifact_size: 1,
        logical_digest_sha256: "c".repeat(64),
        manifest_json: "{}".to_string(),
        physical_attestation:
            crate::server::catalog_authority::test_support::physical_attestation_for_test(
                1,
                PROFILE_REVISION.as_bytes(),
            ),
        durable: true,
        created_at: 1,
    }
    .insert(&conn)
    .unwrap();

    let storage = tempfile::tempdir().unwrap();
    let cache_dir = storage.path().join("cache");
    let ccs_path = cache_dir.join("packages/existing.ccs");
    std::fs::create_dir_all(ccs_path.parent().unwrap()).unwrap();
    std::fs::write(&ccs_path, b"existing").unwrap();
    let converted_digest = if bind_current_digest {
        current_digest.clone()
    } else {
        "sha256:".to_string() + &"1".repeat(64)
    };
    let transport = crate::server::conversion::test_support::test_transport(&[]);
    let mut converted = ConvertedPackage::new_repository(
        exact_profile.to_string(),
        PROFILE_REVISION.to_string(),
        "systemd-udev".to_string(),
        "259.5-1.fc44".to_string(),
        "x86_64".to_string(),
        format.to_string(),
        source_checksum.clone(),
        &transport,
        8,
        "sha256:content".to_string(),
        ccs_path.to_string_lossy().to_string(),
        converted_digest,
    );
    converted.insert_with_conversion_pin(&conn, 1).unwrap();
    drop(conn);

    let service = ConversionService::new(
        storage.path().join("chunks"),
        cache_dir,
        database.path().to_path_buf(),
        None,
    );
    (
        database,
        storage,
        service,
        package,
        source_checksum,
        PROFILE_REVISION.to_string(),
    )
}

#[tokio::test]
async fn cache_hit_ignores_the_catalog_provides_digest() {
    let (_database, _storage, service, package, source_checksum, profile_revision) =
        cache_fixture(false);

    let result = service
        .cached_conversion_result_async("fedora-44", &package, &source_checksum, &profile_revision)
        .await
        .unwrap();

    assert_eq!(
        result.as_ref().map(|result| result.cache_state.as_str()),
        Some("hot")
    );
    let conn = conary_core::db::open(&service.db_path).unwrap();
    assert!(
        ConvertedPackage::find_repository_by_checksum(&conn, &profile_revision, &source_checksum,)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn cache_hit_allows_missing_catalog_provides_digest() {
    let (_database, _storage, service, package, source_checksum, profile_revision) =
        cache_fixture(true);
    let conn = conary_core::db::open(&service.db_path).unwrap();
    let id =
        ConvertedPackage::find_repository_by_checksum(&conn, &profile_revision, &source_checksum)
            .unwrap()
            .unwrap()
            .id
            .unwrap();
    conn.execute(
        "UPDATE converted_packages
         SET repository_provides_digest = NULL
         WHERE id = ?1",
        [id],
    )
    .unwrap();
    drop(conn);

    let result = service
        .cached_conversion_result_async("fedora-44", &package, &source_checksum, &profile_revision)
        .await
        .unwrap()
        .expect("missing diagnostic evidence must not invalidate cache");

    assert_eq!(result.cache_state, "hot");
    let conn = conary_core::db::open(&service.db_path).unwrap();
    let persisted =
        ConvertedPackage::find_repository_by_checksum(&conn, &profile_revision, &source_checksum)
            .unwrap()
            .expect("cache hit must not delete a row without diagnostic evidence");
    assert_eq!(persisted.repository_provides_digest, None);
}

#[tokio::test]
async fn cache_miss_remains_read_only_while_another_writer_is_active() {
    let (_database, _storage, service, package, source_checksum, profile_revision) =
        cache_fixture(true);
    let conn = conary_core::db::open(&service.db_path).unwrap();
    let id =
        ConvertedPackage::find_repository_by_checksum(&conn, &profile_revision, &source_checksum)
            .unwrap()
            .unwrap()
            .id
            .unwrap();
    ConvertedPackage::delete_with_conversion_pin(&conn, id).unwrap();
    drop(conn);

    let db_path = service.db_path.clone();
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let mut conn = conary_core::db::open(&db_path).unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        locked_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        tx.commit().unwrap();
    });
    locked_rx.recv().unwrap();

    let lookup = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        service.cached_conversion_result_async(
            "fedora-44",
            &package,
            &source_checksum,
            &profile_revision,
        ),
    )
    .await;
    release_tx.send(()).unwrap();
    holder.join().unwrap();

    assert!(
        lookup
            .expect("cache miss must not wait for the SQLite writer")
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn cache_hit_uses_exact_revision_without_operational_package_rows() {
    let (_database, _storage, service, package, source_checksum, profile_revision) =
        cache_fixture(true);
    let conn = conary_core::db::open(&service.db_path).unwrap();
    let operational_packages: i64 = conn
        .query_row("SELECT COUNT(*) FROM repository_packages", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(operational_packages, 0);
    drop(conn);

    let result = service
        .cached_conversion_result_async("fedora-44", &package, &source_checksum, &profile_revision)
        .await
        .unwrap()
        .expect("exact conversion cache hit");

    assert_eq!(result.cache_state, "hot");
    assert_eq!(result.name, "systemd-udev");
}

#[test]
fn candidate_eopkg_conversion_cannot_enter_the_public_serving_cache() {
    let source_checksum = "sha1:1826421aded2a344b7864ffff2fae2430778b1f0";
    let (_database, conn) = create_test_db();
    let storage = tempfile::tempdir().unwrap();
    let ccs_path = storage.path().join("candidate-solus.ccs");
    std::fs::write(&ccs_path, b"candidate").unwrap();
    let transport = crate::server::conversion::test_support::test_transport(&[]);
    let mut converted = ConvertedPackage::new_repository(
        "solus".to_string(),
        PROFILE_REVISION.to_string(),
        "systemd-udev".to_string(),
        "259.5-1.fc44".to_string(),
        "x86_64".to_string(),
        "eopkg".to_string(),
        source_checksum.to_string(),
        &transport,
        8,
        "sha256:content".to_string(),
        ccs_path.to_string_lossy().to_string(),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );

    let error = converted
        .insert_with_conversion_pin(&conn, 1)
        .expect_err("candidate profile must not enter public serving state");
    let message = error.to_string();
    assert!(message.contains("unsupported source profile 'solus'"));
    assert!(message.contains(source_checksum));
}

#[tokio::test]
async fn cache_hit_rejects_a_checksum_alias_with_conflicting_package_identity() {
    let (_database, _storage, service, package, source_checksum, profile_revision) =
        cache_fixture(true);
    let mut alias = RepositoryPackage::new(
        package.repository_id,
        "aliased-systemd".to_string(),
        package.version.clone(),
        VersionScheme::Rpm,
        source_checksum.clone(),
        package.size,
        "https://example.com/aliased-systemd.rpm".to_string(),
    );
    alias.architecture = package.architecture.clone();

    let error = service
        .cached_conversion_result_async("fedora-44", &alias, &source_checksum, &profile_revision)
        .await
        .expect_err("one checksum cannot alias two current package identities")
        .to_string();

    assert!(error.contains("conflicting catalog package identity"));
    let conn = conary_core::db::open(&service.db_path).unwrap();
    assert!(
        ConvertedPackage::find_repository_by_checksum(&conn, &profile_revision, &source_checksum,)
            .unwrap()
            .is_some(),
        "the current artifact for the original identity must not be deleted"
    );
}
