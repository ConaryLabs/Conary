// conary-core/src/db/migrations/v79_current.rs

use crate::error::Result;
use rusqlite::Connection;
use tracing::info;

/// Version 79: Scriptlet evidence reconciliation links
pub fn migrate_v79(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE scriptlet_evidence_cluster_reconciliation_links (
            source_cluster_key TEXT NOT NULL
                REFERENCES scriptlet_evidence_clusters(cluster_key) ON DELETE CASCADE,
            target_cluster_key TEXT NOT NULL
                REFERENCES scriptlet_evidence_clusters(cluster_key) ON DELETE CASCADE,
            normalization_version INTEGER NOT NULL,
            migrated_sample_count INTEGER NOT NULL
                CHECK (migrated_sample_count >= 0),
            created_at TEXT NOT NULL
                DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            PRIMARY KEY (
                source_cluster_key,
                target_cluster_key,
                normalization_version
            ),
            CHECK (source_cluster_key != target_cluster_key)
        );

        CREATE INDEX idx_scriptlet_evidence_reconciliation_links_target
            ON scriptlet_evidence_cluster_reconciliation_links(
                target_cluster_key,
                normalization_version,
                source_cluster_key
            );
        ",
    )?;

    info!("Schema version 79 applied successfully (scriptlet evidence reconciliation links)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::migrate;

    #[test]
    fn migrate_v79_adds_many_to_many_reconciliation_links() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let columns = conn
            .prepare("PRAGMA table_info('scriptlet_evidence_cluster_reconciliation_links')")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        for column in [
            "source_cluster_key",
            "target_cluster_key",
            "normalization_version",
            "migrated_sample_count",
            "created_at",
        ] {
            assert!(columns.contains(&column.to_string()), "missing {column}");
        }

        let create_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type = 'table'
                   AND name = 'scriptlet_evidence_cluster_reconciliation_links'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(create_sql.contains("source_cluster_key != target_cluster_key"));
        assert!(create_sql.contains("migrated_sample_count >= 0"));
    }

    #[test]
    fn reconciliation_links_preserve_source_and_target_clusters() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        for cluster_key in ["s1-source", "s1-target"] {
            conn.execute(
                "INSERT INTO scriptlet_evidence_clusters (
                    cluster_key, schema_version, normalization_version, distro,
                    target_profile, blocked_class, command,
                    normalized_command_shape, normalized_command_shape_hash,
                    lifecycle_phase
                 ) VALUES (
                    ?1, 1, 1, 'ubuntu', 'ubuntu-26.04', 'apparmor',
                    'apparmor_parser', 'apparmor_parser -r <path>', ?2,
                    'postinstall'
                 )",
                (cluster_key, format!("hash-{cluster_key}")),
            )
            .unwrap();
        }

        conn.execute(
            "INSERT INTO scriptlet_evidence_cluster_reconciliation_links (
                source_cluster_key, target_cluster_key, normalization_version,
                migrated_sample_count
             ) VALUES ('s1-source', 's1-target', 2, 3)",
            [],
        )
        .unwrap();

        let (link_count, cluster_count): (i64, i64) = conn
            .query_row(
                "SELECT
                    (SELECT COUNT(*)
                     FROM scriptlet_evidence_cluster_reconciliation_links),
                    (SELECT COUNT(*)
                     FROM scriptlet_evidence_clusters
                     WHERE cluster_key IN ('s1-source', 's1-target'))",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(link_count, 1);
        assert_eq!(cluster_count, 2);
    }
}
