// conary-core/src/recipe/hermetic/plan/tests.rs

use super::*;
use crate::recipe::format::{
    BuildSection, LocalSourceSection, PackageSection, Recipe, SourceSection,
};
use crate::recipe::hermetic::{
    BuilderEnvironmentKind, CiMode, HERMETIC_EVIDENCE_SCHEMA, SourceIdentity,
};
use crate::recipe::kitchen::{KitchenConfig, SourceDownloadPolicy};
use crate::recipe::{PatchInfo, PatchSection};
use crate::security::command_risk::CommandRiskSeverity;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const TEST_SYSROOT_IDENTITY: &str =
    "sha256:1111111111111111111111111111111111111111111111111111111111111111";
const TEST_TOOLCHAIN_IDENTITY: &str =
    "sha256:2222222222222222222222222222222222222222222222222222222222222222";

struct RecipeFixture {
    dir: TempDir,
    recipe: Recipe,
    recipe_path: PathBuf,
}

impl RecipeFixture {
    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn recipe_path(&self) -> &Path {
        &self.recipe_path
    }
}

#[test]
fn hermetic_plan_for_local_cargo_project_is_clean() {
    let fixture = cargo_project_with_lock(".");
    let recipe = fixture.recipe.clone();
    let input =
        HermeticBuildInput::explicit_recipe(fixture.path(), fixture.recipe_path(), "sha256:recipe")
            .with_pristine_builder_environment(
                Some(TEST_SYSROOT_IDENTITY),
                Some(TEST_TOOLCHAIN_IDENTITY),
            );

    let plan = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap();

    assert_eq!(plan.evidence.schema_version, HERMETIC_EVIDENCE_SCHEMA);
    assert_eq!(
        plan.evidence.command_risk.highest_severity,
        CommandRiskSeverity::None
    );
    assert_eq!(
        plan.evidence.build_input.builder_environment.kind,
        BuilderEnvironmentKind::Pristine
    );
    assert!(plan.local_files.is_some());
}

#[test]
fn hermetic_plan_apply_sets_kitchen_hermetic_controls() {
    let fixture = cargo_project_with_lock(".");
    let recipe = fixture.recipe.clone();
    let input =
        HermeticBuildInput::explicit_recipe(fixture.path(), fixture.recipe_path(), "sha256:recipe")
            .with_pristine_builder_environment(
                Some(TEST_SYSROOT_IDENTITY),
                Some(TEST_TOOLCHAIN_IDENTITY),
            );
    let plan = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap();
    let mut config = KitchenConfig::default();

    plan.apply_to_kitchen_config(&mut config);

    assert!(config.use_isolation);
    assert!(!config.allow_network);
    assert!(config.pristine_mode);
    assert_eq!(
        config.source_download_policy,
        SourceDownloadPolicy::OfflineCacheOnly
    );
    assert_eq!(config.hermetic_evidence, Some(plan.evidence.clone()));
    assert_eq!(config.hermetic_local_files, plan.local_files);
    assert_eq!(config.reproducibility, Some(plan.reproducibility));
    assert_eq!(
        config.recipe_source_base_dir,
        Some(fixture.path().to_path_buf())
    );
}

#[test]
fn hermetic_plan_records_npm_fetch_as_diagnostic_without_using_it_as_authority() {
    let fixture = npm_project();
    let recipe = fixture.recipe.clone();
    let input =
        HermeticBuildInput::explicit_recipe(fixture.path(), fixture.recipe_path(), "sha256:recipe")
            .with_pristine_builder_environment(
                Some(TEST_SYSROOT_IDENTITY),
                Some(TEST_TOOLCHAIN_IDENTITY),
            );

    let plan = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap();

    assert_eq!(
        plan.evidence.command_risk.highest_severity,
        CommandRiskSeverity::Notice
    );
    assert!(
        plan.evidence
            .command_risk
            .entries
            .iter()
            .any(|entry| entry.command == "npm" && entry.reason_code == "package-manager-fetch")
    );
}

