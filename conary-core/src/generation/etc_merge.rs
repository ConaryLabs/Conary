// conary-core/src/generation/etc_merge.rs

//! Three-way merge for /etc files during generation transitions.
//!
//! When a package updates /etc files, we compare three sources:
//!
//! - **Base** (previous generation's EROFS lower): the /etc files from the
//!   previous generation as recorded in the database.
//! - **Theirs** (new generation's EROFS lower): the /etc files from the
//!   new generation as recorded in the database.
//! - **Ours** (overlay upper): user modifications sitting in the overlay
//!   upper directory.
//!
//! The merge logic follows the bootc/ostree three-way model:
//!
//! | Base == Theirs | User modified? | Result          |
//! |----------------|----------------|-----------------|
//! | yes            | no             | Unchanged       |
//! | yes            | yes            | KeepUser        |
//! | no             | no             | AcceptPackage   |
//! | no             | yes            | Conflict        |
//! | (new file)     | n/a            | NewFromPackage  |
//! | (removed)      | yes            | OrphanedUserFile|

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use tracing::{debug, trace};

use crate::hash::{HashAlgorithm, hash_reader};

/// Result of comparing a single /etc file across three sources.
#[derive(Debug, PartialEq)]
pub enum MergeAction {
    /// File unchanged in all three sources. No action needed.
    Unchanged,
    /// Only package changed (user didn't modify). Accept new package version.
    AcceptPackage,
    /// Only user changed. Keep user version (already in upper).
    KeepUser,
    /// Both changed. Conflict needs resolution.
    Conflict {
        base_hash: String,
        package_hash: String,
        user_hash: String,
    },
    /// New file from package (didn't exist before). No conflict.
    NewFromPackage,
    /// File removed by package but user modified it. Orphaned user file.
    OrphanedUserFile,
}

/// Plan for all /etc files affected by a generation transition.
pub struct MergePlan {
    pub actions: HashMap<PathBuf, MergeAction>,
}

impl MergePlan {
    /// Return only entries that are conflicts.
    pub fn conflicts(&self) -> Vec<(&Path, &MergeAction)> {
        self.actions
            .iter()
            .filter(|(_, a)| matches!(a, MergeAction::Conflict { .. }))
            .map(|(p, a)| (p.as_path(), a))
            .collect()
    }

    /// Check whether any conflicts exist.
    pub fn has_conflicts(&self) -> bool {
        self.actions
            .values()
            .any(|a| matches!(a, MergeAction::Conflict { .. }))
    }

    /// Return entries where the package version should be accepted (the upper
    /// layer copy must be removed so the new EROFS lower shows through).
    pub fn accept_package_paths(&self) -> Vec<&Path> {
        self.actions
            .iter()
            .filter(|(_, a)| matches!(a, MergeAction::AcceptPackage))
            .map(|(p, a)| {
                let _ = a;
                p.as_path()
            })
            .collect()
    }
}

/// Compute the SHA-256 hex digest of a file on disk.
fn sha256_of_file(path: &Path) -> crate::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let hash = hash_reader(HashAlgorithm::Sha256, &mut file).map_err(|e| {
        crate::error::Error::IoError(format!("Failed to hash {}: {e}", path.display()))
    })?;
    Ok(hash.value)
}

