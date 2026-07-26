// apps/conary/src/commands/cook/tests.rs

use super::*;
use conary_core::ccs::verify::verify_package;
use conary_core::ccs::{CcsPackage, SigningKeyPair, TrustPolicy};
use conary_core::recipe::SourceDownloadPolicy;

fn write_local_recipe(recipe_path: &Path) {
    std::fs::write(
        recipe_path,
        r#"
[package]
name = "local"
version = "1.0.0"

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
version = "1.0.0"

[source]
path = "."

[build]
install = "mkdir -p %(destdir)s/usr/share/local && printf cooked > %(destdir)s/usr/share/local/output.txt"
"#,
    )
    .unwrap();
}

fn write_bare_cargo_source(root: &Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"bare-source\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
}

fn write_test_signing_key(root: &Path) -> PathBuf {
    let private_path = root.join("test-signing.private");
    let public_path = root.join("test-signing.public");
    SigningKeyPair::generate()
        .with_key_id("cook-test")
        .save_to_files(&private_path, &public_path)
        .unwrap();
    private_path
}

fn cooked_manifest_provenance(
    output_dir: &Path,
    package_name: &str,
    version: &str,
    signing_key_path: &Path,
) -> conary_core::ccs::manifest::ManifestProvenance {
    let package_path = output_dir.join(format!("{package_name}-{version}-1.ccs"));
    let signer = SigningKeyPair::load_from_file(signing_key_path).unwrap();
    let verified = verify_package(
        &package_path,
        &TrustPolicy::strict(vec![signer.public_key_base64()]),
    )
    .unwrap();
    let package =
        CcsPackage::from_verified_archive(&package_path.to_string_lossy(), &verified).unwrap();
    package.manifest().provenance.clone().unwrap()
}

#[test]
fn recipe_source_base_dir_uses_recipe_parent() {
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
        operation_id: "watch-1".to_string(),
        source_policy: WatchCookSourcePolicy::Refresh,
        signing_key_path: None,
    };
    assert_eq!(
        watch_source_download_policy_override(&options),
        Some(SourceDownloadPolicy::OfflineCacheOnly)
    );
}

#[test]
fn watch_non_hermetic_and_initial_cooks_preserve_source_policy() {
    for (isolated, source_policy) in [
        (false, WatchCookSourcePolicy::Refresh),
        (true, WatchCookSourcePolicy::Initial),
    ] {
        let options = CookForTryWatchOptions {
            target: Some("."),
            recipe: None,
            output_dir: "dist",
            source_cache: "sources",
            jobs: None,
            keep_builddir: false,
            isolated,
            source_policy,
            operation_id: "watch-1".to_string(),
            signing_key_path: None,
        };
        assert_eq!(watch_source_download_policy_override(&options), None);
    }
}

#[test]
fn foreign_package_routing_rejects_misleading_extension() {
    let temp = tempfile::Builder::new().suffix(".rpm").tempfile().unwrap();
    std::fs::write(temp.path(), b"not an rpm").unwrap();
    assert_eq!(foreign_package_format(temp.path()), None);
}

#[test]
fn host_iteration_env_pins_cargo_target_dir_to_recipe_local_target() {
    let mut config = KitchenConfig::default();
    add_host_iteration_env(&mut config);
    assert!(
        config
            .extra_env
            .iter()
            .any(|(key, value)| key == "CARGO_TARGET_DIR" && value == "target")
    );
}

#[test]
fn explicit_recipe_flag_wins_over_positional_target() {
    let temp = tempfile::tempdir().unwrap();
    let recipe_path = temp.path().join("explicit.toml");
    write_local_recipe(&recipe_path);
    let resolved = resolve_cook_input(
        Some("https://example.invalid/source.tar.gz"),
        Some(recipe_path.to_str().unwrap()),
    )
    .unwrap();
    assert_eq!(resolved.recipe_path, recipe_path.canonicalize().unwrap());
    assert_eq!(resolved.recipe.package.name, "local");
}

#[test]
fn cook_accepts_recipe_file_or_directory_containing_recipe_toml() {
    let temp = tempfile::tempdir().unwrap();
    let recipe_path = temp.path().join("recipe.toml");
    write_local_recipe(&recipe_path);
    let expected = recipe_path.canonicalize().unwrap();

    assert_eq!(
        resolve_recipe_path(Some(recipe_path.to_str().unwrap()), None).unwrap(),
        expected
    );
    assert_eq!(
        resolve_recipe_path(Some(temp.path().to_str().unwrap()), None).unwrap(),
        expected
    );
}

#[test]
fn cook_rejects_bare_source_tree_and_url_inference() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    write_bare_cargo_source(&source);

    let source_error = resolve_cook_input(Some(source.to_str().unwrap()), None).unwrap_err();
    assert!(
        source_error
            .to_string()
            .contains("does not contain recipe.toml")
    );

    let url_error =
        resolve_cook_input(Some("https://example.invalid/source.tar.gz"), None).unwrap_err();
    assert!(
        url_error
            .to_string()
            .contains("requires an explicit recipe file")
    );
}

