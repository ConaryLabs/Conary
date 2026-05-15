// apps/conary/src/commands/live_root.rs

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};

const JOURNAL_SCHEMA: &str = "conary.live-root-journal.v1";

#[derive(Debug, Clone)]
pub(crate) struct LiveRootFile {
    pub path: String,
    pub content: Vec<u8>,
    pub mode: i32,
    pub symlink_target: Option<String>,
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
    #[serde(default)]
    removed_dirs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackupRecord {
    path: String,
    backup_path: String,
}

pub(crate) struct LiveRootTransaction {
    root: PathBuf,
    journal_path: PathBuf,
    tx_uuid: String,
    operation: String,
    backups: Vec<BackupRecord>,
    created_paths: Vec<PathBuf>,
    removed_dirs: Vec<PathBuf>,
    committed: bool,
}

pub(crate) fn target_path(root: &Path, package_path: &str) -> Result<PathBuf> {
    let relative = package_path.strip_prefix('/').unwrap_or(package_path);
    let relative_path = Path::new(relative);
    let mut has_path_below_root = false;
    for component in relative_path.components() {
        match component {
            Component::Normal(_) => has_path_below_root = true,
            Component::CurDir => {
                bail!(
                    "package path {package_path} must name a file or directory below the target root"
                );
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("package path {package_path} escapes the target root");
            }
        }
    }
    if !has_path_below_root {
        bail!("package path {package_path} must name a file or directory below the target root");
    }
    Ok(root.join(relative_path))
}

impl LiveRootTransaction {
    pub(crate) fn begin(
        runtime_root: &Path,
        root: &Path,
        tx_uuid: String,
        operation: impl Into<String>,
    ) -> Result<Self> {
        validate_tx_uuid(&tx_uuid)?;
        let journal_dir = runtime_root.join("live-root-journals");
        fs::create_dir_all(&journal_dir)?;
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
            committed: false,
        };
        transaction.write_journal("pending")?;
        Ok(transaction)
    }

