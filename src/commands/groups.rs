// src/commands/groups.rs
//! Package group command implementations

use anyhow::Result;
use conary_core::db::models::{CanonicalPackage, PackageImplementation};

pub fn cmd_groups_list(db_path: &str) -> Result<()> {
    let conn = conary_core::db::open(db_path)?;
    let groups = CanonicalPackage::list_by_kind(&conn, "group")?;
    if groups.is_empty() {
        println!("No package groups found. Run 'conary registry update' to sync.");
        return Ok(());
    }
    println!("Package groups:");
    for g in &groups {
        let desc = g.description.as_deref().unwrap_or("");
        println!("  {} - {desc}", g.name);
    }
    Ok(())
}

pub fn cmd_groups_show(db_path: &str, name: &str, distro: Option<&str>) -> Result<()> {
    let conn = conary_core::db::open(db_path)?;
    let pkg = CanonicalPackage::find_by_name(&conn, name)?;
    let Some(pkg) = pkg else {
        println!("Group '{name}' not found.");
        return Ok(());
    };
    if pkg.kind != "group" {
        println!("'{name}' is a package, not a group.");
        return Ok(());
    }

    println!("Group: {}", pkg.name);
    if let Some(ref desc) = pkg.description {
        println!("Description: {desc}");
    }
    println!();

    let pkg_id = pkg.id.ok_or_else(|| anyhow::anyhow!("canonical package '{}' has no id", name))?;
    let impls = PackageImplementation::find_by_canonical(&conn, pkg_id)?;
    if let Some(distro_filter) = distro {
        let filtered: Vec<_> = impls.iter().filter(|i| i.distro == distro_filter).collect();
        if filtered.is_empty() {
            println!("No implementation for distro '{distro_filter}'");
        } else {
            for i in &filtered {
                println!("  {}: {}", i.distro, i.distro_name);
            }
        }
    } else {
        println!("Implementations:");
        for i in &impls {
            println!("  {}: {}", i.distro, i.distro_name);
        }
    }
    Ok(())
}
