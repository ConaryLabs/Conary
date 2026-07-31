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
    let boot_assets_rs = fs::read_to_string(core_source("generation/builder/boot_assets.rs"))
        .expect("failed to read generation/builder/boot_assets.rs");
    let sysroot_rs = fs::read_to_string(core_source("generation/builder/sysroot.rs"))
        .expect("failed to read generation/builder/sysroot.rs");
    let initramfs_rs = fs::read_to_string(core_source("generation/builder/initramfs.rs"))
        .expect("failed to read generation/builder/initramfs.rs");

    assert!(
        boot_assets_rs.contains("resolve_generation_boot_asset_sources("),
        "runtime generation builds must route boot asset resolution through the generation-aware resolver"
    );
    assert!(
        sysroot_rs.contains("materialize_runtime_generation_sysroot"),
        "default runtime builds must materialize boot inputs from CAS-backed generation contents"
    );
    assert!(
        initramfs_rs.contains(".arg(\"--sysroot\")")
            && initramfs_rs.contains(".arg(\"--kmoddir\")"),
        "dracut must build initramfs content from the materialized generation sysroot, not the live root"
    );
}

#[test]
fn generation_builder_retains_boot_preparation_mutations_before_freezing_erofs() {
    for path in [
        "generation/builder/create.rs",
        "generation/builder/rebuild.rs",
    ] {
        let source =
            fs::read_to_string(core_source(path)).expect("failed to read generation builder");
        let prepare = source
            .find("resolve_generation_boot_asset_sources(&mut runtime_inputs")
            .expect("generation builder must finalize boot-derived manifest authority");
        let freeze = source
            .find("build_erofs_image_from_root_manifest(&runtime_inputs.generation")
            .expect("generation builder must freeze the finalized immutable manifest");

        assert!(
            prepare < freeze,
            "{path} must retain exact depmod output before writing the immutable EROFS image"
        );
    }
}

#[test]
fn runtime_generation_artifact_write_reuses_preverified_cas_inputs() {
    let create_rs = fs::read_to_string(core_source("generation/builder/create.rs"))
        .expect("failed to read generation/builder/create.rs");
    let rebuild_rs = fs::read_to_string(core_source("generation/builder/rebuild.rs"))
        .expect("failed to read generation/builder/rebuild.rs");
    let artifact_rs = fs::read_to_string(core_source("generation/artifact.rs"))
        .expect("failed to read generation/artifact.rs");

    for (label, source) in [
        ("create.rs", create_rs.as_str()),
        ("rebuild.rs", rebuild_rs.as_str()),
    ] {
        assert!(
            source.contains(
                "verify_runtime_generation_cas_object_presence(generations_root, &cas_objects)?;"
            ),
            "{label} must check CAS object presence and size without rehashing every adopted object"
        );
        assert!(
            source.contains("cas_verification: CasObjectVerification::AlreadyVerified"),
            "{label} must reuse the checked CAS set instead of hashing every object a second time"
        );
    }
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
        !recovery_rs.contains("mark_complete_through"),
        "core link/artifact recovery must not make app-owned publication debt terminal"
    );
    assert!(
        transaction_rs.contains("BUILT -> SELECTED -> DONE")
            && !transaction_rs.contains("BUILT -> MOUNTED -> DONE"),
        "transaction lifecycle docs must describe atomic generation selection, not legacy live mounting"
    );
}

