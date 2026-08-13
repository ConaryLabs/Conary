// apps/conary/src/commands/live_root.rs

use anyhow::{Context, Result, bail};
use conary_core::filesystem::durable::write_json_atomic;
use conary_core::generation::root_manifest::{apply_resolved_payload_metadata, capture_root_node};
use conary_core::payload::{PayloadNodeKind, ResolvedPayloadNode};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixListener;
use std::path::{Component, Path, PathBuf};

mod content;
mod durability;
mod path;
mod recovery;
pub(crate) use content::LiveRootContent;
use durability::*;
pub(crate) use path::target_path;
pub(crate) use recovery::recover_pending_journals;
#[cfg(test)]
pub(crate) use recovery::recover_pending_journals_with_changesets;

const JOURNAL_SCHEMA: &str = "conary.live-root-journal.v2";

#[derive(Debug, Clone)]
pub(crate) struct LiveRootFile {
    pub path: String,
    pub content: LiveRootContent,
    pub node: ResolvedPayloadNode,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct LiveRootStats {
    pub files_written: usize,
    pub files_removed: usize,
    pub dirs_created: usize,
    pub dirs_removed: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct LiveRootJournal {
    schema: String,
    tx_uuid: String,
    operation: String,
    state: String,
    backups: Vec<BackupRecord>,
    created_paths: Vec<String>,
    removed_dirs: Vec<String>,
    modified_directories: Vec<DirectoryMetadataRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackupRecord {
    path: String,
    backup_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DirectoryMetadataRecord {
    path: String,
    node: ResolvedPayloadNode,
}

pub(crate) struct LiveRootTransaction {
    root: PathBuf,
    journal_path: PathBuf,
    tx_uuid: String,
    operation: String,
    backups: Vec<BackupRecord>,
    created_paths: Vec<PathBuf>,
    removed_dirs: Vec<PathBuf>,
    modified_directories: Vec<DirectoryMetadataRecord>,
    recovery: LiveRootRecovery,
    committed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveRootRecovery {
    Journaled,
    DisposableOverlay,
}

use path::selected_root_target_path;

impl LiveRootTransaction {
    pub(crate) fn begin(
        runtime_root: &Path,
        root: &Path,
        tx_uuid: String,
        operation: impl Into<String>,
    ) -> Result<Self> {
        Self::begin_with_recovery(
            runtime_root,
            root,
            tx_uuid,
            operation,
            LiveRootRecovery::Journaled,
        )
    }

    /// Mutate a transaction-owned overlay without duplicating rollback state.
    ///
    /// The upper is the sole rollback record and its owner must discard it on
    /// failure. Moving lower-backed nodes into an external journal would cross
    /// filesystems and would duplicate authority even where rename succeeded.
    pub(crate) fn begin_disposable_overlay(
        runtime_root: &Path,
        root: &Path,
        tx_uuid: String,
        operation: impl Into<String>,
    ) -> Result<Self> {
        Self::begin_with_recovery(
            runtime_root,
            root,
            tx_uuid,
            operation,
            LiveRootRecovery::DisposableOverlay,
        )
    }

    fn begin_with_recovery(
        runtime_root: &Path,
        root: &Path,
        tx_uuid: String,
        operation: impl Into<String>,
        recovery: LiveRootRecovery,
    ) -> Result<Self> {
        validate_tx_uuid(&tx_uuid)?;
        let journal_dir = runtime_root.join("live-root-journals");
        if recovery == LiveRootRecovery::Journaled {
            create_dir_all_and_sync(&journal_dir)?;
        }
        let operation = operation.into();
        let journal_path = journal_dir.join(format!("{tx_uuid}.json"));
        let transaction = Self {
            root: root.to_path_buf(),
            journal_path,
            tx_uuid,
            operation,
            backups: Vec::new(),
            created_paths: Vec::new(),
            removed_dirs: Vec::new(),
            modified_directories: Vec::new(),
            recovery,
            committed: false,
        };
        transaction.write_journal("pending")?;
        Ok(transaction)
    }

    pub(crate) fn apply_install_files(&mut self, files: &[LiveRootFile]) -> Result<LiveRootStats> {
        self.apply_install_files_with_references(files, &[])
    }

    /// Apply payload mutations whose hardlink graph may target exact,
    /// already-materialized regular files.
    ///
    /// References participate in graph and content validation but are never
    /// backed up, rewritten, or reported as mutations.
    pub(crate) fn apply_install_files_with_references(
        &mut self,
        files: &[LiveRootFile],
        references: &[LiveRootFile],
    ) -> Result<LiveRootStats> {
        preflight_install_files(files, references)?;
        for reference in references {
            verify_preserved_reference(&self.root, reference)?;
        }
        let mut stats = LiveRootStats::default();
        let mut directories = files
            .iter()
            .filter(|file| matches!(file.node.source.kind, PayloadNodeKind::Directory))
            .collect::<Vec<_>>();
        directories.sort_by_key(|file| file.path.matches('/').count());
        for file in &directories {
            self.apply_directory(file, &mut stats)?;
        }

        let mut completed = references
            .iter()
            .map(|reference| reference.path.as_str())
            .collect::<BTreeSet<_>>();
        for file in files.iter().filter(|file| {
            !matches!(
                file.node.source.kind,
                PayloadNodeKind::Directory | PayloadNodeKind::Hardlink { .. }
            )
        }) {
            self.apply_leaf(file, &mut stats)?;
            completed.insert(file.path.as_str());
        }

        let mut pending = files
            .iter()
            .filter(|file| matches!(file.node.source.kind, PayloadNodeKind::Hardlink { .. }))
            .collect::<Vec<_>>();
        while !pending.is_empty() {
            let before = pending.len();
            let mut next = Vec::new();
            for file in pending {
                let PayloadNodeKind::Hardlink { target, .. } = &file.node.source.kind else {
                    unreachable!("pending list contains only hardlinks");
                };
                if completed.contains(target.as_str()) {
                    self.apply_leaf(file, &mut stats)?;
                    completed.insert(file.path.as_str());
                } else {
                    next.push(file);
                }
            }
            if next.len() == before {
                bail!("payload hardlink graph contains a cycle");
            }
            pending = next;
        }

        directories.sort_by_key(|file| std::cmp::Reverse(file.path.matches('/').count()));
        for file in directories {
            let target = selected_root_target_path(&self.root, &file.path)?;
            apply_resolved_payload_metadata(&target, &file.node)
                .with_context(|| format!("Failed to apply metadata for {}", file.path))?;
            sync_directory(&target)?;
            self.write_journal("in_progress")?;
        }
        Ok(stats)
    }

    fn apply_directory(&mut self, file: &LiveRootFile, stats: &mut LiveRootStats) -> Result<()> {
        let target = selected_root_target_path(&self.root, &file.path)?;
        self.ensure_parent(&target, stats)?;
        match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.file_type().is_dir() => {
                if self.recovery == LiveRootRecovery::Journaled
                    && !self
                        .modified_directories
                        .iter()
                        .any(|record| record.path == target.to_string_lossy())
                {
                    self.modified_directories.push(DirectoryMetadataRecord {
                        path: target.to_string_lossy().into_owned(),
                        node: capture_root_node(&target).with_context(|| {
                            format!("Failed to capture existing directory {}", target.display())
                        })?,
                    });
                }
            }
            Ok(_) => {
                self.backup_existing(&target)?;
                if self.recovery == LiveRootRecovery::Journaled {
                    self.created_paths.push(target.clone());
                }
                self.write_journal("in_progress")?;
                create_dir_and_sync(&target)?;
                stats.dirs_created += 1;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.backup_existing(&target)?;
                create_dir_and_sync(&target)?;
                stats.dirs_created += 1;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to inspect {}", target.display()));
            }
        }
        self.write_journal("in_progress")
    }

    fn apply_leaf(&mut self, file: &LiveRootFile, stats: &mut LiveRootStats) -> Result<()> {
        let target = selected_root_target_path(&self.root, &file.path)?;
        self.ensure_parent(&target, stats)?;
        reject_existing_directory_target(&target)?;
        self.backup_existing(&target)?;

        let temp = temp_path_for(&target, &self.tx_uuid)?;
        match fs::symlink_metadata(&temp) {
            Ok(metadata) if metadata.file_type().is_dir() => fs::remove_dir(&temp)?,
            Ok(_) => fs::remove_file(&temp)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        create_live_root_leaf(&self.root, &temp, file)?;
        apply_resolved_payload_metadata(&temp, &file.node)
            .with_context(|| format!("Failed to apply metadata for {}", file.path))?;
        rename_and_sync(&temp, &target)
            .with_context(|| format!("Failed to move payload node {}", target.display()))?;
        stats.files_written += 1;
        self.write_journal("in_progress")
    }

    pub(crate) fn apply_remove_paths(&mut self, package_paths: &[String]) -> Result<LiveRootStats> {
        let mut stats = LiveRootStats::default();
        let mut dirs = Vec::new();
        for package_path in package_paths {
            let target = selected_root_target_path(&self.root, package_path)?;
            validate_existing_parent(&self.root, &target)?;
            match fs::symlink_metadata(&target) {
                Ok(meta) if meta.is_dir() => dirs.push(target),
                Ok(_) => {
                    self.backup_existing(&target)?;
                    stats.files_removed += 1;
                    self.write_journal("in_progress")?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("Failed to inspect {}", target.display()));
                }
            }
        }

        dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        dirs.dedup();
        for dir in dirs {
            if self.recovery == LiveRootRecovery::Journaled {
                self.removed_dirs.push(dir.clone());
            }
            self.write_journal("in_progress")?;
            match remove_dir_and_sync(&dir) {
                Ok(()) => stats.dirs_removed += 1,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                    ) => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("Failed to remove {}", dir.display()));
                }
            }
        }
        Ok(stats)
    }

    pub(crate) fn rollback(&mut self) -> Result<()> {
        if self.recovery == LiveRootRecovery::DisposableOverlay {
            self.committed = true;
            return Ok(());
        }
        for created in self.created_paths.iter().rev() {
            if validate_existing_parent(&self.root, created).is_err() {
                continue;
            }
            match fs::symlink_metadata(created) {
                Ok(meta) if meta.is_dir() => {
                    let _ = remove_dir_and_sync(created);
                }
                Ok(_) => {
                    let _ = remove_file_and_sync(created);
                }
                Err(_) => {}
            }
        }
        for dir in self.removed_dirs.iter().rev() {
            ensure_safe_directory(&self.root, dir)?;
        }
        for backup in self.backups.iter().rev() {
            let target = PathBuf::from(&backup.path);
            let backup_path = PathBuf::from(&backup.backup_path);
            if backup_path.exists() {
                ensure_safe_parent(&self.root, &target)?;
                rename_and_sync(&backup_path, &target)?;
            }
        }
        for directory in self.modified_directories.iter().rev() {
            let path = PathBuf::from(&directory.path);
            if validate_existing_parent(&self.root, &path).is_ok()
                && fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_dir())
            {
                apply_resolved_payload_metadata(&path, &directory.node)?;
                sync_directory(&path)?;
            }
        }
        self.write_journal("rolled_back")?;
        self.cleanup_transaction_files()?;
        self.committed = true;
        Ok(())
    }

