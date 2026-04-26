// src/commands/install/mod.rs
//! Package installation commands

mod batch;
mod blocklist;
mod conversion;
mod dep_mode;
mod dep_resolution;
mod dependencies;
mod execute;
mod inner;
mod prepare;
mod resolve;
mod restore;
mod scriptlets;
mod system_pm;

pub use batch::{BatchInstaller, prepare_package_for_batch};
pub use blocklist::is_blocked as is_package_blocked;
pub use dep_mode::DepMode;

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
use dependencies::extract_runtime_deps;
// execute::get_files_to_remove is used by batch.rs via super::execute
use prepare::{check_upgrade_status, parse_package};
use resolve::{
    PolicyOptions, ResolutionOutcome, ResolvedSourceType, check_provides_dependencies,
    resolve_package_path_with_policy,
};
use scriptlets::{
    build_execution_mode, get_old_package_scriptlets, run_old_post_remove, run_old_pre_remove,
    run_post_install, run_pre_install, to_scriptlet_format,
};

use super::create_state_snapshot;
use super::progress::{InstallPhase, InstallProgress};
use super::{PackageFormatType, detect_package_format};
use anyhow::{Context, Result};
use conary_core::components::{
    ComponentClassifier, ComponentType, parse_component_spec, should_run_scriptlets,
};
use conary_core::db::models::{Changeset, ChangesetStatus, DerivedPackage};
use conary_core::db::paths::keyring_dir;
use conary_core::dependencies::LanguageDepDetector;
use conary_core::repository;
use conary_core::repository::versioning::VersionScheme;
use conary_core::resolver::MissingDependency;
use conary_core::scriptlet::{PackageFormat as ScriptletPackageFormat, SandboxMode};
use conary_core::transaction::{TransactionConfig, TransactionEngine};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tracing::{debug, info, warn};

/// Options for package installation
#[derive(Debug, Clone, Default)]
pub struct InstallOptions<'a> {
    /// Path to the package database
    pub db_path: &'a str,
    /// Filesystem root for installation
    pub root: &'a str,
    /// Specific version to install
    pub version: Option<String>,
    /// Specific repository to use
    pub repo: Option<String>,
    /// Preferred architecture to resolve/install
    pub architecture: Option<String>,
    /// Preview without installing
    pub dry_run: bool,
    /// Skip dependency resolution
    pub no_deps: bool,
    /// Skip scriptlet execution
    pub no_scripts: bool,
    /// Human-readable reason for installation
    pub selection_reason: Option<&'a str>,
    /// Sandbox mode for scriptlet execution
    pub sandbox_mode: SandboxMode,
    /// Allow installing older versions
    pub allow_downgrade: bool,
    /// Convert legacy packages to CCS format
    pub convert_to_ccs: bool,
    /// Skip the automatic state snapshot that is normally captured after
    /// a successful install.  Named `no_capture` for CLI consistency with
    /// `--no-capture`; equivalent to "skip scriptlet output capture" in
    /// some package managers but here it controls state snapshots.
    pub no_capture: bool,
    /// Force install even for adopted packages
    pub force: bool,
    /// Dependency handling mode: satisfy, adopt, takeover.
    /// `None` means the user did not explicitly set `--dep-mode`, so the
    /// policy-aware resolver uses the system model convergence intent.
    pub dep_mode: Option<DepMode>,
    /// Skip confirmation prompts
    pub yes: bool,
    /// Install from a specific distro (cross-distro canonical resolution)
    pub from_distro: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreparedSourceKind {
    Legacy { format: PackageFormatType },
    Ccs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InstallSemantics {
    source: PreparedSourceKind,
    version_scheme: VersionScheme,
    scriptlet_format: ScriptletPackageFormat,
}

impl InstallSemantics {
    fn legacy(format: PackageFormatType) -> Self {
        Self {
            source: PreparedSourceKind::Legacy { format },
            version_scheme: prepare::version_scheme_for_format(format),
            scriptlet_format: to_scriptlet_format(format),
        }
    }

    fn ccs() -> Self {
        Self {
            source: PreparedSourceKind::Ccs,
            // CCS is the native artifact shape, but the current install/rollback
            // metadata still expects a version-scheme and scriptlet-family.
            // Until CCS carries an explicit scheme, keep the existing RPM
            // fallback for mixed-version comparisons and upgrade scriptlets.
            version_scheme: VersionScheme::Rpm,
            scriptlet_format: ScriptletPackageFormat::Rpm,
        }
    }
}

/// Map a distro identifier string to its `RepositoryDependencyFlavor`.
///
/// Returns `None` for unrecognised distro names.
fn distro_name_to_flavor(
    distro: &str,
) -> Option<conary_core::repository::dependency_model::RepositoryDependencyFlavor> {
    use conary_core::repository::dependency_model::RepositoryDependencyFlavor;
    let d = distro.to_lowercase();
    if d.contains("fedora") || d.contains("rhel") || d.contains("centos") || d.contains("suse") {
        Some(RepositoryDependencyFlavor::Rpm)
    } else if d.contains("ubuntu") || d.contains("debian") || d.contains("mint") {
        Some(RepositoryDependencyFlavor::Deb)
    } else if d.contains("arch") || d.contains("manjaro") {
        Some(RepositoryDependencyFlavor::Arch)
    } else {
        None
    }
}

pub(super) fn run_triggers(
    conn: &rusqlite::Connection,
    root: &Path,
    changeset_id: i64,
    file_paths: &[String],
) {
    let trigger_executor = conary_core::trigger::TriggerExecutor::new(conn, root);

    let triggered = trigger_executor
        .record_triggers(changeset_id, file_paths)
        .unwrap_or_else(|e| {
            warn!("Failed to record triggers: {}", e);
            Vec::new()
        });

    if !triggered.is_empty() {
        info!("Recorded {} trigger(s) for execution", triggered.len());
        match trigger_executor.execute_pending(changeset_id) {
            Ok(results) => {
                if results.total() > 0 {
                    info!(
                        "Triggers: {} succeeded, {} failed, {} skipped",
                        results.succeeded, results.failed, results.skipped
                    );
                    for error in &results.errors {
                        warn!("Trigger error: {}", error);
                    }
                }
            }
            Err(e) => {
                warn!("Trigger execution failed: {}", e);
            }
        }
    }
}

pub(super) fn mark_upgraded_parent_deriveds_stale(
    conn: &rusqlite::Connection,
    parent_name: &str,
    old_version: Option<&str>,
    new_version: &str,
) {
    match DerivedPackage::mark_stale_if_parent_changed(conn, parent_name, old_version, new_version)
    {
        Ok(count) if count > 0 => {
            info!(
                "Marked {} derived package(s) stale after {} changed from {} to {}",
                count,
                parent_name,
                old_version.unwrap_or("unknown"),
                new_version
            );
        }
        Ok(_) => {}
        Err(e) => {
            warn!(
                "Failed to mark derived packages stale for upgraded parent {}: {}",
                parent_name, e
            );
        }
    }
}

/// Classify a dependency name into a human-readable type for diagnostics.
///
/// Returns a short label: "package", "capability", "OR group", "conditional",
/// "file", or "rpmlib" so the user understands what kind of requirement failed.
fn classify_dep_type(dep_name: &str) -> &'static str {
    if dep_name.starts_with("rpmlib(") {
        "rpmlib"
    } else if dep_name.starts_with('/') {
        "file"
    } else if dep_name.contains(" if ")
        || dep_name.contains(" unless ")
        || dep_name.starts_with("((")
    {
        "conditional"
    } else if dep_name.contains(" or ") || dep_name.contains('|') {
        "OR group"
    } else if dep_name.contains("(") || dep_name.contains(".so") || dep_name.contains("pkgconfig(")
    {
        "capability"
    } else {
        "package"
    }
}

