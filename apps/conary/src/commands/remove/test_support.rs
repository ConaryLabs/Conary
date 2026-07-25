// apps/conary/src/commands/remove/test_support.rs

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use conary_core::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
    ResolvedPayloadNode,
};
use std::collections::BTreeMap;
use tempfile::TempDir;
use tracing::warn;

use crate::commands::{FileSnapshot, TroveSnapshot};

#[derive(Debug, Default, PartialEq, Eq)]
struct DirectRemovalStats {
    files_removed: usize,
    dirs_removed: usize,
}

fn resolved_node(kind: PayloadNodeKind, mode: u32) -> ResolvedPayloadNode {
    ResolvedPayloadNode::from_numeric_source(PayloadNode {
        kind,
        mode,
        user: PayloadIdentity::Numeric { id: 0 },
        group: PayloadIdentity::Numeric { id: 0 },
        mtime: PayloadTimestamp::UNIX_EPOCH,
        xattrs: BTreeMap::new(),
    })
    .unwrap()
}

pub(super) fn regular_snapshot(path: &str, permissions: u32) -> FileSnapshot {
    FileSnapshot {
        path: path.to_string(),
        node: resolved_node(
            PayloadNodeKind::Regular {
                hardlink_identity: None,
            },
            libc::S_IFREG | (permissions & 0o7777),
        ),
        content: Some(PayloadContentAuthority {
            sha256: conary_core::hash::sha256(b"x"),
            size: 1,
        }),
    }
}

fn symlink_snapshot(path: &str, target: &str) -> FileSnapshot {
    FileSnapshot {
        path: path.to_string(),
        node: resolved_node(
            PayloadNodeKind::Symlink {
                target: target.to_string(),
            },
            libc::S_IFLNK | 0o777,
        ),
        content: None,
    }
}

fn directory_snapshot(path: &str, permissions: u32) -> FileSnapshot {
    FileSnapshot {
        path: path.to_string(),
        node: resolved_node(
            PayloadNodeKind::Directory,
            libc::S_IFDIR | (permissions & 0o7777),
        ),
        content: None,
    }
}

pub(super) fn remove_snapshot(files: Vec<FileSnapshot>) -> TroveSnapshot {
    TroveSnapshot {
        name: "fixture".to_string(),
        version: "1.0.0".to_string(),
        architecture: Some("x86_64".to_string()),
        description: None,
        install_source: "Package".to_string(),
        source_distro: None,
        version_scheme: conary_core::repository::versioning::VersionScheme::Conary,
        native_lifecycle: None,
        ccs_remove_hook: None,
        installed_from_repository_id: None,
        files,
    }
}

fn snapshot_path_under_root(root: &Path, path: &str) -> PathBuf {
    root.join(path.strip_prefix('/').unwrap_or(path))
}

fn snapshot_entry_is_dir(file: &FileSnapshot) -> bool {
    matches!(file.node.source.kind, PayloadNodeKind::Directory)
}

fn remove_files_from_live_root(
    root: &Path,
    snapshot: &TroveSnapshot,
) -> Result<DirectRemovalStats> {
    let mut stats = DirectRemovalStats::default();
    let mut dirs = Vec::new();

    for file in &snapshot.files {
        let path = snapshot_path_under_root(root, &file.path);
        if snapshot_entry_is_dir(file) {
            dirs.push(path);
            continue;
        }

        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => {
                dirs.push(path);
            }
            Ok(_) => {
                std::fs::remove_file(&path)
                    .with_context(|| format!("Failed to remove package file {}", path.display()))?;
                stats.files_removed += 1;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                warn!(
                    "Package file {} was already absent during removal",
                    path.display()
                );
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to inspect package file {}", path.display()));
            }
        }
    }

    dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    dirs.dedup();
    for dir in dirs {
        match std::fs::remove_dir(&dir) {
            Ok(()) => stats.dirs_removed += 1,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("Failed to remove package directory {}", dir.display())
                });
            }
        }
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_live_root_removal_deletes_files_symlinks_and_empty_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("usr/bin")).unwrap();
        std::fs::create_dir_all(root.join("usr/share/fixture")).unwrap();
        std::fs::write(root.join("usr/bin/fixture"), "fixture").unwrap();
        std::fs::write(root.join("usr/share/fixture/readme"), "fixture").unwrap();
        std::os::unix::fs::symlink("fixture", root.join("usr/bin/fixture-link")).unwrap();

        let snapshot = remove_snapshot(vec![
            regular_snapshot("/usr/bin/fixture", 0o755),
            symlink_snapshot("/usr/bin/fixture-link", "fixture"),
            regular_snapshot("/usr/share/fixture/readme", 0o644),
            directory_snapshot("/usr/share/fixture/", 0o755),
        ]);

        let stats = remove_files_from_live_root(root, &snapshot).unwrap();

        assert_eq!(stats.files_removed, 3);
        assert_eq!(stats.dirs_removed, 1);
        assert!(!root.join("usr/bin/fixture").exists());
        assert!(!root.join("usr/bin/fixture-link").exists());
        assert!(!root.join("usr/share/fixture").exists());
        assert!(root.join("usr/share").exists());
    }

    #[test]
    fn direct_live_root_removal_ignores_already_missing_paths() {
        let tmp = TempDir::new().unwrap();
        let snapshot = remove_snapshot(vec![regular_snapshot("/usr/bin/missing", 0o755)]);

        let stats = remove_files_from_live_root(tmp.path(), &snapshot).unwrap();

        assert_eq!(stats.files_removed, 0);
        assert_eq!(stats.dirs_removed, 0);
    }
}
