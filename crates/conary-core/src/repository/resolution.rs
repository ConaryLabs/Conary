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
//! Check exact installed state ─────────> Match ─> Return Installed
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
//! Select repository package                 No, try next
//! from repository_packages                  |
//!     |                                     v
//!     v                               All strategies failed ──> Error
//! Download selected repository package
//! ```
//!
//! # Repository Package Default
//!
//! When no `package_resolution` entry exists for a package, the resolver
//! constructs a `RepositoryPackage` strategy from the exact
//! `repository_packages` row selected in step one.
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
//! }
//! ```

use crate::db::models::{
    LabelEntry, PackageResolution, PrimaryStrategy, Repository, RepositoryPackage,
    ResolutionStrategy, Trove,
};
use crate::error::{Error, Result};
use crate::label::Label;
use crate::repository::dependency_model::RepositoryDependencyFlavor;
use crate::repository::remi::RemiClient;
use crate::repository::resolution_policy::ResolutionPolicy;
use crate::repository::selector::{PackageSelector, PackageWithRepo, SelectionOptions};
use crate::repository::versioning::{VersionScheme, resolve_package_version_scheme};
use crate::repository::{
    DownloadOptions, download_package_verified, download_static_package_verified,
};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tracing::{debug, info, warn};

/// Maximum depth for delegate chain resolution
const MAX_DELEGATE_DEPTH: usize = 10;

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
    /// Whether to skip the exact-installed-state short circuit
    pub skip_installed: bool,
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
        /// Repository metadata for the selected package, when resolution came
        /// from synced repository metadata.
        repository_provenance: Option<RepositorySourceMetadata>,
    },
    /// CCS package from Remi
    Ccs {
        path: PathBuf,
        _temp_dir: Option<TempDir>,
        /// Repository metadata for the selected package, when a synced
        /// repository routed the install through Remi/CCS conversion.
        repository_provenance: Option<RepositorySourceMetadata>,
    },
    /// Exact installed package identity selected from Conary's persisted state.
    Installed {
        name: String,
        version: String,
        architecture: Option<String>,
        trove_id: Option<i64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositorySourceKind {
    Native,
    Remi,
    Static,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositorySourceMetadata {
    pub repository_id: i64,
    pub source_distro: Option<String>,
    pub version_scheme: VersionScheme,
    pub source_kind: RepositorySourceKind,
}

impl PackageSource {
    /// Get the path to the package file
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Binary { path, .. } => Some(path),
            Self::Ccs { path, .. } => Some(path),
            Self::Installed { .. } => None,
        }
    }
}

/// Context for delegate chain resolution (cycle detection).
///
/// Tracks delegation depth and visited labels.  The `exit()` method must be
/// called after each `enter()` (whether the delegation succeeds or fails) to
/// ensure failed branches do not permanently consume depth or visited state.
#[derive(Default)]
struct DelegateContext {
    depth: usize,
    visited: std::collections::HashSet<String>,
}

impl DelegateContext {
    fn new() -> Self {
        Self::default()
    }

    /// Enter a new delegation level, checking depth and cycle constraints.
    ///
    /// The caller **must** call [`exit`] with the same label when the
    /// delegation branch completes (success or failure).
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

    /// Unwind one delegation level, removing the label from the visited set.
    ///
    /// This must be called after every successful `enter()`, regardless of
    /// whether the delegation branch succeeded or failed.
    fn exit(&mut self, label: &str) {
        self.depth = self.depth.saturating_sub(1);
        self.visited.remove(label);
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
    /// 1. Check if package is already installed locally unless explicitly skipped
    /// 2. Repository selection (using existing priority logic)
    /// 3. Strategy lookup from routing table or exact repository-package default
    /// 4. Strategy execution in priority order
    pub async fn resolve(&self, name: &str, options: &ResolutionOptions) -> Result<PackageSource> {
        // Step 0: Check if already installed locally
        if !options.skip_installed
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
        let strategies = self.get_strategies(&pkg_with_repo, options)?;

        // Step 3: Try each strategy in order
        let mut delegate_ctx = DelegateContext::new();
        self.try_strategies(&strategies, &pkg_with_repo, options, &mut delegate_ctx)
            .await
    }

    /// Get resolution strategies from routing table or the repository contract.
    fn get_strategies(
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
                "binary" | "static" => {}
                other => {
                    return Err(Error::ConfigError(format!(
                        "Repository '{}' declares unsupported default strategy '{}'",
                        pkg_with_repo.repository.name, other
                    )));
                }
            }
        }

