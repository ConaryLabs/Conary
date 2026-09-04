// conary-core/src/generation/gc/tests.rs

use super::*;
use crate::db::models::{
    ConvertedPackage, FileEntry, RemiCatalogPhysicalAttestation, RemiCatalogResource,
    RemiCatalogResourceKind, Repository, RepositoryPackage, RepositoryProvide,
};
use crate::db::schema;
use crate::payload::{PayloadContentAuthority, PayloadNode, ResolvedPayloadNode};
use tempfile::TempDir;

fn create_test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::ensure_current(&conn).unwrap();
    conn
}

fn create_cas_object(objects_dir: &Path, hash: &str, content: &[u8]) {
    let (prefix, suffix) = hash.split_at(2);
    let dir = objects_dir.join(prefix);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(suffix), content).unwrap();
}

fn seed_profile_resource(conn: &Connection, source_profile: &str) -> String {
    let manifest_json = format!(r#"{{"profile":"{source_profile}"}}"#);
    let revision = crate::hash::sha256(manifest_json.as_bytes());
    RemiCatalogResource {
        resource_sha256: revision.clone(),
        kind: RemiCatalogResourceKind::ProfileRevision,
        source_profile: source_profile.to_string(),
        artifact_sha256: crate::hash::sha256(format!("artifact-{revision}").as_bytes()),
        artifact_size: 1,
        logical_digest_sha256: crate::hash::sha256(format!("logical-{revision}").as_bytes()),
        manifest_json,
        physical_attestation: RemiCatalogPhysicalAttestation::test_for_catalog_size(1),
        durable: true,
        created_at: 1,
    }
    .insert(conn)
    .unwrap();
    revision
}

fn seed_repository_source(conn: &Connection, converted: &mut ConvertedPackage) -> i64 {
    let artifact = converted.repository_artifact().unwrap();
    let source_profile = artifact.source_profile.to_string();
    let package_name = artifact.package_name.to_string();
    let package_version = artifact.package_version.to_string();
    let package_architecture = artifact.package_architecture.to_string();
    let original_checksum = converted.original_checksum.clone();
    let profile =
        crate::repository::supported_profiles::profile_by_public_id(&source_profile).unwrap();

    let repository_name = format!("gc-fixture-{source_profile}");
    let repository_id = match Repository::find_by_name(conn, &repository_name).unwrap() {
        Some(repository) => repository.id.expect("persisted fixture repository"),
        None => {
            let mut repository = Repository::new(
                repository_name,
                format!("https://example.invalid/{source_profile}"),
            );
            repository.source_profile = Some(source_profile.clone());
            repository.insert(conn).unwrap()
        }
    };
    let mut package = RepositoryPackage::new(
        repository_id,
        package_name,
        package_version,
        profile.version_scheme(),
        original_checksum,
        1,
        "https://example.invalid/package".to_string(),
    );
    package.architecture = Some(package_architecture);
    package.source_profile = Some(source_profile);
    let repository_package_id = package.insert(conn).unwrap();
    converted.repository_provides_digest = Some(
        RepositoryProvide::conversion_capabilities_digest(conn, repository_package_id).unwrap(),
    );
    repository_package_id
}

#[test]
fn current_database_roots_cover_installed_config_derived_and_chunks() {
    let conn = create_test_db();
    let installed = "1".repeat(64);
    let config = "2".repeat(64);
    let patch = "3".repeat(64);
    let chunk = "4".repeat(64);
    conn.execute(
        "INSERT INTO troves
             (name, version, type, architecture, install_source, install_reason, version_scheme)
             VALUES ('pkg', '1', 'package', 'x86_64', 'file', 'explicit', 'conary')",
        [],
    )
    .unwrap();
    let trove = conn.last_insert_rowid();
    FileEntry::new(
        "/usr/bin/pkg".to_string(),
        ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o644)).unwrap(),
        Some(PayloadContentAuthority {
            sha256: installed.clone(),
            size: 1,
        }),
        trove,
    )
    .insert(&conn)
    .unwrap();
    conn.execute(
        "INSERT INTO config_files
             (path, trove_id, package_name, package_version, original_hash, status, source)
             VALUES ('/etc/pkg', ?1, 'pkg', '1', ?2, 'pristine', 'rpm')",
        rusqlite::params![trove, config],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO derived_packages (name, parent_name)
             VALUES ('pkg-local', 'pkg')",
        [],
    )
    .unwrap();
    let derived = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO derived_patches
             (derived_id, patch_order, patch_name, patch_hash)
             VALUES (?1, 1, 'fix.patch', ?2)",
        rusqlite::params![derived, patch],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO chunk_access (hash, size_bytes, protected)
             VALUES (?1, 1, 1)",
        [&chunk],
    )
    .unwrap();

    let mut roots = CasReachability::new();
    roots.protect_current_database(&conn).unwrap();
    for hash in [installed, config, patch, chunk] {
        assert!(roots.hashes().contains(&hash));
    }
}

