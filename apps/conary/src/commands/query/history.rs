// src/commands/query/history.rs

//! Changeset history commands
//!
//! Functions for displaying changeset/transaction history.

use super::super::open_db;
use anyhow::Result;

fn format_changeset_line(
    changeset: &conary_core::db::models::Changeset,
    publications: &[conary_core::db::models::GenerationPublication],
) -> String {
    let timestamp = changeset
        .applied_at
        .as_ref()
        .or(changeset.rolled_back_at.as_ref())
        .or(changeset.created_at.as_ref())
        .map(|s| s.as_str())
        .unwrap_or("pending");
    let id = changeset
        .id
        .map(|i| i.to_string())
        .unwrap_or_else(|| "?".to_string());
    let deferred = crate::commands::deferred_follow_up(changeset.metadata.as_deref());
    let deferred_marker = if deferred.is_empty() {
        ""
    } else {
        " [deferred]"
    };
    let scriptlet_warnings = crate::commands::scriptlet_warnings(changeset.metadata.as_deref());
    let scriptlet_marker = if scriptlet_warnings.is_empty() {
        ""
    } else {
        " [scriptlet-warning]"
    };
    let publication_marker = publication_marker_for_changeset(publications, changeset.id);
    format!(
        "  [{}] {} - {} ({:?}){}{}{}",
        id,
        timestamp,
        changeset.description,
        changeset.status,
        deferred_marker,
        scriptlet_marker,
        publication_marker
    )
}

fn format_deferred_follow_up_lines(changeset: &conary_core::db::models::Changeset) -> Vec<String> {
    crate::commands::deferred_follow_up(changeset.metadata.as_deref())
        .into_iter()
        .map(|follow_up| {
            let retry = deferred_retry_hint(&follow_up);
            format!(
                "      deferred {} {}: {}{}",
                follow_up.kind, follow_up.status, follow_up.message, retry
            )
        })
        .collect()
}

fn format_scriptlet_warning_lines(changeset: &conary_core::db::models::Changeset) -> Vec<String> {
    crate::commands::scriptlet_warnings(changeset.metadata.as_deref())
        .into_iter()
        .map(|warning| {
            format!(
                "      scriptlet {} {} for {}: {} (requested_sandbox_mode={}, effective_sandbox={})",
                warning.phase,
                warning.failure_kind,
                warning.package,
                warning.message,
                warning.requested_sandbox_mode,
                warning.effective_sandbox
            )
        })
        .collect()
}

fn deferred_retry_hint(follow_up: &crate::commands::DeferredFollowUp) -> String {
    let kind = crate::commands::classify_deferred_follow_up_kind(follow_up);
    match kind {
        crate::commands::DeferredFollowUpKind::GenerationPublication
        | crate::commands::DeferredFollowUpKind::LegacyGenerationRebuild => {
            " Retry: conary system generation publish --yes.".to_string()
        }
        crate::commands::DeferredFollowUpKind::Other => follow_up
            .retry_command
            .as_ref()
            .map(|command| format!(" Retry: {command}."))
            .unwrap_or_default(),
    }
}

fn publication_marker_for_changeset(
    publications: &[conary_core::db::models::GenerationPublication],
    changeset_id: Option<i64>,
) -> &'static str {
    let Some(changeset_id) = changeset_id else {
        return "";
    };
    publications
        .iter()
        .find(|publication| publication.trigger_changeset_id == Some(changeset_id))
        .map(|publication| match publication.status {
            conary_core::db::models::GenerationPublicationStatus::Failed => " [publication-failed]",
            conary_core::db::models::GenerationPublicationStatus::Pending
            | conary_core::db::models::GenerationPublicationStatus::Running => {
                " [publication-pending]"
            }
            conary_core::db::models::GenerationPublicationStatus::Complete
            | conary_core::db::models::GenerationPublicationStatus::Abandoned => "",
        })
        .unwrap_or("")
}

