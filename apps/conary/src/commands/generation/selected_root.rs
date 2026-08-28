// apps/conary/src/commands/generation/selected_root.rs

//! Rollback-safe writable roots for generation-aware transaction execution.

mod config_state;
mod deferred_ima;
mod overlay_session;
mod publication_authority;

#[cfg(test)]
pub(crate) use publication_authority::persist_captured_publication_snapshot;
pub(crate) use publication_authority::{
    load_publication_selected_root, persist_publication_snapshot,
};

use crate::commands::{LiveRootFile, LiveRootStats, LiveRootTransaction};
use anyhow::{Context, Result, bail};
use conary_core::db::models::GenerationPublication;
use conary_core::filesystem::CasStore;
use conary_core::generation::artifact::GenerationArtifact;
use conary_core::generation::root_manifest::{
    CapturedSelectedRoot, SelectedRootSnapshot, materialize_captured_selected_root,
    scan_selected_root,
};
use conary_core::runtime_root::ConaryRuntimeRoot;
use conary_core::transaction::{TransactionConfig, TransactionEngine};
use std::fs;
use std::path::{Path, PathBuf};

use deferred_ima::DeferredImaAuthority;
use overlay_session::SelectedRootOverlaySession;
use publication_authority::latest_selected_root_snapshot;

enum SelectedRootBacking {
    Overlay(SelectedRootOverlaySession),
    /// Try sessions retain a complete tree for later namespace exposure. The
    /// test mount bypass exercises that same explicit non-production boundary.
    Materialized,
}

enum PreparedSelectedRoot {
    Materialized {
        captured: CapturedSelectedRoot,
        snapshot: SelectedRootSnapshot,
    },
    CurrentGeneration {
        artifact: Box<GenerationArtifact>,
        captured: CapturedSelectedRoot,
        snapshot: SelectedRootSnapshot,
    },
}

impl PreparedSelectedRoot {
    fn captured(&self) -> &CapturedSelectedRoot {
        match self {
            Self::Materialized { captured, .. } | Self::CurrentGeneration { captured, .. } => {
                captured
            }
        }
    }

    fn snapshot(&self) -> SelectedRootSnapshot {
        match self {
            Self::Materialized { snapshot, .. } | Self::CurrentGeneration { snapshot, .. } => {
                *snapshot
            }
        }
    }
}

/// One isolated selected-root view with one transaction-owned rollback authority.
///
/// The caller retains its SQLite transaction. This session never changes the
/// host's `current` generation link; final generation publication happens only
/// after the caller commits the database.
pub(crate) struct SelectedRootSession {
    session_dir: PathBuf,
    selected_root: PathBuf,
    transaction: Option<LiveRootTransaction>,
    transaction_engine: TransactionEngine,
    deferred_ima: DeferredImaAuthority,
    prior_snapshot: SelectedRootSnapshot,
    backing: SelectedRootBacking,
}

/// The runtime mutation lock, held before any package authority is read.
///
/// [`SelectedRootSession::begin`] takes this lock and prepares the selected
/// root in one step, which is what a caller whose planning is already complete
/// wants. A caller that must certify a transaction against installed state --
/// requirement satisfaction, promise reliance, negative relation effects --
/// acquires this first and reads those facts with the lock already held, so
/// nothing it certifies can change before it commits. Root preparation is the
/// expensive half, so keeping it separate also lets a rejected transaction
/// fail without paying for a root it will never mutate.
///
/// Dropping without preparing releases the lock and leaves no session
/// directory behind, because the directory is created by `prepare`.
pub(crate) struct LockedRuntimeRoot {
    runtime_root: ConaryRuntimeRoot,
    session_dir: PathBuf,
    session_id: String,
    transaction_engine: TransactionEngine,
}

