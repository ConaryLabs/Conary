// crates/conary-core/src/db/models/remi_catalog/gc.rs

//! Exact reachability and metadata collection for immutable Remi catalogs.
//!
//! This module is deliberately limited to the operational metadata authority.
//! Filesystem collection is an application concern: it can use a plan to
//! remove only the exact artifact identities returned here, then call the
//! metadata deletion function.  Both planning and deletion use typed roots;
//! no age, path, process, or owner-name heuristic participates in collection.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use super::validation::{
    validate_identity, validate_sha256, validate_storage_component, validate_uuid,
};
use super::{MEMBER_COLUMNS, RESOURCE_COLUMNS, RemiCatalogResource, RemiCatalogResourceKind};
use crate::error::Error;

/// The exact profile and source digests retained by current operational roots.
///
/// Profile roots are the active pointers, every profile-revision pin, and the
/// input/candidate profile digests of nonterminal refresh runs.  Source roots
/// additionally include those run members' input/candidate snapshots and all
/// source snapshots named by a reachable profile revision.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemiCatalogReachabilitySnapshot {
    /// Profile revision manifest digests retained by exact authority.
    pub profile_revision_sha256: BTreeSet<String>,
    /// Source snapshot manifest digests retained by exact authority.
    pub source_snapshot_sha256: BTreeSet<String>,
}

impl RemiCatalogReachabilitySnapshot {
    /// Whether an exact profile revision digest is retained.
    #[must_use]
    pub fn contains_profile_revision(&self, digest: &str) -> bool {
        self.profile_revision_sha256.contains(digest)
    }

    /// Whether an exact source snapshot digest is retained.
    #[must_use]
    pub fn contains_source_snapshot(&self, digest: &str) -> bool {
        self.source_snapshot_sha256.contains(digest)
    }
}

/// One profile or source bundle identity journaled by a refresh run.
///
/// Terminal candidates are included so the application collector can remove
/// exact, unregistered candidate bundles without scanning the filesystem.
/// Only nonterminal candidates contribute reachability roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemiCatalogRunCandidate {
    /// The exact refresh run that journaled this candidate.
    pub run_id: String,
    /// The source profile owning the refresh run.
    pub source_profile: String,
    /// The resource class represented by this candidate.
    pub resource_kind: RemiCatalogResourceKind,
    /// The candidate manifest digest.
    pub resource_sha256: String,
    /// The profile member ordinal for a source candidate; `None` is the
    /// profile candidate itself.
    pub member_ordinal: Option<i64>,
    /// Whether the owning run has no terminal timestamp yet.
    pub nonterminal: bool,
}

/// An exact durable intent for a metadata row already removed from SQLite.
///
/// The application removes the corresponding content-addressed filesystem
/// bundle and then acknowledges this intent. The digest, resource kind, and
/// source profile are immutable identity; `queued_at` is audit metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemiCatalogDeletionIntent {
    pub resource_sha256: String,
    pub resource_kind: RemiCatalogResourceKind,
    pub source_profile: String,
    pub queued_at: i64,
}

impl RemiCatalogDeletionIntent {
    fn validate(&self) -> crate::Result<()> {
        validate_sha256(&self.resource_sha256, "catalog deletion resource SHA-256")?;
        validate_storage_component(&self.source_profile, "catalog deletion source profile")?;
        if self.queued_at < 0 {
            return Err(Error::ConfigError(
                "catalog deletion queue time must not be negative".to_string(),
            ));
        }
        Ok(())
    }
}

