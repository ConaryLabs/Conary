// conary-core/src/derivation/stages.rs

//! Stage assignment algorithm for bootstrap build ordering.
//!
//! Analyzes recipe dependency graphs and assigns each package to one of four
//! bootstrap stages: Toolchain, Foundation, System, or Customization. Within
//! each stage, packages are topologically sorted by their build dependencies
//! to produce a deterministic build order.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;

use crate::recipe::Recipe;

/// Bootstrap stage classification.
///
/// Stages are ordered from earliest (Toolchain) to latest (Customization),
/// reflecting the dependency chain of a from-scratch OS bootstrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stage {
    /// Cross-compiled bootstrap toolchain (pass1/pass2 packages, linux-headers, glibc, libstdcxx).
    Toolchain,
    /// Full rebuilds of toolchain packages plus core build tools.
    Foundation,
    /// All remaining packages not designated as custom.
    System,
    /// User-specified custom packages layered on top.
    Customization,
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Stage::Toolchain => write!(f, "toolchain"),
            Stage::Foundation => write!(f, "foundation"),
            Stage::System => write!(f, "system"),
            Stage::Customization => write!(f, "customization"),
        }
    }
}

impl Stage {
    /// Parse a stage name from a string (case-insensitive).
    pub fn from_str_name(s: &str) -> Result<Self, StageError> {
        match s.to_lowercase().as_str() {
            "toolchain" => Ok(Stage::Toolchain),
            "foundation" => Ok(Stage::Foundation),
            "system" => Ok(Stage::System),
            "customization" => Ok(Stage::Customization),
            other => Err(StageError::InvalidStage(other.to_string())),
        }
    }
}

/// A package's assigned stage and build order position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageAssignment {
    /// Package name.
    pub package: String,
    /// Assigned bootstrap stage.
    pub stage: Stage,
    /// Global build order index (0-based, across all stages).
    pub build_order: usize,
}

/// Errors from stage assignment.
#[derive(Debug, thiserror::Error)]
pub enum StageError {
    /// An invalid stage name was encountered.
    #[error("invalid stage: {0}")]
    InvalidStage(String),

    /// A cyclic dependency was detected during topological sort.
    #[error("cyclic dependency detected in build graph")]
    CyclicDependency,
}

