// src/resolver/sat.rs

//! SAT-based dependency resolution using resolvo.
//!
//! Provides `solve_install` and `solve_removal` functions that use the CDCL SAT
//! solver to find optimal package installation plans with backtracking support.

use resolvo::{ConditionalRequirement, Problem, Solver, UnsolvableOrCancelled};
use rusqlite::Connection;

use crate::error::{Error, Result};
use crate::version::{RpmVersion, VersionConstraint};

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
    pub version: RpmVersion,
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
                    version: pkg.version.clone(),
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

    // Build requirements: every installed package except those being removed
    // should still have its dependencies satisfied
    let mut requirements = Vec::new();
    let mut excluded = Vec::new();
    let remove_set: std::collections::HashSet<&str> =
        to_remove.iter().map(String::as_str).collect();

    // First pass: collect info without mutating
    let solvable_count = provider.solvable_count();
    let mut to_exclude = Vec::new();
    let mut to_require = Vec::new();

    for i in 0..solvable_count {
        let sid = resolvo::SolvableId(i as u32);
        let pkg = provider.get_solvable(sid);
        if remove_set.contains(pkg.name.as_str()) {
            to_exclude.push((sid, pkg.name.clone()));
        } else if pkg.trove_id.is_some() {
            to_require.push((sid, pkg.name.clone()));
        }
    }

    // Second pass: intern with mutable access
    for (sid, name) in &to_exclude {
        let reason = provider.intern_string(&format!("Removed by user: {name}"));
        excluded.push((*sid, reason));
    }
    for (_sid, name) in &to_require {
        let name_id = provider.intern_name(name);
        let vs_id = provider.intern_version_set(name_id, VersionConstraint::Any);
        requirements.push(ConditionalRequirement::from(vs_id));
    }

    // We need to mark excluded packages in the candidates
    // Since we can't modify candidates after creation, we check if the removal
    // causes any dependency to become unsatisfied using the graph-based approach
    // as a fallback — the SAT approach is used when the full solver is invoked
    // through engine.rs

    // For now, use the dependency list to find direct + transitive reverse deps
    let mut breaking = Vec::new();
    for i in 0..solvable_count {
        let sid = resolvo::SolvableId(i as u32);
        let pkg = provider.get_solvable(sid);
        if pkg.trove_id.is_none() || remove_set.contains(pkg.name.as_str()) {
            continue;
        }

        // Check if any of this package's dependencies are being removed
        if let Some(deps) = provider.get_dependency_list(sid) {
            for (dep_name, _) in deps {
                if remove_set.contains(dep_name.as_str()) {
                    breaking.push(pkg.name.clone());
                    break;
                }
            }
        }
    }

    // Expand transitively: packages that depend on breaking packages also break
    let mut changed = true;
    let breaking_set = |breaking: &[String]| -> std::collections::HashSet<String> {
        breaking.iter().cloned().collect()
    };
    while changed {
        changed = false;
        let current_breaking = breaking_set(&breaking);
        for i in 0..solvable_count {
            let sid = resolvo::SolvableId(i as u32);
            let pkg = provider.get_solvable(sid);
            if pkg.trove_id.is_none()
                || remove_set.contains(pkg.name.as_str())
                || current_breaking.contains(&pkg.name)
            {
                continue;
            }

            if let Some(deps) = provider.get_dependency_list(sid) {
                for (dep_name, _) in deps {
                    if current_breaking.contains(dep_name) {
                        breaking.push(pkg.name.clone());
                        changed = true;
                        break;
                    }
                }
            }
        }
    }

    // Deduplicate
    breaking.sort();
    breaking.dedup();

    Ok(breaking)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::models::{Changeset, ChangesetStatus, DependencyEntry, Trove, TroveType};

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
        changeset.update_status(conn, ChangesetStatus::Applied).unwrap();

        let mut trove = Trove::new(
            name.to_string(),
            version.to_string(),
            TroveType::Package,
        );
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

        let result = solve_install(&conn, &[
            ("A".to_string(), VersionConstraint::Any),
        ]).unwrap();

        assert!(result.conflict_message.is_none());
        assert!(!result.install_order.is_empty());

        // Both A and B should be in the result
        let names: Vec<&str> = result.install_order.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"A"));
        assert!(names.contains(&"B"));
    }

    #[test]
    fn test_missing_dependency() {
        // A depends on B, but B is not installed or in any repo
        let (_dir, conn) = setup_test_db();
        insert_trove(&conn, "A", "1.0.0", &[("B", Some(">= 2.0.0"))]);

        let result = solve_install(&conn, &[
            ("A".to_string(), VersionConstraint::Any),
        ]).unwrap();

        // Should have a conflict message since B can't be found
        assert!(result.conflict_message.is_some());
    }

    #[test]
    fn test_version_conflict() {
        // A needs B >= 2.0, only B 1.0 is installed
        let (_dir, conn) = setup_test_db();
        insert_trove(&conn, "B", "1.0.0", &[]);
        insert_trove(&conn, "A", "1.0.0", &[("B", Some(">= 2.0.0"))]);

        let result = solve_install(&conn, &[
            ("A".to_string(), VersionConstraint::Any),
        ]).unwrap();

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

        let result = solve_install(&conn, &[
            ("A".to_string(), VersionConstraint::Any),
        ]).unwrap();

        assert!(result.conflict_message.is_none());

        let names: Vec<&str> = result.install_order.iter().map(|p| p.name.as_str()).collect();
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

        let result = solve_install(&conn, &[
            ("A".to_string(), VersionConstraint::Any),
        ]).unwrap();

        assert!(result.conflict_message.is_none());

        let names: Vec<&str> = result.install_order.iter().map(|p| p.name.as_str()).collect();
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
        use crate::resolver::graph::DependencyEdge;
        use crate::resolver::Resolver;

        let (_dir, conn) = setup_test_db();

        // Installed packages
        insert_trove(&conn, "W", "1.0.0", &[]);
        // X depends on Y, but Y is not installed (pre-existing problem)
        insert_trove(&conn, "X", "1.0.0", &[("Y", None)]);

        let mut resolver = Resolver::new(&conn).unwrap();

        // Install Z which only depends on W (already installed)
        let plan = resolver.resolve_install(
            "Z".to_string(),
            RpmVersion::parse("1.0.0").unwrap(),
            vec![DependencyEdge {
                from: "Z".to_string(),
                to: "W".to_string(),
                constraint: VersionConstraint::Any,
                dep_type: "runtime".to_string(),
                kind: "package".to_string(),
            }],
        ).unwrap();

        // Z's deps are all satisfied — no missing, no conflicts
        assert!(plan.missing.is_empty(), "should not report unrelated missing deps");
        assert!(plan.conflicts.is_empty(), "should not report unrelated conflicts");
    }

    #[test]
    fn test_resolve_install_reports_own_missing() {
        // Installing Z which depends on NOTINSTALLED should report it as missing
        use crate::resolver::graph::DependencyEdge;
        use crate::resolver::Resolver;

        let (_dir, conn) = setup_test_db();
        insert_trove(&conn, "W", "1.0.0", &[]);

        let mut resolver = Resolver::new(&conn).unwrap();

        let plan = resolver.resolve_install(
            "Z".to_string(),
            RpmVersion::parse("1.0.0").unwrap(),
            vec![
                DependencyEdge {
                    from: "Z".to_string(),
                    to: "W".to_string(),
                    constraint: VersionConstraint::Any,
                    dep_type: "runtime".to_string(),
                    kind: "package".to_string(),
                },
                DependencyEdge {
                    from: "Z".to_string(),
                    to: "NOTINSTALLED".to_string(),
                    constraint: VersionConstraint::parse(">= 1.0.0").unwrap(),
                    dep_type: "runtime".to_string(),
                    kind: "package".to_string(),
                },
            ],
        ).unwrap();

        assert_eq!(plan.missing.len(), 1);
        assert_eq!(plan.missing[0].name, "NOTINSTALLED");
        assert!(plan.conflicts.is_empty());
    }
}
