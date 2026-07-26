// conary-core/src/repository/static_repo/publish.rs

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Duration, Utc};
use fs2::FileExt;

use crate::ccs::signing::SigningKeyPair;
use crate::hash;
use crate::repository::static_repo::package_staging::{PendingPackageWrites, stage_packages};
use crate::repository::static_repo::publish_context::{
    ArtifactGateContext, DestinationState, PendingKeyPromotions, PendingKeyRecovery,
    PreparedStaticPublishContext, StaticPublishForm, StaticPublishPrepareOptions,
    build_package_keys_file, create_private_dir_all, ensure_key_pair, read_destination_state,
    read_optional, recover_pending_key_promotions, verify_destination_matches_operator_keys,
};
use crate::repository::static_repo::publish_gate::{
    format_publish_gate_failures, verify_static_artifact_publish_eligibility,
};
use crate::repository::static_repo::{
    PackageKeysFile, RepoIdentity, RepoIdentityRepo, RepoIdentityTrust, RepoLocation, StaticIndex,
    StaticPackageEntry, validate_repo_relative_path,
};
use crate::trust::ceremony::{create_initial_root, rotate_key, rotate_publish_key};
use crate::trust::generate::{generate_snapshot, generate_targets, generate_timestamp};
use crate::trust::keys::{sign_tuf_metadata, signing_keypair_to_tuf_key};
use crate::trust::metadata::{RootMetadata, Signed, TargetsMetadata};

const ROOT_EXPIRES_DAYS: i64 = 365;
const TARGETS_EXPIRES_DAYS: i64 = 90;
const SNAPSHOT_EXPIRES_DAYS: i64 = 90;
const TIMESTAMP_EXPIRES_HOURS: i64 = 720;
const ROOT_IDENTITY_WARNING: &str = "the root key **is** the repo's identity — store `root.private` offline if possible, and back up the whole directory; losing it means clients must manually re-trust (§7.4).";
const ATOMIC_WRITE_TEMP_ATTEMPTS: usize = 1024;
static ATOMIC_WRITE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct StaticPublishOptions {
    pub repo_name: String,
    pub repo_description: Option<String>,
    pub destination: RepoLocation,
    pub key_dir: PathBuf,
    pub state_file: PathBuf,
    pub package_paths: Vec<PathBuf>,
    pub refresh: bool,
    pub rotate_publish_key: bool,
    pub rotate_root_key: bool,
    pub artifact_gate_context: Option<ArtifactGateContext>,
}

#[derive(Debug)]
pub struct StaticPublishOutcome {
    pub root_version: u64,
    pub targets_version: u64,
    pub snapshot_version: u64,
    pub timestamp_version: u64,
    pub root_key_ids: Vec<String>,
    pub publish_key_id: String,
    pub package_count: usize,
    pub preview_warning: String,
}

#[cfg(test)]
#[derive(Clone, Copy, Default)]
struct ForcedRefreshForTest {
    root: bool,
    targets: bool,
    snapshot: bool,
}

#[derive(Clone, Copy, Default)]
struct ForcedRefresh {
    root: bool,
    targets: bool,
    snapshot: bool,
}

pub fn publish_static_repo(options: StaticPublishOptions) -> Result<StaticPublishOutcome> {
    publish_static_repo_inner(options, ForcedRefresh::default())
}

#[cfg(test)]
fn publish_static_repo_with_forced_refresh_for_test(
    options: StaticPublishOptions,
    forced: ForcedRefreshForTest,
) -> Result<StaticPublishOutcome> {
    publish_static_repo_inner(
        options,
        ForcedRefresh {
            root: forced.root,
            targets: forced.targets,
            snapshot: forced.snapshot,
        },
    )
}

fn publish_static_repo_inner(
    options: StaticPublishOptions,
    forced_refresh: ForcedRefresh,
) -> Result<StaticPublishOutcome> {
    let publish_form = if options.artifact_gate_context.is_some() {
        StaticPublishForm::Artifact
    } else {
        StaticPublishForm::Project
    };
    let context = StaticPublishPrepareOptions {
        destination: options.destination.clone(),
        key_dir: Some(options.key_dir.clone()),
        publish_form,
    }
    .prepare()?;
    commit_static_publish(options, context, forced_refresh)
}

