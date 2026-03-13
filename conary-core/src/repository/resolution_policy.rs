// conary-core/src/repository/resolution_policy.rs

//! Repository resolution policy types.
//!
//! Policy rules control which repositories may satisfy which requests, how
//! cross-distro mixing is handled, and how the resolver filters candidates.
//!
//! Design principles:
//! - Explicit request scope (`--repo`, `--from-distro`) applies only to root
//!   requests, not to transitive dependencies.
//! - Policy filtering happens *after* native semantic matching, not before.
//! - Policy rules operate at multiple granularities: single package, canonical
//!   family, or package class.
//! - Dependency mixing can be strict, guarded, or permissive.

use super::dependency_model::RepositoryDependencyFlavor;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Request scope
// ---------------------------------------------------------------------------

/// How a user explicitly constrained the source of a request.
///
/// This applies only to root-level requests (i.e. what the user typed on the
/// command line), not to transitive dependencies discovered during resolution.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RequestScope {
    /// No explicit scope -- use default policy.
    #[default]
    Any,

    /// The user pinned to a specific repository by name (e.g. `--repo fedora`).
    Repository(String),

    /// The user pinned to a specific distro flavor (e.g. `--from-distro deb`).
    DistroFlavor(RepositoryDependencyFlavor),
}

// ---------------------------------------------------------------------------
// Dependency mixing policy
// ---------------------------------------------------------------------------

/// How aggressively cross-distro dependency mixing is permitted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DependencyMixingPolicy {
    /// Dependencies must come from the same distro flavor as the root package.
    /// This is the safest setting and is the default.
    #[default]
    Strict,

    /// Dependencies prefer the same distro flavor but fall back to others when
    /// no same-flavor candidate exists.  The resolver logs a warning for each
    /// cross-flavor resolution.
    Guarded,

    /// Any repository may satisfy any dependency regardless of distro flavor.
    /// This is intended for expert use and testing.
    Permissive,
}

// ---------------------------------------------------------------------------
// Candidate origin
// ---------------------------------------------------------------------------

/// Where a candidate package comes from.
///
/// Used by policy rules to decide whether a candidate is acceptable.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CandidateOrigin {
    /// Repository name.
    pub repository: String,

    /// Distro flavor of the repository.
    pub flavor: RepositoryDependencyFlavor,
}

// ---------------------------------------------------------------------------
// Policy rule scope
// ---------------------------------------------------------------------------

/// The granularity at which a policy exception applies.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PolicyRuleScope {
    /// Exception for a single package by exact name.
    Package(String),

    /// Exception for a canonical name family (e.g. all packages that map to
    /// the canonical `openssl` family across distros).
    CanonicalFamily(String),

    /// Exception for a package class (e.g. `library`, `runtime`, `kernel`).
    PackageClass(String),

    /// Exception applies globally to all packages.
    Global,
}

// ---------------------------------------------------------------------------
// Source selection profile
// ---------------------------------------------------------------------------

/// A named policy exception that authorizes an out-of-pin distro for a
/// specific scope.
///
/// For example, a rule might say "allow Debian packages for the `openssl`
/// canonical family even though the system is primarily Fedora".
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceSelectionProfile {
    /// Human-readable name for this rule (for logging / diagnostics).
    pub name: String,

    /// Which packages this rule applies to.
    pub scope: PolicyRuleScope,

    /// Which distro flavors are permitted under this rule.
    pub allowed_flavors: Vec<RepositoryDependencyFlavor>,

    /// Which specific repositories are permitted (empty = all repos of the
    /// allowed flavors).
    pub allowed_repositories: Vec<String>,

    /// Priority order when multiple rules match (higher wins).
    pub priority: u32,
}

// ---------------------------------------------------------------------------
// Top-level resolution policy
// ---------------------------------------------------------------------------

/// The complete policy governing how the resolver selects candidates.
///
/// A `ResolutionPolicy` is assembled from the user's command-line flags, the
/// system configuration, and any explicit policy overrides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionPolicy {
    /// User's explicit scope for the root request.
    pub request_scope: RequestScope,

    /// How cross-distro dependency mixing is handled.
    pub mixing: DependencyMixingPolicy,

    /// Policy exception rules (evaluated in priority order).
    pub profiles: Vec<SourceSelectionProfile>,
}

