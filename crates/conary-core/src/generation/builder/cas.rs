// conary-core/src/generation/builder/cas.rs

use std::path::{Path, PathBuf};

use crate::generation::artifact::{
    CasObjectRef, VerifiedCasObjectPresence, verify_cas_object_presence,
};
use crate::generation::root_manifest::{GenerationRootManifest, MutableStateManifest};

pub(super) fn cas_objects_from_manifests(
    generation: &GenerationRootManifest,
    state: &MutableStateManifest,
) -> Vec<CasObjectRef> {
    generation
        .regular_contents()
        .chain(
            state
                .entries
                .iter()
                .filter_map(|entry| entry.content.as_ref()),
        )
        .map(|content| CasObjectRef {
            sha256: content.sha256.clone(),
            size: content.size,
        })
        .collect()
}

pub(super) fn verify_runtime_generation_cas_object_presence<'objects>(
    generations_root: &Path,
    cas_objects: &'objects [CasObjectRef],
) -> crate::Result<VerifiedCasObjectPresence<'objects>> {
    let artifact_root = artifact_root_for_generations_root(generations_root)?;
    verify_cas_object_presence(&artifact_root.join("objects"), cas_objects)
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