/// Check if missing dependencies can be satisfied by tracked packages.
/// Prints status and returns error if any dependencies cannot be satisfied.
#[allow(dead_code)]
fn report_provides_check(
    conn: &rusqlite::Connection,
    missing: &[conary_core::resolver::MissingDependency],
    package_name: &str,
) -> Result<()> {
    let (satisfied, unsatisfied) = check_provides_dependencies(conn, missing);

    if !satisfied.is_empty() {
        println!(
            "\nDependencies satisfied by tracked packages ({}):",
            satisfied.len()
        );
        for (name, provider, version) in &satisfied {
            if let Some(v) = version {
                println!("  {} -> {} ({})", name, provider, v);
            } else {
                println!("  {} -> {}", name, provider);
            }
        }
    }

    if !unsatisfied.is_empty() {
        println!("\nMissing dependencies:");
        for dep in &unsatisfied {
            println!(
                "  {} {} (required by: {})",
                dep.name,
                dep.constraint,
                dep.required_by.join(", ")
            );
        }
        println!("\nHint: Run 'conary adopt-system' to track all installed packages");
        return Err(anyhow::anyhow!(
            "Cannot install {}: {} unresolvable dependencies",
            package_name,
            unsatisfied.len()
        ));
    }

    println!("All dependencies satisfied by tracked packages");
    Ok(())
}

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
    } = opts;

    // Hint if source policy is unconfigured (first-run guidance)
    super::hint_unconfigured_source_policy();

    // Open the database once for all pre-install checks (canonical resolution,
    // adoption check, promotion check). This connection is later promoted to `mut`
    // for the main install transaction.
    let conn = open_db(db_path)?;

    // Resolve dep_mode: if the user explicitly set --dep-mode use that,
    // otherwise derive from the system model convergence intent.
    let effective_dep_mode = dep_mode.unwrap_or_else(|| {
        if conary_core::model::model_exists(None) {
            conary_core::model::load_model(None)
                .ok()
                .map(|m| DepMode::from_convergence_intent(&m.system.convergence))
                .unwrap_or(DepMode::Satisfy)
        } else {
            DepMode::Satisfy
        }
    });

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
    };

    let Some((pkg, format)) = resolve_and_parse_package(
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
        policy: &policy,
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
}

/// Context for the dependency analysis phase.
struct DepAnalysisContext<'a> {
    conn: &'a rusqlite::Connection,
    pkg: &'a dyn conary_core::packages::PackageFormat,
    no_deps: bool,
    dry_run: bool,
    /// `None` when user did not explicitly set --dep-mode.
    dep_mode: Option<DepMode>,
    yes: bool,
    allow_downgrade: bool,
    db_path: &'a str,
    root: &'a str,
    sandbox_mode: SandboxMode,
    no_scripts: bool,
    policy: &'a conary_core::repository::resolution_policy::ResolutionPolicy,
}