    pub(crate) fn mark_committed_for_recovery(&mut self) -> Result<()> {
        self.write_journal("committed")
    }

    pub(crate) fn commit(mut self) -> Result<()> {
        if let Err(error) = self.mark_committed_for_recovery() {
            self.committed = true;
            self.cleanup_transaction_files().with_context(|| {
                format!(
                    "Failed to cleanup live-root recovery journal after committed marker write failed: {error}"
                )
            })?;
            return Err(error).context(
                "Failed to mark live-root transaction committed; recovery journal was removed",
            );
        }
        self.committed = true;
        self.cleanup_transaction_files()?;
        Ok(())
    }

    fn ensure_parent(&mut self, target: &Path, stats: &mut LiveRootStats) -> Result<()> {
        let Some(parent) = target.parent() else {
            return Ok(());
        };
        let mut current = PathBuf::new();
        for component in parent
            .strip_prefix(&self.root)
            .unwrap_or(parent)
            .components()
        {
            current.push(component.as_os_str());
            let full = self.root.join(&current);
            match fs::symlink_metadata(&full) {
                Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                    bail!("unsafe parent {} for live-root path", full.display());
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if self.recovery == LiveRootRecovery::Journaled {
                        self.created_paths.push(full.clone());
                    }
                    self.write_journal("in_progress")?;
                    create_dir_and_sync(&full)?;
                    stats.dirs_created += 1;
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("Failed to inspect {}", full.display()));
                }
            }
        }
        Ok(())
    }

    fn backup_existing(&mut self, target: &Path) -> Result<()> {
        if self.recovery == LiveRootRecovery::DisposableOverlay {
            return match fs::symlink_metadata(target) {
                Ok(metadata) if metadata.file_type().is_dir() => bail!(
                    "disposable selected-root leaf {} became a directory",
                    target.display()
                ),
                Ok(_) => remove_file_and_sync(target).map_err(Into::into),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            };
        }
        if self.created_paths.iter().any(|created| created == target)
            || self
                .backups
                .iter()
                .any(|backup| Path::new(&backup.path) == target)
        {
            match fs::symlink_metadata(target) {
                Ok(meta) if meta.is_dir() => {
                    bail!(
                        "tracked live-root transaction path {} became a directory",
                        target.display()
                    );
                }
                Ok(_) => remove_file_and_sync(target)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            return Ok(());
        }
        match fs::symlink_metadata(target) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.created_paths.push(target.to_path_buf());
                self.write_journal("in_progress")?;
                return Ok(());
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to inspect {}", target.display()));
            }
        }
        let backup_dir = self.journal_path.with_extension("backups");
        create_dir_all_and_sync(&backup_dir)?;
        let backup_path = backup_dir.join(format!("backup-{}", self.backups.len()));
        self.backups.push(BackupRecord {
            path: target.to_string_lossy().into_owned(),
            backup_path: backup_path.to_string_lossy().into_owned(),
        });
        self.write_journal("in_progress")?;
        rename_and_sync(target, &backup_path)?;
        Ok(())
    }

    fn write_journal(&self, state: &str) -> Result<()> {
        if self.recovery == LiveRootRecovery::DisposableOverlay {
            return Ok(());
        }
        let journal = LiveRootJournal {
            schema: JOURNAL_SCHEMA.to_string(),
            tx_uuid: self.tx_uuid.clone(),
            operation: self.operation.clone(),
            state: state.to_string(),
            backups: self.backups.clone(),
            created_paths: self
                .created_paths
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            removed_dirs: self
                .removed_dirs
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            modified_directories: self.modified_directories.clone(),
        };
        let journal_dir = self
            .journal_path
            .parent()
            .context("live-root journal path has no parent")?;
        create_dir_all_and_sync(journal_dir)?;
        write_json_atomic(&self.journal_path, &journal).with_context(|| {
            format!(
                "Failed to replace live-root journal {}",
                self.journal_path.display()
            )
        })?;
        Ok(())
    }

    fn cleanup_transaction_files(&self) -> Result<()> {
        if self.recovery == LiveRootRecovery::DisposableOverlay {
            return Ok(());
        }
        match remove_file_and_sync(&self.journal_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to remove {}", self.journal_path.display()));
            }
        }
        let backup_dir = self.journal_path.with_extension("backups");
        match remove_dir_all_and_sync(&backup_dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to remove {}", backup_dir.display()));
            }
        }
        if let Some(journal_dir) = self.journal_path.parent() {
            sync_directory(journal_dir)?;
        }
        Ok(())
    }
}

