// src/resolver/graph.rs

//! Dependency graph data structures and algorithms
//!
//! Provides graph construction, topological sorting, cycle detection,
//! and constraint checking for package dependencies.

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
    pub(crate) nodes: HashMap<String, PackageNode>,
    /// Map from package name to its outgoing dependencies
    pub(crate) edges: HashMap<String, Vec<DependencyEdge>>,
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
            // Get trove ID - required for database operations
            let trove_id = trove.id
                .ok_or_else(|| Error::InitError("Trove from database has no ID".to_string()))?;

            // Parse the version
            let version = RpmVersion::parse(&trove.version)?;
            let node = PackageNode::new(trove.name.clone(), version)
                .with_trove_id(trove_id);

            graph.add_node(node);

            // Load dependencies for this trove
            let deps = DependencyEntry::find_by_trove(conn, trove_id)?;

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

    /// Detect circular dependencies involving a specific package
    ///
    /// Only detects cycles that include the named package, ignoring
    /// pre-existing cycles elsewhere in the graph (e.g., glibc <-> glibc-common).
    pub fn detect_cycle_involving(&self, package_name: &str) -> Option<Vec<String>> {
        let mut visited = HashSet::new();
        let mut rec_stack = HashSet::new();
        let mut cycle = Vec::new();

        // Only start DFS from the specific package
        if self.dfs_cycle_detect(package_name, &mut visited, &mut rec_stack, &mut cycle) {
            // Only return if the cycle actually involves our package
            if cycle.contains(&package_name.to_string()) {
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