/// A deterministic metadata collection plan.
///
/// The resource vectors contain registered rows that were unreachable at the
/// planning snapshot.  They are ordered by resource digest and retain their
/// complete immutable metadata so deletion can reject a stale plan if a row
/// was removed and re-registered with a different artifact identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemiCatalogCollectionPlan {
    /// The exact roots and profile-to-source closure used for this plan.
    pub reachability: RemiCatalogReachabilitySnapshot,
    /// Every run-journaled candidate identity, terminal or nonterminal.
    pub run_candidates: Vec<RemiCatalogRunCandidate>,
    /// Metadata deletion intents left for the application filesystem collector
    /// by an earlier collection attempt.
    pub pending_deletions: Vec<RemiCatalogDeletionIntent>,
    /// Registered profile resources unreachable at plan time.
    pub unreachable_profile_resources: Vec<RemiCatalogResource>,
    /// Registered source resources unreachable at plan time.
    pub unreachable_source_resources: Vec<RemiCatalogResource>,
}

/// Metadata rows actually removed after exact revalidation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemiCatalogCollectionResult {
    /// Profile resources deleted before source resources.
    pub deleted_profile_resources: Vec<RemiCatalogResource>,
    /// Source resources deleted after profile-member cascades and recheck.
    pub deleted_source_resources: Vec<RemiCatalogResource>,
}

/// Build a deterministic exact reachability plan for registered catalog rows.
pub fn plan_catalog_collection(conn: &Connection) -> crate::Result<RemiCatalogCollectionPlan> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Deferred)?;
    let reachability = load_reachability(&tx)?;
    let run_candidates = load_run_candidates(&tx)?;
    let pending_deletions = load_deletion_intents(&tx)?;
    let (unreachable_profile_resources, unreachable_source_resources) =
        load_unreachable_resources(&tx, &reachability)?;
    tx.commit()?;
    Ok(RemiCatalogCollectionPlan {
        reachability,
        run_candidates,
        pending_deletions,
        unreachable_profile_resources,
        unreachable_source_resources,
    })
}

/// List exact filesystem deletion intents left by a prior metadata collection.
pub fn list_catalog_deletion_intents(
    conn: &Connection,
) -> crate::Result<Vec<RemiCatalogDeletionIntent>> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Deferred)?;
    let intents = load_deletion_intents(&tx)?;
    tx.commit()?;
    Ok(intents)
}

