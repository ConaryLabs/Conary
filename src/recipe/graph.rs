// src/recipe/graph.rs

//! Recipe dependency graph for build ordering
//!
//! This module provides a directed graph for tracking dependencies between
//! recipes and determining the correct build order using topological sort.
//!
//! # Example
//!
//! ```ignore
//! use conary::recipe::graph::RecipeGraph;
//!
//! let mut graph = RecipeGraph::new();
//!
//! // Add recipes with their dependencies
//! graph.add_recipe("glibc", &["linux-headers"]);
//! graph.add_recipe("gcc", &["glibc", "binutils"]);
//! graph.add_recipe("binutils", &["glibc"]);
//! graph.add_recipe("linux-headers", &[]);
//!
//! // Get topological order for building
//! let order = graph.topological_sort().unwrap();
//! // order: ["linux-headers", "glibc", "binutils", "gcc"]
//! ```
//!
//! # Circular Dependencies
//!
//! Bootstrap scenarios often have circular dependencies (e.g., gcc ↔ glibc).
//! These are detected and can be broken by specifying "bootstrap edges" that
//! are ignored during topological sort.

use crate::error::{Error, Result};
use crate::recipe::format::BuildStage;
use std::collections::{HashMap, HashSet, VecDeque};

/// A directed graph representing recipe dependencies
#[derive(Debug, Default)]
pub struct RecipeGraph {
    /// Map from recipe name to its outgoing edges (dependencies)
    /// Key: recipe name, Value: set of recipes this recipe depends on
    edges: HashMap<String, HashSet<String>>,
    /// Reverse edges for finding dependents
    /// Key: recipe name, Value: set of recipes that depend on this recipe
    reverse_edges: HashMap<String, HashSet<String>>,
    /// Edges to ignore during topological sort (for breaking cycles)
    bootstrap_edges: HashSet<(String, String)>,
}

impl RecipeGraph {
    /// Create a new empty recipe graph
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a recipe with its dependencies
    ///
    /// If the recipe already exists, this merges the dependencies.
    pub fn add_recipe(&mut self, name: &str, dependencies: &[&str]) {
        let name = name.to_string();

        // Ensure the recipe exists in the graph
        self.edges.entry(name.clone()).or_default();
        self.reverse_edges.entry(name.clone()).or_default();

        // Add dependencies
        for dep in dependencies {
            let dep = dep.to_string();

            // Ensure the dependency exists as a node
            self.edges.entry(dep.clone()).or_default();
            self.reverse_edges.entry(dep.clone()).or_default();

            // Add the edge: name -> dep (name depends on dep)
            self.edges.get_mut(&name).unwrap().insert(dep.clone());
            // Add reverse edge: dep <- name (dep is depended on by name)
            self.reverse_edges.get_mut(&dep).unwrap().insert(name.clone());
        }
    }

    /// Add a recipe from a Recipe struct, extracting all build dependencies
    pub fn add_from_recipe(&mut self, recipe: &super::Recipe) {
        let deps: Vec<&str> = recipe.all_build_deps();
        self.add_recipe(&recipe.package.name, &deps);
    }

    /// Mark an edge as a "bootstrap edge" to be ignored during topological sort
    ///
    /// This is used to break circular dependencies in bootstrap scenarios.
    /// The edge from `from` to `to` (meaning `from` depends on `to`) will be
    /// ignored when computing build order.
    pub fn mark_bootstrap_edge(&mut self, from: &str, to: &str) {
        self.bootstrap_edges
            .insert((from.to_string(), to.to_string()));
    }

    /// Get the number of recipes in the graph
    pub fn recipe_count(&self) -> usize {
        self.edges.len()
    }

    /// Check if a recipe exists in the graph
    pub fn contains(&self, name: &str) -> bool {
        self.edges.contains_key(name)
    }

    /// Get the direct dependencies of a recipe
    pub fn dependencies(&self, name: &str) -> Option<&HashSet<String>> {
        self.edges.get(name)
    }

    /// Get the recipes that directly depend on this recipe
    pub fn dependents(&self, name: &str) -> Option<&HashSet<String>> {
        self.reverse_edges.get(name)
    }