#[test]
fn derived_artifact_root_expands_to_typed_content_authority() {
    let conn = create_test_db();
    let tmp = TempDir::new().unwrap();
    let objects_dir = tmp.path().join("objects");
    let cas = CasStore::new(&objects_dir).unwrap();
    let content = b"derived output";
    let content_hash = cas.store(content).unwrap();
    let artifact = serde_json::json!({
        "format": "conary-derived-v2",
        "name": "pkg-local",
        "version": "1+local",
        "parent_name": "pkg",
        "parent_version": "1",
        "total_size": content.len(),
        "files": {
            "/usr/bin/pkg": {
                "node": PayloadNode::regular(0o755),
                "content": {
                    "sha256": content_hash,
                    "size": content.len()
                },
                "modified": true
            }
        }
    });
    let artifact_hash = cas.store(&serde_json::to_vec(&artifact).unwrap()).unwrap();
    conn.execute(
        "INSERT INTO derived_packages
             (name, parent_name, build_artifact_hash)
             VALUES ('pkg-local', 'pkg', ?1)",
        [&artifact_hash],
    )
    .unwrap();

    let mut roots = CasReachability::new();
    roots.protect_current_database(&conn).unwrap();
    roots
        .protect_derived_artifact_contents(&conn, &objects_dir)
        .unwrap();

    assert!(roots.hashes().contains(&artifact_hash));
    assert!(roots.hashes().contains(&content_hash));
}

#[test]
fn malformed_explicit_chunk_authority_fails_closed() {
    let conn = create_test_db();
    let transport = crate::ccs::transport::test_transport(&["NOT-A-HASH".to_string()]);
    let revision = seed_profile_resource(&conn, "fedora-44");
    let mut converted = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        revision,
        "pkg".to_string(),
        "1".to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        "checksum".to_string(),
        &transport,
        1,
        "content".to_string(),
        "/tmp/pkg.ccs".to_string(),
        crate::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    seed_repository_source(&conn, &mut converted);
    converted.insert_with_conversion_pin(&conn, 1).unwrap();
    let error = CasReachability::new()
        .protect_current_database(&conn)
        .expect_err("malformed chunk authority must abort");
    assert!(error.to_string().contains("invalid CAS reference"));
}

