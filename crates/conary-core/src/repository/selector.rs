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

use crate::db::models::{Repository, RepositoryPackage};
use crate::error::{Error, Result};
use crate::repository::resolution_policy::{DependencyMixingPolicy, ResolutionPolicy};
use crate::repository::versioning::{
    VersionScheme, compare_repo_package_versions, resolve_package_version_scheme,
};
use rusqlite::Connection;
use tracing::{debug, info};

/// Options for package selection
#[derive(Debug, Clone, Default)]
pub struct SelectionOptions {
    /// Specific version to select (if None, select latest)
    pub version: Option<String>,
    /// Exact signed CCS build release to select.
    pub package_release: Option<String>,
    /// Specific repository to search (if None, search all enabled)
    pub repository: Option<String>,
    /// Specific architecture to filter (if None, use system architecture)
    pub architecture: Option<String>,
    /// Whether discovery is target-native or intentionally all-architecture.
    pub architecture_scope: ArchitectureScope,
    /// Resolution policy to apply when filtering candidates.
    /// When `None`, all candidates from enabled repositories are accepted.
    pub policy: Option<ResolutionPolicy>,
    /// Whether this selection is for a root (user-typed) request.
    /// Policy request-scope constraints only apply to root requests.
    pub is_root: bool,
}

/// Architecture discovery scope.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ArchitectureScope {
    /// Return only candidates compatible with the target-native architecture.
    #[default]
    Native,
    /// Return every explicitly declared architecture for typed solver filtering.
    All,
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

    /// Check whether one exact source-native package architecture targets the
    /// current machine.
    ///
    /// Handles the arch-independent markers from all three ecosystems:
    /// - RPM: `noarch`
    /// - Debian: `all`
    /// - Arch Linux / ALPM: `any`
    ///
    /// Architecture-independent markers are interpreted only under the
    /// package ecosystem that owns them. For example, Debian `all`, Arch
    /// `any`, and RPM `noarch` are distinct signed tokens rather than generic
    /// aliases.
    pub fn is_architecture_compatible(
        scheme: VersionScheme,
        pkg_arch: Option<&str>,
        system_arch: &str,
    ) -> bool {
        pkg_arch.is_some_and(|architecture| {
            effective_machine_architecture(scheme, architecture, system_arch)
                == native_machine_architecture(system_arch)
        })
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
        if let Some(policy) = &options.policy {
            policy.validate_profile_ids().map_err(Error::ConfigError)?;
        }
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
            let scheme = resolve_package_version_scheme(&pkg);
            let package_architecture = pkg.architecture.as_deref().ok_or_else(|| {
                Error::ConfigError(format!(
                    "repository package '{}-{}' has no architecture authority",
                    pkg.name, pkg.version
                ))
            })?;
            // Filter by version if specified
            if let Some(ref version) = options.version
                && &pkg.version != version
            {
                continue;
            }
            if let Some(ref release) = options.package_release
                && &pkg.package_release != release
            {
                continue;
            }

            // Filter by architecture
            if options.architecture_scope == ArchitectureScope::Native
                && !Self::is_architecture_compatible(
                    scheme,
                    Some(package_architecture),
                    system_arch,
                )
            {
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

            // Repository and package profile claims are one exact authority.
            // Validate them even when no source policy was supplied so an
            // unscoped request cannot admit contradictory metadata.
            let candidate_profile = candidate_source_profile(&pkg, &repo)?;

            // Apply resolution policy filter
            if let Some(ref policy) = options.policy {
                if !candidate_matches_allowed_distros(policy, &pkg, &repo)? {
                    debug!(
                        "Policy rejected package {} {} from repository {} due to allowlist mismatch",
                        pkg.name, pkg.version, repo.name
                    );
                    continue;
                }

                let mut policy_without_allowlist = policy.clone();
                policy_without_allowlist.allowed_distros.clear();
                if !policy_without_allowlist.accepts_candidate(
                    &repo.name,
                    candidate_profile,
                    options.is_root,
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
        _conn: &Connection,
        mut candidates: Vec<PackageWithRepo>,
        options: &SelectionOptions,
    ) -> Result<PackageWithRepo> {
        if candidates.is_empty() {
            return Err(Error::NotFound("No matching packages found".to_string()));
        }

        let guarded_profile = options.policy.as_ref().and_then(|policy| {
            (policy.mixing == DependencyMixingPolicy::Guarded)
                .then(|| policy.primary_profile())
                .flatten()
        });
        let selected_index = exact_winner_index(&candidates, |candidate| {
            let candidate_profile =
                candidate_source_profile(&candidate.package, &candidate.repository)?;
            Ok(guarded_profile.is_some_and(|profile| candidate_profile == Some(profile)))
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

        let selected_index = exact_winner_index(&candidates, |_| Ok(false))?;
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
    is_preferred: impl Fn(&PackageWithRepo) -> Result<bool>,
) -> Result<usize> {
    let preferences = candidates
        .iter()
        .map(&is_preferred)
        .collect::<Result<Vec<_>>>()?;
    let preferred_exists = preferences.iter().any(|preferred| *preferred);
    let top_priority = candidates
        .iter()
        .zip(&preferences)
        .filter(|(_, preferred)| **preferred == preferred_exists)
        .map(|(candidate, _)| candidate.repository.priority)
        .max()
        .expect("selection rejects an empty candidate set before ranking");

    let contenders = candidates
        .iter()
        .enumerate()
        .filter(|(index, candidate)| {
            preferences[*index] == preferred_exists && candidate.repository.priority == top_priority
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

/// Return the one exact public source profile jointly declared by a package
/// row and its repository.
///
/// Package-level metadata may repeat repository authority, but it may never
/// contradict it. This is the shared projection used by selection,
/// resolution, and transaction provenance.
pub fn candidate_source_profile<'a>(
    pkg: &'a RepositoryPackage,
    repo: &'a Repository,
) -> Result<Option<&'a str>> {
    let profile = match (
        pkg.source_profile.as_deref(),
        repo.source_profile.as_deref(),
    ) {
        (Some(package_profile), Some(repository_profile))
            if package_profile != repository_profile =>
        {
            return Err(Error::ConfigError(format!(
                "repository package '{}-{}' declares source profile '{}' but repository '{}' declares '{}'",
                pkg.name, pkg.version, package_profile, repo.name, repository_profile
            )));
        }
        (Some(package_profile), _) => Some(package_profile),
        (None, repository_profile) => repository_profile,
    };
    if let Some(profile) = profile
        && crate::repository::supported_profiles::profile_by_public_id(profile).is_none()
    {
        return Err(Error::ConfigError(format!(
            "repository package '{}-{}' resolves to unsupported source profile '{}'",
            pkg.name, pkg.version, profile
        )));
    }
    Ok(profile)
}

pub(crate) fn candidate_matches_allowed_distros(
    policy: &ResolutionPolicy,
    pkg: &RepositoryPackage,
    repo: &Repository,
) -> Result<bool> {
    if policy.allowed_distros.is_empty() {
        return candidate_source_profile(pkg, repo).map(|_| true);
    }

    let candidate_profile = candidate_source_profile(pkg, repo)?;
    Ok(policy
        .allowed_distros
        .iter()
        .any(|allowed| candidate_profile.is_some_and(|profile| allowed == profile)))
}

/// Compare two exact package architecture tokens under their owning package
/// schemes. Architecture-independent tokens resolve to the native machine for
/// this comparison; they are never rewritten in stored package identity.
pub fn package_architectures_match(
    left_scheme: VersionScheme,
    left_architecture: &str,
    right_scheme: VersionScheme,
    right_architecture: &str,
    native_architecture: &str,
) -> bool {
    effective_machine_architecture(left_scheme, left_architecture, native_architecture)
        == effective_machine_architecture(right_scheme, right_architecture, native_architecture)
}

/// Typed machine identity shared only by architecture compatibility checks.
///
/// The parser below encodes the source-owned tokens from dpkg's `cputable`,
/// RPM's `rpmrc` architecture tables, and makepkg's `CARCH` contract. Unknown
/// tokens retain exact identity and therefore only compare equal literally.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MachineArchitecture {
    X86_64,
    X86_32,
    Aarch64,
    ArmV7HardFloat,
    PowerPc64Le,
    S390x,
    RiscV64,
    Exact(String),
}

fn effective_machine_architecture(
    scheme: VersionScheme,
    architecture: &str,
    native_architecture: &str,
) -> MachineArchitecture {
    if is_architecture_independent(scheme, architecture) {
        return native_machine_architecture(native_architecture);
    }
    package_machine_architecture(scheme, architecture)
}

fn is_architecture_independent(scheme: VersionScheme, architecture: &str) -> bool {
    matches!(
        (scheme, architecture),
        (VersionScheme::Conary | VersionScheme::Rpm, "noarch")
            | (VersionScheme::Debian, "all")
            | (VersionScheme::Arch, "any")
    )
}

fn package_machine_architecture(scheme: VersionScheme, architecture: &str) -> MachineArchitecture {
    match (scheme, architecture) {
        (VersionScheme::Debian, "amd64")
        | (VersionScheme::Rpm | VersionScheme::Arch | VersionScheme::Conary, "x86_64") => {
            MachineArchitecture::X86_64
        }
        (VersionScheme::Debian, "i386")
        | (VersionScheme::Rpm, "i386" | "i486" | "i586" | "i686")
        | (VersionScheme::Arch | VersionScheme::Conary, "i686") => MachineArchitecture::X86_32,
        (VersionScheme::Debian, "arm64")
        | (VersionScheme::Rpm | VersionScheme::Arch | VersionScheme::Conary, "aarch64") => {
            MachineArchitecture::Aarch64
        }
        (VersionScheme::Debian, "armhf")
        | (VersionScheme::Rpm, "armv7hl")
        | (VersionScheme::Arch, "armv7h")
        | (VersionScheme::Conary, "armv7") => MachineArchitecture::ArmV7HardFloat,
        (VersionScheme::Debian, "ppc64el")
        | (VersionScheme::Rpm | VersionScheme::Arch | VersionScheme::Conary, "ppc64le") => {
            MachineArchitecture::PowerPc64Le
        }
        (_, "s390x") => MachineArchitecture::S390x,
        (_, "riscv64") => MachineArchitecture::RiscV64,
        (_, exact) => MachineArchitecture::Exact(exact.to_string()),
    }
}

fn native_machine_architecture(architecture: &str) -> MachineArchitecture {
    match architecture {
        "x86_64" | "amd64" => MachineArchitecture::X86_64,
        "x86" | "i386" | "i686" => MachineArchitecture::X86_32,
        "aarch64" | "arm64" => MachineArchitecture::Aarch64,
        "arm" | "armv7" | "armhf" => MachineArchitecture::ArmV7HardFloat,
        "powerpc64le" | "ppc64le" | "ppc64el" => MachineArchitecture::PowerPc64Le,
        "s390x" => MachineArchitecture::S390x,
        "riscv64" => MachineArchitecture::RiscV64,
        exact => MachineArchitecture::Exact(exact.to_string()),
    }
}

#[cfg(test)]
#[path = "selector/tests.rs"]
mod tests;