/// Context for scriptlet execution phases.
struct ScriptletContext<'a> {
    root: &'a str,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
    semantics: InstallSemantics,
    old_trove: Option<&'a conary_core::db::models::Trove>,
}

/// State captured during pre-install scriptlet phase, needed for post-install.
struct PreScriptletState {
    scriptlet_format: conary_core::scriptlet::PackageFormat,
    execution_mode: conary_core::scriptlet::ExecutionMode,
    old_package_scriptlets: Vec<conary_core::db::models::ScriptletEntry>,
    run_scriptlets: bool,
}

/// Result of file extraction and component classification.
struct ExtractionResult {
    extracted_files: Vec<conary_core::packages::traits::ExtractedFile>,
    classified: HashMap<ComponentType, Vec<String>>,
    installed_component_types: Vec<ComponentType>,
    skipped_components: Vec<&'static str>,
    language_provides: Vec<conary_core::dependencies::LanguageDep>,
}

/// Context for the transaction execution phase.
struct TransactionContext<'a> {
    db_path: &'a str,
    root: &'a str,
    semantics: InstallSemantics,
    selection_reason: Option<&'a str>,
    old_trove_to_upgrade: Option<&'a conary_core::db::models::Trove>,
}

/// Result from a successful transaction execution.
struct InstallTransactionResult {
    changeset_id: i64,
}

// ---------------------------------------------------------------------------
// Extracted helper functions
// ---------------------------------------------------------------------------

/// Overlay install-specific request scope from CLI flags onto the effective policy.
///
/// The `--from-distro` flag constrains the root request to a specific distro
/// flavor; `--repo` constrains to a specific repository.  Both apply to the
/// root request only (transitive deps are governed by the mixing policy).
fn build_resolution_policy(
    mut policy: conary_core::repository::resolution_policy::ResolutionPolicy,
    from_distro: Option<&str>,
    repo: Option<&str>,
) -> conary_core::repository::resolution_policy::ResolutionPolicy {
    use conary_core::repository::resolution_policy::RequestScope;

    let scope = if let Some(target_distro) = from_distro {
        // Map distro name to the correct flavor for request-scope filtering
        let flavor = distro_name_to_flavor(target_distro);
        if let Some(f) = flavor {
            RequestScope::DistroFlavor(f)
        } else {
            // Unknown flavor -- use repo scope as a fallback
            RequestScope::Repository(target_distro.to_string())
        }
    } else if let Some(r) = repo {
        RequestScope::Repository(r.to_string())
    } else {
        RequestScope::Any
    };

    policy.request_scope = scope;
    policy
}

/// Resolve the canonical name for a package.
///
/// If `--from <distro>` was specified, resolve the canonical name to that
/// distro's package name.  Otherwise, use canonical expansion to find the best
/// implementation for the current system (canonical expansion applies only to
/// root requests, never deps).
fn resolve_canonical_name(
    conn: &rusqlite::Connection,
    package: &str,
    from_distro: Option<&str>,
    policy: &conary_core::repository::resolution_policy::ResolutionPolicy,
) -> Result<Option<String>> {
    if let Some(target_distro) = from_distro {
        if let Some(canonical) =
            conary_core::db::models::CanonicalPackage::resolve_name(conn, package)?
        {
            let impls = conary_core::db::models::PackageImplementation::find_by_canonical(
                conn,
                canonical
                    .id
                    .ok_or_else(|| anyhow::anyhow!("Canonical package has no ID"))?,
            )?;
            if let Some(imp) = impls.iter().find(|i| i.distro == target_distro) {
                info!(
                    "Resolved canonical '{}' -> '{}' for {}",
                    package, imp.distro_name, target_distro
                );
                return Ok(Some(imp.distro_name.clone()));
            }
            warn!(
                "No implementation of '{}' found for distro '{}'",
                package, target_distro
            );
        }
        Ok(None)
    } else {
        // No explicit --from-distro: use canonical resolver to expand and rank
        // implementations by pin/affinity/override.  This only applies to root
        // requests -- deps are never canonically expanded.
        use conary_core::resolver::canonical::CanonicalResolver;
        let canonical_resolver = CanonicalResolver::new(conn);
        let candidates = canonical_resolver.expand(package)?;
        if candidates.len() > 1 {
            let ranked = canonical_resolver.rank_candidates_with_policy(&candidates, policy)?;
            info!(
                "Canonical expansion for '{}': {} implementations, best = '{}' ({})",
                package,
                ranked.len(),
                ranked[0].distro_name,
                ranked[0].distro,
            );
            // Use the top-ranked implementation
            Ok(Some(ranked[0].distro_name.clone()))
        } else if candidates.len() == 1 {
            Ok(Some(candidates[0].distro_name.clone()))
        } else {
            // No canonical mapping -- use the name as-is
            Ok(None)
        }
    }
}

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

    // Check if the package is adopted from the system PM
    if let Some(existing) = conary_core::db::models::Trove::find_one_by_name(conn, &package_name)?
        && existing.install_source.is_adopted()
    {
        if !force {
            let pkg_mgr = conary_core::packages::SystemPackageManager::detect();
            return Err(anyhow::anyhow!(
                "Package '{}' is adopted from {}. Use 'conary system adopt --takeover {}' \
                 to take full ownership, or use '--force' to override.",
                package_name,
                pkg_mgr.display_name(),
                package_name
            ));
        }
        println!(
            "[INFO] Package '{}' is adopted -- proceeding with --force",
            package_name
        );
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
        })
        .await?;
        return Ok(None);
    }

    // Detect format and parse legacy packages
    let format = detect_package_format(path_str)
        .with_context(|| format!("Failed to detect package format for '{}'", path_str))?;
    info!("Detected package format: {:?}", format);

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
                })
                .await?;
                return Ok(None);
            }
            ConversionResult::Skipped => {
                // Already converted - fall through to regular install path
            }
        }
    }

    Ok(Some((pkg, format)))
}

