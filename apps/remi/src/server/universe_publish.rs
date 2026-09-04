// apps/remi/src/server/universe_publish.rs

//! Construction, signing, verification, and atomic publication of one Remi universe.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use chrono::{Duration, Utc};
use conary_core::canonical::{CanonicalMapSnapshot, validate_canonical_map_snapshot};
use conary_core::db::models::{
    RemiCatalogPhysicalAttestation, RemiCatalogResource, RemiCatalogResourceKind,
};
use conary_core::repository::catalog::{CATALOG_CONTENT_SCHEMA_V1, ProfileRevisionV2};
use conary_core::repository::universe::{
    REMI_UNIVERSE_SCHEMA_V2, RemiUniverseCanonicalMapObjectV2, RemiUniverseCatalogObjectV2,
    RemiUniverseManifestV2, RemiUniverseProfileV2, verify_remi_universe_manifest_target,
};
use conary_core::trust::verify::{
    extract_role_keys, verify_metadata_hash, verify_not_expired, verify_root, verify_signatures,
    verify_static_snapshot_consistency,
};
use conary_core::trust::{
    MetaFile, Role, Signed, SnapshotMetadata, TUF_SPEC_VERSION, TargetDescription, TargetsMetadata,
    TimestampMetadata, sign_tuf_metadata,
};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use tokio::sync::RwLock;

use super::ServerState;
use super::database_writer::DatabaseWriter;
use super::handlers::canonical::load_canonical_map_snapshot;
use super::signing_authority::{
    UniverseSigningRole, load_universe_role_key, load_universe_root_metadata,
};
use super::universe_validation::validate_canonical_candidate;

pub(crate) const UNIVERSE_MANIFEST_FILE: &str = "manifest.json";
pub(crate) const UNIVERSE_CANONICAL_MAP_FILE: &str = "canonical-map.json";
pub(crate) const UNIVERSE_ROOT_FILE: &str = "root.json";
pub(crate) const UNIVERSE_TARGETS_FILE: &str = "targets.json";
pub(crate) const UNIVERSE_SNAPSHOT_FILE: &str = "snapshot.json";
pub(crate) const UNIVERSE_TIMESTAMP_FILE: &str = "timestamp.json";

const UNIVERSE_FILES: [&str; 6] = [
    UNIVERSE_CANONICAL_MAP_FILE,
    UNIVERSE_MANIFEST_FILE,
    UNIVERSE_ROOT_FILE,
    UNIVERSE_SNAPSHOT_FILE,
    UNIVERSE_TARGETS_FILE,
    UNIVERSE_TIMESTAMP_FILE,
];
const UNIVERSE_RENEWAL_WINDOW: Duration = Duration::hours(6);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UniversePublicationOutcome {
    Unavailable,
    Unchanged {
        manifest_sha256: String,
        sequence: u64,
    },
    Activated {
        manifest_sha256: String,
        sequence: u64,
    },
}

