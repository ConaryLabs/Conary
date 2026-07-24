// conary-core/src/db/models/scriptlet_evidence/reconciliation.rs

use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptletEvidenceClusterReconciliationLink {
    pub source_cluster_key: String,
    pub target_cluster_key: String,
    pub normalization_version: i64,
    pub migrated_sample_count: i64,
    pub created_at: String,
}

impl ScriptletEvidenceClusterReconciliationLink {
    const COLUMNS: &'static str = "source_cluster_key, target_cluster_key, \
         normalization_version, migrated_sample_count, created_at";

    pub fn record(
        conn: &Connection,
        source_cluster_key: &str,
        target_cluster_key: &str,
        normalization_version: i64,
        migrated_sample_count: i64,
    ) -> Result<Self> {
        conn.execute(
            "INSERT INTO scriptlet_evidence_cluster_reconciliation_links (
                source_cluster_key, target_cluster_key, normalization_version,
                migrated_sample_count
             ) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (
                source_cluster_key, target_cluster_key, normalization_version
             ) DO UPDATE SET
                migrated_sample_count = excluded.migrated_sample_count",
            params![
                source_cluster_key,
                target_cluster_key,
                normalization_version,
                migrated_sample_count
            ],
        )?;
        Self::find(
            conn,
            source_cluster_key,
            target_cluster_key,
            normalization_version,
        )?
        .ok_or_else(|| {
            Error::NotFound(format!(
                "scriptlet evidence reconciliation link {source_cluster_key} -> \
                 {target_cluster_key} at normalization v{normalization_version}"
            ))
        })
    }

    pub fn list_outgoing(conn: &Connection, cluster_key: &str) -> Result<Vec<Self>> {
        Self::list_by_column(conn, "source_cluster_key", cluster_key)
    }

    pub fn list_incoming(conn: &Connection, cluster_key: &str) -> Result<Vec<Self>> {
        Self::list_by_column(conn, "target_cluster_key", cluster_key)
    }

    fn find(
        conn: &Connection,
        source_cluster_key: &str,
        target_cluster_key: &str,
        normalization_version: i64,
    ) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM scriptlet_evidence_cluster_reconciliation_links
             WHERE source_cluster_key = ?1
               AND target_cluster_key = ?2
               AND normalization_version = ?3",
            Self::COLUMNS
        );
        conn.query_row(
            &sql,
            params![
                source_cluster_key,
                target_cluster_key,
                normalization_version
            ],
            Self::from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    fn list_by_column(conn: &Connection, column: &str, cluster_key: &str) -> Result<Vec<Self>> {
        debug_assert!(matches!(
            column,
            "source_cluster_key" | "target_cluster_key"
        ));
        let sql = format!(
            "SELECT {} FROM scriptlet_evidence_cluster_reconciliation_links
             WHERE {column} = ?1
             ORDER BY normalization_version, source_cluster_key, target_cluster_key",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map([cluster_key], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            source_cluster_key: row.get(0)?,
            target_cluster_key: row.get(1)?,
            normalization_version: row.get(2)?,
            migrated_sample_count: row.get(3)?,
            created_at: row.get(4)?,
        })
    }
}
