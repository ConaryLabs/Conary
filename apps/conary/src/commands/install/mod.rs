// src/commands/install/mod.rs
//! Package installation commands

mod batch;
mod blocklist;
mod ccs_transaction;
mod conversion;
mod dep_mode;
mod dep_resolution;
mod dependencies;
mod execute;
mod inner;
mod legacy_replay;
mod lifecycle;
mod options;
mod prepare;
mod resolve;
mod restore;
mod scriptlets;
mod semantics;
mod source_policy;
mod system_pm;
mod transaction;

pub use batch::{BatchInstaller, prepare_package_for_batch};
pub use blocklist::is_blocked as is_package_blocked;
pub use dep_mode::DepMode;
pub(crate) use dependencies::resolve_default_dep_mode_from_model;

#[allow(unused_imports)]
pub(crate) use ccs_transaction::{
    CcsTransactionInstallOptions, CcsTransactionInstallResult, install_ccs_package_transactionally,
};

pub use legacy_replay::LegacyReplayOptions;
#[allow(unused_imports)]
pub(crate) use legacy_replay::{
    AcceptedLegacyBundleInstall, LegacyReplayAuditContext, LegacyReplayInstallState,
};
pub(super) use legacy_replay::{
    merge_old_upgrade_legacy_replay_state, plan_ccs_fresh_install_legacy_replay,
    plan_ccs_old_installed_upgrade_legacy_replay,
};
pub use options::InstallOptions;
pub(crate) use options::{RepositoryInstallProvenance, repository_install_provenance_from_package};
pub use prepare::{ComponentSelection, UpgradeCheck};
pub(crate) use restore::{
    add_prepared_install_to_target_state, build_target_state_view,
    finalize_prepared_install_without_snapshot, install_prepared_inner,
    prepare_install_for_restore, run_pre_install_for_prepared,
    validate_prepared_install_dependencies,
};

use super::open_db;
use conversion::{
    ConversionResult, ConvertedCcsInstallOptions, DEFAULT_CCS_DEPENDENCY_PASSES,
    install_converted_ccs, try_convert_to_ccs,
};
use dependencies::{DepAnalysisContext, handle_dependencies};
use execute::{
    PackageExecutionPath, live_root_files_from_stored_files,
    preflight_extracted_live_root_file_ownership, prepare_install_environment_before_scriptlets,
    run_triggers,
};
use lifecycle::{
    ExtractionResult, PreScriptletState, ScriptletContext, extract_and_classify_files,
    finalize_install, finalize_install_without_snapshot, mark_upgraded_parent_deriveds_stale,
    run_pre_install_phase, show_dry_run_summary,
};
use prepare::{check_upgrade_status, parse_package};
use resolve::{
    PolicyOptions, ResolutionOutcome, ResolvedSourceType, resolve_package_path_with_policy,
};
use semantics::{InstallSemantics, PreparedSourceKind, scheme_to_string};
use source_policy::{build_resolution_policy, resolve_canonical_name};
use transaction::{InstallTransactionResult, TransactionContext, execute_install_transaction};

use super::progress::{InstallPhase, InstallProgress};
use super::{PackageFormatType, detect_package_format};
use anyhow::{Context, Result};
use conary_core::components::{ComponentType, parse_component_spec};
use conary_core::scriptlet::SandboxMode;
use std::collections::HashMap;
use tracing::info;
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
    super::hint_unconfigured_source_policy();

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

    let effective_source_policy = conary_core::repository::load_effective_policy(
        &conn,
        conary_core::repository::resolution_policy::RequestScope::Any,
    )?;
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

// ---------------------------------------------------------------------------
// Parameter grouping structs for extracted functions
// ---------------------------------------------------------------------------

