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
use conary_core::repository::catalog::{
    CATALOG_CONTENT_SCHEMA_V1, ProfileRevisionV1, verify_profile_catalog_bundle,
};
use conary_core::repository::universe::{
    REMI_UNIVERSE_SCHEMA_V1, RemiUniverseCanonicalMapObjectV1, RemiUniverseCatalogObjectV1,
    RemiUniverseManifestV1, RemiUniverseProfileV1, verify_remi_universe_manifest_target,
};
use conary_core::trust::ceremony::create_initial_root;
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
use super::signing_authority::{UniverseSigningRole, load_universe_role_key};

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
    tokio::task::spawn_blocking(move || {
        publish_current_universe(
            &db_path,
            &catalog_dir,
            &candidate_dir,
            keys_root.as_deref(),
            &database_writer,
        )
    })
    .await
    .context("signed Remi universe publication task did not complete")?
}

#[derive(Debug)]
struct UniverseInputs {
    base_manifest_sha256: Option<String>,
    base_sequence: u64,
    active_manifest: Option<RemiUniverseManifestV1>,
    profiles: Vec<ProfileRevisionV1>,
    canonical_map: CanonicalMapSnapshot,
}

struct SignedUniverseCandidate {
    manifest: RemiUniverseManifestV1,
    manifest_sha256: String,
    manifest_bytes: Vec<u8>,
    canonical_map_bytes: Vec<u8>,
    root: Signed<conary_core::trust::RootMetadata>,
    root_bytes: Vec<u8>,
    targets: Signed<TargetsMetadata>,
    targets_bytes: Vec<u8>,
    snapshot: Signed<SnapshotMetadata>,
    snapshot_bytes: Vec<u8>,
    timestamp: Signed<TimestampMetadata>,
    timestamp_bytes: Vec<u8>,
}

/// Publish the exact active profile set and canonical map as one signed public
/// universe. Files become durable before the one-row pointer transaction.
pub(crate) fn publish_current_universe(
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
    if let (Some(active), Some(manifest_sha256)) =
        (&inputs.active_manifest, &inputs.base_manifest_sha256)
        && same_authority(active, &inputs.profiles, &canonical_map_sha256)
    {
        verify_published_bundle(catalog_dir, active, manifest_sha256)?;
        if active_bundle_is_fresh(catalog_dir, active, manifest_sha256, Utc::now())? {
            return Ok(UniversePublicationOutcome::Unchanged {
                manifest_sha256: manifest_sha256.clone(),
                sequence: inputs.base_sequence,
            });
        }
    }

    let root = load_or_create_root(catalog_dir, keys_root, &inputs)?;
    let candidate = build_candidate(
        inputs.base_sequence,
        inputs.profiles.clone(),
        canonical_map_bytes,
        root,
        keys_root,
    )?;
    let bundle = publish_candidate_files(candidate_dir, catalog_dir, &candidate)?;
    verify_published_bundle(catalog_dir, &candidate.manifest, &candidate.manifest_sha256)?;

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
            "SELECT active.manifest_sha256, active.sequence, revision.manifest_json
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
                ))
            },
        )
        .optional()?;
    let (base_manifest_sha256, base_sequence, active_manifest) = match active {
        Some((sha256, sequence, manifest_json)) => {
            let sequence =
                u64::try_from(sequence).context("active universe sequence is negative")?;
            let manifest = serde_json::from_str::<RemiUniverseManifestV1>(&manifest_json)
                .context("parse active Remi universe manifest")?;
            manifest.validate().map_err(anyhow::Error::from)?;
            if manifest.sequence != sequence || manifest.manifest_sha256()? != sha256 {
                bail!("active Remi universe pointer disagrees with its manifest authority");
            }
            (Some(sha256), sequence, Some(manifest))
        }
        None => (None, 0, None),
    };

    let mut statement = conn.prepare(
        "SELECT resource.manifest_json
         FROM remi_active_profile_revisions active
         JOIN remi_catalog_resources resource
           ON resource.resource_sha256 = active.profile_revision_sha256
         WHERE resource.resource_kind = 'profile_revision' AND resource.durable = 1
         ORDER BY active.source_profile COLLATE BINARY",
    )?;
    let profiles = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .map(|row| {
            let manifest_json = row?;
            let revision = serde_json::from_str::<ProfileRevisionV1>(&manifest_json)?;
            revision.validate()?;
            Ok::<_, anyhow::Error>(revision)
        })
        .collect::<Result<Vec<_>>>()?;
    let canonical_map = load_canonical_map_snapshot(&conn)?;
    Ok(UniverseInputs {
        base_manifest_sha256,
        base_sequence,
        active_manifest,
        profiles,
        canonical_map,
    })
}

