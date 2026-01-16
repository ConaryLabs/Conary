// src/transaction/planner.rs

//! Transaction planner using VfsTree for conflict detection
//!
//! The planner builds a complete operation plan before any filesystem changes,
//! using the VFS tree to detect conflicts and compute the correct order of
//! operations.

use crate::db::models::FileEntry;
use crate::filesystem::{CasStore, VfsTree};
use crate::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::{ExtractedFile, FileToRemove, FileType, OperationType};

/// A planned operation to perform
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedOperation {
    pub path: PathBuf,
    pub op_type: OperationType,
    pub new_hash: Option<String>,
    pub new_mode: Option<u32>,
    pub symlink_target: Option<PathBuf>,
}

/// Information about a file to backup
#[derive(Debug, Clone)]
pub struct BackupInfo {
    pub path: PathBuf,
    pub file_type: FileType,
    pub current_hash: Option<String>,
    pub mode: u32,
    pub size: u64,
}

/// Information about a file to stage
#[derive(Debug, Clone)]
pub struct StageInfo {
    pub path: PathBuf,
    pub hash: String,
    pub mode: u32,
    pub file_type: FileType,
    pub symlink_target: Option<PathBuf>,
}

/// Conflict detected during planning
#[derive(Debug, Clone)]
pub enum ConflictInfo {
    /// File exists but owned by different package
    FileOwnedByOther { path: PathBuf, owner: String },
    /// File exists but not tracked by any package
    UntrackedFileExists { path: PathBuf },
    /// Directory exists where file should go
    DirectoryBlocksFile { path: PathBuf },
    /// File exists where directory should go
    FileBlocksDirectory { path: PathBuf },
    /// Parent directory doesn't exist and can't be created
    ParentMissing { path: PathBuf, parent: PathBuf },
}

/// The complete transaction plan
#[derive(Debug)]
pub struct TransactionPlan {
    /// VFS tree representing the planned final state
    pub vfs: VfsTree,
    /// Ordered list of operations to perform
    pub operations: Vec<PlannedOperation>,
    /// Directories to create (in order, parents first)
    pub dirs_to_create: Vec<PathBuf>,
    /// Files that need backup (exist on disk and will be replaced/removed)
    pub files_to_backup: Vec<BackupInfo>,
    /// Files to stage from CAS
    pub files_to_stage: Vec<StageInfo>,
    /// Directories to remove after file operations (children first)
    pub dirs_to_remove: Vec<PathBuf>,
    /// Detected conflicts (should be empty for valid plan)
    pub conflicts: Vec<ConflictInfo>,
}

impl TransactionPlan {
    /// Create an empty plan
    pub fn new() -> Self {
        Self {
            vfs: VfsTree::new(),
            operations: Vec::new(),
            dirs_to_create: Vec::new(),
            files_to_backup: Vec::new(),
            files_to_stage: Vec::new(),
            dirs_to_remove: Vec::new(),
            conflicts: Vec::new(),
        }
    }

    /// Check if the plan has any conflicts
    pub fn has_conflicts(&self) -> bool {
        !self.conflicts.is_empty()
    }

    /// Get a summary of the plan
    pub fn summary(&self) -> PlanSummary {
        let mut adds = 0;
        let mut replaces = 0;
        let mut removes = 0;

        for op in &self.operations {
            match op.op_type {
                OperationType::AddFile | OperationType::AddSymlink | OperationType::Mkdir => {
                    adds += 1;
                }
                OperationType::ReplaceFile | OperationType::ReplaceSymlink => replaces += 1,
                OperationType::RemoveFile | OperationType::RemoveSymlink | OperationType::Rmdir => {
                    removes += 1;
                }
            }
        }

        PlanSummary {
            total_operations: self.operations.len(),
            files_to_add: adds,
            files_to_replace: replaces,
            files_to_remove: removes,
            dirs_to_create: self.dirs_to_create.len(),
            dirs_to_remove: self.dirs_to_remove.len(),
            conflicts: self.conflicts.len(),
        }
    }
}

