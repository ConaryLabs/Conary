// src/repository/resolution.rs

//! Unified package resolution with per-package routing
//!
//! This module implements the two-step resolution flow:
//! 1. **Repository Selection**: Use existing priority/version logic to pick winning repository
//! 2. **Strategy Resolution**: Look up package in winner's routing table, try strategies in order
//!
//! # Resolution Flow
//!
//! ```text
//! resolve_package(name, options)
//!     |
//!     v
//! Check local CAS (already have it?) ──> Yes ──> Return LocalCas
//!     |
//!     No
//!     v
//! Repo selection (priority logic) ──────────────> PackageWithRepo
//!     |
//!     v
//! Get routing strategies ──> Found ──> Try each strategy in order
//!     |                                     |
//!     Not found (no routing entry)          v
//!     |                               Strategy succeeded? ──> Return source
//!     v                                     |
//! Construct Legacy strategy                 No, try next
//! from repository_packages                  |
//!     |                                     v
//!     v                               All strategies failed ──> Error
//! Try Legacy strategy
//! ```
//!
//! # Implicit Legacy Fallback
//!
//! When no `package_resolution` entry exists for a package, the resolver
//! implicitly constructs a `Legacy` strategy from the existing `repository_packages`
//! row. This ensures backwards compatibility without data migration.
//!
//! # Example
//!
//! ```ignore
//! let options = ResolutionOptions::default();
//! let source = resolve_package(&conn, "nginx", &options)?;
//!
//! match source {
//!     PackageSource::Binary(path) => install_binary(path),
//!     PackageSource::Ccs(path) => install_ccs(path),
//!     PackageSource::Delta(delta) => apply_delta(delta),
//!     PackageSource::LocalCas(hash) => install_from_cas(hash),
//! }
//! ```

use crate::db::models::{
    PackageResolution, PrimaryStrategy, Repository, RepositoryPackage, ResolutionStrategy,
};
use crate::error::{Error, Result};
use crate::recipe::{parse_recipe, Kitchen, KitchenConfig};
use crate::repository::refinery::RefineryClient;
use crate::repository::selector::{PackageSelector, PackageWithRepo, SelectionOptions};
use crate::repository::{download_package_verified, DownloadOptions};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use tracing::{debug, info, warn};

/// Maximum depth for delegate chain resolution
const MAX_DELEGATE_DEPTH: usize = 10;

/// Fetch content from a URL as a string
///
/// Uses curl to download the content. Supports HTTP(S) URLs.
fn fetch_url_content(url: &str) -> Result<String> {
    debug!("Fetching content from: {}", url);

    let output = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",  // Follow redirects
            url,
        ])
        .output()
        .map_err(|e| Error::IoError(format!("Failed to execute curl: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::DownloadError(format!(
            "Failed to fetch {}: {}",
            url, stderr
        )));
    }

    String::from_utf8(output.stdout)
        .map_err(|e| Error::ParseError(format!("Invalid UTF-8 content from {}: {}", url, e)))
}

/// Options for package resolution
#[derive(Debug, Clone, Default)]
pub struct ResolutionOptions {
    /// Specific version to resolve
    pub version: Option<String>,
    /// Specific repository to search
    pub repository: Option<String>,
    /// Specific architecture
    pub architecture: Option<String>,
    /// Output directory for downloads
    pub output_dir: Option<PathBuf>,
    /// GPG verification options
    pub gpg_options: Option<DownloadOptions>,
    /// Whether to skip local CAS check
    pub skip_cas: bool,
}

impl ResolutionOptions {
    /// Convert to SelectionOptions for repository selection
    pub fn to_selection_options(&self) -> SelectionOptions {
        SelectionOptions {
            version: self.version.clone(),
            repository: self.repository.clone(),
            architecture: self.architecture.clone(),
        }
    }
}

/// Result of package resolution
#[derive(Debug)]
pub enum PackageSource {
    /// Pre-built binary package at path
    Binary {
        path: PathBuf,
        /// Temp directory that must stay alive until installation completes
        _temp_dir: Option<TempDir>,
    },
    /// CCS package from Refinery
    Ccs {
        path: PathBuf,
        _temp_dir: Option<TempDir>,
    },
    /// Delta update (base version, delta path)
    Delta {
        base_version: String,
        delta_path: PathBuf,
        _temp_dir: Option<TempDir>,
    },
    /// Package already in local CAS
    LocalCas { hash: String },
}

