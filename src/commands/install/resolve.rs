// src/commands/install/resolve.rs
//! Package path resolution - downloading from repository if needed
//!
//! This module handles resolving package names to local file paths, using
//! the unified resolution flow with per-package routing strategies.
//!
//! # Resolution Flow
//!
//! 1. Check if package is a local file path
//! 2. Check for package redirects (renames, obsoletes)
//! 3. Use unified resolver to:
//!    a. Select best repository (priority/version logic)
//!    b. Look up routing strategies in `package_resolution` table
//!    c. Try strategies in order (binary, refinery, recipe, delegate, legacy)
//! 4. Return local path to downloaded/built package

use crate::commands::progress::{InstallPhase, InstallProgress};
use anyhow::{Context, Result};
use conary::db::models::{ProvideEntry, Redirect};
use conary::repository::{resolve_package, PackageSource, ResolutionOptions};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tracing::{debug, info};

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
    /// Source type (for logging/UI)
    #[allow(dead_code)] // Will be used for logging/UI in future
    pub source_type: ResolvedSourceType,
}

/// Type of source the package was resolved from
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Variants for future phases
pub enum ResolvedSourceType {
    /// Local file provided by user
    LocalFile,
    /// Downloaded binary from repository
    Binary,
    /// Converted via Refinery
    Refinery,
    /// Built from recipe
    Recipe,
    /// Resolved through label delegation
    Delegate,
    /// Legacy repository_packages path
    Legacy,
    /// From local CAS cache
    LocalCas,
}

impl ResolvedSourceType {
    /// Get a human-readable description
    #[allow(dead_code)] // Will be used for logging/UI in future
    pub fn description(&self) -> &'static str {
        match self {
            Self::LocalFile => "local file",
            Self::Binary => "binary package",
            Self::Refinery => "Refinery conversion",
            Self::Recipe => "recipe build",
            Self::Delegate => "delegated resolution",
            Self::Legacy => "repository",
            Self::LocalCas => "local cache",
        }
    }
}

/// Resolve package to a local path, downloading from repository if needed
///
/// This is the main entry point for package resolution. It uses the unified
/// resolution flow with per-package routing strategies.
pub fn resolve_package_path(
    package: &str,
    db_path: &str,
    version: Option<&str>,
    repo: Option<&str>,
    progress: &InstallProgress,
) -> Result<ResolvedPackage> {
    // Check if package is a local file
    if Path::new(package).exists() {
        info!("Installing from local file: {}", package);
        progress.set_status(&format!("Loading local file: {}", package));
        return Ok(ResolvedPackage {
            path: PathBuf::from(package),
            _temp_dir: None,
            source_type: ResolvedSourceType::LocalFile,
        });
    }

    info!("Searching repositories for package: {}", package);
    progress.set_status("Searching repositories...");

    let conn = conary::db::open(db_path)
        .context("Failed to open package database")?;

    // Check for package redirects (renames, obsoletes, etc.)
    let resolved_name = resolve_redirects(&conn, package, version);

    // Build resolution options
    // Note: keyring_dir will be used when GPG options are integrated into resolution
    let _keyring_dir = get_keyring_dir(db_path);
    let options = ResolutionOptions {
        version: version.map(String::from),
        repository: repo.map(String::from),
        architecture: None,
        output_dir: None,
        gpg_options: None, // Will be set per-repository in resolver
        skip_cas: false,
    };

    // Use unified resolver
    progress.set_status("Resolving package source...");
    let source = resolve_package(&conn, &resolved_name, &options)
        .with_context(|| format!("Failed to resolve package '{}'", package))?;

    // Convert PackageSource to ResolvedPackage
    convert_source_to_resolved(source, package, progress)
}

