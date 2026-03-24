// conary-core/src/derivation/build_order.rs

//! Flat topological build ordering across all packages.
//!
//! Unlike [`super::stages`], which assigns packages to discrete build stages,
//! this module produces a single, deterministically ordered [`Vec<BuildStep>`]
//! across ALL packages. [`BuildPhase`] labels are informational (for progress
//! reporting) and do not act as build boundaries.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;

use crate::recipe::Recipe;

/// Informational phase label for a build step.
///
/// The ordering `Toolchain < Foundation < System < Customization` is used for
/// [`PartialOrd`]/[`Ord`] only; the phases do **not** introduce build
/// boundaries -- all packages are sorted into a single flat sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BuildPhase {
    /// Core toolchain components (headers, libc, binutils, libstdc++).
    Toolchain,
    /// Essential build tools and libraries.
    Foundation,
    /// All other system packages.
    System,
    /// User-supplied custom packages.
    Customization,
}

impl fmt::Display for BuildPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Toolchain => write!(f, "Toolchain"),
            Self::Foundation => write!(f, "Foundation"),
            Self::System => write!(f, "System"),
            Self::Customization => write!(f, "Customization"),
        }
    }
}

/// A single step in the flat build plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildStep {
    /// Package name.
    pub package: String,
    /// Zero-based position in the sorted build sequence.
    pub order: usize,
    /// Informational phase label.
    pub phase: BuildPhase,
}

/// Errors produced by [`compute_build_order`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BuildOrderError {
    /// The dependency graph contains a cycle; a topological ordering is impossible.
    #[error("cyclic dependency detected in build graph")]
    CyclicDependency,
}

// ---------------------------------------------------------------------------
// Phase classification tables
// ---------------------------------------------------------------------------

const TOOLCHAIN_NAMED: &[&str] = &["linux-headers", "glibc", "binutils", "libstdcxx"];

const FOUNDATION_NAMED: &[&str] = &[
    "make",
    "bash",
    "coreutils",
    "sed",
    "gawk",
    "grep",
    "findutils",
    "diffutils",
    "patch",
    "tar",
    "gzip",
    "xz",
    "bzip2",
    "m4",
    "bison",
    "flex",
    "gettext",
    "perl",
    "python",
    "texinfo",
    "util-linux",
    "file",
    "ncurses",
    "readline",
    "zlib",
];

fn classify_phase(package: &str, custom_packages: &HashSet<String>) -> BuildPhase {
    if custom_packages.contains(package) {
        return BuildPhase::Customization;
    }
    if TOOLCHAIN_NAMED.contains(&package) {
        return BuildPhase::Toolchain;
    }
    if FOUNDATION_NAMED.contains(&package) {
        return BuildPhase::Foundation;
    }
    BuildPhase::System
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compute a flat, topologically sorted build plan for all `recipes`.
///
/// Every recipe in `recipes` is included exactly once in the output. Packages
/// listed in `custom_packages` receive the [`BuildPhase::Customization`] label.
///
/// # Errors
///
/// Returns [`BuildOrderError::CyclicDependency`] if the combined dependency
/// graph contains a cycle.
pub fn compute_build_order(
    recipes: &HashMap<String, Recipe>,
    custom_packages: &HashSet<String>,
) -> Result<Vec<BuildStep>, BuildOrderError> {
    // Collect all package names into a sorted set for determinism.
    let packages: BTreeSet<String> = recipes.keys().cloned().collect();

    let sorted = topological_sort(&packages, recipes)?;

    let steps = sorted
        .into_iter()
        .enumerate()
        .map(|(order, package)| {
            let phase = classify_phase(&package, custom_packages);
            BuildStep {
                package,
                order,
                phase,
            }
        })
        .collect();

    Ok(steps)
}

// ---------------------------------------------------------------------------
// Topological sort (Kahn's algorithm, copied from stages.rs)
// ---------------------------------------------------------------------------

/// Topologically sort `packages` using Kahn's algorithm.
///
/// Only dependency edges where **both** endpoints exist in `packages` are
/// considered. Uses [`BTreeMap`]/[`BTreeSet`] throughout for deterministic
/// output.
///
/// # Errors
///
/// Returns [`BuildOrderError::CyclicDependency`] if the graph contains a cycle.
fn topological_sort(
    packages: &BTreeSet<String>,
    recipes: &HashMap<String, Recipe>,
) -> Result<Vec<String>, BuildOrderError> {
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
        return Err(BuildOrderError::CyclicDependency);
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::*;
    use crate::derivation::test_helpers::helpers::make_recipe;

    /// gcc depends on gmp via makedepends; gmp must appear before gcc.
    #[test]
    fn test_build_order_respects_makedepends() {
        let mut recipes = HashMap::new();
        recipes.insert("gmp".to_string(), make_recipe("gmp", &[], &[]));
        recipes.insert("gcc".to_string(), make_recipe("gcc", &[], &["gmp"]));

        let steps = compute_build_order(&recipes, &HashSet::new())
            .expect("no cycle");

        let gmp_order = steps
            .iter()
            .find(|s| s.package == "gmp")
            .expect("gmp present")
            .order;
        let gcc_order = steps
            .iter()
            .find(|s| s.package == "gcc")
            .expect("gcc present")
            .order;

        assert!(
            gmp_order < gcc_order,
            "gmp ({gmp_order}) must precede gcc ({gcc_order})"
        );
    }

    /// Phase classification: glibc=Toolchain, bash=Foundation, openssl=System,
    /// a custom package=Customization.
    #[test]
    fn test_phase_classification() {
        let mut recipes = HashMap::new();
        recipes.insert("glibc".to_string(), make_recipe("glibc", &[], &[]));
        recipes.insert("bash".to_string(), make_recipe("bash", &[], &[]));
        recipes.insert("openssl".to_string(), make_recipe("openssl", &[], &[]));
        recipes.insert(
            "my-custom-app".to_string(),
            make_recipe("my-custom-app", &[], &[]),
        );

        let mut custom = HashSet::new();
        custom.insert("my-custom-app".to_string());

        let steps = compute_build_order(&recipes, &custom).expect("no cycle");

        let phase_of = |name: &str| {
            steps
                .iter()
                .find(|s| s.package == name)
                .unwrap_or_else(|| panic!("{name} not found"))
                .phase
        };

        assert_eq!(phase_of("glibc"), BuildPhase::Toolchain);
        assert_eq!(phase_of("bash"), BuildPhase::Foundation);
        assert_eq!(phase_of("openssl"), BuildPhase::System);
        assert_eq!(phase_of("my-custom-app"), BuildPhase::Customization);
    }

    /// a depends on b and b depends on a -- must return CyclicDependency.
    #[test]
    fn test_cycle_detection() {
        let mut recipes = HashMap::new();
        recipes.insert("a".to_string(), make_recipe("a", &["b"], &[]));
        recipes.insert("b".to_string(), make_recipe("b", &["a"], &[]));

        let result = compute_build_order(&recipes, &HashSet::new());
        assert_eq!(result, Err(BuildOrderError::CyclicDependency));
    }
}
