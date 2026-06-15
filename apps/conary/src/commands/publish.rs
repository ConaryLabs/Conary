// apps/conary/src/commands/publish.rs

//! Publish command - build a recipe project and publish it to a static repo.

use std::env;
use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use conary_core::ccs::manifest::ManifestProvenance;
use conary_core::recipe::Recipe;
use conary_core::recipe::hermetic::{CiMode, DivergenceStatus, HermeticBuildInput};
use conary_core::recipe::{
    Kitchen, KitchenConfig, SourceDownloadPolicy, parse_recipe_file, validate_recipe,
};
use conary_core::repository::static_repo::RepoLocation;
use conary_core::repository::static_repo::publish::{
    StaticPublishOptions, prepare_static_key_dir, publish_static_repo,
};
use conary_core::repository::static_repo::publish_context::{
    ProjectFormAttestationInput, attach_project_form_attestation,
    prepare_artifact_form_static_context, prepare_project_form_static_context,
};
use conary_core::repository::static_repo::publish_gate::{
    format_publish_gate_failures, verify_static_artifact_publish_eligibility,
};

use super::cook::{recipe_source_base_dir, resolve_recipe_path};
use super::hermetic_config::{ensure_no_build_dependencies_for_m2a, load_default_hermetic_builder};
use super::hermetic_state::{load_latest_host_build_record_for_recipe, resolve_default_state_dir};
use super::remi_publish::{RemiPublishOptions, publish_to_remi, resolve_remi_publish_bearer_token};

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
    if let Some(target) = options.target.clone() {
        publish_artifact_form(options, &target).await
    } else {
        publish_project_form(options).await
    }
}

