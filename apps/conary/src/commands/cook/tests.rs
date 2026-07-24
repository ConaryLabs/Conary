// apps/conary/src/commands/cook/tests.rs

use super::*;
use conary_core::ccs::CcsPackage;
use conary_core::packages::PackageFormat;
use conary_core::recipe::SourceDownloadPolicy;
use std::fs::File;
use std::process::Command;
use tar::Builder;

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

fn write_cargo_source_tree(root: &Path, package_name: &str) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join(".cargo")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        format!(
            r#"[package]
name = "{package_name}"
version = "0.1.0"
edition = "2021"
"#
        ),
    )
    .unwrap();
    std::fs::write(
        root.join(".cargo/config.toml"),
        "[build]\ntarget-dir = \"target\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
}

fn write_tar_archive(source_root: &Path, archive_path: &Path, top_level: &str) {
    let file = File::create(archive_path).unwrap();
    let mut builder = Builder::new(file);
    builder.append_dir_all(top_level, source_root).unwrap();
    builder.finish().unwrap();
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn initialize_git_remote(source_root: &Path, remote: &Path, package_name: &str) -> String {
    write_cargo_source_tree(source_root, package_name);
    git(source_root, &["init"]);
    git(
        source_root,
        &["config", "user.email", "conary@example.invalid"],
    );
    git(source_root, &["config", "user.name", "Conary Test"]);
    git(source_root, &["add", "."]);
    git(source_root, &["commit", "-m", "initial"]);
    let commit = git(source_root, &["rev-parse", "HEAD"]);
    git(
        source_root.parent().unwrap(),
        &[
            "clone",
            "--bare",
            source_root.to_str().unwrap(),
            remote.to_str().unwrap(),
        ],
    );
    commit
}

fn cooked_manifest_provenance(
    output_dir: &Path,
    package_name: &str,
    version: &str,
) -> conary_core::ccs::manifest::ManifestProvenance {
    let package_path = output_dir.join(format!("{package_name}-{version}-1.ccs"));
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
fn cooked_artifact_path_extracts_single_ccs_artifact() {
    let mut output = PackagingCommandOutput::succeeded("watch-1", "conary cook");
    output.artifacts.push(PackagingArtifact {
        path: "/tmp/demo.ccs".to_string(),
        kind: Some("ccs".to_string()),
    });

    assert_eq!(
        cooked_artifact_path(&output).unwrap(),
        PathBuf::from("/tmp/demo.ccs")
    );
}

#[test]
fn watch_refresh_cook_options_force_offline_policy_for_hermetic_refresh() {
    let options = CookForTryWatchOptions {
        target: Some("."),
        recipe: None,
        output_dir: "dist",
        source_cache: "sources",
        jobs: None,
        keep_builddir: false,
        isolated: true,
        no_isolation: false,
        hermetic: false,
        operation_id: "watch-1".to_string(),
        source_policy: WatchCookSourcePolicy::Refresh,
    };

    assert_eq!(
        watch_source_download_policy_override(&options),
        Some(SourceDownloadPolicy::OfflineCacheOnly)
    );
}

#[test]
fn watch_refresh_preserves_source_policy_for_non_hermetic_refresh() {
    let options = CookForTryWatchOptions {
        target: Some("."),
        recipe: None,
        output_dir: "dist",
        source_cache: "sources",
        jobs: None,
        keep_builddir: false,
        isolated: false,
        no_isolation: false,
        hermetic: false,
        source_policy: WatchCookSourcePolicy::Refresh,
        operation_id: "watch-1".to_string(),
    };

    assert_eq!(watch_source_download_policy_override(&options), None);
}

#[test]
fn watch_initial_cook_does_not_force_offline_policy() {
    let options = CookForTryWatchOptions {
        target: Some("."),
        recipe: None,
        output_dir: "dist",
        source_cache: "sources",
        jobs: None,
        keep_builddir: false,
        isolated: true,
        no_isolation: false,
        hermetic: false,
        source_policy: WatchCookSourcePolicy::Initial,
        operation_id: "watch-1".to_string(),
    };

    let _adapter: for<'a> fn(CookForTryWatchOptions<'a>) -> Result<PackagingCommandOutput> =
        run_cook_for_try_watch;
    assert_eq!(watch_source_download_policy_override(&options), None);
}

#[test]
fn foreign_package_format_detects_release_artifacts() {
    assert_eq!(foreign_package_format(Path::new("pkg.rpm")), Some("rpm"));
    assert_eq!(foreign_package_format(Path::new("pkg.deb")), Some("deb"));
    assert_eq!(
        foreign_package_format(Path::new("pkg.pkg.tar.zst")),
        Some("arch")
    );
    assert_eq!(foreign_package_format(Path::new("recipe.toml")), None);
}

#[tokio::test]
async fn cook_foreign_package_routes_before_recipe_resolution() {
    let temp = tempfile::tempdir().unwrap();
    let foreign = temp.path().join("demo.rpm");
    let output_dir = temp.path().join("out");
    std::fs::write(&foreign, b"not really rpm").unwrap();
    let mut output = Vec::new();

    let error = cmd_cook_with_output(
        Some(foreign.to_str().unwrap()),
        None,
        output_dir.to_str().unwrap(),
        temp.path().join("sources").to_str().unwrap(),
        None,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        &mut output,
    )
    .await
    .unwrap_err();
    let error = format!("{error:#}");

    assert!(error.contains("Failed to parse foreign package"), "{error}");
}

#[test]
fn host_iteration_env_pins_cargo_target_dir_to_recipe_local_target() {
    let mut config = KitchenConfig::default();

    add_host_iteration_env(&mut config);

    assert!(
        config
            .extra_env
            .iter()
            .any(|(key, value)| key == "CARGO_TARGET_DIR" && value == "target"),
        "host iteration cook should override host Cargo target-dir config: {:?}",
        config.extra_env
    );
}

#[test]
fn resolve_cook_input_prefers_recipe_flag_over_target() {
    let temp = tempfile::tempdir().unwrap();
    let source_tree = temp.path().join("source-tree");
    let recipe_path = temp.path().join("explicit.toml");
    write_cargo_source_tree(&source_tree, "target-marker");
    write_local_recipe(&recipe_path);
    let expected = recipe_path.canonicalize().unwrap();
    let resolved = resolve_cook_input(
        Some(source_tree.to_str().unwrap()),
        Some(recipe_path.to_str().unwrap()),
    )
    .unwrap();

    assert_eq!(resolved.recipe_path.as_deref(), Some(expected.as_path()));
    assert_eq!(resolved.recipe.package.name, "local");
    assert!(resolved.origin_class_override.is_none());
}

#[test]
fn resolve_cook_input_accepts_directory_with_recipe_toml() {
    let temp = tempfile::tempdir().unwrap();
    let recipe_path = temp.path().join("recipe.toml");
    write_local_recipe(&recipe_path);
    let expected = recipe_path.canonicalize().unwrap();
    let resolved = resolve_cook_input(Some(temp.path().to_str().unwrap()), None).unwrap();

    assert_eq!(resolved.recipe_path.as_deref(), Some(expected.as_path()));
    assert!(resolved.origin_class_override.is_none());
}

#[test]
fn recorded_draft_validation_run_options_set_origin_override_and_isolation() {
    let options = CookRecordedDraftOptions {
        recipe: PathBuf::from("recorded/demo/recipe.toml"),
        output_dir: PathBuf::from("recorded/demo/dist"),
        source_cache: PathBuf::from("recorded/demo/sources"),
        operation_id: "record-1".to_string(),
    };
    let recipe = options.recipe.to_string_lossy().to_string();
    let output_dir = options.output_dir.to_string_lossy().to_string();
    let source_cache = options.source_cache.to_string_lossy().to_string();
    let run = recorded_draft_run_options(&options, &recipe, &output_dir, &source_cache);

    assert!(run.isolated);
    assert!(!run.no_isolation);
    assert_eq!(run.origin_class_override.as_deref(), Some("recorded-draft"));
}

#[test]
fn ordinary_cook_run_options_leave_origin_override_unset() {
    let operation_id = "cook-1".to_string();
    let run = CookRunOptions {
        target: Some("recipe.toml"),
        recipe: None,
        output_dir: "dist",
        source_cache: "sources",
        jobs: None,
        keep_builddir: false,
        validate_only: false,
        fetch_only: false,
        explain: false,
        isolated: false,
        no_isolation: false,
        hermetic: false,
        json: false,
        operation_id,
        source_download_policy_override: None,
        origin_class_override: None,
    };

    assert!(run.origin_class_override.is_none());
}

#[test]
fn resolve_cook_input_infers_existing_bare_source_directory_for_m1b() {
    let temp = tempfile::tempdir().unwrap();
    let bare_target = temp.path().join("source-tree");
    write_cargo_source_tree(&bare_target, "bare-source");

    let resolved = resolve_cook_input(Some(bare_target.to_str().unwrap()), None).unwrap();

    assert!(resolved.recipe_path.is_none());
    assert_eq!(resolved.recipe.package.name, "bare-source");
    assert_eq!(
        resolved.origin_class_override.as_deref(),
        Some("inferred-source")
    );
}

#[test]
fn resolve_cook_input_unsupported_target_mentions_supported_forms() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("missing-source-tree");

    let error = resolve_cook_input(Some(target.to_str().unwrap()), None).unwrap_err();

    assert!(
        error.to_string().contains("Unsupported source target"),
        "unsupported target error should name supported forms: {error:#}"
    );
}

