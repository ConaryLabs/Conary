// conary-core/src/ccs/builder/test_support.rs
//! Crate-private CCS builder fixtures for package verification tests.

use super::BuildResult;

pub(crate) fn minimal_file_build_result(name: &str, version: &str, bytes: &[u8]) -> BuildResult {
    single_file_build_result_at(name, version, &format!("/{name}"), bytes)
}

pub(crate) fn single_file_build_result_at(
    name: &str,
    version: &str,
    path: &str,
    bytes: &[u8],
) -> BuildResult {
    use super::{ComponentData, FileEntry};
    use std::collections::HashMap;

    let manifest = crate::ccs::manifest::CcsManifest::new_minimal(name, version);
    let hash = crate::hash::sha256(bytes);
    let entry = FileEntry {
        path: path.to_string(),
        node: crate::payload::PayloadNode::regular(0o755),
        content: Some(crate::payload::PayloadContentAuthority {
            sha256: hash.clone(),
            size: bytes.len() as u64,
        }),
        component: "runtime".to_string(),
        chunks: None,
    };
    BuildResult {
        manifest,
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: vec![entry.clone()],
                hash: hash.clone(),
                size: bytes.len() as u64,
            },
        )]),
        files: vec![entry],
        blobs: HashMap::from([(hash, bytes.to_vec())]),
        total_size: bytes.len() as u64,
        chunked: false,
        chunk_stats: None,
    }
}
