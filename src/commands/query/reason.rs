// src/commands/query/reason.rs

//! Installation reason query commands
//!
//! Functions for querying packages by their installation reason
//! (explicit, dependency, collection, etc.).

use anyhow::Result;
use tracing::info;

/// Query packages by installation reason
///
/// Filters packages by their selection_reason field. Supports patterns:
/// - "explicit" or "explicitly" - packages installed directly by user
/// - "dependency" or "required" - packages installed as dependencies
/// - "collection" or "@*" - packages installed via collections
/// - Custom pattern with * wildcard - e.g., "Required by nginx"
pub fn cmd_query_reason(pattern: Option<&str>, db_path: &str) -> Result<()> {
    info!("Querying packages by reason: {:?}", pattern);
    let conn = conary::db::open(db_path)?;

    let (troves, filter_desc) = match pattern {
        Some("explicit") | Some("explicitly") => {
            (conary::db::models::Trove::find_explicitly_installed(&conn)?, "explicitly installed")
        }
        Some("dependency") | Some("required") | Some("dep") => {
            (conary::db::models::Trove::find_dependencies_installed(&conn)?, "installed as dependencies")
        }
        Some("collection") | Some("@") => {
            (conary::db::models::Trove::find_collection_installed(&conn)?, "installed via collections")
        }
        Some(custom) if custom.starts_with("@") => {
            // Pattern like "@server" - find packages from specific collection
            let pattern = format!("Installed via {}", custom);
            (conary::db::models::Trove::find_by_reason(&conn, &pattern)?, &*format!("installed via {}", custom))
        }
        Some(custom) => {
            // Custom pattern
            (conary::db::models::Trove::find_by_reason(&conn, custom)?, custom)
        }
        None => {
            // Show all with their reasons grouped
            return print_all_with_reasons(&conn);
        }
    };

    if troves.is_empty() {
        println!("No packages found matching reason: {}", filter_desc);
    } else {
        println!("Packages {} ({}):", filter_desc, troves.len());
        for trove in &troves {
            print!("  {} {}", trove.name, trove.version);
            if let Some(reason) = &trove.selection_reason {
                print!(" - {}", reason);
            }
            println!();
        }
    }

    Ok(())
}

/// Print all packages grouped by their installation reason
fn print_all_with_reasons(conn: &rusqlite::Connection) -> Result<()> {
    let all_troves = conary::db::models::Trove::list_all(conn)?;

    // Group by reason
    let mut explicit = Vec::new();
    let mut dependencies = Vec::new();
    let mut collections = Vec::new();
    let mut other = Vec::new();

    for trove in all_troves {
        match &trove.selection_reason {
            Some(r) if r == "Explicitly installed" => explicit.push(trove),
            Some(r) if r.starts_with("Required by") => dependencies.push(trove),
            Some(r) if r.starts_with("Installed via @") => collections.push(trove),
            _ => other.push(trove),
        }
    }

    if !explicit.is_empty() {
        println!("Explicitly installed ({}):", explicit.len());
        for t in &explicit {
            println!("  {} {}", t.name, t.version);
        }
        println!();
    }

    if !dependencies.is_empty() {
        println!("Installed as dependencies ({}):", dependencies.len());
        for t in &dependencies {
            let reason = t.selection_reason.as_deref().unwrap_or("");
            println!("  {} {} - {}", t.name, t.version, reason);
        }
        println!();
    }

    if !collections.is_empty() {
        println!("Installed via collections ({}):", collections.len());
        for t in &collections {
            let reason = t.selection_reason.as_deref().unwrap_or("");
            println!("  {} {} - {}", t.name, t.version, reason);
        }
        println!();
    }

    if !other.is_empty() {
        println!("Other ({}):", other.len());
        for t in &other {
            let reason = t.selection_reason.as_deref().unwrap_or("(no reason recorded)");
            println!("  {} {} - {}", t.name, t.version, reason);
        }
        println!();
    }

    let total = explicit.len() + dependencies.len() + collections.len() + other.len();
    println!("Total: {} package(s)", total);

    Ok(())
}
