// src/resolver/engine.rs

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
        let install_order = self.graph.topological_sort().unwrap_or_default();

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
                        .or_insert_with(|| (edge.constraint.clone(), Vec::new()))
                        .1
                        .push(package_name.clone());
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
                    let mut conflicting = false;
                    for i in 0..constraints.len() {
                        for j in (i + 1)..constraints.len() {
                            if !constraints[i].1.is_compatible_with(&constraints[j].1) {
                                conflicting = true;
                                break;
                            }
                        }
                        if conflicting {
                            break;
                        }
                    }

                    if conflicting {
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
}