#[test]
fn missing_default_recipe_is_not_inferred() {
    let temp = tempfile::tempdir().unwrap();
    let error = resolve_recipe_path_from(None, None, temp.path()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("requires an explicit recipe file")
    );
}

#[test]
fn recorded_draft_run_options_preserve_explicit_origin_and_isolation() {
    let options = CookRecordedDraftOptions {
        recipe: PathBuf::from("recorded/demo/recipe.toml"),
        output_dir: PathBuf::from("recorded/demo/dist"),
        source_cache: PathBuf::from("recorded/demo/sources"),
        operation_id: "record-1".to_string(),
        signing_key_path: None,
    };
    let recipe = options.recipe.to_string_lossy().to_string();
    let output_dir = options.output_dir.to_string_lossy().to_string();
    let source_cache = options.source_cache.to_string_lossy().to_string();
    let run = recorded_draft_run_options(&options, &recipe, &output_dir, &source_cache);
    assert!(run.isolated);
    assert_eq!(run.origin_class_override.as_deref(), Some("recorded-draft"));
}

#[tokio::test]
async fn cook_directory_with_recipe_toml_uses_explicit_recipe_provenance() {
    let temp = tempfile::tempdir().unwrap();
    let recipe_dir = temp.path().join("recipe-dir");
    let output_dir = temp.path().join("out");
    let source_cache = temp.path().join("sources");
    let signing_key = write_test_signing_key(temp.path());
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
        Some(signing_key.as_path()),
    )
    .await
    .unwrap();

    let provenance =
        cooked_manifest_provenance(&output_dir, "local", "1.0.0", signing_key.as_path());
    assert_eq!(provenance.origin_class.as_deref(), Some("native-built"));
    assert_eq!(provenance.hardening_level.as_deref(), Some("host"));
}

#[tokio::test]
async fn cook_build_requires_explicit_signing_authority() {
    let temp = tempfile::tempdir().unwrap();
    let recipe_path = temp.path().join("recipe.toml");
    let output_dir = temp.path().join("out");
    write_installing_local_recipe(&recipe_path);

    let mut output = Vec::new();
    let error = cmd_cook_with_output(
        Some(recipe_path.to_str().unwrap()),
        None,
        output_dir.to_str().unwrap(),
        temp.path().join("sources").to_str().unwrap(),
        None,
        false,
        false,
        false,
        false,
        false,
        None,
        &mut output,
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("requires --key <private-key>"));
    assert!(!output_dir.join("local-1.0.0-1.ccs").exists());
}

#[tokio::test]
async fn explicit_custom_recipe_validates_without_building() {
    let temp = tempfile::tempdir().unwrap();
    let recipe_path = temp.path().join("custom.toml");
    let output_dir = temp.path().join("out");
    write_local_recipe(&recipe_path);

    let mut output = Vec::new();
    cmd_cook_with_output(
        Some(recipe_path.to_str().unwrap()),
        None,
        output_dir.to_str().unwrap(),
        temp.path().join("sources").to_str().unwrap(),
        None,
        false,
        true,
        false,
        false,
        false,
        None,
        &mut output,
    )
    .await
    .unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("Reading recipe:"));
    assert!(output.contains("Recipe validation passed"));
    assert!(!output_dir.exists());
}

#[tokio::test]
async fn cook_validate_only_json_has_schema_version_and_summary() {
    let temp = tempfile::tempdir().unwrap();
    let recipe_path = temp.path().join("recipe.toml");
    write_local_recipe(&recipe_path);

    let mut output = Vec::new();
    cmd_cook_with_output(
        Some(recipe_path.to_str().unwrap()),
        None,
        temp.path().join("out").to_str().unwrap(),
        temp.path().join("sources").to_str().unwrap(),
        None,
        false,
        true,
        false,
        false,
        true,
        None,
        &mut output,
    )
    .await
    .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["status"], "succeeded");
    assert_eq!(value["summary"], "Recipe validation passed");
}

#[tokio::test]
async fn cook_isolated_fails_closed_without_hermetic_config() {
    let temp = tempfile::tempdir().unwrap();
    let recipe_path = temp.path().join("recipe.toml");
    let output_dir = temp.path().join("out");
    write_local_recipe(&recipe_path);
    let signing_key = write_test_signing_key(temp.path());

    let mut output = Vec::new();
    let error = cmd_cook_with_output(
        Some(recipe_path.to_str().unwrap()),
        None,
        output_dir.to_str().unwrap(),
        temp.path().join("sources").to_str().unwrap(),
        None,
        false,
        false,
        false,
        true,
        false,
        Some(signing_key.as_path()),
        &mut output,
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("hermetic config"));
    assert!(!output_dir.join("local-1.0.0-1.ccs").exists());
}
