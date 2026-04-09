// src/commands/profile.rs

//! Build profile command handlers

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use conary_core::derivation::{
    BuildProfile, Pipeline, Seed, SystemManifest, compute_build_order, load_recipes,
};
use conary_core::recipe::Recipe;

struct ResolvedSeedRef {
    id: String,
    source: String,
}

pub(crate) fn canonical_manifest_path(manifest: &Path) -> Result<PathBuf> {
    manifest
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize manifest: {}", manifest.display()))
}

pub(crate) fn resolve_recipe_root_for_manifest(manifest_path: &Path) -> Result<PathBuf> {
    let manifest_dir = manifest_path.parent().ok_or_else(|| {
        anyhow::anyhow!("Manifest path has no parent: {}", manifest_path.display())
    })?;

    let recipes_dir = manifest_dir.join("recipes");
    if recipes_dir.exists() {
        Ok(recipes_dir)
    } else {
        Ok(manifest_dir.to_path_buf())
    }
}

fn resolve_profile_seed(manifest_path: &Path, seed_source: &str) -> Result<ResolvedSeedRef> {
    if let Some(hash) = seed_source.strip_prefix("cas:sha256:") {
        validate_seed_hash(hash)?;
        return Ok(ResolvedSeedRef {
            id: hash.to_string(),
            source: seed_source.to_string(),
        });
    }

    if is_seed_hash(seed_source) {
        return Ok(ResolvedSeedRef {
            id: seed_source.to_string(),
            source: seed_source.to_string(),
        });
    }

    let candidate = if Path::new(seed_source).is_absolute() {
        PathBuf::from(seed_source)
    } else {
        manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(seed_source)
    };
    let canonical_seed = candidate
        .canonicalize()
        .with_context(|| format!("Failed to resolve seed path: {}", candidate.display()))?;
    let seed = Seed::load_local(&canonical_seed)
        .map_err(|e| anyhow::anyhow!("Failed to load seed: {e}"))?;

    Ok(ResolvedSeedRef {
        id: seed.build_env_hash().to_string(),
        source: canonical_seed.display().to_string(),
    })
}

fn collect_needed_recipes(
    all_recipes: &HashMap<String, Recipe>,
    includes: &[String],
) -> Result<HashMap<String, Recipe>> {
    let mut needed = HashSet::new();
    let mut frontier: Vec<String> = includes.to_vec();

    while let Some(package) = frontier.pop() {
        if !needed.insert(package.clone()) {
            continue;
        }

        let recipe = all_recipes
            .get(&package)
            .ok_or_else(|| anyhow::anyhow!("recipe for '{package}' not found"))?;

        frontier.extend(recipe.build.requires.iter().cloned());
        frontier.extend(recipe.build.makedepends.iter().cloned());
    }

    Ok(all_recipes
        .iter()
        .filter(|(name, _)| needed.contains(*name))
        .map(|(name, recipe)| (name.clone(), recipe.clone()))
        .collect())
}

