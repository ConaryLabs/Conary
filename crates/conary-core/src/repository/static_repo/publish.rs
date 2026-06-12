// conary-core/src/repository/static_repo/publish.rs

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Duration, Utc};
use fs2::FileExt;

use crate::ccs::builder::{BuildResult, write_signed_ccs_package};
use crate::ccs::package::CcsPackage;
use crate::ccs::signing::SigningKeyPair;
use crate::hash;
use crate::packages::traits::PackageFormat;
use crate::repository::static_repo::{
    PackageKeyEntry, PackageKeyStatus, PackageKeysFile, RepoIdentity, RepoIdentityRepo,
    RepoIdentityTrust, RepoLocation, StaticIndex, StaticPackageEntry, validate_repo_relative_path,
};
use crate::trust::ceremony::{create_initial_root, rotate_key, rotate_publish_key};
use crate::trust::generate::{generate_snapshot, generate_targets, generate_timestamp};
use crate::trust::keys::{sign_tuf_metadata, signing_keypair_to_tuf_key};
use crate::trust::metadata::{
    Role, RootMetadata, Signed, SnapshotMetadata, TargetsMetadata, TimestampMetadata,
};
use crate::trust::verify::{
    extract_role_keys, verify_metadata_hash, verify_signatures, verify_static_snapshot_consistency,
};

