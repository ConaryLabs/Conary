// apps/remi/src/server/delta_manifests/tests.rs
use super::*;
use conary_core::db::models::{
    CONVERSION_VERSION, ConvertedPackage, RemiCatalogResource, RemiCatalogResourceKind,
};
use conary_core::db::schema;
use tempfile::NamedTempFile;

fn create_test_db() -> (NamedTempFile, Connection) {
    let temp_file = NamedTempFile::new().unwrap();
    let conn = Connection::open(temp_file.path()).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::ensure_current(&conn).unwrap();
    (temp_file, conn)
}

/// Register one durable profile-revision resource for conversion fixtures.
/// The digest is derived from the canonical manifest bytes rather than
/// using a profile label as a fake SHA-256 identity.
fn register_profile_revision(
    conn: &Connection,
    source_profile: &str,
    manifest_json: String,
) -> String {
    let revision = conary_core::hash::sha256(manifest_json.as_bytes());
    if RemiCatalogResource::find_by_sha256(conn, &revision)
        .unwrap()
        .is_none()
    {
        RemiCatalogResource {
            resource_sha256: revision.clone(),
            kind: RemiCatalogResourceKind::ProfileRevision,
            source_profile: source_profile.to_string(),
            artifact_sha256: conary_core::hash::sha256(format!("artifact-{revision}").as_bytes()),
            artifact_size: 1,
            logical_digest_sha256: conary_core::hash::sha256(
                format!("logical-{revision}").as_bytes(),
            ),
            manifest_json,
            durable: true,
            created_at: 1,
        }
        .insert(conn)
        .unwrap();
    }
    revision
}

fn profile_revision(conn: &Connection, source_profile: &str) -> String {
    register_profile_revision(
        conn,
        source_profile,
        format!(r#"{{"profile":"{source_profile}"}}"#),
    )
}

fn alternate_profile_revision(conn: &Connection, source_profile: &str) -> String {
    register_profile_revision(
        conn,
        source_profile,
        format!(r#"{{"marker":"alternate","profile":"{source_profile}"}}"#),
    )
}

fn insert_converted(
    conn: &Connection,
    source_profile: &str,
    name: &str,
    version: &str,
    chunks: &[&str],
    total_size: i64,
) {
    insert_converted_with_conversion_version(
        conn,
        source_profile,
        name,
        version,
        chunks,
        total_size,
        CONVERSION_VERSION,
    );
}

fn insert_converted_with_conversion_version(
    conn: &Connection,
    source_profile: &str,
    name: &str,
    version: &str,
    chunks: &[&str],
    total_size: i64,
    conversion_version: i32,
) {
    let profile_revision_sha256 = profile_revision(conn, source_profile);
    insert_converted_at_revision(
        conn,
        ConvertedFixture {
            source_profile,
            profile_revision_sha256: &profile_revision_sha256,
            name,
            version,
            chunks,
            total_size,
            conversion_version,
        },
    );
}

struct ConvertedFixture<'a> {
    source_profile: &'a str,
    profile_revision_sha256: &'a str,
    name: &'a str,
    version: &'a str,
    chunks: &'a [&'a str],
    total_size: i64,
    conversion_version: i32,
}

