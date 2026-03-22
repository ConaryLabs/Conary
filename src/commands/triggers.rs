// src/commands/triggers.rs

//! Trigger management commands

use super::open_db;
use anyhow::Result;
use conary_core::db::models::{Trigger, TriggerDependency};
use tracing::info;

/// List all triggers
pub async fn cmd_trigger_list(
    db_path: &str,
    show_disabled: bool,
    show_builtin_only: bool,
) -> Result<()> {
    let conn = open_db(db_path)?;

    let triggers = if show_builtin_only {
        Trigger::list_builtin(&conn)?
    } else if show_disabled {
        Trigger::list_all(&conn)?
    } else {
        Trigger::list_enabled(&conn)?
    };

    if triggers.is_empty() {
        println!("No triggers found.");
        return Ok(());
    }

    println!("Triggers:");
    println!(
        "{:<25} {:<8} {:<8} {:<40}",
        "NAME", "ENABLED", "BUILTIN", "PATTERN"
    );
    println!("{}", "-".repeat(85));

    for trigger in &triggers {
        let enabled = if trigger.enabled { "yes" } else { "no" };
        let builtin = if trigger.builtin { "yes" } else { "no" };

        let pattern_display = if trigger.pattern.len() > 38 {
            format!("{}...", &trigger.pattern[..35])
        } else {
            trigger.pattern.clone()
        };

        println!(
            "{:<25} {:<8} {:<8} {:<40}",
            trigger.name, enabled, builtin, pattern_display
        );
    }

    println!("\nTotal: {} trigger(s)", triggers.len());
    Ok(())
}

/// Show details of a specific trigger
pub async fn cmd_trigger_show(name: &str, db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;

    let trigger = Trigger::find_by_name(&conn, name)?
        .ok_or_else(|| anyhow::anyhow!("Trigger '{}' not found", name))?;

    println!("Trigger: {}", trigger.name);
    if let Some(desc) = &trigger.description {
        println!("  Description: {}", desc);
    }
    println!("  Pattern: {}", trigger.pattern);
    println!("  Handler: {}", trigger.handler);
    println!("  Priority: {}", trigger.priority);
    println!("  Enabled: {}", if trigger.enabled { "yes" } else { "no" });
    println!("  Built-in: {}", if trigger.builtin { "yes" } else { "no" });

    if let Some(id) = trigger.id {
        let deps = TriggerDependency::get_dependencies(&conn, id)?;
        if !deps.is_empty() {
            println!("  Dependencies: {}", deps.join(", "));
        }
    }

    println!("\n  Pattern breakdown:");
    for pattern in trigger.patterns() {
        println!("    - {}", pattern);
    }

    Ok(())
}

/// Enable a trigger
pub async fn cmd_trigger_enable(name: &str, db_path: &str) -> Result<()> {
    set_trigger_enabled(name, db_path, true)
}

/// Disable a trigger
pub async fn cmd_trigger_disable(name: &str, db_path: &str) -> Result<()> {
    set_trigger_enabled(name, db_path, false)
}

fn set_trigger_enabled(name: &str, db_path: &str, enable: bool) -> Result<()> {
    let conn = open_db(db_path)?;

    let trigger = Trigger::find_by_name(&conn, name)?
        .ok_or_else(|| anyhow::anyhow!("Trigger '{}' not found", name))?;

    if trigger.enabled == enable {
        let state = if enable { "enabled" } else { "disabled" };
        println!("Trigger '{}' is already {}.", name, state);
        return Ok(());
    }

    let id = trigger
        .id
        .ok_or_else(|| anyhow::anyhow!("Trigger has no ID"))?;

    if enable {
        Trigger::enable(&conn, id)?;
    } else {
        Trigger::disable(&conn, id)?;
    }

    let action = if enable { "Enabled" } else { "Disabled" };
    info!("{} trigger: {}", action, name);
    println!("{} trigger: {}", action, name);
    Ok(())
}

/// Add a new custom trigger
pub async fn cmd_trigger_add(
    name: &str,
    pattern: &str,
    handler: &str,
    description: Option<&str>,
    priority: Option<i32>,
    db_path: &str,
) -> Result<()> {
    let conn = open_db(db_path)?;

    if Trigger::find_by_name(&conn, name)?.is_some() {
        return Err(anyhow::anyhow!("Trigger '{}' already exists", name));
    }

    let mut trigger = Trigger::new(name.to_string(), pattern.to_string(), handler.to_string());

    if let Some(desc) = description {
        trigger.description = Some(desc.to_string());
    }
    if let Some(prio) = priority {
        trigger.priority = prio;
    }

    trigger.insert(&conn)?;

    info!("Created trigger: {} -> {}", name, handler);
    println!("Created trigger: {}", name);
    println!("  Pattern: {}", pattern);
    println!("  Handler: {}", handler);
    if let Some(prio) = priority {
        println!("  Priority: {}", prio);
    }

    Ok(())
}

/// Remove a custom trigger (built-in triggers cannot be removed)
pub async fn cmd_trigger_remove(name: &str, db_path: &str) -> Result<()> {
    let conn = open_db(db_path)?;

    let trigger = Trigger::find_by_name(&conn, name)?
        .ok_or_else(|| anyhow::anyhow!("Trigger '{}' not found", name))?;

    if trigger.builtin {
        return Err(anyhow::anyhow!(
            "Cannot remove built-in trigger '{}'. Use 'conary trigger-disable {}' instead.",
            name,
            name
        ));
    }

    let id = trigger
        .id
        .ok_or_else(|| anyhow::anyhow!("Trigger has no ID"))?;
    if Trigger::delete(&conn, id)? {
        info!("Removed trigger: {}", name);
        println!("Removed trigger: {}", name);
    } else {
        println!("Failed to remove trigger: {}", name);
    }

    Ok(())
}

/// Run pending triggers for a changeset (useful for manual re-runs)
pub async fn cmd_trigger_run(changeset_id: Option<i64>, db_path: &str, root: &str) -> Result<()> {
    let conn = open_db(db_path)?;

    let cs_id = if let Some(id) = changeset_id {
        id
    } else {
        let mut stmt = conn.prepare("SELECT id FROM changesets ORDER BY id DESC LIMIT 1")?;
        stmt.query_row([], |row| row.get(0)).map_err(|_| {
            anyhow::anyhow!("No changesets found. Install or remove a package first.")
        })?
    };

    println!("Running triggers for changeset {}...", cs_id);

    let executor = conary_core::trigger::TriggerExecutor::new(&conn, std::path::Path::new(root));
    let results = executor.execute_pending(cs_id)?;

    if results.total() == 0 {
        println!("No pending triggers for changeset {}", cs_id);
    } else {
        println!("\nTrigger execution complete:");
        println!("  Succeeded: {}", results.succeeded);
        println!("  Failed: {}", results.failed);
        println!("  Skipped: {}", results.skipped);

        if !results.errors.is_empty() {
            println!("\nErrors:");
            for error in &results.errors {
                println!("  - {}", error);
            }
        }
    }

    Ok(())
}
