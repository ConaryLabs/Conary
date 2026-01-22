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

    // Enhancement fields (v36)
    /// Enhancement algorithm version (0 = not enhanced yet)
    pub enhancement_version: i32,
    /// Raw inferred capabilities JSON (for audit trail)
    pub inferred_caps_json: Option<String>,
    /// Extracted provenance JSON (before DB insertion)
    pub extracted_provenance_json: Option<String>,
    /// Enhancement status: pending, in_progress, complete, failed, skipped
    pub enhancement_status: String,
    /// Error message if enhancement failed
    pub enhancement_error: Option<String>,
    /// When enhancement was last attempted
    pub enhancement_attempted_at: Option<String>,

    // Server-side conversion tracking fields (v38)
    /// Package name (for server-side lookups)
    pub package_name: Option<String>,
    /// Package version (for server-side lookups)
    pub package_version: Option<String>,
    /// Distribution (fedora, arch, ubuntu, debian)
    pub distro: Option<String>,
    /// JSON array of chunk hashes
    pub chunk_hashes_json: Option<String>,
    /// Total size of the CCS package
    pub total_size: Option<i64>,
    /// Content hash of the CCS package
    pub content_hash: Option<String>,
    /// Path to the CCS package file
    pub ccs_path: Option<String>,
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
            // Enhancement starts as pending with version 0
            enhancement_version: 0,
            inferred_caps_json: None,
            extracted_provenance_json: None,
            enhancement_status: "pending".to_string(),
            enhancement_error: None,
            enhancement_attempted_at: None,
            // Server-side fields start as None
            package_name: None,
            package_version: None,
            distro: None,
            chunk_hashes_json: None,
            total_size: None,
            content_hash: None,
            ccs_path: None,
        }
    }

    /// Create a new server-side converted package record (for Remi)
    pub fn new_server(
        distro: String,
        package_name: String,
        package_version: String,
        original_format: String,
        original_checksum: String,
        conversion_fidelity: String,
        chunk_hashes: &[String],
        total_size: i64,
        content_hash: String,
        ccs_path: String,
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
            enhancement_version: 0,
            inferred_caps_json: None,
            extracted_provenance_json: None,
            enhancement_status: "pending".to_string(),
            enhancement_error: None,
            enhancement_attempted_at: None,
            package_name: Some(package_name),
            package_version: Some(package_version),
            distro: Some(distro),
            chunk_hashes_json: Some(serde_json::to_string(chunk_hashes).unwrap_or_default()),
            total_size: Some(total_size),
            content_hash: Some(content_hash),
            ccs_path: Some(ccs_path),
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
            // Enhancement fields (v36)
            enhancement_version: row.get(8).unwrap_or(0),
            inferred_caps_json: row.get(9).ok(),
            extracted_provenance_json: row.get(10).ok(),
            enhancement_status: row.get(11).unwrap_or_else(|_| "pending".to_string()),
            enhancement_error: row.get(12).ok(),
            enhancement_attempted_at: row.get(13).ok(),
            // Server-side fields (v38)
            package_name: row.get(14).ok(),
            package_version: row.get(15).ok(),
            distro: row.get(16).ok(),
            chunk_hashes_json: row.get(17).ok(),
            total_size: row.get(18).ok(),
            content_hash: row.get(19).ok(),
            ccs_path: row.get(20).ok(),
        })
    }

    /// Insert this converted package into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO converted_packages (trove_id, original_format, original_checksum, conversion_version, conversion_fidelity, detected_hooks,
                enhancement_version, inferred_caps_json, extracted_provenance_json, enhancement_status,
                package_name, package_version, distro, chunk_hashes_json, total_size, content_hash, ccs_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                self.trove_id,
                &self.original_format,
                &self.original_checksum,
                self.conversion_version,
                &self.conversion_fidelity,
                &self.detected_hooks,
                self.enhancement_version,
                &self.inferred_caps_json,
                &self.extracted_provenance_json,
                &self.enhancement_status,
                &self.package_name,
                &self.package_version,
                &self.distro,
                &self.chunk_hashes_json,
                self.total_size,
                &self.content_hash,
                &self.ccs_path,
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
                "SELECT id, trove_id, original_format, original_checksum, conversion_version, conversion_fidelity, detected_hooks, converted_at,
                        enhancement_version, inferred_caps_json, extracted_provenance_json, enhancement_status, enhancement_error, enhancement_attempted_at,
                        package_name, package_version, distro, chunk_hashes_json, total_size, content_hash, ccs_path
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
                "SELECT id, trove_id, original_format, original_checksum, conversion_version, conversion_fidelity, detected_hooks, converted_at,
                        enhancement_version, inferred_caps_json, extracted_provenance_json, enhancement_status, enhancement_error, enhancement_attempted_at,
                        package_name, package_version, distro, chunk_hashes_json, total_size, content_hash, ccs_path
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
            "SELECT id, trove_id, original_format, original_checksum, conversion_version, conversion_fidelity, detected_hooks, converted_at,
                    enhancement_version, inferred_caps_json, extracted_provenance_json, enhancement_status, enhancement_error, enhancement_attempted_at,
                    package_name, package_version, distro, chunk_hashes_json, total_size, content_hash, ccs_path
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
            "SELECT id, trove_id, original_format, original_checksum, conversion_version, conversion_fidelity, detected_hooks, converted_at,
                    enhancement_version, inferred_caps_json, extracted_provenance_json, enhancement_status, enhancement_error, enhancement_attempted_at,
                    package_name, package_version, distro, chunk_hashes_json, total_size, content_hash, ccs_path
             FROM converted_packages ORDER BY converted_at DESC",
        )?;

        let results = stmt
            .query_map([], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Find a converted package by distro, name, and version (server-side lookup)
    pub fn find_by_package_identity(
        conn: &Connection,
        distro: &str,
        name: &str,
        version: Option<&str>,
    ) -> Result<Option<Self>> {
        let result = if let Some(ver) = version {
            conn.query_row(
                "SELECT id, trove_id, original_format, original_checksum, conversion_version, conversion_fidelity, detected_hooks, converted_at,
                        enhancement_version, inferred_caps_json, extracted_provenance_json, enhancement_status, enhancement_error, enhancement_attempted_at,
                        package_name, package_version, distro, chunk_hashes_json, total_size, content_hash, ccs_path
                 FROM converted_packages
                 WHERE distro = ?1 AND package_name = ?2 AND package_version = ?3",
                params![distro, name, ver],
                Self::from_row,
            )
            .optional()?
        } else {
            // Find latest version for this package
            conn.query_row(
                "SELECT id, trove_id, original_format, original_checksum, conversion_version, conversion_fidelity, detected_hooks, converted_at,
                        enhancement_version, inferred_caps_json, extracted_provenance_json, enhancement_status, enhancement_error, enhancement_attempted_at,
                        package_name, package_version, distro, chunk_hashes_json, total_size, content_hash, ccs_path
                 FROM converted_packages
                 WHERE distro = ?1 AND package_name = ?2
                 ORDER BY converted_at DESC LIMIT 1",
                params![distro, name],
                Self::from_row,
            )
            .optional()?
        };

        Ok(result)
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

    // Enhancement-related methods (v36)

    /// Update enhancement status for this package
    pub fn update_enhancement_status(
        &mut self,
        conn: &Connection,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::Error::NotFound("Cannot update enhancement status on unsaved package".to_string())
        })?;

        conn.execute(
            "UPDATE converted_packages SET enhancement_status = ?1, enhancement_error = ?2, enhancement_attempted_at = datetime('now') WHERE id = ?3",
            rusqlite::params![status, error, id],
        )?;

        self.enhancement_status = status.to_string();
        self.enhancement_error = error.map(|s| s.to_string());
        Ok(())
    }

    /// Mark enhancement as complete with results
    pub fn set_enhancement_complete(
        &mut self,
        conn: &Connection,
        version: i32,
        inferred_caps: Option<&str>,
        extracted_provenance: Option<&str>,
    ) -> Result<()> {
        let id = self.id.ok_or_else(|| {
            crate::Error::NotFound("Cannot update enhancement on unsaved package".to_string())
        })?;

        conn.execute(
            "UPDATE converted_packages SET
                enhancement_version = ?1,
                inferred_caps_json = ?2,
                extracted_provenance_json = ?3,
                enhancement_status = 'complete',
                enhancement_error = NULL,
                enhancement_attempted_at = datetime('now')
             WHERE id = ?4",
            rusqlite::params![version, inferred_caps, extracted_provenance, id],
        )?;

        self.enhancement_version = version;
        self.inferred_caps_json = inferred_caps.map(|s| s.to_string());
        self.extracted_provenance_json = extracted_provenance.map(|s| s.to_string());
        self.enhancement_status = "complete".to_string();
        self.enhancement_error = None;
        Ok(())
    }

    /// Mark enhancement as failed with error message
    pub fn set_enhancement_failed(&mut self, conn: &Connection, error: &str) -> Result<()> {
        self.update_enhancement_status(conn, "failed", Some(error))
    }

    /// Check if this package needs enhancement
    pub fn needs_enhancement(&self, current_version: i32) -> bool {
        self.enhancement_status == "pending"
            || (self.enhancement_status == "complete" && self.enhancement_version < current_version)
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

    #[test]
    fn test_enhancement_methods() {
        let (_temp, conn) = create_test_db();

        // Create and insert a converted package
        let mut converted = ConvertedPackage::new(
            "rpm".to_string(),
            "sha256:enhance_test".to_string(),
            "high".to_string(),
        );
        converted.insert(&conn).unwrap();

        // Check initial enhancement state
        assert_eq!(converted.enhancement_status, "pending");
        assert_eq!(converted.enhancement_version, 0);
        assert!(converted.needs_enhancement(1));

        // Mark as complete
        converted
            .set_enhancement_complete(&conn, 1, Some(r#"{"network": true}"#), None)
            .unwrap();
        assert_eq!(converted.enhancement_status, "complete");
        assert_eq!(converted.enhancement_version, 1);
        assert!(!converted.needs_enhancement(1));
        assert!(converted.needs_enhancement(2)); // outdated

        // Verify persisted in database
        let found = ConvertedPackage::find_by_checksum(&conn, "sha256:enhance_test")
            .unwrap()
            .unwrap();
        assert_eq!(found.enhancement_status, "complete");
        assert_eq!(found.enhancement_version, 1);
        assert!(found.inferred_caps_json.is_some());
    }

    #[test]
    fn test_enhancement_failure() {
        let (_temp, conn) = create_test_db();

        let mut converted = ConvertedPackage::new(
            "deb".to_string(),
            "sha256:fail_test".to_string(),
            "partial".to_string(),
        );
        converted.insert(&conn).unwrap();

        // Mark as failed
        converted
            .set_enhancement_failed(&conn, "Test error message")
            .unwrap();
        assert_eq!(converted.enhancement_status, "failed");
        assert_eq!(converted.enhancement_error.as_deref(), Some("Test error message"));

        // Verify persisted
        let found = ConvertedPackage::find_by_checksum(&conn, "sha256:fail_test")
            .unwrap()
            .unwrap();
        assert_eq!(found.enhancement_status, "failed");
        assert!(found.enhancement_error.is_some());
    }
}
