// src/commands/label.rs

//! Label management CLI commands
//!
//! Commands for managing Conary-style labels (repository@namespace:tag)
//! for package provenance tracking.

use anyhow::Result;
use tracing::info;

/// List all labels
pub fn cmd_label_list(db_path: &str, verbose: bool) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let labels = conary::db::models::LabelEntry::list_all(&conn)?;

    if labels.is_empty() {
        println!("No labels defined.");
        println!("\nUse 'conary label-add <label>' to add a label.");
        return Ok(());
    }

    println!("Labels ({}):", labels.len());
    for label in &labels {
        let label_str = label.to_string();
        if verbose {
            print!("  {}", label_str);
            if let Some(desc) = &label.description {
                print!(" - {}", desc);
            }
            // Show package count
            if let Ok(count) = label.package_count(&conn)
                && count > 0
            {
                print!(" ({} packages)", count);
            }
            // Show parent
            if let Some(parent_id) = label.parent_label_id
                && let Ok(Some(parent)) =
                    conary::db::models::LabelEntry::find_by_id(&conn, parent_id)
            {
                print!(" <- {}", parent);
            }
            println!();
        } else {
            println!("  {}", label_str);
        }
    }

    Ok(())
}

/// Add a new label
pub fn cmd_label_add(
    label_str: &str,
    description: Option<&str>,
    parent: Option<&str>,
    db_path: &str,
) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    // Parse the label
    let spec = conary::Label::parse(label_str)
        .map_err(|e| anyhow::anyhow!("Invalid label format: {}", e))?;

    // Check if already exists
    if let Some(existing) = conary::db::models::LabelEntry::find_by_spec(
        &conn, &spec.repository, &spec.namespace, &spec.tag
    )? {
        println!("Label '{}' already exists (id={})", label_str, existing.id.unwrap_or(0));
        return Ok(());
    }

    // Find parent label if specified
    let parent_id = if let Some(parent_str) = parent {
        let parent_entry = conary::db::models::LabelEntry::find_by_string(&conn, parent_str)?
            .ok_or_else(|| anyhow::anyhow!("Parent label '{}' not found", parent_str))?;
        parent_entry.id
    } else {
        None
    };

    // Create the label
    let mut label = conary::db::models::LabelEntry::from_spec(&spec);
    label.description = description.map(String::from);
    label.parent_label_id = parent_id;

    let id = label.insert(&conn)?;
    info!("Created label '{}' with id={}", label_str, id);

    println!("Added label: {}", label_str);
    if let Some(desc) = description {
        println!("  Description: {}", desc);
    }
    if let Some(p) = parent {
        println!("  Parent: {}", p);
    }

    Ok(())
}

/// Remove a label
pub fn cmd_label_remove(label_str: &str, db_path: &str, force: bool) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    // Find the label
    let label = conary::db::models::LabelEntry::find_by_string(&conn, label_str)?
        .ok_or_else(|| anyhow::anyhow!("Label '{}' not found", label_str))?;

    let label_id = label.id.ok_or_else(|| anyhow::anyhow!("Label has no ID"))?;

    // Check if any packages use this label
    let pkg_count = label.package_count(&conn)?;
    if pkg_count > 0 && !force {
        return Err(anyhow::anyhow!(
            "Cannot remove label '{}': {} package(s) use it. Use --force to override.",
            label_str, pkg_count
        ));
    }

    // Check if any child labels exist
    let children = label.children(&conn)?;
    if !children.is_empty() && !force {
        let child_names: Vec<String> = children.iter().map(|c| c.to_string()).collect();
        return Err(anyhow::anyhow!(
            "Cannot remove label '{}': has child labels ({}). Use --force to override.",
            label_str, child_names.join(", ")
        ));
    }

    // Remove from label path if present
    conary::db::models::remove_from_path(&conn, label_id)?;

    // Delete the label
    conary::db::models::LabelEntry::delete(&conn, label_id)?;

    println!("Removed label: {}", label_str);
    if pkg_count > 0 {
        println!("  Warning: {} package(s) no longer have a label", pkg_count);
    }

    Ok(())
}

