// apps/remi/src/server/promotion.rs

//! Sole evidence-consuming authority for atomic public Remi promotion.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use conary_core::canonical::CanonicalMapSnapshot;
use conary_core::db::models::{
    RemiActiveProfileRevision, RemiCatalogResource, RemiCatalogResourceKind,
    RemiProfileRevisionActivation, publish_profile_candidate_in_transaction,
};
use conary_core::repository::catalog::{
    ProfileRevisionV2, SourceSnapshotV1, verify_source_catalog_bundle,
};
use conary_core::repository::universe::RemiUniverseManifestV2;
use conary_core::repository::{ProfileSyncCandidate, current_profile_sync_candidate};
use futures::{StreamExt, stream};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use super::catalog_authority::{CatalogAuthority, PinnedProfileCatalog, ProfileRevisionSelection};
use super::conversion_crawl::{
    ConversionCrawlOutcomeStateV4, RemiConversionCrawlV4, reopen_conversion_crawl,
    reopen_promotion_binding,
};
use super::database_writer::DatabaseWriter;
use super::handlers::canonical::load_canonical_map_snapshot;
use super::promotion_evidence::{RemiPromotionEvidenceV1, reopen_remi_promotion_evidence};
use super::r2::R2Store;
use super::signing_authority::load_universe_root_metadata;
use super::universe_publish::{
    SignedUniverseCandidate, build_candidate, canonical_bytes, publish_candidate_files,
    verify_published_bundle,
};

const OBJECT_REOPEN_CONCURRENCY: usize = 16;
const OBJECT_REOPEN_BATCH: usize = 256;

