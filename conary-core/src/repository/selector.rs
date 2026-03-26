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
use crate::repository::dependency_model::RepositoryDependencyFlavor;
use crate::repository::resolution_policy::ResolutionPolicy;
use crate::repository::versioning::compare_repo_package_versions;
use rusqlite::Connection;
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
                let flavor = infer_repo_flavor(&repo);
                let scheme = flavor_to_scheme(flavor);
                if !policy.accepts_candidate(
                    &repo.name,
                    scheme,
                    package_name,
                    options.is_root,
                    options.primary_flavor,
                ) {
                    debug!(
                        "Policy rejected package {} {} from repository {} (flavor {:?})",
                        pkg.name, pkg.version, repo.name, flavor
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

    /// Select the best package from a list of candidates
    ///
    /// Selection criteria (in order of priority):
    /// 1. Repository priority (higher is better)
    /// 2. Version (latest version, using scheme-aware comparison)
    /// 3. Repository name as stable tie-breaker (avoids non-determinism)
    pub fn select_best(mut candidates: Vec<PackageWithRepo>) -> Result<PackageWithRepo> {
        if candidates.is_empty() {
            return Err(Error::NotFound("No matching packages found".to_string()));
        }

        candidates.sort_by(
            |a, b| match b.repository.priority.cmp(&a.repository.priority) {
                std::cmp::Ordering::Equal => match compare_repo_package_versions(
                    &a.package,
                    &a.repository,
                    &b.package,
                    &b.repository,
                ) {
                    Some(ord) => ord.reverse(),
                    // Cross-scheme comparison is incomparable -- fall back to
                    // repository name ordering so results are deterministic
                    // without inventing a synthetic version ordering.
                    None => {
                        debug!(
                            "Incomparable version schemes for {} ({}) vs {} ({}); using repo name order",
                            a.repository.name, a.package.version,
                            b.repository.name, b.package.version,
                        );
                        a.repository.name.cmp(&b.repository.name)
                    }
                },
                ord => ord,
            },
        );

        // Safe: we verified candidates is non-empty above
        let selected = candidates.into_iter().next().unwrap();
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

        Self::select_best(candidates)
    }
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

/// Convert a `RepositoryDependencyFlavor` to the corresponding `VersionScheme`.
///
/// This bridges `infer_repo_flavor` output to the `VersionScheme` that
/// `ResolutionPolicy::accepts_candidate` now expects.
fn flavor_to_scheme(
    flavor: RepositoryDependencyFlavor,
) -> crate::repository::versioning::VersionScheme {
    use crate::repository::versioning::VersionScheme;
    match flavor {
        RepositoryDependencyFlavor::Rpm => VersionScheme::Rpm,
        RepositoryDependencyFlavor::Deb => VersionScheme::Debian,
        RepositoryDependencyFlavor::Arch => VersionScheme::Arch,
    }
}

/// Infer the distro flavor of a repository from its name and URL.
///
/// This bridges the gap between the repository model (which stores name/URL)
/// and the policy model (which operates on `RepositoryDependencyFlavor`).
fn infer_repo_flavor(repo: &Repository) -> RepositoryDependencyFlavor {
    use crate::repository::registry::{RepositoryFormat, detect_repository_format};
    match detect_repository_format(&repo.name, &repo.url) {
        RepositoryFormat::Fedora => RepositoryDependencyFlavor::Rpm,
        RepositoryFormat::Debian => RepositoryDependencyFlavor::Deb,
        RepositoryFormat::Arch => RepositoryDependencyFlavor::Arch,
        RepositoryFormat::Json => {
            // Best-effort: check name patterns
            let name = repo.name.to_lowercase();
            if name.contains("fedora")
                || name.contains("rhel")
                || name.contains("centos")
                || name.contains("suse")
            {
                RepositoryDependencyFlavor::Rpm
            } else if name.contains("ubuntu") || name.contains("debian") || name.contains("mint") {
                RepositoryDependencyFlavor::Deb
            } else if name.contains("arch") || name.contains("manjaro") {
                RepositoryDependencyFlavor::Arch
            } else {
                // Default to RPM as the most common
                RepositoryDependencyFlavor::Rpm
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{Repository, RepositoryPackage};
    use crate::db::schema;
    use crate::repository::resolution_policy::{
        DependencyMixingPolicy, RequestScope, ResolutionPolicy,
    };
    use rusqlite::Connection;

    #[test]
    fn test_detect_architecture() {
        let arch = PackageSelector::detect_architecture();
        // Should return one of the known architectures
        assert!(!arch.is_empty());
        // On most development machines, this will be x86_64
        println!("Detected architecture: {}", arch);
    }

    #[test]
    fn test_architecture_compatibility() {
        let system_arch = "x86_64";

        // noarch is compatible with everything
        assert!(PackageSelector::is_architecture_compatible(
            Some("noarch"),
            system_arch
        ));

        // Exact match is compatible
        assert!(PackageSelector::is_architecture_compatible(
            Some("x86_64"),
            system_arch
        ));

        // Different arch is not compatible
        assert!(!PackageSelector::is_architecture_compatible(
            Some("aarch64"),
            system_arch
        ));

        // None (unknown) is compatible
        assert!(PackageSelector::is_architecture_compatible(
            None,
            system_arch
        ));
    }

    #[test]
    fn test_debian_amd64_compatible_with_x86_64() {
        assert!(PackageSelector::is_architecture_compatible(
            Some("amd64"),
            "x86_64"
        ));
    }

    #[test]
    fn test_debian_arm64_compatible_with_aarch64() {
        assert!(PackageSelector::is_architecture_compatible(
            Some("arm64"),
            "aarch64"
        ));
    }

    #[test]
    fn test_debian_i386_compatible_with_i686() {
        assert!(PackageSelector::is_architecture_compatible(
            Some("i386"),
            "i686"
        ));
    }

    #[test]
    fn test_normalize_arch_mappings() {
        assert_eq!(normalize_arch("amd64"), "x86_64");
        assert_eq!(normalize_arch("arm64"), "aarch64");
        assert_eq!(normalize_arch("i386"), "i686");
        assert_eq!(normalize_arch("i486"), "i686");
        assert_eq!(normalize_arch("i586"), "i686");
        assert_eq!(normalize_arch("x86_64"), "x86_64");
        assert_eq!(normalize_arch("aarch64"), "aarch64");
        assert_eq!(normalize_arch("riscv64"), "riscv64");
    }

    #[test]
    fn test_debian_all_architecture_compatible() {
        assert!(PackageSelector::is_architecture_compatible(
            Some("all"),
            "x86_64"
        ));
    }

    #[test]
    fn test_arch_any_architecture_compatible() {
        assert!(PackageSelector::is_architecture_compatible(
            Some("any"),
            "x86_64"
        ));
    }

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn select_best_uses_debian_version_ordering() {
        let conn = test_db();

        let mut repo = Repository::new(
            "ubuntu-noble".to_string(),
            "https://archive.ubuntu.com/ubuntu".to_string(),
        );
        repo.priority = 10;
        repo.insert(&conn).unwrap();
        let repository = Repository::find_by_name(&conn, "ubuntu-noble")
            .unwrap()
            .unwrap();
        let repo_id = repository.id.unwrap();

        let mut prerelease = RepositoryPackage::new(
            repo_id,
            "demo".to_string(),
            "1.0~beta1".to_string(),
            "sha256:beta".to_string(),
            1,
            "https://archive.ubuntu.com/ubuntu/pool/demo_1.0~beta1_amd64.deb".to_string(),
        );
        prerelease.architecture = Some("x86_64".to_string());
        prerelease.insert(&conn).unwrap();

        let mut stable = RepositoryPackage::new(
            repo_id,
            "demo".to_string(),
            "1.0".to_string(),
            "sha256:stable".to_string(),
            1,
            "https://archive.ubuntu.com/ubuntu/pool/demo_1.0_amd64.deb".to_string(),
        );
        stable.architecture = Some("x86_64".to_string());
        stable.insert(&conn).unwrap();

        let candidates =
            PackageSelector::search_packages(&conn, "demo", &SelectionOptions::default()).unwrap();
        let selected = PackageSelector::select_best(candidates).unwrap();

        assert_eq!(selected.package.version, "1.0");
    }

    #[test]
    fn policy_repo_scope_filters_root_request() {
        let conn = test_db();

        // Create two repos: fedora and ubuntu
        let mut fedora_repo = Repository::new(
            "fedora-43".to_string(),
            "https://mirrors.fedoraproject.org/metalink".to_string(),
        );
        fedora_repo.priority = 10;
        fedora_repo.insert(&conn).unwrap();
        let fedora = Repository::find_by_name(&conn, "fedora-43")
            .unwrap()
            .unwrap();

        let mut ubuntu_repo = Repository::new(
            "ubuntu-noble".to_string(),
            "https://archive.ubuntu.com/ubuntu".to_string(),
        );
        ubuntu_repo.priority = 10;
        ubuntu_repo.insert(&conn).unwrap();
        let ubuntu = Repository::find_by_name(&conn, "ubuntu-noble")
            .unwrap()
            .unwrap();

        // Add curl to both
        let mut pkg_fed = RepositoryPackage::new(
            fedora.id.unwrap(),
            "curl".into(),
            "8.9.1".into(),
            "sha256:fed".into(),
            1,
            "https://example.com/curl.rpm".into(),
        );
        pkg_fed.architecture = Some("x86_64".into());
        pkg_fed.insert(&conn).unwrap();

        let mut pkg_ubu = RepositoryPackage::new(
            ubuntu.id.unwrap(),
            "curl".into(),
            "8.5.0".into(),
            "sha256:ubu".into(),
            1,
            "https://example.com/curl.deb".into(),
        );
        pkg_ubu.architecture = Some("x86_64".into());
        pkg_ubu.insert(&conn).unwrap();

        // With --repo fedora-43, root request should only find fedora
        let policy =
            ResolutionPolicy::new().with_scope(RequestScope::Repository("fedora-43".into()));

        let options = SelectionOptions {
            policy: Some(policy),
            is_root: true,
            ..Default::default()
        };
        let candidates = PackageSelector::search_packages(&conn, "curl", &options).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].repository.name, "fedora-43");
    }

    #[test]
    fn policy_repo_scope_does_not_filter_transitive_deps() {
        let conn = test_db();

        let mut fedora_repo = Repository::new(
            "fedora-43".to_string(),
            "https://mirrors.fedoraproject.org/metalink".to_string(),
        );
        fedora_repo.priority = 10;
        fedora_repo.insert(&conn).unwrap();
        let fedora = Repository::find_by_name(&conn, "fedora-43")
            .unwrap()
            .unwrap();

        let mut ubuntu_repo = Repository::new(
            "ubuntu-noble".to_string(),
            "https://archive.ubuntu.com/ubuntu".to_string(),
        );
        ubuntu_repo.priority = 10;
        ubuntu_repo.insert(&conn).unwrap();
        let ubuntu = Repository::find_by_name(&conn, "ubuntu-noble")
            .unwrap()
            .unwrap();

        let mut pkg_fed = RepositoryPackage::new(
            fedora.id.unwrap(),
            "libcurl".into(),
            "8.9.1".into(),
            "sha256:fed".into(),
            1,
            "https://example.com/libcurl.rpm".into(),
        );
        pkg_fed.architecture = Some("x86_64".into());
        pkg_fed.insert(&conn).unwrap();

        let mut pkg_ubu = RepositoryPackage::new(
            ubuntu.id.unwrap(),
            "libcurl".into(),
            "8.5.0".into(),
            "sha256:ubu".into(),
            1,
            "https://example.com/libcurl.deb".into(),
        );
        pkg_ubu.architecture = Some("x86_64".into());
        pkg_ubu.insert(&conn).unwrap();

        // Request scope targets fedora, but is_root=false so scope is ignored
        let policy =
            ResolutionPolicy::new().with_scope(RequestScope::Repository("fedora-43".into()));

        let options = SelectionOptions {
            policy: Some(policy),
            is_root: false,
            ..Default::default()
        };
        let candidates = PackageSelector::search_packages(&conn, "libcurl", &options).unwrap();
        assert_eq!(candidates.len(), 2, "transitive dep sees both repos");
    }

    #[test]
    fn strict_policy_rejects_cross_flavor_dep() {
        let conn = test_db();

        let mut ubuntu_repo = Repository::new(
            "ubuntu-noble".to_string(),
            "https://archive.ubuntu.com/ubuntu".to_string(),
        );
        ubuntu_repo.priority = 10;
        ubuntu_repo.insert(&conn).unwrap();
        let ubuntu = Repository::find_by_name(&conn, "ubuntu-noble")
            .unwrap()
            .unwrap();

        let mut pkg = RepositoryPackage::new(
            ubuntu.id.unwrap(),
            "libssl3".into(),
            "3.0.13".into(),
            "sha256:ssl".into(),
            1,
            "https://example.com/libssl3.deb".into(),
        );
        pkg.architecture = Some("x86_64".into());
        pkg.insert(&conn).unwrap();

        // Strict policy with RPM primary flavor -- debian package should be rejected
        let policy = ResolutionPolicy::new().with_mixing(DependencyMixingPolicy::Strict);

        let options = SelectionOptions {
            policy: Some(policy),
            is_root: false,
            primary_flavor: Some(RepositoryDependencyFlavor::Rpm),
            ..Default::default()
        };
        let candidates = PackageSelector::search_packages(&conn, "libssl3", &options).unwrap();
        assert!(candidates.is_empty(), "strict policy rejects cross-flavor");
    }

    #[test]
    fn permissive_policy_allows_cross_flavor_dep() {
        let conn = test_db();

        let mut ubuntu_repo = Repository::new(
            "ubuntu-noble".to_string(),
            "https://archive.ubuntu.com/ubuntu".to_string(),
        );
        ubuntu_repo.priority = 10;
        ubuntu_repo.insert(&conn).unwrap();
        let ubuntu = Repository::find_by_name(&conn, "ubuntu-noble")
            .unwrap()
            .unwrap();

        let mut pkg = RepositoryPackage::new(
            ubuntu.id.unwrap(),
            "libssl3".into(),
            "3.0.13".into(),
            "sha256:ssl".into(),
            1,
            "https://example.com/libssl3.deb".into(),
        );
        pkg.architecture = Some("x86_64".into());
        pkg.insert(&conn).unwrap();

        let policy = ResolutionPolicy::new().with_mixing(DependencyMixingPolicy::Permissive);

        let options = SelectionOptions {
            policy: Some(policy),
            is_root: false,
            primary_flavor: Some(RepositoryDependencyFlavor::Rpm),
            ..Default::default()
        };
        let candidates = PackageSelector::search_packages(&conn, "libssl3", &options).unwrap();
        assert_eq!(candidates.len(), 1, "permissive policy allows cross-flavor");
    }

    #[test]
    fn guarded_policy_allows_cross_flavor_dep() {
        let conn = test_db();

        let mut ubuntu_repo = Repository::new(
            "ubuntu-noble".to_string(),
            "https://archive.ubuntu.com/ubuntu".to_string(),
        );
        ubuntu_repo.priority = 10;
        ubuntu_repo.insert(&conn).unwrap();
        let ubuntu = Repository::find_by_name(&conn, "ubuntu-noble")
            .unwrap()
            .unwrap();

        let mut pkg = RepositoryPackage::new(
            ubuntu.id.unwrap(),
            "libssl3".into(),
            "3.0.13".into(),
            "sha256:ssl".into(),
            1,
            "https://example.com/libssl3.deb".into(),
        );
        pkg.architecture = Some("x86_64".into());
        pkg.insert(&conn).unwrap();

        // Guarded policy allows cross-flavor but callers should log warnings
        let policy = ResolutionPolicy::new().with_mixing(DependencyMixingPolicy::Guarded);

        let options = SelectionOptions {
            policy: Some(policy),
            is_root: false,
            primary_flavor: Some(RepositoryDependencyFlavor::Rpm),
            ..Default::default()
        };
        let candidates = PackageSelector::search_packages(&conn, "libssl3", &options).unwrap();
        assert_eq!(candidates.len(), 1, "guarded policy allows cross-flavor");
    }

    #[test]
    fn incomparable_cross_scheme_falls_back_to_repo_name_order() {
        let conn = test_db();

        // Create fedora and ubuntu repos at same priority
        let mut fedora_repo = Repository::new(
            "fedora-43".to_string(),
            "https://mirrors.fedoraproject.org/metalink".to_string(),
        );
        fedora_repo.priority = 10;
        fedora_repo.insert(&conn).unwrap();
        let fedora = Repository::find_by_name(&conn, "fedora-43")
            .unwrap()
            .unwrap();

        let mut ubuntu_repo = Repository::new(
            "ubuntu-noble".to_string(),
            "https://archive.ubuntu.com/ubuntu".to_string(),
        );
        ubuntu_repo.priority = 10;
        ubuntu_repo.insert(&conn).unwrap();
        let ubuntu = Repository::find_by_name(&conn, "ubuntu-noble")
            .unwrap()
            .unwrap();

        let mut pkg_fed = RepositoryPackage::new(
            fedora.id.unwrap(),
            "curl".into(),
            "8.9.1-2.fc43".into(),
            "sha256:fed".into(),
            1,
            "https://example.com/curl.rpm".into(),
        );
        pkg_fed.architecture = Some("x86_64".into());
        pkg_fed.insert(&conn).unwrap();

        let mut pkg_ubu = RepositoryPackage::new(
            ubuntu.id.unwrap(),
            "curl".into(),
            "8.5.0-2ubuntu1".into(),
            "sha256:ubu".into(),
            1,
            "https://example.com/curl.deb".into(),
        );
        pkg_ubu.architecture = Some("x86_64".into());
        pkg_ubu.insert(&conn).unwrap();

        // With permissive policy, both candidates are present
        let policy = ResolutionPolicy::new().with_mixing(DependencyMixingPolicy::Permissive);

        let options = SelectionOptions {
            policy: Some(policy),
            is_root: true,
            ..Default::default()
        };
        let candidates = PackageSelector::search_packages(&conn, "curl", &options).unwrap();
        assert_eq!(candidates.len(), 2);

        // select_best should pick deterministically (alphabetical repo name)
        let selected = PackageSelector::select_best(candidates).unwrap();
        // "fedora-43" < "ubuntu-noble" alphabetically, so fedora wins
        assert_eq!(selected.repository.name, "fedora-43");
    }
}
