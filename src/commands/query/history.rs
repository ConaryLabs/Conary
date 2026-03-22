// src/commands/query/history.rs

//! Changeset history commands
//!
//! Functions for displaying changeset/transaction history.

use super::super::open_db;
use anyhow::Result;

/// Show changeset history
pub async fn cmd_history(db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;
    let changesets = conary_core::db::models::Changeset::list_all(&conn)?;

    if changesets.is_empty() {
        println!("No changeset history.");
    } else {
        println!("Changeset history:");
        for changeset in &changesets {
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
            println!(
                "  [{}] {} - {} ({:?})",
                id, timestamp, changeset.description, changeset.status
            );
        }
        println!("\nTotal: {} changeset(s)", changesets.len());
    }

    Ok(())
}
