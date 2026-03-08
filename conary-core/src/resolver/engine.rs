// conary-core/src/resolver/engine.rs

//! Dependency resolver implementation
//!
//! Uses resolvo's SAT solver for resolution with backtracking support.
//! The dependency graph is still maintained for visualization and stats.

use crate::error::Result;
use crate::version::{RpmVersion, VersionConstraint};
use rusqlite::Connection;

use super::conflict::Conflict;
use super::graph::{DependencyEdge, DependencyGraph, PackageNode};
use super::plan::{MissingDependency, ResolutionPlan};
use super::sat;

/// Dependency resolver for determining installation order and conflicts.
///
/// Uses the SAT-based solver (resolvo) for resolution with backtracking.
/// The dependency graph is maintained for visualization and dependency stats.
pub struct Resolver<'db> {
    graph: DependencyGraph,
    conn: &'db Connection,
}

impl<'db> Resolver<'db> {
    /// Create a new SAT-backed resolver from the database.
    pub fn new(conn: &'db Connection) -> Result<Self> {
        let graph = DependencyGraph::build_from_db(conn)?;
        Ok(Self { graph, conn })
    }

    /// Resolve dependencies for installing a new package.
    ///
    /// Adds the new package and its dependency edges to the graph, then
    /// checks only *this package's* edges for missing dependencies and
    /// version conflicts. Pre-existing system issues are not surfaced here.
    /// Callers use the `missing` list to fetch packages from repositories
    /// (via SAT-based transitive resolution).
    pub fn resolve_install(
        &mut self,
        package_name: String,
        version: RpmVersion,
        dependencies: Vec<DependencyEdge>,
    ) -> Result<ResolutionPlan> {
        use crate::db::models::ProvideEntry;

        // Add the new package and its edges to the graph
        let node = PackageNode::new(package_name.clone(), version);
        self.graph.add_node(node);
        for dep in &dependencies {
            self.graph.add_edge(dep.clone());
        }

        let mut conflicts = Vec::new();
        let mut missing = Vec::new();

        // Check only this package's dependency edges for missing deps
        for dep in &dependencies {
            if ProvideEntry::is_virtual_provide(&dep.to) {
                continue;
            }

            if self.graph.get_node(&dep.to).is_none() {
                missing.push(MissingDependency {
                    name: dep.to.clone(),
                    constraint: dep.constraint.clone(),
                    required_by: vec![package_name.clone()],
                });
            } else {
                // Node exists — check version constraint
                let target = self.graph.get_node(&dep.to).unwrap();
                if !dep.constraint.satisfies(&target.version) {
                    conflicts.push(Conflict::UnsatisfiableConstraint {
                        package: dep.to.clone(),
                        installed_version: target.version.to_string(),
                        required_constraint: dep.constraint.to_string(),
                        required_by: package_name.clone(),
                    });
                }
            }
        }

        // Get installation order via topological sort (uses full graph, which is fine —
        // the order just needs to be valid, and including existing packages is harmless)
        let install_order = self.graph.topological_sort()?;

        Ok(ResolutionPlan {
            install_order,
            missing,
            conflicts,
        })
    }

    /// Resolve the current dependency graph for consistency checking.
    ///
    /// Uses the graph-based approach for full system state analysis
    /// (cycle detection, constraint checking across all installed packages).
    pub fn resolve(&self) -> Result<ResolutionPlan> {
        let mut conflicts = Vec::new();
        let mut missing = Vec::new();

        // Check for circular dependencies
        if let Some(cycle) = self.graph.detect_cycle() {
            conflicts.push(Conflict::CircularDependency { cycle });
            return Ok(ResolutionPlan {
                install_order: Vec::new(),
                missing,
                conflicts,
            });
        }

        // Find missing dependencies
        let missing_deps = self.find_missing_dependencies();
        for (name, dep_info) in missing_deps {
            missing.push(MissingDependency {
                name: name.clone(),
                constraint: dep_info.0,
                required_by: dep_info.1,
            });
        }

        // Check version constraints
        let constraint_conflicts = self.check_all_constraints();
        conflicts.extend(constraint_conflicts);

        // Get installation order via topological sort
        let install_order = match self.graph.topological_sort() {
            Ok(order) => order,
            Err(_) => {
                return Ok(ResolutionPlan {
                    install_order: Vec::new(),
                    missing,
                    conflicts,
                });
            }
        };

        Ok(ResolutionPlan {
            install_order,
            missing,
            conflicts,
        })
    }