/// Parameters for CCS direct-install that are forwarded from `InstallOptions`.
struct CcsInstallParams<'a> {
    db_path: &'a str,
    root: &'a str,
    dry_run: bool,
    sandbox_mode: SandboxMode,
    no_deps: bool,
    no_scripts: bool,
    allow_downgrade: bool,
    dep_mode: Option<DepMode>,
    yes: bool,
    repository_provenance: Option<RepositoryInstallProvenance>,
    legacy_replay: LegacyReplayOptions,
}
// ---------------------------------------------------------------------------
// Extracted helper functions
// ---------------------------------------------------------------------------
/// Parse a component spec from the package argument and run pre-install
/// validation checks (blocklist, adoption).
///
/// Returns `(package_name, component_selection)`.
fn parse_component_and_validate(
    conn: &rusqlite::Connection,
    package: &str,
    dep_mode: DepMode,
    force: bool,
) -> Result<(String, ComponentSelection)> {
    // Parse component spec from package argument (e.g., "nginx:devel" or "nginx:all")
    let (package_name, component_selection) = if let Some((pkg, comp)) =
        parse_component_spec(package)
    {
        let selection = if comp == "all" {
            ComponentSelection::All
        } else if let Some(comp_type) = ComponentType::parse(&comp) {
            ComponentSelection::Specific(vec![comp_type])
        } else {
            return Err(anyhow::anyhow!(
                "Unknown component '{}'. Valid components: runtime, lib, devel, doc, config, all",
                comp
            ));
        };
        (pkg, selection)
    } else {
        // No component spec - install defaults only
        (package.to_string(), ComponentSelection::Defaults)
    };

    info!(
        "Installing package: {} (components: {})",
        package_name,
        component_selection.display()
    );

    // Block installation of critical system packages in takeover mode
    if dep_mode == DepMode::Takeover && blocklist::is_blocked(&package_name) {
        return Err(anyhow::anyhow!(
            "Package '{}' is on the critical system blocklist and cannot be taken over. \
             These packages (glibc, systemd, etc.) must remain managed by the system package manager.",
            package_name
        ));
    }

    // Check if the package is adopted from the system PM. `--force` alone must
    // not silently convert native-manager ownership into Conary ownership.
    if let Some(existing) = conary_core::db::models::Trove::find_one_by_name(conn, &package_name)?
        && existing.install_source.is_adopted()
    {
        if dep_mode == DepMode::Takeover {
            println!(
                "[INFO] Package '{}' is adopted -- proceeding with explicit --dep-mode takeover",
                package_name
            );
        } else {
            let pkg_mgr = conary_core::packages::SystemPackageManager::detect();
            let force_note = if force {
                " --force does not override adopted package ownership."
            } else {
                ""
            };
            return Err(anyhow::anyhow!(
                "Package '{}' is adopted from {}.{} Run 'conary system adopt --refresh' after native package-manager changes. Use 'conary install {} --dep-mode takeover' \
                 for explicit package takeover, or 'conary system takeover' for generation-level takeover.",
                package_name,
                pkg_mgr.display_name(),
                force_note,
                package_name
            ));
        }
    }

    Ok((package_name, component_selection))
}

/// Check if the package is already installed as a dependency and promote it
/// to explicit.  Returns `true` if no further work is needed (same version).
fn try_promote_existing_dep(
    conn: &rusqlite::Connection,
    package_name: &str,
    version: Option<&str>,
    selection_reason: Option<&str>,
) -> Result<bool> {
    // Check if the package is already installed as a dependency - if so, promote it
    // This must happen before we try to download, as we may not need to do anything else
    if let Some(existing) = conary_core::db::models::Trove::find_one_by_name(conn, package_name)?
        && existing.install_reason == conary_core::db::models::InstallReason::Dependency
    {
        // Check if we're requesting a specific version that differs
        let needs_version_change = version.is_some_and(|v| v != existing.version);

        // Promote to explicit
        let reason = selection_reason.unwrap_or("Explicitly installed by user");
        conary_core::db::models::Trove::promote_to_explicit(conn, package_name, Some(reason))?;
        println!("Promoted {} from dependency to explicit", package_name);

        // If same version (or no version specified), we're done
        if !needs_version_change {
            println!("{} {} is already installed", package_name, existing.version);
            return Ok(true);
        }
        // Otherwise continue with version upgrade
        info!(
            "Continuing with version change: {} -> {:?}",
            existing.version, version
        );
    }
    Ok(false)
}

