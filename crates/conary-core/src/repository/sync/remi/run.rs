// conary-core/src/repository/sync/remi/run.rs

//! Durable profile-refresh ownership, recovery, and fenced publication.
//!
//! This module owns only the small operational coordinator for immutable Remi
//! profile catalogs. Catalog bytes and package rows never live in this run;
//! the member rows bind the exact repositories and source manifests that a
//! private filesystem candidate produced.

use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use std::time::{SystemTime, UNIX_EPOCH};

mod recovery;

#[cfg(test)]
use recovery::recover_expired_profile_sync_runs_at;
pub use recovery::{
    ProfileSyncRunRecovery, acknowledge_profile_sync_candidate_cleanup,
    recover_expired_profile_sync_runs,
};
use recovery::{abandon_expired_run, pending_candidate_recovery};

/// One page fetch can make three 300-second attempts. Keep the durable lease
/// beyond that exact retry envelope; each successfully persisted coordinator
/// event renews it. Recovery is authorized by this lease, never by a path or
/// name prefix.
const REMI_SYNC_LEASE_SECONDS: i64 = 1_200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSyncRun {
    pub run_id: String,
    pub source_profile: String,
    pub owner_instance_uuid: String,
    pub fencing_epoch: i64,
    /// Exact abandoned runs whose private candidate paths require idempotent
    /// recovery. The list comes from durable run state, never path discovery.
    pub recovery_run_ids: Vec<String>,
}

pub type RemiSyncRun = ProfileSyncRun;

/// Exact ordered member binding recorded by a profile refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSyncRunMember {
    pub ordinal: i64,
    pub repository_id: i64,
    pub source_identity: String,
    pub repository_identity: String,
    pub stream_kind: String,
    pub stream_identity: String,
    pub priority: i64,
    pub required: bool,
    pub input_source_snapshot_sha256: Option<String>,
    pub candidate_source_snapshot_sha256: Option<String>,
}

pub type RemiSyncRunMember = ProfileSyncRunMember;

#[derive(Debug, Clone, Copy)]
pub enum ProfileSyncFailureStage {
    FetchingObjects,
    Ingesting,
    Publishing,
}

#[derive(Debug, Clone, Copy)]
pub enum ProfileSyncFailureCategory {
    Transport,
    WireContract,
    Database,
    Fenced,
    Internal,
}

pub type RemiSyncFailureStage = ProfileSyncFailureStage;
pub type RemiSyncFailureCategory = ProfileSyncFailureCategory;

impl ProfileSyncFailureCategory {
    pub fn from_error(error: &Error) -> Self {
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

impl ProfileSyncFailureStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::FetchingObjects => "fetching_objects",
            Self::Ingesting => "ingesting",
            Self::Publishing => "publishing",
        }
    }
}

/// Begin a profile-scoped refresh using the active profile revision as its
/// exact input digest. The first refresh may legitimately have no input.
pub fn begin_profile_sync_run(
    conn: &Connection,
    source_profile: &str,
    owner_instance_uuid: &str,
) -> Result<RemiSyncRun> {
    validate_profile(source_profile)?;
    validate_uuid(owner_instance_uuid, "sync run owner instance UUID")?;
    begin_profile_sync_run_internal(conn, source_profile, None, false, owner_instance_uuid)
}

/// Begin a profile-scoped refresh while asserting the caller's exact input
/// profile digest. This is the API used when a caller already resolved the
/// active pointer before starting private catalog work.
pub fn begin_profile_sync_run_with_input(
    conn: &Connection,
    source_profile: &str,
    input_profile_digest: Option<&str>,
    owner_instance_uuid: &str,
) -> Result<RemiSyncRun> {
    validate_profile(source_profile)?;
    validate_uuid(owner_instance_uuid, "sync run owner instance UUID")?;
    if let Some(digest) = input_profile_digest {
        validate_digest(digest, "sync run input profile digest")?;
    }
    begin_profile_sync_run_internal(
        conn,
        source_profile,
        input_profile_digest,
        true,
        owner_instance_uuid,
    )
}

