// src/commands/generation/selected_root.rs

//! Rollback-safe writable roots for generation-aware transaction execution.

use crate::commands::{LiveRootFile, LiveRootTransaction};
use anyhow::{Context, Result, bail};
use conary_core::db::models::GenerationPublication;
use conary_core::filesystem::CasStore;
use conary_core::generation::root_manifest::{
    CapturedSelectedRoot, GenerationRootManifest, MutableStateManifest,
    materialize_captured_selected_root, scan_selected_root,
};
use conary_core::runtime_root::ConaryRuntimeRoot;
use std::fs;
use std::path::{Path, PathBuf};

/// One isolated selected-root view backed by a single filesystem journal.
///
/// The caller retains its SQLite transaction. This session never changes the
/// host's `current` generation link; final generation publication happens only
/// after the caller commits the database.
pub(crate) struct SelectedRootSession {
    session_dir: PathBuf,
    selected_root: PathBuf,
    transaction: Option<LiveRootTransaction>,
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
        let selected_root = session_dir.join("root");
        fs::create_dir_all(&selected_root).with_context(|| {
            format!(
                "failed to create selected generation root {}",
                selected_root.display()
            )
        })?;
        if let Err(error) = materialize_current_root(conn, runtime_root, &selected_root) {
            let _ = fs::remove_dir_all(&session_dir);
            return Err(error);
        }
        let transaction =
            LiveRootTransaction::begin(runtime_root.root(), &selected_root, session_id, operation)?;
        Ok(Self {
            session_dir,
            selected_root,
            transaction: Some(transaction),
        })
    }

    pub(crate) fn selected_root(&self) -> &Path {
        &self.selected_root
    }

    pub(crate) fn apply_install_files(&mut self, files: &[LiveRootFile]) -> Result<()> {
        self.transaction_mut()?.apply_install_files(files)?;
        Ok(())
    }

    pub(crate) fn apply_remove_paths(&mut self, paths: &[String]) -> Result<()> {
        self.transaction_mut()?.apply_remove_paths(paths)?;
        Ok(())
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
        let captured = scan_selected_root(&self.selected_root, &cas)?;
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
            let captured = scan_selected_root(&self.selected_root, &cas)?;
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
) -> Result<()> {
    let cas = CasStore::new(runtime_root.objects_dir())?;
    if let Some(candidate) = latest_selected_root_candidate(conn, runtime_root)? {
        return materialize_captured_selected_root(&candidate, &cas, selected_root)
            .map_err(Into::into);
    }

    if let Some(generation) =
        conary_core::generation::mount::current_generation(runtime_root.root())?
    {
        let artifact = conary_core::generation::artifact::load_generation_artifact(
            &runtime_root.generation_path(generation),
        )?;
        return materialize_captured_selected_root(
            &CapturedSelectedRoot {
                generation: artifact.generation_root,
                state: artifact.mutable_state,
            },
            &cas,
            selected_root,
        )
        .map_err(Into::into);
    }

    conary_core::generation::builder::materialize_selected_root_from_db(
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
    if fs::symlink_metadata(&candidate).is_ok() {
        bail!(
            "selected-root publication candidate already exists: {}",
            candidate.display()
        );
    }
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
            content: content.to_vec(),
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
            "1".to_string(),
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
        GenerationPublication::mark_complete_through(&conn, None, 1, 1).unwrap();
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
