// src/commands/repo.rs
//! Repository management commands

use anyhow::Result;
use tracing::info;

/// Add a new repository
pub fn cmd_repo_add(
    name: &str,
    url: &str,
    db_path: &str,
    priority: i32,
    disabled: bool,
) -> Result<()> {
    info!("Adding repository: {} ({})", name, url);
    let conn = conary::db::open(db_path)?;
    let repo = conary::repository::add_repository(
        &conn,
        name.to_string(),
        url.to_string(),
        !disabled,
        priority,
    )?;
    println!("Added repository: {}", repo.name);
    println!("  URL: {}", repo.url);
    println!("  Enabled: {}", repo.enabled);
    println!("  Priority: {}", repo.priority);
    Ok(())
}

/// List repositories
pub fn cmd_repo_list(db_path: &str, all: bool) -> Result<()> {
    info!("Listing repositories");
    let conn = conary::db::open(db_path)?;
    let repos = if all {
        conary::db::models::Repository::list_all(&conn)?
    } else {
        conary::db::models::Repository::list_enabled(&conn)?
    };

    if repos.is_empty() {
        println!("No repositories configured");
    } else {
        println!("Repositories:");
        for repo in repos {
            let enabled_mark = if repo.enabled { "[x]" } else { "[ ]" };
            let sync_status = repo
                .last_sync
                .as_ref()
                .map(|ts| format!("synced {}", ts))
                .unwrap_or_else(|| "never synced".to_string());
            println!(
                "  {} {} (priority: {}, {})",
                enabled_mark, repo.name, repo.priority, sync_status
            );
            println!("      {}", repo.url);
        }
    }
    Ok(())
}

/// Remove a repository
pub fn cmd_repo_remove(name: &str, db_path: &str) -> Result<()> {
    info!("Removing repository: {}", name);
    let conn = conary::db::open(db_path)?;
    conary::repository::remove_repository(&conn, name)?;
    println!("Removed repository: {}", name);
    Ok(())
}

/// Enable a repository
pub fn cmd_repo_enable(name: &str, db_path: &str) -> Result<()> {
    info!("Enabling repository: {}", name);
    let conn = conary::db::open(db_path)?;
    conary::repository::set_repository_enabled(&conn, name, true)?;
    println!("Enabled repository: {}", name);
    Ok(())
}

/// Disable a repository
pub fn cmd_repo_disable(name: &str, db_path: &str) -> Result<()> {
    info!("Disabling repository: {}", name);
    let conn = conary::db::open(db_path)?;
    conary::repository::set_repository_enabled(&conn, name, false)?;
    println!("Disabled repository: {}", name);
    Ok(())
}

/// Sync repository metadata
pub fn cmd_repo_sync(name: Option<String>, db_path: &str, force: bool) -> Result<()> {
    info!("Synchronizing repository metadata");

    let conn = conary::db::open(db_path)?;

    let repos_to_sync = if let Some(repo_name) = name {
        let repo = conary::db::models::Repository::find_by_name(&conn, &repo_name)?
            .ok_or_else(|| anyhow::anyhow!("Repository '{}' not found", repo_name))?;
        vec![repo]
    } else {
        conary::db::models::Repository::list_enabled(&conn)?
    };

    if repos_to_sync.is_empty() {
        println!("No repositories to sync");
        return Ok(());
    }

    let repos_needing_sync: Vec<_> = repos_to_sync
        .into_iter()
        .filter(|repo| force || conary::repository::needs_sync(repo))
        .collect();

    if repos_needing_sync.is_empty() {
        println!("All repositories are up to date");
        return Ok(());
    }

    use rayon::prelude::*;
    let results: Vec<(String, conary::Result<usize>)> = repos_needing_sync
        .par_iter()
        .map(|repo| {
            println!("Syncing repository: {} ...", repo.name);
            let sync_result = (|| -> conary::Result<usize> {
                let conn = conary::db::open(db_path)?;
                let mut repo_mut = repo.clone();
                conary::repository::sync_repository(&conn, &mut repo_mut)
            })();
            (repo.name.clone(), sync_result)
        })
        .collect();

    for (name, result) in results {
        match result {
            Ok(count) => println!("  [OK] Synchronized {} packages from {}", count, name),
            Err(e) => println!("  [FAILED] Failed to sync {}: {}", name, e),
        }
    }

    Ok(())
}

/// Search for packages
pub fn cmd_search(pattern: &str, db_path: &str) -> Result<()> {
    info!("Searching for packages matching: {}", pattern);
    let conn = conary::db::open(db_path)?;
    let packages = conary::repository::search_packages(&conn, pattern)?;

    if packages.is_empty() {
        println!("No packages found matching '{}'", pattern);
    } else {
        println!("Found {} packages matching '{}':", packages.len(), pattern);
        for pkg in packages {
            let arch_str = pkg.architecture.as_deref().unwrap_or("noarch");
            println!("  {} {} ({})", pkg.name, pkg.version, arch_str);
            if let Some(desc) = &pkg.description {
                println!("      {}", desc);
            }
        }
    }
    Ok(())
}