fn begin_profile_sync_run_internal(
    conn: &Connection,
    source_profile: &str,
    input_profile_digest: Option<&str>,
    assert_input: bool,
    owner_instance_uuid: &str,
) -> Result<RemiSyncRun> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let now = unix_seconds()?;
    begin_profile_sync_run_in_transaction(
        tx,
        source_profile,
        input_profile_digest,
        assert_input,
        owner_instance_uuid,
        now,
    )
}

#[cfg(test)]
fn begin_profile_sync_run_at(
    conn: &Connection,
    source_profile: &str,
    input_profile_digest: Option<&str>,
    owner_instance_uuid: &str,
    now: i64,
) -> Result<RemiSyncRun> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    begin_profile_sync_run_in_transaction(
        tx,
        source_profile,
        input_profile_digest,
        true,
        owner_instance_uuid,
        now,
    )
}

fn begin_profile_sync_run_in_transaction(
    tx: Transaction<'_>,
    source_profile: &str,
    requested_input_digest: Option<&str>,
    assert_input: bool,
    owner_instance_uuid: &str,
    now: i64,
) -> Result<RemiSyncRun> {
    let active_input_digest = tx
        .query_row(
            "SELECT profile_revision_sha256
             FROM remi_active_profile_revisions
             WHERE source_profile = ?1",
            [source_profile],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if assert_input && requested_input_digest != active_input_digest.as_deref() {
        return Err(Error::ConflictError(format!(
            "profile {source_profile} input digest does not match its active revision"
        )));
    }
    let input_profile_digest = requested_input_digest.or(active_input_digest.as_deref());

    let current = tx
        .query_row(
            "SELECT scope.fencing_epoch, scope.current_run_id,
                    run.state, run.lease_expires_at
             FROM repository_sync_scopes scope
             JOIN repository_sync_runs run ON run.run_id = scope.current_run_id
             WHERE scope.source_profile = ?1",
            [source_profile],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()?;

    let prior_epoch = match current {
        Some((epoch, run_id, state, lease_expires_at)) => {
            if !is_terminal_state(&state) {
                if lease_expires_at > now {
                    return Err(Error::ConflictError(format!(
                        "profile {source_profile} sync run {run_id} owns fencing epoch \
                         {epoch} until {lease_expires_at}"
                    )));
                }
                abandon_expired_run(
                    &tx,
                    source_profile,
                    &run_id,
                    now,
                    epoch,
                    "successor acquisition",
                )?;
            }
            epoch
        }
        None => 0,
    };
    let fencing_epoch = prior_epoch.checked_add(1).ok_or_else(|| {
        Error::InternalError("repository sync fencing epoch overflow".to_string())
    })?;
    let run_id = uuid::Uuid::new_v4().to_string();
    let lease_expires_at = lease_expiry(now)?;

    tx.execute(
        "INSERT INTO repository_sync_runs (
             run_id, source_profile, owner_instance_uuid, fencing_epoch,
             input_profile_digest, state, started_at, heartbeat_at, lease_expires_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'created', ?6, ?6, ?7)",
        params![
            run_id,
            source_profile,
            owner_instance_uuid,
            fencing_epoch,
            input_profile_digest,
            now,
            lease_expires_at,
        ],
    )?;
    tx.execute(
        "INSERT INTO repository_sync_scopes (
             source_profile, fencing_epoch, current_run_id
         ) VALUES (?1, ?2, ?3)
         ON CONFLICT(source_profile) DO UPDATE SET
             fencing_epoch = excluded.fencing_epoch,
             current_run_id = excluded.current_run_id",
        params![source_profile, fencing_epoch, run_id],
    )?;
    let recovery_run_ids = pending_candidate_recovery(&tx, Some(source_profile))?
        .into_iter()
        .map(|recovery| recovery.run_id)
        .collect();
    tx.commit()?;

    Ok(RemiSyncRun {
        run_id,
        source_profile: source_profile.to_string(),
        owner_instance_uuid: owner_instance_uuid.to_string(),
        fencing_epoch,
        recovery_run_ids,
    })
}

