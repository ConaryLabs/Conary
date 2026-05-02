// conary-core/tests/generation_composefs_runtime_contract.rs

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|dir| {
            dir.join("crates/conary-core/src/generation/mount.rs")
                .is_file()
        })
        .expect("workspace root not found from crate manifest ancestors")
}

fn core_source(path: &str) -> PathBuf {
    workspace_root().join("crates/conary-core/src").join(path)
}

fn app_source(path: &str) -> PathBuf {
    workspace_root().join("apps/conary/src").join(path)
}

fn workspace_file(path: &str) -> PathBuf {
    workspace_root().join(path)
}

#[test]
fn composefs_preflight_requires_the_mount_helper_and_overlay_stack() {
    let composefs_rs = fs::read_to_string(core_source("generation/composefs.rs"))
        .expect("failed to read generation/composefs.rs");

    assert!(
        composefs_rs.contains("mount.composefs"),
        "composefs preflight must name the mount.composefs helper explicitly so missing userspace support fails closed"
    );
    assert!(
        composefs_rs.contains("overlay"),
        "composefs preflight must treat overlayfs as part of the runtime contract instead of only checking for erofs"
    );
    assert!(
        composefs_rs.contains("erofs"),
        "composefs preflight must continue to require EROFS support for the metadata image"
    );
}

#[test]
fn composefs_mount_path_does_not_retain_plain_erofs_fallbacks() {
    let mount_rs = fs::read_to_string(core_source("generation/mount.rs"))
        .expect("failed to read generation/mount.rs");

    assert!(
        !mount_rs.contains("ErofsFallback"),
        "normal generation mounts must not retain an EROFS fallback enum variant once composefs support is required"
    );
    assert!(
        !mount_rs.contains("falling back to EROFS"),
        "mount_generation must fail closed when composefs support is missing instead of silently downgrading to plain EROFS"
    );
}

#[test]
fn live_generation_mounts_do_not_request_verity_from_digest_presence_alone() {
    let composefs_ops_rs = fs::read_to_string(app_source("commands/composefs_ops.rs"))
        .expect("failed to read commands/composefs_ops.rs");

    assert!(
        !composefs_ops_rs
            .contains("let requested_verity = build_result.erofs_verity_digest.is_some();"),
        "live generation remounts must not request composefs verity from the digest alone; they must require proof that root.erofs actually has Linux fs-verity enabled"
    );
}

#[test]
fn generation_switch_does_not_force_verity_when_metadata_says_it_is_unavailable() {
    let switch_rs = fs::read_to_string(app_source("commands/generation/switch.rs"))
        .expect("failed to read commands/generation/switch.rs");

    assert!(
        !switch_rs.contains("verity: true,"),
        "generation switching must respect persisted fs-verity readiness instead of unconditionally retrying root.erofs with verity"
    );
}

#[test]
fn generation_switch_does_not_retry_requested_verity_as_plain_composefs() {
    let switch_rs = fs::read_to_string(app_source("commands/generation/switch.rs"))
        .expect("failed to read commands/generation/switch.rs");

    let requested_verity_branch = switch_rs
        .split("let mount_outcome = if requested_verity {")
        .nth(1)
        .and_then(|rest| rest.split("} else {").next())
        .expect("failed to find requested-verity mount branch");

    assert!(
        !requested_verity_branch.contains(".or_else("),
        "requested fs-verity mounts must fail closed instead of retrying as plain composefs"
    );
    assert!(
        !requested_verity_branch.contains("retrying without"),
        "requested fs-verity mounts must not log or perform a downgrade retry"
    );
    assert!(
        switch_rs.contains("} else {\n        mount_generation(&opts_plain)"),
        "plain composefs remains valid only when persisted metadata says fs-verity is unavailable"
    );
}

#[test]
fn composefs_apply_prints_etc_overlay_failures_to_stderr() {
    let composefs_ops_rs = fs::read_to_string(app_source("commands/composefs_ops.rs"))
        .expect("failed to read commands/composefs_ops.rs");

    assert!(
        composefs_ops_rs
            .contains("warn!(\"Failed to mount /etc overlay: {e}; /etc may be stale\");"),
        "composefs apply must keep logging /etc overlay mount failures"
    );
    assert!(
        composefs_ops_rs.contains(
            "eprintln!(\"Warning: Failed to mount /etc overlay: {e}; /etc may be stale\");"
        ),
        "composefs apply must also print /etc overlay mount failures to stderr"
    );
}

#[test]
fn generation_switch_prints_etc_overlay_failures_to_stderr() {
    let switch_rs = fs::read_to_string(app_source("commands/generation/switch.rs"))
        .expect("failed to read commands/generation/switch.rs");

    assert!(
        switch_rs.contains("warn!(\"Failed to mount /etc overlay: {e}; /etc may be stale\");"),
        "generation switch must keep logging /etc overlay mount failures"
    );
    assert!(
        switch_rs.contains(
            "eprintln!(\"Warning: Failed to mount /etc overlay: {e}; /etc may be stale\");"
        ),
        "generation switch must also print /etc overlay mount failures to stderr"
    );
}

#[test]
fn initramfs_generation_mounts_have_empty_usr_symlink_fallback() {
    let dracut_generator = fs::read_to_string(workspace_file(
        "packaging/dracut/90conary/conary-generator.sh",
    ))
    .expect("failed to read conary dracut generator");
    let bootstrap_config = fs::read_to_string(core_source("bootstrap/system_config.rs"))
        .expect("failed to read bootstrap system config");

    for (label, source) in [
        ("dracut generator", dracut_generator.as_str()),
        ("bootstrap initramfs", bootstrap_config.as_str()),
    ] {
        assert!(
            source.contains("expose_generation_usr"),
            "{label} must route generation /usr exposure through the shared fallback shape"
        );
        assert!(
            source.contains("rmdir \"$usr_target\""),
            "{label} must only replace an empty carrier-root /usr placeholder"
        );
        assert!(
            source.contains("ln -s conary/mnt/usr \"$usr_target\""),
            "{label} must fall back to a relative /usr symlink into the mounted generation"
        );
        assert!(
            source.contains("ensure_root_symlink sbin usr/sbin"),
            "{label} must ensure /sbin resolves through usr-merge before switch_root"
        );
    }
}
