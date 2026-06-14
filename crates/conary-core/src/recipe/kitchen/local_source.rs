// conary-core/src/recipe/kitchen/local_source.rs

use crate::error::{Error, Result};
use crate::recipe::hermetic::source_identity::{
    CanonicalLocalFile, CanonicalLocalFileKind, hash_canonical_local_file_at,
};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub fn materialize_local_source_from_file_list(
    source_root: &Path,
    destination: &Path,
    files: &[CanonicalLocalFile],
) -> Result<()> {
    let source_root = canonical_source_root(source_root)?;
    fs::create_dir_all(destination)?;

    for file in files {
        let relative_path = validate_materialized_relative_path(&file.relative_path)?;
        let source_path = source_root.join(&relative_path);
        let destination_path = destination.join(&relative_path);
        let (actual_hash, actual_target, _) =
            hash_canonical_local_file_at(&source_path, file.kind)?;

        if actual_hash != file.hash {
            return Err(Error::ChecksumMismatch {
                expected: file.hash.clone(),
                actual: actual_hash,
            });
        }

        match file.kind {
            CanonicalLocalFileKind::Regular => {
                materialize_regular_file(&source_root, &source_path, &destination_path, file)?;
            }
            CanonicalLocalFileKind::Symlink => {
                let actual_target = actual_target.ok_or_else(|| {
                    Error::ConfigError(format!(
                        "Local source symlink target missing from file list: {}",
                        file.relative_path.display()
                    ))
                })?;
                if file.symlink_target.as_ref() != Some(&actual_target) {
                    return Err(Error::ConfigError(format!(
                        "Local source symlink target changed after planning: {}",
                        file.relative_path.display()
                    )));
                }
                materialize_symlink(
                    &source_root,
                    &source_path,
                    &destination_path,
                    &actual_target,
                )?;
            }
        }
    }

    Ok(())
}

pub(crate) fn copy_dir_contents(source_root: &Path, source: &Path, dest: &Path) -> Result<()> {
    let source_root = canonical_source_root(source_root)?;
    copy_dir_contents_inner(&source_root, source, dest)
}

fn copy_dir_contents_inner(source_root: &Path, source: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;

    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            copy_dir_contents_inner(source_root, &source_path, &dest_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &dest_path)?;
        } else if file_type.is_symlink() {
            let target = fs::read_link(&source_path)?;
            let resolved = source_path.canonicalize().map_err(|e| {
                Error::ConfigError(format!(
                    "Local source symlink target not found: {} -> {} ({e})",
                    source_path.display(),
                    target.display()
                ))
            })?;
            if !resolved.starts_with(source_root) {
                return Err(Error::ConfigError(format!(
                    "Local source symlink must stay within the source directory: {} -> {}",
                    source_path.display(),
                    target.display()
                )));
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &dest_path)?;
            #[cfg(not(unix))]
            {
                if resolved.is_dir() {
                    copy_dir_contents_inner(source_root, &resolved, &dest_path)?;
                } else {
                    fs::copy(&resolved, &dest_path)?;
                }
            }
        }
    }

    Ok(())
}

fn materialize_regular_file(
    source_root: &Path,
    source_path: &Path,
    destination_path: &Path,
    file: &CanonicalLocalFile,
) -> Result<()> {
    ensure_regular_source_stays_within_root(source_root, source_path)?;
    if let Some(parent) = destination_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source_path, destination_path)?;
    set_mode_if_available(destination_path, file.mode)?;
    Ok(())
}

fn materialize_symlink(
    source_root: &Path,
    source_path: &Path,
    destination_path: &Path,
    target: &Path,
) -> Result<()> {
    let resolved = source_path.canonicalize().map_err(|e| {
        Error::ConfigError(format!(
            "Local source symlink target not found: {} -> {} ({e})",
            source_path.display(),
            target.display()
        ))
    })?;
    if !resolved.starts_with(source_root) {
        return Err(Error::ConfigError(format!(
            "Local source symlink must stay within the source directory: {} -> {}",
            source_path.display(),
            target.display()
        )));
    }

    if let Some(parent) = destination_path.parent() {
        fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(target, destination_path)?;

    #[cfg(not(unix))]
    {
        if resolved.is_dir() {
            copy_dir_contents_inner(source_root, &resolved, destination_path)?;
        } else {
            fs::copy(&resolved, destination_path)?;
        }
    }

    Ok(())
}

fn ensure_regular_source_stays_within_root(source_root: &Path, source_path: &Path) -> Result<()> {
    let resolved = source_path.canonicalize().map_err(|e| {
        Error::ConfigError(format!(
            "Local source file not found: {} ({e})",
            source_path.display()
        ))
    })?;
    if !resolved.starts_with(source_root) {
        return Err(Error::ConfigError(format!(
            "Local source file must stay within the source directory: {}",
            source_path.display()
        )));
    }
    Ok(())
}

fn canonical_source_root(source_root: &Path) -> Result<PathBuf> {
    let metadata = fs::metadata(source_root).map_err(|e| {
        Error::NotFound(format!(
            "Local source root not found: {} ({e})",
            source_root.display()
        ))
    })?;
    if !metadata.is_dir() {
        return Err(Error::ConfigError(format!(
            "Local source root must be a directory: {}",
            source_root.display()
        )));
    }
    Ok(fs::canonicalize(source_root)?)
}

fn validate_materialized_relative_path(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(Error::InvalidPath(
            "Local source file list entry cannot be empty".to_string(),
        ));
    }
    if path.is_absolute() {
        return Err(Error::InvalidPath(format!(
            "Local source file list entry must be relative, not absolute: {}",
            path.display()
        )));
    }

    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(Error::PathTraversal(format!(
                    "Local source file list entry contains parent traversal: {}",
                    path.display()
                )));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(Error::InvalidPath(format!(
                    "Local source file list entry must be relative, not absolute: {}",
                    path.display()
                )));
            }
        }
    }

    if clean.as_os_str().is_empty() {
        return Err(Error::InvalidPath(
            "Local source file list entry cannot be empty".to_string(),
        ));
    }

    Ok(clean)
}