/// Core foundation packages that belong in the Foundation stage.
///
/// These are the essential build tools required to compile the rest of the
/// system once the cross-compiled toolchain is available.
///
/// Note: `glibc` is intentionally absent here -- it appears in
/// `TOOLCHAIN_NAMED` and the toolchain check runs before the foundation
/// check in `assign_stages`, so it is always classified as toolchain.
const FOUNDATION_PACKAGES: &[&str] = &[
    // Core unix tools -- enough to build everything else.
    // GCC and its deps (gmp, mpfr, mpc) are intentionally NOT here:
    // within a stage, packages can't see each other's outputs (only the
    // previous stage's EROFS). GCC needs gmp/mpfr/mpc headers, so they
    // must be in an earlier stage. Foundation provides the basic tools;
    // System rebuilds the compiler on top of the Foundation EROFS.
    "binutils",
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

/// Toolchain packages identified by name (not by pass suffix).
const TOOLCHAIN_NAMED: &[&str] = &["linux-headers", "glibc", "libstdcxx"];

/// Assign bootstrap stages to all recipes and produce a globally ordered build plan.
///
/// # Algorithm
///
/// 1. **Toolchain**: recipes ending in `-pass1` or `-pass2`, plus `linux-headers`,
///    `glibc`, and `libstdcxx` (cross-compiled bootstrap packages).
/// 2. **Foundation**: full versions of pass recipes (e.g., `gcc` if `gcc-pass1` exists),
///    plus core build tools from [`FOUNDATION_PACKAGES`].
/// 3. **System**: everything else not in `custom_packages`.
/// 4. **Customization**: packages listed in `custom_packages`.
/// 5. Within each stage, packages are topologically sorted by `makedepends`/`requires`.
///
/// Manual stage hints (via `build.stage` in a recipe) override automatic classification.
///
/// # Errors
///
/// Returns [`StageError::CyclicDependency`] if a cycle is found within any stage.
/// Returns [`StageError::InvalidStage`] if a manual stage hint is unrecognized.
pub fn assign_stages(
    recipes: &HashMap<String, Recipe>,
    custom_packages: &HashSet<String>,
) -> Result<Vec<StageAssignment>, StageError> {
    let mut toolchain = BTreeSet::new();
    let mut foundation = BTreeSet::new();
    let mut system = BTreeSet::new();
    let mut customization = BTreeSet::new();

    // Collect base names from pass recipes (e.g., "gcc" from "gcc-pass1")
    let mut pass_base_names: HashSet<String> = HashSet::new();
    for name in recipes.keys() {
        if let Some(base) = strip_pass_suffix(name) {
            pass_base_names.insert(base.to_string());
        }
    }

    for (name, recipe) in recipes {
        // Check for manual stage hint first
        if let Some(ref stage_hint) = recipe.build.stage {
            let stage = Stage::from_str_name(stage_hint)?;
            match stage {
                Stage::Toolchain => {
                    toolchain.insert(name.clone());
                }
                Stage::Foundation => {
                    foundation.insert(name.clone());
                }
                Stage::System => {
                    system.insert(name.clone());
                }
                Stage::Customization => {
                    customization.insert(name.clone());
                }
            }
            continue;
        }

        // Automatic classification
        if custom_packages.contains(name) {
            customization.insert(name.clone());
        } else if is_toolchain_package(name) {
            toolchain.insert(name.clone());
        } else if is_foundation_package(name, &pass_base_names) {
            foundation.insert(name.clone());
        } else {
            system.insert(name.clone());
        }
    }

    // Topological sort within each stage, then concatenate
    let mut assignments = Vec::new();
    let mut order = 0;

    for (stage, packages) in [
        (Stage::Toolchain, &toolchain),
        (Stage::Foundation, &foundation),
        (Stage::System, &system),
        (Stage::Customization, &customization),
    ] {
        let sorted = topological_sort(packages, recipes)?;
        for pkg in sorted {
            assignments.push(StageAssignment {
                package: pkg,
                stage,
                build_order: order,
            });
            order += 1;
        }
    }

    Ok(assignments)
}

/// Check if a package name indicates a toolchain pass package or named toolchain package.
fn is_toolchain_package(name: &str) -> bool {
    strip_pass_suffix(name).is_some() || TOOLCHAIN_NAMED.contains(&name)
}

/// Check if a package belongs in the Foundation stage.
fn is_foundation_package(name: &str, pass_base_names: &HashSet<String>) -> bool {
    FOUNDATION_PACKAGES.contains(&name) || pass_base_names.contains(name)
}

/// Strip `-pass1` or `-pass2` suffix, returning the base name if present.
fn strip_pass_suffix(name: &str) -> Option<&str> {
    name.strip_suffix("-pass1")
        .or_else(|| name.strip_suffix("-pass2"))
}

/// Topologically sort packages within a stage using Kahn's algorithm.
///
/// Only considers dependency edges where both endpoints are in the `packages` set
/// (cross-stage dependencies are ignored for ordering purposes). Uses `BTreeMap`
/// and `BTreeSet` throughout for deterministic output.
///
/// # Errors
///
/// Returns [`StageError::CyclicDependency`] if the dependency graph contains a cycle.
fn topological_sort(
    packages: &BTreeSet<String>,
    recipes: &HashMap<String, Recipe>,
) -> Result<Vec<String>, StageError> {
    if packages.is_empty() {
        return Ok(Vec::new());
    }

    // Build adjacency list and in-degree map (only for edges within this stage)
    let mut adjacency: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut in_degree: BTreeMap<&str, usize> = BTreeMap::new();

    // Initialize all packages with zero in-degree
    for pkg in packages {
        adjacency.entry(pkg.as_str()).or_default();
        in_degree.entry(pkg.as_str()).or_insert(0);
    }

    // Add edges: if package B depends on package A, then A -> B (A must be built first)
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

    // Kahn's algorithm with BTreeSet-based queue for deterministic ordering
    let mut queue: VecDeque<&str> = VecDeque::new();
    // Collect zero-degree nodes in sorted order
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

        // Collect newly freed nodes in sorted order for determinism
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
        return Err(StageError::CyclicDependency);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derivation::test_helpers::helpers::make_recipe;

    /// Create a recipe with a manual stage hint.
    fn make_recipe_with_stage(
        name: &str,
        requires: &[&str],
        makedepends: &[&str],
        stage: &str,
    ) -> Recipe {
        let mut recipe = make_recipe(name, requires, makedepends);
        recipe.build.stage = Some(stage.to_string());
        recipe
    }

    #[test]
    fn test_stage_display() {
        assert_eq!(Stage::Toolchain.to_string(), "toolchain");
        assert_eq!(Stage::Foundation.to_string(), "foundation");
        assert_eq!(Stage::System.to_string(), "system");
        assert_eq!(Stage::Customization.to_string(), "customization");
    }

    #[test]
    fn test_stage_ordering() {
        assert!(Stage::Toolchain < Stage::Foundation);
        assert!(Stage::Foundation < Stage::System);
        assert!(Stage::System < Stage::Customization);
    }

    #[test]
    fn test_stage_from_str_name() {
        assert_eq!(Stage::from_str_name("toolchain").unwrap(), Stage::Toolchain);
        assert_eq!(
            Stage::from_str_name("FOUNDATION").unwrap(),
            Stage::Foundation
        );
        assert_eq!(Stage::from_str_name("System").unwrap(), Stage::System);
        assert_eq!(
            Stage::from_str_name("Customization").unwrap(),
            Stage::Customization
        );
        assert!(Stage::from_str_name("invalid").is_err());
    }

    #[test]
    fn test_pass_recipes_go_to_toolchain() {
        let mut recipes = HashMap::new();
        recipes.insert("gcc-pass1".to_string(), make_recipe("gcc-pass1", &[], &[]));
        recipes.insert(
            "gcc-pass2".to_string(),
            make_recipe("gcc-pass2", &["gcc-pass1"], &[]),
        );
        recipes.insert(
            "binutils-pass1".to_string(),
            make_recipe("binutils-pass1", &[], &[]),
        );

        let custom = HashSet::new();
        let assignments = assign_stages(&recipes, &custom).unwrap();

        for a in &assignments {
            assert_eq!(
                a.stage,
                Stage::Toolchain,
                "expected {} in Toolchain",
                a.package
            );
        }
    }

    #[test]
    fn test_named_toolchain_packages() {
        let mut recipes = HashMap::new();
        recipes.insert(
            "linux-headers".to_string(),
            make_recipe("linux-headers", &[], &[]),
        );
        recipes.insert(
            "glibc".to_string(),
            make_recipe("glibc", &["linux-headers"], &[]),
        );
        recipes.insert("libstdcxx".to_string(), make_recipe("libstdcxx", &[], &[]));

        let custom = HashSet::new();
        let assignments = assign_stages(&recipes, &custom).unwrap();

        for a in &assignments {
            assert_eq!(
                a.stage,
                Stage::Toolchain,
                "expected {} in Toolchain",
                a.package
            );
        }
    }

    #[test]
    fn test_base_name_of_pass_recipe_goes_to_foundation() {
        let mut recipes = HashMap::new();
        recipes.insert("gcc-pass1".to_string(), make_recipe("gcc-pass1", &[], &[]));
        recipes.insert("gcc".to_string(), make_recipe("gcc", &[], &[]));

        let custom = HashSet::new();
        let assignments = assign_stages(&recipes, &custom).unwrap();

        let gcc_pass = assignments
            .iter()
            .find(|a| a.package == "gcc-pass1")
            .unwrap();
        assert_eq!(gcc_pass.stage, Stage::Toolchain);

        let gcc_full = assignments.iter().find(|a| a.package == "gcc").unwrap();
        assert_eq!(gcc_full.stage, Stage::Foundation);
    }

    #[test]
    fn test_foundation_packages() {
        let mut recipes = HashMap::new();
        for name in &["make", "bash", "coreutils", "sed", "gawk", "grep"] {
            recipes.insert(name.to_string(), make_recipe(name, &[], &[]));
        }

        let custom = HashSet::new();
        let assignments = assign_stages(&recipes, &custom).unwrap();

        for a in &assignments {
            assert_eq!(
                a.stage,
                Stage::Foundation,
                "expected {} in Foundation",
                a.package
            );
        }
    }

    #[test]
    fn test_custom_packages_go_to_customization() {
        let mut recipes = HashMap::new();
        recipes.insert("my-app".to_string(), make_recipe("my-app", &[], &[]));
        recipes.insert("my-tool".to_string(), make_recipe("my-tool", &[], &[]));
        recipes.insert("nginx".to_string(), make_recipe("nginx", &[], &[]));

        let mut custom = HashSet::new();
        custom.insert("my-app".to_string());
        custom.insert("my-tool".to_string());

        let assignments = assign_stages(&recipes, &custom).unwrap();

        let my_app = assignments.iter().find(|a| a.package == "my-app").unwrap();
        assert_eq!(my_app.stage, Stage::Customization);

        let my_tool = assignments.iter().find(|a| a.package == "my-tool").unwrap();
        assert_eq!(my_tool.stage, Stage::Customization);

        let nginx = assignments.iter().find(|a| a.package == "nginx").unwrap();
        assert_eq!(nginx.stage, Stage::System);
    }

    #[test]
    fn test_uncategorized_goes_to_system() {
        let mut recipes = HashMap::new();
        recipes.insert("nginx".to_string(), make_recipe("nginx", &[], &[]));
        recipes.insert("openssh".to_string(), make_recipe("openssh", &[], &[]));

        let custom = HashSet::new();
        let assignments = assign_stages(&recipes, &custom).unwrap();

        for a in &assignments {
            assert_eq!(a.stage, Stage::System, "expected {} in System", a.package);
        }
    }

    #[test]
    fn test_topological_sort_respects_dependencies() {
        let mut recipes = HashMap::new();
        recipes.insert("a".to_string(), make_recipe("a", &[], &[]));
        recipes.insert("b".to_string(), make_recipe("b", &["a"], &[]));
        recipes.insert("c".to_string(), make_recipe("c", &["b"], &[]));

        let packages: BTreeSet<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

        let sorted = topological_sort(&packages, &recipes).unwrap();
        assert_eq!(sorted, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_topological_sort_makedepends() {
        let mut recipes = HashMap::new();
        recipes.insert("libc".to_string(), make_recipe("libc", &[], &[]));
        recipes.insert("gcc".to_string(), make_recipe("gcc", &[], &["libc"]));
        recipes.insert("make".to_string(), make_recipe("make", &[], &["gcc"]));

        let packages: BTreeSet<String> = ["libc", "gcc", "make"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let sorted = topological_sort(&packages, &recipes).unwrap();

        let pos_libc = sorted.iter().position(|s| s == "libc").unwrap();
        let pos_gcc = sorted.iter().position(|s| s == "gcc").unwrap();
        let pos_make = sorted.iter().position(|s| s == "make").unwrap();

        assert!(pos_libc < pos_gcc, "libc must come before gcc");
        assert!(pos_gcc < pos_make, "gcc must come before make");
    }

    #[test]
    fn test_topological_sort_cycle_detection() {
        let mut recipes = HashMap::new();
        recipes.insert("a".to_string(), make_recipe("a", &["c"], &[]));
        recipes.insert("b".to_string(), make_recipe("b", &["a"], &[]));
        recipes.insert("c".to_string(), make_recipe("c", &["b"], &[]));

        let packages: BTreeSet<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

        let result = topological_sort(&packages, &recipes);
        assert!(matches!(result, Err(StageError::CyclicDependency)));
    }

    #[test]
    fn test_topological_sort_empty() {
        let recipes = HashMap::new();
        let packages = BTreeSet::new();

        let sorted = topological_sort(&packages, &recipes).unwrap();
        assert!(sorted.is_empty());
    }

    #[test]
    fn test_topological_sort_independent_packages_alphabetical() {
        let mut recipes = HashMap::new();
        recipes.insert("zlib".to_string(), make_recipe("zlib", &[], &[]));
        recipes.insert("curl".to_string(), make_recipe("curl", &[], &[]));
        recipes.insert("bzip2".to_string(), make_recipe("bzip2", &[], &[]));

        let packages: BTreeSet<String> = ["zlib", "curl", "bzip2"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let sorted = topological_sort(&packages, &recipes).unwrap();
        // Independent packages should come out in alphabetical order (BTreeSet/BTreeMap)
        assert_eq!(sorted, vec!["bzip2", "curl", "zlib"]);
    }

    #[test]
    fn test_cross_stage_deps_ignored_in_sort() {
        // "b" depends on "a", but "a" is not in the stage set -- should not block "b"
        let mut recipes = HashMap::new();
        recipes.insert("a".to_string(), make_recipe("a", &[], &[]));
        recipes.insert("b".to_string(), make_recipe("b", &["a"], &[]));

        let packages: BTreeSet<String> = ["b"].iter().map(|s| s.to_string()).collect();

        let sorted = topological_sort(&packages, &recipes).unwrap();
        assert_eq!(sorted, vec!["b"]);
    }

    #[test]
    fn test_build_order_is_global() {
        let mut recipes = HashMap::new();
        // Toolchain
        recipes.insert("gcc-pass1".to_string(), make_recipe("gcc-pass1", &[], &[]));
        // Foundation
        recipes.insert("make".to_string(), make_recipe("make", &[], &[]));
        // System
        recipes.insert("nginx".to_string(), make_recipe("nginx", &[], &[]));
        // Customization
        recipes.insert("my-app".to_string(), make_recipe("my-app", &[], &[]));

        let mut custom = HashSet::new();
        custom.insert("my-app".to_string());

        let assignments = assign_stages(&recipes, &custom).unwrap();

        // Verify ordering: toolchain < foundation < system < customization
        let order_of = |name: &str| -> usize {
            assignments
                .iter()
                .find(|a| a.package == name)
                .unwrap()
                .build_order
        };

        assert!(order_of("gcc-pass1") < order_of("make"));
        assert!(order_of("make") < order_of("nginx"));
        assert!(order_of("nginx") < order_of("my-app"));
    }

    #[test]
    fn test_manual_stage_hint_overrides() {
        let mut recipes = HashMap::new();
        // "nginx" would normally go to System, but manual hint puts it in Foundation
        recipes.insert(
            "nginx".to_string(),
            make_recipe_with_stage("nginx", &[], &[], "foundation"),
        );
        // "gcc-pass1" would normally go to Toolchain, but manual hint puts it in System
        recipes.insert(
            "gcc-pass1".to_string(),
            make_recipe_with_stage("gcc-pass1", &[], &[], "system"),
        );

        let custom = HashSet::new();
        let assignments = assign_stages(&recipes, &custom).unwrap();

        let nginx = assignments.iter().find(|a| a.package == "nginx").unwrap();
        assert_eq!(nginx.stage, Stage::Foundation);

        let gcc = assignments
            .iter()
            .find(|a| a.package == "gcc-pass1")
            .unwrap();
        assert_eq!(gcc.stage, Stage::System);
    }

    #[test]
    fn test_invalid_stage_hint_errors() {
        let mut recipes = HashMap::new();
        recipes.insert(
            "bad".to_string(),
            make_recipe_with_stage("bad", &[], &[], "nonsense"),
        );

        let custom = HashSet::new();
        let result = assign_stages(&recipes, &custom);
        assert!(matches!(result, Err(StageError::InvalidStage(_))));
    }

    #[test]
    fn test_glibc_in_toolchain_when_named() {
        // glibc is in TOOLCHAIN_NAMED, so it goes to Toolchain even without a pass suffix.
        // But if there's also a "glibc" in FOUNDATION_PACKAGES, the toolchain check wins
        // because toolchain is checked first.
        let mut recipes = HashMap::new();
        recipes.insert("glibc".to_string(), make_recipe("glibc", &[], &[]));

        let custom = HashSet::new();
        let assignments = assign_stages(&recipes, &custom).unwrap();

        let glibc = assignments.iter().find(|a| a.package == "glibc").unwrap();
        assert_eq!(glibc.stage, Stage::Toolchain);
    }

    #[test]
    fn test_full_bootstrap_scenario() {
        let mut recipes = HashMap::new();

        // Toolchain pass packages
        recipes.insert(
            "binutils-pass1".to_string(),
            make_recipe("binutils-pass1", &[], &[]),
        );
        recipes.insert(
            "gcc-pass1".to_string(),
            make_recipe("gcc-pass1", &["binutils-pass1"], &[]),
        );
        recipes.insert(
            "linux-headers".to_string(),
            make_recipe("linux-headers", &[], &[]),
        );
        recipes.insert(
            "glibc".to_string(),
            make_recipe("glibc", &["linux-headers", "gcc-pass1"], &[]),
        );
        recipes.insert(
            "libstdcxx".to_string(),
            make_recipe("libstdcxx", &["glibc"], &[]),
        );
        recipes.insert(
            "gcc-pass2".to_string(),
            make_recipe("gcc-pass2", &["libstdcxx"], &[]),
        );

        // Foundation: full rebuilds
        recipes.insert("gcc".to_string(), make_recipe("gcc", &[], &[]));
        recipes.insert("binutils".to_string(), make_recipe("binutils", &[], &[]));
        recipes.insert("make".to_string(), make_recipe("make", &[], &[]));
        recipes.insert("bash".to_string(), make_recipe("bash", &[], &["make"]));

        // System
        recipes.insert("nginx".to_string(), make_recipe("nginx", &[], &[]));
        recipes.insert("openssh".to_string(), make_recipe("openssh", &[], &[]));

        // Customization
        recipes.insert("my-app".to_string(), make_recipe("my-app", &[], &[]));

        let mut custom = HashSet::new();
        custom.insert("my-app".to_string());

        let assignments = assign_stages(&recipes, &custom).unwrap();

        // Verify stage assignments
        let stage_of = |name: &str| -> Stage {
            assignments
                .iter()
                .find(|a| a.package == name)
                .unwrap()
                .stage
        };

        assert_eq!(stage_of("binutils-pass1"), Stage::Toolchain);
        assert_eq!(stage_of("gcc-pass1"), Stage::Toolchain);
        assert_eq!(stage_of("linux-headers"), Stage::Toolchain);
        assert_eq!(stage_of("glibc"), Stage::Toolchain);
        assert_eq!(stage_of("libstdcxx"), Stage::Toolchain);
        assert_eq!(stage_of("gcc-pass2"), Stage::Toolchain);

        assert_eq!(stage_of("gcc"), Stage::Foundation);
        assert_eq!(stage_of("binutils"), Stage::Foundation);
        assert_eq!(stage_of("make"), Stage::Foundation);
        assert_eq!(stage_of("bash"), Stage::Foundation);

        assert_eq!(stage_of("nginx"), Stage::System);
        assert_eq!(stage_of("openssh"), Stage::System);

        assert_eq!(stage_of("my-app"), Stage::Customization);

        // Verify topological order within toolchain
        let order_of = |name: &str| -> usize {
            assignments
                .iter()
                .find(|a| a.package == name)
                .unwrap()
                .build_order
        };

        assert!(
            order_of("binutils-pass1") < order_of("gcc-pass1"),
            "binutils-pass1 before gcc-pass1"
        );
        assert!(
            order_of("gcc-pass1") < order_of("glibc"),
            "gcc-pass1 before glibc"
        );
        assert!(
            order_of("glibc") < order_of("libstdcxx"),
            "glibc before libstdcxx"
        );
        assert!(
            order_of("libstdcxx") < order_of("gcc-pass2"),
            "libstdcxx before gcc-pass2"
        );

        // Verify foundation ordering (make before bash since bash makedepends on make)
        assert!(order_of("make") < order_of("bash"), "make before bash");

        // All assignments should have unique build_order
        let orders: Vec<usize> = assignments.iter().map(|a| a.build_order).collect();
        let unique: HashSet<usize> = orders.iter().copied().collect();
        assert_eq!(orders.len(), unique.len(), "build orders must be unique");
    }
}