/// Compare /etc files between previous generation, new generation, and user
/// overlay to produce a merge plan.
///
/// # Arguments
///
/// * `prev_etc_files` - `HashMap<relative_path, sha256_hash>` from the
///   previous generation's DB state (e.g. `"etc/resolv.conf"` -> `"abcd..."`).
/// * `new_etc_files` - `HashMap<relative_path, sha256_hash>` from the new
///   generation's DB state.
/// * `upper_dir` - Path to the /etc overlay upper directory where user
///   modifications live on disk.
///
/// Relative paths should NOT have a leading `/`.
pub fn plan_etc_merge(
    prev_etc_files: &HashMap<String, String>,
    new_etc_files: &HashMap<String, String>,
    upper_dir: &Path,
) -> crate::Result<MergePlan> {
    let mut actions: HashMap<PathBuf, MergeAction> = HashMap::new();

    // Collect the union of all relative paths across all three sources.
    let mut all_paths: HashSet<&str> = HashSet::new();
    for key in prev_etc_files.keys() {
        all_paths.insert(key.as_str());
    }
    for key in new_etc_files.keys() {
        all_paths.insert(key.as_str());
    }

    // Also scan the upper directory for files that may exist only there
    // (user-created files not tracked by any package). We include them in
    // the union so they appear as KeepUser if not in either generation.
    let upper_files = scan_upper_dir(upper_dir);
    for key in upper_files.keys() {
        all_paths.insert(key.as_str());
    }

    for rel_path in all_paths {
        let prev_hash = prev_etc_files.get(rel_path);
        let new_hash = new_etc_files.get(rel_path);

        // Determine whether the user modified the file by checking the
        // overlay upper directory.
        let user_hash = upper_file_hash(upper_dir, rel_path, &upper_files)?;

        let action = classify(prev_hash, new_hash, user_hash.as_deref());

        // Only record non-trivial actions (skip Unchanged to keep the plan
        // small on systems with thousands of /etc files).
        if action != MergeAction::Unchanged {
            trace!(path = rel_path, action = ?action, "etc merge action");
            actions.insert(PathBuf::from(rel_path), action);
        }
    }

    debug!(
        total = actions.len(),
        conflicts = actions
            .values()
            .filter(|a| matches!(a, MergeAction::Conflict { .. }))
            .count(),
        "etc merge plan computed"
    );

    Ok(MergePlan { actions })
}

/// Classify a single file given optional hashes from each source.
fn classify(
    prev_hash: Option<&String>,
    new_hash: Option<&String>,
    user_hash: Option<&str>,
) -> MergeAction {
    match (prev_hash, new_hash) {
        // File exists in neither generation -- must be user-created.
        (None, None) => {
            if user_hash.is_some() {
                MergeAction::KeepUser
            } else {
                MergeAction::Unchanged
            }
        }

        // File only in new generation (not in prev): new from package.
        (None, Some(_)) => MergeAction::NewFromPackage,

        // File in prev but removed in new generation.
        (Some(prev), None) => {
            if let Some(uh) = user_hash {
                // User has a copy in upper. If it differs from the
                // previous generation's version, the user modified it.
                if uh != prev.as_str() {
                    MergeAction::OrphanedUserFile
                } else {
                    // User didn't modify; the upper file is identical to the
                    // old base. Package removal wins silently -- the caller
                    // should remove the upper copy.
                    MergeAction::AcceptPackage
                }
            } else {
                // No user modification, package removed it. Silent removal;
                // nothing to do (the new EROFS simply won't contain it).
                MergeAction::Unchanged
            }
        }

        // File in both generations.
        (Some(prev), Some(new)) => {
            let package_changed = prev != new;

            match (package_changed, user_hash) {
                // Package didn't change, no user modification.
                (false, None) => MergeAction::Unchanged,

                // Package didn't change, user modified.
                (false, Some(uh)) => {
                    if uh == prev.as_str() {
                        // Upper file is identical to base -- not really a
                        // user modification (maybe a leftover copy).
                        MergeAction::Unchanged
                    } else {
                        MergeAction::KeepUser
                    }
                }

                // Package changed, no user modification.
                (true, None) => MergeAction::AcceptPackage,

                // Package changed AND user modified.
                (true, Some(uh)) => {
                    if uh == new.as_str() {
                        // User's version happens to match the new package
                        // version -- no real conflict.
                        MergeAction::AcceptPackage
                    } else if uh == prev.as_str() {
                        // Upper is identical to old base -- user didn't
                        // actually change anything meaningful. Accept the
                        // new package version.
                        MergeAction::AcceptPackage
                    } else {
                        MergeAction::Conflict {
                            base_hash: prev.clone(),
                            package_hash: new.clone(),
                            user_hash: uh.to_string(),
                        }
                    }
                }
            }
        }
    }
}

/// Lazily scan the upper directory to build a map of relative paths to their
/// on-disk locations. This avoids repeated filesystem walks.
fn scan_upper_dir(upper_dir: &Path) -> HashMap<String, PathBuf> {
    let mut map = HashMap::new();
    if !upper_dir.exists() {
        return map;
    }
    scan_dir_recursive(upper_dir, upper_dir, &mut map);
    map
}