fn insert_converted_at_revision(conn: &Connection, fixture: ConvertedFixture<'_>) {
    let ConvertedFixture {
        source_profile,
        profile_revision_sha256,
        name,
        version,
        chunks,
        total_size,
        conversion_version,
    } = fixture;
    let chunk_strings: Vec<String> = chunks.iter().map(|s| (*s).to_string()).collect();
    let transport = crate::server::conversion::test_support::test_transport(&chunk_strings);
    let mut pkg = ConvertedPackage::new_repository(
        source_profile.to_string(),
        profile_revision_sha256.to_string(),
        name.to_string(),
        version.to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        format!("sha256:{name}-{version}"),
        &transport,
        total_size,
        format!("sha256:content-{name}-{version}"),
        format!("/data/{name}-{version}.ccs"),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    pkg.conversion_version = conversion_version;
    if conversion_version == CONVERSION_VERSION {
        pkg.insert_with_conversion_pin(conn, 1).unwrap();
    } else {
        pkg.insert(conn).unwrap();
    }
}

fn insert_stale_conversion(
    conn: &Connection,
    source_profile: &str,
    name: &str,
    version: &str,
    chunks: &[&str],
    total_size: i64,
) {
    let profile_revision_sha256 = profile_revision(conn, source_profile);
    let chunk_strings: Vec<String> = chunks.iter().map(|s| (*s).to_string()).collect();
    let transport = crate::server::conversion::test_support::test_transport(&chunk_strings);
    let mut pkg = ConvertedPackage::new_repository(
        source_profile.to_string(),
        profile_revision_sha256,
        name.to_string(),
        version.to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        format!("sha256:{name}-{version}"),
        &transport,
        total_size,
        format!("sha256:content-{name}-{version}"),
        format!("/data/{name}-{version}.ccs"),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    pkg.conversion_version = CONVERSION_VERSION - 1;
    pkg.insert(conn).unwrap();
}

fn insert_chunk(conn: &Connection, hash: &str, size: i64) {
    use conary_core::db::models::ChunkAccess;
    let chunk = ChunkAccess::new(hash.to_string(), size);
    chunk.upsert(conn).unwrap();
    let rows = {
        let mut statement = conn
            .prepare(
                "SELECT id, transport_json FROM converted_packages
                 WHERE transport_json LIKE ?1",
            )
            .unwrap();
        statement
            .query_map([format!("%\"{hash}\"%")], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    for (id, json) in rows {
        let mut transport: conary_core::ccs::CcsTransportEnvelopeV1 =
            serde_json::from_str(&json).unwrap();
        if let Some(object) = transport
            .objects
            .iter_mut()
            .find(|object| object.sha256 == hash)
        {
            object.size = u64::try_from(size).unwrap();
            conn.execute(
                "UPDATE converted_packages SET transport_json = ?1 WHERE id = ?2",
                params![serde_json::to_string(&transport).unwrap(), id],
            )
            .unwrap();
        }
    }
}

#[test]
fn test_compute_delta_basic() {
    let (_temp, conn) = create_test_db();

    // Version 1 has chunks A, B, C
    insert_converted(
        &conn,
        "fedora-44",
        "nginx",
        "1.0",
        &["chunkA", "chunkB", "chunkC"],
        3000,
    );
    // Version 2 has chunks B, C, D (A removed, D added)
    insert_converted(
        &conn,
        "fedora-44",
        "nginx",
        "2.0",
        &["chunkB", "chunkC", "chunkD"],
        3500,
    );

    insert_chunk(&conn, "chunkA", 1000);
    insert_chunk(&conn, "chunkB", 1000);
    insert_chunk(&conn, "chunkC", 1000);
    insert_chunk(&conn, "chunkD", 1500);

    let delta = compute_delta(
        &conn,
        &profile_revision(&conn, "fedora-44"),
        "nginx",
        "1.0",
        "2.0",
    )
    .unwrap();

    assert_eq!(delta.source_profile, "fedora-44");
    assert_eq!(delta.package_name, "nginx");
    assert_eq!(delta.from_version, "1.0");
    assert_eq!(delta.to_version, "2.0");
    assert_eq!(delta.new_chunks.len(), 1);
    assert!(delta.new_chunks.contains(&"chunkD".to_string()));
    assert_eq!(delta.removed_chunks.len(), 1);
    assert!(delta.removed_chunks.contains(&"chunkA".to_string()));
    assert_eq!(delta.download_size, 1500); // size of chunkD
    assert_eq!(delta.full_size, 3500);
}

#[test]
fn test_compute_delta_identical_versions() {
    let (_temp, conn) = create_test_db();

    insert_converted(
        &conn,
        "fedora-44",
        "curl",
        "1.0",
        &["chunkX", "chunkY"],
        2000,
    );
    insert_converted(
        &conn,
        "fedora-44",
        "curl",
        "1.1",
        &["chunkX", "chunkY"],
        2000,
    );

    let delta = compute_delta(
        &conn,
        &profile_revision(&conn, "fedora-44"),
        "curl",
        "1.0",
        "1.1",
    )
    .unwrap();

    assert!(delta.new_chunks.is_empty());
    assert!(delta.removed_chunks.is_empty());
    assert_eq!(delta.download_size, 0);
}

#[test]
fn test_compute_delta_no_overlap() {
    let (_temp, conn) = create_test_db();

    insert_converted(&conn, "arch", "vim", "1.0", &["chunkA", "chunkB"], 2000);
    insert_converted(&conn, "arch", "vim", "2.0", &["chunkC", "chunkD"], 2500);

    insert_chunk(&conn, "chunkC", 1200);
    insert_chunk(&conn, "chunkD", 1300);

    let delta =
        compute_delta(&conn, &profile_revision(&conn, "arch"), "vim", "1.0", "2.0").unwrap();

    assert_eq!(delta.new_chunks.len(), 2);
    assert_eq!(delta.removed_chunks.len(), 2);
    assert_eq!(delta.download_size, 2500); // chunkC(1200) + chunkD(1300)
}

#[test]
fn test_get_delta_not_found() {
    let (_temp, conn) = create_test_db();

    let result = get_delta(
        &conn,
        &profile_revision(&conn, "fedora-44"),
        "nonexistent",
        "1.0",
        "2.0",
    )
    .unwrap();
    assert!(result.is_none());
}

#[test]
fn test_get_delta_after_compute() {
    let (_temp, conn) = create_test_db();

    insert_converted(
        &conn,
        "fedora-44",
        "nginx",
        "1.0",
        &["chunkA", "chunkB"],
        2000,
    );
    insert_converted(
        &conn,
        "fedora-44",
        "nginx",
        "2.0",
        &["chunkB", "chunkC"],
        2500,
    );
    insert_chunk(&conn, "chunkC", 1500);

    compute_delta(
        &conn,
        &profile_revision(&conn, "fedora-44"),
        "nginx",
        "1.0",
        "2.0",
    )
    .unwrap();

    let cached = get_delta(
        &conn,
        &profile_revision(&conn, "fedora-44"),
        "nginx",
        "1.0",
        "2.0",
    )
    .unwrap()
    .unwrap();

    assert_eq!(cached.from_version, "1.0");
    assert_eq!(cached.to_version, "2.0");
    assert!(cached.new_chunks.contains(&"chunkC".to_string()));
    assert!(cached.removed_chunks.contains(&"chunkA".to_string()));
}

#[test]
fn get_delta_rejects_corrupt_persisted_chunk_sets() {
    let (_temp, conn) = create_test_db();
    insert_converted(&conn, "fedora-44", "nginx", "1.0", &["chunkA"], 1000);
    insert_converted(&conn, "fedora-44", "nginx", "2.0", &["chunkB"], 1000);
    insert_chunk(&conn, "chunkB", 1000);
    compute_delta(
        &conn,
        &profile_revision(&conn, "fedora-44"),
        "nginx",
        "1.0",
        "2.0",
    )
    .unwrap();
    conn.execute(
        "UPDATE delta_manifests SET new_chunks = 'not-json'
         WHERE source_profile = 'fedora-44' AND package_name = 'nginx'
           AND from_version = '1.0' AND to_version = '2.0'",
        [],
    )
    .unwrap();

    let error = get_delta(
        &conn,
        &profile_revision(&conn, "fedora-44"),
        "nginx",
        "1.0",
        "2.0",
    )
    .expect_err("corrupt delta metadata must not become an empty chunk set");
    assert!(error.to_string().contains("corrupt new_chunks"));
}

#[test]
fn cached_delta_requires_current_source_and_target_conversions() {
    let (_temp, conn) = create_test_db();
    insert_converted_with_conversion_version(
        &conn,
        "fedora-44",
        "nginx",
        "1.0",
        &["chunkA"],
        1000,
        CONVERSION_VERSION - 1,
    );
    insert_converted(&conn, "fedora-44", "nginx", "2.0", &["chunkB"], 1000);
    conn.execute(
        "INSERT INTO delta_manifests
         (source_profile, package_name, from_version, to_version, new_chunks, removed_chunks, download_size, full_size)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            "fedora-44",
            "nginx",
            "1.0",
            "2.0",
            "[\"chunkB\"]",
            "[\"chunkA\"]",
            1000_i64,
            1000_i64
        ],
    )
    .unwrap();

    let cached = get_delta(
        &conn,
        &profile_revision(&conn, "fedora-44"),
        "nginx",
        "1.0",
        "2.0",
    )
    .unwrap();

    assert!(cached.is_none());
}

#[test]
fn delta_fails_closed_when_a_current_conversion_pin_is_missing() {
    let (_temp, conn) = create_test_db();
    insert_converted(&conn, "fedora-44", "nginx", "1.0", &["chunkA"], 1000);
    insert_converted(&conn, "fedora-44", "nginx", "2.0", &["chunkB"], 1000);
    let revision = profile_revision(&conn, "fedora-44");
    let id: i64 = conn
        .query_row(
            "SELECT id FROM converted_packages
             WHERE profile_revision_sha256 = ?1 AND package_name = 'nginx'
               AND package_version = '1.0'",
            [&revision],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute(
        "DELETE FROM remi_profile_revision_pins WHERE pin_id = ?1",
        [ConvertedPackage::conversion_pin_id(id)],
    )
    .unwrap();

    let error = compute_delta(&conn, &revision, "nginx", "1.0", "2.0").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("has no exact profile-revision pin")
    );
}

#[test]
fn delta_fails_closed_when_a_current_conversion_pin_is_mismatched() {
    let (_temp, conn) = create_test_db();
    insert_converted(&conn, "fedora-44", "nginx", "1.0", &["chunkA"], 1000);
    insert_converted(&conn, "fedora-44", "nginx", "2.0", &["chunkB"], 1000);
    let revision = profile_revision(&conn, "fedora-44");
    let other_revision = profile_revision(&conn, "arch");
    let id: i64 = conn
        .query_row(
            "SELECT id FROM converted_packages
             WHERE profile_revision_sha256 = ?1 AND package_name = 'nginx'
               AND package_version = '1.0'",
            [&revision],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute(
        "DELETE FROM remi_profile_revision_pins WHERE pin_id = ?1",
        [ConvertedPackage::conversion_pin_id(id)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO remi_profile_revision_pins (
             pin_id, source_profile, profile_revision_sha256, owner_kind,
             owner_identity, runtime_session_id, pinned_at
         ) VALUES (?1, 'arch', ?2, 'conversion', ?3, NULL, 1)",
        params![
            ConvertedPackage::conversion_pin_id(id),
            other_revision,
            ConvertedPackage::conversion_pin_owner_identity(id),
        ],
    )
    .unwrap();

    let error = compute_delta(&conn, &revision, "nginx", "1.0", "2.0").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("has mismatched profile-revision pin identity")
    );
}

#[test]
fn cached_delta_is_revalidated_after_profile_revision_changes() {
    let (_temp, conn) = create_test_db();
    insert_converted(&conn, "fedora-44", "nginx", "1.0", &["chunkA"], 1000);
    insert_converted(&conn, "fedora-44", "nginx", "2.0", &["chunkB"], 1000);
    let old_revision = profile_revision(&conn, "fedora-44");
    compute_delta(&conn, &old_revision, "nginx", "1.0", "2.0").unwrap();

    let new_revision = alternate_profile_revision(&conn, "fedora-44");
    insert_converted_at_revision(
        &conn,
        ConvertedFixture {
            source_profile: "fedora-44",
            profile_revision_sha256: &new_revision,
            name: "nginx",
            version: "1.0",
            chunks: &["chunkA"],
            total_size: 1000,
            conversion_version: CONVERSION_VERSION,
        },
    );
    insert_converted_at_revision(
        &conn,
        ConvertedFixture {
            source_profile: "fedora-44",
            profile_revision_sha256: &new_revision,
            name: "nginx",
            version: "2.0",
            chunks: &["chunkC"],
            total_size: 1000,
            conversion_version: CONVERSION_VERSION,
        },
    );

    let cached = get_delta(&conn, &new_revision, "nginx", "1.0", "2.0").unwrap();
    assert!(cached.is_none());
}

#[test]
fn test_compute_deltas_for_package() {
    let (_temp, conn) = create_test_db();

    insert_converted(&conn, "fedora-44", "nginx", "1.0", &["chunkA"], 1000);
    insert_converted(
        &conn,
        "fedora-44",
        "nginx",
        "2.0",
        &["chunkA", "chunkB"],
        2000,
    );
    insert_converted(
        &conn,
        "fedora-44",
        "nginx",
        "3.0",
        &["chunkB", "chunkC"],
        2500,
    );
    insert_chunk(&conn, "chunkB", 1000);
    insert_chunk(&conn, "chunkC", 1500);

    let deltas =
        compute_deltas_for_package(&conn, &profile_revision(&conn, "fedora-44"), "nginx").unwrap();

    assert_eq!(deltas.len(), 2);
    assert_eq!(deltas[0].from_version, "1.0");
    assert_eq!(deltas[0].to_version, "2.0");
    assert_eq!(deltas[1].from_version, "2.0");
    assert_eq!(deltas[1].to_version, "3.0");
}

#[test]
fn version_chunks_ignore_stale_converted_rows() {
    let (_temp, conn) = create_test_db();
    insert_converted_with_conversion_version(
        &conn,
        "fedora-44",
        "nginx",
        "1.0",
        &["staleA"],
        1000,
        CONVERSION_VERSION - 1,
    );

    let (chunks, size) =
        get_version_chunks(&conn, &profile_revision(&conn, "fedora-44"), "nginx", "1.0").unwrap();

    assert!(chunks.is_empty());
    assert_eq!(size, 0);
}

#[test]
fn compute_deltas_for_package_excludes_stale_versions() {
    let (_temp, conn) = create_test_db();

    insert_converted_with_conversion_version(
        &conn,
        "fedora-44",
        "nginx",
        "1.0",
        &["staleA"],
        1000,
        CONVERSION_VERSION - 1,
    );
    insert_converted(&conn, "fedora-44", "nginx", "2.0", &["chunkA"], 1000);
    insert_converted(
        &conn,
        "fedora-44",
        "nginx",
        "3.0",
        &["chunkA", "chunkB"],
        2000,
    );
    insert_chunk(&conn, "chunkB", 1000);

    let deltas =
        compute_deltas_for_package(&conn, &profile_revision(&conn, "fedora-44"), "nginx").unwrap();

    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].from_version, "2.0");
    assert_eq!(deltas[0].to_version, "3.0");
}

#[test]
fn delta_manifests_ignore_stale_conversions() {
    let (_temp, conn) = create_test_db();
    insert_stale_conversion(&conn, "fedora-44", "nginx", "1.0", &["staleA"], 1000);
    insert_converted(&conn, "fedora-44", "nginx", "2.0", &["chunkA"], 1000);
    insert_converted(
        &conn,
        "fedora-44",
        "nginx",
        "3.0",
        &["chunkA", "chunkB"],
        2000,
    );
    insert_chunk(&conn, "chunkB", 1000);

    let (chunks, size) =
        get_version_chunks(&conn, &profile_revision(&conn, "fedora-44"), "nginx", "1.0").unwrap();
    let deltas =
        compute_deltas_for_package(&conn, &profile_revision(&conn, "fedora-44"), "nginx").unwrap();

    assert!(chunks.is_empty());
    assert_eq!(size, 0);
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].from_version, "2.0");
    assert_eq!(deltas[0].to_version, "3.0");
    assert!(
        !versions_have_current_conversions(
            &conn,
            &profile_revision(&conn, "fedora-44"),
            "nginx",
            "1.0",
            "2.0",
        )
        .unwrap()
    );
}