fn commit_static_publish(
    options: StaticPublishOptions,
    context: PreparedStaticPublishContext,
    forced_refresh: ForcedRefresh,
) -> Result<StaticPublishOutcome> {
    let PreparedStaticPublishContext {
        destination,
        key_dir,
        active_publish_key,
        accepted_signers,
        ..
    } = context;
    let RepoLocation::File { root: repo_root } = &destination else {
        bail!("M1a static publisher supports local filesystem destinations only");
    };

    validate_repo_name_for_identity(&options.repo_name)?;
    fs::create_dir_all(repo_root)
        .with_context(|| format!("create static repo destination {}", repo_root.display()))?;

    let _publish_lock = PublishLock::acquire(repo_root)?;
    let mut root_key = ensure_key_pair(&key_dir, "root")?;
    let mut publish_key = active_publish_key;
    let mut pending_key_promotions = PendingKeyPromotions::default();
    let destination = read_destination_state(repo_root)?;
    check_watermark(&destination, &options)?;

    let mut old_publish_public_key = None;
    let mut root_metadata = if destination.initial {
        create_initial_root(
            &root_key,
            &publish_key,
            &publish_key,
            &publish_key,
            ROOT_EXPIRES_DAYS,
        )
        .map_err(anyhow::Error::from)?
    } else {
        destination
            .root
            .clone()
            .expect("verified destination has root")
    };
    let mut recovered_pending_keys = PendingKeyRecovery::default();
    if !destination.initial {
        recovered_pending_keys = recover_pending_key_promotions(
            &root_metadata,
            &key_dir,
            &mut root_key,
            &mut publish_key,
            &mut pending_key_promotions,
        )?;
        verify_destination_matches_operator_keys(&root_metadata, &root_key, &publish_key)?;
    }

    let mut root_changed = destination.initial;
    let mut identity_changed = destination.initial;
    let should_rotate_publish_key = options.rotate_publish_key && !recovered_pending_keys.publish;
    let should_rotate_root_key = options.rotate_root_key && !recovered_pending_keys.root;
    if should_rotate_publish_key {
        old_publish_public_key = Some(publish_key.public_key_base64());
        let new_publish_key = pending_key_promotions.stage_or_load(&key_dir, "publish")?;
        root_metadata = rotate_publish_key(
            &root_metadata,
            &publish_key,
            &new_publish_key,
            &root_key,
            ROOT_EXPIRES_DAYS,
        )
        .map_err(anyhow::Error::from)?;
        publish_key = new_publish_key;
        root_changed = true;
    }
    if should_rotate_root_key {
        let new_root_key = pending_key_promotions.stage_or_load(&key_dir, "root")?;
        root_metadata = rotate_key(
            &root_metadata,
            "root",
            &root_key,
            &new_root_key,
            &root_key,
            ROOT_EXPIRES_DAYS,
        )
        .map_err(anyhow::Error::from)?;
        root_changed = true;
        identity_changed = true;
    } else if should_refresh_root(&options, &destination, forced_refresh) {
        root_metadata = refresh_root(&root_metadata, &root_key)?;
        root_changed = true;
    }

    let old_index = destination
        .index_bytes
        .as_deref()
        .map(|bytes| {
            let text = std::str::from_utf8(bytes)
                .context("verified destination index.json is not UTF-8")?;
            StaticIndex::parse(text).context("verified destination index.json is invalid")
        })
        .transpose()?;
    let old_package_keys = destination
        .package_keys_bytes
        .as_deref()
        .map(|bytes| {
            let text = std::str::from_utf8(bytes)
                .context("verified destination package-keys.json is not UTF-8")?;
            PackageKeysFile::parse(text)
                .context("verified destination package-keys.json is invalid")
        })
        .transpose()?;

    if let Some(gate_context) = options.artifact_gate_context.as_ref() {
        for package_path in &options.package_paths {
            let report = verify_static_artifact_publish_eligibility(
                package_path,
                &gate_context.accepted_signers,
                &gate_context.publish_policy_digest,
            )?;
            if !report.is_passed() {
                bail!("{}", format_publish_gate_failures(&report));
            }
        }
    }

    let mut pending_package_writes = stage_packages(
        repo_root,
        &options.package_paths,
        &accepted_signers,
        &publish_key,
    )?;
    let mut package_entries = old_index
        .as_ref()
        .map(|index| index.packages.clone())
        .unwrap_or_default();
    for staged in pending_package_writes.package_entries() {
        if let Some(existing) = package_entries
            .iter()
            .find(|entry| entry.path == staged.path)
        {
            if existing.sha256 != staged.sha256 || existing.size != staged.size {
                bail!(
                    "immutable package index entry {} disagrees with verified artifact bytes",
                    staged.path
                );
            }
            continue;
        }
        package_entries.push(staged.clone());
    }
    package_entries.sort_by(|left, right| left.path.cmp(&right.path));

    let targets_bump = destination.initial
        || !options.package_paths.is_empty()
        || should_rotate_publish_key
        || should_refresh_targets(&options, &destination, forced_refresh);
    let snapshot_bump = destination.initial
        || root_changed
        || targets_bump
        || should_refresh_snapshot(&options, &destination, forced_refresh);

    let current_targets_version = destination
        .targets
        .as_ref()
        .map(|metadata| metadata.signed.version)
        .unwrap_or(0);
    let current_snapshot_version = destination
        .snapshot
        .as_ref()
        .map(|metadata| metadata.signed.version)
        .unwrap_or(0);
    let current_timestamp_version = destination
        .timestamp
        .as_ref()
        .map(|metadata| metadata.signed.version)
        .unwrap_or(0);

    let targets_version = if targets_bump {
        current_targets_version + 1
    } else {
        current_targets_version
    };
    let snapshot_version = if snapshot_bump {
        current_snapshot_version + 1
    } else {
        current_snapshot_version
    };
    let timestamp_version = current_timestamp_version + 1;

    let (_package_keys_file, package_keys_bytes, index, index_bytes) = if targets_bump {
        let package_keys_file = build_package_keys_file(
            old_package_keys.as_ref(),
            &publish_key,
            old_publish_public_key,
        )?;
        let package_keys_bytes = serde_json::to_vec_pretty(&package_keys_file)?;
        let index = build_index(&options.repo_name, targets_version, package_entries);
        let index_bytes = serde_json::to_vec_pretty(&index)?;
        StaticIndex::parse(std::str::from_utf8(&index_bytes)?)?
            .validate_with_keys(&package_keys_file)?;
        (package_keys_file, package_keys_bytes, index, index_bytes)
    } else {
        let package_keys_bytes = destination
            .package_keys_bytes
            .clone()
            .context("cannot refresh timestamp without keys/package-keys.json")?;
        let package_keys_file = PackageKeysFile::parse(std::str::from_utf8(&package_keys_bytes)?)?;
        let index_bytes = destination
            .index_bytes
            .clone()
            .context("cannot refresh timestamp without index.json")?;
        let index = StaticIndex::parse(std::str::from_utf8(&index_bytes)?)?;
        index.validate_with_keys(&package_keys_file)?;
        (package_keys_file, package_keys_bytes, index, index_bytes)
    };

    let targets_metadata = if targets_bump {
        let target_entries = build_target_entries(
            repo_root,
            &index.packages,
            &pending_package_writes,
            &index_bytes,
            &package_keys_bytes,
        )?;
        generate_targets(
            &target_entries,
            &publish_key,
            targets_version,
            TARGETS_EXPIRES_DAYS,
        )
        .map_err(anyhow::Error::from)?
    } else {
        destination
            .targets
            .clone()
            .context("cannot refresh timestamp without existing targets metadata")?
    };
    let targets_bytes = serde_json::to_vec(&targets_metadata)?;

    ensure_index_targets_invariants(&index, &targets_metadata, &index_bytes, &package_keys_bytes)?;

    let snapshot_metadata = if snapshot_bump {
        generate_snapshot(
            root_metadata.signed.version,
            &targets_metadata,
            &publish_key,
            snapshot_version,
            SNAPSHOT_EXPIRES_DAYS,
        )
        .map_err(anyhow::Error::from)?
    } else {
        destination
            .snapshot
            .clone()
            .context("cannot refresh timestamp without existing snapshot metadata")?
    };
    let snapshot_bytes = serde_json::to_vec(&snapshot_metadata)?;
    let timestamp_metadata = generate_timestamp(
        &snapshot_metadata,
        &publish_key,
        timestamp_version,
        TIMESTAMP_EXPIRES_HOURS,
    )
    .map_err(anyhow::Error::from)?;
    let timestamp_bytes = serde_json::to_vec(&timestamp_metadata)?;

    let identity = build_identity(&options, &root_metadata)?;
    let identity_bytes = toml::to_string_pretty(&identity)?.into_bytes();
    RepoIdentity::parse(std::str::from_utf8(&identity_bytes)?)?;
    let root_bytes = serde_json::to_vec(&root_metadata)?;

    write_step_a(StepAWrite {
        repo_root,
        destination: &destination,
        package_keys_bytes: &package_keys_bytes,
        root_changed,
        root_metadata: &root_metadata,
        root_bytes: &root_bytes,
        identity_changed,
        identity_bytes: &identity_bytes,
    })?;
    if targets_bump {
        conditional_write(
            repo_root,
            "index.json",
            &index_bytes,
            destination.index_bytes.as_deref(),
        )?;
        conditional_write(
            repo_root,
            "metadata/targets.json",
            &targets_bytes,
            destination.targets_bytes.as_deref(),
        )?;
    }
    if snapshot_bump {
        conditional_write(
            repo_root,
            "metadata/snapshot.json",
            &snapshot_bytes,
            destination.snapshot_bytes.as_deref(),
        )?;
    }
    pending_key_promotions.promote(&key_dir)?;
    pending_package_writes.promote()?;
    ensure_timestamp_unchanged(repo_root, &destination)?;
    conditional_write(
        repo_root,
        "metadata/timestamp.json",
        &timestamp_bytes,
        destination.timestamp_bytes.as_deref(),
    )?;
    pending_package_writes.commit();

    let watermark = PublishWatermark {
        root_version: root_metadata.signed.version,
        targets_version,
        snapshot_version,
        timestamp_version,
    };
    write_watermark(&options.state_file, &watermark)?;

    let (publish_key_id, _) =
        signing_keypair_to_tuf_key(&publish_key).map_err(anyhow::Error::from)?;
    let warning = if destination.initial || identity_changed {
        ROOT_IDENTITY_WARNING.to_string()
    } else {
        String::new()
    };

    Ok(StaticPublishOutcome {
        root_version: root_metadata.signed.version,
        targets_version,
        snapshot_version,
        timestamp_version,
        root_key_ids: root_metadata.signed.roles["root"].keyids.clone(),
        publish_key_id,
        package_count: index.packages.len(),
        preview_warning: warning,
    })
}

