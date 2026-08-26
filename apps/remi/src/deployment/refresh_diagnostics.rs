// apps/remi/src/deployment/refresh_diagnostics.rs

//! Public-sanitized diagnostics for the latest durable profile refresh run.

use anyhow::{Context, Result, bail};
use conary_core::diagnostics::RedactionMarker;
use rusqlite::OptionalExtension;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentRefreshRunState {
    Created,
    FetchingRoots,
    FetchingObjects,
    Authenticated,
    Ingesting,
    Validating,
    ReadyToPublish,
    Candidate,
    Published,
    Failed,
    Abandoned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentRefreshFailureStage {
    Created,
    FetchingRoots,
    FetchingObjects,
    Authenticated,
    Ingesting,
    Validating,
    ReadyToPublish,
    Publishing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentRefreshFailureCategory {
    Transport,
    WireContract,
    Database,
    Fenced,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeploymentProfileRefreshState {
    pub run_id: String,
    pub fencing_epoch: i64,
    pub state: DeploymentRefreshRunState,
    pub started_at: i64,
    pub heartbeat_at: i64,
    pub finished_at: Option<i64>,
    pub failure_stage: Option<DeploymentRefreshFailureStage>,
    pub failure_category: Option<DeploymentRefreshFailureCategory>,
    pub run_members: u64,
    pub candidate_members: u64,
    pub failure_evidence_sha256: Option<String>,
    pub failure_diagnostic: Option<String>,
    pub redactions: Vec<RedactionMarker>,
}

pub(super) fn latest_profile_refresh(
    conn: &rusqlite::Connection,
    profile: &str,
) -> Result<Option<DeploymentProfileRefreshState>> {
    let row = conn
        .query_row(
            "SELECT run.run_id, run.fencing_epoch, run.state,
                    run.started_at, run.heartbeat_at, run.finished_at,
                    run.failure_stage, run.failure_category, run.failure_evidence,
                    (SELECT COUNT(*) FROM repository_sync_run_members member
                     WHERE member.run_id = run.run_id),
                    (SELECT COUNT(*) FROM repository_sync_run_members member
                     WHERE member.run_id = run.run_id
                       AND member.candidate_source_snapshot_sha256 IS NOT NULL)
             FROM repository_sync_runs run
             WHERE run.source_profile = ?1
             ORDER BY run.fencing_epoch DESC, run.run_id DESC
             LIMIT 1",
            [profile],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, i64>(10)?,
                ))
            },
        )
        .optional()?;
    let Some((
        run_id,
        fencing_epoch,
        state,
        started_at,
        heartbeat_at,
        finished_at,
        failure_stage,
        failure_category,
        failure_evidence,
        run_members,
        candidate_members,
    )) = row
    else {
        return Ok(None);
    };
    let (failure_evidence_sha256, failure_diagnostic, redactions) =
        sanitize_failure(failure_evidence.as_deref());
    Ok(Some(DeploymentProfileRefreshState {
        run_id,
        fencing_epoch,
        state: DeploymentRefreshRunState::parse(&state)?,
        started_at,
        heartbeat_at,
        finished_at,
        failure_stage: failure_stage
            .as_deref()
            .map(DeploymentRefreshFailureStage::parse)
            .transpose()?,
        failure_category: failure_category
            .as_deref()
            .map(DeploymentRefreshFailureCategory::parse)
            .transpose()?,
        run_members: u64::try_from(run_members)
            .context("profile refresh member count is negative")?,
        candidate_members: u64::try_from(candidate_members)
            .context("profile refresh candidate member count is negative")?,
        failure_evidence_sha256,
        failure_diagnostic,
        redactions,
    }))
}

fn sanitize_failure(
    evidence: Option<&str>,
) -> (Option<String>, Option<String>, Vec<RedactionMarker>) {
    let Some(evidence) = evidence else {
        return (None, None, Vec::new());
    };
    let digest = conary_core::hash::sha256(evidence.as_bytes());
    let mut diagnostic = conary_core::diagnostics::redaction::redact_log(evidence);
    for path in host_path_tokens(&diagnostic.value) {
        diagnostic.value = diagnostic.value.replace(&path, "[REDACTED-PATH]");
        diagnostic.redactions.push(RedactionMarker::new(
            "failure_diagnostic",
            "host-local-path",
        ));
    }
    (Some(digest), Some(diagnostic.value), diagnostic.redactions)
}

fn host_path_tokens(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .map(|token| {
            token.trim_matches(|character: char| {
                matches!(character, '\'' | '"' | ',' | ';' | ':' | '(' | ')')
            })
        })
        .filter(|token| {
            token.starts_with('/') || token.starts_with("~/") || token.starts_with("./")
        })
        .map(ToOwned::to_owned)
        .collect()
}

