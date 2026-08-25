// crates/conary-core/src/repository/sync/remi/run/recovery.rs

//! Exclusive-restart fencing and exact private-candidate cleanup state.

use super::*;

/// One terminal profile run whose exact private candidate directory still
/// requires idempotent cleanup after exclusive runtime ownership is acquired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSyncRunRecovery {
    pub run_id: String,
    pub source_profile: String,
}

/// Fence every expired profile refresh after an exclusive runtime ownership
/// transition and return the exact terminal runs whose private candidates have
/// not yet been durably acknowledged as removed.
///
/// The caller must hold the process-wide runtime-root lock. Lease expiry is the
/// only run-state recovery authority; filesystem contents are never inspected
/// to decide whether a run is stale.
pub fn recover_expired_profile_sync_runs(conn: &Connection) -> Result<Vec<ProfileSyncRunRecovery>> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let now = unix_seconds()?;
    recover_expired_profile_sync_runs_in_transaction(tx, now)
}

#[cfg(test)]
pub(super) fn recover_expired_profile_sync_runs_at(
    conn: &Connection,
    now: i64,
) -> Result<Vec<ProfileSyncRunRecovery>> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    recover_expired_profile_sync_runs_in_transaction(tx, now)
}

fn recover_expired_profile_sync_runs_in_transaction(
    tx: Transaction<'_>,
    now: i64,
) -> Result<Vec<ProfileSyncRunRecovery>> {
    let expired = {
        let mut statement = tx.prepare(
            "SELECT run_id, source_profile, fencing_epoch
             FROM repository_sync_runs
             WHERE state NOT IN ('candidate', 'published', 'failed', 'abandoned')
               AND lease_expires_at <= ?1
             ORDER BY source_profile, fencing_epoch",
        )?;
        statement
            .query_map([now], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (run_id, source_profile, fencing_epoch) in expired {
        abandon_expired_run(
            &tx,
            &source_profile,
            &run_id,
            now,
            fencing_epoch,
            "exclusive runtime restart recovery",
        )?;
    }
    let pending = pending_candidate_recovery(&tx, None)?;
    tx.commit()?;
    Ok(pending)
}

/// Durably acknowledge that one exact terminal run's private candidate path
/// is absent. Returns false for a replay or a run that is not terminal.
pub fn acknowledge_profile_sync_candidate_cleanup(conn: &Connection, run_id: &str) -> Result<bool> {
    validate_uuid(run_id, "profile sync candidate cleanup run ID")?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let now = unix_seconds()?;
    let updated = tx.execute(
        "UPDATE repository_sync_runs
         SET candidate_cleaned_at = ?1
         WHERE run_id = ?2
           AND state IN ('candidate', 'published', 'failed', 'abandoned')
           AND finished_at IS NOT NULL
           AND candidate_cleaned_at IS NULL",
        params![now, run_id],
    )?;
    tx.commit()?;
    Ok(updated == 1)
}

pub(super) fn abandon_expired_run(
    tx: &Transaction<'_>,
    source_profile: &str,
    run_id: &str,
    now: i64,
    fencing_epoch: i64,
    recovery_context: &str,
) -> Result<()> {
    let evidence = format!(
        "durable lease for profile {source_profile} fencing epoch {fencing_epoch} expired before {recovery_context}"
    );
    let updated = tx.execute(
        "UPDATE repository_sync_runs
         SET state = 'abandoned', heartbeat_at = ?1, lease_expires_at = ?1,
             finished_at = ?1, failure_stage = 'publishing',
             failure_category = 'fenced', failure_evidence = ?2
         WHERE run_id = ?3
           AND source_profile = ?4
           AND state NOT IN ('candidate', 'published', 'failed', 'abandoned')
           AND lease_expires_at <= ?1",
        params![now, evidence, run_id, source_profile],
    )?;
    if updated != 1 {
        return Err(Error::ConflictError(format!(
            "profile {source_profile} sync run {run_id} is not durably abandoned"
        )));
    }
    Ok(())
}

pub(super) fn pending_candidate_recovery(
    tx: &Transaction<'_>,
    source_profile: Option<&str>,
) -> Result<Vec<ProfileSyncRunRecovery>> {
    let mut statement = tx.prepare(
        "SELECT run_id, source_profile
         FROM repository_sync_runs
         WHERE state IN ('candidate', 'published', 'failed', 'abandoned')
           AND candidate_cleaned_at IS NULL
           AND (?1 IS NULL OR source_profile = ?1)
         ORDER BY finished_at, source_profile, fencing_epoch",
    )?;
    Ok(statement
        .query_map([source_profile], |row| {
            Ok(ProfileSyncRunRecovery {
                run_id: row.get(0)?,
                source_profile: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}
