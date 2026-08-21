// conary-core/src/repository/sync/remi/run.rs

//! Durable sparse-sync run ownership, recovery, and fenced publication.

use crate::db::models::{Repository, RepositoryPackage};
use crate::error::{Error, Result};
use crate::repository::remi_metadata::RemiSparseRevision;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use super::super::types::SyncedPackageRow;
use super::super::{current_timestamp, link_canonical_ids};
use super::append_synced_package_rows;

/// One page fetch can make three 300-second attempts. Keep the durable lease
/// beyond that exact retry envelope; each successfully persisted page renews
/// it. Recovery is authorized by this lease, never by a staging name or age.
const REMI_SYNC_LEASE_SECONDS: i64 = 1_200;

static PROCESS_INSTANCE_UUID: LazyLock<String> = LazyLock::new(|| uuid::Uuid::new_v4().to_string());

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RemiSyncRun {
    pub(super) run_id: String,
    pub(super) repository_id: i64,
    pub(super) staging_repository_id: i64,
    pub(super) owner_instance_uuid: String,
    pub(super) fencing_epoch: i64,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum RemiSyncFailureStage {
    FetchingObjects,
    Ingesting,
    Publishing,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum RemiSyncFailureCategory {
    Transport,
    WireContract,
    Database,
    Fenced,
    Internal,
}

impl RemiSyncFailureCategory {
    pub(super) fn from_error(error: &Error) -> Self {
        match error {
            Error::DownloadError(_) | Error::HttpStatus { .. } | Error::TimeoutError(_) => {
                Self::Transport
            }
            Error::ParseError(_) | Error::ConfigError(_) | Error::Json(_) => Self::WireContract,
            Error::Database(_) => Self::Database,
            Error::ConflictError(_) => Self::Fenced,
            _ => Self::Internal,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Transport => "transport",
            Self::WireContract => "wire_contract",
            Self::Database => "database",
            Self::Fenced => "fenced",
            Self::Internal => "internal",
        }
    }
}

impl RemiSyncFailureStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::FetchingObjects => "fetching_objects",
            Self::Ingesting => "ingesting",
            Self::Publishing => "publishing",
        }
    }
}

pub(super) fn begin_remi_sync_run(conn: &Connection, repo: &Repository) -> Result<RemiSyncRun> {
    begin_remi_sync_run_at(conn, repo, &PROCESS_INSTANCE_UUID, unix_seconds()?)
}

fn begin_remi_sync_run_at(
    conn: &Connection,
    repo: &Repository,
    owner_instance_uuid: &str,
    now: i64,
) -> Result<RemiSyncRun> {
    let repository_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let current = tx
        .query_row(
            "SELECT scope.fencing_epoch, scope.current_run_id, scope.active_revision,
                    run.state, run.lease_expires_at, run.staging_repository_id
             FROM repository_sync_scopes scope
             JOIN repository_sync_runs run ON run.run_id = scope.current_run_id
             WHERE scope.repository_id = ?1",
            [repository_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()?;

    let (prior_epoch, input_revision) = match current {
        Some((epoch, run_id, active_revision, state, lease_expires_at, staging_repository_id)) => {
            if !is_terminal_state(&state) {
                if lease_expires_at > now {
                    return Err(Error::ConflictError(format!(
                        "repository {repository_id} sparse sync run {run_id} owns fencing epoch \
                         {epoch} until {lease_expires_at}"
                    )));
                }
                abandon_expired_run(&tx, &run_id, staging_repository_id, now, epoch)?;
            }
            (epoch, active_revision)
        }
        None => (0, None),
    };
    let fencing_epoch = prior_epoch.checked_add(1).ok_or_else(|| {
        Error::InternalError("repository sync fencing epoch overflow".to_string())
    })?;
    let run_id = uuid::Uuid::new_v4().to_string();
    let lease_expires_at = lease_expiry(now)?;

    let mut staging = repo.clone();
    staging.id = None;
    staging.name = format!("remi-sync-run-{run_id}");
    staging.enabled = false;
    staging.last_sync = None;
    staging.created_at = None;
    staging.tuf_enabled = false;
    staging.tuf_root_version = None;
    staging.tuf_root_url = None;
    let staging_repository_id = staging.insert(&tx)?;

    tx.execute(
        "INSERT INTO repository_sync_runs (
             run_id, repository_id, scope_kind, scope_identity, owner_instance_uuid,
             fencing_epoch, staging_repository_id, input_revision, state, started_at,
             heartbeat_at, lease_expires_at
         ) VALUES (?1, ?2, 'repository', ?3, ?4, ?5, ?6, ?7, 'created', ?8, ?8, ?9)",
        params![
            run_id,
            repository_id,
            repository_id.to_string(),
            owner_instance_uuid,
            fencing_epoch,
            staging_repository_id,
            input_revision,
            now,
            lease_expires_at,
        ],
    )?;
    tx.execute(
        "INSERT INTO repository_sync_scopes (
             repository_id, fencing_epoch, current_run_id, active_revision
         ) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(repository_id) DO UPDATE SET
             fencing_epoch = excluded.fencing_epoch,
             current_run_id = excluded.current_run_id,
             active_revision = excluded.active_revision",
        params![repository_id, fencing_epoch, run_id, input_revision],
    )?;
    transition_owned_run(
        &tx,
        &run_id,
        fencing_epoch,
        owner_instance_uuid,
        "created",
        "fetching_objects",
        now,
    )?;
    tx.commit()?;

    Ok(RemiSyncRun {
        run_id,
        repository_id,
        staging_repository_id,
        owner_instance_uuid: owner_instance_uuid.to_string(),
        fencing_epoch,
    })
}

