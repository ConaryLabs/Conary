// conary-core/src/repository/resolution_policy.rs

//! Repository resolution policy types.
//!
//! Policy rules control which repositories may satisfy which requests, how
//! cross-source mixing is handled, and how the resolver filters candidates.
//!
//! Design principles:
//! - Explicit request scope (`--repo`, `--from`) applies only to root
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

    /// The user selected one exact persisted native source identity.
    SourceIdentity(String),
}

// ---------------------------------------------------------------------------
// Dependency mixing policy
// ---------------------------------------------------------------------------

/// How aggressively dependencies may mix exact source identities.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DependencyMixingPolicy {
    /// Packages must come from one exact source identity.
    /// This is the safest setting and is the default.
    #[default]
    Strict,

    /// Packages prefer one exact source identity but may fall back to others.
    /// The resolver logs a warning for each cross-source resolution.
    Guarded,

    /// Any repository may satisfy any dependency regardless of source identity.
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

    /// Exact native source identity that owns this transaction.
    ///
    /// This is intentionally separate from package format. Fedora and
    /// openSUSE are both RPM ecosystems but are not interchangeable source
    /// authorities. Runtime loaders populate this from an explicit source
    /// scope or repository declaration. It is never inferred from a distro
    /// name, package format, repository label, or URL.
    primary_source_identity: Option<String>,
}

