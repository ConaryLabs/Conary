// conary-core/src/db/models/repology_cache.rs

use rusqlite::{Connection, params};

/// A cached Repology project -> distro mapping.
#[derive(Debug, Clone)]
pub struct RepologyCacheEntry {
    pub project_name: String,
    pub distro: String,
    pub distro_name: String,
    pub version: Option<String>,
    pub status: Option<String>,
    pub fetched_at: String,
}

impl RepologyCacheEntry {
    pub fn insert_or_replace(
        conn: &Connection,
        entry: &RepologyCacheEntry,
    ) -> rusqlite::Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO repology_cache
             (project_name, distro, distro_name, version, status, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                entry.project_name,
                entry.distro,
                entry.distro_name,
                entry.version,
                entry.status,
                entry.fetched_at,
            ],
        )?;
        Ok(())
    }

    /// Read all cache entries.
    pub fn find_all(conn: &Connection) -> rusqlite::Result<Vec<RepologyCacheEntry>> {
        let mut stmt = conn.prepare(
            "SELECT project_name, distro, distro_name, version, status, fetched_at
             FROM repology_cache",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(RepologyCacheEntry {
                project_name: row.get(0)?,
                distro: row.get(1)?,
                distro_name: row.get(2)?,
                version: row.get(3)?,
                status: row.get(4)?,
                fetched_at: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    /// Clear all cache entries.
    pub fn clear_all(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute("DELETE FROM repology_cache", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::migrate;

    #[test]
    fn test_repology_cache_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let entry = RepologyCacheEntry {
            project_name: "python".into(),
            distro: "arch".into(),
            distro_name: "python".into(),
            version: Some("3.12.0".into()),
            status: Some("newest".into()),
            fetched_at: "2026-03-19T00:00:00Z".into(),
        };
        RepologyCacheEntry::insert_or_replace(&conn, &entry).unwrap();

        let all = RepologyCacheEntry::find_all(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].project_name, "python");
        assert_eq!(all[0].distro_name, "python");
    }
}
