// apps/conary/src/commands/publish.rs

//! Publish command - build a recipe project and publish it to a static repo.

use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use conary_core::recipe::{Kitchen, KitchenConfig, parse_recipe_file, validate_recipe};
use conary_core::repository::static_repo::RepoLocation;
use conary_core::repository::static_repo::publish::{
    StaticPublishOptions, prepare_static_key_dir, publish_static_repo,
};

use super::cook::{recipe_source_base_dir, resolve_recipe_path};

const ARTIFACT_FORM_REJECTION: &str = "artifact-form publish requires M2 attestation support; run project-form publish from a recipe project";

pub struct PublishOptions {
    pub what: String,
    pub target: Option<String>,
    pub recipe: Option<String>,
    pub key_dir: Option<String>,
    pub state_file: Option<String>,
    pub refresh: bool,
    pub force_reinit: bool,
    pub accept_destination_state: bool,
    pub rotate_publish_key: bool,
    pub rotate_root_key: bool,
    pub yes: bool,
}

pub async fn cmd_publish(options: PublishOptions) -> Result<()> {
    if options.target.is_some() {
        bail!(ARTIFACT_FORM_REJECTION);
    }

    let destination = RepoLocation::parse(&options.what)
        .with_context(|| format!("parse static repo destination {}", options.what))?;
    ensure_m1a_publish_destination(&destination)?;
    let repo_name = derive_repo_name(&options.what)?;
    let key_dir = resolve_key_dir(options.key_dir.as_deref(), &repo_name)?;
    let state_file = options
        .state_file
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| key_dir.join("last-published.toml"));
    let recipe_path = resolve_recipe_path(None, options.recipe.as_deref())?;
    // Parsed for future interactive confirmation; M1a publish is non-interactive.
    let _ = options.yes;

    println!("Reading recipe: {}", recipe_path.display());
    let recipe = parse_recipe_file(&recipe_path)
        .with_context(|| format!("Failed to parse recipe: {}", recipe_path.display()))?;
    let warnings = validate_recipe(&recipe).with_context(|| "Recipe validation failed")?;
    for warning in &warnings {
        println!("Warning: {}", warning);
    }

    let output_dir = tempfile::tempdir().context("create temporary publish output directory")?;
    let config = publish_kitchen_config(&recipe_path, output_dir.path());
    let kitchen = Kitchen::new(config);

    println!("M1a static repos are preview repos, not reproducible release evidence.");
    println!(
        "Cooking {} {} for static publish (sandboxed, network allowed)...",
        recipe.package.name, recipe.package.version
    );

    let result = kitchen
        .cook(&recipe, output_dir.path())
        .with_context(|| format!("Failed to cook {}", recipe.package.name))?;

    let outcome = publish_static_repo(StaticPublishOptions {
        repo_name: repo_name.clone(),
        repo_description: None,
        destination,
        key_dir,
        state_file,
        package_paths: vec![result.package_path.clone()],
        refresh: options.refresh,
        force_reinit: options.force_reinit,
        accept_destination_state: options.accept_destination_state,
        rotate_publish_key: options.rotate_publish_key,
        rotate_root_key: options.rotate_root_key,
    })
    .with_context(|| format!("publish static repo {}", repo_name))?;

    println!("Published static repo: {repo_name}");
    println!("Root fingerprint(s): {}", outcome.root_key_ids.join(", "));
    println!("Publish key ID: {}", outcome.publish_key_id);
    println!(
        "Versions: root={} targets={} snapshot={} timestamp={}",
        outcome.root_version,
        outcome.targets_version,
        outcome.snapshot_version,
        outcome.timestamp_version
    );
    println!("Packages: {}", outcome.package_count);
    if !outcome.preview_warning.is_empty() {
        println!("{}", outcome.preview_warning);
    }

    Ok(())
}