/// Begin a profile run and record its ordered member bindings in one logical
/// operation. Member persistence remains individually fenced so a resumed
/// worker can safely replay an identical member.
pub fn begin_profile_sync_run_with_members(
    conn: &Connection,
    source_profile: &str,
    input_profile_digest: Option<&str>,
    owner_instance_uuid: &str,
    members: &[RemiSyncRunMember],
) -> Result<RemiSyncRun> {
    let run = begin_profile_sync_run_with_input(
        conn,
        source_profile,
        input_profile_digest,
        owner_instance_uuid,
    )?;
    for member in members {
        if let Err(error) = record_profile_sync_run_member(conn, &run, member) {
            let _ = abort_profile_sync_run(
                conn,
                &run,
                ProfileSyncFailureStage::Ingesting,
                ProfileSyncFailureCategory::from_error(&error),
                &error.to_string(),
            );
            return Err(error);
        }
    }
    Ok(run)
}

/// Renew a live profile run without changing its candidate identity.
pub fn heartbeat_profile_sync_run(conn: &Connection, run: &RemiSyncRun) -> Result<()> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let now = unix_seconds()?;
    require_owned_run(&tx, run, "created", "fetching_objects", now)?;
    touch_owned_run(&tx, run, now)?;
    tx.commit()?;
    Ok(())
}

/// Record one exact ordered source member and its candidate source snapshot.
pub fn record_profile_sync_run_member(
    conn: &Connection,
    run: &RemiSyncRun,
    member: &RemiSyncRunMember,
) -> Result<()> {
    validate_member(member)?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let now = unix_seconds()?;
    require_owned_run(&tx, run, "created", "fetching_objects", now)?;
    let existing = tx
        .query_row(
            "SELECT ordinal, repository_id, source_identity, repository_identity,
                    stream_kind, stream_identity, priority, required,
                    input_source_snapshot_sha256,
                    candidate_source_snapshot_sha256
             FROM repository_sync_run_members
             WHERE run_id = ?1 AND ordinal = ?2",
            params![&run.run_id, member.ordinal],
            RemiSyncRunMember::from_row,
        )
        .optional()?;
    match existing {
        Some(stored) => {
            if stored.immutable_part() != member.immutable_part()
                || (stored.input_source_snapshot_sha256.is_some()
                    && stored.input_source_snapshot_sha256 != member.input_source_snapshot_sha256)
                || (stored.candidate_source_snapshot_sha256.is_some()
                    && stored.candidate_source_snapshot_sha256
                        != member.candidate_source_snapshot_sha256)
            {
                return Err(Error::ConflictError(format!(
                    "profile {} run {} member {} changed after recording",
                    run.source_profile, run.run_id, member.ordinal
                )));
            }
            if stored.candidate_source_snapshot_sha256.is_none()
                && member.candidate_source_snapshot_sha256.is_some()
            {
                tx.execute(
                    "UPDATE repository_sync_run_members
                     SET candidate_source_snapshot_sha256 = ?1
                     WHERE run_id = ?2 AND ordinal = ?3",
                    params![
                        &member.candidate_source_snapshot_sha256,
                        &run.run_id,
                        member.ordinal
                    ],
                )?;
            }
            if stored.input_source_snapshot_sha256.is_none()
                && member.input_source_snapshot_sha256.is_some()
            {
                tx.execute(
                    "UPDATE repository_sync_run_members
                     SET input_source_snapshot_sha256 = ?1
                     WHERE run_id = ?2 AND ordinal = ?3",
                    params![
                        &member.input_source_snapshot_sha256,
                        &run.run_id,
                        member.ordinal
                    ],
                )?;
            }
        }
        None => {
            tx.execute(
                "INSERT INTO repository_sync_run_members (
                     run_id, ordinal, repository_id, source_identity,
                     repository_identity, stream_kind, stream_identity,
                     priority, required, input_source_snapshot_sha256,
                     candidate_source_snapshot_sha256
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    &run.run_id,
                    member.ordinal,
                    member.repository_id,
                    &member.source_identity,
                    &member.repository_identity,
                    &member.stream_kind,
                    &member.stream_identity,
                    member.priority,
                    member.required as i64,
                    &member.input_source_snapshot_sha256,
                    &member.candidate_source_snapshot_sha256,
                ],
            )?;
        }
    }
    touch_owned_run(&tx, run, now)?;
    tx.commit()?;
    Ok(())
}