    /// Compute the in-degree of each node (accounting for bootstrap edges)
    fn compute_in_degrees(&self) -> HashMap<String, usize> {
        let mut in_degrees: HashMap<String, usize> = HashMap::new();

        // Initialize all nodes with 0 in-degree
        for name in self.edges.keys() {
            in_degrees.insert(name.clone(), 0);
        }

        // Count incoming edges (excluding bootstrap edges)
        for (name, deps) in &self.edges {
            for dep in deps {
                // Skip bootstrap edges
                if self.bootstrap_edges.contains(&(name.clone(), dep.clone())) {
                    continue;
                }
                // This edge means `name` depends on `dep`, so `dep` has an incoming edge FROM `name`
                // Wait, that's backwards. Let me reconsider.
                //
                // edges[name] contains deps that `name` depends on.
                // So there's an edge name -> dep (name depends on dep).
                // For topological sort, we need in-degree of each node.
                // In-degree of X = number of nodes that depend on X = number of edges pointing TO X.
                // Since edges[name] contains deps that name depends on,
                // the edge direction is name -> dep (name points to dep).
                // So the in-degree of `dep` is increased by this edge.
                //
                // Actually, for Kahn's algorithm, we want in-degree = number of prerequisites.
                // If name depends on dep, then `name` can only be built after `dep`.
                // So `name` has `dep` as a prerequisite, meaning in-degree of `name` should include dep.
                //
                // Let me reconsider the edge direction:
                // - edges[name] = set of things `name` depends on (prerequisites of name)
                // - For topological sort, node X has in-degree = |prerequisites of X|
                // - So in_degrees[name] = |edges[name]|

                // Actually, we're counting in-degree correctly above. The issue is:
                // We initialized in_degrees, then we're iterating edges and for each dep in edges[name],
                // we're... doing nothing with in_degrees right now.
                //
                // Let me fix this properly.
            }
        }

        // Actually compute in-degrees: for each node, count its dependencies
        for (name, deps) in &self.edges {
            let effective_deps: usize = deps
                .iter()
                .filter(|dep| !self.bootstrap_edges.contains(&(name.clone(), (*dep).clone())))
                .count();
            *in_degrees.get_mut(name).unwrap() = effective_deps;
        }

        in_degrees
    }

    /// Perform topological sort using Kahn's algorithm
    ///
    /// Returns the recipes in build order (dependencies before dependents).
    /// Returns an error if there's a cycle (that isn't broken by bootstrap edges).
    pub fn topological_sort(&self) -> Result<Vec<String>> {
        let mut in_degrees = self.compute_in_degrees();
        let mut result = Vec::with_capacity(self.edges.len());

        // Queue of nodes with no remaining dependencies
        let mut queue: VecDeque<String> = in_degrees
            .iter()
            .filter(|&(_, deg)| *deg == 0)
            .map(|(name, _)| name.clone())
            .collect();

        while let Some(node) = queue.pop_front() {
            result.push(node.clone());

            // For each recipe that depends on this node...
            if let Some(dependents) = self.reverse_edges.get(&node) {
                for dependent in dependents {
                    // Skip if this is a bootstrap edge
                    if self
                        .bootstrap_edges
                        .contains(&(dependent.clone(), node.clone()))
                    {
                        continue;
                    }

                    // Decrement in-degree
                    if let Some(deg) = in_degrees.get_mut(dependent) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 {
                            queue.push_back(dependent.clone());
                        }
                    }
                }
            }
        }

        // Check if all nodes were processed
        if result.len() != self.edges.len() {
            // Find the cycle
            let remaining: Vec<String> = self
                .edges
                .keys()
                .filter(|k| !result.contains(k))
                .cloned()
                .collect();

            return Err(Error::ResolutionError(format!(
                "Circular dependency detected. Remaining recipes: {}. \
                 Use mark_bootstrap_edge() to break the cycle.",
                remaining.join(", ")
            )));
        }