/// Show changeset history
pub async fn cmd_history(db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;
    let changesets = conary_core::db::models::Changeset::list_all(&conn)?;
    let publications = conary_core::db::models::GenerationPublication::pending_recoverable(&conn)?;

    if changesets.is_empty() {
        println!("No changeset history.");
    } else {
        println!("Changeset history:");
        for changeset in &changesets {
            println!("{}", format_changeset_line(changeset, &publications));
            for line in format_deferred_follow_up_lines(changeset) {
                println!("{line}");
            }
            for line in format_scriptlet_warning_lines(changeset) {
                println!("{line}");
            }
        }
        println!("\nTotal: {} changeset(s)", changesets.len());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{Changeset, ChangesetStatus};

    #[test]
    fn clean_applied_changeset_has_no_deferred_marker() {
        let mut changeset = Changeset::new("Install fixture-1.0.0".to_string());
        changeset.id = Some(7);
        changeset.status = ChangesetStatus::Applied;
        changeset.applied_at = Some("2026-05-14 12:00:00".to_string());

        assert_eq!(
            format_changeset_line(&changeset, &[]),
            "  [7] 2026-05-14 12:00:00 - Install fixture-1.0.0 (Applied)"
        );
        assert!(format_deferred_follow_up_lines(&changeset).is_empty());
        assert!(format_scriptlet_warning_lines(&changeset).is_empty());
    }

    #[test]
    fn applied_changeset_with_deferred_metadata_is_marked() {
        let warning = crate::commands::DeferredFollowUp {
            kind: "generation_rebuild".to_string(),
            status: "failed".to_string(),
            message: "root is not self-contained".to_string(),
            retry_command: Some("conary system generation build --summary retry --yes".to_string()),
        };
        let mut changeset = Changeset::new("Install fixture-1.0.0".to_string());
        changeset.id = Some(8);
        changeset.status = ChangesetStatus::Applied;
        changeset.applied_at = Some("2026-05-14 12:01:00".to_string());
        changeset.metadata = Some(
            crate::commands::metadata_with_deferred_follow_up(Vec::new(), vec![warning]).unwrap(),
        );

        assert_eq!(
            format_changeset_line(&changeset, &[]),
            "  [8] 2026-05-14 12:01:00 - Install fixture-1.0.0 (Applied) [deferred]"
        );
        let details = format_deferred_follow_up_lines(&changeset);
        assert_eq!(details.len(), 1);
        assert!(details[0].contains("deferred generation_rebuild failed"));
        assert!(details[0].contains("Retry: conary system generation publish --yes"));
        assert!(details[0].contains("system generation publish"));
        assert!(!details[0].contains("system generation build"));
    }

    #[test]
    fn publication_marker_marks_failed_debt() {
        let publication = conary_core::db::models::GenerationPublication {
            id: Some(1),
            trigger_changeset_id: Some(8),
            published_through_changeset_id: None,
            tx_uuid: None,
            db_path: "/tmp/db".to_string(),
            runtime_root: "/tmp/root".to_string(),
            phase: conary_core::db::models::GenerationPublicationPhase::PendingBuild,
            status: conary_core::db::models::GenerationPublicationStatus::Failed,
            state_number: None,
            generation_number: None,
            summary: "fixture".to_string(),
            last_error: Some("forced".to_string()),
            retry_count: 1,
            recoverable: true,
            created_at: None,
            updated_at: None,
            completed_at: None,
        };
        assert_eq!(
            publication_marker_for_changeset(&[publication], Some(8)),
            " [publication-failed]"
        );
    }

    #[test]
    fn legacy_generation_rebuild_deferred_line_uses_publish_retry() {
        let warning = crate::commands::DeferredFollowUp {
            kind: "generation_rebuild".to_string(),
            status: "failed".to_string(),
            message: "root is not self-contained".to_string(),
            retry_command: Some("conary system generation build --summary retry --yes".to_string()),
        };
        let mut changeset = Changeset::new("Install fixture".to_string());
        changeset.id = Some(8);
        changeset.metadata = Some(
            crate::commands::metadata_with_deferred_follow_up(Vec::new(), vec![warning]).unwrap(),
        );
        let details = format_deferred_follow_up_lines(&changeset);
        assert_eq!(details.len(), 1);
        assert!(details[0].contains("system generation publish"));
        assert!(!details[0].contains("system generation build"));
    }

    #[test]
    fn applied_changeset_with_scriptlet_warning_is_marked() {
        let warning = crate::commands::ScriptletWarning::new(
            "post-install",
            "fixture",
            "ScriptExited",
            "auto",
            "direct",
            "post-install scriptlet failed after package files were installed",
        );
        let mut changeset = Changeset::new("Install fixture".to_string());
        changeset.id = Some(9);
        changeset.status = ChangesetStatus::Applied;
        changeset.metadata = Some(
            crate::commands::metadata_with_full_envelope(
                Vec::new(),
                Vec::new(),
                Vec::new(),
                vec![warning],
            )
            .unwrap(),
        );

        assert_eq!(
            format_changeset_line(&changeset, &[]),
            "  [9] pending - Install fixture (Applied) [scriptlet-warning]"
        );
        let details = format_scriptlet_warning_lines(&changeset);
        assert_eq!(details.len(), 1);
        assert!(details[0].contains("scriptlet post-install ScriptExited for fixture"));
        assert!(details[0].contains("effective_sandbox=direct"));
    }
}
