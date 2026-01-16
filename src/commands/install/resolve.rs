// src/commands/install/resolve.rs
//! Package path resolution - downloading from repository if needed

use crate::commands::progress::{InstallPhase, InstallProgress};
use anyhow::{Context, Result};
use conary::repository::{self, DownloadOptions, PackageSelector, SelectionOptions};
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tracing::info;

/// Get the keyring directory based on db_path
pub fn get_keyring_dir(db_path: &str) -> PathBuf {
    let db_dir = std::env::var("CONARY_DB_DIR").unwrap_or_else(|_| {
        Path::new(db_path)
            .parent()
            .unwrap_or(Path::new("/var/lib/conary"))
            .to_string_lossy()
            .to_string()
    });
    PathBuf::from(db_dir).join("keys")
}

/// Result of resolving a package path
pub struct ResolvedPackage {
    pub path: PathBuf,
    /// Temp directory that must stay alive until installation completes
    pub _temp_dir: Option<TempDir>,
}

/// Resolve package to a local path, downloading from repository if needed
pub fn resolve_package_path(
    package: &str,
    db_path: &str,
    version: Option<&str>,
    repo: Option<&str>,
    progress: &InstallProgress,
) -> Result<ResolvedPackage> {
    if Path::new(package).exists() {
        info!("Installing from local file: {}", package);
        progress.set_status(&format!("Loading local file: {}", package));
        return Ok(ResolvedPackage {
            path: PathBuf::from(package),
            _temp_dir: None,
        });
    }

    info!("Searching repositories for package: {}", package);
    progress.set_status("Searching repositories...");

    let conn = conary::db::open(db_path)
        .context("Failed to open package database")?;

    let options = SelectionOptions {
        version: version.map(String::from),
        repository: repo.map(String::from),
        architecture: None,
    };

    let pkg_with_repo = PackageSelector::find_best_package(&conn, package, &options)
        .with_context(|| format!("Failed to find package '{}' in repositories", package))?;

    info!(
        "Found package {} {} in repository {} (priority {})",
        pkg_with_repo.package.name,
        pkg_with_repo.package.version,
        pkg_with_repo.repository.name,
        pkg_with_repo.repository.priority
    );

    let temp_dir = TempDir::new()
        .context("Failed to create temporary directory for download")?;

    // Set up GPG verification options if enabled for this repository
    let gpg_options = if pkg_with_repo.repository.gpg_check {
        let keyring_dir = get_keyring_dir(db_path);
        Some(DownloadOptions {
            gpg_check: true,
            gpg_strict: pkg_with_repo.repository.gpg_strict,
            keyring_dir,
            repository_name: pkg_with_repo.repository.name.clone(),
        })
    } else {
        None
    };

    progress.set_phase(&pkg_with_repo.package.name, InstallPhase::Downloading);
    let download_path = repository::download_package_verified(
        &pkg_with_repo.package,
        temp_dir.path(),
        gpg_options.as_ref(),
    )
    .with_context(|| format!("Failed to download package '{}'", pkg_with_repo.package.name))?;

    info!("Downloaded package to: {}", download_path.display());

    Ok(ResolvedPackage {
        path: download_path,
        _temp_dir: Some(temp_dir),
    })
}