        Ok(result)
    }

    /// Find all cycles in the graph
    ///
    /// Returns a list of cycles, where each cycle is a list of recipe names.
    /// Useful for identifying which bootstrap edges need to be added.
    pub fn find_cycles(&self) -> Vec<Vec<String>> {
        let mut cycles = Vec::new();
        let mut visited = HashSet::new();
        let mut rec_stack = HashSet::new();
        let mut path = Vec::new();

        for start in self.edges.keys() {
            if !visited.contains(start) {
                self.find_cycles_dfs(start, &mut visited, &mut rec_stack, &mut path, &mut cycles);
            }
        }

        cycles
    }

    fn find_cycles_dfs(
        &self,
        node: &str,
        visited: &mut HashSet<String>,
        rec_stack: &mut HashSet<String>,
        path: &mut Vec<String>,
        cycles: &mut Vec<Vec<String>>,
    ) {
        visited.insert(node.to_string());
        rec_stack.insert(node.to_string());
        path.push(node.to_string());

        if let Some(deps) = self.edges.get(node) {
            for dep in deps {
                if !visited.contains(dep) {
                    self.find_cycles_dfs(dep, visited, rec_stack, path, cycles);
                } else if rec_stack.contains(dep) {
                    // Found a cycle - extract it from path
                    let cycle_start = path.iter().position(|x| x == dep).unwrap();
                    let cycle: Vec<String> = path[cycle_start..].to_vec();
                    cycles.push(cycle);
                }
            }
        }

        path.pop();
        rec_stack.remove(node);
    }

    /// Get all recipes that a given recipe transitively depends on
    pub fn transitive_dependencies(&self, name: &str) -> HashSet<String> {
        let mut deps = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();

        if let Some(direct_deps) = self.edges.get(name) {
            for dep in direct_deps {
                queue.push_back(dep.clone());
            }
        }

        while let Some(dep) = queue.pop_front() {
            if deps.insert(dep.clone()) {
                if let Some(indirect_deps) = self.edges.get(&dep) {
                    for indirect in indirect_deps {
                        if !deps.contains(indirect) {
                            queue.push_back(indirect.clone());
                        }
                    }
                }
            }
        }

        deps
    }

    /// Get all recipes that transitively depend on a given recipe
    pub fn transitive_dependents(&self, name: &str) -> HashSet<String> {
        let mut dependents = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();

        if let Some(direct) = self.reverse_edges.get(name) {
            for dep in direct {
                queue.push_back(dep.clone());
            }
        }

        while let Some(dep) = queue.pop_front() {
            if dependents.insert(dep.clone()) {
                if let Some(indirect) = self.reverse_edges.get(&dep) {
                    for ind in indirect {
                        if !dependents.contains(ind) {
                            queue.push_back(ind.clone());
                        }
                    }
                }
            }
        }

        dependents
    }

    /// Suggest bootstrap edges to break circular dependencies
    ///
    /// This analyzes cycles in the graph and suggests which edges to break
    /// based on common patterns (e.g., glibc -> gcc in bootstrap).
    ///
    /// Returns a list of (from, to) edges that could be marked as bootstrap edges.
    pub fn suggest_bootstrap_edges(&self) -> Vec<(String, String)> {
        let cycles = self.find_cycles();
        let mut suggestions = Vec::new();

        // Known patterns that indicate which edge to break
        // These are based on LFS/Gentoo bootstrap experience
        let bootstrap_patterns = [
            // glibc normally depends on gcc, but in stage0 we use cross-gcc
            ("glibc", "gcc"),
            // Same pattern for musl
            ("musl", "gcc"),
            // libstdc++ needs glibc, but stage1 can use minimal glibc
            ("libstdc++", "glibc"),
            // Perl/Python have chicken-egg with system libs
            ("perl", "glibc"),
            ("python", "glibc"),
        ];

        for cycle in cycles {
            // Check if any known pattern matches this cycle
            for (from, to) in &bootstrap_patterns {
                if cycle.contains(&from.to_string()) && cycle.contains(&to.to_string()) {
                    let edge = (from.to_string(), to.to_string());
                    if !suggestions.contains(&edge) {
                        suggestions.push(edge);
                    }
                }
            }

            // If no pattern matched, suggest breaking at the "least critical" edge
            // Heuristic: break the edge from the recipe with most dependents
            if suggestions.is_empty() && cycle.len() >= 2 {
                let mut best_break = None;
                let mut best_score = 0;

                for i in 0..cycle.len() {
                    let from = &cycle[i];
                    let to = &cycle[(i + 1) % cycle.len()];

                    // Score by number of dependents (more dependents = more critical = less likely to break)
                    let score = self
                        .reverse_edges
                        .get(from)
                        .map_or(0, |deps| deps.len());

                    if best_break.is_none() || score < best_score {
                        best_break = Some((from.clone(), to.clone()));
                        best_score = score;
                    }
                }

                if let Some(edge) = best_break {
                    suggestions.push(edge);
                }
            }
        }

        suggestions
    }

    /// Auto-break detected cycles using suggested bootstrap edges
    ///
    /// This is a convenience method that calls `suggest_bootstrap_edges()`
    /// and marks all suggested edges as bootstrap edges.
    ///
    /// Returns the edges that were marked.
    pub fn auto_break_cycles(&mut self) -> Vec<(String, String)> {
        let suggestions = self.suggest_bootstrap_edges();

        for (from, to) in &suggestions {
            self.mark_bootstrap_edge(from, to);
        }

        suggestions
    }

    /// Clear all bootstrap edges
    pub fn clear_bootstrap_edges(&mut self) {
        self.bootstrap_edges.clear();
    }

    /// Get all marked bootstrap edges
    pub fn bootstrap_edges(&self) -> &HashSet<(String, String)> {
        &self.bootstrap_edges
    }
}

