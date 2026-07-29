// conary-core/src/filesystem/selected_root.rs
//! Bounded root-relative inspection of selected-root paths.

use crate::error::{Error, Result};
use crate::generation::root_manifest::capture_existing_payload_node;
use crate::payload::ResolvedPayloadNode;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

const MAX_SELECTED_ROOT_SYMLINK_DEPTH: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedRootSymlinkTarget {
    pub package_path: String,
    pub node: ResolvedPayloadNode,
}

/// Resolve symlinks in a package path's selected-root ancestors without
/// following the leaf itself.
///
/// The returned spelling is absolute and root-relative. If an ancestor
/// symlink points into a tree that is intentionally absent from the selected
/// root, the missing tail is retained lexically after the proven symlink
/// target. This lets callers classify the current effective path domain while
/// preserving the original package spelling as ownership authority.
pub fn selected_root_effective_package_path(root: &Path, package_path: &str) -> Result<String> {
    validate_selected_root(root)?;
    let relative = root_relative_package_path(package_path)?;
    let relative = resolve_leaf_without_following_with_policy(
        root,
        &relative,
        package_path,
        MissingAncestorPolicy::PreserveMissingTail,
    )?;
    let relative = relative.to_str().ok_or_else(|| {
        Error::InvalidPath(format!(
            "effective selected-root path for {package_path} is not UTF-8"
        ))
    })?;
    Ok(format!("/{relative}"))
}

/// Capture the exact leaf node named by a package path without following it.
pub fn capture_selected_root_node(
    root: &Path,
    package_path: &str,
) -> Result<Option<ResolvedPayloadNode>> {
    validate_selected_root(root)?;
    let relative = root_relative_package_path(package_path)?;
    let relative = match resolve_leaf_without_following(root, &relative, package_path) {
        Ok(relative) => relative,
        Err(Error::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error),
    };
    let path = root.join(relative);
    match fs::symlink_metadata(&path) {
        Ok(_) => capture_existing_payload_node(&path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::Io(error)),
    }
}

/// Prove that the exact package-path leaf is a symlink whose bounded,
/// root-relative target resolves to a real directory inside `root`.
pub fn selected_root_symlink_targets_directory(
    root: &Path,
    package_path: &str,
) -> Result<Option<SelectedRootSymlinkTarget>> {
    validate_selected_root(root)?;
    let relative = root_relative_package_path(package_path)?;
    let relative = match resolve_leaf_without_following(root, &relative, package_path) {
        Ok(relative) => relative,
        Err(Error::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error),
    };
    let leaf = root.join(&relative);
    let metadata = match fs::symlink_metadata(&leaf) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::Io(error)),
    };
    if !metadata.file_type().is_symlink() {
        return Ok(None);
    }
    let resolved = match resolve_existing_root_relative_path(root, &relative, package_path) {
        Ok(resolved) => resolved,
        Err(Error::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error),
    };
    let target = root.join(&resolved);
    let target_metadata = match fs::symlink_metadata(&target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::Io(error)),
    };
    if !target_metadata.file_type().is_dir() {
        return Ok(None);
    }
    let node = capture_existing_payload_node(&target)?;
    let resolved = resolved.to_str().ok_or_else(|| {
        Error::InvalidPath(format!(
            "resolved selected-root target for {package_path} is not UTF-8"
        ))
    })?;
    let package_path = format!("/{resolved}");
    Ok(Some(SelectedRootSymlinkTarget { package_path, node }))
}