/// Resolve package redirects (renames, obsoletes)
fn resolve_redirects(
    conn: &rusqlite::Connection,
    package: &str,
    version: Option<&str>,
) -> String {
    match Redirect::resolve(conn, package, version) {
        Ok(result) => {
            if result.was_redirected {
                // Print redirect messages to user
                for msg in &result.messages {
                    eprintln!("Note: {}", msg);
                }
                eprintln!(
                    "Note: '{}' has been redirected to '{}'",
                    package, result.resolved
                );
                info!(
                    "Package '{}' redirected to '{}' (chain: {})",
                    package,
                    result.resolved,
                    result.chain.join(" -> ")
                );
                result.resolved
            } else {
                package.to_string()
            }
        }
        Err(e) => {
            // Log the error but continue with original name
            // (redirect table might not exist on older DBs)
            info!("Redirect check failed (continuing with original name): {}", e);
            package.to_string()
        }
    }
}

/// Convert a PackageSource to a ResolvedPackage
fn convert_source_to_resolved(
    source: PackageSource,
    package: &str,
    progress: &InstallProgress,
) -> Result<ResolvedPackage> {
    match source {
        PackageSource::Binary { path, _temp_dir } => {
            info!("Resolved {} from binary source: {}", package, path.display());
            progress.set_phase(package, InstallPhase::Downloading);
            Ok(ResolvedPackage {
                path,
                _temp_dir,
                source_type: ResolvedSourceType::Binary,
            })
        }

        PackageSource::Ccs { path, _temp_dir } => {
            info!("Resolved {} from Refinery: {}", package, path.display());
            progress.set_phase(package, InstallPhase::Downloading);
            Ok(ResolvedPackage {
                path,
                _temp_dir,
                source_type: ResolvedSourceType::Refinery,
            })
        }

        PackageSource::Delta { base_version, delta_path, _temp_dir } => {
            info!(
                "Resolved {} from delta (base: {}): {}",
                package, base_version, delta_path.display()
            );
            progress.set_phase(package, InstallPhase::Downloading);
            // For now, treat delta as binary - the installer will handle it
            Ok(ResolvedPackage {
                path: delta_path,
                _temp_dir,
                source_type: ResolvedSourceType::Binary,
            })
        }

        PackageSource::LocalCas { hash } => {
            // Package is in local CAS - need to extract to temp location
            // For now, this is a placeholder - CAS integration is future work
            info!("Resolved {} from local CAS: {}", package, hash);
            Err(anyhow::anyhow!(
                "Local CAS resolution not yet implemented (hash: {})",
                hash
            ))
        }
    }
}

/// Check if missing dependencies are satisfied by packages in the provides table
///
/// This is a self-contained approach that doesn't query the host package manager.
/// Instead, it checks if any tracked package provides the required capability.
///
/// Returns a tuple of:
/// - satisfied: Vec of (dep_name, provider_name, version)
/// - unsatisfied: Vec of MissingDependency (cloned)
#[allow(clippy::type_complexity)]
pub fn check_provides_dependencies(
    conn: &Connection,
    missing: &[conary::resolver::MissingDependency],
) -> (
    Vec<(String, String, Option<String>)>,
    Vec<conary::resolver::MissingDependency>,
) {
    let mut satisfied = Vec::new();
    let mut unsatisfied = Vec::new();

    for dep in missing {
        // Check if this capability is provided by any tracked package (with fuzzy matching)
        match ProvideEntry::find_satisfying_provider_fuzzy(conn, &dep.name) {
            Ok(Some((provider, version))) => {
                satisfied.push((dep.name.clone(), provider, Some(version)));
            }
            Ok(None) => {
                unsatisfied.push(dep.clone());
            }
            Err(e) => {
                debug!("Error checking provides for {}: {}", dep.name, e);
                unsatisfied.push(dep.clone());
            }
        }
    }

    (satisfied, unsatisfied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolved_source_type_description() {
        assert_eq!(ResolvedSourceType::LocalFile.description(), "local file");
        assert_eq!(ResolvedSourceType::Binary.description(), "binary package");
        assert_eq!(ResolvedSourceType::Refinery.description(), "Refinery conversion");
        assert_eq!(ResolvedSourceType::Recipe.description(), "recipe build");
        assert_eq!(ResolvedSourceType::Legacy.description(), "repository");
    }

    #[test]
    fn test_get_keyring_dir() {
        let keyring = get_keyring_dir("/var/lib/conary/conary.db");
        assert!(keyring.ends_with("keys"));
    }
}
