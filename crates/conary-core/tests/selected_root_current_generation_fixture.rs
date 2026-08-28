// conary-core/tests/selected_root_current_generation_fixture.rs

use conary_core::generation::artifact::load_generation_artifact_with_verified_cas;
use conary_core::generation::mount::current_generation;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn selected_root_current_generation_fixture_is_an_exact_mountable_artifact() {
    let runtime_root = tempfile::tempdir().unwrap();
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/conary/tests/fixtures/selected-root-current-generation/prepare.py");
    let status = Command::new("python3")
        .arg(script)
        .arg(runtime_root.path())
        .status()
        .unwrap();
    assert!(status.success());

    assert_eq!(current_generation(runtime_root.path()).unwrap(), Some(1));
    let artifact =
        load_generation_artifact_with_verified_cas(&runtime_root.path().join("generations/1"))
            .unwrap();
    assert_eq!(artifact.generation, 1);
    assert!(artifact.generation_root.entries.is_empty());
    assert!(artifact.mutable_state.entries.is_empty());
    assert_eq!(artifact.cas_dir, runtime_root.path().join("objects"));
}
