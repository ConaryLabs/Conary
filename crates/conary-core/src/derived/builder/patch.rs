// conary-core/src/derived/builder/patch.rs

//! Typed unified-diff application for derived package payloads.

use super::DerivedFile;
use crate::error::{Error, Result};
use crate::hash;
use crate::payload::{PayloadContentAuthority, PayloadNode};
use diffy::patch_set::{FileOperation, ParseOptions, PatchSet};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// Traditional unified diffs carry no file-mode field. Conary defines created
/// files from that grammar as regular, non-executable payloads.
const UNIFIED_DIFF_CREATED_FILE_MODE: u32 = 0o644;

pub(super) fn apply_patch_set(
    work_dir: &Path,
    patch_content: &[u8],
    strip_level: u32,
    files: &mut HashMap<String, DerivedFile>,
    blobs: &mut HashMap<String, Vec<u8>>,
) -> Result<()> {
    for parsed in PatchSet::parse_bytes(patch_content, ParseOptions::unidiff()) {
        let file_patch = parsed
            .map_err(|error| Error::InitError(format!("Failed to parse patch set: {error}")))?;
        let patch = file_patch.patch().as_text().ok_or_else(|| {
            Error::InitError(
                "Binary patch content is not part of the derived unified-diff contract".to_string(),
            )
        })?;

        match file_patch.operation() {
            FileOperation::Create(path) => {
                let target = patch_target(work_dir, path, strip_level)?;
                apply_create(&target, patch, files, blobs)?;
            }
            FileOperation::Delete(path) => {
                let target = patch_target(work_dir, path, strip_level)?;
                apply_delete(&target, patch, files)?;
            }
            FileOperation::Modify { original, modified } => {
                let original = patch_target(work_dir, original, strip_level)?;
                let modified = patch_target(work_dir, modified, strip_level)?;
                apply_modify(&original, &modified, patch, files, blobs)?;
            }
            FileOperation::Rename { .. } | FileOperation::Copy { .. } => {
                return Err(Error::InitError(
                    "Git rename/copy metadata is not part of the derived unified-diff contract"
                        .to_string(),
                ));
            }
        }
    }

    Ok(())
}

#[derive(Debug)]
struct PatchTarget {
    package_path: String,
    work_path: PathBuf,
}

fn patch_target(work_dir: &Path, raw_path: &[u8], strip_level: u32) -> Result<PatchTarget> {
    let raw_path = std::str::from_utf8(raw_path)
        .map_err(|error| Error::InvalidPath(format!("patch path is not UTF-8: {error}")))?;
    if raw_path.contains('\0') {
        return Err(Error::InvalidPath(
            "patch path contains a null byte".to_string(),
        ));
    }

    let mut components = Vec::new();
    for component in Path::new(raw_path).components() {
        match component {
            Component::Normal(part) => components.push(part),
            Component::CurDir | Component::RootDir => {}
            Component::ParentDir | Component::Prefix(_) => {
                return Err(Error::PathTraversal(format!(
                    "derived patch path '{raw_path}' contains a non-root-relative component"
                )));
            }
        }
    }

    let strip_level = usize::try_from(strip_level)
        .map_err(|_| Error::InvalidPath("patch strip level exceeds platform range".to_string()))?;
    if components.len() <= strip_level {
        return Err(Error::InvalidPath(format!(
            "patch path '{raw_path}' has {} component(s), fewer than required by strip level {strip_level}",
            components.len()
        )));
    }

    let relative_path = components[strip_level..].iter().collect::<PathBuf>();
    let relative_text = relative_path.to_str().ok_or_else(|| {
        Error::InvalidPath(format!(
            "normalized patch path is not UTF-8: {}",
            relative_path.display()
        ))
    })?;
    Ok(PatchTarget {
        package_path: format!("/{relative_text}"),
        work_path: work_dir.join(relative_path),
    })
}

fn apply_create(
    target: &PatchTarget,
    patch: &diffy::Patch<'_, [u8]>,
    files: &mut HashMap<String, DerivedFile>,
    blobs: &mut HashMap<String, Vec<u8>>,
) -> Result<()> {
    if files.contains_key(&target.package_path) || target.work_path.exists() {
        return Err(Error::ConflictError(format!(
            "derived patch creates existing payload path {}",
            target.package_path
        )));
    }
    let content = diffy::apply_bytes(&[], patch).map_err(|error| {
        Error::InitError(format!(
            "Failed to apply creation patch for {}: {error}",
            target.package_path
        ))
    })?;
    write_patch_file(target, &content)?;

    let mut file = DerivedFile {
        path: target.package_path.clone(),
        node: PayloadNode::regular(UNIFIED_DIFF_CREATED_FILE_MODE),
        content: None,
        modified: true,
    };
    record_content(&mut file, content, blobs);
    files.insert(target.package_path.clone(), file);
    Ok(())
}

