// src/db/models/converted.rs

//! Converted package tracking model
//!
//! Tracks packages converted from legacy formats (RPM/DEB/Arch) to CCS format.
//! This enables:
//! - Skip re-conversion of same package artifact (checksum-based dedup)
//! - Track conversion fidelity for debugging and user warnings
//! - Store detected hooks extracted from scriptlets
//! - Re-convert when conversion algorithm is upgraded

use crate::error::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};

/// Current conversion algorithm version
/// Bump this when making changes that require re-conversion of existing packages
pub const CONVERSION_VERSION: i32 = 1;

/// A converted package record
#[derive(Debug, Clone)]
pub struct ConvertedPackage {
    pub id: Option<i64>,
    /// Reference to the converted trove (CCS package that was installed)
    pub trove_id: Option<i64>,
    /// Original package format (rpm, deb, arch)
    pub original_format: String,
    /// Checksum of original package file (skip if already converted)
    pub original_checksum: String,
    /// Conversion algorithm version (re-convert if upgraded)
    pub conversion_version: i32,
    /// Fidelity level achieved (full, high, partial, low)
    pub conversion_fidelity: String,
    /// JSON of extracted hooks and fidelity details
    pub detected_hooks: Option<String>,
    /// When the conversion occurred
    pub converted_at: Option<String>,
}

impl ConvertedPackage {
    /// Create a new converted package record
    pub fn new(
        original_format: String,
        original_checksum: String,
        conversion_fidelity: String,
    ) -> Self {
        Self {
            id: None,
            trove_id: None,
            original_format,
            original_checksum,
            conversion_version: CONVERSION_VERSION,
            conversion_fidelity,
            detected_hooks: None,
            converted_at: None,
        }
    }

