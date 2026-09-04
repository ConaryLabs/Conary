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
    ConfigurationProjected,
    ActiveMarked,
    DatabaseBackedUp,
}

impl GenerationPublicationPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PendingBuild => "pending_build",
            Self::Building => "building",
            Self::ArtifactReady => "artifact_ready",
            Self::CurrentPublished => "current_published",
            Self::ConfigurationProjected => "configuration_projected",
            Self::ActiveMarked => "active_marked",
            Self::DatabaseBackedUp => "database_backed_up",
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
    pub selected_root_snapshot_id: Option<i64>,
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
        tx_uuid, selected_root_snapshot_id, db_path, runtime_root, phase, status, state_number, \
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

    /// Return the terminal publication that established one generation's
    /// exact package changeset high-water mark.
    pub fn completed_for_generation(
        conn: &Connection,
        generation_number: i64,
    ) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM generation_publications
             WHERE generation_number = ?1
               AND phase = 'database_backed_up'
               AND status = 'complete'
               AND recoverable = 0
             ORDER BY id DESC
             LIMIT 1",
            Self::COLUMNS
        );
        conn.prepare(&sql)?
            .query_row([generation_number], Self::from_row)
            .optional()
            .map_err(Into::into)
    }

    pub fn bind_selected_root_snapshot(&self, conn: &Connection, snapshot_id: i64) -> Result<()> {
        let id = self
            .id
            .ok_or_else(|| crate::error::Error::MissingId("publication id missing".to_string()))?;
        let updated = conn.execute(
            "UPDATE generation_publications
             SET selected_root_snapshot_id = ?1,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2
               AND (selected_root_snapshot_id IS NULL OR selected_root_snapshot_id = ?1)",
            params![snapshot_id, id],
        )?;
        if updated != 1 {
            return Err(crate::error::Error::RecoveryFailed(format!(
                "publication debt {id} is already bound to different selected-root authority"
            )));
        }
        Ok(())
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

    pub fn selected_root_snapshot_for_generation(
        conn: &Connection,
        generation_number: i64,
    ) -> Result<Option<i64>> {
        conn.query_row(
            "SELECT selected_root_snapshot_id
             FROM generation_publications
             WHERE generation_number = ?1
               AND selected_root_snapshot_id IS NOT NULL
             ORDER BY id DESC
             LIMIT 1",
            [generation_number],
            |row| row.get(0),
        )
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
        &self,
        conn: &Connection,
        applied_high_water_changeset_id: Option<i64>,
        state_number: i64,
        generation_number: i64,
    ) -> Result<usize> {
        let id = self
            .id
            .ok_or_else(|| crate::error::Error::MissingId("publication id missing".to_string()))?;
        let backup_is_durable = conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM generation_publications
                 WHERE id = ?1
                   AND phase = 'database_backed_up'
                   AND status = 'running'
                   AND recoverable = 1
                   AND state_number = ?2
                   AND generation_number = ?3
             )",
            params![id, state_number, generation_number],
            |row| row.get::<_, bool>(0),
        )?;
        if !backup_is_durable {
            return Err(crate::error::Error::RecoveryFailed(format!(
                "publication debt {id} cannot become terminal before its exact generation DB backup is durable"
            )));
        }
        let rows = conn.execute(
            "UPDATE generation_publications
             SET status = 'complete',
                 phase = 'database_backed_up',
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
        let phase_raw: String = row.get(7)?;
        let status_raw: String = row.get(8)?;
        let phase = phase_raw
            .parse::<GenerationPublicationPhase>()
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    7,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
        let status = status_raw
            .parse::<GenerationPublicationStatus>()
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    8,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
        let config_transaction_json: String = row.get(12)?;
        let config_transaction = serde_json::from_str::<GenerationConfigTransaction>(
            &config_transaction_json,
        )
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                12,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        config_transaction.validate().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                12,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        let recoverable: i32 = row.get(15)?;
        Ok(Self {
            id: Some(row.get(0)?),
            trigger_changeset_id: row.get(1)?,
            published_through_changeset_id: row.get(2)?,
            tx_uuid: row.get(3)?,
            selected_root_snapshot_id: row.get(4)?,
            db_path: row.get(5)?,
            runtime_root: row.get(6)?,
            phase,
            status,
            state_number: row.get(9)?,
            generation_number: row.get(10)?,
            summary: row.get(11)?,
            config_transaction,
            last_error: row.get(13)?,
            retry_count: row.get(14)?,
            recoverable: recoverable != 0,
            created_at: row.get(16)?,
            updated_at: row.get(17)?,
            completed_at: row.get(18)?,
        })
    }
}

#[cfg(test)]
mod tests;
