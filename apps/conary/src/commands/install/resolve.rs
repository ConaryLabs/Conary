// apps/conary/src/commands/install/resolve.rs
//! Package path resolution - downloading from repository if needed
//!
//! This module handles resolving package names to local file paths, using
//! the unified resolution flow with per-package routing strategies.
//!
//! # Resolution Flow
//!
//! 1. Check if package is a local file path
//! 2. Check for package redirects (renames, obsoletes)
//! 3. Use unified resolver to:
//!    a. Select best repository (priority/version logic)
//!    b. Look up routing strategies in `package_resolution` table
//!    c. Try strategies in order (binary, Remi, recipe, delegate, repository)
//! 4. Return local path to downloaded/built package

use super::super::open_db;
use crate::commands::progress::{InstallPhase, InstallProgress};
use anyhow::Result;
#[cfg(test)]
use conary_core::db::models::ProvideEntry;
use conary_core::db::models::Redirect;
use conary_core::db::paths::keyring_dir;
use conary_core::repository::resolution_policy::ResolutionPolicy;
use conary_core::repository::{
    PackageSource, RepositorySourceMetadata, ResolutionOptions, resolve_package,
};
#[cfg(test)]
use conary_core::version::VersionConstraint;
#[cfg(test)]
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tracing::info;

/// Result of resolving a package path
pub struct ResolvedPackage {
    pub path: PathBuf,
    /// Temp directory that must stay alive until installation completes
    pub _temp_dir: Option<TempDir>,
    /// Exact acquisition path used for lifecycle handling.
    pub source_type: ResolvedSourceType,
    pub repository_provenance: Option<RepositorySourceMetadata>,
}

/// Outcome of package resolution - either resolved to a path or already installed
pub enum ResolutionOutcome {
    /// Package resolved to a downloadable/local path
    Resolved(ResolvedPackage),
    /// Package is already installed at the requested version
    AlreadyInstalled { name: String, version: String },
}

/// Type of source the package was resolved from
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedSourceType {
    /// Local file provided by user
    LocalFile,
    /// Downloaded binary from repository
    Binary,
    /// Verified CCS artifact from a repository
    Ccs,
}

/// Options that control policy-aware resolution at the install layer.
#[derive(Debug, Clone, Default)]
pub struct PolicyOptions {
    /// Resolution policy to filter candidates.
    pub policy: Option<ResolutionPolicy>,
    /// Whether this is a root (user-typed) request.
    pub is_root: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct PackageResolutionRequest<'a> {
    pub package: &'a str,
    pub db_path: &'a str,
    pub version: Option<&'a str>,
    pub package_release: Option<&'a str>,
    pub repository: Option<&'a str>,
    pub architecture: Option<&'a str>,
}

fn build_resolution_options(
    version: Option<&str>,
    package_release: Option<&str>,
    repo: Option<&str>,
    architecture: Option<&str>,
    policy_opts: &PolicyOptions,
) -> ResolutionOptions {
    ResolutionOptions {
        version: version.map(String::from),
        package_release: package_release.map(String::from),
        repository: repo.map(String::from),
        architecture: architecture.map(String::from),
        output_dir: None,
        skip_installed: false,
        policy: policy_opts.policy.clone(),
        is_root: policy_opts.is_root,
    }
}