fn apply_delete(
    target: &PatchTarget,
    patch: &diffy::Patch<'_, [u8]>,
    files: &mut HashMap<String, DerivedFile>,
) -> Result<()> {
    require_regular_payload(files, &target.package_path)?;
    let original = read_patch_file(target)?;
    let result = diffy::apply_bytes(&original, patch).map_err(|error| {
        Error::InitError(format!(
            "Failed to apply deletion patch for {}: {error}",
            target.package_path
        ))
    })?;
    if !result.is_empty() {
        return Err(Error::InitError(format!(
            "deletion patch for {} produced {} residual byte(s)",
            target.package_path,
            result.len()
        )));
    }

    std::fs::remove_file(&target.work_path).map_err(|error| {
        Error::InitError(format!(
            "Failed to remove patched file {}: {error}",
            target.work_path.display()
        ))
    })?;
    files.remove(&target.package_path);
    Ok(())
}

fn apply_modify(
    original: &PatchTarget,
    modified: &PatchTarget,
    patch: &diffy::Patch<'_, [u8]>,
    files: &mut HashMap<String, DerivedFile>,
    blobs: &mut HashMap<String, Vec<u8>>,
) -> Result<()> {
    require_regular_payload(files, &original.package_path)?;
    if original.package_path != modified.package_path
        && (files.contains_key(&modified.package_path) || modified.work_path.exists())
    {
        return Err(Error::ConflictError(format!(
            "derived patch moves {} onto existing payload path {}",
            original.package_path, modified.package_path
        )));
    }

    let original_content = read_patch_file(original)?;
    let content = diffy::apply_bytes(&original_content, patch).map_err(|error| {
        Error::InitError(format!(
            "Failed to apply patch from {} to {}: {error}",
            original.package_path, modified.package_path
        ))
    })?;
    write_patch_file(modified, &content)?;

    if original.package_path == modified.package_path {
        let file = files
            .get_mut(&original.package_path)
            .expect("regular payload was checked immediately before mutation");
        record_content(file, content, blobs);
        return Ok(());
    }

    std::fs::remove_file(&original.work_path).map_err(|error| {
        Error::InitError(format!(
            "Failed to remove renamed patch source {}: {error}",
            original.work_path.display()
        ))
    })?;
    let mut file = files
        .remove(&original.package_path)
        .expect("regular payload was checked immediately before mutation");
    file.path = modified.package_path.clone();
    record_content(&mut file, content, blobs);
    files.insert(modified.package_path.clone(), file);
    Ok(())
}

fn require_regular_payload<'a>(
    files: &'a HashMap<String, DerivedFile>,
    package_path: &str,
) -> Result<&'a DerivedFile> {
    let file = files.get(package_path).ok_or_else(|| {
        Error::NotFound(format!(
            "derived patch target {package_path} is absent from the parent payload"
        ))
    })?;
    if !file.node.kind.is_regular() {
        return Err(Error::ConflictError(format!(
            "derived patch target {package_path} is not a regular payload node"
        )));
    }
    Ok(file)
}

fn read_patch_file(target: &PatchTarget) -> Result<Vec<u8>> {
    std::fs::read(&target.work_path).map_err(|error| {
        Error::InitError(format!(
            "Failed to read patch target {}: {error}",
            target.work_path.display()
        ))
    })
}

fn write_patch_file(target: &PatchTarget, content: &[u8]) -> Result<()> {
    let parent = target.work_path.parent().ok_or_else(|| {
        Error::InvalidPath(format!(
            "patch target {} has no parent directory",
            target.work_path.display()
        ))
    })?;
    std::fs::create_dir_all(parent).map_err(|error| {
        Error::InitError(format!(
            "Failed to create patch target directory {}: {error}",
            parent.display()
        ))
    })?;
    std::fs::write(&target.work_path, content).map_err(|error| {
        Error::InitError(format!(
            "Failed to write patched file {}: {error}",
            target.work_path.display()
        ))
    })
}

