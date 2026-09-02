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
use crate::repository::architecture::{
    KnownPackageArchitecture, NativeMachineIdentityV1, NativeResolutionArchitectureDecisionV1,
    host_machine_identity, known_package_architecture, native_host_machine_identity,
    native_resolution_architecture_decision, require_profile_host_architecture,
    require_profile_host_architecture_token,
};
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
    /// Package architecture variant requested by the caller.
    pub variant: Option<PackageArchitectureVariant>,
    /// Explicit assertion about the machine architecture used by a profile.
    pub host_assertion: Option<HostArchitectureAssertion>,
    /// Whether discovery is target-native or intentionally all-architecture.
    pub architecture_scope: ArchitectureScope,
    /// Resolution policy to apply when filtering candidates.
    /// When `None`, all candidates from enabled repositories are accepted.
    pub policy: Option<ResolutionPolicy>,
    /// Whether this selection is for a root (user-typed) request.
    /// Policy request-scope constraints only apply to root requests.
    pub is_root: bool,
}

/// A package-variant selector, optionally bound to the source scheme that
/// supplied the token.
///
/// Installed package identities use [`Self::from_package`] so architecture-
/// independent variants remain independent across replatforming. User-facing
/// selectors that do not yet have a source scheme use [`Self::unscoped`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageArchitectureVariant {
    scheme: Option<VersionScheme>,
    token: String,
}

impl PackageArchitectureVariant {
    #[must_use]
    pub fn from_package(scheme: VersionScheme, token: impl Into<String>) -> Self {
        Self {
            scheme: Some(scheme),
            token: token.into(),
        }
    }

