// src/commands/state.rs
//! System state snapshot management commands

use anyhow::{Context, Result};
use conary::db::models::{StateDiff, StateEngine, SystemState};
use tracing::info;

/// List all system states
pub fn cmd_state_list(db_path: &str, limit: Option<i64>) -> Result<()> {
    info!("Listing system states...");

    let conn = conary::db::open(db_path)
        .context("Failed to open package database")?;

    let states = if let Some(n) = limit {
        SystemState::list_recent(&conn, n)?
    } else {
        SystemState::list_all(&conn)?
    };

    if states.is_empty() {
        println!("No system states recorded.");
        println!("\nStates are created automatically after install/remove operations.");
        return Ok(());
    }

    println!("System States:");
    println!("{:>6}  {:>8}  {:20}  SUMMARY", "STATE", "PACKAGES", "CREATED");
    println!("{}", "-".repeat(70));

    for state in &states {
        let active_marker = if state.is_active { "*" } else { " " };
        let created = state.created_at.as_deref().unwrap_or("unknown");
        // Truncate to date/time portion
        let created_short = if created.len() > 19 { &created[..19] } else { created };

        println!(
            "{:>5}{} {:>8}  {:20}  {}",
            state.state_number,
            active_marker,
            state.package_count,
            created_short,
            state.summary
        );
    }

    println!();
    println!("* = active state");
    println!("Total: {} state(s)", states.len());

    Ok(())
}

/// Show details of a specific state
pub fn cmd_state_show(db_path: &str, state_number: i64) -> Result<()> {
    info!("Showing state {}...", state_number);

    let conn = conary::db::open(db_path)
        .context("Failed to open package database")?;

    let state = SystemState::find_by_number(&conn, state_number)?
        .ok_or_else(|| anyhow::anyhow!("State {} not found", state_number))?;

    println!("State {}", state.state_number);
    println!("{}", "=".repeat(40));
    println!("Summary:     {}", state.summary);
    if let Some(desc) = &state.description {
        println!("Description: {}", desc);
    }
    println!("Created:     {}", state.created_at.as_deref().unwrap_or("unknown"));
    println!("Packages:    {}", state.package_count);
    println!("Active:      {}", if state.is_active { "Yes" } else { "No" });
    if let Some(cs_id) = state.changeset_id {
        println!("Changeset:   {}", cs_id);
    }

    // Show packages in this state
    let members = state.get_members(&conn)?;
    if !members.is_empty() {
        println!("\nPackages ({}):", members.len());
        for member in &members {
            let arch = member.architecture.as_deref().unwrap_or("");
            let reason = member.install_reason.as_str();
            let marker = if reason == "dependency" { " (dep)" } else { "" };
            println!("  {} {} [{}]{}", member.trove_name, member.trove_version, arch, marker);
        }
    }

    Ok(())
}

/// Show diff between two states
pub fn cmd_state_diff(db_path: &str, from_state: i64, to_state: i64) -> Result<()> {
    info!("Comparing states {} -> {}...", from_state, to_state);

    let conn = conary::db::open(db_path)
        .context("Failed to open package database")?;

    let from = SystemState::find_by_number(&conn, from_state)?
        .ok_or_else(|| anyhow::anyhow!("State {} not found", from_state))?;
    let to = SystemState::find_by_number(&conn, to_state)?
        .ok_or_else(|| anyhow::anyhow!("State {} not found", to_state))?;

    let from_id = from.id.ok_or_else(|| anyhow::anyhow!("State has no ID"))?;
    let to_id = to.id.ok_or_else(|| anyhow::anyhow!("State has no ID"))?;

    let diff = StateDiff::compare(&conn, from_id, to_id)?;

    println!("State Diff: {} -> {}", from_state, to_state);
    println!("{}", "=".repeat(50));

    if diff.is_empty() {
        println!("No differences between states.");
        return Ok(());
    }

    if !diff.added.is_empty() {
        println!("\nAdded ({}):", diff.added.len());
        for member in &diff.added {
            println!("  + {} {}", member.trove_name, member.trove_version);
        }
    }

    if !diff.removed.is_empty() {
        println!("\nRemoved ({}):", diff.removed.len());
        for member in &diff.removed {
            println!("  - {} {}", member.trove_name, member.trove_version);
        }
    }

    if !diff.upgraded.is_empty() {
        println!("\nChanged ({}):", diff.upgraded.len());
        for (old, new) in &diff.upgraded {
            println!("  ~ {} {} -> {}", old.trove_name, old.trove_version, new.trove_version);
        }
    }

    println!("\nTotal changes: {}", diff.change_count());

    Ok(())
}

