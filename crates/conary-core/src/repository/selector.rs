// conary-core/src/repository/selector.rs

//! Package selection logic for repository-based installation
//!
//! This module handles selecting the best package when multiple matches exist
//! across different repositories, versions, or architectures.
//!
//! Policy awareness is layered on top of existing priority/version logic:
//! - Architecture compatibility handles RPM `noarch`, Debian `all`, Arch `any`
//! - Version ordering uses scheme-aware comparison (never cross-scheme)
//! - `ResolutionPolicy` filters candidates by request scope and mixing policy
//! - Canonical expansion surfaces all cross-distro implementations for root requests

use crate::db::models::{RepologyCacheEntry, Repository, RepositoryPackage};
use crate::error::{Error, Result};
use crate::repository::LatestSignal;
use crate::repository::dependency_model::RepositoryDependencyFlavor;
use crate::repository::resolution_policy::{ResolutionPolicy, SelectionMode};
use crate::repository::versioning::{
    compare_repo_package_versions, resolve_package_version_scheme,
};
use chrono::Utc;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use tracing::{debug, info};

/// Options for package selection
#[derive(Debug, Clone, Default)]
pub struct SelectionOptions {
    /// Specific version to select (if None, select latest)
    pub version: Option<String>,
    /// Specific repository to search (if None, search all enabled)
    pub repository: Option<String>,
    /// Specific architecture to filter (if None, use system architecture)
    pub architecture: Option<String>,
    /// Resolution policy to apply when filtering candidates.
    /// When `None`, all candidates from enabled repositories are accepted.
    pub policy: Option<ResolutionPolicy>,
    /// Whether this selection is for a root (user-typed) request.
    /// Policy request-scope constraints only apply to root requests.
    pub is_root: bool,
    /// The primary distro flavor of the system (for mixing policy checks).
    pub primary_flavor: Option<RepositoryDependencyFlavor>,
}

/// Information about a package with its repository
#[derive(Debug, Clone)]
pub struct PackageWithRepo {
    pub package: RepositoryPackage,
    pub repository: Repository,
}

/// Package selector for choosing the best package from multiple matches
pub struct PackageSelector;

impl PackageSelector {
    /// Detect the current system architecture
    pub fn detect_architecture() -> String {
        super::registry::detect_system_arch()
    }

    /// Check if a package architecture is compatible with the system.
    ///
    /// Handles the arch-independent markers from all three ecosystems:
    /// - RPM: `noarch`
    /// - Debian: `all`
    /// - Arch Linux / ALPM: `any`
    ///
    /// Also handles cross-ecosystem arch name aliases (e.g. Debian `amd64`
    /// matches RPM `x86_64`) via [`normalize_arch`].
    pub fn is_architecture_compatible(pkg_arch: Option<&str>, system_arch: &str) -> bool {
        match pkg_arch {
            None => true,
            Some("noarch" | "all" | "any") => true,
            Some(arch) => normalize_arch(arch) == normalize_arch(system_arch),
        }
    }

    /// Search for packages by name with selection options
    ///
    /// Returns all matching packages with their repository information,
    /// filtered by the selection options and resolution policy.
    pub fn search_packages(
        conn: &Connection,
        package_name: &str,
        options: &SelectionOptions,
    ) -> Result<Vec<PackageWithRepo>> {
        let detected_arch = Self::detect_architecture();
        let system_arch = options.architecture.as_deref().unwrap_or(&detected_arch);

        debug!(
            "Searching for package '{}' (arch: {})",
            package_name, system_arch
        );

        // Find all matching packages
        let packages = RepositoryPackage::find_by_name(conn, package_name)?;

        if packages.is_empty() {
            return Ok(Vec::new());
        }

        // Get repository information for each package
        let mut results = Vec::new();
        for pkg in packages {
            // Filter by version if specified
            if let Some(ref version) = options.version
                && &pkg.version != version
            {
                continue;
            }

            // Filter by architecture
            if !Self::is_architecture_compatible(pkg.architecture.as_deref(), system_arch) {
                debug!(
                    "Skipping package {} {} with incompatible arch {:?}",
                    pkg.name, pkg.version, pkg.architecture
                );
                continue;
            }

            // Get repository information
            let repo = Repository::find_by_id(conn, pkg.repository_id)?.ok_or_else(|| {
                Error::NotFound(format!(
                    "Repository {} not found for package {}",
                    pkg.repository_id, pkg.name
                ))
            })?;

            // Filter by repository if specified
            if let Some(ref repo_name) = options.repository
                && &repo.name != repo_name
            {
                continue;
            }

            // Only include enabled repositories
            if !repo.enabled {
                debug!(
                    "Skipping package {} from disabled repository {}",
                    pkg.name, repo.name
                );
                continue;
            }

            // Apply resolution policy filter
            if let Some(ref policy) = options.policy {
                if !candidate_matches_allowed_distros(policy, &pkg, &repo) {
                    debug!(
                        "Policy rejected package {} {} from repository {} due to allowlist mismatch",
                        pkg.name, pkg.version, repo.name
                    );
                    continue;
                }

                let mut policy_without_allowlist = policy.clone();
                policy_without_allowlist.allowed_distros.clear();
                let scheme = resolve_package_version_scheme(&pkg);
                if !policy_without_allowlist.accepts_candidate(
                    &repo.name,
                    scheme,
                    package_name,
                    options.is_root,
                    options.primary_flavor,
                ) {
                    debug!(
                        "Policy rejected package {} {} from repository {} (scheme {:?})",
                        pkg.name, pkg.version, repo.name, scheme
                    );
                    continue;
                }
            }

            results.push(PackageWithRepo {
                package: pkg,
                repository: repo,
            });
        }

        Ok(results)
    }

