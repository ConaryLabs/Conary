// conary-core/src/repository/resolution.rs

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
    LabelEntry, PackageResolution, PrimaryStrategy, Repository, RepositoryPackage,
    ResolutionStrategy, Trove,
};
use crate::error::{Error, Result};
use crate::label::Label;
use crate::recipe::{Kitchen, KitchenConfig, parse_recipe};
use crate::repository::client::RepositoryClient;
use crate::repository::dependency_model::RepositoryDependencyFlavor;
use crate::repository::remi::RemiClient;
use crate::repository::resolution_policy::ResolutionPolicy;
use crate::repository::selector::{PackageSelector, PackageWithRepo, SelectionOptions};
use crate::repository::{DownloadOptions, download_package_verified};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tracing::{debug, info, warn};

/// Maximum depth for delegate chain resolution
const MAX_DELEGATE_DEPTH: usize = 10;

/// Fetch content from a URL as a string
///
/// Uses reqwest via `RepositoryClient` to download the content. Supports HTTP(S) URLs
/// with automatic redirect following and retry support.
fn fetch_url_content(url: &str) -> Result<String> {
    debug!("Fetching content from: {}", url);

    let client = RepositoryClient::new()?;
    let bytes = client.download_to_bytes(url)?;

    String::from_utf8(bytes)
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
    /// Resolution policy controlling cross-distro selection.
    pub policy: Option<ResolutionPolicy>,
    /// Whether this resolution is for a root (user-typed) request.
    pub is_root: bool,
    /// The primary distro flavor of the system (for mixing policy checks).
    pub primary_flavor: Option<RepositoryDependencyFlavor>,
}