impl LockedRuntimeRoot {
    /// Acquire the runtime mutation lock for the runtime root owning `db_path`.
    pub(crate) fn acquire(db_path: &str) -> Result<Self> {
        Self::acquire_for_runtime(ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path)))
    }

    fn acquire_for_runtime(runtime_root: ConaryRuntimeRoot) -> Result<Self> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let session_dir = runtime_root
            .root()
            .join("selected-root-sessions")
            .join(&session_id);
        Self::acquire_in_session_dir(runtime_root, session_dir, session_id)
    }

    fn acquire_in_session_dir(
        runtime_root: ConaryRuntimeRoot,
        session_dir: PathBuf,
        session_id: String,
    ) -> Result<Self> {
        // The runtime transaction lock is acquired before reading either
        // SQLite package authority or the selected generation. Holding it
        // through snapshot persistence and the caller-owned DB commit makes
        // the prepared root a serializable mutation base.
        let mut transaction_engine =
            TransactionEngine::new(TransactionConfig::for_runtime_root(&runtime_root))?;
        transaction_engine.begin()?;
        Ok(Self {
            runtime_root,
            session_dir,
            session_id,
            transaction_engine,
        })
    }

    /// Prepare installed package state as a writable selected root.
    ///
    /// Consumes the lock holder: the lock is not released here, it moves into
    /// the returned session and is released when that session finishes.
    pub(crate) fn prepare(
        self,
        conn: &rusqlite::Connection,
        operation: impl Into<String>,
    ) -> Result<SelectedRootSession> {
        let materialized_backing = use_materialized_selected_root_backing();
        self.prepare_with_backing(conn, operation, materialized_backing)
    }

    fn prepare_retained(
        self,
        conn: &rusqlite::Connection,
        operation: impl Into<String>,
    ) -> Result<SelectedRootSession> {
        self.prepare_with_backing(conn, operation, true)
    }

    fn prepare_with_backing(
        self,
        conn: &rusqlite::Connection,
        operation: impl Into<String>,
        materialized_backing: bool,
    ) -> Result<SelectedRootSession> {
        let Self {
            runtime_root,
            session_dir,
            session_id,
            transaction_engine,
        } = self;
        let prepared =
            match prepare_current_root(conn, &runtime_root, &session_dir, materialized_backing) {
                Ok(prepared) => prepared,
                Err(error) => {
                    let _ = fs::remove_dir_all(&session_dir);
                    return Err(error);
                }
            };
        let prior_snapshot = prepared.snapshot();
        let prior = prepared.captured();
        let deferred_ima = match DeferredImaAuthority::from_captured(prior) {
            Ok(authority) => authority,
            Err(error) => {
                let _ = fs::remove_dir_all(&session_dir);
                return Err(error);
            }
        };
        let mut backing = match &prepared {
            PreparedSelectedRoot::Materialized { .. } if materialized_backing => {
                SelectedRootBacking::Materialized
            }
            PreparedSelectedRoot::Materialized { .. } => {
                match SelectedRootOverlaySession::begin_materialized(&session_dir, prior) {
                    Ok(overlay) => SelectedRootBacking::Overlay(overlay),
                    Err(error) => {
                        let _ = fs::remove_dir_all(&session_dir);
                        return Err(error);
                    }
                }
            }
            PreparedSelectedRoot::CurrentGeneration { artifact, .. } => {
                let cas = transaction_engine.cas();
                match SelectedRootOverlaySession::begin_current_generation(
                    &session_dir,
                    prior,
                    artifact,
                    cas,
                ) {
                    Ok(overlay) => SelectedRootBacking::Overlay(overlay),
                    Err(error) => {
                        let _ = fs::remove_dir_all(&session_dir);
                        return Err(error);
                    }
                }
            }
        };
        let selected_root = match &backing {
            SelectedRootBacking::Overlay(overlay) => overlay.selected_root().to_path_buf(),
            SelectedRootBacking::Materialized => session_dir.join("root"),
        };
        let transaction = match if matches!(&backing, SelectedRootBacking::Overlay(_)) {
            LiveRootTransaction::begin_disposable_overlay(
                runtime_root.root(),
                &selected_root,
                session_id,
                operation,
            )
        } else {
            LiveRootTransaction::begin(runtime_root.root(), &selected_root, session_id, operation)
        } {
            Ok(transaction) => transaction,
            Err(error) => {
                if let SelectedRootBacking::Overlay(overlay) = &mut backing {
                    let _ = overlay.unmount_for_discard();
                }
                let _ = fs::remove_dir_all(&session_dir);
                return Err(error);
            }
        };
        Ok(SelectedRootSession {
            session_dir,
            selected_root,
            transaction: Some(transaction),
            transaction_engine,
            deferred_ima,
            prior_snapshot,
            backing,
        })
    }
}

