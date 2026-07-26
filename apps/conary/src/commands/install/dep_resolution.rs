// apps/conary/src/commands/install/dep_resolution.rs
//! Repository-backed dependency planning.
//!
//! Normal installation never asks a native package manager or probes the live
//! filesystem to decide whether a requirement is satisfied. Installed
//! providers come from Conary's persisted provider graph; missing providers
//! must resolve through exact repository metadata.

use anyhow::{Context, Result};
use conary_core::db::models::{Repository, RepositoryPackage};
use conary_core::repository;
#[cfg(test)]
use conary_core::repository::versioning::{
    RepoVersionConstraint, VersionScheme, repo_version_satisfies,
};
use conary_core::resolver::{MissingDependency, SatPackage, SatSource};
#[cfg(test)]
use conary_core::version::VersionConstraint;

/// A dependency selected for installation from a configured repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDep {
    pub package: SatPackage,
    pub required_by: Vec<String>,
}

/// Result of resolving requirements that were not satisfied by Conary's
/// installed provider graph.
#[derive(Debug, Default)]
pub struct DepResolutionPlan {
    pub to_install: Vec<ResolvedDep>,
    pub unresolvable: Vec<MissingDependency>,
}

#[cfg(test)]
pub(super) fn version_satisfies_constraint(
    scheme: VersionScheme,
    version: &str,
    constraint: &VersionConstraint,
) -> Result<bool> {
    match constraint {
        VersionConstraint::Any => Ok(true),
        VersionConstraint::And(left, right) => {
            Ok(version_satisfies_constraint(scheme, version, left)?
                && version_satisfies_constraint(scheme, version, right)?)
        }
        VersionConstraint::Exact(expected) => Ok(repo_version_satisfies(
            scheme,
            version,
            &RepoVersionConstraint::Exact(expected.to_string()),
        )?),
        VersionConstraint::GreaterThan(expected) => Ok(repo_version_satisfies(
            scheme,
            version,
            &RepoVersionConstraint::GreaterThan(expected.to_string()),
        )?),
        VersionConstraint::GreaterOrEqual(expected) => Ok(repo_version_satisfies(
            scheme,
            version,
            &RepoVersionConstraint::GreaterOrEqual(expected.to_string()),
        )?),
        VersionConstraint::LessThan(expected) => Ok(repo_version_satisfies(
            scheme,
            version,
            &RepoVersionConstraint::LessThan(expected.to_string()),
        )?),
        VersionConstraint::LessOrEqual(expected) => Ok(repo_version_satisfies(
            scheme,
            version,
            &RepoVersionConstraint::LessOrEqual(expected.to_string()),
        )?),
        VersionConstraint::NotEqual(expected) => Ok(repo_version_satisfies(
            scheme,
            version,
            &RepoVersionConstraint::NotEqual(expected.to_string()),
        )?),
    }
}

pub fn exact_repository_downloads(
    conn: &rusqlite::Connection,
    selected: &[ResolvedDep],
) -> Result<Vec<(String, repository::PackageWithRepo)>> {
    selected
        .iter()
        .map(|dependency| {
            let expected = &dependency.package;
            if expected.source != SatSource::Repository {
                anyhow::bail!(
                    "dependency '{}' is not a SAT-selected repository package",
                    expected.name
                );
            }
            let package_id = expected
                .repo_package_id
                .context("SAT-selected repository dependency has no repository package identity")?;
            let repository_id = expected
                .repository_id
                .context("SAT-selected repository dependency has no repository identity")?;
            let package = RepositoryPackage::find_by_id(conn, package_id)?
                .context("SAT-selected repository package row disappeared before execution")?;
            let repository = Repository::find_by_id(conn, repository_id)?
                .context("SAT-selected repository row disappeared before execution")?;
            let exact = package.repository_id == repository_id
                && package.name == expected.name
                && package.version == expected.version
                && Some(package.package_release.as_str()) == expected.package_release.as_deref()
                && package.architecture == expected.architecture
                && package.version_scheme == expected.version_scheme
                && expected.repository_name.as_deref() == Some(repository.name.as_str());
            if !exact {
                anyhow::bail!(
                    "SAT-selected repository identity for '{}-{}' drifted before execution",
                    expected.name,
                    expected.version
                );
            }
            Ok((
                expected.name.clone(),
                repository::PackageWithRepo {
                    package,
                    repository,
                },
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{Repository, RepositoryPackage};
    use conary_core::db::schema;
    use conary_core::repository::versioning::VersionScheme;

    fn test_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        schema::ensure_current(&conn).unwrap();
        conn
    }

    fn add_repository_package(
        conn: &rusqlite::Connection,
        name: &str,
        version: &str,
    ) -> RepositoryPackage {
        let repo_id = match Repository::find_by_name(conn, "test").unwrap() {
            Some(repo) => repo.id.unwrap(),
            None => {
                let mut repo =
                    Repository::new("test".to_string(), "https://example.invalid".to_string());
                repo.insert(conn).unwrap();
                repo.id.unwrap()
            }
        };
        let mut package = RepositoryPackage::new(
            repo_id,
            name.to_string(),
            version.to_string(),
            VersionScheme::Rpm,
            format!("sha256:{name}"),
            1,
            format!("https://example.invalid/{name}.ccs"),
        );
        package.insert(conn).unwrap();
        package
    }

    fn selected(package: &RepositoryPackage, repository: &Repository) -> ResolvedDep {
        ResolvedDep {
            package: SatPackage {
                name: package.name.clone(),
                version: package.version.clone(),
                package_release: Some(package.package_release.clone()),
                architecture: package.architecture.clone(),
                version_scheme: package.version_scheme,
                repo_package_id: package.id,
                repository_id: repository.id,
                repository_name: Some(repository.name.clone()),
                installed_trove_id: None,
                source: SatSource::Repository,
            },
            required_by: vec!["consumer".to_string()],
        }
    }

    #[test]
    fn exact_repository_download_preserves_selected_row() {
        let conn = test_db();
        let mut package = add_repository_package(&conn, "openssl-libs", "3.5.1-1");
        package.package_release = "7".to_string();
        conn.execute(
            "UPDATE repository_packages SET package_release = '7' WHERE id = ?1",
            [package.id.unwrap()],
        )
        .unwrap();
        let repository = Repository::find_by_name(&conn, "test").unwrap().unwrap();

        let downloads =
            exact_repository_downloads(&conn, &[selected(&package, &repository)]).unwrap();

        assert_eq!(downloads.len(), 1);
        assert_eq!(downloads[0].1.package.id, package.id);
        assert_eq!(downloads[0].1.package.package_release, "7");
    }

    #[test]
    fn exact_repository_download_rejects_row_drift() {
        let conn = test_db();
        let package = add_repository_package(&conn, "glibc", "2.43-2");
        let repository = Repository::find_by_name(&conn, "test").unwrap().unwrap();
        let selected = selected(&package, &repository);
        conn.execute(
            "UPDATE repository_packages SET package_release = '9' WHERE id = ?1",
            [package.id.unwrap()],
        )
        .unwrap();

        let error = exact_repository_downloads(&conn, &[selected])
            .unwrap_err()
            .to_string();
        assert!(error.contains("drifted before execution"), "{error}");
    }
}
