// apps/remi/src/server/catalog_gc.rs

//! Exact reachability collection for immutable Remi catalog bundles.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use conary_core::db::models::{
    RemiCatalogDeletionIntent, RemiCatalogResource, RemiCatalogResourceKind,
    RemiCatalogRunCandidate, acknowledge_catalog_deletion, delete_catalog_collection,
    plan_catalog_collection,
};
use conary_core::repository::catalog::{
    CATALOG_FILE_NAME, CATALOG_MANIFEST_FILE_NAME, CATALOG_PORTABLE_MANIFEST_FILE_NAME,
    SOURCE_METADATA_DIRECTORY_NAME, portable_chunk_count_v1, portable_manifest_size_v1,
};
use conary_core::repository::{
    acknowledge_profile_sync_candidate_cleanup, recover_expired_profile_sync_runs,
};
use tokio::sync::{Mutex, RwLock};

use super::ServerState;
use super::catalog_authority::CatalogAuthority;
use super::catalog_refresh::cleanup_candidate_run;
use super::database_writer::DatabaseWriter;

/// Exact collection outcome for diagnostics and focused tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogGcReport {
    pub deleted_profile_resources: usize,
    pub deleted_source_resources: usize,
    pub removed_bundles: usize,
    pub acknowledged_deletions: usize,
}

