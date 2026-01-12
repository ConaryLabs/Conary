// src/commands/query.rs
//! Query and dependency inspection commands

use anyhow::Result;
use tracing::info;

/// Query installed packages
pub fn cmd_query(pattern: Option<&str>, db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let troves = if let Some(pattern) = pattern {
        conary::db::models::Trove::find_by_name(&conn, pattern)?
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, name, version, type, architecture, description, installed_at, installed_by_changeset_id
             FROM troves ORDER BY name, version"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(conary::db::models::Trove {
                id: Some(row.get(0)?),
                name: row.get(1)?,
                version: row.get(2)?,
                trove_type: row.get::<_, String>(3)?
                    .parse()
                    .unwrap_or(conary::db::models::TroveType::Package),
                architecture: row.get(4)?,
                description: row.get(5)?,
                installed_at: row.get(6)?,
                installed_by_changeset_id: row.get(7)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    if troves.is_empty() {
        println!("No packages found.");
    } else {
        println!("Installed packages:");
        for trove in &troves {
            print!(
                "  {} {} ({:?})",
                trove.name, trove.version, trove.trove_type
            );
            if let Some(arch) = &trove.architecture {
                print!(" [{}]", arch);
            }
            println!();
        }
        println!("\nTotal: {} package(s)", troves.len());
    }

    Ok(())
}

/// Show changeset history
pub fn cmd_history(db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;
    let changesets = conary::db::models::Changeset::list_all(&conn)?;

    if changesets.is_empty() {
        println!("No changeset history.");
    } else {
        println!("Changeset history:");
        for changeset in &changesets {
            let timestamp = changeset
                .applied_at
                .as_ref()
                .or(changeset.rolled_back_at.as_ref())
                .or(changeset.created_at.as_ref())
                .map(|s| s.as_str())
                .unwrap_or("pending");
            let id = changeset
                .id
                .map(|i| i.to_string())
                .unwrap_or_else(|| "?".to_string());
            println!(
                "  [{}] {} - {} ({:?})",
                id, timestamp, changeset.description, changeset.status
            );
        }
        println!("\nTotal: {} changeset(s)", changesets.len());
    }

    Ok(())
}

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
            print!("  {} ({})", dep.depends_on_name, dep.dependency_type);
            if let Some(version) = dep.depends_on_version {
                print!(" - version: {}", version);
            }
            if let Some(constraint) = dep.version_constraint {
                print!(" - constraint: {}", constraint);
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
                print!("  {} ({})", trove.name, dep.dependency_type);
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
