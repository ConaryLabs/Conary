// conary-core/src/derivation/index.rs

//! Persistent derivation index backed by SQLite.
//!
//! [`DerivationIndex`] maps `derivation_id` to its output hash and metadata,
//! enabling build caching: if a derivation has already been built, we can skip
//! the build and reuse the stored output.

use crate::error::Result;
use rusqlite::Connection;

/// A completed derivation record stored in the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivationRecord {
    /// Content-addressed derivation identifier (SHA-256 hex).
    pub derivation_id: String,
    /// Hash of the build output (CAS object).
    pub output_hash: String,
    /// Human-readable package name.
    pub package_name: String,
    /// Package version string.
    pub package_version: String,
    /// CAS hash of the output manifest.
    pub manifest_cas_hash: String,
    /// Bootstrap stage (e.g. "phase1", "phase2a"), if applicable.
    pub stage: Option<String>,
    /// Hash of the build environment EROFS image, if applicable.
    pub build_env_hash: Option<String>,
    /// ISO 8601 timestamp of when the build completed.
    pub built_at: String,
    /// Wall-clock build duration in seconds.
    pub build_duration_secs: u64,
    /// Trust level (0=unverified, 1=substituted, 2=locally built,
    /// 3=independently verified, 4=diverse-verified).
    pub trust_level: u8,
    /// CAS hash of the JSON provenance record.
    pub provenance_cas_hash: Option<String>,
    /// Reproducibility status: None=unknown, Some(true)=reproducible, Some(false)=not.
    pub reproducible: Option<bool>,
}

/// Human-readable name for a trust level value.
#[must_use]
pub fn trust_level_name(level: u8) -> &'static str {
    match level {
        0 => "unverified",
        1 => "substituted",
        2 => "locally built",
        3 => "independently verified",
        4 => "diverse-verified",
        _ => "unknown",
    }
}

/// Persistent `derivation_id -> output_hash` mapping stored in SQLite.
///
/// This is the build cache for the CAS-layered bootstrap: before starting a
/// build, check `lookup()` to see if an identical derivation has already been
/// built. After a successful build, call `insert()` to record the result.
pub struct DerivationIndex<'a> {
    conn: &'a Connection,
}

