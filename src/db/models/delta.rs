// src/db/models/delta.rs

//! Delta models - package deltas for efficient updates and bandwidth tracking

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};

/// Package delta information for efficient updates
#[derive(Debug, Clone)]
pub struct PackageDelta {
    pub id: Option<i64>,
    pub package_name: String,
    pub from_version: String,
    pub to_version: String,
    pub from_hash: String,
    pub to_hash: String,
    pub delta_url: String,
    pub delta_size: i64,
    pub delta_checksum: String,
    pub full_size: i64,
    pub compression_ratio: f64,
    pub created_at: Option<String>,
}

impl PackageDelta {
    /// Create a new PackageDelta
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        package_name: String,
        from_version: String,
        to_version: String,
        from_hash: String,
        to_hash: String,
        delta_url: String,
        delta_size: i64,
        delta_checksum: String,
        full_size: i64,
    ) -> Self {
        let compression_ratio = if full_size > 0 {
            delta_size as f64 / full_size as f64
        } else {
            1.0
        };

        Self {
            id: None,
            package_name,
            from_version,
            to_version,
            from_hash,
            to_hash,
            delta_url,
            delta_size,
            delta_checksum,
            full_size,
            compression_ratio,
            created_at: None,
        }
    }

    /// Insert this package delta into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO package_deltas
             (package_name, from_version, to_version, from_hash, to_hash, delta_url, delta_size, delta_checksum, full_size, compression_ratio)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                &self.package_name,
                &self.from_version,
                &self.to_version,
                &self.from_hash,
                &self.to_hash,
                &self.delta_url,
                &self.delta_size,
                &self.delta_checksum,
                &self.full_size,
                &self.compression_ratio,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find delta for a specific version transition
    pub fn find_delta(
        conn: &Connection,
        package_name: &str,
        from_version: &str,
        to_version: &str,
    ) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, package_name, from_version, to_version, from_hash, to_hash,
                    delta_url, delta_size, delta_checksum, full_size, compression_ratio, created_at
             FROM package_deltas
             WHERE package_name = ?1 AND from_version = ?2 AND to_version = ?3",
        )?;

        let delta = stmt
            .query_row([package_name, from_version, to_version], Self::from_row)
            .optional()?;

        Ok(delta)
    }

    /// Find all available deltas for a package
    pub fn find_by_package(conn: &Connection, package_name: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, package_name, from_version, to_version, from_hash, to_hash,
                    delta_url, delta_size, delta_checksum, full_size, compression_ratio, created_at
             FROM package_deltas
             WHERE package_name = ?1
             ORDER BY created_at DESC",
        )?;

        let deltas = stmt
            .query_map([package_name], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(deltas)
    }

    /// Delete a package delta
    pub fn delete(conn: &Connection, id: i64) -> Result<()> {
        conn.execute("DELETE FROM package_deltas WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Convert a database row to a PackageDelta
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            package_name: row.get(1)?,
            from_version: row.get(2)?,
            to_version: row.get(3)?,
            from_hash: row.get(4)?,
            to_hash: row.get(5)?,
            delta_url: row.get(6)?,
            delta_size: row.get(7)?,
            delta_checksum: row.get(8)?,
            full_size: row.get(9)?,
            compression_ratio: row.get(10)?,
            created_at: row.get(11)?,
        })
    }
}

/// Delta statistics for tracking bandwidth savings
#[derive(Debug, Clone)]
pub struct DeltaStats {
    pub id: Option<i64>,
    pub changeset_id: i64,
    pub total_bytes_saved: i64,
    pub deltas_applied: i32,
    pub full_downloads: i32,
    pub delta_failures: i32,
    pub created_at: Option<String>,
}

impl DeltaStats {
    /// Create new DeltaStats
    pub fn new(changeset_id: i64) -> Self {
        Self {
            id: None,
            changeset_id,
            total_bytes_saved: 0,
            deltas_applied: 0,
            full_downloads: 0,
            delta_failures: 0,
            created_at: None,
        }
    }

    /// Insert delta stats into the database
    pub fn insert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO delta_stats
             (changeset_id, total_bytes_saved, deltas_applied, full_downloads, delta_failures)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &self.changeset_id,
                &self.total_bytes_saved,
                &self.deltas_applied,
                &self.full_downloads,
                &self.delta_failures,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Find delta stats by changeset ID
    pub fn find_by_changeset(conn: &Connection, changeset_id: i64) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, changeset_id, total_bytes_saved, deltas_applied, full_downloads, delta_failures, created_at
             FROM delta_stats
             WHERE changeset_id = ?1",
        )?;

        let stats = stmt.query_row([changeset_id], Self::from_row).optional()?;

        Ok(stats)
    }

    /// Get aggregate statistics across all changesets
    pub fn get_total_stats(conn: &Connection) -> Result<Self> {
        let mut stmt = conn.prepare(
            "SELECT 0, 0,
                    SUM(total_bytes_saved),
                    SUM(deltas_applied),
                    SUM(full_downloads),
                    SUM(delta_failures),
                    NULL
             FROM delta_stats",
        )?;

        let stats = stmt.query_row([], Self::from_row)?;

        Ok(stats)
    }

    /// Convert a database row to DeltaStats
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: {
                let id: i64 = row.get(0)?;
                if id == 0 {
                    None
                } else {
                    Some(id)
                }
            },
            changeset_id: row.get(1)?,
            total_bytes_saved: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            deltas_applied: row.get::<_, Option<i32>>(3)?.unwrap_or(0),
            full_downloads: row.get::<_, Option<i32>>(4)?.unwrap_or(0),
            delta_failures: row.get::<_, Option<i32>>(5)?.unwrap_or(0),
            created_at: row.get(6)?,
        })
    }
}
