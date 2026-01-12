// src/repository/management.rs

//! Repository management operations
//!
//! Functions for adding, removing, enabling/disabling repositories,
//! and searching for packages.

use crate::db::models::{Repository, RepositoryPackage};
use crate::error::{Error, Result};
use rusqlite::Connection;
use tracing::info;

/// Add a new repository to the database
pub fn add_repository(
    conn: &Connection,
    name: String,
    url: String,
    enabled: bool,
    priority: i32,
) -> Result<Repository> {
    // Check if repository with this name already exists
    if Repository::find_by_name(conn, &name)?.is_some() {
        return Err(Error::ConflictError(format!(
            "Repository '{name}' already exists"
        )));
    }

    let mut repo = Repository::new(name, url);
    repo.enabled = enabled;
    repo.priority = priority;

    repo.insert(conn)?;

    info!("Added repository: {} ({})", repo.name, repo.url);
    Ok(repo)
}

/// Remove a repository from the database
pub fn remove_repository(conn: &Connection, name: &str) -> Result<()> {
    let repo = Repository::find_by_name(conn, name)?
        .ok_or_else(|| Error::NotFoundError(format!("Repository '{name}' not found")))?;

    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;
    Repository::delete(conn, repo_id)?;
    info!("Removed repository: {}", name);
    Ok(())
}

/// Enable or disable a repository
pub fn set_repository_enabled(conn: &Connection, name: &str, enabled: bool) -> Result<()> {
    let mut repo = Repository::find_by_name(conn, name)?
        .ok_or_else(|| Error::NotFoundError(format!("Repository '{name}' not found")))?;

    repo.enabled = enabled;
    repo.update(conn)?;

    info!(
        "Repository '{}' {}",
        name,
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}

/// Search for packages across all enabled repositories
pub fn search_packages(conn: &Connection, pattern: &str) -> Result<Vec<RepositoryPackage>> {
    let packages = RepositoryPackage::search(conn, pattern)?;
    Ok(packages)
}
