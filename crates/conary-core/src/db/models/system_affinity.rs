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
                 SELECT COALESCE(
                     rsp.source_identity,
                     t.source_profile,
                     r.source_profile
                 ) AS source_identity
                 FROM troves t
                 LEFT JOIN repositories r ON t.installed_from_repository_id = r.id
                 LEFT JOIN repository_source_policies rsp ON rsp.id = r.source_policy_id
                 WHERE COALESCE(
                     rsp.source_identity,
                     t.source_profile,
                     r.source_profile
                 ) IS NOT NULL
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::create_test_db;

    #[test]
    fn native_repository_identity_precedes_optional_named_profile() {
        let (_database, conn) = create_test_db();
        conn.execute(
            "INSERT INTO repository_source_policies
             (source_identity, scope_kind, scope_identity, ecosystem, version_scheme,
              stream_kind, stream_identity, update_mode)
             VALUES ('opensuse:tumbleweed', 'repository', 'tumbleweed:x86_64',
                     'rpm', 'rpm', 'rolling', 'current', 'follow')",
            [],
        )
        .unwrap();
        let policy_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO repositories
             (name, url, source_profile, package_format, parser_config_json,
              source_policy_id, repository_identity, stream_binding_sha256)
             VALUES ('tumbleweed', 'https://example.test/tumbleweed', 'fedora-44',
                     'rpm', '{\"format\":\"rpm\",\"architecture\":\"x86_64\"}',
                     ?1, 'tumbleweed:x86_64', ?2)",
            rusqlite::params![policy_id, "a".repeat(64)],
        )
        .unwrap();
        let repository_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO troves
             (name, version, type, install_source, install_reason, source_profile,
              version_scheme, installed_from_repository_id)
             VALUES ('demo', '1.0-1', 'package', 'repository', 'explicit',
                     'fedora-44', 'rpm', ?1)",
            [repository_id],
        )
        .unwrap();

        SystemAffinity::recompute(&conn).unwrap();

        let affinities = SystemAffinity::list(&conn).unwrap();
        assert_eq!(affinities.len(), 1);
        assert_eq!(affinities[0].source_identity, "opensuse:tumbleweed");
        assert_eq!(affinities[0].package_count, 1);
    }
}