    /// Find all missing dependencies in the graph.
    fn find_missing_dependencies(
        &self,
    ) -> std::collections::HashMap<String, (VersionConstraint, Vec<String>)> {
        use crate::db::models::ProvideEntry;
        let mut missing: std::collections::HashMap<String, (VersionConstraint, Vec<String>)> =
            std::collections::HashMap::new();

        for (package_name, edges) in &self.graph.edges {
            for edge in edges {
                if ProvideEntry::is_virtual_provide(&edge.to) {
                    continue;
                }

                if self.graph.get_node(&edge.to).is_none() {
                    missing
                        .entry(edge.to.clone())
                        .and_modify(|(existing_constraint, requirers)| {
                            // Keep the stricter constraint when multiple packages
                            // require the same missing dependency
                            if *existing_constraint == VersionConstraint::Any
                                && edge.constraint != VersionConstraint::Any
                            {
                                *existing_constraint = edge.constraint.clone();
                            }
                            requirers.push(package_name.clone());
                        })
                        .or_insert_with(|| (edge.constraint.clone(), vec![package_name.clone()]));
                }
            }
        }

        missing
    }

    /// Check all version constraints and return conflicts.
    fn check_all_constraints(&self) -> Vec<Conflict> {
        let mut conflicts = Vec::new();
        let mut constraint_map: std::collections::HashMap<
            String,
            Vec<(String, VersionConstraint)>,
        > = std::collections::HashMap::new();

        for (requirer, edges) in &self.graph.edges {
            for edge in edges {
                constraint_map
                    .entry(edge.to.clone())
                    .or_default()
                    .push((requirer.clone(), edge.constraint.clone()));
            }
        }

        for (package_name, constraints) in constraint_map {
            if let Some(node) = self.graph.get_node(&package_name) {
                for (requirer, constraint) in &constraints {
                    if !constraint.satisfies(&node.version) {
                        conflicts.push(Conflict::UnsatisfiableConstraint {
                            package: package_name.clone(),
                            installed_version: node.version.to_string(),
                            required_constraint: constraint.to_string(),
                            required_by: requirer.clone(),
                        });
                    }
                }

                if constraints.len() > 1 {
                    let has_conflict = constraints
                        .iter()
                        .enumerate()
                        .any(|(i, (_, ci))| {
                            constraints[i + 1..]
                                .iter()
                                .any(|(_, cj)| !ci.is_compatible_with(cj))
                        });

                    if has_conflict {
                        conflicts.push(Conflict::ConflictingConstraints {
                            package: package_name.clone(),
                            constraints: constraints
                                .iter()
                                .map(|(r, c)| (r.clone(), c.to_string()))
                                .collect(),
                        });
                    }
                }
            }
        }

        conflicts
    }

    /// Check if removing a package would break dependencies.
    pub fn check_removal(&self, package_name: &str) -> Result<Vec<String>> {
        sat::solve_removal(self.conn, &[package_name.to_string()])
    }

    /// Get the dependency graph (for visualization/stats).
    pub fn graph(&self) -> &DependencyGraph {
        &self.graph
    }