impl PackageSource {
    /// Get the path to the package file
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Binary { path, .. } => Some(path),
            Self::Ccs { path, .. } => Some(path),
            Self::Delta { delta_path, .. } => Some(delta_path),
            Self::LocalCas { .. } => None,
        }
    }
}

/// Context for delegate chain resolution (cycle detection)
#[derive(Debug, Default)]
struct DelegateContext {
    depth: usize,
    visited: std::collections::HashSet<String>,
}

impl DelegateContext {
    fn new() -> Self {
        Self::default()
    }

    fn enter(&mut self, label: &str) -> Result<()> {
        if self.depth >= MAX_DELEGATE_DEPTH {
            return Err(Error::ResolutionError(format!(
                "Delegate chain too deep (max {}): {}",
                MAX_DELEGATE_DEPTH, label
            )));
        }
        if !self.visited.insert(label.to_string()) {
            return Err(Error::ResolutionError(format!(
                "Delegate cycle detected: {}",
                label
            )));
        }
        self.depth += 1;
        Ok(())
    }
}

/// Unified package resolver
///
/// Implements the two-step resolution flow with per-package routing.
pub struct PackageResolver<'a> {
    conn: &'a Connection,
}

impl<'a> PackageResolver<'a> {
    /// Create a new package resolver
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Resolve a package to its source
    ///
    /// This is the main entry point for package resolution. It performs:
    /// 1. Repository selection (using existing priority logic)
    /// 2. Strategy lookup from routing table (with implicit legacy fallback)
    /// 3. Strategy execution in priority order
    pub fn resolve(
        &self,
        name: &str,
        options: &ResolutionOptions,
    ) -> Result<PackageSource> {
        // TODO: Check local CAS first (when implemented)
        // if !options.skip_cas {
        //     if let Some(cached) = self.check_cas(name, options)? {
        //         return Ok(PackageSource::LocalCas { hash: cached });
        //     }
        // }

        // Step 1: Repository selection
        let pkg_with_repo = PackageSelector::find_best_package(
            self.conn,
            name,
            &options.to_selection_options(),
        )?;

        info!(
            "Selected package {} {} from repository {} (priority {})",
            pkg_with_repo.package.name,
            pkg_with_repo.package.version,
            pkg_with_repo.repository.name,
            pkg_with_repo.repository.priority
        );

        // Step 2: Get resolution strategies
        let strategies = self.get_strategies_or_legacy(&pkg_with_repo, options)?;

        // Step 3: Try each strategy in order
        let mut delegate_ctx = DelegateContext::new();
        self.try_strategies(&strategies, &pkg_with_repo, options, &mut delegate_ctx)
    }

    /// Get resolution strategies from routing table, or construct legacy fallback
    fn get_strategies_or_legacy(
        &self,
        pkg_with_repo: &PackageWithRepo,
        options: &ResolutionOptions,
    ) -> Result<Vec<ResolutionStrategy>> {
        let repo_id = pkg_with_repo.repository.id.ok_or_else(|| {
            Error::InitError("Repository missing ID".to_string())
        })?;

        // Check routing table first
        if let Some(resolution) = PackageResolution::find(
            self.conn,
            repo_id,
            &pkg_with_repo.package.name,
            options.version.as_deref(),
        )? {
            debug!(
                "Found routing entry for {} with {} strategies (primary: {:?})",
                pkg_with_repo.package.name,
                resolution.strategies.len(),
                resolution.primary_strategy
            );
            return Ok(resolution.strategies);
        }

        // Implicit legacy fallback - construct from repository_packages
        debug!(
            "No routing entry for {}, using legacy fallback",
            pkg_with_repo.package.name
        );

        let pkg_id = pkg_with_repo.package.id.ok_or_else(|| {
            Error::InitError("Package missing ID".to_string())
        })?;

        Ok(vec![ResolutionStrategy::Legacy {
            repository_package_id: pkg_id,
        }])
    }