/// Handle dependency analysis: resolve, prompt, adopt, install deps.
async fn handle_dependencies(ctx: &DepAnalysisContext<'_>) -> Result<()> {
    // Extract runtime dependencies from the package
    let runtime_deps = extract_runtime_deps(ctx.pkg);

    if ctx.no_deps && !runtime_deps.is_empty() {
        info!("Skipping dependency check (--no-deps specified)");
        println!(
            "Skipping {} dependencies (--no-deps specified)",
            runtime_deps.len()
        );
        return Ok(());
    }

    if runtime_deps.is_empty() {
        return Ok(());
    }

    let progress = InstallProgress::single("Installing");
    progress.set_phase(ctx.pkg.name(), InstallPhase::ResolvingDeps);
    info!(
        "Resolving {} dependencies with SAT solver...",
        runtime_deps.len()
    );
    println!("Checking dependencies for {}...", ctx.pkg.name());

    // Use SAT solver to find missing dependencies.
    // Build request tuples for solve_install -- this does full transitive
    // resolution and tells us which packages need to come from repos.
    let sat_requests: Vec<(String, conary_core::version::VersionConstraint)> = runtime_deps
        .iter()
        .map(|d| (d.name.clone(), d.constraint.clone()))
        .collect();

    let sat_result =
        conary_core::resolver::solve_install_with_policy(ctx.conn, &sat_requests, ctx.policy)
            .with_context(|| format!("Failed to resolve dependencies for '{}'", ctx.pkg.name()))?;

    // If SAT reports a conflict, surface it
    if let Some(ref conflict_msg) = sat_result.conflict_message {
        eprintln!("\nDependency conflicts detected:");
        eprintln!("  {}", conflict_msg);
        return Err(anyhow::anyhow!(
            "Cannot install {}: dependency conflict(s) detected",
            ctx.pkg.name(),
        ));
    }

    // Build list of missing dependencies from SAT results.
    // Packages the SAT solver says come from a Repository are the ones
    // that need to be fetched/installed.
    let missing: Vec<MissingDependency> = sat_result
        .install_order
        .iter()
        .filter(|p| p.source == conary_core::resolver::SatSource::Repository)
        .map(|p| MissingDependency {
            name: p.name.clone(),
            constraint: conary_core::version::VersionConstraint::Any,
            required_by: vec![ctx.pkg.name().to_string()],
        })
        .collect();

    // Handle missing dependencies with dep-mode awareness
    if missing.is_empty() {
        println!("All dependencies already satisfied");
        return Ok(());
    }

    info!("Found {} missing dependencies", missing.len());

    // Dep-mode-aware resolution -- use policy-aware variant so the
    // system model convergence intent provides a default when the
    // user has not explicitly set --dep-mode.
    let convergence_intent = if conary_core::model::model_exists(None) {
        conary_core::model::load_model(None)
            .ok()
            .map(|m| m.system.convergence.clone())
            .unwrap_or_default()
    } else {
        conary_core::model::ConvergenceIntent::default()
    };
    let dep_plan = dep_resolution::resolve_missing_deps_policy_aware(
        ctx.conn,
        &missing,
        ctx.dep_mode,
        &convergence_intent,
    );

    // Report blocked packages
    if !dep_plan.blocked.is_empty() {
        println!(
            "  Blocked (critical system packages): {}",
            dep_plan.blocked.join(", ")
        );
    }

    // Report satisfied packages
    for (name, reason) in &dep_plan.satisfied {
        debug!("Dependency {} satisfied: {}", name, reason);
    }
    if !dep_plan.satisfied.is_empty() {
        println!(
            "  {} dependencies satisfied by system",
            dep_plan.satisfied.len()
        );
    }

    // Confirmation prompt for non-trivial dependency installs
    let total_changes = dep_plan.to_install.len() + dep_plan.to_adopt.len();
    if total_changes > 0 && !ctx.dry_run && !ctx.yes {
        println!();
        print!("Proceed with {} dependency changes? [Y/n] ", total_changes);
        use std::io::Write;
        std::io::stdout().flush()?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();
        if input == "n" || input == "no" {
            println!("Cancelled.");
            return Ok(());
        }
    }

    handle_dep_adoptions(&dep_plan, ctx.dry_run, ctx.db_path).await;

    handle_dep_installs(ctx, &dep_plan, &progress).await?;

    // Check for unresolvable dependencies
    check_unresolvable_deps(ctx, &dep_plan, &convergence_intent)?;

    Ok(())
}

