// apps/conary/src/commands/generation/selected_root.rs

//! Rollback-safe writable roots for generation-aware transaction execution.

mod deferred_ima;

use crate::commands::{LiveRootFile, LiveRootStats, LiveRootTransaction};
use anyhow::{Context, Result, bail};
use conary_core::db::models::GenerationPublication;
use conary_core::filesystem::CasStore;
use conary_core::generation::root_manifest::{
    CapturedSelectedRoot, GenerationRootManifest, MutableStateManifest,
    materialize_captured_selected_root, scan_selected_root,
};
use conary_core::runtime_root::ConaryRuntimeRoot;
use conary_core::transaction::{TransactionConfig, TransactionEngine};
use std::fs;
use std::path::{Path, PathBuf};

use deferred_ima::DeferredImaAuthority;

/// One isolated selected-root view backed by a single filesystem journal.
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
}

/// The runtime mutation lock, held before any package authority is read.
///
/// [`SelectedRootSession::begin`] takes this lock and materializes the selected
/// root in one step, which is what a caller whose planning is already complete
/// wants. A caller that must certify a transaction against installed state --
/// requirement satisfaction, promise reliance, negative relation effects --
/// acquires this first and reads those facts with the lock already held, so
/// nothing it certifies can change before it commits. Materialization is the
/// expensive half, so keeping it separate also lets a rejected transaction fail
/// without paying for a root it will never mutate.
///
/// Dropping without materializing releases the lock and leaves no session
/// directory behind, because the directory is created by `materialize`.
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
        // through candidate persistence and the caller-owned DB commit makes
        // the materialized root a serializable mutation base.
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

    /// Materialize installed package state into a writable selected root.
    ///
    /// Consumes the lock holder: the lock is not released here, it moves into
    /// the returned session and is released when that session finishes.
    pub(crate) fn materialize(
        self,
        conn: &rusqlite::Connection,
        operation: impl Into<String>,
    ) -> Result<SelectedRootSession> {
        let Self {
            runtime_root,
            session_dir,
            session_id,
            transaction_engine,
        } = self;
        let selected_root = session_dir.join("root");
        fs::create_dir_all(&selected_root).with_context(|| {
            format!(
                "failed to create selected generation root {}",
                selected_root.display()
            )
        })?;
        let materialized = match materialize_current_root(conn, &runtime_root, &selected_root) {
            Ok(materialized) => materialized,
            Err(error) => {
                let _ = fs::remove_dir_all(&session_dir);
                return Err(error);
            }
        };
        let deferred_ima = match DeferredImaAuthority::from_captured(&materialized) {
            Ok(authority) => authority,
            Err(error) => {
                let _ = fs::remove_dir_all(&session_dir);
                return Err(error);
            }
        };
        let transaction = match LiveRootTransaction::begin(
            runtime_root.root(),
            &selected_root,
            session_id,
            operation,
        ) {
            Ok(transaction) => transaction,
            Err(error) => {
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
        Self::begin_in_session_dir(conn, runtime_root, session_dir, session_id, operation)
    }

    fn begin_in_session_dir(
        conn: &rusqlite::Connection,
        runtime_root: &ConaryRuntimeRoot,
        session_dir: PathBuf,
        session_id: String,
        operation: impl Into<String>,
    ) -> Result<Self> {
        LockedRuntimeRoot::acquire_in_session_dir(runtime_root.clone(), session_dir, session_id)?
            .materialize(conn, operation)
    }

    pub(crate) fn selected_root(&self) -> &Path {
        &self.selected_root
    }

    pub(crate) fn cas(&self) -> &CasStore {
        self.transaction_engine.cas()
    }

    /// Capture the complete selected-root authority before a transaction mutates it.
    ///
    /// Rollback persists this manifest with the changeset. Rebuilding from package
    /// rows alone would lose exact parent-directory and lifecycle-created state.
    pub(crate) fn capture_rollback_authority(&self) -> Result<CapturedSelectedRoot> {
        let mut captured = scan_selected_root(&self.selected_root, self.transaction_engine.cas())?;
        self.deferred_ima.restore_into(&mut captured)?;
        Ok(captured)
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
        let cas = CasStore::new(runtime_root.objects_dir())?;
        let mut captured = scan_selected_root(&self.selected_root, &cas)?;
        self.deferred_ima.restore_into(&mut captured)?;
        Ok((self.selected_root.clone(), captured))
    }

    /// Commit and durably persist the selected-root publication authority.
    ///
    /// The database transaction that created `debt` is still caller-owned.
    /// Persisting this candidate before that transaction commits means a
    /// committed selected-root mutation always has a retryable typed root.
    pub(crate) fn persist_for_publication(
        &mut self,
        runtime_root: &ConaryRuntimeRoot,
        debt: &GenerationPublication,
    ) -> Result<CapturedSelectedRoot> {
        let result = (|| {
            self.transaction
                .take()
                .context("selected-root transaction already completed")?
                .commit()?;
            let cas = CasStore::new(runtime_root.objects_dir())?;
            let mut captured = scan_selected_root(&self.selected_root, &cas)?;
            self.deferred_ima.restore_into(&mut captured)?;
            persist_publication_candidate(runtime_root, debt, &self.session_dir, &captured)?;
            Ok(captured)
        })();
        if result.is_err() {
            let _ = remove_session_dir(&self.session_dir);
        }
        result
    }

    pub(crate) fn rollback(&mut self) -> Result<()> {
        if let Some(mut transaction) = self.transaction.take() {
            transaction.rollback()?;
        }
        remove_session_dir(&self.session_dir)
    }

    fn transaction_mut(&mut self) -> Result<&mut LiveRootTransaction> {
        self.transaction
            .as_mut()
            .context("selected-root transaction already completed")
    }
}

impl Drop for SelectedRootSession {
    fn drop(&mut self) {
        if self.transaction.is_some() {
            let _ = self.rollback();
        }
    }
}

fn materialize_current_root(
    conn: &rusqlite::Connection,
    runtime_root: &ConaryRuntimeRoot,
    selected_root: &Path,
) -> Result<CapturedSelectedRoot> {
    let cas = CasStore::new(runtime_root.objects_dir())?;
    if let Some(candidate) = latest_selected_root_candidate(conn, runtime_root)? {
        materialize_captured_selected_root(&candidate, &cas, selected_root)?;
        return Ok(candidate);
    }

    if let Some(generation) =
        conary_core::generation::mount::current_generation(runtime_root.root())?
    {
        let artifact = conary_core::generation::artifact::load_generation_artifact(
            &runtime_root.generation_path(generation),
        )?;
        let captured = CapturedSelectedRoot {
            generation: artifact.generation_root,
            state: artifact.mutable_state,
        };
        materialize_captured_selected_root(&captured, &cas, selected_root)?;
        return Ok(captured);
    }

    conary_core::generation::builder::materialize_selected_root_from_db_with_authority(
        conn,
        &runtime_root.objects_dir(),
        selected_root,
    )
    .map_err(Into::into)
}

pub(crate) fn publication_candidate_dir(
    runtime_root: &ConaryRuntimeRoot,
    debt: &GenerationPublication,
) -> Result<PathBuf> {
    let id = debt
        .id
        .context("generation publication candidate requires a persisted debt id")?;
    Ok(runtime_root
        .root()
        .join("publication-roots")
        .join(id.to_string()))
}

pub(crate) fn load_publication_candidate(
    runtime_root: &ConaryRuntimeRoot,
    debt: &GenerationPublication,
) -> Result<CapturedSelectedRoot> {
    let candidate = publication_candidate_dir(runtime_root, debt)?;
    if !candidate.join("root").is_dir() {
        bail!(
            "selected-root publication candidate is missing for debt {} at {}",
            debt.id.unwrap_or_default(),
            candidate.display()
        );
    }
    Ok(CapturedSelectedRoot {
        generation: GenerationRootManifest::read_from(&candidate)?,
        state: MutableStateManifest::read_from(&candidate)?,
    })
}

pub(crate) fn remove_publication_candidate(
    runtime_root: &ConaryRuntimeRoot,
    debt: &GenerationPublication,
) -> Result<()> {
    let candidate = publication_candidate_dir(runtime_root, debt)?;
    match fs::remove_dir_all(&candidate) {
        Ok(()) => {
            conary_core::filesystem::durable::sync_parent_directory(&candidate)?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Persist an already-captured selected root as retryable publication input.
///
/// Rollback carries this authority in changeset metadata, so it has no live
/// selected-root session directory to transfer into the candidate.
pub(crate) fn persist_captured_publication_candidate(
    runtime_root: &ConaryRuntimeRoot,
    debt: &GenerationPublication,
    captured: &CapturedSelectedRoot,
) -> Result<()> {
    let candidate = publication_candidate_dir(runtime_root, debt)?;
    remove_uncommitted_candidate_collision(&candidate)?;
    let candidates_root = candidate
        .parent()
        .context("publication candidate has no parent")?;
    fs::create_dir_all(candidates_root)?;
    let temporary = candidates_root.join(format!(".candidate-{}.tmp", uuid::Uuid::new_v4()));
    fs::create_dir(&temporary)?;
    let result = (|| -> Result<()> {
        captured.generation.write_to(&temporary)?;
        captured.state.write_to(&temporary)?;
        let candidate_root = temporary.join("root");
        let cas = CasStore::new(runtime_root.objects_dir())?;
        materialize_captured_selected_root(captured, &cas, &candidate_root)?;
        fs::rename(&temporary, &candidate)?;
        conary_core::filesystem::durable::sync_parent_directory(&candidate)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temporary);
        let _ = fs::remove_dir_all(&candidate);
    }
    result
}

#[cfg(test)]
pub(crate) fn remove_terminal_publication_candidates(
    conn: &rusqlite::Connection,
    runtime_root: &ConaryRuntimeRoot,
    candidates: &[GenerationPublication],
) -> Result<()> {
    for candidate in candidates {
        let id = candidate
            .id
            .context("publication candidate cleanup requires a persisted debt id")?;
        let current = GenerationPublication::find_by_id(conn, id)?
            .context("publication candidate cleanup debt disappeared")?;
        if current.recoverable {
            continue;
        }
        remove_publication_candidate(runtime_root, &current)?;
    }
    Ok(())
}

/// Remove publication inputs after the aggregate generation backup is durable.
///
/// A debt in `DatabaseBackedUp` replays from the generation artifact and
/// verified backup, so it no longer depends on its selected-root candidate.
/// Deleting candidates before the terminal DB update makes the crash tail
/// idempotent: retry can repeat deletion and only then complete the debt.
pub(crate) fn remove_backed_up_publication_candidates(
    runtime_root: &ConaryRuntimeRoot,
    candidates: &[GenerationPublication],
) -> Result<()> {
    for candidate in candidates {
        candidate
            .id
            .context("backed-up publication candidate requires a persisted debt id")?;
        remove_publication_candidate(runtime_root, candidate)?;
    }
    Ok(())
}

fn latest_selected_root_candidate(
    conn: &rusqlite::Connection,
    runtime_root: &ConaryRuntimeRoot,
) -> Result<Option<CapturedSelectedRoot>> {
    let debts = GenerationPublication::pending_recoverable(conn)?;
    let Some(latest) = debts.last() else {
        return Ok(None);
    };
    load_publication_candidate(runtime_root, latest).map(Some)
}

fn persist_publication_candidate(
    runtime_root: &ConaryRuntimeRoot,
    debt: &GenerationPublication,
    session_dir: &Path,
    captured: &CapturedSelectedRoot,
) -> Result<()> {
    let candidate = publication_candidate_dir(runtime_root, debt)?;
    remove_uncommitted_candidate_collision(&candidate)?;
    let candidates_root = candidate
        .parent()
        .context("publication candidate has no parent")?;
    fs::create_dir_all(candidates_root)?;
    let temporary = candidates_root.join(format!(".candidate-{}.tmp", uuid::Uuid::new_v4()));
    fs::create_dir(&temporary)?;
    let result = (|| -> Result<()> {
        captured.generation.write_to(&temporary)?;
        captured.state.write_to(&temporary)?;
        fs::rename(session_dir.join("root"), temporary.join("root"))?;
        remove_session_dir(session_dir)?;
        fs::rename(&temporary, &candidate)?;
        conary_core::filesystem::durable::sync_parent_directory(&candidate)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temporary);
    }
    result
}

/// Remove a candidate left by a process that died before its SQLite insert
/// committed.
///
/// Candidate IDs come from an uncommitted AUTOINCREMENT row and are therefore
/// reusable after SQLite rolls that transaction back. Every caller reaches
/// this boundary while holding the single runtime mutation lock and immediately
/// after creating a new debt row, so an existing directory at the same ID
/// cannot belong to another live or committed transaction.
fn remove_uncommitted_candidate_collision(candidate: &Path) -> Result<()> {
    match fs::symlink_metadata(candidate) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            fs::remove_dir_all(candidate)?;
            conary_core::filesystem::durable::sync_parent_directory(candidate)?;
            Ok(())
        }
        Ok(_) => bail!(
            "selected-root publication candidate has an unexpected file type: {}",
            candidate.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
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
    fn selected_root_candidate_is_retryable_until_debt_driven_cleanup() {
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
            .persist_for_publication(&runtime_root, &debt)
            .unwrap();
        let candidate = publication_candidate_dir(&runtime_root, &debt).unwrap();
        assert_eq!(
            fs::read(candidate.join("root/opt/lifecycle-created")).unwrap(),
            b"exact lifecycle output"
        );
        let captured = load_publication_candidate(&runtime_root, &debt).unwrap();
        assert!(
            captured
                .generation
                .entries
                .iter()
                .any(|entry| entry.path == "/opt/lifecycle-created")
        );

        remove_terminal_publication_candidates(&conn, &runtime_root, std::slice::from_ref(&debt))
            .unwrap();
        assert!(candidate.exists());
        debt.set_phase(
            &conn,
            conary_core::db::models::GenerationPublicationPhase::DatabaseBackedUp,
            GenerationPublicationStatus::Running,
            Some(1),
            Some(1),
        )
        .unwrap();
        debt.mark_complete_through(&conn, None, 1, 1).unwrap();
        remove_terminal_publication_candidates(&conn, &runtime_root, &[debt]).unwrap();
        assert!(!candidate.exists());
    }

    #[test]
    fn later_selected_root_candidate_cumulates_prior_pending_effects() {
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
            .persist_for_publication(&runtime_root, &first_debt)
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
            .persist_for_publication(&runtime_root, &second_debt)
            .unwrap();

        let latest = load_publication_candidate(&runtime_root, &second_debt).unwrap();
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
        let still_retryable = load_publication_candidate(&runtime_root, &retried).unwrap();
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
        first.persist_for_publication(&runtime_root, &debt).unwrap();
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
    fn abandoned_selected_root_candidate_is_removed_deterministically() {
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
            .persist_for_publication(&runtime_root, &debt)
            .unwrap();
        let candidate = publication_candidate_dir(&runtime_root, &debt).unwrap();
        assert!(candidate.is_dir());

        assert_eq!(
            GenerationPublication::abandon_recoverable_for_changeset(&conn, changeset_id).unwrap(),
            1
        );
        remove_terminal_publication_candidates(&conn, &runtime_root, &[debt]).unwrap();
        assert!(!candidate.exists());
    }
}
