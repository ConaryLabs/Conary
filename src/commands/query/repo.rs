// src/commands/query/repo.rs

//! Repository query commands
//!
//! Functions for querying packages available in repositories (not installed).

use anyhow::Result;

/// Query packages available in repositories (not installed)
///
/// This is similar to `dnf repoquery` or `apt-cache search`.
pub fn cmd_repquery(pattern: Option<&str>, db_path: &str, info: bool) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let packages = if let Some(pattern) = pattern {
        conary::db::models::RepositoryPackage::search(&conn, pattern)?
    } else {
        conary::db::models::RepositoryPackage::list_all(&conn)?
    };

    if packages.is_empty() {
        if let Some(p) = pattern {
            println!("No packages matching '{}' found in repositories.", p);
        } else {
            println!("No packages in repositories. Run 'conary repo-sync' first.");
        }
        return Ok(());
    }

    // If info mode and single result, show detailed info
    if info && packages.len() == 1 {
        return show_repo_package_info(&conn, &packages[0]);
    }

    println!("Available packages{}:", pattern.map(|p| format!(" matching '{}'", p)).unwrap_or_default());
    for pkg in &packages {
        print!("  {} {}", pkg.name, pkg.version);
        if let Some(arch) = &pkg.architecture {
            print!(" [{}]", arch);
        }
        // Show which repo it's from
        if let Ok(repo_name) = pkg.get_repository_name(&conn) {
            print!(" @{}", repo_name);
        }
        println!();
    }
    println!("\nTotal: {} package(s) available", packages.len());

    Ok(())
}

/// Show detailed info for a repository package
fn show_repo_package_info(
    conn: &rusqlite::Connection,
    pkg: &conary::db::models::RepositoryPackage,
) -> Result<()> {
    println!("Name        : {}", pkg.name);
    println!("Version     : {}", pkg.version);

    if let Some(arch) = &pkg.architecture {
        println!("Architecture: {}", arch);
    }

    if let Some(desc) = &pkg.description {
        println!("Description : {}", desc);
    }

    println!("Size        : {}", pkg.size_human());

    if let Ok(repo_name) = pkg.get_repository_name(conn) {
        println!("Repository  : {}", repo_name);
    }

    println!("Checksum    : {}", pkg.checksum);
    println!("URL         : {}", pkg.download_url);

    // Check if installed
    let installed = conary::db::models::Trove::find_by_name(conn, &pkg.name)?;
    if let Some(installed_pkg) = installed.first() {
        println!("Status      : Installed ({})", installed_pkg.version);
    } else {
        println!("Status      : Not installed");
    }

    // Show dependencies
    if let Ok(deps) = pkg.parse_dependencies()
        && !deps.is_empty()
    {
        println!("\nDependencies ({}):", deps.len());
        for dep in &deps {
            println!("  {}", dep);
        }
    }

    Ok(())
}