/// Acknowledge one exact deletion intent after its filesystem bundle is gone.
///
/// A still-registered resource is a hard error: callers must never acknowledge
/// an intent before the metadata deletion phase has committed.
pub fn acknowledge_catalog_deletion(
    conn: &Connection,
    intent: &RemiCatalogDeletionIntent,
) -> crate::Result<bool> {
    intent.validate()?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let resource_exists = tx
        .query_row(
            "SELECT 1 FROM remi_catalog_resources WHERE resource_sha256 = ?1",
            [&intent.resource_sha256],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if resource_exists {
        return Err(Error::ConflictError(format!(
            "catalog deletion intent {} still has registered metadata",
            intent.resource_sha256
        )));
    }
    let deleted = tx.execute(
        "DELETE FROM remi_catalog_gc_deletions
         WHERE resource_sha256 = ?1
           AND resource_kind = ?2
           AND source_profile = ?3
           AND queued_at = ?4",
        params![
            &intent.resource_sha256,
            intent.resource_kind.as_str(),
            &intent.source_profile,
            intent.queued_at,
        ],
    )?;
    tx.commit()?;
    Ok(deleted == 1)
}

/// Delete the exact rows from a previously planned collection.
///
/// The function acquires SQLite's immediate writer lock before re-reading all
/// roots.  It deletes only rows whose complete immutable metadata still equals
/// the plan, and only while the current root closure proves them unreachable.
/// Profile rows are deleted first; their member rows then disappear through
/// the schema's cascade, after which source rows are independently rechecked.
/// A resource that became reachable or changed since planning is skipped and
/// remains authoritative.
pub fn delete_catalog_collection(
    conn: &Connection,
    plan: &RemiCatalogCollectionPlan,
) -> crate::Result<RemiCatalogCollectionResult> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let initial_reachability = load_reachability(&tx)?;
    let mut result = RemiCatalogCollectionResult::default();
    let queued_at = unix_seconds()?;

    for resource in &plan.unreachable_profile_resources {
        if initial_reachability.contains_profile_revision(&resource.resource_sha256)
            || !resource_matches_exact(&tx, resource)?
        {
            continue;
        }
        queue_catalog_deletion(&tx, resource, queued_at)?;
        let deleted = tx.execute(
            "DELETE FROM remi_catalog_resources
             WHERE resource_sha256 = ?1
               AND resource_kind = 'profile_revision'
               AND source_profile = ?2
               AND artifact_sha256 = ?3
               AND artifact_size = ?4
               AND logical_digest_sha256 = ?5
               AND manifest_json = ?6
               AND durable = ?7
               AND created_at = ?8
               AND NOT EXISTS (
                   SELECT 1 FROM remi_active_profile_revisions active
                   WHERE active.profile_revision_sha256 = ?1
               )
               AND NOT EXISTS (
                   SELECT 1 FROM remi_profile_revision_pins pin
                   WHERE pin.profile_revision_sha256 = ?1
               )
               AND NOT EXISTS (
                   SELECT 1 FROM repository_sync_runs run
                   WHERE run.finished_at IS NULL
                     AND (run.input_profile_digest = ?1
                          OR run.candidate_profile_digest = ?1)
               )",
            params![
                &resource.resource_sha256,
                &resource.source_profile,
                &resource.artifact_sha256,
                resource.artifact_size,
                &resource.logical_digest_sha256,
                &resource.manifest_json,
                resource.durable as i64,
                resource.created_at,
            ],
        )?;
        if deleted == 1 {
            result.deleted_profile_resources.push(resource.clone());
        } else {
            cancel_catalog_deletion(&tx, resource)?;
        }
    }

    // Deleting a profile removes its member edges. Rebuild the closure after
    // that phase so a shared source survives while any remaining profile still
    // names it, then disappears only after the final edge is gone.
    let source_reachability = load_reachability(&tx)?;
    for resource in &plan.unreachable_source_resources {
        if source_reachability.contains_source_snapshot(&resource.resource_sha256)
            || !resource_matches_exact(&tx, resource)?
        {
            continue;
        }
        queue_catalog_deletion(&tx, resource, queued_at)?;
        let deleted = tx.execute(
            "DELETE FROM remi_catalog_resources
             WHERE resource_sha256 = ?1
               AND resource_kind = 'source_snapshot'
               AND source_profile = ?2
               AND artifact_sha256 = ?3
               AND artifact_size = ?4
               AND logical_digest_sha256 = ?5
               AND manifest_json = ?6
               AND durable = ?7
               AND created_at = ?8
               AND NOT EXISTS (
                   SELECT 1 FROM remi_profile_revision_members member
                   WHERE member.source_snapshot_sha256 = ?1
               )
               AND NOT EXISTS (
                   SELECT 1
                   FROM repository_sync_runs run
                   JOIN repository_sync_run_members member
                     ON member.run_id = run.run_id
                   WHERE run.finished_at IS NULL
                     AND (member.input_source_snapshot_sha256 = ?1
                          OR member.candidate_source_snapshot_sha256 = ?1)
               )",
            params![
                &resource.resource_sha256,
                &resource.source_profile,
                &resource.artifact_sha256,
                resource.artifact_size,
                &resource.logical_digest_sha256,
                &resource.manifest_json,
                resource.durable as i64,
                resource.created_at,
            ],
        )?;
        if deleted == 1 {
            result.deleted_source_resources.push(resource.clone());
        } else {
            cancel_catalog_deletion(&tx, resource)?;
        }
    }

    tx.commit()?;
    Ok(result)
}