#[test]
fn publication_writes_generation_db_backup_before_marking_debt_complete() {
    let publication_rs = fs::read_to_string(app_source("commands/generation/publication.rs"))
        .expect("failed to read generation publication source");
    let replay_rs = publication_rs
        .split_once("fn replay_publication")
        .map(|(_, replay)| replay)
        .expect("publication must expose the persisted replay machine");

    let mark_active = replay_rs
        .find("mark_generation_state_active")
        .expect("publication must mark the selected generation active");
    let backup = replay_rs
        .find("create_generation_db_backup")
        .expect("publication must write a generation-bound DB backup");
    let backup_phase = replay_rs
        .find("GenerationPublicationPhase::DatabaseBackedUp")
        .expect("publication must persist generation DB backup completion");

    assert!(
        mark_active < backup && backup < backup_phase,
        "generation DB backup must be written after state activation and before the durable backup phase is recorded"
    );
    assert!(
        publication_rs.contains("debt.mark_complete_through"),
        "publication must finalize covered debt through the backup-gated model API"
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
fn dracut_generator_is_the_single_generation_activation_authority() {
    let dracut_generator = fs::read_to_string(workspace_file(
        "packaging/dracut/90conary/conary-generator.sh",
    ))
    .expect("failed to read conary dracut generator");
    let dracut_init =
        fs::read_to_string(workspace_file("packaging/dracut/90conary/conary-init.sh"))
            .expect("failed to read conary dracut init");
    let dracut_module =
        fs::read_to_string(workspace_file("packaging/dracut/90conary/module-setup.sh"))
            .expect("failed to read conary dracut module setup");

    assert!(
        dracut_module.contains("install_conary_script \"$moddir/conary-init.sh\" \"/init\""),
        "installed-runtime exports need a Conary-owned /init so the carrier root can activate composefs before switch_root"
    );
    assert!(
        dracut_module.contains("${dracutsysrootdir-}/conary/generations"),
        "dracut module detection must honor --sysroot when generation builds materialize boot assets from CAS"
    );
    assert!(
        dracut_module.contains(
            "install_conary_script \"$moddir/conary-generator.sh\" \"/sbin/conary-generator\""
        ),
        "the Conary init script must be able to invoke the generator directly"
    );
    assert!(
        dracut_module.contains("/var/lib/dracut/hooks/pre-pivot/90-conary-generator.sh"),
        "the generated module must still support dracut's normal pre-pivot hook path"
    );
    assert!(
        dracut_init.contains("exec switch_root \"$SYSROOT\" /sbin/init"),
        "Conary init must switch into the generation-activated sysroot instead of falling through to the carrier root"
    );
    assert!(
        dracut_init.contains("/sbin/conary-generator"),
        "Conary init must run generation activation before switch_root"
    );
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
    assert!(
        dracut_generator.contains("ETC_LOWER=\"${SYSROOT}/conary/etc-lower\""),
        "generations without immutable /etc need one explicit empty overlay lower"
    );
    assert!(
        dracut_generator.contains("cp -a \"$ETC_STATE_SEED\" \"$ETC_STATE_RUNTIME\""),
        "readonly carriers must seed their tmpfs upper from exported config-state authority"
    );
    assert!(
        dracut_generator.contains("if [ ! -d \"$ETC_STATE_SEED\" ]; then")
            && dracut_generator.contains("if [ ! -d \"$ETC_UPPER\" ]; then"),
        "boot must fail closed instead of creating an empty /etc upper when typed state is absent"
    );
    assert!(
        dracut_module.contains("inst_multiple -o blkid cp grep"),
        "the initramfs must carry the copy tool required for readonly config-state seeding"
    );

    assert!(
        dracut_generator.contains("expose_generation_usr"),
        "the dracut generator must route generation /usr exposure through the post-composefs helper"
    );
    assert!(
        dracut_generator.contains("ensure_root_symlink sbin usr/sbin"),
        "the dracut generator must ensure /sbin resolves through usr-merge before switch_root"
    );
    assert!(
        !dracut_init.contains("expose_generation_usr()"),
        "conary-init must delegate generation activation to /sbin/conary-generator instead of reimplementing /usr exposure"
    );
    assert!(
        dracut_generator.contains("read_kernel_value conary.carrier"),
        "the dracut generator must read the carrier mode from the kernel command line"
    );
    assert!(
        dracut_generator.contains("ETC_STATE_BASE=\"${SYSROOT}/run/conary/etc-state\""),
        "a readonly carrier must place the /etc overlay upper under the runtime tmpfs"
    );
    assert!(
        dracut_generator.contains("ETC_STATE_BASE=\"${SYSROOT}/conary/etc-state\""),
        "a writable carrier must keep the /etc overlay upper on persistent generation state"
    );
    assert!(
        dracut_init.contains("mount_once tmpfs tmpfs \"$SYSROOT/run\""),
        "conary-init must provide the readonly carrier runtime tmpfs the generator's etc-state depends on"
    );
    assert!(
        dracut_init.contains("mount_once tmpfs tmpfs \"$SYSROOT/var\""),
        "conary-init must provide a writable /var for readonly carrier roots"
    );
}

#[test]
fn installed_generation_export_boot_assets_force_conary_initramfs() {
    let boot_assets_rs = fs::read_to_string(core_source("generation/builder/boot_assets.rs"))
        .expect("failed to read generation/builder/boot_assets.rs");

    assert!(
        boot_assets_rs.contains("InitramfsPolicy::GenerateConary"),
        "installed-runtime generation export must generate a Conary-aware initramfs instead of reusing an adopted host initramfs"
    );
    assert!(
        boot_assets_rs.contains("resolve_generation_boot_asset_sources_with_tools"),
        "the default /boot generation path must have an explicit boot-asset resolver that can force Conary initramfs generation"
    );
}