/// Resolve package to a local path with explicit policy control.
///
/// Like `resolve_package_path` but accepts a `PolicyOptions` to constrain
/// candidate selection via `ResolutionPolicy`.
pub async fn resolve_package_path_with_policy(
    request: PackageResolutionRequest<'_>,
    progress: &InstallProgress,
    policy_opts: &PolicyOptions,
) -> Result<ResolutionOutcome> {
    let PackageResolutionRequest {
        package,
        db_path,
        version,
        package_release,
        repository,
        architecture,
    } = request;
    // Check if package is a local file
    if Path::new(package).exists() {
        info!("Installing from local file: {}", package);
        progress.set_status(&format!("Loading local file: {}", package));
        return Ok(ResolutionOutcome::Resolved(ResolvedPackage {
            path: PathBuf::from(package),
            _temp_dir: None,
            source_type: ResolvedSourceType::LocalFile,
            repository_provenance: None,
        }));
    }

    info!("Searching repositories for package: {}", package);
    progress.set_status("Searching repositories...");

    let conn = open_db(db_path)?;

    // Check for package redirects (renames, obsoletes, etc.)
    let resolved_name = resolve_redirects(&conn, package, version);

    // Build resolution options
    // Note: keyring_dir will be used when GPG options are integrated into resolution
    let _keyring_dir = keyring_dir(db_path);
    let options = build_resolution_options(
        version,
        package_release,
        repository,
        architecture,
        policy_opts,
    );

    // Use unified resolver
    progress.set_status("Resolving package source...");
    let source = resolve_package(&conn, &resolved_name, &options)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve package '{}': {}", package, e))?;

    // Convert PackageSource to ResolvedPackage
    convert_source_to_resolved(source, package, progress)
}

/// Resolve package redirects (renames, obsoletes)
fn resolve_redirects(conn: &rusqlite::Connection, package: &str, version: Option<&str>) -> String {
    match Redirect::resolve(conn, package, version) {
        Ok(result) => {
            if result.was_redirected {
                // Print redirect messages to user
                for msg in &result.messages {
                    eprintln!("Note: {}", msg);
                }
                eprintln!(
                    "Note: '{}' has been redirected to '{}'",
                    package, result.resolved
                );
                info!(
                    "Package '{}' redirected to '{}' (chain: {})",
                    package,
                    result.resolved,
                    result.chain.join(" -> ")
                );
                result.resolved
            } else {
                package.to_string()
            }
        }
        Err(e) => {
            // Log the error but continue with original name
            // (redirect table might not exist on older DBs)
            info!(
                "Redirect check failed (continuing with original name): {}",
                e
            );
            package.to_string()
        }
    }
}

/// Convert a PackageSource to a ResolutionOutcome
fn convert_source_to_resolved(
    source: PackageSource,
    package: &str,
    progress: &InstallProgress,
) -> Result<ResolutionOutcome> {
    match source {
        PackageSource::Binary {
            path,
            _temp_dir,
            repository_provenance,
        } => {
            info!(
                "Resolved {} from binary source: {}",
                package,
                path.display()
            );
            progress.set_phase(package, InstallPhase::Downloading);
            Ok(ResolutionOutcome::Resolved(ResolvedPackage {
                path,
                _temp_dir,
                source_type: ResolvedSourceType::Binary,
                repository_provenance,
            }))
        }

        PackageSource::Ccs {
            path,
            _temp_dir,
            repository_provenance,
        } => {
            info!(
                "Resolved {} as a repository CCS artifact: {}",
                package,
                path.display()
            );
            progress.set_phase(package, InstallPhase::Downloading);
            Ok(ResolutionOutcome::Resolved(ResolvedPackage {
                path,
                _temp_dir,
                source_type: ResolvedSourceType::Ccs,
                repository_provenance,
            }))
        }

        PackageSource::Installed { name, version, .. } => {
            info!(
                "Package {} {} is already installed, skipping download",
                name, version
            );
            Ok(ResolutionOutcome::AlreadyInstalled { name, version })
        }
    }
}