    /// Create from a database row
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            trove_id: row.get(1)?,
            original_format: row.get(2)?,
            original_checksum: row.get(3)?,
            conversion_version: row.get(4)?,
            conversion_fidelity: row.get(5)?,
            detected_hooks: row.get(6)?,
            converted_at: row.get(7)?,
        })
    }

    /// Insert this converted package into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO converted_packages (trove_id, original_format, original_checksum, conversion_version, conversion_fidelity, detected_hooks)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                self.trove_id,
                &self.original_format,
                &self.original_checksum,
                self.conversion_version,
                &self.conversion_fidelity,
                &self.detected_hooks,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Update the trove_id after the converted package is installed
    pub fn set_trove_id(&mut self, conn: &Connection, trove_id: i64) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::Error::NotFound("Cannot update trove_id on unconverted package".to_string())
        })?;

        conn.execute(
            "UPDATE converted_packages SET trove_id = ?1 WHERE id = ?2",
            params![trove_id, id],
        )?;

        self.trove_id = Some(trove_id);
        Ok(())
    }

    /// Find a converted package by its original checksum
    pub fn find_by_checksum(conn: &Connection, checksum: &str) -> Result<Option<Self>> {
        let result = conn
            .query_row(
                "SELECT id, trove_id, original_format, original_checksum, conversion_version, conversion_fidelity, detected_hooks, converted_at
                 FROM converted_packages WHERE original_checksum = ?1",
                [checksum],
                Self::from_row,
            )
            .optional()?;

        Ok(result)
    }

    /// Find a converted package by trove_id
    pub fn find_by_trove(conn: &Connection, trove_id: i64) -> Result<Option<Self>> {
        let result = conn
            .query_row(
                "SELECT id, trove_id, original_format, original_checksum, conversion_version, conversion_fidelity, detected_hooks, converted_at
                 FROM converted_packages WHERE trove_id = ?1",
                [trove_id],
                Self::from_row,
            )
            .optional()?;

        Ok(result)
    }

    /// Check if a package needs re-conversion (algorithm upgraded)
    pub fn needs_reconversion(&self) -> bool {
        self.conversion_version < CONVERSION_VERSION
    }

    /// List all converted packages with a specific fidelity level
    pub fn find_by_fidelity(conn: &Connection, fidelity: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, original_format, original_checksum, conversion_version, conversion_fidelity, detected_hooks, converted_at
             FROM converted_packages WHERE conversion_fidelity = ?1
             ORDER BY converted_at DESC",
        )?;

        let results = stmt
            .query_map([fidelity], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// List all converted packages
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, trove_id, original_format, original_checksum, conversion_version, conversion_fidelity, detected_hooks, converted_at
             FROM converted_packages ORDER BY converted_at DESC",
        )?;

        let results = stmt
            .query_map([], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Delete a converted package record by checksum
    pub fn delete_by_checksum(conn: &Connection, checksum: &str) -> Result<()> {
        conn.execute(
            "DELETE FROM converted_packages WHERE original_checksum = ?1",
            [checksum],
        )?;
        Ok(())
    }

    /// Count converted packages by format
    pub fn count_by_format(conn: &Connection) -> Result<Vec<(String, i64)>> {
        let mut stmt = conn.prepare(
            "SELECT original_format, COUNT(*) FROM converted_packages GROUP BY original_format ORDER BY COUNT(*) DESC",
        )?;

        let results = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    #[test]
    fn test_converted_package_crud() {
        let (_temp, conn) = create_test_db();

        // Create a converted package
        let mut converted = ConvertedPackage::new(
            "rpm".to_string(),
            "sha256:abc123def456".to_string(),
            "high".to_string(),
        );
        converted.detected_hooks = Some(r#"{"users": [{"name": "nginx"}]}"#.to_string());

        let id = converted.insert(&conn).unwrap();
        assert!(id > 0);

        // Find by checksum
        let found = ConvertedPackage::find_by_checksum(&conn, "sha256:abc123def456")
            .unwrap()
            .unwrap();
        assert_eq!(found.original_format, "rpm");
        assert_eq!(found.conversion_fidelity, "high");
        assert!(found.detected_hooks.is_some());

        // List all
        let all = ConvertedPackage::list_all(&conn).unwrap();
        assert_eq!(all.len(), 1);

        // Delete
        ConvertedPackage::delete_by_checksum(&conn, "sha256:abc123def456").unwrap();
        let deleted = ConvertedPackage::find_by_checksum(&conn, "sha256:abc123def456").unwrap();
        assert!(deleted.is_none());
    }

    #[test]
    fn test_needs_reconversion() {
        let mut converted = ConvertedPackage::new(
            "deb".to_string(),
            "sha256:test".to_string(),
            "full".to_string(),
        );
        converted.conversion_version = CONVERSION_VERSION;

        assert!(!converted.needs_reconversion());

        converted.conversion_version = CONVERSION_VERSION - 1;
        assert!(converted.needs_reconversion());
    }

    #[test]
    fn test_find_by_fidelity() {
        let (_temp, conn) = create_test_db();

        // Create multiple converted packages
        let mut high1 = ConvertedPackage::new("rpm".to_string(), "sha256:111".to_string(), "high".to_string());
        high1.insert(&conn).unwrap();

        let mut high2 = ConvertedPackage::new("deb".to_string(), "sha256:222".to_string(), "high".to_string());
        high2.insert(&conn).unwrap();

        let mut low1 = ConvertedPackage::new("arch".to_string(), "sha256:333".to_string(), "low".to_string());
        low1.insert(&conn).unwrap();

        // Find by fidelity
        let high = ConvertedPackage::find_by_fidelity(&conn, "high").unwrap();
        assert_eq!(high.len(), 2);

        let low = ConvertedPackage::find_by_fidelity(&conn, "low").unwrap();
        assert_eq!(low.len(), 1);
    }

    #[test]
    fn test_count_by_format() {
        let (_temp, conn) = create_test_db();

        // Create converted packages with different formats
        let mut rpm1 = ConvertedPackage::new("rpm".to_string(), "sha256:r1".to_string(), "high".to_string());
        rpm1.insert(&conn).unwrap();

        let mut rpm2 = ConvertedPackage::new("rpm".to_string(), "sha256:r2".to_string(), "high".to_string());
        rpm2.insert(&conn).unwrap();

        let mut deb1 = ConvertedPackage::new("deb".to_string(), "sha256:d1".to_string(), "high".to_string());
        deb1.insert(&conn).unwrap();

        // Count by format
        let counts = ConvertedPackage::count_by_format(&conn).unwrap();
        assert_eq!(counts.len(), 2);

        // RPM should be first (most common)
        assert_eq!(counts[0].0, "rpm");
        assert_eq!(counts[0].1, 2);
        assert_eq!(counts[1].0, "deb");
        assert_eq!(counts[1].1, 1);
    }

    #[test]
    fn test_unique_checksum_constraint() {
        let (_temp, conn) = create_test_db();

        let mut converted1 = ConvertedPackage::new(
            "rpm".to_string(),
            "sha256:same_checksum".to_string(),
            "high".to_string(),
        );
        converted1.insert(&conn).unwrap();

        // Try to insert with same checksum - should fail
        let mut converted2 = ConvertedPackage::new(
            "deb".to_string(),
            "sha256:same_checksum".to_string(),
            "full".to_string(),
        );
        let result = converted2.insert(&conn);
        assert!(result.is_err());
    }
}