#[derive(Debug, Clone)]
pub struct RemiPromotionActivationConfig {
    pub db_path: PathBuf,
    pub catalog_dir: PathBuf,
    pub catalog_candidate_dir: PathBuf,
    pub chunk_dir: PathBuf,
    pub repository_keys_dir: PathBuf,
    pub promotion_evidence_path: PathBuf,
    pub conversion_crawl_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemiPromotionActivationOutcome {
    AlreadyActive {
        manifest_sha256: String,
        sequence: u64,
    },
    Activated {
        manifest_sha256: String,
        sequence: u64,
        promoted_profiles: usize,
        reopened_objects: u64,
    },
}

#[derive(Debug)]
struct ActiveUniverseBinding {
    manifest_sha256: String,
    sequence: u64,
    promotion_evidence_sha256: String,
    conversion_crawl_sha256: String,
    manifest: RemiUniverseManifestV2,
}

struct PromotionProfile {
    manifest: ProfileRevisionV2,
    activation: Option<RemiProfileRevisionActivation>,
    _pin: PinnedProfileCatalog,
}

#[derive(Clone)]
enum DurableObjectAuthority {
    Local(PathBuf),
    R2(Arc<R2Store>),
}

pub(crate) async fn activate_remi_promotion(
    config: &RemiPromotionActivationConfig,
    database_writer: &DatabaseWriter,
    catalog_authority: &CatalogAuthority,
    r2_store: Option<Arc<R2Store>>,
) -> Result<RemiPromotionActivationOutcome> {
    let evidence = reopen_remi_promotion_evidence(&config.promotion_evidence_path)
        .context("reopen exact Remi promotion evidence")?;
    let evidence_bytes = conary_core::json::canonical_json(&evidence)
        .map_err(anyhow::Error::msg)
        .context("canonicalize reopened Remi promotion evidence")?;
    let promotion_evidence_sha256 = conary_core::hash::sha256(&evidence_bytes);
    let (crawl, crawl_bytes) = reopen_conversion_crawl(&config.conversion_crawl_path)
        .context("reopen exact complete conversion crawl")?;
    let conversion_crawl_sha256 = conary_core::hash::sha256(&crawl_bytes);
    ensure!(
        evidence.conversion_crawl_sha256 == conversion_crawl_sha256,
        "promotion evidence names a different complete conversion crawl"
    );

    let conn = super::open_runtime_db(&config.db_path)?;
    let canonical_map = load_canonical_map_snapshot(&conn)?;
    verify_canonical_binding(&evidence, &canonical_map)?;
    let profiles = resolve_profiles(
        &conn,
        &config.catalog_dir,
        catalog_authority,
        &evidence,
        &crawl,
    )?;
    let active = load_active_universe(&conn)?;
    let spool = build_object_spool(&conn, &crawl)?;
    drop(conn);

    let object_authority = r2_store.map_or_else(
        || DurableObjectAuthority::Local(config.chunk_dir.clone()),
        DurableObjectAuthority::R2,
    );
    let reopened_objects = reopen_all_objects(&spool, object_authority).await?;

    if let Some(active_binding) = active.as_ref()
        && active_matches(
            active_binding,
            &profiles,
            &canonical_map,
            &promotion_evidence_sha256,
            &conversion_crawl_sha256,
        )?
    {
        verify_published_bundle(
            &config.catalog_dir,
            &active_binding.manifest,
            &active_binding.manifest_sha256,
            Some(catalog_authority),
        )?;
        return Ok(RemiPromotionActivationOutcome::AlreadyActive {
            manifest_sha256: active_binding.manifest_sha256.clone(),
            sequence: active_binding.sequence,
        });
    }

    let promoted_profiles = profiles
        .iter()
        .filter(|profile| profile.activation.is_some())
        .count();
    ensure!(
        promoted_profiles > 0,
        "promotion has no exact current candidate and does not replay the active universe"
    );
    let base_sequence = active.as_ref().map_or(0, |active| active.sequence);
    let candidate = build_signed_candidate(
        base_sequence,
        &profiles,
        &canonical_map,
        &config.repository_keys_dir,
    )?;
    let bundle = publish_candidate_files(
        &config.catalog_candidate_dir,
        &config.catalog_dir,
        &candidate,
        Some(catalog_authority),
    )?;

    database_writer.execute(|| {
        activate_transaction(
            &config.db_path,
            &profiles,
            &candidate,
            &bundle,
            active.as_ref(),
            &promotion_evidence_sha256,
            &conversion_crawl_sha256,
            &canonical_map,
        )
    })?;
    Ok(RemiPromotionActivationOutcome::Activated {
        manifest_sha256: candidate.manifest_sha256,
        sequence: candidate.manifest.sequence,
        promoted_profiles,
        reopened_objects,
    })
}

fn verify_canonical_binding(
    evidence: &RemiPromotionEvidenceV1,
    canonical_map: &CanonicalMapSnapshot,
) -> Result<()> {
    let bytes = canonical_bytes(canonical_map)?;
    ensure!(
        evidence.canonical_map.sha256 == conary_core::hash::sha256(&bytes)
            && evidence.canonical_map.revision == canonical_map.revision
            && evidence.canonical_map.entry_count == u64::try_from(canonical_map.entries.len())?,
        "promotion canonical-map authority changed after evidence production"
    );
    Ok(())
}

fn resolve_profiles(
    conn: &Connection,
    catalog_dir: &Path,
    authority: &CatalogAuthority,
    evidence: &RemiPromotionEvidenceV1,
    crawl: &RemiConversionCrawlV4,
) -> Result<Vec<PromotionProfile>> {
    ensure!(
        evidence.profiles.len() == crawl.profiles.len(),
        "promotion evidence and crawl profile counts differ"
    );
    evidence
        .profiles
        .iter()
        .zip(&crawl.profiles)
        .map(|(evidence_profile, crawl_profile)| {
            ensure!(
                evidence_profile.profile == crawl_profile.profile
                    && evidence_profile.profile_revision_sha256
                        == crawl_profile.profile_revision_sha256,
                "promotion evidence and crawl profile authority differ"
            );
            let selection = ProfileRevisionSelection {
                source_profile: evidence_profile.profile.clone(),
                profile_revision_sha256: evidence_profile.profile_revision_sha256.clone(),
            };
            let pin = authority
                .open_selected_profile(&selection)
                .with_context(|| {
                    format!("reopen promotion profile '{}'", selection.source_profile)
                })?;
            let manifest = pin.manifest().clone();
            ensure!(
                manifest.catalog.sha256 == evidence_profile.catalog_sha256
                    && manifest.catalog.size == evidence_profile.catalog_size,
                "promotion profile catalog differs from its exact evidence"
            );
            reopen_source_catalogs(conn, catalog_dir, &manifest)?;
            let candidate = current_profile_sync_candidate(conn, &selection.source_profile)?;
            let active = RemiActiveProfileRevision::find(conn, &selection.source_profile)?;
            let activation = resolve_activation(&manifest, candidate, active)?;
            Ok(PromotionProfile {
                manifest,
                activation,
                _pin: pin,
            })
        })
        .collect()
}

fn resolve_activation(
    manifest: &ProfileRevisionV2,
    candidate: Option<ProfileSyncCandidate>,
    active: Option<RemiActiveProfileRevision>,
) -> Result<Option<RemiProfileRevisionActivation>> {
    let revision = manifest.manifest_sha256()?;
    if let Some(candidate) = candidate {
        ensure!(
            candidate.profile_revision_sha256 == revision,
            "profile '{}' current candidate changed after promotion evidence",
            manifest.profile
        );
        return Ok(Some(RemiProfileRevisionActivation {
            source_profile: manifest.profile.clone(),
            profile_revision_sha256: revision,
            artifact_sha256: manifest.catalog.sha256.clone(),
            artifact_size: i64::try_from(manifest.catalog.size)
                .context("promotion catalog size exceeds SQLite INTEGER range")?,
            logical_digest_sha256: manifest.logical_digest_sha256.clone(),
            run_id: candidate.run_id,
            owner_instance_uuid: candidate.owner_instance_uuid,
            fencing_epoch: candidate.fencing_epoch,
        }));
    }
    ensure!(
        active
            .as_ref()
            .is_some_and(|active| active.profile_revision_sha256 == revision),
        "profile '{}' is neither the exact current candidate nor already active",
        manifest.profile
    );
    Ok(None)
}

fn reopen_source_catalogs(
    conn: &Connection,
    catalog_dir: &Path,
    profile: &ProfileRevisionV2,
) -> Result<()> {
    for member in &profile.members {
        let resource = RemiCatalogResource::find_by_sha256(conn, &member.source_snapshot_sha256)?
            .with_context(|| {
            format!(
                "profile '{}' source {} is not registered",
                profile.profile, member.source_snapshot_sha256
            )
        })?;
        ensure!(
            resource.kind == RemiCatalogResourceKind::SourceSnapshot
                && resource.source_profile == profile.profile
                && resource.durable,
            "profile '{}' source {} lacks exact durable authority",
            profile.profile,
            member.source_snapshot_sha256
        );
        let manifest: SourceSnapshotV1 = serde_json::from_str(&resource.manifest_json)
            .context("parse registered promotion source manifest")?;
        ensure!(
            manifest.manifest_sha256()? == member.source_snapshot_sha256
                && manifest.catalog.sha256 == resource.artifact_sha256
                && i64::try_from(manifest.catalog.size)? == resource.artifact_size
                && manifest.logical_digest_sha256 == resource.logical_digest_sha256,
            "profile '{}' source {} metadata drifted",
            profile.profile,
            member.source_snapshot_sha256
        );
        verify_source_catalog_bundle(
            catalog_dir
                .join("sources")
                .join(&member.source_snapshot_sha256),
            &manifest,
        )
        .with_context(|| {
            format!(
                "reopen promotion source catalog {}",
                member.source_snapshot_sha256
            )
        })?;
    }
    Ok(())
}

struct ObjectSpool {
    _directory: tempfile::TempDir,
    path: PathBuf,
}

fn build_object_spool(conn: &Connection, crawl: &RemiConversionCrawlV4) -> Result<ObjectSpool> {
    let directory = tempfile::tempdir().context("create promotion object spool")?;
    let path = directory.path().join("objects.sqlite");
    let mut spool = Connection::open(&path).context("open promotion object spool")?;
    spool.execute_batch(
        "PRAGMA journal_mode = OFF;
         PRAGMA synchronous = OFF;
         CREATE TABLE objects (
             sha256 TEXT PRIMARY KEY,
             size INTEGER NOT NULL CHECK(size >= 0)
         ) WITHOUT ROWID;",
    )?;
    let tx = spool.transaction()?;
    for profile in &crawl.profiles {
        for outcome in &profile.outcomes {
            ensure!(
                outcome.state == ConversionCrawlOutcomeStateV4::Succeeded,
                "complete promotion crawl contains a failed package"
            );
            let proof = outcome
                .conversion_proof
                .as_ref()
                .context("successful promotion crawl package has no proof")?;
            let transport = reopen_promotion_binding(
                conn,
                &profile.profile_revision_sha256,
                &outcome.repository_checksum,
                proof,
            )?;
            for object in transport.objects {
                let size = i64::try_from(object.size)
                    .context("promotion CAS object size exceeds SQLite INTEGER range")?;
                tx.execute(
                    "INSERT INTO objects (sha256, size) VALUES (?1, ?2)
                     ON CONFLICT(sha256) DO NOTHING",
                    params![&object.sha256, size],
                )?;
                let stored: i64 = tx.query_row(
                    "SELECT size FROM objects WHERE sha256 = ?1",
                    [&object.sha256],
                    |row| row.get(0),
                )?;
                ensure!(
                    stored == size,
                    "promotion transports contradict CAS object {} size",
                    object.sha256
                );
            }
        }
    }
    tx.commit()?;
    drop(spool);
    Ok(ObjectSpool {
        _directory: directory,
        path,
    })
}

async fn reopen_all_objects(spool: &ObjectSpool, authority: DurableObjectAuthority) -> Result<u64> {
    let mut last = String::new();
    let mut reopened = 0_u64;
    loop {
        let conn = Connection::open(&spool.path)?;
        let mut statement = conn.prepare(
            "SELECT sha256, size FROM objects WHERE sha256 > ?1
             ORDER BY sha256 LIMIT ?2",
        )?;
        let batch = statement
            .query_map(params![&last, OBJECT_REOPEN_BATCH as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        drop(conn);
        if batch.is_empty() {
            break;
        }
        stream::iter(batch.iter().cloned())
            .map(|(hash, size)| {
                let authority = authority.clone();
                async move { reopen_object(&authority, &hash, u64::try_from(size)?).await }
            })
            .buffer_unordered(OBJECT_REOPEN_CONCURRENCY)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        reopened = reopened
            .checked_add(u64::try_from(batch.len())?)
            .context("promotion reopened-object count overflow")?;
        last = batch.last().expect("nonempty batch").0.clone();
    }
    Ok(reopened)
}

async fn reopen_object(
    authority: &DurableObjectAuthority,
    hash: &str,
    expected_size: u64,
) -> Result<()> {
    let bytes = match authority {
        DurableObjectAuthority::Local(chunk_dir) => {
            let path = super::handlers::cas_object_path(chunk_dir, hash);
            let metadata = tokio::fs::symlink_metadata(&path)
                .await
                .with_context(|| format!("inspect durable CAS object {hash}"))?;
            ensure!(
                metadata.file_type().is_file(),
                "durable CAS object {hash} is not a plain file"
            );
            tokio::fs::read(&path)
                .await
                .with_context(|| format!("reopen durable CAS object {hash}"))?
        }
        DurableObjectAuthority::R2(store) => store
            .get_chunk(hash)
            .await
            .with_context(|| format!("reopen R2 CAS object {hash}"))?
            .with_context(|| format!("durable R2 CAS object {hash} is missing"))?,
    };
    ensure!(
        bytes.len() as u64 == expected_size,
        "durable CAS object {hash} size drifted"
    );
    ensure!(
        conary_core::hash::sha256(&bytes) == hash,
        "durable CAS object {hash} digest drifted"
    );
    Ok(())
}

fn load_active_universe(conn: &Connection) -> Result<Option<ActiveUniverseBinding>> {
    conn.query_row(
        "SELECT active.manifest_sha256, active.sequence,
                revision.promotion_evidence_sha256,
                revision.conversion_crawl_sha256, revision.manifest_json
         FROM remi_active_universe_revision active
         JOIN remi_universe_revisions revision
           ON revision.manifest_sha256 = active.manifest_sha256
         WHERE active.singleton = 1",
        [],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        },
    )
    .optional()?
    .map(
        |(manifest_sha256, sequence, promotion_evidence, conversion_crawl, manifest_json)| {
            let manifest: RemiUniverseManifestV2 = serde_json::from_str(&manifest_json)
                .context("parse active Remi universe manifest")?;
            manifest.validate().map_err(anyhow::Error::from)?;
            ensure!(
                manifest.manifest_sha256()? == manifest_sha256
                    && manifest.sequence == u64::try_from(sequence)?,
                "active Remi universe pointer disagrees with its manifest"
            );
            Ok(ActiveUniverseBinding {
                manifest_sha256,
                sequence: u64::try_from(sequence)?,
                promotion_evidence_sha256: promotion_evidence,
                conversion_crawl_sha256: conversion_crawl,
                manifest,
            })
        },
    )
    .transpose()
}

fn active_matches(
    active: &ActiveUniverseBinding,
    profiles: &[PromotionProfile],
    canonical_map: &CanonicalMapSnapshot,
    promotion_evidence_sha256: &str,
    conversion_crawl_sha256: &str,
) -> Result<bool> {
    let mut expected_profiles = profiles
        .iter()
        .map(|profile| &profile.manifest)
        .collect::<Vec<_>>();
    expected_profiles.sort_by(|left, right| left.profile.cmp(&right.profile));
    Ok(profiles.iter().all(|profile| profile.activation.is_none())
        && active.promotion_evidence_sha256 == promotion_evidence_sha256
        && active.conversion_crawl_sha256 == conversion_crawl_sha256
        && active.manifest.canonical_map.sha256
            == conary_core::hash::sha256(&canonical_bytes(canonical_map)?)
        && active.manifest.profiles.len() == profiles.len()
        && active
            .manifest
            .profiles
            .iter()
            .zip(expected_profiles)
            .all(|(member, profile)| member.revision == *profile))
}

fn build_signed_candidate(
    base_sequence: u64,
    profiles: &[PromotionProfile],
    canonical_map: &CanonicalMapSnapshot,
    repository_keys_dir: &Path,
) -> Result<SignedUniverseCandidate> {
    let root = load_universe_root_metadata(repository_keys_dir)?;
    let mut manifests = profiles
        .iter()
        .map(|profile| profile.manifest.clone())
        .collect::<Vec<_>>();
    manifests.sort_by(|left, right| left.profile.cmp(&right.profile));
    build_candidate(
        base_sequence,
        manifests,
        canonical_bytes(canonical_map)?,
        root,
        repository_keys_dir,
    )
}

#[allow(clippy::too_many_arguments)]
fn activate_transaction(
    db_path: &Path,
    profiles: &[PromotionProfile],
    candidate: &SignedUniverseCandidate,
    _bundle: &Path,
    expected_active: Option<&ActiveUniverseBinding>,
    promotion_evidence_sha256: &str,
    conversion_crawl_sha256: &str,
    canonical_map: &CanonicalMapSnapshot,
) -> Result<()> {
    let conn = super::open_runtime_db(db_path)?;
    let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
    require_active_universe_unchanged(&tx, expected_active)?;
    let current_canonical = load_canonical_map_snapshot(&tx)?;
    ensure!(
        canonical_bytes(&current_canonical)? == canonical_bytes(canonical_map)?,
        "canonical map changed while promotion was being built"
    );
    let now = chrono::Utc::now().timestamp();
    for profile in profiles {
        if let Some(activation) = &profile.activation {
            publish_profile_candidate_in_transaction(&tx, activation, now)
                .map_err(anyhow::Error::from)?;
        }
    }
    require_active_profiles(&tx, &candidate.manifest)?;
    insert_universe_revision(
        &tx,
        candidate,
        promotion_evidence_sha256,
        conversion_crawl_sha256,
        now,
    )?;
    tx.commit()?;
    Ok(())
}

fn require_active_universe_unchanged(
    tx: &Transaction<'_>,
    expected: Option<&ActiveUniverseBinding>,
) -> Result<()> {
    let current = tx
        .query_row(
            "SELECT manifest_sha256, sequence FROM remi_active_universe_revision
             WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    let expected = expected.map(|active| {
        Ok::<_, anyhow::Error>((
            active.manifest_sha256.clone(),
            i64::try_from(active.sequence)?,
        ))
    });
    ensure!(
        current == expected.transpose()?,
        "active Remi universe changed while promotion was being built"
    );
    Ok(())
}

fn require_active_profiles(tx: &Transaction<'_>, manifest: &RemiUniverseManifestV2) -> Result<()> {
    for profile in &manifest.profiles {
        let active =
            RemiActiveProfileRevision::find(tx, &profile.revision.profile)?.with_context(|| {
                format!(
                    "profile '{}' has no active pointer",
                    profile.revision.profile
                )
            })?;
        ensure!(
            active.profile_revision_sha256 == profile.profile_revision_sha256,
            "profile '{}' active pointer differs from promoted universe",
            profile.revision.profile
        );
    }
    Ok(())
}

fn insert_universe_revision(
    tx: &Transaction<'_>,
    candidate: &SignedUniverseCandidate,
    promotion_evidence_sha256: &str,
    conversion_crawl_sha256: &str,
    now: i64,
) -> Result<()> {
    let sequence = i64::try_from(candidate.manifest.sequence)?;
    tx.execute(
        "INSERT INTO remi_universe_revisions (
             manifest_sha256, sequence, promotion_evidence_sha256,
             conversion_crawl_sha256, metadata_root_sha256,
             canonical_map_sha256, canonical_map_size, targets_version,
             snapshot_version, timestamp_version, manifest_json, durable, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, ?12)",
        params![
            &candidate.manifest_sha256,
            sequence,
            promotion_evidence_sha256,
            conversion_crawl_sha256,
            &candidate.manifest.metadata_root_sha256,
            &candidate.manifest.canonical_map.sha256,
            i64::try_from(candidate.manifest.canonical_map.size)?,
            i64::try_from(candidate.targets.signed.version)?,
            i64::try_from(candidate.snapshot.signed.version)?,
            i64::try_from(candidate.timestamp.signed.version)?,
            String::from_utf8(candidate.manifest_bytes.clone())?,
            now,
        ],
    )?;
    for profile in &candidate.manifest.profiles {
        tx.execute(
            "INSERT INTO remi_universe_profile_revisions (
                 manifest_sha256, ordinal, source_profile, profile_revision_sha256,
                 catalog_sha256, catalog_size
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &candidate.manifest_sha256,
                i64::from(profile.ordinal),
                &profile.revision.profile,
                &profile.profile_revision_sha256,
                &profile.catalog.sha256,
                i64::try_from(profile.catalog.size)?,
            ],
        )?;
    }
    tx.execute(
        "INSERT INTO remi_active_universe_revision (
             singleton, manifest_sha256, sequence, activated_at
         ) VALUES (1, ?1, ?2, ?3)
         ON CONFLICT(singleton) DO UPDATE SET
             manifest_sha256 = excluded.manifest_sha256,
             sequence = excluded.sequence,
             activated_at = excluded.activated_at",
        params![&candidate.manifest_sha256, sequence, now],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::DirBuilderExt;

    use conary_core::ccs::attestation::{
        BuildOutputIdentity, FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1, ForeignConversionBoundary,
    };
    use conary_core::ccs::{CcsTransportEnvelopeV1, CcsTransportObjectV1};
    use conary_core::db::models::{ConvertedPackage, MetadataTable, set_metadata};
    use conary_core::repository::catalog::{
        NATIVE_PARITY_COMPARISON_SCHEMA_V1, NATIVE_RESOLUTION_COMPARISON_SCHEMA_V1,
        NativeParityComparisonV1, NativeParityCountsV1, NativeResolutionComparisonV1,
        NativeResolutionCountsV1,
    };

    use super::*;
    use crate::server::catalog_authority::ProfileRevisionSelection;
    use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};
    use crate::server::conversion::{ScriptletPackageMetadata, ServerConversionResult};
    use crate::server::conversion_crawl::{
        CCS_ARTIFACT_REOPEN_PROOF_SCHEMA_V1, CCS_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
        CcsArtifactReopenProofV1, CcsTargetCompatibilityProofV1, ConversionCrawlPackageOutcomeV4,
        ConversionCrawlProfileV4, ConversionProofDispositionV1, ConversionProofStore,
        REMI_CONVERSION_CRAWL_SCHEMA_V4, ReopenedCcsArtifactEvidence,
        write_and_reopen_conversion_crawl,
    };
    use crate::server::promotion_evidence::{
        REMI_PROMOTION_EVIDENCE_SCHEMA_V1, RemiPromotionCanonicalMapV1,
        RemiPromotionProfileEvidenceV1,
    };
    use crate::server::signing_authority::ensure_universe_authority;

