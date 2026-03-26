// src/commands/install/dependencies.rs

//! Dependency resolution for package installation
//!
//! Handles checking and resolving package dependencies, including
//! downloading missing dependencies from repositories.

#![allow(dead_code)]

use super::resolve::check_provides_dependencies;
use crate::commands::progress::{InstallPhase, InstallProgress};
use crate::commands::{SandboxMode, cmd_install};
use anyhow::Result;
use conary_core::db::paths::keyring_dir;
use conary_core::packages::PackageFormat;
use conary_core::packages::traits::DependencyType;
use conary_core::repository;
use conary_core::resolver::ResolutionPlan;
use conary_core::version::VersionConstraint;
use rusqlite::Connection;
use tempfile::TempDir;
use tracing::{debug, info};

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

/// Check for dependency conflicts and handle missing dependencies
///
/// Returns Ok(()) if all dependencies can be satisfied, or an error with details.
#[allow(clippy::too_many_arguments)]
pub async fn handle_missing_dependencies(
    conn: &mut Connection,
    pkg: &dyn PackageFormat,
    plan: &ResolutionPlan,
    dry_run: bool,
    root: &str,
    db_path: &str,
    progress: &InstallProgress,
    sandbox_mode: SandboxMode,
) -> Result<()> {
    // Check for conflicts (fail on any conflict)
    if !plan.conflicts.is_empty() {
        eprintln!("\nDependency conflicts detected:");
        for conflict in &plan.conflicts {
            eprintln!("  {}", conflict);
        }
        return Err(anyhow::anyhow!(
            "Cannot install {}: {} dependency conflict(s) detected",
            pkg.name(),
            plan.conflicts.len()
        ));
    }

    // Handle missing dependencies
    if plan.missing.is_empty() {
        println!("All dependencies already satisfied");
        return Ok(());
    }

    info!("Found {} missing dependencies", plan.missing.len());

    // Try to find missing deps in repositories.
    // NOTE: This legacy path drops version constraints by using name-only lookup.
    // The main install flow in mod.rs now uses `resolve_dependencies_transitive_requests`
    // with full (name, constraint) tuples.  This function is kept for backward
    // compatibility with callers that go through `handle_missing_dependencies` directly.
    // TODO: remove after full migration -- callers should use the policy-aware path
    // in mod.rs or conversion.rs instead.
    let missing_names: Vec<String> = plan.missing.iter().map(|m| m.name.clone()).collect();

    match repository::resolve_dependencies_transitive(conn, &missing_names, 10) {
        Ok(to_download) => {
            if !to_download.is_empty() {
                handle_downloadable_deps(
                    conn,
                    pkg,
                    &to_download,
                    dry_run,
                    root,
                    db_path,
                    progress,
                    sandbox_mode,
                )
                .await?;
            } else {
                // Dependencies not found in Conary repos - check provides table
                check_provides_fallback(conn, pkg, &plan.missing)?;
            }
        }
        Err(e) => {
            debug!("Repository lookup failed: {}", e);
            // Check provides table for dependencies
            check_provides_fallback(conn, pkg, &plan.missing)?;
        }
    }

    Ok(())
}

/// Handle dependencies that can be downloaded from repositories
#[allow(clippy::too_many_arguments)]
async fn handle_downloadable_deps(
    _conn: &mut Connection,
    pkg: &dyn PackageFormat,
    to_download: &[(String, repository::PackageWithRepo)],
    dry_run: bool,
    root: &str,
    db_path: &str,
    progress: &InstallProgress,
    sandbox_mode: SandboxMode,
) -> Result<()> {
    if dry_run {
        println!("Would install {} missing dependencies:", to_download.len());
    } else {
        println!("Installing {} missing dependencies:", to_download.len());
    }
    for (dep_name, pkg_with_repo) in to_download {
        println!("  {} ({})", dep_name, pkg_with_repo.package.version);
    }

    if dry_run {
        return Ok(());
    }

    progress.set_phase(pkg.name(), InstallPhase::InstallingDeps);
    let temp_dir = TempDir::new()?;
    let keyring_dir = keyring_dir(db_path);

    match repository::download_dependencies(to_download, temp_dir.path(), Some(&keyring_dir)).await
    {
        Ok(downloaded) => {
            let parent_name = pkg.name().to_string();
            for (dep_name, dep_path) in downloaded {
                progress.set_status(&format!("Installing dependency: {}", dep_name));
                info!("Installing dependency: {}", dep_name);
                println!("Installing dependency: {}", dep_name);
                let reason = format!("Required by {}", parent_name);
                let path_str = dep_path.to_string_lossy().to_string();
                if let Err(e) = cmd_install(
                    &path_str,
                    super::InstallOptions {
                        db_path,
                        root,
                        dry_run,
                        selection_reason: Some(&reason),
                        sandbox_mode,
                        ..Default::default()
                    },
                )
                .await
                {
                    return Err(anyhow::anyhow!(
                        "Failed to install dependency {}: {}",
                        dep_name,
                        e
                    ));
                }
                println!("  [OK] Installed {}", dep_name);
            }
            Ok(())
        }
        Err(e) => Err(anyhow::anyhow!("Failed to download dependencies: {}", e)),
    }
}

/// Check if dependencies can be satisfied by tracked provides
fn check_provides_fallback(
    conn: &Connection,
    pkg: &dyn PackageFormat,
    missing: &[conary_core::resolver::MissingDependency],
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
            pkg.name(),
            unsatisfied.len()
        ));
    }

    // All dependencies satisfied by tracked packages
    println!("All dependencies satisfied by tracked packages");
    Ok(())
}