struct CatalogGcTargets {
    pending_deletions: Vec<RemiCatalogDeletionIntent>,
    terminal_candidates: Vec<RemiCatalogRunCandidate>,
    deleted_profile_resources: usize,
    deleted_source_resources: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogBundleDeletionPolicy {
    CurrentOnly,
    CurrentOrRetiredSchema54,
}

/// Fence expired refresh runs after the server has acquired exclusive runtime
/// ownership, remove only their exact durable candidate paths, and acknowledge
/// each absence so recovery remains bounded and replayable.
pub async fn recover_catalog_refresh_runs(state: &Arc<RwLock<ServerState>>) -> Result<usize> {
    let (coordinator, db_path, candidate_dir, database_writer) = {
        let state = state.read().await;
        (
            state.publication_coordinator.clone(),
            state.config.db_path.clone(),
            state.config.catalog_candidate_dir.clone(),
            state.database_writer.clone(),
        )
    };
    let _publication_guard = coordinator.lock_owned().await;
    recover_catalog_refresh_runs_uncoordinated(db_path, candidate_dir, database_writer).await
}

pub(crate) async fn recover_catalog_refresh_runs_uncoordinated(
    db_path: PathBuf,
    candidate_dir: PathBuf,
    database_writer: DatabaseWriter,
) -> Result<usize> {
    tokio::task::spawn_blocking(move || {
        let recoveries = database_writer
            .execute(|| {
                let conn = conary_core::db::open_fast(&db_path)?;
                recover_expired_profile_sync_runs(&conn)
            })
            .map_err(anyhow::Error::from)?;
        for recovery in &recoveries {
            cleanup_candidate_run(&candidate_dir, &recovery.run_id).with_context(|| {
                format!(
                    "remove exact terminal catalog candidate run {} for profile {}",
                    recovery.run_id, recovery.source_profile
                )
            })?;
            let acknowledged = database_writer
                .execute(|| {
                    let conn = conary_core::db::open_fast(&db_path)?;
                    acknowledge_profile_sync_candidate_cleanup(&conn, &recovery.run_id)
                })
                .map_err(anyhow::Error::from)?;
            if !acknowledged {
                bail!(
                    "terminal catalog candidate run {} was not pending cleanup acknowledgement",
                    recovery.run_id
                );
            }
        }
        Ok(recoveries.len())
    })
    .await
    .context("catalog refresh recovery task panicked")?
}

/// Collect catalogs at startup or from another caller that does not already
/// own the complete publication coordinator.
pub async fn collect_catalog_garbage(state: &Arc<RwLock<ServerState>>) -> Result<CatalogGcReport> {
    let (coordinator, db_path, catalog_dir, database_writer, catalog_authority) = {
        let state = state.read().await;
        (
            state.publication_coordinator.clone(),
            state.config.db_path.clone(),
            state.config.catalog_dir.clone(),
            state.database_writer.clone(),
            state.catalog_authority.clone(),
        )
    };
    let _publication_guard = coordinator.lock_owned().await;
    collect_catalog_garbage_uncoordinated(db_path, catalog_dir, database_writer, catalog_authority)
        .await
}

/// Collect catalogs while the caller owns the complete publication
/// coordinator. Database mutations still pass through the shared writer.
pub(crate) async fn collect_catalog_garbage_uncoordinated(
    db_path: PathBuf,
    catalog_dir: PathBuf,
    database_writer: DatabaseWriter,
    catalog_authority: CatalogAuthority,
) -> Result<CatalogGcReport> {
    let targets = tokio::task::spawn_blocking({
        let db_path = db_path.clone();
        let database_writer = database_writer.clone();
        move || {
            database_writer
                .execute(|| {
                    let conn = conary_core::db::open_fast(&db_path)?;
                    let plan = plan_catalog_collection(&conn)?;
                    let deleted = delete_catalog_collection(&conn, &plan)?;
                    let current = plan_catalog_collection(&conn)?;
                    let pending_keys = current
                        .pending_deletions
                        .iter()
                        .map(deletion_key)
                        .collect::<BTreeSet<_>>();
                    let mut terminal_candidates = Vec::new();
                    let mut seen = BTreeSet::new();
                    for candidate in current
                        .run_candidates
                        .iter()
                        .filter(|item| !item.nonterminal)
                    {
                        let reachable = match candidate.resource_kind {
                            RemiCatalogResourceKind::ProfileRevision => current
                                .reachability
                                .contains_profile_revision(&candidate.resource_sha256),
                            RemiCatalogResourceKind::SourceSnapshot => current
                                .reachability
                                .contains_source_snapshot(&candidate.resource_sha256),
                        };
                        let key = candidate_key(candidate);
                        if reachable
                            || pending_keys.contains(&key)
                            || !seen.insert(key)
                            || RemiCatalogResource::find_by_sha256(
                                &conn,
                                &candidate.resource_sha256,
                            )?
                            .is_some()
                        {
                            continue;
                        }
                        terminal_candidates.push(candidate.clone());
                    }
                    Ok::<_, conary_core::Error>(CatalogGcTargets {
                        pending_deletions: current.pending_deletions,
                        terminal_candidates,
                        deleted_profile_resources: deleted.deleted_profile_resources.len(),
                        deleted_source_resources: deleted.deleted_source_resources.len(),
                    })
                })
                .map_err(anyhow::Error::from)
        }
    })
    .await
    .context("catalog metadata collection task panicked")??;

    let pending_deletions = targets.pending_deletions.clone();
    let terminal_candidates = targets.terminal_candidates.clone();
    let (removed_bundles, acknowledged_deletions) =
        tokio::task::spawn_blocking(move || -> Result<(usize, Vec<RemiCatalogDeletionIntent>)> {
            let mut removed = 0;
            let mut acknowledged = Vec::with_capacity(pending_deletions.len());
            for intent in pending_deletions {
                removed += usize::from(remove_exact_bundle_and_evict_reader(
                    &catalog_dir,
                    intent.resource_kind,
                    &intent.source_profile,
                    &intent.resource_sha256,
                    CatalogBundleDeletionPolicy::CurrentOnly,
                    &catalog_authority,
                )?);
                acknowledged.push(intent);
            }
            for candidate in terminal_candidates {
                removed += usize::from(remove_exact_bundle_and_evict_reader(
                    &catalog_dir,
                    candidate.resource_kind,
                    &candidate.source_profile,
                    &candidate.resource_sha256,
                    CatalogBundleDeletionPolicy::CurrentOrRetiredSchema54,
                    &catalog_authority,
                )?);
            }
            Ok((removed, acknowledged))
        })
        .await
        .context("catalog filesystem collection task panicked")??;

    let acknowledged_count = acknowledged_deletions.len();
    if !acknowledged_deletions.is_empty() {
        tokio::task::spawn_blocking(move || {
            database_writer
                .execute(|| {
                    let conn = conary_core::db::open_fast(&db_path)?;
                    for intent in &acknowledged_deletions {
                        if !acknowledge_catalog_deletion(&conn, intent)? {
                            return Err(conary_core::Error::ConflictError(format!(
                                "catalog deletion intent {} disappeared before acknowledgement",
                                intent.resource_sha256
                            )));
                        }
                    }
                    Ok::<(), conary_core::Error>(())
                })
                .map_err(anyhow::Error::from)
        })
        .await
        .context("catalog deletion acknowledgement task panicked")??;
    }

    Ok(CatalogGcReport {
        deleted_profile_resources: targets.deleted_profile_resources,
        deleted_source_resources: targets.deleted_source_resources,
        removed_bundles,
        acknowledged_deletions: acknowledged_count,
    })
}

/// Serialize an exact collection inside a publication cycle whose profile
/// refresh jobs may otherwise run concurrently.
pub(crate) async fn collect_catalog_garbage_serialized(
    coordinator: Arc<Mutex<()>>,
    db_path: PathBuf,
    catalog_dir: PathBuf,
    database_writer: DatabaseWriter,
    catalog_authority: CatalogAuthority,
) -> Result<CatalogGcReport> {
    let _collection_guard = coordinator.lock_owned().await;
    collect_catalog_garbage_uncoordinated(db_path, catalog_dir, database_writer, catalog_authority)
        .await
}

fn deletion_key(intent: &RemiCatalogDeletionIntent) -> (RemiCatalogResourceKind, String, String) {
    (
        intent.resource_kind,
        intent.source_profile.clone(),
        intent.resource_sha256.clone(),
    )
}

fn candidate_key(candidate: &RemiCatalogRunCandidate) -> (RemiCatalogResourceKind, String, String) {
    (
        candidate.resource_kind,
        candidate.source_profile.clone(),
        candidate.resource_sha256.clone(),
    )
}

#[cfg(test)]
fn bundle_path(
    catalog_root: &Path,
    kind: RemiCatalogResourceKind,
    source_profile: &str,
    resource_sha256: &str,
) -> PathBuf {
    match kind {
        RemiCatalogResourceKind::SourceSnapshot => {
            catalog_root.join("sources").join(resource_sha256)
        }
        RemiCatalogResourceKind::ProfileRevision => catalog_root
            .join("profiles")
            .join(source_profile)
            .join(resource_sha256),
    }
}

/// Atomically move one exact immutable bundle into a private deterministic GC
/// tombstone, then remove that tombstone. A crash at either side of the rename
/// is retryable from the durable database/run journal.
fn remove_exact_bundle(
    catalog_root: &Path,
    kind: RemiCatalogResourceKind,
    source_profile: &str,
    resource_sha256: &str,
    deletion_policy: CatalogBundleDeletionPolicy,
) -> Result<bool> {
    require_storage_component(source_profile, "catalog deletion source profile")?;
    require_digest(resource_sha256, "catalog deletion resource digest")?;
    let tombstone = tombstone_path(catalog_root, kind, source_profile, resource_sha256);
    let Some(parent) = require_bundle_parent(catalog_root, kind, source_profile)? else {
        return remove_gc_tombstone(&tombstone);
    };
    let path = parent.join(resource_sha256);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return remove_gc_tombstone(&tombstone);
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        bail!(
            "catalog deletion target {} is not a real directory",
            path.display()
        );
    }
    validate_exact_bundle_deletion_layout(&path, kind, resource_sha256, deletion_policy)?;
    if fs::symlink_metadata(&tombstone).is_ok() {
        bail!(
            "catalog deletion target {} and tombstone {} both exist",
            path.display(),
            tombstone.display()
        );
    }
    let tombstone_parent = ensure_tombstone_parent(catalog_root, kind, source_profile)?;
    fs::rename(&path, &tombstone).with_context(|| {
        format!(
            "atomically move catalog deletion target {} to {}",
            path.display(),
            tombstone.display()
        )
    })?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    File::open(&tombstone_parent)?.sync_all()?;
    remove_gc_tombstone(&tombstone)
}

