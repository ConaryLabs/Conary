// src/commands/query/history.rs

//! Changeset history commands
//!
//! Functions for displaying changeset/transaction history.

use super::super::open_db;
use anyhow::Result;

fn format_changeset_line(changeset: &conary_core::db::models::Changeset) -> String {
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
    let marker = if deferred.is_empty() {
        ""
    } else {
        " [deferred]"
    };
    format!(
        "  [{}] {} - {} ({:?}){}",
        id, timestamp, changeset.description, changeset.status, marker
    )
}

fn format_deferred_follow_up_lines(changeset: &conary_core::db::models::Changeset) -> Vec<String> {
    crate::commands::deferred_follow_up(changeset.metadata.as_deref())
        .into_iter()
        .map(|follow_up| {
            let retry = follow_up
                .retry_command
                .map(|command| format!(" Retry: {command}."))
                .unwrap_or_default();
            format!(
                "      deferred {} {}: {}{}",
                follow_up.kind, follow_up.status, follow_up.message, retry
            )
        })
        .collect()
}

/// Show changeset history
pub async fn cmd_history(db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;
    let changesets = conary_core::db::models::Changeset::list_all(&conn)?;

    if changesets.is_empty() {
        println!("No changeset history.");
    } else {
        println!("Changeset history:");
        for changeset in &changesets {
            println!("{}", format_changeset_line(changeset));
            for line in format_deferred_follow_up_lines(changeset) {
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
            format_changeset_line(&changeset),
            "  [7] 2026-05-14 12:00:00 - Install fixture-1.0.0 (Applied)"
        );
        assert!(format_deferred_follow_up_lines(&changeset).is_empty());
    }

    #[test]
    fn applied_changeset_with_deferred_metadata_is_marked() {
        let warning = crate::commands::DeferredFollowUp {
            kind: "generation_rebuild".to_string(),
            status: "failed".to_string(),
            message: "root is not self-contained".to_string(),
            retry_command: Some(
                "conary --allow-live-system-mutation system generation build --summary retry"
                    .to_string(),
            ),
        };
        let mut changeset = Changeset::new("Install fixture-1.0.0".to_string());
        changeset.id = Some(8);
        changeset.status = ChangesetStatus::Applied;
        changeset.applied_at = Some("2026-05-14 12:01:00".to_string());
        changeset.metadata = Some(
            crate::commands::metadata_with_deferred_follow_up(Vec::new(), vec![warning]).unwrap(),
        );

        assert_eq!(
            format_changeset_line(&changeset),
            "  [8] 2026-05-14 12:01:00 - Install fixture-1.0.0 (Applied) [deferred]"
        );
        let details = format_deferred_follow_up_lines(&changeset);
        assert_eq!(details.len(), 1);
        assert!(details[0].contains("deferred generation_rebuild failed"));
        assert!(details[0].contains("Retry: conary --allow-live-system-mutation"));
    }
}
