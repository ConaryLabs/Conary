// apps/conary/src/commands/live_root/recovery.rs

//! Durable recovery of incomplete live-root filesystem journals.

use super::*;

pub(crate) fn recover_pending_journals(runtime_root: &Path, root: &Path) -> Result<()> {
    recover_pending_journals_by(runtime_root, root, |_| {
        Ok(JournalRecoveryDecision::Rollback)
    })
}

pub(crate) fn recover_pending_journals_with_changesets(
    runtime_root: &Path,
    root: &Path,
    conn: &rusqlite::Connection,
) -> Result<()> {
    use conary_core::db::models::{Changeset, ChangesetStatus};

    recover_pending_journals_by(runtime_root, root, |journal| {
        let changeset = Changeset::find_by_tx_uuid(conn, &journal.tx_uuid)?;
        Ok(match changeset.map(|changeset| changeset.status) {
            Some(ChangesetStatus::Applied) => JournalRecoveryDecision::Cleanup,
            Some(ChangesetStatus::Pending | ChangesetStatus::RolledBack) | None => {
                JournalRecoveryDecision::Rollback
            }
        })
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JournalRecoveryDecision {
    Cleanup,
    Rollback,
}

fn recover_pending_journals_by(
    runtime_root: &Path,
    root: &Path,
    decide: impl Fn(&LiveRootJournal) -> Result<JournalRecoveryDecision>,
) -> Result<()> {
    let journal_dir = runtime_root.join("live-root-journals");
    if !journal_dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&journal_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let raw = fs::read(&path)?;
        let journal: LiveRootJournal = serde_json::from_slice(&raw)?;
        if journal.schema != JOURNAL_SCHEMA {
            bail!(
                "unsupported live-root journal schema {} in {}",
                journal.schema,
                path.display()
            );
        }
        validate_recovered_journal_tx_uuid(&path, &journal.tx_uuid)?;
        validate_recovered_backup_records(
            root,
            &path,
            &journal.backups,
            &journal.removed_dirs,
            &journal.modified_directories,
        )?;
        if journal.state == "committed" || journal.state == "rolled_back" {
            cleanup_recovered_journal_files(&path)?;
            continue;
        }
        match decide(&journal)? {
            JournalRecoveryDecision::Cleanup => cleanup_recovered_journal_files(&path)?,
            JournalRecoveryDecision::Rollback => {
                let mut tx = live_root_transaction_from_journal(root, path, journal);
                tx.rollback()?;
            }
        }
    }
    Ok(())
}

fn live_root_transaction_from_journal(
    root: &Path,
    journal_path: PathBuf,
    journal: LiveRootJournal,
) -> LiveRootTransaction {
    LiveRootTransaction {
        root: root.to_path_buf(),
        journal_path,
        tx_uuid: journal.tx_uuid,
        operation: journal.operation,
        backups: journal.backups,
        created_paths: journal
            .created_paths
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        removed_dirs: journal
            .removed_dirs
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        modified_directories: journal.modified_directories,
        committed: false,
    }
}

fn cleanup_recovered_journal_files(path: &Path) -> Result<()> {
    match remove_file_and_sync(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to remove {}", path.display()));
        }
    }
    match remove_dir_all_and_sync(&path.with_extension("backups")) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Failed to remove {}",
                    path.with_extension("backups").display()
                )
            });
        }
    }
    if let Some(journal_dir) = path.parent() {
        sync_directory(journal_dir)?;
    }
    Ok(())
}

fn validate_recovered_journal_tx_uuid(path: &Path, tx_uuid: &str) -> Result<()> {
    validate_tx_uuid(tx_uuid)?;
    let filename_tx_uuid = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .context("live-root journal filename has no valid transaction id")?;
    if tx_uuid != filename_tx_uuid {
        bail!(
            "live-root journal transaction id {tx_uuid:?} does not match journal filename {filename_tx_uuid:?}"
        );
    }
    Ok(())
}

fn validate_recovered_backup_records(
    root: &Path,
    journal_path: &Path,
    backups: &[BackupRecord],
    removed_dirs: &[String],
    modified_directories: &[DirectoryMetadataRecord],
) -> Result<()> {
    let backup_dir = journal_path.with_extension("backups");
    let removed_dirs = removed_dirs
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<PathBuf>>();
    for (index, backup) in backups.iter().enumerate() {
        let target = PathBuf::from(&backup.path);
        validate_recovered_backup_target(root, &target, &removed_dirs)?;
        let backup_path = PathBuf::from(&backup.backup_path);
        let expected_backup_path = backup_dir.join(format!("backup-{index}"));
        if backup_path != expected_backup_path {
            bail!(
                "invalid live-root backup path {} for journal {}; expected {}",
                backup_path.display(),
                journal_path.display(),
                expected_backup_path.display()
            );
        }
    }
    let mut seen_directories = BTreeSet::new();
    for directory in modified_directories {
        let target = PathBuf::from(&directory.path);
        validate_recovered_backup_target(root, &target, &removed_dirs)?;
        if !seen_directories.insert(directory.path.as_str()) {
            bail!(
                "live-root journal records directory {} more than once",
                target.display()
            );
        }
        directory.node.validate()?;
        if !matches!(directory.node.source.kind, PayloadNodeKind::Directory) {
            bail!(
                "live-root journal metadata for {} is not a directory",
                target.display()
            );
        }
    }
    Ok(())
}

fn validate_recovered_backup_target(
    root: &Path,
    target: &Path,
    removed_dirs: &[PathBuf],
) -> Result<()> {
    target.strip_prefix(root).with_context(|| {
        format!(
            "live-root path {} is not below target root {}",
            target.display(),
            root.display()
        )
    })?;
    validate_existing_or_removed_parent(root, target, removed_dirs)
}
