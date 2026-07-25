// apps/conary/src/commands/install/dep_resolution.rs
//! Repository-backed dependency planning.
//!
//! Normal installation never asks a native package manager or probes the live
//! filesystem to decide whether a requirement is satisfied. Installed
//! providers come from Conary's persisted provider graph; missing providers
//! must resolve through exact repository metadata.

use anyhow::{Context, Result};
use conary_core::repository;
use conary_core::repository::versioning::{
    RepoVersionConstraint, VersionScheme, repo_version_satisfies,
};
use conary_core::resolver::MissingDependency;
use conary_core::version::VersionConstraint;
use tracing::debug;

/// A dependency selected for installation from a configured repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDep {
    pub name: String,
    pub constraint: VersionConstraint,
    pub required_by: Vec<String>,
}

/// Result of resolving requirements that were not satisfied by Conary's
/// installed provider graph.
#[derive(Debug, Default)]
pub struct DepResolutionPlan {
    pub to_install: Vec<ResolvedDep>,
    pub unresolvable: Vec<MissingDependency>,
}

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

/// Plan every missing requirement against normalized repository facts.
///
/// A successful lookup proves that a configured repository has an exact
/// package or capability provider. A lookup failure remains an explicit
/// unresolved requirement; it is never converted into a live-host guess.
pub fn plan_repository_dependencies(
    conn: &rusqlite::Connection,
    missing: &[MissingDependency],
    options: &repository::SelectionOptions,
) -> Result<DepResolutionPlan> {
    let mut plan = DepResolutionPlan::default();

    for dep in missing {
        let request = [(dep.name.clone(), dep.constraint.clone())];
        match repository::resolve_dependency_requests(conn, &request, options) {
            Ok(resolved) if !resolved.is_empty() => {
                let selected = &resolved[0].1.package;
                let constraint = VersionConstraint::parse(&format!("= {}", selected.version))
                    .with_context(|| {
                        format!(
                            "repository selected invalid version '{}' for dependency '{}'",
                            selected.version, dep.name
                        )
                    })?;
                plan.to_install.push(ResolvedDep {
                    name: selected.name.clone(),
                    constraint,
                    required_by: dep.required_by.clone(),
                });
            }
            Ok(_) | Err(conary_core::Error::NotFound(_)) => {
                plan.unresolvable.push(dep.clone());
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to resolve repository dependency '{}'", dep.name)
                });
            }
        }
    }

    plan.to_install.sort_by(|left, right| {
        left.name.cmp(&right.name).then_with(|| {
            left.constraint
                .to_string()
                .cmp(&right.constraint.to_string())
        })
    });
    plan.to_install
        .dedup_by(|left, right| left.name == right.name && left.constraint == right.constraint);

    debug!(
        "Repository dependency plan: {} to install, {} unresolved",
        plan.to_install.len(),
        plan.unresolvable.len()
    );
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{Repository, RepositoryPackage, RepositoryProvide};
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

    fn dependency(name: &str, constraint: VersionConstraint) -> MissingDependency {
        MissingDependency {
            name: name.to_string(),
            constraint,
            required_by: vec!["consumer".to_string()],
        }
    }

    #[test]
    fn missing_package_becomes_exact_repository_install() {
        let conn = test_db();
        add_repository_package(&conn, "openssl-libs", "3.5.1-1");

        let plan = plan_repository_dependencies(
            &conn,
            &[dependency("openssl-libs", VersionConstraint::Any)],
            &repository::SelectionOptions::default(),
        )
        .unwrap();

        assert!(plan.unresolvable.is_empty());
        assert_eq!(plan.to_install.len(), 1);
        assert_eq!(plan.to_install[0].name, "openssl-libs");
        assert_eq!(plan.to_install[0].constraint.to_string(), "= 3.5.1-1");
    }

    #[test]
    fn normalized_capability_selects_declared_provider() {
        let conn = test_db();
        let package = add_repository_package(&conn, "glibc", "2.43-2");
        let mut provide = RepositoryProvide::new(
            package.id.unwrap(),
            "libc.so.6(GLIBC_2.34)(64bit)".to_string(),
            None,
            "soname".to_string(),
            None,
            VersionScheme::Rpm,
        );
        provide.insert(&conn).unwrap();

        let plan = plan_repository_dependencies(
            &conn,
            &[dependency(
                "libc.so.6(GLIBC_2.34)(64bit)",
                VersionConstraint::Any,
            )],
            &repository::SelectionOptions::default(),
        )
        .unwrap();

        assert!(plan.unresolvable.is_empty());
        assert_eq!(plan.to_install[0].name, "glibc");
    }

    #[test]
    fn package_names_and_runtime_capabilities_have_no_special_exceptions() {
        let conn = test_db();
        let missing = vec![
            dependency("systemd", VersionConstraint::Any),
            dependency("libc.so.6(GLIBC_2.34)(64bit)", VersionConstraint::Any),
            dependency("/usr/bin/sh", VersionConstraint::Any),
        ];

        let plan =
            plan_repository_dependencies(&conn, &missing, &repository::SelectionOptions::default())
                .unwrap();

        assert!(plan.to_install.is_empty());
        assert_eq!(plan.unresolvable, missing);
    }
}
