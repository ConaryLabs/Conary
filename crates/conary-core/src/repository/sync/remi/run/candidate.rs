// crates/conary-core/src/repository/sync/remi/run/candidate.rs

//! Durable successful private-candidate completion and selection.

use super::*;

/// The exact completed private candidate currently selected for one source
/// profile. This is promotion input, never serving authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSyncCandidate {
    pub source_profile: String,
    pub profile_revision_sha256: String,
    pub run_id: String,
    pub owner_instance_uuid: String,
    pub fencing_epoch: i64,
    pub completed_at: i64,
}

/// Complete the exact current ready run as a durable private candidate.
///
/// The caller must first durably publish, independently reopen, and register
/// the immutable profile revision named by the run. This transition ends the
/// refresh lease without advancing any active pointer.
pub fn complete_profile_sync_candidate(
    conn: &Connection,
    run: &RemiSyncRun,
) -> Result<ProfileSyncCandidate> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let now = unix_seconds()?;
    require_owned_run(&tx, run, "ready_to_publish", "", "", now)?;
    verify_registered_candidate(&tx, run)?;
    let updated = tx.execute(
        "UPDATE repository_sync_runs
         SET state = 'candidate', heartbeat_at = ?1, lease_expires_at = ?1,
             finished_at = ?1
         WHERE run_id = ?2
           AND source_profile = ?3
           AND owner_instance_uuid = ?4
           AND fencing_epoch = ?5
           AND state = 'ready_to_publish'
           AND candidate_profile_digest IS NOT NULL
           AND lease_expires_at > ?1",
        params![
            now,
            &run.run_id,
            &run.source_profile,
            &run.owner_instance_uuid,
            run.fencing_epoch,
        ],
    )?;
    if updated != 1 {
        return Err(fenced_error(run, "candidate completion was rejected"));
    }
    let candidate = current_profile_sync_candidate_in_transaction(&tx, &run.source_profile)?
        .ok_or_else(|| {
            Error::InternalError(format!(
                "profile {} completed candidate disappeared",
                run.source_profile
            ))
        })?;
    tx.commit()?;
    Ok(candidate)
}

fn verify_registered_candidate(tx: &Transaction<'_>, run: &RemiSyncRun) -> Result<()> {
    let candidate_digest = tx.query_row(
        "SELECT candidate_profile_digest
         FROM repository_sync_runs
         WHERE run_id = ?1 AND source_profile = ?2
           AND owner_instance_uuid = ?3 AND fencing_epoch = ?4
           AND state = 'ready_to_publish'",
        params![
            &run.run_id,
            &run.source_profile,
            &run.owner_instance_uuid,
            run.fencing_epoch,
        ],
        |row| row.get::<_, String>(0),
    )?;
    let durable_profile = tx.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM remi_catalog_resources
             WHERE resource_sha256 = ?1
               AND resource_kind = 'profile_revision'
               AND source_profile = ?2
               AND durable = 1
         )",
        params![&candidate_digest, &run.source_profile],
        |row| row.get::<_, bool>(0),
    )?;
    if !durable_profile {
        return Err(Error::ConflictError(format!(
            "profile {} run {} candidate {} lacks durable registered metadata",
            run.source_profile, run.run_id, candidate_digest
        )));
    }
    let (run_count, profile_count, exact_count): (i64, i64, i64) = tx.query_row(
        "SELECT
             (SELECT COUNT(*) FROM repository_sync_run_members
              WHERE run_id = ?1),
             (SELECT COUNT(*) FROM remi_profile_revision_members
              WHERE profile_revision_sha256 = ?2),
             (SELECT COUNT(*)
              FROM repository_sync_run_members run_member
              JOIN remi_profile_revision_members profile_member
                ON profile_member.profile_revision_sha256 = ?2
               AND profile_member.ordinal = run_member.ordinal
               AND profile_member.source_snapshot_sha256 =
                   run_member.candidate_source_snapshot_sha256
               AND profile_member.source_identity = run_member.source_identity
               AND profile_member.repository_identity = run_member.repository_identity
               AND profile_member.stream_kind = run_member.stream_kind
               AND profile_member.stream_identity = run_member.stream_identity
               AND profile_member.role = run_member.role
               AND profile_member.precedence = run_member.precedence
               AND profile_member.required = run_member.required
              WHERE run_member.run_id = ?1)",
        params![&run.run_id, &candidate_digest],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if run_count == 0 || run_count != profile_count || run_count != exact_count {
        return Err(Error::ConflictError(format!(
            "profile {} run {} candidate {} does not have its exact ordered member set",
            run.source_profile, run.run_id, candidate_digest
        )));
    }
    Ok(())
}

/// Resolve the exact current durable private candidate for one profile.
pub fn current_profile_sync_candidate(
    conn: &Connection,
    source_profile: &str,
) -> Result<Option<ProfileSyncCandidate>> {
    validate_profile(source_profile)?;
    current_profile_sync_candidate_in_transaction(conn, source_profile)
}

fn current_profile_sync_candidate_in_transaction(
    conn: &Connection,
    source_profile: &str,
) -> Result<Option<ProfileSyncCandidate>> {
    conn.query_row(
        "SELECT run.source_profile, run.candidate_profile_digest, run.run_id,
                run.owner_instance_uuid, run.fencing_epoch, run.finished_at
         FROM repository_sync_scopes scope
         JOIN repository_sync_runs run
           ON run.run_id = scope.current_run_id
          AND run.source_profile = scope.source_profile
          AND run.fencing_epoch = scope.fencing_epoch
         WHERE scope.source_profile = ?1
           AND run.state = 'candidate'",
        [source_profile],
        |row| {
            Ok(ProfileSyncCandidate {
                source_profile: row.get(0)?,
                profile_revision_sha256: row.get(1)?,
                run_id: row.get(2)?,
                owner_instance_uuid: row.get(3)?,
                fencing_epoch: row.get(4)?,
                completed_at: row.get(5)?,
            })
        },
    )
    .optional()
    .map_err(Error::from)
}