fn publish_kitchen_config(recipe_path: &Path, output_dir: &Path) -> KitchenConfig {
    KitchenConfig {
        source_cache: output_dir.join("sources"),
        recipe_source_base_dir: Some(recipe_source_base_dir(recipe_path)),
        allow_network: true,
        use_isolation: true,
        pristine_mode: false,
        ..Default::default()
    }
}

fn ensure_m1a_publish_destination(destination: &RepoLocation) -> Result<()> {
    if matches!(destination, RepoLocation::Http { .. }) {
        bail!("M1a static publisher supports local filesystem destinations only");
    }

    Ok(())
}

fn derive_repo_name(destination: &str) -> Result<String> {
    let location = RepoLocation::parse(destination)
        .with_context(|| format!("parse static repo destination {}", destination))?;
    let repo_name = match location {
        RepoLocation::File { root } => root.file_name().map(|name| name.to_owned()),
        RepoLocation::Http { base } => base
            .rsplit('/')
            .find(|segment| !segment.is_empty())
            .map(std::ffi::OsString::from),
    };

    let repo_name = repo_name
        .and_then(|name| name.into_string().ok())
        .filter(|name| !name.trim().is_empty())
        .with_context(|| format!("derive static repo name from destination {destination}"))?;

    Ok(repo_name)
}

fn resolve_key_dir(key_dir: Option<&str>, repo_name: &str) -> Result<PathBuf> {
    if let Some(key_dir) = key_dir {
        return Ok(PathBuf::from(key_dir));
    }

    prepare_static_key_dir(&config_base_dir()?.join("conary").join("keys"), repo_name)
}

fn config_base_dir() -> Result<PathBuf> {
    if let Some(config_home) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(config_home));
    }

    if let Some(home) = env::var_os("HOME") {
        return Ok(PathBuf::from(home).join(".config"));
    }

    bail!("cannot determine config directory; set XDG_CONFIG_HOME or HOME")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn artifact_form_publish_is_rejected_in_m1a() {
        let error = cmd_publish(PublishOptions {
            what: "dist/pkg.ccs".to_string(),
            target: Some("./repo".to_string()),
            recipe: None,
            key_dir: None,
            state_file: None,
            refresh: false,
            force_reinit: false,
            accept_destination_state: false,
            rotate_publish_key: false,
            rotate_root_key: false,
            yes: false,
        })
        .await
        .unwrap_err();

        assert_eq!(error.to_string(), ARTIFACT_FORM_REJECTION);
    }

    #[test]
    fn publish_kitchen_config_forces_isolation_and_allows_network() {
        let recipe_path = std::path::Path::new("/work/pkg/recipe.toml");
        let output_dir = std::path::Path::new("/tmp/conary-publish-out");
        let config = publish_kitchen_config(recipe_path, output_dir);

        assert!(config.use_isolation);
        assert!(config.allow_network);
        assert!(!config.pristine_mode);
        assert_eq!(
            config.recipe_source_base_dir,
            Some(std::path::PathBuf::from("/work/pkg"))
        );
    }

    #[tokio::test]
    async fn http_publish_destination_is_rejected_before_local_side_effects() {
        let temp_dir = tempfile::tempdir().unwrap();
        let key_dir = temp_dir.path().join("keys");
        let error = cmd_publish(PublishOptions {
            what: "https://example.invalid/static/repo".to_string(),
            target: None,
            recipe: Some("missing-recipe.toml".to_string()),
            key_dir: Some(key_dir.display().to_string()),
            state_file: None,
            refresh: false,
            force_reinit: false,
            accept_destination_state: false,
            rotate_publish_key: false,
            rotate_root_key: false,
            yes: false,
        })
        .await
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "M1a static publisher supports local filesystem destinations only"
        );
        assert!(!key_dir.exists());
    }

    #[test]
    fn repo_name_is_derived_from_destination_tail() {
        assert_eq!(derive_repo_name("./repo").unwrap(), "repo");
        assert_eq!(
            derive_repo_name("https://example.invalid/static/acme").unwrap(),
            "acme"
        );
    }
}