impl SelectedRootSession {
    pub(crate) fn begin(
        conn: &rusqlite::Connection,
        db_path: &str,
        operation: impl Into<String>,
    ) -> Result<Self> {
        let runtime_root = ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path));
        Self::begin_for_runtime(conn, &runtime_root, operation)
    }

    /// Materialize installed package state with objects owned by an explicit
    /// runtime root.
    ///
    /// Try sessions use a copied database with the live runtime's shared CAS,
    /// so deriving the object root from the copied DB path would be incorrect.
    pub(crate) fn begin_for_runtime(
        conn: &rusqlite::Connection,
        runtime_root: &ConaryRuntimeRoot,
        operation: impl Into<String>,
    ) -> Result<Self> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let session_dir = runtime_root
            .root()
            .join("selected-root-sessions")
            .join(&session_id);
        Self::begin_in_session_dir(conn, runtime_root, session_dir, session_id, operation)
    }

    /// Create a selected root at a try-session-owned location.
    pub(crate) fn begin_for_try(
        conn: &rusqlite::Connection,
        runtime_root: &ConaryRuntimeRoot,
        session_dir: PathBuf,
        operation: impl Into<String>,
    ) -> Result<Self> {
        validate_try_session_dir(runtime_root, &session_dir)?;
        let session_id = uuid::Uuid::new_v4().to_string();
        LockedRuntimeRoot::acquire_in_session_dir(runtime_root.clone(), session_dir, session_id)?
            .prepare_retained(conn, operation)
    }

    fn begin_in_session_dir(
        conn: &rusqlite::Connection,
        runtime_root: &ConaryRuntimeRoot,
        session_dir: PathBuf,
        session_id: String,
        operation: impl Into<String>,
    ) -> Result<Self> {
        LockedRuntimeRoot::acquire_in_session_dir(runtime_root.clone(), session_dir, session_id)?
            .prepare(conn, operation)
    }

    pub(crate) fn selected_root(&self) -> &Path {
        &self.selected_root
    }

    pub(crate) fn cas(&self) -> &CasStore {
        self.transaction_engine.cas()
    }

    /// Return the exact typed authority selected before mutation.
    ///
    /// The lower is immutable, so rollback needs neither a complete scan nor a
    /// reconstruction from package rows.
    pub(crate) fn capture_rollback_authority(&self) -> Result<SelectedRootSnapshot> {
        Ok(self.prior_snapshot)
    }

    pub(crate) fn apply_install_files(&mut self, files: &[LiveRootFile]) -> Result<()> {
        self.transaction_mut()?.apply_install_files(files)?;
        self.deferred_ima.record_overlay(files)?;
        Ok(())
    }

    pub(crate) fn apply_install_files_with_references(
        &mut self,
        files: &[LiveRootFile],
        references: &[LiveRootFile],
    ) -> Result<()> {
        self.transaction_mut()?
            .apply_install_files_with_references(files, references)?;
        self.deferred_ima.record_overlay(files)?;
        Ok(())
    }

    pub(crate) fn apply_remove_paths(&mut self, paths: &[String]) -> Result<LiveRootStats> {
        let stats = self.transaction_mut()?.apply_remove_paths(paths)?;
        self.deferred_ima.remove_paths(paths);
        Ok(stats)
    }

    /// Commit and capture the exact selected root while retaining its writable
    /// filesystem tree for a try namespace.
    pub(crate) fn capture_preserving_root(
        mut self,
        runtime_root: &ConaryRuntimeRoot,
    ) -> Result<(PathBuf, CapturedSelectedRoot)> {
        self.transaction
            .take()
            .context("selected-root transaction already completed")?
            .commit()?;
        if !matches!(&self.backing, SelectedRootBacking::Materialized) {
            bail!("only an explicit retained try-session root can be preserved");
        }
        let cas = CasStore::new(runtime_root.objects_dir())?;
        let mut captured = scan_selected_root(&self.selected_root, &cas)?;
        self.deferred_ima.restore_into(&mut captured)?;
        Ok((self.selected_root.clone(), captured))
    }

    /// Commit and durably persist the selected-root publication authority.
    ///
    /// The database transaction that created `debt` is still caller-owned.
    /// Persisting this snapshot before that transaction commits means a
    /// committed selected-root mutation always has a retryable typed root.
    pub(crate) fn persist_for_publication(
        &mut self,
        conn: &rusqlite::Connection,
        runtime_root: &ConaryRuntimeRoot,
        debt: &GenerationPublication,
    ) -> Result<SelectedRootSnapshot> {
        let result = (|| {
            self.transaction
                .take()
                .context("selected-root transaction already completed")?
                .commit()?;
            let cas = CasStore::new(runtime_root.objects_dir())?;
            let snapshot = match &mut self.backing {
                SelectedRootBacking::Overlay(overlay) => {
                    let mut delta = overlay.freeze_and_decode(conn, self.prior_snapshot, &cas)?;
                    self.deferred_ima.restore_into_delta(&mut delta)?;
                    self.prior_snapshot.apply_delta(conn, &delta)?
                }
                SelectedRootBacking::Materialized => {
                    let mut captured = scan_selected_root(&self.selected_root, &cas)?;
                    self.deferred_ima.restore_into(&mut captured)?;
                    SelectedRootSnapshot::capture(conn, &captured)?
                }
            };
            persist_publication_snapshot(conn, debt, snapshot)?;
            remove_session_dir(&self.session_dir)?;
            Ok(snapshot)
        })();
        if result.is_err() {
            let _ = remove_session_dir(&self.session_dir);
        }
        result
    }

    pub(crate) fn rollback(&mut self) -> Result<()> {
        let transaction_result = if let Some(transaction) = self.transaction.take() {
            // The merged root is disposable. Completing the journal and
            // discarding the upper is both safer and cheaper than replaying
            // path-by-path restoration into a tree that cannot be published.
            transaction.commit()
        } else {
            Ok(())
        };
        let unmount_result = match &mut self.backing {
            SelectedRootBacking::Overlay(overlay) => overlay.unmount_for_discard(),
            SelectedRootBacking::Materialized => Ok(()),
        };
        let removal_result = remove_session_dir(&self.session_dir);
        transaction_result?;
        unmount_result?;
        removal_result
    }

    fn transaction_mut(&mut self) -> Result<&mut LiveRootTransaction> {
        self.transaction
            .as_mut()
            .context("selected-root transaction already completed")
    }
}

