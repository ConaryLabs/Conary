// conary-core/src/db/models/appstream_cache.rs

use rusqlite::{Connection, params};

/// A cached AppStream component.
#[derive(Debug, Clone)]
pub struct AppstreamCacheEntry {
    pub appstream_id: String,
    pub pkgname: String,
    pub display_name: Option<String>,
    pub summary: Option<String>,
    pub distro: String,
    pub fetched_at: String,
}

impl AppstreamCacheEntry {
    pub fn insert_or_replace(
        conn: &Connection,
        entry: &AppstreamCacheEntry,
    ) -> rusqlite::Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO appstream_cache
             (appstream_id, pkgname, display_name, summary, distro, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                entry.appstream_id,
                entry.pkgname,
                entry.display_name,
                entry.summary,
                entry.distro,
                entry.fetched_at,
            ],
        )?;
        Ok(())
    }

    pub fn find_all(conn: &Connection) -> rusqlite::Result<Vec<AppstreamCacheEntry>> {
        let mut stmt = conn.prepare(
            "SELECT appstream_id, pkgname, display_name, summary, distro, fetched_at
             FROM appstream_cache",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AppstreamCacheEntry {
                appstream_id: row.get(0)?,
                pkgname: row.get(1)?,
                display_name: row.get(2)?,
                summary: row.get(3)?,
                distro: row.get(4)?,
                fetched_at: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    pub fn clear_all(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute("DELETE FROM appstream_cache", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::migrate;

    #[test]
    fn test_appstream_cache_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let entry = AppstreamCacheEntry {
            appstream_id: "org.mozilla.firefox".into(),
            pkgname: "firefox".into(),
            display_name: Some("Firefox".into()),
            summary: Some("Web Browser".into()),
            distro: "fedora".into(),
            fetched_at: "2026-03-19T00:00:00Z".into(),
        };
        AppstreamCacheEntry::insert_or_replace(&conn, &entry).unwrap();

        let all = AppstreamCacheEntry::find_all(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].pkgname, "firefox");
    }
}
