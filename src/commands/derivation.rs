// src/commands/derivation.rs

//! Derivation engine command handlers

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use conary_core::derivation::id::{DerivationId, DerivationInputs};
use conary_core::derivation::recipe_hash::{build_script_hash, source_hash};
use conary_core::recipe::parse_recipe_file;

/// Build a recipe into CAS via the derivation engine.
///
/// Loads the recipe, computes the derivation ID, and prints it. The actual
/// CAS build pipeline is not yet wired up -- this prints a TODO message for
/// the build step while still exercising derivation ID computation.
pub async fn cmd_derivation_build(
    recipe: &Path,
    env: &Path,
    cas_dir: &Path,
    _db_path: Option<&Path>,
) -> Result<()> {
    let parsed = parse_recipe_file(recipe)
        .with_context(|| format!("Failed to parse recipe: {}", recipe.display()))?;

    println!(
        "Recipe: {} v{}",
        parsed.package.name, parsed.package.version
    );

    // Compute the environment hash from the image path.
    let env_hash = sha256_of_path(env)
        .with_context(|| format!("Failed to hash environment image: {}", env.display()))?;

    let inputs = DerivationInputs {
        source_hash: source_hash(&parsed),
        build_script_hash: build_script_hash(&parsed),
        dependency_ids: BTreeMap::new(),
        build_env_hash: env_hash,
        target_triple: current_target_triple(),
        build_options: BTreeMap::new(),
    };

    let drv_id = DerivationId::compute(&inputs).context("Derivation input validation failed")?;
    println!("Derivation ID: {drv_id}");
    println!("CAS directory: {}", cas_dir.display());
    println!();
    println!("[TODO] Full CAS build pipeline not yet connected.");
    println!("       Derivation ID has been computed successfully.");

    Ok(())
}

/// Show the derivation ID for a recipe without building.
///
/// Computes the content-addressed derivation ID from the recipe inputs and
/// the provided build environment hash, then prints it.
pub async fn cmd_derivation_show(recipe: &Path, env_hash: &str) -> Result<()> {
    let parsed = parse_recipe_file(recipe)
        .with_context(|| format!("Failed to parse recipe: {}", recipe.display()))?;

    println!(
        "Recipe: {} v{}",
        parsed.package.name, parsed.package.version
    );

    let inputs = DerivationInputs {
        source_hash: source_hash(&parsed),
        build_script_hash: build_script_hash(&parsed),
        dependency_ids: BTreeMap::new(),
        build_env_hash: env_hash.to_owned(),
        target_triple: current_target_triple(),
        build_options: BTreeMap::new(),
    };

    let drv_id = DerivationId::compute(&inputs).context("Derivation input validation failed")?;
    println!("Derivation ID: {drv_id}");

    Ok(())
}

/// SHA-256 hash of a file's contents, returned as a 64-char hex string.
fn sha256_of_path(path: &Path) -> Result<String> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
    conary_core::hash::sha256_reader_hex(&mut file)
        .with_context(|| format!("Failed to hash {}", path.display()))
}

/// Return the current platform's target triple.
fn current_target_triple() -> String {
    // Built-in cfg values give us the components; assemble the triple.
    format!("{}-unknown-linux-gnu", std::env::consts::ARCH)
}