/// Restore to a previous state
pub fn cmd_state_restore(db_path: &str, state_number: i64, dry_run: bool) -> Result<()> {
    info!("Restoring to state {}...", state_number);

    let conn = conary::db::open(db_path)
        .context("Failed to open package database")?;

    let target = SystemState::find_by_number(&conn, state_number)?
        .ok_or_else(|| anyhow::anyhow!("State {} not found", state_number))?;

    let target_id = target.id.ok_or_else(|| anyhow::anyhow!("State has no ID"))?;

    let engine = StateEngine::new(&conn);
    let plan = engine.plan_restore(target_id)?;

    if plan.is_empty() {
        println!("System is already at state {}.", state_number);
        return Ok(());
    }

    println!("Restore Plan: State {} -> State {}", plan.from_state.state_number, plan.to_state.state_number);
    println!("{}", "=".repeat(50));

    if !plan.to_remove.is_empty() {
        println!("\nPackages to remove ({}):", plan.to_remove.len());
        for member in &plan.to_remove {
            println!("  - {} {}", member.trove_name, member.trove_version);
        }
    }

    if !plan.to_install.is_empty() {
        println!("\nPackages to install ({}):", plan.to_install.len());
        for member in &plan.to_install {
            println!("  + {} {}", member.trove_name, member.trove_version);
        }
    }

    if !plan.to_upgrade.is_empty() {
        println!("\nPackages to change ({}):", plan.to_upgrade.len());
        for (old, new) in &plan.to_upgrade {
            println!("  ~ {} {} -> {}", old.trove_name, old.trove_version, new.trove_version);
        }
    }

    println!("\nTotal operations: {}", plan.operation_count());

    if dry_run {
        println!("\nDry run - no changes made.");
        println!("Run without --dry-run to apply these changes.");
        return Ok(());
    }

    // For now, just show the plan - actual restore requires more infrastructure
    // to download/install packages that aren't locally available
    println!("\nNote: Full state restore is not yet implemented.");
    println!("Use 'conary rollback' to reverse individual changesets,");
    println!("or manually install/remove packages to match the target state.");

    Ok(())
}

/// Prune old states, keeping only the most recent N
pub fn cmd_state_prune(db_path: &str, keep_count: i64, dry_run: bool) -> Result<()> {
    info!("Pruning states, keeping {} most recent...", keep_count);

    if keep_count < 1 {
        return Err(anyhow::anyhow!("Must keep at least 1 state"));
    }

    let conn = conary::db::open(db_path)
        .context("Failed to open package database")?;

    let all_states = SystemState::list_all(&conn)?;
    let total_count = all_states.len() as i64;

    if total_count <= keep_count {
        println!("Only {} state(s) exist, nothing to prune.", total_count);
        return Ok(());
    }

    let to_prune = total_count - keep_count;

    // Show states that would be pruned
    let prune_candidates: Vec<_> = all_states
        .iter()
        .rev()  // Oldest first
        .take(to_prune as usize)
        .filter(|s| !s.is_active)  // Never prune active state
        .collect();

    if prune_candidates.is_empty() {
        println!("No states to prune (active state is protected).");
        return Ok(());
    }

    println!("States to prune ({}):", prune_candidates.len());
    for state in &prune_candidates {
        println!("  State {}: {} ({})", state.state_number, state.summary,
            state.created_at.as_deref().unwrap_or("unknown"));
    }

    if dry_run {
        println!("\nDry run - no states will be deleted.");
        return Ok(());
    }

    let engine = StateEngine::new(&conn);
    let deleted = engine.prune(keep_count)?;

    println!("\nPruned {} state(s). Keeping {} most recent.", deleted, keep_count);

    Ok(())
}

/// Create a manual state snapshot
pub fn cmd_state_create(db_path: &str, summary: &str, description: Option<&str>) -> Result<()> {
    info!("Creating manual state snapshot...");

    let conn = conary::db::open(db_path)
        .context("Failed to open package database")?;

    let engine = StateEngine::new(&conn);
    let state = engine.create_snapshot(summary, description, None)?;

    println!("Created state {}", state.state_number);
    println!("  Summary:  {}", state.summary);
    println!("  Packages: {}", state.package_count);

    Ok(())
}