#[tokio::test]
async fn cook_directory_with_recipe_toml_uses_explicit_recipe_provenance() {
    let temp = tempfile::tempdir().unwrap();
    let recipe_dir = temp.path().join("recipe-dir");
    let output_dir = temp.path().join("out");
    let source_cache = temp.path().join("sources");
    std::fs::create_dir_all(&recipe_dir).unwrap();
    write_installing_local_recipe(&recipe_dir.join("recipe.toml"));

    cmd_cook(
        Some(recipe_dir.to_str().unwrap()),
        None,
        output_dir.to_str().unwrap(),
        source_cache.to_str().unwrap(),
        None,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
    )
    .await
    .unwrap();

    let provenance = cooked_manifest_provenance(&output_dir, "local", "1.0");
    assert_eq!(provenance.origin_class.as_deref(), Some("native-built"));
    assert_eq!(provenance.hardening_level.as_deref(), Some("host"));
}

#[tokio::test]
async fn cook_cargo_directory_infers_recipe_and_stamps_inferred_source() {
    let temp = tempfile::tempdir().unwrap();
    let source_tree = temp.path().join("cargo-source");
    let output_dir = temp.path().join("out");
    let source_cache = temp.path().join("sources");
    write_cargo_source_tree(&source_tree, "inferred-local");

    cmd_cook(
        Some(source_tree.to_str().unwrap()),
        None,
        output_dir.to_str().unwrap(),
        source_cache.to_str().unwrap(),
        None,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
    )
    .await
    .unwrap();

    let provenance = cooked_manifest_provenance(&output_dir, "inferred-local", "0.1.0");
    assert_eq!(provenance.origin_class.as_deref(), Some("inferred-source"));
    assert_eq!(provenance.hardening_level.as_deref(), Some("host"));
    assert!(
        provenance
            .upstream_url
            .as_deref()
            .is_some_and(|url| url.starts_with("local:")),
        "local source inference should stamp a local source marker: {provenance:?}"
    );
}