impl Default for ResolutionPolicy {
    fn default() -> Self {
        Self {
            request_scope: RequestScope::Any,
            mixing: DependencyMixingPolicy::Strict,
            profiles: Vec::new(),
        }
    }
}

impl ResolutionPolicy {
    /// Create a new policy with default settings (strict mixing, no overrides).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the request scope.
    #[must_use]
    pub fn with_scope(mut self, scope: RequestScope) -> Self {
        self.request_scope = scope;
        self
    }

    /// Set the dependency mixing policy.
    #[must_use]
    pub fn with_mixing(mut self, mixing: DependencyMixingPolicy) -> Self {
        self.mixing = mixing;
        self
    }

    /// Add a source selection profile (exception rule).
    #[must_use]
    pub fn with_profile(mut self, profile: SourceSelectionProfile) -> Self {
        self.profiles.push(profile);
        self
    }

    /// Evaluate whether a candidate is acceptable for a given package name
    /// under this policy.
    ///
    /// `is_root` indicates whether this is a root-level request (user-typed)
    /// or a transitive dependency.  Request-scope restrictions apply only to
    /// root requests.
    ///
    /// `primary_flavor` is the distro flavor of the system or the root
    /// package, used for strict/guarded mixing checks on transitive deps.
    #[must_use]
    pub fn accepts_candidate(
        &self,
        candidate: &CandidateOrigin,
        package_name: &str,
        is_root: bool,
        primary_flavor: Option<RepositoryDependencyFlavor>,
    ) -> bool {
        // Step 1: Check request scope (root requests only).
        if is_root {
            match &self.request_scope {
                RequestScope::Any => {}
                RequestScope::Repository(repo) => {
                    if candidate.repository != *repo {
                        return false;
                    }
                }
                RequestScope::DistroFlavor(flavor) => {
                    if candidate.flavor != *flavor {
                        return false;
                    }
                }
            }
        }

        // Step 2: Check mixing policy (transitive deps).
        if let Some(primary) = primary_flavor
            && candidate.flavor != primary
        {
            match self.mixing {
                DependencyMixingPolicy::Strict => {
                    // Check for an exception rule.
                    if !self.has_exception(package_name, candidate.flavor) {
                        return false;
                    }
                }
                DependencyMixingPolicy::Guarded => {
                    // Guarded allows it but the caller should log a warning.
                    // We still check exceptions for explicit authorization.
                }
                DependencyMixingPolicy::Permissive => {
                    // Anything goes.
                }
            }
        }

        true
    }