fn queue_catalog_deletion(
    conn: &Connection,
    resource: &RemiCatalogResource,
    queued_at: i64,
) -> crate::Result<()> {
    let existing = conn
        .query_row(
            "SELECT resource_sha256, resource_kind, source_profile, queued_at
             FROM remi_catalog_gc_deletions WHERE resource_sha256 = ?1",
            [&resource.resource_sha256],
            |row| {
                Ok(RemiCatalogDeletionIntent {
                    resource_sha256: row.get(0)?,
                    resource_kind: RemiCatalogResourceKind::from_db(&row.get::<_, String>(1)?, 1)?,
                    source_profile: row.get(2)?,
                    queued_at: row.get(3)?,
                })
            },
        )
        .optional()?;
    if let Some(existing) = existing {
        if existing.resource_kind != resource.kind
            || existing.source_profile != resource.source_profile
        {
            return Err(crate::Error::ConflictError(format!(
                "catalog deletion intent {} changed resource identity",
                resource.resource_sha256
            )));
        }
        return Ok(());
    }
    conn.execute(
        "INSERT INTO remi_catalog_gc_deletions (
             resource_sha256, resource_kind, source_profile, queued_at
         ) VALUES (?1, ?2, ?3, ?4)",
        params![
            &resource.resource_sha256,
            resource.kind.as_str(),
            &resource.source_profile,
            queued_at,
        ],
    )?;
    Ok(())
}

fn cancel_catalog_deletion(conn: &Connection, resource: &RemiCatalogResource) -> crate::Result<()> {
    conn.execute(
        "DELETE FROM remi_catalog_gc_deletions
         WHERE resource_sha256 = ?1 AND resource_kind = ?2 AND source_profile = ?3",
        params![
            &resource.resource_sha256,
            resource.kind.as_str(),
            &resource.source_profile,
        ],
    )?;
    Ok(())
}

fn unix_seconds() -> crate::Result<i64> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            crate::Error::InternalError(format!("system time precedes Unix epoch: {error}"))
        })?
        .as_secs();
    i64::try_from(seconds)
        .map_err(|_| crate::Error::InternalError("system time exceeds i64".to_string()))
}

fn resource_matches_exact(
    conn: &Connection,
    expected: &RemiCatalogResource,
) -> crate::Result<bool> {
    let stored = conn
        .query_row(
            &format!(
                "SELECT {RESOURCE_COLUMNS} FROM remi_catalog_resources
                 WHERE resource_sha256 = ?1"
            ),
            [&expected.resource_sha256],
            RemiCatalogResource::from_row,
        )
        .optional()?;
    Ok(stored.as_ref() == Some(expected))
}