/// Publish from the configured server roots without holding the state lock
/// across filesystem or SQLite work.
pub(crate) async fn publish_current_universe_from_state(
    state: &Arc<RwLock<ServerState>>,
) -> Result<UniversePublicationOutcome> {
    let (db_path, catalog_dir, candidate_dir, keys_root, database_writer) = {
        let guard = state.read().await;
        (
            guard.config.db_path.clone(),
            guard.config.catalog_dir.clone(),
            guard.config.catalog_candidate_dir.clone(),
            guard.config.release_publish.repository_keys_dir.clone(),
            guard.database_writer.clone(),
        )
    };
    let outcome = tokio::task::spawn_blocking(move || {
        publish_current_universe_from_roots(
            &db_path,
            &catalog_dir,
            &candidate_dir,
            keys_root.as_deref(),
            &database_writer,
        )
    })
    .await
    .context("signed Remi universe publication task did not complete")??;

    let activated = matches!(&outcome, UniversePublicationOutcome::Activated { .. });
    if !matches!(outcome, UniversePublicationOutcome::Unavailable) {
        let (db_path, catalog_authority, search_engine) = {
            let guard = state.read().await;
            (
                guard.config.db_path.clone(),
                guard.catalog_authority.clone(),
                guard.search_engine.clone(),
            )
        };
        if let Some(search_engine) = search_engine {
            let rebuild_engine = Arc::clone(&search_engine);
            let rebuild = tokio::task::spawn_blocking(move || {
                let universe = match super::public_universe::PublicUniverseSnapshot::load(&db_path)?
                {
                    super::public_universe::PublicUniverseLoadOutcome::Current(universe) => {
                        universe
                    }
                    super::public_universe::PublicUniverseLoadOutcome::NoActiveUniverse => {
                        anyhow::bail!("activated Remi universe pointer is absent")
                    }
                    super::public_universe::PublicUniverseLoadOutcome::ObsoleteUniverseSchema {
                        ..
                    } => {
                        anyhow::bail!("activated Remi universe schema is obsolete")
                    }
                    super::public_universe::PublicUniverseLoadOutcome::ObsoleteProfileSchema => {
                        anyhow::bail!("activated Remi universe contains obsolete profile revisions")
                    }
                };
                rebuild_engine
                    .rebuild_from_universe(&db_path, &catalog_authority, &universe)
                    .context("rebuild search projection for current Remi universe")?;
                Ok::<_, anyhow::Error>(())
            })
            .await;
            if let Err(error) = rebuild
                .context("current-universe search rebuild task did not complete")
                .and_then(|result| result)
            {
                if activated {
                    search_engine.mark_unavailable();
                    tracing::error!(%error, "Activated Remi universe has no current search projection");
                } else {
                    tracing::error!(%error, "Unchanged Remi universe search refresh failed; preserving its existing search authority");
                }
            }
        }
    }

    Ok(outcome)
}

#[derive(Debug)]
struct UniverseInputs {
    base_manifest_sha256: Option<String>,
    base_sequence: u64,
    base_promotion_evidence_sha256: Option<String>,
    base_conversion_crawl_sha256: Option<String>,
    base_profile_authority: Vec<(String, String)>,
    base_canonical_map_sha256: Option<String>,
    active_manifest: Option<RemiUniverseManifestV2>,
    profiles: Vec<ProfileRevisionV2>,
    profile_physical_attestations: BTreeMap<String, RemiCatalogPhysicalAttestation>,
    canonical_map: CanonicalMapSnapshot,
}

type ProfilePhysicalAttestations = BTreeMap<String, RemiCatalogPhysicalAttestation>;

pub(crate) struct SignedUniverseCandidate {
    pub(crate) manifest: RemiUniverseManifestV2,
    pub(crate) manifest_sha256: String,
    pub(crate) manifest_bytes: Vec<u8>,
    pub(crate) canonical_map_bytes: Vec<u8>,
    pub(crate) root: Signed<conary_core::trust::RootMetadata>,
    pub(crate) root_bytes: Vec<u8>,
    pub(crate) targets: Signed<TargetsMetadata>,
    pub(crate) targets_bytes: Vec<u8>,
    pub(crate) snapshot: Signed<SnapshotMetadata>,
    pub(crate) snapshot_bytes: Vec<u8>,
    pub(crate) timestamp: Signed<TimestampMetadata>,
    pub(crate) timestamp_bytes: Vec<u8>,
}

/// Publish the exact active profile set and canonical map as one signed public
/// universe. Files become durable before the one-row pointer transaction.
#[cfg(test)]
pub(crate) fn publish_current_universe(
    db_path: &Path,
    catalog_dir: &Path,
    candidate_dir: &Path,
    keys_root: Option<&Path>,
    database_writer: &DatabaseWriter,
) -> Result<UniversePublicationOutcome> {
    publish_current_universe_from_roots(
        db_path,
        catalog_dir,
        candidate_dir,
        keys_root,
        database_writer,
    )
}