impl Drop for LiveRootTransaction {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.rollback();
        }
    }
}

fn validate_tx_uuid(tx_uuid: &str) -> Result<()> {
    let mut components = Path::new(tx_uuid).components();
    let valid_single_component =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    if tx_uuid.is_empty()
        || tx_uuid.contains('/')
        || tx_uuid.contains('\\')
        || !valid_single_component
    {
        bail!("invalid live-root transaction id {tx_uuid:?}");
    }
    Ok(())
}

fn preflight_install_files(files: &[LiveRootFile], references: &[LiveRootFile]) -> Result<()> {
    let mut by_path = BTreeMap::new();
    for file in references.iter().chain(files) {
        target_path(Path::new("/"), &file.path)?;
        file.node
            .validate()
            .with_context(|| format!("Invalid payload node {}", file.path))?;
        match file.node.source.kind {
            PayloadNodeKind::Regular { .. } if file.content.is_absent() => {
                bail!("regular payload node {} has no content stream", file.path);
            }
            PayloadNodeKind::Regular { .. } => {}
            _ if !file.content.is_absent() => {
                bail!(
                    "non-regular payload node {} carries a content stream",
                    file.path
                );
            }
            _ => {}
        }
        if by_path.insert(file.path.as_str(), file).is_some() {
            bail!("payload path {} is declared more than once", file.path);
        }
    }

    let hardlink_targets = files
        .iter()
        .filter_map(|file| match &file.node.source.kind {
            PayloadNodeKind::Hardlink { target, .. } => Some(target.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    for reference in references {
        if !matches!(reference.node.source.kind, PayloadNodeKind::Regular { .. }) {
            bail!(
                "preserved hardlink reference {} is not a regular payload node",
                reference.path
            );
        }
        if !hardlink_targets.contains(reference.path.as_str()) {
            bail!(
                "preserved payload reference {} is not targeted by this hardlink graph",
                reference.path
            );
        }
    }

    for file in files {
        let mut parent = Path::new(&file.path).parent();
        while let Some(path) = parent {
            let Some(path) = path.to_str() else {
                bail!("payload path is not UTF-8: {}", file.path);
            };
            if let Some(ancestor) = by_path.get(path)
                && !matches!(ancestor.node.source.kind, PayloadNodeKind::Directory)
            {
                bail!(
                    "payload path {} is below non-directory payload node {}",
                    file.path,
                    path
                );
            }
            parent = path_parent_above_root(path);
        }

        let PayloadNodeKind::Hardlink { target, identity } = &file.node.source.kind else {
            continue;
        };
        let target_file = by_path.get(target.as_str()).ok_or_else(|| {
            anyhow::anyhow!(
                "payload hardlink {} targets undeclared path {}",
                file.path,
                target
            )
        })?;
        let target_identity = match &target_file.node.source.kind {
            PayloadNodeKind::Regular {
                hardlink_identity: Some(target_identity),
            }
            | PayloadNodeKind::Hardlink {
                identity: target_identity,
                ..
            } => target_identity,
            _ => {
                bail!(
                    "payload hardlink {} target {} does not declare hardlink identity {}",
                    file.path,
                    target,
                    identity
                );
            }
        };
        if target_identity != identity {
            bail!(
                "payload hardlink {} identity {} differs from target {} identity {}",
                file.path,
                identity,
                target,
                target_identity
            );
        }
        if file.node.uid != target_file.node.uid
            || file.node.gid != target_file.node.gid
            || file.node.source.mode != target_file.node.source.mode
            || file.node.source.mtime != target_file.node.source.mtime
            || file.node.source.xattrs != target_file.node.source.xattrs
        {
            bail!(
                "payload hardlink {} metadata differs from target {}",
                file.path,
                target
            );
        }
    }
    Ok(())
}

fn verify_preserved_reference(root: &Path, reference: &LiveRootFile) -> Result<()> {
    let path = selected_root_target_path(root, &reference.path)?;
    let metadata = fs::symlink_metadata(&path).with_context(|| {
        format!(
            "preserved hardlink target {} is unavailable",
            reference.path
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        bail!(
            "preserved hardlink target {} is not a regular file",
            reference.path
        );
    }
    let authority = reference.content.authority().with_context(|| {
        format!(
            "preserved hardlink target {} has no content authority",
            reference.path
        )
    })?;
    if metadata.len() != authority.size {
        bail!(
            "preserved hardlink target {} size changed: expected {}, got {}",
            reference.path,
            authority.size,
            metadata.len()
        );
    }
    let mut input = File::open(&path)
        .with_context(|| format!("open preserved hardlink target {}", reference.path))?;
    let digest = conary_core::hash::sha256_reader_hex(&mut input)
        .with_context(|| format!("hash preserved hardlink target {}", reference.path))?;
    if digest != authority.sha256 {
        bail!(
            "preserved hardlink target {} digest changed: expected {}, got {}",
            reference.path,
            authority.sha256,
            digest
        );
    }
    Ok(())
}

fn path_parent_above_root(path: &str) -> Option<&Path> {
    let parent = Path::new(path).parent()?;
    (parent != Path::new("/") && !parent.as_os_str().is_empty()).then_some(parent)
}

fn create_live_root_leaf(root: &Path, path: &Path, file: &LiveRootFile) -> Result<()> {
    match &file.node.source.kind {
        PayloadNodeKind::Regular { .. } => {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .with_context(|| format!("Failed to create {}", path.display()))?;
            file.content
                .copy_verified_to(&mut output)
                .with_context(|| format!("Failed to write {}", path.display()))?;
            output
                .sync_all()
                .with_context(|| format!("Failed to sync {}", path.display()))?;
        }
        PayloadNodeKind::Symlink { target } => {
            std::os::unix::fs::symlink(target, path)
                .with_context(|| format!("Failed to create symlink {}", path.display()))?;
        }
        PayloadNodeKind::Hardlink { target, .. } => {
            let target = selected_root_target_path(root, target)?;
            let metadata = fs::symlink_metadata(&target).with_context(|| {
                format!(
                    "payload hardlink {} target is unavailable: {}",
                    file.path,
                    target.display()
                )
            })?;
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                bail!(
                    "payload hardlink {} target is not a regular file: {}",
                    file.path,
                    target.display()
                );
            }
            fs::hard_link(&target, path).with_context(|| {
                format!(
                    "Failed to create hardlink {} to {}",
                    path.display(),
                    target.display()
                )
            })?;
        }
        PayloadNodeKind::BlockDevice { major, minor } => {
            create_live_root_device(path, libc::S_IFBLK, *major, *minor)?;
        }
        PayloadNodeKind::CharacterDevice { major, minor } => {
            create_live_root_device(path, libc::S_IFCHR, *major, *minor)?;
        }
        PayloadNodeKind::Fifo => {
            let c_path = c_path(path)?;
            if unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) } != 0 {
                return Err(io::Error::last_os_error())
                    .with_context(|| format!("Failed to create FIFO {}", path.display()));
            }
        }
        PayloadNodeKind::Socket => {
            let socket = UnixListener::bind(path)
                .with_context(|| format!("Failed to create socket {}", path.display()))?;
            drop(socket);
        }
        PayloadNodeKind::Directory => unreachable!("directories use apply_directory"),
    }
    Ok(())
}

fn create_live_root_device(path: &Path, kind: libc::mode_t, major: u64, minor: u64) -> Result<()> {
    let major = libc::c_uint::try_from(major)
        .with_context(|| format!("device major is not representable at {}", path.display()))?;
    let minor = libc::c_uint::try_from(minor)
        .with_context(|| format!("device minor is not representable at {}", path.display()))?;
    let c_path = c_path(path)?;
    if unsafe { libc::mknod(c_path.as_ptr(), kind | 0o600, libc::makedev(major, minor)) } != 0 {
        return Err(io::Error::last_os_error())
            .with_context(|| format!("Failed to create device {}", path.display()));
    }
    Ok(())
}

fn c_path(path: &Path) -> Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("filesystem path contains NUL: {}", path.display()))
}

