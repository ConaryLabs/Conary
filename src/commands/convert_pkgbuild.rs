// src/commands/convert_pkgbuild.rs

//! Convert PKGBUILD to Recipe command

use anyhow::{Context, Result};
use conary::recipe::pkgbuild::{convert_pkgbuild, pkgbuild_to_toml};
use std::fs;
use std::io::Write;
use std::path::Path;

/// Convert an Arch Linux PKGBUILD to a Conary recipe
///
/// # Arguments
/// * `pkgbuild_path` - Path to the PKGBUILD file
/// * `output` - Optional output file path (None = stdout)
pub fn cmd_convert_pkgbuild(pkgbuild_path: &str, output: Option<&str>) -> Result<()> {
    let path = Path::new(pkgbuild_path);

    // Read the PKGBUILD
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read PKGBUILD: {}", path.display()))?;

    // Convert to recipe
    let result = convert_pkgbuild(&content)
        .with_context(|| "Failed to convert PKGBUILD")?;

    // Print warnings
    for warning in &result.warnings {
        eprintln!("Warning: {}", warning);
    }

    // Convert to TOML
    let toml = pkgbuild_to_toml(&content)
        .with_context(|| "Failed to serialize recipe to TOML")?;

    // Output
    match output {
        Some(output_path) => {
            let mut file = fs::File::create(output_path)
                .with_context(|| format!("Failed to create output file: {}", output_path))?;
            file.write_all(toml.as_bytes())
                .with_context(|| "Failed to write recipe")?;
            println!("Recipe written to: {}", output_path);
        }
        None => {
            println!("{}", toml);
        }
    }

    println!("\nConverted: {} version {}", result.recipe.package.name, result.recipe.package.version);
    if !result.warnings.is_empty() {
        println!("{} warning(s) - review recipe before use", result.warnings.len());
    }

    Ok(())
}
