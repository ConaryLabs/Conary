// conary-test/src/deploy/status.rs

use crate::deploy::plan::{RolloutPlan, RolloutSource, RolloutTarget};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutSourceKind {
    GitRef,
    LocalSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutTargetKind {
    Unit,
    Group,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RolloutProvenance {
    pub source_kind: RolloutSourceKind,
    pub requested_ref: Option<String>,
    pub resolved_commit: String,
    pub target_kind: RolloutTargetKind,
    pub rollout_name: String,
    pub units: Vec<String>,
    pub deployed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RolloutStatus {
    pub source_kind: RolloutSourceKind,
    pub requested_ref: Option<String>,
    pub resolved_commit: String,
    pub target_kind: RolloutTargetKind,
    pub rollout_name: String,
    pub units: Vec<String>,
    pub deployed_at: String,
    pub drifted: bool,
    pub binary_matches_rollout: bool,
    pub checkout_matches_rollout: bool,
}

impl RolloutProvenance {
    pub fn from_plan(
        plan: &RolloutPlan,
        resolved_commit: impl Into<String>,
        deployed_at: DateTime<Utc>,
    ) -> Self {
        let (source_kind, requested_ref) = match &plan.source {
            RolloutSource::GitRef { requested_ref } => {
                (RolloutSourceKind::GitRef, Some(requested_ref.clone()))
            }
            RolloutSource::PathSnapshot { .. } => (RolloutSourceKind::LocalSnapshot, None),
        };

        let (target_kind, rollout_name) = match &plan.target {
            RolloutTarget::Unit(name) => (RolloutTargetKind::Unit, name.clone()),
            RolloutTarget::Group(name) => (RolloutTargetKind::Group, name.clone()),
        };

        Self {
            source_kind,
            requested_ref,
            resolved_commit: resolved_commit.into(),
            target_kind,
            rollout_name,
            units: plan.units.iter().map(|unit| unit.name.clone()).collect(),
            deployed_at: deployed_at.to_rfc3339(),
        }
    }
}

pub fn evaluate_rollout_status(
    rollout: &RolloutProvenance,
    running_binary_commit: Option<&str>,
    checkout_commit: Option<&str>,
) -> RolloutStatus {
    let binary_matches_rollout = running_binary_commit == Some(rollout.resolved_commit.as_str());
    let checkout_matches_rollout = checkout_commit == Some(rollout.resolved_commit.as_str());

    RolloutStatus {
        source_kind: rollout.source_kind.clone(),
        requested_ref: rollout.requested_ref.clone(),
        resolved_commit: rollout.resolved_commit.clone(),
        target_kind: rollout.target_kind.clone(),
        rollout_name: rollout.rollout_name.clone(),
        units: rollout.units.clone(),
        deployed_at: rollout.deployed_at.clone(),
        drifted: !(binary_matches_rollout && checkout_matches_rollout),
        binary_matches_rollout,
        checkout_matches_rollout,
    }
}

pub fn load_rollout_provenance(path: &Path) -> Result<Option<RolloutProvenance>> {
    if !path.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read rollout provenance `{}`", path.display()))?;
    let rollout = serde_json::from_str(&contents).with_context(|| {
        format!(
            "failed to parse rollout provenance JSON `{}`",
            path.display()
        )
    })?;
    Ok(Some(rollout))
}

pub fn write_rollout_provenance(path: &Path, rollout: &RolloutProvenance) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create rollout provenance directory `{}`",
                parent.display()
            )
        })?;
    }

    let temp_path = temporary_rollout_path(path);
    let json = serde_json::to_vec_pretty(rollout).context("failed to serialize rollout metadata")?;
    fs::write(&temp_path, json).with_context(|| {
        format!(
            "failed to write temporary rollout provenance `{}`",
            temp_path.display()
        )
    })?;
    fs::rename(&temp_path, path).with_context(|| {
        format!(
            "failed to atomically replace rollout provenance `{}`",
            path.display()
        )
    })?;

    Ok(())
}

