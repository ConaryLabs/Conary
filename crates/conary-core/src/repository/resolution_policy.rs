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
//! - Dependency mixing can be strict, guarded, or permissive.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

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

    /// The user selected one exact supported distro profile.
    DistroProfile(String),
}

// ---------------------------------------------------------------------------
// Dependency mixing policy
// ---------------------------------------------------------------------------

/// How aggressively cross-distro dependency mixing is permitted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DependencyMixingPolicy {
    /// Packages must come from one exact distro profile.
    /// This is the safest setting and is the default.
    #[default]
    Strict,

    /// Packages prefer one exact distro profile but may fall back to others.
    /// The resolver logs a warning for each cross-profile resolution.
    Guarded,

    /// Any repository may satisfy any dependency regardless of distro profile.
    /// This is intended for expert use and testing.
    Permissive,
}

impl DependencyMixingPolicy {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Guarded => "guarded",
            Self::Permissive => "permissive",
        }
    }
}

impl fmt::Display for DependencyMixingPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for DependencyMixingPolicy {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "strict" => Ok(Self::Strict),
            "guarded" => Ok(Self::Guarded),
            "permissive" => Ok(Self::Permissive),
            _ => Err(format!(
                "unsupported dependency mixing policy '{value}'; expected strict, guarded, or permissive"
            )),
        }
    }
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

    /// Optional allowlist for distro/repository identifiers.
    pub allowed_distros: Vec<String>,

    /// Exact distro profile that owns this transaction.
    ///
    /// This is intentionally separate from package format. Fedora and
    /// openSUSE are both RPM ecosystems but are not interchangeable source
    /// authorities. Runtime loaders populate this from an explicit source
    /// scope, repository declaration, or persisted system pin.
    primary_profile: Option<String>,
}

