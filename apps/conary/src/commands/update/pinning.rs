// src/commands/update/pinning.rs

//! Update pinning command handlers.

use super::super::{InstalledPackageSelector, open_db, resolve_installed_package};
use anyhow::Result;
use conary_core::db::models::Trove;
use tracing::info;

/// Pin a package to prevent updates and removal
pub async fn cmd_pin(selector: InstalledPackageSelector, db_path: &str) -> Result<()> {
    info!("Pinning package: {}", selector.name);
    let conn = open_db(db_path)?;
    let resolved = resolve_installed_package(&conn, &selector)?;
    let trove = resolved.trove;
    let trove_id = resolved.trove_id;

    if trove.pinned {
        println!("Package '{}' is already pinned", trove.name);
        return Ok(());
    }

    Trove::pin(&conn, trove_id)?;
    println!(
        "Pinned package '{}' at version {}",
        trove.name, trove.version
    );
    println!("This package will be skipped during updates and cannot be removed until unpinned.");

    Ok(())
}

/// Unpin a package to allow updates and removal
pub async fn cmd_unpin(selector: InstalledPackageSelector, db_path: &str) -> Result<()> {
    info!("Unpinning package: {}", selector.name);
    let conn = open_db(db_path)?;
    let resolved = resolve_installed_package(&conn, &selector)?;
    let trove = resolved.trove;
    let trove_id = resolved.trove_id;

    if !trove.pinned {
        println!("Package '{}' is not pinned", trove.name);
        return Ok(());
    }

    Trove::unpin(&conn, trove_id)?;
    println!(
        "Unpinned package '{}' (version {})",
        trove.name, trove.version
    );
    println!("This package can now be updated or removed.");

    Ok(())
}

/// List all pinned packages
pub async fn cmd_list_pinned(db_path: &str) -> Result<()> {
    info!("Listing pinned packages");

    let conn = open_db(db_path)?;
    let pinned = Trove::find_pinned(&conn)?;

    if pinned.is_empty() {
        println!("No packages are pinned.");
        return Ok(());
    }

    println!("Pinned packages:");
    for trove in &pinned {
        print!("  {} {}", trove.name, trove.version);
        if let Some(arch) = &trove.architecture {
            print!(" [{}]", arch);
        }
        println!();
    }
    println!("\nTotal: {} pinned package(s)", pinned.len());

    Ok(())
}
