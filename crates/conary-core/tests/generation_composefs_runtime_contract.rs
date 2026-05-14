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
        "runtime generation publication must not request composefs verity from the digest alone; it must require proof that root.erofs actually has Linux fs-verity enabled"
    );
}

#[test]
fn generation_builder_stages_boot_assets_from_cas_sysroot_for_default_runtime_builds() {
    let builder_rs = fs::read_to_string(core_source("generation/builder.rs"))
        .expect("failed to read generation/builder.rs");

    assert!(
        builder_rs.contains("resolve_generation_boot_asset_sources("),
        "runtime generation builds must route boot asset resolution through the generation-aware resolver"
    );
    assert!(
        builder_rs.contains("materialize_runtime_generation_sysroot"),
        "default runtime builds must materialize boot inputs from CAS-backed generation contents"
    );
    assert!(
        builder_rs.contains(".arg(\"--sysroot\")") && builder_rs.contains(".arg(\"--kmoddir\")"),
        "dracut must build initramfs content from the materialized generation sysroot, not the live root"
    );
}

#[test]
fn runtime_generation_artifact_write_reuses_preverified_cas_inputs() {
    let builder_rs = fs::read_to_string(core_source("generation/builder.rs"))
        .expect("failed to read generation/builder.rs");
    let artifact_rs = fs::read_to_string(core_source("generation/artifact.rs"))
        .expect("failed to read generation/artifact.rs");

    assert!(
        builder_rs.contains(
            "verify_runtime_generation_cas_object_presence(generations_root, &cas_objects)?;"
        ),
        "runtime generation builds must check CAS object presence and size without rehashing every adopted object"
    );
    assert!(
        builder_rs.contains("cas_verification: CasObjectVerification::AlreadyVerified"),
        "runtime generation artifact writing must reuse the checked CAS set instead of hashing every object a second time"
    );
    assert!(
        artifact_rs.contains("CasObjectVerification::AlreadyVerified")
            && artifact_rs
                .contains("pub(crate) fn verify_cas_object_files_exist_with_expected_sizes"),
        "the artifact writer must have an explicit prechecked path that avoids duplicate deep CAS hashing"
    );
    assert!(
        artifact_rs
            .contains("load_generation_artifact_with_cas_verification(generation_dir, CasObjectVerification::Deep)")
            && artifact_rs
                .contains("CasObjectVerification::Deep => verify_cas_objects(&cas_dir, &cas_manifest.objects)?"),
        "export/import artifact loading must remain the deep verification point"
    );
    assert!(
        artifact_rs.contains("pub fn load_generation_artifact_for_activation")
            && artifact_rs.contains("CasObjectVerification::AlreadyVerified"),
        "local activation must validate the artifact contract without rehashing every CAS object"
    );
}