#[cfg(test)]
fn publish_initial_universe_for_test(
    db_path: &Path,
    catalog_dir: &Path,
    candidate_dir: &Path,
    keys_root: &Path,
    database_writer: &DatabaseWriter,
) -> Result<UniversePublicationOutcome> {
    let mut inputs = database_writer.execute(|| load_inputs(db_path))?;
    anyhow::ensure!(
        inputs.active_manifest.is_none() && !inputs.profiles.is_empty(),
        "test initial universe requires profiles and no active universe"
    );
    inputs.base_promotion_evidence_sha256 = Some("e".repeat(64));
    inputs.base_conversion_crawl_sha256 = Some("c".repeat(64));
    let canonical_map_bytes = canonical_bytes(&inputs.canonical_map)?;
    validate_canonical_candidate(
        catalog_dir,
        &inputs.canonical_map,
        registered_profile_pairs(&inputs.profiles, &inputs.profile_physical_attestations)?,
    )?;
    let candidate = build_candidate(
        0,
        inputs.profiles.clone(),
        canonical_map_bytes,
        load_universe_root_metadata(keys_root)?,
        keys_root,
    )?;
    let bundle = publish_candidate_files(
        candidate_dir,
        catalog_dir,
        &candidate,
        &inputs.profile_physical_attestations,
    )?;
    database_writer.execute(|| activate_candidate(db_path, &inputs, &candidate, &bundle))?;
    Ok(UniversePublicationOutcome::Activated {
        manifest_sha256: candidate.manifest_sha256,
        sequence: candidate.manifest.sequence,
    })
}

fn publish_current_universe_from_roots(
    db_path: &Path,
    catalog_dir: &Path,
    candidate_dir: &Path,
    keys_root: Option<&Path>,
    database_writer: &DatabaseWriter,
) -> Result<UniversePublicationOutcome> {
    let inputs = database_writer.execute(|| load_inputs(db_path))?;
    if inputs.profiles.is_empty() {
        return Ok(UniversePublicationOutcome::Unavailable);
    }
    let keys_root = keys_root.context(
        "release_publish.repository_keys_dir is required to sign the public Remi universe",
    )?;
    let canonical_map_bytes = canonical_bytes(&inputs.canonical_map)?;
    let canonical_map_sha256 = conary_core::hash::sha256(&canonical_map_bytes);
    if inputs.base_manifest_sha256.is_none() {
        return Ok(UniversePublicationOutcome::Unavailable);
    }
    if let Some(manifest_sha256) = &inputs.base_manifest_sha256 {
        if !same_stored_authority(&inputs, &canonical_map_sha256)? {
            bail!("evidence-free universe publication cannot change active profile authority");
        }
        if let Some(active) = &inputs.active_manifest {
            verify_published_bundle(
                catalog_dir,
                active,
                manifest_sha256,
                &inputs.profile_physical_attestations,
            )?;
            if active_bundle_is_fresh(catalog_dir, active, manifest_sha256, Utc::now())? {
                return Ok(UniversePublicationOutcome::Unchanged {
                    manifest_sha256: manifest_sha256.clone(),
                    sequence: inputs.base_sequence,
                });
            }
        }
    }

    validate_canonical_candidate(
        catalog_dir,
        &inputs.canonical_map,
        registered_profile_pairs(&inputs.profiles, &inputs.profile_physical_attestations)?,
    )
    .context("validate canonical contracts against the candidate universe")?;
    let root = load_universe_root_metadata(keys_root)?;
    let candidate = build_candidate(
        inputs.base_sequence,
        inputs.profiles.clone(),
        canonical_map_bytes,
        root,
        keys_root,
    )?;
    let bundle = publish_candidate_files(
        candidate_dir,
        catalog_dir,
        &candidate,
        &inputs.profile_physical_attestations,
    )?;

    database_writer
        .execute(|| activate_candidate(db_path, &inputs, &candidate, &bundle))
        .with_context(|| {
            format!(
                "activate durable signed universe {}",
                candidate.manifest_sha256
            )
        })?;
    Ok(UniversePublicationOutcome::Activated {
        manifest_sha256: candidate.manifest_sha256,
        sequence: candidate.manifest.sequence,
    })
}

