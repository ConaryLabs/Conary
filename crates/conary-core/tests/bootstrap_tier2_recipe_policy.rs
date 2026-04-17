// conary-core/tests/bootstrap_tier2_recipe_policy.rs

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use conary_core::recipe::{Recipe, parse_recipe_file};
use toml::Value;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|dir| dir.join("recipes/tier2/conary.toml").is_file())
        .expect("workspace root not found from crate manifest ancestors")
}

fn tier2_dir() -> PathBuf {
    workspace_root().join("recipes/tier2")
}

fn versions_toml() -> PathBuf {
    workspace_root().join("recipes/versions.toml")
}

fn expected_tier2_recipe_names() -> BTreeSet<&'static str> {
    [
        "conary",
        "curl",
        "linux-pam",
        "make-ca",
        "nano",
        "openssh",
        "rust",
        "sudo",
    ]
    .into_iter()
    .collect()
}

fn expected_tier2_versions() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        ("linux-pam", "1.7.2"),
        ("openssh", "10.3p1"),
        ("make-ca", "1.16.1"),
        ("curl", "8.19.0"),
        ("sudo", "1.9.17p2"),
        ("nano", "9.0"),
        ("rust", "1.94.0"),
    ])
}

fn load_tier2_recipes() -> BTreeMap<String, Recipe> {
    let mut recipes = BTreeMap::new();

    for entry in fs::read_dir(tier2_dir()).expect("failed to read recipes/tier2") {
        let entry = entry.expect("failed to read recipes/tier2 entry");
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }

        let recipe = parse_recipe_file(&path)
            .unwrap_or_else(|err| panic!("failed to parse {}: {err}", path.display()));
        recipes.insert(recipe.package.name.clone(), recipe);
    }

    recipes
}

fn load_tier2_versions() -> BTreeMap<String, String> {
    let content =
        fs::read_to_string(versions_toml()).expect("failed to read recipes/versions.toml");
    let parsed: Value =
        toml::from_str(&content).expect("recipes/versions.toml must parse as TOML");
    let tier2 = parsed
        .get("tier2")
        .and_then(Value::as_table)
        .expect("recipes/versions.toml must contain a [tier2] table");

    tier2.iter()
        .map(|(name, value)| {
            let version = value
                .as_str()
                .unwrap_or_else(|| panic!("tier2 version for {name} must be a string"));
            (name.clone(), version.to_string())
        })
        .collect()
}

#[test]
fn tier2_recipe_set_matches_self_hosting_milestone() {
    let recipes = load_tier2_recipes();
    let actual: BTreeSet<&str> = recipes.keys().map(String::as_str).collect();
    let expected = expected_tier2_recipe_names();

    assert_eq!(
        actual, expected,
        "recipes/tier2 must contain exactly the approved self-hosting milestone package set"
    );
}

#[test]
fn tier2_required_recipes_use_repo_owned_sha256_checksums() {
    let recipes = load_tier2_recipes();

    for recipe_name in expected_tier2_recipe_names() {
        let recipe = recipes
            .get(recipe_name)
            .unwrap_or_else(|| panic!("missing Tier 2 recipe {recipe_name}"));

        if recipe_name == "conary" {
            continue;
        }

        assert!(
            recipe.source.checksum.starts_with("sha256:"),
            "Tier 2 recipe {recipe_name} must use a repo-owned sha256 checksum, found {}",
            recipe.source.checksum
        );
        assert!(
            !recipe.source.checksum.contains("VERIFY_BEFORE_BUILD"),
            "Tier 2 recipe {recipe_name} still contains a placeholder checksum"
        );
    }
}

#[test]
fn conary_recipe_keeps_staged_workspace_contract() {
    let recipes = load_tier2_recipes();
    let recipe = recipes
        .get("conary")
        .expect("missing Tier 2 recipe conary");

    assert!(
        recipe.source.archive.is_empty(),
        "conary Tier 2 recipe must not fetch a remote archive"
    );
    assert!(
        recipe.source.checksum.is_empty(),
        "conary Tier 2 recipe must not declare a remote checksum"
    );
    assert_eq!(
        recipe.build.requires,
        ["glibc", "openssl", "sqlite", "rust"],
        "conary Tier 2 recipe must declare the staged-source dependency contract"
    );
}

#[test]
fn versions_toml_records_the_approved_tier2_audit() {
    let versions = load_tier2_versions();

    for (package_name, expected_version) in expected_tier2_versions() {
        let actual_version = versions
            .get(package_name)
            .unwrap_or_else(|| panic!("recipes/versions.toml missing [tier2].{package_name}"));
        assert_eq!(
            actual_version, expected_version,
            "recipes/versions.toml drifted for Tier 2 package {package_name}"
        );
    }
}