/// Handle auto-adoption of dependencies (adopt mode).
async fn handle_dep_adoptions(
    dep_plan: &dep_resolution::DepResolutionPlan,
    dry_run: bool,
    db_path: &str,
) {
    if dep_plan.to_adopt.is_empty() {
        return;
    }

    if dry_run {
        println!(
            "  Would auto-adopt {} system dependencies:",
            dep_plan.to_adopt.len()
        );
        for name in &dep_plan.to_adopt {
            println!("    {}", name);
        }
    } else {
        println!(
            "  Auto-adopting {} system dependencies:",
            dep_plan.to_adopt.len()
        );
        for name in &dep_plan.to_adopt {
            println!("    {}", name);
        }
        // Use the adopt subsystem
        if let Err(e) = crate::commands::adopt::cmd_adopt(&dep_plan.to_adopt, db_path, false).await
        {
            warn!("Failed to auto-adopt dependencies: {}", e);
            // Non-fatal -- deps are still on the system
        }
    }
}

/// Handle packages that need to be installed from repos.
async fn handle_dep_installs(
    ctx: &DepAnalysisContext<'_>,
    dep_plan: &dep_resolution::DepResolutionPlan,
    progress: &InstallProgress,
) -> Result<()> {
    if dep_plan.to_install.is_empty() {
        return Ok(());
    }

    // Build full request tuples preserving version constraints from the
    // resolution plan.  The old name-only path dropped constraints,
    // causing the SAT solver to pick arbitrary versions.
    let dep_requests: Vec<(String, conary_core::version::VersionConstraint)> = dep_plan
        .to_install
        .iter()
        .map(|d| {
            let constraint = d
                .version
                .as_ref()
                .and_then(|v| conary_core::version::VersionConstraint::parse(v).ok())
                .unwrap_or(conary_core::version::VersionConstraint::Any);
            (d.name.clone(), constraint)
        })
        .collect();

    if ctx.dry_run {
        println!(
            "  Would install {} dependencies from Remi:",
            dep_requests.len()
        );
        // Validate that deps are actually resolvable even in dry-run
        match repository::resolve_dependencies_transitive_requests(ctx.conn, &dep_requests, 10) {
            Ok(to_download) => {
                for (name, _) in &dep_requests {
                    println!("    {}", name);
                }
                if to_download.is_empty() {
                    println!("  (all dependencies already available locally)");
                }
            }
            Err(e) => {
                for (name, _) in &dep_requests {
                    println!("    {} (resolution pending)", name);
                }
                println!("  [WARN] Dependency resolution check failed: {}", e);
            }
        }
        return Ok(());
    }

    println!("  Installing {} dependencies:", dep_requests.len());
    for (name, _) in &dep_requests {
        println!("    {}", name);
    }

    // Use transitive resolution with full version constraints
    match repository::resolve_dependencies_transitive_requests(ctx.conn, &dep_requests, 10) {
        Ok(to_download) => {
            if !to_download.is_empty() {
                progress.set_phase(ctx.pkg.name(), InstallPhase::InstallingDeps);
                let temp_dir = TempDir::new()?;
                let keyring_dir = keyring_dir(ctx.db_path);
                let downloaded = repository::download_dependencies(
                    &to_download,
                    temp_dir.path(),
                    Some(&keyring_dir),
                )
                .await?;

                let parent_name = ctx.pkg.name().to_string();
                let mut prepared_packages = Vec::with_capacity(downloaded.len());

                for (dep_name, dep_path) in &downloaded {
                    progress.set_status(&format!("Preparing dependency: {}", dep_name));
                    let reason = format!("Required by {}", parent_name);
                    match prepare_package_for_batch(
                        dep_path,
                        ctx.db_path,
                        &reason,
                        ctx.allow_downgrade,
                    ) {
                        Ok(prepared) => {
                            prepared_packages.push(prepared);
                        }
                        Err(e) => {
                            if e.to_string().contains("already installed") {
                                info!("Dependency {} already installed, skipping", dep_name);
                                continue;
                            }
                            return Err(anyhow::anyhow!(
                                "Failed to prepare dependency {}: {}",
                                dep_name,
                                e
                            ));
                        }
                    }
                }

                if !prepared_packages.is_empty() {
                    let installer = BatchInstaller::new(
                        ctx.db_path,
                        ctx.root,
                        ctx.sandbox_mode,
                        ctx.no_scripts,
                    );
                    installer.install_batch(prepared_packages)?;
                    println!("  [OK] Installed {} dependencies", downloaded.len());
                }
            }
        }
        Err(e) => {
            return Err(anyhow::anyhow!(
                "Failed to resolve dependencies from repositories: {}",
                e
            ));
        }
    }

    Ok(())
}