#[test]
fn test_compute_deltas_single_version() {
    let (_temp, conn) = create_test_db();

    insert_converted(&conn, "fedora-44", "curl", "1.0", &["chunkA"], 1000);

    let deltas =
        compute_deltas_for_package(&conn, &profile_revision(&conn, "fedora-44"), "curl").unwrap();
    assert!(deltas.is_empty());
}

#[test]
fn test_delta_response_savings() {
    let delta = DeltaManifest {
        id: Some(1),
        source_profile: "fedora-44".to_string(),
        package_name: "nginx".to_string(),
        from_version: "1.0".to_string(),
        to_version: "2.0".to_string(),
        new_chunks: vec!["chunkD".to_string()],
        removed_chunks: vec!["chunkA".to_string()],
        download_size: 1500,
        full_size: 3500,
        computed_at: None,
    };

    let response = delta.to_response();
    assert_eq!(response.from_version, "1.0");
    assert_eq!(response.to_version, "2.0");
    // Savings: (3500 - 1500) / 3500 * 100 = 57.14%
    assert!((response.savings_percent - 57.14).abs() < 0.1);
}

#[test]
fn test_delta_response_zero_full_size() {
    let delta = DeltaManifest {
        id: None,
        source_profile: "fedora-44".to_string(),
        package_name: "empty".to_string(),
        from_version: "1.0".to_string(),
        to_version: "2.0".to_string(),
        new_chunks: vec![],
        removed_chunks: vec![],
        download_size: 0,
        full_size: 0,
        computed_at: None,
    };

    let response = delta.to_response();
    assert_eq!(response.savings_percent, 0.0);
}

#[test]
fn test_compute_delta_missing_version() {
    let (_temp, conn) = create_test_db();

    // Only one version exists
    insert_converted(&conn, "fedora-44", "nginx", "1.0", &["chunkA"], 1000);
    insert_chunk(&conn, "chunkA", 1000);

    // Compute delta with nonexistent from_version - should succeed with empty from set
    let delta = compute_delta(
        &conn,
        &profile_revision(&conn, "fedora-44"),
        "nginx",
        "0.9",
        "1.0",
    )
    .unwrap();

    assert_eq!(delta.new_chunks.len(), 1);
    assert!(delta.new_chunks.contains(&"chunkA".to_string()));
    assert!(delta.removed_chunks.is_empty());
}

#[test]
fn compute_delta_uses_signed_size_without_chunk_access_side_authority() {
    let (_temp, conn) = create_test_db();
    insert_converted(&conn, "fedora-44", "nginx", "1.0", &["chunkA"], 1000);

    let delta = compute_delta(
        &conn,
        &profile_revision(&conn, "fedora-44"),
        "nginx",
        "0.9",
        "1.0",
    )
    .unwrap();
    assert_eq!(delta.download_size, 1);
    assert_eq!(delta.full_size, 1);
}