#[test]
fn hermetic_plan_blocks_unlocked_build_dependencies() {
    let fixture = cargo_project_with_lock(".");
    let mut recipe = fixture.recipe.clone();
    recipe.build.makedepends = vec!["openssl-devel".to_string()];
    let input =
        HermeticBuildInput::explicit_recipe(fixture.path(), fixture.recipe_path(), "sha256:recipe")
            .with_pristine_builder_environment(
                Some(TEST_SYSROOT_IDENTITY),
                Some(TEST_TOOLCHAIN_IDENTITY),
            );

    let error = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap_err();

    assert!(error.to_string().contains("build dependency"));
    assert!(error.to_string().contains("content identity"));
}

#[test]
fn hermetic_plan_resolves_source_path_relative_to_recipe_base() {
    let fixture = cargo_project_with_lock("src");
    fs::write(fixture.path().join("README.md"), "outside source root\n").unwrap();
    let input =
        HermeticBuildInput::explicit_recipe(fixture.path(), fixture.recipe_path(), "sha256:recipe")
            .with_pristine_builder_environment(
                Some(TEST_SYSROOT_IDENTITY),
                Some(TEST_TOOLCHAIN_IDENTITY),
            );

    let plan = HermeticBuildPlan::from_recipe(&fixture.recipe, input, CiMode::Off).unwrap();
    let local_files = plan.local_files.as_ref().unwrap();

    assert!(matches!(
        &plan.evidence.build_input.source,
        SourceIdentity::LocalTree { root_display, .. } if root_display.ends_with("/src")
    ));
    assert!(
        local_files
            .iter()
            .any(|file| file.relative_path == Path::new("Cargo.toml"))
    );
    assert!(
        local_files
            .iter()
            .all(|file| !file.relative_path.starts_with(".."))
    );
    assert!(
        local_files
            .iter()
            .all(|file| file.relative_path != Path::new("README.md"))
    );
}

#[test]
fn hermetic_plan_blocks_default_builder_identity() {
    let fixture = cargo_project_with_lock(".");
    let recipe = fixture.recipe.clone();
    let input =
        HermeticBuildInput::explicit_recipe(fixture.path(), fixture.recipe_path(), "sha256:recipe");

    let error = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap_err();

    assert!(error.to_string().contains("builder environment identity"));
    assert!(error.to_string().contains("sha256"));
}

#[test]
fn hermetic_plan_rejects_placeholder_builder_identity() {
    let fixture = cargo_project_with_lock(".");
    let recipe = fixture.recipe.clone();
    let input =
        HermeticBuildInput::explicit_recipe(fixture.path(), fixture.recipe_path(), "sha256:recipe")
            .with_pristine_builder_environment(
                Some("sha256:m2a-pristine-sysroot-test"),
                Some(TEST_TOOLCHAIN_IDENTITY),
            );

    let error = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap_err();

    assert!(error.to_string().contains("builder environment identity"));
    assert!(error.to_string().contains("sha256"));
}

#[test]
fn hermetic_plan_rejects_parent_traversal_local_patch() {
    let fixture = cargo_project_with_lock(".");
    let mut recipe = fixture.recipe.clone();
    recipe.patches = Some(PatchSection {
        files: vec![PatchInfo {
            file: "../outside.patch".to_string(),
            checksum: None,
            strip: 1,
            condition: None,
        }],
    });
    let input =
        HermeticBuildInput::explicit_recipe(fixture.path(), fixture.recipe_path(), "sha256:recipe")
            .with_pristine_builder_environment(
                Some(TEST_SYSROOT_IDENTITY),
                Some(TEST_TOOLCHAIN_IDENTITY),
            );

    let error = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap_err();

    assert!(error.to_string().contains("local patch"));
    assert!(error.to_string().contains("recipe directory"));
}

#[test]
fn hermetic_plan_rejects_absolute_local_patch() {
    let fixture = cargo_project_with_lock(".");
    let mut recipe = fixture.recipe.clone();
    recipe.patches = Some(PatchSection {
        files: vec![PatchInfo {
            file: fixture
                .path()
                .join("local.patch")
                .to_string_lossy()
                .to_string(),
            checksum: None,
            strip: 1,
            condition: None,
        }],
    });
    let input =
        HermeticBuildInput::explicit_recipe(fixture.path(), fixture.recipe_path(), "sha256:recipe")
            .with_pristine_builder_environment(
                Some(TEST_SYSROOT_IDENTITY),
                Some(TEST_TOOLCHAIN_IDENTITY),
            );

    let error = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap_err();

    assert!(error.to_string().contains("local patch"));
    assert!(error.to_string().contains("relative"));
}