fn same_authority(
    active: &RemiUniverseManifestV1,
    profiles: &[ProfileRevisionV1],
    canonical_map_sha256: &str,
) -> bool {
    active.canonical_map.sha256 == canonical_map_sha256
        && active.profiles.len() == profiles.len()
        && active
            .profiles
            .iter()
            .zip(profiles)
            .all(|(left, right)| left.revision == *right)
}

fn active_bundle_is_fresh(
    catalog_dir: &Path,
    active: &RemiUniverseManifestV1,
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

fn load_or_create_root(
    catalog_dir: &Path,
    keys_root: &Path,
    inputs: &UniverseInputs,
) -> Result<Signed<conary_core::trust::RootMetadata>> {
    let root = if let Some(active) = &inputs.base_manifest_sha256 {
        let bytes = fs::read(universe_bundle_path(catalog_dir, active).join(UNIVERSE_ROOT_FILE))
            .with_context(|| format!("read active universe root for {active}"))?;
        serde_json::from_slice(&bytes).context("parse active universe root")?
    } else {
        let root_key = load_universe_role_key(keys_root, UniverseSigningRole::Root)?;
        let targets_key = load_universe_role_key(keys_root, UniverseSigningRole::Targets)?;
        let snapshot_key = load_universe_role_key(keys_root, UniverseSigningRole::Snapshot)?;
        let timestamp_key = load_universe_role_key(keys_root, UniverseSigningRole::Timestamp)?;
        create_initial_root(&root_key, &targets_key, &snapshot_key, &timestamp_key, 3650)
            .map_err(anyhow::Error::from)?
    };
    let (root_keys, root_threshold) = extract_role_keys(&root.signed, Role::Root)?;
    verify_root(&root, &root_keys, root_threshold)?;
    verify_not_expired(Role::Root, &root.signed.expires)?;
    Ok(root)
}

fn build_candidate(
    base_sequence: u64,
    profiles: Vec<ProfileRevisionV1>,
    canonical_map_bytes: Vec<u8>,
    root: Signed<conary_core::trust::RootMetadata>,
    keys_root: &Path,
) -> Result<SignedUniverseCandidate> {
    let sequence = base_sequence
        .checked_add(1)
        .context("Remi universe sequence overflow")?;
    let now = Utc::now();
    let root_bytes = canonical_json(&root, "universe root")?;
    let metadata_root_sha256 = conary_core::hash::sha256(&root_bytes);
    let canonical_map = conary_core::canonical::parse_snapshot(&canonical_map_bytes)?;
    let canonical_map_sha256 = conary_core::hash::sha256(&canonical_map_bytes);
    let profile_descriptors = profiles
        .into_iter()
        .enumerate()
        .map(|(index, revision)| {
            let ordinal = u32::try_from(index).context("too many universe profiles")?;
            let profile_revision_sha256 = revision.manifest_sha256()?;
            Ok(RemiUniverseProfileV1 {
                ordinal,
                profile_revision_sha256,
                catalog: RemiUniverseCatalogObjectV1 {
                    schema_version: CATALOG_CONTENT_SCHEMA_V1,
                    sha256: revision.catalog.sha256.clone(),
                    size: revision.catalog.size,
                    logical_digest_sha256: revision.logical_digest_sha256.clone(),
                },
                revision,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let manifest = RemiUniverseManifestV1 {
        schema_version: REMI_UNIVERSE_SCHEMA_V1,
        sequence,
        metadata_root_sha256,
        generated_at: now,
        expires_at: now + Duration::days(7),
        profiles: profile_descriptors,
        canonical_map: RemiUniverseCanonicalMapObjectV1 {
            schema_version: canonical_map.schema_version,
            sha256: canonical_map_sha256,
            size: u64::try_from(canonical_map_bytes.len())
                .context("canonical-map object size exceeds u64")?,
            revision: canonical_map.revision,
            entry_count: u64::try_from(canonical_map.entries.len())
                .context("canonical-map entry count exceeds u64")?,
        },
    };
    manifest.validate().map_err(anyhow::Error::from)?;
    let manifest_sha256 = manifest.manifest_sha256()?;
    let manifest_bytes = canonical_json(&manifest, "universe manifest")?;

    let mut target_descriptions = BTreeMap::new();
    insert_target(
        &mut target_descriptions,
        manifest.target_path()?,
        manifest_sha256.clone(),
        u64::try_from(manifest_bytes.len()).context("universe manifest size exceeds u64")?,
    )?;
    for profile in &manifest.profiles {
        insert_target(
            &mut target_descriptions,
            profile.catalog.target_path(),
            profile.catalog.sha256.clone(),
            profile.catalog.size,
        )?;
    }
    insert_target(
        &mut target_descriptions,
        manifest.canonical_map.target_path(),
        manifest.canonical_map.sha256.clone(),
        manifest.canonical_map.size,
    )?;

    let targets_key = load_universe_role_key(keys_root, UniverseSigningRole::Targets)?;
    let targets = TargetsMetadata {
        type_field: "targets".to_string(),
        spec_version: TUF_SPEC_VERSION.to_string(),
        version: sequence,
        expires: now + Duration::days(30),
        targets: target_descriptions,
    };
    let targets = Signed {
        signatures: vec![sign_tuf_metadata(&targets_key, &targets)?],
        signed: targets,
    };
    let targets_bytes = canonical_json(&targets, "universe targets")?;

    let snapshot_key = load_universe_role_key(keys_root, UniverseSigningRole::Snapshot)?;
    let snapshot = SnapshotMetadata {
        type_field: "snapshot".to_string(),
        spec_version: TUF_SPEC_VERSION.to_string(),
        version: sequence,
        expires: now + Duration::days(7),
        meta: BTreeMap::from([
            (
                "root.json".to_string(),
                metadata_reference(root.signed.version, &root_bytes)?,
            ),
            (
                "targets.json".to_string(),
                metadata_reference(sequence, &targets_bytes)?,
            ),
        ]),
    };
    let snapshot = Signed {
        signatures: vec![sign_tuf_metadata(&snapshot_key, &snapshot)?],
        signed: snapshot,
    };
    let snapshot_bytes = canonical_json(&snapshot, "universe snapshot")?;

    let timestamp_key = load_universe_role_key(keys_root, UniverseSigningRole::Timestamp)?;
    let timestamp = TimestampMetadata {
        type_field: "timestamp".to_string(),
        spec_version: TUF_SPEC_VERSION.to_string(),
        version: sequence,
        expires: now + Duration::days(1),
        meta: BTreeMap::from([(
            "snapshot.json".to_string(),
            metadata_reference(sequence, &snapshot_bytes)?,
        )]),
    };
    let timestamp = Signed {
        signatures: vec![sign_tuf_metadata(&timestamp_key, &timestamp)?],
        signed: timestamp,
    };
    let timestamp_bytes = canonical_json(&timestamp, "universe timestamp")?;

    let candidate = SignedUniverseCandidate {
        manifest,
        manifest_sha256,
        manifest_bytes,
        canonical_map_bytes,
        root,
        root_bytes,
        targets,
        targets_bytes,
        snapshot,
        snapshot_bytes,
        timestamp,
        timestamp_bytes,
    };
    verify_candidate(&candidate)?;
    Ok(candidate)
}

fn insert_target(
    targets: &mut BTreeMap<String, TargetDescription>,
    path: String,
    sha256: String,
    size: u64,
) -> Result<()> {
    let prior = targets.insert(
        path.clone(),
        TargetDescription {
            length: size,
            hashes: BTreeMap::from([("sha256".to_string(), sha256)]),
        },
    );
    if prior.is_some() {
        bail!("universe repeats target path {path}");
    }
    Ok(())
}

fn metadata_reference(version: u64, bytes: &[u8]) -> Result<MetaFile> {
    Ok(MetaFile {
        version,
        length: Some(u64::try_from(bytes.len()).context("TUF metadata size exceeds u64")?),
        hashes: Some(BTreeMap::from([(
            "sha256".to_string(),
            conary_core::hash::sha256(bytes),
        )])),
    })
}

fn verify_candidate(candidate: &SignedUniverseCandidate) -> Result<()> {
    if conary_core::hash::sha256(&candidate.root_bytes) != candidate.manifest.metadata_root_sha256 {
        bail!("universe manifest metadata-root digest disagrees with root bytes");
    }
    let (root_keys, root_threshold) = extract_role_keys(&candidate.root.signed, Role::Root)?;
    verify_root(&candidate.root, &root_keys, root_threshold)?;
    for (role, expires) in [
        (Role::Root, candidate.root.signed.expires),
        (Role::Targets, candidate.targets.signed.expires),
        (Role::Snapshot, candidate.snapshot.signed.expires),
        (Role::Timestamp, candidate.timestamp.signed.expires),
    ] {
        verify_not_expired(role, &expires)?;
    }
    let (targets_keys, targets_threshold) =
        extract_role_keys(&candidate.root.signed, Role::Targets)?;
    verify_signatures(
        &candidate.targets,
        Role::Targets,
        &targets_keys,
        targets_threshold,
    )?;
    let (snapshot_keys, snapshot_threshold) =
        extract_role_keys(&candidate.root.signed, Role::Snapshot)?;
    verify_signatures(
        &candidate.snapshot,
        Role::Snapshot,
        &snapshot_keys,
        snapshot_threshold,
    )?;
    let (timestamp_keys, timestamp_threshold) =
        extract_role_keys(&candidate.root.signed, Role::Timestamp)?;
    verify_signatures(
        &candidate.timestamp,
        Role::Timestamp,
        &timestamp_keys,
        timestamp_threshold,
    )?;
    let targets_ref = candidate
        .snapshot
        .signed
        .meta
        .get("targets.json")
        .context("universe snapshot omits targets.json")?;
    verify_metadata_hash(targets_ref, &candidate.targets_bytes, true)?;
    let snapshot_ref = candidate
        .timestamp
        .signed
        .meta
        .get("snapshot.json")
        .context("universe timestamp omits snapshot.json")?;
    verify_metadata_hash(snapshot_ref, &candidate.snapshot_bytes, true)?;
    verify_static_snapshot_consistency(
        &candidate.snapshot.signed,
        candidate.root.signed.version,
        candidate.targets.signed.version,
    )?;
    verify_remi_universe_manifest_target(
        &candidate.manifest_bytes,
        &candidate.targets.signed.targets,
    )?;
    conary_core::canonical::parse_snapshot(&candidate.canonical_map_bytes)?;
    Ok(())
}

fn publish_candidate_files(
    candidate_root: &Path,
    catalog_dir: &Path,
    candidate: &SignedUniverseCandidate,
) -> Result<PathBuf> {
    require_real_directory(candidate_root, "universe candidate root")?;
    require_real_directory(catalog_dir, "catalog root")?;
    if fs::metadata(candidate_root)?.dev() != fs::metadata(catalog_dir)?.dev() {
        bail!("universe candidate and catalog roots must share one filesystem");
    }
    let universes = ensure_real_subdirectory(catalog_dir, "universes")?;
    let destination = universes.join(&candidate.manifest_sha256);
    if destination.exists() {
        verify_published_bundle(catalog_dir, &candidate.manifest, &candidate.manifest_sha256)?;
        return Ok(destination);
    }

    let staged = tempfile::Builder::new()
        .prefix("universe-")
        .tempdir_in(candidate_root)?;
    for (name, bytes) in [
        (UNIVERSE_MANIFEST_FILE, candidate.manifest_bytes.as_slice()),
        (
            UNIVERSE_CANONICAL_MAP_FILE,
            candidate.canonical_map_bytes.as_slice(),
        ),
        (UNIVERSE_ROOT_FILE, candidate.root_bytes.as_slice()),
        (UNIVERSE_TARGETS_FILE, candidate.targets_bytes.as_slice()),
        (UNIVERSE_SNAPSHOT_FILE, candidate.snapshot_bytes.as_slice()),
        (
            UNIVERSE_TIMESTAMP_FILE,
            candidate.timestamp_bytes.as_slice(),
        ),
    ] {
        write_new_file(&staged.path().join(name), bytes)?;
    }
    File::open(staged.path())?.sync_all()?;
    let staged_path = staged.keep();
    fs::rename(&staged_path, &destination)
        .with_context(|| format!("publish signed universe {}", candidate.manifest_sha256))?;
    File::open(&universes)?.sync_all()?;
    verify_published_bundle(catalog_dir, &candidate.manifest, &candidate.manifest_sha256)?;
    Ok(destination)
}

fn verify_published_bundle(
    catalog_dir: &Path,
    expected: &RemiUniverseManifestV1,
    expected_sha256: &str,
) -> Result<()> {
    expected.validate().map_err(anyhow::Error::from)?;
    if expected.manifest_sha256()? != expected_sha256 {
        bail!("published universe path identity disagrees with its manifest");
    }
    let directory = universe_bundle_path(catalog_dir, expected_sha256);
    require_real_directory(&directory, "published universe bundle")?;
    let mut names = fs::read_dir(&directory)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    names.sort();
    let mut expected_names = UNIVERSE_FILES.map(std::ffi::OsString::from).to_vec();
    expected_names.sort();
    if names != expected_names {
        bail!("published universe bundle contains an unexpected file set");
    }
    let manifest_bytes = read_plain_file(&directory.join(UNIVERSE_MANIFEST_FILE))?;
    if manifest_bytes != canonical_json(expected, "expected universe manifest")? {
        bail!("published universe manifest bytes disagree with pointer authority");
    }
    let root_bytes = read_plain_file(&directory.join(UNIVERSE_ROOT_FILE))?;
    if conary_core::hash::sha256(&root_bytes) != expected.metadata_root_sha256 {
        bail!("published universe root digest disagrees with its manifest");
    }
    let targets: Signed<TargetsMetadata> =
        serde_json::from_slice(&read_plain_file(&directory.join(UNIVERSE_TARGETS_FILE))?)?;
    verify_remi_universe_manifest_target(&manifest_bytes, &targets.signed.targets)?;
    let canonical_bytes = read_plain_file(&directory.join(UNIVERSE_CANONICAL_MAP_FILE))?;
    if canonical_bytes.len() as u64 != expected.canonical_map.size
        || conary_core::hash::sha256(&canonical_bytes) != expected.canonical_map.sha256
    {
        bail!("published universe canonical-map object disagrees with its manifest");
    }
    let canonical = conary_core::canonical::parse_snapshot(&canonical_bytes)?;
    if canonical.revision != expected.canonical_map.revision
        || canonical.entries.len() as u64 != expected.canonical_map.entry_count
    {
        bail!("published universe canonical-map facts disagree with its manifest");
    }
    for profile in &expected.profiles {
        let bundle = catalog_dir
            .join("profiles")
            .join(&profile.revision.profile)
            .join(&profile.profile_revision_sha256);
        verify_profile_catalog_bundle(bundle, &profile.revision)?;
    }
    Ok(())
}

fn activate_candidate(
    db_path: &Path,
    inputs: &UniverseInputs,
    candidate: &SignedUniverseCandidate,
    _bundle: &Path,
) -> Result<()> {
    let conn = conary_core::db::open_fast(db_path)?;
    let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
    let current = tx
        .query_row(
            "SELECT manifest_sha256, sequence FROM remi_active_universe_revision
             WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    let expected_current = inputs.base_manifest_sha256.as_ref().map(|sha256| {
        (
            sha256.as_str(),
            i64::try_from(inputs.base_sequence).expect("stored sequence already fit i64"),
        )
    });
    if current
        .as_ref()
        .map(|(sha256, sequence)| (sha256.as_str(), *sequence))
        != expected_current
    {
        bail!("active Remi universe changed while a replacement was being built");
    }
    require_profile_inputs_unchanged(&tx, &candidate.manifest)?;
    let current_canonical = load_canonical_map_snapshot(&tx)?;
    let current_canonical_bytes = canonical_bytes(&current_canonical)?;
    if conary_core::hash::sha256(&current_canonical_bytes)
        != candidate.manifest.canonical_map.sha256
    {
        bail!("canonical map changed while a Remi universe was being built");
    }

    let sequence = i64::try_from(candidate.manifest.sequence)
        .context("universe sequence exceeds SQLite integer range")?;
    tx.execute(
        "INSERT INTO remi_universe_revisions (
             manifest_sha256, sequence, metadata_root_sha256,
             canonical_map_sha256, canonical_map_size, targets_version,
             snapshot_version, timestamp_version, manifest_json, durable, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10)",
        params![
            &candidate.manifest_sha256,
            sequence,
            &candidate.manifest.metadata_root_sha256,
            &candidate.manifest.canonical_map.sha256,
            i64::try_from(candidate.manifest.canonical_map.size)
                .context("canonical-map size exceeds SQLite integer range")?,
            i64::try_from(candidate.targets.signed.version)
                .context("targets version exceeds SQLite integer range")?,
            i64::try_from(candidate.snapshot.signed.version)
                .context("snapshot version exceeds SQLite integer range")?,
            i64::try_from(candidate.timestamp.signed.version)
                .context("timestamp version exceeds SQLite integer range")?,
            String::from_utf8(candidate.manifest_bytes.clone())?,
            candidate.manifest.generated_at.timestamp(),
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
                i64::try_from(profile.catalog.size)
                    .context("profile catalog size exceeds SQLite integer range")?,
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
        params![&candidate.manifest_sha256, sequence, Utc::now().timestamp(),],
    )?;
    tx.commit()?;
    Ok(())
}

fn require_profile_inputs_unchanged(
    tx: &Transaction<'_>,
    manifest: &RemiUniverseManifestV1,
) -> Result<()> {
    let mut statement = tx.prepare(
        "SELECT source_profile, profile_revision_sha256
         FROM remi_active_profile_revisions
         ORDER BY source_profile COLLATE BINARY",
    )?;
    let active = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let expected = manifest
        .profiles
        .iter()
        .map(|profile| {
            (
                profile.revision.profile.clone(),
                profile.profile_revision_sha256.clone(),
            )
        })
        .collect::<Vec<_>>();
    if active != expected {
        bail!("active profile set changed while a Remi universe was being built");
    }
    Ok(())
}

fn canonical_bytes(snapshot: &CanonicalMapSnapshot) -> Result<Vec<u8>> {
    validate_canonical_map_snapshot(snapshot).map_err(anyhow::Error::from)?;
    canonical_json(snapshot, "canonical map")
}

fn canonical_json(value: &impl serde::Serialize, label: &str) -> Result<Vec<u8>> {
    conary_core::json::canonical_json(value)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("serialize {label}"))
}

pub(crate) fn universe_bundle_path(catalog_dir: &Path, manifest_sha256: &str) -> PathBuf {
    catalog_dir.join("universes").join(manifest_sha256)
}

fn ensure_real_subdirectory(parent: &Path, name: &str) -> Result<PathBuf> {
    require_real_directory(parent, "directory parent")?;
    let path = parent.join(name);
    match fs::create_dir(&path) {
        Ok(()) => File::open(parent)?.sync_all()?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    require_real_directory(&path, name)?;
    Ok(path)
}

fn require_real_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        bail!("{label} {} must be a real directory", path.display());
    }
    Ok(())
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn read_plain_file(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        bail!("universe file {} must be a plain file", path.display());
    }
    if metadata.permissions().mode() & 0o022 != 0 {
        bail!(
            "universe file {} must not be group/world writable",
            path.display()
        );
    }
    Ok(fs::read(path)?)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::DirBuilderExt;

    use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};
    use crate::server::signing_authority::ensure_universe_authority;

    use super::*;

    struct PublicationFixture {
        catalogs: ActiveCatalogFixture,
        candidate_dir: PathBuf,
        keys_root: PathBuf,
        database_writer: DatabaseWriter,
    }

    impl PublicationFixture {
        fn new() -> Self {
            let catalogs = ActiveCatalogFixture::new();
            let root = catalogs
                .catalog_dir()
                .parent()
                .expect("fixture root")
                .to_path_buf();
            let candidate_dir = root.join("universe-candidates");
            fs::create_dir(&candidate_dir).expect("create universe candidate root");
            let keys_root = root.join("repository-keys");
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&keys_root)
                .expect("create universe key root");
            ensure_universe_authority(&keys_root).expect("provision universe authority");
            Self {
                catalogs,
                candidate_dir,
                keys_root,
                database_writer: DatabaseWriter::default(),
            }
        }

        fn publish(&self) -> Result<UniversePublicationOutcome> {
            publish_current_universe(
                self.catalogs.db_path(),
                self.catalogs.catalog_dir(),
                &self.candidate_dir,
                Some(&self.keys_root),
                &self.database_writer,
            )
        }
    }

    #[test]
    fn duplicate_target_path_is_rejected() {
        let mut targets = BTreeMap::new();
        insert_target(
            &mut targets,
            "objects/sha256/a".to_string(),
            "a".repeat(64),
            1,
        )
        .unwrap();
        assert!(
            insert_target(
                &mut targets,
                "objects/sha256/a".to_string(),
                "b".repeat(64),
                1,
            )
            .is_err()
        );
    }

    #[test]
    fn publication_binds_all_profiles_and_only_advances_on_authority_change() {
        let fixture = PublicationFixture::new();
        let fedora_v1 = fixture.catalogs.activate(
            "fedora-44",
            1,
            vec![package(
                "fedora-44",
                "bash",
                "5.3",
                "1.fc44",
                Some("x86_64"),
                100,
                "fedora-bash-v1",
            )],
        );
        let ubuntu_v1 = fixture.catalogs.activate(
            "ubuntu-26.04",
            1,
            vec![package(
                "ubuntu-26.04",
                "bash",
                "5.3",
                "1ubuntu1",
                Some("amd64"),
                101,
                "ubuntu-bash-v1",
            )],
        );

        let first = fixture.publish().expect("publish initial universe");
        let UniversePublicationOutcome::Activated {
            manifest_sha256: first_sha256,
            sequence: 1,
        } = first
        else {
            panic!("initial publication did not activate sequence 1");
        };
        let conn = fixture.catalogs.connection();
        let manifest_json = conn
            .query_row(
                "SELECT manifest_json FROM remi_universe_revisions
                 WHERE manifest_sha256 = ?1",
                [&first_sha256],
                |row| row.get::<_, String>(0),
            )
            .expect("load universe manifest");
        let manifest: RemiUniverseManifestV1 =
            serde_json::from_str(&manifest_json).expect("parse universe manifest");
        assert_eq!(manifest.sequence, 1);
        assert_eq!(
            manifest
                .profiles
                .iter()
                .map(|profile| (
                    profile.revision.profile.as_str(),
                    profile.profile_revision_sha256.as_str(),
                ))
                .collect::<Vec<_>>(),
            vec![
                ("fedora-44", fedora_v1.as_str()),
                ("ubuntu-26.04", ubuntu_v1.as_str()),
            ]
        );
        assert_eq!(
            fixture.publish().expect("repeat publication"),
            UniversePublicationOutcome::Unchanged {
                manifest_sha256: first_sha256.clone(),
                sequence: 1,
            }
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM remi_universe_revisions", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            1
        );

        let fedora_v2 = fixture.catalogs.activate(
            "fedora-44",
            2,
            vec![package(
                "fedora-44",
                "bash",
                "5.3",
                "2.fc44",
                Some("x86_64"),
                102,
                "fedora-bash-v2",
            )],
        );
        let second = fixture.publish().expect("publish changed universe");
        let UniversePublicationOutcome::Activated {
            manifest_sha256: second_sha256,
            sequence: 2,
        } = second
        else {
            panic!("changed publication did not activate sequence 2");
        };
        assert_ne!(second_sha256, first_sha256);
        assert_ne!(fedora_v2, fedora_v1);
        assert_eq!(
            conn.query_row(
                "SELECT manifest_sha256 FROM remi_active_universe_revision WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            second_sha256
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM remi_universe_revisions", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            2
        );
    }

    #[test]
    fn tampered_active_bundle_fails_closed_without_advancing_pointer() {
        let fixture = PublicationFixture::new();
        fixture.catalogs.activate(
            "fedora-44",
            1,
            vec![package(
                "fedora-44",
                "bash",
                "5.3",
                "1.fc44",
                Some("x86_64"),
                100,
                "fedora-bash",
            )],
        );
        let first = fixture.publish().expect("publish initial universe");
        let UniversePublicationOutcome::Activated {
            manifest_sha256,
            sequence: 1,
        } = first
        else {
            panic!("initial publication did not activate sequence 1");
        };
        let canonical_path = universe_bundle_path(fixture.catalogs.catalog_dir(), &manifest_sha256)
            .join(UNIVERSE_CANONICAL_MAP_FILE);
        fs::write(&canonical_path, b"{}\n").expect("tamper canonical map");

        let error = fixture.publish().expect_err("tampered bundle must fail");
        assert!(
            error.to_string().contains("canonical-map object disagrees"),
            "{error:#}"
        );
        let conn = fixture.catalogs.connection();
        assert_eq!(
            conn.query_row(
                "SELECT manifest_sha256, sequence FROM remi_active_universe_revision
                 WHERE singleton = 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap(),
            (manifest_sha256, 1)
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM remi_universe_revisions", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            1
        );
    }

    #[test]
    fn unchanged_authority_renews_before_timestamp_expiry() {
        let now = "2026-08-22T12:00:00Z".parse().unwrap();
        assert!(!requires_renewal(
            now,
            now + Duration::days(7),
            now + Duration::hours(7),
        ));
        assert!(requires_renewal(
            now,
            now + Duration::days(7),
            now + Duration::hours(6),
        ));
        assert!(requires_renewal(
            now,
            now + Duration::hours(5),
            now + Duration::days(1),
        ));
    }
}
