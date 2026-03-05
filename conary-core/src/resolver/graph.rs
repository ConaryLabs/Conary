// conary-core/src/resolver/graph.rs

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
    /// The kind of dependency (package, python, soname, pkgconfig, etc.)
    pub kind: String,
}

impl DependencyEdge {
    /// Create a new package dependency edge
    pub fn new(from: String, to: String, constraint: VersionConstraint, dep_type: String) -> Self {
        Self {
            from,
            to,
            constraint,
            dep_type,
            kind: "package".to_string(),
        }
    }

    /// Create a new typed dependency edge
    pub fn typed(
        from: String,
        to: String,
        constraint: VersionConstraint,
        dep_type: String,
        kind: String,
    ) -> Self {
        Self {
            from,
            to,
            constraint,
            dep_type,
            kind,
        }
    }
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
            let trove_id = trove
                .id
                .ok_or_else(|| Error::InitError("Trove from database has no ID".to_string()))?;

            // Parse the version
            let version = RpmVersion::parse(&trove.version)?;
            let node = PackageNode::new(trove.name.clone(), version).with_trove_id(trove_id);

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
                    kind: dep.kind.clone(),
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
        self.edges
            .get(name)
            .map(|v| v.iter().collect())
            .unwrap_or_default()
    }

    /// Get all packages that depend on this package (reverse dependencies)
    pub fn get_dependents(&self, name: &str) -> Vec<String> {
        self.reverse_edges.get(name).cloned().unwrap_or_default()
    }

    /// Perform topological sort using Kahn's algorithm
    ///
    /// Returns packages in installation order (dependencies before dependents).
    /// Only considers edges between nodes that exist in the graph. Edges pointing
    /// to missing (phantom) nodes are ignored -- those are reported separately as
    /// missing dependencies, not as cycles.
    pub fn topological_sort(&self) -> Result<Vec<String>> {
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut result = Vec::new();
        let mut queue = VecDeque::new();

        // Calculate in-degrees for all nodes
        for name in self.nodes.keys() {
            in_degree.insert(name.clone(), 0);
        }

        // Only count edges where the target is a known node.
        // Edges to phantom (missing) nodes are not cycles -- they are missing
        // dependencies and should not influence the topological sort.
        for edges in self.edges.values() {
            for edge in edges {
                if self.nodes.contains_key(&edge.to)
                    && let Some(degree) = in_degree.get_mut(&edge.to)
                {
                    *degree += 1;
                }
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

            // Reduce in-degree of neighbors (only for known nodes)
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

        // If we haven't processed all nodes, there's a cycle.
        // Identify the packages involved for a useful error message.
        if result.len() != self.nodes.len() {
            let processed: HashSet<&str> = result.iter().map(String::as_str).collect();
            let mut cycle_members: Vec<String> = self
                .nodes
                .keys()
                .filter(|n| !processed.contains(n.as_str()))
                .cloned()
                .collect();
            cycle_members.sort();
            return Err(Error::ResolutionError(format!(
                "Circular dependency detected involving: {}",
                cycle_members.join(", ")
            )));
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
                        if edge.to == package_name && !edge.constraint.satisfies(version) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::{RpmVersion, VersionConstraint};

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

    // --- Empty graph ---

    #[test]
    fn test_empty_graph_topological_sort() {
        let graph = DependencyGraph::new();
        let sorted = graph.topological_sort().unwrap();
        assert!(sorted.is_empty());
    }

    #[test]
    fn test_empty_graph_no_cycle() {
        let graph = DependencyGraph::new();
        assert!(graph.detect_cycle().is_none());
    }

    #[test]
    fn test_empty_graph_stats() {
        let graph = DependencyGraph::new();
        let stats = graph.stats();
        assert_eq!(stats.total_packages, 0);
        assert_eq!(stats.total_dependencies, 0);
    }

    // --- Single node ---

    #[test]
    fn test_single_node_topological_sort() {
        let mut graph = DependencyGraph::new();
        graph.add_node(node("solo"));
        let sorted = graph.topological_sort().unwrap();
        assert_eq!(sorted, vec!["solo"]);
    }

    // --- Linear chain ---

    #[test]
    fn test_linear_chain_topological_order() {
        // A -> B -> C -> D (A depends on B, B on C, C on D)
        let mut graph = DependencyGraph::new();
        for name in &["A", "B", "C", "D"] {
            graph.add_node(node(name));
        }
        graph.add_edge(edge("A", "B"));
        graph.add_edge(edge("B", "C"));
        graph.add_edge(edge("C", "D"));

        let sorted = graph.topological_sort().unwrap();
        assert_eq!(sorted.len(), 4);

        // Installation order: D first (no deps), then C, B, A
        let pos = |n: &str| sorted.iter().position(|x| x == n).unwrap();
        assert!(pos("D") < pos("C"));
        assert!(pos("C") < pos("B"));
        assert!(pos("B") < pos("A"));
    }

    // --- Diamond pattern ---

    #[test]
    fn test_diamond_topological_order() {
        //     A
        //    / \
        //   B   C
        //    \ /
        //     D
        let mut graph = DependencyGraph::new();
        for name in &["A", "B", "C", "D"] {
            graph.add_node(node(name));
        }
        graph.add_edge(edge("A", "B"));
        graph.add_edge(edge("A", "C"));
        graph.add_edge(edge("B", "D"));
        graph.add_edge(edge("C", "D"));

        let sorted = graph.topological_sort().unwrap();
        assert_eq!(sorted.len(), 4);

        let pos = |n: &str| sorted.iter().position(|x| x == n).unwrap();
        assert!(pos("D") < pos("B"));
        assert!(pos("D") < pos("C"));
        assert!(pos("B") < pos("A"));
        assert!(pos("C") < pos("A"));
    }

    // --- Cycle detection ---

    #[test]
    fn test_real_cycle_topological_sort_error() {
        // A -> B -> C -> A
        let mut graph = DependencyGraph::new();
        for name in &["A", "B", "C"] {
            graph.add_node(node(name));
        }
        graph.add_edge(edge("A", "B"));
        graph.add_edge(edge("B", "C"));
        graph.add_edge(edge("C", "A"));

        let result = graph.topological_sort();
        assert!(result.is_err());

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Circular dependency"),
            "Error should mention circular dependency, got: {err_msg}"
        );
        // The error should name the involved packages
        assert!(err_msg.contains("A"), "Error should mention A: {err_msg}");
        assert!(err_msg.contains("B"), "Error should mention B: {err_msg}");
        assert!(err_msg.contains("C"), "Error should mention C: {err_msg}");
    }

    #[test]
    fn test_real_cycle_detect_cycle() {
        // A -> B -> A
        let mut graph = DependencyGraph::new();
        graph.add_node(node("A"));
        graph.add_node(node("B"));
        graph.add_edge(edge("A", "B"));
        graph.add_edge(edge("B", "A"));

        let cycle = graph.detect_cycle();
        assert!(cycle.is_some());
        let cycle = cycle.unwrap();
        assert!(cycle.contains(&"A".to_string()));
        assert!(cycle.contains(&"B".to_string()));
    }

    #[test]
    fn test_self_cycle() {
        // A -> A
        let mut graph = DependencyGraph::new();
        graph.add_node(node("A"));
        graph.add_edge(edge("A", "A"));

        let cycle = graph.detect_cycle();
        assert!(cycle.is_some());

        let result = graph.topological_sort();
        assert!(result.is_err());
    }

    #[test]
    fn test_cycle_with_non_cyclic_nodes() {
        // D -> A -> B -> C -> A (cycle among A,B,C; D depends on A but is not in cycle)
        let mut graph = DependencyGraph::new();
        for name in &["A", "B", "C", "D"] {
            graph.add_node(node(name));
        }
        graph.add_edge(edge("D", "A"));
        graph.add_edge(edge("A", "B"));
        graph.add_edge(edge("B", "C"));
        graph.add_edge(edge("C", "A"));

        let result = graph.topological_sort();
        assert!(result.is_err());

        let err_msg = result.unwrap_err().to_string();
        // D should NOT be in the cycle members (it has no incoming cycle edges)
        // A, B, C should be mentioned
        assert!(err_msg.contains("A"), "Error should mention A: {err_msg}");
        assert!(err_msg.contains("B"), "Error should mention B: {err_msg}");
        assert!(err_msg.contains("C"), "Error should mention C: {err_msg}");
    }

    // --- Phantom (missing) dependency handling ---

    #[test]
    fn test_phantom_dependency_does_not_cause_false_cycle() {
        // A depends on B, but B is not a node in the graph (missing package).
        // This should NOT be reported as a cycle.
        let mut graph = DependencyGraph::new();
        graph.add_node(node("A"));
        graph.add_edge(edge("A", "phantom-lib"));

        let result = graph.topological_sort();
        assert!(
            result.is_ok(),
            "Phantom dependency should not cause cycle error, got: {:?}",
            result.unwrap_err()
        );
        let sorted = result.unwrap();
        assert_eq!(sorted, vec!["A"]);
    }

    #[test]
    fn test_multiple_phantom_dependencies_no_false_cycle() {
        // A -> phantom1, A -> phantom2, A -> B (B exists)
        let mut graph = DependencyGraph::new();
        graph.add_node(node("A"));
        graph.add_node(node("B"));
        graph.add_edge(edge("A", "B"));
        graph.add_edge(edge("A", "phantom1"));
        graph.add_edge(edge("A", "phantom2"));

        let result = graph.topological_sort();
        assert!(
            result.is_ok(),
            "Multiple phantom deps should not cause false cycle: {:?}",
            result.unwrap_err()
        );
        let sorted = result.unwrap();
        assert_eq!(sorted.len(), 2);
        let pos = |n: &str| sorted.iter().position(|x| x == n).unwrap();
        assert!(pos("B") < pos("A"));
    }

    #[test]
    fn test_chain_with_phantom_leaf_no_false_cycle() {
        // A -> B -> C -> phantom-lib
        // All of A, B, C exist but C depends on a missing package.
        let mut graph = DependencyGraph::new();
        for name in &["A", "B", "C"] {
            graph.add_node(node(name));
        }
        graph.add_edge(edge("A", "B"));
        graph.add_edge(edge("B", "C"));
        graph.add_edge(edge("C", "phantom-lib"));

        let result = graph.topological_sort();
        assert!(
            result.is_ok(),
            "Phantom leaf dep should not cause false cycle: {:?}",
            result.unwrap_err()
        );
        let sorted = result.unwrap();
        assert_eq!(sorted.len(), 3);
        let pos = |n: &str| sorted.iter().position(|x| x == n).unwrap();
        assert!(pos("C") < pos("B"));
        assert!(pos("B") < pos("A"));
    }

    // --- detect_cycle_involving ---

    #[test]
    fn test_detect_cycle_involving_target_package() {
        // A -> B -> A (cycle), C -> D (no cycle)
        let mut graph = DependencyGraph::new();
        for name in &["A", "B", "C", "D"] {
            graph.add_node(node(name));
        }
        graph.add_edge(edge("A", "B"));
        graph.add_edge(edge("B", "A"));
        graph.add_edge(edge("C", "D"));

        // Detecting for A should find the cycle
        assert!(graph.detect_cycle_involving("A").is_some());
        // Detecting for C should find no cycle
        assert!(graph.detect_cycle_involving("C").is_none());
    }

    // --- Graph construction ---

    #[test]
    fn test_add_node_and_retrieve() {
        let mut graph = DependencyGraph::new();
        let n = PackageNode::new("pkg".to_string(), v("2.5.1"));
        graph.add_node(n.clone());

        assert_eq!(graph.get_node("pkg"), Some(&n));
        assert_eq!(graph.get_node("nonexistent"), None);
    }

    #[test]
    fn test_add_edge_forward_and_reverse() {
        let mut graph = DependencyGraph::new();
        graph.add_node(node("app"));
        graph.add_node(node("lib"));
        graph.add_edge(edge("app", "lib"));

        let deps = graph.get_dependencies("app");
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].to, "lib");

        let dependents = graph.get_dependents("lib");
        assert_eq!(dependents, vec!["app"]);
    }

    #[test]
    fn test_node_with_trove_id() {
        let n = PackageNode::new("pkg".to_string(), v("1.0.0")).with_trove_id(42);
        assert_eq!(n.trove_id, Some(42));
    }

    // --- Version constraint checking ---

    #[test]
    fn test_check_constraints_with_multiple_dependents() {
        let mut graph = DependencyGraph::new();
        graph.add_node(node("lib"));
        graph.add_node(node("app1"));
        graph.add_node(node("app2"));

        graph.add_edge(DependencyEdge {
            from: "app1".to_string(),
            to: "lib".to_string(),
            constraint: VersionConstraint::parse(">= 1.0.0").unwrap(),
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        });
        graph.add_edge(DependencyEdge {
            from: "app2".to_string(),
            to: "lib".to_string(),
            constraint: VersionConstraint::parse(">= 0.5.0").unwrap(),
            dep_type: "runtime".to_string(),
            kind: "package".to_string(),
        });

        // lib is 1.0.0, both constraints satisfied
        assert!(graph.check_constraints("lib", &v("1.0.0")).is_ok());
        // lib is 0.3.0, both constraints violated
        assert!(graph.check_constraints("lib", &v("0.3.0")).is_err());
    }

    // --- Typed edges ---

    #[test]
    fn test_typed_edge_constructor() {
        let e = DependencyEdge::typed(
            "app".to_string(),
            "libfoo.so.1".to_string(),
            VersionConstraint::Any,
            "runtime".to_string(),
            "soname".to_string(),
        );
        assert_eq!(e.kind, "soname");
        assert_eq!(e.from, "app");
        assert_eq!(e.to, "libfoo.so.1");
    }

    // --- Default trait ---

    #[test]
    fn test_default_graph() {
        let graph = DependencyGraph::default();
        assert_eq!(graph.nodes.len(), 0);
        assert_eq!(graph.edges.len(), 0);
    }
}
