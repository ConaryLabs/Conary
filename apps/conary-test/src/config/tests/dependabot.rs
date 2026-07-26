// conary-test/src/config/tests/dependabot.rs

use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize)]
struct DependabotConfig {
    version: u8,
    updates: Vec<DependabotUpdate>,
}

#[derive(Debug, Deserialize)]
struct DependabotUpdate {
    #[serde(rename = "package-ecosystem")]
    package_ecosystem: String,
    #[serde(default)]
    directory: Option<String>,
    #[serde(default)]
    directories: Vec<String>,
    schedule: DependabotSchedule,
    #[serde(rename = "target-branch", default)]
    target_branch: Option<String>,
    #[serde(default)]
    groups: BTreeMap<String, DependabotGroup>,
}

#[derive(Debug, Deserialize)]
struct DependabotSchedule {
    interval: String,
}

#[derive(Debug, Deserialize)]
struct DependabotGroup {
    #[serde(rename = "applies-to", default)]
    applies_to: Option<String>,
    #[serde(default)]
    patterns: Vec<String>,
    #[serde(rename = "group-by", default)]
    group_by: Option<String>,
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load_dependabot_config(root: &Path) -> DependabotConfig {
    let path = root.join(".github/dependabot.yml");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_yaml::from_str(&source)
        .unwrap_or_else(|error| panic!("parse {} as typed YAML: {error}", path.display()))
}

fn top_level_npm_projects(root: &Path) -> BTreeSet<String> {
    std::fs::read_dir(root)
        .unwrap_or_else(|error| panic!("read repository root {}: {error}", root.display()))
        .map(|entry| entry.expect("read repository entry"))
        .filter(|entry| {
            entry
                .file_type()
                .expect("read repository entry type")
                .is_dir()
        })
        .filter_map(|entry| {
            let path = entry.path();
            (path.join("package.json").is_file() && path.join("package-lock.json").is_file()).then(
                || {
                    let directory = entry
                        .file_name()
                        .into_string()
                        .expect("npm project directory name must be UTF-8");
                    format!("/{directory}")
                },
            )
        })
        .collect()
}

#[test]
fn dependabot_npm_policy_covers_every_top_level_project() {
    let root = repository_root();
    let config = load_dependabot_config(&root);

    assert_eq!(config.version, 2, "Dependabot config must use version 2");

    let npm_updates = config
        .updates
        .iter()
        .filter(|update| update.package_ecosystem == "npm")
        .collect::<Vec<_>>();
    assert_eq!(
        npm_updates.len(),
        1,
        "Dependabot must have exactly one npm update block so cross-directory grouping stays effective"
    );

    let npm = npm_updates[0];
    assert_eq!(
        npm.directory, None,
        "the npm update block must use `directories`, not a single `directory`"
    );
    assert_eq!(
        npm.directories.iter().cloned().collect::<BTreeSet<_>>(),
        top_level_npm_projects(&root),
        "Dependabot npm directories must cover every top-level project with package.json and package-lock.json"
    );
    assert_eq!(
        npm.schedule.interval, "weekly",
        "routine frontend dependency updates must run weekly"
    );
    assert_eq!(
        npm.target_branch, None,
        "target-branch disables security updates for the ecosystem and must stay unset"
    );

    let security_groups = npm
        .groups
        .values()
        .filter(|group| group.applies_to.as_deref() == Some("security-updates"))
        .collect::<Vec<_>>();
    assert_eq!(
        security_groups.len(),
        1,
        "the npm policy must define exactly one security-update group"
    );
    assert!(
        security_groups[0]
            .patterns
            .iter()
            .any(|pattern| pattern == "*"),
        "the security-update group must cover every npm dependency"
    );

    let version_groups = npm
        .groups
        .values()
        .filter(|group| {
            group.applies_to.as_deref().unwrap_or("version-updates") == "version-updates"
                && group.group_by.as_deref() == Some("dependency-name")
        })
        .count();
    assert_eq!(
        version_groups, 1,
        "routine updates must be grouped by dependency name across npm projects"
    );
}
