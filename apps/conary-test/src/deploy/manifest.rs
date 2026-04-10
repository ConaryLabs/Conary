// conary-test/src/deploy/manifest.rs

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RolloutManifest {
    pub units: BTreeMap<String, RolloutUnit>,
    pub groups: BTreeMap<String, RolloutGroup>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RolloutUnit {
    pub build: BuildSpec,
    pub restart: Option<RestartSpec>,
    pub verify: VerifyMode,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BuildSpec {
    pub cargo_package: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RestartSpec {
    pub systemd_user_unit: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RolloutGroup {
    pub units: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerifyMode {
    None,
    ForgeSmoke,
}

#[derive(Debug, Clone, Deserialize)]
struct RawRolloutManifest {
    #[serde(default)]
    units: BTreeMap<String, RawRolloutUnit>,
    #[serde(default)]
    groups: BTreeMap<String, RolloutGroup>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawRolloutUnit {
    build: BuildSpec,
    restart: Option<RawRestartSpec>,
    verify: String,
}

#[derive(Debug, Clone, Deserialize)]
struct RawRestartSpec {
    systemd_user_unit: Option<String>,
}

pub fn load_rollout_manifest_from_file(path: &Path) -> Result<RolloutManifest> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    load_rollout_manifest_from_str(&content)
}

pub fn load_rollout_manifest_from_str(input: &str) -> Result<RolloutManifest> {
    let raw: RawRolloutManifest =
        toml::from_str(input).context("failed to parse Forge rollout manifest TOML")?;

    let mut units = BTreeMap::new();
    for (unit_name, raw_unit) in raw.units {
        let verify = match raw_unit.verify.as_str() {
            "none" => VerifyMode::None,
            "forge_smoke" => VerifyMode::ForgeSmoke,
            other => bail!("unit `{unit_name}` uses unsupported verify mode `{other}`"),
        };

        let restart = match raw_unit.restart {
            Some(raw_restart) => Some(RestartSpec {
                systemd_user_unit: raw_restart.systemd_user_unit.ok_or_else(|| {
                    anyhow::anyhow!(
                        "unit `{unit_name}` restart metadata must include `systemd_user_unit`"
                    )
                })?,
            }),
            None => None,
        };

        units.insert(
            unit_name,
            RolloutUnit {
                build: raw_unit.build,
                restart,
                verify,
            },
        );
    }

    let manifest = RolloutManifest {
        units,
        groups: raw.groups,
    };

    for (group_name, group) in &manifest.groups {
        for unit_name in &group.units {
            if !manifest.units.contains_key(unit_name) {
                bail!("group `{group_name}` references unknown unit `{unit_name}`");
            }
        }
    }

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::VerifyMode;
    use super::*;

    const VALID_MANIFEST: &str = r#"
[units.conary_test]
build = { cargo_package = "conary-test" }
restart = { systemd_user_unit = "conary-test.service" }
verify = "forge_smoke"

[groups.control_plane]
units = ["conary_test"]
"#;

    #[test]
    fn parses_singleton_units_and_groups() {
        let manifest = load_rollout_manifest_from_str(VALID_MANIFEST).expect("manifest parses");
        let unit = manifest.units.get("conary_test").expect("unit exists");
        assert_eq!(unit.build.cargo_package, "conary-test");
        assert_eq!(unit.verify, VerifyMode::ForgeSmoke);
        assert_eq!(
            unit.restart
                .as_ref()
                .expect("restart exists")
                .systemd_user_unit,
            "conary-test.service"
        );
        assert_eq!(
            manifest
                .groups
                .get("control_plane")
                .expect("group exists")
                .units,
            vec!["conary_test".to_string()]
        );
    }

    #[test]
    fn rejects_unknown_group_unit_references() {
        let manifest = r#"
[units.conary_test]
build = { cargo_package = "conary-test" }
verify = "forge_smoke"

[groups.control_plane]
units = ["missing_unit"]
"#;

        let error = load_rollout_manifest_from_str(manifest).expect_err("unknown unit rejected");
        assert!(error.to_string().contains("missing_unit"));
    }

    #[test]
    fn rejects_unsupported_verification_modes() {
        let manifest = r#"
[units.conary_test]
build = { cargo_package = "conary-test" }
verify = "custom_hook"
"#;

        let error =
            load_rollout_manifest_from_str(manifest).expect_err("unsupported verify rejected");
        assert!(error.to_string().contains("custom_hook"));
    }

    #[test]
    fn rejects_invalid_restart_metadata() {
        let manifest = r#"
[units.conary_test]
build = { cargo_package = "conary-test" }
restart = {}
verify = "forge_smoke"
"#;

        let error = load_rollout_manifest_from_str(manifest).expect_err("invalid restart rejected");
        assert!(error.to_string().contains("systemd_user_unit"));
    }
}
