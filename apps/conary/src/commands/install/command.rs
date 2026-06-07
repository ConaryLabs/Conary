// src/commands/install/command.rs

use super::acquire::{CcsInstallParams, resolve_and_parse_package};
use super::dependencies::{DepAnalysisContext, handle_dependencies};
use super::prepare::check_upgrade_status;
use super::validation::{parse_component_and_validate, try_promote_existing_dep};
use super::{
    InstallOptions, InstallProgress, InstallSemantics, ScriptletContext, TransactionContext,
    UpgradeCheck, build_resolution_policy, execute_install_transaction, extract_and_classify_files,
    finalize_install, preflight_extracted_live_root_file_ownership,
    prepare_install_environment_before_scriptlets, resolve_canonical_name,
    resolve_default_dep_mode_from_model, run_pre_install_phase, show_dry_run_summary,
};
use crate::commands::open_db;
use anyhow::Result;
use conary_core::components::parse_component_spec;
use conary_core::repository::resolution_policy::RequestScope;

/// Install a package
///
/// Uses the unified resolution flow with per-package routing strategies.
/// Packages can be resolved from binary repos, on-demand converters, or recipes
/// based on their routing table entries.
pub async fn cmd_install(package: &str, opts: InstallOptions<'_>) -> Result<()> {
    let InstallOptions {
        db_path,
        root,
        version,
        repo,
        architecture,
        dry_run,
        no_deps,
        no_scripts,
        selection_reason,
        sandbox_mode,
        allow_downgrade,
        convert_to_ccs,
        no_capture,
        force,
        dep_mode,
        yes,
        from_distro,
        repository_provenance: requested_repository_provenance,
        legacy_replay,
    } = opts;

    // Hint if source policy is unconfigured (first-run guidance)
    crate::commands::hint_unconfigured_source_policy();

    // Open the database once for all pre-install checks (canonical resolution,
    // adoption check, promotion check). This connection is later promoted to `mut`
    // for the main install transaction.
    let conn = open_db(db_path)?;

    // Resolve dep_mode: if the user explicitly set --dep-mode use that,
    // otherwise derive from the system model convergence intent.
    let effective_dep_mode = dep_mode.unwrap_or_else(resolve_default_dep_mode_from_model);

    // --- Phase 1: Component parsing + canonical resolution + policy ---
    //
    // Parse component spec FIRST so that `nginx:devel` is split into base
    // name `nginx` and component `devel` before canonical resolution.
    // Without this, `resolve_canonical_name("nginx:devel")` looks for a
    // canonical package literally named "nginx:devel" and fails.
    let (base_name_for_canonical, early_component) = parse_component_spec(package)
        .map_or_else(|| (package.to_string(), None), |(b, c)| (b, Some(c)));

    let effective_source_policy =
        conary_core::repository::load_effective_policy(&conn, RequestScope::Any)?;
    let policy = build_resolution_policy(
        effective_source_policy.resolution,
        from_distro.as_deref(),
        repo.as_deref(),
    );
    let primary_flavor = effective_source_policy.primary_flavor;
    let resolved_name = resolve_canonical_name(
        &conn,
        &base_name_for_canonical,
        from_distro.as_deref(),
        &policy,
    )?;
    // If canonical resolution found a mapping, re-attach any component suffix
    // so downstream `parse_component_and_validate` sees the full spec.
    let resolved_package: String = match (&resolved_name, &early_component) {
        (Some(resolved), Some(comp)) => format!("{resolved}:{comp}"),
        (Some(resolved), None) => resolved.clone(),
        _ => package.to_string(),
    };
    let package: &str = &resolved_package;

    // --- Phase 2: Component parsing + pre-install validation ---
    let (package_name, component_selection) =
        parse_component_and_validate(&conn, package, effective_dep_mode, force)?;

    // --- Phase 3: Dependency-as-explicit promotion check ---
    if try_promote_existing_dep(&conn, &package_name, version.as_deref(), selection_reason)? {
        return Ok(());
    }

    // --- Phase 4: Package resolution + format detection ---
    let ccs_install_opts = CcsInstallParams {
        db_path,
        root,
        dry_run,
        sandbox_mode,
        no_deps,
        no_scripts,
        allow_downgrade,
        dep_mode: Some(effective_dep_mode),
        yes,
        repository_provenance: requested_repository_provenance,
        legacy_replay,
    };

    let Some((pkg, format, repository_provenance)) = resolve_and_parse_package(
        &conn,
        &package_name,
        package,
        db_path,
        version.as_deref(),
        repo.as_deref(),
        architecture.as_deref(),
        convert_to_ccs,
        no_capture,
        &policy,
        primary_flavor,
        &ccs_install_opts,
    )
    .await?
    else {
        // Already installed as CCS — no further processing needed.
        return Ok(());
    };
    let semantics = InstallSemantics::legacy(format);

    // Promote the pre-install connection to mutable for the main install transaction
    let mut conn = conn;

    let execution_path = prepare_install_environment_before_scriptlets(&conn, db_path, root)?;

    // --- Phase 5: Dependency analysis ---
    let dep_ctx = DepAnalysisContext {
        conn: &conn,
        pkg: pkg.as_ref(),
        no_deps,
        dry_run,
        dep_mode: Some(effective_dep_mode),
        yes,
        allow_downgrade,
        db_path,
        root,
        sandbox_mode,
        no_scripts,
        legacy_replay,
        policy: &policy,
        execution_path,
    };
    handle_dependencies(&dep_ctx).await?;

    // --- Phase 6: Dry run summary ---
    if dry_run {
        show_dry_run_summary(pkg.as_ref(), &component_selection);
        return Ok(());
    }

    // --- Phase 7: File extraction + component classification ---
    let progress = InstallProgress::single("Installing");
    let extraction = extract_and_classify_files(pkg.as_ref(), &component_selection, &progress)?;
    preflight_extracted_live_root_file_ownership(&conn, pkg.as_ref(), &extraction, execution_path)?;

    // --- Phase 8: Scriptlet execution (pre-install) ---
    let old_trove_to_upgrade =
        match check_upgrade_status(&conn, pkg.as_ref(), &semantics, allow_downgrade)? {
            UpgradeCheck::FreshInstall => None,
            UpgradeCheck::Upgrade(trove) | UpgradeCheck::Downgrade(trove) => Some(trove),
        };

    let scriptlet_ctx = ScriptletContext {
        root,
        no_scripts,
        sandbox_mode,
        semantics,
        old_trove: old_trove_to_upgrade.as_deref(),
    };
    let pre_scriptlet_state = run_pre_install_phase(
        &conn,
        pkg.as_ref(),
        &extraction.installed_component_types,
        &scriptlet_ctx,
        &progress,
    )?;

    // --- Phase 9: Transaction execution ---
    let tx_ctx = TransactionContext {
        db_path,
        root,
        semantics,
        selection_reason,
        old_trove_to_upgrade: old_trove_to_upgrade.as_deref(),
        ccs_manifest_provides: None,
        ccs_capabilities: None,
        execution_path,
        defer_generation: false,
        repository_provenance,
        legacy_replay,
        accepted_legacy_bundle: None,
    };
    let tx_result =
        execute_install_transaction(&mut conn, pkg.as_ref(), &extraction, &tx_ctx, &progress)?;

    // --- Phase 10: Post-install finalization ---
    finalize_install(
        &conn,
        pkg.as_ref(),
        &extraction,
        &scriptlet_ctx,
        &pre_scriptlet_state,
        &tx_result,
        &progress,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {

    #[test]
    fn package_execution_path_is_prepared_before_dependency_handling() {
        let source = include_str!("command.rs");
        let cmd_install_source = source;

        let execution_path_pos = cmd_install_source
            .find("let execution_path = prepare_install_environment_before_scriptlets")
            .expect("cmd_install should prepare execution path");
        let dependency_pos = cmd_install_source
            .find("handle_dependencies(&dep_ctx).await?")
            .expect("cmd_install should handle dependencies");

        assert!(
            execution_path_pos < dependency_pos,
            "cmd_install must fail closed and recover mutable journals before dependency installs can run scriptlets"
        );
    }

    #[test]
    fn direct_install_preflights_live_root_ownership_before_scriptlets() {
        let source = include_str!("command.rs");
        let cmd_install_source = source;

        let extraction_pos = cmd_install_source
            .find("let extraction = extract_and_classify_files")
            .expect("cmd_install should extract files");
        let preflight_pos = cmd_install_source
            .find("preflight_extracted_live_root_file_ownership(")
            .expect("cmd_install should preflight live-root ownership");
        let scriptlet_pos = cmd_install_source
            .find("run_pre_install_phase(")
            .expect("cmd_install should run pre-install scriptlets");

        assert!(
            extraction_pos < preflight_pos && preflight_pos < scriptlet_pos,
            "direct installs must preflight live-root ownership after extraction and before scriptlets"
        );
    }
}
