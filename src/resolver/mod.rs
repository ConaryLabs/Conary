// src/resolver/mod.rs

//! Dependency resolution and conflict detection
//!
//! This module provides dependency graph construction, topological sorting,
//! cycle detection, and conflict resolution for package dependencies.

use crate::db::models::{DependencyEntry, Trove};
use crate::error::{Error, Result};
use crate::version::{RpmVersion, VersionConstraint};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet, VecDeque};

/// A node in the dependency graph representing a package
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackageNode {
    pub name: String,
    pub version: RpmVersion,
    pub trove_id: Option<i64>,
}

impl PackageNode {
    pub fn new(name: String, version: RpmVersion) -> Self {
        Self {
            name,
            version,
            trove_id: None,
        }
    }

    pub fn with_trove_id(mut self, trove_id: i64) -> Self {
        self.trove_id = Some(trove_id);
        self
    }
}

/// A dependency edge with version constraints
#[derive(Debug, Clone)]
pub struct DependencyEdge {
    pub from: String,
    pub to: String,
    pub constraint: VersionConstraint,
    pub dep_type: String,
}

/// Dependency graph for resolution and ordering
#[derive(Debug)]
pub struct DependencyGraph {
    /// Map from package name to its node
    nodes: HashMap<String, PackageNode>,
    /// Map from package name to its outgoing dependencies
    edges: HashMap<String, Vec<DependencyEdge>>,
    /// Map from package name to packages that depend on it (reverse edges)
    reverse_edges: HashMap<String, Vec<String>>,
}

