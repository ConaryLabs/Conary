// src/commands/query/package.rs

//! Package query commands
//!
//! Functions for querying installed packages, showing package info,
//! and listing package files.

use super::super::open_db;
use super::QueryOptions;
use anyhow::Result;

/// Query installed packages
pub async fn cmd_query(pattern: Option<&str>, db_path: &str, options: QueryOptions) -> Result<()> {
    let conn = open_db(db_path)?;

    // Path query mode: find package containing a file
    if let Some(file_path) = &options.path {
        return query_by_path(&conn, file_path, &options);
    }

    let troves = if let Some(pattern) = pattern {
        conary_core::db::models::Trove::find_by_name(&conn, pattern)?
    } else {
        conary_core::db::models::Trove::list_all(&conn)?
    };

    if troves.is_empty() {
        println!("No packages found.");
        return Ok(());
    }

    // Detailed info mode
    if options.info && troves.len() == 1 {
        return show_package_info(&conn, &troves[0], &options);
    }

    // List with files (ls -l style)
    if (options.lsl || options.files) && troves.len() == 1 {
        return list_package_files(&conn, &troves[0], options.lsl);
    }

    // Standard listing
    println!("Installed packages:");
    for trove in &troves {
        print!(
            "  {} {} ({:?})",
            trove.name, trove.version, trove.trove_type
        );
        if let Some(arch) = &trove.architecture {
            print!(" [{}]", arch);
        }
        println!();
    }
    println!("\nTotal: {} package(s)", troves.len());

    Ok(())
}

/// Query package by file path
fn query_by_path(
    conn: &rusqlite::Connection,
    file_path: &str,
    options: &QueryOptions,
) -> Result<()> {
    // Try exact match first
    let file = conary_core::db::models::FileEntry::find_by_path(conn, file_path)?;

    if let Some(file) = file
        && let Ok(Some(trove)) = conary_core::db::models::Trove::find_by_id(conn, file.trove_id)
    {
        if options.info {
            return show_package_info(conn, &trove, options);
        }
        println!("{} {} provides:", trove.name, trove.version);
        println!("  {}", file_path);
        return Ok(());
    }

    // Try pattern match
    let pattern = if file_path.contains('%') || file_path.contains('*') {
        file_path.replace('*', "%")
    } else {
        format!("%{file_path}%")
    };

    let files = conary_core::db::models::FileEntry::find_by_path_pattern(conn, &pattern)?;
    if files.is_empty() {
        println!("No package owns file matching '{}'", file_path);
        return Ok(());
    }

    // Group by trove
    let mut trove_files: std::collections::HashMap<i64, Vec<String>> =
        std::collections::HashMap::new();
    for file in &files {
        trove_files
            .entry(file.trove_id)
            .or_default()
            .push(file.path.clone());
    }

    println!("Packages owning files matching '{}':", file_path);
    for (trove_id, paths) in &trove_files {
        if let Ok(Some(trove)) = conary_core::db::models::Trove::find_by_id(conn, *trove_id) {
            println!("\n{} {}:", trove.name, trove.version);
            for path in paths {
                println!("  {}", path);
            }
        }
    }

    Ok(())
}

/// Show detailed package information
fn show_package_info(
    conn: &rusqlite::Connection,
    trove: &conary_core::db::models::Trove,
    _options: &QueryOptions,
) -> Result<()> {
    let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;

    println!("Name        : {}", trove.name);
    println!("Version     : {}", trove.version);
    println!("Type        : {:?}", trove.trove_type);

    if let Some(arch) = &trove.architecture {
        println!("Architecture: {}", arch);
    }

    if let Some(desc) = &trove.description {
        println!("Description : {}", desc);
    }

    if let Some(installed) = &trove.installed_at {
        println!("Installed   : {}", installed);
    }

    if let Some(reason) = &trove.selection_reason {
        println!("Reason      : {}", reason);
    }

    // Show install reason
    println!("Install Type: {:?}", trove.install_reason);
    println!("Pinned      : {}", if trove.pinned { "yes" } else { "no" });

    // Count files
    let files = conary_core::db::models::FileEntry::find_by_trove(conn, trove_id)?;
    println!("Files       : {}", files.len());

    // Calculate total size
    let total_size: i64 = files.iter().map(|f| f.size).sum();
    println!(
        "Size        : {}",
        crate::commands::format_bytes(total_size as u64)
    );

    // Dependencies
    let deps = conary_core::db::models::DependencyEntry::find_by_trove(conn, trove_id)?;
    if !deps.is_empty() {
        println!("\nDependencies ({}):", deps.len());
        for dep in &deps {
            println!("  {}", dep.to_typed_string());
        }
    }

    // Provides
    let provides = conary_core::db::models::ProvideEntry::find_by_trove(conn, trove_id)?;
    if !provides.is_empty() {
        println!("\nProvides ({}):", provides.len());
        for p in &provides {
            println!("  {}", p.to_typed_string());
        }
    }

    // Components
    let components = conary_core::db::models::Component::find_by_trove(conn, trove_id)?;
    if !components.is_empty() {
        println!("\nComponents ({}):", components.len());
        for comp in &components {
            let installed = if comp.is_installed {
                ""
            } else {
                " [not installed]"
            };
            println!("  :{}{}", comp.name, installed);
        }
    }

    Ok(())
}

/// List package files
fn list_package_files(
    conn: &rusqlite::Connection,
    trove: &conary_core::db::models::Trove,
    lsl: bool,
) -> Result<()> {
    let trove_id = trove.id.ok_or_else(|| anyhow::anyhow!("Trove has no ID"))?;
    let files = conary_core::db::models::FileEntry::list_files_lsl(conn, trove_id)?;

    if files.is_empty() {
        println!("No files in package {} {}", trove.name, trove.version);
        return Ok(());
    }

    println!(
        "Files in {} {} ({} files):",
        trove.name,
        trove.version,
        files.len()
    );

    if lsl {
        // ls -l style output
        for file in &files {
            println!(
                "{} {:>8} {:>8} {:>8} {}",
                file.format_permissions(),
                file.owner.as_deref().unwrap_or("root"),
                file.group_name.as_deref().unwrap_or("root"),
                file.size_human(),
                file.path
            );
        }
    } else {
        // Simple list
        for file in &files {
            println!("{}", file.path);
        }
    }

    Ok(())
}