    struct PromotionFixture {
        catalogs: ActiveCatalogFixture,
        config: RemiPromotionActivationConfig,
        object_path: PathBuf,
    }

    impl PromotionFixture {
        fn new() -> Self {
            let catalogs = ActiveCatalogFixture::new();
            let root = catalogs
                .catalog_dir()
                .parent()
                .expect("promotion fixture root")
                .to_path_buf();
            let candidate_dir = root.join("universe-candidates");
            let chunk_dir = root.join("chunks");
            let keys_dir = root.join("repository-keys");
            fs::create_dir(&candidate_dir).expect("create promotion candidate directory");
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&keys_dir)
                .expect("create promotion key directory");
            ensure_universe_authority(&keys_dir).expect("provision universe keys");

            let conn = catalogs.connection();
            set_metadata(&conn, MetadataTable::Server, "canonical_map_revision", "1")
                .expect("set canonical revision");
            set_metadata(
                &conn,
                MetadataTable::Server,
                "last_canonical_rebuild",
                "2026-08-25T00:00:00Z",
            )
            .expect("set canonical timestamp");
            drop(conn);

            let object_bytes = b"durable promotion object";
            let object_sha256 = conary_core::hash::sha256(object_bytes);
            let object_path = crate::server::handlers::cas_object_path(&chunk_dir, &object_sha256);
            fs::create_dir_all(object_path.parent().expect("CAS object parent"))
                .expect("create CAS object directory");
            fs::write(&object_path, object_bytes).expect("write durable CAS object");