/// A bootstrap build phase
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapPhase {
    /// Name of this phase
    pub name: String,
    /// The build stage for recipes in this phase
    pub stage: BuildStage,
    /// Recipes to build in this phase (in order)
    pub recipes: Vec<String>,
    /// Description of what this phase accomplishes
    pub description: String,
}

/// A complete bootstrap plan
///
/// This represents a multi-phase bootstrap plan that can build a system
/// from scratch, handling circular dependencies by building packages
/// in multiple passes.
#[derive(Debug, Clone)]
pub struct BootstrapPlan {
    /// The phases of the bootstrap, in order
    pub phases: Vec<BootstrapPhase>,
    /// Bootstrap edges that were identified
    pub bootstrap_edges: Vec<(String, String)>,
}

impl BootstrapPlan {
    /// Create a bootstrap plan from a recipe graph
    ///
    /// This analyzes the graph and creates a multi-phase plan that:
    /// 1. Identifies circular dependencies and how to break them
    /// 2. Determines which packages need to be built in multiple passes
    /// 3. Organizes builds into stages (stage0, stage1, final)
    pub fn from_graph(graph: &mut RecipeGraph) -> Result<Self> {
        // Find and break cycles
        let bootstrap_edges = graph.auto_break_cycles();

        // Get topological order with cycles broken
        let build_order = graph.topological_sort()?;

        // Identify packages that are part of cycles (need multiple passes)
        let cycle_packages: HashSet<String> = graph
            .find_cycles()
            .into_iter()
            .flatten()
            .collect();

        // Group recipes into phases
        let mut phases = Vec::new();

        // Stage 0: Cross-compiled toolchain
        // Packages with no dependencies or only linux-headers dependency
        let stage0_recipes: Vec<String> = build_order
            .iter()
            .filter(|r| {
                let deps = graph.dependencies(r).map_or(0, |d| d.len());
                deps == 0 || (deps == 1 && graph.dependencies(r).unwrap().contains("linux-headers"))
            })
            .cloned()
            .collect();

        if !stage0_recipes.is_empty() {
            phases.push(BootstrapPhase {
                name: "stage0".to_string(),
                stage: BuildStage::Stage0,
                recipes: stage0_recipes.clone(),
                description: "Cross-compiled initial toolchain from host".to_string(),
            });
        }

        // Stage 1: Self-hosted toolchain rebuild
        // Packages that were in cycles need a second build
        let stage1_recipes: Vec<String> = build_order
            .iter()
            .filter(|r| cycle_packages.contains(*r) && !stage0_recipes.contains(r))
            .cloned()
            .collect();

        if !stage1_recipes.is_empty() {
            phases.push(BootstrapPhase {
                name: "stage1".to_string(),
                stage: BuildStage::Stage1,
                recipes: stage1_recipes.clone(),
                description: "Self-hosted toolchain built with stage0 tools".to_string(),
            });
        }

        // Final: Everything else
        let final_recipes: Vec<String> = build_order
            .iter()
            .filter(|r| !stage0_recipes.contains(r) && !stage1_recipes.contains(r))
            .cloned()
            .collect();

        if !final_recipes.is_empty() {
            phases.push(BootstrapPhase {
                name: "final".to_string(),
                stage: BuildStage::Final,
                recipes: final_recipes,
                description: "Final system packages".to_string(),
            });
        }

        Ok(Self {
            phases,
            bootstrap_edges,
        })
    }