#[test]
fn recursive_ccs_dependency_installs_defer_generation_publication_until_root_package() {
    let conversion_rs = fs::read_to_string(app_source("commands/install/conversion.rs"))
        .expect("failed to read commands/install/conversion.rs");
    let install_rs = fs::read_to_string(app_source("commands/install/mod.rs"))
        .expect("failed to read commands/install/mod.rs");

    assert!(
        conversion_rs.contains("install_converted_ccs_with_pending(opts, Vec::new(), false)"),
        "root converted CCS installs must retain responsibility for publishing the generation"
    );
    assert!(
        conversion_rs
            .contains("child_pending_providers,\n                                    true,"),
        "recursive CCS dependency installs must defer generation publication until the root dependency closure is installed"
    );
    assert!(
        install_rs.contains("pub defer_generation: bool")
            && install_rs.contains("defer_generation: opts.defer_generation"),
        "CCS transaction options must carry the generation-publication boundary into transaction execution"
    );

    let transaction_body = install_rs
        .split("fn execute_install_transaction")
        .nth(1)
        .expect("failed to isolate execute_install_transaction body");
    let deferred_branch = transaction_body
        .find("if ctx.defer_generation")
        .expect("deferred CCS dependencies must skip generation rebuild");
    let rebuild_generation = transaction_body
        .find("composefs_ops::rebuild_and_mount")
        .expect("root installs must still publish a composefs generation");
    assert!(
        deferred_branch < rebuild_generation,
        "deferred dependency commits must return before rebuilding and selecting a generation"
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
fn generation_activation_validates_artifacts_before_pointer_updates() {
    let commands_rs = fs::read_to_string(app_source("commands/generation/commands.rs"))
        .expect("failed to read commands/generation/commands.rs");
    let switch_rs = fs::read_to_string(app_source("commands/generation/switch.rs"))
        .expect("failed to read commands/generation/switch.rs");
    let builder_rs = fs::read_to_string(app_source("commands/generation/builder.rs"))
        .expect("failed to read commands/generation/builder.rs");

    assert!(
        commands_rs.contains("load_generation_artifact_for_activation"),
        "next-boot activation must validate the generation artifact contract before selecting a generation without rehashing every local CAS object"
    );

    let switch_body = commands_rs
        .split("pub fn cmd_generation_switch")
        .nth(1)
        .and_then(|rest| rest.split("/// Roll back").next())
        .expect("failed to isolate cmd_generation_switch body");
    let switch_validate = switch_body
        .find("validate_generation_activation_artifact(&runtime_root, number)?;")
        .expect("generation switch must validate artifact contract");
    let switch_update = switch_body
        .find("update_current_symlink")
        .expect("generation switch must update current pointer");
    assert!(
        switch_validate < switch_update,
        "generation switch must validate the artifact before updating /conary/current"
    );
    assert!(
        switch_body.contains("mark_generation_state_active(&runtime_root, number)?;"),
        "generation switch must mark the matching DB state active when it publishes /conary/current"
    );

    let rollback_body = commands_rs
        .split("pub fn cmd_generation_rollback")
        .nth(1)
        .and_then(|rest| rest.split("/// Recover").next())
        .expect("failed to isolate cmd_generation_rollback body");
    let rollback_validate = rollback_body
        .find("validate_generation_activation_artifact(&runtime_root, *previous)?;")
        .expect("generation rollback must validate artifact contract");
    let rollback_update = rollback_body
        .find("update_current_symlink")
        .expect("generation rollback must update current pointer");
    assert!(
        rollback_validate < rollback_update,
        "generation rollback must validate the artifact before updating /conary/current"
    );
    assert!(
        rollback_body.contains("mark_generation_state_active(&runtime_root, *previous)?;"),
        "generation rollback must mark the matching DB state active when it publishes /conary/current"
    );

    assert!(
        switch_rs.contains("load_generation_artifact_for_activation(&gen_dir)"),
        "debug live switch must also validate the local artifact contract before mounting"
    );
    assert!(
        builder_rs.contains("GenerationActivation::Inactive"),
        "manual generation build must prepare an inactive generation; activation belongs to generation switch"
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
    let transaction_rs =
        fs::read_to_string(core_source("transaction/mod.rs")).expect("failed to read mod.rs");

    assert!(
        recovery_rs.contains("load_generation_artifact_for_activation"),
        "recovery must load the generation artifact contract before promoting a generation"
    );
    assert!(
        !recovery_rs.contains("is_valid_erofs_image"),
        "recovery must not retain the old EROFS magic-number promotion helper"
    );
    assert!(
        !recovery_rs.contains("verity: false,\n                digest: None,"),
        "recovery must not hard-code plain composefs when metadata requests verity"
    );
    assert!(
        recovery_rs.contains("SelectedGenerationOnly"),
        "transaction recovery must not auto-promote unselected build-only generations"
    );
    assert!(
        recovery_rs.contains("leaving boot selection unmounted"),
        "ordinary transaction recovery must repair /conary/current selection without live-mounting it"
    );
    assert!(
        transaction_rs.contains("BUILT -> SELECTED -> DONE")
            && !transaction_rs.contains("BUILT -> MOUNTED -> DONE"),
        "transaction lifecycle docs must describe atomic generation selection, not legacy live mounting"
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
fn composefs_apply_publishes_next_boot_generation_without_live_mounting() {
    let composefs_ops_rs = fs::read_to_string(app_source("commands/composefs_ops.rs"))
        .expect("failed to read commands/composefs_ops.rs");
    let rebuild_body = composefs_ops_rs
        .split("pub fn rebuild_and_mount")
        .nth(1)
        .expect("failed to isolate rebuild_and_mount body");

    assert!(
        rebuild_body
            .contains("enable_generation_rootfs_verity(&gen_dir, &build_result.image_path)"),
        "runtime package mutation must preserve the fs-verity enablement step before generation selection"
    );
    assert!(
        rebuild_body.contains("update_current_symlink(runtime_root.root(), gen_num)"),
        "runtime package mutation must publish the generated artifact by updating /conary/current"
    );
    assert!(
        !rebuild_body.contains("mount_generation("),
        "runtime package mutation must not attempt live composefs remounts; activation is atomic next-boot selection"
    );
    assert!(
        !rebuild_body.contains("mount_etc_overlay(")
            && !rebuild_body.contains("Path::new(\"/etc\")"),
        "runtime package mutation must not remount the live /etc overlay during package installs or removes"
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
fn generation_recovery_fails_hard_on_etc_overlay_failures() {
    let commands_rs = fs::read_to_string(app_source("commands/generation/commands.rs"))
        .expect("failed to read commands/generation/commands.rs");

    assert!(
        commands_rs
            .contains("Failed to restore /etc overlay after recovery for generation {gen_num}"),
        "generation recovery must fail hard on /etc overlay mount failures"
    );
    assert!(
        commands_rs.contains("unmount_generation(&staging)"),
        "generation recovery must clean up the staged generation mount when /etc overlay setup fails"
    );
    assert!(
        !commands_rs
            .contains("tracing::warn!(\"Failed to restore /etc overlay after recovery: {e}\");"),
        "generation recovery must not keep warning-only /etc overlay behavior"
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