#[tokio::test]
async fn cook_archive_target_stamps_archive_source_identity() {
    let temp = tempfile::tempdir().unwrap();
    let source_tree = temp.path().join("archive-source");
    let archive_path = temp.path().join("archive-demo.tar");
    let output_dir = temp.path().join("out");
    let source_cache = temp.path().join("sources");
    write_cargo_source_tree(&source_tree, "archive-demo");
    write_tar_archive(&source_tree, &archive_path, "archive-demo-0.1.0");

    cmd_cook(
        Some(archive_path.to_str().unwrap()),
        None,
        output_dir.to_str().unwrap(),
        source_cache.to_str().unwrap(),
        None,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
    )
    .await
    .unwrap();

    let provenance = cooked_manifest_provenance(&output_dir, "archive-demo", "0.1.0");
    assert_eq!(provenance.origin_class.as_deref(), Some("inferred-source"));
    assert_eq!(provenance.upstream_url.as_deref(), archive_path.to_str());
    assert!(
        provenance
            .upstream_hash
            .as_deref()
            .is_some_and(|hash| hash.starts_with("sha256:")),
        "archive inference should stamp archive checksum: {provenance:?}"
    );
    assert!(provenance.git_commit.is_none());
}

#[tokio::test]
async fn cook_git_target_stamps_git_source_identity() {
    let temp = tempfile::tempdir().unwrap();
    let source_tree = temp.path().join("git-source");
    let remote = temp.path().join("git-demo.git");
    let output_dir = temp.path().join("out");
    let source_cache = temp.path().join("sources");
    let commit = initialize_git_remote(&source_tree, &remote, "git-demo");

    cmd_cook(
        Some(remote.to_str().unwrap()),
        None,
        output_dir.to_str().unwrap(),
        source_cache.to_str().unwrap(),
        None,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
    )
    .await
    .unwrap();

    let provenance = cooked_manifest_provenance(&output_dir, "git-demo", "0.1.0");
    assert_eq!(provenance.origin_class.as_deref(), Some("inferred-source"));
    assert_eq!(provenance.upstream_url.as_deref(), remote.to_str());
    assert_eq!(provenance.git_commit.as_deref(), Some(commit.as_str()));
    assert!(provenance.upstream_hash.is_none());
}

