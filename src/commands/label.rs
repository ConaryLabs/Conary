// src/commands/label.rs

//! Label management CLI commands
//!
//! Commands for managing Conary-style labels (repository@namespace:tag)
//! for package provenance tracking.

use super::open_db;
use anyhow::{Context, Result};
use std::collections::HashSet;
use tracing::info;

/// List all labels
pub async fn cmd_label_list(db_path: &str, verbose: bool) -> Result<()> {
    let conn = open_db(db_path)?;

    let labels = conary_core::db::models::LabelEntry::list_all(&conn)?;

    if labels.is_empty() {
        println!("No labels defined.");
        println!("\nUse 'conary query label add <label>' to add a label.");
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
                    conary_core::db::models::LabelEntry::find_by_id(&conn, parent_id)
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
pub async fn cmd_label_add(
    label_str: &str,
    description: Option<&str>,
    parent: Option<&str>,
    db_path: &str,
) -> Result<()> {
    let conn = open_db(db_path)?;

    // Parse the label
    let spec = conary_core::Label::parse(label_str)
        .map_err(|e| anyhow::anyhow!("Invalid label format: {}", e))?;

    // Check if already exists
    if let Some(existing) = conary_core::db::models::LabelEntry::find_by_spec(
        &conn,
        &spec.repository,
        &spec.namespace,
        &spec.tag,
    )? {
        println!(
            "Label '{}' already exists (id={})",
            label_str,
            existing.id.unwrap_or(0)
        );
        return Ok(());
    }

    // Find parent label if specified
    let parent_id = if let Some(parent_str) = parent {
        let parent_entry = conary_core::db::models::LabelEntry::find_by_string(&conn, parent_str)?
            .ok_or_else(|| anyhow::anyhow!("Parent label '{}' not found", parent_str))?;
        parent_entry.id
    } else {
        None
    };

    // Create the label
    let mut label = conary_core::db::models::LabelEntry::from_spec(&spec);
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
pub async fn cmd_label_remove(label_str: &str, db_path: &str, force: bool) -> Result<()> {
    let conn = open_db(db_path)?;

    // Find the label
    let label = conary_core::db::models::LabelEntry::find_by_string(&conn, label_str)?
        .ok_or_else(|| anyhow::anyhow!("Label '{}' not found", label_str))?;

    let label_id = label.id.ok_or_else(|| anyhow::anyhow!("Label has no ID"))?;

    // Check if any packages use this label
    let pkg_count = label.package_count(&conn)?;
    if pkg_count > 0 && !force {
        return Err(anyhow::anyhow!(
            "Cannot remove label '{}': {} package(s) use it. Use --force to override.",
            label_str,
            pkg_count
        ));
    }

    // Check if any child labels exist
    let children = label.children(&conn)?;
    if !children.is_empty() && !force {
        let child_names: Vec<String> = children.iter().map(|c| c.to_string()).collect();
        return Err(anyhow::anyhow!(
            "Cannot remove label '{}': has child labels ({}). Use --force to override.",
            label_str,
            child_names.join(", ")
        ));
    }

    // Remove from label path if present
    conary_core::db::models::remove_from_path(&conn, label_id)?;

    // Delete the label
    conary_core::db::models::LabelEntry::delete(&conn, label_id)?;

    println!("Removed label: {}", label_str);
    if pkg_count > 0 {
        println!("  Warning: {} package(s) no longer have a label", pkg_count);
    }

    Ok(())
}

/// Show or modify the label path
pub async fn cmd_label_path(
    db_path: &str,
    add: Option<&str>,
    remove: Option<&str>,
    priority: Option<i32>,
) -> Result<()> {
    let conn = open_db(db_path)?;

    // Handle modifications
    if let Some(label_str) = add {
        let label = conary_core::db::models::LabelEntry::find_by_string(&conn, label_str)?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Label '{}' not found. Add it first with 'conary query label add'.",
                    label_str
                )
            })?;

        let label_id = label.id.ok_or_else(|| anyhow::anyhow!("Label has no ID"))?;
        let prio = priority.unwrap_or(50); // Default priority

        conary_core::db::models::add_to_path(&conn, label_id, prio)?;
        println!("Added '{}' to label path with priority {}", label_str, prio);
        return Ok(());
    }

    if let Some(label_str) = remove {
        let label = conary_core::db::models::LabelEntry::find_by_string(&conn, label_str)?
            .ok_or_else(|| anyhow::anyhow!("Label '{}' not found", label_str))?;

        let label_id = label.id.ok_or_else(|| anyhow::anyhow!("Label has no ID"))?;

        conary_core::db::models::remove_from_path(&conn, label_id)?;
        println!("Removed '{}' from label path", label_str);
        return Ok(());
    }

    // Show current label path
    let path_entries = conary_core::db::models::LabelPathEntry::list_ordered(&conn)?;

    if path_entries.is_empty() {
        println!("Label path is empty.");
        println!("\nUse 'conary query label path --add <label>' to add labels to the path.");
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
pub async fn cmd_label_show(package_name: &str, db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;

    let troves = conary_core::db::models::Trove::find_by_name(&conn, package_name)?;
    if troves.is_empty() {
        return Err(anyhow::anyhow!(
            "Package '{}' is not installed",
            package_name
        ));
    }

    for trove in &troves {
        print!("{} {}", trove.name, trove.version);

        if let Some(label_id) = trove.label_id {
            if let Some(label) = conary_core::db::models::LabelEntry::find_by_id(&conn, label_id)? {
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
pub async fn cmd_label_set(package_name: &str, label_str: &str, db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;

    // Find the package
    let troves = conary_core::db::models::Trove::find_by_name(&conn, package_name)?;
    if troves.is_empty() {
        return Err(anyhow::anyhow!(
            "Package '{}' is not installed",
            package_name
        ));
    }

    // Find or create the label
    let spec = conary_core::Label::parse(label_str)
        .map_err(|e| anyhow::anyhow!("Invalid label format: {}", e))?;

    let mut label = conary_core::db::models::LabelEntry::from_spec(&spec);
    let label_id = label.insert_or_get(&conn)?;

    // Update all matching troves
    for trove in &troves {
        if let Some(trove_id) = trove.id {
            conn.execute(
                "UPDATE troves SET label_id = ?1 WHERE id = ?2",
                rusqlite::params![label_id, trove_id],
            )
            .with_context(|| format!("Failed to set label for package '{}'", trove.name))?;
            println!(
                "Set label for {} {} to {}",
                trove.name, trove.version, label_str
            );
        }
    }

    Ok(())
}

/// Find packages by label
pub async fn cmd_label_query(label_str: &str, db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;

    // Find the label
    let label = conary_core::db::models::LabelEntry::find_by_string(&conn, label_str)?
        .ok_or_else(|| anyhow::anyhow!("Label '{}' not found", label_str))?;

    let label_id = label.id.ok_or_else(|| anyhow::anyhow!("Label has no ID"))?;

    // Query packages with this label
    let mut stmt = conn.prepare(
        "SELECT name, version, architecture FROM troves WHERE label_id = ?1 ORDER BY name, version",
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

/// Link a label to a repository for federation
pub async fn cmd_label_link(
    label_str: &str,
    repository: Option<&str>,
    unlink: bool,
    db_path: &str,
) -> Result<()> {
    let conn = open_db(db_path)?;

    // Find the label
    let mut label = conary_core::db::models::LabelEntry::find_by_string(&conn, label_str)?
        .ok_or_else(|| anyhow::anyhow!("Label '{}' not found", label_str))?;

    if unlink {
        // Remove the repository link
        if label.repository_id.is_none() {
            println!("Label '{}' is not linked to any repository", label_str);
            return Ok(());
        }

        label.set_repository(&conn, None)?;
        println!("Unlinked label '{}' from repository", label_str);
        return Ok(());
    }

    let repo_name =
        repository.ok_or_else(|| anyhow::anyhow!("Repository name required (or use --unlink)"))?;

    // Find the repository
    let repo = conary_core::db::models::Repository::find_by_name(&conn, repo_name)?
        .ok_or_else(|| anyhow::anyhow!("Repository '{}' not found", repo_name))?;

    let repo_id = repo
        .id
        .ok_or_else(|| anyhow::anyhow!("Repository has no ID"))?;

    // Set the repository link
    label.set_repository(&conn, Some(repo_id))?;

    println!("Linked label '{}' to repository '{}'", label_str, repo_name);
    println!(
        "Packages resolved through this label will come from '{}'",
        repo_name
    );

    Ok(())
}

/// Set up delegation from one label to another
pub async fn cmd_label_delegate(
    label_str: &str,
    target: Option<&str>,
    undelegate: bool,
    db_path: &str,
) -> Result<()> {
    let conn = open_db(db_path)?;

    // Find the source label
    let mut label = conary_core::db::models::LabelEntry::find_by_string(&conn, label_str)?
        .ok_or_else(|| anyhow::anyhow!("Label '{}' not found", label_str))?;

    if undelegate {
        // Remove the delegation
        if label.delegate_to_label_id.is_none() {
            println!("Label '{}' does not delegate to another label", label_str);
            return Ok(());
        }

        label.set_delegate(&conn, None)?;
        println!("Removed delegation from label '{}'", label_str);
        return Ok(());
    }

    let target_str =
        target.ok_or_else(|| anyhow::anyhow!("Target label required (or use --undelegate)"))?;

    // Find the target label
    let target_label = conary_core::db::models::LabelEntry::find_by_string(&conn, target_str)?
        .ok_or_else(|| anyhow::anyhow!("Target label '{}' not found", target_str))?;

    let target_id = target_label
        .id
        .ok_or_else(|| anyhow::anyhow!("Target label has no ID"))?;

    // Check for self-delegation
    if label.id == Some(target_id) {
        return Err(anyhow::anyhow!("Cannot delegate label to itself"));
    }

    // Check for delegation cycles by walking the full chain from the target
    {
        let mut visited = HashSet::new();
        if let Some(source_id) = label.id {
            visited.insert(source_id);
        }
        visited.insert(target_id);
        let mut current_id = target_label.delegate_to_label_id;
        while let Some(next_id) = current_id {
            if !visited.insert(next_id) {
                return Err(anyhow::anyhow!(
                    "Circular delegation detected: setting '{}' -> '{}' would create a cycle",
                    label_str,
                    target_str
                ));
            }
            current_id = conary_core::db::models::LabelEntry::find_by_id(&conn, next_id)?
                .and_then(|l| l.delegate_to_label_id);
        }
    }

    // Set the delegation
    label.set_delegate(&conn, Some(target_id))?;

    println!("Label '{}' now delegates to '{}'", label_str, target_str);
    println!(
        "Packages resolved through '{}' will be fetched from '{}'",
        label_str, target_str
    );

    Ok(())
}
