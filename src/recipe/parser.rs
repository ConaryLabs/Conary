// src/recipe/parser.rs

//! Recipe file parsing

use crate::error::{Error, Result};
use crate::recipe::format::Recipe;
use std::path::Path;

/// Parse a recipe from a TOML string
pub fn parse_recipe(content: &str) -> Result<Recipe> {
    toml::from_str(content).map_err(|e| Error::ParseError(format!("Invalid recipe: {}", e)))
}

/// Parse a recipe from a file
pub fn parse_recipe_file(path: &Path) -> Result<Recipe> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| Error::IoError(format!("Failed to read recipe file: {}", e)))?;

    parse_recipe(&content)
}

/// Validate a recipe for completeness and correctness
pub fn validate_recipe(recipe: &Recipe) -> Result<Vec<String>> {
    let mut warnings = Vec::new();

    // Check for empty name/version
    if recipe.package.name.is_empty() {
        return Err(Error::ParseError("Recipe package name cannot be empty".to_string()));
    }
    if recipe.package.version.is_empty() {
        return Err(Error::ParseError("Recipe package version cannot be empty".to_string()));
    }

    // Validate checksum format
    if !recipe.source.checksum.starts_with("sha256:")
        && !recipe.source.checksum.starts_with("sha512:")
        && !recipe.source.checksum.starts_with("blake3:")
    {
        return Err(Error::ParseError(format!(
            "Invalid checksum format: {}. Expected sha256:..., sha512:..., or blake3:...",
            recipe.source.checksum
        )));
    }

    // Warn about missing fields
    if recipe.package.summary.is_none() {
        warnings.push("Missing package summary".to_string());
    }
    if recipe.package.license.is_none() {
        warnings.push("Missing package license".to_string());
    }

    // Warn about missing install command
    if recipe.build.install.is_none() && recipe.build.script_file.is_none() {
        warnings.push("No install command or script_file specified".to_string());
    }

    // Validate patch checksums for remote patches
    if let Some(patches) = &recipe.patches {
        for patch in &patches.files {
            if patch.file.starts_with("http://") || patch.file.starts_with("https://") {
                if patch.checksum.is_none() {
                    warnings.push(format!("Remote patch {} has no checksum", patch.file));
                }
            }
        }
    }

    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_recipe() {
        let content = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test-1.0.tar.gz"
checksum = "sha256:abc123"

[build]
install = "make install DESTDIR=%(destdir)s"
"#;

        let recipe = parse_recipe(content).unwrap();
        assert_eq!(recipe.package.name, "test");
    }

    #[test]
    fn test_parse_invalid_recipe() {
        let content = "this is not valid toml at all {}";
        assert!(parse_recipe(content).is_err());
    }

    #[test]
    fn test_validate_empty_name() {
        let content = r#"
[package]
name = ""
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
"#;

        let recipe = parse_recipe(content).unwrap();
        assert!(validate_recipe(&recipe).is_err());
    }

    #[test]
    fn test_validate_bad_checksum() {
        let content = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "md5:abc123"

[build]
"#;

        let recipe = parse_recipe(content).unwrap();
        assert!(validate_recipe(&recipe).is_err());
    }

    #[test]
    fn test_validate_warnings() {
        let content = r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test.tar.gz"
checksum = "sha256:abc"

[build]
"#;

        let recipe = parse_recipe(content).unwrap();
        let warnings = validate_recipe(&recipe).unwrap();
        assert!(warnings.iter().any(|w| w.contains("summary")));
        assert!(warnings.iter().any(|w| w.contains("license")));
        assert!(warnings.iter().any(|w| w.contains("install")));
    }
}