    pub fn select_best_with_options(
        conn: &Connection,
        mut candidates: Vec<PackageWithRepo>,
        options: &SelectionOptions,
    ) -> Result<PackageWithRepo> {
        if candidates.is_empty() {
            return Err(Error::NotFound("No matching packages found".to_string()));
        }

        let latest_positive_keys = latest_positive_keys(conn, &candidates, options)?;
        let selected_index = exact_winner_index(&candidates, |candidate| {
            candidate_latest_key(candidate)
                .as_ref()
                .is_some_and(|key| latest_positive_keys.contains(key))
        })?;
        let selected = candidates.swap_remove(selected_index);
        info!(
            "Selected package {} {} from repository {} (priority {})",
            selected.package.name,
            selected.package.version,
            selected.repository.name,
            selected.repository.priority
        );

        Ok(selected)
    }

    /// Select the best package from a list of candidates
    ///
    /// Selection criteria (in order of priority):
    /// 1. Repository priority (higher is better)
    /// 2. Version (latest version, using scheme-aware comparison)
    ///
    /// Equal-priority candidates that version authority cannot distinguish
    /// return [`Error::AmbiguousPackageSelection`].
    pub fn select_best(mut candidates: Vec<PackageWithRepo>) -> Result<PackageWithRepo> {
        if candidates.is_empty() {
            return Err(Error::NotFound("No matching packages found".to_string()));
        }

        let selected_index = exact_winner_index(&candidates, |_| false)?;
        let selected = candidates.swap_remove(selected_index);
        info!(
            "Selected package {} {} from repository {} (priority {})",
            selected.package.name,
            selected.package.version,
            selected.repository.name,
            selected.repository.priority
        );

        Ok(selected)
    }

    /// Find and select the best package matching the given name and options
    ///
    /// This is a convenience function that combines search and selection.
    pub fn find_best_package(
        conn: &Connection,
        package_name: &str,
        options: &SelectionOptions,
    ) -> Result<PackageWithRepo> {
        let candidates = Self::search_packages(conn, package_name, options)?;

        if candidates.is_empty() {
            let mut msg = format!("Package '{}' not found in any repository", package_name);

            if let Some(ref repo) = options.repository {
                msg.push_str(&format!(" (searched repository: {})", repo));
            }

            if let Some(ref version) = options.version {
                msg.push_str(&format!(" (version: {})", version));
            }

            return Err(Error::NotFound(msg));
        }

        Self::select_best_with_options(conn, candidates, options)
    }
}

