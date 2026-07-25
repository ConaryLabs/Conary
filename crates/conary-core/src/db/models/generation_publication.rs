// conary-core/src/db/models/generation_publication.rs

use crate::config_transaction::GenerationConfigTransaction;
use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use strum_macros::{AsRefStr, Display, EnumString};

#[derive(Debug, Clone, Copy, PartialEq, Eq, AsRefStr, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum GenerationPublicationPhase {
    PendingBuild,
    Building,
    ArtifactReady,
    CurrentPublished,
    ActiveMarked,
}

impl GenerationPublicationPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PendingBuild => "pending_build",
            Self::Building => "building",
            Self::ArtifactReady => "artifact_ready",
            Self::CurrentPublished => "current_published",
            Self::ActiveMarked => "active_marked",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, AsRefStr, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum GenerationPublicationStatus {
    Pending,
    Running,
    Failed,
    Complete,
    Abandoned,
}

impl GenerationPublicationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Failed => "failed",
            Self::Complete => "complete",
            Self::Abandoned => "abandoned",
        }
    }

    pub fn is_recoverable(self) -> bool {
        matches!(self, Self::Pending | Self::Running | Self::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationPublication {
    pub id: Option<i64>,
    pub trigger_changeset_id: Option<i64>,
    pub published_through_changeset_id: Option<i64>,
    pub tx_uuid: Option<String>,
    pub db_path: String,
    pub runtime_root: String,
    pub phase: GenerationPublicationPhase,
    pub status: GenerationPublicationStatus,
    pub state_number: Option<i64>,
    pub generation_number: Option<i64>,
    pub summary: String,
    pub config_transaction: GenerationConfigTransaction,
    pub last_error: Option<String>,
    pub retry_count: i64,
    pub recoverable: bool,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub completed_at: Option<String>,
}

impl GenerationPublication {
    const COLUMNS: &'static str = "id, trigger_changeset_id, published_through_changeset_id, \
        tx_uuid, db_path, runtime_root, phase, status, state_number, \
        generation_number, summary, config_transaction_json, last_error, retry_count, \
        recoverable, created_at, updated_at, completed_at";

    pub fn create_pending(
        conn: &Connection,
        trigger_changeset_id: Option<i64>,
        tx_uuid: Option<&str>,
        db_path: &str,
        runtime_root: &str,
        summary: &str,
        config_transaction: &GenerationConfigTransaction,
    ) -> Result<Self> {
        config_transaction.validate()?;
        let config_transaction_json = serde_json::to_string(config_transaction)?;
        conn.execute(
            "INSERT INTO generation_publications (
                trigger_changeset_id, tx_uuid, db_path, runtime_root, phase, status,
                summary, config_transaction_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                trigger_changeset_id,
                tx_uuid,
                db_path,
                runtime_root,
                GenerationPublicationPhase::PendingBuild.as_str(),
                GenerationPublicationStatus::Pending.as_str(),
                summary,
                config_transaction_json,
            ],
        )?;
        Self::find_by_id(conn, conn.last_insert_rowid())?.ok_or_else(|| {
            crate::error::Error::InternalError("inserted publication row not found".to_string())
        })
    }

    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM generation_publications WHERE id = ?1",
            Self::COLUMNS
        );
        conn.prepare(&sql)?
            .query_row([id], Self::from_row)
            .optional()
            .map_err(Into::into)
    }

    pub fn pending_recoverable(conn: &Connection) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM generation_publications
             WHERE recoverable = 1 AND status IN ('pending', 'running', 'failed')
             ORDER BY id ASC",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], Self::from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn pending_for_changeset(conn: &Connection, changeset_id: i64) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM generation_publications
             WHERE trigger_changeset_id = ?1
               AND recoverable = 1
               AND status IN ('pending', 'running', 'failed')
             ORDER BY id DESC LIMIT 1",
            Self::COLUMNS
        );
        conn.prepare(&sql)?
            .query_row([changeset_id], Self::from_row)
            .optional()
            .map_err(Into::into)
    }

    /// Retire forward publication debt when its package changeset is rolled
    /// back. The compensating rollback publication is recorded separately.
    pub fn abandon_recoverable_for_changeset(
        conn: &Connection,
        changeset_id: i64,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE generation_publications
             SET status = 'abandoned',
                 recoverable = 0,
                 updated_at = CURRENT_TIMESTAMP,
                 completed_at = CURRENT_TIMESTAMP
             WHERE trigger_changeset_id = ?1
               AND recoverable = 1
               AND status IN ('pending', 'running', 'failed')",
            [changeset_id],
        )
        .map_err(Into::into)
    }

    pub fn applied_high_water_changeset_id(conn: &Connection) -> Result<Option<i64>> {
        conn.query_row(
            "SELECT MAX(id) FROM changesets WHERE status = 'applied'",
            [],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    pub fn mark_failed(&self, conn: &Connection, message: &str) -> Result<()> {
        let id = self
            .id
            .ok_or_else(|| crate::error::Error::MissingId("publication id missing".to_string()))?;
        conn.execute(
            "UPDATE generation_publications
             SET status = 'failed',
                 last_error = ?1,
                 retry_count = retry_count + 1,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2",
            params![message, id],
        )?;
        Ok(())
    }

    pub fn set_phase(
        &self,
        conn: &Connection,
        phase: GenerationPublicationPhase,
        status: GenerationPublicationStatus,
        state_number: Option<i64>,
        generation_number: Option<i64>,
    ) -> Result<()> {
        let id = self
            .id
            .ok_or_else(|| crate::error::Error::MissingId("publication id missing".to_string()))?;
        conn.execute(
            "UPDATE generation_publications
             SET phase = ?1,
                 status = ?2,
                 state_number = COALESCE(?3, state_number),
                 generation_number = COALESCE(?4, generation_number),
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?5",
            params![
                phase.as_str(),
                status.as_str(),
                state_number,
                generation_number,
                id
            ],
        )?;
        Ok(())
    }

    pub fn mark_complete_through(
        conn: &Connection,
        applied_high_water_changeset_id: Option<i64>,
        state_number: i64,
        generation_number: i64,
    ) -> Result<usize> {
        let rows = conn.execute(
            "UPDATE generation_publications
             SET status = 'complete',
                 phase = 'active_marked',
                 published_through_changeset_id = ?1,
                 state_number = COALESCE(state_number, ?2),
                 generation_number = COALESCE(generation_number, ?3),
                 recoverable = 0,
                 completed_at = CURRENT_TIMESTAMP,
                 updated_at = CURRENT_TIMESTAMP
             WHERE recoverable = 1
               AND status IN ('pending', 'running', 'failed')
               AND (?1 IS NULL OR trigger_changeset_id IS NULL OR trigger_changeset_id <= ?1)",
            params![
                applied_high_water_changeset_id,
                state_number,
                generation_number
            ],
        )?;
        Ok(rows)
    }

    pub fn protected_generation_numbers(conn: &Connection) -> Result<Vec<i64>> {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT generation_number
             FROM generation_publications
             WHERE recoverable = 1
               AND status IN ('pending', 'running', 'failed')
               AND generation_number IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let phase_raw: String = row.get(6)?;
        let status_raw: String = row.get(7)?;
        let phase = phase_raw
            .parse::<GenerationPublicationPhase>()
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    6,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
        let status = status_raw
            .parse::<GenerationPublicationStatus>()
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    7,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
        let config_transaction_json: String = row.get(11)?;
        let config_transaction = serde_json::from_str::<GenerationConfigTransaction>(
            &config_transaction_json,
        )
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                11,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        config_transaction.validate().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                11,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        let recoverable: i32 = row.get(14)?;
        Ok(Self {
            id: Some(row.get(0)?),
            trigger_changeset_id: row.get(1)?,
            published_through_changeset_id: row.get(2)?,
            tx_uuid: row.get(3)?,
            db_path: row.get(4)?,
            runtime_root: row.get(5)?,
            phase,
            status,
            state_number: row.get(8)?,
            generation_number: row.get(9)?,
            summary: row.get(10)?,
            config_transaction,
            last_error: row.get(12)?,
            retry_count: row.get(13)?,
            recoverable: recoverable != 0,
            created_at: row.get(15)?,
            updated_at: row.get(16)?,
            completed_at: row.get(17)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::{PayloadNode, ResolvedPayloadNode};
    use std::str::FromStr;

    fn exact_config_transaction() -> GenerationConfigTransaction {
        let node = ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o640)).unwrap();
        let artifact = crate::config_transaction::ConfigArtifact::regular(b"exact", node).unwrap();
        GenerationConfigTransaction {
            entries: vec![crate::config_transaction::ConfigPathTransaction {
                path: "/etc/exact.conf".to_string(),
                operation: crate::config_transaction::ConfigTransactionOperation::Install,
                before: None,
                current: None,
                after: Some(crate::config_transaction::ConfigPackageState {
                    source: crate::db::models::ConfigSource::Auto,
                    noreplace: false,
                    ghost: false,
                    original_sha256: Some(artifact.sha256().to_string()),
                    artifact: Some(artifact),
                }),
                auxiliaries: Vec::new(),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn phase_and_status_reject_unknown_values() {
        assert_eq!(
            GenerationPublicationPhase::from_str("artifact_ready").unwrap(),
            GenerationPublicationPhase::ArtifactReady
        );
        assert!(GenerationPublicationPhase::from_str("current_renamed").is_err());
        assert_eq!(
            GenerationPublicationStatus::from_str("failed").unwrap(),
            GenerationPublicationStatus::Failed
        );
        assert!(GenerationPublicationStatus::from_str("mystery").is_err());
    }

    #[test]
    fn create_pending_and_mark_complete_sweeps_covered_debts() {
        let (_tmp, conn) = crate::db::testing::create_test_db();

        conn.execute(
            "INSERT INTO changesets (description, status) VALUES ('A', 'applied')",
            [],
        )
        .unwrap();
        let cs_a = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO changesets (description, status) VALUES ('B', 'applied')",
            [],
        )
        .unwrap();
        let cs_b = conn.last_insert_rowid();

        let a = GenerationPublication::create_pending(
            &conn,
            Some(cs_a),
            None,
            "/tmp/conary.db",
            "/tmp/conary",
            "A",
            &Default::default(),
        )
        .unwrap();
        let b = GenerationPublication::create_pending(
            &conn,
            Some(cs_b),
            None,
            "/tmp/conary.db",
            "/tmp/conary",
            "B",
            &Default::default(),
        )
        .unwrap();
        a.mark_failed(&conn, "forced").unwrap();
        b.set_phase(
            &conn,
            GenerationPublicationPhase::ArtifactReady,
            GenerationPublicationStatus::Running,
            Some(7),
            Some(7),
        )
        .unwrap();

        let completed =
            GenerationPublication::mark_complete_through(&conn, Some(cs_b), 7, 7).unwrap();
        assert_eq!(completed, 2);
        assert!(
            GenerationPublication::pending_recoverable(&conn)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn pending_for_changeset_finds_recoverable_debt_only() {
        let (_tmp, conn) = crate::db::testing::create_test_db();
        conn.execute(
            "INSERT INTO changesets (description, status) VALUES ('A', 'applied')",
            [],
        )
        .unwrap();
        let cs_a = conn.last_insert_rowid();

        let debt = GenerationPublication::create_pending(
            &conn,
            Some(cs_a),
            None,
            "/tmp/conary.db",
            "/tmp/conary",
            "A",
            &Default::default(),
        )
        .unwrap();
        assert_eq!(
            GenerationPublication::pending_for_changeset(&conn, cs_a)
                .unwrap()
                .unwrap()
                .id,
            debt.id
        );

        GenerationPublication::mark_complete_through(&conn, Some(cs_a), 1, 1).unwrap();
        assert!(
            GenerationPublication::pending_for_changeset(&conn, cs_a)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn abandoning_changeset_debt_excludes_forward_transaction_from_retry() {
        let (_tmp, conn) = crate::db::testing::create_test_db();
        conn.execute(
            "INSERT INTO changesets (description, status) VALUES ('upgrade', 'applied')",
            [],
        )
        .unwrap();
        let changeset_id = conn.last_insert_rowid();
        GenerationPublication::create_pending(
            &conn,
            Some(changeset_id),
            None,
            "/tmp/conary.db",
            "/tmp/conary",
            "upgrade",
            &exact_config_transaction(),
        )
        .unwrap();

        assert_eq!(
            GenerationPublication::abandon_recoverable_for_changeset(&conn, changeset_id).unwrap(),
            1
        );
        assert!(
            GenerationPublication::pending_recoverable(&conn)
                .unwrap()
                .is_empty()
        );
        let status: String = conn
            .query_row(
                "SELECT status FROM generation_publications
                 WHERE trigger_changeset_id = ?1",
                [changeset_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "abandoned");
    }

    #[test]
    fn publication_snapshot_obeys_the_callers_sql_transaction() {
        let (_tmp, conn) = crate::db::testing::create_test_db();
        let snapshot = exact_config_transaction();

        {
            let tx = conn.unchecked_transaction().unwrap();
            GenerationPublication::create_pending(
                &tx,
                None,
                None,
                "/tmp/conary.db",
                "/tmp/conary",
                "rolled back",
                &snapshot,
            )
            .unwrap();
        }
        assert!(
            GenerationPublication::pending_recoverable(&conn)
                .unwrap()
                .is_empty()
        );

        let tx = conn.unchecked_transaction().unwrap();
        GenerationPublication::create_pending(
            &tx,
            None,
            None,
            "/tmp/conary.db",
            "/tmp/conary",
            "committed",
            &snapshot,
        )
        .unwrap();
        tx.commit().unwrap();

        let debts = GenerationPublication::pending_recoverable(&conn).unwrap();
        assert_eq!(debts.len(), 1);
        assert_eq!(debts[0].config_transaction, snapshot);
    }

    #[test]
    fn applied_high_water_ignores_pending_and_rolled_back_changesets() {
        let (_tmp, conn) = crate::db::testing::create_test_db();
        conn.execute(
            "INSERT INTO changesets (description, status) VALUES ('A', 'applied')",
            [],
        )
        .unwrap();
        let applied = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO changesets (description, status) VALUES ('B', 'pending')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO changesets (description, status) VALUES ('C', 'rolled_back')",
            [],
        )
        .unwrap();

        assert_eq!(
            GenerationPublication::applied_high_water_changeset_id(&conn).unwrap(),
            Some(applied)
        );
    }
}
