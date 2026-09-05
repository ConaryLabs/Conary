// crates/conary-core/src/transaction/recovery/scan.rs

//! Descending artifact selection and typed recovery evidence.

use super::{TransactionEngine, load_generation_artifact_for_number};
use crate::Result;
use crate::generation::artifact::GenerationArtifact;
use crate::generation::verity_policy::{VerityPolicy, VerityPolicyError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoverySkipReason {
    InvalidArtifact { detail: String },
    VerityPolicy(VerityPolicyError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverySkippedArtifact {
    pub generation: i64,
    pub reason: RecoverySkipReason,
}

/// Evidence returned by boot recovery, including candidates rejected before
/// the selected artifact. This is not a persisted generation format.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecoveryEvidence {
    pub selected_generation: Option<i64>,
    pub skipped_artifacts: Vec<RecoverySkippedArtifact>,
}

impl RecoveryEvidence {
    pub(super) fn selected(generation: i64) -> Self {
        Self {
            selected_generation: Some(generation),
            skipped_artifacts: Vec::new(),
        }
    }
}

#[derive(Debug, Default)]
pub(in crate::transaction) struct RecoveryScan {
    pub artifact: Option<GenerationArtifact>,
    pub skipped_artifacts: Vec<RecoverySkippedArtifact>,
}

impl TransactionEngine {
    /// Select the highest intact artifact eligible for the active policy.
    /// Missing evidence disqualifies a candidate, never the verified policy.
    pub(in crate::transaction) fn find_latest_intact_generation(
        &self,
        verity: &VerityPolicy,
    ) -> Result<RecoveryScan> {
        verity.requires_verification()?;
        let mut result = RecoveryScan::default();
        let entries = match std::fs::read_dir(&self.config.generations_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(result),
            Err(error) => return Err(error.into()),
        };
        let mut candidates = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let Some(generation) = name.to_str().and_then(|name| name.parse::<i64>().ok()) else {
                tracing::debug!(path = %entry.path().display(), "Recovery: ignoring non-generation directory");
                continue;
            };
            candidates.push(generation);
        }
        candidates.sort_unstable_by(|a, b| b.cmp(a));

        for generation in candidates {
            let gen_dir = self.config.generations_dir.join(generation.to_string());
            let reason = match load_generation_artifact_for_number(generation, &gen_dir) {
                Ok(artifact) => match verity.mount_requirements(&artifact.metadata) {
                    Ok(_) => {
                        result.artifact = Some(artifact);
                        return Ok(result);
                    }
                    Err(error @ VerityPolicyError::MissingGenerationVerity { .. }) => {
                        RecoverySkipReason::VerityPolicy(error)
                    }
                    Err(error) => return Err(error.into()),
                },
                Err(error) => RecoverySkipReason::InvalidArtifact {
                    detail: error.to_string(),
                },
            };
            tracing::warn!(
                generation,
                ?reason,
                "Recovery: skipping ineligible generation artifact"
            );
            result
                .skipped_artifacts
                .push(RecoverySkippedArtifact { generation, reason });
        }
        Ok(result)
    }
}