impl ResolutionOptions {
    /// Convert to SelectionOptions for repository selection
    pub fn to_selection_options(&self) -> SelectionOptions {
        SelectionOptions {
            version: self.version.clone(),
            repository: self.repository.clone(),
            architecture: self.architecture.clone(),
            policy: self.policy.clone(),
            is_root: self.is_root,
            primary_flavor: self.primary_flavor,
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
    /// CCS package from Remi
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
#[derive(Default)]
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

fn effective_remi_version<'a>(
    pkg_with_repo: &'a PackageWithRepo,
    options: &'a ResolutionOptions,
) -> Option<&'a str> {
    options
        .version
        .as_deref()
        .or(Some(pkg_with_repo.package.version.as_str()))
}

/// Create a temp directory and resolve the output directory from options.
///
/// Returns `(temp_dir, output_dir)` where `output_dir` is either the user-specified
/// output directory or the temp directory path.
fn create_output_dir(options: &ResolutionOptions) -> Result<(TempDir, PathBuf)> {
    let temp_dir =
        TempDir::new().map_err(|e| Error::IoError(format!("Failed to create temp dir: {e}")))?;
    let output_dir = options
        .output_dir
        .clone()
        .unwrap_or_else(|| temp_dir.path().to_path_buf());
    Ok((temp_dir, output_dir))
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
    /// 1. Check if package is already installed locally (skip with skip_cas option)
    /// 2. Repository selection (using existing priority logic)
    /// 3. Strategy lookup from routing table (with implicit legacy fallback)
    /// 4. Strategy execution in priority order
    pub fn resolve(&self, name: &str, options: &ResolutionOptions) -> Result<PackageSource> {
        // Step 0: Check if already installed locally
        if !options.skip_cas
            && let Some(installed) = self.check_installed(name, options)?
        {
            return Ok(installed);
        }

        // Step 1: Repository selection
        let pkg_with_repo =
            PackageSelector::find_best_package(self.conn, name, &options.to_selection_options())?;

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

    /// Get resolution strategies from routing table, repo default, or legacy fallback
    fn get_strategies_or_legacy(
        &self,
        pkg_with_repo: &PackageWithRepo,
        options: &ResolutionOptions,
    ) -> Result<Vec<ResolutionStrategy>> {
        let repo_id = pkg_with_repo
            .repository
            .id
            .ok_or_else(|| Error::InitError("Repository missing ID".to_string()))?;

        // Check routing table first (per-package routing)
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

        // Check repository's default strategy
        if let Some(ref strategy) = pkg_with_repo.repository.default_strategy {
            debug!(
                "No routing entry for {}, using repo default strategy: {}",
                pkg_with_repo.package.name, strategy
            );

            match strategy.as_str() {
                "remi" => {
                    // Construct Remi strategy from repo config
                    let endpoint = pkg_with_repo.repository.default_strategy_endpoint.clone()
                        .ok_or_else(|| Error::ConfigError(
                            format!("Repository '{}' has default_strategy=remi but no endpoint configured",
                                pkg_with_repo.repository.name)
                        ))?;
                    let distro = pkg_with_repo.repository.default_strategy_distro.clone()
                        .ok_or_else(|| Error::ConfigError(
                            format!("Repository '{}' has default_strategy=remi but no distro configured",
                                pkg_with_repo.repository.name)
                        ))?;

                    return Ok(vec![ResolutionStrategy::Remi {
                        endpoint,
                        distro,
                        source_name: None, // Use package name as-is
                    }]);
                }
                "binary" | "legacy" => {
                    // Fall through to legacy handling below
                }
                other => {
                    warn!(
                        "Unknown default_strategy '{}' for repo '{}', falling back to legacy",
                        other, pkg_with_repo.repository.name
                    );
                }
            }
        }

        // Implicit legacy fallback - construct from repository_packages
        debug!(
            "No routing entry for {}, using legacy fallback",
            pkg_with_repo.package.name
        );

        let pkg_id = pkg_with_repo
            .package
            .id
            .ok_or_else(|| Error::InitError("Package missing ID".to_string()))?;

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
            } => self.try_binary(url, checksum, delta_base.as_deref(), pkg_with_repo, options),

            ResolutionStrategy::Remi {
                endpoint,
                distro,
                source_name,
            } => {
                let pkg_name = source_name
                    .as_deref()
                    .unwrap_or(&pkg_with_repo.package.name);
                self.try_remi(
                    endpoint,
                    distro,
                    pkg_name,
                    effective_remi_version(pkg_with_repo, options),
                    options,
                )
            }

            ResolutionStrategy::Recipe {
                recipe_url,
                source_urls,
                patches,
            } => self.try_recipe(recipe_url, source_urls, patches, options),

            ResolutionStrategy::Delegate { label } => {
                delegate_ctx.enter(label)?;
                self.try_delegate(label, &pkg_with_repo.package.name, options, delegate_ctx)
            }

            ResolutionStrategy::Legacy {
                repository_package_id,
            } => self.try_legacy(*repository_package_id, pkg_with_repo, options),
        }
    }

    /// Check if package is already installed locally
    ///
    /// Returns `Some(LocalCas)` if the package is installed with a matching version,
    /// `None` if not installed or version doesn't match.
    fn check_installed(
        &self,
        name: &str,
        options: &ResolutionOptions,
    ) -> Result<Option<PackageSource>> {
        let installed = Trove::find_by_name(self.conn, name)?;

        if installed.is_empty() {
            debug!("Package {} not installed locally", name);
            return Ok(None);
        }

        // Check if any installed version matches the requested version
        for trove in &installed {
            let version_matches = match &options.version {
                // Specific version requested - must match exactly
                Some(requested) => &trove.version == requested,
                // No specific version - any installed version counts
                None => true,
            };

            if version_matches {
                info!(
                    "Package {} {} already installed locally (trove_id: {:?})",
                    trove.name, trove.version, trove.id
                );

                // Return a LocalCas source with identifier for the installed package
                // Format: "installed:{name}:{version}" allows downstream to identify this
                let hash = format!("installed:{}:{}", trove.name, trove.version);
                return Ok(Some(PackageSource::LocalCas { hash }));
            }
        }

        // Package is installed but with a different version
        if let Some(requested) = &options.version {
            debug!(
                "Package {} installed but version {} requested (have: {:?})",
                name,
                requested,
                installed.iter().map(|t| &t.version).collect::<Vec<_>>()
            );
        }

        Ok(None)
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
        let (temp_dir, output_dir) = create_output_dir(options)?;

        // TODO: Try delta first if base available
        if let Some(base) = delta_base {
            debug!(
                "Delta base available: {}, but delta fetch not yet implemented",
                base
            );
        }

        // Construct a temporary RepositoryPackage with overridden URL and checksum
        let temp_pkg = RepositoryPackage {
            checksum: checksum.to_string(),
            download_url: url.to_string(),
            ..pkg_with_repo.package.clone()
        };

        let path = download_package_verified(&temp_pkg, &output_dir, options.gpg_options.as_ref())?;

        Ok(PackageSource::Binary {
            path,
            _temp_dir: Some(temp_dir),
        })
    }