pub(super) fn append_remi_sync_page(
    conn: &Connection,
    run: &RemiSyncRun,
    mut rows: Vec<SyncedPackageRow>,
    revision: RemiSparseRevision,
) -> Result<usize> {
    for row in &mut rows {
        row.package.repository_id = run.staging_repository_id;
    }
    let now = unix_seconds()?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    renew_owned_run(&tx, run, now, Some(revision))?;
    let count = append_synced_package_rows(&tx, rows)?;
    tx.commit()?;
    Ok(count)
}

pub(super) fn finish_remi_sync_run(
    conn: &Connection,
    repo: &mut Repository,
    run: &RemiSyncRun,
    revision: RemiSparseRevision,
) -> Result<usize> {
    prepare_publication(conn, run, revision)?;

    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    require_owned_run(&tx, run, "ready_to_publish")?;
    let candidate_revision = serde_json::to_string(&revision)?;
    let stored_revision = tx.query_row(
        "SELECT candidate_revision FROM repository_sync_runs WHERE run_id = ?1",
        [&run.run_id],
        |row| row.get::<_, Option<String>>(0),
    )?;
    if stored_revision.as_deref() != Some(candidate_revision.as_str()) {
        return Err(Error::ConflictError(format!(
            "repository {} sparse sync run {} candidate revision changed before publication",
            run.repository_id, run.run_id
        )));
    }

    let count = tx.query_row(
        "SELECT COUNT(*) FROM repository_packages WHERE repository_id = ?1",
        [run.staging_repository_id],
        |row| row.get::<_, i64>(0),
    )?;
    let count = usize::try_from(count)
        .map_err(|_| Error::InitError("sparse sync package count exceeds usize".to_string()))?;
    RepositoryPackage::delete_by_repository(&tx, run.repository_id)?;
    tx.execute(
        "UPDATE repository_packages SET repository_id = ?1 WHERE repository_id = ?2",
        [run.repository_id, run.staging_repository_id],
    )?;
    Repository::delete(&tx, run.staging_repository_id)?;
    link_canonical_ids(&tx, run.repository_id)?;
    repo.last_sync = Some(current_timestamp());
    repo.update(&tx)?;

    let now = unix_seconds()?;
    let updated = tx.execute(
        "UPDATE repository_sync_runs
         SET state = 'published', heartbeat_at = ?1, lease_expires_at = ?1,
             finished_at = ?1
         WHERE run_id = ?2
           AND owner_instance_uuid = ?3
           AND fencing_epoch = ?4
           AND state = 'ready_to_publish'",
        params![now, run.run_id, run.owner_instance_uuid, run.fencing_epoch],
    )?;
    if updated != 1 {
        return Err(fenced_error(run, "publication state changed"));
    }
    let updated = tx.execute(
        "UPDATE repository_sync_scopes
         SET active_revision = ?1
         WHERE repository_id = ?2
           AND current_run_id = ?3
           AND fencing_epoch = ?4",
        params![
            candidate_revision,
            run.repository_id,
            run.run_id,
            run.fencing_epoch
        ],
    )?;
    if updated != 1 {
        return Err(fenced_error(run, "publication fence changed"));
    }
    tx.commit()?;
    Ok(count)
}