fn remove_exact_bundle_and_evict_reader(
    catalog_root: &Path,
    kind: RemiCatalogResourceKind,
    source_profile: &str,
    resource_sha256: &str,
    deletion_policy: CatalogBundleDeletionPolicy,
    catalog_authority: &CatalogAuthority,
) -> Result<bool> {
    let removed = remove_exact_bundle(
        catalog_root,
        kind,
        source_profile,
        resource_sha256,
        deletion_policy,
    )?;
    match kind {
        RemiCatalogResourceKind::ProfileRevision => {
            catalog_authority.evict_removed_profile_catalog(source_profile, resource_sha256);
        }
        RemiCatalogResourceKind::SourceSnapshot => {
            catalog_authority.evict_removed_source_catalog(source_profile, resource_sha256);
        }
    }
    Ok(removed)
}

fn require_bundle_parent(
    catalog_root: &Path,
    kind: RemiCatalogResourceKind,
    source_profile: &str,
) -> Result<Option<PathBuf>> {
    require_real_directory(catalog_root, "catalog root")?;
    match kind {
        RemiCatalogResourceKind::SourceSnapshot => {
            let sources = catalog_root.join("sources");
            require_real_directory(&sources, "source catalog parent")?;
            Ok(Some(sources))
        }
        RemiCatalogResourceKind::ProfileRevision => {
            let profiles = catalog_root.join("profiles");
            require_real_directory(&profiles, "profile catalog parent")?;
            let profile = profiles.join(source_profile);
            optional_real_directory(&profile, "profile revision catalog parent")
                .map(|exists| exists.then_some(profile))
        }
    }
}