    /// Try Remi conversion strategy
    fn try_remi(
        &self,
        endpoint: &str,
        distro: &str,
        name: &str,
        version: Option<&str>,
        options: &ResolutionOptions,
    ) -> Result<PackageSource> {
        let (temp_dir, output_dir) = create_output_dir(options)?;

        let client = RemiClient::new(endpoint)?;
        let path = client.fetch_package(distro, name, version, &output_dir)?;

        Ok(PackageSource::Ccs {
            path,
            _temp_dir: Some(temp_dir),
        })
    }

    /// Try recipe build strategy
    fn try_recipe(
        &self,
        recipe_url: &str,
        source_urls: &[String],
        patches: &[String],
        options: &ResolutionOptions,
    ) -> Result<PackageSource> {
        // TODO: source_urls and patches are part of the resolution strategy schema
        // but not yet wired into Kitchen. They would allow pre-fetching sources and
        // applying patches before cooking. For now, the recipe itself specifies sources.
        if !source_urls.is_empty() {
            debug!(
                "Recipe strategy includes {} source URLs (not yet implemented, recipe defines its own sources)",
                source_urls.len()
            );
        }
        if !patches.is_empty() {
            debug!(
                "Recipe strategy includes {} patches (not yet implemented)",
                patches.len()
            );
        }

        let (temp_dir, output_dir) = create_output_dir(options)?;

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

        let result = kitchen
            .cook(&recipe, &output_dir)
            .map_err(|e| Error::IoError(format!("Recipe cooking failed: {}", e)))?;

        Ok(PackageSource::Ccs {
            path: result.package_path,
            _temp_dir: Some(temp_dir),
        })
    }

    /// Try delegate strategy (federation)
    ///
    /// Resolves a package through a label chain. The label can either:
    /// 1. Delegate to another label (chain continues)
    /// 2. Link to a repository (resolution happens through that repo)
    fn try_delegate(
        &self,
        label_str: &str,
        package_name: &str,
        options: &ResolutionOptions,
        delegate_ctx: &mut DelegateContext,
    ) -> Result<PackageSource> {
        info!(
            "Resolving '{}' through label delegation: {}",
            package_name, label_str
        );

        // Parse the label string
        let label_spec = Label::parse(label_str)
            .map_err(|e| Error::ParseError(format!("Invalid label '{}': {}", label_str, e)))?;

        // Look up the label in the database
        let label_entry = LabelEntry::find_by_spec(
            self.conn,
            &label_spec.repository,
            &label_spec.namespace,
            &label_spec.tag,
        )?
        .ok_or_else(|| Error::NotFound(format!("Label '{}' not found in database", label_str)))?;

        // Check for delegation chain
        if let Some(delegate_to_id) = label_entry.delegate_to_label_id {
            // Get the target label
            let target_label =
                LabelEntry::find_by_id(self.conn, delegate_to_id)?.ok_or_else(|| {
                    Error::NotFound(format!(
                        "Delegation target label (id={}) not found",
                        delegate_to_id
                    ))
                })?;

            let target_label_str = target_label.to_string();
            debug!(
                "Label {} delegates to {} for package {}",
                label_str, target_label_str, package_name
            );

            // Recursively resolve through the target label
            // DelegateContext tracks depth and visited labels for cycle detection
            return self.try_delegate(&target_label_str, package_name, options, delegate_ctx);
        }

        // Check for repository link
        if let Some(repo_id) = label_entry.repository_id {
            debug!(
                "Label {} links to repository id={} for package {}",
                label_str, repo_id, package_name
            );

            // Get the repository
            let repo = Repository::find_by_id(self.conn, repo_id)?.ok_or_else(|| {
                Error::NotFound(format!(
                    "Repository (id={}) linked from label '{}' not found",
                    repo_id, label_str
                ))
            })?;

            info!(
                "Resolving '{}' through repository '{}' via label '{}'",
                package_name, repo.name, label_str
            );

            // Create options that force resolution through this specific repository
            let mut repo_options = options.clone();
            repo_options.repository = Some(repo.name.clone());

            // Use the package selector to find the package in this repository
            let pkg_with_repo = PackageSelector::find_best_package(
                self.conn,
                package_name,
                &repo_options.to_selection_options(),
            )?;

            // Get strategies for this package in the target repository
            let strategies = self.get_strategies_or_legacy(&pkg_with_repo, &repo_options)?;

            // Try strategies (but skip Delegate to avoid infinite loops - we're already delegating)
            for strategy in &strategies {
                if matches!(strategy, ResolutionStrategy::Delegate { .. }) {
                    debug!("Skipping nested delegation to prevent loops");
                    continue;
                }

                match self.try_strategy(strategy, &pkg_with_repo, &repo_options, delegate_ctx) {
                    Ok(source) => return Ok(source),
                    Err(e) => {
                        debug!("Strategy failed in delegated repository: {}", e);
                        continue;
                    }
                }
            }

            return Err(Error::ResolutionError(format!(
                "Package '{}' not resolvable through label '{}' (repository '{}')",
                package_name, label_str, repo.name
            )));
        }

        // Label has neither delegation nor repository link
        Err(Error::ResolutionError(format!(
            "Label '{}' has no delegation target and no linked repository. \
             Configure with 'conary label-link' or 'conary label-delegate'.",
            label_str
        )))
    }

