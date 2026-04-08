// conary-core/src/db/models/repology_cache.rs

use crate::error::Result;
use rusqlite::{Connection, params};
use rusqlite::{ToSql, params_from_iter};

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

    /// Read cache entries for a canonical package restricted to a distro set.
    pub fn find_for_canonical_and_distros(
        conn: &Connection,
        canonical_id: i64,
        distros: &[String],
    ) -> Result<Vec<RepologyCacheEntry>> {
        if distros.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders = (0..distros.len())
            .map(|idx| format!("?{}", idx + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT rc.project_name, rc.distro, rc.distro_name, rc.version, rc.status, rc.fetched_at
             FROM repology_cache rc
             JOIN canonical_packages cp ON cp.name = rc.project_name
             WHERE cp.id = ?1 AND rc.distro IN ({})
             ORDER BY rc.distro",
            placeholders
        );

        let mut stmt = conn.prepare(&sql)?;
        let mut params: Vec<&dyn ToSql> = Vec::with_capacity(distros.len() + 1);
        params.push(&canonical_id);
        for distro in distros {
            params.push(distro);
        }

        let rows = stmt.query_map(params_from_iter(params), |row| {
            Ok(RepologyCacheEntry {
                project_name: row.get(0)?,
                distro: row.get(1)?,
                distro_name: row.get(2)?,
                version: row.get(3)?,
                status: row.get(4)?,
                fetched_at: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
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
    use crate::db::models::CanonicalPackage;
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

    #[test]
    fn test_find_for_canonical_and_distros() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let mut canonical = CanonicalPackage::new("python".into(), "package".into());
        let canonical_id = canonical.insert(&conn).unwrap();

        RepologyCacheEntry::insert_or_replace(
            &conn,
            &RepologyCacheEntry {
                project_name: "python".into(),
                distro: "arch".into(),
                distro_name: "python".into(),
                version: Some("3.12.0".into()),
                status: Some("newest".into()),
                fetched_at: "2026-03-19T00:00:00Z".into(),
            },
        )
        .unwrap();
        RepologyCacheEntry::insert_or_replace(
            &conn,
            &RepologyCacheEntry {
                project_name: "python".into(),
                distro: "fedora".into(),
                distro_name: "python3".into(),
                version: Some("3.12.0".into()),
                status: Some("newest".into()),
                fetched_at: "2026-03-19T00:00:00Z".into(),
            },
        )
        .unwrap();

        let rows = RepologyCacheEntry::find_for_canonical_and_distros(
            &conn,
            canonical_id,
            &["fedora".into()],
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].distro, "fedora");
    }
}