#[tokio::test]
async fn cook_recipe_flag_wins_over_source_target_markers() {
    let temp = tempfile::tempdir().unwrap();
    let source_tree = temp.path().join("cargo-source");
    let recipe_path = temp.path().join("explicit.toml");
    let output_dir = temp.path().join("out");
    let source_cache = temp.path().join("sources");
    write_cargo_source_tree(&source_tree, "target-marker");
    write_local_recipe(&recipe_path);

    let mut output = Vec::new();
    cmd_cook_with_output(
        Some(source_tree.to_str().unwrap()),
        Some(recipe_path.to_str().unwrap()),
        output_dir.to_str().unwrap(),
        source_cache.to_str().unwrap(),
        None,
        false,
        true,
        false,
        false,
        false,
        false,
        false,
        false,
        &mut output,
    )
    .await
    .unwrap();

    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("Recipe: local version 1.0"), "{output}");
    assert!(
        !output.contains("target-marker"),
        "--recipe should bypass target inference: {output}"
    );
}

#[tokio::test]
async fn cook_positional_custom_toml_recipe_validates() {
    let temp = tempfile::tempdir().unwrap();
    let recipe_path = temp.path().join("custom.toml");
    let output_dir = temp.path().join("out");
    let source_cache = temp.path().join("sources");
    write_local_recipe(&recipe_path);

    let mut output = Vec::new();
    cmd_cook_with_output(
        Some(recipe_path.to_str().unwrap()),
        None,
        output_dir.to_str().unwrap(),
        source_cache.to_str().unwrap(),
        None,
        false,
        true,
        false,
        false,
        false,
        false,
        false,
        false,
        &mut output,
    )
    .await
    .unwrap();

    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("Reading recipe:"), "{output}");
    assert!(output.contains("Recipe: local version 1.0"), "{output}");
    assert!(output.contains("Recipe validation passed"), "{output}");
    assert!(
        !output_dir.exists(),
        "validate-only custom recipe should not create build output"
    );
}

#[tokio::test]
async fn cook_validate_only_json_has_schema_version_and_summary() {
    let temp = tempfile::tempdir().unwrap();
    let recipe_path = temp.path().join("recipe.toml");
    let output_dir = temp.path().join("out");
    let source_cache = temp.path().join("sources");
    write_local_recipe(&recipe_path);

    let mut output = Vec::new();
    cmd_cook_with_output(
        Some(recipe_path.to_str().unwrap()),
        None,
        output_dir.to_str().unwrap(),
        source_cache.to_str().unwrap(),
        None,
        false,
        true,
        false,
        false,
        false,
        false,
        false,
        true,
        &mut output,
    )
    .await
    .unwrap();

    let value: serde_json::Value = serde_json::from_slice(&output).expect("valid cook json");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["command"], "conary cook");
    assert_eq!(value["status"], "succeeded");
    assert_eq!(value["summary"], "Recipe validation passed");
    assert!(value["operation_id"].as_str().unwrap().starts_with("cook-"));
}

#[tokio::test]
async fn cook_json_conflict_error_is_single_structured_json() {
    let temp = tempfile::tempdir().unwrap();
    let recipe_path = temp.path().join("recipe.toml");
    let output_dir = temp.path().join("out");
    let source_cache = temp.path().join("sources");
    write_local_recipe(&recipe_path);

    let mut output = Vec::new();
    let error = cmd_cook_with_output(
        Some(recipe_path.to_str().unwrap()),
        None,
        output_dir.to_str().unwrap(),
        source_cache.to_str().unwrap(),
        None,
        false,
        false,
        false,
        false,
        true,
        true,
        false,
        true,
        &mut output,
    )
    .await
    .unwrap_err();

    let rendered = String::from_utf8(output).unwrap();
    assert!(format!("{error:#}").contains("--no-isolation conflicts"));
    assert!(rendered.trim_start().starts_with('{'), "{rendered}");
    assert!(!rendered.contains("Reading recipe:"));
    let value: serde_json::Value = serde_json::from_str(&rendered).expect("valid error json");
    assert_eq!(value["status"], "failed");
    assert_eq!(value["diagnostics"][0]["code"], "cook-failed");
}