#[derive(Debug)]
struct PublishLock {
    _file: File,
}

impl PublishLock {
    fn acquire(repo_root: &Path) -> Result<Self> {
        let file = open_publish_lock_file(repo_root)?;
        file.lock_exclusive()
            .with_context(|| format!("lock static repo publisher for {}", repo_root.display()))?;
        Ok(Self { _file: file })
    }

    #[cfg(test)]
    fn try_acquire(repo_root: &Path) -> Result<Self> {
        let file = open_publish_lock_file(repo_root)?;
        file.try_lock_exclusive().map_err(|error| {
            anyhow!(
                "another static repo publish is already running for {}: {}",
                repo_root.display(),
                error
            )
        })?;
        Ok(Self { _file: file })
    }
}

fn open_publish_lock_file(repo_root: &Path) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(repo_root.join(".conary-publish.lock"))
        .with_context(|| format!("open static repo publish lock in {}", repo_root.display()))
}

#[cfg(test)]
fn try_acquire_publish_lock_for_test(repo_root: &Path) -> Result<PublishLock> {
    PublishLock::try_acquire(repo_root)
}

#[derive(serde::Deserialize, serde::Serialize)]
struct PublishWatermark {
    root_version: u64,
    targets_version: u64,
    snapshot_version: u64,
    timestamp_version: u64,
}