            let evidence_dir = root.join("evidence");
            fs::create_dir(&evidence_dir).expect("create evidence directory");
            let crawl_path = evidence_dir.join("crawl.json");
            let evidence_path = evidence_dir.join("promotion.json");
            let mut crawl_profiles = Vec::new();
            let mut evidence_profiles = Vec::new();
            for (ordinal, profile) in conary_core::repository::supported_profiles::public_profiles()
                .iter()
                .enumerate()
            {
                let architecture = if profile.id() == "ubuntu-26.04" {
                    "amd64"
                } else {
                    "x86_64"
                };
                let mut input = package(
                    profile.id(),
                    "demo",
                    "1.0",
                    "1",
                    Some(architecture),
                    42,
                    profile.id(),
                );
                input.checksum = format!(
                    "sha256:{}",
                    conary_core::hash::sha256(profile.id().as_bytes())
                );
                let revision_sha256 = catalogs.candidate(
                    profile.id(),
                    i64::try_from(ordinal + 1).expect("fence fits i64"),
                    vec![input],
                );
                let pin = catalogs
                    .authority()
                    .open_selected_profile(&ProfileRevisionSelection {
                        source_profile: profile.id().to_string(),
                        profile_revision_sha256: revision_sha256.clone(),
                    })
                    .expect("open promotion candidate");
                let revision = pin.manifest().clone();
                let packages = pin.reader().packages().expect("read candidate packages");
                let candidate_package = packages.first().expect("fixture candidate package");
                let proof = install_conversion_proof(
                    &catalogs,
                    &revision_sha256,
                    candidate_package,
                    &object_sha256,
                    object_bytes,
                    &root,
                );
                crawl_profiles.push(ConversionCrawlProfileV4 {
                    profile: profile.id().to_string(),
                    profile_revision_sha256: revision_sha256.clone(),
                    expected_packages: 1,
                    outcomes: vec![ConversionCrawlPackageOutcomeV4 {
                        package_key_sha256: candidate_package.package_key_sha256.clone(),
                        name: candidate_package.name.clone(),
                        version: candidate_package.version.clone(),
                        package_release: candidate_package.package_release.clone(),
                        architecture: candidate_package.architecture.clone(),
                        repository_checksum: candidate_package.checksum.clone(),
                        state: ConversionCrawlOutcomeStateV4::Succeeded,
                        proof_disposition: Some(ConversionProofDispositionV1::Validated),
                        conversion_proof: Some(proof),
                        failure: None,
                    }],
                });
                let package_oracle_sha256 = conary_core::hash::sha256(
                    format!("package-oracle-{}", profile.id()).as_bytes(),
                );
                evidence_profiles.push(RemiPromotionProfileEvidenceV1 {
                    ordinal: u32::try_from(ordinal).expect("ordinal fits u32"),
                    profile: profile.id().to_string(),
                    profile_revision_sha256: revision_sha256.clone(),
                    catalog_sha256: revision.catalog.sha256.clone(),
                    catalog_size: revision.catalog.size,
                    package_parity: NativeParityComparisonV1 {
                        schema_version: NATIVE_PARITY_COMPARISON_SCHEMA_V1,
                        profile: profile.id().to_string(),
                        profile_revision_sha256: revision_sha256.clone(),
                        oracle_manifest_sha256: package_oracle_sha256.clone(),
                        counts: NativeParityCountsV1::from(revision.counts),
                    },
                    resolution_parity: NativeResolutionComparisonV1 {
                        schema_version: NATIVE_RESOLUTION_COMPARISON_SCHEMA_V1,
                        profile: profile.id().to_string(),
                        profile_revision_sha256: revision_sha256,
                        package_oracle_manifest_sha256: package_oracle_sha256,
                        oracle_manifest_sha256: conary_core::hash::sha256(
                            format!("native-resolution-{}", profile.id()).as_bytes(),
                        ),
                        candidate_manifest_sha256: conary_core::hash::sha256(
                            format!("candidate-resolution-{}", profile.id()).as_bytes(),
                        ),
                        counts: NativeResolutionCountsV1 {
                            roots: 1,
                            resolved_roots: 1,
                            unresolved_roots: 0,
                            closure_package_references: 1,
                            unresolved_dependencies: 0,
                        },
                    },
                });
            }
            let crawl = RemiConversionCrawlV4 {
                schema_version: REMI_CONVERSION_CRAWL_SCHEMA_V4,
                profiles: crawl_profiles,
            };
            write_and_reopen_conversion_crawl(&crawl_path, &crawl)
                .expect("write complete promotion crawl");
            let canonical_map = {
                let conn = catalogs.connection();
                load_canonical_map_snapshot(&conn).expect("load fixture canonical map")
            };
            let canonical_bytes = canonical_bytes(&canonical_map).expect("canonical map bytes");
            let evidence = RemiPromotionEvidenceV1 {
                schema_version: REMI_PROMOTION_EVIDENCE_SCHEMA_V1,
                conversion_crawl_sha256: conary_core::hash::sha256(
                    &fs::read(&crawl_path).expect("read crawl bytes"),
                ),
                canonical_map: RemiPromotionCanonicalMapV1 {
                    sha256: conary_core::hash::sha256(&canonical_bytes),
                    revision: canonical_map.revision,
                    entry_count: u64::try_from(canonical_map.entries.len())
                        .expect("entry count fits u64"),
                },
                profiles: evidence_profiles,
            };
            fs::write(
                &evidence_path,
                conary_core::json::canonical_json(&evidence).expect("serialize promotion evidence"),
            )
            .expect("write promotion evidence");

