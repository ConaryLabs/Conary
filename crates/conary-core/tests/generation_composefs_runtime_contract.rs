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
fn runtime_generation_paths_are_routed_through_runtime_root_contract() {
    let transaction_rs = fs::read_to_string(core_source("transaction/mod.rs"))
        .expect("failed to read transaction/mod.rs");
    let composefs_ops_rs = fs::read_to_string(app_source("commands/composefs_ops.rs"))
        .expect("failed to read commands/composefs_ops.rs");
    let generation_commands_rs = fs::read_to_string(app_source("commands/generation/commands.rs"))
        .expect("failed to read commands/generation/commands.rs");
    let generation_switch_rs = fs::read_to_string(app_source("commands/generation/switch.rs"))
        .expect("failed to read commands/generation/switch.rs");

    assert!(
        transaction_rs.contains("ConaryRuntimeRoot"),
        "TransactionConfig must derive runtime generation paths through ConaryRuntimeRoot"
    );
    assert!(
        composefs_ops_rs.contains("ConaryRuntimeRoot::from_db_path"),
        "composefs apply must use ConaryRuntimeRoot when deriving generation paths from a DB path"
    );
    assert!(
        generation_commands_rs.contains("ConaryRuntimeRoot"),
        "generation commands must use ConaryRuntimeRoot for current, generation, and GC paths"
    );
    assert!(
        generation_switch_rs.contains("ConaryRuntimeRoot"),
        "generation switch orchestration must use ConaryRuntimeRoot for CAS, mount, and current paths"
    );
    assert!(
        !generation_commands_rs.contains("GENERATION_DB_CANDIDATES"),
        "generation commands must not retain mixed /conary and /var/lib/conary DB discovery"
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
fn recovery_does_not_promote_generations_by_erofs_magic_only() {
    let recovery_rs = fs::read_to_string(core_source("transaction/recovery.rs"))
        .expect("failed to read recovery.rs");

    assert!(
        recovery_rs.contains("load_installed_generation_artifact")
            || recovery_rs.contains("load_generation_artifact"),
        "recovery must load the generation artifact contract before promoting a generation"
    );
    assert!(
        !recovery_rs.contains("verity: false,\n                digest: None,"),
        "recovery must not hard-code plain composefs when metadata requests verity"
    );
}

#[test]
fn oci_generation_export_uses_generation_artifact_loader() {
    let export_rs = fs::read_to_string(app_source("commands/export.rs"))
        .expect("failed to read commands/export.rs");
    let cli_rs = fs::read_to_string(app_source("cli/mod.rs")).expect("failed to read cli/mod.rs");

    assert!(
        export_rs.contains("load_installed_generation_artifact(n)"),
        "explicit-generation OCI export must load the installed GenerationArtifact contract"
    );
    assert!(
        export_rs.contains("load_generation_artifact(current_path)"),
        "current-generation OCI export must load the GenerationArtifact contract from the current pointer"
    );
    assert!(
        export_rs.contains("Path::new(\"/conary/current\")"),
        "default OCI export must use /conary/current as the current-generation artifact pointer"
    );
    assert!(
        !export_rs.contains("let gen_dir = generation_path(gen_number);"),
        "OCI export must not independently resolve generation paths"
    );
    assert!(
        !export_rs.contains("_db_path")
            && !cli_rs.contains("db: String")
            && !cli_rs.contains("Path to the Conary database"),
        "OCI export must not retain DB-scoped compatibility arguments after artifact CAS scope becomes authoritative"
    );
}

#[test]
fn release_generation_commands_do_not_expose_live_switch_as_normal_activation() {
    let commands_rs = fs::read_to_string(app_source("commands/generation/commands.rs"))
        .expect("failed to read generation commands");
    let dispatch_rs = fs::read_to_string(workspace_file("apps/conary/src/dispatch.rs"))
        .expect("failed to read dispatch");
    let cli_rs = fs::read_to_string(workspace_file("apps/conary/src/cli/generation.rs"))
        .expect("failed to read generation cli");

    assert!(
        !commands_rs.contains("switch_live("),
        "release-facing generation commands must not call live switch directly"
    );
    assert!(
        !dispatch_rs.contains("switch_live("),
        "release-facing dispatch must not wire generation commands to live switch"
    );
    assert!(
        cli_rs.contains("Select a specific generation for next boot"),
        "generation switch CLI help must describe next-boot selection, not live activation"
    );
    assert!(
        cli_rs.contains("Select the previous generation for next boot"),
        "generation rollback CLI help must describe next-boot selection, not live activation"
    );
    assert!(
        !cli_rs.contains("Switch to a specific generation"),
        "generation switch CLI help must not preserve live activation wording"
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
fn generation_switch_fails_hard_on_etc_overlay_failures() {
    let switch_rs = fs::read_to_string(app_source("commands/generation/switch.rs"))
        .expect("failed to read commands/generation/switch.rs");

    assert!(
        switch_rs.contains("Failed to mount /etc overlay for live debug switch"),
        "debug live switch must fail hard on /etc overlay mount failures"
    );
    assert!(
        switch_rs.contains("let _ = unmount_generation(&staging);"),
        "debug live switch must clean up the PathBuf staging mount when /etc overlay setup fails"
    );
    assert!(
        !switch_rs.contains("eprintln!(\"Warning: Failed to mount /etc overlay: {e};"),
        "debug live switch must not treat /etc overlay failures as warning-only"
    );
}

#[test]
fn initramfs_generation_mounts_expose_usr_without_partial_generation_fallback() {
    let dracut_generator = fs::read_to_string(workspace_file(
        "packaging/dracut/90conary/conary-generator.sh",
    ))
    .expect("failed to read conary dracut generator");
    let bootstrap_config = fs::read_to_string(core_source("bootstrap/system_config.rs"))
        .expect("failed to read bootstrap system config");

    assert!(
        !dracut_generator.contains("Fall back to legacy bind-mount"),
        "dracut must not describe missing root.erofs as a compatibility path"
    );
    assert!(
        !dracut_generator.contains("mount --bind \"${GEN_DIR}/${dir}\""),
        "dracut must not bind-mount usr/etc from partial generation directories"
    );
    assert!(
        dracut_generator.contains("[ -f \"$EROFS_IMG\" ] ||"),
        "dracut must hard-fail when root.erofs is absent"
    );

    for (label, source) in [
        ("dracut generator", dracut_generator.as_str()),
        ("bootstrap initramfs", bootstrap_config.as_str()),
    ] {
        assert!(
            source.contains("expose_generation_usr"),
            "{label} must route generation /usr exposure through the shared post-composefs helper"
        );
        assert!(
            source.contains("ensure_root_symlink sbin usr/sbin"),
            "{label} must ensure /sbin resolves through usr-merge before switch_root"
        );
    }
}
