// apps/remi/src/server/scriptlet_evidence_queue/types.rs

use serde::{Deserialize, Serialize};

use conary_core::db::models::{NewScriptletEvidenceCluster, NewScriptletEvidenceSample};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterKeyInput {
    pub schema_version: u32,
    pub distro: String,
    pub target_profile: String,
    pub blocked_class: String,
    pub command: String,
    pub normalized_command_shape: String,
    pub lifecycle_phase: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StableClusterKey {
    pub cluster_key: String,
    pub normalized_command_shape_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingEvidenceSample {
    pub cluster: NewScriptletEvidenceCluster,
    pub sample: NewScriptletEvidenceSample,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordSummary {
    pub sample_count: usize,
    pub cluster_count: usize,
}
