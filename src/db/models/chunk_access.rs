// src/db/models/chunk_access.rs

//! Chunk access tracking model for LRU cache management
//!
//! Tracks chunk access patterns to enable smarter cache eviction decisions.
//! This provides persistent metadata that complements filesystem-based LRU tracking.

use crate::error::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};

/// A chunk access record
#[derive(Debug, Clone)]
pub struct ChunkAccess {
    /// SHA-256 hash of the chunk (primary key)
    pub hash: String,
    /// Size of the chunk in bytes
    pub size_bytes: i64,
    /// Number of times this chunk has been accessed
    pub access_count: i64,
    /// When this chunk was first stored
    pub created_at: Option<String>,
    /// When this chunk was last accessed
    pub last_accessed: Option<String>,
    /// Which packages reference this chunk (JSON array)
    pub referenced_by: Option<String>,
    /// Whether this chunk is protected from eviction
    pub protected: bool,
}

impl ChunkAccess {
    /// Create a new chunk access record
    pub fn new(hash: String, size_bytes: i64) -> Self {
        Self {
            hash,
            size_bytes,
            access_count: 1,
            created_at: None,
            last_accessed: None,
            referenced_by: None,
            protected: false,
        }
    }

    /// Create from a database row
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            hash: row.get(0)?,
            size_bytes: row.get(1)?,
            access_count: row.get(2)?,
            created_at: row.get(3)?,
            last_accessed: row.get(4)?,
            referenced_by: row.get(5)?,
            protected: row.get::<_, i32>(6)? != 0,
        })
    }

    /// Insert or update this chunk access record (upsert)
    pub fn upsert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT INTO chunk_access (hash, size_bytes, access_count, referenced_by, protected)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(hash) DO UPDATE SET
                access_count = access_count + 1,
                last_accessed = CURRENT_TIMESTAMP",
            params![
                &self.hash,
                self.size_bytes,
                self.access_count,
                &self.referenced_by,
                if self.protected { 1 } else { 0 },
            ],
        )?;
        Ok(())
    }

    /// Record an access to this chunk (increment count, update timestamp)
    pub fn record_access(conn: &Connection, hash: &str) -> Result<()> {
        conn.execute(
            "UPDATE chunk_access SET access_count = access_count + 1, last_accessed = CURRENT_TIMESTAMP WHERE hash = ?1",
            [hash],
        )?;
        Ok(())
    }

    /// Find a chunk by hash
    pub fn find_by_hash(conn: &Connection, hash: &str) -> Result<Option<Self>> {
        let result = conn
            .query_row(
                "SELECT hash, size_bytes, access_count, created_at, last_accessed, referenced_by, protected
                 FROM chunk_access WHERE hash = ?1",
                [hash],
                Self::from_row,
            )
            .optional()?;

        Ok(result)
    }

    /// Get least recently used chunks (for eviction)
    pub fn get_lru_chunks(conn: &Connection, limit: usize) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT hash, size_bytes, access_count, created_at, last_accessed, referenced_by, protected
             FROM chunk_access
             WHERE protected = 0
             ORDER BY last_accessed ASC
             LIMIT ?1",
        )?;

        let results = stmt
            .query_map([limit as i64], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get chunks older than a given timestamp
    pub fn get_stale_chunks(conn: &Connection, before: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT hash, size_bytes, access_count, created_at, last_accessed, referenced_by, protected
             FROM chunk_access
             WHERE protected = 0 AND last_accessed < ?1
             ORDER BY last_accessed ASC",
        )?;

        let results = stmt
            .query_map([before], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Set protection status for a chunk
    pub fn set_protected(conn: &Connection, hash: &str, protected: bool) -> Result<()> {
        conn.execute(
            "UPDATE chunk_access SET protected = ?1 WHERE hash = ?2",
            params![if protected { 1 } else { 0 }, hash],
        )?;
        Ok(())
    }

    /// Set protection status for multiple chunks
    pub fn protect_chunks(conn: &Connection, hashes: &[String]) -> Result<()> {
        for hash in hashes {
            Self::set_protected(conn, hash, true)?;
        }
        Ok(())
    }

    /// Remove protection from multiple chunks
    pub fn unprotect_chunks(conn: &Connection, hashes: &[String]) -> Result<()> {
        for hash in hashes {
            Self::set_protected(conn, hash, false)?;
        }
        Ok(())
    }

    /// Delete a chunk record
    pub fn delete(conn: &Connection, hash: &str) -> Result<()> {
        conn.execute("DELETE FROM chunk_access WHERE hash = ?1", [hash])?;
        Ok(())
    }

    /// Get cache statistics
    pub fn get_stats(conn: &Connection) -> Result<ChunkStats> {
        let (total_chunks, total_bytes, total_accesses): (i64, i64, i64) = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0), COALESCE(SUM(access_count), 0)
             FROM chunk_access",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;

        let protected_chunks: i64 = conn.query_row(
            "SELECT COUNT(*) FROM chunk_access WHERE protected = 1",
            [],
            |row| row.get(0),
        )?;

        Ok(ChunkStats {
            total_chunks: total_chunks as usize,
            total_bytes: total_bytes as u64,
            total_accesses: total_accesses as u64,
            protected_chunks: protected_chunks as usize,
        })
    }

    /// Get most popular chunks
    pub fn get_popular_chunks(conn: &Connection, limit: usize) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT hash, size_bytes, access_count, created_at, last_accessed, referenced_by, protected
             FROM chunk_access
             ORDER BY access_count DESC
             LIMIT ?1",
        )?;

        let results = stmt
            .query_map([limit as i64], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get largest chunks
    pub fn get_largest_chunks(conn: &Connection, limit: usize) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT hash, size_bytes, access_count, created_at, last_accessed, referenced_by, protected
             FROM chunk_access
             ORDER BY size_bytes DESC
             LIMIT ?1",
        )?;

        let results = stmt
            .query_map([limit as i64], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }
}