/// Check for unresolvable dependencies and report them.
fn check_unresolvable_deps(
    ctx: &DepAnalysisContext<'_>,
    dep_plan: &dep_resolution::DepResolutionPlan,
    convergence_intent: &conary_core::model::ConvergenceIntent,
) -> Result<()> {
    if dep_plan.unresolvable.is_empty() {
        return Ok(());
    }

    // Last resort: check provides table
    let (satisfied, still_missing) = check_provides_dependencies(ctx.conn, &dep_plan.unresolvable);
    if !satisfied.is_empty() {
        for (name, provider, _) in &satisfied {
            println!("  {} provided by {}", name, provider);
        }
    }
    if !still_missing.is_empty() {
        eprintln!("\nUnresolvable dependencies:");
        for dep in &still_missing {
            eprintln!(
                "  {} {} (type: {}, required by: {})",
                dep.name,
                dep.constraint,
                classify_dep_type(&dep.name),
                dep.required_by.join(", ")
            );
        }
        eprintln!(
            "\nResolution context: dep-mode={}, convergence={}",
            ctx.dep_mode.map_or("auto".to_string(), |m| m.to_string()),
            convergence_intent.display_name(),
        );
        // List repos that were searched
        if let Ok(repos) = conary_core::db::models::Repository::list_all(ctx.conn) {
            let repo_names: Vec<&str> = repos.iter().map(|r| r.name.as_str()).collect();
            if !repo_names.is_empty() {
                eprintln!("Repositories searched: {}", repo_names.join(", "));
            } else {
                eprintln!("No repositories configured (run 'conary repo add' first)");
            }
        }
        return Err(anyhow::anyhow!(
            "Cannot install {}: {} unresolvable dependencies\n\
             Hint: Use --dep-mode adopt to auto-adopt system packages\n\
             Hint: Use --dep-mode takeover to install CCS versions from Remi\n\
             Hint: Use --no-deps to skip dependency checking",
            ctx.pkg.name(),
            still_missing.len()
        ));
    }

    Ok(())
}

/// Display a dry-run summary showing what would be installed.
fn show_dry_run_summary(
    pkg: &dyn conary_core::packages::PackageFormat,
    component_selection: &ComponentSelection,
) {
    // For dry run, classify files to show component info
    let dry_run_paths: Vec<String> = pkg.files().iter().map(|f| f.path.clone()).collect();
    let dry_run_classified = ComponentClassifier::classify_all(&dry_run_paths);
    let dry_run_available: Vec<_> = dry_run_classified.keys().collect();
    let dry_run_selected: Vec<_> = dry_run_available
        .iter()
        .filter(|c| component_selection.should_install(***c))
        .collect();
    let dry_run_skipped: Vec<_> = dry_run_available
        .iter()
        .filter(|c| !component_selection.should_install(***c))
        .collect();

    let selected_file_count: usize = dry_run_classified
        .iter()
        .filter(|(c, _)| component_selection.should_install(**c))
        .map(|(_, files)| files.len())
        .sum();

    println!(
        "\nWould install package: {} version {}",
        pkg.name(),
        pkg.version()
    );
    println!("  Architecture: {}", pkg.architecture().unwrap_or("none"));
    println!(
        "  Components to install: {} ({} files)",
        dry_run_selected
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        selected_file_count
    );
    if !dry_run_skipped.is_empty() {
        println!(
            "  Components skipped: {} (use {}:all to include)",
            dry_run_skipped
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            pkg.name()
        );
    }
    println!("  Dependencies: {}", pkg.dependencies().len());
    println!("\nDry run complete. No changes made.");
}

/// Extract files from the package and classify them into components.
fn extract_and_classify_files(
    pkg: &dyn conary_core::packages::PackageFormat,
    component_selection: &ComponentSelection,
    progress: &InstallProgress,
) -> Result<ExtractionResult> {
    // Extract and install
    progress.set_phase(pkg.name(), InstallPhase::Extracting);
    info!("Extracting file contents from package...");
    let extracted_files = pkg
        .extract_file_contents()
        .with_context(|| format!("Failed to extract files from package '{}'", pkg.name()))?;
    info!("Extracted {} files", extracted_files.len());

    // Classify files into components
    let file_paths: Vec<String> = extracted_files.iter().map(|f| f.path.clone()).collect();
    let all_classified = ComponentClassifier::classify_all(&file_paths);

    // Show what components are available in the package
    let available_components: Vec<ComponentType> = all_classified.keys().copied().collect();
    info!(
        "Package contains {} component types: {:?}",
        available_components.len(),
        available_components
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>()
    );

    // Filter to only selected components
    let classified: HashMap<ComponentType, Vec<String>> = all_classified
        .into_iter()
        .filter(|(comp_type, _)| component_selection.should_install(*comp_type))
        .collect();

    // Build set of paths for selected components
    let selected_paths: std::collections::HashSet<&str> =
        classified.values().flatten().map(|s| s.as_str()).collect();

    // Filter extracted files to only include selected components
    let extracted_files: Vec<_> = extracted_files
        .into_iter()
        .filter(|f| selected_paths.contains(f.path.as_str()))
        .collect();

    let installed_component_types: Vec<ComponentType> = classified.keys().copied().collect();

    // Show what we're actually installing
    let skipped_components: Vec<&str> = available_components
        .iter()
        .filter(|c| !component_selection.should_install(**c))
        .map(|c| c.as_str())
        .collect();

    if !skipped_components.is_empty() {
        info!(
            "Skipping non-default components: {:?} (use package:all to install everything)",
            skipped_components
        );
    }

    info!(
        "Installing {} files from {} component(s): {:?}",
        extracted_files.len(),
        classified.len(),
        installed_component_types
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>()
    );

    // Detect language-specific provides from installed files
    // Do this before the transaction so we can display the count in the summary
    let installed_paths: Vec<String> = extracted_files.iter().map(|f| f.path.clone()).collect();
    let language_provides = LanguageDepDetector::detect_all_provides(&installed_paths);
    if !language_provides.is_empty() {
        info!(
            "Detected {} language-specific provides: {:?}",
            language_provides.len(),
            language_provides
                .iter()
                .take(5)
                .map(|d| d.to_dep_string())
                .collect::<Vec<_>>()
        );
    }

    Ok(ExtractionResult {
        extracted_files,
        classified,
        installed_component_types,
        skipped_components,
        language_provides,
    })
}