/// Resolve a package path, detect its format, and parse it.
///
/// Handles early returns for CCS packages (from Remi, by extension, or via
/// conversion).  Returns `None` if the package was already installed as CCS
/// (no further processing needed), or `Some(...)` with the parsed legacy
/// package and its format type.
#[allow(clippy::too_many_arguments)]
async fn resolve_and_parse_package(
    conn: &rusqlite::Connection,
    package_name: &str,
    package: &str,
    db_path: &str,
    version: Option<&str>,
    repo: Option<&str>,
    architecture: Option<&str>,
    convert_to_ccs: bool,
    no_capture: bool,
    policy: &conary_core::repository::resolution_policy::ResolutionPolicy,
    primary_flavor: Option<conary_core::repository::dependency_model::RepositoryDependencyFlavor>,
    ccs_opts: &CcsInstallParams<'_>,
) -> Result<
    Option<(
        Box<dyn conary_core::packages::PackageFormat>,
        PackageFormatType,
        Option<RepositoryInstallProvenance>,
    )>,
> {
    // Create progress tracker for single package installation
    let progress = InstallProgress::single("Installing");
    progress.set_phase(package_name, InstallPhase::Downloading);

    // Build policy options for resolution.  The root request carries the
    // full policy; transitive deps will inherit the mixing policy but not
    // the request scope (that is handled inside the resolver).
    let policy_opts = PolicyOptions {
        policy: Some(policy.clone()),
        is_root: true,
        primary_flavor,
    };

    // Resolve package path (download if needed).
    // Checksum verification and temp-file cleanup on failure are handled
    // inside conary_core::repository::download (fix 1.4).
    // TODO(round2): Surface partial-download byte counts in error messages
    // so users can diagnose connection issues vs corrupt mirrors.
    let resolved = match resolve_package_path_with_policy(
        package_name,
        db_path,
        version,
        repo,
        architecture,
        &progress,
        &policy_opts,
    )
    .await
    {
        Err(e) => {
            print_package_suggestions(conn, package_name);
            return Err(e);
        }
        Ok(ResolutionOutcome::AlreadyInstalled { name, version }) => {
            // Use a specific error type that the caller handles as a clean exit
            return Err(anyhow::anyhow!(
                "ALREADY_INSTALLED:{} {} is already installed (skipping download)",
                name,
                version
            ));
        }
        Ok(ResolutionOutcome::Resolved(pkg)) => pkg,
    };

    // If resolved from Remi, it's already CCS format - install directly
    if resolved.source_type == ResolvedSourceType::Remi {
        info!("Package from Remi is already CCS format, installing directly");
        let ccs_path = resolved
            .path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid CCS path (non-UTF8)"))?;
        install_converted_ccs(ConvertedCcsInstallOptions {
            ccs_path,
            db_path: ccs_opts.db_path,
            root: ccs_opts.root,
            dry_run: ccs_opts.dry_run,
            sandbox_mode: ccs_opts.sandbox_mode,
            no_deps: ccs_opts.no_deps,
            no_scripts: ccs_opts.no_scripts,
            allow_downgrade: ccs_opts.allow_downgrade,
            dep_mode: ccs_opts.dep_mode,
            yes: ccs_opts.yes,
            dependency_passes_remaining: DEFAULT_CCS_DEPENDENCY_PASSES,
            repository_provenance: install_provenance_from_resolved(&resolved)
                .or_else(|| ccs_opts.repository_provenance.clone()),
            legacy_replay: ccs_opts.legacy_replay,
        })
        .await?;
        return Ok(None);
    }

    let path_str = resolved
        .path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid package path (non-UTF8)"))?;

    // Check if it's a CCS package by extension (from update command or local file)
    if path_str.ends_with(".ccs") {
        info!("Detected CCS package from path extension, installing directly");
        install_converted_ccs(ConvertedCcsInstallOptions {
            ccs_path: path_str,
            db_path: ccs_opts.db_path,
            root: ccs_opts.root,
            dry_run: ccs_opts.dry_run,
            sandbox_mode: ccs_opts.sandbox_mode,
            no_deps: ccs_opts.no_deps,
            no_scripts: ccs_opts.no_scripts,
            allow_downgrade: ccs_opts.allow_downgrade,
            dep_mode: ccs_opts.dep_mode,
            yes: ccs_opts.yes,
            dependency_passes_remaining: DEFAULT_CCS_DEPENDENCY_PASSES,
            repository_provenance: install_provenance_from_resolved(&resolved)
                .or_else(|| ccs_opts.repository_provenance.clone()),
            legacy_replay: ccs_opts.legacy_replay,
        })
        .await?;
        return Ok(None);
    }

    // Detect format and parse legacy packages
    let format = detect_package_format(path_str)
        .with_context(|| format!("Failed to detect package format for '{}'", path_str))?;
    info!("Detected package format: {:?}", format);

    let repository_provenance = install_provenance_from_resolved(&resolved)
        .or_else(|| ccs_opts.repository_provenance.clone());

    progress.set_phase(package, InstallPhase::Parsing);
    let pkg = parse_package(&resolved.path, format)?;

    // Convert to CCS format if requested (only for legacy packages)
    if convert_to_ccs {
        progress.set_status(&format!("Converting {} to CCS format...", pkg.name()));

        match try_convert_to_ccs(pkg.as_ref(), &resolved.path, format, db_path, !no_capture).await?
        {
            ConversionResult::Converted {
                ccs_path,
                temp_dir: _temp_dir,
            } => {
                // Install via CCS path (temp_dir kept alive until install completes)
                install_converted_ccs(ConvertedCcsInstallOptions {
                    ccs_path: &ccs_path,
                    db_path: ccs_opts.db_path,
                    root: ccs_opts.root,
                    dry_run: ccs_opts.dry_run,
                    sandbox_mode: ccs_opts.sandbox_mode,
                    no_deps: ccs_opts.no_deps,
                    no_scripts: ccs_opts.no_scripts,
                    allow_downgrade: ccs_opts.allow_downgrade,
                    dep_mode: ccs_opts.dep_mode,
                    yes: ccs_opts.yes,
                    dependency_passes_remaining: DEFAULT_CCS_DEPENDENCY_PASSES,
                    repository_provenance: install_provenance_from_resolved(&resolved)
                        .or_else(|| ccs_opts.repository_provenance.clone()),
                    legacy_replay: ccs_opts.legacy_replay,
                })
                .await?;
                return Ok(None);
            }
            ConversionResult::Skipped => {
                // Already converted - fall through to regular install path
            }
        }
    }

    Ok(Some((pkg, format, repository_provenance)))
}

