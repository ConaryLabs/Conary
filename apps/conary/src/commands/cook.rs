// src/commands/cook.rs

//! Cook command - build packages from recipes

use anyhow::{Context, Result};
use conary_core::recipe::{Kitchen, KitchenConfig, parse_recipe_file, validate_recipe};
use std::path::{Path, PathBuf};
use tracing::info;

fn recipe_source_base_dir(recipe_path: &Path) -> PathBuf {
    recipe_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn resolve_recipe_path(target: Option<&str>, recipe: Option<&str>) -> Result<PathBuf> {
    if let Some(recipe) = recipe {
        return Ok(PathBuf::from(recipe));
    }

    let Some(target) = target else {
        let recipe_path = PathBuf::from("recipe.toml");
        if recipe_path.is_file() {
            return Ok(recipe_path);
        }

        anyhow::bail!(
            "No cook target provided; M1a requires ./recipe.toml in the current directory"
        );
    };

    let target_path = PathBuf::from(target);
    if target_path.is_dir() {
        let recipe_path = target_path.join("recipe.toml");
        if recipe_path.is_file() {
            return Ok(recipe_path);
        }

        anyhow::bail!(
            "Cook target directory {} does not contain recipe.toml; bare source inference is an M1b feature, and M1a requires a recipe path, --recipe, or a directory containing recipe.toml",
            target_path.display()
        );
    }

    if target_path.is_file() || target_path.extension().is_some() {
        return Ok(target_path);
    }

    anyhow::bail!(
        "Bare source inference for '{}' is an M1b feature; M1a requires a recipe path, --recipe, or a directory containing recipe.toml",
        target_path.display()
    );
}

/// Cook a package from a recipe
///
/// # Arguments
/// * `target` - Optional recipe path or directory containing recipe.toml
/// * `recipe` - Optional explicit recipe path. Wins over target when present.
/// * `output_dir` - Output directory for the built package
/// * `source_cache` - Directory for caching downloaded sources
/// * `jobs` - Number of parallel build jobs (None = auto)
/// * `keep_builddir` - Keep build directory after completion
/// * `validate_only` - Only validate the recipe, don't cook
/// * `fetch_only` - Only fetch sources, don't build
/// * `isolated` - Use the M1a sandboxed isolation path
/// * `no_isolation` - Hidden compatibility no-op for the M1a host default
/// * `hermetic` - Hidden compatibility flag rejected until M2
#[allow(clippy::too_many_arguments)]
pub async fn cmd_cook(
    target: Option<&str>,
    recipe: Option<&str>,
    output_dir: &str,
    source_cache: &str,
    jobs: Option<u32>,
    keep_builddir: bool,
    validate_only: bool,
    fetch_only: bool,
    isolated: bool,
    no_isolation: bool,
    hermetic: bool,
) -> Result<()> {
    if hermetic {
        anyhow::bail!(
            "Hermetic cook/publish is an M2 feature; M1a supports host or --isolated builds only"
        );
    }

    if isolated && no_isolation {
        anyhow::bail!("--isolated conflicts with --no-isolation");
    }

    let recipe_path = resolve_recipe_path(target, recipe)?;
    let output_dir = Path::new(output_dir);

    // Parse the recipe
    println!("Reading recipe: {}", recipe_path.display());
    let recipe = parse_recipe_file(&recipe_path)
        .with_context(|| format!("Failed to parse recipe: {}", recipe_path.display()))?;

    println!(
        "Recipe: {} version {}",
        recipe.package.name, recipe.package.version
    );

    // Validate the recipe
    let warnings = validate_recipe(&recipe).with_context(|| "Recipe validation failed")?;

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

    // Configure the kitchen. M1a defaults to host builds; --isolated opts into
    // the sandboxed path. --no-isolation is retained as a hidden no-op alias.
    let mut config = KitchenConfig {
        source_cache: PathBuf::from(source_cache),
        recipe_source_base_dir: Some(recipe_source_base_dir(&recipe_path)),
        keep_builddir,
        use_isolation: isolated,
        pristine_mode: false,
        ..Default::default()
    };

    if let Some(j) = jobs {
        config.jobs = j;
    }

    let kitchen = Kitchen::new(config.clone());

    // Fetch-only mode: just download sources and exit
    if fetch_only {
        println!("Fetching sources (fetch-only mode)...");
        let sources = kitchen
            .fetch(&recipe)
            .with_context(|| format!("Failed to fetch sources for {}", recipe.package.name))?;

        println!("\n[COMPLETE] Fetched {} source file(s):", sources.len());
        for source in &sources {
            println!("  - {}", source.display());
        }

        if kitchen.sources_cached(&recipe) {
            println!("\n[OK] All sources are cached. Ready for offline build.");
        }

        return Ok(());
    }

    // Create output directory if needed
    std::fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "Failed to create output directory: {}",
            output_dir.display()
        )
    })?;

    // Print mode information
    if isolated {
        println!("Cooking with {} parallel jobs (isolated)...", config.jobs);
        println!("  - Network isolated during build");
    } else {
        println!("Cooking with {} parallel jobs (host)...", config.jobs);
    }

    // Check if sources are cached
    if kitchen.sources_cached(&recipe) {
        println!("  - Sources already cached (offline build possible)");
    } else {
        println!("Fetching source...");
    }

    println!("Configuring...");
    println!("Building ({} parallel jobs)...", config.jobs);

    // Create kitchen and cook
    let result = kitchen
        .cook(&recipe, output_dir)
        .with_context(|| format!("Failed to cook {}", recipe.package.name))?;

    println!("Installing to staging...");

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

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::ccs::CcsPackage;
    use conary_core::packages::PackageFormat;

    fn write_local_recipe(recipe_path: &Path) {
        std::fs::write(
            recipe_path,
            r#"
[package]
name = "local"
version = "1.0"

[source]
path = "."

[build]
install = "true"
"#,
        )
        .unwrap();
    }

    fn write_installing_local_recipe(recipe_path: &Path) {
        std::fs::write(
            recipe_path,
            r#"
[package]
name = "local"
version = "1.0"

[source]
path = "."

[build]
install = "mkdir -p %(destdir)s/usr/share/local && printf cooked > %(destdir)s/usr/share/local/output.txt"
"#,
        )
        .unwrap();
    }

    fn cooked_manifest_provenance(
        output_dir: &Path,
    ) -> conary_core::ccs::manifest::ManifestProvenance {
        let package_path = output_dir.join("local-1.0-1.ccs");
        let package = CcsPackage::parse(&package_path.to_string_lossy()).unwrap();
        package.manifest().provenance.clone().unwrap()
    }

    #[test]
    fn test_recipe_source_base_dir_uses_recipe_parent() {
        assert_eq!(
            recipe_source_base_dir(Path::new("/work/recipes/pkg/recipe.toml")),
            PathBuf::from("/work/recipes/pkg")
        );
    }

    #[test]
    fn resolve_recipe_path_prefers_recipe_flag_over_target() {
        assert_eq!(
            resolve_recipe_path(Some("ignored-target"), Some("explicit.toml")).unwrap(),
            PathBuf::from("explicit.toml")
        );
    }

    #[test]
    fn resolve_recipe_path_accepts_directory_with_recipe_toml() {
        let temp = tempfile::tempdir().unwrap();
        let recipe_path = temp.path().join("recipe.toml");
        write_local_recipe(&recipe_path);

        assert_eq!(
            resolve_recipe_path(Some(temp.path().to_str().unwrap()), None).unwrap(),
            recipe_path
        );
    }

    #[test]
    fn resolve_recipe_path_rejects_bare_source_inference_for_m1a() {
        let temp = tempfile::tempdir().unwrap();
        let bare_target = temp.path().join("source-tree");

        let error = resolve_recipe_path(Some(bare_target.to_str().unwrap()), None).unwrap_err();

        assert!(
            error.to_string().contains("M1b"),
            "bare source inference error should name M1b: {error:#}"
        );
    }

    #[test]
    fn resolve_recipe_path_rejects_existing_bare_source_directory_for_m1a() {
        let temp = tempfile::tempdir().unwrap();
        let source_tree = temp.path().join("source-tree");
        std::fs::create_dir(&source_tree).unwrap();

        let error = resolve_recipe_path(Some(source_tree.to_str().unwrap()), None).unwrap_err();

        assert!(
            error.to_string().contains("M1b"),
            "existing directory bare-source error should name M1b: {error:#}"
        );
    }

    #[tokio::test]
    async fn cook_hermetic_fails_before_build_execution_without_writing_package() {
        let temp = tempfile::tempdir().unwrap();
        let recipe_path = temp.path().join("recipe.toml");
        let output_dir = temp.path().join("out");
        let source_cache = temp.path().join("sources");
        write_local_recipe(&recipe_path);

        let error = cmd_cook(
            Some(recipe_path.to_str().unwrap()),
            None,
            output_dir.to_str().unwrap(),
            source_cache.to_str().unwrap(),
            None,
            false,
            false,
            false,
            false,
            false,
            true,
        )
        .await
        .unwrap_err();

        assert!(
            error.to_string().contains("M2"),
            "hermetic error should name M2: {error:#}"
        );
        assert!(
            !output_dir.exists(),
            "hermetic rejection should happen before output/package creation"
        );
    }

    #[tokio::test]
    async fn cook_no_isolation_is_hidden_host_default_compatibility_noop_with_provenance() {
        let temp = tempfile::tempdir().unwrap();
        let default_root = temp.path().join("default");
        let compat_root = temp.path().join("compat");
        std::fs::create_dir_all(&default_root).unwrap();
        std::fs::create_dir_all(&compat_root).unwrap();
        let default_recipe = default_root.join("recipe.toml");
        let compat_recipe = compat_root.join("recipe.toml");
        let default_output = temp.path().join("default-out");
        let compat_output = temp.path().join("compat-out");
        let source_cache = temp.path().join("sources");
        write_installing_local_recipe(&default_recipe);
        write_installing_local_recipe(&compat_recipe);

        cmd_cook(
            Some(default_recipe.to_str().unwrap()),
            None,
            default_output.to_str().unwrap(),
            source_cache.to_str().unwrap(),
            None,
            false,
            false,
            false,
            false,
            false,
            false,
        )
        .await
        .unwrap();
        cmd_cook(
            Some(compat_recipe.to_str().unwrap()),
            None,
            compat_output.to_str().unwrap(),
            source_cache.to_str().unwrap(),
            None,
            false,
            false,
            false,
            false,
            true,
            false,
        )
        .await
        .unwrap();

        for provenance in [
            cooked_manifest_provenance(&default_output),
            cooked_manifest_provenance(&compat_output),
        ] {
            assert_eq!(provenance.origin_class.as_deref(), Some("native-built"));
            assert_eq!(provenance.hardening_level.as_deref(), Some("host"));
        }
    }

    #[tokio::test]
    async fn cook_no_isolation_conflicts_with_isolated() {
        let temp = tempfile::tempdir().unwrap();
        let recipe_path = temp.path().join("recipe.toml");
        let output_dir = temp.path().join("out");
        let source_cache = temp.path().join("sources");
        write_local_recipe(&recipe_path);

        let error = cmd_cook(
            Some(recipe_path.to_str().unwrap()),
            None,
            output_dir.to_str().unwrap(),
            source_cache.to_str().unwrap(),
            None,
            false,
            false,
            false,
            true,
            true,
            false,
        )
        .await
        .unwrap_err();

        assert!(
            error.to_string().contains("conflict"),
            "--isolated and --no-isolation conflict should be explicit: {error:#}"
        );
    }
}
