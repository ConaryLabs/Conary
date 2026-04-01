// conary-core/src/derivation/graph.rs

//! Shared graph algorithms for the derivation module.

use std::collections::HashMap;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::recipe::Recipe;

/// Topologically sort packages using Kahn's algorithm.
///
/// Only considers dependency edges where both endpoints are in the `packages` set
/// (cross-stage dependencies are ignored for ordering purposes). Uses `BTreeMap`
/// and `BTreeSet` throughout for deterministic output.
///
/// Returns `Err(())` if the dependency graph contains a cycle.
pub(crate) fn topological_sort(
    packages: &BTreeSet<String>,
    recipes: &HashMap<String, Recipe>,
) -> Result<Vec<String>, ()> {
    if packages.is_empty() {
        return Ok(Vec::new());
    }

    // Build adjacency list and in-degree map (only for edges within the package set).
    let mut adjacency: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut in_degree: BTreeMap<&str, usize> = BTreeMap::new();

    // Initialize all packages with zero in-degree.
    for pkg in packages {
        adjacency.entry(pkg.as_str()).or_default();
        in_degree.entry(pkg.as_str()).or_insert(0);
    }

    // Add edges: if B depends on A, add A -> B (A must be built before B).
    for pkg in packages {
        if let Some(recipe) = recipes.get(pkg) {
            for dep in &recipe.build.requires {
                let dep_name = dep.as_str();
                if packages.contains(dep_name)
                    && dep_name != pkg.as_str()
                    && adjacency.entry(dep_name).or_default().insert(pkg.as_str())
                {
                    *in_degree.entry(pkg.as_str()).or_insert(0) += 1;
                }
            }
            for dep in &recipe.build.makedepends {
                let dep_name = dep.as_str();
                if packages.contains(dep_name)
                    && dep_name != pkg.as_str()
                    && adjacency.entry(dep_name).or_default().insert(pkg.as_str())
                {
                    *in_degree.entry(pkg.as_str()).or_insert(0) += 1;
                }
            }
        }
    }

    // Kahn's algorithm with a BTreeSet-based queue for deterministic ordering.
    let mut queue: VecDeque<&str> = VecDeque::new();
    let mut zero_degree: BTreeSet<&str> = BTreeSet::new();
    for (pkg, &deg) in &in_degree {
        if deg == 0 {
            zero_degree.insert(pkg);
        }
    }
    for pkg in &zero_degree {
        queue.push_back(pkg);
    }

    let mut result = Vec::with_capacity(packages.len());

    while let Some(node) = queue.pop_front() {
        result.push(node.to_string());

        // Collect newly freed nodes in sorted order for determinism.
        let mut newly_free: BTreeSet<&str> = BTreeSet::new();
        if let Some(neighbors) = adjacency.get(node) {
            for &neighbor in neighbors {
                let deg = in_degree.get_mut(neighbor).expect("node must exist");
                *deg -= 1;
                if *deg == 0 {
                    newly_free.insert(neighbor);
                }
            }
        }
        for pkg in &newly_free {
            queue.push_back(pkg);
        }
    }

    if result.len() != packages.len() {
        return Err(());
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derivation::test_helpers::helpers::make_recipe;

    #[test]
    fn test_respects_dependencies() {
        let mut recipes = HashMap::new();
        recipes.insert("a".to_string(), make_recipe("a", &[], &[]));
        recipes.insert("b".to_string(), make_recipe("b", &["a"], &[]));
        recipes.insert("c".to_string(), make_recipe("c", &["b"], &[]));

        let packages: BTreeSet<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

        let sorted = topological_sort(&packages, &recipes).unwrap();
        assert_eq!(sorted, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_cycle_detection() {
        let mut recipes = HashMap::new();
        recipes.insert("a".to_string(), make_recipe("a", &["c"], &[]));
        recipes.insert("b".to_string(), make_recipe("b", &["a"], &[]));
        recipes.insert("c".to_string(), make_recipe("c", &["b"], &[]));

        let packages: BTreeSet<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

        assert!(topological_sort(&packages, &recipes).is_err());
    }

    #[test]
    fn test_empty() {
        let recipes = HashMap::new();
        let packages = BTreeSet::new();

        let sorted = topological_sort(&packages, &recipes).unwrap();
        assert!(sorted.is_empty());
    }

    #[test]
    fn test_independent_packages_alphabetical() {
        let mut recipes = HashMap::new();
        recipes.insert("zlib".to_string(), make_recipe("zlib", &[], &[]));
        recipes.insert("curl".to_string(), make_recipe("curl", &[], &[]));
        recipes.insert("bzip2".to_string(), make_recipe("bzip2", &[], &[]));

        let packages: BTreeSet<String> = ["zlib", "curl", "bzip2"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let sorted = topological_sort(&packages, &recipes).unwrap();
        assert_eq!(sorted, vec!["bzip2", "curl", "zlib"]);
    }
}