/// Mark a profile candidate ready to activate after every required member has
/// supplied one exact source snapshot digest.
pub fn ready_profile_sync_run(
    conn: &Connection,
    run: &RemiSyncRun,
    candidate_profile_digest: &str,
) -> Result<()> {
    validate_digest(
        candidate_profile_digest,
        "sync run candidate profile digest",
    )?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let now = unix_seconds()?;
    require_owned_run(&tx, run, "created", "fetching_objects", now)?;
    let (member_count, missing_required): (i64, i64) = tx.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(CASE WHEN required = 1
                                      AND candidate_source_snapshot_sha256 IS NULL
                                  THEN 1 ELSE 0 END), 0)
         FROM repository_sync_run_members
         WHERE run_id = ?1",
        [&run.run_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if member_count == 0 {
        return Err(Error::ConflictError(format!(
            "profile {} run {} has no source members",
            run.source_profile, run.run_id
        )));
    }
    if missing_required != 0 {
        return Err(Error::ConflictError(format!(
            "profile {} run {} has {missing_required} required members without candidate source snapshots",
            run.source_profile, run.run_id
        )));
    }
    let updated = tx.execute(
        "UPDATE repository_sync_runs
         SET state = 'ready_to_publish', candidate_profile_digest = ?1,
             heartbeat_at = ?2, lease_expires_at = ?3
         WHERE run_id = ?4
           AND source_profile = ?5
           AND owner_instance_uuid = ?6
           AND fencing_epoch = ?7
           AND state IN ('created', 'fetching_objects')
           AND lease_expires_at > ?2",
        params![
            candidate_profile_digest,
            now,
            lease_expiry(now)?,
            &run.run_id,
            &run.source_profile,
            &run.owner_instance_uuid,
            run.fencing_epoch,
        ],
    )?;
    if updated != 1 {
        return Err(fenced_error(run, "ready transition was rejected"));
    }
    tx.commit()?;
    Ok(())
}

/// Abort a profile run under its exact lease. No repository or package rows
/// are deleted because this coordinator never owns candidate row storage.
pub fn abort_profile_sync_run(
    conn: &Connection,
    run: &RemiSyncRun,
    stage: RemiSyncFailureStage,
    category: RemiSyncFailureCategory,
    evidence: &str,
) -> Result<()> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let now = unix_seconds()?;
    let state = tx
        .query_row(
            "SELECT state FROM repository_sync_runs
             WHERE run_id = ?1 AND source_profile = ?2
               AND owner_instance_uuid = ?3 AND fencing_epoch = ?4",
            params![
                &run.run_id,
                &run.source_profile,
                &run.owner_instance_uuid,
                run.fencing_epoch
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(state) = state else {
        return Err(fenced_error(run, "run does not exist"));
    };
    if is_terminal_state(&state) {
        tx.commit()?;
        return Ok(());
    }
    require_owned_run(&tx, run, &state, "", now)?;
    let updated = tx.execute(
        "UPDATE repository_sync_runs
         SET state = 'abandoned', heartbeat_at = ?1, lease_expires_at = ?1,
             finished_at = ?1, failure_stage = ?2, failure_category = ?3,
             failure_evidence = ?4
         WHERE run_id = ?5
           AND source_profile = ?6
           AND owner_instance_uuid = ?7
           AND fencing_epoch = ?8
           AND state NOT IN ('published', 'failed', 'abandoned')
           AND lease_expires_at > ?1",
        params![
            now,
            stage.as_str(),
            category.as_str(),
            evidence,
            &run.run_id,
            &run.source_profile,
            &run.owner_instance_uuid,
            run.fencing_epoch,
        ],
    )?;
    if updated != 1 {
        return Err(fenced_error(run, "abort was rejected"));
    }
    tx.commit()?;
    Ok(())
}

fn require_owned_run(
    tx: &Transaction<'_>,
    run: &RemiSyncRun,
    first_state: &str,
    second_state: &str,
    now: i64,
) -> Result<()> {
    let owned = tx.query_row(
        "SELECT EXISTS(
             SELECT 1
             FROM repository_sync_scopes scope
             JOIN repository_sync_runs current ON current.run_id = scope.current_run_id
             WHERE scope.source_profile = ?1
               AND scope.current_run_id = ?2
               AND scope.fencing_epoch = ?3
               AND current.source_profile = ?1
               AND current.owner_instance_uuid = ?4
               AND (?5 = '' OR current.state = ?5 OR current.state = ?6)
               AND current.lease_expires_at > ?7
         )",
        params![
            &run.source_profile,
            &run.run_id,
            run.fencing_epoch,
            &run.owner_instance_uuid,
            first_state,
            second_state,
            now,
        ],
        |row| row.get::<_, bool>(0),
    )?;
    if !owned {
        return Err(fenced_error(run, "ownership proof failed"));
    }
    Ok(())
}