    /// Get the total number of recipes across all phases
    pub fn total_recipes(&self) -> usize {
        self.phases.iter().map(|p| p.recipes.len()).sum()
    }

    /// Get all recipes in order (across all phases)
    pub fn all_recipes(&self) -> Vec<&str> {
        self.phases
            .iter()
            .flat_map(|p| p.recipes.iter().map(|s| s.as_str()))
            .collect()
    }

    /// Find which phase a recipe belongs to
    pub fn phase_for_recipe(&self, recipe: &str) -> Option<&BootstrapPhase> {
        self.phases.iter().find(|p| p.recipes.iter().any(|r| r == recipe))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_graph() {
        let graph = RecipeGraph::new();
        assert_eq!(graph.recipe_count(), 0);
        let order = graph.topological_sort().unwrap();
        assert!(order.is_empty());
    }

    #[test]
    fn test_single_recipe() {
        let mut graph = RecipeGraph::new();
        graph.add_recipe("hello", &[]);

        assert_eq!(graph.recipe_count(), 1);
        assert!(graph.contains("hello"));

        let order = graph.topological_sort().unwrap();
        assert_eq!(order, vec!["hello"]);
    }

    #[test]
    fn test_linear_dependencies() {
        let mut graph = RecipeGraph::new();
        graph.add_recipe("c", &["b"]);
        graph.add_recipe("b", &["a"]);
        graph.add_recipe("a", &[]);

        let order = graph.topological_sort().unwrap();
        assert_eq!(order, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_diamond_dependencies() {
        let mut graph = RecipeGraph::new();
        graph.add_recipe("d", &["b", "c"]);
        graph.add_recipe("b", &["a"]);
        graph.add_recipe("c", &["a"]);
        graph.add_recipe("a", &[]);

        let order = graph.topological_sort().unwrap();

        // a must come first, d must come last
        assert_eq!(order.first(), Some(&"a".to_string()));
        assert_eq!(order.last(), Some(&"d".to_string()));
        // b and c can be in either order
        assert!(order.contains(&"b".to_string()));
        assert!(order.contains(&"c".to_string()));
    }

    #[test]
    fn test_bootstrap_dependencies() {
        // Simulate gcc <-> glibc circular dependency
        let mut graph = RecipeGraph::new();
        graph.add_recipe("gcc", &["glibc", "binutils"]);
        graph.add_recipe("glibc", &["gcc", "linux-headers"]); // Creates cycle with gcc
        graph.add_recipe("binutils", &["glibc"]);
        graph.add_recipe("linux-headers", &[]);

        // Without bootstrap edge, should fail
        let result = graph.topological_sort();
        assert!(result.is_err());

        // Mark the bootstrap edge (glibc -> gcc is the bootstrap edge)
        // This means: during bootstrap, glibc is built with a pre-existing gcc
        graph.mark_bootstrap_edge("glibc", "gcc");

        // Now it should succeed
        let order = graph.topological_sort().unwrap();
        assert_eq!(order.len(), 4);

        // linux-headers must come before glibc
        let lh_pos = order.iter().position(|x| x == "linux-headers").unwrap();
        let glibc_pos = order.iter().position(|x| x == "glibc").unwrap();
        assert!(lh_pos < glibc_pos);
    }

    #[test]
    fn test_cycle_detection() {
        let mut graph = RecipeGraph::new();
        graph.add_recipe("a", &["b"]);
        graph.add_recipe("b", &["c"]);
        graph.add_recipe("c", &["a"]); // Creates cycle

        let cycles = graph.find_cycles();
        assert!(!cycles.is_empty());
    }

    #[test]
    fn test_transitive_dependencies() {
        let mut graph = RecipeGraph::new();
        graph.add_recipe("d", &["c"]);
        graph.add_recipe("c", &["b"]);
        graph.add_recipe("b", &["a"]);
        graph.add_recipe("a", &[]);

        let deps = graph.transitive_dependencies("d");
        assert!(deps.contains("c"));
        assert!(deps.contains("b"));
        assert!(deps.contains("a"));
        assert!(!deps.contains("d")); // Should not contain itself
    }

    #[test]
    fn test_transitive_dependents() {
        let mut graph = RecipeGraph::new();
        graph.add_recipe("d", &["c"]);
        graph.add_recipe("c", &["b"]);
        graph.add_recipe("b", &["a"]);
        graph.add_recipe("a", &[]);

        let dependents = graph.transitive_dependents("a");
        assert!(dependents.contains("b"));
        assert!(dependents.contains("c"));
        assert!(dependents.contains("d"));
        assert!(!dependents.contains("a")); // Should not contain itself
    }

    #[test]
    fn test_dependencies_and_dependents() {
        let mut graph = RecipeGraph::new();
        graph.add_recipe("gcc", &["glibc", "binutils"]);
        graph.add_recipe("glibc", &["linux-headers"]);
        graph.add_recipe("binutils", &["glibc"]);
        graph.add_recipe("linux-headers", &[]);

        // gcc depends on glibc and binutils
        let gcc_deps = graph.dependencies("gcc").unwrap();
        assert!(gcc_deps.contains("glibc"));
        assert!(gcc_deps.contains("binutils"));

        // glibc is depended on by gcc and binutils
        let glibc_dependents = graph.dependents("glibc").unwrap();
        assert!(glibc_dependents.contains("gcc"));
        assert!(glibc_dependents.contains("binutils"));
    }

    #[test]
    fn test_add_recipe_merges_deps() {
        let mut graph = RecipeGraph::new();
        graph.add_recipe("foo", &["a", "b"]);
        graph.add_recipe("foo", &["c"]); // Add more deps

        let deps = graph.dependencies("foo").unwrap();
        assert!(deps.contains("a"));
        assert!(deps.contains("b"));
        assert!(deps.contains("c"));
    }

    #[test]
    fn test_real_bootstrap_scenario() {
        // A realistic bootstrap scenario based on LFS
        let mut graph = RecipeGraph::new();

        // Stage 0: Cross tools from host
        graph.add_recipe("binutils-pass1", &[]);
        graph.add_recipe("gcc-pass1", &["binutils-pass1"]);
        graph.add_recipe("linux-headers", &[]);
        graph.add_recipe("glibc", &["gcc-pass1", "linux-headers"]);
        graph.add_recipe("libstdc++", &["glibc", "gcc-pass1"]);

        // Stage 1: Native tools
        graph.add_recipe("binutils-pass2", &["glibc", "libstdc++"]);
        graph.add_recipe("gcc-pass2", &["binutils-pass2", "glibc", "libstdc++"]);

        let order = graph.topological_sort().unwrap();

        // Verify key ordering constraints
        let positions: HashMap<&str, usize> = order
            .iter()
            .enumerate()
            .map(|(i, s)| (s.as_str(), i))
            .collect();

        assert!(positions["binutils-pass1"] < positions["gcc-pass1"]);
        assert!(positions["gcc-pass1"] < positions["glibc"]);
        assert!(positions["linux-headers"] < positions["glibc"]);
        assert!(positions["glibc"] < positions["libstdc++"]);
        assert!(positions["libstdc++"] < positions["binutils-pass2"]);
        assert!(positions["binutils-pass2"] < positions["gcc-pass2"]);
    }

    #[test]
    fn test_suggest_bootstrap_edges_glibc_gcc() {
        let mut graph = RecipeGraph::new();
        // gcc <-> glibc cycle
        graph.add_recipe("gcc", &["glibc"]);
        graph.add_recipe("glibc", &["gcc"]);

        let suggestions = graph.suggest_bootstrap_edges();

        // Should suggest breaking glibc -> gcc (known pattern)
        assert!(suggestions.contains(&("glibc".to_string(), "gcc".to_string())));
    }

    #[test]
    fn test_auto_break_cycles() {
        let mut graph = RecipeGraph::new();
        graph.add_recipe("gcc", &["glibc"]);
        graph.add_recipe("glibc", &["gcc"]);

        // Should fail before breaking
        assert!(graph.topological_sort().is_err());

        // Auto-break
        let broken = graph.auto_break_cycles();
        assert!(!broken.is_empty());

        // Should succeed after breaking
        let order = graph.topological_sort().unwrap();
        assert_eq!(order.len(), 2);
    }

    #[test]
    fn test_clear_bootstrap_edges() {
        let mut graph = RecipeGraph::new();
        graph.add_recipe("a", &["b"]);
        graph.add_recipe("b", &["a"]);

        graph.mark_bootstrap_edge("a", "b");
        assert!(!graph.bootstrap_edges().is_empty());

        graph.clear_bootstrap_edges();
        assert!(graph.bootstrap_edges().is_empty());
    }

    #[test]
    fn test_bootstrap_plan_simple() {
        let mut graph = RecipeGraph::new();
        graph.add_recipe("linux-headers", &[]);
        graph.add_recipe("glibc", &["linux-headers"]);
        graph.add_recipe("binutils", &["glibc"]);
        graph.add_recipe("gcc", &["glibc", "binutils"]);

        let plan = BootstrapPlan::from_graph(&mut graph).unwrap();

        // Should have at least one phase
        assert!(!plan.phases.is_empty());

        // Total recipes should match graph
        assert_eq!(plan.total_recipes(), 4);

        // linux-headers should be in stage0 (no deps)
        let phase = plan.phase_for_recipe("linux-headers").unwrap();
        assert_eq!(phase.stage, BuildStage::Stage0);
    }

    #[test]
    fn test_bootstrap_plan_with_cycle() {
        let mut graph = RecipeGraph::new();
        graph.add_recipe("linux-headers", &[]);
        graph.add_recipe("gcc", &["glibc", "binutils"]);
        graph.add_recipe("glibc", &["gcc", "linux-headers"]); // cycle with gcc
        graph.add_recipe("binutils", &["glibc"]);

        let plan = BootstrapPlan::from_graph(&mut graph).unwrap();

        // Should have identified bootstrap edges
        assert!(!plan.bootstrap_edges.is_empty());

        // Should be able to get all recipes
        let all = plan.all_recipes();
        assert_eq!(all.len(), 4);
    }

    #[test]
    fn test_bootstrap_plan_phases() {
        let mut graph = RecipeGraph::new();
        graph.add_recipe("linux-headers", &[]);
        graph.add_recipe("binutils-pass1", &[]);
        graph.add_recipe("gcc-pass1", &["binutils-pass1"]);
        graph.add_recipe("glibc", &["gcc-pass1", "linux-headers"]);
        graph.add_recipe("app", &["glibc"]);

        let plan = BootstrapPlan::from_graph(&mut graph).unwrap();

        // linux-headers and binutils-pass1 have no deps, should be stage0
        let lh_phase = plan.phase_for_recipe("linux-headers").unwrap();
        let bp1_phase = plan.phase_for_recipe("binutils-pass1").unwrap();
        assert_eq!(lh_phase.stage, BuildStage::Stage0);
        assert_eq!(bp1_phase.stage, BuildStage::Stage0);

        // app depends on glibc, should be in a later stage
        let app_phase = plan.phase_for_recipe("app");
        assert!(app_phase.is_some());
    }
}