#[test]
fn hermetic_plan_records_substituted_local_patch_identity() {
    let fixture = cargo_project_with_lock(".");
    let patch_dir = fixture.path().join("patches");
    fs::create_dir_all(&patch_dir).unwrap();
    let patch_bytes = b"diff --git a/file.txt b/file.txt\n";
    fs::write(patch_dir.join("0.1.0.patch"), patch_bytes).unwrap();
    let mut recipe = fixture.recipe.clone();
    recipe.patches = Some(PatchSection {
        files: vec![PatchInfo {
            file: "patches/%(version)s.patch".to_string(),
            checksum: None,
            strip: 1,
            condition: None,
        }],
    });
    let input =
        HermeticBuildInput::explicit_recipe(fixture.path(), fixture.recipe_path(), "sha256:recipe")
            .with_pristine_builder_environment(
                Some(TEST_SYSROOT_IDENTITY),
                Some(TEST_TOOLCHAIN_IDENTITY),
            );

    let plan = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap();
    let patch = &plan.evidence.build_input.patches[0];

    assert!(patch.path.ends_with("patches/0.1.0.patch"));
    assert_eq!(patch.hash, crate::hash::sha256_prefixed(patch_bytes));
}

fn cargo_project_with_lock(source_path: &str) -> RecipeFixture {
    let dir = tempfile::tempdir().unwrap();
    let source_root = dir.path().join(source_path);
    fs::create_dir_all(source_root.join("src")).unwrap();
    fs::write(
        source_root.join("Cargo.toml"),
        r#"[package]
name = "hello"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(
        source_root.join("Cargo.lock"),
        r#"# This file is automatically @generated by Cargo.
version = 3

[[package]]
name = "hello"
version = "0.1.0"
"#,
    )
    .unwrap();
    fs::write(
        source_root.join("src").join("main.rs"),
        "fn main() { println!(\"hello\"); }\n",
    )
    .unwrap();
    fs::write(dir.path().join("local.patch"), "diff --git a/a b/a\n").unwrap();

    RecipeFixture {
        recipe: recipe_with_local_source(
            source_path,
            Some("cargo build --release --locked --offline"),
            Some("install -Dm755 target/release/hello %(destdir)s/usr/bin/hello"),
        ),
        recipe_path: dir.path().join("recipe.toml"),
        dir,
    }
}

fn npm_project() -> RecipeFixture {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{"scripts":{"build":"node build.js"}}"#,
    )
    .unwrap();
    fs::write(dir.path().join("package-lock.json"), "{}\n").unwrap();
    fs::write(dir.path().join("local.patch"), "diff --git a/a b/a\n").unwrap();

    RecipeFixture {
        recipe: recipe_with_local_source(
            ".",
            Some("npm ci --omit=dev"),
            Some("mkdir -p %(destdir)s/usr/lib/hello && cp -a . %(destdir)s/usr/lib/hello"),
        ),
        recipe_path: dir.path().join("recipe.toml"),
        dir,
    }
}

fn recipe_with_local_source(
    source_path: &str,
    make: Option<&str>,
    install: Option<&str>,
) -> Recipe {
    Recipe {
        package: PackageSection {
            name: "hello".to_string(),
            version: "0.1.0".to_string(),
            release: "1".to_string(),
            summary: None,
            description: None,
            license: None,
            homepage: None,
        },
        source: SourceSection::Local(LocalSourceSection {
            path: PathBuf::from(source_path),
        }),
        build: BuildSection {
            requires: Vec::new(),
            makedepends: Vec::new(),
            configure: None,
            make: make.map(str::to_string),
            install: install.map(str::to_string),
            check: None,
            setup: None,
            post_install: None,
            workdir: None,
            environment: HashMap::new(),
            jobs: None,
            script_file: None,
            stage: None,
        },
        patches: Some(PatchSection {
            files: vec![PatchInfo {
                file: "local.patch".to_string(),
                checksum: None,
                strip: 1,
                condition: None,
            }],
        }),
        cross: None,
        components: None,
        variables: HashMap::new(),
    }
}