fn record_content(file: &mut DerivedFile, content: Vec<u8>, blobs: &mut HashMap<String, Vec<u8>>) {
    let sha256 = hash::sha256(&content);
    file.content = Some(PayloadContentAuthority {
        sha256: sha256.clone(),
        size: content.len() as u64,
    });
    file.modified = true;
    blobs.insert(sha256, content);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regular_file(path: &str, content: &[u8]) -> DerivedFile {
        DerivedFile {
            path: path.to_string(),
            node: PayloadNode::regular(0o644),
            content: Some(PayloadContentAuthority {
                sha256: hash::sha256(content),
                size: content.len() as u64,
            }),
            modified: false,
        }
    }

    #[test]
    fn typed_patch_set_creates_new_payload_authority() {
        let work = tempfile::tempdir().unwrap();
        let mut files = HashMap::new();
        let mut blobs = HashMap::new();
        let patch = b"--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+new bytes\n";

        apply_patch_set(work.path(), patch, 1, &mut files, &mut blobs).unwrap();

        let file = files.get("/new.txt").unwrap();
        let authority = file.content.as_ref().unwrap();
        assert_eq!(file.node.mode, libc::S_IFREG | 0o644);
        assert_eq!(blobs.get(&authority.sha256).unwrap(), b"new bytes\n");
        assert_eq!(
            std::fs::read(work.path().join("new.txt")).unwrap(),
            b"new bytes\n"
        );
    }

    #[test]
    fn typed_patch_set_deletes_payload_authority() {
        let work = tempfile::tempdir().unwrap();
        std::fs::write(work.path().join("old.txt"), b"old bytes\n").unwrap();
        let mut files = HashMap::from([(
            "/old.txt".to_string(),
            regular_file("/old.txt", b"old bytes\n"),
        )]);
        let mut blobs = HashMap::new();
        let patch = b"--- a/old.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-old bytes\n";

        apply_patch_set(work.path(), patch, 1, &mut files, &mut blobs).unwrap();

        assert!(!files.contains_key("/old.txt"));
        assert!(!work.path().join("old.txt").exists());
    }

    #[test]
    fn typed_patch_set_applies_every_file_in_git_preamble_form() {
        let work = tempfile::tempdir().unwrap();
        std::fs::write(work.path().join("one.txt"), b"old one\n").unwrap();
        std::fs::write(work.path().join("two.txt"), b"old two\n").unwrap();
        let mut files = HashMap::from([
            (
                "/one.txt".to_string(),
                regular_file("/one.txt", b"old one\n"),
            ),
            (
                "/two.txt".to_string(),
                regular_file("/two.txt", b"old two\n"),
            ),
        ]);
        let mut blobs = HashMap::new();
        let patch = b"diff --git a/one.txt b/one.txt\n\
index 1111111..2222222 100644\n\
--- a/one.txt\n\
+++ b/one.txt\n\
@@ -1 +1 @@\n\
-old one\n\
+new one\n\
diff --git a/two.txt b/two.txt\n\
index 3333333..4444444 100644\n\
--- a/two.txt\n\
+++ b/two.txt\n\
@@ -1 +1 @@\n\
-old two\n\
+new two\n";

        apply_patch_set(work.path(), patch, 1, &mut files, &mut blobs).unwrap();

        assert_eq!(
            std::fs::read(work.path().join("one.txt")).unwrap(),
            b"new one\n"
        );
        assert_eq!(
            std::fs::read(work.path().join("two.txt")).unwrap(),
            b"new two\n"
        );
        assert!(files.values().all(|file| file.modified));
    }

    #[test]
    fn strip_level_must_leave_a_target_component() {
        let work = tempfile::tempdir().unwrap();
        let mut files = HashMap::new();
        let mut blobs = HashMap::new();
        let patch = b"--- /dev/null\n+++ file.txt\n@@ -0,0 +1 @@\n+new\n";

        let error = apply_patch_set(work.path(), patch, 1, &mut files, &mut blobs).unwrap_err();

        assert!(error.to_string().contains("strip level 1"), "{error}");
    }

    #[test]
    fn typed_patch_path_rejects_parent_traversal() {
        let work = tempfile::tempdir().unwrap();
        let mut files = HashMap::new();
        let mut blobs = HashMap::new();
        let patch = b"--- /dev/null\n+++ b/../escape.txt\n@@ -0,0 +1 @@\n+escape\n";

        let error = apply_patch_set(work.path(), patch, 1, &mut files, &mut blobs).unwrap_err();

        assert!(matches!(error, Error::PathTraversal(_)), "{error}");
        assert!(!work.path().join("escape.txt").exists());
    }
}