fn load_deletion_intents(conn: &Connection) -> crate::Result<Vec<RemiCatalogDeletionIntent>> {
    let mut statement = conn.prepare(
        "SELECT resource_sha256, resource_kind, source_profile, queued_at
         FROM remi_catalog_gc_deletions
         ORDER BY resource_kind, source_profile, resource_sha256",
    )?;
    let intents = statement
        .query_map([], |row| {
            Ok(RemiCatalogDeletionIntent {
                resource_sha256: row.get(0)?,
                resource_kind: RemiCatalogResourceKind::from_db(&row.get::<_, String>(1)?, 1)?,
                source_profile: row.get(2)?,
                queued_at: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for intent in &intents {
        intent.validate()?;
    }
    Ok(intents)
}

fn load_unreachable_resources(
    conn: &Connection,
    reachability: &RemiCatalogReachabilitySnapshot,
) -> crate::Result<(Vec<RemiCatalogResource>, Vec<RemiCatalogResource>)> {
    let mut profile_resources = Vec::new();
    let mut source_resources = Vec::new();
    let mut statement = conn.prepare(&format!(
        "SELECT {RESOURCE_COLUMNS} FROM remi_catalog_resources
         ORDER BY resource_kind, resource_sha256"
    ))?;
    let resources = statement.query_map([], RemiCatalogResource::from_row)?;
    for row in resources {
        let resource = row?;
        resource.validate()?;
        match resource.kind {
            RemiCatalogResourceKind::ProfileRevision
                if !reachability.contains_profile_revision(&resource.resource_sha256) =>
            {
                profile_resources.push(resource);
            }
            RemiCatalogResourceKind::SourceSnapshot
                if !reachability.contains_source_snapshot(&resource.resource_sha256) =>
            {
                source_resources.push(resource);
            }
            _ => {}
        }
    }
    Ok((profile_resources, source_resources))
}

fn load_reachability(conn: &Connection) -> crate::Result<RemiCatalogReachabilitySnapshot> {
    let mut reachability = RemiCatalogReachabilitySnapshot::default();

    load_profile_roots(conn, &mut reachability)?;
    load_source_run_roots(conn, &mut reachability)?;
    load_profile_member_edges(conn, &mut reachability)?;
    Ok(reachability)
}

fn load_profile_roots(
    conn: &Connection,
    reachability: &mut RemiCatalogReachabilitySnapshot,
) -> crate::Result<()> {
    let mut active = conn.prepare(
        "SELECT source_profile, profile_revision_sha256
         FROM remi_active_profile_revisions
         ORDER BY source_profile",
    )?;
    let active_rows = active.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in active_rows {
        let (source_profile, digest) = row?;
        validate_identity(&source_profile, "active catalog source profile")?;
        insert_profile_digest(reachability, &digest, "active profile revision")?;
    }

    let mut pins = conn.prepare(
        "SELECT source_profile, profile_revision_sha256
         FROM remi_profile_revision_pins
         ORDER BY source_profile, profile_revision_sha256, pin_id",
    )?;
    let pin_rows = pins.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in pin_rows {
        let (source_profile, digest) = row?;
        validate_identity(&source_profile, "pinned catalog source profile")?;
        insert_profile_digest(reachability, &digest, "pinned profile revision")?;
    }

    let mut runs = conn.prepare(
        "SELECT input_profile_digest, candidate_profile_digest
         FROM repository_sync_runs
         WHERE finished_at IS NULL
         ORDER BY run_id",
    )?;
    let run_rows = runs.query_map([], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
        ))
    })?;
    for row in run_rows {
        let (input, candidate) = row?;
        insert_optional_profile_digest(
            reachability,
            input.as_deref(),
            "live run input profile revision",
        )?;
        insert_optional_profile_digest(
            reachability,
            candidate.as_deref(),
            "live run candidate profile revision",
        )?;
    }
    Ok(())
}

fn load_source_run_roots(
    conn: &Connection,
    reachability: &mut RemiCatalogReachabilitySnapshot,
) -> crate::Result<()> {
    let mut members = conn.prepare(
        "SELECT member.input_source_snapshot_sha256,
                member.candidate_source_snapshot_sha256
         FROM repository_sync_run_members member
         JOIN repository_sync_runs run ON run.run_id = member.run_id
         WHERE run.finished_at IS NULL
         ORDER BY member.run_id, member.ordinal",
    )?;
    let member_rows = members.query_map([], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
        ))
    })?;
    for row in member_rows {
        let (input, candidate) = row?;
        insert_optional_source_digest(
            reachability,
            input.as_deref(),
            "live run input source snapshot",
        )?;
        insert_optional_source_digest(
            reachability,
            candidate.as_deref(),
            "live run candidate source snapshot",
        )?;
    }
    Ok(())
}

fn load_profile_member_edges(
    conn: &Connection,
    reachability: &mut RemiCatalogReachabilitySnapshot,
) -> crate::Result<()> {
    // The profile set is copied so the source closure can grow independently
    // without ever treating a source digest as a profile root.
    let profiles = reachability
        .profile_revision_sha256
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    for profile_digest in profiles {
        let mut members = conn.prepare(&format!(
            "SELECT {MEMBER_COLUMNS} FROM remi_profile_revision_members
             WHERE profile_revision_sha256 = ?1 ORDER BY ordinal"
        ))?;
        let member_rows = members.query_map([&profile_digest], |row| row.get::<_, String>(2))?;
        for row in member_rows {
            let source_digest = row?;
            insert_source_digest(
                reachability,
                &source_digest,
                "profile revision source member",
            )?;
        }
    }
    Ok(())
}