    /// Try strategies in order until one succeeds
    fn try_strategies(
        &self,
        strategies: &[ResolutionStrategy],
        pkg_with_repo: &PackageWithRepo,
        options: &ResolutionOptions,
        delegate_ctx: &mut DelegateContext,
    ) -> Result<PackageSource> {
        let mut last_error = None;

        for (i, strategy) in strategies.iter().enumerate() {
            debug!(
                "Trying strategy {}/{} for {}: {:?}",
                i + 1,
                strategies.len(),
                pkg_with_repo.package.name,
                PrimaryStrategy::from(strategy)
            );

            match self.try_strategy(strategy, pkg_with_repo, options, delegate_ctx) {
                Ok(source) => {
                    info!(
                        "Strategy {:?} succeeded for {}",
                        PrimaryStrategy::from(strategy),
                        pkg_with_repo.package.name
                    );
                    // TODO: Cache if policy says to
                    // self.maybe_cache(&pkg_with_repo.package.name, &source, strategy)?;
                    return Ok(source);
                }
                Err(e) => {
                    warn!(
                        "Strategy {:?} failed for {}: {}",
                        PrimaryStrategy::from(strategy),
                        pkg_with_repo.package.name,
                        e
                    );
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            Error::ResolutionError(format!(
                "No resolution strategy available for {}",
                pkg_with_repo.package.name
            ))
        }))
    }

    /// Try a single resolution strategy
    fn try_strategy(
        &self,
        strategy: &ResolutionStrategy,
        pkg_with_repo: &PackageWithRepo,
        options: &ResolutionOptions,
        delegate_ctx: &mut DelegateContext,
    ) -> Result<PackageSource> {
        match strategy {
            ResolutionStrategy::Binary {
                url,
                checksum,
                delta_base,
            } => {
                self.try_binary(url, checksum, delta_base.as_deref(), pkg_with_repo, options)
            }

            ResolutionStrategy::Refinery {
                endpoint,
                distro,
                source_name,
            } => {
                let pkg_name = source_name.as_deref().unwrap_or(&pkg_with_repo.package.name);
                self.try_refinery(endpoint, distro, pkg_name, options)
            }

            ResolutionStrategy::Recipe {
                recipe_url,
                source_urls,
                patches,
            } => {
                self.try_recipe(recipe_url, source_urls, patches, options)
            }

            ResolutionStrategy::Delegate { label } => {
                delegate_ctx.enter(label)?;
                self.try_delegate(label, options, delegate_ctx)
            }

            ResolutionStrategy::Legacy {
                repository_package_id,
            } => {
                self.try_legacy(*repository_package_id, pkg_with_repo, options)
            }
        }
    }

    /// Try binary download strategy
    fn try_binary(
        &self,
        url: &str,
        checksum: &str,
        delta_base: Option<&str>,
        pkg_with_repo: &PackageWithRepo,
        options: &ResolutionOptions,
    ) -> Result<PackageSource> {
        let temp_dir = TempDir::new()
            .map_err(|e| Error::IoError(format!("Failed to create temp dir: {e}")))?;

        let output_dir = options
            .output_dir
            .as_deref()
            .unwrap_or(temp_dir.path());

        // TODO: Try delta first if base available
        if let Some(base) = delta_base {
            debug!("Delta base available: {}, but delta fetch not yet implemented", base);
        }

        // Construct a temporary RepositoryPackage for download
        let temp_pkg = RepositoryPackage {
            id: pkg_with_repo.package.id,
            repository_id: pkg_with_repo.package.repository_id,
            name: pkg_with_repo.package.name.clone(),
            version: pkg_with_repo.package.version.clone(),
            architecture: pkg_with_repo.package.architecture.clone(),
            description: pkg_with_repo.package.description.clone(),
            checksum: checksum.to_string(),
            size: pkg_with_repo.package.size,
            download_url: url.to_string(),
            dependencies: pkg_with_repo.package.dependencies.clone(),
            metadata: pkg_with_repo.package.metadata.clone(),
            synced_at: pkg_with_repo.package.synced_at.clone(),
            is_security_update: pkg_with_repo.package.is_security_update,
            severity: pkg_with_repo.package.severity.clone(),
            cve_ids: pkg_with_repo.package.cve_ids.clone(),
            advisory_id: pkg_with_repo.package.advisory_id.clone(),
            advisory_url: pkg_with_repo.package.advisory_url.clone(),
        };

        let path = download_package_verified(&temp_pkg, output_dir, options.gpg_options.as_ref())?;

        Ok(PackageSource::Binary {
            path,
            _temp_dir: Some(temp_dir),
        })
    }