macro_rules! parse_enum {
    ($type:ident, $value:expr, {$($name:literal => $variant:ident),+ $(,)?}) => {
        match $value {
            $($name => Ok(Self::$variant),)+
            unknown => bail!("unknown {} value '{unknown}'", stringify!($type)),
        }
    };
}

impl DeploymentRefreshRunState {
    fn parse(value: &str) -> Result<Self> {
        parse_enum!(DeploymentRefreshRunState, value, {
            "created" => Created,
            "fetching_roots" => FetchingRoots,
            "fetching_objects" => FetchingObjects,
            "authenticated" => Authenticated,
            "ingesting" => Ingesting,
            "validating" => Validating,
            "ready_to_publish" => ReadyToPublish,
            "candidate" => Candidate,
            "published" => Published,
            "failed" => Failed,
            "abandoned" => Abandoned,
        })
    }
}

impl DeploymentRefreshFailureStage {
    fn parse(value: &str) -> Result<Self> {
        parse_enum!(DeploymentRefreshFailureStage, value, {
            "created" => Created,
            "fetching_roots" => FetchingRoots,
            "fetching_objects" => FetchingObjects,
            "authenticated" => Authenticated,
            "ingesting" => Ingesting,
            "validating" => Validating,
            "ready_to_publish" => ReadyToPublish,
            "publishing" => Publishing,
        })
    }
}

impl DeploymentRefreshFailureCategory {
    fn parse(value: &str) -> Result<Self> {
        parse_enum!(DeploymentRefreshFailureCategory, value, {
            "transport" => Transport,
            "wire_contract" => WireContract,
            "database" => Database,
            "fenced" => Fenced,
            "internal" => Internal,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE repository_sync_runs (
                run_id TEXT PRIMARY KEY,
                source_profile TEXT NOT NULL,
                fencing_epoch INTEGER NOT NULL,
                state TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                heartbeat_at INTEGER NOT NULL,
                finished_at INTEGER,
                failure_stage TEXT,
                failure_category TEXT,
                failure_evidence TEXT
             );
             CREATE TABLE repository_sync_run_members (
                run_id TEXT NOT NULL,
                candidate_source_snapshot_sha256 TEXT
             );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn latest_fencing_epoch_and_exact_member_progress_are_reported() {
        let conn = connection();
        conn.execute(
            "INSERT INTO repository_sync_runs VALUES
             ('older', 'fedora-44', 1, 'failed', 10, 11, 12,
              'fetching_objects', 'transport', 'old')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO repository_sync_runs VALUES
             ('newer', 'fedora-44', 2, 'abandoned', 20, 21, 22,
              'publishing', 'internal',
              'failed at /conary/private https://user:pass@example.invalid/source \
https://second:secret@example.invalid/source Bearer abc.def \
/home/dev/.ssh/id_ed25519 TOKEN=secret TOKEN=second')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO repository_sync_run_members VALUES ('newer', ?1), ('newer', NULL)",
            ["a".repeat(64)],
        )
        .unwrap();

        let state = latest_profile_refresh(&conn, "fedora-44").unwrap().unwrap();
        assert_eq!(state.run_id, "newer");
        assert_eq!(state.fencing_epoch, 2);
        assert_eq!(state.state, DeploymentRefreshRunState::Abandoned);
        assert_eq!(state.run_members, 2);
        assert_eq!(state.candidate_members, 1);
        assert_eq!(
            state.failure_stage,
            Some(DeploymentRefreshFailureStage::Publishing)
        );
        assert_eq!(
            state.failure_category,
            Some(DeploymentRefreshFailureCategory::Internal)
        );
        let diagnostic = state.failure_diagnostic.unwrap();
        assert!(!diagnostic.contains("/conary/private"));
        assert!(!diagnostic.contains("user:pass"));
        assert!(!diagnostic.contains("second:secret"));
        assert!(!diagnostic.contains("abc.def"));
        assert!(!diagnostic.contains("id_ed25519"));
        assert!(!diagnostic.contains("secret"));
        assert!(!diagnostic.contains("TOKEN=second"));
        assert!(diagnostic.contains("[REDACTED-PATH]"));
        assert!(diagnostic.contains("https://[REDACTED]@example.invalid/source"));
        assert!(diagnostic.contains("Bearer [REDACTED]"));
        assert!(diagnostic.contains("TOKEN=[REDACTED]"));
        assert!(state.failure_evidence_sha256.is_some());
        assert!(state.redactions.len() >= 2);
    }

    #[test]
    fn missing_profile_run_is_explicitly_absent() {
        assert_eq!(
            latest_profile_refresh(&connection(), "ubuntu-26.04").unwrap(),
            None
        );
    }
}