fn exact_winner_index(
    candidates: &[PackageWithRepo],
    is_preferred: impl Fn(&PackageWithRepo) -> bool,
) -> Result<usize> {
    let preferred_exists = candidates.iter().any(&is_preferred);
    let top_priority = candidates
        .iter()
        .filter(|candidate| is_preferred(candidate) == preferred_exists)
        .map(|candidate| candidate.repository.priority)
        .max()
        .expect("selection rejects an empty candidate set before ranking");

    let contenders = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| {
            is_preferred(candidate) == preferred_exists
                && candidate.repository.priority == top_priority
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    if contenders.len() == 1 {
        return Ok(contenders[0]);
    }

    let schemes = contenders
        .iter()
        .map(|index| {
            let candidate = &candidates[*index];
            resolve_package_version_scheme(&candidate.package)
        })
        .collect::<Vec<_>>();

    if schemes.windows(2).any(|pair| pair[0] != pair[1]) {
        return Err(ambiguous_selection(candidates, &contenders, &schemes));
    }

    let mut best = contenders[0];
    let mut tied = vec![best];
    for contender in contenders.iter().copied().skip(1) {
        let ordering = compare_repo_package_versions(
            &candidates[contender].package,
            &candidates[best].package,
        )?;
        match ordering {
            std::cmp::Ordering::Greater => {
                best = contender;
                tied.clear();
                tied.push(contender);
            }
            std::cmp::Ordering::Equal => tied.push(contender),
            std::cmp::Ordering::Less => {}
        }
    }

    if tied.len() > 1 {
        let tied_schemes = vec![schemes[0]; tied.len()];
        return Err(ambiguous_selection(candidates, &tied, &tied_schemes));
    }

    Ok(best)
}

fn ambiguous_selection(
    candidates: &[PackageWithRepo],
    contender_indices: &[usize],
    schemes: &[crate::repository::versioning::VersionScheme],
) -> Error {
    let package = candidates[contender_indices[0]].package.name.clone();
    let candidates = contender_indices
        .iter()
        .zip(schemes)
        .map(|(index, scheme)| {
            let candidate = &candidates[*index];
            format!(
                "{}:{}:{}:{}",
                candidate.repository.name,
                candidate.package.version,
                scheme.as_str(),
                candidate.package.architecture.as_deref().unwrap_or("any")
            )
        })
        .collect();
    Error::AmbiguousPackageSelection {
        package,
        candidates,
    }
}

fn candidate_distro_identifier<'a>(
    pkg: &'a RepositoryPackage,
    repo: &'a Repository,
) -> Option<&'a str> {
    pkg.distro
        .as_deref()
        .or(repo.default_strategy_distro.as_deref())
}

fn candidate_matches_allowed_distros(
    policy: &ResolutionPolicy,
    pkg: &RepositoryPackage,
    repo: &Repository,
) -> bool {
    if policy.allowed_distros.is_empty() {
        return true;
    }

    policy.allowed_distros.iter().any(|allowed| {
        allowed == &repo.name
            || candidate_distro_identifier(pkg, repo).is_some_and(|distro| {
                allowed == distro
                    || crate::repository::supported_profiles::profile_for_remi_target(distro)
                        .is_some_and(|profile| allowed == profile.id())
            })
    })
}

fn candidate_latest_key(candidate: &PackageWithRepo) -> Option<(i64, String)> {
    Some((
        candidate.package.canonical_id?,
        candidate_distro_identifier(&candidate.package, &candidate.repository)?.to_string(),
    ))
}

fn latest_positive_keys(
    conn: &Connection,
    candidates: &[PackageWithRepo],
    options: &SelectionOptions,
) -> Result<HashSet<(i64, String)>> {
    if options
        .policy
        .as_ref()
        .is_none_or(|policy| policy.selection_mode != SelectionMode::Latest)
    {
        return Ok(HashSet::new());
    }

    let mut distros_by_canonical: HashMap<i64, HashSet<String>> = HashMap::new();
    for candidate in candidates {
        let Some((canonical_id, distro)) = candidate_latest_key(candidate) else {
            continue;
        };
        distros_by_canonical
            .entry(canonical_id)
            .or_default()
            .insert(distro);
    }

    let now = Utc::now();
    let mut positive = HashSet::new();
    for (canonical_id, distros) in distros_by_canonical {
        let distro_list = distros.into_iter().collect::<Vec<_>>();
        let rows =
            RepologyCacheEntry::find_for_canonical_and_distros(conn, canonical_id, &distro_list)?;
        for row in rows {
            let status = row.status.as_deref().unwrap_or("");
            let signal =
                LatestSignal::from_repology(status, row.version.as_deref(), &row.fetched_at, now)?;
            if signal.is_positive() {
                positive.insert((canonical_id, row.distro));
            }
        }
    }

    Ok(positive)
}

/// Normalize an architecture name to a canonical form.
///
/// Different package ecosystems use different names for the same CPU
/// architecture.  This function maps all known aliases to a single
/// canonical string so that comparisons work across ecosystems:
///
/// | Canonical  | Aliases                     |
/// |------------|-----------------------------|
/// | `x86_64`   | `amd64`                     |
/// | `aarch64`  | `arm64`                     |
/// | `i686`     | `i386`, `i486`, `i586`      |
///
/// Unknown names are returned as-is (lowercase).
pub fn normalize_arch(arch: &str) -> &str {
    match arch {
        "amd64" => "x86_64",
        "arm64" => "aarch64",
        "i386" | "i486" | "i586" => "i686",
        // ARM 32-bit: Debian armhf, RPM armv7hl, and raw arm/armv7 all
        // map to armv7l (the kernel's name for 32-bit ARM with hard-float)
        "arm" | "armhf" | "armv7" | "armv7hl" => "armv7l",
        // ppc64le aliases
        "ppc64el" => "ppc64le",
        other => other,
    }
}

#[cfg(test)]
#[path = "selector/tests.rs"]
mod tests;
