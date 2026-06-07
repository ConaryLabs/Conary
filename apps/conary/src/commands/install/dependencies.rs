// src/commands/install/dependencies.rs

//! Dependency resolution for package installation
//!
//! Currently this module only exposes helpers for extracting runtime
//! dependencies from package metadata.

use super::dep_resolution;
use super::resolve::check_provides_dependencies;
use super::{
    BatchInstaller, DepMode, InstallPhase, InstallProgress, LegacyReplayOptions,
    PackageExecutionPath, prepare_package_for_batch, repository_install_provenance_from_package,
};
use anyhow::{Context, Result};
use conary_core::db::paths::keyring_dir;
use conary_core::packages::PackageFormat;
use conary_core::packages::traits::DependencyType;
use conary_core::repository;
use conary_core::resolver::{MissingDependency, SatResolution, SatSource};
use conary_core::scriptlet::SandboxMode;
use conary_core::version::VersionConstraint;
use std::collections::HashMap;
use tempfile::TempDir;
use tracing::{debug, info, warn};

/// A runtime dependency extracted from a package.
#[derive(Debug, Clone)]
pub struct RuntimeDep {
    /// Dependency name (package or capability).
    pub name: String,
    /// Version constraint (Any if unspecified).
    pub constraint: VersionConstraint,
}

/// Extract runtime dependencies from a package as `(name, constraint)` pairs.
#[must_use]
pub fn extract_runtime_deps(pkg: &dyn PackageFormat) -> Vec<RuntimeDep> {
    pkg.dependencies()
        .iter()
        .filter(|d| d.dep_type == DependencyType::Runtime)
        .map(|d| {
            let constraint = d
                .version
                .as_ref()
                .and_then(|v| VersionConstraint::parse(v).ok())
                .unwrap_or(VersionConstraint::Any);
            RuntimeDep {
                name: d.name.clone(),
                constraint,
            }
        })
        .collect()
}

pub(crate) fn resolve_default_dep_mode_from_model() -> DepMode {
    let convergence = if conary_core::model::model_exists(None) {
        conary_core::model::load_model(None)
            .map(|model| model.system.convergence)
            .unwrap_or_default()
    } else {
        conary_core::model::ConvergenceIntent::default()
    };
    DepMode::from_convergence_intent(&convergence)
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
    missing: &[MissingDependency],
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
        println!(
            "\nHint: Run 'conary system adopt --system' to track all installed native packages"
        );
        return Err(anyhow::anyhow!(
            "Cannot install {}: {} unresolvable dependencies",
            package_name,
            unsatisfied.len()
        ));
    }

    println!("All dependencies satisfied by tracked packages");
    Ok(())
}

/// Context for the dependency analysis phase.
pub(super) struct DepAnalysisContext<'a> {
    pub(super) conn: &'a rusqlite::Connection,
    pub(super) pkg: &'a dyn PackageFormat,
    pub(super) no_deps: bool,
    pub(super) dry_run: bool,
    /// `None` when user did not explicitly set --dep-mode.
    pub(super) dep_mode: Option<DepMode>,
    pub(super) yes: bool,
    pub(super) allow_downgrade: bool,
    pub(super) db_path: &'a str,
    pub(super) root: &'a str,
    pub(super) sandbox_mode: SandboxMode,
    pub(super) no_scripts: bool,
    pub(super) legacy_replay: LegacyReplayOptions,
    pub(super) policy: &'a conary_core::repository::resolution_policy::ResolutionPolicy,
    pub(super) execution_path: PackageExecutionPath,
}

/// Handle dependency analysis: resolve, prompt, adopt, install deps.
pub(super) async fn handle_dependencies(ctx: &DepAnalysisContext<'_>) -> Result<()> {
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

    let missing = missing_repository_deps_from_sat_result(&sat_result, ctx.pkg.name());

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

fn missing_repository_deps_from_sat_result(
    sat_result: &SatResolution,
    required_by: &str,
) -> Vec<MissingDependency> {
    sat_result
        .install_order
        .iter()
        .filter(|p| p.source == SatSource::Repository)
        .map(|p| MissingDependency {
            name: p.name.clone(),
            constraint: conary_core::version::VersionConstraint::parse(&format!("= {}", p.version))
                .unwrap_or(conary_core::version::VersionConstraint::Any),
            required_by: vec![required_by.to_string()],
        })
        .collect()
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
                let mut provenance_by_dep = HashMap::new();
                for (dep_name, pkg_with_repo) in &to_download {
                    provenance_by_dep.insert(
                        dep_name.clone(),
                        repository_install_provenance_from_package(
                            &pkg_with_repo.package,
                            &pkg_with_repo.repository,
                        )?,
                    );
                }
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
                            let mut prepared = prepared;
                            prepared.repository_provenance =
                                provenance_by_dep.get(dep_name).cloned();
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
                        ctx.legacy_replay,
                    )
                    .with_preflighted_execution_path(ctx.execution_path);
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
    fn missing_model_uses_preview_convergence_dep_mode() {
        assert_eq!(resolve_default_dep_mode_from_model(), DepMode::Adopt);
    }

    #[test]
    fn missing_repository_deps_preserve_sat_selected_version() {
        let sat_result = conary_core::resolver::SatResolution {
            install_order: vec![
                conary_core::resolver::SatPackage {
                    name: "kernel-core".to_string(),
                    version: "6.19.10-300.fc44".to_string(),
                    source: conary_core::resolver::SatSource::Repository,
                },
                conary_core::resolver::SatPackage {
                    name: "glibc".to_string(),
                    version: "2.43-2.fc44".to_string(),
                    source: conary_core::resolver::SatSource::Installed,
                },
            ],
            conflict_message: None,
        };

        let missing = missing_repository_deps_from_sat_result(&sat_result, "kernel");

        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].name, "kernel-core");
        assert_eq!(missing[0].constraint.to_string(), "= 6.19.10-300.fc44");
        assert_eq!(missing[0].required_by, vec!["kernel"]);
    }
}
