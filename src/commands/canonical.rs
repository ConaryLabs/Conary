// src/commands/canonical.rs
//! Canonical package identity command implementations

use super::open_db;
use anyhow::Result;
use conary_core::db::models::{CanonicalPackage, PackageImplementation};

pub fn cmd_canonical_show(db_path: &str, name: &str) -> Result<()> {
    let conn = open_db(db_path)?;
    let pkg = CanonicalPackage::resolve_name(&conn, name)?;
    let Some(pkg) = pkg else {
        println!("No canonical mapping found for '{name}'");
        return Ok(());
    };

    println!("Canonical: {}", pkg.name);
    if let Some(ref appstream) = pkg.appstream_id {
        println!("AppStream: {appstream}");
    }
    if let Some(ref desc) = pkg.description {
        println!("Description: {desc}");
    }
    println!("Kind: {}", pkg.kind);
    if let Some(ref cat) = pkg.category {
        println!("Category: {cat}");
    }
    println!();

    let pkg_id = pkg
        .id
        .ok_or_else(|| anyhow::anyhow!("Canonical package '{}' has no database ID", name))?;
    let impls = PackageImplementation::find_by_canonical(&conn, pkg_id)?;
    if impls.is_empty() {
        println!("No implementations found.");
    } else {
        println!("Implementations:");
        for i in &impls {
            println!("  {}: {} (source: {})", i.distro, i.distro_name, i.source);
        }
    }
    Ok(())
}

pub fn cmd_canonical_search(db_path: &str, query: &str) -> Result<()> {
    let conn = open_db(db_path)?;
    let results = CanonicalPackage::search(&conn, query)?;
    if results.is_empty() {
        println!("No packages found matching '{query}'");
        return Ok(());
    }
    for pkg in &results {
        let kind_tag = if pkg.kind == "group" { " [group]" } else { "" };
        let desc = pkg.description.as_deref().unwrap_or("");
        println!("  {}{kind_tag} - {desc}", pkg.name);
    }
    Ok(())
}

pub fn cmd_canonical_unmapped(db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT t.name FROM troves t
         WHERE t.is_collection = 0
         AND t.is_component = 0
         AND NOT EXISTS (
             SELECT 1 FROM package_implementations pi WHERE pi.distro_name = t.name
         )
         AND NOT EXISTS (
             SELECT 1 FROM canonical_packages cp WHERE cp.name = t.name
         )
         ORDER BY t.name",
    )?;
    let names: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    if names.is_empty() {
        println!("All installed packages have canonical mappings.");
    } else {
        println!(
            "{} installed packages without canonical mapping:",
            names.len()
        );
        for name in &names {
            println!("  {name}");
        }
    }
    Ok(())
}