fn is_seed_hash(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn validate_seed_hash(value: &str) -> Result<()> {
    if is_seed_hash(value) {
        Ok(())
    } else {
        anyhow::bail!("Invalid seed hash: {value}")
    }
}

/// Generate a build profile from a system manifest.
///
/// Loads the manifest, resolves the seed reference, computes the transitive
/// recipe closure, orders it, computes concrete derivation IDs, and writes the
/// resulting build profile.
pub async fn cmd_profile_generate(manifest: &Path, output: Option<&Path>) -> Result<()> {
    if !manifest.exists() {
        anyhow::bail!("Manifest not found: {}", manifest.display());
    }

    let manifest_path = canonical_manifest_path(manifest)?;
    let system_manifest = SystemManifest::load(&manifest_path)
        .map_err(|e| anyhow::anyhow!("Failed to load system manifest: {e}"))?;
    let resolved_seed = resolve_profile_seed(&manifest_path, &system_manifest.seed.source)?;
    let recipe_root = resolve_recipe_root_for_manifest(&manifest_path)?;
    let all_recipes = load_recipes(&recipe_root)
        .with_context(|| format!("Failed to load recipes from {}", recipe_root.display()))?;
    let recipes = collect_needed_recipes(&all_recipes, &system_manifest.packages.include)?;
    let build_steps = compute_build_order(&recipes, &HashSet::new())
        .map_err(|e| anyhow::anyhow!("Build order computation failed: {e}"))?;

    let manifest_str = manifest_path.display().to_string();
    let profile = Pipeline::generate_profile(
        &resolved_seed.id,
        &resolved_seed.source,
        &system_manifest.system.target,
        &recipes,
        &build_steps,
        &manifest_str,
    )
    .map_err(|e| anyhow::anyhow!("Failed to compute derivation IDs: {e}"))?;

    let profile_toml = profile
        .to_toml()
        .context("Failed to serialize generated profile")?;

    println!("Manifest: {}", manifest_path.display());
    if let Some(out) = output {
        println!("Output:   {}", out.display());
        std::fs::write(out, &profile_toml)
            .with_context(|| format!("Failed to write profile: {}", out.display()))?;
    } else {
        println!("{profile_toml}");
    }

    Ok(())
}

/// Display a build profile from a TOML file.
///
/// Loads the profile, recomputes its hash for verification, and prints a
/// human-readable summary including seed, stages, and derivation counts.
pub async fn cmd_profile_show(path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read profile: {}", path.display()))?;

    let profile = BuildProfile::from_toml(&content)
        .with_context(|| format!("Failed to parse profile: {}", path.display()))?;

    let computed_hash = profile.compute_hash();

    println!("Profile: {}", profile.profile.manifest);
    println!("Target:  {}", profile.profile.target);
    println!("Hash:    {computed_hash}");
    if !profile.profile.generated_at.is_empty() {
        println!("Generated: {}", profile.profile.generated_at);
    }
    println!();

    println!(
        "Seed: {} (source: {})",
        profile.seed.id, profile.seed.source
    );
    println!();

    let total_derivations: usize = profile.stages.iter().map(|s| s.derivations.len()).sum();

    println!(
        "Stages: {}  Derivations: {}",
        profile.stages.len(),
        total_derivations
    );
    println!();

    for stage in &profile.stages {
        println!(
            "  [{}] env={} ({} derivations)",
            stage.name,
            stage.build_env,
            stage.derivations.len()
        );
        for drv in &stage.derivations {
            println!(
                "    {} v{} ({}...)",
                drv.package,
                drv.version,
                &drv.derivation_id[..12.min(drv.derivation_id.len())]
            );
        }
    }

    Ok(())
}

/// Compare two build profiles and display the diff.
///
/// Loads both profiles, computes their diff, and prints added, removed, and
/// changed packages.
pub async fn cmd_profile_diff(old_path: &Path, new_path: &Path) -> Result<()> {
    let old_content = std::fs::read_to_string(old_path)
        .with_context(|| format!("Failed to read old profile: {}", old_path.display()))?;
    let new_content = std::fs::read_to_string(new_path)
        .with_context(|| format!("Failed to read new profile: {}", new_path.display()))?;

    let old_profile = BuildProfile::from_toml(&old_content)
        .with_context(|| format!("Failed to parse old profile: {}", old_path.display()))?;
    let new_profile = BuildProfile::from_toml(&new_content)
        .with_context(|| format!("Failed to parse new profile: {}", new_path.display()))?;

    let old_hash = old_profile.compute_hash();
    let new_hash = new_profile.compute_hash();

    if old_hash == new_hash {
        println!("Profiles are identical (hash: {old_hash})");
        return Ok(());
    }

    println!("Old: {} (hash: {})", old_profile.profile.manifest, old_hash);
    println!("New: {} (hash: {})", new_profile.profile.manifest, new_hash);
    println!();

    let diff = old_profile.diff(&new_profile);

    if !diff.added.is_empty() {
        println!("Added ({}):", diff.added.len());
        for pkg in &diff.added {
            println!("  + {pkg}");
        }
    }

    if !diff.removed.is_empty() {
        println!("Removed ({}):", diff.removed.len());
        for pkg in &diff.removed {
            println!("  - {pkg}");
        }
    }

    if !diff.changed.is_empty() {
        println!("Changed ({}):", diff.changed.len());
        for pkg in &diff.changed {
            println!("  ~ {pkg}");
        }
    }

    if diff.added.is_empty() && diff.removed.is_empty() && diff.changed.is_empty() {
        println!("No package changes (metadata-only difference).");
    }

    Ok(())
}

/// Publish a profile to a remote Remi endpoint.
pub async fn cmd_profile_publish(
    profile_path: &str,
    endpoint: Option<&str>,
    token: Option<&str>,
) -> Result<()> {
    let content =
        std::fs::read(profile_path).map_err(|e| anyhow::anyhow!("failed to read profile: {e}"))?;

    let hash = conary_core::hash::sha256(&content);

    let endpoint =
        endpoint.ok_or_else(|| anyhow::anyhow!("--endpoint is required for profile publish"))?;
    let token = token.ok_or_else(|| anyhow::anyhow!("--token is required for profile publish"))?;

    let client = reqwest::Client::new();
    let resp = client
        .put(format!("{endpoint}/v1/profiles/{hash}"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/toml")
        .body(content)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("HTTP error: {e}"))?;

    if resp.status().is_success() {
        println!("Published profile to {endpoint}/v1/profiles/{hash}");
    } else {
        anyhow::bail!(
            "Server returned {}: {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::cmd_profile_generate;
    use conary_core::derivation::BuildProfile;
    use conary_core::derivation::compose::erofs_image_hash;
    use conary_core::derivation::seed::{SeedMetadata, SeedSource};
    use std::fs;
    use std::path::{Path, PathBuf};

    fn recipe_toml(name: &str, requires: &[&str], makedepends: &[&str]) -> String {
        let requires = requires
            .iter()
            .map(|dep| format!("\"{dep}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let makedepends = makedepends
            .iter()
            .map(|dep| format!("\"{dep}\""))
            .collect::<Vec<_>>()
            .join(", ");

        format!(
            r#"[package]
name = "{name}"
version = "1.0.0"

[source]
archive = "https://example.com/{name}-1.0.0.tar.gz"
checksum = "sha256:abc123"

[build]
requires = [{requires}]
makedepends = [{makedepends}]
install = "make install DESTDIR=%(destdir)s"
"#
        )
    }

    fn write_recipe(recipe_root: &Path, relative_path: &str, name: &str) {
        let path = recipe_root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, recipe_toml(name, &[], &[])).unwrap();
    }

    fn write_seed_dir(root: &Path) -> PathBuf {
        let seed_dir = root.join("seed");
        fs::create_dir_all(&seed_dir).unwrap();

        let image_path = seed_dir.join("seed.erofs");
        fs::write(&image_path, b"phase3 profile test seed").unwrap();
        let seed_id = erofs_image_hash(&image_path).unwrap();

        let seed = SeedMetadata {
            seed_id,
            source: SeedSource::SelfBuilt,
            origin_url: None,
            builder: Some("test".to_string()),
            packages: vec!["hello".to_string()],
            target_triple: "x86_64-unknown-linux-gnu".to_string(),
            verified_by: vec![],
            origin_distro: None,
            origin_version: None,
        };

        fs::write(seed_dir.join("seed.toml"), toml::to_string(&seed).unwrap()).unwrap();
        seed_dir
    }

    fn write_manifest(root: &Path, seed_source: &Path) -> PathBuf {
        let manifest = root.join("system.toml");
        fs::write(
            &manifest,
            format!(
                r#"[system]
name = "phase3-test"
target = "x86_64-unknown-linux-gnu"

[seed]
source = "{}"

[packages]
include = ["hello"]
"#,
                seed_source.display()
            ),
        )
        .unwrap();
        manifest
    }

    #[tokio::test]
    async fn test_profile_generate_writes_real_derivation_ids() {
        let temp = tempfile::tempdir().unwrap();
        let recipe_root = temp.path().join("recipes");
        write_recipe(&recipe_root, "system/hello.toml", "hello");
        let seed_dir = write_seed_dir(temp.path());
        let manifest = write_manifest(temp.path(), &seed_dir);
        let output = temp.path().join("profile.toml");

        cmd_profile_generate(&manifest, Some(&output))
            .await
            .unwrap();

        let profile = BuildProfile::from_toml(&fs::read_to_string(&output).unwrap()).unwrap();
        assert_eq!(profile.stages.len(), 1);
        assert_eq!(profile.stages[0].derivations.len(), 1);
        assert_ne!(profile.stages[0].derivations[0].derivation_id, "pending");
    }

    #[tokio::test]
    async fn test_profile_generate_stores_canonical_manifest_path() {
        let temp = tempfile::tempdir().unwrap();
        let recipe_root = temp.path().join("recipes");
        write_recipe(&recipe_root, "system/hello.toml", "hello");
        let seed_dir = write_seed_dir(temp.path());
        let manifest = write_manifest(temp.path(), &seed_dir);
        let output = temp.path().join("profile.toml");

        cmd_profile_generate(&manifest, Some(&output))
            .await
            .unwrap();

        let profile = BuildProfile::from_toml(&fs::read_to_string(&output).unwrap()).unwrap();
        assert_eq!(
            profile.profile.manifest,
            manifest.canonicalize().unwrap().display().to_string()
        );
    }

    #[tokio::test]
    async fn test_profile_generate_uses_seed_hash_for_stage_build_env() {
        let temp = tempfile::tempdir().unwrap();
        let recipe_root = temp.path().join("recipes");
        write_recipe(&recipe_root, "system/hello.toml", "hello");
        let seed_dir = write_seed_dir(temp.path());
        let manifest = write_manifest(temp.path(), &seed_dir);
        let output = temp.path().join("profile.toml");

        cmd_profile_generate(&manifest, Some(&output))
            .await
            .unwrap();

        let profile = BuildProfile::from_toml(&fs::read_to_string(&output).unwrap()).unwrap();
        assert_eq!(profile.stages[0].build_env, profile.seed.id);
    }

    #[tokio::test]
    async fn test_profile_generate_errors_when_recipe_is_missing() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("recipes")).unwrap();
        let seed_dir = write_seed_dir(temp.path());
        let manifest = write_manifest(temp.path(), &seed_dir);
        let output = temp.path().join("profile.toml");

        let error = cmd_profile_generate(&manifest, Some(&output))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("recipe"));
    }
}
