// conary-test/src/deploy/plan.rs

use crate::deploy::manifest::{BuildSpec, RestartSpec, RolloutManifest, VerifyMode};
use anyhow::{Result, bail};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolloutPlanRequest {
    pub unit: Option<String>,
    pub group: Option<String>,
    pub git_ref: Option<String>,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RolloutTarget {
    Unit(String),
    Group(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RolloutSource {
    GitRef { requested_ref: String },
    PathSnapshot { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedUnit {
    pub name: String,
    pub build: BuildSpec,
    pub restart: Option<RestartSpec>,
    pub verify: VerifyMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolloutPlan {
    pub target: RolloutTarget,
    pub source: RolloutSource,
    pub units: Vec<PlannedUnit>,
}

pub fn build_rollout_plan(
    manifest: &RolloutManifest,
    request: RolloutPlanRequest,
) -> Result<RolloutPlan> {
    let target = match (request.unit, request.group) {
        (Some(_), Some(_)) => bail!("exactly one of `--unit` or `--group` is required"),
        (Some(unit), None) => RolloutTarget::Unit(unit),
        (None, Some(group)) => RolloutTarget::Group(group),
        (None, None) => bail!("one of `--unit` or `--group` is required"),
    };

    let source = match (request.git_ref, request.path) {
        (Some(_), Some(_)) => bail!("`--ref` and `--path` are mutually exclusive"),
        (Some(requested_ref), None) => RolloutSource::GitRef { requested_ref },
        (None, Some(path)) => RolloutSource::PathSnapshot { path },
        (None, None) => bail!("one of `--ref` or `--path` is required"),
    };

    let unit_names = match &target {
        RolloutTarget::Unit(unit_name) => vec![unit_name.clone()],
        RolloutTarget::Group(group_name) => manifest
            .groups
            .get(group_name)
            .ok_or_else(|| anyhow::anyhow!("unknown rollout group `{group_name}`"))?
            .units
            .clone(),
    };

    let mut units = Vec::with_capacity(unit_names.len());
    for unit_name in unit_names {
        let unit = manifest
            .units
            .get(&unit_name)
            .ok_or_else(|| anyhow::anyhow!("unknown rollout unit `{unit_name}`"))?;
        units.push(PlannedUnit {
            name: unit_name,
            build: unit.build.clone(),
            restart: unit.restart.clone(),
            verify: unit.verify.clone(),
        });
    }

    Ok(RolloutPlan {
        target,
        source,
        units,
    })
}

#[cfg(test)]
mod tests {
    use crate::deploy::manifest::load_rollout_manifest_from_str;

    use super::*;

    const VALID_MANIFEST: &str = r#"
[units.conary_test]
build = { cargo_package = "conary-test" }
restart = { systemd_user_unit = "conary-test.service" }
verify = "forge_smoke"

[units.conary]
build = { cargo_package = "conary" }
verify = "none"

[groups.control_plane]
units = ["conary_test"]

[groups.all_forge_tooling]
units = ["conary_test", "conary"]
"#;

    #[test]
    fn unit_target_resolves_to_exactly_one_manifest_unit() {
        let manifest = load_rollout_manifest_from_str(VALID_MANIFEST).expect("manifest parses");
        let request = RolloutPlanRequest {
            unit: Some("conary_test".to_string()),
            group: None,
            git_ref: Some("main".to_string()),
            path: None,
        };

        let plan = build_rollout_plan(&manifest, request).expect("plan builds");
        assert_eq!(plan.units.len(), 1);
        assert_eq!(plan.units[0].name, "conary_test");
    }

    #[test]
    fn group_target_expands_in_manifest_order() {
        let manifest = load_rollout_manifest_from_str(VALID_MANIFEST).expect("manifest parses");
        let request = RolloutPlanRequest {
            unit: None,
            group: Some("all_forge_tooling".to_string()),
            git_ref: Some("78e7194e".to_string()),
            path: None,
        };

        let plan = build_rollout_plan(&manifest, request).expect("plan builds");
        let names: Vec<_> = plan.units.iter().map(|unit| unit.name.as_str()).collect();
        assert_eq!(names, vec!["conary_test", "conary"]);
    }

    #[test]
    fn ref_and_path_are_mutually_exclusive_plan_inputs() {
        let manifest = load_rollout_manifest_from_str(VALID_MANIFEST).expect("manifest parses");
        let request = RolloutPlanRequest {
            unit: Some("conary_test".to_string()),
            group: None,
            git_ref: Some("main".to_string()),
            path: Some(PathBuf::from("/tmp/forge")),
        };

        let error = build_rollout_plan(&manifest, request).expect_err("mixed source rejected");
        assert!(error.to_string().contains("--ref"));
        assert!(error.to_string().contains("--path"));
    }

    #[test]
    fn execution_plan_is_materialized_before_any_checkout_mutation() {
        let mut manifest = load_rollout_manifest_from_str(VALID_MANIFEST).expect("manifest parses");
        let request = RolloutPlanRequest {
            unit: None,
            group: Some("all_forge_tooling".to_string()),
            git_ref: Some("main".to_string()),
            path: None,
        };

        let plan = build_rollout_plan(&manifest, request).expect("plan builds");
        manifest.groups.remove("all_forge_tooling");
        manifest.units.clear();

        let names: Vec<_> = plan.units.iter().map(|unit| unit.name.as_str()).collect();
        assert_eq!(names, vec!["conary_test", "conary"]);
    }
}