fn optional_real_directory(path: &Path, label: &str) -> Result<bool> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("inspect {label} {}", path.display()));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        bail!("{label} {} is not a real directory", path.display());
    }
    Ok(true)
}

fn require_real_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        bail!("{label} {} is not a real directory", path.display());
    }
    Ok(())
}

fn require_storage_component(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        bail!("{label} is not a safe ASCII storage-path component");
    }
    Ok(())
}

fn require_digest(value: &str, label: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("{label} is not an exact lowercase SHA-256 digest");
    }
    Ok(())
}

/// Accept only the exact current bundle layout or the exact layout retired by
/// the schema-55 hard cut. The retired shape is permitted only for an exact
/// unregistered terminal run candidate; ordinary schema-55 deletion intents,
/// serving, and reuse continue to require the current portable proof sidecar.
fn validate_exact_bundle_deletion_layout(
    path: &Path,
    kind: RemiCatalogResourceKind,
    resource_sha256: &str,
    deletion_policy: CatalogBundleDeletionPolicy,
) -> Result<()> {
    let mut names = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    names.sort();
    let mut retired_expected = vec![
        std::ffi::OsString::from(CATALOG_FILE_NAME),
        std::ffi::OsString::from(CATALOG_MANIFEST_FILE_NAME),
    ];
    if kind == RemiCatalogResourceKind::SourceSnapshot {
        retired_expected.push(std::ffi::OsString::from(SOURCE_METADATA_DIRECTORY_NAME));
    }
    retired_expected.sort();
    let mut current_expected = retired_expected.clone();
    current_expected.push(std::ffi::OsString::from(
        CATALOG_PORTABLE_MANIFEST_FILE_NAME,
    ));
    current_expected.sort();
    let has_portable_manifest = if names == current_expected {
        true
    } else if deletion_policy == CatalogBundleDeletionPolicy::CurrentOrRetiredSchema54
        && names == retired_expected
    {
        false
    } else {
        bail!(
            "catalog deletion target {} does not have a permitted exact bundle layout",
            path.display()
        );
    };
    for name in [CATALOG_FILE_NAME, CATALOG_MANIFEST_FILE_NAME] {
        let child = path.join(name);
        let child_metadata = fs::symlink_metadata(&child)?;
        if child_metadata.file_type().is_symlink() || !child_metadata.file_type().is_file() {
            bail!(
                "catalog deletion target child {} is not a regular file",
                child.display()
            );
        }
    }
    let manifest_path = path.join(CATALOG_MANIFEST_FILE_NAME);
    let mut manifest_file = File::open(&manifest_path)
        .with_context(|| format!("open catalog deletion manifest {}", manifest_path.display()))?;
    let manifest_sha256 = conary_core::hash::hash_reader(
        conary_core::hash::HashAlgorithm::Sha256,
        &mut manifest_file,
    )
    .with_context(|| format!("hash catalog deletion manifest {}", manifest_path.display()))?
    .value;
    if manifest_sha256 != resource_sha256 {
        bail!(
            "catalog deletion manifest {} does not match its journaled resource digest",
            manifest_path.display()
        );
    }
    if has_portable_manifest {
        let portable_path = path.join(CATALOG_PORTABLE_MANIFEST_FILE_NAME);
        let portable_metadata = fs::symlink_metadata(&portable_path)?;
        if portable_metadata.file_type().is_symlink() || !portable_metadata.file_type().is_file() {
            bail!(
                "catalog deletion target child {} is not a regular file",
                portable_path.display()
            );
        }
        let catalog_size = fs::metadata(path.join(CATALOG_FILE_NAME))?.len();
        let expected_portable_size =
            portable_manifest_size_v1(portable_chunk_count_v1(catalog_size)?)?;
        if portable_metadata.len() != expected_portable_size {
            bail!(
                "catalog deletion target portable manifest {} has {} bytes; expected {}",
                portable_path.display(),
                portable_metadata.len(),
                expected_portable_size
            );
        }
    }
    if kind == RemiCatalogResourceKind::SourceSnapshot {
        validate_source_metadata_layout(&path.join(SOURCE_METADATA_DIRECTORY_NAME))?;
    }
    Ok(())
}