async fn publish_project_form(options: PublishOptions) -> Result<()> {
    let destination = RepoLocation::parse(&options.what)
        .with_context(|| format!("parse static repo destination {}", options.what))?;
    ensure_static_local_publish_destination(&destination)?;
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

    let builder = load_default_hermetic_builder()?;
    ensure_no_build_dependencies_for_m2a(&recipe)?;

    let output_dir = tempfile::tempdir().context("create temporary publish output directory")?;
    let builder_identity = builder.identity;
    let builder_sysroot = builder.sysroot_path;
    let hermetic_input = HermeticBuildInput::explicit_recipe(
        recipe_source_base_dir(&recipe_path),
        recipe_path.clone(),
        sha256_prefixed_file(&recipe_path)?,
    )
    .with_builder_environment(builder_identity);
    let mut config = publish_kitchen_config(&recipe_path, output_dir.path(), builder_sysroot);
    configure_host_record_for_publish(&mut config, &recipe);
    let kitchen = Kitchen::new(config);

    println!(
        "Cooking and attesting {} {} for static release publish...",
        recipe.package.name, recipe.package.version
    );

    let result = kitchen
        .cook_hermetic(
            &recipe,
            hermetic_input,
            output_dir.path(),
            release_publish_ci_mode(),
        )
        .with_context(|| format!("Failed to hermetically cook {}", recipe.package.name))?;
    print_divergence_summary(result.provenance.as_ref());
    let prepared =
        prepare_project_form_static_context(&destination, &key_dir, options.force_reinit)
            .with_context(|| format!("prepare static publish context for {}", repo_name))?;
    let attested_package_path = attach_project_form_attestation(ProjectFormAttestationInput {
        package_path: &result.package_path,
        provenance: result
            .provenance
            .as_ref()
            .context("project-form publish requires hermetic provenance")?,
        context: &prepared,
        conary_version: env!("CARGO_PKG_VERSION"),
    })?;

    let outcome = publish_static_repo(StaticPublishOptions {
        repo_name: repo_name.clone(),
        repo_description: None,
        destination,
        key_dir,
        state_file,
        package_paths: vec![attested_package_path],
        refresh: options.refresh,
        force_reinit: options.force_reinit,
        accept_destination_state: options.accept_destination_state,
        rotate_publish_key: options.rotate_publish_key,
        rotate_root_key: options.rotate_root_key,
        artifact_gate_context: None,
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

async fn publish_artifact_form(options: PublishOptions, target: &str) -> Result<()> {
    match classify_publish_target(target)? {
        PublishTargetRoute::StaticLocal => publish_static_artifact_form(options, target).await,
        PublishTargetRoute::RemiRelease => publish_remi_artifact_form(options, target).await,
    }
}

async fn publish_static_artifact_form(options: PublishOptions, target: &str) -> Result<()> {
    let artifact_path = PathBuf::from(&options.what);
    let destination =
        RepoLocation::parse(target).with_context(|| format!("parse publish target {target}"))?;
    ensure_static_local_publish_destination(&destination)?;
    let repo_name = derive_repo_name(target)?;
    let key_dir = resolve_key_dir(options.key_dir.as_deref(), &repo_name)?;
    let prepared =
        prepare_artifact_form_static_context(&destination, &key_dir, options.force_reinit)
            .with_context(|| format!("prepare static artifact publish context for {repo_name}"))?;
    let report = verify_static_artifact_publish_eligibility(
        &artifact_path,
        &prepared.accepted_signers,
        &prepared.publish_policy_digest,
    )?;
    if !report.is_passed() {
        bail!("{}", format_publish_gate_failures(&report));
    }
    let state_file = options
        .state_file
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| key_dir.join("last-published.toml"));
    let outcome = publish_static_repo(StaticPublishOptions {
        repo_name: repo_name.clone(),
        repo_description: None,
        destination,
        key_dir,
        state_file,
        package_paths: vec![artifact_path],
        refresh: options.refresh,
        force_reinit: options.force_reinit,
        accept_destination_state: options.accept_destination_state,
        rotate_publish_key: options.rotate_publish_key,
        rotate_root_key: options.rotate_root_key,
        artifact_gate_context: Some(prepared.artifact_gate_context()),
    })
    .with_context(|| format!("publish attested artifact to static repo {repo_name}"))?;

    println!("Published attested artifact to static repo: {repo_name}");
    println!("Publish key ID: {}", outcome.publish_key_id);

    Ok(())
}

async fn publish_remi_artifact_form(options: PublishOptions, target: &str) -> Result<()> {
    let artifact_path = PathBuf::from(&options.what);
    let bearer_token = resolve_remi_publish_bearer_token()?;
    publish_to_remi(RemiPublishOptions {
        artifact_path: &artifact_path,
        target_url: target,
        bearer_token: &bearer_token,
    })
    .await?;

    println!("Published attested artifact to Remi release endpoint: {target}");
    Ok(())
}

fn publish_kitchen_config(
    recipe_path: &Path,
    output_dir: &Path,
    sysroot: PathBuf,
) -> KitchenConfig {
    KitchenConfig {
        source_cache: output_dir.join("sources"),
        recipe_source_base_dir: Some(recipe_source_base_dir(recipe_path)),
        allow_network: false,
        use_isolation: true,
        pristine_mode: true,
        sysroot: Some(sysroot),
        auto_makedepends: false,
        cleanup_makedepends: false,
        source_download_policy: SourceDownloadPolicy::AllowDownloads,
        ..Default::default()
    }
}

fn release_publish_ci_mode() -> CiMode {
    CiMode::On
}

fn sha256_prefixed_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)
        .with_context(|| format!("Failed to open recipe for hashing: {}", path.display()))?;
    let hash = conary_core::hash::sha256_reader_hex(&mut file)
        .with_context(|| format!("Failed to hash recipe: {}", path.display()))?;
    Ok(format!("sha256:{hash}"))
}

