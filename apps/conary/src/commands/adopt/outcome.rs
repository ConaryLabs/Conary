// apps/conary/src/commands/adopt/outcome.rs

use crate::commands::{AdoptionWarning, append_adoption_warning_metadata};
use anyhow::Result;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkAdoptionFailureStage {
    FileQuery,
    RequirementQuery,
    ProvideQuery,
    PayloadCapture,
    TroveInsert,
    MetadataInsert,
}

impl fmt::Display for BulkAdoptionFailureStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::FileQuery => "file-query",
            Self::RequirementQuery => "requirement-query",
            Self::ProvideQuery => "provide-query",
            Self::PayloadCapture => "payload-capture",
            Self::TroveInsert => "trove-insert",
            Self::MetadataInsert => "metadata-insert",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkAdoptionFailure {
    pub package: String,
    pub stage: BulkAdoptionFailureStage,
    pub message: String,
}

impl BulkAdoptionFailure {
    pub fn new(
        package: impl Into<String>,
        stage: BulkAdoptionFailureStage,
        message: impl Into<String>,
    ) -> Self {
        Self {
            package: package.into(),
            stage,
            message: message.into(),
        }
    }

    pub fn record(&self) -> String {
        format!("{} [{}]: {}", self.package, self.stage, self.message)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BulkAdoptionOutcome {
    pub considered_packages: Vec<String>,
    pub adopted_packages: Vec<String>,
    pub already_tracked_packages: Vec<String>,
    pub failures: Vec<BulkAdoptionFailure>,
}

impl BulkAdoptionOutcome {
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty()
    }

    #[must_use]
    pub fn failure_records(&self) -> Vec<String> {
        self.failures
            .iter()
            .map(BulkAdoptionFailure::record)
            .collect()
    }
}

pub(crate) fn write_warning_metadata(
    conn: &rusqlite::Connection,
    changeset_id: i64,
    warnings: Vec<AdoptionWarning>,
) -> Result<()> {
    append_adoption_warning_metadata(conn, changeset_id, warnings)
}

#[cfg(test)]
mod tests {
    use super::{BulkAdoptionFailure, BulkAdoptionFailureStage, BulkAdoptionOutcome};
    use crate::commands::changeset_metadata::CHANGESET_METADATA_SCHEMA;
    use crate::commands::{
        AdoptionWarning, adoption_warnings, metadata_with_adoption_warnings,
        parse_rollback_snapshots,
    };

    #[test]
    fn adoption_warning_metadata_preserves_versioned_envelope() {
        let json = metadata_with_adoption_warnings(
            vec![],
            vec![],
            vec![AdoptionWarning::refresh_replacement_failure(
                "curl",
                "exact dependency projection failed",
            )],
        )
        .unwrap();

        assert!(json.contains(&format!("\"schema\":\"{CHANGESET_METADATA_SCHEMA}\"")));
        assert!(json.contains("\"package\":\"curl\""));
        assert!(json.contains(
            "\"reason\":\"refresh_replacement_failed: exact dependency projection failed\""
        ));
        assert!(parse_rollback_snapshots(&json).unwrap().is_empty());
        assert_eq!(adoption_warnings(Some(&json)).unwrap().len(), 1);
    }

    #[test]
    fn bulk_adoption_outcome_preserves_exact_failure_stage_and_message() {
        let outcome = BulkAdoptionOutcome {
            considered_packages: vec!["broken".to_string()],
            failures: vec![BulkAdoptionFailure::new(
                "broken",
                BulkAdoptionFailureStage::PayloadCapture,
                "failed to capture exact node /usr/bin/broken",
            )],
            ..Default::default()
        };

        assert!(!outcome.is_complete());
        assert_eq!(
            outcome.failure_records(),
            vec![
                "broken [payload-capture]: failed to capture exact node /usr/bin/broken"
                    .to_string()
            ]
        );
    }
}
