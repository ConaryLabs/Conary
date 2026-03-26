// conary-core/src/resolver/sat.rs

//! SAT-based dependency resolution using resolvo.
//!
//! Provides `solve_install` and `solve_removal` functions that use the CDCL SAT
//! solver to find optimal package installation plans with backtracking support.

use resolvo::{ConditionalRequirement, Problem, Solver, UnsolvableOrCancelled};
use rusqlite::Connection;

use crate::error::{Error, Result};
use crate::repository::versioning::repo_version_satisfies;
use crate::version::{RpmVersion, VersionConstraint};

use super::provider::ConaryProvider;
use super::provider::types::ConaryConstraint;

/// Source of a resolved package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SatSource {
    /// Package is already installed on the system.
    Installed,
    /// Package comes from a repository.
    Repository,
}

/// A single package in the SAT resolution result.
#[derive(Debug, Clone)]
pub struct SatPackage {
    pub name: String,
    pub version: String,
    pub source: SatSource,
}

/// Result of SAT-based dependency resolution.
#[derive(Debug)]
pub struct SatResolution {
    /// Packages to install/upgrade, in dependency order.
    pub install_order: Vec<SatPackage>,
    /// Human-readable conflict explanation if unsolvable.
    pub conflict_message: Option<String>,
}

/// Solve an install request using the SAT solver.
///
/// Takes a list of `(package_name, version_constraint)` pairs and returns a
/// `SatResolution` containing the packages to install in dependency order, or
/// a conflict message if the request is unsolvable.
pub fn solve_install(
    conn: &Connection,
    requests: &[(String, VersionConstraint)],
) -> Result<SatResolution> {
    if requests.is_empty() {
        return Ok(SatResolution {
            install_order: Vec::new(),
            conflict_message: None,
        });
    }

    let mut provider = ConaryProvider::new(conn);

    // Load all installed packages as solvables
    provider.load_installed_packages()?;

    // Build the provides index for O(1) capability-to-provider lookups.
    provider.build_provides_index()?;

    // Pre-load canonical equivalents for cross-distro fallback (O(1) lookups).
    provider.load_canonical_index()?;

    // Load repo packages transitively to fixed point: keep discovering new
    // dependency names and loading their repo candidates until no new names appear.
    let mut loaded_names: std::collections::HashSet<String> =
        requests.iter().map(|(n, _)| n.clone()).collect();
    let mut to_load: Vec<String> = loaded_names.iter().cloned().collect();

    while !to_load.is_empty() {
        provider.load_repo_packages_for_names(&to_load)?;

        // Discover new dependency names that we haven't loaded yet
        // (only allocates strings for names not already in loaded_names)
        let mut new_names: Vec<String> = provider
            .new_dependency_names(&loaded_names)
            .into_iter()
            .filter(|n| loaded_names.insert(n.clone()))
            .collect();

        // Also load canonical equivalents of any new names so the solver
        // has candidates available when the fallback fires.
        let equiv_names: Vec<String> = new_names
            .iter()
            .flat_map(|n| provider.canonical_equivalents(n).iter().cloned())
            .filter(|n| loaded_names.insert(n.clone()))
            .collect();
        new_names.extend(equiv_names);

        to_load = new_names;
    }

    // Intern version sets for all dependencies so get_dependencies can find them
    provider.intern_all_dependency_version_sets()?;

    // Build the problem: each request becomes a requirement
    let mut requirements = Vec::new();
    for (name, constraint) in requests {
        let name_id = provider.intern_name(name)?;
        let vs_id = provider.intern_version_set(name_id, constraint.clone())?;
        requirements.push(ConditionalRequirement::from(vs_id));
    }

    let problem = Problem::new().requirements(requirements);

    // Solve
    let mut solver = Solver::new(provider);
    match solver.solve(problem) {
        Ok(solvable_ids) => {
            let provider = solver.provider();
            let mut install_order = Vec::new();
            for sid in &solvable_ids {
                let pkg = provider.get_solvable(*sid);
                install_order.push(SatPackage {
                    name: pkg.name.clone(),
                    version: pkg.version.clone(),
                    source: if pkg.installed_trove_id.is_some() {
                        SatSource::Installed
                    } else {
                        SatSource::Repository
                    },
                });
            }
            Ok(SatResolution {
                install_order,
                conflict_message: None,
            })
        }
        Err(UnsolvableOrCancelled::Unsolvable(conflict)) => {
            let message = conflict.display_user_friendly(&solver).to_string();
            Ok(SatResolution {
                install_order: Vec::new(),
                conflict_message: Some(message),
            })
        }
        Err(UnsolvableOrCancelled::Cancelled(_)) => Err(Error::InitError(
            "Dependency resolution was cancelled".to_string(),
        )),
    }
}

/// Check whether a provider's version satisfies a dependency constraint.
///
/// When the provider has no version, only `Any` constraints are satisfied.
/// Otherwise, the version is parsed according to the constraint's scheme and
/// checked against the constraint.
fn provider_version_satisfies_constraint(
    constraint: &ConaryConstraint,
    provider_version: Option<&str>,
) -> bool {
    match constraint {
        ConaryConstraint::Legacy(VersionConstraint::Any) => true,
        ConaryConstraint::Legacy(vc) => {
            let Some(ver_str) = provider_version else {
                return false;
            };
            RpmVersion::parse(ver_str)
                .map(|v| vc.satisfies(&v))
                .unwrap_or(false)
        }
        ConaryConstraint::Repository {
            constraint: crate::repository::versioning::RepoVersionConstraint::Any,
            ..
        } => true,
        ConaryConstraint::Repository {
            scheme,
            constraint: repo_constraint,
            ..
        } => {
            let Some(ver_str) = provider_version else {
                return false;
            };
            repo_version_satisfies(*scheme, ver_str, repo_constraint)
        }
    }
}