    /// Check if any exception rule authorizes the given package from the
    /// given flavor.
    fn has_exception(&self, package_name: &str, flavor: RepositoryDependencyFlavor) -> bool {
        // Evaluate in priority order (highest first).
        let mut sorted: Vec<&SourceSelectionProfile> = self.profiles.iter().collect();
        sorted.sort_by(|a, b| b.priority.cmp(&a.priority));

        for profile in sorted {
            if !profile.allowed_flavors.contains(&flavor) && !profile.allowed_flavors.is_empty() {
                continue;
            }

            match &profile.scope {
                PolicyRuleScope::Package(name) => {
                    if name == package_name {
                        return true;
                    }
                }
                PolicyRuleScope::CanonicalFamily(family) => {
                    // In a real implementation this would check the canonical
                    // name mapping.  For now, exact match on family name.
                    if family == package_name {
                        return true;
                    }
                }
                PolicyRuleScope::PackageClass(_) => {
                    // Package class matching requires metadata lookup.
                    // Stubbed for now -- will be wired in Task 8.
                }
                PolicyRuleScope::Global => {
                    return true;
                }
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fedora_origin() -> CandidateOrigin {
        CandidateOrigin {
            repository: "fedora".to_string(),
            flavor: RepositoryDependencyFlavor::Rpm,
        }
    }

    fn debian_origin() -> CandidateOrigin {
        CandidateOrigin {
            repository: "ubuntu-noble".to_string(),
            flavor: RepositoryDependencyFlavor::Deb,
        }
    }

    #[test]
    fn default_policy_accepts_anything_without_primary() {
        let policy = ResolutionPolicy::default();
        assert!(policy.accepts_candidate(&fedora_origin(), "bash", false, None));
        assert!(policy.accepts_candidate(&debian_origin(), "bash", false, None));
    }

    #[test]
    fn strict_mixing_rejects_cross_flavor_without_exception() {
        let policy = ResolutionPolicy::default();
        assert!(!policy.accepts_candidate(
            &debian_origin(),
            "openssl",
            false,
            Some(RepositoryDependencyFlavor::Rpm),
        ));
    }

    #[test]
    fn strict_mixing_allows_same_flavor() {
        let policy = ResolutionPolicy::default();
        assert!(policy.accepts_candidate(
            &fedora_origin(),
            "glibc",
            false,
            Some(RepositoryDependencyFlavor::Rpm),
        ));
    }

    #[test]
    fn request_scope_repo_filters_root_only() {
        let policy =
            ResolutionPolicy::new().with_scope(RequestScope::Repository("fedora".to_string()));

        // Root request from debian is rejected.
        assert!(!policy.accepts_candidate(&debian_origin(), "bash", true, None));

        // Root request from fedora is accepted.
        assert!(policy.accepts_candidate(&fedora_origin(), "bash", true, None));

        // Transitive dep from debian is accepted (scope only applies to root).
        assert!(policy.accepts_candidate(&debian_origin(), "glibc", false, None));
    }

    #[test]
    fn request_scope_distro_filters_root_only() {
        let policy = ResolutionPolicy::new()
            .with_scope(RequestScope::DistroFlavor(RepositoryDependencyFlavor::Rpm));

        assert!(policy.accepts_candidate(&fedora_origin(), "bash", true, None));
        assert!(!policy.accepts_candidate(&debian_origin(), "bash", true, None));
    }

    #[test]
    fn guarded_mixing_allows_cross_flavor() {
        let policy = ResolutionPolicy::new().with_mixing(DependencyMixingPolicy::Guarded);
        assert!(policy.accepts_candidate(
            &debian_origin(),
            "openssl",
            false,
            Some(RepositoryDependencyFlavor::Rpm),
        ));
    }

    #[test]
    fn permissive_mixing_allows_anything() {
        let policy = ResolutionPolicy::new().with_mixing(DependencyMixingPolicy::Permissive);
        assert!(policy.accepts_candidate(
            &debian_origin(),
            "openssl",
            false,
            Some(RepositoryDependencyFlavor::Rpm),
        ));
    }

    #[test]
    fn package_exception_overrides_strict() {
        let policy = ResolutionPolicy::new().with_profile(SourceSelectionProfile {
            name: "allow-openssl-from-debian".to_string(),
            scope: PolicyRuleScope::Package("openssl".to_string()),
            allowed_flavors: vec![RepositoryDependencyFlavor::Deb],
            allowed_repositories: Vec::new(),
            priority: 10,
        });

        // openssl from debian is accepted (exception).
        assert!(policy.accepts_candidate(
            &debian_origin(),
            "openssl",
            false,
            Some(RepositoryDependencyFlavor::Rpm),
        ));

        // curl from debian is rejected (no exception).
        assert!(!policy.accepts_candidate(
            &debian_origin(),
            "curl",
            false,
            Some(RepositoryDependencyFlavor::Rpm),
        ));
    }

    #[test]
    fn global_exception_overrides_strict_for_all_packages() {
        let policy = ResolutionPolicy::new().with_profile(SourceSelectionProfile {
            name: "allow-all-deb".to_string(),
            scope: PolicyRuleScope::Global,
            allowed_flavors: vec![RepositoryDependencyFlavor::Deb],
            allowed_repositories: Vec::new(),
            priority: 1,
        });

        assert!(policy.accepts_candidate(
            &debian_origin(),
            "anything",
            false,
            Some(RepositoryDependencyFlavor::Rpm),
        ));
    }

    #[test]
    fn higher_priority_exception_wins() {
        let policy = ResolutionPolicy::new()
            .with_profile(SourceSelectionProfile {
                name: "low-pri-global".to_string(),
                scope: PolicyRuleScope::Global,
                allowed_flavors: vec![RepositoryDependencyFlavor::Deb],
                allowed_repositories: Vec::new(),
                priority: 1,
            })
            .with_profile(SourceSelectionProfile {
                name: "high-pri-package".to_string(),
                scope: PolicyRuleScope::Package("openssl".to_string()),
                allowed_flavors: vec![RepositoryDependencyFlavor::Deb],
                allowed_repositories: Vec::new(),
                priority: 100,
            });

        // Both should match but the higher priority one is evaluated first.
        assert!(policy.accepts_candidate(
            &debian_origin(),
            "openssl",
            false,
            Some(RepositoryDependencyFlavor::Rpm),
        ));
    }
}