impl Default for TransactionPlan {
    fn default() -> Self {
        Self::new()
    }
}

/// Summary of a transaction plan
#[derive(Debug, Clone)]
pub struct PlanSummary {
    pub total_operations: usize,
    pub files_to_add: usize,
    pub files_to_replace: usize,
    pub files_to_remove: usize,
    pub dirs_to_create: usize,
    pub dirs_to_remove: usize,
    pub conflicts: usize,
}

/// Builds transaction plans using VfsTree for conflict detection
pub struct TransactionPlanner<'a> {
    conn: &'a Connection,
    root: &'a Path,
    cas: &'a CasStore,
    vfs: VfsTree,
}

impl<'a> TransactionPlanner<'a> {
    /// Create a new planner
    pub fn new(conn: &'a Connection, root: &'a Path, cas: &'a CasStore) -> Self {
        Self {
            conn,
            root,
            cas,
            vfs: VfsTree::new(),
        }
    }

    /// Plan an install/upgrade operation
    pub fn plan_install(
        &mut self,
        new_files: &[ExtractedFile],
        old_files: &[FileToRemove],
        package_name: &str,
        is_upgrade: bool,
    ) -> Result<TransactionPlan> {
        let mut plan = TransactionPlan::new();

        // Build map of old file paths for quick lookup
        let old_file_map: HashMap<&str, &FileToRemove> = old_files.iter()
            .map(|f| (f.path.as_str(), f))
            .collect();

        // Phase 1: Analyze new files, detect conflicts, plan directories
        for file in new_files {
            let path = Path::new(&file.path);

            // Ensure parent directories exist in VFS and plan their creation
            if let Some(parent) = path.parent()
                && parent != Path::new("")
                && parent != Path::new("/")
            {
                self.ensure_directory_path(parent, &mut plan)?;
            }

            // Check for conflicts on the actual filesystem
            // Strip leading / to allow joining with non-root prefixes
            let relative_path = file.path.strip_prefix('/').unwrap_or(&file.path);
            let target_path = self.root.join(relative_path);
            let target_exists = target_path.symlink_metadata().is_ok();

            if target_exists {
                // Check if owned by this package (upgrade) or another
                if let Ok(Some(existing)) = FileEntry::find_by_path(self.conn, &file.path) {
                    // File is tracked - check owner
                    if let Ok(Some(owner)) =
                        crate::db::models::Trove::find_by_id(self.conn, existing.trove_id)
                        && owner.name != package_name
                    {
                        plan.conflicts.push(ConflictInfo::FileOwnedByOther {
                            path: path.to_path_buf(),
                            owner: owner.name,
                        });
                        continue;
                    }

                    // Owned by this package - will be replaced
                    plan.files_to_backup.push(BackupInfo {
                        path: path.to_path_buf(),
                        file_type: if file.is_symlink {
                            FileType::Symlink
                        } else {
                            FileType::Regular
                        },
                        current_hash: Some(existing.sha256_hash),
                        mode: existing.permissions as u32,
                        size: existing.size as u64,
                    });

                    plan.operations.push(PlannedOperation {
                        path: path.to_path_buf(),
                        op_type: if file.is_symlink {
                            OperationType::ReplaceSymlink
                        } else {
                            OperationType::ReplaceFile
                        },
                        new_hash: if file.is_symlink {
                            // Use consistent symlink hash computation
                            file.symlink_target.as_ref().map(|t| CasStore::compute_symlink_hash(t))
                        } else {
                            Some(self.cas.compute_hash(&file.content))
                        },
                        new_mode: Some(file.mode),
                        symlink_target: file.symlink_target.as_ref().map(PathBuf::from),
                    });
                } else if is_upgrade && let Some(old_file) = old_file_map.get(file.path.as_str()) {
                    // File from old version being replaced
                    plan.files_to_backup.push(BackupInfo {
                        path: path.to_path_buf(),
                        file_type: if file.is_symlink {
                            FileType::Symlink
                        } else {
                            FileType::Regular
                        },
                        current_hash: Some(old_file.hash.clone()),
                        mode: old_file.mode,
                        size: old_file.size as u64,
                    });

                    plan.operations.push(PlannedOperation {
                        path: path.to_path_buf(),
                        op_type: if file.is_symlink {
                            OperationType::ReplaceSymlink
                        } else {
                            OperationType::ReplaceFile
                        },
                        new_hash: if file.is_symlink {
                            // Use consistent symlink hash computation
                            file.symlink_target.as_ref().map(|t| CasStore::compute_symlink_hash(t))
                        } else {
                            Some(self.cas.compute_hash(&file.content))
                        },
                        new_mode: Some(file.mode),
                        symlink_target: file.symlink_target.as_ref().map(PathBuf::from),
                    });
                } else {
                    // Untracked file exists - conflict
                    plan.conflicts.push(ConflictInfo::UntrackedFileExists {
                        path: path.to_path_buf(),
                    });
                    continue;
                }
            } else {
                // New file
                plan.operations.push(PlannedOperation {
                    path: path.to_path_buf(),
                    op_type: if file.is_symlink {
                        OperationType::AddSymlink
                    } else {
                        OperationType::AddFile
                    },
                    new_hash: if file.is_symlink {
                        // Use consistent symlink hash computation
                        file.symlink_target.as_ref().map(|t| CasStore::compute_symlink_hash(t))
                    } else {
                        Some(self.cas.compute_hash(&file.content))
                    },
                    new_mode: Some(file.mode),
                    symlink_target: file.symlink_target.as_ref().map(PathBuf::from),
                });
            }

            // Add to staging list
            plan.files_to_stage.push(StageInfo {
                path: path.to_path_buf(),
                hash: if file.is_symlink {
                    // Use consistent symlink hash computation
                    file.symlink_target.as_ref()
                        .map(|t| CasStore::compute_symlink_hash(t))
                        .unwrap_or_default()
                } else {
                    self.cas.compute_hash(&file.content)
                },
                mode: file.mode,
                file_type: if file.is_symlink {
                    FileType::Symlink
                } else {
                    FileType::Regular
                },
                symlink_target: file.symlink_target.as_ref().map(PathBuf::from),
            });

            // Add to VFS
            if let Some(parent) = path.parent()
                && parent != Path::new("")
                && parent != Path::new("/")
            {
                let _ = self.vfs.mkdir_p(parent);
            }
            if file.is_symlink {
                let target = file.symlink_target.as_deref().unwrap_or("");
                let _ = self.vfs.add_symlink(&file.path, target);
            } else {
                let hash = self.cas.compute_hash(&file.content);
                let _ = self.vfs.add_file(&file.path, &hash, file.content.len() as u64, file.mode);
            }
        }

        // Phase 2: Handle files to remove (upgrade case - old files not in new package)
        let new_paths: HashSet<&str> = new_files.iter().map(|f| f.path.as_str()).collect();
        for old_file in old_files {
            if !new_paths.contains(old_file.path.as_str()) {
                // File from old package not in new package - needs removal
                plan.files_to_backup.push(BackupInfo {
                    path: PathBuf::from(&old_file.path),
                    file_type: FileType::Regular,
                    current_hash: Some(old_file.hash.clone()),
                    mode: old_file.mode,
                    size: old_file.size as u64,
                });

                plan.operations.push(PlannedOperation {
                    path: PathBuf::from(&old_file.path),
                    op_type: OperationType::RemoveFile,
                    new_hash: None,
                    new_mode: None,
                    symlink_target: None,
                });
            }
        }

        // Phase 3: Compute directory cleanup (for removed files)
        self.compute_dir_cleanup(old_files, new_files, &mut plan)?;

        // Transfer VFS to plan
        plan.vfs = std::mem::take(&mut self.vfs);

        Ok(plan)
    }