fn load_run_candidates(conn: &Connection) -> crate::Result<Vec<RemiCatalogRunCandidate>> {
    let mut candidates = Vec::new();
    let mut profiles = conn.prepare(
        "SELECT run_id, source_profile, candidate_profile_digest, finished_at
         FROM repository_sync_runs
         WHERE candidate_profile_digest IS NOT NULL
         ORDER BY run_id",
    )?;
    let profile_rows = profiles.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<i64>>(3)?,
        ))
    })?;
    for row in profile_rows {
        let (run_id, source_profile, digest, finished_at) = row?;
        validate_run_candidate_identity(&run_id, &source_profile, &digest)?;
        candidates.push(RemiCatalogRunCandidate {
            run_id,
            source_profile,
            resource_kind: RemiCatalogResourceKind::ProfileRevision,
            resource_sha256: digest,
            member_ordinal: None,
            nonterminal: finished_at.is_none(),
        });
    }

    let mut sources = conn.prepare(
        "SELECT run.run_id, run.source_profile, member.ordinal,
                member.candidate_source_snapshot_sha256, run.finished_at
         FROM repository_sync_run_members member
         JOIN repository_sync_runs run ON run.run_id = member.run_id
         WHERE member.candidate_source_snapshot_sha256 IS NOT NULL
         ORDER BY run.run_id, member.ordinal",
    )?;
    let source_rows = sources.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<i64>>(4)?,
        ))
    })?;
    for row in source_rows {
        let (run_id, source_profile, ordinal, digest, finished_at) = row?;
        validate_run_candidate_identity(&run_id, &source_profile, &digest)?;
        candidates.push(RemiCatalogRunCandidate {
            run_id,
            source_profile,
            resource_kind: RemiCatalogResourceKind::SourceSnapshot,
            resource_sha256: digest,
            member_ordinal: Some(ordinal),
            nonterminal: finished_at.is_none(),
        });
    }
    Ok(candidates)
}

fn validate_run_candidate_identity(
    run_id: &str,
    source_profile: &str,
    digest: &str,
) -> crate::Result<()> {
    validate_uuid(run_id, "catalog refresh run ID")?;
    validate_storage_component(source_profile, "catalog refresh source profile")?;
    validate_sha256(digest, "catalog refresh candidate resource SHA-256")
}

fn insert_profile_digest(
    reachability: &mut RemiCatalogReachabilitySnapshot,
    digest: &str,
    context: &str,
) -> crate::Result<()> {
    validate_sha256(digest, context)?;
    reachability
        .profile_revision_sha256
        .insert(digest.to_string());
    Ok(())
}

fn insert_optional_profile_digest(
    reachability: &mut RemiCatalogReachabilitySnapshot,
    digest: Option<&str>,
    context: &str,
) -> crate::Result<()> {
    if let Some(digest) = digest {
        insert_profile_digest(reachability, digest, context)?;
    }
    Ok(())
}

fn insert_source_digest(
    reachability: &mut RemiCatalogReachabilitySnapshot,
    digest: &str,
    context: &str,
) -> crate::Result<()> {
    validate_sha256(digest, context)?;
    reachability
        .source_snapshot_sha256
        .insert(digest.to_string());
    Ok(())
}

fn insert_optional_source_digest(
    reachability: &mut RemiCatalogReachabilitySnapshot,
    digest: Option<&str>,
    context: &str,
) -> crate::Result<()> {
    if let Some(digest) = digest {
        insert_source_digest(reachability, digest, context)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "gc/tests.rs"]
mod tests;