fn check_watermark(destination: &DestinationState, options: &StaticPublishOptions) -> Result<()> {
    let Some(bytes) = read_state_file(&options.state_file)? else {
        return Ok(());
    };
    let watermark: PublishWatermark = toml::from_str(std::str::from_utf8(&bytes)?)
        .with_context(|| format!("parse publish watermark {}", options.state_file.display()))?;
    let destination_versions = PublishWatermark {
        root_version: destination
            .root
            .as_ref()
            .map(|metadata| metadata.signed.version)
            .unwrap_or(0),
        targets_version: destination
            .targets
            .as_ref()
            .map(|metadata| metadata.signed.version)
            .unwrap_or(0),
        snapshot_version: destination
            .snapshot
            .as_ref()
            .map(|metadata| metadata.signed.version)
            .unwrap_or(0),
        timestamp_version: destination
            .timestamp
            .as_ref()
            .map(|metadata| metadata.signed.version)
            .unwrap_or(0),
    };

    let regressed = destination_versions.root_version < watermark.root_version
        || destination_versions.targets_version < watermark.targets_version
        || destination_versions.snapshot_version < watermark.snapshot_version
        || destination_versions.timestamp_version < watermark.timestamp_version;
    if regressed {
        bail!(
            "destination versions root={} targets={} snapshot={} timestamp={} are below local watermark root={} targets={} snapshot={} timestamp={}",
            destination_versions.root_version,
            destination_versions.targets_version,
            destination_versions.snapshot_version,
            destination_versions.timestamp_version,
            watermark.root_version,
            watermark.targets_version,
            watermark.snapshot_version,
            watermark.timestamp_version
        );
    }

    Ok(())
}

