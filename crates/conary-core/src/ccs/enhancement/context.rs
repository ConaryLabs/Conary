// conary-core/src/ccs/enhancement/context.rs
//! Enhancement context providing package data and database access

use super::EnhancementStatus;
use super::error::{EnhancementError, EnhancementResult};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct EnhancementPackageMetadata {
    pub name: String,
    pub version: String,
}

/// Context provided to enhancement engines during execution
///
/// This struct provides:
/// - Access to the database connection for reading/writing
/// - Package metadata from the conversion
/// - File information from the installed package
/// - Methods to store enhancement results
pub struct EnhancementContext<'a> {
    /// Database connection
    pub conn: &'a Connection,
    /// Trove ID being enhanced
    pub trove_id: i64,
    /// Converted package record ID
    pub converted_id: i64,
    /// Package metadata
    pub metadata: EnhancementPackageMetadata,
    /// Original package format (rpm, deb, arch)
    pub original_format: String,
    /// Original package checksum
    pub original_checksum: String,
}

impl<'a> EnhancementContext<'a> {
    /// Create a new enhancement context for a converted package
    pub fn new(conn: &'a Connection, trove_id: i64) -> EnhancementResult<Self> {
        // Load trove metadata
        let (name, version): (String, String) = conn
            .query_row(
                "SELECT name, version FROM troves WHERE id = ?1",
                [trove_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| EnhancementError::PackageNotFound(trove_id))?;

        // Load converted package info
        let (converted_id, original_format, original_checksum): (i64, String, String) = conn
            .query_row(
                "SELECT id, original_format, original_checksum
                 FROM converted_packages WHERE trove_id = ?1",
                [trove_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|_| EnhancementError::PackageNotFound(trove_id))?;

        let metadata = EnhancementPackageMetadata { name, version };

        Ok(Self {
            conn,
            trove_id,
            converted_id,
            metadata,
            original_format,
            original_checksum,
        })
    }

    /// Update enhancement status in the database
    pub fn set_status(&self, status: EnhancementStatus) -> EnhancementResult<()> {
        self.conn.execute(
            "UPDATE converted_packages SET enhancement_status = ?1 WHERE id = ?2",
            rusqlite::params![status.to_db_str(), self.converted_id],
        )?;
        Ok(())
    }

    /// Update enhancement status with error message
    pub fn set_status_with_error(
        &self,
        status: EnhancementStatus,
        error: &str,
    ) -> EnhancementResult<()> {
        self.conn.execute(
            "UPDATE converted_packages
             SET enhancement_status = ?1, enhancement_error = ?2, enhancement_attempted_at = CURRENT_TIMESTAMP
             WHERE id = ?3",
            rusqlite::params![status.to_db_str(), error, self.converted_id],
        )?;
        Ok(())
    }

    /// Store extracted provenance JSON
    pub fn store_extracted_provenance<T: Serialize>(
        &self,
        provenance: &T,
    ) -> EnhancementResult<()> {
        let json = serde_json::to_string(provenance)?;
        self.conn.execute(
            "UPDATE converted_packages SET extracted_provenance_json = ?1 WHERE id = ?2",
            rusqlite::params![json, self.converted_id],
        )?;
        Ok(())
    }

    /// Update enhancement version after successful enhancement
    pub fn set_enhancement_version(&self, version: i32) -> EnhancementResult<()> {
        self.conn.execute(
            "UPDATE converted_packages
             SET enhancement_version = ?1, enhancement_status = 'complete', enhancement_attempted_at = CURRENT_TIMESTAMP
             WHERE id = ?2",
            rusqlite::params![version, self.converted_id],
        )?;
        Ok(())
    }
}

/// Information about a converted package for enhancement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvertedPackageInfo {
    /// Database ID of the converted_packages record
    pub id: i64,
    /// Trove ID
    pub trove_id: i64,
    /// Package name
    pub name: String,
    /// Package version
    pub version: String,
    /// Original format (rpm, deb, arch)
    pub original_format: String,
    /// Current enhancement status
    pub enhancement_status: EnhancementStatus,
    /// Current enhancement version
    pub enhancement_version: i32,
}

impl ConvertedPackageInfo {
    /// Load all packages needing enhancement
    pub fn find_pending(conn: &Connection) -> EnhancementResult<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT cp.id, cp.trove_id, t.name, t.version, cp.original_format,
                    cp.enhancement_status, cp.enhancement_version
             FROM converted_packages cp
             JOIN troves t ON t.id = cp.trove_id
             WHERE cp.enhancement_status = 'pending'
             ORDER BY t.name",
        )?;

        let packages = stmt
            .query_map([], |row| {
                Ok(ConvertedPackageInfo {
                    id: row.get(0)?,
                    trove_id: row.get(1)?,
                    name: row.get(2)?,
                    version: row.get(3)?,
                    original_format: row.get(4)?,
                    enhancement_status: EnhancementStatus::from_db_str(&row.get::<_, String>(5)?),
                    enhancement_version: row.get(6)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(packages)
    }

    /// Load all packages with outdated enhancement version
    pub fn find_outdated(conn: &Connection, current_version: i32) -> EnhancementResult<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT cp.id, cp.trove_id, t.name, t.version, cp.original_format,
                    cp.enhancement_status, cp.enhancement_version
             FROM converted_packages cp
             JOIN troves t ON t.id = cp.trove_id
             WHERE cp.enhancement_version < ?1
             ORDER BY t.name",
        )?;

        let packages = stmt
            .query_map([current_version], |row| {
                Ok(ConvertedPackageInfo {
                    id: row.get(0)?,
                    trove_id: row.get(1)?,
                    name: row.get(2)?,
                    version: row.get(3)?,
                    original_format: row.get(4)?,
                    enhancement_status: EnhancementStatus::from_db_str(&row.get::<_, String>(5)?),
                    enhancement_version: row.get(6)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(packages)
    }

    /// Count packages by enhancement status
    pub fn count_by_status(conn: &Connection) -> EnhancementResult<EnhancementStats> {
        let mut stats = EnhancementStats::default();

        let mut stmt = conn.prepare(
            "SELECT enhancement_status, COUNT(*) FROM converted_packages GROUP BY enhancement_status",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;

        for row in rows.flatten() {
            match row.0.as_str() {
                "pending" => stats.pending = row.1 as usize,
                "in_progress" => stats.in_progress = row.1 as usize,
                "complete" => stats.complete = row.1 as usize,
                "failed" => stats.failed = row.1 as usize,
                "skipped" => stats.skipped = row.1 as usize,
                _ => {}
            }
        }

        stats.total =
            stats.pending + stats.in_progress + stats.complete + stats.failed + stats.skipped;
        Ok(stats)
    }
}

/// Statistics about enhancement status across all converted packages
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnhancementStats {
    /// Total converted packages
    pub total: usize,
    /// Packages pending enhancement
    pub pending: usize,
    /// Packages with enhancement in progress
    pub in_progress: usize,
    /// Packages with completed enhancement
    pub complete: usize,
    /// Packages with failed enhancement
    pub failed: usize,
    /// Packages with skipped enhancement
    pub skipped: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::create_test_db;

    #[test]
    fn test_enhancement_stats_default() {
        let stats = EnhancementStats::default();
        assert_eq!(stats.total, 0);
        assert_eq!(stats.pending, 0);
    }

    #[test]
    fn test_count_by_status_empty() {
        let (_temp, conn) = create_test_db();
        let stats = ConvertedPackageInfo::count_by_status(&conn).unwrap();
        assert_eq!(stats.total, 0);
        assert_eq!(stats.pending, 0);
        assert_eq!(stats.complete, 0);
    }
}
