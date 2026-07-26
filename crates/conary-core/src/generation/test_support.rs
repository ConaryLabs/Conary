// conary-core/src/generation/test_support.rs

//! Shared constructors for complete generation-artifact test authority.

use std::path::Path;

use crate::generation::artifact::CasObjectRef;
use crate::generation::root_manifest::{
    GENERATION_ROOT_MANIFEST_FILE, GENERATION_ROOT_MANIFEST_VERSION, GenerationRootEntry,
    GenerationRootManifest, MUTABLE_STATE_MANIFEST_FILE, MutableStateManifest,
};
use crate::payload::{PayloadContentAuthority, PayloadNode, PayloadNodeKind, ResolvedPayloadNode};

pub(crate) fn write_root_manifests(generation_dir: &Path) -> (String, String) {
    write_root_manifests_with_objects(generation_dir, &[])
}

pub(crate) fn write_root_manifests_with_objects(
    generation_dir: &Path,
    objects: &[CasObjectRef],
) -> (String, String) {
    let root = directory_node();
    let mut entries = Vec::new();
    if !objects.is_empty() {
        entries.push(GenerationRootEntry {
            path: "/objects".to_string(),
            node: root.clone(),
            content: None,
        });
        entries.extend(objects.iter().map(|object| GenerationRootEntry {
            path: format!("/objects/{}", object.sha256),
            node: ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o644)).unwrap(),
            content: Some(PayloadContentAuthority {
                sha256: object.sha256.clone(),
                size: object.size,
            }),
        }));
        entries.sort_by(|left, right| left.path.cmp(&right.path));
    }

    GenerationRootManifest {
        version: GENERATION_ROOT_MANIFEST_VERSION,
        root,
        entries,
    }
    .write_to(generation_dir)
    .unwrap();
    MutableStateManifest::empty()
        .write_to(generation_dir)
        .unwrap();

    (
        digest_file(&generation_dir.join(GENERATION_ROOT_MANIFEST_FILE)),
        digest_file(&generation_dir.join(MUTABLE_STATE_MANIFEST_FILE)),
    )
}

fn directory_node() -> ResolvedPayloadNode {
    let mut root = PayloadNode::regular(0o755);
    root.kind = PayloadNodeKind::Directory;
    root.mode = libc::S_IFDIR | 0o755;
    ResolvedPayloadNode::from_numeric_source(root).unwrap()
}

fn digest_file(path: &Path) -> String {
    crate::hash::sha256(&std::fs::read(path).unwrap())
}