fn resolve_leaf_without_following(
    root: &Path,
    relative: &Path,
    package_path: &str,
) -> Result<PathBuf> {
    resolve_leaf_without_following_with_policy(
        root,
        relative,
        package_path,
        MissingAncestorPolicy::Reject,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MissingAncestorPolicy {
    Reject,
    PreserveMissingTail,
}

fn resolve_leaf_without_following_with_policy(
    root: &Path,
    relative: &Path,
    package_path: &str,
    missing_ancestor_policy: MissingAncestorPolicy,
) -> Result<PathBuf> {
    let file_name = relative.file_name().ok_or_else(|| {
        Error::InvalidPath(format!(
            "package path {package_path} has no selected-root leaf"
        ))
    })?;
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    let resolved_parent = if parent.as_os_str().is_empty() {
        PathBuf::new()
    } else {
        resolve_root_relative_path(root, parent, package_path, missing_ancestor_policy)?
    };
    Ok(resolved_parent.join(file_name))
}

fn validate_selected_root(root: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(root).map_err(Error::Io)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(Error::PathTraversal(format!(
            "selected root must be a real directory: {}",
            root.display()
        )));
    }
    Ok(())
}

fn root_relative_package_path(package_path: &str) -> Result<PathBuf> {
    let candidate = package_path.strip_prefix('/').unwrap_or(package_path);
    let mut relative = PathBuf::new();
    for component in Path::new(candidate).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(Error::PathTraversal(format!(
                    "package path {package_path} is not root-relative"
                )));
            }
        }
    }
    if relative.as_os_str().is_empty() {
        return Err(Error::InvalidPath(format!(
            "package path {package_path} does not name a selected-root node"
        )));
    }
    Ok(relative)
}

fn resolve_existing_root_relative_path(
    root: &Path,
    relative: &Path,
    package_path: &str,
) -> Result<PathBuf> {
    resolve_root_relative_path(root, relative, package_path, MissingAncestorPolicy::Reject)
}

fn resolve_root_relative_path(
    root: &Path,
    relative: &Path,
    package_path: &str,
    missing_ancestor_policy: MissingAncestorPolicy,
) -> Result<PathBuf> {
    let mut pending = components(relative)?;
    let mut resolved = PathBuf::new();
    let mut symlink_depth = 0usize;

    while let Some(component) = pending.pop_front() {
        let candidate_relative = resolved.join(&component);
        let candidate = root.join(&candidate_relative);
        let metadata = match fs::symlink_metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && missing_ancestor_policy == MissingAncestorPolicy::PreserveMissingTail =>
            {
                resolved.push(component);
                resolved.extend(pending);
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::NotFound(format!(
                    "selected-root path {} is unavailable while resolving {package_path}: {error}",
                    candidate.display()
                )));
            }
            Err(error) => return Err(Error::Io(error)),
        };
        if !metadata.file_type().is_symlink() {
            resolved.push(component);
            continue;
        }

        symlink_depth += 1;
        if symlink_depth > MAX_SELECTED_ROOT_SYMLINK_DEPTH {
            return Err(Error::PathTraversal(format!(
                "package path {package_path} exceeds {MAX_SELECTED_ROOT_SYMLINK_DEPTH} selected-root symlinks"
            )));
        }
        let target = fs::read_link(&candidate).map_err(Error::Io)?;
        let mut redirected = if target.is_absolute() {
            PathBuf::new()
        } else {
            resolved.clone()
        };
        append_lexical_target(&mut redirected, &target, package_path, &candidate_relative)?;
        redirected.extend(pending);
        pending = components(&redirected)?;
        resolved.clear();
    }
    Ok(resolved)
}

fn components(path: &Path) -> Result<VecDeque<OsString>> {
    path.components()
        .map(|component| match component {
            Component::Normal(part) => Ok(part.to_os_string()),
            Component::CurDir | Component::RootDir => Ok(OsString::new()),
            Component::ParentDir | Component::Prefix(_) => Err(Error::PathTraversal(format!(
                "selected-root path has invalid component: {}",
                path.display()
            ))),
        })
        .filter(|component| {
            component
                .as_ref()
                .map_or(true, |component| !component.is_empty())
        })
        .collect()
}