            let config = RemiPromotionActivationConfig {
                db_path: catalogs.db_path().to_path_buf(),
                catalog_dir: catalogs.catalog_dir().to_path_buf(),
                catalog_candidate_dir: candidate_dir,
                chunk_dir,
                repository_keys_dir: keys_dir,
                promotion_evidence_path: evidence_path,
                conversion_crawl_path: crawl_path,
            };
            Self {
                catalogs,
                config,
                object_path,
            }
        }

        async fn activate(&self) -> Result<RemiPromotionActivationOutcome> {
            activate_remi_promotion(
                &self.config,
                &DatabaseWriter::default(),
                self.catalogs.authority(),
                None,
            )
            .await
        }
    }

    fn install_conversion_proof(
        catalogs: &ActiveCatalogFixture,
        revision_sha256: &str,
        package: &conary_core::repository::catalog::CatalogPackageRecordV1,
        object_sha256: &str,
        object_bytes: &[u8],
        root: &Path,
    ) -> crate::server::conversion_crawl::ConversionProofV1 {
        let source_sha256 = package
            .checksum
            .strip_prefix("sha256:")
            .expect("fixture source checksum");
        let boundary = ForeignConversionBoundary {
            schema_version: FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1,
            source_format: conary_core::repository::supported_profiles::profile_by_public_id(
                &package.source_profile,
            )
            .expect("public fixture profile")
            .package_format()
            .as_str()
            .to_string(),
            source_checksum: package.checksum.clone(),
            output_identity: BuildOutputIdentity {
                file_merkle_root: "e".repeat(64),
                package_name: package.name.clone(),
                package_version: package.version.clone(),
                package_release: package.package_release.clone(),
                architecture: package.architecture.clone(),
                origin_class: "foreign_conversion".to_string(),
                hardening_level: "converted".to_string(),
                hermetic_evidence_hash: "f".repeat(64),
                canonical_content_identity: "1".repeat(64),
            },
            build_risk_report_hash: None,
            build_risk_report: None,
            scriptlet_risk_report_hash: None,
            scriptlet_risk_report: None,
            diagnostics: Vec::new(),
        };
        let transport = CcsTransportEnvelopeV1 {
            schema_version: conary_core::ccs::transport::CCS_TRANSPORT_SCHEMA_V1,
            manifest_base64: String::new(),
            signature_json: "{}".to_string(),
            debug_toml_base64: None,
            build_attestation_json: None,
            foreign_conversion_boundary_json: Some(
                serde_json::to_string(&boundary).expect("serialize fixture boundary"),
            ),
            objects: vec![CcsTransportObjectV1 {
                sha256: object_sha256.to_string(),
                size: u64::try_from(object_bytes.len()).expect("object size fits u64"),
            }],
        };
        let ccs_bytes = format!("fixture CCS for {}", package.source_profile).into_bytes();
        let ccs_sha256 = conary_core::hash::sha256(&ccs_bytes);
        let ccs_path = root.join(format!("{}.ccs", package.source_profile));
        fs::write(&ccs_path, &ccs_bytes).expect("write fixture CCS");
        let signer_sha256 = "3".repeat(64);
        let reopened = ReopenedCcsArtifactEvidence {
            source_artifact_sha256: source_sha256.to_string(),
            ccs_sha256: ccs_sha256.clone(),
            reopen_proof: CcsArtifactReopenProofV1 {
                schema_version: CCS_ARTIFACT_REOPEN_PROOF_SCHEMA_V1,
                ccs_format_version: conary_core::ccs::v3::FORMAT_VERSION_V3,
                foreign_conversion_boundary_schema_version: FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1,
                signer_public_key_sha256: signer_sha256.clone(),
                transport_sha256: conary_core::hash::sha256(
                    &conary_core::json::canonical_json(&transport)
                        .expect("serialize fixture transport"),
                ),
                verified_files: 1,
                verified_objects: 1,
            },
            target_compatibility_proofs: conary_core::ccs::supported_target_contracts()
                .iter()
                .map(|contract| CcsTargetCompatibilityProofV1 {
                    schema_version: CCS_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
                    ccs_sha256: ccs_sha256.clone(),
                    compatibility: conary_core::ccs::StaticTargetCompatibilityProofV1 {
                        schema_version:
                            conary_core::ccs::STATIC_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
                        target_profile: contract.target_profile,
                        target_contract_sha256: contract.sha256().expect("target contract digest"),
                        required_capabilities: Vec::new(),
                        required_systemd_operations: Vec::new(),
                        required_linux_process_capabilities: Vec::new(),
                    },
                })
                .collect(),
        };
        let result = ServerConversionResult {
            name: package.name.clone(),
            version: package.version.clone(),
            source_profile: Some(package.source_profile.clone()),
            transport: transport.clone(),
            total_size: u64::try_from(ccs_bytes.len()).expect("CCS size fits u64"),
            content_hash: format!("sha256:{ccs_sha256}"),
            ccs_path: ccs_path.clone(),
            cache_state: "cold".to_string(),
            scriptlets: ScriptletPackageMetadata {
                scriptlet_fidelity: "native-free".to_string(),
                evidence_digest: None,
            },
            timing: None,
        };
        let original_format = conary_core::repository::supported_profiles::profile_by_public_id(
            &package.source_profile,
        )
        .expect("public fixture profile")
        .package_format()
        .as_str()
        .to_string();
        let conn = catalogs.connection();
        let mut converted = ConvertedPackage::new_repository(
            package.source_profile.clone(),
            revision_sha256.to_string(),
            package.name.clone(),
            package.version.clone(),
            package.architecture.clone().expect("fixture architecture"),
            original_format,
            package.checksum.clone(),
            &transport,
            i64::try_from(ccs_bytes.len()).expect("CCS size fits i64"),
            format!("sha256:{ccs_sha256}"),
            ccs_path.to_string_lossy().to_string(),
            conary_core::ccs::attestation::canonical_json_hash(&package.provides)
                .expect("provides digest"),
        );
        converted
            .set_scriptlet_metadata(&conary_core::ccs::convert::ScriptletBundleSummary::default())
            .expect("scriptlet summary");
        converted
            .insert_with_conversion_pin(&conn, 1)
            .expect("insert fixture conversion");
        drop(conn);
        ConversionProofStore::new(catalogs.db_path().to_path_buf(), DatabaseWriter::default())
            .publish(package, revision_sha256, &signer_sha256, &result, &reopened)
            .expect("publish fixture conversion proof")
    }

    #[tokio::test]
    async fn exact_promotion_activates_atomically_and_replays() {
        let fixture = PromotionFixture::new();
        let first = fixture.activate().await.expect("activate exact promotion");
        let RemiPromotionActivationOutcome::Activated {
            manifest_sha256,
            sequence: 1,
            promoted_profiles: 3,
            reopened_objects: 1,
        } = first
        else {
            panic!("initial promotion did not atomically activate")
        };
        let conn = fixture.catalogs.connection();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM remi_active_profile_revisions",
                [],
                |row| { row.get::<_, i64>(0) }
            )
            .unwrap(),
            3
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM repository_sync_runs WHERE state = 'published'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            3
        );
        let evidence_sha256 =
            conary_core::hash::sha256(&fs::read(&fixture.config.promotion_evidence_path).unwrap());
        let crawl_sha256 =
            conary_core::hash::sha256(&fs::read(&fixture.config.conversion_crawl_path).unwrap());
        assert_eq!(
            conn.query_row(
                "SELECT manifest_sha256, sequence, promotion_evidence_sha256,
                        conversion_crawl_sha256
                 FROM remi_universe_revisions",
                [],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?
                )),
            )
            .unwrap(),
            (manifest_sha256.clone(), 1, evidence_sha256, crawl_sha256)
        );
        drop(conn);
        assert_eq!(
            fixture.activate().await.expect("replay exact promotion"),
            RemiPromotionActivationOutcome::AlreadyActive {
                manifest_sha256,
                sequence: 1,
            }
        );
    }

    #[tokio::test]
    async fn corrupt_cas_object_preserves_all_candidate_state() {
        let fixture = PromotionFixture::new();
        fs::write(&fixture.object_path, b"corrupt").expect("corrupt CAS object");
        let error = fixture.activate().await.expect_err("corrupt CAS must fail");
        assert!(format!("{error:#}").contains("size drifted"), "{error:#}");
        let conn = fixture.catalogs.connection();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM remi_active_profile_revisions",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM repository_sync_runs WHERE state = 'candidate'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            3
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM remi_active_universe_revision",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn successor_candidate_fences_stale_promotion() {
        let fixture = PromotionFixture::new();
        fixture.catalogs.candidate(
            "fedora-44",
            10,
            vec![package(
                "fedora-44",
                "demo",
                "2.0",
                "1",
                Some("x86_64"),
                43,
                "successor",
            )],
        );
        let error = fixture
            .activate()
            .await
            .expect_err("stale promotion must fail");
        assert!(
            format!("{error:#}").contains("current candidate changed after promotion evidence"),
            "{error:#}"
        );
        let conn = fixture.catalogs.connection();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM remi_active_profile_revisions",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM remi_active_universe_revision",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
}