fn touch_owned_run(tx: &Transaction<'_>, run: &RemiSyncRun, now: i64) -> Result<()> {
    let updated = tx.execute(
        "UPDATE repository_sync_runs
         SET state = CASE WHEN state = 'created' THEN 'fetching_objects' ELSE state END,
             heartbeat_at = ?1, lease_expires_at = ?2
         WHERE run_id = ?3
           AND source_profile = ?4
           AND owner_instance_uuid = ?5
           AND fencing_epoch = ?6
           AND state IN ('created', 'fetching_objects')
           AND lease_expires_at > ?1",
        params![
            now,
            lease_expiry(now)?,
            &run.run_id,
            &run.source_profile,
            &run.owner_instance_uuid,
            run.fencing_epoch,
        ],
    )?;
    if updated != 1 {
        return Err(fenced_error(run, "heartbeat was rejected"));
    }
    Ok(())
}

impl RemiSyncRunMember {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            ordinal: row.get(0)?,
            repository_id: row.get(1)?,
            source_identity: row.get(2)?,
            repository_identity: row.get(3)?,
            stream_kind: row.get(4)?,
            stream_identity: row.get(5)?,
            priority: row.get(6)?,
            required: row.get::<_, i64>(7)? != 0,
            input_source_snapshot_sha256: row.get(8)?,
            candidate_source_snapshot_sha256: row.get(9)?,
        })
    }

    fn immutable_part(&self) -> (&i64, &i64, &str, &str, &str, &str, &i64, bool) {
        (
            &self.ordinal,
            &self.repository_id,
            &self.source_identity,
            &self.repository_identity,
            &self.stream_kind,
            &self.stream_identity,
            &self.priority,
            self.required,
        )
    }
}

fn validate_member(member: &RemiSyncRunMember) -> Result<()> {
    if member.ordinal < 0 {
        return Err(Error::ConfigError(
            "sync run member ordinal must not be negative".to_string(),
        ));
    }
    if member.repository_id <= 0 {
        return Err(Error::ConfigError(
            "sync run member repository ID must be positive".to_string(),
        ));
    }
    validate_identity(&member.source_identity, "sync run member source identity")?;
    validate_identity(
        &member.repository_identity,
        "sync run member repository identity",
    )?;
    validate_identity(&member.stream_identity, "sync run member stream identity")?;
    if !matches!(
        member.stream_kind.as_str(),
        "release" | "channel" | "rolling"
    ) {
        return Err(Error::ConfigError(format!(
            "sync run member stream kind '{}' is unsupported",
            member.stream_kind
        )));
    }
    if let Some(digest) = member.candidate_source_snapshot_sha256.as_deref() {
        validate_digest(digest, "sync run candidate source snapshot digest")?;
    }
    if let Some(digest) = member.input_source_snapshot_sha256.as_deref() {
        validate_digest(digest, "sync run input source snapshot digest")?;
    }
    Ok(())
}

fn validate_profile(value: &str) -> Result<()> {
    validate_identity(value, "sync run source profile")
}

fn validate_identity(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 255
        || value.trim() != value
        || !value.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
    {
        return Err(Error::ConfigError(format!(
            "{label} must contain 1 to 255 printable ASCII characters without surrounding whitespace"
        )));
    }
    Ok(())
}

