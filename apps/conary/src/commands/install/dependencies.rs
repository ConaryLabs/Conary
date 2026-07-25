// src/commands/install/dependencies.rs

//! Dependency resolution for package installation
//!
//! Runtime requirements are parsed from package metadata, solved against
//! Conary's persisted provider graph, and completed from configured
//! repositories.

use super::dep_resolution;
use super::{
    BatchInstaller, InstallPhase, InstallProgress, prepare_package_for_batch,
    repository_install_provenance_from_package,
};
use anyhow::{Context, Result};
use conary_core::db::paths::keyring_dir;
use conary_core::packages::PackageFormat;
use conary_core::repository;
use conary_core::repository::dependency_model::RepositoryRequirementKind;
use conary_core::resolver::{MissingDependency, SatResolution, SatSource};
use conary_core::scriptlet::SandboxMode;
use conary_core::version::VersionConstraint;
use std::collections::HashMap;
use tempfile::TempDir;
use tracing::info;

/// Context for the dependency analysis phase.
pub(super) struct DepAnalysisContext<'a> {
    pub(super) conn: &'a rusqlite::Connection,
    pub(super) pkg: &'a dyn PackageFormat,
    pub(super) no_deps: bool,
    pub(super) dry_run: bool,
    pub(super) yes: bool,
    pub(super) allow_downgrade: bool,
    pub(super) db_path: &'a str,
    pub(super) sandbox_mode: SandboxMode,
    pub(super) policy: &'a conary_core::repository::resolution_policy::ResolutionPolicy,
    pub(super) primary_flavor:
        Option<conary_core::repository::dependency_model::RepositoryDependencyFlavor>,
}

/// Handle dependency analysis: resolve, prompt, and install repository deps.
pub(super) async fn handle_dependencies(ctx: &DepAnalysisContext<'_>) -> Result<()> {
    let runtime_requirement_count = ctx
        .pkg
        .requirements()
        .iter()
        .filter(|requirement| {
            matches!(
                requirement.kind,
                RepositoryRequirementKind::Depends | RepositoryRequirementKind::PreDepends
            )
        })
        .count();

    if ctx.no_deps && runtime_requirement_count != 0 {
        info!("Skipping dependency check (--no-deps specified)");
        println!(
            "Skipping {} dependencies (--no-deps specified)",
            runtime_requirement_count
        );
        return Ok(());
    }

    if runtime_requirement_count == 0 {
        return Ok(());
    }

    let progress = InstallProgress::single("Installing");
    progress.set_phase(ctx.pkg.name(), InstallPhase::ResolvingDeps);
    info!(
        "Resolving {} dependencies with SAT solver...",
        runtime_requirement_count
    );
    println!("Checking dependencies for {}...", ctx.pkg.name());

    let sat_result = conary_core::resolver::solve_requirement_groups_with_policy(
        ctx.conn,
        ctx.pkg.requirements(),
        ctx.pkg.version_scheme(),
        ctx.policy,
    )
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

    let missing = missing_repository_deps_from_sat_result(&sat_result, ctx.pkg.name())?;

    if missing.is_empty() {
        println!("All dependencies already satisfied");
        return Ok(());
    }

    info!("Found {} missing dependencies", missing.len());

    let selection_options = repository::SelectionOptions {
        policy: Some(ctx.policy.clone()),
        is_root: false,
        primary_flavor: ctx.primary_flavor,
        ..Default::default()
    };
    let dep_plan =
        dep_resolution::plan_repository_dependencies(ctx.conn, &missing, &selection_options)?;

    // Confirmation prompt for non-trivial dependency installs
    let total_changes = dep_plan.to_install.len();
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

    handle_dep_installs(ctx, &dep_plan, &selection_options, &progress).await?;

    // Check for unresolvable dependencies
    check_unresolvable_deps(ctx, &dep_plan)?;

    Ok(())
}

