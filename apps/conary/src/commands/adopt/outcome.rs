// apps/conary/src/commands/adopt/outcome.rs

use crate::commands::{AdoptionWarning, append_adoption_warning_metadata};
use anyhow::Result;

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
    use super::metadata_insert_succeeded;
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

        assert!(json.contains("\"schema\":\"conary.changeset.metadata.v1\""));
        assert!(json.contains("\"package\":\"curl\""));
        assert!(json.contains("\"reason\":\"partial_metadata_insert_failure\""));
        assert!(json.contains("\"package\":\"bash\""));
        assert!(json.contains("\"reason\":\"all_metadata_inserts_failed\""));
        assert!(parse_rollback_snapshots(&json).unwrap().is_empty());
        assert_eq!(adoption_warnings(Some(&json)).len(), 2);
    }
}