        // The package selector already established this exact source row.
        debug!(
            "No routing entry for {}, using selected repository package",
            pkg_with_repo.package.name
        );

        let pkg_id = pkg_with_repo
            .package
            .id
            .ok_or_else(|| Error::InitError("Package missing ID".to_string()))?;

        Ok(vec![ResolutionStrategy::RepositoryPackage {
            repository_package_id: pkg_id,
        }])
    }

    /// Try strategies in order until one succeeds
    async fn try_strategies(
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

            match Box::pin(self.try_strategy(strategy, pkg_with_repo, options, delegate_ctx)).await
            {
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
    async fn try_strategy(
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
                    .await
            }

            ResolutionStrategy::Remi {
                endpoint,
                distro,
                source_name,
            } => {
                self.try_remi(
                    endpoint,
                    distro,
                    source_name.as_deref(),
                    pkg_with_repo,
                    options,
                )
                .await
            }

            ResolutionStrategy::Delegate { label } => {
                delegate_ctx.enter(label)?;
                let result = self
                    .try_delegate(label, &pkg_with_repo.package.name, options, delegate_ctx)
                    .await;
                delegate_ctx.exit(label);
                result
            }

            ResolutionStrategy::RepositoryPackage {
                repository_package_id,
            } => {
                self.try_repository_package(*repository_package_id, pkg_with_repo, options)
                    .await
            }
        }
    }

    /// Check if package is already installed locally
    ///
    /// Returns `Some(Installed)` if the package is installed with a matching
    /// version, `None` if not installed or the version does not match.
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
            let architecture_matches = options.architecture.as_deref().is_none_or(|requested| {
                PackageSelector::is_architecture_compatible(
                    trove.architecture.as_deref(),
                    requested,
                )
            });
            let version_matches = match &options.version {
                // Specific version requested - must match exactly
                Some(requested) => &trove.version == requested,
                // No specific version - any installed version counts
                None => true,
            };

            if architecture_matches && version_matches {
                info!(
                    "Package {} {} already installed locally (trove_id: {:?})",
                    trove.name, trove.version, trove.id
                );

                return Ok(Some(PackageSource::Installed {
                    name: trove.name.clone(),
                    version: trove.version.clone(),
                    architecture: trove.architecture.clone(),
                    trove_id: trove.id,
                }));
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
    async fn try_binary(
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

        let trust = build_download_options(self.conn, &pkg_with_repo.repository)?;
        let path = download_package_verified(&temp_pkg, &output_dir, &trust).await?;

        Ok(PackageSource::Binary {
            path,
            _temp_dir: Some(temp_dir),
            repository_provenance: Some(repository_source_metadata(pkg_with_repo)),
        })
    }

    /// Try Remi conversion strategy
    async fn try_remi(
        &self,
        endpoint: &str,
        distro: &str,
        source_name: Option<&str>,
        pkg_with_repo: &PackageWithRepo,
        options: &ResolutionOptions,
    ) -> Result<PackageSource> {
        let (temp_dir, output_dir) = create_output_dir(options)?;
        let name = source_name.unwrap_or(&pkg_with_repo.package.name);
        let version = effective_remi_version(pkg_with_repo, options);
        let architecture = pkg_with_repo.package.architecture.as_deref();

        let profile = crate::repository::supported_profiles::profile_for_remi_target(distro)
            .ok_or_else(|| Error::ConfigError(format!("unsupported Remi target: {distro}")))?;

        let client = RemiClient::new(endpoint)?;
        let path = client
            .fetch_package(
                profile.remi_route_slug(),
                name,
                version,
                architecture,
                &output_dir,
            )
            .await?;

        Ok(PackageSource::Ccs {
            path,
            _temp_dir: Some(temp_dir),
            repository_provenance: Some(repository_source_metadata(pkg_with_repo)),
        })
    }

    /// Try delegate strategy (federation)
    ///
    /// Resolves a package through a label chain. The label can either:
    /// 1. Delegate to another label (chain continues)
    /// 2. Link to a repository (resolution happens through that repo)
    async fn try_delegate(
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

            // Recursively resolve through the target label.
            // DelegateContext tracks depth and visited labels for cycle detection.
            // enter/exit bracket the recursive call so failed branches unwind.
            delegate_ctx.enter(&target_label_str)?;
            let result =
                Box::pin(self.try_delegate(&target_label_str, package_name, options, delegate_ctx))
                    .await;
            delegate_ctx.exit(&target_label_str);
            return result;
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
            let strategies = self.get_strategies(&pkg_with_repo, &repo_options)?;

            // Try strategies (but skip Delegate to avoid infinite loops - we're already delegating)
            for strategy in &strategies {
                if matches!(strategy, ResolutionStrategy::Delegate { .. }) {
                    debug!("Skipping nested delegation to prevent loops");
                    continue;
                }

                match Box::pin(self.try_strategy(
                    strategy,
                    &pkg_with_repo,
                    &repo_options,
                    delegate_ctx,
                ))
                .await
                {
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

    /// Download the exact repository package selected earlier.
    ///
    /// The `repository_package_id` is part of the persisted strategy for
    /// structural identity; package data is already available in
    /// `pkg_with_repo` from repository selection.
    async fn try_repository_package(
        &self,
        _repository_package_id: i64,
        pkg_with_repo: &PackageWithRepo,
        options: &ResolutionOptions,
    ) -> Result<PackageSource> {
        let (temp_dir, output_dir) = create_output_dir(options)?;

        // Use the package info we already have from repository selection
        let path = download_verified_repository_package(
            &pkg_with_repo.package,
            &output_dir,
            pkg_with_repo,
            self.conn,
        )
        .await?;

        Ok(PackageSource::Binary {
            path,
            _temp_dir: Some(temp_dir),
            repository_provenance: Some(repository_source_metadata(pkg_with_repo)),
        })
    }
}

fn repository_source_metadata(pkg_with_repo: &PackageWithRepo) -> RepositorySourceMetadata {
    RepositorySourceMetadata {
        repository_id: pkg_with_repo.package.repository_id,
        source_distro: pkg_with_repo
            .package
            .distro
            .clone()
            .or_else(|| pkg_with_repo.repository.default_strategy_distro.clone()),
        version_scheme: resolve_package_version_scheme(&pkg_with_repo.package),
        source_kind: selected_repository_source_kind(pkg_with_repo),
    }
}

async fn download_verified_repository_package(
    package: &RepositoryPackage,
    output_dir: &Path,
    pkg_with_repo: &PackageWithRepo,
    conn: &Connection,
) -> Result<PathBuf> {
    if selected_repository_source_kind(pkg_with_repo) == RepositorySourceKind::Static {
        download_static_package_verified(package, output_dir, None).await
    } else {
        let trust = build_download_options(conn, &pkg_with_repo.repository)?;
        download_package_verified(package, output_dir, &trust).await
    }
}

fn selected_repository_source_kind(pkg_with_repo: &PackageWithRepo) -> RepositorySourceKind {
    match pkg_with_repo.repository.default_strategy.as_deref() {
        Some("static") => RepositorySourceKind::Static,
        Some("remi") => RepositorySourceKind::Remi,
        _ => RepositorySourceKind::Native,
    }
}

/// Convenience function for resolving a package
pub async fn resolve_package(
    conn: &Connection,
    name: &str,
    options: &ResolutionOptions,
) -> Result<PackageSource> {
    let resolver = PackageResolver::new(conn);
    resolver.resolve(name, options).await
}

fn build_download_options(conn: &Connection, repo: &Repository) -> Result<DownloadOptions> {
    let mut stmt = conn.prepare("PRAGMA database_list")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
    })?;
    for row in rows {
        let (name, file) = row?;
        if name == "main" && !file.is_empty() {
            let keyring = crate::db::paths::keyring_dir(&file);
            return DownloadOptions::for_repository(repo, &keyring);
        }
    }
    Err(Error::ConfigError(format!(
        "repository '{}' download cannot resolve the database trust-store path",
        repo.name
    )))
}

#[cfg(test)]
include!("resolution/tests.rs");