fn load_inputs(db_path: &Path) -> Result<UniverseInputs> {
    let conn = conary_core::db::open_fast(db_path)?;
    let active = conn
        .query_row(
            "SELECT active.manifest_sha256, active.sequence,
                    revision.promotion_evidence_sha256,
                    revision.conversion_crawl_sha256,
                    revision.canonical_map_sha256, revision.manifest_json
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
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;
    let (
        base_manifest_sha256,
        base_sequence,
        base_promotion_evidence_sha256,
        base_conversion_crawl_sha256,
        base_canonical_map_sha256,
        active_manifest,
        obsolete_profile_schema,
    ) = match active {
        Some((
            sha256,
            sequence,
            promotion_evidence,
            conversion_crawl,
            canonical_map_sha256,
            manifest_json,
        )) => {
            let sequence =
                u64::try_from(sequence).context("active universe sequence is negative")?;
            let (manifest, obsolete_profile_schema) = match super::universe_revision_inspection::inspect_stored_universe_manifest_v2(
                &sha256,
                i64::try_from(sequence).context("active universe sequence exceeds SQLite INTEGER range")?,
                &manifest_json,
            )? {
                super::universe_revision_inspection::StoredUniverseManifestV2::Current(manifest) => (Some(manifest), false),
                super::universe_revision_inspection::StoredUniverseManifestV2::ObsoleteUniverseSchema { .. } => (None, false),
                super::universe_revision_inspection::StoredUniverseManifestV2::ObsoleteProfileSchema => (None, true),
            };
            (
                Some(sha256),
                sequence,
                Some(promotion_evidence),
                Some(conversion_crawl),
                Some(canonical_map_sha256),
                manifest,
                obsolete_profile_schema,
            )
        }
        None => (None, 0, None, None, None, None, false),
    };

    let base_profile_authority = match &base_manifest_sha256 {
        Some(manifest_sha256) => {
            let mut statement = conn.prepare(
                "SELECT source_profile, profile_revision_sha256
                 FROM remi_universe_profile_revisions
                 WHERE manifest_sha256 = ?1
                 ORDER BY ordinal",
            )?;
            statement
                .query_map([manifest_sha256], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        }
        None => Vec::new(),
    };

    if obsolete_profile_schema {
        return Ok(UniverseInputs {
            base_manifest_sha256,
            base_sequence,
            base_promotion_evidence_sha256,
            base_conversion_crawl_sha256,
            base_profile_authority,
            base_canonical_map_sha256,
            active_manifest,
            profiles: Vec::new(),
            profile_physical_attestations: BTreeMap::new(),
            canonical_map: load_canonical_map_snapshot(&conn)?,
        });
    }

    let mut statement = conn.prepare(
        "SELECT resource.resource_sha256
         FROM remi_active_profile_revisions active
         JOIN remi_catalog_resources resource
           ON resource.resource_sha256 = active.profile_revision_sha256
         WHERE resource.resource_kind = 'profile_revision' AND resource.durable = 1
         ORDER BY active.source_profile COLLATE BINARY",
    )?;
    let resource_sha256s = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(statement);
    let mut profiles = Vec::new();
    let mut profile_physical_attestations = BTreeMap::new();
    for resource_sha256 in resource_sha256s {
        let resource = RemiCatalogResource::find_by_sha256(&conn, &resource_sha256)?
            .with_context(|| format!("active profile resource {resource_sha256} is absent"))?;
        anyhow::ensure!(
            resource.kind == RemiCatalogResourceKind::ProfileRevision && resource.durable,
            "active profile resource {resource_sha256} lacks durable profile authority"
        );
        let revision = serde_json::from_str::<ProfileRevisionV2>(&resource.manifest_json)
            .context("parse active profile revision")?;
        revision.validate()?;
        anyhow::ensure!(
            revision.manifest_sha256()? == resource_sha256
                && revision.catalog.sha256 == resource.artifact_sha256
                && i64::try_from(revision.catalog.size)? == resource.artifact_size
                && revision.logical_digest_sha256 == resource.logical_digest_sha256,
            "active profile resource {resource_sha256} metadata drifted"
        );
        if conary_core::repository::supported_profiles::profile_by_public_id(&revision.profile)
            .is_none()
        {
            continue;
        }
        anyhow::ensure!(
            profile_physical_attestations
                .insert(resource_sha256, resource.physical_attestation)
                .is_none(),
            "active public profile resource identity repeats"
        );
        profiles.push(revision);
    }
    let canonical_map = load_canonical_map_snapshot(&conn)?;
    Ok(UniverseInputs {
        base_manifest_sha256,
        base_sequence,
        base_promotion_evidence_sha256,
        base_conversion_crawl_sha256,
        base_profile_authority,
        base_canonical_map_sha256,
        active_manifest,
        profiles,
        profile_physical_attestations,
        canonical_map,
    })
}

fn registered_profile_pairs<'a>(
    profiles: &'a [ProfileRevisionV2],
    physical_attestations: &'a ProfilePhysicalAttestations,
) -> Result<Vec<(&'a ProfileRevisionV2, &'a RemiCatalogPhysicalAttestation)>> {
    anyhow::ensure!(
        profiles.len() == physical_attestations.len(),
        "profile physical authority count differs from the candidate universe"
    );
    profiles
        .iter()
        .map(|revision| {
            let revision_sha256 = revision.manifest_sha256()?;
            let physical_attestation = physical_attestations
                .get(&revision_sha256)
                .with_context(|| {
                    format!(
                        "profile '{}' revision {revision_sha256} lacks persisted physical authority",
                        revision.profile
                    )
                })?;
            Ok((revision, physical_attestation))
        })
        .collect()
}

fn same_stored_authority(inputs: &UniverseInputs, canonical_map_sha256: &str) -> Result<bool> {
    let current_profiles = inputs
        .profiles
        .iter()
        .map(|profile| Ok((profile.profile.clone(), profile.manifest_sha256()?)))
        .collect::<Result<Vec<_>>>()?;
    Ok(
        inputs.base_canonical_map_sha256.as_deref() == Some(canonical_map_sha256)
            && inputs.base_profile_authority == current_profiles,
    )
}

fn active_bundle_is_fresh(
    catalog_dir: &Path,
    active: &RemiUniverseManifestV2,
    manifest_sha256: &str,
    now: chrono::DateTime<Utc>,
) -> Result<bool> {
    let timestamp_bytes =
        fs::read(universe_bundle_path(catalog_dir, manifest_sha256).join(UNIVERSE_TIMESTAMP_FILE))
            .with_context(|| format!("read active universe timestamp for {manifest_sha256}"))?;
    let timestamp: Signed<TimestampMetadata> =
        serde_json::from_slice(&timestamp_bytes).context("parse active universe timestamp")?;
    Ok(!requires_renewal(
        now,
        active.expires_at,
        timestamp.signed.expires,
    ))
}

fn requires_renewal(
    now: chrono::DateTime<Utc>,
    manifest_expires: chrono::DateTime<Utc>,
    timestamp_expires: chrono::DateTime<Utc>,
) -> bool {
    manifest_expires <= now + UNIVERSE_RENEWAL_WINDOW
        || timestamp_expires <= now + UNIVERSE_RENEWAL_WINDOW
}

include!("universe_publish/candidate.rs");
include!("universe_publish/durable_bundle.rs");
include!("universe_publish/activation.rs");

#[cfg(test)]
mod tests;
