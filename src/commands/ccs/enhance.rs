// src/commands/ccs/enhance.rs
//! Enhancement command for retroactive CCS feature addition

use anyhow::{Context, Result};
use conary::ccs::enhancement::{
    EnhancementResult_, EnhancementRunner, EnhancementType, ENHANCEMENT_VERSION,
};
use conary::ccs::enhancement::context::ConvertedPackageInfo;
use conary::ccs::enhancement::runner::EnhancementOptions;
use conary::db::schema;
use rusqlite::Connection;
use std::path::PathBuf;

/// Run the `conary ccs enhance` command
#[allow(clippy::too_many_arguments)]
pub fn cmd_ccs_enhance(
    db_path: &str,
    trove_id: Option<i64>,
    all_pending: bool,
    update_outdated: bool,
    types: Option<Vec<String>>,
    force: bool,
    stats: bool,
    dry_run: bool,
    install_root: &str,
) -> Result<()> {
    let conn = Connection::open(db_path)
        .with_context(|| format!("Failed to open database: {}", db_path))?;
    schema::migrate(&conn)?;

    // Parse enhancement types
    let enhancement_types: Vec<EnhancementType> = if let Some(ref type_strs) = types {
        type_strs
            .iter()
            .filter_map(|s| EnhancementType::from_str(s))
            .collect()
    } else {
        EnhancementType::all().to_vec()
    };

    if enhancement_types.is_empty() && types.is_some() {
        eprintln!("Error: No valid enhancement types specified.");
        eprintln!("Valid types: capabilities, provenance, subpackages");
        return Ok(());
    }

    // Stats only mode
    if stats {
        return show_enhancement_stats(&conn);
    }

    // Build options
    let options = EnhancementOptions {
        types: enhancement_types,
        force,
        install_root: PathBuf::from(install_root),
        fail_fast: false,
        parallel: true,
        parallel_workers: 0, // auto-detect
        cancel_token: None,
    };

    let runner = EnhancementRunner::with_options(&conn, options);

    // Determine what to enhance
    if dry_run {
        return show_dry_run(&conn, trove_id, all_pending, update_outdated);
    }

    if let Some(tid) = trove_id {
        // Enhance specific package
        match runner.enhance(tid) {
            Ok(result) => {
                if result.is_success() {
                    println!("Enhancement complete for trove_id={}", tid);
                    println!("  Applied: {:?}", result.applied);
                    if !result.skipped.is_empty() {
                        println!("  Skipped: {:?}", result.skipped);
                    }
                } else {
                    println!("Enhancement failed for trove_id={}", tid);
                    for (t, err) in &result.failed {
                        println!("  {}: {}", t, err);
                    }
                }
            }
            Err(e) => {
                eprintln!("Error enhancing trove_id={}: {}", tid, e);
            }
        }
    } else if all_pending {
        // Enhance all pending packages
        let results = runner.enhance_all_pending()?;
        print_batch_summary(&results);
    } else if update_outdated {
        // Re-enhance outdated packages
        let results = runner.enhance_all_outdated()?;
        print_batch_summary(&results);
    } else {
        // Default: show help
        eprintln!("Usage: conary ccs enhance [OPTIONS]");
        eprintln!();
        eprintln!("Options:");
        eprintln!("  --trove-id <ID>    Enhance specific package");
        eprintln!("  --all-pending      Enhance all pending packages");
        eprintln!("  --update-outdated  Re-enhance packages with outdated version");
        eprintln!("  --stats            Show enhancement statistics");
        eprintln!("  --dry-run          Show what would be enhanced");
        eprintln!();

        // Show quick stats
        show_enhancement_stats(&conn)?;
    }

    Ok(())
}

fn show_enhancement_stats(conn: &rusqlite::Connection) -> Result<()> {
    let stats = ConvertedPackageInfo::count_by_status(conn)?;

    println!("Enhancement Statistics");
    println!("======================");
    println!("Total converted packages: {}", stats.total);
    println!();
    println!("By status:");
    println!("  Pending:     {}", stats.pending);
    println!("  In Progress: {}", stats.in_progress);
    println!("  Complete:    {}", stats.complete);
    println!("  Failed:      {}", stats.failed);
    println!("  Skipped:     {}", stats.skipped);
    println!();
    println!("Current enhancement version: {}", ENHANCEMENT_VERSION);

    // Show outdated count
    let outdated = ConvertedPackageInfo::find_outdated(conn, ENHANCEMENT_VERSION)?;
    if !outdated.is_empty() {
        println!("Packages with outdated enhancement: {}", outdated.len());
    }

    Ok(())
}

fn show_dry_run(
    conn: &rusqlite::Connection,
    trove_id: Option<i64>,
    all_pending: bool,
    update_outdated: bool,
) -> Result<()> {
    println!("Dry run - showing what would be enhanced:");
    println!();

    if let Some(tid) = trove_id {
        let name: String = conn
            .query_row("SELECT name FROM troves WHERE id = ?1", [tid], |row| row.get(0))
            .unwrap_or_else(|_| format!("(unknown trove {})", tid));
        println!("Would enhance: {} (trove_id={})", name, tid);
    } else if all_pending {
        let pending = ConvertedPackageInfo::find_pending(conn)?;
        println!("Would enhance {} pending packages:", pending.len());
        for pkg in pending.iter().take(20) {
            println!("  - {} v{} (trove_id={})", pkg.name, pkg.version, pkg.trove_id);
        }
        if pending.len() > 20 {
            println!("  ... and {} more", pending.len() - 20);
        }
    } else if update_outdated {
        let outdated = ConvertedPackageInfo::find_outdated(conn, ENHANCEMENT_VERSION)?;
        println!("Would re-enhance {} outdated packages:", outdated.len());
        for pkg in outdated.iter().take(20) {
            println!(
                "  - {} v{} (version {} -> {})",
                pkg.name, pkg.version, pkg.enhancement_version, ENHANCEMENT_VERSION
            );
        }
        if outdated.len() > 20 {
            println!("  ... and {} more", outdated.len() - 20);
        }
    }

    Ok(())
}

fn print_batch_summary(results: &[EnhancementResult_]) {
    let succeeded = results.iter().filter(|r| r.is_success()).count();
    let failed = results.len() - succeeded;

    println!();
    println!("Enhancement Summary");
    println!("===================");
    println!("Total:    {}", results.len());
    println!("Success:  {}", succeeded);
    println!("Failed:   {}", failed);

    if failed > 0 {
        println!();
        println!("Failed packages:");
        for result in results.iter().filter(|r| !r.is_success()) {
            println!("  trove_id={}:", result.trove_id);
            for (t, err) in &result.failed {
                println!("    {}: {}", t, err);
            }
        }
    }
}