fn scan_dir_recursive(base: &Path, current: &Path, map: &mut HashMap<String, PathBuf>) {
    let entries = match std::fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        // Use symlink_metadata to avoid following symlinks outside the overlay.
        // path.is_dir() / path.is_file() follow symlinks, which could escape
        // the overlay boundary via a crafted symlink.
        let meta = match path.symlink_metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_dir() {
            scan_dir_recursive(base, &path, map);
        } else if meta.is_file()
            && let Ok(rel) = path.strip_prefix(base)
        {
            map.insert(rel.to_string_lossy().into_owned(), path.clone());
        }
        // Symlinks are intentionally skipped -- they should not be followed
        // into locations outside the overlay upper directory.
    }
}

/// Get the SHA-256 hash of a file in the overlay upper directory, if it
/// exists. Uses the pre-scanned map to avoid redundant stat calls.
fn upper_file_hash(
    upper_dir: &Path,
    rel_path: &str,
    scanned: &HashMap<String, PathBuf>,
) -> crate::Result<Option<String>> {
    // First check the pre-scanned map.
    if let Some(abs_path) = scanned.get(rel_path) {
        return sha256_of_file(abs_path).map(Some);
    }

    // Fall back to direct path check (in case the scan missed something,
    // e.g. due to a race or symlink). Use symlink_metadata() instead of
    // is_file() to avoid following symlinks, which could escape the upper
    // directory and read unintended host files.
    let abs_path = upper_dir.join(rel_path);
    if abs_path
        .symlink_metadata()
        .map(|m| m.is_file())
        .unwrap_or(false)
    {
        return sha256_of_file(&abs_path).map(Some);
    }

    Ok(None)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::TempDir;

    /// Helper: create a temp upper dir and write a file with given content.
    fn write_upper_file(upper: &Path, rel_path: &str, content: &[u8]) {
        let full = upper.join(rel_path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(full, content).unwrap();
    }

    /// Helper: compute sha256 hex of bytes.
    fn sha(data: &[u8]) -> String {
        crate::hash::sha256(data)
    }

    // -----------------------------------------------------------------
    // Unchanged: same hash in prev and new, no user modification
    // -----------------------------------------------------------------

    #[test]
    fn test_unchanged_files() {
        let upper = TempDir::new().unwrap();
        let hash = sha(b"original content");

        let prev: HashMap<String, String> = [("etc/resolv.conf".into(), hash.clone())].into();
        let new: HashMap<String, String> = [("etc/resolv.conf".into(), hash)].into();

        let plan = plan_etc_merge(&prev, &new, upper.path()).unwrap();
        // Unchanged entries are omitted from the plan.
        assert!(plan.actions.is_empty());
        assert!(!plan.has_conflicts());
    }

    // -----------------------------------------------------------------
    // AcceptPackage: package updates, user didn't modify
    // -----------------------------------------------------------------

    #[test]
    fn test_package_only_change() {
        let upper = TempDir::new().unwrap();
        let old_hash = sha(b"old content");
        let new_hash = sha(b"new content");

        let prev: HashMap<String, String> = [("etc/nginx/nginx.conf".into(), old_hash)].into();
        let new: HashMap<String, String> = [("etc/nginx/nginx.conf".into(), new_hash)].into();

        let plan = plan_etc_merge(&prev, &new, upper.path()).unwrap();
        assert_eq!(plan.actions.len(), 1);
        assert_eq!(
            plan.actions[Path::new("etc/nginx/nginx.conf")],
            MergeAction::AcceptPackage,
        );
        assert!(!plan.has_conflicts());
    }

    // -----------------------------------------------------------------
    // KeepUser: user modified, package didn't update
    // -----------------------------------------------------------------

    #[test]
    fn test_user_only_change() {
        let upper = TempDir::new().unwrap();
        let base_hash = sha(b"base content");
        let user_content = b"user modified content";
        write_upper_file(upper.path(), "etc/hosts", user_content);

        let prev: HashMap<String, String> = [("etc/hosts".into(), base_hash.clone())].into();
        let new: HashMap<String, String> = [("etc/hosts".into(), base_hash)].into();

        let plan = plan_etc_merge(&prev, &new, upper.path()).unwrap();
        assert_eq!(plan.actions.len(), 1);
        assert_eq!(plan.actions[Path::new("etc/hosts")], MergeAction::KeepUser,);
        assert!(!plan.has_conflicts());
    }

    // -----------------------------------------------------------------
    // Conflict: both package and user changed
    // -----------------------------------------------------------------

    #[test]
    fn test_conflict() {
        let upper = TempDir::new().unwrap();
        let base_hash = sha(b"base");
        let pkg_hash = sha(b"package update");
        let user_content = b"user edit";
        let user_hash = sha(user_content);
        write_upper_file(upper.path(), "etc/ssh/sshd_config", user_content);

        let prev: HashMap<String, String> =
            [("etc/ssh/sshd_config".into(), base_hash.clone())].into();
        let new: HashMap<String, String> =
            [("etc/ssh/sshd_config".into(), pkg_hash.clone())].into();

        let plan = plan_etc_merge(&prev, &new, upper.path()).unwrap();
        assert_eq!(plan.actions.len(), 1);
        assert!(plan.has_conflicts());

        let action = &plan.actions[Path::new("etc/ssh/sshd_config")];
        assert_eq!(
            *action,
            MergeAction::Conflict {
                base_hash,
                package_hash: pkg_hash,
                user_hash,
            },
        );

        let conflicts = plan.conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].0, Path::new("etc/ssh/sshd_config"));
    }

    // -----------------------------------------------------------------
    // NewFromPackage: file in new but not prev
    // -----------------------------------------------------------------

    #[test]
    fn test_new_from_package() {
        let upper = TempDir::new().unwrap();
        let new_hash = sha(b"brand new config");

        let prev: HashMap<String, String> = HashMap::new();
        let new: HashMap<String, String> = [("etc/newpkg.conf".into(), new_hash)].into();

        let plan = plan_etc_merge(&prev, &new, upper.path()).unwrap();
        assert_eq!(plan.actions.len(), 1);
        assert_eq!(
            plan.actions[Path::new("etc/newpkg.conf")],
            MergeAction::NewFromPackage,
        );
        assert!(!plan.has_conflicts());
    }

    // -----------------------------------------------------------------
    // OrphanedUserFile: removed by package, modified by user
    // -----------------------------------------------------------------

    #[test]
    fn test_package_removes_user_modified() {
        let upper = TempDir::new().unwrap();
        let base_hash = sha(b"original");
        let user_content = b"user tweaked it";
        write_upper_file(upper.path(), "etc/obsolete.conf", user_content);

        let prev: HashMap<String, String> = [("etc/obsolete.conf".into(), base_hash)].into();
        let new: HashMap<String, String> = HashMap::new();

        let plan = plan_etc_merge(&prev, &new, upper.path()).unwrap();
        assert_eq!(plan.actions.len(), 1);
        assert_eq!(
            plan.actions[Path::new("etc/obsolete.conf")],
            MergeAction::OrphanedUserFile,
        );
    }

    // -----------------------------------------------------------------
    // No conflicts returns empty conflicts list
    // -----------------------------------------------------------------

    #[test]
    fn test_no_conflicts_returns_empty() {
        let upper = TempDir::new().unwrap();
        let hash_a = sha(b"aaa");
        let hash_b = sha(b"bbb");

        let prev: HashMap<String, String> = [("etc/a.conf".into(), hash_a)].into();
        let new: HashMap<String, String> = [("etc/a.conf".into(), hash_b)].into();

        let plan = plan_etc_merge(&prev, &new, upper.path()).unwrap();
        // AcceptPackage, not a conflict.
        assert!(!plan.has_conflicts());
        assert!(plan.conflicts().is_empty());
    }

    // -----------------------------------------------------------------
    // Edge case: user upper file identical to base (not a real change)
    // -----------------------------------------------------------------

    #[test]
    fn test_upper_identical_to_base_is_unchanged() {
        let upper = TempDir::new().unwrap();
        let content = b"same content";
        let hash = sha(content);
        write_upper_file(upper.path(), "etc/unchanged.conf", content);

        let prev: HashMap<String, String> = [("etc/unchanged.conf".into(), hash.clone())].into();
        let new: HashMap<String, String> = [("etc/unchanged.conf".into(), hash)].into();

        let plan = plan_etc_merge(&prev, &new, upper.path()).unwrap();
        // The upper file matches the base -- no meaningful change.
        assert!(plan.actions.is_empty());
    }

    // -----------------------------------------------------------------
    // Edge case: user upper matches new package version (no real conflict)
    // -----------------------------------------------------------------

    #[test]
    fn test_user_matches_new_package_is_accept() {
        let upper = TempDir::new().unwrap();
        let base_hash = sha(b"old");
        let new_content = b"new version";
        let new_hash = sha(new_content);
        write_upper_file(upper.path(), "etc/converged.conf", new_content);

        let prev: HashMap<String, String> = [("etc/converged.conf".into(), base_hash)].into();
        let new: HashMap<String, String> = [("etc/converged.conf".into(), new_hash)].into();

        let plan = plan_etc_merge(&prev, &new, upper.path()).unwrap();
        assert_eq!(
            plan.actions[Path::new("etc/converged.conf")],
            MergeAction::AcceptPackage,
        );
        assert!(!plan.has_conflicts());
    }

    // -----------------------------------------------------------------
    // Mixed scenario: multiple files with different outcomes
    // -----------------------------------------------------------------

    #[test]
    fn test_mixed_plan() {
        let upper = TempDir::new().unwrap();

        let unchanged_hash = sha(b"unchanged");
        let pkg_old = sha(b"pkg-old");
        let pkg_new = sha(b"pkg-new");
        let user_content = b"user-edit";
        write_upper_file(upper.path(), "etc/user.conf", user_content);
        write_upper_file(upper.path(), "etc/conflict.conf", b"conflict-user");

        let mut prev = HashMap::new();
        prev.insert("etc/same.conf".into(), unchanged_hash.clone());
        prev.insert("etc/pkg-update.conf".into(), pkg_old.clone());
        prev.insert("etc/user.conf".into(), sha(b"base-user"));
        prev.insert("etc/conflict.conf".into(), sha(b"base-conflict"));

        let mut new = HashMap::new();
        new.insert("etc/same.conf".into(), unchanged_hash);
        new.insert("etc/pkg-update.conf".into(), pkg_new);
        new.insert("etc/user.conf".into(), sha(b"base-user")); // pkg didn't change
        new.insert("etc/conflict.conf".into(), sha(b"pkg-conflict")); // pkg changed
        new.insert("etc/brand-new.conf".into(), sha(b"new-file")); // new file

        let plan = plan_etc_merge(&prev, &new, upper.path()).unwrap();

        // same.conf -> Unchanged (omitted)
        assert!(!plan.actions.contains_key(Path::new("etc/same.conf")));

        // pkg-update.conf -> AcceptPackage
        assert_eq!(
            plan.actions[Path::new("etc/pkg-update.conf")],
            MergeAction::AcceptPackage,
        );

        // user.conf -> KeepUser
        assert_eq!(
            plan.actions[Path::new("etc/user.conf")],
            MergeAction::KeepUser,
        );

        // conflict.conf -> Conflict
        assert!(matches!(
            plan.actions[Path::new("etc/conflict.conf")],
            MergeAction::Conflict { .. },
        ));

        // brand-new.conf -> NewFromPackage
        assert_eq!(
            plan.actions[Path::new("etc/brand-new.conf")],
            MergeAction::NewFromPackage,
        );

        assert!(plan.has_conflicts());
        assert_eq!(plan.conflicts().len(), 1);
    }

    // -----------------------------------------------------------------
    // accept_package_paths helper
    // -----------------------------------------------------------------

    #[test]
    fn test_accept_package_paths() {
        let upper = TempDir::new().unwrap();
        let old = sha(b"old");
        let new_val = sha(b"new");

        let prev: HashMap<String, String> = [
            ("etc/a.conf".into(), old.clone()),
            ("etc/b.conf".into(), old),
        ]
        .into();
        let new: HashMap<String, String> = [
            ("etc/a.conf".into(), new_val.clone()),
            ("etc/b.conf".into(), new_val),
        ]
        .into();

        let plan = plan_etc_merge(&prev, &new, upper.path()).unwrap();
        let paths = plan.accept_package_paths();
        assert_eq!(paths.len(), 2);
    }
}