#[test]
fn chunk_roots_use_only_current_conversions_and_public_native_rows() {
    let conn = create_test_db();
    let current = "5".repeat(64);
    let stale = "6".repeat(64);
    let public_native = "7".repeat(64);
    let revision = seed_profile_resource(&conn, "fedora-44");
    let current_transport = crate::ccs::transport::test_transport(std::slice::from_ref(&current));
    let mut current_conversion = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        revision.clone(),
        "current".to_string(),
        "1".to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        "current-checksum".to_string(),
        &current_transport,
        1,
        current.clone(),
        "/tmp/current.ccs".to_string(),
        crate::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    seed_repository_source(&conn, &mut current_conversion);
    current_conversion
        .insert_with_conversion_pin(&conn, 1)
        .unwrap();
    let stale_transport = crate::ccs::transport::test_transport(std::slice::from_ref(&stale));
    let mut stale_conversion = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        revision,
        "stale".to_string(),
        "1".to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        "stale-checksum".to_string(),
        &stale_transport,
        1,
        stale.clone(),
        "/tmp/stale.ccs".to_string(),
        crate::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    stale_conversion.conversion_version = crate::db::models::CONVERSION_VERSION.saturating_sub(1);
    stale_conversion.insert(&conn).unwrap();

    conn.execute(
        "INSERT INTO repositories (name, url) VALUES ('fixture', 'https://example.test')",
        [],
    )
    .unwrap();
    let repository = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO repository_packages
             (repository_id, name, version, architecture, checksum, size, download_url,
              version_scheme, package_release)
             VALUES (?1, 'native', '1', 'x86_64', 'checksum', 1,
                     'https://example.test/native', 'rpm', '1')",
        [repository],
    )
    .unwrap();
    let package = conn.last_insert_rowid();
    for (name, status, transport) in [
        (
            "native",
            "public",
            serde_json::to_string(&crate::ccs::transport::test_transport(
                std::slice::from_ref(&public_native),
            ))
            .unwrap(),
        ),
        (
            "retired",
            "superseded",
            serde_json::to_string(&crate::ccs::transport::test_transport(&[
                "NOT-A-CAS-HASH".to_string()
            ]))
            .unwrap(),
        ),
    ] {
        conn.execute(
            "INSERT INTO native_package_publications
                 (repository_id, repository_package_id, source_profile, name, version,
                  package_release, architecture, package_kind, authority_format_version,
                  status, content_hash, transport_json, total_size, package_path,
                  target_path, trust_status)
                 VALUES (?1, ?2, 'fedora-44', ?3, '1', '1', 'x86_64', 'rpm', 1,
                         ?4, 'content', ?5, 1, '/tmp/native.rpm',
                         'Packages/native.rpm', 'verified')",
            rusqlite::params![repository, package, name, status, transport],
        )
        .unwrap();
    }

    let mut roots = CasReachability::new();
    roots.protect_current_database(&conn).unwrap();
    assert!(roots.hashes().contains(&current));
    assert!(!roots.hashes().contains(&stale));
    assert!(roots.hashes().contains(&public_native));
}

#[test]
fn current_conversion_without_exact_pin_is_not_a_gc_root() {
    let conn = create_test_db();
    let revision = seed_profile_resource(&conn, "fedora-44");
    let transport = crate::ccs::transport::test_transport(&["5".repeat(64)]);
    let mut converted = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        revision,
        "unpinned".to_string(),
        "1".to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        "unpinned-checksum".to_string(),
        &transport,
        1,
        "content".to_string(),
        "/tmp/unpinned.ccs".to_string(),
        crate::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    seed_repository_source(&conn, &mut converted);
    let id = converted.insert_with_conversion_pin(&conn, 1).unwrap();
    conn.execute(
        "DELETE FROM remi_profile_revision_pins WHERE pin_id = ?1",
        [ConvertedPackage::conversion_pin_id(id)],
    )
    .unwrap();

    let error = CasReachability::new()
        .protect_current_database(&conn)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("has no exact profile-revision pin"),
        "{error}"
    );
}

#[test]
fn gc_removes_only_old_unreferenced_objects() {
    let tmp = TempDir::new().unwrap();
    let objects_dir = tmp.path().join("objects");
    let live = "a".repeat(64);
    let dead = "b".repeat(64);
    create_cas_object(&objects_dir, &live, b"live");
    create_cas_object(&objects_dir, &dead, b"dead");

    let stats = gc_cas_objects_at(
        &objects_dir,
        &[live.clone()].into_iter().collect(),
        SystemTime::now() + GC_RECENT_OBJECT_GRACE_PERIOD + Duration::from_secs(1),
        GC_RECENT_OBJECT_GRACE_PERIOD,
    )
    .unwrap();

    assert_eq!(stats.objects_checked, 2);
    assert_eq!(stats.deleted_hashes, vec![dead.clone()]);
    assert!(objects_dir.join(&live[..2]).join(&live[2..]).exists());
    assert!(!objects_dir.join(&dead[..2]).join(&dead[2..]).exists());
}

#[test]
fn gc_preserves_recent_unreferenced_objects() {
    let tmp = TempDir::new().unwrap();
    let objects_dir = tmp.path().join("objects");
    let recent = "c".repeat(64);
    create_cas_object(&objects_dir, &recent, b"recent");

    let collection = CasObjectCollectionSession::acquire(&objects_dir).unwrap();
    let stats = gc_cas_objects(&collection, &HashSet::new()).unwrap();
    assert_eq!(stats.objects_checked, 1);
    assert_eq!(stats.objects_removed, 0);
}