/// Run pre-install scriptlets and query old package scriptlets for upgrades.
fn run_pre_install_phase(
    conn: &rusqlite::Connection,
    pkg: &dyn conary_core::packages::PackageFormat,
    installed_component_types: &[ComponentType],
    ctx: &ScriptletContext<'_>,
    progress: &InstallProgress,
) -> Result<PreScriptletState> {
    // Determine package format and execution mode for scriptlet execution
    let scriptlet_format = ctx.semantics.scriptlet_format;
    let execution_mode = build_execution_mode(ctx.old_trove.map(|t| t.version.as_str()));

    // Execute pre-install scriptlet (before any changes)
    // Scriptlets only run when :runtime or :lib is being installed
    let scriptlets = pkg.scriptlets();
    let run_scriptlets = should_run_scriptlets(installed_component_types);
    if !ctx.no_scripts && !scriptlets.is_empty() && run_scriptlets {
        progress.set_phase(pkg.name(), InstallPhase::PreScript);
        run_pre_install(
            Path::new(ctx.root),
            pkg.name(),
            pkg.version(),
            scriptlets,
            scriptlet_format,
            &execution_mode,
            ctx.sandbox_mode,
        )?;
    } else if !ctx.no_scripts && !scriptlets.is_empty() && !run_scriptlets {
        info!(
            "Skipping scriptlets: no :runtime or :lib component being installed (components: {:?})",
            installed_component_types
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
        );
    }

    // Query old package's scriptlets BEFORE we delete it from DB
    // We need these for running pre-remove and post-remove during upgrade
    let old_trove_id = ctx.old_trove.and_then(|t| t.id);
    let old_package_scriptlets = get_old_package_scriptlets(conn, old_trove_id)?;

    // For RPM/DEB upgrades: run old package's pre-remove scriptlet
    if !ctx.no_scripts
        && let Some(old_trove) = ctx.old_trove
    {
        run_old_pre_remove(
            Path::new(ctx.root),
            &old_trove.name,
            &old_trove.version,
            pkg.version(),
            &old_package_scriptlets,
            scriptlet_format,
            ctx.sandbox_mode,
        )?;
    }

    Ok(PreScriptletState {
        scriptlet_format,
        execution_mode,
        old_package_scriptlets,
        run_scriptlets,
    })
}

/// Execute the main install transaction: filesystem changes + DB commit.
fn execute_install_transaction(
    conn: &mut rusqlite::Connection,
    pkg: &dyn conary_core::packages::PackageFormat,
    extraction: &ExtractionResult,
    ctx: &TransactionContext<'_>,
    progress: &InstallProgress,
) -> Result<InstallTransactionResult> {
    let db_path_buf = PathBuf::from(ctx.db_path);
    let tx_config = TransactionConfig::from_paths(PathBuf::from(ctx.root), db_path_buf);
    let mut engine =
        TransactionEngine::new(tx_config).context("Failed to create transaction engine")?;

    engine
        .recover(conn)
        .context("Failed to recover incomplete transactions")?;

    let tx_description = if let Some(old_trove) = ctx.old_trove_to_upgrade {
        format!(
            "Upgrade {} from {} to {}",
            pkg.name(),
            old_trove.version,
            pkg.version()
        )
    } else {
        format!("Install {}-{}", pkg.name(), pkg.version())
    };
    engine.begin().context("Failed to begin transaction")?;

    // Capture /etc snapshot BEFORE the DB transaction so the three-way merge
    // can distinguish pre- from post-install state.
    let prev_etc = crate::commands::composefs_ops::collect_etc_files(conn)?;

    let mut changeset = Changeset::new(tx_description.clone());
    let tx = conn.unchecked_transaction()?;
    let changeset_id = changeset.insert(&tx)?;

    let inner_result = match inner::install_inner(
        &tx,
        &mut engine,
        changeset_id,
        pkg,
        extraction,
        ctx,
        progress,
    ) {
        Ok(result) => result,
        Err(e) => {
            engine.release_lock();
            return Err(e);
        }
    };

    tx.commit()?;
    info!(
        "DB commit successful: changeset={}, trove={}",
        changeset_id, inner_result.trove_id
    );

    let post_commit_result = (|| -> Result<()> {
        crate::commands::composefs_ops::rebuild_and_mount(
            conn,
            ctx.db_path,
            &tx_description,
            Some(prev_etc),
        )?;

        changeset.update_status(conn, ChangesetStatus::Applied)?;
        Ok(())
    })();

    engine.release_lock();
    post_commit_result?;

    Ok(InstallTransactionResult { changeset_id })
}