impl Default for ResolutionPolicy {
    fn default() -> Self {
        Self {
            request_scope: RequestScope::Any,
            mixing: DependencyMixingPolicy::Strict,
            primary_source_identity: None,
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

    /// Set the exact native source identity that owns this transaction.
    #[must_use]
    pub fn with_primary_source_identity(mut self, source_identity: impl Into<String>) -> Self {
        self.primary_source_identity = Some(source_identity.into());
        self
    }

    /// Replace or clear the exact native source identity for this transaction.
    pub fn set_primary_source_identity(&mut self, source_identity: Option<String>) {
        self.primary_source_identity = source_identity;
    }

    /// Return the exact native source identity that owns this transaction.
    #[must_use]
    pub fn primary_source_identity(&self) -> Option<&str> {
        self.primary_source_identity.as_deref()
    }

    /// Validate exact source identities without consulting a named catalog.
    pub fn validate_source_identities(&self) -> Result<(), String> {
        if let Some(primary) = self.primary_source_identity() {
            validate_source_identity(primary, "primary source identity")?;
        }

        if let RequestScope::SourceIdentity(source_identity) = &self.request_scope {
            validate_source_identity(source_identity, "request source identity")?;
            if let Some(primary) = self.primary_source_identity()
                && primary != source_identity
            {
                return Err(format!(
                    "request source identity '{source_identity}' conflicts with transaction source identity '{primary}'"
                ));
            }
        }
        Ok(())
    }

    /// Validate that strict dependency solving has one exact transaction
    /// source identity. Guarded and permissive policies may operate without one, but
    /// strict resolution must never infer authority from a package name,
    /// repository label, URL, or package format.
    pub fn validate_for_dependency_resolution(&self) -> Result<(), String> {
        self.validate_source_identities()?;
        if self.mixing == DependencyMixingPolicy::Strict
            && self.primary_source_identity().is_none()
            && !matches!(self.request_scope, RequestScope::Repository(_))
        {
            return Err(
                "strict dependency resolution requires an exact transaction source identity or native CCS repository scope; select --from/--repo or resolve a repository-backed root package first"
                    .to_string(),
            );
        }
        Ok(())
    }

    /// Evaluate whether a candidate is acceptable for a given package name
    /// under this policy.
    ///
    /// Callers pass the repository name and exact source identity directly
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
        source_identity: Option<&str>,
        is_root: bool,
    ) -> bool {
        // Step 1: Check request scope (root requests only).
        if is_root {
            match &self.request_scope {
                RequestScope::Any => {}
                RequestScope::Repository(repo) => {
                    if repository_name != repo.as_str() {
                        return false;
                    }
                }
                RequestScope::SourceIdentity(expected) => {
                    if source_identity != Some(expected.as_str()) {
                        return false;
                    }
                }
            }
        }

        // Step 2: Enforce one exact transaction source under strict policy.
        //
        // An explicit root scope is already checked above. An unscoped root
        // may establish an identity later, but an unidentified transitive
        // candidate can never satisfy strict mixing: accepting it would turn
        // package format or repository naming into an implicit authority.
        if self.mixing == DependencyMixingPolicy::Strict {
            match self.primary_source_identity() {
                Some(primary) if source_identity != Some(primary) => return false,
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

/// Validate the closed lexical envelope shared by persisted and request source
/// identities. Semantic authority comes from the signed repository policy,
/// not membership in a distro-name catalog.
pub fn validate_source_identity(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 255
        || value.trim() != value
        || !value.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
    {
        return Err(format!(
            "{label} must contain 1 to 255 printable ASCII characters without surrounding whitespace"
        ));
    }
    Ok(())
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
    fn default_strict_policy_allows_root_discovery_but_rejects_unprofiled_transitives() {
        let policy = ResolutionPolicy::default();
        assert!(policy.accepts_candidate(FEDORA_REPOSITORY, Some("fedora-44"), true));
        assert!(!policy.accepts_candidate(FEDORA_REPOSITORY, Some("fedora-44"), false,));
    }

    #[test]
    fn strict_mixing_rejects_a_different_exact_profile() {
        let policy = ResolutionPolicy::default().with_primary_source_identity("fedora-44");
        assert!(!policy.accepts_candidate("opensuse", Some("opensuse-tumbleweed"), false,));
    }

    #[test]
    fn strict_mixing_does_not_override_explicit_root_source_selection() {
        let policy = ResolutionPolicy::default()
            .with_scope(RequestScope::SourceIdentity("ubuntu-26.04".into()))
            .with_primary_source_identity("ubuntu-26.04");
        assert!(policy.accepts_candidate(UBUNTU_REPOSITORY, Some("ubuntu-26.04"), true,));
    }

    #[test]
    fn strict_mixing_allows_the_same_exact_profile() {
        let policy = ResolutionPolicy::default().with_primary_source_identity("fedora-44");
        assert!(policy.accepts_candidate(FEDORA_REPOSITORY, Some("fedora-44"), false,));
    }

    #[test]
    fn strict_mixing_never_equates_two_rpm_profiles() {
        let policy = ResolutionPolicy::default().with_primary_source_identity("fedora-44");
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
    fn request_scope_source_identity_does_not_collapse_to_package_format() {
        let policy =
            ResolutionPolicy::new().with_scope(RequestScope::SourceIdentity("fedora-44".into()));

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
    fn source_identity_validation_is_lexical_not_catalog_based() {
        ResolutionPolicy::new()
            .with_primary_source_identity("opensuse:tumbleweed:x86_64")
            .validate_source_identities()
            .unwrap();
        for invalid in ["", " leading", "trailing ", "line\nbreak"] {
            assert!(
                ResolutionPolicy::new()
                    .with_primary_source_identity(invalid)
                    .validate_source_identities()
                    .is_err(),
                "{invalid:?}"
            );
        }
    }

    #[test]
    fn strict_dependency_resolution_requires_exact_transaction_profile() {
        let error = ResolutionPolicy::new()
            .validate_for_dependency_resolution()
            .unwrap_err();
        assert!(
            error.contains("exact transaction source identity"),
            "{error}"
        );

        ResolutionPolicy::new()
            .with_primary_source_identity("fedora-44")
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
    fn source_identity_validation_rejects_conflicting_scope_and_primary() {
        let error = ResolutionPolicy::new()
            .with_scope(RequestScope::SourceIdentity("ubuntu-26.04".to_string()))
            .with_primary_source_identity("fedora-44")
            .validate_source_identities()
            .unwrap_err();
        assert!(
            error.contains("conflicts with transaction source identity"),
            "{error}"
        );
    }
}
