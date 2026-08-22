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
use conary_core::repository::catalog::{CATALOG_FILE_NAME, CATALOG_MANIFEST_FILE_NAME};
use conary_core::repository::{
    acknowledge_profile_sync_candidate_cleanup, recover_expired_profile_sync_runs,
};
use tokio::sync::RwLock;

use super::ServerState;
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
    let (coordinator, db_path, catalog_dir, database_writer) = {
        let state = state.read().await;
        (
            state.publication_coordinator.clone(),
            state.config.db_path.clone(),
            state.config.catalog_dir.clone(),
            state.database_writer.clone(),
        )
    };
    let _publication_guard = coordinator.lock_owned().await;
    collect_catalog_garbage_uncoordinated(db_path, catalog_dir, database_writer).await
}

/// Collect catalogs while the caller owns the complete publication
/// coordinator. Database mutations still pass through the shared writer.
pub(crate) async fn collect_catalog_garbage_uncoordinated(
    db_path: PathBuf,
    catalog_dir: PathBuf,
    database_writer: DatabaseWriter,
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
                removed += usize::from(remove_exact_bundle(
                    &catalog_dir,
                    intent.resource_kind,
                    &intent.source_profile,
                    &intent.resource_sha256,
                )?);
                acknowledged.push(intent);
            }
            for candidate in terminal_candidates {
                removed += usize::from(remove_exact_bundle(
                    &catalog_dir,
                    candidate.resource_kind,
                    &candidate.source_profile,
                    &candidate.resource_sha256,
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
    validate_exact_bundle_layout(&path)?;
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

fn validate_exact_bundle_layout(path: &Path) -> Result<()> {
    let mut names = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    names.sort();
    let mut expected = vec![
        std::ffi::OsString::from(CATALOG_FILE_NAME),
        std::ffi::OsString::from(CATALOG_MANIFEST_FILE_NAME),
    ];
    expected.sort();
    if names != expected {
        bail!(
            "catalog deletion target {} does not have the exact immutable bundle layout",
            path.display()
        );
    }
    for name in &expected {
        let child = path.join(name);
        let child_metadata = fs::symlink_metadata(&child)?;
        if child_metadata.file_type().is_symlink() || !child_metadata.file_type().is_file() {
            bail!(
                "catalog deletion target child {} is not a regular file",
                child.display()
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
mod tests {
    use super::*;

    fn digest(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    fn resource_digest(byte: char) -> String {
        conary_core::hash::sha256(format!("{{\"resource\":\"{byte}\"}}").as_bytes())
    }

    fn exact_bundle(path: &Path) {
        fs::create_dir_all(path).unwrap();
        fs::write(path.join(CATALOG_FILE_NAME), b"catalog").unwrap();
        fs::write(path.join(CATALOG_MANIFEST_FILE_NAME), b"manifest").unwrap();
    }

    #[tokio::test]
    async fn restart_recovery_fences_and_acknowledges_exact_expired_candidate() {
        let root = tempfile::tempdir().unwrap();
        let db_path = root.path().join("remi.db");
        let candidate_root = root.path().join("catalog-candidates");
        fs::create_dir(&candidate_root).unwrap();
        conary_core::db::init(&db_path).unwrap();
        let run_id = "10000000-0000-4000-8000-000000000001";
        let run_path = candidate_root.join(run_id);
        fs::create_dir(&run_path).unwrap();
        fs::write(run_path.join("private-candidate"), b"fixture").unwrap();
        let conn = conary_core::db::open_fast(&db_path).unwrap();
        conn.execute(
            "INSERT INTO repository_sync_runs (
                 run_id, source_profile, owner_instance_uuid, fencing_epoch,
                 state, started_at, heartbeat_at, lease_expires_at
             ) VALUES (?1, 'fedora-44', ?2, 1, 'fetching_objects', 1, 1, 1)",
            rusqlite::params![run_id, "00000000-0000-4000-8000-000000000001",],
        )
        .unwrap();
        drop(conn);

        assert_eq!(
            recover_catalog_refresh_runs_uncoordinated(
                db_path.clone(),
                candidate_root,
                DatabaseWriter::default(),
            )
            .await
            .unwrap(),
            1
        );
        assert!(!run_path.exists());
        let conn = conary_core::db::open_fast(&db_path).unwrap();
        let recovered: (String, bool) = conn
            .query_row(
                "SELECT state, candidate_cleaned_at IS NOT NULL
                 FROM repository_sync_runs WHERE run_id = ?1",
                [run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(recovered, ("abandoned".to_string(), true));
    }

    #[test]
    fn exact_bundle_removal_refuses_unknown_or_symlinked_content() {
        let root = tempfile::tempdir().unwrap();
        let catalog_root = root.path().join("catalogs");
        fs::create_dir_all(catalog_root.join("sources")).unwrap();
        let exact_digest = digest('a');
        let exact = bundle_path(
            &catalog_root,
            RemiCatalogResourceKind::SourceSnapshot,
            "fedora-44",
            &exact_digest,
        );
        exact_bundle(&exact);
        assert!(
            remove_exact_bundle(
                &catalog_root,
                RemiCatalogResourceKind::SourceSnapshot,
                "fedora-44",
                &exact_digest,
            )
            .unwrap()
        );
        assert!(!exact.exists());

        let malformed_digest = digest('b');
        let malformed = bundle_path(
            &catalog_root,
            RemiCatalogResourceKind::SourceSnapshot,
            "fedora-44",
            &malformed_digest,
        );
        exact_bundle(&malformed);
        fs::write(malformed.join("unexpected"), b"evidence").unwrap();
        assert!(
            remove_exact_bundle(
                &catalog_root,
                RemiCatalogResourceKind::SourceSnapshot,
                "fedora-44",
                &malformed_digest,
            )
            .is_err()
        );
        assert!(malformed.join("unexpected").exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let target = root.path().join("target");
            exact_bundle(&target);
            let linked_digest = digest('c');
            let linked = bundle_path(
                &catalog_root,
                RemiCatalogResourceKind::SourceSnapshot,
                "fedora-44",
                &linked_digest,
            );
            symlink(&target, &linked).unwrap();
            assert!(
                remove_exact_bundle(
                    &catalog_root,
                    RemiCatalogResourceKind::SourceSnapshot,
                    "fedora-44",
                    &linked_digest,
                )
                .is_err()
            );
            assert!(target.exists());

            let redirected_root = root.path().join("redirected-catalogs");
            fs::create_dir(&redirected_root).unwrap();
            let redirected_digest = digest('d');
            let redirected = redirected_root.join(&redirected_digest);
            exact_bundle(&redirected);
            let symlinked_catalog_root = root.path().join("symlinked-parent-catalogs");
            fs::create_dir(&symlinked_catalog_root).unwrap();
            symlink(&redirected_root, symlinked_catalog_root.join("sources")).unwrap();
            assert!(
                remove_exact_bundle(
                    &symlinked_catalog_root,
                    RemiCatalogResourceKind::SourceSnapshot,
                    "fedora-44",
                    &redirected_digest,
                )
                .is_err()
            );
            assert!(redirected.exists());
        }
    }

    #[test]
    fn deletion_resumes_from_exact_gc_tombstone_after_rename() {
        let root = tempfile::tempdir().unwrap();
        let catalog_root = root.path().join("catalogs");
        fs::create_dir_all(catalog_root.join("sources")).unwrap();
        let resource_digest = digest('d');
        let original = bundle_path(
            &catalog_root,
            RemiCatalogResourceKind::SourceSnapshot,
            "fedora-44",
            &resource_digest,
        );
        exact_bundle(&original);
        let tombstone_parent = ensure_tombstone_parent(
            &catalog_root,
            RemiCatalogResourceKind::SourceSnapshot,
            "fedora-44",
        )
        .unwrap();
        let tombstone = tombstone_path(
            &catalog_root,
            RemiCatalogResourceKind::SourceSnapshot,
            "fedora-44",
            &resource_digest,
        );
        fs::rename(&original, &tombstone).unwrap();
        File::open(tombstone_parent).unwrap().sync_all().unwrap();

        assert!(
            remove_exact_bundle(
                &catalog_root,
                RemiCatalogResourceKind::SourceSnapshot,
                "fedora-44",
                &resource_digest,
            )
            .unwrap()
        );
        assert!(!original.exists());
        assert!(!tombstone.exists());
    }

    #[test]
    fn absent_profile_namespace_is_idempotent_bundle_absence() {
        let root = tempfile::tempdir().unwrap();
        let catalog_root = root.path().join("catalogs");
        fs::create_dir_all(catalog_root.join("profiles")).unwrap();

        assert!(
            !remove_exact_bundle(
                &catalog_root,
                RemiCatalogResourceKind::ProfileRevision,
                "fedora-44",
                &digest('a'),
            )
            .unwrap()
        );
    }

    #[tokio::test]
    async fn registered_unreachable_resources_are_journaled_removed_and_acknowledged() {
        let root = tempfile::tempdir().unwrap();
        let db_path = root.path().join("remi.db");
        let catalog_root = root.path().join("catalogs");
        fs::create_dir_all(catalog_root.join("sources")).unwrap();
        fs::create_dir_all(catalog_root.join("profiles/fedora-44")).unwrap();
        conary_core::db::init(&db_path).unwrap();
        let conn = conary_core::db::open_fast(&db_path).unwrap();
        for (byte, kind) in [
            ('a', RemiCatalogResourceKind::SourceSnapshot),
            ('b', RemiCatalogResourceKind::ProfileRevision),
        ] {
            let resource = RemiCatalogResource {
                resource_sha256: resource_digest(byte),
                kind,
                source_profile: "fedora-44".to_string(),
                artifact_sha256: digest(byte),
                artifact_size: 7,
                logical_digest_sha256: digest('d'),
                manifest_json: format!("{{\"resource\":\"{byte}\"}}"),
                durable: true,
                created_at: 1,
            };
            resource.insert(&conn).unwrap();
            exact_bundle(&bundle_path(
                &catalog_root,
                kind,
                "fedora-44",
                &resource.resource_sha256,
            ));
        }
        drop(conn);

        let report = collect_catalog_garbage_uncoordinated(
            db_path.clone(),
            catalog_root,
            DatabaseWriter::default(),
        )
        .await
        .unwrap();
        assert_eq!(report.deleted_profile_resources, 1);
        assert_eq!(report.deleted_source_resources, 1);
        assert_eq!(report.removed_bundles, 2);
        assert_eq!(report.acknowledged_deletions, 2);

        let conn = conary_core::db::open_fast(&db_path).unwrap();
        assert!(
            plan_catalog_collection(&conn)
                .unwrap()
                .pending_deletions
                .is_empty()
        );
    }

    #[tokio::test]
    async fn terminal_run_journal_removes_exact_unregistered_publication() {
        let root = tempfile::tempdir().unwrap();
        let db_path = root.path().join("remi.db");
        let catalog_root = root.path().join("catalogs");
        fs::create_dir_all(catalog_root.join("sources")).unwrap();
        fs::create_dir_all(catalog_root.join("profiles/fedora-44")).unwrap();
        conary_core::db::init(&db_path).unwrap();
        let profile_digest = digest('a');
        let source_digest = digest('b');
        let conn = conary_core::db::open_fast(&db_path).unwrap();
        let repository_id = conn
            .query_row(
                "INSERT INTO repositories(name, url, source_profile)
                 VALUES ('fixture', 'https://fixture.test', 'fedora-44')
                 RETURNING id",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO repository_sync_runs (
                 run_id, source_profile, owner_instance_uuid, fencing_epoch,
                 candidate_profile_digest, state, started_at, heartbeat_at,
                 lease_expires_at, finished_at, failure_stage, failure_category,
                 failure_evidence
             ) VALUES (?1, 'fedora-44', ?2, 1, ?3, 'abandoned', 1, 1, 1, 2,
                       'publishing', 'internal', 'injected crash after rename')",
            rusqlite::params![
                "10000000-0000-4000-8000-000000000001",
                "00000000-0000-4000-8000-000000000001",
                &profile_digest,
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO repository_sync_run_members (
                 run_id, ordinal, repository_id, source_identity,
                 repository_identity, stream_kind, stream_identity, priority,
                 required, candidate_source_snapshot_sha256
             ) VALUES (?1, 0, ?2, 'fixture-source', 'fixture-repository',
                       'release', '44', 0, 1, ?3)",
            rusqlite::params![
                "10000000-0000-4000-8000-000000000001",
                repository_id,
                &source_digest,
            ],
        )
        .unwrap();
        drop(conn);
        let profile_path = bundle_path(
            &catalog_root,
            RemiCatalogResourceKind::ProfileRevision,
            "fedora-44",
            &profile_digest,
        );
        let source_path = bundle_path(
            &catalog_root,
            RemiCatalogResourceKind::SourceSnapshot,
            "fedora-44",
            &source_digest,
        );
        exact_bundle(&profile_path);
        exact_bundle(&source_path);

        let report =
            collect_catalog_garbage_uncoordinated(db_path, catalog_root, DatabaseWriter::default())
                .await
                .unwrap();
        assert_eq!(report.removed_bundles, 2);
        assert!(!profile_path.exists());
        assert!(!source_path.exists());
    }
}
