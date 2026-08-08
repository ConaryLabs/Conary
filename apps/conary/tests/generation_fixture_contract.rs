// apps/conary/tests/generation_fixture_contract.rs

use conary_core::generation::artifact::{ARTIFACT_MANIFEST_VERSION, load_generation_artifact};
use std::path::PathBuf;

#[test]
fn synthetic_generation_fixture_is_current_and_complete() {
    let generation_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/synthetic-generation-artifact/generations/42");

    let artifact = load_generation_artifact(&generation_dir)
        .expect("synthetic QEMU fixture must satisfy the current generation artifact contract");
    assert_eq!(
        artifact.artifact_manifest.version,
        ARTIFACT_MANIFEST_VERSION
    );
    assert_eq!(artifact.artifact_manifest.generation, 42);
    assert_eq!(artifact.generation_root.entries.len(), 2);
    assert!(artifact.mutable_state.entries.is_empty());
}