    /// Try legacy strategy (existing repository_packages)
    ///
    /// The `repository_package_id` is part of the `ResolutionStrategy::Legacy` variant
    /// for schema completeness, but the actual package data is already available in
    /// `pkg_with_repo` from the earlier repository selection step.
    fn try_legacy(
        &self,
        _repository_package_id: i64,
        pkg_with_repo: &PackageWithRepo,
        options: &ResolutionOptions,
    ) -> Result<PackageSource> {
        let (temp_dir, output_dir) = create_output_dir(options)?;

        // Use the package info we already have from repository selection
        let path = download_package_verified(
            &pkg_with_repo.package,
            &output_dir,
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
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn create_test_package(conn: &Connection, repo_id: i64, name: &str, version: &str) -> i64 {
        conn.execute(
            "INSERT INTO repository_packages
             (repository_id, name, version, checksum, size, download_url)
             VALUES (?1, ?2, ?3, 'sha256:abc123', 1024, 'https://example.com/pkg.rpm')",
            rusqlite::params![repo_id, name, version],
        )
        .unwrap();
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
            policy: None,
            is_root: false,
            primary_flavor: None,
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
        let pkg_with_repo =
            PackageSelector::find_best_package(&conn, "nginx", &SelectionOptions::default())
                .unwrap();

        // Get strategies - should fall back to legacy since no routing entry
        let resolver = PackageResolver::new(&conn);
        let options = ResolutionOptions::default();
        let strategies = resolver
            .get_strategies_or_legacy(&pkg_with_repo, &options)
            .unwrap();

        assert_eq!(strategies.len(), 1);
        assert!(matches!(strategies[0], ResolutionStrategy::Legacy { .. }));
    }

    #[test]
    fn test_get_strategies_from_routing_table() {
        let (_temp, conn) = create_test_db();
        let repo_id = create_test_repo(&conn);
        let _pkg_id = create_test_package(&conn, repo_id, "nginx", "1.24.0");

        // Create routing entry
        let mut resolution = PackageResolution::remi(
            repo_id,
            "nginx".to_string(),
            "https://remi.example.com".to_string(),
            "fedora".to_string(),
        );
        resolution.insert(&conn).unwrap();

        // Find package
        let pkg_with_repo =
            PackageSelector::find_best_package(&conn, "nginx", &SelectionOptions::default())
                .unwrap();

        // Get strategies - should use routing entry
        let resolver = PackageResolver::new(&conn);
        let options = ResolutionOptions::default();
        let strategies = resolver
            .get_strategies_or_legacy(&pkg_with_repo, &options)
            .unwrap();

        assert_eq!(strategies.len(), 1);
        assert!(matches!(strategies[0], ResolutionStrategy::Remi { .. }));
    }

    #[test]
    fn test_effective_remi_version_defaults_to_selected_repo_version() {
        let (_temp, conn) = create_test_db();
        let repo_id = create_test_repo(&conn);
        let _pkg_id = create_test_package(&conn, repo_id, "nginx", "1.24.0");

        let pkg_with_repo =
            PackageSelector::find_best_package(&conn, "nginx", &SelectionOptions::default())
                .unwrap();

        let options = ResolutionOptions::default();
        assert_eq!(
            effective_remi_version(&pkg_with_repo, &options),
            Some("1.24.0")
        );
    }

    #[test]
    fn test_effective_remi_version_prefers_explicit_request() {
        let (_temp, conn) = create_test_db();
        let repo_id = create_test_repo(&conn);
        let _pkg_id = create_test_package(&conn, repo_id, "nginx", "1.24.0");

        let pkg_with_repo =
            PackageSelector::find_best_package(&conn, "nginx", &SelectionOptions::default())
                .unwrap();

        let options = ResolutionOptions {
            version: Some("1.25.0".to_string()),
            ..Default::default()
        };
        assert_eq!(
            effective_remi_version(&pkg_with_repo, &options),
            Some("1.25.0")
        );
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

    #[test]
    fn test_check_installed_not_installed() {
        let (_temp, conn) = create_test_db();
        let resolver = PackageResolver::new(&conn);
        let options = ResolutionOptions::default();

        // Package not installed - should return None
        let result = resolver.check_installed("nginx", &options).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_check_installed_matching_version() {
        let (_temp, conn) = create_test_db();

        // Insert an installed trove
        conn.execute(
            "INSERT INTO troves (name, version, type, install_source, install_reason)
             VALUES ('nginx', '1.24.0', 'package', 'repository', 'explicit')",
            [],
        )
        .unwrap();

        let resolver = PackageResolver::new(&conn);

        // No version specified - should match
        let options = ResolutionOptions::default();
        let result = resolver.check_installed("nginx", &options).unwrap();
        assert!(result.is_some());
        if let Some(PackageSource::LocalCas { hash }) = result {
            assert!(hash.starts_with("installed:nginx:1.24.0"));
        } else {
            panic!("Expected LocalCas source");
        }

        // Matching version specified - should match
        let options = ResolutionOptions {
            version: Some("1.24.0".to_string()),
            ..Default::default()
        };
        let result = resolver.check_installed("nginx", &options).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn test_check_installed_different_version() {
        let (_temp, conn) = create_test_db();

        // Insert an installed trove
        conn.execute(
            "INSERT INTO troves (name, version, type, install_source, install_reason)
             VALUES ('nginx', '1.24.0', 'package', 'repository', 'explicit')",
            [],
        )
        .unwrap();

        let resolver = PackageResolver::new(&conn);

        // Different version requested - should NOT match
        let options = ResolutionOptions {
            version: Some("1.25.0".to_string()),
            ..Default::default()
        };
        let result = resolver.check_installed("nginx", &options).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_check_installed_skip_cas() {
        let (_temp, conn) = create_test_db();

        // Insert an installed trove
        conn.execute(
            "INSERT INTO troves (name, version, type, install_source, install_reason)
             VALUES ('nginx', '1.24.0', 'package', 'repository', 'explicit')",
            [],
        )
        .unwrap();

        let resolver = PackageResolver::new(&conn);

        // With skip_cas=true, check_installed is not called (tested at resolve level)
        // But we can verify the method itself still works when called directly
        let options = ResolutionOptions {
            skip_cas: true, // Note: this doesn't affect check_installed directly
            ..Default::default()
        };
        let result = resolver.check_installed("nginx", &options).unwrap();
        assert!(result.is_some()); // Method still finds it
    }
}
