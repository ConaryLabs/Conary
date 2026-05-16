// src/commands/query/dependency.rs

//! Dependency query commands
//!
//! Functions for querying package dependencies, reverse dependencies,
//! what would break on removal, and what provides a capability.

use super::super::open_db;
use crate::commands::{InstalledPackageSelector, resolve_installed_package};
use anyhow::Result;
use conary_core::db::models::{
    DependencyEntry, ProvideEntry, Repository, RepositoryPackage, RepositoryProvide, Trove,
};
use std::collections::HashSet;
use tracing::info;

/// Show dependencies for a package
pub async fn cmd_depends(package_name: &str, db_path: &str) -> Result<()> {
    info!("Showing dependencies for package: {}", package_name);
    let conn = open_db(db_path)?;

    let trove = Trove::find_one_by_name(&conn, package_name)?
        .ok_or_else(|| anyhow::anyhow!("Package '{}' not found", package_name))?;
    let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

    let deps = DependencyEntry::find_by_trove(&conn, trove_id)?;

    if deps.is_empty() {
        println!("Package '{}' has no dependencies", package_name);
    } else {
        println!("Dependencies for package '{}':", package_name);
        for dep in deps {
            // Display typed dependency
            let typed_str = dep.to_typed_string();
            print!("  {} [{}]", typed_str, dep.dependency_type);
            if let Some(version) = dep.depends_on_version {
                print!(" - version: {}", version);
            }
            println!();
        }
    }

    Ok(())
}

/// Show reverse dependencies
pub async fn cmd_rdepends(package_name: &str, db_path: &str) -> Result<()> {
    info!("Showing reverse dependencies for package: {}", package_name);
    let conn = open_db(db_path)?;

    let dependents = DependencyEntry::find_dependents(&conn, package_name)?;

    if dependents.is_empty() {
        println!(
            "No packages depend on '{}' (or package not installed)",
            package_name
        );
    } else {
        println!("Packages that depend on '{}':", package_name);
        for dep in dependents {
            if let Ok(Some(trove)) = Trove::find_by_id(&conn, dep.trove_id) {
                // Show the dependency kind if not a plain package
                let kind_str = if dep.kind != "package" && !dep.kind.is_empty() {
                    format!(" [{}]", dep.kind)
                } else {
                    String::new()
                };
                print!("  {} ({}){}", trove.name, dep.dependency_type, kind_str);
                if let Some(constraint) = dep.version_constraint {
                    print!(" - requires: {}", constraint);
                }
                println!();
            }
        }
    }

    Ok(())
}

/// Show what packages would break if a package is removed
pub async fn cmd_whatbreaks(package_name: &str, db_path: &str) -> Result<()> {
    info!(
        "Checking what would break if '{}' is removed...",
        package_name
    );
    let conn = open_db(db_path)?;

    let selector = InstalledPackageSelector::new(package_name.to_string(), None, None);
    let resolved = resolve_installed_package(&conn, &selector)?;
    let trove = resolved.trove;

    let mut has_preflight_blocker = false;
    if trove.pinned {
        println!(
            "Package '{}' is pinned and remove would be refused before mutation.",
            trove.name
        );
        has_preflight_blocker = true;
    }
    if crate::commands::install::is_package_blocked(&trove.name) {
        println!(
            "Package '{}' is critical and remove would be refused before mutation.",
            trove.name
        );
        has_preflight_blocker = true;
    }
    if trove.install_source.is_adopted() {
        println!(
            "Package '{}' is adopted; native package-manager authority is preserved.",
            trove.name
        );
        has_preflight_blocker = true;
    }

    let breaking = conary_core::resolver::solve_removal(&conn, std::slice::from_ref(&trove.name))?;

    if breaking.is_empty() {
        if has_preflight_blocker {
            println!(
                "No dependency breakage found, but remove would still be refused before mutation."
            );
        } else {
            println!(
                "Package '{}' can be safely removed (no dependencies)",
                trove.name
            );
        }
    } else {
        println!(
            "Removing '{}' would break the following packages:",
            trove.name
        );
        for pkg in &breaking {
            println!("  {}", pkg);
        }
        println!("\nTotal: {} packages would be affected", breaking.len());
    }

    Ok(())
}