const ROOT_EXPIRES_DAYS: i64 = 365;
const TARGETS_EXPIRES_DAYS: i64 = 90;
const SNAPSHOT_EXPIRES_DAYS: i64 = 90;
const TIMESTAMP_EXPIRES_HOURS: i64 = 720;
const ROOT_IDENTITY_WARNING: &str = "the root key **is** the repo's identity — store `root.private` offline if possible, and back up the whole directory; losing it means clients must manually re-trust (§7.4).";
const FORCE_REINIT_WARNING: &str = "force reinit started a fresh repo identity; clients must run repo reset-trust and re-pin the new repo fingerprint";
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
    pub force_reinit: bool,
    pub accept_destination_state: bool,
    pub rotate_publish_key: bool,
    pub rotate_root_key: bool,
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
    let RepoLocation::File { root: repo_root } = &options.destination else {
        bail!("M1a static publisher supports local filesystem destinations only");
    };

    validate_repo_name_for_identity(&options.repo_name)?;
    create_private_dir_all(&options.key_dir).with_context(|| {
        format!(
            "create static repo key directory {}",
            options.key_dir.display()
        )
    })?;
    fs::create_dir_all(repo_root)
        .with_context(|| format!("create static repo destination {}", repo_root.display()))?;

    let _publish_lock = PublishLock::acquire(repo_root)?;
    let mut root_key = ensure_key_pair(&options.key_dir, "root")?;
    let mut publish_key = ensure_key_pair(&options.key_dir, "publish")?;
    let mut pending_key_promotions = PendingKeyPromotions::default();
    let destination = read_destination_state(repo_root, options.force_reinit)?;
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
            &options.key_dir,
            &mut root_key,
            &mut publish_key,
            &mut pending_key_promotions,
        )?;
        verify_destination_matches_operator_keys(&root_metadata, &root_key, &publish_key)?;
    }

    let mut root_changed = destination.initial || options.force_reinit;
    let mut identity_changed = destination.initial || options.force_reinit;
    let should_rotate_publish_key = options.rotate_publish_key && !recovered_pending_keys.publish;
    let should_rotate_root_key = options.rotate_root_key && !recovered_pending_keys.root;
    if should_rotate_publish_key {
        old_publish_public_key = Some(publish_key.public_key_base64());
        let new_publish_key = pending_key_promotions.stage_or_load(&options.key_dir, "publish")?;
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
        let new_root_key = pending_key_promotions.stage_or_load(&options.key_dir, "root")?;
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
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|text| StaticIndex::parse(text).ok());
    let old_package_keys = destination
        .package_keys_bytes
        .as_deref()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|text| PackageKeysFile::parse(text).ok());

    let mut pending_package_writes =
        stage_packages(repo_root, &options.package_paths, &publish_key)?;
    let package_entries = if options.refresh
        && options.package_paths.is_empty()
        && old_index.is_some()
        && should_preserve_index_packages(&destination, forced_refresh)
    {
        old_index.clone().expect("checked").packages
    } else {
        let mut entries = collect_package_entries(repo_root)?;
        entries.extend(
            pending_package_writes
                .package_entries()
                .into_iter()
                .cloned(),
        );
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        entries
    };

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
        force_reinit: options.force_reinit,
    })?;
    if targets_bump {
        conditional_write(
            repo_root,
            "index.json",
            &index_bytes,
            destination.index_bytes.as_deref(),
            options.force_reinit,
        )?;
        conditional_write(
            repo_root,
            "metadata/targets.json",
            &targets_bytes,
            destination.targets_bytes.as_deref(),
            options.force_reinit,
        )?;
    }
    if snapshot_bump {
        conditional_write(
            repo_root,
            "metadata/snapshot.json",
            &snapshot_bytes,
            destination.snapshot_bytes.as_deref(),
            options.force_reinit,
        )?;
    }
    pending_key_promotions.promote(&options.key_dir)?;
    pending_package_writes.promote()?;
    ensure_timestamp_unchanged(repo_root, &destination)?;
    conditional_write(
        repo_root,
        "metadata/timestamp.json",
        &timestamp_bytes,
        destination.timestamp_bytes.as_deref(),
        options.force_reinit,
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
    let warning = if options.force_reinit {
        FORCE_REINIT_WARNING.to_string()
    } else if destination.initial || identity_changed {
        ROOT_IDENTITY_WARNING.to_string()
    } else if options.accept_destination_state {
        "accepted destination versions below local watermark".to_string()
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

#[derive(Default)]
struct DestinationState {
    initial: bool,
    root: Option<Signed<RootMetadata>>,
    targets: Option<Signed<TargetsMetadata>>,
    snapshot: Option<Signed<SnapshotMetadata>>,
    timestamp: Option<Signed<TimestampMetadata>>,
    root_bytes: Option<Vec<u8>>,
    targets_bytes: Option<Vec<u8>>,
    snapshot_bytes: Option<Vec<u8>>,
    timestamp_bytes: Option<Vec<u8>>,
    identity_bytes: Option<Vec<u8>>,
    index_bytes: Option<Vec<u8>>,
    package_keys_bytes: Option<Vec<u8>>,
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

#[derive(Default)]
struct PendingKeyRecovery {
    root: bool,
    publish: bool,
}

#[derive(Default)]
struct PendingKeyPromotions {
    entries: Vec<PendingKeyPromotion>,
}

struct PendingKeyPromotion {
    role: String,
    pending_role: String,
}

impl PendingKeyPromotions {
    fn stage_or_load(&mut self, key_dir: &Path, role: &str) -> Result<SigningKeyPair> {
        let pending_role = format!("{role}.pending");
        let key = ensure_pending_key_pair(key_dir, role, &pending_role)?;
        self.track(role);
        Ok(key)
    }

    fn track(&mut self, role: &str) {
        if !self.entries.iter().any(|entry| entry.role == role) {
            self.entries.push(PendingKeyPromotion {
                role: role.to_string(),
                pending_role: format!("{role}.pending"),
            });
        }
    }

    fn promote(&self, key_dir: &Path) -> Result<()> {
        for entry in &self.entries {
            promote_pending_key(key_dir, entry)
                .with_context(|| format!("promote pending {} key", entry.role))?;
        }
        Ok(())
    }
}

fn recover_pending_key_promotions(
    root: &Signed<RootMetadata>,
    key_dir: &Path,
    root_key: &mut SigningKeyPair,
    publish_key: &mut SigningKeyPair,
    pending_key_promotions: &mut PendingKeyPromotions,
) -> Result<PendingKeyRecovery> {
    let mut recovered = PendingKeyRecovery::default();

    if !role_contains_key(root, "root", root_key)?
        && let Some(pending_root_key) = load_pending_key_pair(key_dir, "root")?
        && role_contains_key(root, "root", &pending_root_key)?
    {
        *root_key = pending_root_key;
        pending_key_promotions.track("root");
        recovered.root = true;
    }

    if !publish_roles_contain_key(root, publish_key)?
        && let Some(pending_publish_key) = load_pending_key_pair(key_dir, "publish")?
        && publish_roles_contain_key(root, &pending_publish_key)?
    {
        *publish_key = pending_publish_key;
        pending_key_promotions.track("publish");
        recovered.publish = true;
    }

    Ok(recovered)
}

fn role_contains_key(
    root: &Signed<RootMetadata>,
    role_name: &str,
    key: &SigningKeyPair,
) -> Result<bool> {
    let (key_id, _) = signing_keypair_to_tuf_key(key).map_err(anyhow::Error::from)?;
    let role = root
        .signed
        .roles
        .get(role_name)
        .with_context(|| format!("destination root metadata missing {role_name} role"))?;
    Ok(role.keyids.contains(&key_id))
}

fn publish_roles_contain_key(root: &Signed<RootMetadata>, key: &SigningKeyPair) -> Result<bool> {
    for role_name in ["targets", "snapshot", "timestamp"] {
        if !role_contains_key(root, role_name, key)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn load_pending_key_pair(key_dir: &Path, role: &str) -> Result<Option<SigningKeyPair>> {
    let pending_role = format!("{role}.pending");
    let pending_private = key_dir.join(format!("{pending_role}.private"));
    if !pending_private.exists() {
        return Ok(None);
    }

    let key = SigningKeyPair::load_from_file(&pending_private)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("load pending {role} key {}", pending_private.display()))?;
    save_key_pair(&key, key_dir, &pending_role)
        .with_context(|| format!("refresh pending {role} key files"))?;
    Ok(Some(key))
}

fn ensure_pending_key_pair(
    key_dir: &Path,
    role: &str,
    pending_role: &str,
) -> Result<SigningKeyPair> {
    let pending_private = key_dir.join(format!("{pending_role}.private"));
    if pending_private.exists() {
        let key = SigningKeyPair::load_from_file(&pending_private)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("load pending {role} key {}", pending_private.display()))?;
        save_key_pair(&key, key_dir, pending_role)
            .with_context(|| format!("refresh pending {role} key files"))?;
        return Ok(key);
    }

    let key = SigningKeyPair::generate().with_key_id(role);
    save_key_pair(&key, key_dir, pending_role)
        .with_context(|| format!("stage pending {role} key promotion"))?;
    Ok(key)
}

fn promote_pending_key(key_dir: &Path, entry: &PendingKeyPromotion) -> Result<()> {
    let pending_private = key_dir.join(format!("{}.private", entry.pending_role));
    let pending_public = key_dir.join(format!("{}.public", entry.pending_role));
    let active_private = key_dir.join(format!("{}.private", entry.role));
    let active_public = key_dir.join(format!("{}.public", entry.role));

    fs::rename(&pending_private, &active_private).with_context(|| {
        format!(
            "replace active {} private key {} with {}",
            entry.role,
            active_private.display(),
            pending_private.display()
        )
    })?;
    fs::rename(&pending_public, &active_public).with_context(|| {
        format!(
            "replace active {} public key {} with {}",
            entry.role,
            active_public.display(),
            pending_public.display()
        )
    })
}

#[derive(serde::Deserialize, serde::Serialize)]
struct PublishWatermark {
    root_version: u64,
    targets_version: u64,
    snapshot_version: u64,
    timestamp_version: u64,
}

fn read_destination_state(repo_root: &Path, force_reinit: bool) -> Result<DestinationState> {
    let root_bytes = read_optional(repo_root, "metadata/root.json")?;
    let targets_bytes = read_optional(repo_root, "metadata/targets.json")?;
    let snapshot_bytes = read_optional(repo_root, "metadata/snapshot.json")?;
    let timestamp_bytes = read_optional(repo_root, "metadata/timestamp.json")?;

    let all_absent = root_bytes.is_none()
        && targets_bytes.is_none()
        && snapshot_bytes.is_none()
        && timestamp_bytes.is_none();
    if all_absent || force_reinit {
        return Ok(DestinationState {
            initial: true,
            root_bytes,
            targets_bytes,
            snapshot_bytes,
            timestamp_bytes,
            identity_bytes: read_optional(repo_root, "conary-repo.toml")?,
            index_bytes: read_optional(repo_root, "index.json")?,
            package_keys_bytes: read_optional(repo_root, "keys/package-keys.json")?,
            ..DestinationState::default()
        });
    }

    if root_bytes.is_none()
        || targets_bytes.is_none()
        || snapshot_bytes.is_none()
        || timestamp_bytes.is_none()
    {
        bail!(
            "static repo destination is damaged or partially initialized; rerun with force_reinit to start a fresh identity"
        );
    }

    let root: Signed<RootMetadata> = serde_json::from_slice(root_bytes.as_ref().expect("checked"))
        .context("parse destination metadata/root.json")?;
    let targets: Signed<TargetsMetadata> =
        serde_json::from_slice(targets_bytes.as_ref().expect("checked"))
            .context("parse destination metadata/targets.json")?;
    let snapshot: Signed<SnapshotMetadata> =
        serde_json::from_slice(snapshot_bytes.as_ref().expect("checked"))
            .context("parse destination metadata/snapshot.json")?;
    let timestamp: Signed<TimestampMetadata> =
        serde_json::from_slice(timestamp_bytes.as_ref().expect("checked"))
            .context("parse destination metadata/timestamp.json")?;
    verify_destination_metadata(
        &root,
        &targets,
        &snapshot,
        &timestamp,
        targets_bytes.as_ref().expect("checked"),
        snapshot_bytes.as_ref().expect("checked"),
    )?;

    Ok(DestinationState {
        initial: false,
        root: Some(root),
        targets: Some(targets),
        snapshot: Some(snapshot),
        timestamp: Some(timestamp),
        root_bytes,
        targets_bytes,
        snapshot_bytes,
        timestamp_bytes,
        identity_bytes: read_optional(repo_root, "conary-repo.toml")?,
        index_bytes: read_optional(repo_root, "index.json")?,
        package_keys_bytes: read_optional(repo_root, "keys/package-keys.json")?,
    })
}

fn verify_destination_metadata(
    root: &Signed<RootMetadata>,
    targets: &Signed<TargetsMetadata>,
    snapshot: &Signed<SnapshotMetadata>,
    timestamp: &Signed<TimestampMetadata>,
    targets_bytes: &[u8],
    snapshot_bytes: &[u8],
) -> Result<()> {
    let (root_keys, root_threshold) =
        extract_role_keys(&root.signed, Role::Root).map_err(anyhow::Error::from)?;
    verify_signatures(root, Role::Root, &root_keys, root_threshold).map_err(anyhow::Error::from)?;

    let (targets_keys, targets_threshold) =
        extract_role_keys(&root.signed, Role::Targets).map_err(anyhow::Error::from)?;
    verify_signatures(targets, Role::Targets, &targets_keys, targets_threshold)
        .map_err(anyhow::Error::from)?;

    let (snapshot_keys, snapshot_threshold) =
        extract_role_keys(&root.signed, Role::Snapshot).map_err(anyhow::Error::from)?;
    verify_signatures(snapshot, Role::Snapshot, &snapshot_keys, snapshot_threshold)
        .map_err(anyhow::Error::from)?;

    let (timestamp_keys, timestamp_threshold) =
        extract_role_keys(&root.signed, Role::Timestamp).map_err(anyhow::Error::from)?;
    let timestamp_signature_result = verify_signatures(
        timestamp,
        Role::Timestamp,
        &timestamp_keys,
        timestamp_threshold,
    )
    .map_err(anyhow::Error::from);

    verify_static_snapshot_consistency(
        &snapshot.signed,
        root.signed.version,
        targets.signed.version,
    )
    .map_err(anyhow::Error::from)?;
    let targets_ref = snapshot
        .signed
        .meta
        .get("targets.json")
        .context("snapshot metadata missing targets.json")?;
    verify_metadata_hash(targets_ref, targets_bytes, true).map_err(anyhow::Error::from)?;
    let timestamp_pins_current_snapshot =
        timestamp_pins_current_snapshot(timestamp, snapshot, snapshot_bytes)?;
    if timestamp_pins_current_snapshot {
        timestamp_signature_result?;
    }

    Ok(())
}

fn timestamp_pins_current_snapshot(
    timestamp: &Signed<TimestampMetadata>,
    snapshot: &Signed<SnapshotMetadata>,
    snapshot_bytes: &[u8],
) -> Result<bool> {
    let snapshot_ref = timestamp
        .signed
        .meta
        .get("snapshot.json")
        .context("timestamp metadata missing snapshot.json")?;
    if snapshot_ref.version != snapshot.signed.version {
        return Ok(false);
    }
    if let Some(length) = snapshot_ref.length
        && length != snapshot_bytes.len() as u64
    {
        bail!(
            "timestamp pins snapshot.json length {} but current snapshot length is {}",
            length,
            snapshot_bytes.len()
        );
    }
    verify_metadata_hash(snapshot_ref, snapshot_bytes, true).map_err(anyhow::Error::from)?;

    Ok(true)
}

fn verify_destination_matches_operator_keys(
    root: &Signed<RootMetadata>,
    root_key: &SigningKeyPair,
    publish_key: &SigningKeyPair,
) -> Result<()> {
    let (root_key_id, _) = signing_keypair_to_tuf_key(root_key).map_err(anyhow::Error::from)?;
    let root_role = root
        .signed
        .roles
        .get("root")
        .context("destination root metadata missing root role")?;
    if !root_role.keyids.contains(&root_key_id) {
        bail!(
            "destination root role does not match local root key; use force_reinit only for a fresh repo identity"
        );
    }

    let (publish_key_id, _) =
        signing_keypair_to_tuf_key(publish_key).map_err(anyhow::Error::from)?;
    for role in ["targets", "snapshot", "timestamp"] {
        let role_def = root
            .signed
            .roles
            .get(role)
            .with_context(|| format!("destination root metadata missing {role} role"))?;
        if !role_def.keyids.contains(&publish_key_id) {
            bail!(
                "destination {role} role does not match local publish key; use force_reinit only for a fresh repo identity"
            );
        }
    }

    Ok(())
}

fn read_optional(root: &Path, relative: &str) -> Result<Option<Vec<u8>>> {
    validate_repo_relative_path(relative)?;
    let path = root.join(relative);
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

fn check_watermark(destination: &DestinationState, options: &StaticPublishOptions) -> Result<()> {
    if options.force_reinit {
        return Ok(());
    }

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
    if regressed && !options.accept_destination_state {
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

fn ensure_key_pair(key_dir: &Path, role: &str) -> Result<SigningKeyPair> {
    let private_path = key_dir.join(format!("{role}.private"));
    if private_path.exists() {
        return SigningKeyPair::load_from_file(&private_path)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("load {role} key {}", private_path.display()));
    }

    let key = SigningKeyPair::generate().with_key_id(role);
    save_key_pair(&key, key_dir, role)?;
    Ok(key)
}

fn save_key_pair(key: &SigningKeyPair, key_dir: &Path, role: &str) -> Result<()> {
    key.save_to_files(
        &key_dir.join(format!("{role}.private")),
        &key_dir.join(format!("{role}.public")),
    )
    .map_err(anyhow::Error::from)
    .with_context(|| format!("save {role} key in {}", key_dir.display()))
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

fn should_preserve_index_packages(destination: &DestinationState, forced: ForcedRefresh) -> bool {
    forced.targets
        || forced.root
        || forced.snapshot
        || destination.targets.as_ref().is_some_and(|metadata| {
            is_near_expiry(metadata.signed.expires, TARGETS_EXPIRES_DAYS * 24)
        })
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

#[derive(Default)]
struct PendingPackageWrites {
    writes: Vec<PendingPackageWrite>,
    committed: bool,
}

struct PendingPackageWrite {
    entry: StaticPackageEntry,
    pending_path: PathBuf,
    final_path: PathBuf,
    promoted: bool,
}

impl PendingPackageWrites {
    fn package_entries(&self) -> Vec<&StaticPackageEntry> {
        self.writes.iter().map(|write| &write.entry).collect()
    }

    fn target_entry(&self, relative: &str) -> Option<(u64, String)> {
        self.writes
            .iter()
            .find(|write| write.entry.path == relative)
            .map(|write| (write.entry.size, write.entry.sha256.clone()))
    }

    fn promote(&mut self) -> Result<()> {
        for write in &mut self.writes {
            if write.final_path.exists() {
                let existing = fs::read(&write.final_path)
                    .with_context(|| format!("read {}", write.final_path.display()))?;
                let pending = fs::read(&write.pending_path)
                    .with_context(|| format!("read {}", write.pending_path.display()))?;
                if existing == pending {
                    fs::remove_file(&write.pending_path)
                        .with_context(|| format!("remove {}", write.pending_path.display()))?;
                    continue;
                }
                bail!(
                    "immutable package artifact {} appeared during publish with different bytes",
                    write.entry.path
                );
            }
            if let Some(parent) = write.final_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create package directory {}", parent.display()))?;
            }
            fs::rename(&write.pending_path, &write.final_path).with_context(|| {
                format!(
                    "promote package {} to {}",
                    write.pending_path.display(),
                    write.final_path.display()
                )
            })?;
            write.promoted = true;
        }
        Ok(())
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for PendingPackageWrites {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for write in &self.writes {
            let _ = fs::remove_file(&write.pending_path);
            if write.promoted {
                let _ = fs::remove_file(&write.final_path);
            }
        }
    }
}

fn stage_packages(
    repo_root: &Path,
    package_paths: &[PathBuf],
    publish_key: &SigningKeyPair,
) -> Result<PendingPackageWrites> {
    let mut pending = PendingPackageWrites::default();
    for package_path in package_paths {
        let package = CcsPackage::parse(package_path.to_str().ok_or_else(|| {
            anyhow!(
                "package path is not valid UTF-8: {}",
                package_path.display()
            )
        })?)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("parse CCS package {}", package_path.display()))?;
        let signed_bytes = sign_package_bytes(&package, publish_key)
            .with_context(|| format!("sign CCS package {}", package_path.display()))?;
        let relative = package_relative_path(&package);
        let destination = repo_root.join(&relative);
        if let Some(existing) = read_optional(repo_root, &relative)? {
            if existing != signed_bytes {
                bail!("immutable package artifact {relative} already exists with different bytes");
            }
            continue;
        }
        let pending_path = write_pending_package(&destination, &signed_bytes)?;
        pending.writes.push(PendingPackageWrite {
            entry: package_entry_from_package(&relative, &package, &signed_bytes)?,
            pending_path,
            final_path: destination,
            promoted: false,
        });
    }

    Ok(pending)
}

fn write_pending_package(final_path: &Path, bytes: &[u8]) -> Result<PathBuf> {
    if let Some(parent) = final_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create package directory {}", parent.display()))?;
    }
    let (pending_path, mut file) = create_atomic_temp_file(final_path)?;
    if let Err(error) = write_atomic_temp_file(&pending_path, &mut file, bytes) {
        drop(file);
        let _ = fs::remove_file(&pending_path);
        return Err(error);
    }
    drop(file);
    Ok(pending_path)
}

fn sign_package_bytes(package: &CcsPackage, publish_key: &SigningKeyPair) -> Result<Vec<u8>> {
    let build_result = BuildResult {
        manifest: package.manifest().clone(),
        components: package.components().clone(),
        files: package.file_entries().to_vec(),
        blobs: package.extract_all_content().map_err(anyhow::Error::from)?,
        total_size: package.file_entries().iter().map(|entry| entry.size).sum(),
        chunked: package
            .file_entries()
            .iter()
            .any(|entry| entry.chunks.is_some()),
        chunk_stats: None,
    };
    let signed_package = tempfile::NamedTempFile::new()?;
    write_signed_ccs_package(&build_result, signed_package.path(), publish_key)?;
    fs::read(signed_package.path())
        .with_context(|| format!("read signed package {}", signed_package.path().display()))
}

fn package_relative_path(package: &CcsPackage) -> String {
    let arch = package.architecture().unwrap_or("noarch");
    format!(
        "packages/{}/{}-{}-1-{}.ccs",
        package.name(),
        package.name(),
        package.version(),
        arch
    )
}

fn package_entry_from_package(
    relative: &str,
    package: &CcsPackage,
    bytes: &[u8],
) -> Result<StaticPackageEntry> {
    let (name, version, release, arch) = parse_package_filename(relative)?;
    if package.name() != name || package.version() != version {
        bail!(
            "package metadata {}-{} does not match artifact path {}-{}",
            package.name(),
            package.version(),
            name,
            version
        );
    }
    if package.architecture().unwrap_or("noarch") != arch {
        bail!(
            "package architecture {:?} does not match artifact path {arch}",
            package.architecture()
        );
    }
    Ok(StaticPackageEntry {
        name,
        version,
        release,
        arch,
        path: relative.to_string(),
        sha256: hash::sha256(bytes),
        size: bytes.len() as u64,
        description: package.description().map(str::to_string),
        dependencies: package
            .dependencies()
            .iter()
            .map(|dep| match &dep.version {
                Some(version) => format!("{} {}", dep.name, version),
                None => dep.name.clone(),
            })
            .collect(),
    })
}

fn collect_package_entries(repo_root: &Path) -> Result<Vec<StaticPackageEntry>> {
    let packages_root = repo_root.join("packages");
    if !packages_root.exists() {
        return Ok(Vec::new());
    }

    let mut package_paths = Vec::new();
    collect_ccs_paths(&packages_root, &mut package_paths)?;
    package_paths.sort();

    let mut entries = Vec::new();
    for path in package_paths {
        let relative = path
            .strip_prefix(repo_root)
            .map_err(|_| anyhow!("package path escaped repo root: {}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        validate_repo_relative_path(&relative)?;
        let package = CcsPackage::parse(
            path.to_str()
                .ok_or_else(|| anyhow!("package path is not valid UTF-8: {}", path.display()))?,
        )
        .map_err(anyhow::Error::from)
        .with_context(|| format!("parse published CCS package {}", path.display()))?;
        let bytes = fs::read(&path).with_context(|| format!("read package {}", path.display()))?;
        entries.push(package_entry_from_package(&relative, &package, &bytes)?);
    }

    Ok(entries)
}

fn collect_ccs_paths(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read directory {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_ccs_paths(&path, out)?;
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "ccs") {
            out.push(path);
        }
    }
    Ok(())
}

fn parse_package_filename(relative: &str) -> Result<(String, String, String, String)> {
    let filename = relative
        .rsplit('/')
        .next()
        .ok_or_else(|| anyhow!("package path has no filename: {relative}"))?;
    let stem = filename
        .strip_suffix(".ccs")
        .ok_or_else(|| anyhow!("package path is not a .ccs artifact: {relative}"))?;
    let mut parts = stem.rsplitn(4, '-').collect::<Vec<_>>();
    if parts.len() != 4 {
        bail!("package filename must be <name>-<version>-<release>-<arch>.ccs: {relative}");
    }
    parts.reverse();
    Ok((
        parts[0].to_string(),
        parts[1].to_string(),
        parts[2].to_string(),
        parts[3].to_string(),
    ))
}

fn build_package_keys_file(
    old_keys: Option<&PackageKeysFile>,
    publish_key: &SigningKeyPair,
    retired_public_key: Option<String>,
) -> Result<PackageKeysFile> {
    let active_public_key = publish_key.public_key_base64();
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();

    if let Some(old_keys) = old_keys {
        for key in &old_keys.keys {
            let mut key = key.clone();
            if Some(key.public_key.as_str()) == retired_public_key.as_deref() {
                key.status = PackageKeyStatus::Retired;
            }
            if key.public_key == active_public_key {
                continue;
            }
            if seen.insert(key.public_key.clone()) {
                entries.push(key);
            }
        }
    }

    if let Some(public_key) = retired_public_key
        && public_key != active_public_key
        && seen.insert(public_key.clone())
    {
        entries.push(PackageKeyEntry {
            algorithm: "ed25519".to_string(),
            public_key,
            key_id: Some("publish".to_string()),
            status: PackageKeyStatus::Retired,
            comment: Some("retired publishing key".to_string()),
        });
    }

    entries.push(PackageKeyEntry {
        algorithm: "ed25519".to_string(),
        public_key: active_public_key,
        key_id: Some("publish".to_string()),
        status: PackageKeyStatus::Active,
        comment: Some("primary publishing key".to_string()),
    });

    let keys = PackageKeysFile {
        schema: 1,
        keys: entries,
    };
    keys.validate()?;
    Ok(keys)
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
    force_reinit: bool,
}

fn write_step_a(input: StepAWrite<'_>) -> Result<()> {
    conditional_write(
        input.repo_root,
        "keys/package-keys.json",
        input.package_keys_bytes,
        input.destination.package_keys_bytes.as_deref(),
        input.force_reinit,
    )?;
    if input.root_changed {
        let historical_root = format!("metadata/{}.root.json", input.root_metadata.signed.version);
        write_immutable(
            input.repo_root,
            &historical_root,
            input.root_bytes,
            input.force_reinit,
        )?;
        conditional_write(
            input.repo_root,
            "metadata/root.json",
            input.root_bytes,
            input.destination.root_bytes.as_deref(),
            input.force_reinit,
        )?;
    }
    if input.identity_changed {
        conditional_write(
            input.repo_root,
            "conary-repo.toml",
            input.identity_bytes,
            input.destination.identity_bytes.as_deref(),
            input.force_reinit,
        )?;
    }
    Ok(())
}

fn write_immutable(
    repo_root: &Path,
    relative: &str,
    bytes: &[u8],
    force_reinit: bool,
) -> Result<()> {
    validate_repo_relative_path(relative)?;
    let path = repo_root.join(relative);
    if let Ok(existing) = fs::read(&path) {
        if existing == bytes {
            return Ok(());
        }
        if force_reinit {
            return write_file_atomic(&path, bytes);
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
    force_reinit: bool,
) -> Result<()> {
    validate_repo_relative_path(relative)?;
    let path = repo_root.join(relative);
    match fs::read(&path) {
        Ok(existing) => {
            if !force_reinit && expected_previous != Some(existing.as_slice()) {
                bail!("static repo path {relative} changed during publish");
            }
            if existing == bytes {
                return Ok(());
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            if expected_previous.is_some() && !force_reinit {
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

#[cfg(unix)]
fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

#[cfg(test)]
mod tests {
    use super::{
        ForcedRefreshForTest, StaticPublishOptions, prepare_static_key_dir, publish_static_repo,
        publish_static_repo_with_forced_refresh_for_test, save_key_pair, stage_packages,
        try_acquire_publish_lock_for_test, unique_atomic_temp_path,
    };
    use crate::ccs::builder::{CcsBuilder, write_ccs_package};
    use crate::ccs::manifest::CcsManifest;
    use crate::ccs::signing::SigningKeyPair;
    use crate::ccs::verify::{SignatureStatus, TrustPolicy, verify_package};
    use crate::packages::traits::PackageFormat;
    use crate::repository::static_repo::{
        PackageKeyStatus, PackageKeysFile, RepoIdentity, RepoLocation, StaticIndex,
    };
    use crate::trust::keys::sign_tuf_metadata;
    use crate::trust::metadata::{
        RootMetadata, Signed, SnapshotMetadata, TargetsMetadata, TimestampMetadata,
    };
    use crate::trust::signing_keypair_to_tuf_key;
    use chrono::{Duration, Utc};
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    const ROOT_IDENTITY_WARNING: &str = "the root key **is** the repo's identity — store `root.private` offline if possible, and back up the whole directory; losing it means clients must manually re-trust (§7.4).";

    #[test]
    #[cfg(unix)]
    fn prepare_static_key_dir_creates_repo_key_dir_0700() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let key_base = temp_dir.path().join(".config/conary/keys");

        let key_dir = prepare_static_key_dir(&key_base, "test-repo").unwrap();

        assert_eq!(key_dir, key_base.join("test-repo"));
        assert!(key_dir.is_dir());
        let mode = std::fs::metadata(&key_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn prepare_static_key_dir_rejects_empty_repo_name() {
        let temp_dir = tempfile::tempdir().unwrap();

        let error = prepare_static_key_dir(temp_dir.path(), "").unwrap_err();

        assert!(
            error.to_string().contains("repo name must not be empty"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn prepare_static_key_dir_rejects_unsafe_repo_name_segments() {
        let temp_dir = tempfile::tempdir().unwrap();
        let key_base = temp_dir.path().join(".config/conary/keys");
        let absolute_name = temp_dir.path().join("escape").display().to_string();

        for repo_name in [
            "nested/repo",
            "../escape",
            &absolute_name,
            r"nested\repo",
            "   ",
            ".",
            "..",
        ] {
            let error = prepare_static_key_dir(&key_base, repo_name).unwrap_err();
            assert!(
                error.to_string().contains("safe path segment"),
                "unexpected error for {repo_name:?}: {error}"
            );
        }
    }

    #[test]
    fn initial_publish_creates_static_repo_layout_and_identity_warning() {
        let fixture = PublishFixture::new();
        let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");

        let outcome = publish_static_repo(fixture.options(vec![package])).unwrap();

        assert_eq!(outcome.root_version, 1);
        assert_eq!(outcome.targets_version, 1);
        assert_eq!(outcome.snapshot_version, 1);
        assert_eq!(outcome.timestamp_version, 1);
        assert_eq!(outcome.package_count, 1);
        assert_eq!(outcome.preview_warning, ROOT_IDENTITY_WARNING);
        assert_eq!(outcome.root_key_ids.len(), 1);
        assert!(!outcome.publish_key_id.is_empty());

        for relative in [
            "conary-repo.toml",
            "metadata/1.root.json",
            "metadata/root.json",
            "metadata/targets.json",
            "metadata/snapshot.json",
            "metadata/timestamp.json",
            "index.json",
            "keys/package-keys.json",
            "packages/widget/widget-1.0.0-1-x86_64.ccs",
        ] {
            assert!(
                fixture.repo_path(relative).exists(),
                "expected {relative} to be published"
            );
        }

        assert_eq!(
            fs::read(fixture.repo_path("metadata/1.root.json")).unwrap(),
            fs::read(fixture.repo_path("metadata/root.json")).unwrap()
        );

        let identity = read_identity(&fixture.repo_path("conary-repo.toml"));
        let root = read_root(&fixture.repo_path("metadata/root.json"));
        assert_eq!(
            identity.trust.root_key_ids,
            root.signed.roles["root"].keyids
        );
        assert_eq!(identity.trust.root_key_ids, outcome.root_key_ids);
    }

    #[test]
    fn package_overwrite_with_different_bytes_fails() {
        let fixture = PublishFixture::new();
        let first = fixture.build_package("widget", "1.0.0", "x86_64", b"first\n");
        publish_static_repo(fixture.options(vec![first])).unwrap();
        let second = fixture.build_package("widget", "1.0.0", "x86_64", b"second\n");

        let error = publish_static_repo(fixture.options(vec![second])).unwrap_err();

        assert!(
            error.to_string().contains("immutable package artifact"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn index_version_matches_targets_and_package_keys_include_active_publish_key() {
        let fixture = PublishFixture::new();
        let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
        let outcome = publish_static_repo(fixture.options(vec![package])).unwrap();

        let index = read_index(&fixture.repo_path("index.json"));
        let targets = read_targets(&fixture.repo_path("metadata/targets.json"));
        let package_keys = read_package_keys(&fixture.repo_path("keys/package-keys.json"));

        assert_eq!(index.index_version, targets.signed.version);
        assert!(targets.signed.targets.contains_key("index.json"));
        assert!(
            targets
                .signed
                .targets
                .contains_key("keys/package-keys.json")
        );
        assert_eq!(package_keys.keys.len(), 1);
        assert!(matches!(
            package_keys.keys[0].status,
            PackageKeyStatus::Active
        ));
        assert_eq!(package_keys.keys[0].key_id.as_deref(), Some("publish"));

        let publish_key = SigningKeyPair::load_from_file(&fixture.key_dir.join("publish.private"))
            .expect("publish key loads");
        let (publish_key_id, _) = signing_keypair_to_tuf_key(&publish_key).unwrap();
        assert_eq!(outcome.publish_key_id, publish_key_id);
        assert_eq!(
            package_keys.keys[0].public_key,
            publish_key.public_key_base64()
        );
    }

    #[test]
    fn publish_signs_unsigned_package_with_active_publish_key() {
        let fixture = PublishFixture::new();
        let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
        publish_static_repo(fixture.options(vec![package])).unwrap();

        let index = read_index(&fixture.repo_path("index.json"));
        let package_keys = read_package_keys(&fixture.repo_path("keys/package-keys.json"));
        let active_public_key = package_keys
            .keys
            .iter()
            .find(|key| matches!(key.status, PackageKeyStatus::Active))
            .expect("active package key")
            .public_key
            .clone();
        let published_package = fixture.repo_path(&index.packages[0].path);
        let verification = verify_package(
            &published_package,
            &TrustPolicy::strict(vec![active_public_key]),
        )
        .unwrap();

        assert!(verification.valid);
        assert!(matches!(
            verification.signature_status,
            SignatureStatus::Valid { .. }
        ));
    }

    #[test]
    fn refresh_without_near_expiry_changes_only_timestamp() {
        let fixture = PublishFixture::new();
        let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
        publish_static_repo(fixture.options(vec![package])).unwrap();

        let before_index = fs::read(fixture.repo_path("index.json")).unwrap();
        let before_targets = fs::read(fixture.repo_path("metadata/targets.json")).unwrap();
        let before_snapshot = fs::read(fixture.repo_path("metadata/snapshot.json")).unwrap();
        let before_timestamp = read_timestamp(&fixture.repo_path("metadata/timestamp.json"));

        let mut options = fixture.options(Vec::new());
        options.refresh = true;
        let outcome = publish_static_repo(options).unwrap();

        assert_eq!(outcome.root_version, 1);
        assert_eq!(outcome.targets_version, 1);
        assert_eq!(outcome.snapshot_version, 1);
        assert_eq!(outcome.timestamp_version, 2);
        assert_eq!(
            fs::read(fixture.repo_path("index.json")).unwrap(),
            before_index
        );
        assert_eq!(
            fs::read(fixture.repo_path("metadata/targets.json")).unwrap(),
            before_targets
        );
        assert_eq!(
            fs::read(fixture.repo_path("metadata/snapshot.json")).unwrap(),
            before_snapshot
        );
        assert_eq!(
            read_timestamp(&fixture.repo_path("metadata/timestamp.json"))
                .signed
                .version,
            before_timestamp.signed.version + 1
        );
    }

    #[test]
    fn forced_targets_refresh_cascades_to_snapshot_timestamp_and_preserves_packages() {
        let fixture = PublishFixture::new();
        let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
        publish_static_repo(fixture.options(vec![package])).unwrap();

        let before_index = read_index(&fixture.repo_path("index.json"));
        let before_targets = read_targets(&fixture.repo_path("metadata/targets.json"));
        let before_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));

        let mut options = fixture.options(Vec::new());
        options.refresh = true;
        let outcome = publish_static_repo_with_forced_refresh_for_test(
            options,
            ForcedRefreshForTest {
                root: false,
                targets: true,
                snapshot: false,
            },
        )
        .unwrap();

        let after_index = read_index(&fixture.repo_path("index.json"));
        let after_targets = read_targets(&fixture.repo_path("metadata/targets.json"));
        let after_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));

        assert_eq!(outcome.root_version, 1);
        assert_eq!(outcome.targets_version, before_targets.signed.version + 1);
        assert_eq!(outcome.snapshot_version, before_snapshot.signed.version + 1);
        assert_eq!(outcome.timestamp_version, 2);
        assert_eq!(after_index.index_version, after_targets.signed.version);
        assert_ne!(after_index.generated, before_index.generated);
        assert_eq!(after_index.packages.len(), before_index.packages.len());
        assert_eq!(after_index.packages[0].path, before_index.packages[0].path);
        assert_eq!(
            after_index.packages[0].sha256,
            before_index.packages[0].sha256
        );
        assert_eq!(
            after_snapshot.signed.meta["targets.json"].version,
            after_targets.signed.version
        );
    }

    #[test]
    fn refresh_selects_near_expiry_targets_and_preserves_packages() {
        let fixture = PublishFixture::new();
        let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
        publish_static_repo(fixture.options(vec![package])).unwrap();

        let before_index = read_index(&fixture.repo_path("index.json"));
        let before_root = read_root(&fixture.repo_path("metadata/root.json"));
        let before_targets = read_targets(&fixture.repo_path("metadata/targets.json"));
        let before_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));
        set_targets_expiry(&fixture, Utc::now() + Duration::days(10));

        let mut options = fixture.options(Vec::new());
        options.refresh = true;
        let outcome = publish_static_repo(options).unwrap();

        let after_index = read_index(&fixture.repo_path("index.json"));
        let after_root = read_root(&fixture.repo_path("metadata/root.json"));
        let after_targets = read_targets(&fixture.repo_path("metadata/targets.json"));
        let after_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));

        assert_eq!(outcome.root_version, before_root.signed.version);
        assert_eq!(outcome.targets_version, before_targets.signed.version + 1);
        assert_eq!(outcome.snapshot_version, before_snapshot.signed.version + 1);
        assert_eq!(outcome.timestamp_version, 2);
        assert_eq!(after_root.signed.version, before_root.signed.version);
        assert_eq!(after_index.index_version, after_targets.signed.version);
        assert_ne!(after_index.generated, before_index.generated);
        assert_eq!(
            serde_json::to_value(&after_index.packages).unwrap(),
            serde_json::to_value(&before_index.packages).unwrap()
        );
        assert_eq!(
            after_snapshot.signed.meta["targets.json"].version,
            after_targets.signed.version
        );
    }

    #[test]
    fn forced_root_refresh_cascades_to_snapshot_and_timestamp() {
        let fixture = PublishFixture::new();
        let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
        publish_static_repo(fixture.options(vec![package])).unwrap();

        let before_root = read_root(&fixture.repo_path("metadata/root.json"));
        let before_targets = read_targets(&fixture.repo_path("metadata/targets.json"));
        let before_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));

        let mut options = fixture.options(Vec::new());
        options.refresh = true;
        let outcome = publish_static_repo_with_forced_refresh_for_test(
            options,
            ForcedRefreshForTest {
                root: true,
                targets: false,
                snapshot: false,
            },
        )
        .unwrap();

        let after_root = read_root(&fixture.repo_path("metadata/root.json"));
        let after_targets = read_targets(&fixture.repo_path("metadata/targets.json"));
        let after_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));

        assert_eq!(outcome.root_version, before_root.signed.version + 1);
        assert_eq!(outcome.targets_version, before_targets.signed.version);
        assert_eq!(outcome.snapshot_version, before_snapshot.signed.version + 1);
        assert_eq!(outcome.timestamp_version, 2);
        assert_eq!(
            after_root.signed.roles["root"].keyids,
            before_root.signed.roles["root"].keyids
        );
        assert_eq!(after_targets.signed.version, before_targets.signed.version);
        assert_eq!(
            after_snapshot.signed.meta["root.json"].version,
            after_root.signed.version
        );
    }

    #[test]
    fn refresh_selects_near_expiry_root_and_cascades_without_targets_bump() {
        let fixture = PublishFixture::new();
        let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
        publish_static_repo(fixture.options(vec![package])).unwrap();

        let before_root = read_root(&fixture.repo_path("metadata/root.json"));
        let before_targets = read_targets(&fixture.repo_path("metadata/targets.json"));
        let before_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));
        set_root_expiry(&fixture, Utc::now() + Duration::days(80));

        let mut options = fixture.options(Vec::new());
        options.refresh = true;
        let outcome = publish_static_repo(options).unwrap();

        let after_root = read_root(&fixture.repo_path("metadata/root.json"));
        let after_targets = read_targets(&fixture.repo_path("metadata/targets.json"));
        let after_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));

        assert_eq!(outcome.root_version, before_root.signed.version + 1);
        assert_eq!(outcome.targets_version, before_targets.signed.version);
        assert_eq!(outcome.snapshot_version, before_snapshot.signed.version + 1);
        assert_eq!(outcome.timestamp_version, 2);
        assert_eq!(after_targets.signed.version, before_targets.signed.version);
        assert_eq!(
            after_snapshot.signed.meta["root.json"].version,
            after_root.signed.version
        );
    }

    #[test]
    fn refresh_repairs_expired_snapshot_destination_metadata() {
        let fixture = PublishFixture::new();
        let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
        publish_static_repo(fixture.options(vec![package])).unwrap();
        let before_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));
        set_snapshot_expiry(&fixture, Utc::now() - Duration::hours(1));

        let mut options = fixture.options(Vec::new());
        options.refresh = true;
        let outcome = publish_static_repo(options).unwrap();

        let after_snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));
        let after_timestamp = read_timestamp(&fixture.repo_path("metadata/timestamp.json"));

        assert_eq!(outcome.snapshot_version, before_snapshot.signed.version + 1);
        assert!(after_snapshot.signed.expires > Utc::now());
        assert!(after_timestamp.signed.expires > Utc::now());
    }

    #[test]
    fn state_file_watermark_rejects_destination_version_regression() {
        let fixture = PublishFixture::new();
        let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
        publish_static_repo(fixture.options(vec![package])).unwrap();
        fs::write(
            &fixture.state_file,
            "root_version = 1\ntargets_version = 5\nsnapshot_version = 5\ntimestamp_version = 5\n",
        )
        .unwrap();

        let mut options = fixture.options(Vec::new());
        options.refresh = true;
        let error = publish_static_repo(options).unwrap_err();

        assert!(
            error.to_string().contains("destination versions")
                && error.to_string().contains("watermark"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn force_reinit_allows_unverifiable_destination_with_new_identity_warning() {
        let fixture = PublishFixture::new();
        fs::create_dir_all(fixture.repo_path("metadata")).unwrap();
        fs::write(fixture.repo_path("metadata/timestamp.json"), b"not json").unwrap();
        let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");

        let mut options = fixture.options(vec![package]);
        options.force_reinit = true;
        let outcome = publish_static_repo(options).unwrap();

        assert_eq!(outcome.root_version, 1);
        assert!(
            outcome.preview_warning.contains("repo reset-trust")
                && outcome
                    .preview_warning
                    .contains("re-pin the new repo fingerprint"),
            "unexpected warning: {}",
            outcome.preview_warning
        );
        assert!(fixture.repo_path("metadata/root.json").exists());
    }

    #[test]
    fn force_reinit_ignores_old_watermark_and_writes_new_watermark() {
        let fixture = PublishFixture::new();
        let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
        publish_static_repo(fixture.options(vec![package])).unwrap();
        fs::write(
            &fixture.state_file,
            "root_version = 9\ntargets_version = 9\nsnapshot_version = 9\ntimestamp_version = 9\n",
        )
        .unwrap();
        fs::write(fixture.repo_path("metadata/timestamp.json"), b"not json").unwrap();

        let mut options = fixture.options(Vec::new());
        options.force_reinit = true;
        let outcome = publish_static_repo(options).unwrap();

        assert_eq!(outcome.root_version, 1);
        assert_eq!(outcome.targets_version, 1);
        assert_eq!(outcome.snapshot_version, 1);
        assert_eq!(outcome.timestamp_version, 1);
        let watermark = fs::read_to_string(&fixture.state_file).unwrap();
        assert!(watermark.contains("root_version = 1"));
        assert!(watermark.contains("targets_version = 1"));
        assert!(watermark.contains("snapshot_version = 1"));
        assert!(watermark.contains("timestamp_version = 1"));
    }

    #[test]
    fn publish_key_rotation_updates_roles_and_retires_old_package_key() {
        let fixture = PublishFixture::new();
        let package = fixture.build_package("widget", "1.0.0", "x86_64", b"hello\n");
        publish_static_repo(fixture.options(vec![package])).unwrap();
        let before_root = read_root(&fixture.repo_path("metadata/root.json"));
        let before_keys = read_package_keys(&fixture.repo_path("keys/package-keys.json"));
        let old_public_key = before_keys.keys[0].public_key.clone();

        let mut options = fixture.options(Vec::new());
        options.rotate_publish_key = true;
        let outcome = publish_static_repo(options).unwrap();

        let after_root = read_root(&fixture.repo_path("metadata/root.json"));
        let after_keys = read_package_keys(&fixture.repo_path("keys/package-keys.json"));

        assert_eq!(outcome.root_version, before_root.signed.version + 1);
        assert_eq!(
            after_root.signed.roles["root"].keyids,
            before_root.signed.roles["root"].keyids
        );
        for role in ["targets", "snapshot", "timestamp"] {
            assert_ne!(
                after_root.signed.roles[role].keyids,
                before_root.signed.roles[role].keyids
            );
            assert_eq!(
                after_root.signed.roles[role].keyids,
                after_root.signed.roles["targets"].keyids
            );
        }
        assert!(after_keys.keys.iter().any(|key| {
            key.public_key == old_public_key && matches!(key.status, PackageKeyStatus::Retired)
        }));
        assert!(after_keys.keys.iter().any(|key| {
            key.public_key != old_public_key && matches!(key.status, PackageKeyStatus::Active)
        }));
    }

    #[test]
    fn failed_publish_key_rotation_keeps_active_key_files_unchanged() {
        let fixture = PublishFixture::new();
        let first = fixture.build_package("widget", "1.0.0", "x86_64", b"first\n");
        publish_static_repo(fixture.options(vec![first])).unwrap();
        let before_private = fs::read(fixture.key_dir.join("publish.private")).unwrap();
        let before_public = fs::read(fixture.key_dir.join("publish.public")).unwrap();
        let before_key =
            SigningKeyPair::load_from_file(&fixture.key_dir.join("publish.private")).unwrap();

        let conflicting = fixture.build_package("widget", "1.0.0", "x86_64", b"second\n");
        let mut options = fixture.options(vec![conflicting]);
        options.rotate_publish_key = true;
        let error = publish_static_repo(options).unwrap_err();

        assert!(
            error.to_string().contains("immutable package artifact"),
            "unexpected error: {error}"
        );
        assert_eq!(
            fs::read(fixture.key_dir.join("publish.private")).unwrap(),
            before_private
        );
        assert_eq!(
            fs::read(fixture.key_dir.join("publish.public")).unwrap(),
            before_public
        );
        let after_key =
            SigningKeyPair::load_from_file(&fixture.key_dir.join("publish.private")).unwrap();
        assert_eq!(
            after_key.public_key_base64(),
            before_key.public_key_base64()
        );
    }

    #[test]
    fn package_staging_keeps_rotation_artifact_pending_until_promoted() {
        let fixture = PublishFixture::new();
        let first = fixture.build_package("widget", "1.0.0", "x86_64", b"first\n");
        publish_static_repo(fixture.options(vec![first])).unwrap();

        let pending_key = SigningKeyPair::generate().with_key_id("publish");
        let staged = fixture.build_package("gadget", "1.0.0", "x86_64", b"pending\n");
        let mut pending = stage_packages(
            &fixture.repo_dir,
            std::slice::from_ref(&staged),
            &pending_key,
        )
        .unwrap();
        let entries = pending.package_entries();
        let relative = entries[0].path.clone();
        let pending_path = pending.writes[0].pending_path.clone();
        let final_path = fixture.repo_path(&relative);

        assert!(pending_path.exists());
        assert!(
            !final_path.exists(),
            "package staging must not expose final immutable path before commit"
        );

        pending.promote().unwrap();
        assert!(final_path.exists());
        pending.commit();
    }

    #[test]
    fn publish_recovers_pending_publish_key_after_repo_commit_before_key_promotion() {
        let fixture = PublishFixture::new();
        let package = fixture.build_package("widget", "1.0.0", "x86_64", b"first\n");
        publish_static_repo(fixture.options(vec![package])).unwrap();
        let old_private = fs::read(fixture.key_dir.join("publish.private")).unwrap();
        let old_public = fs::read(fixture.key_dir.join("publish.public")).unwrap();
        let old_timestamp = fs::read(fixture.repo_path("metadata/timestamp.json")).unwrap();
        let old_watermark = fs::read(&fixture.state_file).unwrap();

        let mut rotate = fixture.options(Vec::new());
        rotate.rotate_publish_key = true;
        let rotated = publish_static_repo(rotate).unwrap();
        let rotated_key =
            SigningKeyPair::load_from_file(&fixture.key_dir.join("publish.private")).unwrap();
        save_key_pair(&rotated_key, &fixture.key_dir, "publish.pending").unwrap();
        fs::write(fixture.key_dir.join("publish.private"), old_private).unwrap();
        fs::write(fixture.key_dir.join("publish.public"), old_public).unwrap();
        fs::write(fixture.repo_path("metadata/timestamp.json"), old_timestamp).unwrap();
        fs::write(&fixture.state_file, old_watermark).unwrap();

        let mut retry = fixture.options(Vec::new());
        retry.rotate_publish_key = true;
        let recovered = publish_static_repo(retry).unwrap();

        let active_key =
            SigningKeyPair::load_from_file(&fixture.key_dir.join("publish.private")).unwrap();
        assert_eq!(
            active_key.public_key_base64(),
            rotated_key.public_key_base64()
        );
        assert_eq!(recovered.root_version, rotated.root_version);
        assert!(!fixture.key_dir.join("publish.pending.private").exists());
        assert!(!fixture.key_dir.join("publish.pending.public").exists());
    }

    #[test]
    fn publish_lock_rejects_second_local_holder() {
        let fixture = PublishFixture::new();
        fs::create_dir_all(&fixture.repo_dir).unwrap();
        let _first = try_acquire_publish_lock_for_test(&fixture.repo_dir).unwrap();

        let error = try_acquire_publish_lock_for_test(&fixture.repo_dir).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("another static repo publish is already running"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn atomic_temp_paths_are_unique_next_to_destination() {
        let temp_dir = tempfile::tempdir().unwrap();
        let destination = temp_dir.path().join("metadata/timestamp.json");

        let first = unique_atomic_temp_path(&destination);
        let second = unique_atomic_temp_path(&destination);

        assert_ne!(first, second);
        assert_eq!(first.parent(), destination.parent());
        assert_eq!(second.parent(), destination.parent());
    }

    struct PublishFixture {
        _temp: TempDir,
        repo_dir: PathBuf,
        key_dir: PathBuf,
        state_file: PathBuf,
        package_dir: PathBuf,
    }

    impl PublishFixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let repo_dir = temp.path().join("repo");
            let key_dir = temp.path().join("keys");
            let state_file = temp.path().join("last-published.toml");
            let package_dir = temp.path().join("packages");
            fs::create_dir_all(&package_dir).unwrap();

            Self {
                _temp: temp,
                repo_dir,
                key_dir,
                state_file,
                package_dir,
            }
        }

        fn options(&self, package_paths: Vec<PathBuf>) -> StaticPublishOptions {
            StaticPublishOptions {
                repo_name: "test-repo".to_string(),
                repo_description: Some("test static repo".to_string()),
                destination: RepoLocation::File {
                    root: self.repo_dir.clone(),
                },
                key_dir: self.key_dir.clone(),
                state_file: self.state_file.clone(),
                package_paths,
                refresh: false,
                force_reinit: false,
                accept_destination_state: false,
                rotate_publish_key: false,
                rotate_root_key: false,
            }
        }

        fn repo_path(&self, relative: &str) -> PathBuf {
            self.repo_dir.join(relative)
        }

        fn build_package(&self, name: &str, version: &str, arch: &str, content: &[u8]) -> PathBuf {
            let source_dir = self
                .package_dir
                .join(format!("{name}-{version}-{arch}-src"));
            fs::create_dir_all(source_dir.join("usr/share")).unwrap();
            fs::write(source_dir.join("usr/share/payload"), content).unwrap();
            let manifest = CcsManifest::parse(&format!(
                r#"
[package]
name = "{name}"
version = "{version}"
description = "fixture package"
license = "MIT"

[package.platform]
arch = "{arch}"
"#
            ))
            .unwrap();
            let result = CcsBuilder::new(manifest, &source_dir).build().unwrap();
            let package_path = self
                .package_dir
                .join(format!("{name}-{version}-{arch}-input.ccs"));
            write_ccs_package(&result, &package_path).unwrap();

            let parsed =
                crate::ccs::package::CcsPackage::parse(package_path.to_str().unwrap()).unwrap();
            assert_eq!(parsed.name(), name);
            assert_eq!(parsed.version(), version);
            assert_eq!(parsed.architecture(), Some(arch));

            package_path
        }
    }

    fn read_identity(path: &Path) -> RepoIdentity {
        RepoIdentity::parse(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn read_index(path: &Path) -> StaticIndex {
        StaticIndex::parse(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn read_package_keys(path: &Path) -> PackageKeysFile {
        PackageKeysFile::parse(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn read_root(path: &Path) -> Signed<RootMetadata> {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    fn read_targets(path: &Path) -> Signed<TargetsMetadata> {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    fn read_snapshot(path: &Path) -> Signed<SnapshotMetadata> {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    fn read_timestamp(path: &Path) -> Signed<TimestampMetadata> {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    fn set_root_expiry(fixture: &PublishFixture, expires: chrono::DateTime<Utc>) {
        let root_key =
            SigningKeyPair::load_from_file(&fixture.key_dir.join("root.private")).unwrap();
        let mut root = read_root(&fixture.repo_path("metadata/root.json"));
        root.signed.expires = expires;
        root.signatures = vec![sign_tuf_metadata(&root_key, &root.signed).unwrap()];
        write_json(&fixture.repo_path("metadata/root.json"), &root);
        write_json(
            &fixture.repo_path(&format!("metadata/{}.root.json", root.signed.version)),
            &root,
        );
    }

    fn set_targets_expiry(fixture: &PublishFixture, expires: chrono::DateTime<Utc>) {
        let publish_key =
            SigningKeyPair::load_from_file(&fixture.key_dir.join("publish.private")).unwrap();
        let mut targets = read_targets(&fixture.repo_path("metadata/targets.json"));
        targets.signed.expires = expires;
        targets.signatures = vec![sign_tuf_metadata(&publish_key, &targets.signed).unwrap()];
        write_json(&fixture.repo_path("metadata/targets.json"), &targets);
        repin_snapshot_targets_and_timestamp(fixture, &publish_key);
    }

    fn set_snapshot_expiry(fixture: &PublishFixture, expires: chrono::DateTime<Utc>) {
        let publish_key =
            SigningKeyPair::load_from_file(&fixture.key_dir.join("publish.private")).unwrap();
        let mut snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));
        snapshot.signed.expires = expires;
        snapshot.signatures = vec![sign_tuf_metadata(&publish_key, &snapshot.signed).unwrap()];
        write_json(&fixture.repo_path("metadata/snapshot.json"), &snapshot);
        repin_timestamp_snapshot(fixture, &publish_key);
    }

    fn repin_snapshot_targets_and_timestamp(
        fixture: &PublishFixture,
        publish_key: &SigningKeyPair,
    ) {
        let targets_bytes = fs::read(fixture.repo_path("metadata/targets.json")).unwrap();
        let mut snapshot = read_snapshot(&fixture.repo_path("metadata/snapshot.json"));
        let targets_ref = snapshot
            .signed
            .meta
            .get_mut("targets.json")
            .expect("snapshot pins targets");
        targets_ref.length = Some(targets_bytes.len() as u64);
        targets_ref.hashes = Some({
            let mut hashes = std::collections::BTreeMap::new();
            hashes.insert("sha256".to_string(), crate::hash::sha256(&targets_bytes));
            hashes
        });
        snapshot.signatures = vec![sign_tuf_metadata(publish_key, &snapshot.signed).unwrap()];
        write_json(&fixture.repo_path("metadata/snapshot.json"), &snapshot);
        repin_timestamp_snapshot(fixture, publish_key);
    }

    fn repin_timestamp_snapshot(fixture: &PublishFixture, publish_key: &SigningKeyPair) {
        let snapshot_bytes = fs::read(fixture.repo_path("metadata/snapshot.json")).unwrap();
        let mut timestamp = read_timestamp(&fixture.repo_path("metadata/timestamp.json"));
        let snapshot_ref = timestamp
            .signed
            .meta
            .get_mut("snapshot.json")
            .expect("timestamp pins snapshot");
        snapshot_ref.length = Some(snapshot_bytes.len() as u64);
        snapshot_ref.hashes = Some({
            let mut hashes = std::collections::BTreeMap::new();
            hashes.insert("sha256".to_string(), crate::hash::sha256(&snapshot_bytes));
            hashes
        });
        timestamp.signatures = vec![sign_tuf_metadata(publish_key, &timestamp.signed).unwrap()];
        write_json(&fixture.repo_path("metadata/timestamp.json"), &timestamp);
    }

    fn write_json<T: serde::Serialize>(path: &Path, value: &T) {
        fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    }
}