fn validate_digest(value: &str, label: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::ConfigError(format!(
            "{label} must be exactly 64 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

fn validate_uuid(value: &str, label: &str) -> Result<()> {
    if value.len() != 36 || uuid::Uuid::parse_str(value).is_err() {
        return Err(Error::ConfigError(format!(
            "{label} must be a canonical 36-character UUID"
        )));
    }
    Ok(())
}

fn is_terminal_state(state: &str) -> bool {
    matches!(state, "published" | "failed" | "abandoned")
}

fn fenced_error(run: &RemiSyncRun, reason: &str) -> Error {
    Error::ConflictError(format!(
        "profile {} sync run {} lost fencing epoch {}: {reason}",
        run.source_profile, run.run_id, run.fencing_epoch
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
    use crate::db::models::Repository;
    use crate::db::schema::ensure_current;

    const OWNER_ONE: &str = "00000000-0000-4000-8000-000000000001";
    const OWNER_TWO: &str = "00000000-0000-4000-8000-000000000002";

    fn digest(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    fn test_repo(conn: &Connection, name: &str, profile: &str) -> Repository {
        let mut repo = Repository::new(name.to_string(), "https://remi.test".to_string());
        repo.source_profile = Some(profile.to_string());
        repo.id = Some(repo.insert(conn).unwrap());
        repo
    }

    fn member(repository_id: i64, ordinal: i64, digest: Option<&str>) -> RemiSyncRunMember {
        RemiSyncRunMember {
            ordinal,
            repository_id,
            source_identity: format!("source-{ordinal}"),
            repository_identity: format!("repository-{ordinal}"),
            stream_kind: "release".to_string(),
            stream_identity: "fixture".to_string(),
            priority: ordinal,
            required: true,
            input_source_snapshot_sha256: None,
            candidate_source_snapshot_sha256: digest.map(str::to_string),
        }
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
    fn active_profile_lease_rejects_a_concurrent_run() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let first = begin_profile_sync_run_at(&conn, "fedora-44", None, OWNER_ONE, 100).unwrap();
        let error =
            begin_profile_sync_run_at(&conn, "fedora-44", None, OWNER_TWO, 101).unwrap_err();
        assert!(error.to_string().contains("owns fencing epoch 1"));
        assert_eq!(run_state(&conn, &first), "created");
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM repositories", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn expired_lease_recovers_only_its_exact_profile_run() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("restart-recovery.db");
        crate::db::init(&db_path).unwrap();
        let conn = crate::db::open_fast(&db_path).unwrap();
        let repo = test_repo(&conn, "unrelated", "ubuntu-26.04");
        let first = begin_profile_sync_run_at(&conn, "fedora-44", None, OWNER_ONE, 100).unwrap();
        drop(conn);

        let conn = crate::db::open_fast(&db_path).unwrap();
        let second = begin_profile_sync_run_at(
            &conn,
            "fedora-44",
            None,
            OWNER_TWO,
            100 + REMI_SYNC_LEASE_SECONDS,
        )
        .unwrap();
        assert_eq!(second.fencing_epoch, 2);
        assert_eq!(second.recovery_run_ids, vec![first.run_id.clone()]);
        assert_eq!(run_state(&conn, &first), "abandoned");
        assert!(
            Repository::find_by_id(&conn, repo.id.unwrap())
                .unwrap()
                .is_some()
        );
        let error = heartbeat_profile_sync_run(&conn, &first).unwrap_err();
        assert!(error.to_string().contains("lost fencing epoch 1"));
    }

    #[test]
    fn restart_recovery_fences_only_expired_runs_and_replays_cleanup_until_acknowledged() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let expired = begin_profile_sync_run_at(&conn, "fedora-44", None, OWNER_ONE, 100).unwrap();
        let live = begin_profile_sync_run_at(&conn, "ubuntu-26.04", None, OWNER_TWO, 500).unwrap();
        let recovery_time = 100 + REMI_SYNC_LEASE_SECONDS;

        let first = recover_expired_profile_sync_runs_at(&conn, recovery_time).unwrap();
        assert_eq!(
            first,
            vec![ProfileSyncRunRecovery {
                run_id: expired.run_id.clone(),
                source_profile: expired.source_profile.clone(),
            }]
        );
        assert_eq!(run_state(&conn, &expired), "abandoned");
        assert_eq!(run_state(&conn, &live), "created");

        assert_eq!(
            recover_expired_profile_sync_runs_at(&conn, recovery_time).unwrap(),
            first
        );
        assert!(acknowledge_profile_sync_candidate_cleanup(&conn, &expired.run_id).unwrap());
        assert!(!acknowledge_profile_sync_candidate_cleanup(&conn, &expired.run_id).unwrap());
        assert!(
            recover_expired_profile_sync_runs_at(&conn, recovery_time)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn member_binding_is_exact_and_ready_requires_required_candidates() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let repo = test_repo(&conn, "source", "fedora-44");
        let run =
            begin_profile_sync_run_at(&conn, "fedora-44", None, OWNER_ONE, unix_seconds().unwrap())
                .unwrap();
        record_profile_sync_run_member(&conn, &run, &member(repo.id.unwrap(), 0, None)).unwrap();
        let error = ready_profile_sync_run(&conn, &run, &digest('a')).unwrap_err();
        assert!(error.to_string().contains("required members"));

        let candidate_digest = digest('b');
        record_profile_sync_run_member(
            &conn,
            &run,
            &member(repo.id.unwrap(), 0, Some(&candidate_digest)),
        )
        .unwrap();
        let profile_digest = digest('c');
        ready_profile_sync_run(&conn, &run, &profile_digest).unwrap();
        let (state, candidate): (String, String) = conn
            .query_row(
                "SELECT state, candidate_profile_digest
                 FROM repository_sync_runs WHERE run_id = ?1",
                [&run.run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, "ready_to_publish");
        assert_eq!(candidate, profile_digest);
    }

    #[test]
    fn historical_run_member_does_not_block_repository_replacement() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let repo = test_repo(&conn, "source", "fedora-44");
        let run =
            begin_profile_sync_run_at(&conn, "fedora-44", None, OWNER_ONE, unix_seconds().unwrap())
                .unwrap();
        record_profile_sync_run_member(
            &conn,
            &run,
            &member(repo.id.unwrap(), 0, Some(&digest('a'))),
        )
        .unwrap();

        Repository::delete(&conn, repo.id.unwrap()).unwrap();

        assert!(
            Repository::find_by_id(&conn, repo.id.unwrap())
                .unwrap()
                .is_none()
        );
        assert_eq!(
            conn.query_row(
                "SELECT repository_id FROM repository_sync_run_members WHERE run_id = ?1",
                [&run.run_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            repo.id.unwrap()
        );
    }

    #[test]
    fn heartbeat_does_not_fill_or_rewrite_candidate_digest() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let run =
            begin_profile_sync_run_at(&conn, "fedora-44", None, OWNER_ONE, unix_seconds().unwrap())
                .unwrap();
        heartbeat_profile_sync_run(&conn, &run).unwrap();
        let candidate = conn
            .query_row(
                "SELECT candidate_profile_digest FROM repository_sync_runs WHERE run_id = ?1",
                [&run.run_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap();
        assert!(candidate.is_none());
    }

    #[test]
    fn abort_marks_only_the_exact_owned_run_abandoned() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let run =
            begin_profile_sync_run_at(&conn, "fedora-44", None, OWNER_ONE, unix_seconds().unwrap())
                .unwrap();
        abort_profile_sync_run(
            &conn,
            &run,
            RemiSyncFailureStage::Ingesting,
            RemiSyncFailureCategory::Internal,
            "fixture failure",
        )
        .unwrap();
        assert_eq!(run_state(&conn, &run), "abandoned");
        assert!(
            abort_profile_sync_run(
                &conn,
                &run,
                RemiSyncFailureStage::Ingesting,
                RemiSyncFailureCategory::Internal,
                "replay",
            )
            .is_ok()
        );
        let forged = ProfileSyncRun {
            owner_instance_uuid: OWNER_TWO.to_string(),
            ..run
        };
        assert!(
            abort_profile_sync_run(
                &conn,
                &forged,
                RemiSyncFailureStage::Ingesting,
                RemiSyncFailureCategory::Internal,
                "wrong owner",
            )
            .is_err()
        );
    }
}
