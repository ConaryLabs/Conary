// src/commands/cook.rs

//! Cook command - build packages from recipes

use anyhow::{Context, Result};
use conary::recipe::{parse_recipe_file, validate_recipe, Kitchen, KitchenConfig};
use std::path::{Path, PathBuf};
use tracing::info;

/// Cook a package from a recipe
///
/// # Arguments
/// * `recipe_path` - Path to the recipe file
/// * `output_dir` - Output directory for the built package
/// * `source_cache` - Directory for caching downloaded sources
/// * `jobs` - Number of parallel build jobs (None = auto)
/// * `keep_builddir` - Keep build directory after completion
/// * `validate_only` - Only validate the recipe, don't cook
/// * `isolate` - Run build in container isolation
pub fn cmd_cook(
    recipe_path: &str,
    output_dir: &str,
    source_cache: &str,
    jobs: Option<u32>,
    keep_builddir: bool,
    validate_only: bool,
    isolate: bool,
) -> Result<()> {
    let recipe_path = Path::new(recipe_path);
    let output_dir = Path::new(output_dir);

    // Parse the recipe
    println!("Reading recipe: {}", recipe_path.display());
    let recipe = parse_recipe_file(recipe_path)
        .with_context(|| format!("Failed to parse recipe: {}", recipe_path.display()))?;

    println!("Recipe: {} version {}", recipe.package.name, recipe.package.version);

    // Validate the recipe
    let warnings = validate_recipe(&recipe)
        .with_context(|| "Recipe validation failed")?;

    for warning in &warnings {
        println!("Warning: {}", warning);
    }

    if validate_only {
        println!("Recipe validation passed");
        if warnings.is_empty() {
            println!("[OK] No issues found");
        } else {
            println!("[OK] {} warning(s)", warnings.len());
        }
        return Ok(());
    }

    // Create output directory if needed
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("Failed to create output directory: {}", output_dir.display()))?;

    // Configure the kitchen
    let mut config = KitchenConfig {
        source_cache: PathBuf::from(source_cache),
        keep_builddir,
        use_isolation: isolate,
        ..Default::default()
    };

    if let Some(j) = jobs {
        config.jobs = j;
    }

    if isolate {
        println!("Cooking with {} parallel jobs (isolated)...", config.jobs);
    } else {
        println!("Cooking with {} parallel jobs...", config.jobs);
    }

    // Create kitchen and cook
    let kitchen = Kitchen::new(config);
    let result = kitchen.cook(&recipe, output_dir)
        .with_context(|| format!("Failed to cook {}", recipe.package.name))?;

    println!("\n[COMPLETE] Cooked: {}", result.package_path.display());

    if !result.warnings.is_empty() {
        println!("\nBuild warnings:");
        for warning in &result.warnings {
            println!("  - {}", warning);
        }
    }

    info!(
        "Successfully cooked {} to {}",
        recipe.package.name,
        result.package_path.display()
    );

    Ok(())
}
