// conary-core/src/derivation/build_order.rs

//! Build ordering and stage classification for bootstrap packages.
//!
//! Produces a single, deterministically ordered [`Vec<BuildStep>`] across ALL
//! packages. [`Stage`] labels are informational (for progress reporting) and
//! do not act as build boundaries.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;

use crate::recipe::Recipe;

// ---------------------------------------------------------------------------
// Stage enum
// ---------------------------------------------------------------------------

/// Bootstrap stage classification.
///
/// Stages are ordered from earliest (Toolchain) to latest (Customization),
/// reflecting the dependency chain of a from-scratch OS bootstrap. These are
/// informational labels for progress reporting -- the chroot pipeline builds
/// all packages in a single flat sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stage {
    /// Core toolchain: compiler, linker, libc, kernel headers.
    Toolchain,
    /// Essential build tools: make, bash, coreutils, etc.
    Foundation,
    /// All remaining system packages.
    System,
    /// User-specified custom packages.
    Customization,
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Toolchain => write!(f, "toolchain"),
            Self::Foundation => write!(f, "foundation"),
            Self::System => write!(f, "system"),
            Self::Customization => write!(f, "customization"),
        }
    }
}

impl Stage {
    /// Parse a stage name from a string (case-insensitive).
    pub fn from_str_name(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "toolchain" => Some(Self::Toolchain),
            "foundation" => Some(Self::Foundation),
            "system" => Some(Self::System),
            "customization" => Some(Self::Customization),
            _ => None,
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
    /// Informational stage label.
    pub stage: Stage,
}

/// Errors produced by [`compute_build_order`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BuildOrderError {
    /// The dependency graph contains a cycle; a topological ordering is impossible.
    #[error("cyclic dependency detected in build graph")]
    CyclicDependency,
}

// ---------------------------------------------------------------------------
// Stage classification tables
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

fn classify_stage(package: &str, custom_packages: &HashSet<String>) -> Stage {
    if custom_packages.contains(package) {
        return Stage::Customization;
    }
    if TOOLCHAIN_NAMED.contains(&package) {
        return Stage::Toolchain;
    }
    if FOUNDATION_NAMED.contains(&package) {
        return Stage::Foundation;
    }
    Stage::System
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compute a flat, topologically sorted build plan for all `recipes`.
///
/// Every recipe in `recipes` is included exactly once in the output. Packages
/// listed in `custom_packages` receive the [`Stage::Customization`] label.
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
            let stage = classify_stage(&package, custom_packages);
            BuildStep {
                package,
                order,
                stage,
            }
        })
        .collect();

    Ok(steps)
}

// ---------------------------------------------------------------------------
// Topological sort (delegates to shared graph module)
// ---------------------------------------------------------------------------

/// Topologically sort `packages`, delegating to the shared Kahn's algorithm
/// in [`super::graph`].
///
/// # Errors
///
/// Returns [`BuildOrderError::CyclicDependency`] if the graph contains a cycle.
fn topological_sort(
    packages: &BTreeSet<String>,
    recipes: &HashMap<String, Recipe>,
) -> Result<Vec<String>, BuildOrderError> {
    super::graph::topological_sort(packages, recipes)
        .map_err(|()| BuildOrderError::CyclicDependency)
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

        let steps = compute_build_order(&recipes, &HashSet::new()).expect("no cycle");

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

    /// Stage classification: glibc=Toolchain, bash=Foundation, openssl=System,
    /// a custom package=Customization.
    #[test]
    fn test_stage_classification() {
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

        let stage_of = |name: &str| {
            steps
                .iter()
                .find(|s| s.package == name)
                .unwrap_or_else(|| panic!("{name} not found"))
                .stage
        };

        assert_eq!(stage_of("glibc"), Stage::Toolchain);
        assert_eq!(stage_of("bash"), Stage::Foundation);
        assert_eq!(stage_of("openssl"), Stage::System);
        assert_eq!(stage_of("my-custom-app"), Stage::Customization);
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