impl DependencyGraph {
    /// Create a new empty dependency graph
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: HashMap::new(),
            reverse_edges: HashMap::new(),
        }
    }

    /// Build a dependency graph from the database for installed packages
    pub fn build_from_db(conn: &Connection) -> Result<Self> {
        let mut graph = Self::new();

        // Load all installed troves
        let troves = Trove::list_all(conn)?;

        for trove in troves {
            // Parse the version
            let version = RpmVersion::parse(&trove.version)?;
            let node = PackageNode::new(trove.name.clone(), version)
                .with_trove_id(trove.id.unwrap());

            graph.add_node(node);

            // Load dependencies for this trove
            let deps = DependencyEntry::find_by_trove(conn, trove.id.unwrap())?;

            for dep in deps {
                let constraint = if let Some(ref constraint_str) = dep.version_constraint {
                    VersionConstraint::parse(constraint_str)?
                } else {
                    VersionConstraint::Any
                };

                let edge = DependencyEdge {
                    from: trove.name.clone(),
                    to: dep.depends_on_name.clone(),
                    constraint,
                    dep_type: dep.dependency_type.clone(),
                };

                graph.add_edge(edge);
            }
        }

        Ok(graph)
    }

    /// Add a package node to the graph
    pub fn add_node(&mut self, node: PackageNode) {
        self.nodes.insert(node.name.clone(), node);
    }

    /// Add a dependency edge to the graph
    pub fn add_edge(&mut self, edge: DependencyEdge) {
        // Add to forward edges
        self.edges
            .entry(edge.from.clone())
            .or_default()
            .push(edge.clone());

        // Add to reverse edges
        self.reverse_edges
            .entry(edge.to.clone())
            .or_default()
            .push(edge.from.clone());
    }

    /// Get a node by package name
    pub fn get_node(&self, name: &str) -> Option<&PackageNode> {
        self.nodes.get(name)
    }

    /// Get all dependencies of a package
    pub fn get_dependencies(&self, name: &str) -> Vec<&DependencyEdge> {
        self.edges.get(name).map(|v| v.iter().collect()).unwrap_or_default()
    }

    /// Get all packages that depend on this package (reverse dependencies)
    pub fn get_dependents(&self, name: &str) -> Vec<String> {
        self.reverse_edges
            .get(name)
            .cloned()
            .unwrap_or_default()
    }

    /// Perform topological sort using Kahn's algorithm
    ///
    /// Returns packages in installation order (dependencies before dependents)
    pub fn topological_sort(&self) -> Result<Vec<String>> {
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut result = Vec::new();
        let mut queue = VecDeque::new();

        // Calculate in-degrees for all nodes
        for name in self.nodes.keys() {
            in_degree.insert(name.clone(), 0);
        }

        for edges in self.edges.values() {
            for edge in edges {
                *in_degree.entry(edge.to.clone()).or_insert(0) += 1;
            }
        }

        // Add all nodes with in-degree 0 to the queue
        for (name, &degree) in &in_degree {
            if degree == 0 {
                queue.push_back(name.clone());
            }
        }

        // Process nodes in topological order
        while let Some(name) = queue.pop_front() {
            result.push(name.clone());

            // Reduce in-degree of neighbors
            if let Some(edges) = self.edges.get(&name) {
                for edge in edges {
                    if let Some(degree) = in_degree.get_mut(&edge.to) {
                        *degree -= 1;
                        if *degree == 0 {
                            queue.push_back(edge.to.clone());
                        }
                    }
                }
            }
        }

        // If we haven't processed all nodes, there's a cycle
        if result.len() != self.nodes.len() {
            return Err(Error::InitError(
                "Circular dependency detected in package graph".to_string(),
            ));
        }

        // Reverse to get installation order (dependencies before dependents)
        result.reverse();
        Ok(result)
    }

    /// Detect circular dependencies in the graph
    ///
    /// Returns the packages involved in a cycle, or None if no cycle exists
    pub fn detect_cycle(&self) -> Option<Vec<String>> {
        let mut visited = HashSet::new();
        let mut rec_stack = HashSet::new();
        let mut cycle = Vec::new();

        for name in self.nodes.keys() {
            if !visited.contains(name)
                && self.dfs_cycle_detect(name, &mut visited, &mut rec_stack, &mut cycle)
            {
                return Some(cycle);
            }
        }

        None
    }

    /// DFS helper for cycle detection
    fn dfs_cycle_detect(
        &self,
        name: &str,
        visited: &mut HashSet<String>,
        rec_stack: &mut HashSet<String>,
        cycle: &mut Vec<String>,
    ) -> bool {
        visited.insert(name.to_string());
        rec_stack.insert(name.to_string());

        if let Some(edges) = self.edges.get(name) {
            for edge in edges {
                if !visited.contains(&edge.to) {
                    if self.dfs_cycle_detect(&edge.to, visited, rec_stack, cycle) {
                        cycle.push(name.to_string());
                        return true;
                    }
                } else if rec_stack.contains(&edge.to) {
                    // Found a cycle
                    cycle.push(edge.to.clone());
                    cycle.push(name.to_string());
                    return true;
                }
            }
        }

        rec_stack.remove(name);
        false
    }

    /// Check if a version satisfies all constraints for a dependency
    pub fn check_constraints(&self, package_name: &str, version: &RpmVersion) -> Result<()> {
        // Find all packages that depend on this package
        if let Some(dependents) = self.reverse_edges.get(package_name) {
            for dependent in dependents {
                if let Some(edges) = self.edges.get(dependent) {
                    for edge in edges {
                        if edge.to == package_name
                            && !edge.constraint.satisfies(version)
                        {
                            return Err(Error::InitError(format!(
                                "Version {} of {} does not satisfy constraint {} required by {}",
                                version, package_name, edge.constraint, dependent
                            )));
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Find all packages that would break if this package is removed
    ///
    /// This performs a transitive closure of reverse dependencies,
    /// finding all packages that depend on this one directly or indirectly.
    pub fn find_breaking_packages(&self, package_name: &str) -> Vec<String> {
        let mut breaking = HashSet::new();
        let mut queue = VecDeque::new();

        queue.push_back(package_name.to_string());

        while let Some(name) = queue.pop_front() {
            if let Some(dependents) = self.reverse_edges.get(&name) {
                for dependent in dependents {
                    if breaking.insert(dependent.clone()) {
                        queue.push_back(dependent.clone());
                    }
                }
            }
        }

        breaking.into_iter().collect()
    }

    /// Get statistics about the dependency graph
    pub fn stats(&self) -> GraphStats {
        let total_nodes = self.nodes.len();
        let total_edges: usize = self.edges.values().map(|v| v.len()).sum();

        let mut max_dependencies = 0;
        let mut max_dependents = 0;

        for edges in self.edges.values() {
            max_dependencies = max_dependencies.max(edges.len());
        }

        for dependents in self.reverse_edges.values() {
            max_dependents = max_dependents.max(dependents.len());
        }

        GraphStats {
            total_packages: total_nodes,
            total_dependencies: total_edges,
            max_dependencies,
            max_dependents,
        }
    }
}

impl Default for DependencyGraph {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about the dependency graph
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphStats {
    pub total_packages: usize,
    pub total_dependencies: usize,
    pub max_dependencies: usize,
    pub max_dependents: usize,
}

/// Result of dependency resolution
#[derive(Debug, Clone)]
pub struct ResolutionPlan {
    /// Packages to install in order (dependencies first)
    pub install_order: Vec<String>,
    /// Packages that are missing and need to be fetched
    pub missing: Vec<MissingDependency>,
    /// Conflicts detected during resolution
    pub conflicts: Vec<Conflict>,
}

/// A missing dependency that needs to be installed
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingDependency {
    pub name: String,
    pub constraint: VersionConstraint,
    pub required_by: Vec<String>,
}

/// A conflict between package requirements
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflict {
    /// Version constraint cannot be satisfied
    UnsatisfiableConstraint {
        package: String,
        installed_version: String,
        required_constraint: String,
        required_by: String,
    },
    /// Multiple packages require incompatible versions
    ConflictingConstraints {
        package: String,
        constraints: Vec<(String, String)>, // (requirer, constraint)
    },
    /// Circular dependency detected
    CircularDependency { cycle: Vec<String> },
    /// Package is missing and cannot be found
    MissingPackage {
        package: String,
        required_by: Vec<String>,
    },
}

impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Conflict::UnsatisfiableConstraint {
                package,
                installed_version,
                required_constraint,
                required_by,
            } => write!(
                f,
                "Package {} version {} does not satisfy constraint {} required by {}",
                package, installed_version, required_constraint, required_by
            ),
            Conflict::ConflictingConstraints {
                package,
                constraints,
            } => {
                writeln!(f, "Conflicting version requirements for package {}:", package)?;
                for (requirer, constraint) in constraints {
                    writeln!(f, "  - {} requires {}", requirer, constraint)?;
                }
                Ok(())
            }
            Conflict::CircularDependency { cycle } => {
                write!(f, "Circular dependency: {}", cycle.join(" -> "))
            }
            Conflict::MissingPackage {
                package,
                required_by,
            } => {
                write!(
                    f,
                    "Missing package {} required by {}",
                    package,
                    required_by.join(", ")
                )
            }
        }
    }
}

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

        // Resolve the full graph
        self.resolve()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_version(s: &str) -> RpmVersion {
        RpmVersion::parse(s).unwrap()
    }

    #[test]
    fn test_graph_creation() {
        let graph = DependencyGraph::new();
        assert_eq!(graph.nodes.len(), 0);
        assert_eq!(graph.edges.len(), 0);
    }

    #[test]
    fn test_add_node() {
        let mut graph = DependencyGraph::new();
        let node = PackageNode::new("test-package".to_string(), make_version("1.0.0"));
        graph.add_node(node.clone());

        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(graph.get_node("test-package"), Some(&node));
    }

    #[test]
    fn test_add_edge() {
        let mut graph = DependencyGraph::new();

        let node1 = PackageNode::new("package-a".to_string(), make_version("1.0.0"));
        let node2 = PackageNode::new("package-b".to_string(), make_version("2.0.0"));

        graph.add_node(node1);
        graph.add_node(node2);

        let edge = DependencyEdge {
            from: "package-a".to_string(),
            to: "package-b".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        };

        graph.add_edge(edge);

        let deps = graph.get_dependencies("package-a");
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].to, "package-b");

        let dependents = graph.get_dependents("package-b");
        assert_eq!(dependents.len(), 1);
        assert_eq!(dependents[0], "package-a");
    }

    #[test]
    fn test_topological_sort_simple() {
        let mut graph = DependencyGraph::new();

        // Create a simple dependency chain: A -> B -> C
        graph.add_node(PackageNode::new("A".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("B".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("C".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "B".to_string(),
            to: "C".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        let sorted = graph.topological_sort().unwrap();

        // C should come before B, and B should come before A
        let pos_a = sorted.iter().position(|x| x == "A").unwrap();
        let pos_b = sorted.iter().position(|x| x == "B").unwrap();
        let pos_c = sorted.iter().position(|x| x == "C").unwrap();

        assert!(pos_c < pos_b);
        assert!(pos_b < pos_a);
    }

    #[test]
    fn test_topological_sort_diamond() {
        let mut graph = DependencyGraph::new();

        // Diamond dependency:
        //     A
        //    / \
        //   B   C
        //    \ /
        //     D

        graph.add_node(PackageNode::new("A".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("B".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("C".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("D".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "C".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "B".to_string(),
            to: "D".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "C".to_string(),
            to: "D".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        let sorted = graph.topological_sort().unwrap();

        // D should come before both B and C, and both B and C before A
        let pos_a = sorted.iter().position(|x| x == "A").unwrap();
        let pos_b = sorted.iter().position(|x| x == "B").unwrap();
        let pos_c = sorted.iter().position(|x| x == "C").unwrap();
        let pos_d = sorted.iter().position(|x| x == "D").unwrap();

        assert!(pos_d < pos_b);
        assert!(pos_d < pos_c);
        assert!(pos_b < pos_a);
        assert!(pos_c < pos_a);
    }

    #[test]
    fn test_cycle_detection_simple() {
        let mut graph = DependencyGraph::new();

        // Create a cycle: A -> B -> C -> A
        graph.add_node(PackageNode::new("A".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("B".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("C".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "B".to_string(),
            to: "C".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "C".to_string(),
            to: "A".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        let cycle = graph.detect_cycle();
        assert!(cycle.is_some());

        // Topological sort should fail
        let result = graph.topological_sort();
        assert!(result.is_err());
    }

    #[test]
    fn test_no_cycle() {
        let mut graph = DependencyGraph::new();

        // Create a DAG: A -> B -> C
        graph.add_node(PackageNode::new("A".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("B".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("C".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "B".to_string(),
            to: "C".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        let cycle = graph.detect_cycle();
        assert!(cycle.is_none());
    }

    #[test]
    fn test_check_constraints_satisfied() {
        let mut graph = DependencyGraph::new();

        graph.add_node(PackageNode::new("lib".to_string(), make_version("2.0.0")));
        graph.add_node(PackageNode::new("app".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "app".to_string(),
            to: "lib".to_string(),
            constraint: VersionConstraint::parse(">= 1.0.0").unwrap(),
            dep_type: "runtime".to_string(),
        });

        let lib_version = make_version("2.0.0");
        assert!(graph.check_constraints("lib", &lib_version).is_ok());
    }

    #[test]
    fn test_check_constraints_violated() {
        let mut graph = DependencyGraph::new();

        graph.add_node(PackageNode::new("lib".to_string(), make_version("0.5.0")));
        graph.add_node(PackageNode::new("app".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "app".to_string(),
            to: "lib".to_string(),
            constraint: VersionConstraint::parse(">= 1.0.0").unwrap(),
            dep_type: "runtime".to_string(),
        });

        let lib_version = make_version("0.5.0");
        assert!(graph.check_constraints("lib", &lib_version).is_err());
    }

    #[test]
    fn test_find_breaking_packages() {
        let mut graph = DependencyGraph::new();

        // Create dependency chain: lib <- app1 <- app2
        graph.add_node(PackageNode::new("lib".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("app1".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("app2".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "app1".to_string(),
            to: "lib".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "app2".to_string(),
            to: "app1".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        let breaking = graph.find_breaking_packages("lib");

        // Both app1 and app2 should break if lib is removed
        assert_eq!(breaking.len(), 2);
        assert!(breaking.contains(&"app1".to_string()));
        assert!(breaking.contains(&"app2".to_string()));
    }

    #[test]
    fn test_graph_stats() {
        let mut graph = DependencyGraph::new();

        graph.add_node(PackageNode::new("A".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("B".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("C".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "C".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        let stats = graph.stats();
        assert_eq!(stats.total_packages, 3);
        assert_eq!(stats.total_dependencies, 2);
        assert_eq!(stats.max_dependencies, 2); // A has 2 dependencies
        assert_eq!(stats.max_dependents, 1); // B and C each have 1 dependent
    }

    #[test]
    fn test_resolver_simple() {
        let mut graph = DependencyGraph::new();

        // Simple case: A depends on B
        graph.add_node(PackageNode::new("A".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("B".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        let resolver = Resolver::with_graph(graph);
        let plan = resolver.resolve().unwrap();

        assert_eq!(plan.conflicts.len(), 0);
        assert_eq!(plan.missing.len(), 0);
        assert_eq!(plan.install_order.len(), 2);
    }

    #[test]
    fn test_resolver_missing_dependency() {
        let mut graph = DependencyGraph::new();

        // A depends on B, but B is not installed
        graph.add_node(PackageNode::new("A".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            constraint: VersionConstraint::parse(">= 1.0.0").unwrap(),
            dep_type: "runtime".to_string(),
        });

        let resolver = Resolver::with_graph(graph);
        let plan = resolver.resolve().unwrap();

        assert_eq!(plan.missing.len(), 1);
        assert_eq!(plan.missing[0].name, "B");
        assert_eq!(plan.missing[0].required_by, vec!["A"]);
    }

    #[test]
    fn test_resolver_version_conflict() {
        let mut graph = DependencyGraph::new();

        // A depends on B >= 2.0.0, but B 1.0.0 is installed
        graph.add_node(PackageNode::new("A".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("B".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            constraint: VersionConstraint::parse(">= 2.0.0").unwrap(),
            dep_type: "runtime".to_string(),
        });

        let resolver = Resolver::with_graph(graph);
        let plan = resolver.resolve().unwrap();

        assert_eq!(plan.conflicts.len(), 1);
        match &plan.conflicts[0] {
            Conflict::UnsatisfiableConstraint {
                package,
                installed_version,
                required_constraint,
                required_by,
            } => {
                assert_eq!(package, "B");
                assert_eq!(installed_version, "1.0.0");
                assert_eq!(required_constraint, ">= 2.0.0");
                assert_eq!(required_by, "A");
            }
            _ => panic!("Expected UnsatisfiableConstraint"),
        }
    }

    #[test]
    fn test_resolver_circular_dependency() {
        let mut graph = DependencyGraph::new();

        // Circular: A -> B -> C -> A
        graph.add_node(PackageNode::new("A".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("B".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("C".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "B".to_string(),
            to: "C".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "C".to_string(),
            to: "A".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        let resolver = Resolver::with_graph(graph);
        let plan = resolver.resolve().unwrap();

        assert_eq!(plan.conflicts.len(), 1);
        match &plan.conflicts[0] {
            Conflict::CircularDependency { cycle } => {
                assert!(cycle.len() >= 3);
            }
            _ => panic!("Expected CircularDependency"),
        }
    }

    #[test]
    fn test_resolver_check_removal() {
        let mut graph = DependencyGraph::new();

        // lib <- app1 <- app2
        graph.add_node(PackageNode::new("lib".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("app1".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("app2".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "app1".to_string(),
            to: "lib".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "app2".to_string(),
            to: "app1".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        let resolver = Resolver::with_graph(graph);
        let breaking = resolver.check_removal("lib").unwrap();

        assert_eq!(breaking.len(), 2);
        assert!(breaking.contains(&"app1".to_string()));
        assert!(breaking.contains(&"app2".to_string()));
    }

    #[test]
    fn test_resolver_install_order() {
        let mut graph = DependencyGraph::new();

        // Complex dependency chain: app -> lib1 -> lib2
        //                                 -> lib3
        graph.add_node(PackageNode::new("app".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("lib1".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("lib2".to_string(), make_version("1.0.0")));
        graph.add_node(PackageNode::new("lib3".to_string(), make_version("1.0.0")));

        graph.add_edge(DependencyEdge {
            from: "app".to_string(),
            to: "lib1".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "app".to_string(),
            to: "lib3".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        graph.add_edge(DependencyEdge {
            from: "lib1".to_string(),
            to: "lib2".to_string(),
            constraint: VersionConstraint::Any,
            dep_type: "runtime".to_string(),
        });

        let resolver = Resolver::with_graph(graph);
        let plan = resolver.resolve().unwrap();

        assert_eq!(plan.conflicts.len(), 0);
        assert_eq!(plan.missing.len(), 0);

        // lib2 and lib3 should come before lib1, lib1 before app
        let pos_app = plan.install_order.iter().position(|x| x == "app").unwrap();
        let pos_lib1 = plan.install_order.iter().position(|x| x == "lib1").unwrap();
        let pos_lib2 = plan.install_order.iter().position(|x| x == "lib2").unwrap();
        let pos_lib3 = plan.install_order.iter().position(|x| x == "lib3").unwrap();

        assert!(pos_lib2 < pos_lib1);
        assert!(pos_lib3 < pos_app);
        assert!(pos_lib1 < pos_app);
    }
}