#[tokio::test]
async fn cook_validate_only_explain_prints_trace_for_inferred_recipe_without_building() {
    let temp = tempfile::tempdir().unwrap();
    let source_tree = temp.path().join("cargo-source");
    let output_dir = temp.path().join("out");
    let source_cache = temp.path().join("sources");
    write_cargo_source_tree(&source_tree, "validate-demo");

    let mut output = Vec::new();
    cmd_cook_with_output(
        Some(source_tree.to_str().unwrap()),
        None,
        output_dir.to_str().unwrap(),
        source_cache.to_str().unwrap(),
        None,
        false,
        true,
        false,
        true,
        false,
        false,
        false,
        false,
        &mut output,
    )
    .await
    .unwrap();

    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("Inference trace:"), "{output}");
    assert!(output.contains("Recipe validation passed"), "{output}");
    assert!(
        !output_dir.exists(),
        "validate-only inference should not create build output"
    );
}

#[tokio::test]
async fn cook_fetch_only_inferred_local_source_reports_no_remote_fetch() {
    let temp = tempfile::tempdir().unwrap();
    let source_tree = temp.path().join("cargo-source");
    let output_dir = temp.path().join("out");
    let source_cache = temp.path().join("sources");
    write_cargo_source_tree(&source_tree, "fetch-demo");

    let mut output = Vec::new();
    cmd_cook_with_output(
        Some(source_tree.to_str().unwrap()),
        None,
        output_dir.to_str().unwrap(),
        source_cache.to_str().unwrap(),
        None,
        false,
        false,
        true,
        false,
        false,
        false,
        false,
        false,
        &mut output,
    )
    .await
    .unwrap();

    let output = String::from_utf8(output).unwrap();
    assert!(
        output.contains("No remote source fetch is required"),
        "{output}"
    );
    assert!(
        !output_dir.exists(),
        "fetch-only local inference should not build package output"
    );
}

#[tokio::test]
async fn cook_hermetic_requires_hermetic_config_before_planning() {
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
        false,
        true,
        false,
    )
    .await
    .unwrap_err();
    let error = format!("{error:#}");

    assert!(error.contains("hermetic config"), "{error}");
    assert!(
        !error.contains("Hermetic cook/publish is an M2 feature"),
        "hermetic cook should no longer use the old reserved-feature rejection: {error}"
    );
    assert!(
        !output_dir.join("local-1.0-1.ccs").exists(),
        "hermetic planning failure should not write a package"
    );
}

#[tokio::test]
async fn cook_isolated_fails_closed_without_hermetic_config() {
    let temp = tempfile::tempdir().unwrap();
    let recipe_path = temp.path().join("recipe.toml");
    let output_dir = temp.path().join("out");
    let source_cache = temp.path().join("sources");
    write_local_recipe(&recipe_path);

    let mut output = Vec::new();
    let error = cmd_cook_with_output(
        Some(recipe_path.to_str().unwrap()),
        None,
        output_dir.to_str().unwrap(),
        source_cache.to_str().unwrap(),
        None,
        false,
        false,
        false,
        false,
        true,
        false,
        false,
        false,
        &mut output,
    )
    .await
    .unwrap_err();
    let error = format!("{error:#}");
    let output = String::from_utf8(output).unwrap();

    assert!(error.contains("hermetic config"), "{error}");
    assert!(
        !output.contains("attested"),
        "M2a cook output must not claim attestation before M2b: {output}"
    );
    assert!(
        !output.contains("Cooking with"),
        "missing hermetic config should fail before cooking starts: {output}"
    );
    assert!(
        !output_dir.join("local-1.0-1.ccs").exists(),
        "hermetic config failure should not write a package"
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
        false,
        true,
        false,
        false,
    )
    .await
    .unwrap();

    for provenance in [
        cooked_manifest_provenance(&default_output, "local", "1.0"),
        cooked_manifest_provenance(&compat_output, "local", "1.0"),
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
        false,
        true,
        true,
        false,
        false,
    )
    .await
    .unwrap_err();

    assert!(
        error.to_string().contains("conflict"),
        "--isolated and --no-isolation conflict should be explicit: {error:#}"
    );
}