pub(super) fn missing_repository_deps_from_sat_result(
    sat_result: &SatResolution,
    required_by: &str,
) -> Result<Vec<MissingDependency>> {
    sat_result
        .install_order
        .iter()
        .filter(|p| p.source == SatSource::Repository)
        .map(|package| {
            let constraint = VersionConstraint::parse(&format!("= {}", package.version))
                .with_context(|| {
                    format!(
                        "SAT selected invalid repository version '{}' for package '{}'",
                        package.version, package.name
                    )
                })?;
            Ok(MissingDependency {
                name: package.name.clone(),
                constraint,
                required_by: vec![required_by.to_string()],
            })
        })
        .collect()
}

/// Handle packages that need to be installed from repos.
async fn handle_dep_installs(
    ctx: &DepAnalysisContext<'_>,
    dep_plan: &dep_resolution::DepResolutionPlan,
    selection_options: &repository::SelectionOptions,
    progress: &InstallProgress,
) -> Result<()> {
    if dep_plan.to_install.is_empty() {
        return Ok(());
    }

    let dep_requests: Vec<(String, conary_core::version::VersionConstraint)> = dep_plan
        .to_install
        .iter()
        .map(|dependency| (dependency.name.clone(), dependency.constraint.clone()))
        .collect();

    if ctx.dry_run {
        println!(
            "  Would install {} dependencies from Remi:",
            dep_requests.len()
        );
        // Validate that deps are actually resolvable even in dry-run
        let to_download = repository::resolve_dependencies_transitive_requests(
            ctx.conn,
            &dep_requests,
            10,
            selection_options,
        )
        .context("dependency dry-run could not produce a complete repository plan")?;
        for (name, _) in &dep_requests {
            println!("    {}", name);
        }
        if to_download.is_empty() {
            println!("  (all dependencies already available locally)");
        }
        return Ok(());
    }

    println!("  Installing {} dependencies:", dep_requests.len());
    for (name, _) in &dep_requests {
        println!("    {}", name);
    }

    // Use transitive resolution with full version constraints
    match repository::resolve_dependencies_transitive_requests(
        ctx.conn,
        &dep_requests,
        10,
        selection_options,
    ) {
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
                let downloaded =
                    repository::download_dependencies(&to_download, temp_dir.path(), &keyring_dir)
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
                        Ok(Some(prepared)) => {
                            let mut prepared = prepared;
                            prepared.repository_provenance =
                                provenance_by_dep.get(dep_name).cloned();
                            prepared_packages.push(prepared);
                        }
                        Ok(None) => {
                            info!("Dependency {} already installed, skipping", dep_name);
                        }
                        Err(e) => {
                            return Err(anyhow::anyhow!(
                                "Failed to prepare dependency {}: {}",
                                dep_name,
                                e
                            ));
                        }
                    }
                }

                if !prepared_packages.is_empty() {
                    let installer = BatchInstaller::new(ctx.db_path, ctx.sandbox_mode);
                    installer.install_batch(prepared_packages)?;
                    crate::ui::row(
                        crate::ui::Status::Ok,
                        &[&format!("Installed {} dependencies", downloaded.len())],
                    );
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
) -> Result<()> {
    if dep_plan.unresolvable.is_empty() {
        return Ok(());
    }

    eprintln!("\nUnresolvable dependencies:");
    for dep in &dep_plan.unresolvable {
        eprintln!(
            "  {} {} (required by: {})",
            dep.name,
            dep.constraint,
            dep.required_by.join(", ")
        );
    }
    if let Ok(repos) = conary_core::db::models::Repository::list_all(ctx.conn) {
        let repo_names: Vec<&str> = repos.iter().map(|repo| repo.name.as_str()).collect();
        if repo_names.is_empty() {
            eprintln!("No repositories configured (run 'conary repo add' first)");
        } else {
            eprintln!("Repositories searched: {}", repo_names.join(", "));
        }
    }

    Err(anyhow::anyhow!(
        "Cannot install {}: {} requirements have no installed or repository provider",
        ctx.pkg.name(),
        dep_plan.unresolvable.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            remove_order: Vec::new(),
            conflict_message: None,
        };

        let missing = missing_repository_deps_from_sat_result(&sat_result, "kernel").unwrap();

        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].name, "kernel-core");
        assert_eq!(missing[0].constraint.to_string(), "= 6.19.10-300.fc44");
        assert_eq!(missing[0].required_by, vec!["kernel"]);
    }
}