fn temporary_rollout_path(path: &Path) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    let file_name = path
        .file_name()
        .and_then(|file| file.to_str())
        .unwrap_or("forge-rollout.json");
    path.with_file_name(format!("{file_name}.tmp-{unique}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy::manifest::load_rollout_manifest_from_str;
    use crate::deploy::plan::{RolloutPlanRequest, build_rollout_plan};
    use std::fs;

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

    fn unique_temp_root(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("conary-test-rollout-status-{label}-{unique}"))
    }

    fn sample_plan() -> RolloutPlan {
        let manifest = load_rollout_manifest_from_str(VALID_MANIFEST).expect("manifest parses");
        build_rollout_plan(
            &manifest,
            RolloutPlanRequest {
                unit: None,
                group: Some("all_forge_tooling".to_string()),
                git_ref: Some("main".to_string()),
                path: None,
            },
        )
        .expect("plan builds")
    }

    #[test]
    fn writes_and_reads_last_successful_rollout_metadata() {
        let temp_root = unique_temp_root("write-read");
        fs::create_dir_all(&temp_root).expect("create temp root");
        let path = temp_root.join("forge-rollout.json");
        let rollout = RolloutProvenance::from_plan(
            &sample_plan(),
            "6533e5ddcafe".to_string(),
            DateTime::parse_from_rfc3339("2026-04-09T18:22:00Z")
                .expect("timestamp parses")
                .with_timezone(&Utc),
        );

        write_rollout_provenance(&path, &rollout).expect("write succeeds");
        let loaded = load_rollout_provenance(&path)
            .expect("read succeeds")
            .expect("rollout exists");

        assert_eq!(loaded, rollout);

        fs::remove_dir_all(temp_root).expect("cleanup");
    }

    #[test]
    fn metadata_contains_rollout_identity_fields() {
        let rollout = RolloutProvenance::from_plan(
            &sample_plan(),
            "6533e5ddcafebabe".to_string(),
            DateTime::parse_from_rfc3339("2026-04-09T19:00:00Z")
                .expect("timestamp parses")
                .with_timezone(&Utc),
        );

        assert_eq!(rollout.source_kind, RolloutSourceKind::GitRef);
        assert_eq!(rollout.requested_ref.as_deref(), Some("main"));
        assert_eq!(rollout.resolved_commit, "6533e5ddcafebabe");
        assert_eq!(rollout.target_kind, RolloutTargetKind::Group);
        assert_eq!(rollout.rollout_name, "all_forge_tooling");
        assert_eq!(rollout.units, vec!["conary_test", "conary"]);
        assert_eq!(rollout.deployed_at, "2026-04-09T19:00:00+00:00");
    }

    #[test]
    fn failed_write_does_not_overwrite_previous_last_successful_rollout() {
        let temp_root = unique_temp_root("failed-overwrite");
        fs::create_dir_all(&temp_root).expect("create temp root");
        let path = temp_root.join("forge-rollout.json");
        let original = RolloutProvenance::from_plan(
            &sample_plan(),
            "before123".to_string(),
            DateTime::parse_from_rfc3339("2026-04-09T20:00:00Z")
                .expect("timestamp parses")
                .with_timezone(&Utc),
        );

        write_rollout_provenance(&path, &original).expect("seed write succeeds");

        let permissions = fs::metadata(&temp_root).expect("metadata").permissions();
        let mut read_only_permissions = permissions.clone();
        read_only_permissions.set_readonly(true);
        fs::set_permissions(&temp_root, read_only_permissions).expect("set readonly");

        let replacement = RolloutProvenance::from_plan(
            &sample_plan(),
            "after456".to_string(),
            DateTime::parse_from_rfc3339("2026-04-09T21:00:00Z")
                .expect("timestamp parses")
                .with_timezone(&Utc),
        );
        let error = write_rollout_provenance(&path, &replacement).expect_err("write should fail");
        assert!(error.to_string().contains("temporary rollout provenance"));

        fs::set_permissions(&temp_root, permissions).expect("restore permissions");

        let loaded = load_rollout_provenance(&path)
            .expect("read succeeds")
            .expect("rollout exists");
        assert_eq!(loaded, original);

        fs::remove_dir_all(temp_root).expect("cleanup");
    }

    #[test]
    fn rollout_status_has_no_drift_when_binary_and_checkout_match() {
        let rollout = RolloutProvenance::from_plan(
            &sample_plan(),
            "6533e5ddcafebabe".to_string(),
            DateTime::parse_from_rfc3339("2026-04-09T19:00:00Z")
                .expect("timestamp parses")
                .with_timezone(&Utc),
        );

        let status = evaluate_rollout_status(
            &rollout,
            Some("6533e5ddcafebabe"),
            Some("6533e5ddcafebabe"),
        );

        assert!(!status.drifted);
        assert!(status.binary_matches_rollout);
        assert!(status.checkout_matches_rollout);
    }

    #[test]
    fn rollout_status_flags_binary_drift() {
        let rollout = RolloutProvenance::from_plan(
            &sample_plan(),
            "6533e5ddcafebabe".to_string(),
            DateTime::parse_from_rfc3339("2026-04-09T19:00:00Z")
                .expect("timestamp parses")
                .with_timezone(&Utc),
        );

        let status = evaluate_rollout_status(
            &rollout,
            Some("different-binary"),
            Some("6533e5ddcafebabe"),
        );

        assert!(status.drifted);
        assert!(!status.binary_matches_rollout);
        assert!(status.checkout_matches_rollout);
    }

    #[test]
    fn rollout_status_flags_checkout_drift() {
        let rollout = RolloutProvenance::from_plan(
            &sample_plan(),
            "6533e5ddcafebabe".to_string(),
            DateTime::parse_from_rfc3339("2026-04-09T19:00:00Z")
                .expect("timestamp parses")
                .with_timezone(&Utc),
        );

        let status = evaluate_rollout_status(
            &rollout,
            Some("6533e5ddcafebabe"),
            Some("different-checkout"),
        );

        assert!(status.drifted);
        assert!(status.binary_matches_rollout);
        assert!(!status.checkout_matches_rollout);
    }
}