impl Default for ResolutionPolicy {
    fn default() -> Self {
        Self {
            request_scope: RequestScope::Any,
            mixing: DependencyMixingPolicy::Strict,
            allowed_distros: Vec::new(),
            primary_profile: None,
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

    /// Set an explicit allowlist for distro/repository identifiers.
    #[must_use]
    pub fn with_allowed_distros(mut self, allowed_distros: Vec<String>) -> Self {
        self.allowed_distros = allowed_distros;
        self
    }

    /// Set the exact distro profile that owns this transaction.
    #[must_use]
    pub fn with_primary_profile(mut self, primary_profile: impl Into<String>) -> Self {
        self.primary_profile = Some(primary_profile.into());
        self
    }

    /// Replace or clear the exact distro profile that owns this transaction.
    pub fn set_primary_profile(&mut self, primary_profile: Option<String>) {
        self.primary_profile = primary_profile;
    }

    /// Return the exact distro profile that owns this transaction.
    #[must_use]
    pub fn primary_profile(&self) -> Option<&str> {
        self.primary_profile.as_deref()
    }

    /// Validate every profile-bearing policy value against the exact public
    /// profile catalog.
    ///
    /// Constructors intentionally remain lightweight data builders. Runtime
    /// boundaries call this before a policy can select or reject candidates,
    /// so route slugs, package formats, repository names, and misspellings
    /// never become source authority.
    pub fn validate_profile_ids(&self) -> Result<(), String> {
        let validate = |field: &str, profile: &str| {
            super::supported_profiles::profile_by_public_id(profile)
                .map(|_| ())
                .ok_or_else(|| {
                    format!(
                        "{field} contains unsupported source profile '{profile}'; use an exact public profile ID"
                    )
                })
        };

        if let Some(primary) = self.primary_profile() {
            validate("primary profile", primary)?;
        }

        let mut seen = std::collections::HashSet::new();
        for allowed in &self.allowed_distros {
            validate("source allowlist", allowed)?;
            if !seen.insert(allowed.as_str()) {
                return Err(format!(
                    "source allowlist contains duplicate exact profile '{allowed}'"
                ));
            }
        }

        if let RequestScope::DistroProfile(profile) = &self.request_scope {
            validate("request scope", profile)?;
            if let Some(primary) = self.primary_profile()
                && primary != profile
            {
                return Err(format!(
                    "request scope profile '{profile}' conflicts with transaction profile '{primary}'"
                ));
            }
        }

        if self.mixing == DependencyMixingPolicy::Strict
            && let Some(primary) = self.primary_profile()
            && !self.allowed_distros.is_empty()
            && !self
                .allowed_distros
                .iter()
                .any(|allowed| allowed == primary)
        {
            return Err(format!(
                "strict transaction profile '{primary}' is excluded by the source allowlist"
            ));
        }

        Ok(())
    }

    /// Validate that strict dependency solving has one exact transaction
    /// profile. Guarded and permissive policies may operate without one, but
    /// strict resolution must never infer authority from a package name,
    /// repository label, URL, or package format.
    pub fn validate_for_dependency_resolution(&self) -> Result<(), String> {
        self.validate_profile_ids()?;
        if self.mixing == DependencyMixingPolicy::Strict
            && self.primary_profile().is_none()
            && !matches!(self.request_scope, RequestScope::Repository(_))
        {
            return Err(
                "strict dependency resolution requires an exact transaction source profile or native CCS repository scope; pin the system, select --from/--repo, or resolve a repository-backed root package first"
                    .to_string(),
            );
        }
        Ok(())
    }

    /// Evaluate whether a candidate is acceptable for a given package name
    /// under this policy.
    ///
    /// Callers pass the repository name and exact repository profile directly
    /// from persisted package identity.
    ///
    /// `is_root` indicates whether this is a root-level request (user-typed)
    /// or a transitive dependency.  Request-scope restrictions apply only to
    /// root requests.
    ///
    #[must_use]
    pub fn accepts_candidate(
        &self,
        repository_name: &str,
        repository_profile: Option<&str>,
        is_root: bool,
    ) -> bool {
        if !self.allowed_distros.is_empty()
            && !self
                .allowed_distros
                .iter()
                .any(|allowed| repository_profile == Some(allowed.as_str()))
        {
            return false;
        }

        // Step 1: Check request scope (root requests only).
        if is_root {
            match &self.request_scope {
                RequestScope::Any => {}
                RequestScope::Repository(repo) => {
                    if repository_name != repo.as_str() {
                        return false;
                    }
                }
                RequestScope::DistroProfile(profile) => {
                    if repository_profile != Some(profile.as_str()) {
                        return false;
                    }
                }
            }
        }

        // Step 2: Enforce one exact transaction profile under strict policy.
        //
        // An explicit root scope is already checked above. An unscoped root
        // may establish a profile later, but an unprofiled transitive
        // candidate can never satisfy strict mixing: accepting it would turn
        // package format or repository naming into an implicit authority.
        if self.mixing == DependencyMixingPolicy::Strict {
            match self.primary_profile() {
                Some(primary) if repository_profile != Some(primary) => return false,
                Some(_) => {}
                None => match &self.request_scope {
                    RequestScope::Repository(repository)
                        if repository_name == repository.as_str() => {}
                    _ if !is_root => return false,
                    _ => {}
                },
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEDORA_REPOSITORY: &str = "fedora";
    const UBUNTU_REPOSITORY: &str = "ubuntu";

    #[test]
    fn mixing_policy_parses_only_exact_persisted_values() {
        for (raw, expected) in [
            ("strict", DependencyMixingPolicy::Strict),
            ("guarded", DependencyMixingPolicy::Guarded),
            ("permissive", DependencyMixingPolicy::Permissive),
        ] {
            assert_eq!(raw.parse(), Ok(expected));
            assert_eq!(expected.as_str(), raw);
        }
        for invalid in ["hard", "Strict", " strict", "guarded "] {
            assert!(
                invalid.parse::<DependencyMixingPolicy>().is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn resolution_policy_rejects_candidates_outside_allowed_distros() {
        let policy = ResolutionPolicy::new().with_allowed_distros(vec!["fedora-44".to_string()]);

        assert!(policy.accepts_candidate(FEDORA_REPOSITORY, Some("fedora-44"), true,));
        assert!(!policy.accepts_candidate(UBUNTU_REPOSITORY, Some("ubuntu-26.04"), true,));
    }

    #[test]
    fn allowed_profile_matches_exact_repository_profile_not_repo_name_shape() {
        let policy = ResolutionPolicy::new().with_allowed_distros(vec!["fedora-44".to_string()]);

        assert!(policy.accepts_candidate("updates", Some("fedora-44"), true,));
        assert!(!policy.accepts_candidate("updates", Some("ubuntu-26.04"), true,));
    }

    #[test]
    fn default_strict_policy_allows_root_discovery_but_rejects_unprofiled_transitives() {
        let policy = ResolutionPolicy::default();
        assert!(policy.accepts_candidate(FEDORA_REPOSITORY, Some("fedora-44"), true));
        assert!(!policy.accepts_candidate(FEDORA_REPOSITORY, Some("fedora-44"), false,));
    }

    #[test]
    fn strict_mixing_rejects_a_different_exact_profile() {
        let policy = ResolutionPolicy::default().with_primary_profile("fedora-44");
        assert!(!policy.accepts_candidate("opensuse", Some("opensuse-tumbleweed"), false,));
    }

    #[test]
    fn strict_mixing_does_not_override_explicit_root_source_selection() {
        let policy = ResolutionPolicy::default()
            .with_scope(RequestScope::DistroProfile("ubuntu-26.04".into()))
            .with_primary_profile("ubuntu-26.04");
        assert!(policy.accepts_candidate(UBUNTU_REPOSITORY, Some("ubuntu-26.04"), true,));
    }

    #[test]
    fn strict_mixing_allows_the_same_exact_profile() {
        let policy = ResolutionPolicy::default().with_primary_profile("fedora-44");
        assert!(policy.accepts_candidate(FEDORA_REPOSITORY, Some("fedora-44"), false,));
    }

    #[test]
    fn strict_mixing_never_equates_two_rpm_profiles() {
        let policy = ResolutionPolicy::default().with_primary_profile("fedora-44");
        assert!(policy.accepts_candidate(FEDORA_REPOSITORY, Some("fedora-44"), false,));
        assert!(!policy.accepts_candidate("opensuse", Some("opensuse-tumbleweed"), false,));
    }

    #[test]
    fn request_scope_repo_filters_root_only() {
        let policy =
            ResolutionPolicy::new().with_scope(RequestScope::Repository("fedora".to_string()));

        assert!(!policy.accepts_candidate(UBUNTU_REPOSITORY, None, true));
        assert!(policy.accepts_candidate(FEDORA_REPOSITORY, None, true));
        assert!(!policy.accepts_candidate(UBUNTU_REPOSITORY, None, false));
    }

    #[test]
    fn request_scope_profile_does_not_collapse_to_package_format() {
        let policy =
            ResolutionPolicy::new().with_scope(RequestScope::DistroProfile("fedora-44".into()));

        assert!(policy.accepts_candidate("fedora-repository", Some("fedora-44"), true,));
        assert!(!policy.accepts_candidate(
            "opensuse-repository",
            Some("opensuse-tumbleweed"),
            true,
        ));
    }

    #[test]
    fn guarded_mixing_allows_cross_profile() {
        let policy = ResolutionPolicy::new().with_mixing(DependencyMixingPolicy::Guarded);
        assert!(policy.accepts_candidate(UBUNTU_REPOSITORY, Some("ubuntu-26.04"), false,));
    }

    #[test]
    fn permissive_mixing_allows_anything() {
        let policy = ResolutionPolicy::new().with_mixing(DependencyMixingPolicy::Permissive);
        assert!(policy.accepts_candidate(UBUNTU_REPOSITORY, Some("ubuntu-26.04"), false,));
    }

    #[test]
    fn profile_validation_rejects_routes_formats_and_duplicates() {
        for invalid in ["fedora", "rpm", "ubuntu"] {
            let error = ResolutionPolicy::new()
                .with_allowed_distros(vec![invalid.to_string()])
                .validate_profile_ids()
                .unwrap_err();
            assert!(error.contains("exact public profile ID"), "{error}");
        }

        let error = ResolutionPolicy::new()
            .with_allowed_distros(vec!["fedora-44".to_string(), "fedora-44".to_string()])
            .validate_profile_ids()
            .unwrap_err();
        assert!(error.contains("duplicate exact profile"), "{error}");
    }

    #[test]
    fn strict_dependency_resolution_requires_exact_transaction_profile() {
        let error = ResolutionPolicy::new()
            .validate_for_dependency_resolution()
            .unwrap_err();
        assert!(
            error.contains("exact transaction source profile"),
            "{error}"
        );

        ResolutionPolicy::new()
            .with_primary_profile("fedora-44")
            .validate_for_dependency_resolution()
            .unwrap();
    }

    #[test]
    fn strict_native_ccs_repository_scope_is_its_own_dependency_authority() {
        let policy =
            ResolutionPolicy::new().with_scope(RequestScope::Repository("native-ccs".to_string()));

        policy.validate_for_dependency_resolution().unwrap();
        assert!(policy.accepts_candidate("native-ccs", None, true));
        assert!(policy.accepts_candidate("native-ccs", None, false));
        assert!(!policy.accepts_candidate("other-ccs", None, false));
    }

    #[test]
    fn profile_validation_rejects_conflicting_scope_and_primary() {
        let error = ResolutionPolicy::new()
            .with_scope(RequestScope::DistroProfile("ubuntu-26.04".to_string()))
            .with_primary_profile("fedora-44")
            .validate_profile_ids()
            .unwrap_err();
        assert!(
            error.contains("conflicts with transaction profile"),
            "{error}"
        );
    }
}