    /// Create a resolver with a pre-built graph (for testing).
    #[cfg(test)]
    fn with_graph(graph: DependencyGraph, conn: &'db Connection) -> Self {
        Self { graph, conn }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::version::{RpmVersion, VersionConstraint};
    use tempfile::TempDir;

    fn v(s: &str) -> RpmVersion {
        RpmVersion::parse(s).unwrap()
    }

    fn edge(from: &str, to: &str) -> DependencyEdge {
        DependencyEdge {
            from: from.to_string(),
            to: to.to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        }
    }

    fn node(name: &str) -> PackageNode {
        PackageNode::new(name.to_string(), v("1.0.0"))
    }

    /// Create a minimal test database and return (TempDir, Connection).
    /// Keep TempDir alive to prevent cleanup.
    fn test_db() -> (TempDir, rusqlite::Connection) {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db_str = db_path.to_str().unwrap();
        db::init(db_str).unwrap();
        let conn = db::open(db_str).unwrap();
        (temp_dir, conn)
    }

    // --- resolve_install: basic success ---

    #[test]
    fn test_resolve_install_no_deps() {
        let (_dir, conn) = test_db();
        let graph = DependencyGraph::new();
        let mut resolver = Resolver::with_graph(graph, &conn);

        let plan = resolver
            .resolve_install("new-pkg".to_string(), v("1.0.0"), vec![])
            .unwrap();

        assert!(plan.conflicts.is_empty());
        assert!(plan.missing.is_empty());
        assert_eq!(plan.install_order, vec!["new-pkg"]);
    }

    #[test]
    fn test_resolve_install_with_satisfied_dep() {
        let (_dir, conn) = test_db();
        let mut graph = DependencyGraph::new();
        graph.add_node(PackageNode::new("libfoo".to_string(), v("2.0.0")));

        let mut resolver = Resolver::with_graph(graph, &conn);

        let deps = vec![DependencyEdge {
            from: "app".to_string(),
            to: "libfoo".to_string(),
            constraint: VersionConstraint::parse(">= 1.0.0").unwrap(),
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        }];

        let plan = resolver
            .resolve_install("app".to_string(), v("1.0.0"), deps)
            .unwrap();

        assert!(plan.conflicts.is_empty());
        assert!(plan.missing.is_empty());
        assert_eq!(plan.install_order.len(), 2);
        // libfoo should be installed before app
        let pos = |n: &str| plan.install_order.iter().position(|x| x == n).unwrap();
        assert!(pos("libfoo") < pos("app"));
    }

    // --- resolve_install: missing dependency ---

    #[test]
    fn test_resolve_install_missing_dep() {
        let (_dir, conn) = test_db();
        let graph = DependencyGraph::new();
        let mut resolver = Resolver::with_graph(graph, &conn);

        let deps = vec![edge("app", "missing-lib")];

        let plan = resolver
            .resolve_install("app".to_string(), v("1.0.0"), deps)
            .unwrap();

        assert_eq!(plan.missing.len(), 1);
        assert_eq!(plan.missing[0].name, "missing-lib");
        assert_eq!(plan.missing[0].required_by, vec!["app"]);
    }

    // --- resolve_install: version conflict ---

    #[test]
    fn test_resolve_install_version_conflict() {
        let (_dir, conn) = test_db();
        let mut graph = DependencyGraph::new();
        graph.add_node(PackageNode::new("libold".to_string(), v("0.5.0")));

        let mut resolver = Resolver::with_graph(graph, &conn);

        let deps = vec![DependencyEdge {
            from: "app".to_string(),
            to: "libold".to_string(),
            constraint: VersionConstraint::parse(">= 2.0.0").unwrap(),
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        }];

        let plan = resolver
            .resolve_install("app".to_string(), v("1.0.0"), deps)
            .unwrap();

        assert_eq!(plan.conflicts.len(), 1);
        match &plan.conflicts[0] {
            Conflict::UnsatisfiableConstraint {
                package,
                required_by,
                ..
            } => {
                assert_eq!(package, "libold");
                assert_eq!(required_by, "app");
            }
            other => panic!("Expected UnsatisfiableConstraint, got: {other:?}"),
        }
    }

    // --- resolve_install: cycle propagates error ---

    #[test]
    fn test_resolve_install_cycle_propagates_error() {
        let (_dir, conn) = test_db();
        let mut graph = DependencyGraph::new();
        // Pre-existing cycle in the graph: X -> Y -> X
        graph.add_node(node("X"));
        graph.add_node(node("Y"));
        graph.add_edge(edge("X", "Y"));
        graph.add_edge(edge("Y", "X"));

        let mut resolver = Resolver::with_graph(graph, &conn);

        // Installing a new package on top of a cyclic graph should error
        let result = resolver.resolve_install("new-pkg".to_string(), v("1.0.0"), vec![]);

        assert!(
            result.is_err(),
            "resolve_install should propagate cycle error, not silently succeed"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Circular dependency"),
            "Error should mention circular dependency, got: {err_msg}"
        );
    }

    // --- resolve_install: phantom dep does not cause false cycle ---

    #[test]
    fn test_resolve_install_phantom_dep_no_false_cycle() {
        let (_dir, conn) = test_db();
        let mut graph = DependencyGraph::new();
        // Existing package with a dependency on a missing package
        graph.add_node(node("existing"));
        graph.add_edge(edge("existing", "phantom-lib"));

        let mut resolver = Resolver::with_graph(graph, &conn);

        // Installing a new package should succeed (phantom is missing, not a cycle)
        let plan = resolver
            .resolve_install("new-pkg".to_string(), v("1.0.0"), vec![])
            .unwrap();

        // No cycles, just the two real packages in install order
        assert!(plan.conflicts.is_empty());
        assert_eq!(plan.install_order.len(), 2);
    }

    // --- resolve: consistency check ---

    #[test]
    fn test_resolve_clean_graph() {
        let (_dir, conn) = test_db();
        let mut graph = DependencyGraph::new();
        graph.add_node(node("A"));
        graph.add_node(node("B"));
        graph.add_edge(edge("A", "B"));

        let resolver = Resolver::with_graph(graph, &conn);
        let plan = resolver.resolve().unwrap();

        assert!(plan.conflicts.is_empty());
        assert!(plan.missing.is_empty());
        assert_eq!(plan.install_order.len(), 2);
    }

    #[test]
    fn test_resolve_detects_cycle() {
        let (_dir, conn) = test_db();
        let mut graph = DependencyGraph::new();
        graph.add_node(node("A"));
        graph.add_node(node("B"));
        graph.add_edge(edge("A", "B"));
        graph.add_edge(edge("B", "A"));

        let resolver = Resolver::with_graph(graph, &conn);
        let plan = resolver.resolve().unwrap();

        // resolve() returns Ok with conflicts, not Err
        assert!(!plan.conflicts.is_empty());
        assert!(plan.install_order.is_empty());
        match &plan.conflicts[0] {
            Conflict::CircularDependency { cycle } => {
                assert!(cycle.contains(&"A".to_string()));
                assert!(cycle.contains(&"B".to_string()));
            }
            other => panic!("Expected CircularDependency, got: {other:?}"),
        }
    }

    #[test]
    fn test_resolve_finds_missing_deps() {
        let (_dir, conn) = test_db();
        let mut graph = DependencyGraph::new();
        graph.add_node(node("app"));
        graph.add_edge(edge("app", "missing-lib"));

        let resolver = Resolver::with_graph(graph, &conn);
        let plan = resolver.resolve().unwrap();

        assert_eq!(plan.missing.len(), 1);
        assert_eq!(plan.missing[0].name, "missing-lib");
        assert!(plan.missing[0].required_by.contains(&"app".to_string()));
    }

    // --- Resolver::new from real DB ---

    #[test]
    fn test_resolver_new_empty_db() {
        let (_dir, conn) = test_db();
        let resolver = Resolver::new(&conn).unwrap();

        let plan = resolver.resolve().unwrap();
        assert!(plan.conflicts.is_empty());
        assert!(plan.missing.is_empty());
        assert!(plan.install_order.is_empty());
    }

    #[test]
    fn test_graph_accessor() {
        let (_dir, conn) = test_db();
        let mut graph = DependencyGraph::new();
        graph.add_node(node("pkg1"));
        graph.add_node(node("pkg2"));

        let resolver = Resolver::with_graph(graph, &conn);
        let g = resolver.graph();
        assert_eq!(g.stats().total_packages, 2);
    }
}
