// src/db/models/remote_collection.rs

//! Remote collection cache model for model include resolution
//!
//! Caches collections fetched from Remi servers so that repeated
//! model-diff/apply operations don't re-fetch on every invocation.
//! Entries have a TTL (expires_at) and are refreshed when stale.

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};

/// Default cache TTL in seconds (1 hour)
pub const DEFAULT_CACHE_TTL_SECS: i64 = 3600;

/// A cached remote collection fetched from a Remi server
#[derive(Debug, Clone)]
pub struct RemoteCollection {
    pub id: Option<i64>,
    /// Collection name (e.g. "group-base-server")
    pub name: String,
    /// Label string used to fetch (e.g. "myrepo:stable")
    pub label: Option<String>,
    /// Version of the collection at fetch time
    pub version: Option<String>,
    /// SHA-256 content hash for integrity verification
    pub content_hash: String,
    /// Serialized CollectionData JSON
    pub data_json: String,
    /// When this entry was fetched (ISO 8601)
    pub fetched_at: Option<String>,
    /// When this cache entry expires (ISO 8601)
    pub expires_at: String,
    /// Repository this was fetched from
    pub repository_id: Option<i64>,
    /// Ed25519 signature bytes (if signed)
    pub signature: Option<Vec<u8>>,
    /// Hex-encoded signer key ID (first 8 bytes of public key)
    pub signer_key_id: Option<String>,
}

impl RemoteCollection {
    /// Create a new remote collection cache entry
    pub fn new(
        name: String,
        label: Option<String>,
        content_hash: String,
        data_json: String,
        expires_at: String,
    ) -> Self {
        Self {
            id: None,
            name,
            label,
            version: None,
            content_hash,
            data_json,
            fetched_at: None,
            expires_at,
            repository_id: None,
            signature: None,
            signer_key_id: None,
        }
    }