/// Check what packages would break if the given packages are removed.
///
/// Returns the names of packages whose dependencies would be unsatisfied.
///
/// Instead of BFS on package names (which mishandles OR-deps and virtual
/// provides), this evaluates each dependent's full dependency clause set.
/// For OR-deps, a clause is only broken when ALL alternatives are gone.
/// The analysis iterates to a fixed point: breaking one package may cause
/// others to lose a provider, so we re-evaluate until no new breakage is found.
pub fn solve_removal(conn: &Connection, to_remove: &[String]) -> Result<Vec<String>> {
    let mut provider = ConaryProvider::new(conn);

    // Load installed packages
    provider.load_installed_packages()?;

    // Intern version sets for dependencies
    provider.intern_all_dependency_version_sets()?;

    // Build provides index and unfiltered deps for removal analysis
    provider.load_removal_data()?;

    let mut gone_set: std::collections::HashSet<String> = to_remove.iter().cloned().collect();

    let solvable_count = provider.solvable_count();

    // Helper closure: check if a single (dep_name, constraint) alternative
    // is satisfiable by any installed package that is NOT in `gone_set`.
    let alternative_satisfiable = |dep_name: &str,
                                   constraint: &super::provider::ConaryConstraint,
                                   gone: &std::collections::HashSet<String>|
     -> bool {
        // 1. Check provides index -- verify the remaining provider's version
        //    satisfies the constraint.
        let providers = provider.find_providers(dep_name);
        if !providers.is_empty() {
            let any_remaining = providers.iter().any(|(trove_id, prov_version)| {
                let not_gone = provider
                    .trove_name(*trove_id)
                    .is_some_and(|name| !gone.contains(name));
                if !not_gone {
                    return false;
                }
                provider_version_satisfies_constraint(constraint, prov_version.as_deref())
            });
            if any_remaining {
                return true;
            }
        }
        // 2. Fallback: check by package name among loaded solvables.
        let name_match = (0..solvable_count).any(|j| {
            let alt_sid = resolvo::SolvableId(j as u32);
            let alt = provider.get_solvable(alt_sid);
            alt.installed_trove_id.is_some()
                && alt.name == dep_name
                && !gone.contains(&alt.name)
                && super::provider::constraint_matches_package(
                    constraint,
                    &alt.version,
                    alt.version_scheme,
                )
        });
        if name_match {
            return true;
        }
        // 3. If no provider and no name match among tracked packages, this dep
        //    is satisfied by the base system (libc.so.6, ld-linux, etc.) or
        //    another untracked capability. Only flag as broken if we had a
        //    provider that was in the removal set.
        let was_provided_by_removed = providers.iter().any(|(trove_id, _)| {
            provider
                .trove_name(*trove_id)
                .is_some_and(|name| gone.contains(name))
        });
        // Also check if any going-away solvable matches by name
        let name_was_removed = (0..solvable_count).any(|j| {
            let alt_sid = resolvo::SolvableId(j as u32);
            let alt = provider.get_solvable(alt_sid);
            alt.installed_trove_id.is_some() && alt.name == dep_name && gone.contains(&alt.name)
        });
        // If neither -- the dep is untracked, assume base system satisfies it
        !was_provided_by_removed && !name_was_removed
    };

    // Helper closure: check if a SolverDep clause is satisfiable.
    // For Single deps, one alternative must be satisfiable.
    // For OrGroup deps, at least one alternative must be satisfiable.
    let clause_satisfiable =
        |dep: &super::provider::SolverDep, gone: &std::collections::HashSet<String>| -> bool {
            match dep {
                super::provider::SolverDep::Single(name, constraint) => {
                    alternative_satisfiable(name, constraint, gone)
                }
                super::provider::SolverDep::OrGroup(alts) => alts
                    .iter()
                    .any(|(name, constraint)| alternative_satisfiable(name, constraint, gone)),
            }
        };

    // Iterate to fixed point: each round may discover new broken packages
    // whose removal breaks further dependents.
    let mut breaking_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    loop {
        let mut changed = false;

        for i in 0..solvable_count {
            let sid = resolvo::SolvableId(i as u32);
            let pkg = provider.get_solvable(sid);

            // Skip non-installed, already-gone, and already-broken packages
            if pkg.installed_trove_id.is_none()
                || gone_set.contains(&pkg.name)
                || breaking_set.contains(&pkg.name)
            {
                continue;
            }

            if let Some(deps) = provider.get_removal_dependency_list(sid) {
                let any_broken = deps.iter().any(|dep| !clause_satisfiable(dep, &gone_set));

                if any_broken {
                    breaking_set.insert(pkg.name.clone());
                    // A broken package is effectively gone for subsequent
                    // dependency checks -- its provides are unreliable.
                    gone_set.insert(pkg.name.clone());
                    changed = true;
                }
            }
        }

        if !changed {
            break;
        }
    }

    let mut breaking: Vec<String> = breaking_set.into_iter().collect();
    breaking.sort();

    Ok(breaking)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::models::{
        Changeset, ChangesetStatus, DependencyEntry, Repository, RepositoryPackage,
        RepositoryRequirement, Trove, TroveType,
    };

    fn setup_test_db() -> (tempfile::TempDir, Connection) {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        db::init(&db_path).unwrap();
        let conn = db::open(&db_path).unwrap();
        (temp_dir, conn)
    }

    /// Helper: insert a trove and its dependencies into the DB
    fn insert_trove(
        conn: &Connection,
        name: &str,
        version: &str,
        deps: &[(&str, Option<&str>)],
    ) -> i64 {
        let mut changeset = Changeset::new(format!("Install {name}"));
        let _cs_id = changeset.insert(conn).unwrap();
        changeset
            .update_status(conn, ChangesetStatus::Applied)
            .unwrap();

        let mut trove = Trove::new(name.to_string(), version.to_string(), TroveType::Package);
        let trove_id = trove.insert(conn).unwrap();

        for (dep_name, constraint) in deps {
            let mut dep = DependencyEntry::new(
                trove_id,
                dep_name.to_string(),
                None,
                "runtime".to_string(),
                constraint.map(|s| s.to_string()),
            );
            dep.insert(conn).unwrap();
        }

        trove_id
    }

    #[test]
    fn test_simple_install() {
        // A depends on B, both available as installed
        let (_dir, conn) = setup_test_db();
        insert_trove(&conn, "B", "1.0.0", &[]);
        insert_trove(&conn, "A", "1.0.0", &[("B", None)]);

        let result = solve_install(&conn, &[("A".to_string(), VersionConstraint::Any)]).unwrap();

        assert!(result.conflict_message.is_none());
        assert!(!result.install_order.is_empty());

        // Both A and B should be in the result
        let names: Vec<&str> = result
            .install_order
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert!(names.contains(&"A"));
        assert!(names.contains(&"B"));
    }

    #[test]
    fn test_missing_dependency() {
        // A depends on B, but B is not installed or in any repo
        let (_dir, conn) = setup_test_db();
        insert_trove(&conn, "A", "1.0.0", &[("B", Some(">= 2.0.0"))]);

        let result = solve_install(&conn, &[("A".to_string(), VersionConstraint::Any)]).unwrap();

        // Should have a conflict message since B can't be found
        assert!(result.conflict_message.is_some());
    }

    #[test]
    fn test_version_conflict() {
        // A needs B >= 2.0, only B 1.0 is installed
        let (_dir, conn) = setup_test_db();
        insert_trove(&conn, "B", "1.0.0", &[]);
        insert_trove(&conn, "A", "1.0.0", &[("B", Some(">= 2.0.0"))]);

        let result = solve_install(&conn, &[("A".to_string(), VersionConstraint::Any)]).unwrap();

        // B 1.0 doesn't satisfy >= 2.0, so this should report a conflict
        // (unless a repo has B >= 2.0, which it doesn't here)
        assert!(result.conflict_message.is_some());
    }

    #[test]
    fn test_diamond_dependency() {
        // A -> B, C; B -> D; C -> D
        let (_dir, conn) = setup_test_db();
        insert_trove(&conn, "D", "1.0.0", &[]);
        insert_trove(&conn, "B", "1.0.0", &[("D", None)]);
        insert_trove(&conn, "C", "1.0.0", &[("D", None)]);
        insert_trove(&conn, "A", "1.0.0", &[("B", None), ("C", None)]);

        let result = solve_install(&conn, &[("A".to_string(), VersionConstraint::Any)]).unwrap();

        assert!(result.conflict_message.is_none());

        let names: Vec<&str> = result
            .install_order
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert!(names.contains(&"A"));
        assert!(names.contains(&"B"));
        assert!(names.contains(&"C"));
        assert!(names.contains(&"D"));
    }

    #[test]
    fn test_removal_check() {
        // A depends on B; removing B should list A as breaking
        let (_dir, conn) = setup_test_db();
        insert_trove(&conn, "B", "1.0.0", &[]);
        insert_trove(&conn, "A", "1.0.0", &[("B", None)]);

        let breaking = solve_removal(&conn, &["B".to_string()]).unwrap();
        assert!(breaking.contains(&"A".to_string()));
    }

    #[test]
    fn test_removal_transitive() {
        // C depends on B, B depends on A; removing A should break both B and C
        let (_dir, conn) = setup_test_db();
        insert_trove(&conn, "A", "1.0.0", &[]);
        insert_trove(&conn, "B", "1.0.0", &[("A", None)]);
        insert_trove(&conn, "C", "1.0.0", &[("B", None)]);

        let breaking = solve_removal(&conn, &["A".to_string()]).unwrap();
        assert!(breaking.contains(&"B".to_string()));
        assert!(breaking.contains(&"C".to_string()));
    }

    #[test]
    fn test_empty_install() {
        let (_dir, conn) = setup_test_db();
        let result = solve_install(&conn, &[]).unwrap();
        assert!(result.install_order.is_empty());
        assert!(result.conflict_message.is_none());
    }

    #[test]
    fn test_deep_transitive_chain() {
        // A -> B -> C -> D -> E (5-level chain, all installed)
        // Verifies fixed-point loading resolves arbitrarily deep chains
        let (_dir, conn) = setup_test_db();
        insert_trove(&conn, "E", "1.0.0", &[]);
        insert_trove(&conn, "D", "1.0.0", &[("E", None)]);
        insert_trove(&conn, "C", "1.0.0", &[("D", None)]);
        insert_trove(&conn, "B", "1.0.0", &[("C", None)]);
        insert_trove(&conn, "A", "1.0.0", &[("B", None)]);

        let result = solve_install(&conn, &[("A".to_string(), VersionConstraint::Any)]).unwrap();

        assert!(result.conflict_message.is_none());

        let names: Vec<&str> = result
            .install_order
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert!(names.contains(&"A"));
        assert!(names.contains(&"B"));
        assert!(names.contains(&"C"));
        assert!(names.contains(&"D"));
        assert!(names.contains(&"E"));
    }

    #[test]
    fn test_sat_install_uses_repo_native_debian_constraints_via_provider() {
        let (_dir, conn) = setup_test_db();

        let mut repo = Repository::new(
            "ubuntu-main".to_string(),
            "https://archive.ubuntu.com/ubuntu".to_string(),
        );
        let repo_id = repo.insert(&conn).unwrap();

        let mut app = RepositoryPackage::new(
            repo_id,
            "myapp".to_string(),
            "2.0-1".to_string(),
            "sha256:app".to_string(),
            1,
            "https://archive.ubuntu.com/ubuntu/pool/main/m/myapp.deb".to_string(),
        );
        app.insert(&conn).unwrap();
        let app_id = app.id.unwrap();

        let mut app_req = RepositoryRequirement::new(
            app_id,
            "libfoo".to_string(),
            Some(">= 1.0~beta1".to_string()),
            "package".to_string(),
            "runtime".to_string(),
            Some("libfoo (>= 1.0~beta1)".to_string()),
        );
        app_req.insert(&conn).unwrap();

        let mut libfoo = RepositoryPackage::new(
            repo_id,
            "libfoo".to_string(),
            "1.0-1".to_string(),
            "sha256:libfoo".to_string(),
            1,
            "https://archive.ubuntu.com/ubuntu/pool/main/libf/libfoo.deb".to_string(),
        );
        libfoo.insert(&conn).unwrap();

        let result =
            solve_install(&conn, &[("myapp".to_string(), VersionConstraint::Any)]).unwrap();

        assert!(result.conflict_message.is_none(), "{result:?}");
        let names: Vec<&str> = result
            .install_order
            .iter()
            .map(|pkg| pkg.name.as_str())
            .collect();
        assert!(names.contains(&"myapp"));
        assert!(names.contains(&"libfoo"));
    }

    #[test]
    fn test_sat_install_uses_repo_native_arch_constraints_via_provider() {
        let (_dir, conn) = setup_test_db();

        let mut repo = Repository::new(
            "arch-core".to_string(),
            "https://geo.mirror.pkgbuild.com".to_string(),
        );
        let repo_id = repo.insert(&conn).unwrap();

        let mut app = RepositoryPackage::new(
            repo_id,
            "ripgrep".to_string(),
            "14.1.0-1".to_string(),
            "sha256:ripgrep".to_string(),
            1,
            "https://geo.mirror.pkgbuild.com/core/os/x86_64/ripgrep.pkg.tar.zst".to_string(),
        );
        app.insert(&conn).unwrap();
        let app_id = app.id.unwrap();

        let mut app_req = RepositoryRequirement::new(
            app_id,
            "glibc".to_string(),
            Some(">= 2.39".to_string()),
            "package".to_string(),
            "runtime".to_string(),
            Some("glibc >= 2.39".to_string()),
        );
        app_req.insert(&conn).unwrap();

        let mut glibc = RepositoryPackage::new(
            repo_id,
            "glibc".to_string(),
            "2.39-1".to_string(),
            "sha256:glibc".to_string(),
            1,
            "https://geo.mirror.pkgbuild.com/core/os/x86_64/glibc.pkg.tar.zst".to_string(),
        );
        glibc.insert(&conn).unwrap();

        let result =
            solve_install(&conn, &[("ripgrep".to_string(), VersionConstraint::Any)]).unwrap();

        assert!(result.conflict_message.is_none(), "{result:?}");
        let names: Vec<&str> = result
            .install_order
            .iter()
            .map(|pkg| pkg.name.as_str())
            .collect();
        assert!(names.contains(&"ripgrep"));
        assert!(names.contains(&"glibc"));
    }

    // ==========================================================================
    // Task 11: Cross-distro SAT and policy regression tests
    // ==========================================================================

    use crate::db::models::{RepositoryProvide, RepositoryRequirementGroup};

    fn insert_provide(conn: &Connection, trove_id: i64, capability: &str, version: Option<&str>) {
        use crate::db::models::ProvideEntry;
        let mut provide =
            ProvideEntry::new(trove_id, capability.to_string(), version.map(String::from));
        provide.insert_or_ignore(conn).unwrap();
    }

    #[test]
    fn test_removal_checks_provides_not_just_names() {
        let (_dir, conn) = setup_test_db();
        let id_a = insert_trove(&conn, "provider-a", "1.0.0", &[]);
        insert_provide(&conn, id_a, "virtual-cap", Some("1.0.0"));
        let id_b = insert_trove(&conn, "provider-b", "1.0.0", &[]);
        insert_provide(&conn, id_b, "virtual-cap", Some("1.0.0"));
        let _id_c = insert_trove(&conn, "consumer", "1.0.0", &[("virtual-cap", None)]);

        let breaking = solve_removal(&conn, &["provider-a".to_string()]).unwrap();
        assert!(
            breaking.is_empty(),
            "Should be safe (provider-b still provides virtual-cap), got: {breaking:?}"
        );
    }

    #[test]
    fn test_removal_blocked_when_sole_provider() {
        let (_dir, conn) = setup_test_db();
        let id_a = insert_trove(&conn, "provider-a", "1.0.0", &[]);
        insert_provide(&conn, id_a, "virtual-cap", Some("1.0.0"));
        let _id_c = insert_trove(&conn, "consumer", "1.0.0", &[("virtual-cap", None)]);

        let breaking = solve_removal(&conn, &["provider-a".to_string()]).unwrap();
        assert!(
            breaking.contains(&"consumer".to_string()),
            "Removing sole provider should break consumer, got: {breaking:?}"
        );
    }

    #[test]
    fn test_removal_with_soname_provides() {
        let (_dir, conn) = setup_test_db();
        let id_glibc = insert_trove(&conn, "glibc", "2.38", &[]);
        insert_provide(&conn, id_glibc, "libc.so.6", Some("2.38"));
        let _id_consumer = insert_trove(&conn, "curl", "8.0", &[("libc.so.6", None)]);
        let _id_other = insert_trove(&conn, "tree", "2.1", &[("libc.so.6", None)]);

        let breaking = solve_removal(&conn, &["tree".to_string()]).unwrap();
        assert!(
            breaking.is_empty(),
            "Removing tree should not break curl (glibc provides libc.so.6), got: {breaking:?}"
        );
    }

    #[test]
    fn test_removal_name_fallback_still_works() {
        let (_dir, conn) = setup_test_db();
        insert_trove(&conn, "B", "1.0.0", &[]);
        let _id_a = insert_trove(&conn, "A", "1.0.0", &[("B", None)]);

        let breaking = solve_removal(&conn, &["B".to_string()]).unwrap();
        assert!(
            breaking.contains(&"A".to_string()),
            "Removing B should break A (name-based dep), got: {breaking:?}"
        );
    }

    #[test]
    fn test_removal_both_providers_breaks_consumer() {
        let (_dir, conn) = setup_test_db();
        let id_a = insert_trove(&conn, "provider-a", "1.0.0", &[]);
        insert_provide(&conn, id_a, "virtual-cap", Some("1.0.0"));
        let id_b = insert_trove(&conn, "provider-b", "1.0.0", &[]);
        insert_provide(&conn, id_b, "virtual-cap", Some("1.0.0"));
        let _id_c = insert_trove(&conn, "consumer", "1.0.0", &[("virtual-cap", None)]);

        let breaking =
            solve_removal(&conn, &["provider-a".to_string(), "provider-b".to_string()]).unwrap();
        assert!(
            breaking.contains(&"consumer".to_string()),
            "Removing all providers should break consumer, got: {breaking:?}"
        );
    }

    #[test]
    fn test_removal_untracked_soname_deps_not_flagged() {
        // Packages adopted from the system have soname deps like
        // "libc.so.6(GLIBC_2.34)(64bit)" that conary doesn't track as
        // provides. Removing any package should NOT flag these as broken.
        let (_dir, conn) = setup_test_db();

        // "bash" has a soname dep that no conary package provides
        let _id_bash = insert_trove(
            &conn,
            "bash",
            "5.2",
            &[("libc.so.6(GLIBC_2.34)(64bit)", None)],
        );
        let _id_tree = insert_trove(&conn, "tree", "2.1", &[]);

        let breaking = solve_removal(&conn, &["tree".to_string()]).unwrap();
        assert!(
            breaking.is_empty(),
            "Untracked soname deps should be treated as system-satisfied, got: {breaking:?}"
        );
    }

    /// Helper: insert a trove with version_scheme and its dependencies
    fn insert_native_trove(
        conn: &Connection,
        name: &str,
        version: &str,
        scheme: &str,
        deps: &[(&str, Option<&str>)],
    ) -> i64 {
        let mut changeset = Changeset::new(format!("Install {name}"));
        let _cs_id = changeset.insert(conn).unwrap();
        changeset
            .update_status(conn, ChangesetStatus::Applied)
            .unwrap();

        let mut trove = Trove::new(name.to_string(), version.to_string(), TroveType::Package);
        trove.version_scheme = Some(scheme.to_string());
        let trove_id = trove.insert(conn).unwrap();

        for (dep_name, constraint) in deps {
            let mut dep = DependencyEntry::new(
                trove_id,
                dep_name.to_string(),
                None,
                "runtime".to_string(),
                constraint.map(|s| s.to_string()),
            );
            dep.insert(conn).unwrap();
        }

        trove_id
    }

    /// Helper: insert a repo package with normalized requirements
    fn insert_repo_pkg_with_reqs(
        conn: &Connection,
        repo_id: i64,
        name: &str,
        version: &str,
        url: &str,
        reqs: &[(&str, Option<&str>)],
    ) -> i64 {
        let mut pkg = RepositoryPackage::new(
            repo_id,
            name.to_string(),
            version.to_string(),
            format!("sha256:{name}"),
            1,
            url.to_string(),
        );
        pkg.insert(conn).unwrap();
        let pkg_id = pkg.id.unwrap();

        for (cap, constraint) in reqs {
            let mut req = RepositoryRequirement::new(
                pkg_id,
                cap.to_string(),
                constraint.map(|s| s.to_string()),
                "package".to_string(),
                "runtime".to_string(),
                None,
            );
            req.insert(conn).unwrap();
        }

        pkg_id
    }

    #[test]
    fn rpm_transitive_capability_chain() {
        // kernel -> kernel-core-uname-r = X (capability provided by kernel-core)
        // kernel-core -> glibc >= 2.39
        let (_dir, conn) = setup_test_db();

        let mut repo = Repository::new(
            "fedora-main".to_string(),
            "https://mirror.fedora.invalid".to_string(),
        );
        let repo_id = repo.insert(&conn).unwrap();

        // kernel-core provides kernel-core-uname-r
        let kc_id = insert_repo_pkg_with_reqs(
            &conn,
            repo_id,
            "kernel-core",
            "6.19.6-200.fc43",
            "https://mirror.fedora.invalid/kernel-core.rpm",
            &[("glibc", Some(">= 2.39"))],
        );
        let mut provide = RepositoryProvide::new(
            kc_id,
            "kernel-core-uname-r".to_string(),
            Some("6.19.6-200.fc43.x86_64".to_string()),
            "package".to_string(),
            Some("kernel-core-uname-r = 6.19.6-200.fc43.x86_64".to_string()),
        );
        provide.insert(&conn).unwrap();

        // kernel depends on kernel-core-uname-r
        insert_repo_pkg_with_reqs(
            &conn,
            repo_id,
            "kernel",
            "6.19.6-200.fc43",
            "https://mirror.fedora.invalid/kernel.rpm",
            &[("kernel-core-uname-r", None)],
        );

        // glibc (leaf)
        insert_repo_pkg_with_reqs(
            &conn,
            repo_id,
            "glibc",
            "2.39-22.fc43",
            "https://mirror.fedora.invalid/glibc.rpm",
            &[],
        );

        let result =
            solve_install(&conn, &[("kernel".to_string(), VersionConstraint::Any)]).unwrap();
        assert!(
            result.conflict_message.is_none(),
            "RPM capability chain should resolve: {:?}",
            result.conflict_message
        );
        let names: Vec<&str> = result
            .install_order
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert!(names.contains(&"kernel"));
        assert!(names.contains(&"kernel-core"));
        assert!(names.contains(&"glibc"));
    }

    #[test]
    fn debian_or_plus_versioned_virtual_dep() {
        // postfix depends on "default-mta | mail-transport-agent"
        // exim4 provides mail-transport-agent
        let (_dir, conn) = setup_test_db();

        let mut repo = Repository::new(
            "ubuntu-main".to_string(),
            "https://archive.ubuntu.com/ubuntu".to_string(),
        );
        let repo_id = repo.insert(&conn).unwrap();

        // exim4 provides "mail-transport-agent"
        let exim_id = insert_repo_pkg_with_reqs(
            &conn,
            repo_id,
            "exim4",
            "4.97-1",
            "https://archive.ubuntu.com/ubuntu/pool/exim4.deb",
            &[],
        );
        let mut provide = RepositoryProvide::new(
            exim_id,
            "mail-transport-agent".to_string(),
            None,
            "package".to_string(),
            None,
        );
        provide.insert(&conn).unwrap();

        // bsd-mailx has OR dep: default-mta | mail-transport-agent (via groups)
        let mut mailx = RepositoryPackage::new(
            repo_id,
            "bsd-mailx".to_string(),
            "8.1.2-0.20220412cvs-1build2".to_string(),
            "sha256:mailx".to_string(),
            1,
            "https://archive.ubuntu.com/ubuntu/pool/bsd-mailx.deb".to_string(),
        );
        mailx.insert(&conn).unwrap();
        let mailx_id = mailx.id.unwrap();

        let mut group =
            RepositoryRequirementGroup::new(mailx_id, "depends".to_string(), "hard".to_string());
        group.native_text = Some("default-mta | mail-transport-agent".to_string());
        group.insert(&conn).unwrap();
        let group_id = group.id.unwrap();

        let mut clause_a = RepositoryRequirement::new(
            mailx_id,
            "default-mta".to_string(),
            None,
            "package".to_string(),
            "runtime".to_string(),
            None,
        )
        .with_group(group_id);
        clause_a.insert(&conn).unwrap();

        let mut clause_b = RepositoryRequirement::new(
            mailx_id,
            "mail-transport-agent".to_string(),
            None,
            "package".to_string(),
            "runtime".to_string(),
            None,
        )
        .with_group(group_id);
        clause_b.insert(&conn).unwrap();

        let result =
            solve_install(&conn, &[("bsd-mailx".to_string(), VersionConstraint::Any)]).unwrap();

        assert!(
            result.conflict_message.is_none(),
            "Debian OR dep should resolve via mail-transport-agent provider: {:?}",
            result.conflict_message
        );
        let names: Vec<&str> = result
            .install_order
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert!(names.contains(&"bsd-mailx"));
        assert!(
            names.contains(&"exim4"),
            "exim4 should be pulled in via OR dep"
        );
    }

    #[test]
    fn arch_versioned_provider_chain() {
        // ripgrep -> glibc >= 2.39 -> (leaf)
        // Uses Arch version semantics throughout
        let (_dir, conn) = setup_test_db();

        let mut repo = Repository::new(
            "arch-core".to_string(),
            "https://geo.mirror.pkgbuild.com".to_string(),
        );
        let repo_id = repo.insert(&conn).unwrap();

        insert_repo_pkg_with_reqs(
            &conn,
            repo_id,
            "glibc",
            "2.39-1",
            "https://geo.mirror.pkgbuild.com/core/glibc.pkg.tar.zst",
            &[],
        );
        insert_repo_pkg_with_reqs(
            &conn,
            repo_id,
            "ripgrep",
            "14.1.0-1",
            "https://geo.mirror.pkgbuild.com/extra/ripgrep.pkg.tar.zst",
            &[("glibc", Some(">= 2.39"))],
        );

        let result =
            solve_install(&conn, &[("ripgrep".to_string(), VersionConstraint::Any)]).unwrap();
        assert!(result.conflict_message.is_none(), "{result:?}");
        let names: Vec<&str> = result
            .install_order
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert!(names.contains(&"ripgrep"));
        assert!(names.contains(&"glibc"));
    }

    #[test]
    fn installed_debian_satisfies_debian_dep_in_sat() {
        // libc6 is already installed (Debian scheme).
        // Installing myapp which depends on libc6 >= 2.38 should find it installed.
        let (_dir, conn) = setup_test_db();

        insert_native_trove(&conn, "libc6", "2.39-0ubuntu2", "debian", &[]);

        let mut repo = Repository::new(
            "ubuntu-main".to_string(),
            "https://archive.ubuntu.com/ubuntu".to_string(),
        );
        let repo_id = repo.insert(&conn).unwrap();

        insert_repo_pkg_with_reqs(
            &conn,
            repo_id,
            "myapp",
            "1.0-1",
            "https://archive.ubuntu.com/ubuntu/pool/myapp.deb",
            &[("libc6", Some(">= 2.38"))],
        );

        let result =
            solve_install(&conn, &[("myapp".to_string(), VersionConstraint::Any)]).unwrap();
        assert!(
            result.conflict_message.is_none(),
            "Installed Debian libc6 should satisfy dep: {:?}",
            result.conflict_message
        );

        // libc6 should be in the result as Installed
        let libc6 = result
            .install_order
            .iter()
            .find(|p| p.name == "libc6")
            .unwrap();
        assert_eq!(libc6.source, SatSource::Installed);
    }

    #[test]
    fn provide_version_used_instead_of_package_version() {
        // Package kernel-modules-core version 6.19.6-200.fc43
        // Provides kernel-modules-core-uname-r = 6.19.6-200.fc43.x86_64
        // A dep on kernel-modules-core-uname-r = 6.19.6-200.fc43.x86_64
        // should match via the provide version, not the package version.
        let (_dir, conn) = setup_test_db();

        let mut repo = Repository::new(
            "fedora-main".to_string(),
            "https://mirror.fedora.invalid".to_string(),
        );
        let repo_id = repo.insert(&conn).unwrap();

        // kernel-modules-core with a provide
        let kmc_id = insert_repo_pkg_with_reqs(
            &conn,
            repo_id,
            "kernel-modules-core",
            "6.19.6-200.fc43",
            "https://mirror.fedora.invalid/kernel-modules-core.rpm",
            &[],
        );
        let mut provide = RepositoryProvide::new(
            kmc_id,
            "kernel-modules-core-uname-r".to_string(),
            Some("6.19.6-200.fc43.x86_64".to_string()),
            "package".to_string(),
            None,
        );
        provide.insert(&conn).unwrap();

        // kernel depends on kernel-modules-core-uname-r (any version)
        insert_repo_pkg_with_reqs(
            &conn,
            repo_id,
            "kernel",
            "6.19.6-200.fc43",
            "https://mirror.fedora.invalid/kernel.rpm",
            &[("kernel-modules-core-uname-r", None)],
        );

        let result =
            solve_install(&conn, &[("kernel".to_string(), VersionConstraint::Any)]).unwrap();
        assert!(
            result.conflict_message.is_none(),
            "Should resolve via provide version: {:?}",
            result.conflict_message
        );
        let names: Vec<&str> = result
            .install_order
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert!(names.contains(&"kernel"));
        assert!(
            names.contains(&"kernel-modules-core"),
            "kernel-modules-core should be pulled in via capability provide"
        );
    }

    #[test]
    fn legacy_installed_rpm_fallback_in_sat() {
        // Legacy trove (no version_scheme) should still participate in SAT
        let (_dir, conn) = setup_test_db();

        insert_trove(&conn, "bash", "5.2.21-2.fc43", &[]);

        let mut repo = Repository::new(
            "fedora-main".to_string(),
            "https://mirror.fedora.invalid".to_string(),
        );
        let repo_id = repo.insert(&conn).unwrap();

        insert_repo_pkg_with_reqs(
            &conn,
            repo_id,
            "myshell",
            "1.0-1.fc43",
            "https://mirror.fedora.invalid/myshell.rpm",
            &[("bash", Some(">= 5.0"))],
        );

        let result =
            solve_install(&conn, &[("myshell".to_string(), VersionConstraint::Any)]).unwrap();
        assert!(
            result.conflict_message.is_none(),
            "Legacy RPM trove should satisfy dep: {:?}",
            result.conflict_message
        );

        let bash = result
            .install_order
            .iter()
            .find(|p| p.name == "bash")
            .unwrap();
        assert_eq!(bash.source, SatSource::Installed);
    }

    // ==========================================================================
    // Canonical fallback tests
    // ==========================================================================

    use crate::db::models::{CanonicalPackage, PackageImplementation};

    /// Helper: insert a canonical package with distro-specific implementations.
    fn insert_canonical(
        conn: &Connection,
        canonical_name: &str,
        impls: &[(&str, &str)], // (distro, distro_name)
    ) {
        let mut pkg = CanonicalPackage::new(canonical_name.to_string(), "package".to_string());
        let can_id = pkg.insert(conn).unwrap();

        for (distro, distro_name) in impls {
            let mut pi = PackageImplementation::new(
                can_id,
                distro.to_string(),
                distro_name.to_string(),
                "auto".to_string(),
            );
            pi.insert(conn).unwrap();
        }
    }

    #[test]
    fn canonical_fallback_resolves_cross_distro_dep() {
        // App depends on "libssl3" (Debian name), but only "openssl" (Fedora)
        // is available in repos. Canonical mapping links them.
        let (_dir, conn) = setup_test_db();

        insert_canonical(
            &conn,
            "openssl",
            &[("fedora", "openssl"), ("debian", "libssl3")],
        );

        let mut repo = Repository::new(
            "fedora-main".to_string(),
            "https://mirror.fedora.invalid".to_string(),
        );
        let repo_id = repo.insert(&conn).unwrap();

        insert_repo_pkg_with_reqs(
            &conn,
            repo_id,
            "openssl",
            "3.2.0-1.fc43",
            "https://mirror.fedora.invalid/openssl.rpm",
            &[],
        );

        insert_repo_pkg_with_reqs(
            &conn,
            repo_id,
            "myapp",
            "1.0-1.fc43",
            "https://mirror.fedora.invalid/myapp.rpm",
            &[("libssl3", None)],
        );

        let result =
            solve_install(&conn, &[("myapp".to_string(), VersionConstraint::Any)]).unwrap();

        assert!(
            result.conflict_message.is_none(),
            "Canonical fallback should resolve libssl3 -> openssl: {:?}",
            result.conflict_message,
        );
        let names: Vec<&str> = result
            .install_order
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert!(names.contains(&"myapp"));
        assert!(
            names.contains(&"openssl"),
            "openssl should be pulled in via canonical fallback for libssl3"
        );
    }

    #[test]
    fn canonical_fallback_not_used_when_exact_match_exists() {
        // When the exact name exists in repos, canonical fallback should not
        // interfere -- the direct candidate should be used.
        let (_dir, conn) = setup_test_db();

        insert_canonical(&conn, "kernel", &[("fedora", "kernel"), ("arch", "linux")]);

        let mut repo = Repository::new(
            "fedora-main".to_string(),
            "https://mirror.fedora.invalid".to_string(),
        );
        let repo_id = repo.insert(&conn).unwrap();

        insert_repo_pkg_with_reqs(
            &conn,
            repo_id,
            "kernel",
            "6.19.6-200.fc43",
            "https://mirror.fedora.invalid/kernel.rpm",
            &[],
        );

        // Also add "linux" in case it leaks through
        insert_repo_pkg_with_reqs(
            &conn,
            repo_id,
            "linux",
            "6.19.6-1",
            "https://mirror.fedora.invalid/linux.rpm",
            &[],
        );

        let result =
            solve_install(&conn, &[("kernel".to_string(), VersionConstraint::Any)]).unwrap();

        assert!(result.conflict_message.is_none());
        let names: Vec<&str> = result
            .install_order
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert!(names.contains(&"kernel"));
        // "linux" should NOT appear -- exact name matched first
        assert!(
            !names.contains(&"linux"),
            "Canonical fallback should not fire when exact name exists"
        );
    }

    #[test]
    fn canonical_fallback_with_no_mapping_still_fails() {
        // When no canonical mapping exists and the name is absent, the
        // solver should still report a conflict.
        let (_dir, conn) = setup_test_db();

        let mut repo = Repository::new(
            "fedora-main".to_string(),
            "https://mirror.fedora.invalid".to_string(),
        );
        let repo_id = repo.insert(&conn).unwrap();

        insert_repo_pkg_with_reqs(
            &conn,
            repo_id,
            "myapp",
            "1.0-1",
            "https://mirror.fedora.invalid/myapp.rpm",
            &[("nonexistent-lib", None)],
        );

        let result =
            solve_install(&conn, &[("myapp".to_string(), VersionConstraint::Any)]).unwrap();

        assert!(
            result.conflict_message.is_some(),
            "Should report conflict when dep has no candidates and no canonical mapping"
        );
    }

    // ==========================================================================
    // Task 10: Integration tests for resolver pipeline redesign
    // ==========================================================================

    use crate::repository::versioning::VersionScheme;
    use crate::resolver::identity::PackageIdentity;
    use crate::resolver::provides_index::ProvidesIndex;

    #[test]
    fn test_cross_distro_canonical_resolution() {
        // Two repos (fedora, debian) with same canonical_id.
        // fedora has "httpd" 2.4, debian has "apache2" 2.4.
        // Both linked to canonical "apache".
        // PackageIdentity should find httpd with its canonical_id,
        // and find_canonical_equivalents should return "apache2".
        let (_dir, conn) = setup_test_db();

        // Create canonical mapping
        conn.execute(
            "INSERT INTO canonical_packages (name, kind) VALUES ('apache', 'package')",
            [],
        )
        .unwrap();
        let canonical_id = conn.last_insert_rowid();

        // Create two repos
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority, default_strategy_distro)
             VALUES ('fedora-41', 'https://f.com', 1, 10, 'fedora-41')",
            [],
        )
        .unwrap();
        let fed_repo = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority, default_strategy_distro)
             VALUES ('debian-bookworm', 'https://d.com', 1, 5, 'debian-bookworm')",
            [],
        )
        .unwrap();
        let deb_repo = conn.last_insert_rowid();

        // Insert packages with canonical links
        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, version_scheme, canonical_id)
             VALUES (?1, 'httpd', '2.4.58', 'sha256:a', 100, 'https://f.com/httpd', 'rpm', ?2)",
            rusqlite::params![fed_repo, canonical_id],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, version_scheme, canonical_id)
             VALUES (?1, 'apache2', '2.4.57', 'sha256:b', 100, 'https://d.com/apache2', 'debian', ?2)",
            rusqlite::params![deb_repo, canonical_id],
        )
        .unwrap();

        // Verify PackageIdentity finds httpd with canonical link
        let httpd = PackageIdentity::find_all_by_name(&conn, "httpd").unwrap();
        assert_eq!(httpd.len(), 1);
        assert_eq!(httpd[0].canonical_id, Some(canonical_id));

        // Verify canonical equivalents
        let equivs = PackageIdentity::find_canonical_equivalents(&conn, "httpd").unwrap();
        assert_eq!(equivs, vec!["apache2"]);
    }

    #[test]
    fn test_multi_arch_candidates_coexist() {
        // Same package name with different architectures should return
        // multiple candidates from PackageIdentity::find_all_by_name.
        let (_dir, conn) = setup_test_db();

        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('fedora', 'https://f.com', 1, 10)",
            [],
        )
        .unwrap();
        let repo_id = conn.last_insert_rowid();

        // Same name, different architectures
        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, architecture, checksum, size, download_url)
             VALUES (?1, 'glibc', '2.38', 'x86_64', 'sha256:a', 100, 'https://f.com/glibc-x64')",
            [repo_id],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, architecture, checksum, size, download_url)
             VALUES (?1, 'glibc', '2.38', 'i686', 'sha256:b', 100, 'https://f.com/glibc-i686')",
            [repo_id],
        )
        .unwrap();

        let candidates = PackageIdentity::find_all_by_name(&conn, "glibc").unwrap();
        assert_eq!(candidates.len(), 2);

        let arches: Vec<_> = candidates
            .iter()
            .map(|c| c.architecture.as_deref())
            .collect();
        assert!(arches.contains(&Some("x86_64")));
        assert!(arches.contains(&Some("i686")));
    }

    #[test]
    fn test_provides_index_cross_source() {
        // ProvidesIndex should aggregate providers from both repository_provides
        // and appstream_provides into a single unified index.
        let (_dir, conn) = setup_test_db();

        // Repo provide
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('fedora', 'https://f.com', 1, 10)",
            [],
        )
        .unwrap();
        let repo_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url)
             VALUES (?1, 'openssl-libs', '3.2', 'sha256:a', 100, 'https://f.com/x')",
            [repo_id],
        )
        .unwrap();
        let pkg_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO repository_provides (repository_package_id, capability, version, kind)
             VALUES (?1, 'libssl.so.3', '3.2', 'library')",
            [pkg_id],
        )
        .unwrap();

        // AppStream provide (cross-distro)
        conn.execute(
            "INSERT INTO canonical_packages (name, kind) VALUES ('openssl', 'package')",
            [],
        )
        .unwrap();
        let canonical_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO appstream_provides (canonical_id, provide_type, capability)
             VALUES (?1, 'library', 'libcrypto.so.3')",
            [canonical_id],
        )
        .unwrap();

        let index = ProvidesIndex::build(&conn).unwrap();

        // Repo provide found
        assert_eq!(index.find_providers("libssl.so.3").len(), 1);
        // AppStream provide found
        assert_eq!(index.find_providers("libcrypto.so.3").len(), 1);
        // Unknown not found
        assert!(index.find_providers("libfoo.so.1").is_empty());
    }

    #[test]
    fn test_version_scheme_explicit_over_inferred() {
        // When a package has an explicit version_scheme that differs from
        // the repo's distro inference, the explicit scheme should win.
        let (_dir, conn) = setup_test_db();

        // Repo with fedora distro but package has explicit debian scheme
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority, default_strategy_distro)
             VALUES ('mixed-repo', 'https://m.com', 1, 10, 'fedora-41')",
            [],
        )
        .unwrap();
        let repo_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, version_scheme)
             VALUES (?1, 'pkg', '1.0', 'sha256:a', 100, 'https://m.com/x', 'debian')",
            [repo_id],
        )
        .unwrap();

        let identities = PackageIdentity::find_all_by_name(&conn, "pkg").unwrap();
        assert_eq!(identities.len(), 1);
        // Explicit debian scheme should win over fedora inference
        assert_eq!(identities[0].version_scheme, VersionScheme::Debian);
    }
}