fn reject_existing_directory_target(target: &Path) -> Result<()> {
    match fs::symlink_metadata(target) {
        Ok(meta) if meta.is_dir() => {
            bail!(
                "live-root install refuses to replace existing directory {}",
                target.display()
            );
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("Failed to inspect {}", target.display())),
    }
}

fn temp_path_for(target: &Path, tx_uuid: &str) -> Result<PathBuf> {
    let parent = target
        .parent()
        .context("live-root target path has no parent")?;
    let name = target
        .file_name()
        .context("live-root target path has no file name")?
        .to_string_lossy();
    Ok(parent.join(format!(".{name}.conary-tmp-{tx_uuid}")))
}

fn validate_existing_parent(root: &Path, target: &Path) -> Result<()> {
    let Some(parent) = target.parent() else {
        return Ok(());
    };
    validate_parent_components(root, parent)
}

fn validate_parent_components(root: &Path, parent: &Path) -> Result<()> {
    let root_meta = fs::symlink_metadata(root)
        .with_context(|| format!("Failed to inspect target root {}", root.display()))?;
    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        bail!("unsafe parent {} for live-root path", root.display());
    }
    let relative = parent.strip_prefix(root).with_context(|| {
        format!(
            "live-root path {} is not below target root {}",
            parent.display(),
            root.display()
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(part) => current.push(part),
            Component::CurDir => continue,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!(
                    "live-root path {} escapes the target root",
                    parent.display()
                );
            }
        }
        let meta = fs::symlink_metadata(&current)
            .with_context(|| format!("Failed to inspect {}", current.display()))?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            bail!("unsafe parent {} for live-root path", current.display());
        }
    }
    Ok(())
}

