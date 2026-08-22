// crates/conary-core/src/db/models/remi_catalog/activation.rs

//! Exact fenced-run proof and atomic activation for one Remi profile.

use super::{
    ACTIVE_COLUMNS, MEMBER_COLUMNS, RESOURCE_COLUMNS, RemiActiveProfileRevision,
    RemiCatalogResource, RemiCatalogResourceKind, RemiProfileRevisionMember,
};
use crate::db::models::remi_catalog::validation::{
    validate_identity, validate_sha256, validate_uuid,
};
use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::time::{SystemTime, UNIX_EPOCH};

/// The exact proof required before moving a profile pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemiProfileRevisionActivation {
    pub source_profile: String,
    pub profile_revision_sha256: String,
    pub artifact_sha256: String,
    pub artifact_size: i64,
    pub logical_digest_sha256: String,
    pub run_id: String,
    pub owner_instance_uuid: String,
    pub fencing_epoch: i64,
}

impl RemiProfileRevisionActivation {
    fn validate(&self) -> Result<()> {
        validate_identity(&self.source_profile, "activation source profile")?;
        validate_sha256(
            &self.profile_revision_sha256,
            "activation profile revision SHA-256",
        )?;
        validate_sha256(&self.artifact_sha256, "activation catalog artifact SHA-256")?;
        validate_sha256(
            &self.logical_digest_sha256,
            "activation catalog logical digest",
        )?;
        if self.artifact_size < 0 {
            return Err(Error::ConfigError(
                "activation catalog artifact size must not be negative".to_string(),
            ));
        }
        validate_uuid(&self.run_id, "activation run ID")?;
        validate_uuid(&self.owner_instance_uuid, "activation owner instance UUID")?;
        if self.fencing_epoch <= 0 {
            return Err(Error::ConfigError(
                "activation fencing epoch must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

/// Whether the transaction moved the pointer or proved an exact replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemiProfileActivationOutcome {
    Activated(RemiActiveProfileRevision),
    AlreadyActive(RemiActiveProfileRevision),
}

/// Prove the exact current run owner and durable catalog metadata, then move
/// one profile pointer under a single immediate SQLite transaction.
pub fn activate_profile_revision(
    conn: &Connection,
    request: &RemiProfileRevisionActivation,
) -> Result<RemiProfileActivationOutcome> {
    request.validate()?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    // Read the lease clock only after acquiring the writer lock. A timestamp
    // captured while waiting could already be expired when the transaction
    // finally obtains activation authority.
    let now = current_unix_seconds()?;
    let outcome = activate_profile_revision_in_transaction(&tx, request, now)?;
    tx.commit()?;
    Ok(outcome)
}

#[cfg(test)]
pub(super) fn activate_profile_revision_at(
    conn: &Connection,
    request: &RemiProfileRevisionActivation,
    now: i64,
) -> Result<RemiProfileActivationOutcome> {
    request.validate()?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let outcome = activate_profile_revision_in_transaction(&tx, request, now)?;
    tx.commit()?;
    Ok(outcome)
}

fn activate_profile_revision_in_transaction(
    tx: &Transaction<'_>,
    request: &RemiProfileRevisionActivation,
    now: i64,
) -> Result<RemiProfileActivationOutcome> {
    let profile_resource = tx
        .query_row(
            &format!(
                "SELECT {RESOURCE_COLUMNS} FROM remi_catalog_resources
                 WHERE resource_sha256 = ?1
                   AND resource_kind = 'profile_revision'
                   AND source_profile = ?2"
            ),
            params![&request.profile_revision_sha256, &request.source_profile],
            RemiCatalogResource::from_row,
        )
        .optional()?;
    let Some(profile_resource) = profile_resource else {
        return Err(Error::ConflictError(format!(
            "profile {} revision {} lacks exact durable catalog metadata",
            request.source_profile, request.profile_revision_sha256
        )));
    };
    profile_resource.validate()?;
    let resource_matches = profile_resource.durable
        && profile_resource.artifact_sha256 == request.artifact_sha256
        && profile_resource.artifact_size == request.artifact_size
        && profile_resource.logical_digest_sha256 == request.logical_digest_sha256;
    if !resource_matches {
        return Err(Error::ConflictError(format!(
            "profile {} revision {} lacks exact durable catalog metadata",
            request.source_profile, request.profile_revision_sha256
        )));
    }

    let (member_count, min_ordinal, max_ordinal) = tx.query_row(
        "SELECT COUNT(*), COALESCE(MIN(ordinal), -1), COALESCE(MAX(ordinal), -1)
         FROM remi_profile_revision_members
         WHERE profile_revision_sha256 = ?1",
        params![&request.profile_revision_sha256,],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        },
    )?;
    if member_count == 0 || min_ordinal != 0 || max_ordinal != member_count - 1 {
        return Err(Error::ConflictError(format!(
            "profile {} revision {} has noncanonical ordered members",
            request.source_profile, request.profile_revision_sha256
        )));
    }

    let mut source_statement = tx.prepare(&format!(
        "SELECT source.{RESOURCE_COLUMNS}
         FROM remi_profile_revision_members member
         JOIN remi_catalog_resources source
           ON source.resource_sha256 = member.source_snapshot_sha256
         WHERE member.profile_revision_sha256 = ?1
         ORDER BY member.ordinal"
    ))?;
    let mut source_rows = source_statement.query([&request.profile_revision_sha256])?;
    let mut source_count = 0_i64;
    while let Some(row) = source_rows.next()? {
        let source = RemiCatalogResource::from_row(row)?;
        source.validate()?;
        if source.kind != RemiCatalogResourceKind::SourceSnapshot
            || source.source_profile != request.source_profile
            || !source.durable
        {
            return Err(Error::ConflictError(format!(
                "profile {} revision {} has a missing or non-durable source snapshot",
                request.source_profile, request.profile_revision_sha256
            )));
        }
        source_count += 1;
    }
    if source_count != member_count {
        return Err(Error::ConflictError(format!(
            "profile {} revision {} has a missing source snapshot member",
            request.source_profile, request.profile_revision_sha256
        )));
    }

    let run_state = tx
        .query_row(
            "SELECT run.state, run.candidate_profile_digest
             FROM repository_sync_runs run
             JOIN repository_sync_scopes scope
               ON scope.source_profile = run.source_profile
              AND scope.current_run_id = run.run_id
              AND scope.fencing_epoch = run.fencing_epoch
             WHERE run.run_id = ?1
               AND run.source_profile = ?2
               AND run.owner_instance_uuid = ?3
               AND run.fencing_epoch = ?4
               AND (run.state = 'published'
                    OR (run.state = 'ready_to_publish' AND run.lease_expires_at > ?5))",
            params![
                &request.run_id,
                &request.source_profile,
                &request.owner_instance_uuid,
                request.fencing_epoch,
                now,
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?;
    let Some((run_state, candidate_profile_digest)) = run_state else {
        return Err(Error::ConflictError(format!(
            "profile {} activation lost run {} fencing epoch {}",
            request.source_profile, request.run_id, request.fencing_epoch
        )));
    };
    if candidate_profile_digest.as_deref() != Some(request.profile_revision_sha256.as_str()) {
        return Err(Error::ConflictError(format!(
            "profile {} activation candidate digest does not match run {}",
            request.source_profile, request.run_id
        )));
    }
    verify_profile_run_members(tx, request)?;

    let current = RemiActiveProfileRevision::find_in_transaction(tx, &request.source_profile)?;
    if let Some(current) = current {
        if current.fencing_epoch > request.fencing_epoch {
            return Err(Error::ConflictError(format!(
                "profile {} activation fencing epoch {} is stale behind {}",
                request.source_profile, request.fencing_epoch, current.fencing_epoch
            )));
        }
        if current.fencing_epoch == request.fencing_epoch {
            if current.profile_revision_sha256 == request.profile_revision_sha256
                && current.activation_run_id == request.run_id
                && current.owner_instance_uuid == request.owner_instance_uuid
            {
                if run_state == "published" {
                    return Ok(RemiProfileActivationOutcome::AlreadyActive(current));
                }
                return Err(Error::ConflictError(format!(
                    "profile {} activation pointer exists before run {} was published",
                    request.source_profile, request.run_id
                )));
            }
            return Err(Error::ConflictError(format!(
                "profile {} activation replay conflicts at fencing epoch {}",
                request.source_profile, request.fencing_epoch
            )));
        }
    }

    if run_state != "ready_to_publish" {
        return Err(Error::ConflictError(format!(
            "profile {} run {} is already published without its active pointer",
            request.source_profile, request.run_id
        )));
    }

    let published_at = now;
    let published = tx.execute(
        "UPDATE repository_sync_runs
         SET state = 'published', heartbeat_at = ?1, lease_expires_at = ?1,
             finished_at = ?1
         WHERE run_id = ?2
           AND source_profile = ?3
           AND owner_instance_uuid = ?4
           AND fencing_epoch = ?5
           AND state = 'ready_to_publish'
           AND candidate_profile_digest = ?6
           AND lease_expires_at > ?1",
        params![
            published_at,
            &request.run_id,
            &request.source_profile,
            &request.owner_instance_uuid,
            request.fencing_epoch,
            &request.profile_revision_sha256,
        ],
    )?;
    if published != 1 {
        return Err(Error::ConflictError(format!(
            "profile {} activation lost run {} before publication",
            request.source_profile, request.run_id
        )));
    }

    let updated = tx.execute(
        "INSERT INTO remi_active_profile_revisions (
             source_profile, profile_revision_sha256, fencing_epoch,
             activation_run_id, owner_instance_uuid, activated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(source_profile) DO UPDATE SET
             profile_revision_sha256 = excluded.profile_revision_sha256,
             fencing_epoch = excluded.fencing_epoch,
             activation_run_id = excluded.activation_run_id,
             owner_instance_uuid = excluded.owner_instance_uuid,
             activated_at = excluded.activated_at
         WHERE excluded.fencing_epoch > remi_active_profile_revisions.fencing_epoch",
        params![
            &request.source_profile,
            &request.profile_revision_sha256,
            request.fencing_epoch,
            &request.run_id,
            &request.owner_instance_uuid,
            now,
        ],
    )?;
    if updated != 1 {
        return Err(Error::ConflictError(format!(
            "profile {} activation was rejected by monotonic fencing",
            request.source_profile
        )));
    }
    let active = RemiActiveProfileRevision::find_in_transaction(tx, &request.source_profile)?
        .ok_or_else(|| Error::InternalError("activated profile pointer disappeared".to_string()))?;
    Ok(RemiProfileActivationOutcome::Activated(active))
}

fn verify_profile_run_members(
    tx: &Transaction<'_>,
    request: &RemiProfileRevisionActivation,
) -> Result<()> {
    let run_input_digest = tx.query_row(
        "SELECT input_profile_digest
         FROM repository_sync_runs
         WHERE run_id = ?1 AND source_profile = ?2",
        params![&request.run_id, &request.source_profile],
        |row| row.get::<_, Option<String>>(0),
    )?;
    let (profile_count, run_count): (i64, i64) = tx.query_row(
        "SELECT
            (SELECT COUNT(*) FROM remi_profile_revision_members
             WHERE profile_revision_sha256 = ?1),
            (SELECT COUNT(*) FROM repository_sync_run_members WHERE run_id = ?2)",
        params![&request.profile_revision_sha256, &request.run_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if profile_count == 0 || profile_count != run_count {
        return Err(Error::ConflictError(format!(
            "profile {} revision {} does not have the exact run member set",
            request.source_profile, request.profile_revision_sha256
        )));
    }

    let mut statement = tx.prepare(
        "SELECT ordinal, repository_id, source_identity, repository_identity,
                stream_kind, stream_identity, priority, required,
                input_source_snapshot_sha256, candidate_source_snapshot_sha256
         FROM repository_sync_run_members
         WHERE run_id = ?1 ORDER BY ordinal",
    )?;
    let mut rows = statement.query([&request.run_id])?;
    let mut seen = 0_i64;
    while let Some(row) = rows.next()? {
        let ordinal = row.get::<_, i64>(0)?;
        let repository_id = row.get::<_, i64>(1)?;
        let source_identity = row.get::<_, String>(2)?;
        let repository_identity = row.get::<_, String>(3)?;
        let stream_kind = row.get::<_, String>(4)?;
        let stream_identity = row.get::<_, String>(5)?;
        let priority = row.get::<_, i64>(6)?;
        let required = row.get::<_, i64>(7)? != 0;
        let input_source_snapshot = row.get::<_, Option<String>>(8)?;
        let candidate_source_snapshot = row.get::<_, Option<String>>(9)?;

        let profile_member = tx
            .query_row(
                &format!("SELECT {MEMBER_COLUMNS} FROM remi_profile_revision_members WHERE profile_revision_sha256 = ?1 AND ordinal = ?2"),
                params![&request.profile_revision_sha256, ordinal],
                RemiProfileRevisionMember::from_row,
            )
            .optional()?;
        let Some(profile_member) = profile_member else {
            return Err(Error::ConflictError(format!(
                "profile {} run {} is missing member ordinal {}",
                request.source_profile, request.run_id, ordinal
            )));
        };
        if profile_member.source_identity != source_identity
            || profile_member.repository_identity != repository_identity
            || profile_member.stream_kind != stream_kind
            || profile_member.stream_identity != stream_identity
            || profile_member.priority != priority
            || profile_member.required != required
            || candidate_source_snapshot.as_deref()
                != Some(profile_member.source_snapshot_sha256.as_str())
        {
            return Err(Error::ConflictError(format!(
                "profile {} run {} member ordinal {} disagrees with candidate revision",
                request.source_profile, request.run_id, ordinal
            )));
        }

        if let Some(input_digest) = run_input_digest.as_deref() {
            let input_member = tx
                .query_row(
                    &format!("SELECT {MEMBER_COLUMNS} FROM remi_profile_revision_members WHERE profile_revision_sha256 = ?1 AND repository_identity = ?2"),
                    params![input_digest, &repository_identity],
                    RemiProfileRevisionMember::from_row,
                )
                .optional()?;
            match input_member {
                Some(input_member)
                    if input_source_snapshot.as_deref()
                        == Some(input_member.source_snapshot_sha256.as_str()) => {}
                None if input_source_snapshot.is_none() => {}
                _ => {
                    return Err(Error::ConflictError(format!(
                        "profile {} run {} member ordinal {} disagrees with input revision",
                        request.source_profile, request.run_id, ordinal
                    )));
                }
            }
        } else if input_source_snapshot.is_some() {
            return Err(Error::ConflictError(format!(
                "profile {} run {} carries an input source snapshot without an input profile",
                request.source_profile, request.run_id
            )));
        }

        let repository = tx
            .query_row(
                "SELECT source_profile, repository_identity, priority, enabled
                 FROM repositories WHERE id = ?1",
                [repository_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)? != 0,
                    ))
                },
            )
            .optional()?;
        let Some((repository_profile, actual_repository_identity, actual_priority, enabled)) =
            repository
        else {
            return Err(Error::ConflictError(format!(
                "profile {} run {} member ordinal {} references a missing repository",
                request.source_profile, request.run_id, ordinal
            )));
        };
        if repository_profile.as_deref() != Some(request.source_profile.as_str())
            || actual_repository_identity.as_deref() != Some(repository_identity.as_str())
            || actual_priority != priority
            || !enabled
        {
            return Err(Error::ConflictError(format!(
                "profile {} run {} member ordinal {} repository binding changed",
                request.source_profile, request.run_id, ordinal
            )));
        }
        seen += 1;
    }
    if seen != profile_count {
        return Err(Error::ConflictError(format!(
            "profile {} run {} has noncanonical ordered members",
            request.source_profile, request.run_id
        )));
    }
    Ok(())
}

impl RemiActiveProfileRevision {
    fn find_in_transaction(tx: &Transaction<'_>, source_profile: &str) -> Result<Option<Self>> {
        let sql = format!(
            "SELECT {ACTIVE_COLUMNS} FROM remi_active_profile_revisions
             WHERE source_profile = ?1"
        );
        tx.query_row(&sql, [source_profile], Self::from_row)
            .optional()
            .map_err(Into::into)
    }
}

fn current_unix_seconds() -> Result<i64> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| Error::InternalError(format!("system time precedes Unix epoch: {error}")))?
        .as_secs();
    i64::try_from(seconds)
        .map_err(|_| Error::InternalError("system time exceeds SQLite integer range".to_string()))
}
