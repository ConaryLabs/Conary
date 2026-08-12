// conary-core/src/db/models/system_affinity.rs

//! Diagnostic source-affinity statistics derived from installed provenance.

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row};

#[derive(Debug, Clone)]
pub struct SystemAffinity {
    pub source_identity: String,
    pub package_count: i64,
    pub percentage: f64,
}

impl SystemAffinity {
    /// Recompute diagnostic affinity from exact installed-package provenance.
    /// This output never selects package or target compatibility.
    pub fn recompute(conn: &Connection) -> Result<()> {
        conn.execute("DELETE FROM system_affinity", [])?;
        conn.execute(
            "INSERT INTO system_affinity (source_identity, package_count, percentage, updated_at)
             SELECT
                 source_identity,
                 COUNT(*) AS package_count,
                 CAST(COUNT(*) AS REAL) * 100.0
                     / MAX(1, (SELECT COUNT(*) FROM troves)) AS percentage,
                 datetime('now')
             FROM (
                 SELECT source_profile AS source_identity FROM troves
                 WHERE source_profile IS NOT NULL
                 UNION ALL
                 SELECT COALESCE(rsp.source_identity, r.source_profile) AS source_identity
                 FROM troves t
                 JOIN repositories r ON t.installed_from_repository_id = r.id
                 LEFT JOIN repository_source_policies rsp ON rsp.id = r.source_policy_id
                 WHERE t.source_profile IS NULL
                   AND t.installed_from_repository_id IS NOT NULL
                   AND COALESCE(rsp.source_identity, r.source_profile) IS NOT NULL
             )
             WHERE source_identity IS NOT NULL
             GROUP BY source_identity",
            [],
        )?;
        Ok(())
    }

    pub fn list(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT source_identity, package_count, percentage
             FROM system_affinity ORDER BY percentage DESC",
        )?;
        Ok(stmt
            .query_map([], Self::from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn get_for_source_identity(
        conn: &Connection,
        source_identity: &str,
    ) -> Result<Option<Self>> {
        Ok(conn
            .query_row(
                "SELECT source_identity, package_count, percentage
                 FROM system_affinity WHERE source_identity = ?1",
                [source_identity],
                Self::from_row,
            )
            .optional()?)
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            source_identity: row.get(0)?,
            package_count: row.get(1)?,
            percentage: row.get(2)?,
        })
    }
}