fn install_provenance_from_resolved(
    resolved: &resolve::ResolvedPackage,
) -> Option<RepositoryInstallProvenance> {
    resolved
        .repository_provenance
        .as_ref()
        .map(|provenance| RepositoryInstallProvenance {
            repository_id: provenance.repository_id,
            source_distro: provenance.source_distro.clone(),
            version_scheme: provenance.version_scheme.clone(),
        })
}
/// Search canonical packages and repository packages for names similar to the
/// given query. Returns up to 5 `(name, distros)` pairs suitable for "did you
/// mean?" suggestions.
fn find_package_suggestions(
    conn: &rusqlite::Connection,
    name: &str,
) -> std::result::Result<Vec<(String, String)>, rusqlite::Error> {
    let mut hits: HashMap<String, Vec<String>> = HashMap::new();

    // 1. Search canonical_packages by substring
    {
        let mut stmt = conn.prepare(
            "SELECT cp.name, pi.distro
             FROM canonical_packages cp
             LEFT JOIN package_implementations pi ON pi.canonical_id = cp.id
             WHERE cp.name LIKE '%' || ?1 || '%'
             LIMIT 10",
        )?;
        let rows = stmt.query_map([name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (pkg_name, distro) = row?;
            let entry = hits.entry(pkg_name).or_default();
            if let Some(d) = distro
                && !entry.contains(&d)
            {
                entry.push(d);
            }
        }
    }

    // 2. Search repository_packages by prefix
    {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT rp.name, r.name
             FROM repository_packages rp
             JOIN repositories r ON r.id = rp.repository_id
             WHERE rp.name LIKE ?1 || '%'
             LIMIT 10",
        )?;
        let rows = stmt.query_map([name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (pkg_name, repo_name) = row?;
            let entry = hits.entry(pkg_name).or_default();
            if !entry.contains(&repo_name) {
                entry.push(repo_name);
            }
        }
    }

    // Remove exact match (the user already tried it)
    hits.remove(name);

    // Collect, sort by name, and take top 5
    let mut results: Vec<(String, String)> = hits
        .into_iter()
        .map(|(pkg, distros)| {
            let info = if distros.is_empty() {
                String::new()
            } else {
                distros.join(", ")
            };
            (pkg, info)
        })
        .collect();
    results.sort_by(|a, b| a.0.cmp(&b.0));
    results.truncate(5);
    Ok(results)
}

/// Print "did you mean?" suggestions when a package is not found.
///
/// Silently does nothing if the DB query fails (don't make a bad error worse).
fn print_package_suggestions(conn: &rusqlite::Connection, package_name: &str) {
    if let Ok(suggestions) = find_package_suggestions(conn, package_name)
        && !suggestions.is_empty()
    {
        eprintln!("\nDid you mean:");
        for (name, distros) in suggestions.iter().take(5) {
            if distros.is_empty() {
                eprintln!("  {name}");
            } else {
                eprintln!("  {name:<20} ({distros})");
            }
        }
        let stem = package_name.split('-').next().unwrap_or(package_name);
        eprintln!("\nUse 'conary canonical search {}' for more options.", stem);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn package_execution_path_is_prepared_before_dependency_handling() {
        let source = include_str!("mod.rs");
        let cmd_install_start = source
            .find("pub async fn cmd_install")
            .expect("cmd_install should exist");
        let helper_section_start = source[cmd_install_start..]
            .find("// ---------------------------------------------------------------------------")
            .expect("cmd_install helper boundary should exist");
        let cmd_install_source =
            &source[cmd_install_start..cmd_install_start + helper_section_start];

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
        let source = include_str!("mod.rs");
        let cmd_install_start = source
            .find("pub async fn cmd_install")
            .expect("cmd_install should exist");
        let helper_section_start = source[cmd_install_start..]
            .find("// ---------------------------------------------------------------------------")
            .expect("cmd_install helper boundary should exist");
        let cmd_install_source =
            &source[cmd_install_start..cmd_install_start + helper_section_start];

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
    #[test]
    fn force_install_over_adopted_package_is_not_silent_takeover() {
        use crate::commands::test_helpers::create_test_db;
        use conary_core::db::models::{InstallSource, Trove, TroveType};

        let (_tmp, db_path) = create_test_db();
        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = Trove::new_with_source(
            "curl".to_string(),
            "8.0.0".to_string(),
            TroveType::Package,
            InstallSource::AdoptedFull,
        );
        trove.insert(&conn).unwrap();

        let err = parse_component_and_validate(&conn, "curl", DepMode::Adopt, true).unwrap_err();
        let message = err.to_string();

        assert!(message.contains("curl"));
        assert!(message.contains("--dep-mode takeover"));
        assert!(message.contains("conary system takeover"));
    }

    #[test]
    fn explicit_takeover_over_adopted_package_is_allowed() {
        use crate::commands::test_helpers::create_test_db;
        use conary_core::db::models::{InstallSource, Trove, TroveType};

        let (_tmp, db_path) = create_test_db();
        let conn = conary_core::db::open(&db_path).unwrap();
        let mut trove = Trove::new_with_source(
            "curl".to_string(),
            "8.0.0".to_string(),
            TroveType::Package,
            InstallSource::AdoptedFull,
        );
        trove.insert(&conn).unwrap();

        let (package_name, _component_selection) =
            parse_component_and_validate(&conn, "curl", DepMode::Takeover, false).unwrap();

        assert_eq!(package_name, "curl");
    }
}
