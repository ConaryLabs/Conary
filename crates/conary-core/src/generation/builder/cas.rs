// conary-core/src/generation/builder/cas.rs

use std::path::{Path, PathBuf};

use super::FileEntryRef;
use crate::generation::artifact::{
    CasObjectRef, verify_cas_object_files_exist_with_expected_sizes,
};

pub(super) fn cas_objects_from_file_refs(file_refs: &[FileEntryRef]) -> Vec<CasObjectRef> {
    file_refs
        .iter()
        .map(|file| CasObjectRef {
            sha256: file.sha256_hash.clone(),
            size: file.size,
        })
        .collect()
}

pub(super) fn verify_runtime_generation_cas_object_presence(
    generations_root: &Path,
    cas_objects: &[CasObjectRef],
) -> crate::Result<()> {
    let artifact_root = artifact_root_for_generations_root(generations_root)?;
    verify_cas_object_files_exist_with_expected_sizes(&artifact_root.join("objects"), cas_objects)
}

pub(super) fn artifact_root_for_generations_root(
    generations_root: &Path,
) -> crate::Result<PathBuf> {
    generations_root
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            crate::error::Error::InvalidPath(format!(
                "generation root {} has no parent artifact root",
                generations_root.display()
            ))
        })
}
