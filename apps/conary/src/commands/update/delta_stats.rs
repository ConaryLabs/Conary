// src/commands/update/delta_stats.rs

//! Delta update statistics command handler.

use super::super::open_db;
use anyhow::Result;
use conary_core::db::models::DeltaStats;
use tracing::info;

/// Show delta update statistics
pub async fn cmd_delta_stats(db_path: &str) -> Result<()> {
    info!("Showing delta update statistics");

    let conn = open_db(db_path)?;
    let total_stats = DeltaStats::get_total_stats(&conn)?;

    let all_stats = {
        let mut stmt = conn.prepare(
            "SELECT id, changeset_id, total_bytes_saved, deltas_applied, full_downloads, delta_failures, created_at
             FROM delta_stats ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(DeltaStats {
                id: Some(row.get(0)?),
                changeset_id: row.get(1)?,
                total_bytes_saved: row.get(2)?,
                deltas_applied: row.get(3)?,
                full_downloads: row.get(4)?,
                delta_failures: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    if all_stats.is_empty() {
        println!("No delta statistics available");
        println!("Run 'conary update' to start tracking delta usage");
        return Ok(());
    }

    println!("=== Delta Update Statistics ===\n");
    println!("Total Statistics:");
    println!("  Delta updates applied: {}", total_stats.deltas_applied);
    println!("  Full downloads: {}", total_stats.full_downloads);
    println!("  Delta failures: {}", total_stats.delta_failures);

    let total_mb = total_stats.total_bytes_saved as f64 / 1_048_576.0;
    println!("  Total bandwidth saved: {:.2} MB", total_mb);

    let total_updates = total_stats.deltas_applied + total_stats.full_downloads;
    if total_updates > 0 {
        let success_rate = (total_stats.deltas_applied as f64 / total_updates as f64) * 100.0;
        println!("  Delta success rate: {:.1}%", success_rate);
    }

    println!("\nRecent Operations:");
    for (idx, stats) in all_stats.iter().take(10).enumerate() {
        if idx > 0 {
            println!();
        }

        let timestamp = stats.created_at.as_deref().unwrap_or("unknown");
        println!("  [Changeset {}] {}", stats.changeset_id, timestamp);
        println!("    Deltas applied: {}", stats.deltas_applied);
        println!("    Full downloads: {}", stats.full_downloads);

        if stats.delta_failures > 0 {
            println!("    Delta failures: {}", stats.delta_failures);
        }

        if stats.total_bytes_saved > 0 {
            let saved_mb = stats.total_bytes_saved as f64 / 1_048_576.0;
            println!("    Bandwidth saved: {:.2} MB", saved_mb);
        }
    }

    if all_stats.len() > 10 {
        println!("\n... and {} more operations", all_stats.len() - 10);
    }

    Ok(())
}