/// Check if missing dependencies are satisfied by packages in the provides table
///
/// This is a self-contained approach that doesn't query the host package manager.
/// Instead, it checks if any tracked package provides the required capability.
///
/// Returns a tuple of:
/// - satisfied: Vec of (dep_name, provider_name, version)
/// - unsatisfied: Vec of MissingDependency (cloned)
#[allow(clippy::type_complexity)]
#[cfg(test)]
pub fn check_provides_dependencies(
    conn: &Connection,
    missing: &[conary_core::resolver::MissingDependency],
) -> Result<(
    Vec<(String, String, Option<String>)>,
    Vec<conary_core::resolver::MissingDependency>,
)> {
    let mut satisfied = Vec::new();
    let mut unsatisfied = Vec::new();

    for dep in missing {
        // Check only declared capability metadata. Repository/AppStream sync
        // owns capability normalization; install should not guess providers.
        let providers = ProvideEntry::find_declared_provider_contracts(conn, &dep.name)?;
        let mut satisfying_provider = None;
        for provider in providers {
            let satisfies = match &dep.constraint {
                VersionConstraint::Any => true,
                constraint => {
                    let Some(capability_version) = provider.capability_version.as_deref() else {
                        continue;
                    };
                    super::dep_resolution::version_satisfies_constraint(
                        provider.version_scheme,
                        capability_version,
                        constraint,
                    )?
                }
            };
            if satisfies {
                satisfying_provider = Some((provider.package_name, provider.package_version));
                break;
            }
        }
        if let Some((provider, version)) = satisfying_provider {
            satisfied.push((dep.name.clone(), provider, Some(version)));
        } else {
            unsatisfied.push(dep.clone());
        }
    }

    Ok((satisfied, unsatisfied))
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{ProvideEntry, Trove, TroveType};
    use conary_core::db::schema;
    use conary_core::repository::RepositorySourceKind;
    use conary_core::resolver::MissingDependency;
    use conary_core::version::VersionConstraint;
    use rusqlite::Connection;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;",
        )
        .unwrap();
        schema::ensure_current(&conn).unwrap();
        conn
    }

    #[test]
    fn test_get_keyring_dir() {
        let keyring = keyring_dir("/var/lib/conary/conary.db");
        assert!(keyring.ends_with("keys"));
    }

    #[test]
    fn test_build_resolution_options_preserves_architecture() {
        let options = build_resolution_options(
            Some("1.2.3"),
            Some("4"),
            Some("stable"),
            Some("x86_64"),
            &PolicyOptions {
                is_root: true,
                ..Default::default()
            },
        );

        assert_eq!(options.version.as_deref(), Some("1.2.3"));
        assert_eq!(options.package_release.as_deref(), Some("4"));
        assert_eq!(options.repository.as_deref(), Some("stable"));
        assert_eq!(options.architecture.as_deref(), Some("x86_64"));
        assert!(options.is_root);
    }

    #[test]
    fn check_provides_dependencies_does_not_guess_package_name_variations() {
        let conn = test_db();
        let mut trove = Trove::new(
            "glibc".to_string(),
            "2.42".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(&conn).unwrap();
        ProvideEntry::new(
            trove_id,
            "glibc".to_string(),
            Some("2.42".to_string()),
            conary_core::repository::versioning::VersionScheme::Conary,
        )
        .insert(&conn)
        .unwrap();

        let missing = vec![MissingDependency {
            name: "libc.so.6".to_string(),
            constraint: VersionConstraint::Any,
            required_by: vec!["demo".to_string()],
        }];

        let (satisfied, unsatisfied) = check_provides_dependencies(&conn, &missing).unwrap();

        assert!(satisfied.is_empty());
        assert_eq!(unsatisfied, missing);
    }

    #[test]
    fn check_provides_dependencies_accepts_declared_capability_key() {
        let conn = test_db();
        let mut trove = Trove::new(
            "openssl-libs".to_string(),
            "3.1.0".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        let trove_id = trove.insert(&conn).unwrap();
        ProvideEntry::new_typed(
            trove_id,
            conary_core::repository::dependency_model::RepositoryCapabilityKind::Soname,
            "libssl.so.3".to_string(),
            None,
            conary_core::repository::versioning::VersionScheme::Conary,
            Default::default(),
        )
        .insert(&conn)
        .unwrap();

        let missing = vec![MissingDependency {
            name: "libssl.so.3".to_string(),
            constraint: VersionConstraint::Any,
            required_by: vec!["demo".to_string()],
        }];

        let (satisfied, unsatisfied) = check_provides_dependencies(&conn, &missing).unwrap();

        assert_eq!(
            satisfied,
            vec![(
                "libssl.so.3".to_string(),
                "openssl-libs".to_string(),
                Some("3.1.0".to_string())
            )]
        );
        assert!(unsatisfied.is_empty());
    }

    #[test]
    fn versioned_requirement_rejects_unversioned_declared_capability() {
        let conn = test_db();
        let mut trove = Trove::new(
            "provider".to_string(),
            "9.0".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Debian,
        );
        trove.debian_multi_arch =
            Some(conary_core::repository::dependency_model::DebianMultiArch::No);
        let trove_id = trove.insert(&conn).unwrap();
        ProvideEntry::new_typed(
            trove_id,
            conary_core::repository::dependency_model::RepositoryCapabilityKind::Virtual,
            "virtual-abi".to_string(),
            None,
            conary_core::repository::versioning::VersionScheme::Debian,
            Default::default(),
        )
        .insert(&conn)
        .unwrap();

        let missing = vec![MissingDependency {
            name: "virtual-abi".to_string(),
            constraint: VersionConstraint::parse(">= 2.0").unwrap(),
            required_by: vec!["consumer".to_string()],
        }];

        let (satisfied, unsatisfied) = check_provides_dependencies(&conn, &missing).unwrap();
        assert!(satisfied.is_empty());
        assert_eq!(unsatisfied, missing);
    }

    #[test]
    fn versioned_requirement_checks_all_exact_provider_contracts() {
        let conn = test_db();
        for (name, package_version, capability_version) in [
            ("old-provider", "1.0", "1.0"),
            ("new-provider", "3.0", "3.0"),
        ] {
            let mut trove = Trove::new(
                name.to_string(),
                package_version.to_string(),
                TroveType::Package,
                conary_core::repository::versioning::VersionScheme::Rpm,
            );
            let trove_id = trove.insert(&conn).unwrap();
            ProvideEntry::new_typed(
                trove_id,
                conary_core::repository::dependency_model::RepositoryCapabilityKind::Virtual,
                "virtual-abi".to_string(),
                Some(capability_version.to_string()),
                conary_core::repository::versioning::VersionScheme::Rpm,
                Default::default(),
            )
            .insert(&conn)
            .unwrap();
        }

        let missing = vec![MissingDependency {
            name: "virtual-abi".to_string(),
            constraint: VersionConstraint::parse(">= 2.0").unwrap(),
            required_by: vec!["consumer".to_string()],
        }];

        let (satisfied, unsatisfied) = check_provides_dependencies(&conn, &missing).unwrap();
        assert_eq!(
            satisfied,
            vec![(
                "virtual-abi".to_string(),
                "new-provider".to_string(),
                Some("3.0".to_string())
            )]
        );
        assert!(unsatisfied.is_empty());
    }

    #[test]
    fn convert_source_preserves_remi_repository_provenance() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("tree.ccs");
        std::fs::write(&path, b"ccs").unwrap();
        let progress = InstallProgress::single("Testing");

        let outcome = convert_source_to_resolved(
            PackageSource::Ccs {
                path,
                _temp_dir: None,
                repository_provenance: Some(RepositorySourceMetadata {
                    repository_id: 42,
                    source_profile: Some("fedora-44".to_string()),
                    version_scheme: conary_core::repository::versioning::VersionScheme::Rpm,
                    source_kind: RepositorySourceKind::Remi,
                }),
            },
            "tree",
            &progress,
        )
        .unwrap();

        let ResolutionOutcome::Resolved(resolved) = outcome else {
            panic!("expected resolved package");
        };

        let provenance = resolved
            .repository_provenance
            .expect("Remi CCS resolution should keep repository provenance");
        assert_eq!(resolved.source_type, ResolvedSourceType::Ccs);
        assert_eq!(provenance.repository_id, 42);
        assert_eq!(provenance.source_profile.as_deref(), Some("fedora-44"));
        assert_eq!(
            provenance.version_scheme,
            conary_core::repository::versioning::VersionScheme::Rpm
        );
        assert_eq!(provenance.source_kind, RepositorySourceKind::Remi);
    }
}