fn use_materialized_selected_root_backing() -> bool {
    cfg!(test) || std::env::var_os("CONARY_TEST_SKIP_GENERATION_MOUNT").is_some()
}

impl Drop for SelectedRootSession {
    fn drop(&mut self) {
        if self.transaction.is_some() {
            let _ = self.rollback();
        }
    }
}

fn prepare_current_root(
    conn: &rusqlite::Connection,
    runtime_root: &ConaryRuntimeRoot,
    session_dir: &Path,
    require_materialized: bool,
) -> Result<PreparedSelectedRoot> {
    let cas = CasStore::new(runtime_root.objects_dir())?;
    if let Some((snapshot, captured)) = latest_selected_root_snapshot(conn)? {
        let selected_root =
            selected_root_materialization_destination(session_dir, require_materialized)?;
        materialize_captured_selected_root(&captured, &cas, &selected_root)?;
        return Ok(PreparedSelectedRoot::Materialized { captured, snapshot });
    }

    if let Some(generation) =
        conary_core::generation::mount::current_generation(runtime_root.root())?
    {
        let generation_path = runtime_root.generation_path(generation);
        let artifact = if require_materialized {
            conary_core::generation::artifact::load_generation_artifact(&generation_path)?
        } else {
            conary_core::generation::artifact::load_generation_artifact_with_verified_cas(
                &generation_path,
            )?
        };
        let mut captured = CapturedSelectedRoot {
            generation: artifact.generation_root.clone(),
            state: artifact.mutable_state.clone(),
        };
        let mut snapshot = match GenerationPublication::selected_root_snapshot_for_generation(
            conn,
            generation,
        )? {
            Some(snapshot_id) => SelectedRootSnapshot::find(conn, snapshot_id)?.with_context(|| {
                format!(
                    "generation {generation} references missing selected-root snapshot {snapshot_id}"
                )
            })?,
            None => SelectedRootSnapshot::capture(conn, &captured)?,
        };
        if let Some((active_snapshot, active_captured)) =
            config_state::capture_active_upper(conn, runtime_root, generation, snapshot, &cas)?
        {
            snapshot = active_snapshot;
            captured = active_captured;
        }
        if require_materialized {
            let selected_root = selected_root_materialization_destination(session_dir, true)?;
            materialize_captured_selected_root(&captured, &cas, &selected_root)?;
            return Ok(PreparedSelectedRoot::Materialized { captured, snapshot });
        }
        return Ok(PreparedSelectedRoot::CurrentGeneration {
            artifact: Box::new(artifact),
            captured,
            snapshot,
        });
    }

    let selected_root =
        selected_root_materialization_destination(session_dir, require_materialized)?;
    let captured =
        conary_core::generation::builder::materialize_selected_root_from_db_with_authority(
            conn,
            &runtime_root.objects_dir(),
            &selected_root,
        )?;
    let snapshot = SelectedRootSnapshot::capture(conn, &captured)?;
    Ok(PreparedSelectedRoot::Materialized { captured, snapshot })
}