    /// Ensure all parent directories exist in the plan
    fn ensure_directory_path(&mut self, path: &Path, plan: &mut TransactionPlan) -> Result<()> {
        let mut to_create = Vec::new();
        let mut current = path;

        // Walk up the path, collecting directories that need creation
        while current != Path::new("") && current != Path::new("/") {
            // Strip leading / to allow joining with non-root prefixes
            let current_str = current.to_string_lossy();
            let relative = current_str.strip_prefix('/').unwrap_or(&current_str);
            let target = self.root.join(relative);
            if !target.exists() && !plan.dirs_to_create.contains(&current.to_path_buf()) {
                to_create.push(current.to_path_buf());
            }
            current = current.parent().unwrap_or(Path::new(""));
        }

        // Add in parent-first order
        to_create.reverse();
        for dir in to_create {
            if !plan.dirs_to_create.contains(&dir) {
                plan.dirs_to_create.push(dir.clone());
                plan.operations.push(PlannedOperation {
                    path: dir.clone(),
                    op_type: OperationType::Mkdir,
                    new_hash: None,
                    new_mode: Some(0o755),
                    symlink_target: None,
                });
                let _ = self.vfs.mkdir_p(&dir);
            }
        }

        Ok(())
    }

    /// Compute directories that can be removed after file removal
    fn compute_dir_cleanup(
        &self,
        old_files: &[FileToRemove],
        new_files: &[ExtractedFile],
        plan: &mut TransactionPlan,
    ) -> Result<()> {
        // Collect all unique parent directories of removed files
        let new_paths: HashSet<&str> = new_files.iter().map(|f| f.path.as_str()).collect();
        let mut potentially_empty: HashSet<PathBuf> = HashSet::new();

        for old_file in old_files {
            if !new_paths.contains(old_file.path.as_str())
                && let Some(parent) = Path::new(&old_file.path).parent()
                && parent != Path::new("")
                && parent != Path::new("/")
            {
                potentially_empty.insert(parent.to_path_buf());
            }
        }

        // Sort deepest first for removal
        let mut dirs: Vec<PathBuf> = potentially_empty.into_iter().collect();
        dirs.sort_by(|a, b| {
            let a_depth = a.components().count();
            let b_depth = b.components().count();
            b_depth.cmp(&a_depth)
        });

        plan.dirs_to_remove = dirs;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn setup_test_env() -> (TempDir, Connection, CasStore) {
        let temp_dir = TempDir::new().unwrap();
        let conn = Connection::open_in_memory().unwrap();

        // Create minimal schema for testing
        conn.execute_batch(
            "
            CREATE TABLE troves (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                version TEXT NOT NULL
            );
            CREATE TABLE files (
                id INTEGER PRIMARY KEY,
                trove_id INTEGER NOT NULL,
                path TEXT NOT NULL,
                sha256_hash TEXT NOT NULL,
                size INTEGER NOT NULL,
                permissions INTEGER NOT NULL
            );
            ",
        )
        .unwrap();

        let cas = CasStore::new(temp_dir.path().join("cas")).unwrap();

        (temp_dir, conn, cas)
    }

    #[test]
    fn test_plan_simple_install() {
        let (temp_dir, conn, cas) = setup_test_env();

        let mut planner = TransactionPlanner::new(&conn, temp_dir.path(), &cas);

        let files = vec![ExtractedFile {
            path: "usr/bin/hello".to_string(),
            content: b"#!/bin/bash\necho hello".to_vec(),
            mode: 0o755,
            is_symlink: false,
            symlink_target: None,
        }];

        let plan = planner.plan_install(&files, &[], "hello", false).unwrap();

        assert!(!plan.has_conflicts());
        assert_eq!(plan.files_to_stage.len(), 1);
        assert!(plan.dirs_to_create.iter().any(|d| d == Path::new("usr/bin")));
    }

    #[test]
    fn test_plan_detects_untracked_conflict() {
        let (temp_dir, conn, cas) = setup_test_env();

        // Create an existing file that's not tracked
        let file_path = temp_dir.path().join("usr/bin/existing");
        std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        std::fs::write(&file_path, "existing content").unwrap();

        let mut planner = TransactionPlanner::new(&conn, temp_dir.path(), &cas);

        let files = vec![ExtractedFile {
            path: "usr/bin/existing".to_string(),
            content: b"new content".to_vec(),
            mode: 0o755,
            is_symlink: false,
            symlink_target: None,
        }];

        let plan = planner.plan_install(&files, &[], "test", false).unwrap();

        assert!(plan.has_conflicts());
        assert!(matches!(
            &plan.conflicts[0],
            ConflictInfo::UntrackedFileExists { .. }
        ));
    }

    #[test]
    fn test_plan_summary() {
        let plan = TransactionPlan {
            vfs: VfsTree::new(),
            operations: vec![
                PlannedOperation {
                    path: PathBuf::from("usr/bin"),
                    op_type: OperationType::Mkdir,
                    new_hash: None,
                    new_mode: Some(0o755),
                    symlink_target: None,
                },
                PlannedOperation {
                    path: PathBuf::from("usr/bin/foo"),
                    op_type: OperationType::AddFile,
                    new_hash: Some("abc".to_string()),
                    new_mode: Some(0o755),
                    symlink_target: None,
                },
                PlannedOperation {
                    path: PathBuf::from("usr/bin/bar"),
                    op_type: OperationType::ReplaceFile,
                    new_hash: Some("def".to_string()),
                    new_mode: Some(0o755),
                    symlink_target: None,
                },
            ],
            dirs_to_create: vec![PathBuf::from("usr/bin")],
            files_to_backup: vec![],
            files_to_stage: vec![],
            dirs_to_remove: vec![],
            conflicts: vec![],
        };

        let summary = plan.summary();
        assert_eq!(summary.total_operations, 3);
        assert_eq!(summary.files_to_add, 2); // mkdir + addfile
        assert_eq!(summary.files_to_replace, 1);
        assert_eq!(summary.dirs_to_create, 1);
    }

    #[test]
    fn test_plan_upgrade_with_removed_files() {
        let (temp_dir, conn, cas) = setup_test_env();

        // Simulate existing file from old package
        let old_path = temp_dir.path().join("usr/bin/old");
        std::fs::create_dir_all(old_path.parent().unwrap()).unwrap();
        std::fs::write(&old_path, "old content").unwrap();

        // Register it in DB
        conn.execute(
            "INSERT INTO troves (id, name, version) VALUES (1, 'test', '1.0')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO files (trove_id, path, sha256_hash, size, permissions)
             VALUES (1, 'usr/bin/old', 'oldhash', 11, 493)",
            [],
        )
        .unwrap();

        let mut planner = TransactionPlanner::new(&conn, temp_dir.path(), &cas);

        let new_files = vec![ExtractedFile {
            path: "usr/bin/new".to_string(),
            content: b"new content".to_vec(),
            mode: 0o755,
            is_symlink: false,
            symlink_target: None,
        }];

        let old_files = vec![FileToRemove {
            path: "usr/bin/old".to_string(),
            hash: "oldhash".to_string(),
            size: 11,
            mode: 0o755,
        }];

        let plan = planner
            .plan_install(&new_files, &old_files, "test", true)
            .unwrap();

        assert!(!plan.has_conflicts());

        // Should have operation to remove old file
        assert!(plan
            .operations
            .iter()
            .any(|op| op.op_type == OperationType::RemoveFile
                && op.path == PathBuf::from("usr/bin/old")));

        // Old file should be in backup list
        assert!(plan
            .files_to_backup
            .iter()
            .any(|b| b.path == PathBuf::from("usr/bin/old")));
    }
}
