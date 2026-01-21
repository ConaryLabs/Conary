// src/ccs/enhancement/context.rs
//! Enhancement context providing package data and database access

use super::error::{EnhancementError, EnhancementResult};
use super::EnhancementStatus;
use crate::capability::inference::{InferredCapabilities, PackageFile, PackageMetadataRef};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    pub metadata: PackageMetadataRef,
    /// Package files (loaded lazily)
    files: Option<Vec<PackageFile>>,
    /// Root path where package files are installed
    pub install_root: PathBuf,
    /// Original package format (rpm, deb, arch)
    pub original_format: String,
    /// Original package checksum
    pub original_checksum: String,
}

impl<'a> EnhancementContext<'a> {
    /// Create a new enhancement context for a converted package
    pub fn new(
        conn: &'a Connection,
        trove_id: i64,
        install_root: PathBuf,
    ) -> EnhancementResult<Self> {
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

        // Load dependencies
        let mut dep_stmt = conn.prepare(
            "SELECT depends_on_name FROM dependencies WHERE trove_id = ?1",
        )?;
        let dependencies: Vec<String> = dep_stmt
            .query_map([trove_id], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        let metadata = PackageMetadataRef {
            name,
            version,
            dependencies,
            ..Default::default()
        };

        Ok(Self {
            conn,
            trove_id,
            converted_id,
            metadata,
            files: None,
            install_root,
            original_format,
            original_checksum,
        })
    }

    /// Get package files, loading them if not already loaded
    pub fn get_files(&mut self) -> EnhancementResult<&[PackageFile]> {
        if self.files.is_none() {
            self.load_files()?;
        }
        Ok(self.files.as_ref().unwrap())
    }

    /// Load package files from the database
    fn load_files(&mut self) -> EnhancementResult<()> {
        let mut stmt = self.conn.prepare(
            "SELECT path, sha256_hash, size, permissions FROM files WHERE trove_id = ?1",
        )?;

        let files: Vec<PackageFile> = stmt
            .query_map([self.trove_id], |row| {
                let path: String = row.get(0)?;
                let hash: String = row.get(1)?;
                let size: i64 = row.get(2)?;
                let permissions: i32 = row.get(3)?;

                let is_executable = path.starts_with("/usr/bin")
                    || path.starts_with("/usr/sbin")
                    || path.starts_with("/bin")
                    || path.starts_with("/sbin")
                    || (permissions & 0o111) != 0;

                Ok(PackageFile {
                    path,
                    content_hash: Some(hash),
                    size: size as u64,
                    mode: permissions as u32,
                    is_executable,
                    content: None, // Content loaded lazily if needed
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        self.files = Some(files);
        Ok(())
    }

    /// Load file content for binary analysis
    ///
    /// This reads the actual file content from the filesystem for files
    /// that need binary analysis.
    pub fn load_file_contents(&mut self) -> EnhancementResult<()> {
        let files = self.get_files()?.to_vec();
        let mut files_with_content = Vec::with_capacity(files.len());

        for mut file in files {
            let full_path = self.install_root.join(file.path.trim_start_matches('/'));
            if full_path.exists() {
                if let Ok(content) = std::fs::read(&full_path) {
                    file.content = Some(content);
                }
            }
            files_with_content.push(file);
        }

        self.files = Some(files_with_content);
        Ok(())
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

    /// Store inferred capabilities JSON
    pub fn store_inferred_capabilities(
        &self,
        inferred: &InferredCapabilities,
    ) -> EnhancementResult<()> {
        let json = serde_json::to_string(inferred)?;
        self.conn.execute(
            "UPDATE converted_packages SET inferred_caps_json = ?1 WHERE id = ?2",
            rusqlite::params![json, self.converted_id],
        )?;
        Ok(())
    }

    /// Store extracted provenance JSON
    pub fn store_extracted_provenance<T: Serialize>(&self, provenance: &T) -> EnhancementResult<()> {
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

    /// Store a subpackage relationship
    pub fn store_subpackage_relationship(
        &self,
        base_package: &str,
        component_type: &str,
    ) -> EnhancementResult<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO subpackage_relationships
             (base_package, subpackage_name, component_type, source_format)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                base_package,
                &self.metadata.name,
                component_type,
                &self.original_format
            ],
        )?;
        Ok(())
    }

    /// Add an implicit dependency on a package (used for subpackage->base dependencies)
    ///
    /// This adds a runtime dependency without a version constraint.
    /// The kind is set to 'implicit' to distinguish from declared dependencies.
    pub fn add_implicit_dependency(&self, depends_on: &str) -> EnhancementResult<()> {
        // Check if dependency already exists
        let exists: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM dependencies WHERE trove_id = ?1 AND depends_on_name = ?2)",
                rusqlite::params![self.trove_id, depends_on],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !exists {
            // Use kind='implicit' to mark auto-generated dependencies
            self.conn.execute(
                "INSERT INTO dependencies (trove_id, depends_on_name, dependency_type, kind)
                 VALUES (?1, ?2, 'runtime', 'implicit')",
                rusqlite::params![self.trove_id, depends_on],
            )?;
            tracing::debug!(
                "Added implicit dependency: {} -> {}",
                self.metadata.name,
                depends_on
            );
        }
        Ok(())
    }

    /// Add a virtual provide for this package
    ///
    /// Virtual provides allow packages to be requested by alternate names.
    /// For subpackages, this enables requesting "base:component" syntax.
    pub fn add_virtual_provide(&self, capability: &str) -> EnhancementResult<()> {
        // Check if provide already exists
        let exists: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM provides WHERE trove_id = ?1 AND capability = ?2)",
                rusqlite::params![self.trove_id, capability],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !exists {
            // Use kind='virtual' to mark auto-generated provides
            self.conn.execute(
                "INSERT INTO provides (trove_id, capability, kind)
                 VALUES (?1, ?2, 'virtual')",
                rusqlite::params![self.trove_id, capability],
            )?;
            tracing::debug!(
                "Added virtual provide: {} provides {}",
                self.metadata.name,
                capability
            );
        }
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
                    enhancement_status: EnhancementStatus::from_db_str(
                        &row.get::<_, String>(5)?,
                    ),
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
                    enhancement_status: EnhancementStatus::from_db_str(
                        &row.get::<_, String>(5)?,
                    ),
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

        stats.total = stats.pending + stats.in_progress + stats.complete + stats.failed + stats.skipped;
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
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        crate::db::schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

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