/// Find what package provides a capability
///
/// Searches for packages that provide a given capability, which can be:
/// - A package name
/// - A virtual provide (e.g., perl(DBI))
/// - A file path (e.g., /usr/bin/python3)
/// - A typed capability (e.g., soname(libssl.so.3))
pub async fn cmd_whatprovides(capability: &str, db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;

    let providers = installed_providers_for_capability(&conn, capability)?;
    let repo_providers = repository_providers_for_capability(&conn, capability)?;

    if providers.is_empty() && repo_providers.is_empty() {
        println!("No package provides '{}'", capability);
        return Ok(());
    }

    println!("Capability '{}' is provided by:", capability);
    if !providers.is_empty() {
        println!("Installed providers:");
        for provide in &providers {
            if let Ok(Some(trove)) = Trove::find_by_id(&conn, provide.trove_id) {
                print!("  {} {}", trove.name, trove.version);
                if let Some(ref ver) = provide.version {
                    print!(" (provides version: {})", ver);
                }
                if let Some(ref arch) = trove.architecture {
                    print!(" [{}]", arch);
                }
                println!();
            }
        }
    }

    let mut rendered_repo_providers = 0usize;
    if !repo_providers.is_empty() {
        println!("Repository providers:");
        for provide in &repo_providers {
            let Some(pkg) = RepositoryPackage::find_by_id(&conn, provide.repository_package_id)?
            else {
                continue;
            };
            let repo_name = Repository::find_by_id(&conn, pkg.repository_id)?
                .map(|repo| repo.name)
                .unwrap_or_else(|| "unknown-repo".to_string());
            print!("  {} {}", pkg.name, pkg.version);
            if let Some(arch) = &pkg.architecture {
                print!(" [{}]", arch);
            }
            print!(" @{}", repo_name);
            if let Some(version) = &provide.version {
                print!(" (provides version: {})", version);
            }
            println!();
            rendered_repo_providers += 1;
        }
    }

    println!(
        "\nTotal: {} provider(s)",
        providers.len() + rendered_repo_providers
    );
    Ok(())
}

fn installed_providers_for_capability(
    conn: &rusqlite::Connection,
    capability: &str,
) -> Result<Vec<ProvideEntry>> {
    let mut providers = Vec::new();
    let mut seen_troves = HashSet::new();

    for provider in ProvideEntry::find_all_by_cli_exact_query(conn, capability)? {
        if seen_troves.insert(provider.trove_id) {
            providers.push(provider);
        }
    }

    if let Some((kind, typed_capability)) = parse_typed_capability_query(capability) {
        for provider in ProvideEntry::find_all_typed(conn, kind, typed_capability)? {
            if seen_troves.insert(provider.trove_id) {
                providers.push(provider);
            }
        }
    }

    Ok(providers)
}

fn repository_providers_for_capability(
    conn: &rusqlite::Connection,
    capability: &str,
) -> Result<Vec<RepositoryProvide>> {
    let mut providers = Vec::new();
    let mut seen_packages = HashSet::new();

    for provider in RepositoryProvide::find_by_cli_exact_query(conn, capability)? {
        if seen_packages.insert(provider.repository_package_id) {
            providers.push(provider);
        }
    }

    if let Some((kind, typed_capability)) = parse_typed_capability_query(capability) {
        for provider in
            RepositoryProvide::find_by_capability_and_kind(conn, typed_capability, kind)?
        {
            if seen_packages.insert(provider.repository_package_id) {
                providers.push(provider);
            }
        }
    }
    Ok(providers)
}

fn parse_typed_capability_query(capability: &str) -> Option<(&str, &str)> {
    let (kind, value) = capability.split_once('(')?;
    let value = value.strip_suffix(')')?;
    if kind.is_empty() || value.is_empty() {
        return None;
    }
    if !kind
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return None;
    }
    Some((kind, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_typed_capability_query_reads_explicit_wrapper() {
        let parsed = parse_typed_capability_query("soname(libssl.so.3)");

        assert_eq!(parsed, Some(("soname", "libssl.so.3")));
    }

    #[test]
    fn parse_typed_capability_query_ignores_native_suffix_metadata() {
        let parsed = parse_typed_capability_query("libssl.so.3()(64bit)");

        assert_eq!(parsed, None);
    }
}