pub(super) fn abort_remi_sync_run(
    conn: &Connection,
    run: &RemiSyncRun,
    stage: RemiSyncFailureStage,
    category: RemiSyncFailureCategory,
    evidence: &str,
) -> Result<()> {
    let now = unix_seconds()?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let state = tx
        .query_row(
            "SELECT state FROM repository_sync_runs WHERE run_id = ?1",
            [&run.run_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if state.as_deref().is_some_and(is_terminal_state) {
        tx.commit()?;
        return Ok(());
    }
    let updated = tx.execute(
        "UPDATE repository_sync_runs
         SET state = 'failed', heartbeat_at = ?1, lease_expires_at = ?1,
             finished_at = ?1, failure_stage = ?2, failure_category = ?3,
             failure_evidence = ?4
         WHERE run_id = ?5
           AND owner_instance_uuid = ?6
           AND fencing_epoch = ?7
           AND state NOT IN ('published', 'failed', 'abandoned')",
        params![
            now,
            stage.as_str(),
            category.as_str(),
            evidence,
            run.run_id,
            run.owner_instance_uuid,
            run.fencing_epoch,
        ],
    )?;
    if updated == 1 {
        Repository::delete(&tx, run.staging_repository_id)?;
        tx.execute(
            "UPDATE repository_sync_runs SET state = 'abandoned' WHERE run_id = ?1",
            [&run.run_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn prepare_publication(
    conn: &Connection,
    run: &RemiSyncRun,
    revision: RemiSparseRevision,
) -> Result<()> {
    let now = unix_seconds()?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    require_owned_run(&tx, run, "fetching_objects")?;
    let candidate_revision = serde_json::to_string(&revision)?;
    let stored_revision = tx.query_row(
        "SELECT candidate_revision FROM repository_sync_runs WHERE run_id = ?1",
        [&run.run_id],
        |row| row.get::<_, Option<String>>(0),
    )?;
    if stored_revision.as_deref() != Some(candidate_revision.as_str()) {
        return Err(Error::ConflictError(format!(
            "repository {} sparse sync run {} did not retain one candidate revision",
            run.repository_id, run.run_id
        )));
    }
    transition_owned_run(
        &tx,
        &run.run_id,
        run.fencing_epoch,
        &run.owner_instance_uuid,
        "fetching_objects",
        "validating",
        now,
    )?;
    transition_owned_run(
        &tx,
        &run.run_id,
        run.fencing_epoch,
        &run.owner_instance_uuid,
        "validating",
        "ready_to_publish",
        now,
    )?;
    tx.commit()?;
    Ok(())
}

fn renew_owned_run(
    tx: &Transaction<'_>,
    run: &RemiSyncRun,
    now: i64,
    revision: Option<RemiSparseRevision>,
) -> Result<()> {
    require_owned_run(tx, run, "fetching_objects")?;
    let revision = revision
        .map(|value| serde_json::to_string(&value))
        .transpose()?;
    let updated = tx.execute(
        "UPDATE repository_sync_runs
         SET heartbeat_at = ?1, lease_expires_at = ?2,
             candidate_revision = COALESCE(candidate_revision, ?3)
         WHERE run_id = ?4
           AND owner_instance_uuid = ?5
           AND fencing_epoch = ?6
           AND state = 'fetching_objects'
           AND (candidate_revision IS NULL OR candidate_revision = ?3)",
        params![
            now,
            lease_expiry(now)?,
            revision,
            run.run_id,
            run.owner_instance_uuid,
            run.fencing_epoch,
        ],
    )?;
    if updated != 1 {
        return Err(fenced_error(
            run,
            "heartbeat or candidate revision was rejected",
        ));
    }
    Ok(())
}

fn require_owned_run(tx: &Transaction<'_>, run: &RemiSyncRun, state: &str) -> Result<()> {
    let owned = tx.query_row(
        "SELECT EXISTS(
             SELECT 1
             FROM repository_sync_scopes scope
             JOIN repository_sync_runs current ON current.run_id = scope.current_run_id
             WHERE scope.repository_id = ?1
               AND scope.current_run_id = ?2
               AND scope.fencing_epoch = ?3
               AND current.owner_instance_uuid = ?4
               AND current.state = ?5
         )",
        params![
            run.repository_id,
            run.run_id,
            run.fencing_epoch,
            run.owner_instance_uuid,
            state,
        ],
        |row| row.get::<_, bool>(0),
    )?;
    if !owned {
        return Err(fenced_error(run, "ownership proof failed"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn transition_owned_run(
    tx: &Transaction<'_>,
    run_id: &str,
    fencing_epoch: i64,
    owner_instance_uuid: &str,
    from: &str,
    to: &str,
    now: i64,
) -> Result<()> {
    let updated = tx.execute(
        "UPDATE repository_sync_runs
         SET state = ?1, heartbeat_at = ?2, lease_expires_at = ?3
         WHERE run_id = ?4
           AND owner_instance_uuid = ?5
           AND fencing_epoch = ?6
           AND state = ?7",
        params![
            to,
            now,
            lease_expiry(now)?,
            run_id,
            owner_instance_uuid,
            fencing_epoch,
            from,
        ],
    )?;
    if updated != 1 {
        return Err(Error::ConflictError(format!(
            "repository sync run {run_id} cannot transition from {from} to {to} under fencing \
             epoch {fencing_epoch}"
        )));
    }
    Ok(())
}

fn abandon_expired_run(
    tx: &Transaction<'_>,
    run_id: &str,
    staging_repository_id: i64,
    now: i64,
    fencing_epoch: i64,
) -> Result<()> {
    let evidence = format!(
        "durable lease for fencing epoch {fencing_epoch} expired before successor acquisition"
    );
    let updated = tx.execute(
        "UPDATE repository_sync_runs
         SET state = 'failed', heartbeat_at = ?1, lease_expires_at = ?1,
             finished_at = ?1, failure_stage = 'publishing',
             failure_category = 'fenced', failure_evidence = ?2
         WHERE run_id = ?3
           AND state NOT IN ('published', 'failed', 'abandoned')
           AND lease_expires_at <= ?1",
        params![now, evidence, run_id],
    )?;
    if updated != 1 {
        return Err(Error::ConflictError(format!(
            "repository sync run {run_id} is not durably abandoned"
        )));
    }
    Repository::delete(tx, staging_repository_id)?;
    tx.execute(
        "UPDATE repository_sync_runs SET state = 'abandoned' WHERE run_id = ?1",
        [run_id],
    )?;
    Ok(())
}

fn is_terminal_state(state: &str) -> bool {
    matches!(state, "published" | "failed" | "abandoned")
}

fn fenced_error(run: &RemiSyncRun, reason: &str) -> Error {
    Error::ConflictError(format!(
        "repository {} sparse sync run {} lost fencing epoch {}: {reason}",
        run.repository_id, run.run_id, run.fencing_epoch
    ))
}

fn lease_expiry(now: i64) -> Result<i64> {
    now.checked_add(REMI_SYNC_LEASE_SECONDS)
        .ok_or_else(|| Error::InternalError("repository sync lease expiry overflow".to_string()))
}

fn unix_seconds() -> Result<i64> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| Error::InternalError(format!("system time precedes Unix epoch: {error}")))?
        .as_secs();
    i64::try_from(seconds)
        .map_err(|_| Error::InternalError("system time exceeds SQLite integer range".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_repo(conn: &Connection) -> Repository {
        let mut repo = Repository::new("fedora-remi".to_string(), "https://remi.test".to_string());
        repo.default_strategy = Some("remi".to_string());
        repo.default_strategy_endpoint = Some("https://remi.test".to_string());
        repo.source_profile = Some("fedora-44".to_string());
        repo.id = Some(repo.insert(conn).unwrap());
        repo
    }

    fn run_state(conn: &Connection, run: &RemiSyncRun) -> String {
        conn.query_row(
            "SELECT state FROM repository_sync_runs WHERE run_id = ?1",
            [&run.run_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn active_durable_lease_rejects_a_concurrent_sync() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::ensure_current(&conn).unwrap();
        let repo = test_repo(&conn);
        let first =
            begin_remi_sync_run_at(&conn, &repo, "00000000-0000-4000-8000-000000000001", 100)
                .unwrap();

        let error =
            begin_remi_sync_run_at(&conn, &repo, "00000000-0000-4000-8000-000000000002", 101)
                .unwrap_err();

        assert!(error.to_string().contains("owns fencing epoch 1"));
        assert_eq!(run_state(&conn, &first), "fetching_objects");
        assert_eq!(Repository::list_all(&conn).unwrap().len(), 2);
    }

    #[test]
    fn expired_lease_recovers_only_its_exact_stage_and_fences_the_old_owner() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("restart-recovery.db");
        crate::db::init(&db_path).unwrap();
        let conn = crate::db::open_fast(&db_path).unwrap();
        let repo = test_repo(&conn);
        let first =
            begin_remi_sync_run_at(&conn, &repo, "00000000-0000-4000-8000-000000000001", 100)
                .unwrap();
        let mut unrelated = repo.clone();
        unrelated.id = None;
        unrelated.name = "remi-sync-run-unowned-old-looking-candidate".to_string();
        unrelated.enabled = false;
        let unrelated_id = unrelated.insert(&conn).unwrap();
        drop(conn);

        let conn = crate::db::open_fast(&db_path).unwrap();

        let second = begin_remi_sync_run_at(
            &conn,
            &repo,
            "00000000-0000-4000-8000-000000000002",
            100 + REMI_SYNC_LEASE_SECONDS,
        )
        .unwrap();

        assert_eq!(second.fencing_epoch, 2);
        assert_eq!(run_state(&conn, &first), "abandoned");
        assert!(
            Repository::find_by_id(&conn, first.staging_repository_id)
                .unwrap()
                .is_none()
        );
        assert!(
            Repository::find_by_id(&conn, unrelated_id)
                .unwrap()
                .is_some()
        );
        let error =
            prepare_publication(&conn, &first, RemiSparseRevision { sequence: 1 }).unwrap_err();
        assert!(error.to_string().contains("lost fencing epoch 1"));
    }
}