fn validate_existing_or_removed_parent(
    root: &Path,
    target: &Path,
    removed_dirs: &[PathBuf],
) -> Result<()> {
    let Some(parent) = target.parent() else {
        return Ok(());
    };
    let root_meta = fs::symlink_metadata(root)
        .with_context(|| format!("Failed to inspect target root {}", root.display()))?;
    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        bail!("unsafe parent {} for live-root path", root.display());
    }
    let relative = parent.strip_prefix(root).with_context(|| {
        format!(
            "live-root path {} is not below target root {}",
            parent.display(),
            root.display()
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(part) => current.push(part),
            Component::CurDir => continue,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!(
                    "live-root path {} escapes the target root",
                    parent.display()
                );
            }
        }
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                bail!("unsafe parent {} for live-root path", current.display());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if removed_dirs.iter().any(|dir| current.starts_with(dir)) {
                    return Ok(());
                }
                return Err(error)
                    .with_context(|| format!("Failed to inspect {}", current.display()));
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to inspect {}", current.display()));
            }
        }
    }
    Ok(())
}

fn ensure_safe_parent(root: &Path, target: &Path) -> Result<()> {
    let parent = target
        .parent()
        .context("live-root target path has no parent")?;
    ensure_safe_directory(root, parent)
}

fn ensure_safe_directory(root: &Path, dir: &Path) -> Result<()> {
    let root_meta = fs::symlink_metadata(root)
        .with_context(|| format!("Failed to inspect target root {}", root.display()))?;
    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        bail!("unsafe parent {} for live-root path", root.display());
    }
    let relative = dir.strip_prefix(root).with_context(|| {
        format!(
            "live-root path {} is not below target root {}",
            dir.display(),
            root.display()
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(part) => current.push(part),
            Component::CurDir => continue,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("live-root path {} escapes the target root", dir.display());
            }
        }
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                bail!("unsafe parent {} for live-root path", current.display());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                create_dir_and_sync(&current)
                    .with_context(|| format!("Failed to create {}", current.display()))?;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to inspect {}", current.display()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "live_root/tests.rs"]
mod tests;