    /// Try Refinery conversion strategy
    fn try_refinery(
        &self,
        endpoint: &str,
        distro: &str,
        name: &str,
        options: &ResolutionOptions,
    ) -> Result<PackageSource> {
        let temp_dir = TempDir::new()
            .map_err(|e| Error::IoError(format!("Failed to create temp dir: {e}")))?;

        let output_dir = options
            .output_dir
            .as_deref()
            .unwrap_or(temp_dir.path());

        let client = RefineryClient::new(endpoint)?;
        let path = client.fetch_package(distro, name, options.version.as_deref(), output_dir)?;

        Ok(PackageSource::Ccs {
            path,
            _temp_dir: Some(temp_dir),
        })
    }

    /// Try recipe build strategy
    fn try_recipe(
        &self,
        recipe_url: &str,
        _source_urls: &[String],
        _patches: &[String],
        options: &ResolutionOptions,
    ) -> Result<PackageSource> {
        let temp_dir = TempDir::new()
            .map_err(|e| Error::IoError(format!("Failed to create temp dir: {e}")))?;

        let output_dir = options
            .output_dir
            .as_deref()
            .unwrap_or(temp_dir.path());

        // Fetch the recipe file
        info!("Fetching recipe from: {}", recipe_url);
        let recipe_content = fetch_url_content(recipe_url)?;

        // Parse the recipe
        let recipe = parse_recipe(&recipe_content)
            .map_err(|e| Error::ParseError(format!("Failed to parse recipe: {}", e)))?;

        info!("Cooking {} from recipe", recipe.package.name);

        // Configure and run the kitchen
        let config = KitchenConfig::default();
        let kitchen = Kitchen::new(config);

        let result = kitchen.cook(&recipe, output_dir)
            .map_err(|e| Error::IoError(format!("Recipe cooking failed: {}", e)))?;

        Ok(PackageSource::Ccs {
            path: result.package_path,
            _temp_dir: Some(temp_dir),
        })
    }

    /// Try delegate strategy (federation)
    fn try_delegate(
        &self,
        label: &str,
        _options: &ResolutionOptions,
        _delegate_ctx: &mut DelegateContext,
    ) -> Result<PackageSource> {
        // Label delegation is Phase 4 - not yet implemented
        // Would parse label, resolve through label chain
        Err(Error::NotImplemented(format!(
            "Label delegation to '{}' is not yet implemented",
            label
        )))
    }

    /// Try legacy strategy (existing repository_packages)
    fn try_legacy(
        &self,
        _repository_package_id: i64,
        pkg_with_repo: &PackageWithRepo,
        options: &ResolutionOptions,
    ) -> Result<PackageSource> {
        let temp_dir = TempDir::new()
            .map_err(|e| Error::IoError(format!("Failed to create temp dir: {e}")))?;

        let output_dir = options
            .output_dir
            .as_deref()
            .unwrap_or(temp_dir.path());

        // Use the package info we already have
        let path = download_package_verified(
            &pkg_with_repo.package,
            output_dir,
            options.gpg_options.as_ref(),
        )?;

        Ok(PackageSource::Binary {
            path,
            _temp_dir: Some(temp_dir),
        })
    }
}

/// Convenience function for resolving a package
pub fn resolve_package(
    conn: &Connection,
    name: &str,
    options: &ResolutionOptions,
) -> Result<PackageSource> {
    let resolver = PackageResolver::new(conn);
    resolver.resolve(name, options)
}

