// conary-core/tests/bootstrap_image_builder_contract.rs

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|dir| {
            dir.join("crates/conary-core/src/bootstrap/image.rs")
                .is_file()
        })
        .expect("workspace root not found from crate manifest ancestors")
}

fn core_source(path: &str) -> PathBuf {
    workspace_root().join("crates/conary-core/src").join(path)
}

#[test]
fn raw_image_builder_does_not_keep_legacy_loop_device_path() {
    let image_rs = fs::read_to_string(core_source("bootstrap/image.rs"))
        .expect("failed to read bootstrap/image.rs");

    assert!(
        image_rs.contains("build_raw_repart"),
        "bootstrap image builder must keep the systemd-repart implementation"
    );
    assert!(
        !image_rs.contains("fn build_raw_legacy"),
        "bootstrap image builder should not keep the legacy loop-device raw image path once systemd-repart is the supported contract"
    );
    assert!(
        !image_rs.contains("setup_loop_device("),
        "bootstrap image builder should not retain loop-device setup helpers after removing the legacy image path"
    );
    assert!(
        !image_rs.contains("losetup"),
        "bootstrap image builder should not shell out to losetup after removing the legacy image path"
    );
    assert!(
        !image_rs.contains("Uid::effective().is_root()"),
        "bootstrap image builder should not branch on root execution once systemd-repart is the only supported path"
    );
}