/// Chunk cache statistics
#[derive(Debug, Clone)]
pub struct ChunkStats {
    /// Total number of tracked chunks
    pub total_chunks: usize,
    /// Total bytes of all chunks
    pub total_bytes: u64,
    /// Total access count across all chunks
    pub total_accesses: u64,
    /// Number of protected chunks
    pub protected_chunks: usize,
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
    fn test_chunk_access_upsert() {
        let (_temp, conn) = create_test_db();

        let chunk = ChunkAccess::new("abc123def456".to_string(), 1024);
        chunk.upsert(&conn).unwrap();

        // Find it
        let found = ChunkAccess::find_by_hash(&conn, "abc123def456").unwrap().unwrap();
        assert_eq!(found.size_bytes, 1024);
        assert_eq!(found.access_count, 1);

        // Upsert again - should increment count
        chunk.upsert(&conn).unwrap();
        let found2 = ChunkAccess::find_by_hash(&conn, "abc123def456").unwrap().unwrap();
        assert_eq!(found2.access_count, 2);
    }

    #[test]
    fn test_chunk_access_record() {
        let (_temp, conn) = create_test_db();

        let chunk = ChunkAccess::new("abc123".to_string(), 512);
        chunk.upsert(&conn).unwrap();

        // Record access
        ChunkAccess::record_access(&conn, "abc123").unwrap();
        let found = ChunkAccess::find_by_hash(&conn, "abc123").unwrap().unwrap();
        assert_eq!(found.access_count, 2);
    }

    #[test]
    fn test_chunk_protection() {
        let (_temp, conn) = create_test_db();

        let chunk = ChunkAccess::new("protected_chunk".to_string(), 2048);
        chunk.upsert(&conn).unwrap();

        // Set protected
        ChunkAccess::set_protected(&conn, "protected_chunk", true).unwrap();
        let found = ChunkAccess::find_by_hash(&conn, "protected_chunk").unwrap().unwrap();
        assert!(found.protected);

        // LRU query should not return protected chunks
        let lru = ChunkAccess::get_lru_chunks(&conn, 10).unwrap();
        assert!(lru.is_empty());

        // Unprotect
        ChunkAccess::set_protected(&conn, "protected_chunk", false).unwrap();
        let lru2 = ChunkAccess::get_lru_chunks(&conn, 10).unwrap();
        assert_eq!(lru2.len(), 1);
    }

    #[test]
    fn test_chunk_stats() {
        let (_temp, conn) = create_test_db();

        ChunkAccess::new("chunk1".to_string(), 1000).upsert(&conn).unwrap();
        ChunkAccess::new("chunk2".to_string(), 2000).upsert(&conn).unwrap();
        ChunkAccess::new("chunk3".to_string(), 3000).upsert(&conn).unwrap();

        let stats = ChunkAccess::get_stats(&conn).unwrap();
        assert_eq!(stats.total_chunks, 3);
        assert_eq!(stats.total_bytes, 6000);
        assert_eq!(stats.total_accesses, 3);
    }

    #[test]
    fn test_popular_chunks() {
        let (_temp, conn) = create_test_db();

        let chunk1 = ChunkAccess::new("popular".to_string(), 1000);
        chunk1.upsert(&conn).unwrap();
        chunk1.upsert(&conn).unwrap(); // access_count = 2
        chunk1.upsert(&conn).unwrap(); // access_count = 3

        let chunk2 = ChunkAccess::new("unpopular".to_string(), 1000);
        chunk2.upsert(&conn).unwrap(); // access_count = 1

        let popular = ChunkAccess::get_popular_chunks(&conn, 10).unwrap();
        assert_eq!(popular.len(), 2);
        assert_eq!(popular[0].hash, "popular");
        assert_eq!(popular[0].access_count, 3);
    }

    #[test]
    fn test_delete_chunk() {
        let (_temp, conn) = create_test_db();

        ChunkAccess::new("to_delete".to_string(), 512).upsert(&conn).unwrap();
        assert!(ChunkAccess::find_by_hash(&conn, "to_delete").unwrap().is_some());

        ChunkAccess::delete(&conn, "to_delete").unwrap();
        assert!(ChunkAccess::find_by_hash(&conn, "to_delete").unwrap().is_none());
    }
}