    #[must_use]
    pub fn unscoped(token: impl Into<String>) -> Self {
        Self {
            scheme: None,
            token: token.into(),
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.token
    }
}

/// A machine-architecture token asserted by the caller at a profile boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostArchitectureAssertion(String);

impl HostArchitectureAssertion {
    #[must_use]
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
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
    pub fn detect_architecture() -> Result<String> {
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
    pub fn is_machine_architecture_compatible(
        scheme: VersionScheme,
        pkg_arch: Option<&str>,
        system_arch: &str,
    ) -> bool {
        let Some(architecture) = pkg_arch else {
            return false;
        };
        effective_machine_architecture(scheme, architecture, system_arch)
            == native_machine_architecture(system_arch)
    }

    /// Check one installed package against an explicit token in that package's
    /// own architecture vocabulary.
    pub fn is_package_architecture_compatible(
        scheme: VersionScheme,
        pkg_arch: Option<&str>,
        requested_arch: &str,
    ) -> bool {
        let Some(architecture) = pkg_arch else {
            return false;
        };
        effective_package_architecture(scheme, architecture, requested_arch)
            == package_machine_architecture_in_scheme(scheme, requested_arch)
    }

    /// Check package machine and ABI admission against its exact source profile.
    pub fn is_architecture_compatible_for_profile(
        profile: &crate::repository::supported_profiles::SupportedProfile,
        pkg_arch: Option<&str>,
    ) -> Result<bool> {
        let Some(architecture) = pkg_arch else {
            return Ok(false);
        };
        Ok(matches!(
            native_resolution_architecture_decision(profile, architecture).into_result()?,
            NativeResolutionArchitectureDecisionV1::Admitted
        ))
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
            policy
                .validate_source_identities()
                .map_err(Error::ConfigError)?;
        }
        let detected_arch = Self::detect_architecture()?;
        let detected_identity = native_host_machine_identity()?;
        let host_architecture = options
            .host_assertion
            .as_ref()
            .map(HostArchitectureAssertion::as_str)
            .unwrap_or(&detected_arch);

        debug!(
            "Searching for package '{}' (host arch: {}, variant: {:?})",
            package_name,
            host_architecture,
            options
                .variant
                .as_ref()
                .map(PackageArchitectureVariant::as_str)
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

            if options.architecture_scope == ArchitectureScope::Native {
                // Foreign package admission is bound to the source profile's
                // machine token and package ABI, never the executable's libc.
                let architecture_compatible = match scheme {
                    VersionScheme::Rpm | VersionScheme::Debian | VersionScheme::Arch => {
                        let Some(profile_id) = candidate_source_profile(&pkg, &repo)? else {
                            return Err(Error::ConfigError(format!(
                                "repository '{}' has no source profile for native package admission",
                                repo.name
                            )));
                        };
                        let profile =
                            crate::repository::supported_profiles::profile_by_public_id(profile_id)
                                .ok_or_else(|| {
                                    Error::ConfigError(format!(
                                        "repository '{}' declares unsupported source profile '{}'",
                                        repo.name, profile_id
                                    ))
                                })?;
                        if options.host_assertion.is_some() {
                            require_profile_host_architecture_token(profile, host_architecture)?;
                        } else {
                            require_profile_host_architecture(
                                profile,
                                &detected_identity,
                                &detected_arch,
                            )?;
                        }
                        Self::is_architecture_compatible_for_profile(
                            profile,
                            Some(package_architecture),
                        )?
                    }
                    VersionScheme::Conary | VersionScheme::Eopkg => {
                        // These schemes do not have the typed foreign-profile
                        // architecture authority owned by RPM, dpkg, and ALPM.
                        Self::is_machine_architecture_compatible(
                            scheme,
                            Some(package_architecture),
                            host_architecture,
                        )
                    }
                };
                if !architecture_compatible {
                    debug!(
                        "Skipping package {} {} with incompatible arch {:?}",
                        pkg.name, pkg.version, pkg.architecture
                    );
                    continue;
                }
            }

            if let Some(requested_variant) = options.variant.as_ref()
                && !package_variant_matches(
                    requested_variant,
                    scheme,
                    package_architecture,
                    &detected_arch,
                )
            {
                debug!(
                    "Skipping package {} {} with variant {:?}; requested {}",
                    pkg.name,
                    pkg.version,
                    pkg.architecture,
                    requested_variant.as_str()
                );
                continue;
            }

            // Native source identity comes from the stream-bound repository
            // policy. Public feed-profile claims remain an exact projection
            // for Remi/static repositories and may not contradict each other.
            let candidate_identity = candidate_source_identity(&pkg, &repo)?;

            // Apply resolution policy filter
            if let Some(ref policy) = options.policy
                && !policy.accepts_candidate(&repo.name, candidate_identity, options.is_root)
            {
                debug!(
                    "Policy rejected package {} {} from repository {} (scheme {:?})",
                    pkg.name, pkg.version, repo.name, scheme
                );
                continue;
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

        let guarded_source_identity = options.policy.as_ref().and_then(|policy| {
            (policy.mixing == DependencyMixingPolicy::Guarded)
                .then(|| policy.primary_source_identity())
                .flatten()
        });
        let selected_index =
            exact_winner_index(&candidates, |candidate| {
                let candidate_identity =
                    candidate_source_identity(&candidate.package, &candidate.repository)?;
                Ok(guarded_source_identity
                    .is_some_and(|identity| candidate_identity == Some(identity)))
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

/// Return the one exact public feed profile jointly declared by a package row
/// and its repository.
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
    Ok(profile)
}

/// Return the exact native source identity for one candidate.
///
/// A stream-bound native repository policy is authoritative. Remi/static
/// candidates without one use their exact public feed-profile projection.
pub fn candidate_source_identity<'a>(
    pkg: &'a RepositoryPackage,
    repo: &'a Repository,
) -> Result<Option<&'a str>> {
    let feed_profile = candidate_source_profile(pkg, repo)?;
    if let Some(policy) = repo.source_policy.as_ref() {
        policy.validate()?;
        return Ok(Some(policy.source_identity.as_str()));
    }
    Ok(feed_profile)
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

/// Typed machine identity shared only by non-admission literal comparisons.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MachineArchitecture {
    Known(NativeMachineIdentityV1),
    Literal(String),
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

fn effective_package_architecture(
    scheme: VersionScheme,
    architecture: &str,
    native_architecture: &str,
) -> MachineArchitecture {
    if is_architecture_independent(scheme, architecture) {
        return package_machine_architecture_in_scheme(scheme, native_architecture);
    }
    package_machine_architecture_in_scheme(scheme, architecture)
}

fn package_machine_architecture_in_scheme(
    scheme: VersionScheme,
    architecture: &str,
) -> MachineArchitecture {
    match known_package_architecture(scheme, architecture) {
        Some(KnownPackageArchitecture::Machine { identity, .. }) => {
            MachineArchitecture::Known(identity)
        }
        Some(KnownPackageArchitecture::Independent) | None => {
            MachineArchitecture::Literal(architecture.to_string())
        }
    }
}

fn is_architecture_independent(scheme: VersionScheme, architecture: &str) -> bool {
    if scheme == VersionScheme::Arch {
        return architecture == "any";
    }
    matches!(
        known_package_architecture(scheme, architecture),
        Some(KnownPackageArchitecture::Independent)
    )
}

fn package_variant_matches(
    requested: &PackageArchitectureVariant,
    candidate_scheme: VersionScheme,
    candidate_architecture: &str,
    native_architecture: &str,
) -> bool {
    let Some(requested_scheme) = requested.scheme else {
        return PackageSelector::is_package_architecture_compatible(
            candidate_scheme,
            Some(candidate_architecture),
            requested.as_str(),
        );
    };

    let requested_is_independent =
        is_architecture_independent(requested_scheme, requested.as_str());
    let candidate_is_independent =
        is_architecture_independent(candidate_scheme, candidate_architecture);

    if requested_is_independent {
        return candidate_is_independent;
    }
    if candidate_is_independent {
        return true;
    }

    package_architectures_match(
        requested_scheme,
        requested.as_str(),
        candidate_scheme,
        candidate_architecture,
        native_architecture,
    )
}

fn package_machine_architecture(scheme: VersionScheme, architecture: &str) -> MachineArchitecture {
    match known_package_architecture(scheme, architecture) {
        Some(KnownPackageArchitecture::Machine { identity, .. }) => {
            MachineArchitecture::Known(identity)
        }
        Some(KnownPackageArchitecture::Independent) | None => host_machine_identity(architecture)
            .map(MachineArchitecture::Known)
            .unwrap_or_else(|| MachineArchitecture::Literal(architecture.to_string())),
    }
}

fn native_machine_architecture(architecture: &str) -> MachineArchitecture {
    host_machine_identity(architecture)
        .map(MachineArchitecture::Known)
        .unwrap_or_else(|| MachineArchitecture::Literal(architecture.to_string()))
}

#[cfg(test)]
#[path = "selector/tests.rs"]
mod tests;