    /// Find a cached collection that hasn't expired
    ///
    /// Returns None if not cached or if the cache entry has expired.
    pub fn find_cached(
        conn: &Connection,
        name: &str,
        label: Option<&str>,
    ) -> Result<Option<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, label, version, content_hash, data_json,
                    fetched_at, expires_at, repository_id, signature, signer_key_id
             FROM remote_collections
             WHERE name = ?1 AND (label = ?2 OR (?2 IS NULL AND label IS NULL))
               AND expires_at > datetime('now')",
        )?;

        let entry = stmt
            .query_row(params![name, label], Self::from_row)
            .optional()?;

        Ok(entry)
    }

    /// Insert or update a cache entry (upsert on name+label)
    pub fn upsert(&mut self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO remote_collections
                (name, label, version, content_hash, data_json, fetched_at, expires_at,
                 repository_id, signature, signer_key_id)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'), ?6, ?7, ?8, ?9)
             ON CONFLICT(name, label) DO UPDATE SET
                version = excluded.version,
                content_hash = excluded.content_hash,
                data_json = excluded.data_json,
                fetched_at = datetime('now'),
                expires_at = excluded.expires_at,
                repository_id = excluded.repository_id,
                signature = excluded.signature,
                signer_key_id = excluded.signer_key_id",
            params![
                &self.name,
                &self.label,
                &self.version,
                &self.content_hash,
                &self.data_json,
                &self.expires_at,
                &self.repository_id,
                &self.signature,
                &self.signer_key_id,
            ],
        )?;

        let id = conn.last_insert_rowid();
        self.id = Some(id);
        Ok(id)
    }

    /// Remove all expired cache entries
    pub fn purge_expired(conn: &Connection) -> Result<usize> {
        let deleted = conn.execute(
            "DELETE FROM remote_collections WHERE expires_at <= datetime('now')",
            [],
        )?;
        Ok(deleted)
    }

    /// Remove all cache entries (force refresh)
    pub fn purge_all(conn: &Connection) -> Result<usize> {
        let deleted = conn.execute("DELETE FROM remote_collections", [])?;
        Ok(deleted)
    }

    /// Remove cache entries matching a specific name and optional label
    ///
    /// Used by `remote-diff --refresh` to force re-fetch of specific collections.
    pub fn purge_by_name(conn: &Connection, name: &str, label: Option<&str>) -> Result<usize> {
        let deleted = conn.execute(
            "DELETE FROM remote_collections
             WHERE name = ?1 AND (label = ?2 OR (?2 IS NULL AND label IS NULL))",
            params![name, label],
        )?;
        Ok(deleted)
    }

    /// Convert a database row to a RemoteCollection
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: Some(row.get(0)?),
            name: row.get(1)?,
            label: row.get(2)?,
            version: row.get(3)?,
            content_hash: row.get(4)?,
            data_json: row.get(5)?,
            fetched_at: row.get(6)?,
            expires_at: row.get(7)?,
            repository_id: row.get(8)?,
            signature: row.get(9)?,
            signer_key_id: row.get(10)?,
        })
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
    fn test_upsert_and_find_cached() {
        let (_temp, conn) = create_test_db();

        let mut entry = RemoteCollection::new(
            "group-base".to_string(),
            Some("myrepo:stable".to_string()),
            "sha256:abc123".to_string(),
            r#"{"name":"group-base","version":"1.0","members":[],"includes":[],"pins":{},"exclude":[],"content_hash":"sha256:abc123","published_at":"2026-01-01T00:00:00Z"}"#.to_string(),
            "2099-12-31T23:59:59".to_string(), // Far future so it won't expire
        );
        entry.version = Some("1.0".to_string());

        let id = entry.upsert(&conn).unwrap();
        assert!(id > 0);

        // Should find the cached entry
        let found = RemoteCollection::find_cached(&conn, "group-base", Some("myrepo:stable"))
            .unwrap();
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.name, "group-base");
        assert_eq!(found.content_hash, "sha256:abc123");
        assert_eq!(found.version, Some("1.0".to_string()));
    }

    #[test]
    fn test_expired_cache_not_returned() {
        let (_temp, conn) = create_test_db();

        let mut entry = RemoteCollection::new(
            "group-expired".to_string(),
            Some("repo:tag".to_string()),
            "sha256:def456".to_string(),
            "{}".to_string(),
            "2020-01-01T00:00:00".to_string(), // Already expired
        );

        entry.upsert(&conn).unwrap();

        // Should NOT find expired entry
        let found =
            RemoteCollection::find_cached(&conn, "group-expired", Some("repo:tag")).unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn test_purge_expired() {
        let (_temp, conn) = create_test_db();

        // Insert an expired entry
        let mut expired = RemoteCollection::new(
            "group-old".to_string(),
            None,
            "sha256:old".to_string(),
            "{}".to_string(),
            "2020-01-01T00:00:00".to_string(),
        );
        expired.upsert(&conn).unwrap();

        // Insert a fresh entry
        let mut fresh = RemoteCollection::new(
            "group-fresh".to_string(),
            None,
            "sha256:fresh".to_string(),
            "{}".to_string(),
            "2099-12-31T23:59:59".to_string(),
        );
        fresh.upsert(&conn).unwrap();

        // Purge expired
        let count = RemoteCollection::purge_expired(&conn).unwrap();
        assert_eq!(count, 1);

        // Fresh entry should still be there
        let found = RemoteCollection::find_cached(&conn, "group-fresh", None).unwrap();
        assert!(found.is_some());
    }

    #[test]
    fn test_upsert_updates_existing() {
        let (_temp, conn) = create_test_db();

        let mut entry = RemoteCollection::new(
            "group-update".to_string(),
            Some("repo:v1".to_string()),
            "sha256:first".to_string(),
            r#"{"version":"1"}"#.to_string(),
            "2099-12-31T23:59:59".to_string(),
        );
        entry.upsert(&conn).unwrap();

        // Update with new data
        let mut updated = RemoteCollection::new(
            "group-update".to_string(),
            Some("repo:v1".to_string()),
            "sha256:second".to_string(),
            r#"{"version":"2"}"#.to_string(),
            "2099-12-31T23:59:59".to_string(),
        );
        updated.upsert(&conn).unwrap();

        // Should get the updated entry
        let found =
            RemoteCollection::find_cached(&conn, "group-update", Some("repo:v1")).unwrap();
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.content_hash, "sha256:second");
    }

    #[test]
    fn test_find_cached_with_null_label() {
        let (_temp, conn) = create_test_db();

        let mut entry = RemoteCollection::new(
            "group-local".to_string(),
            None,
            "sha256:local".to_string(),
            "{}".to_string(),
            "2099-12-31T23:59:59".to_string(),
        );
        entry.upsert(&conn).unwrap();

        // Should find with None label
        let found = RemoteCollection::find_cached(&conn, "group-local", None).unwrap();
        assert!(found.is_some());

        // Should NOT find with a specific label
        let found =
            RemoteCollection::find_cached(&conn, "group-local", Some("repo:tag")).unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn test_purge_by_name_and_label() {
        let (_temp, conn) = create_test_db();

        // Insert two entries: same name, different labels
        let mut entry1 = RemoteCollection::new(
            "group-purge".to_string(),
            Some("repo:stable".to_string()),
            "sha256:stable".to_string(),
            "{}".to_string(),
            "2099-12-31T23:59:59".to_string(),
        );
        entry1.upsert(&conn).unwrap();

        let mut entry2 = RemoteCollection::new(
            "group-purge".to_string(),
            Some("repo:dev".to_string()),
            "sha256:dev".to_string(),
            "{}".to_string(),
            "2099-12-31T23:59:59".to_string(),
        );
        entry2.upsert(&conn).unwrap();

        // Purge only the stable label
        let deleted =
            RemoteCollection::purge_by_name(&conn, "group-purge", Some("repo:stable")).unwrap();
        assert_eq!(deleted, 1);

        // Stable should be gone
        let found =
            RemoteCollection::find_cached(&conn, "group-purge", Some("repo:stable")).unwrap();
        assert!(found.is_none());

        // Dev should still exist
        let found =
            RemoteCollection::find_cached(&conn, "group-purge", Some("repo:dev")).unwrap();
        assert!(found.is_some());
    }
}
