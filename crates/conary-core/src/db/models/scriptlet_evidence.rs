// conary-core/src/db/models/scriptlet_evidence.rs

//! Row helpers for Remi's admin-only scriptlet evidence queue.

use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, Row, ToSql, params};
use serde::{Deserialize, Serialize};

pub const CLUSTER_KEY_PREFIX: &str = "s1-";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScriptletEvidenceState {
    NeedsTriage,
    AdapterCandidate,
    InDesign,
    InImplementation,
    CoveredPartial,
    CoveredPublicReady,
    WontSupport,
}

impl ScriptletEvidenceState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NeedsTriage => "needs-triage",
            Self::AdapterCandidate => "adapter-candidate",
            Self::InDesign => "in-design",
            Self::InImplementation => "in-implementation",
            Self::CoveredPartial => "covered-partial",
            Self::CoveredPublicReady => "covered-public-ready",
            Self::WontSupport => "wont-support",
        }
    }

    pub fn from_db(value: &str, column: usize) -> rusqlite::Result<Self> {
        match value {
            "needs-triage" => Ok(Self::NeedsTriage),
            "adapter-candidate" => Ok(Self::AdapterCandidate),
            "in-design" => Ok(Self::InDesign),
            "in-implementation" => Ok(Self::InImplementation),
            "covered-partial" => Ok(Self::CoveredPartial),
            "covered-public-ready" => Ok(Self::CoveredPublicReady),
            "wont-support" => Ok(Self::WontSupport),
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                column,
                rusqlite::types::Type::Text,
                format!("invalid scriptlet evidence state {other}").into(),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewScriptletEvidenceCluster {
    pub cluster_key: String,
    pub schema_version: i64,
    pub distro: String,
    pub target_profile: String,
    pub blocked_class: String,
    pub command: String,
    pub normalized_command_shape: String,
    pub normalized_command_shape_hash: String,
    pub lifecycle_phase: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptletEvidenceCluster {
    pub cluster_key: String,
    pub schema_version: i64,
    pub distro: String,
    pub target_profile: String,
    pub blocked_class: String,
    pub command: String,
    pub normalized_command_shape: String,
    pub normalized_command_shape_hash: String,
    pub lifecycle_phase: String,
    pub state: ScriptletEvidenceState,
    pub first_seen: String,
    pub last_seen: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScriptletEvidenceClusterListFilter {
    pub state: Option<ScriptletEvidenceState>,
    pub distro: Option<String>,
    pub blocked_class: Option<String>,
    pub command: Option<String>,
    pub package: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptletEvidenceClusterSummary {
    pub cluster: ScriptletEvidenceCluster,
    pub attempt_count: i64,
    pub unique_package_count: i64,
    pub architectures: Vec<String>,
    pub stale_sample_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptletEvidenceClusterDetail {
    pub cluster: ScriptletEvidenceCluster,
    pub samples: Vec<ScriptletEvidenceSample>,
    pub state_events: Vec<ScriptletEvidenceStateEvent>,
    pub notes: Vec<ScriptletEvidenceNote>,
}

impl ScriptletEvidenceCluster {
    const COLUMNS: &'static str = "cluster_key, schema_version, distro, target_profile, \
         blocked_class, command, normalized_command_shape, normalized_command_shape_hash, \
         lifecycle_phase, state, first_seen, last_seen, updated_at";

    pub fn upsert(conn: &Connection, cluster: &NewScriptletEvidenceCluster) -> Result<Self> {
        conn.execute(
            "INSERT INTO scriptlet_evidence_clusters (
                cluster_key, schema_version, distro, target_profile, blocked_class,
                command, normalized_command_shape, normalized_command_shape_hash,
                lifecycle_phase
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(cluster_key) DO UPDATE SET
                schema_version = excluded.schema_version,
                distro = excluded.distro,
                target_profile = excluded.target_profile,
                blocked_class = excluded.blocked_class,
                command = excluded.command,
                normalized_command_shape = excluded.normalized_command_shape,
                normalized_command_shape_hash = excluded.normalized_command_shape_hash,
                lifecycle_phase = excluded.lifecycle_phase,
                last_seen = (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                updated_at = (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
            params![
                &cluster.cluster_key,
                cluster.schema_version,
                &cluster.distro,
                &cluster.target_profile,
                &cluster.blocked_class,
                &cluster.command,
                &cluster.normalized_command_shape,
                &cluster.normalized_command_shape_hash,
                &cluster.lifecycle_phase,
            ],
        )?;
        Self::find(conn, &cluster.cluster_key)?.ok_or_else(|| {
            Error::NotFound(format!(
                "scriptlet evidence cluster {}",
                cluster.cluster_key
            ))
        })
    }

    pub fn find(conn: &Connection, cluster_key: &str) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {} FROM scriptlet_evidence_clusters WHERE cluster_key = ?1",
            Self::COLUMNS
        );
        conn.query_row(&sql, [cluster_key], Self::from_row)
            .optional()
            .map_err(Into::into)
    }

    pub fn list(
        conn: &Connection,
        filter: &ScriptletEvidenceClusterListFilter,
    ) -> Result<Vec<ScriptletEvidenceClusterSummary>> {
        let mut sql = format!(
            "SELECT {}, COUNT(s.id) AS attempt_count,
                    COUNT(DISTINCT s.package_name || '|' || s.package_version || '|' || COALESCE(s.package_architecture, '')) AS unique_package_count,
                    COALESCE(GROUP_CONCAT(DISTINCT s.package_architecture), '') AS architectures,
                    COALESCE(SUM(s.review_artifact_stale), 0) AS stale_sample_count
             FROM scriptlet_evidence_clusters c
             LEFT JOIN scriptlet_evidence_cluster_samples s ON s.cluster_key = c.cluster_key
             WHERE 1 = 1",
            Self::prefixed_columns("c")
        );
        let mut values: Vec<Box<dyn ToSql>> = Vec::new();

        if let Some(state) = filter.state {
            sql.push_str(" AND c.state = ?");
            values.push(Box::new(state.as_str().to_string()));
        }
        if let Some(distro) = filter.distro.as_ref() {
            sql.push_str(" AND c.distro = ?");
            values.push(Box::new(distro.clone()));
        }
        if let Some(blocked_class) = filter.blocked_class.as_ref() {
            sql.push_str(" AND c.blocked_class = ?");
            values.push(Box::new(blocked_class.clone()));
        }
        if let Some(command) = filter.command.as_ref() {
            sql.push_str(" AND c.command = ?");
            values.push(Box::new(command.clone()));
        }
        if let Some(package) = filter.package.as_ref() {
            sql.push_str(" AND s.package_name = ?");
            values.push(Box::new(package.clone()));
        }

        let limit = filter.limit.unwrap_or(100).clamp(1, 1000);
        let offset = filter.offset.unwrap_or(0).max(0);
        sql.push_str(" GROUP BY c.cluster_key ORDER BY c.last_seen DESC LIMIT ? OFFSET ?");
        values.push(Box::new(limit));
        values.push(Box::new(offset));

        let params: Vec<&dyn ToSql> = values.iter().map(|value| value.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params.as_slice(), Self::summary_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn detail(
        conn: &Connection,
        cluster_key: &str,
    ) -> Result<Option<ScriptletEvidenceClusterDetail>> {
        let Some(cluster) = Self::find(conn, cluster_key)? else {
            return Ok(None);
        };
        Ok(Some(ScriptletEvidenceClusterDetail {
            cluster,
            samples: ScriptletEvidenceSample::list_for_cluster(conn, cluster_key)?,
            state_events: ScriptletEvidenceStateEvent::list_for_cluster(conn, cluster_key)?,
            notes: ScriptletEvidenceNote::list_for_cluster(conn, cluster_key)?,
        }))
    }

    pub fn update_state(
        conn: &Connection,
        cluster_key: &str,
        to_state: ScriptletEvidenceState,
        actor: &str,
        reason: Option<&str>,
    ) -> Result<()> {
        let tx = conn.unchecked_transaction()?;
        let from_state: Option<String> = tx
            .query_row(
                "SELECT state FROM scriptlet_evidence_clusters WHERE cluster_key = ?1",
                [cluster_key],
                |row| row.get(0),
            )
            .optional()?;
        let Some(from_state) = from_state else {
            return Err(Error::NotFound(format!(
                "scriptlet evidence cluster {cluster_key}"
            )));
        };

        tx.execute(
            "UPDATE scriptlet_evidence_clusters
             SET state = ?2, updated_at = (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
             WHERE cluster_key = ?1",
            params![cluster_key, to_state.as_str()],
        )?;
        tx.execute(
            "INSERT INTO scriptlet_evidence_state_events
                (cluster_key, from_state, to_state, actor, reason)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![cluster_key, from_state, to_state.as_str(), actor, reason],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn prefixed_columns(alias: &str) -> String {
        Self::COLUMNS
            .split(", ")
            .map(|column| format!("{alias}.{column}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let state: String = row.get(9)?;
        Ok(Self {
            cluster_key: row.get(0)?,
            schema_version: row.get(1)?,
            distro: row.get(2)?,
            target_profile: row.get(3)?,
            blocked_class: row.get(4)?,
            command: row.get(5)?,
            normalized_command_shape: row.get(6)?,
            normalized_command_shape_hash: row.get(7)?,
            lifecycle_phase: row.get(8)?,
            state: ScriptletEvidenceState::from_db(&state, 9)?,
            first_seen: row.get(10)?,
            last_seen: row.get(11)?,
            updated_at: row.get(12)?,
        })
    }

    fn summary_from_row(row: &Row<'_>) -> rusqlite::Result<ScriptletEvidenceClusterSummary> {
        let architectures: String = row.get(15)?;
        let mut architectures = architectures
            .split(',')
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        architectures.sort();

        Ok(ScriptletEvidenceClusterSummary {
            cluster: Self::from_row(row)?,
            attempt_count: row.get(13)?,
            unique_package_count: row.get(14)?,
            architectures,
            stale_sample_count: row.get(16)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewScriptletEvidenceSample {
    pub cluster_key: String,
    pub converted_package_id: Option<i64>,
    pub original_checksum: String,
    pub distro: String,
    pub package_name: String,
    pub package_version: String,
    pub package_architecture: Option<String>,
    pub publication_status: String,
    pub scriptlet_fidelity: String,
    pub target_compatibility: String,
    pub reason_codes_json: String,
    pub blocked_classes_json: String,
    pub boot_security_intents_json: String,
    pub review_artifact_path: Option<String>,
    pub review_artifact_stale: bool,
    pub evidence_digest: Option<String>,
    pub curation_evidence_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptletEvidenceSample {
    pub id: i64,
    pub cluster_key: String,
    pub converted_package_id: Option<i64>,
    pub original_checksum: String,
    pub distro: String,
    pub package_name: String,
    pub package_version: String,
    pub package_architecture: Option<String>,
    pub publication_status: String,
    pub scriptlet_fidelity: String,
    pub target_compatibility: String,
    pub reason_codes_json: String,
    pub blocked_classes_json: String,
    pub boot_security_intents_json: String,
    pub review_artifact_path: Option<String>,
    pub review_artifact_stale: bool,
    pub evidence_digest: Option<String>,
    pub curation_evidence_digest: Option<String>,
    pub observed_at: String,
}

impl ScriptletEvidenceSample {
    const COLUMNS: &'static str = "id, cluster_key, converted_package_id, original_checksum, \
         distro, package_name, package_version, package_architecture, publication_status, \
         scriptlet_fidelity, target_compatibility, reason_codes_json, blocked_classes_json, \
         boot_security_intents_json, review_artifact_path, review_artifact_stale, \
         evidence_digest, curation_evidence_digest, observed_at";

    pub fn upsert(conn: &Connection, sample: &NewScriptletEvidenceSample) -> Result<i64> {
        let review_artifact_stale = i64::from(sample.review_artifact_stale);
        conn.execute(
            "INSERT OR IGNORE INTO scriptlet_evidence_cluster_samples (
                cluster_key, converted_package_id, original_checksum, distro, package_name,
                package_version, package_architecture, publication_status, scriptlet_fidelity,
                target_compatibility, reason_codes_json, blocked_classes_json,
                boot_security_intents_json, review_artifact_path, review_artifact_stale,
                evidence_digest, curation_evidence_digest
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                &sample.cluster_key,
                sample.converted_package_id,
                &sample.original_checksum,
                &sample.distro,
                &sample.package_name,
                &sample.package_version,
                &sample.package_architecture,
                &sample.publication_status,
                &sample.scriptlet_fidelity,
                &sample.target_compatibility,
                &sample.reason_codes_json,
                &sample.blocked_classes_json,
                &sample.boot_security_intents_json,
                &sample.review_artifact_path,
                review_artifact_stale,
                &sample.evidence_digest,
                &sample.curation_evidence_digest,
            ],
        )?;
        conn.execute(
            "UPDATE scriptlet_evidence_cluster_samples
             SET converted_package_id = ?5,
                 distro = ?6,
                 package_architecture = ?7,
                 publication_status = ?8,
                 scriptlet_fidelity = ?9,
                 target_compatibility = ?10,
                 reason_codes_json = ?11,
                 blocked_classes_json = ?12,
                 boot_security_intents_json = ?13,
                 review_artifact_path = ?14,
                 review_artifact_stale = ?15,
                 evidence_digest = ?16,
                 curation_evidence_digest = ?17,
                 observed_at = (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
             WHERE cluster_key = ?1
               AND original_checksum = ?2
               AND package_name = ?3
               AND package_version = ?4
               AND COALESCE(package_architecture, '') = COALESCE(?7, '')",
            params![
                &sample.cluster_key,
                &sample.original_checksum,
                &sample.package_name,
                &sample.package_version,
                sample.converted_package_id,
                &sample.distro,
                &sample.package_architecture,
                &sample.publication_status,
                &sample.scriptlet_fidelity,
                &sample.target_compatibility,
                &sample.reason_codes_json,
                &sample.blocked_classes_json,
                &sample.boot_security_intents_json,
                &sample.review_artifact_path,
                review_artifact_stale,
                &sample.evidence_digest,
                &sample.curation_evidence_digest,
            ],
        )?;
        Self::find_observation_id(conn, sample)
    }

    pub fn list_for_cluster(conn: &Connection, cluster_key: &str) -> Result<Vec<Self>> {
        let sql = format!(
            "SELECT {} FROM scriptlet_evidence_cluster_samples
             WHERE cluster_key = ?1
             ORDER BY observed_at DESC, id DESC",
            Self::COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map([cluster_key], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn find_observation_id(conn: &Connection, sample: &NewScriptletEvidenceSample) -> Result<i64> {
        conn.query_row(
            "SELECT id FROM scriptlet_evidence_cluster_samples
             WHERE cluster_key = ?1
               AND original_checksum = ?2
               AND package_name = ?3
               AND package_version = ?4
               AND COALESCE(package_architecture, '') = COALESCE(?5, '')",
            params![
                &sample.cluster_key,
                &sample.original_checksum,
                &sample.package_name,
                &sample.package_version,
                &sample.package_architecture,
            ],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let stale: i64 = row.get(15)?;
        Ok(Self {
            id: row.get(0)?,
            cluster_key: row.get(1)?,
            converted_package_id: row.get(2)?,
            original_checksum: row.get(3)?,
            distro: row.get(4)?,
            package_name: row.get(5)?,
            package_version: row.get(6)?,
            package_architecture: row.get(7)?,
            publication_status: row.get(8)?,
            scriptlet_fidelity: row.get(9)?,
            target_compatibility: row.get(10)?,
            reason_codes_json: row.get(11)?,
            blocked_classes_json: row.get(12)?,
            boot_security_intents_json: row.get(13)?,
            review_artifact_path: row.get(14)?,
            review_artifact_stale: stale != 0,
            evidence_digest: row.get(16)?,
            curation_evidence_digest: row.get(17)?,
            observed_at: row.get(18)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptletEvidenceStateEvent {
    pub id: i64,
    pub cluster_key: String,
    pub from_state: Option<ScriptletEvidenceState>,
    pub to_state: ScriptletEvidenceState,
    pub actor: String,
    pub reason: Option<String>,
    pub created_at: String,
}

impl ScriptletEvidenceStateEvent {
    pub fn list_for_cluster(conn: &Connection, cluster_key: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, cluster_key, from_state, to_state, actor, reason, created_at
             FROM scriptlet_evidence_state_events
             WHERE cluster_key = ?1
             ORDER BY id",
        )?;
        let rows = stmt
            .query_map([cluster_key], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let from_state: Option<String> = row.get(2)?;
        let to_state: String = row.get(3)?;
        Ok(Self {
            id: row.get(0)?,
            cluster_key: row.get(1)?,
            from_state: from_state
                .as_deref()
                .map(|state| ScriptletEvidenceState::from_db(state, 2))
                .transpose()?,
            to_state: ScriptletEvidenceState::from_db(&to_state, 3)?,
            actor: row.get(4)?,
            reason: row.get(5)?,
            created_at: row.get(6)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptletEvidenceNote {
    pub id: i64,
    pub cluster_key: String,
    pub actor: String,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
}

impl ScriptletEvidenceNote {
    pub fn insert(conn: &Connection, cluster_key: &str, actor: &str, body: &str) -> Result<Self> {
        conn.execute(
            "INSERT INTO scriptlet_evidence_notes (cluster_key, actor, body)
             VALUES (?1, ?2, ?3)",
            params![cluster_key, actor, body],
        )?;
        Self::find(conn, conn.last_insert_rowid())?.ok_or_else(|| {
            Error::NotFound(format!(
                "scriptlet evidence note {}",
                conn.last_insert_rowid()
            ))
        })
    }

    pub fn list_for_cluster(conn: &Connection, cluster_key: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, cluster_key, actor, body, created_at, updated_at
             FROM scriptlet_evidence_notes
             WHERE cluster_key = ?1
             ORDER BY id",
        )?;
        let rows = stmt
            .query_map([cluster_key], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn find(conn: &Connection, id: i64) -> Result<Option<Self>> {
        conn.query_row(
            "SELECT id, cluster_key, actor, body, created_at, updated_at
             FROM scriptlet_evidence_notes
             WHERE id = ?1",
            [id],
            Self::from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            cluster_key: row.get(1)?,
            actor: row.get(2)?,
            body: row.get(3)?,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackfillStatus {
    Running,
    Complete,
    Failed,
}

impl BackfillStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Complete => "complete",
            Self::Failed => "failed",
        }
    }

    pub fn from_db(value: &str, column: usize) -> rusqlite::Result<Self> {
        match value {
            "running" => Ok(Self::Running),
            "complete" => Ok(Self::Complete),
            "failed" => Ok(Self::Failed),
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                column,
                rusqlite::types::Type::Text,
                format!("invalid scriptlet evidence backfill status {other}").into(),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptletEvidenceBackfillRun {
    pub id: i64,
    pub status: BackfillStatus,
    pub started_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub last_converted_package_id: i64,
    pub scanned_count: i64,
    pub clustered_count: i64,
    pub error_message: Option<String>,
}

impl ScriptletEvidenceBackfillRun {
    pub fn create(conn: &Connection) -> Result<Self> {
        conn.execute(
            "INSERT INTO scriptlet_evidence_backfill_runs (status) VALUES ('running')",
            [],
        )?;
        Self::find_or_not_found(conn, conn.last_insert_rowid())
    }

    pub fn update_progress(
        conn: &Connection,
        id: i64,
        last_converted_package_id: i64,
        scanned_count: i64,
        clustered_count: i64,
    ) -> Result<Self> {
        conn.execute(
            "UPDATE scriptlet_evidence_backfill_runs
             SET updated_at = (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                 last_converted_package_id = ?2,
                 scanned_count = ?3,
                 clustered_count = ?4
             WHERE id = ?1",
            params![
                id,
                last_converted_package_id,
                scanned_count,
                clustered_count
            ],
        )?;
        Self::find_or_not_found(conn, id)
    }

    pub fn complete(
        conn: &Connection,
        id: i64,
        last_converted_package_id: i64,
        scanned_count: i64,
        clustered_count: i64,
    ) -> Result<Self> {
        conn.execute(
            "UPDATE scriptlet_evidence_backfill_runs
             SET status = 'complete',
                 updated_at = (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                 completed_at = (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                 last_converted_package_id = ?2,
                 scanned_count = ?3,
                 clustered_count = ?4,
                 error_message = NULL
             WHERE id = ?1",
            params![
                id,
                last_converted_package_id,
                scanned_count,
                clustered_count
            ],
        )?;
        Self::find_or_not_found(conn, id)
    }

    pub fn fail(conn: &Connection, id: i64, error_message: &str) -> Result<Self> {
        conn.execute(
            "UPDATE scriptlet_evidence_backfill_runs
             SET status = 'failed',
                 updated_at = (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                 completed_at = (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                 error_message = ?2
             WHERE id = ?1",
            params![id, error_message],
        )?;
        Self::find_or_not_found(conn, id)
    }

    fn find_or_not_found(conn: &Connection, id: i64) -> Result<Self> {
        Self::find(conn, id)?
            .ok_or_else(|| Error::NotFound(format!("scriptlet evidence backfill run {id}")))
    }

    fn find(conn: &Connection, id: i64) -> Result<Option<Self>> {
        conn.query_row(
            "SELECT id, status, started_at, updated_at, completed_at,
                    last_converted_package_id, scanned_count, clustered_count, error_message
             FROM scriptlet_evidence_backfill_runs
             WHERE id = ?1",
            [id],
            Self::from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let status: String = row.get(1)?;
        Ok(Self {
            id: row.get(0)?,
            status: BackfillStatus::from_db(&status, 1)?,
            started_at: row.get(2)?,
            updated_at: row.get(3)?,
            completed_at: row.get(4)?,
            last_converted_package_id: row.get(5)?,
            scanned_count: row.get(6)?,
            clustered_count: row.get(7)?,
            error_message: row.get(8)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::migrate;
    use rusqlite::Connection;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    fn new_cluster() -> NewScriptletEvidenceCluster {
        NewScriptletEvidenceCluster {
            cluster_key: "s1-dracut".to_string(),
            schema_version: 1,
            distro: "fedora".to_string(),
            target_profile: "fedora-44".to_string(),
            blocked_class: "initramfs".to_string(),
            command: "dracut".to_string(),
            normalized_command_shape: "dracut --force <boot>/initramfs-<kver>.img".to_string(),
            normalized_command_shape_hash: "shape-hash".to_string(),
            lifecycle_phase: "postinstall".to_string(),
        }
    }

    fn new_sample(package_name: &str) -> NewScriptletEvidenceSample {
        NewScriptletEvidenceSample {
            cluster_key: "s1-dracut".to_string(),
            converted_package_id: None,
            original_checksum: format!("sha256:{package_name}"),
            distro: "fedora".to_string(),
            package_name: package_name.to_string(),
            package_version: "1.0.0".to_string(),
            package_architecture: Some("x86_64".to_string()),
            publication_status: "blocked".to_string(),
            scriptlet_fidelity: "blocked".to_string(),
            target_compatibility: "blocked".to_string(),
            reason_codes_json: r#"["boot-security-initramfs"]"#.to_string(),
            blocked_classes_json: r#"["initramfs"]"#.to_string(),
            boot_security_intents_json: r#"[{"command":"dracut"}]"#.to_string(),
            review_artifact_path: Some("/cache/scriptlet-review/fedora/pkg.json".to_string()),
            review_artifact_stale: false,
            evidence_digest: Some("evidence-digest".to_string()),
            curation_evidence_digest: None,
        }
    }

    #[test]
    fn upsert_cluster_preserves_existing_state() {
        let conn = test_conn();
        let cluster = ScriptletEvidenceCluster::upsert(&conn, &new_cluster()).unwrap();
        assert_eq!(cluster.state, ScriptletEvidenceState::NeedsTriage);

        ScriptletEvidenceCluster::update_state(
            &conn,
            "s1-dracut",
            ScriptletEvidenceState::AdapterCandidate,
            "maintainer",
            Some("repeated dracut shape"),
        )
        .unwrap();

        let cluster = ScriptletEvidenceCluster::upsert(&conn, &new_cluster()).unwrap();
        assert_eq!(cluster.state, ScriptletEvidenceState::AdapterCandidate);
    }

    #[test]
    fn upsert_sample_deduplicates_observation_and_list_computes_counts() {
        let conn = test_conn();
        ScriptletEvidenceCluster::upsert(&conn, &new_cluster()).unwrap();

        let first = ScriptletEvidenceSample::upsert(&conn, &new_sample("kernel")).unwrap();
        let second = ScriptletEvidenceSample::upsert(&conn, &new_sample("kernel")).unwrap();
        assert_eq!(first, second);
        ScriptletEvidenceSample::upsert(&conn, &new_sample("kernel-core")).unwrap();

        let clusters =
            ScriptletEvidenceCluster::list(&conn, &ScriptletEvidenceClusterListFilter::default())
                .unwrap();
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].attempt_count, 2);
        assert_eq!(clusters[0].unique_package_count, 2);
        assert_eq!(clusters[0].architectures, vec!["x86_64".to_string()]);
        assert_eq!(clusters[0].stale_sample_count, 0);
    }

    #[test]
    fn detail_includes_samples_events_and_notes() {
        let conn = test_conn();
        ScriptletEvidenceCluster::upsert(&conn, &new_cluster()).unwrap();
        ScriptletEvidenceSample::upsert(&conn, &new_sample("kernel")).unwrap();
        ScriptletEvidenceCluster::update_state(
            &conn,
            "s1-dracut",
            ScriptletEvidenceState::InDesign,
            "maintainer",
            None,
        )
        .unwrap();
        ScriptletEvidenceNote::insert(&conn, "s1-dracut", "maintainer", "Check Fedora first")
            .unwrap();

        let detail = ScriptletEvidenceCluster::detail(&conn, "s1-dracut")
            .unwrap()
            .unwrap();
        assert_eq!(detail.cluster.cluster_key, "s1-dracut");
        assert_eq!(detail.samples.len(), 1);
        assert_eq!(detail.state_events.len(), 1);
        assert_eq!(detail.notes.len(), 1);
    }

    #[test]
    fn backfill_run_records_progress_completion_and_failure() {
        let conn = test_conn();
        let run = ScriptletEvidenceBackfillRun::create(&conn).unwrap();
        assert_eq!(run.status, BackfillStatus::Running);

        let run = ScriptletEvidenceBackfillRun::update_progress(&conn, run.id, 10, 7, 4).unwrap();
        assert_eq!(run.last_converted_package_id, 10);
        assert_eq!(run.scanned_count, 7);
        assert_eq!(run.clustered_count, 4);

        let run = ScriptletEvidenceBackfillRun::complete(&conn, run.id, 11, 8, 5).unwrap();
        assert_eq!(run.status, BackfillStatus::Complete);
        assert!(run.completed_at.is_some());

        let failed = ScriptletEvidenceBackfillRun::create(&conn).unwrap();
        let failed =
            ScriptletEvidenceBackfillRun::fail(&conn, failed.id, "bad scriptlet JSON").unwrap();
        assert_eq!(failed.status, BackfillStatus::Failed);
        assert_eq!(failed.error_message.as_deref(), Some("bad scriptlet JSON"));
    }
}