#[cfg(test)]
#[path = "publish/tests.rs"]
mod tests;

fn read_state_file(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

fn write_watermark(path: &Path, watermark: &PublishWatermark) -> Result<()> {
    let bytes = toml::to_string_pretty(watermark)?.into_bytes();
    write_file_atomic(path, &bytes)
}

fn should_refresh_root(
    options: &StaticPublishOptions,
    destination: &DestinationState,
    forced: ForcedRefresh,
) -> bool {
    options.refresh
        && (forced.root
            || destination.root.as_ref().is_some_and(|metadata| {
                is_near_expiry(metadata.signed.expires, ROOT_EXPIRES_DAYS * 24)
            }))
}

fn should_refresh_targets(
    options: &StaticPublishOptions,
    destination: &DestinationState,
    forced: ForcedRefresh,
) -> bool {
    options.refresh
        && (forced.targets
            || destination.targets.as_ref().is_some_and(|metadata| {
                is_near_expiry(metadata.signed.expires, TARGETS_EXPIRES_DAYS * 24)
            }))
}

fn should_refresh_snapshot(
    options: &StaticPublishOptions,
    destination: &DestinationState,
    forced: ForcedRefresh,
) -> bool {
    options.refresh
        && (forced.snapshot
            || destination.snapshot.as_ref().is_some_and(|metadata| {
                is_near_expiry(metadata.signed.expires, SNAPSHOT_EXPIRES_DAYS * 24)
            }))
}

fn is_near_expiry(expires: chrono::DateTime<Utc>, lifetime_hours: i64) -> bool {
    expires - Utc::now() <= Duration::hours(lifetime_hours / 4)
}

fn refresh_root(
    current_root: &Signed<RootMetadata>,
    root_key: &SigningKeyPair,
) -> Result<Signed<RootMetadata>> {
    let mut root = current_root.signed.clone();
    root.version += 1;
    root.expires = Utc::now() + Duration::days(ROOT_EXPIRES_DAYS);
    let sig = sign_tuf_metadata(root_key, &root).map_err(anyhow::Error::from)?;
    Ok(Signed {
        signed: root,
        signatures: vec![sig],
    })
}

fn build_index(
    repo_name: &str,
    targets_version: u64,
    package_entries: Vec<StaticPackageEntry>,
) -> StaticIndex {
    StaticIndex {
        schema: 1,
        name: repo_name.to_string(),
        index_version: targets_version,
        generated: Utc::now(),
        packages: package_entries,
    }
}

fn build_target_entries(
    repo_root: &Path,
    packages: &[StaticPackageEntry],
    pending_package_writes: &PendingPackageWrites,
    index_bytes: &[u8],
    package_keys_bytes: &[u8],
) -> Result<Vec<(String, u64, String)>> {
    let mut entries = Vec::new();
    for package in packages {
        if let Some((length, sha256)) = pending_package_writes.target_entry(&package.path) {
            entries.push((package.path.clone(), length, sha256));
        } else {
            let bytes = fs::read(repo_root.join(&package.path))
                .with_context(|| format!("read target {}", package.path))?;
            entries.push((
                package.path.clone(),
                bytes.len() as u64,
                hash::sha256(&bytes),
            ));
        }
    }
    entries.push((
        "index.json".to_string(),
        index_bytes.len() as u64,
        hash::sha256(index_bytes),
    ));
    entries.push((
        "keys/package-keys.json".to_string(),
        package_keys_bytes.len() as u64,
        hash::sha256(package_keys_bytes),
    ));
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(entries)
}

fn ensure_index_targets_invariants(
    index: &StaticIndex,
    targets: &Signed<TargetsMetadata>,
    index_bytes: &[u8],
    package_keys_bytes: &[u8],
) -> Result<()> {
    if index.index_version != targets.signed.version {
        bail!(
            "index_version {} must equal targets version {}",
            index.index_version,
            targets.signed.version
        );
    }
    ensure_target_entry(
        targets,
        "index.json",
        index_bytes.len() as u64,
        &hash::sha256(index_bytes),
    )?;
    ensure_target_entry(
        targets,
        "keys/package-keys.json",
        package_keys_bytes.len() as u64,
        &hash::sha256(package_keys_bytes),
    )?;
    for package in &index.packages {
        ensure_target_entry(targets, &package.path, package.size, &package.sha256)?;
    }
    Ok(())
}

fn ensure_target_entry(
    targets: &Signed<TargetsMetadata>,
    path: &str,
    length: u64,
    sha256: &str,
) -> Result<()> {
    let entry = targets
        .signed
        .targets
        .get(path)
        .with_context(|| format!("targets metadata missing {path}"))?;
    if entry.length != length || entry.hashes.get("sha256").map(String::as_str) != Some(sha256) {
        bail!("target entry for {path} does not match length/hash");
    }
    Ok(())
}

fn build_identity(
    options: &StaticPublishOptions,
    root: &Signed<RootMetadata>,
) -> Result<RepoIdentity> {
    let root_key_ids = root
        .signed
        .roles
        .get("root")
        .ok_or_else(|| anyhow!("root metadata missing root role"))?
        .keyids
        .clone();
    let identity = RepoIdentity {
        schema: 1,
        repo: RepoIdentityRepo {
            name: options.repo_name.clone(),
            description: options.repo_description.clone(),
        },
        trust: RepoIdentityTrust { root_key_ids },
    };
    identity.validate()?;
    Ok(identity)
}

struct StepAWrite<'a> {
    repo_root: &'a Path,
    destination: &'a DestinationState,
    package_keys_bytes: &'a [u8],
    root_changed: bool,
    root_metadata: &'a Signed<RootMetadata>,
    root_bytes: &'a [u8],
    identity_changed: bool,
    identity_bytes: &'a [u8],
}

