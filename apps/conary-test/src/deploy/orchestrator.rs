// conary-test/src/deploy/orchestrator.rs

use crate::deploy::manifest::VerifyMode;
use crate::deploy::plan::{RolloutPlan, RolloutSource};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::Path;

#[async_trait]
pub trait RolloutExecutor {
    async fn git_fetch(&mut self, repo_dir: &Path, git_ref: &str) -> Result<()>;
    async fn git_checkout(&mut self, repo_dir: &Path, git_ref: &str) -> Result<()>;
    async fn cargo_build_package(&mut self, repo_dir: &Path, package: &str) -> Result<()>;
    async fn restart_systemd_user_unit(&mut self, unit: &str) -> Result<()>;
    async fn verify(&mut self, verify_mode: &str, repo_dir: &Path) -> Result<()>;
    async fn record_success(&mut self, plan: &RolloutPlan) -> Result<()>;
}

pub async fn execute_rollout<E: RolloutExecutor + Send>(
    executor: &mut E,
    plan: &RolloutPlan,
    forge_checkout: &Path,
) -> Result<()> {
    let work_tree = match &plan.source {
        RolloutSource::GitRef { requested_ref } => {
            executor
                .git_fetch(forge_checkout, requested_ref)
                .await
                .with_context(|| format!("git fetch failed for `{requested_ref}`"))?;
            executor
                .git_checkout(forge_checkout, requested_ref)
                .await
                .with_context(|| format!("git checkout failed for `{requested_ref}`"))?;
            forge_checkout
        }
        RolloutSource::PathSnapshot { path } => path.as_path(),
    };

    for unit in &plan.units {
        executor
            .cargo_build_package(work_tree, &unit.build.cargo_package)
            .await
            .with_context(|| format!("cargo_build failed for rollout unit `{}`", unit.name))?;
    }

    for unit in &plan.units {
        if let Some(restart) = &unit.restart {
            executor
                .restart_systemd_user_unit(&restart.systemd_user_unit)
                .await
                .with_context(|| {
                    format!(
                        "restart failed for systemd user unit `{}`",
                        restart.systemd_user_unit
                    )
                })?;
        }
    }

    let mut seen_verify_modes = Vec::new();
    for unit in &plan.units {
        let verify = match unit.verify {
            VerifyMode::None => None,
            VerifyMode::ForgeSmoke => Some("forge_smoke"),
        };

        if let Some(verify_mode) = verify
            && !seen_verify_modes.iter().any(|seen| seen == &verify_mode)
        {
            executor
                .verify(verify_mode, work_tree)
                .await
                .with_context(|| format!("verify failed for `{verify_mode}`"))?;
            seen_verify_modes.push(verify_mode);
        }
    }

    executor
        .record_success(plan)
        .await
        .context("failed to record successful rollout")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::deploy::manifest::load_rollout_manifest_from_str;
    use crate::deploy::plan::{RolloutPlan, RolloutPlanRequest, build_rollout_plan};

    use super::*;
    use anyhow::{Result, bail};
    use std::path::{Path, PathBuf};

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

    #[derive(Default)]
    struct MockExecutor {
        actions: Vec<String>,
        fail_at: Option<String>,
        recorded_success: bool,
    }

    impl MockExecutor {
        fn with_failure(stage: &str) -> Self {
            Self {
                fail_at: Some(stage.to_string()),
                ..Self::default()
            }
        }

        fn maybe_fail(&self, stage: &str) -> Result<()> {
            if self.fail_at.as_deref() == Some(stage) {
                bail!("mock failure at {stage}");
            }
            Ok(())
        }
    }

    #[async_trait]
    impl RolloutExecutor for MockExecutor {
        async fn git_fetch(&mut self, repo_dir: &Path, git_ref: &str) -> Result<()> {
            let stage = format!("git_fetch:{git_ref}@{}", repo_dir.display());
            self.actions.push(stage.clone());
            self.maybe_fail(&stage)
        }

        async fn git_checkout(&mut self, repo_dir: &Path, git_ref: &str) -> Result<()> {
            let stage = format!("git_checkout:{git_ref}@{}", repo_dir.display());
            self.actions.push(stage.clone());
            self.maybe_fail(&stage)
        }

        async fn cargo_build_package(&mut self, repo_dir: &Path, package: &str) -> Result<()> {
            let stage = format!("cargo_build:{package}@{}", repo_dir.display());
            self.actions.push(stage.clone());
            self.maybe_fail(&stage)
        }

        async fn restart_systemd_user_unit(&mut self, unit: &str) -> Result<()> {
            let stage = format!("restart:{unit}");
            self.actions.push(stage.clone());
            self.maybe_fail(&stage)
        }

        async fn verify(&mut self, verify_mode: &str, repo_dir: &Path) -> Result<()> {
            let stage = format!("verify:{verify_mode}@{}", repo_dir.display());
            self.actions.push(stage.clone());
            self.maybe_fail(&stage)
        }

        async fn record_success(&mut self, _plan: &RolloutPlan) -> Result<()> {
            let stage = "record_success".to_string();
            self.actions.push(stage.clone());
            self.recorded_success = true;
            self.maybe_fail(&stage)
        }
    }

    fn build_ref_plan(group: &str) -> RolloutPlan {
        let manifest = load_rollout_manifest_from_str(VALID_MANIFEST).expect("manifest parses");
        build_rollout_plan(
            &manifest,
            RolloutPlanRequest {
                unit: None,
                group: Some(group.to_string()),
                git_ref: Some("main".to_string()),
                path: None,
            },
        )
        .expect("plan builds")
    }

    #[tokio::test]
    async fn executes_successful_plan_in_order() {
        let plan = build_ref_plan("control_plane");
        let mut executor = MockExecutor::default();
        let forge_checkout = PathBuf::from("/home/peter/Conary");

        execute_rollout(&mut executor, &plan, &forge_checkout)
            .await
            .expect("execution succeeds");

        assert_eq!(
            executor.actions,
            vec![
                "git_fetch:main@/home/peter/Conary",
                "git_checkout:main@/home/peter/Conary",
                "cargo_build:conary-test@/home/peter/Conary",
                "restart:conary-test.service",
                "verify:forge_smoke@/home/peter/Conary",
                "record_success",
            ]
        );
        assert!(executor.recorded_success);
    }

    #[tokio::test]
    async fn build_failure_aborts_before_restart_and_verify() {
        let plan = build_ref_plan("control_plane");
        let mut executor =
            MockExecutor::with_failure("cargo_build:conary-test@/home/peter/Conary");
        let forge_checkout = PathBuf::from("/home/peter/Conary");

        let error = execute_rollout(&mut executor, &plan, &forge_checkout)
            .await
            .expect_err("build failure propagates");

        assert!(error.to_string().contains("cargo_build"));
        assert!(!executor.actions.iter().any(|action| action.starts_with("restart:")));
        assert!(!executor.actions.iter().any(|action| action.starts_with("verify:")));
        assert!(!executor.recorded_success);
    }

    #[tokio::test]
    async fn verification_failure_does_not_record_last_successful_rollout() {
        let plan = build_ref_plan("control_plane");
        let mut executor =
            MockExecutor::with_failure("verify:forge_smoke@/home/peter/Conary");
        let forge_checkout = PathBuf::from("/home/peter/Conary");

        let error = execute_rollout(&mut executor, &plan, &forge_checkout)
            .await
            .expect_err("verify failure propagates");

        assert!(error.to_string().contains("verify"));
        assert!(!executor.actions.iter().any(|action| action == "record_success"));
        assert!(!executor.recorded_success);
    }

    #[tokio::test]
    async fn ref_rollout_uses_precomputed_units_without_manifest_access() {
        let plan = build_ref_plan("all_forge_tooling");
        let mut executor = MockExecutor::default();
        let forge_checkout = PathBuf::from("/home/peter/Conary");

        execute_rollout(&mut executor, &plan, &forge_checkout)
            .await
            .expect("execution succeeds");

        let build_actions: Vec<_> = executor
            .actions
            .iter()
            .filter(|action| action.starts_with("cargo_build:"))
            .cloned()
            .collect();
        assert_eq!(
            build_actions,
            vec![
                "cargo_build:conary-test@/home/peter/Conary".to_string(),
                "cargo_build:conary@/home/peter/Conary".to_string(),
            ]
        );
    }
}
