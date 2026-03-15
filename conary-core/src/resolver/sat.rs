// conary-core/src/resolver/sat.rs

//! SAT-based dependency resolution using resolvo.
//!
//! Provides `solve_install` and `solve_removal` functions that use the CDCL SAT
//! solver to find optimal package installation plans with backtracking support.

use resolvo::{ConditionalRequirement, Problem, Solver, UnsolvableOrCancelled};
use rusqlite::Connection;

use crate::error::{Error, Result};
use crate::version::VersionConstraint;

use super::provider::ConaryProvider;

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

    // Load repo packages transitively to fixed point: keep discovering new
    // dependency names and loading their repo candidates until no new names appear.
    let mut loaded_names: std::collections::HashSet<String> =
        requests.iter().map(|(n, _)| n.clone()).collect();
    let mut to_load: Vec<String> = loaded_names.iter().cloned().collect();

    while !to_load.is_empty() {
        provider.load_repo_packages_for_names(&to_load)?;

        // Discover new dependency names that we haven't loaded yet
        let new_names: Vec<String> = provider
            .dependency_names()
            .into_iter()
            .filter(|n| loaded_names.insert(n.clone()))
            .collect();

        to_load = new_names;
    }

    // Intern version sets for all dependencies so get_dependencies can find them
    provider.intern_all_dependency_version_sets();

    // Build the problem: each request becomes a requirement
    let mut requirements = Vec::new();
    for (name, constraint) in requests {
        let name_id = provider.intern_name(name);
        let vs_id = provider.intern_version_set(name_id, constraint.clone());
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
                    version: pkg.version.to_string(),
                    source: if pkg.trove_id.is_some() {
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

/// Check what packages would break if the given packages are removed.
///
/// Returns the names of packages whose dependencies would be unsatisfied.
pub fn solve_removal(conn: &Connection, to_remove: &[String]) -> Result<Vec<String>> {
    let mut provider = ConaryProvider::new(conn);

    // Load installed packages
    provider.load_installed_packages()?;

    // Intern version sets for dependencies
    provider.intern_all_dependency_version_sets();

    // Build provides index and unfiltered deps for removal analysis
    provider.load_removal_data()?;

    let remove_set: std::collections::HashSet<&str> =
        to_remove.iter().map(String::as_str).collect();

    let solvable_count = provider.solvable_count();

    // Single pass: find directly broken packages and build reverse dependency map
    let mut breaking_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut reverse_deps: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    for i in 0..solvable_count {
        let sid = resolvo::SolvableId(i as u32);
        let pkg = provider.get_solvable(sid);
        if pkg.trove_id.is_none() || remove_set.contains(pkg.name.as_str()) {
            continue;
        }

        if let Some(deps) = provider.get_removal_dependency_list(sid) {
            for dep in deps {
                // Extract simple (name, constraint) pairs for removal analysis.
                // OR groups: if *any* alternative is still installed, the dep is satisfied.
                let singles: Vec<(&str, &super::provider::ConaryConstraint)> = match dep {
                    super::provider::SolverDep::Single(name, constraint) => {
                        vec![(name.as_str(), constraint)]
                    }
                    super::provider::SolverDep::OrGroup(alts) => {
                        alts.iter().map(|(n, c)| (n.as_str(), c)).collect()
                    }
                };

                for &(dep_name, _) in &singles {
                    reverse_deps
                        .entry(dep_name.to_string())
                        .or_default()
                        .push(pkg.name.clone());
                }

                // For OR groups, the dep is broken only if ALL alternatives are gone
                if !breaking_set.contains(&pkg.name) {
                    let any_satisfied = singles.iter().any(|&(dep_name, constraint)| {
                        // 1. Check provides index
                        let providers = provider.find_providers(dep_name);
                        if !providers.is_empty() {
                            return providers.iter().any(|(trove_id, _)| {
                                provider
                                    .trove_name(*trove_id)
                                    .is_some_and(|name| !remove_set.contains(name))
                            });
                        }
                        // 2. Fallback: check by package name with version constraint
                        (0..solvable_count).any(|j| {
                            let alt_sid = resolvo::SolvableId(j as u32);
                            let alt = provider.get_solvable(alt_sid);
                            alt.trove_id.is_some()
                                && alt.name == dep_name
                                && !remove_set.contains(alt.name.as_str())
                                && super::provider::constraint_matches_package(
                                    constraint,
                                    &alt.version,
                                )
                        })
                    });
                    if !any_satisfied {
                        breaking_set.insert(pkg.name.clone());
                    }
                }
            }
        }
    }

    // BFS from breaking packages through reverse deps
    let mut queue: std::collections::VecDeque<String> = breaking_set.iter().cloned().collect();
    while let Some(broken) = queue.pop_front() {
        if let Some(rdeps) = reverse_deps.get(&broken) {
            for rdep in rdeps {
                if !breaking_set.contains(rdep) && !remove_set.contains(rdep.as_str()) {
                    breaking_set.insert(rdep.clone());
                    queue.push_back(rdep.clone());
                }
            }
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
    use crate::version::RpmVersion;

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
    fn test_resolve_install_scoped_to_package() {
        // Pre-existing broken dep: X depends on Y, but Y is not installed.
        // Installing new package Z (which depends on installed W) should
        // succeed without reporting X's missing dependency Y.
        use crate::resolver::Resolver;
        use crate::resolver::graph::DependencyEdge;

        let (_dir, conn) = setup_test_db();

        // Installed packages
        insert_trove(&conn, "W", "1.0.0", &[]);
        // X depends on Y, but Y is not installed (pre-existing problem)
        insert_trove(&conn, "X", "1.0.0", &[("Y", None)]);

        let mut resolver = Resolver::new(&conn).unwrap();

        // Install Z which only depends on W (already installed)
        let plan = resolver
            .resolve_install(
                "Z".to_string(),
                RpmVersion::parse("1.0.0").unwrap(),
                vec![DependencyEdge {
                    from: "Z".to_string(),
                    to: "W".to_string(),
                    constraint: VersionConstraint::Any,
                    raw_constraint: None,
                    dep_type: "runtime".to_string(),
                    kind: "package".to_string(),
                }],
            )
            .unwrap();

        // Z's deps are all satisfied — no missing, no conflicts
        assert!(
            plan.missing.is_empty(),
            "should not report unrelated missing deps"
        );
        assert!(
            plan.conflicts.is_empty(),
            "should not report unrelated conflicts"
        );
    }

    #[test]
    fn test_resolve_install_reports_own_missing() {
        // Installing Z which depends on NOTINSTALLED should report it as missing
        use crate::resolver::Resolver;
        use crate::resolver::graph::DependencyEdge;

        let (_dir, conn) = setup_test_db();
        insert_trove(&conn, "W", "1.0.0", &[]);

        let mut resolver = Resolver::new(&conn).unwrap();

        let plan = resolver
            .resolve_install(
                "Z".to_string(),
                RpmVersion::parse("1.0.0").unwrap(),
                vec![
                    DependencyEdge {
                        from: "Z".to_string(),
                        to: "W".to_string(),
                        constraint: VersionConstraint::Any,
                        raw_constraint: None,
                        dep_type: "runtime".to_string(),
                        kind: "package".to_string(),
                    },
                    DependencyEdge {
                        from: "Z".to_string(),
                        to: "NOTINSTALLED".to_string(),
                        constraint: VersionConstraint::parse(">= 1.0.0").unwrap(),
                        raw_constraint: Some(">= 1.0.0".to_string()),
                        dep_type: "runtime".to_string(),
                        kind: "package".to_string(),
                    },
                ],
            )
            .unwrap();

        assert_eq!(plan.missing.len(), 1);
        assert_eq!(plan.missing[0].name, "NOTINSTALLED");
        assert!(plan.conflicts.is_empty());
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
}