fn write_step_a(input: StepAWrite<'_>) -> Result<()> {
    conditional_write(
        input.repo_root,
        "keys/package-keys.json",
        input.package_keys_bytes,
        input.destination.package_keys_bytes.as_deref(),
    )?;
    if input.root_changed {
        let historical_root = format!("metadata/{}.root.json", input.root_metadata.signed.version);
        write_immutable(input.repo_root, &historical_root, input.root_bytes)?;
        conditional_write(
            input.repo_root,
            "metadata/root.json",
            input.root_bytes,
            input.destination.root_bytes.as_deref(),
        )?;
    }
    if input.identity_changed {
        conditional_write(
            input.repo_root,
            "conary-repo.toml",
            input.identity_bytes,
            input.destination.identity_bytes.as_deref(),
        )?;
    }
    Ok(())
}

fn write_immutable(repo_root: &Path, relative: &str, bytes: &[u8]) -> Result<()> {
    validate_repo_relative_path(relative)?;
    let path = repo_root.join(relative);
    if let Ok(existing) = fs::read(&path) {
        if existing == bytes {
            return Ok(());
        }
        bail!("immutable static repo path {relative} already exists with different bytes");
    }
    write_file_atomic(&path, bytes)
}

fn conditional_write(
    repo_root: &Path,
    relative: &str,
    bytes: &[u8],
    expected_previous: Option<&[u8]>,
) -> Result<()> {
    validate_repo_relative_path(relative)?;
    let path = repo_root.join(relative);
    match fs::read(&path) {
        Ok(existing) => {
            if expected_previous != Some(existing.as_slice()) {
                bail!("static repo path {relative} changed during publish");
            }
            if existing == bytes {
                return Ok(());
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            if expected_previous.is_some() {
                bail!("static repo path {relative} disappeared during publish");
            }
        }
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    }
    write_file_atomic(&path, bytes)
}

fn ensure_timestamp_unchanged(repo_root: &Path, destination: &DestinationState) -> Result<()> {
    let Some(start_bytes) = destination.timestamp_bytes.as_deref() else {
        return Ok(());
    };
    let current = read_optional(repo_root, "metadata/timestamp.json")?
        .context("metadata/timestamp.json disappeared during publish")?;
    if current != start_bytes {
        bail!("metadata/timestamp.json changed during publish; concurrent writer detected");
    }
    Ok(())
}

fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create directory {}", parent.display()))?;
    }
    let (tmp, mut file) = create_atomic_temp_file(path)?;
    if let Err(error) = write_atomic_temp_file(&tmp, &mut file, bytes) {
        drop(file);
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    drop(file);

    let result = fs::rename(&tmp, path)
        .with_context(|| format!("rename {} to {}", tmp.display(), path.display()));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn create_atomic_temp_file(path: &Path) -> Result<(PathBuf, File)> {
    for _ in 0..ATOMIC_WRITE_TEMP_ATTEMPTS {
        let tmp = unique_atomic_temp_path(path);
        match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(file) => return Ok((tmp, file)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("create temp file {}", tmp.display()));
            }
        }
    }

    bail!(
        "failed to create unique temp file next to {} after {} attempts",
        path.display(),
        ATOMIC_WRITE_TEMP_ATTEMPTS
    )
}

