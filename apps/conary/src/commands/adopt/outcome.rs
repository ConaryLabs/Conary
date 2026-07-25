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
    pub degraded_packages: Vec<String>,
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

pub(crate) fn metadata_insert_succeeded(total_inserts: usize, insert_failures: usize) -> bool {
    total_inserts == 0 || insert_failures < total_inserts
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
    use super::{
        BulkAdoptionFailure, BulkAdoptionFailureStage, BulkAdoptionOutcome,
        metadata_insert_succeeded,
    };
    use crate::commands::changeset_metadata::CHANGESET_METADATA_SCHEMA;
    use crate::commands::{
        AdoptionWarning, adoption_warnings, metadata_with_adoption_warnings,
        parse_rollback_snapshots,
    };

    #[test]
    fn metadata_insert_succeeded_rejects_all_failed_non_empty_metadata() {
        assert!(!metadata_insert_succeeded(3, 3));
    }

    #[test]
    fn metadata_insert_succeeded_allows_partial_success_and_empty_real_metadata() {
        assert!(metadata_insert_succeeded(3, 2));
        assert!(metadata_insert_succeeded(0, 0));
    }

    #[test]
    fn adoption_warning_metadata_preserves_versioned_envelope() {
        let json = metadata_with_adoption_warnings(
            vec![],
            vec![],
            vec![
                AdoptionWarning::partial_insert_failure("curl", 4, 1),
                AdoptionWarning::all_insert_failure("bash", 3),
            ],
        )
        .unwrap();

        assert!(json.contains(&format!("\"schema\":\"{CHANGESET_METADATA_SCHEMA}\"")));
        assert!(json.contains("\"package\":\"curl\""));
        assert!(json.contains("\"reason\":\"partial_metadata_insert_failure\""));
        assert!(json.contains("\"package\":\"bash\""));
        assert!(json.contains("\"reason\":\"all_metadata_inserts_failed\""));
        assert!(parse_rollback_snapshots(&json).unwrap().is_empty());
        assert_eq!(adoption_warnings(Some(&json)).unwrap().len(), 2);
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
