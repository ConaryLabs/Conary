// src/commands/install/command.rs

use super::acquire::{CcsInstallParams, resolve_and_parse_package};
use super::ccs_removal_hooks::CcsRemovalHookPlan;
use super::dependencies::{DepAnalysisContext, handle_dependencies};
use super::native_events::{NativeInstallInput, PreparedNativeTransaction};
use super::prepare::check_upgrade_status;
use super::validation::{parse_component_and_validate, try_promote_existing_dep};
use super::{
    InstallIntent, InstallOptions, InstallProgress, InstallSemantics, NativeLifecycleInstallState,
    TransactionContext, UpgradeCheck, bind_transaction_source_identity, build_execution_mode,
    build_resolution_policy, effective_source_profile,
    execute_install_transaction_in_selected_root, extract_and_classify_files, finalize_install,
    preflight_extracted_file_ownership, resolve_canonical_name, show_dry_run_summary,
    source_profile_projection,
};
use crate::commands::generation::selected_root::LockedRuntimeRoot;
use crate::commands::open_db;
use anyhow::{Context, Result};
use conary_core::components::parse_component_spec;
use conary_core::repository::resolution_policy::RequestScope;
use conary_core::transaction::{plan_package_relations, validate_package_relation_plan};
use std::path::Path;

/// Install a package
///
/// Uses the unified resolution flow with per-package routing strategies.
/// Packages can be resolved from binary repos, on-demand converters, or recipes
/// based on their routing table entries.
pub async fn cmd_install(package: &str, opts: InstallOptions<'_>) -> Result<()> {
    cmd_install_with_intent(package, opts, InstallIntent::PackageChange).await
}

pub(crate) async fn cmd_install_replatform(package: &str, opts: InstallOptions<'_>) -> Result<()> {
    cmd_install_with_intent(package, opts, InstallIntent::Replatform).await
}