/// Run post-install scriptlets, triggers, and print the final summary.
fn finalize_install_without_snapshot(
    conn: &rusqlite::Connection,
    pkg: &dyn conary_core::packages::PackageFormat,
    extraction: &ExtractionResult,
    scriptlet_ctx: &ScriptletContext<'_>,
    pre_state: &PreScriptletState,
    tx_result: &InstallTransactionResult,
    progress: &InstallProgress,
) -> Result<()> {
    // For RPM/DEB upgrades: run old package's post-remove scriptlet
    if !scriptlet_ctx.no_scripts
        && let Some(old_trove) = scriptlet_ctx.old_trove
    {
        run_old_post_remove(
            Path::new(scriptlet_ctx.root),
            &old_trove.name,
            &old_trove.version,
            pkg.version(),
            &pre_state.old_package_scriptlets,
            pre_state.scriptlet_format,
            scriptlet_ctx.sandbox_mode,
        );
    }

    // Execute post-install scriptlet (after files are deployed)
    let scriptlets = pkg.scriptlets();
    if !scriptlet_ctx.no_scripts && !scriptlets.is_empty() && pre_state.run_scriptlets {
        progress.set_phase(pkg.name(), InstallPhase::PostScript);
        run_post_install(
            Path::new(scriptlet_ctx.root),
            pkg.name(),
            pkg.version(),
            scriptlets,
            pre_state.scriptlet_format,
            &pre_state.execution_mode,
            scriptlet_ctx.sandbox_mode,
        );
    }

    progress.set_phase(pkg.name(), InstallPhase::Triggers);
    let file_paths: Vec<String> = extraction
        .extracted_files
        .iter()
        .map(|f| f.path.clone())
        .collect();
    run_triggers(
        conn,
        Path::new(scriptlet_ctx.root),
        tx_result.changeset_id,
        &file_paths,
    );

    progress.finish(&format!("Installed {} {}", pkg.name(), pkg.version()));

    // Show what components were available vs installed
    let skipped_info = if !extraction.skipped_components.is_empty() {
        format!(" (skipped: {})", extraction.skipped_components.join(", "))
    } else {
        String::new()
    };

    println!(
        "Installed package: {} version {}",
        pkg.name(),
        pkg.version()
    );
    println!("  Architecture: {}", pkg.architecture().unwrap_or("none"));
    println!("  Files installed: {}", extraction.extracted_files.len());
    println!(
        "  Components: {}{}",
        extraction
            .installed_component_types
            .iter()
            .map(|c| format!(":{}", c.as_str()))
            .collect::<Vec<_>>()
            .join(", "),
        skipped_info
    );
    println!("  Dependencies: {}", pkg.dependencies().len());
    if !extraction.language_provides.is_empty() {
        println!(
            "  Provides: {} (language-specific capabilities)",
            extraction.language_provides.len()
        );
    }

    Ok(())
}

fn finalize_install(
    conn: &rusqlite::Connection,
    pkg: &dyn conary_core::packages::PackageFormat,
    extraction: &ExtractionResult,
    scriptlet_ctx: &ScriptletContext<'_>,
    pre_state: &PreScriptletState,
    tx_result: &InstallTransactionResult,
    progress: &InstallProgress,
) -> Result<()> {
    finalize_install_without_snapshot(
        conn,
        pkg,
        extraction,
        scriptlet_ctx,
        pre_state,
        tx_result,
        progress,
    )?;
    create_state_snapshot(
        conn,
        tx_result.changeset_id,
        &format!("Install {}", pkg.name()),
    )?;
    Ok(())
}

fn scheme_to_string(scheme: VersionScheme) -> String {
    match scheme {
        VersionScheme::Rpm => "rpm".to_string(),
        VersionScheme::Debian => "debian".to_string(),
        VersionScheme::Arch => "arch".to_string(),
    }
}

/// Search canonical packages and repository packages for names similar to the
/// given query. Returns up to 5 `(name, distros)` pairs suitable for "did you
/// mean?" suggestions.
fn find_package_suggestions(
    conn: &rusqlite::Connection,
    name: &str,
) -> std::result::Result<Vec<(String, String)>, rusqlite::Error> {
    use std::collections::HashMap;

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
    fn classify_dep_type_packages() {
        assert_eq!(classify_dep_type("bash"), "package");
        assert_eq!(classify_dep_type("libcurl-devel"), "package");
    }

    #[test]
    fn classify_dep_type_capabilities() {
        assert_eq!(classify_dep_type("libcurl.so.4()(64bit)"), "capability");
        assert_eq!(classify_dep_type("pkgconfig(libcurl)"), "capability");
    }

    #[test]
    fn classify_dep_type_files() {
        assert_eq!(classify_dep_type("/usr/bin/python3"), "file");
    }

    #[test]
    fn classify_dep_type_rpmlib() {
        assert_eq!(classify_dep_type("rpmlib(CompressedFileNames)"), "rpmlib");
    }

    #[test]
    fn classify_dep_type_conditional() {
        assert_eq!(
            classify_dep_type("(systemd if systemd-resolved)"),
            "conditional"
        );
        assert_eq!(
            classify_dep_type("((kernel-core if kernel))"),
            "conditional"
        );
    }

    #[test]
    fn classify_dep_type_or_group() {
        assert_eq!(
            classify_dep_type("default-mta | mail-transport-agent"),
            "OR group"
        );
    }

    #[test]
    fn distro_name_to_flavor_known() {
        use conary_core::repository::dependency_model::RepositoryDependencyFlavor;
        assert_eq!(
            distro_name_to_flavor("fedora43"),
            Some(RepositoryDependencyFlavor::Rpm)
        );
        assert_eq!(
            distro_name_to_flavor("ubuntu-noble"),
            Some(RepositoryDependencyFlavor::Deb)
        );
        assert_eq!(
            distro_name_to_flavor("arch"),
            Some(RepositoryDependencyFlavor::Arch)
        );
    }

    #[test]
    fn distro_name_to_flavor_unknown() {
        assert_eq!(distro_name_to_flavor("nixos"), None);
    }
}
