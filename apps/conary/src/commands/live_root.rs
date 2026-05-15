// apps/conary/src/commands/live_root.rs

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};

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
    committed: bool,
}

pub(crate) fn target_path(root: &Path, package_path: &str) -> Result<PathBuf> {
    let relative = package_path.strip_prefix('/').unwrap_or(package_path);
    let relative_path = Path::new(relative);
    if relative_path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        bail!("package path {package_path} escapes the target root");
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

            let temp = target.with_extension(format!("conary-tmp-{}", self.tx_uuid));
            let _ = fs::remove_file(&temp);
            if let Some(target_value) = file.symlink_target.as_deref() {
                symlink(target_value, &temp)
                    .with_context(|| format!("Failed to create symlink {}", temp.display()))?;
                fs::rename(&temp, &target)
                    .with_context(|| format!("Failed to move symlink {}", target.display()))?;
            } else {
                fs::write(&temp, &file.content)
                    .with_context(|| format!("Failed to write {}", temp.display()))?;
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
        for backup in self.backups.iter().rev() {
            let target = PathBuf::from(&backup.path);
            let backup_path = PathBuf::from(&backup.backup_path);
            if backup_path.exists() {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::rename(&backup_path, &target)?;
            }
        }
        self.write_journal("rolled_back")?;
        self.committed = true;
        Ok(())
    }

    pub(crate) fn commit(mut self) -> Result<()> {
        self.committed = true;
        self.write_journal("committed")?;
        let _ = fs::remove_file(&self.journal_path);
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
            if !full.exists() {
                fs::create_dir(&full)?;
                self.created_paths.push(full);
                stats.dirs_created += 1;
            }
        }
        Ok(())
    }

    fn backup_existing(&mut self, target: &Path) -> Result<()> {
        if fs::symlink_metadata(target).is_err() {
            self.created_paths.push(target.to_path_buf());
            return Ok(());
        }
        let backup_dir = self.journal_path.with_extension("backups");
        fs::create_dir_all(&backup_dir)?;
        let backup_path = backup_dir.join(format!("backup-{}", self.backups.len()));
        fs::rename(target, &backup_path)?;
        self.backups.push(BackupRecord {
            path: target.to_string_lossy().into_owned(),
            backup_path: backup_path.to_string_lossy().into_owned(),
        });
        Ok(())
    }

    fn write_journal(&self, state: &str) -> Result<()> {
        let journal = LiveRootJournal {
            schema: "conary.live-root-journal.v1".to_string(),
            tx_uuid: self.tx_uuid.clone(),
            operation: self.operation.clone(),
            state: state.to_string(),
            backups: self.backups.clone(),
            created_paths: self
                .created_paths
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
        };
        fs::write(&self.journal_path, serde_json::to_vec_pretty(&journal)?)?;
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
        if journal.state == "committed" || journal.state == "rolled_back" {
            let _ = fs::remove_file(&path);
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
            committed: false,
        };
        tx.rollback()?;
    }
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
}