#[cfg(unix)]
fn set_mode_if_available(path: &Path, mode: Option<u32>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if let Some(mode) = mode {
        let permissions = fs::Permissions::from_mode(mode);
        fs::set_permissions(path, permissions)?;
    }

    Ok(())
}

#[cfg(not(unix))]
fn set_mode_if_available(_path: &Path, _mode: Option<u32>) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::hermetic::source_identity::{
        CanonicalLocalFile, CanonicalLocalFileKind, CiMode, canonical_local_file_list,
    };
    use std::path::PathBuf;

    fn regular_file(relative_path: impl Into<PathBuf>) -> CanonicalLocalFile {
        CanonicalLocalFile {
            relative_path: relative_path.into(),
            hash: "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            kind: CanonicalLocalFileKind::Regular,
            mode: None,
            symlink_target: None,
        }
    }

    #[test]
    fn materialization_copies_only_hashed_files() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let destination = dir.path().join("dest");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("included.txt"), "included\n").unwrap();
        std::fs::write(source.join("excluded.txt"), "excluded\n").unwrap();
        let mut files = canonical_local_file_list(&source, CiMode::Off).unwrap();
        files.retain(|file| file.relative_path == PathBuf::from("included.txt"));

        materialize_local_source_from_file_list(&source, &destination, &files).unwrap();

        assert_eq!(
            std::fs::read_to_string(destination.join("included.txt")).unwrap(),
            "included\n"
        );
        assert!(!destination.join("excluded.txt").exists());
    }

    #[test]
    fn materialization_refuses_hash_mismatch_after_planning() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let destination = dir.path().join("dest");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("file.txt"), "planned\n").unwrap();
        let files = canonical_local_file_list(&source, CiMode::Off).unwrap();
        std::fs::write(source.join("file.txt"), "changed\n").unwrap();

        let error =
            materialize_local_source_from_file_list(&source, &destination, &files).unwrap_err();

        assert!(
            error.to_string().contains("Checksum mismatch"),
            "expected checksum mismatch, got: {error}"
        );
    }

    #[test]
    fn materialization_rejects_parent_or_absolute_paths() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let destination = dir.path().join("dest");
        std::fs::create_dir_all(&source).unwrap();

        let parent_error = materialize_local_source_from_file_list(
            &source,
            &destination,
            &[regular_file("../escape.txt")],
        )
        .unwrap_err();
        assert!(
            parent_error.to_string().contains("parent"),
            "expected parent traversal rejection, got: {parent_error}"
        );

        let absolute_error = materialize_local_source_from_file_list(
            &source,
            &destination,
            &[regular_file(PathBuf::from("/tmp/escape.txt"))],
        )
        .unwrap_err();
        assert!(
            absolute_error.to_string().contains("absolute"),
            "expected absolute path rejection, got: {absolute_error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn materialization_rejects_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let outside = dir.path().join("outside");
        let destination = dir.path().join("dest");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "secret\n").unwrap();
        std::os::unix::fs::symlink("../outside/secret.txt", source.join("escape.txt")).unwrap();
        let files = canonical_local_file_list(&source, CiMode::Off).unwrap();

        let error =
            materialize_local_source_from_file_list(&source, &destination, &files).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Local source symlink must stay within the source directory"),
            "expected symlink escape rejection, got: {error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn materialization_preserves_safe_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let destination = dir.path().join("dest");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("target.txt"), "safe\n").unwrap();
        std::os::unix::fs::symlink("target.txt", source.join("link.txt")).unwrap();
        let files = canonical_local_file_list(&source, CiMode::Off).unwrap();

        materialize_local_source_from_file_list(&source, &destination, &files).unwrap();

        assert!(
            std::fs::symlink_metadata(destination.join("link.txt"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_link(destination.join("link.txt")).unwrap(),
            PathBuf::from("target.txt")
        );
        assert_eq!(
            std::fs::read_to_string(destination.join("link.txt")).unwrap(),
            "safe\n"
        );
    }
}
