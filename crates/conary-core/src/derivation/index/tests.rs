// crates/conary-core/src/derivation/index/tests.rs

use super::*;
use crate::db::schema::ensure_current;

fn setup() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    conn
}

fn sample_record(derivation_id: &str, package_name: &str) -> DerivationRecord {
    DerivationRecord {
        derivation_id: derivation_id.to_owned(),
        output_hash: crate::hash::sha256(derivation_id.as_bytes()),
        package_name: package_name.to_owned(),
        package_version: "1.0.0".to_owned(),
        manifest_cas_hash: format!("manifest_{derivation_id}"),
        stage: Some("phase1".to_owned()),
        build_env_hash: Some("envhash_abc".to_owned()),
        built_at: "2026-03-19T12:00:00Z".to_owned(),
        build_duration_secs: 42,
        trust_level: crate::derivation::index::DerivationTrustLevel::Unverified,
        provenance_cas_hash: None,
        reproducible: None,
    }
}

#[test]
fn lookup_returns_none_for_missing() {
    let conn = setup();
    let idx = DerivationIndex::new(&conn);
    let result = idx.lookup("nonexistent_id").unwrap();
    assert!(result.is_none());
}

#[test]
fn insert_then_lookup_succeeds() {
    let conn = setup();
    let idx = DerivationIndex::new(&conn);
    let record = sample_record("drv_aaa", "glibc");

    idx.insert(&record).unwrap();
    let found = idx.lookup("drv_aaa").unwrap().expect("should find record");

    assert_eq!(found, record);
}

#[test]
fn by_package_returns_only_matching() {
    let conn = setup();
    let idx = DerivationIndex::new(&conn);

    idx.insert(&sample_record("drv_1", "glibc")).unwrap();
    idx.insert(&sample_record("drv_2", "glibc")).unwrap();
    idx.insert(&sample_record("drv_3", "zlib")).unwrap();

    let glibc_records = idx.by_package("glibc").unwrap();
    assert_eq!(glibc_records.len(), 2);
    assert!(glibc_records.iter().all(|r| r.package_name == "glibc"));

    let zlib_records = idx.by_package("zlib").unwrap();
    assert_eq!(zlib_records.len(), 1);
    assert_eq!(zlib_records[0].package_name, "zlib");
}

#[test]
fn remove_deletes_and_returns_true() {
    let conn = setup();
    let idx = DerivationIndex::new(&conn);

    idx.insert(&sample_record("drv_del", "bash")).unwrap();
    assert!(idx.remove("drv_del").unwrap());
    assert!(idx.lookup("drv_del").unwrap().is_none());
}

#[test]
fn remove_returns_false_for_missing() {
    let conn = setup();
    let idx = DerivationIndex::new(&conn);
    assert!(!idx.remove("never_existed").unwrap());
}

#[test]
fn insert_or_replace_overwrites() {
    let conn = setup();
    let idx = DerivationIndex::new(&conn);

    let mut record = sample_record("drv_dup", "gcc");
    idx.insert(&record).unwrap();

    record.output_hash = crate::hash::sha256(b"new output");
    record.build_duration_secs = 99;
    idx.insert(&record).unwrap();

    let found = idx.lookup("drv_dup").unwrap().expect("should exist");
    assert_eq!(found.output_hash, crate::hash::sha256(b"new output"));
    assert_eq!(found.build_duration_secs, 99);
}

#[test]
fn by_package_returns_empty_for_unknown() {
    let conn = setup();
    let idx = DerivationIndex::new(&conn);
    let records = idx.by_package("nonexistent_pkg").unwrap();
    assert!(records.is_empty());
}

#[test]
fn nullable_fields_round_trip() {
    let conn = setup();
    let idx = DerivationIndex::new(&conn);

    let record = DerivationRecord {
        derivation_id: "drv_null".to_owned(),
        output_hash: crate::hash::sha256(b"null output"),
        package_name: "test-pkg".to_owned(),
        package_version: "2.0.0".to_owned(),
        manifest_cas_hash: "manifest_null".to_owned(),
        stage: None,
        build_env_hash: None,
        built_at: "2026-03-19T13:00:00Z".to_owned(),
        build_duration_secs: 0,
        trust_level: crate::derivation::index::DerivationTrustLevel::Unverified,
        provenance_cas_hash: None,
        reproducible: None,
    };

    idx.insert(&record).unwrap();
    let found = idx.lookup("drv_null").unwrap().expect("should exist");
    assert_eq!(found.stage, None);
    assert_eq!(found.build_env_hash, None);
}