fn append_lexical_target(
    resolved: &mut PathBuf,
    target: &Path,
    package_path: &str,
    symlink: &Path,
) -> Result<()> {
    for component in target.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(part) => resolved.push(part),
            Component::ParentDir => {
                if !resolved.pop() {
                    return Err(Error::PathTraversal(format!(
                        "package path {package_path} escapes the selected root through symlink {} -> {}",
                        symlink.display(),
                        target.display()
                    )));
                }
            }
            Component::Prefix(_) => {
                return Err(Error::PathTraversal(format!(
                    "package path {package_path} has unsupported symlink target {}",
                    target.display()
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::symlink;

    #[test]
    fn absolute_symlink_targets_are_resolved_inside_the_selected_root() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("real")).unwrap();
        symlink("/real", root.path().join("link")).unwrap();

        let resolved = selected_root_symlink_targets_directory(root.path(), "/link")
            .unwrap()
            .unwrap();
        assert_eq!(resolved.package_path, "/real");
    }

    #[test]
    fn symlink_escape_and_loop_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        symlink("../../outside", root.path().join("escape")).unwrap();
        let escape = selected_root_symlink_targets_directory(root.path(), "/escape").unwrap_err();
        assert!(matches!(escape, Error::PathTraversal(_)));

        symlink("loop", root.path().join("loop")).unwrap();
        let loop_error = selected_root_symlink_targets_directory(root.path(), "/loop").unwrap_err();
        assert!(matches!(loop_error, Error::PathTraversal(_)));
    }

    #[test]
    fn ancestor_symlinks_are_resolved_inside_root_before_leaf_capture() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("real")).unwrap();
        fs::write(root.path().join("real/node"), b"inside").unwrap();
        symlink("/real", root.path().join("absolute")).unwrap();

        assert!(
            capture_selected_root_node(root.path(), "/absolute/node")
                .unwrap()
                .is_some()
        );

        fs::create_dir_all(root.path().join("nested")).unwrap();
        symlink("../../outside", root.path().join("nested/escape")).unwrap();
        let error = capture_selected_root_node(root.path(), "/nested/escape/node").unwrap_err();
        assert!(matches!(error, Error::PathTraversal(_)));
    }

    #[test]
    fn effective_package_path_resolves_aliases_before_a_missing_leaf_tree() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("var")).unwrap();
        symlink("../run", root.path().join("var/run")).unwrap();

        assert_eq!(
            selected_root_effective_package_path(root.path(), "/var/run/faillock").unwrap(),
            "/run/faillock"
        );

        fs::create_dir_all(root.path().join("usr/lib/sub")).unwrap();
        symlink("usr/lib", root.path().join("lib")).unwrap();
        assert_eq!(
            selected_root_effective_package_path(root.path(), "/lib/sub/missing").unwrap(),
            "/usr/lib/sub/missing"
        );
    }

    #[test]
    fn effective_package_path_rejects_escaping_and_looping_ancestors() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("nested")).unwrap();
        symlink("../../outside", root.path().join("nested/escape")).unwrap();
        let escape =
            selected_root_effective_package_path(root.path(), "/nested/escape/node").unwrap_err();
        assert!(matches!(escape, Error::PathTraversal(_)));

        symlink("loop", root.path().join("loop")).unwrap();
        let loop_error =
            selected_root_effective_package_path(root.path(), "/loop/node").unwrap_err();
        assert!(matches!(loop_error, Error::PathTraversal(_)));
    }

    #[test]
    fn dangling_or_non_directory_symlink_targets_are_not_directories() {
        let root = tempfile::tempdir().unwrap();
        symlink("/missing", root.path().join("dangling")).unwrap();
        fs::write(root.path().join("regular"), b"not a directory").unwrap();
        symlink("/regular", root.path().join("regular-link")).unwrap();

        assert!(
            capture_selected_root_node(root.path(), "/dangling")
                .unwrap()
                .is_some(),
            "the exact dangling symlink leaf still exists"
        );
        assert!(
            selected_root_symlink_targets_directory(root.path(), "/dangling")
                .unwrap()
                .is_none()
        );
        assert!(
            selected_root_symlink_targets_directory(root.path(), "/regular-link")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_missing_ancestor_is_an_absent_selected_root_node() {
        let root = tempfile::tempdir().unwrap();

        assert!(
            capture_selected_root_node(root.path(), "/missing/leaf")
                .unwrap()
                .is_none()
        );
        assert!(
            selected_root_symlink_targets_directory(root.path(), "/missing/leaf")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn non_utf8_resolved_target_path_fails_closed() {
        let root = tempfile::tempdir().unwrap();
        let target = OsString::from_vec(vec![b'n', b'o', b'n', b'-', 0xff]);
        fs::create_dir(root.path().join(&target)).unwrap();
        symlink(&target, root.path().join("link")).unwrap();

        let error = selected_root_symlink_targets_directory(root.path(), "/link").unwrap_err();
        assert!(matches!(error, Error::InvalidPath(_)), "{error}");
    }
}