impl<'a> DerivationIndex<'a> {
    /// Create a new index backed by the given connection.
    ///
    /// The connection must already have the `derivation_index` table
    /// (schema v54+).
    #[must_use]
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Look up a derivation by its content-addressed ID.
    ///
    /// Returns `None` if the derivation has not been built yet.
    pub fn lookup(&self, derivation_id: &str) -> Result<Option<DerivationRecord>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT derivation_id, output_hash, package_name, package_version,
                    manifest_cas_hash, stage, build_env_hash, built_at,
                    build_duration_secs, trust_level, provenance_cas_hash,
                    reproducible
             FROM derivation_index
             WHERE derivation_id = ?1",
        )?;

        let result = stmt.query_row([derivation_id], |row| {
            Ok(DerivationRecord {
                derivation_id: row.get(0)?,
                output_hash: row.get(1)?,
                package_name: row.get(2)?,
                package_version: row.get(3)?,
                manifest_cas_hash: row.get(4)?,
                stage: row.get(5)?,
                build_env_hash: row.get(6)?,
                built_at: row.get(7)?,
                build_duration_secs: row.get(8)?,
                trust_level: row.get(9)?,
                provenance_cas_hash: row.get(10)?,
                reproducible: row.get(11)?,
            })
        });

        match result {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Record a completed build. Uses INSERT OR REPLACE so that re-building
    /// the same derivation (e.g. after a cache clear) overwrites the old entry.
    pub fn insert(&self, record: &DerivationRecord) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO derivation_index
                (derivation_id, output_hash, package_name, package_version,
                 manifest_cas_hash, stage, build_env_hash, built_at,
                 build_duration_secs, trust_level, provenance_cas_hash,
                 reproducible)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params![
                record.derivation_id,
                record.output_hash,
                record.package_name,
                record.package_version,
                record.manifest_cas_hash,
                record.stage,
                record.build_env_hash,
                record.built_at,
                record.build_duration_secs,
                record.trust_level,
                record.provenance_cas_hash,
                record.reproducible,
            ],
        )?;
        Ok(())
    }

    /// List all derivation records for a given package name.
    pub fn by_package(&self, name: &str) -> Result<Vec<DerivationRecord>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT derivation_id, output_hash, package_name, package_version,
                    manifest_cas_hash, stage, build_env_hash, built_at,
                    build_duration_secs, trust_level, provenance_cas_hash,
                    reproducible
             FROM derivation_index
             WHERE package_name = ?1
             ORDER BY built_at DESC",
        )?;

        let rows = stmt.query_map([name], |row| {
            Ok(DerivationRecord {
                derivation_id: row.get(0)?,
                output_hash: row.get(1)?,
                package_name: row.get(2)?,
                package_version: row.get(3)?,
                manifest_cas_hash: row.get(4)?,
                stage: row.get(5)?,
                build_env_hash: row.get(6)?,
                built_at: row.get(7)?,
                build_duration_secs: row.get(8)?,
                trust_level: row.get(9)?,
                provenance_cas_hash: row.get(10)?,
                reproducible: row.get(11)?,
            })
        })?;

        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }

    /// Upgrade trust level (monotonic via SQL MAX).
    ///
    /// The trust level can only increase: if the current level is higher than
    /// the requested level, the row is unchanged.
    pub fn set_trust_level(&self, derivation_id: &str, level: u8) -> Result<()> {
        self.conn.execute(
            "UPDATE derivation_index SET trust_level = MAX(trust_level, ?2) WHERE derivation_id = ?1",
            rusqlite::params![derivation_id, level],
        )?;
        Ok(())
    }

    /// Set reproducibility flag.
    pub fn set_reproducible(&self, derivation_id: &str, reproducible: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE derivation_index SET reproducible = ?2 WHERE derivation_id = ?1",
            rusqlite::params![derivation_id, reproducible],
        )?;
        Ok(())
    }

    /// Set provenance CAS hash on a derivation record.
    pub fn set_provenance_hash(&self, derivation_id: &str, hash: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE derivation_index SET provenance_cas_hash = ?2 WHERE derivation_id = ?1",
            rusqlite::params![derivation_id, hash],
        )?;
        Ok(())
    }

    /// Remove a derivation record by ID.
    ///
    /// Returns `true` if a row was deleted, `false` if the ID was not found.
    pub fn remove(&self, derivation_id: &str) -> Result<bool> {
        let count = self.conn.execute(
            "DELETE FROM derivation_index WHERE derivation_id = ?1",
            [derivation_id],
        )?;
        Ok(count > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::migrate;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    fn sample_record(derivation_id: &str, package_name: &str) -> DerivationRecord {
        DerivationRecord {
            derivation_id: derivation_id.to_owned(),
            output_hash: format!("out_{derivation_id}"),
            package_name: package_name.to_owned(),
            package_version: "1.0.0".to_owned(),
            manifest_cas_hash: format!("manifest_{derivation_id}"),
            stage: Some("phase1".to_owned()),
            build_env_hash: Some("envhash_abc".to_owned()),
            built_at: "2026-03-19T12:00:00Z".to_owned(),
            build_duration_secs: 42,
            trust_level: 0,
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

        record.output_hash = "new_output_hash".to_owned();
        record.build_duration_secs = 99;
        idx.insert(&record).unwrap();

        let found = idx.lookup("drv_dup").unwrap().expect("should exist");
        assert_eq!(found.output_hash, "new_output_hash");
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
            output_hash: "out_null".to_owned(),
            package_name: "test-pkg".to_owned(),
            package_version: "2.0.0".to_owned(),
            manifest_cas_hash: "manifest_null".to_owned(),
            stage: None,
            build_env_hash: None,
            built_at: "2026-03-19T13:00:00Z".to_owned(),
            build_duration_secs: 0,
            trust_level: 0,
            provenance_cas_hash: None,
            reproducible: None,
        };

        idx.insert(&record).unwrap();
        let found = idx.lookup("drv_null").unwrap().expect("should exist");
        assert_eq!(found.stage, None);
        assert_eq!(found.build_env_hash, None);
    }

    #[test]
    fn set_trust_level_is_monotonic() {
        let conn = setup();
        let idx = DerivationIndex::new(&conn);

        let mut record = sample_record("drv_trust", "bash");
        record.trust_level = 2;
        idx.insert(&record).unwrap();

        // Upgrade from 2 to 3
        idx.set_trust_level("drv_trust", 3).unwrap();
        let r = idx.lookup("drv_trust").unwrap().unwrap();
        assert_eq!(r.trust_level, 3);

        // Attempt downgrade from 3 to 1 -- should stay at 3
        idx.set_trust_level("drv_trust", 1).unwrap();
        let r = idx.lookup("drv_trust").unwrap().unwrap();
        assert_eq!(r.trust_level, 3, "trust level should not decrease");
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
}