#[test]
fn lookup_rejects_malformed_persisted_output_hashes() {
    let malformed = [
        "short".to_string(),
        "A".repeat(64),
        format!("{}é", "a".repeat(62)),
    ];

    for (index, output_hash) in malformed.into_iter().enumerate() {
        let conn = setup();
        let derivation_id = format!("drv_bad_{index}");
        conn.execute(
            "INSERT INTO derivation_index
                    (derivation_id, output_hash, package_name, package_version,
                     manifest_cas_hash, built_at, build_duration_secs)
                 VALUES (?1, ?2, 'bad', '1', 'manifest', '2026-09-03T00:00:00Z', 1)",
            rusqlite::params![derivation_id, output_hash],
        )
        .unwrap();

        let error = DerivationIndex::new(&conn)
            .lookup(&derivation_id)
            .expect_err("malformed persisted output hash must fail row decoding");

        assert!(
            error.to_string().contains(
                "persisted derivation output_hash must be a 64-character lowercase hex digest"
            ),
            "{error}"
        );
    }
}

#[test]
fn set_trust_level_is_monotonic() {
    let conn = setup();
    let idx = DerivationIndex::new(&conn);

    let mut record = sample_record("drv_trust", "bash");
    record.trust_level = crate::derivation::index::DerivationTrustLevel::LocallyBuilt;
    idx.insert(&record).unwrap();

    // Upgrade from 2 to 3
    idx.set_trust_level(
        "drv_trust",
        crate::derivation::index::DerivationTrustLevel::IndependentlyVerified,
    )
    .unwrap();
    let r = idx.lookup("drv_trust").unwrap().unwrap();
    assert_eq!(
        r.trust_level,
        crate::derivation::index::DerivationTrustLevel::IndependentlyVerified
    );

    // Attempt downgrade from 3 to 1 -- should stay at 3
    idx.set_trust_level(
        "drv_trust",
        crate::derivation::index::DerivationTrustLevel::Substituted,
    )
    .unwrap();
    let r = idx.lookup("drv_trust").unwrap().unwrap();
    assert_eq!(
        r.trust_level,
        crate::derivation::index::DerivationTrustLevel::IndependentlyVerified,
        "trust level should not decrease"
    );
}

#[test]
fn set_reproducible_round_trip() {
    let conn = setup();
    let idx = DerivationIndex::new(&conn);

    let record = sample_record("drv_repro", "gcc");
    idx.insert(&record).unwrap();

    // Initially None
    let r = idx.lookup("drv_repro").unwrap().unwrap();
    assert_eq!(r.reproducible, None);

    // Set to true
    idx.set_reproducible("drv_repro", true).unwrap();
    let r = idx.lookup("drv_repro").unwrap().unwrap();
    assert_eq!(r.reproducible, Some(true));

    // Set to false
    idx.set_reproducible("drv_repro", false).unwrap();
    let r = idx.lookup("drv_repro").unwrap().unwrap();
    assert_eq!(r.reproducible, Some(false));
}
#[test]
fn unknown_trust_cannot_be_read_or_upgraded() {
    let conn = setup();
    let idx = DerivationIndex::new(&conn);
    idx.insert(&sample_record("obsolete", "pkg")).unwrap();
    for value in [-1, 5, 256, i64::MAX] {
        conn.execute("UPDATE derivation_index SET trust_level = ?1", [value])
            .unwrap();
        for error in [
            idx.lookup("obsolete").unwrap_err(),
            idx.by_package("pkg").unwrap_err(),
            idx.set_trust_level("obsolete", DerivationTrustLevel::LocallyBuilt)
                .unwrap_err(),
        ] {
            let crate::Error::Database(rusqlite::Error::FromSqlConversionFailure(_, _, source)) =
                error
            else {
                panic!("expected typed persisted-value error: {error:?}");
            };
            assert!(
                source
                    .downcast_ref::<crate::db::models::InvalidPersistedValue>()
                    .is_some()
            );
        }
        let stored: i64 = conn
            .query_row("SELECT trust_level FROM derivation_index", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, value);
    }
}

#[test]
fn trust_upgrade_write_failure_propagates() {
    let conn = setup();
    let idx = DerivationIndex::new(&conn);
    idx.insert(&sample_record("blocked", "pkg")).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER reject_trust BEFORE UPDATE OF trust_level ON derivation_index
            BEGIN SELECT RAISE(ABORT, 'trust write rejected'); END;",
    )
    .unwrap();
    assert!(
        idx.set_trust_level("blocked", DerivationTrustLevel::LocallyBuilt)
            .is_err()
    );
    assert_eq!(
        idx.lookup("blocked").unwrap().unwrap().trust_level,
        DerivationTrustLevel::Unverified
    );
}

#[test]
fn trust_upgrade_uses_the_callers_transaction() {
    let mut conn = setup();
    DerivationIndex::new(&conn)
        .insert(&sample_record("nested", "pkg"))
        .unwrap();
    let tx = conn.transaction().unwrap();
    DerivationIndex::new(&tx)
        .set_trust_level("nested", DerivationTrustLevel::LocallyBuilt)
        .unwrap();
    tx.rollback().unwrap();
    assert_eq!(
        DerivationIndex::new(&conn)
            .lookup("nested")
            .unwrap()
            .unwrap()
            .trust_level,
        DerivationTrustLevel::Unverified
    );
}