fn unique_atomic_temp_path(path: &Path) -> PathBuf {
    let suffix = ATOMIC_WRITE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("atomic-write");
    path.with_file_name(format!(".{filename}.tmp.{}.{}", std::process::id(), suffix))
}

fn write_atomic_temp_file(path: &Path, file: &mut File, bytes: &[u8]) -> Result<()> {
    file.write_all(bytes)
        .with_context(|| format!("write temp file {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync temp file {}", path.display()))
}

fn validate_repo_name_for_identity(repo_name: &str) -> Result<()> {
    let identity = RepoIdentity {
        schema: 1,
        repo: RepoIdentityRepo {
            name: repo_name.to_string(),
            description: None,
        },
        trust: RepoIdentityTrust {
            root_key_ids: vec![
                "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            ],
        },
    };
    identity.validate()
}

pub fn prepare_static_key_dir(base: &Path, repo_name: &str) -> Result<PathBuf> {
    validate_static_repo_name(repo_name)?;

    let key_dir = base.join(repo_name);
    create_private_dir_all(&key_dir)
        .with_context(|| format!("create static repo key directory {}", key_dir.display()))?;

    Ok(key_dir)
}

fn validate_static_repo_name(repo_name: &str) -> Result<()> {
    if repo_name.trim().is_empty() {
        bail!("repo name must not be empty and must be one safe path segment");
    }

    let repo_path = Path::new(repo_name);
    if repo_path.is_absolute()
        || repo_name.contains('/')
        || repo_name.contains('\\')
        || repo_name == "."
        || repo_name == ".."
        || repo_path.components().count() != 1
    {
        bail!("repo name must be one safe path segment");
    }

    Ok(())
}