/// Show or modify the label path
pub fn cmd_label_path(
    db_path: &str,
    add: Option<&str>,
    remove: Option<&str>,
    priority: Option<i32>,
) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    // Handle modifications
    if let Some(label_str) = add {
        let label = conary::db::models::LabelEntry::find_by_string(&conn, label_str)?
            .ok_or_else(|| anyhow::anyhow!("Label '{}' not found. Add it first with 'conary label-add'.", label_str))?;

        let label_id = label.id.ok_or_else(|| anyhow::anyhow!("Label has no ID"))?;
        let prio = priority.unwrap_or(50); // Default priority

        conary::db::models::add_to_path(&conn, label_id, prio)?;
        println!("Added '{}' to label path with priority {}", label_str, prio);
        return Ok(());
    }

    if let Some(label_str) = remove {
        let label = conary::db::models::LabelEntry::find_by_string(&conn, label_str)?
            .ok_or_else(|| anyhow::anyhow!("Label '{}' not found", label_str))?;

        let label_id = label.id.ok_or_else(|| anyhow::anyhow!("Label has no ID"))?;

        conary::db::models::remove_from_path(&conn, label_id)?;
        println!("Removed '{}' from label path", label_str);
        return Ok(());
    }

    // Show current label path
    let path_entries = conary::db::models::LabelPathEntry::list_ordered(&conn)?;

    if path_entries.is_empty() {
        println!("Label path is empty.");
        println!("\nUse 'conary label-path --add <label>' to add labels to the path.");
        return Ok(());
    }

    println!("Label path (search order):");
    for entry in &path_entries {
        if let Some(label) = entry.label(&conn)? {
            let enabled = if entry.enabled { "" } else { " [disabled]" };
            println!("  {:3}. {}{}", entry.priority, label, enabled);
        }
    }

    Ok(())
}

/// Show label for a package
pub fn cmd_label_show(package_name: &str, db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    let troves = conary::db::models::Trove::find_by_name(&conn, package_name)?;
    if troves.is_empty() {
        return Err(anyhow::anyhow!("Package '{}' is not installed", package_name));
    }

    for trove in &troves {
        print!("{} {}", trove.name, trove.version);

        if let Some(label_id) = trove.label_id {
            if let Some(label) = conary::db::models::LabelEntry::find_by_id(&conn, label_id)? {
                println!(" ({})", label);
            } else {
                println!(" (label id={} not found)", label_id);
            }
        } else {
            println!(" (no label)");
        }
    }

    Ok(())
}

/// Set the label for a package
pub fn cmd_label_set(package_name: &str, label_str: &str, db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    // Find the package
    let troves = conary::db::models::Trove::find_by_name(&conn, package_name)?;
    if troves.is_empty() {
        return Err(anyhow::anyhow!("Package '{}' is not installed", package_name));
    }

    // Find or create the label
    let spec = conary::Label::parse(label_str)
        .map_err(|e| anyhow::anyhow!("Invalid label format: {}", e))?;

    let mut label = conary::db::models::LabelEntry::from_spec(&spec);
    let label_id = label.insert_or_get(&conn)?;

    // Update all matching troves
    for trove in &troves {
        if let Some(trove_id) = trove.id {
            conn.execute(
                "UPDATE troves SET label_id = ?1 WHERE id = ?2",
                rusqlite::params![label_id, trove_id],
            )?;
            println!("Set label for {} {} to {}", trove.name, trove.version, label_str);
        }
    }

    Ok(())
}

/// Find packages by label
pub fn cmd_label_query(label_str: &str, db_path: &str) -> Result<()> {
    let conn = conary::db::open(db_path)?;

    // Find the label
    let label = conary::db::models::LabelEntry::find_by_string(&conn, label_str)?
        .ok_or_else(|| anyhow::anyhow!("Label '{}' not found", label_str))?;

    let label_id = label.id.ok_or_else(|| anyhow::anyhow!("Label has no ID"))?;

    // Query packages with this label
    let mut stmt = conn.prepare(
        "SELECT name, version, architecture FROM troves WHERE label_id = ?1 ORDER BY name, version"
    )?;

    let packages: Vec<(String, String, Option<String>)> = stmt
        .query_map([label_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    if packages.is_empty() {
        println!("No packages with label '{}'", label_str);
        return Ok(());
    }

    println!("Packages with label '{}' ({}):", label_str, packages.len());
    for (name, version, arch) in &packages {
        print!("  {} {}", name, version);
        if let Some(a) = arch {
            print!(" [{}]", a);
        }
        println!();
    }

    Ok(())
}
