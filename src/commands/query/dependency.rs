// src/commands/query/dependency.rs

//! Dependency query commands
//!
//! Functions for querying package dependencies, reverse dependencies,
//! what would break on removal, and what provides a capability.

use anyhow::Result;
use tracing::info;

/// Show dependencies for a package
pub fn cmd_depends(package_name: &str, db_path: &str) -> Result<()> {
    info!("Showing dependencies for package: {}", package_name);
    let conn = conary::db::open(db_path)?;

    let troves = conary::db::models::Trove::find_by_name(&conn, package_name)?;
    let trove = troves
        .first()
        .ok_or_else(|| anyhow::anyhow!("Package '{}' not found", package_name))?;
    let trove_id = trove
        .id
        .ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

    let deps = conary::db::models::DependencyEntry::find_by_trove(&conn, trove_id)?;

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
pub fn cmd_rdepends(package_name: &str, db_path: &str) -> Result<()> {
    info!(
        "Showing reverse dependencies for package: {}",
        package_name
    );
    let conn = conary::db::open(db_path)?;

    let dependents = conary::db::models::DependencyEntry::find_dependents(&conn, package_name)?;

    if dependents.is_empty() {
        println!(
            "No packages depend on '{}' (or package not installed)",
            package_name
        );
    } else {
        println!("Packages that depend on '{}':", package_name);
        for dep in dependents {
            if let Ok(Some(trove)) = conary::db::models::Trove::find_by_id(&conn, dep.trove_id) {
                // Show the dependency kind if not a plain package
                let kind_str = if dep.kind != "package" && !dep.kind.is_empty() {
                    format!(" [{}]", dep.kind)
                } else {
                    String::new()
                };
                print!("  {} ({}){}",trove.name, dep.dependency_type, kind_str);
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
pub fn cmd_whatbreaks(package_name: &str, db_path: &str) -> Result<()> {
    info!(
        "Checking what would break if '{}' is removed...",
        package_name
    );
    let conn = conary::db::open(db_path)?;

    let troves = conary::db::models::Trove::find_by_name(&conn, package_name)?;
    troves
        .first()
        .ok_or_else(|| anyhow::anyhow!("Package '{}' not found", package_name))?;

    let resolver = conary::resolver::Resolver::new(&conn)?;
    let breaking = resolver.check_removal(package_name)?;

    if breaking.is_empty() {
        println!(
            "Package '{}' can be safely removed (no dependencies)",
            package_name
        );
    } else {
        println!(
            "Removing '{}' would break the following packages:",
            package_name
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
/// - A shared library (e.g., libssl.so.3)
pub fn cmd_whatprovides(capability: &str, db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    // First try exact match
    let mut providers = conary::db::models::ProvideEntry::find_all_by_capability(&conn, capability)?;

    // If no exact match, try pattern search
    if providers.is_empty() {
        // Try with wildcards for partial matching
        let pattern = format!("%{}%", capability);
        providers = conary::db::models::ProvideEntry::search_capability(&conn, &pattern)?;
    }

    if providers.is_empty() {
        println!("No package provides '{}'", capability);
        return Ok(());
    }

    println!("Capability '{}' is provided by:", capability);
    for provide in &providers {
        if let Ok(Some(trove)) = conary::db::models::Trove::find_by_id(&conn, provide.trove_id) {
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

    println!("\nTotal: {} provider(s)", providers.len());
    Ok(())
}
