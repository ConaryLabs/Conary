// src/resolver/engine.rs

//! Dependency resolver implementation
//!
//! The main resolver that uses the dependency graph to determine
//! installation order, find missing dependencies, and detect conflicts.

use crate::error::Result;
use crate::version::{RpmVersion, VersionConstraint};
use rusqlite::Connection;
use std::collections::HashMap;

use super::conflict::Conflict;
use super::graph::{DependencyEdge, DependencyGraph, PackageNode};
use super::plan::{MissingDependency, ResolutionPlan};

/// Dependency resolver for determining installation order and conflicts
pub struct Resolver {
    graph: DependencyGraph,
}

impl Resolver {
    /// Create a new resolver with the current state from database
    pub fn new(conn: &Connection) -> Result<Self> {
        let graph = DependencyGraph::build_from_db(conn)?;
        Ok(Self { graph })
    }

    /// Create a resolver with a custom graph (for testing)
    pub fn with_graph(graph: DependencyGraph) -> Self {
        Self { graph }
    }

    /// Resolve dependencies for installing a new package
    ///
    /// This determines:
    /// - What packages need to be installed in what order
    /// - What dependencies are missing
    /// - What conflicts exist
    ///
    /// Note: We only check for cycles involving the new package, not pre-existing
    /// cycles in the system (like glibc <-> glibc-common which are tolerated).
    pub fn resolve_install(
        &mut self,
        package_name: String,
        version: RpmVersion,
        dependencies: Vec<DependencyEdge>,
    ) -> Result<ResolutionPlan> {
        // Add the new package to the graph
        let node = PackageNode::new(package_name.clone(), version.clone());
        self.graph.add_node(node);

        // Add its dependencies
        for dep in dependencies {
            self.graph.add_edge(dep);
        }

        // Resolve with single-package focus (skip global cycle detection)
        self.resolve_single_install(&package_name)
    }

    /// Resolve for a single new package install
    ///
    /// This is a focused resolution that only checks constraints relevant to the
    /// new package, ignoring pre-existing cycles in the dependency graph.
    ///
    /// Cycle detection is skipped because a NEW package cannot be part of a cycle:
    /// nothing in the system depends on it yet, so there's no way for a dependency
    /// path to lead back to it.
    fn resolve_single_install(&self, package_name: &str) -> Result<ResolutionPlan> {
        let mut missing = Vec::new();

        // Find missing dependencies for the new package
        if let Some(edges) = self.graph.edges.get(package_name) {
            for edge in edges {
                // Skip virtual provides like perl(Cwd), python3dist(foo), etc.
                // These are capabilities provided by packages, not package names.
                // A proper solution would check the "provides" table in the DB.
                if is_virtual_provide(&edge.to) {
                    continue;
                }

                if self.graph.get_node(&edge.to).is_none() {
                    missing.push(MissingDependency {
                        name: edge.to.clone(),
                        constraint: edge.constraint.clone(),
                        required_by: vec![package_name.to_string()],
                    });
                }
            }
        }

        // Check version constraints for the new package's dependencies
        // (only for non-virtual dependencies)
        let conflicts = self.check_constraints_for_package(package_name);

        // For single install, install order is just this package
        // (dependencies are assumed to already be installed)
        let install_order = vec![package_name.to_string()];

        Ok(ResolutionPlan {
            install_order,
            missing,
            conflicts,
        })
    }

    /// Check constraints only for a specific package's dependencies
    fn check_constraints_for_package(&self, package_name: &str) -> Vec<Conflict> {
        let mut conflicts = Vec::new();

        if let Some(edges) = self.graph.edges.get(package_name) {
            for edge in edges {
                // Skip virtual provides - they don't have nodes in our graph
                if is_virtual_provide(&edge.to) {
                    continue;
                }

                if let Some(node) = self.graph.get_node(&edge.to) {
                    // Check if installed version satisfies the constraint
                    if !edge.constraint.satisfies(&node.version) {
                        conflicts.push(Conflict::UnsatisfiableConstraint {
                            package: edge.to.clone(),
                            installed_version: node.version.to_string(),
                            required_constraint: edge.constraint.to_string(),
                            required_by: package_name.to_string(),
                        });
                    }
                }
            }
        }

        conflicts
    }

    /// Resolve the current dependency graph
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
                // Should have been caught by cycle detection, but handle anyway
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

    /// Find all missing dependencies in the graph
    fn find_missing_dependencies(
        &self,
    ) -> HashMap<String, (VersionConstraint, Vec<String>)> {
        let mut missing: HashMap<String, (VersionConstraint, Vec<String>)> = HashMap::new();

        for (package_name, edges) in &self.graph.edges {
            for edge in edges {
                // Check if the dependency exists in the graph
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

    /// Check all version constraints and return conflicts
    fn check_all_constraints(&self) -> Vec<Conflict> {
        let mut conflicts = Vec::new();
        let mut constraint_map: HashMap<String, Vec<(String, VersionConstraint)>> =
            HashMap::new();

        // Collect all constraints for each package
        for (requirer, edges) in &self.graph.edges {
            for edge in edges {
                constraint_map
                    .entry(edge.to.clone())
                    .or_default()
                    .push((requirer.clone(), edge.constraint.clone()));
            }
        }

        // Check each package's constraints
        for (package_name, constraints) in constraint_map {
            if let Some(node) = self.graph.get_node(&package_name) {
                // Check if installed version satisfies all constraints
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

                // Check for conflicting constraints
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

    /// Check if removing a package would break dependencies
    pub fn check_removal(&self, package_name: &str) -> Result<Vec<String>> {
        let breaking = self.graph.find_breaking_packages(package_name);
        Ok(breaking)
    }

    /// Get the dependency graph
    pub fn graph(&self) -> &DependencyGraph {
        &self.graph
    }
}

/// Check if a dependency name is a virtual provide (capability) rather than a package name.
///
/// Virtual provides have patterns like:
/// - perl(Cwd) - Perl module
/// - python3dist(setuptools) - Python package
/// - config(package) - Configuration capability
/// - pkgconfig(foo) - pkg-config module
/// - lib*.so.* - Shared library
fn is_virtual_provide(name: &str) -> bool {
    // Check for common virtual provide patterns
    name.contains('(')  // perl(Foo), python3dist(bar), etc.
        || name.starts_with("lib") && name.contains(".so")  // libfoo.so.1
        || name.starts_with("/")  // File path dependencies
}