fn validate_source_metadata_layout(path: &Path) -> Result<()> {
    require_real_directory(path, "source catalog native-metadata directory")?;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("source metadata object has a non-UTF-8 name"))?;
        require_digest(&name, "source metadata object digest")?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            bail!(
                "source metadata object {} is not a regular file",
                entry.path().display()
            );
        }
    }
    Ok(())
}

fn tombstone_path(
    catalog_root: &Path,
    kind: RemiCatalogResourceKind,
    source_profile: &str,
    resource_sha256: &str,
) -> PathBuf {
    match kind {
        RemiCatalogResourceKind::SourceSnapshot => catalog_root
            .join(".gc")
            .join("sources")
            .join(resource_sha256),
        RemiCatalogResourceKind::ProfileRevision => catalog_root
            .join(".gc")
            .join("profiles")
            .join(source_profile)
            .join(resource_sha256),
    }
}

fn ensure_tombstone_parent(
    catalog_root: &Path,
    kind: RemiCatalogResourceKind,
    source_profile: &str,
) -> Result<PathBuf> {
    let gc = ensure_real_subdirectory(catalog_root, ".gc")?;
    match kind {
        RemiCatalogResourceKind::SourceSnapshot => ensure_real_subdirectory(&gc, "sources"),
        RemiCatalogResourceKind::ProfileRevision => {
            let profiles = ensure_real_subdirectory(&gc, "profiles")?;
            ensure_real_subdirectory(&profiles, source_profile)
        }
    }
}

fn ensure_real_subdirectory(parent: &Path, name: &str) -> Result<PathBuf> {
    let parent_metadata = fs::symlink_metadata(parent)?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.file_type().is_dir() {
        bail!(
            "catalog GC parent {} is not a real directory",
            parent.display()
        );
    }
    let path = parent.join(name);
    match fs::create_dir(&path) {
        Ok(()) => File::open(parent)?.sync_all()?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        bail!(
            "catalog GC directory {} is not a real directory",
            path.display()
        );
    }
    Ok(path)
}

fn remove_gc_tombstone(path: &Path) -> Result<bool> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        bail!(
            "catalog GC tombstone {} is not a real directory",
            path.display()
        );
    }
    fs::remove_dir_all(path)?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
