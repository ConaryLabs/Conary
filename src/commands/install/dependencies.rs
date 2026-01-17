// src/commands/install/dependencies.rs
#![allow(dead_code)]

//! Dependency resolution for package installation
//!
//! Handles checking and resolving package dependencies, including
//! downloading missing dependencies from repositories.
//!
//! This module provides extracted helper functions for dependency resolution.
//! Currently the inline code in mod.rs is still used, but these functions
//! can be adopted incrementally to simplify the main install flow.

use super::resolve::{check_provides_dependencies, get_keyring_dir};
use crate::commands::install_package_from_file;
use crate::commands::progress::{InstallPhase, InstallProgress};
use anyhow::{Context, Result};
use conary::packages::traits::DependencyType;
use conary::packages::PackageFormat;
use conary::repository;
use conary::resolver::{DependencyEdge, ResolutionPlan, Resolver};
use conary::version::{RpmVersion, VersionConstraint};
use rusqlite::Connection;
use tempfile::TempDir;
use tracing::{debug, info};

/// Build dependency edges from a package's dependencies
pub fn build_dependency_edges(pkg: &dyn PackageFormat) -> Vec<DependencyEdge> {
    pkg.dependencies()
        .iter()
        .filter(|d| d.dep_type == DependencyType::Runtime)
        .map(|d| {
            let constraint = d
                .version
                .as_ref()
                .and_then(|v| VersionConstraint::parse(v).ok())
                .unwrap_or(VersionConstraint::Any);
            DependencyEdge {
                from: pkg.name().to_string(),
                to: d.name.clone(),
                constraint,
                dep_type: "runtime".to_string(),
                kind: "package".to_string(),
            }
        })
        .collect()
}


/// Resolve dependencies for a package
///
/// Returns the resolution plan if successful, or an error if there are conflicts.
pub fn resolve_dependencies(
    conn: &Connection,
    pkg: &dyn PackageFormat,
    dependency_edges: Vec<DependencyEdge>,
) -> Result<ResolutionPlan> {
    let package_version = RpmVersion::parse(pkg.version())
        .with_context(|| format!("Failed to parse version '{}' for package '{}'", pkg.version(), pkg.name()))?;

    let mut resolver = Resolver::new(conn)
        .context("Failed to initialize dependency resolver")?;

    resolver.resolve_install(
        pkg.name().to_string(),
        package_version,
        dependency_edges,
    ).with_context(|| format!("Failed to resolve dependencies for '{}'", pkg.name()))
}

/// Check for dependency conflicts and handle missing dependencies
///
/// Returns Ok(()) if all dependencies can be satisfied, or an error with details.
pub fn handle_missing_dependencies(
    conn: &mut Connection,
    pkg: &dyn PackageFormat,
    plan: &ResolutionPlan,
    dry_run: bool,
    root: &str,
    db_path: &str,
    progress: &InstallProgress,
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

    // Try to find missing deps in repositories
    let missing_names: Vec<String> = plan.missing.iter().map(|m| m.name.clone()).collect();

    match repository::resolve_dependencies_transitive(conn, &missing_names, 10) {
        Ok(to_download) => {
            if !to_download.is_empty() {
                handle_downloadable_deps(
                    conn, pkg, &to_download, dry_run, root, db_path, progress
                )?;
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
fn handle_downloadable_deps(
    conn: &mut Connection,
    pkg: &dyn PackageFormat,
    to_download: &[(String, repository::PackageWithRepo)],
    dry_run: bool,
    root: &str,
    db_path: &str,
    progress: &InstallProgress,
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
    let keyring_dir = get_keyring_dir(db_path);

    match repository::download_dependencies(to_download, temp_dir.path(), Some(&keyring_dir)) {
        Ok(downloaded) => {
            let parent_name = pkg.name().to_string();
            for (dep_name, dep_path) in downloaded {
                progress.set_status(&format!("Installing dependency: {}", dep_name));
                info!("Installing dependency: {}", dep_name);
                println!("Installing dependency: {}", dep_name);
                let reason = format!("Required by {}", parent_name);
                if let Err(e) = install_package_from_file(&dep_path, conn, root, db_path, None, Some(&reason)) {
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
    missing: &[conary::resolver::MissingDependency],
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