fn selected_root_materialization_destination(
    session_dir: &Path,
    retained: bool,
) -> Result<PathBuf> {
    let destination = if retained {
        session_dir.join("root")
    } else {
        session_dir.join("lower")
    };
    fs::create_dir_all(&destination).with_context(|| {
        format!(
            "failed to create selected-root materialization destination {}",
            destination.display()
        )
    })?;
    Ok(destination)
}

fn remove_session_dir(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        bail!("selected-root session path has no parent");
    };
    let ordinary_session =
        parent.file_name().and_then(|name| name.to_str()) == Some("selected-root-sessions");
    let try_session = path.file_name().and_then(|name| name.to_str())
        == Some("selected-root-session")
        && path
            .ancestors()
            .nth(2)
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some("try");
    if !ordinary_session && !try_session {
        bail!(
            "refusing to remove unexpected selected-root session path {}",
            path.display()
        );
    }
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn validate_try_session_dir(runtime_root: &ConaryRuntimeRoot, path: &Path) -> Result<()> {
    let try_root = runtime_root.root().join("try");
    if path.file_name().and_then(|name| name.to_str()) != Some("selected-root-session")
        || !path.starts_with(&try_root)
        || path.parent() == Some(try_root.as_path())
    {
        bail!(
            "try selected-root session path {} is outside a concrete runtime try session",
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{FileEntry, GenerationPublicationStatus, Trove, TroveType};
    use conary_core::payload::{
        PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, ResolvedPayloadNode,
    };

    fn resolved_regular(mode: u32) -> ResolvedPayloadNode {
        let mut source = PayloadNode::regular(mode & 0o7777);
        source.user = PayloadIdentity::Numeric {
            id: u64::from(unsafe { libc::geteuid() }),
        };
        source.group = PayloadIdentity::Numeric {
            id: u64::from(unsafe { libc::getegid() }),
        };
        ResolvedPayloadNode::from_numeric_source(source).unwrap()
    }

    fn resolved_directory(mode: u32) -> ResolvedPayloadNode {
        let mut source = PayloadNode::regular(mode & 0o7777);
        source.kind = PayloadNodeKind::Directory;
        source.mode = libc::S_IFDIR | (mode & 0o7777);
        source.user = PayloadIdentity::Numeric {
            id: u64::from(unsafe { libc::geteuid() }),
        };
        source.group = PayloadIdentity::Numeric {
            id: u64::from(unsafe { libc::getegid() }),
        };
        ResolvedPayloadNode::from_numeric_source(source).unwrap()
    }

    fn live_regular(path: &str, content: &[u8], mode: u32) -> LiveRootFile {
        LiveRootFile {
            path: path.to_string(),
            content: crate::commands::LiveRootContent::from_in_memory_bytes(content),
            node: resolved_regular(mode),
        }
    }

    #[test]
    fn no_current_generation_materializes_authoritative_db_state_into_selected_root() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();
        let runtime_root = ConaryRuntimeRoot::from_db_path(&db_path);
        let cas = conary_core::filesystem::CasStore::new(runtime_root.objects_dir()).unwrap();
        let hash = cas.store(b"package-a").unwrap();
        let mut trove = Trove::new(
            "package-a".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(&conn).unwrap();
        for path in ["/usr", "/usr/lib"] {
            FileEntry::new(path.to_string(), resolved_directory(0o755), None, trove_id)
                .insert(&conn)
                .unwrap();
        }
        FileEntry::new(
            "/usr/lib/package-a".to_string(),
            resolved_regular(0o644),
            Some(PayloadContentAuthority {
                sha256: hash,
                size: 9,
            }),
            trove_id,
        )
        .insert(&conn)
        .unwrap();
        assert!(
            conary_core::generation::mount::current_generation(runtime_root.root())
                .unwrap()
                .is_none()
        );

        let mut session =
            SelectedRootSession::begin(&conn, db_path.to_str().unwrap(), "batch graph").unwrap();
        assert_eq!(
            fs::read_to_string(session.selected_root().join("usr/lib/package-a")).unwrap(),
            "package-a"
        );
        session
            .apply_install_files(&[live_regular("/usr/lib/package-b", b"package-b", 0o644)])
            .unwrap();
        assert_eq!(
            fs::read_to_string(session.selected_root().join("usr/lib/package-b")).unwrap(),
            "package-b"
        );
        let (_, captured) = session.capture_preserving_root(&runtime_root).unwrap();
        assert_eq!(
            captured
                .generation
                .entries
                .iter()
                .find(|entry| entry.path == "/usr/lib/package-b")
                .and_then(|entry| entry.content.as_ref())
                .map(|content| content.size),
            Some(9)
        );
    }

    #[test]
    fn rollback_authority_is_the_immutable_prior_not_a_mutated_root_scan() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();
        let mut session =
            SelectedRootSession::begin(&conn, db_path.to_str().unwrap(), "rollback prior").unwrap();
        let before = session.capture_rollback_authority().unwrap();
        session
            .apply_install_files(&[live_regular("/opt/new", b"new", 0o644)])
            .unwrap();

        assert_eq!(session.capture_rollback_authority().unwrap(), before);
        assert!(
            !before
                .materialize(&conn)
                .unwrap()
                .generation
                .entries
                .iter()
                .any(|entry| entry.path == "/opt/new")
        );
    }

    #[test]
    fn selected_root_snapshot_is_retryable_after_publication_becomes_terminal() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();
        let runtime_root = ConaryRuntimeRoot::from_db_path(&db_path);
        let mut session =
            SelectedRootSession::begin(&conn, db_path.to_str().unwrap(), "selected root").unwrap();
        let debt = GenerationPublication::create_pending(
            &conn,
            None,
            None,
            db_path.to_str().unwrap(),
            &runtime_root.root().display().to_string(),
            "selected root",
            &Default::default(),
        )
        .unwrap();
        session
            .apply_install_files(&[live_regular(
                "/opt/lifecycle-created",
                b"exact lifecycle output",
                0o640,
            )])
            .unwrap();

        session
            .persist_for_publication(&conn, &runtime_root, &debt)
            .unwrap();
        let captured = load_publication_selected_root(&conn, &debt).unwrap();
        let entry = captured
            .generation
            .entries
            .iter()
            .find(|entry| entry.path == "/opt/lifecycle-created")
            .unwrap();
        assert_eq!(entry.content.as_ref().map(|content| content.size), Some(22));

        debt.set_phase(
            &conn,
            conary_core::db::models::GenerationPublicationPhase::DatabaseBackedUp,
            GenerationPublicationStatus::Running,
            Some(1),
            Some(1),
        )
        .unwrap();
        debt.mark_complete_through(&conn, None, 1, 1).unwrap();
        assert!(load_publication_selected_root(&conn, &debt).is_ok());
    }

    #[test]
    fn later_selected_root_snapshot_cumulates_prior_pending_effects() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();
        let runtime_root = ConaryRuntimeRoot::from_db_path(&db_path);

        let mut first =
            SelectedRootSession::begin(&conn, db_path.to_str().unwrap(), "first").unwrap();
        first
            .apply_install_files(&[live_regular("/opt/first-effect", b"first", 0o644)])
            .unwrap();
        let first_debt = GenerationPublication::create_pending(
            &conn,
            None,
            None,
            db_path.to_str().unwrap(),
            &runtime_root.root().display().to_string(),
            "first",
            &Default::default(),
        )
        .unwrap();
        first
            .persist_for_publication(&conn, &runtime_root, &first_debt)
            .unwrap();
        first_debt
            .mark_failed(&conn, "forced first publication failure")
            .unwrap();
        drop(first);

        let mut second =
            SelectedRootSession::begin(&conn, db_path.to_str().unwrap(), "second").unwrap();
        assert_eq!(
            fs::read(second.selected_root().join("opt/first-effect")).unwrap(),
            b"first"
        );
        second
            .apply_install_files(&[live_regular("/opt/second-effect", b"second", 0o644)])
            .unwrap();
        let second_debt = GenerationPublication::create_pending(
            &conn,
            None,
            None,
            db_path.to_str().unwrap(),
            &runtime_root.root().display().to_string(),
            "second",
            &Default::default(),
        )
        .unwrap();
        second
            .persist_for_publication(&conn, &runtime_root, &second_debt)
            .unwrap();

        let latest = load_publication_selected_root(&conn, &second_debt).unwrap();
        let paths = latest
            .generation
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(paths.contains("/opt/first-effect"));
        assert!(paths.contains("/opt/second-effect"));

        let outcome = crate::commands::generation::publication::retry_pending_publication(
            &conn,
            db_path.to_str().unwrap(),
            "retry cumulative selected root",
        )
        .unwrap();
        assert!(outcome.needs_publication);
        let retried = GenerationPublication::find_by_id(&conn, second_debt.id.unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(retried.status, GenerationPublicationStatus::Failed);
        assert_eq!(retried.retry_count, 1);
        let still_retryable = load_publication_selected_root(&conn, &retried).unwrap();
        let retry_paths = still_retryable
            .generation
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(retry_paths.contains("/opt/first-effect"));
        assert!(retry_paths.contains("/opt/second-effect"));
    }

    #[test]
    fn mutation_lock_prevents_stale_root_and_preserves_root_db_parity() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();
        let runtime_root = ConaryRuntimeRoot::from_db_path(&db_path);
        let mut first =
            SelectedRootSession::begin(&conn, db_path.to_str().unwrap(), "first writer").unwrap();
        first
            .apply_install_files(&[live_regular("/opt/serialized-effect", b"serialized", 0o644)])
            .unwrap();

        let (attempt_tx, attempt_rx) = std::sync::mpsc::sync_channel(0);
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(0);
        let thread_db_path = db_path.clone();
        let waiter = std::thread::spawn(move || {
            let waiter_conn = conary_core::db::open(&thread_db_path).unwrap();
            attempt_tx.send(()).unwrap();
            let session = SelectedRootSession::begin(
                &waiter_conn,
                thread_db_path.to_str().unwrap(),
                "second writer",
            );
            let result = session
                .and_then(|session| {
                    let root_bytes =
                        fs::read(session.selected_root().join("opt/serialized-effect"))?;
                    let db_entry = FileEntry::find_by_path(&waiter_conn, "/opt/serialized-effect")?
                        .context("serialized root effect has no committed DB authority")?;
                    Ok((root_bytes, db_entry.content))
                })
                .map_err(|error| error.to_string());
            result_tx.send(result).unwrap();
        });
        attempt_rx.recv().unwrap();
        assert!(
            matches!(
                result_rx.recv_timeout(std::time::Duration::from_millis(250)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ),
            "second writer materialized a root before the first writer committed"
        );

        let cas_hash = first.cas().store(b"serialized").unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        let mut trove = Trove::new(
            "serialized-fixture".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(&tx).unwrap();
        FileEntry::new(
            "/opt/serialized-effect".to_string(),
            resolved_regular(0o644),
            Some(PayloadContentAuthority {
                sha256: cas_hash,
                size: 10,
            }),
            trove_id,
        )
        .insert(&tx)
        .unwrap();
        let debt = GenerationPublication::create_pending(
            &tx,
            None,
            None,
            db_path.to_str().unwrap(),
            &runtime_root.root().display().to_string(),
            "first writer",
            &Default::default(),
        )
        .unwrap();
        first
            .persist_for_publication(&tx, &runtime_root, &debt)
            .unwrap();
        tx.commit().unwrap();
        drop(first);

        let (root_bytes, db_content) = result_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
        waiter.join().unwrap();
        assert_eq!(root_bytes, b"serialized");
        let db_content = db_content.expect("serialized DB file has no content authority");
        assert_eq!(db_content.size, root_bytes.len() as u64);
        assert_eq!(db_content.sha256, CasStore::compute_sha256(&root_bytes));
    }

    #[test]
    fn abandoned_selected_root_snapshot_remains_valid_for_history() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        let conn = conary_core::db::open(&db_path).unwrap();
        let runtime_root = ConaryRuntimeRoot::from_db_path(&db_path);
        conn.execute(
            "INSERT INTO changesets (description, status) VALUES ('forward', 'applied')",
            [],
        )
        .unwrap();
        let changeset_id = conn.last_insert_rowid();
        let mut session =
            SelectedRootSession::begin(&conn, db_path.to_str().unwrap(), "forward").unwrap();
        let debt = GenerationPublication::create_pending(
            &conn,
            Some(changeset_id),
            None,
            db_path.to_str().unwrap(),
            &runtime_root.root().display().to_string(),
            "forward",
            &Default::default(),
        )
        .unwrap();
        session
            .apply_install_files(&[live_regular("/opt/forward", b"forward", 0o644)])
            .unwrap();
        session
            .persist_for_publication(&conn, &runtime_root, &debt)
            .unwrap();
        let snapshot_id = GenerationPublication::find_by_id(&conn, debt.id.unwrap())
            .unwrap()
            .unwrap()
            .selected_root_snapshot_id
            .unwrap();

        assert_eq!(
            GenerationPublication::abandon_recoverable_for_changeset(&conn, changeset_id).unwrap(),
            1
        );
        assert!(
            SelectedRootSnapshot::find(&conn, snapshot_id)
                .unwrap()
                .is_some()
        );
    }
}