    pub(crate) fn apply_install_files(&mut self, files: &[LiveRootFile]) -> Result<LiveRootStats> {
        let mut stats = LiveRootStats::default();
        for file in files {
            let target = target_path(&self.root, &file.path)?;
            self.ensure_parent(&target, &mut stats)?;
            self.backup_existing(&target)?;

            let temp = temp_path_for(&target, &self.tx_uuid)?;
            if let Some(target_value) = file.symlink_target.as_deref() {
                symlink(target_value, &temp)
                    .with_context(|| format!("Failed to create symlink {}", temp.display()))?;
                fs::rename(&temp, &target)
                    .with_context(|| format!("Failed to move symlink {}", target.display()))?;
            } else {
                let mut temp_file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temp)
                    .with_context(|| format!("Failed to create {}", temp.display()))?;
                temp_file
                    .write_all(&file.content)
                    .with_context(|| format!("Failed to write {}", temp.display()))?;
                temp_file
                    .sync_all()
                    .with_context(|| format!("Failed to sync {}", temp.display()))?;
                fs::set_permissions(
                    &temp,
                    fs::Permissions::from_mode((file.mode as u32) & 0o7777),
                )?;
                fs::rename(&temp, &target)
                    .with_context(|| format!("Failed to move file {}", target.display()))?;
            }
            stats.files_written += 1;
            self.write_journal("in_progress")?;
        }
        Ok(stats)
    }

    pub(crate) fn apply_remove_paths(&mut self, package_paths: &[String]) -> Result<LiveRootStats> {
        let mut stats = LiveRootStats::default();
        let mut dirs = Vec::new();
        for package_path in package_paths {
            let target = target_path(&self.root, package_path)?;
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
            self.removed_dirs.push(dir.clone());
            self.write_journal("in_progress")?;
            match fs::remove_dir(&dir) {
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
        for created in self.created_paths.iter().rev() {
            if validate_existing_parent(&self.root, created).is_err() {
                continue;
            }
            match fs::symlink_metadata(created) {
                Ok(meta) if meta.is_dir() => {
                    let _ = fs::remove_dir(created);
                }
                Ok(_) => {
                    let _ = fs::remove_file(created);
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
                fs::rename(&backup_path, &target)?;
            }
        }
        self.write_journal("rolled_back")?;
        self.cleanup_transaction_files()?;
        self.committed = true;
        Ok(())
    }

    pub(crate) fn commit(mut self) -> Result<()> {
        self.committed = true;
        self.write_journal("committed")?;
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
                    self.created_paths.push(full.clone());
                    self.write_journal("in_progress")?;
                    fs::create_dir(&full)?;
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
        fs::create_dir_all(&backup_dir)?;
        let backup_path = backup_dir.join(format!("backup-{}", self.backups.len()));
        self.backups.push(BackupRecord {
            path: target.to_string_lossy().into_owned(),
            backup_path: backup_path.to_string_lossy().into_owned(),
        });
        self.write_journal("in_progress")?;
        fs::rename(target, &backup_path)?;
        Ok(())
    }

    fn write_journal(&self, state: &str) -> Result<()> {
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
        };
        let journal_dir = self
            .journal_path
            .parent()
            .context("live-root journal path has no parent")?;
        fs::create_dir_all(journal_dir)?;
        let temp_path = journal_dir.join(format!(".{}.json.tmp", self.tx_uuid));
        let _ = fs::remove_file(&temp_path);
        let mut temp = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .with_context(|| format!("Failed to create {}", temp_path.display()))?;
        temp.write_all(&serde_json::to_vec_pretty(&journal)?)?;
        temp.sync_all()
            .with_context(|| format!("Failed to sync {}", temp_path.display()))?;
        drop(temp);
        fs::rename(&temp_path, &self.journal_path).with_context(|| {
            format!(
                "Failed to replace live-root journal {}",
                self.journal_path.display()
            )
        })?;
        sync_directory(journal_dir)?;
        Ok(())
    }

    fn cleanup_transaction_files(&self) -> Result<()> {
        match fs::remove_file(&self.journal_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to remove {}", self.journal_path.display()));
            }
        }
        let backup_dir = self.journal_path.with_extension("backups");
        match fs::remove_dir_all(&backup_dir) {
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

pub(crate) fn recover_pending_journals(runtime_root: &Path, root: &Path) -> Result<()> {
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
        if journal.state == "committed" || journal.state == "rolled_back" {
            let _ = fs::remove_file(&path);
            let _ = fs::remove_dir_all(path.with_extension("backups"));
            continue;
        }
        let mut tx = LiveRootTransaction {
            root: root.to_path_buf(),
            journal_path: path,
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
            committed: false,
        };
        tx.rollback()?;
    }
    Ok(())
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
                fs::create_dir(&current)
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

fn sync_directory(path: &Path) -> Result<()> {
    let dir = File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    dir.sync_all()
        .with_context(|| format!("Failed to sync {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;
    use uuid::Uuid;

    #[test]
    fn target_path_rejects_parent_dir_escape() {
        let root = TempDir::new().unwrap();
        let err = target_path(root.path(), "/usr/../escape")
            .unwrap_err()
            .to_string();

        assert!(err.contains("escapes the target root"));
    }

    #[test]
    fn target_path_rejects_root_empty_and_current_dir_paths() {
        let root = TempDir::new().unwrap();

        for package_path in ["", "/", ".", "/."] {
            let err = target_path(root.path(), package_path)
                .unwrap_err()
                .to_string();

            assert!(
                err.contains("must name a file or directory below the target root"),
                "{package_path:?} returned {err}"
            );
        }
    }

    #[test]
    fn install_rejects_symlink_parent_without_writing_outside_root() {
        let temp = TempDir::new().unwrap();
        let runtime = temp.path().join("runtime");
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&runtime).unwrap();
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, root.join("usr")).unwrap();

        let mut tx = LiveRootTransaction::begin(
            &runtime,
            &root,
            Uuid::new_v4().to_string(),
            "install fixture",
        )
        .unwrap();
        let err = tx
            .apply_install_files(&[LiveRootFile {
                path: "/usr/bin/fixture".to_string(),
                content: b"fixture".to_vec(),
                mode: 0o100755,
                symlink_target: None,
            }])
            .unwrap_err()
            .to_string();

        assert!(err.contains("unsafe parent"));
        assert!(!outside.join("bin/fixture").exists());
    }

    #[test]
    fn remove_rejects_symlink_parent_without_removing_outside_root() {
        let temp = TempDir::new().unwrap();
        let runtime = temp.path().join("runtime");
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&runtime).unwrap();
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(outside.join("bin")).unwrap();
        fs::write(outside.join("bin/fixture"), "outside").unwrap();
        symlink(&outside, root.join("usr")).unwrap();

        let mut tx = LiveRootTransaction::begin(
            &runtime,
            &root,
            Uuid::new_v4().to_string(),
            "remove fixture",
        )
        .unwrap();
        let err = tx
            .apply_remove_paths(&["/usr/bin/fixture".to_string()])
            .unwrap_err()
            .to_string();

        assert!(err.contains("unsafe parent"));
        assert_eq!(
            fs::read_to_string(outside.join("bin/fixture")).unwrap(),
            "outside"
        );
    }

    #[test]
    fn begin_rejects_empty_or_path_like_transaction_ids() {
        let temp = TempDir::new().unwrap();
        let runtime = temp.path().join("runtime");
        let root = temp.path().join("root");
        fs::create_dir_all(&runtime).unwrap();
        fs::create_dir_all(&root).unwrap();

        for tx_uuid in ["", ".", "..", "../escape", "nested/id"] {
            let err =
                match LiveRootTransaction::begin(&runtime, &root, tx_uuid.to_string(), "install") {
                    Ok(_) => panic!("accepted invalid transaction id {tx_uuid:?}"),
                    Err(error) => error.to_string(),
                };

            assert!(err.contains("invalid live-root transaction id"));
        }
    }

    #[test]
    fn install_writes_regular_file_and_symlink() {
        let temp = TempDir::new().unwrap();
        let runtime = temp.path().join("runtime");
        let root = temp.path().join("root");
        fs::create_dir_all(&runtime).unwrap();
        fs::create_dir_all(&root).unwrap();
        let mut tx = LiveRootTransaction::begin(
            &runtime,
            &root,
            Uuid::new_v4().to_string(),
            "install fixture",
        )
        .unwrap();

        let stats = tx
            .apply_install_files(&[
                LiveRootFile {
                    path: "/usr/bin/fixture".to_string(),
                    content: b"fixture".to_vec(),
                    mode: 0o100755,
                    symlink_target: None,
                },
                LiveRootFile {
                    path: "/usr/bin/fixture-link".to_string(),
                    content: Vec::new(),
                    mode: 0o120777,
                    symlink_target: Some("fixture".to_string()),
                },
            ])
            .unwrap();
        tx.commit().unwrap();

        assert_eq!(stats.files_written, 2);
        assert_eq!(
            fs::read_to_string(root.join("usr/bin/fixture")).unwrap(),
            "fixture"
        );
        assert_eq!(
            fs::read_link(root.join("usr/bin/fixture-link")).unwrap(),
            PathBuf::from("fixture")
        );
        assert_eq!(
            fs::metadata(root.join("usr/bin/fixture"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }

    #[test]
    fn rollback_restores_replaced_file() {
        let temp = TempDir::new().unwrap();
        let runtime = temp.path().join("runtime");
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("usr/bin")).unwrap();
        fs::create_dir_all(&runtime).unwrap();
        fs::write(root.join("usr/bin/fixture"), "old").unwrap();

        let mut tx = LiveRootTransaction::begin(
            &runtime,
            &root,
            Uuid::new_v4().to_string(),
            "install fixture",
        )
        .unwrap();
        tx.apply_install_files(&[LiveRootFile {
            path: "/usr/bin/fixture".to_string(),
            content: b"new".to_vec(),
            mode: 0o100755,
            symlink_target: None,
        }])
        .unwrap();
        tx.rollback().unwrap();

        assert_eq!(
            fs::read_to_string(root.join("usr/bin/fixture")).unwrap(),
            "old"
        );
    }

    #[test]
    fn remove_deletes_files_and_empty_dirs() {
        let temp = TempDir::new().unwrap();
        let runtime = temp.path().join("runtime");
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("usr/share/fixture")).unwrap();
        fs::create_dir_all(&runtime).unwrap();
        fs::write(root.join("usr/share/fixture/readme"), "fixture").unwrap();

        let mut tx = LiveRootTransaction::begin(
            &runtime,
            &root,
            Uuid::new_v4().to_string(),
            "remove fixture",
        )
        .unwrap();
        let stats = tx
            .apply_remove_paths(&[
                "/usr/share/fixture/readme".to_string(),
                "/usr/share/fixture/".to_string(),
            ])
            .unwrap();
        tx.commit().unwrap();

        assert_eq!(stats.files_removed, 1);
        assert_eq!(stats.dirs_removed, 1);
        assert!(!root.join("usr/share/fixture").exists());
    }

    #[test]
    fn rollback_restores_removed_empty_dirs() {
        let temp = TempDir::new().unwrap();
        let runtime = temp.path().join("runtime");
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("usr/share/fixture")).unwrap();
        fs::create_dir_all(&runtime).unwrap();

        let mut tx = LiveRootTransaction::begin(
            &runtime,
            &root,
            Uuid::new_v4().to_string(),
            "remove fixture",
        )
        .unwrap();
        tx.apply_remove_paths(&["/usr/share/fixture".to_string()])
            .unwrap();
        tx.rollback().unwrap();

        assert!(root.join("usr/share/fixture").is_dir());
    }

    #[test]
    fn recovery_restores_in_progress_removed_file_from_persisted_journal() {
        let temp = TempDir::new().unwrap();
        let runtime = temp.path().join("runtime");
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("usr/bin")).unwrap();
        fs::create_dir_all(&runtime).unwrap();
        fs::write(root.join("usr/bin/fixture"), "old").unwrap();

        let tx_uuid = Uuid::new_v4().to_string();
        let mut tx =
            LiveRootTransaction::begin(&runtime, &root, tx_uuid.clone(), "remove fixture").unwrap();
        tx.apply_remove_paths(&["/usr/bin/fixture".to_string()])
            .unwrap();
        std::mem::forget(tx);

        assert!(!root.join("usr/bin/fixture").exists());
        recover_pending_journals(&runtime, &root).unwrap();

        assert_eq!(
            fs::read_to_string(root.join("usr/bin/fixture")).unwrap(),
            "old"
        );
        assert!(
            !runtime
                .join("live-root-journals")
                .join(format!("{tx_uuid}.json"))
                .exists()
        );
    }

    #[test]
    fn commit_removes_backup_directory() {
        let temp = TempDir::new().unwrap();
        let runtime = temp.path().join("runtime");
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("usr/bin")).unwrap();
        fs::create_dir_all(&runtime).unwrap();
        fs::write(root.join("usr/bin/fixture"), "old").unwrap();

        let tx_uuid = Uuid::new_v4().to_string();
        let mut tx =
            LiveRootTransaction::begin(&runtime, &root, tx_uuid.clone(), "install fixture")
                .unwrap();
        tx.apply_install_files(&[LiveRootFile {
            path: "/usr/bin/fixture".to_string(),
            content: b"new".to_vec(),
            mode: 0o100755,
            symlink_target: None,
        }])
        .unwrap();
        tx.commit().unwrap();

        assert!(
            !runtime
                .join("live-root-journals")
                .join(format!("{tx_uuid}.backups"))
                .exists()
        );
    }
}