fn configure_host_record_for_publish(config: &mut KitchenConfig, recipe: &Recipe) {
    let architecture = Some(std::env::consts::ARCH);
    match resolve_default_state_dir() {
        Ok(state_dir) => {
            let lookup = load_latest_host_build_record_for_recipe(&state_dir, recipe, architecture);
            config.expected_host_build_record = lookup.record;
            config.host_build_record_diagnostics = lookup.diagnostics;
        }
        Err(error) => {
            config.host_build_record_diagnostics = vec![format!(
                "failed to resolve hermetic host record state directory: {error}"
            )];
        }
    }
}

fn print_divergence_summary(provenance: Option<&ManifestProvenance>) {
    let Some(evidence) = provenance.and_then(|provenance| provenance.hermetic_evidence.as_ref())
    else {
        return;
    };
    if evidence.divergence.status == DivergenceStatus::DiffersFromHost {
        println!(
            "Warning: hermetic output differs from the latest host build record; this is diagnostic-only in M2a."
        );
    }
}

fn ensure_static_local_publish_destination(destination: &RepoLocation) -> Result<()> {
    if matches!(destination, RepoLocation::Http { .. }) {
        bail!(
            "static publisher supports local filesystem destinations; Remi HTTP(S) targets use the Remi release path"
        );
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublishTargetRoute {
    StaticLocal,
    RemiRelease,
}

fn classify_publish_target(target: &str) -> Result<PublishTargetRoute> {
    if target.starts_with("http://") || target.starts_with("https://") {
        if target.contains("/v1/admin/releases/") {
            return Ok(PublishTargetRoute::RemiRelease);
        }
        bail!(
            "HTTP(S) publish targets must use the Remi release endpoint /v1/admin/releases/{{distro}}"
        );
    }
    Ok(PublishTargetRoute::StaticLocal)
}

#[cfg(test)]
fn classify_publish_target_for_tests(target: &str) -> Result<PublishTargetRoute> {
    classify_publish_target(target)
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
    use std::ffi::OsString;
    use std::process::Command;
    use std::sync::Mutex;

    const TEST_HASH: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[tokio::test]
    async fn artifact_form_publish_rejects_missing_attestation() {
        let fixture = ArtifactPublishFixture::without_attestation();

        let error = cmd_publish(fixture.options()).await.unwrap_err();
        let error = format!("{error:#}");

        assert!(
            error.contains("artifact is missing a build attestation"),
            "{error}"
        );
    }

    #[test]
    fn publish_kitchen_config_uses_hermetic_defaults() {
        let recipe_path = std::path::Path::new("/work/pkg/recipe.toml");
        let output_dir = std::path::Path::new("/tmp/conary-publish-out");
        let sysroot = std::path::PathBuf::from("/var/lib/conary/sysroots/test");
        let config = publish_kitchen_config(recipe_path, output_dir, sysroot.clone());

        assert!(config.use_isolation);
        assert!(!config.allow_network);
        assert!(config.pristine_mode);
        assert_eq!(config.sysroot, Some(sysroot));
        assert_eq!(
            config.source_download_policy,
            conary_core::recipe::SourceDownloadPolicy::AllowDownloads
        );
        assert_eq!(
            config.recipe_source_base_dir,
            Some(std::path::PathBuf::from("/work/pkg"))
        );
    }

    #[tokio::test]
    async fn project_form_publish_uses_release_dirty_tree_refusal() {
        let fixture = DirtyGitPublishFixture::new();
        let _env_lock = ENV_LOCK.lock().unwrap();
        let _config_guard = EnvVarGuard::set("CONARY_HERMETIC_CONFIG", &fixture.config_path);
        let _conary_ci_guard = EnvVarGuard::set("CONARY_HERMETIC_CI", "0");
        let _ci_guard = EnvVarGuard::remove("CI");

        let error = cmd_publish(fixture.options()).await.unwrap_err();
        let error = format!("{error:#}");

        assert!(error.contains("dirty local tree"), "{error}");
    }

    #[test]
    fn publish_prefetch_config_allows_downloads_before_hermetic_build() {
        let recipe_path = std::path::Path::new("/work/pkg/recipe.toml");
        let output_dir = std::path::Path::new("/tmp/conary-publish-out");
        let sysroot = std::path::PathBuf::from("/tmp/sysroot");
        let config = publish_kitchen_config(recipe_path, output_dir, sysroot);

        assert!(!config.allow_network);
        assert_eq!(
            config.source_download_policy,
            conary_core::recipe::SourceDownloadPolicy::AllowDownloads
        );
    }

    #[tokio::test]
    async fn project_form_publish_fails_without_hermetic_config() {
        let temp = tempfile::tempdir().unwrap();
        let recipe_path = temp.path().join("recipe.toml");
        let repo_dir = temp.path().join("repo");
        let key_dir = temp.path().join("keys");
        let state_file = temp.path().join("publish-state.toml");
        std::fs::write(
            &recipe_path,
            r#"
[package]
name = "publish-local"
version = "1.0"

[source]
path = "."

[build]
install = "mkdir -p %(destdir)s/usr/share/publish-local && printf hi > %(destdir)s/usr/share/publish-local/hello.txt"
"#,
        )
        .unwrap();

        let error = cmd_publish(PublishOptions {
            what: repo_dir.display().to_string(),
            target: None,
            recipe: Some(recipe_path.display().to_string()),
            key_dir: Some(key_dir.display().to_string()),
            state_file: Some(state_file.display().to_string()),
            refresh: false,
            force_reinit: false,
            accept_destination_state: false,
            rotate_publish_key: false,
            rotate_root_key: false,
            yes: true,
        })
        .await
        .unwrap_err();
        let error = format!("{error:#}");

        assert!(error.contains("hermetic config"), "{error}");
        assert!(
            !repo_dir.exists(),
            "publish should fail before writing the static repo"
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
            "static publisher supports local filesystem destinations; Remi HTTP(S) targets use the Remi release path"
        );
        assert!(!key_dir.exists());
    }

    #[test]
    fn http_publish_target_routes_to_remi_release_path() {
        let route = classify_publish_target_for_tests(
            "https://remi.example.invalid/v1/admin/releases/test",
        )
        .unwrap();

        assert_eq!(route, PublishTargetRoute::RemiRelease);
    }

    #[test]
    fn remi_release_publish_target_routes_to_release_endpoint() {
        let route = classify_publish_target_for_tests(
            "https://remi.example.invalid/v1/admin/releases/test",
        )
        .unwrap();

        assert_eq!(route, PublishTargetRoute::RemiRelease);
    }

    #[test]
    fn static_local_guard_still_rejects_http_static_path() {
        let destination = RepoLocation::parse("https://repo.example.invalid/static").unwrap();
        let error = ensure_static_local_publish_destination(&destination).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Remi HTTP(S) targets use the Remi release path")
        );
    }

    #[test]
    fn repo_name_is_derived_from_destination_tail() {
        assert_eq!(derive_repo_name("./repo").unwrap(), "repo");
        assert_eq!(
            derive_repo_name("https://example.invalid/static/acme").unwrap(),
            "acme"
        );
    }

    struct DirtyGitPublishFixture {
        _temp: tempfile::TempDir,
        recipe_path: PathBuf,
        repo_dir: PathBuf,
        key_dir: PathBuf,
        state_file: PathBuf,
        config_path: PathBuf,
    }

    impl DirtyGitPublishFixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let project_dir = temp.path().join("project");
            let recipe_path = project_dir.join("recipe.toml");
            let sysroot = temp.path().join("sysroot");
            let config_path = temp.path().join("hermetic.toml");
            std::fs::create_dir_all(&project_dir).unwrap();
            std::fs::create_dir_all(&sysroot).unwrap();
            std::fs::write(project_dir.join("source.txt"), "clean\n").unwrap();
            std::fs::write(
                &recipe_path,
                r#"
[package]
name = "dirty-release"
version = "1.0"

[source]
path = "."

[build]
install = "mkdir -p %(destdir)s/usr/share/dirty-release && printf hi > %(destdir)s/usr/share/dirty-release/payload"
"#,
            )
            .unwrap();
            run_git(&project_dir, &["init"]);
            run_git(
                &project_dir,
                &["config", "user.email", "test@example.invalid"],
            );
            run_git(&project_dir, &["config", "user.name", "Conary Test"]);
            run_git(&project_dir, &["add", "."]);
            run_git(&project_dir, &["commit", "-m", "initial"]);
            std::fs::write(project_dir.join("source.txt"), "dirty\n").unwrap();
            std::fs::write(
                &config_path,
                format!(
                    r#"
default_builder = "test"

[builders.test]
kind = "pristine"
sysroot_path = "{}"
sysroot_hash = "{TEST_HASH}"
"#,
                    sysroot.display()
                ),
            )
            .unwrap();

            Self {
                repo_dir: temp.path().join("repo"),
                key_dir: temp.path().join("keys"),
                state_file: temp.path().join("publish-state.toml"),
                _temp: temp,
                recipe_path,
                config_path,
            }
        }

        fn options(&self) -> PublishOptions {
            PublishOptions {
                what: self.repo_dir.display().to_string(),
                target: None,
                recipe: Some(self.recipe_path.display().to_string()),
                key_dir: Some(self.key_dir.display().to_string()),
                state_file: Some(self.state_file.display().to_string()),
                refresh: false,
                force_reinit: false,
                accept_destination_state: false,
                rotate_publish_key: false,
                rotate_root_key: false,
                yes: true,
            }
        }
    }

    struct ArtifactPublishFixture {
        _temp: tempfile::TempDir,
        package_path: PathBuf,
        repo_dir: PathBuf,
        key_dir: PathBuf,
        state_file: PathBuf,
    }

    impl ArtifactPublishFixture {
        fn without_attestation() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let source_dir = temp.path().join("source");
            let package_path = temp.path().join("dist/widget-1.0.0.ccs");
            let key_dir = temp.path().join("keys");
            std::fs::create_dir_all(source_dir.join("usr/share/widget")).unwrap();
            std::fs::create_dir_all(package_path.parent().unwrap()).unwrap();
            std::fs::write(source_dir.join("usr/share/widget/payload"), "hello\n").unwrap();
            let manifest = conary_core::ccs::CcsManifest::parse(
                r#"
[package]
name = "widget"
version = "1.0.0"
description = "fixture package"
license = "MIT"

[provenance]
origin_class = "native-built"
hardening_level = "hermetic"
"#,
            )
            .unwrap();
            let result = conary_core::ccs::CcsBuilder::new(manifest, &source_dir)
                .build()
                .unwrap();
            let key = conary_core::ccs::SigningKeyPair::generate().with_key_id("publish");
            key.save_to_files(
                &key_dir.join("publish.private"),
                &key_dir.join("publish.public"),
            )
            .unwrap();
            conary_core::ccs::builder::write_signed_ccs_package(&result, &package_path, &key)
                .unwrap();

            Self {
                repo_dir: temp.path().join("repo"),
                state_file: temp.path().join("artifact-publish-state.toml"),
                _temp: temp,
                package_path,
                key_dir,
            }
        }

        fn options(&self) -> PublishOptions {
            PublishOptions {
                what: self.package_path.display().to_string(),
                target: Some(self.repo_dir.display().to_string()),
                recipe: None,
                key_dir: Some(self.key_dir.display().to_string()),
                state_file: Some(self.state_file.display().to_string()),
                refresh: false,
                force_reinit: false,
                accept_destination_state: false,
                rotate_publish_key: false,
                rotate_root_key: false,
                yes: true,
            }
        }
    }

    fn run_git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(key);
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(previous) = &self.previous {
                    std::env::set_var(self.key, previous);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    static ENV_LOCK: Mutex<()> = Mutex::new(());
}
