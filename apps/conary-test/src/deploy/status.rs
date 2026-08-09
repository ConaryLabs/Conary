// conary-test/src/deploy/status.rs

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentStatus {
    pub binary: BinaryStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryStatus {
    pub version: String,
    pub git_commit: String,
    pub commit_timestamp: String,
    pub build_timestamp: Option<String>,
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_root(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("conary-test-rollout-status-{label}-{unique}"))
    }

    fn sample_rollout() -> RolloutProvenance {
        RolloutProvenance {
            source_kind: RolloutSourceKind::GitRef,
            requested_ref: Some("main".to_string()),
            resolved_commit: "6533e5ddcafebabe".to_string(),
            target_kind: RolloutTargetKind::Group,
            rollout_name: "control_plane".to_string(),
            units: vec!["conary_test".to_string(), "conary".to_string()],
            deployed_at: "2026-04-09T19:00:00+00:00".to_string(),
        }
    }

    #[test]
    fn loads_last_successful_rollout_metadata() {
        let temp_root = unique_temp_root("load");
        fs::create_dir_all(&temp_root).expect("create temp root");
        let path = temp_root.join("forge-rollout.json");
        let rollout = sample_rollout();
        fs::write(
            &path,
            serde_json::to_vec_pretty(&rollout).expect("serialize succeeds"),
        )
        .expect("write succeeds");

        let loaded = load_rollout_provenance(&path)
            .expect("read succeeds")
            .expect("rollout exists");

        assert_eq!(loaded, rollout);
        fs::remove_dir_all(temp_root).expect("cleanup");
    }

    #[test]
    fn metadata_contains_rollout_identity_fields() {
        let rollout = sample_rollout();

        assert_eq!(rollout.source_kind, RolloutSourceKind::GitRef);
        assert_eq!(rollout.requested_ref.as_deref(), Some("main"));
        assert_eq!(rollout.resolved_commit, "6533e5ddcafebabe");
        assert_eq!(rollout.target_kind, RolloutTargetKind::Group);
        assert_eq!(rollout.rollout_name, "control_plane");
        assert_eq!(rollout.units, vec!["conary_test", "conary"]);
        assert_eq!(rollout.deployed_at, "2026-04-09T19:00:00+00:00");
    }

    #[test]
    fn rollout_status_has_no_drift_when_binary_and_checkout_match() {
        let rollout = sample_rollout();

        let status =
            evaluate_rollout_status(&rollout, Some("6533e5ddcafebabe"), Some("6533e5ddcafebabe"));

        assert!(!status.drifted);
        assert!(status.binary_matches_rollout);
        assert!(status.checkout_matches_rollout);
    }

    #[test]
    fn rollout_status_flags_binary_drift() {
        let rollout = sample_rollout();

        let status =
            evaluate_rollout_status(&rollout, Some("different-binary"), Some("6533e5ddcafebabe"));

        assert!(status.drifted);
        assert!(!status.binary_matches_rollout);
        assert!(status.checkout_matches_rollout);
    }

    #[test]
    fn rollout_status_flags_checkout_drift() {
        let rollout = sample_rollout();

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