async fn cmd_install_with_intent(
    package: &str,
    opts: InstallOptions<'_>,
    intent: InstallIntent,
) -> Result<()> {
    let InstallOptions {
        db_path,
        root,
        version,
        package_release,
        repo,
        architecture,
        dry_run,
        no_deps,
        selection_reason,
        sandbox_mode,
        allow_downgrade,
        convert_to_ccs,
        ownership,
        yes,
        from_source,
        repository_provenance: requested_repository_provenance,
    } = opts;

    // Hint if source policy is unconfigured (first-run guidance)
    crate::commands::hint_default_convergence();

    // Open the database once for all pre-install checks (canonical resolution,
    // adoption check, promotion check). This connection is later promoted to `mut`
    // for the main install transaction.
    let conn = open_db(db_path)?;

    // Preserve native ownership unless takeover is explicit on this operation.
    let effective_ownership = ownership.unwrap_or_default();

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
    let requested_source_profile = from_source.as_deref().and_then(source_profile_projection);
    let policy = build_resolution_policy(
        &conn,
        effective_source_policy.resolution,
        from_source.as_deref(),
        repo.as_deref(),
    )?;
    let resolved_name = resolve_canonical_name(
        &conn,
        &base_name_for_canonical,
        from_source.as_deref(),
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
        parse_component_and_validate(&conn, package, architecture.as_deref(), effective_ownership)?;

    // --- Phase 3: Dependency-as-explicit promotion check ---
    if try_promote_existing_dep(
        &conn,
        &package_name,
        version.as_deref(),
        architecture.as_deref(),
        selection_reason,
    )? {
        return Ok(());
    }

    // --- Phase 4: Package resolution + format detection ---
    let ccs_install_opts = CcsInstallParams {
        db_path,
        root,
        dry_run,
        sandbox_mode,
        no_deps,
        allow_downgrade,
        intent,
        yes,
        repository_provenance: requested_repository_provenance,
    };

    let Some((pkg, format, repository_provenance)) = resolve_and_parse_package(
        &conn,
        &package_name,
        package,
        db_path,
        version.as_deref(),
        package_release.as_deref(),
        repo.as_deref(),
        architecture.as_deref(),
        convert_to_ccs,
        &policy,
        requested_source_profile,
        &ccs_install_opts,
    )
    .await?
    else {
        // Already installed as CCS — no further processing needed.
        return Ok(());
    };
    let policy = bind_transaction_source_identity(policy, repository_provenance.as_ref())?;
    let source_profile =
        effective_source_profile(repository_provenance.as_ref(), requested_source_profile);
    let semantics = InstallSemantics::native_package(format);

    // Promote the pre-install connection to mutable for the main install transaction
    let mut conn = conn;

    // --- Phase 5: Dependency analysis ---
    let dep_ctx = DepAnalysisContext {
        conn: &conn,
        pkg: pkg.as_ref(),
        no_deps,
        dry_run,
        yes,
        allow_downgrade,
        db_path,
        sandbox_mode,
        policy: &policy,
    };
    handle_dependencies(&dep_ctx).await?;

    // Dry-run planning is read-only and does not participate in the runtime
    // mutation serialization boundary. A real install takes that boundary
    // before reading any installed authority its transaction will certify.
    let locked_root = if dry_run {
        None
    } else {
        Some(LockedRuntimeRoot::acquire(db_path)?)
    };
    let relation_plan = plan_package_relations(&conn, pkg.as_ref(), semantics.version_scheme)
        .context("Failed to plan package conflicts and replacements")?;
    validate_package_relation_plan(&conn, &relation_plan)
        .context("Package conflicts and replacements cannot be applied")?;

    // --- Phase 6: Dry run summary ---
    if dry_run {
        show_dry_run_summary(pkg.as_ref(), &component_selection)?;
        show_relation_removals(&relation_plan);
        return Ok(());
    }

    // --- Phase 7: File extraction + lossless component assignment ---
    let progress = InstallProgress::single("Installing");
    let extraction = extract_and_classify_files(pkg.as_ref(), &component_selection, &progress)?;
    preflight_extracted_file_ownership(&conn, pkg.as_ref(), &extraction, &relation_plan.removals)?;

    // --- Phase 8: Scriptlet execution (pre-install) ---
    let old_trove_to_upgrade =
        match check_upgrade_status(&conn, pkg.as_ref(), &semantics, allow_downgrade, intent)? {
            UpgradeCheck::FreshInstall => None,
            UpgradeCheck::AlreadyInstalled(trove) => {
                anyhow::bail!(
                    "Package {} version {} ({}) is already installed",
                    trove.name,
                    trove.version,
                    trove.architecture.as_deref().unwrap_or("no-arch")
                )
            }
            UpgradeCheck::Upgrade(trove)
            | UpgradeCheck::Downgrade(trove)
            | UpgradeCheck::Replatform(trove) => Some(trove),
        };
    let native_lifecycle_state =
        NativeLifecycleInstallState::from_native_package(pkg.as_ref(), format, source_profile)?;
    let repository_enrollments = if format == crate::commands::PackageFormatType::Rpm {
        let architecture = pkg.architecture().ok_or_else(|| {
            anyhow::anyhow!("RPM repository enrollment requires package architecture authority")
        })?;
        conary_core::repository::enrollment::derive::derive_rpm_repository_enrollments(
            &extraction.extracted_files,
            architecture,
            source_profile,
        )?
    } else {
        Vec::new()
    };
    let resolution_capabilities = pkg.resolution_capabilities()?;
    let native_transaction = PreparedNativeTransaction::prepare_install(
        &conn,
        NativeInstallInput {
            package_name: pkg.name(),
            package_version: pkg.version(),
            package_arch: pkg.architecture(),
            version_scheme: semantics.version_scheme,
            provides: &resolution_capabilities,
            new_bundle: native_lifecycle_state.bundle_to_persist.as_ref(),
            old_trove: old_trove_to_upgrade.as_deref(),
            relation_removals: &relation_plan.removals,
            relation_deconfigurations: &relation_plan.deconfigurations,
            paths: extraction
                .extracted_files
                .iter()
                .map(|file| file.path.clone())
                .collect(),
        },
    )?;
    let ccs_removal_hook_plan = CcsRemovalHookPlan::prepare(
        &conn,
        old_trove_to_upgrade.as_deref(),
        relation_plan.removals.iter(),
    )?;
    let native_execution_mode = build_execution_mode(
        old_trove_to_upgrade
            .as_deref()
            .map(|trove| trove.version.as_str()),
    );
    let mut selected_root = locked_root
        .context("real install has no locked runtime root")?
        .materialize(&conn, format!("Install {}-{}", pkg.name(), pkg.version()))?;
    let transaction_root = selected_root.selected_root().to_string_lossy().into_owned();
    native_transaction.preflight(Path::new(&transaction_root), &native_execution_mode)?;
    let preflighted_ccs_removal_hooks =
        ccs_removal_hook_plan.preflight(Path::new(&transaction_root), sandbox_mode)?;
    preflighted_ccs_removal_hooks.execute()?;
    // --- Phase 9: Graph-driven lifecycle and transaction execution ---
    let tx_ctx = TransactionContext {
        db_path,
        root: &transaction_root,
        semantics,
        selection_reason,
        old_trove_to_upgrade: old_trove_to_upgrade.as_deref(),
        ccs_capabilities: None,
        ccs_file_capabilities: None,
        defer_generation: false,
        repository_provenance,
        native_lifecycle_bundle: native_lifecycle_state.bundle_to_persist.as_ref(),
        repository_enrollments: &repository_enrollments,
        relation_removals: &relation_plan.removals,
        relation_deconfigurations: &relation_plan.deconfigurations,
        retain_replaced_payload_until_lifecycle: native_transaction
            .requires_upgrade_payload_boundary(),
    };
    let tx_result = execute_install_transaction_in_selected_root(
        &mut conn,
        pkg.as_ref(),
        &extraction,
        &tx_ctx,
        &progress,
        &mut selected_root,
        &native_transaction,
        &native_execution_mode,
    )?;

    // --- Phase 10: Post-install finalization ---
    finalize_install(
        &conn,
        pkg.as_ref(),
        &extraction,
        root,
        &tx_result,
        &progress,
    )?;
    Ok(())
}

fn show_relation_removals(plan: &conary_core::transaction::PackageRelationPlan) {
    for removal in &plan.removals {
        println!(
            "  Would remove {} {} ({})",
            removal.package_name,
            removal.package_version,
            removal.kind.as_str()
        );
    }
}

#[cfg(test)]
#[path = "command_mutation_lock_tests.rs"]
mod mutation_lock_tests;