/// Build GPG verification options for a repository
pub fn build_gpg_options(repo: &Repository, keyring_dir: &Path) -> Option<DownloadOptions> {
    if repo.gpg_check {
        Some(DownloadOptions {
            gpg_check: true,
            gpg_strict: repo.gpg_strict,
            keyring_dir: keyring_dir.to_path_buf(),
            repository_name: repo.name.clone(),
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    fn create_test_repo(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('test-repo', 'https://example.com', 1, 10)",
            [],
        ).unwrap();
        conn.last_insert_rowid()
    }

    fn create_test_package(conn: &Connection, repo_id: i64, name: &str, version: &str) -> i64 {
        conn.execute(
            "INSERT INTO repository_packages
             (repository_id, name, version, checksum, size, download_url)
             VALUES (?1, ?2, ?3, 'sha256:abc123', 1024, 'https://example.com/pkg.rpm')",
            rusqlite::params![repo_id, name, version],
        ).unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn test_delegate_context_depth_limit() {
        let mut ctx = DelegateContext::new();

        for i in 0..MAX_DELEGATE_DEPTH {
            ctx.enter(&format!("label{}", i)).unwrap();
        }

        // Should fail at max depth
        let result = ctx.enter("too_deep");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too deep"));
    }

    #[test]
    fn test_delegate_context_cycle_detection() {
        let mut ctx = DelegateContext::new();

        ctx.enter("label_a").unwrap();
        ctx.enter("label_b").unwrap();

        // Should fail on cycle
        let result = ctx.enter("label_a");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cycle"));
    }

    #[test]
    fn test_resolution_options_to_selection_options() {
        let res_options = ResolutionOptions {
            version: Some("1.0.0".to_string()),
            repository: Some("test-repo".to_string()),
            architecture: Some("x86_64".to_string()),
            output_dir: None,
            gpg_options: None,
            skip_cas: false,
        };

        let sel_options = res_options.to_selection_options();
        assert_eq!(sel_options.version, Some("1.0.0".to_string()));
        assert_eq!(sel_options.repository, Some("test-repo".to_string()));
        assert_eq!(sel_options.architecture, Some("x86_64".to_string()));
    }

    #[test]
    fn test_get_strategies_legacy_fallback() {
        let (_temp, conn) = create_test_db();
        let repo_id = create_test_repo(&conn);
        let _pkg_id = create_test_package(&conn, repo_id, "nginx", "1.24.0");

        // Find package
        let pkg_with_repo = PackageSelector::find_best_package(
            &conn,
            "nginx",
            &SelectionOptions::default(),
        ).unwrap();

        // Get strategies - should fall back to legacy since no routing entry
        let resolver = PackageResolver::new(&conn);
        let options = ResolutionOptions::default();
        let strategies = resolver.get_strategies_or_legacy(&pkg_with_repo, &options).unwrap();

        assert_eq!(strategies.len(), 1);
        assert!(matches!(strategies[0], ResolutionStrategy::Legacy { .. }));
    }

    #[test]
    fn test_get_strategies_from_routing_table() {
        let (_temp, conn) = create_test_db();
        let repo_id = create_test_repo(&conn);
        let _pkg_id = create_test_package(&conn, repo_id, "nginx", "1.24.0");

        // Create routing entry
        let mut resolution = PackageResolution::refinery(
            repo_id,
            "nginx".to_string(),
            "https://refinery.example.com".to_string(),
            "fedora".to_string(),
        );
        resolution.insert(&conn).unwrap();

        // Find package
        let pkg_with_repo = PackageSelector::find_best_package(
            &conn,
            "nginx",
            &SelectionOptions::default(),
        ).unwrap();

        // Get strategies - should use routing entry
        let resolver = PackageResolver::new(&conn);
        let options = ResolutionOptions::default();
        let strategies = resolver.get_strategies_or_legacy(&pkg_with_repo, &options).unwrap();

        assert_eq!(strategies.len(), 1);
        assert!(matches!(strategies[0], ResolutionStrategy::Refinery { .. }));
    }

    #[test]
    fn test_package_source_path() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.ccs");
        std::fs::write(&path, "test").unwrap();

        let binary = PackageSource::Binary {
            path: path.clone(),
            _temp_dir: None,
        };
        assert_eq!(binary.path(), Some(path.as_path()));

        let ccs = PackageSource::Ccs {
            path: path.clone(),
            _temp_dir: None,
        };
        assert_eq!(ccs.path(), Some(path.as_path()));

        let cas = PackageSource::LocalCas {
            hash: "sha256:abc".to_string(),
        };
        assert_eq!(cas.path(), None);
    }
}
