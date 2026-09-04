// crates/conary-core/src/derivation/index.rs

//! Persistent derivation index backed by SQLite.
//!
//! [`DerivationIndex`] maps `derivation_id` to its output hash and metadata,
//! enabling build caching: if a derivation has already been built, we can skip
//! the build and reuse the stored output.

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row};
use std::io;

/// Trust evidence recorded with a derivation; persisted integer values are stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(i64)]
pub enum DerivationTrustLevel {
    Unverified = 0,
    Substituted = 1,
    LocallyBuilt = 2,
    IndependentlyVerified = 3,
    DiverseVerified = 4,
}

impl DerivationTrustLevel {
    pub const fn as_i64(self) -> i64 {
        self as i64
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Unverified => "unverified",
            Self::Substituted => "substituted",
            Self::LocallyBuilt => "locally built",
            Self::IndependentlyVerified => "independently verified",
            Self::DiverseVerified => "diverse-verified",
        }
    }
}

impl TryFrom<i64> for DerivationTrustLevel {
    type Error = crate::db::models::InvalidPersistedValue;

    fn try_from(value: i64) -> std::result::Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Unverified),
            1 => Ok(Self::Substituted),
            2 => Ok(Self::LocallyBuilt),
            3 => Ok(Self::IndependentlyVerified),
            4 => Ok(Self::DiverseVerified),
            other => Err(Self::Error::new(
                "derivation trust level",
                other.to_string(),
                "a current trust level (0 through 4); rebuild before trusting this record",
            )),
        }
    }
}

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
    /// Trust evidence validated by the index owner.
    pub trust_level: DerivationTrustLevel,
    /// CAS hash of the JSON provenance record.
    pub provenance_cas_hash: Option<String>,
    /// Reproducibility status: None=unknown, Some(true)=reproducible, Some(false)=not.
    pub reproducible: Option<bool>,
}

impl DerivationRecord {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let output_hash: String = row.get(1)?;
        if output_hash.len() != 64
            || !output_hash
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "persisted derivation output_hash must be a 64-character lowercase hex digest",
                )),
            ));
        }

        Ok(Self {
            derivation_id: row.get(0)?,
            output_hash,
            package_name: row.get(2)?,
            package_version: row.get(3)?,
            manifest_cas_hash: row.get(4)?,
            stage: row.get(5)?,
            build_env_hash: row.get(6)?,
            built_at: row.get(7)?,
            build_duration_secs: row.get::<_, i64>(8)? as u64,
            trust_level: DerivationTrustLevel::try_from(row.get::<_, i64>(9)?).map_err(
                |error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        9,
                        rusqlite::types::Type::Integer,
                        Box::new(error),
                    )
                },
            )?,
            provenance_cas_hash: row.get(10)?,
            reproducible: row.get(11)?,
        })
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
    /// The connection must already have the current `derivation_index` table.
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

        let result = stmt.query_row([derivation_id], DerivationRecord::from_row);

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
                record.build_duration_secs as i64,
                record.trust_level.as_i64(),
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

        let rows = stmt.query_map([name], DerivationRecord::from_row)?;

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
    pub fn set_trust_level(&self, derivation_id: &str, level: DerivationTrustLevel) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let stored = tx
            .query_row(
                "SELECT trust_level FROM derivation_index WHERE derivation_id = ?1",
                [derivation_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if let Some(stored) = stored {
            DerivationTrustLevel::try_from(stored).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Integer,
                    Box::new(error),
                )
            })?;
        }
        tx.execute(
            "UPDATE derivation_index SET trust_level = MAX(trust_level, ?2) WHERE derivation_id = ?1",
            rusqlite::params![derivation_id, level.as_i64()],
        )?;
        tx.commit()?;
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
mod tests;
