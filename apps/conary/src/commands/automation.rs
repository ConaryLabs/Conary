// src/commands/automation.rs

//! Command implementations for automation system.

use super::open_db;
use anyhow::{Context, Result};
use conary_core::automation::{
    AutomationManager, AutomationSummary,
    action::{ActionExecutor, PlannedOp},
    check::AutomationChecker,
    prompt::{AutomationPrompt, SummaryResponse},
    scheduler::AutomationDaemon,
};
use conary_core::model::{
    AutomationCategory, AutomationConfig, DEFAULT_MODEL_PATH, load_model, model_exists,
};
use rusqlite::params;
use std::path::Path;
use toml_edit::{DocumentMut, Item, Table, value};

fn category_filter(categories: Option<Vec<String>>) -> Option<Vec<AutomationCategory>> {
    categories.map(|cats| {
        cats.iter()
            .filter_map(|c| match c.to_lowercase().as_str() {
                "security" => Some(AutomationCategory::Security),
                "orphans" => Some(AutomationCategory::Orphans),
                "updates" => Some(AutomationCategory::Updates),
                "major_upgrades" | "major-upgrades" => Some(AutomationCategory::MajorUpgrades),
                "integrity" | "repair" => Some(AutomationCategory::Repair),
                _ => None,
            })
            .collect()
    })
}

fn category_key(category: AutomationCategory) -> &'static str {
    match category {
        AutomationCategory::Security => "security",
        AutomationCategory::Orphans => "orphans",
        AutomationCategory::Updates => "updates",
        AutomationCategory::MajorUpgrades => "major_upgrades",
        AutomationCategory::Repair => "repair",
    }
}

fn format_planned_op(op: &PlannedOp) -> String {
    match op {
        PlannedOp::Install {
            package,
            version,
            architecture,
        } => format!(
            "install {}{}{}",
            package,
            version
                .as_ref()
                .map(|v| format!(" version {v}"))
                .unwrap_or_default(),
            architecture
                .as_ref()
                .map(|arch| format!(" [{arch}]"))
                .unwrap_or_default()
        ),
        PlannedOp::Remove {
            package,
            version,
            architecture,
        } => format!(
            "remove {}{}{}",
            package,
            version
                .as_ref()
                .map(|v| format!(" version {v}"))
                .unwrap_or_default(),
            architecture
                .as_ref()
                .map(|arch| format!(" [{arch}]"))
                .unwrap_or_default()
        ),
        PlannedOp::Restore {
            package,
            version,
            architecture,
        } => format!(
            "restore {}{}{}",
            package,
            version
                .as_ref()
                .map(|v| format!(" version {v}"))
                .unwrap_or_default(),
            architecture
                .as_ref()
                .map(|arch| format!(" [{arch}]"))
                .unwrap_or_default()
        ),
    }
}

async fn execute_planned_op(op: &PlannedOp, db_path: &str, root: &str) -> Result<()> {
    match op {
        PlannedOp::Install {
            package,
            version,
            architecture,
        } => {
            super::cmd_install(
                package,
                super::InstallOptions {
                    db_path,
                    root,
                    version: version.clone(),
                    package_release: None,
                    repo: None,
                    architecture: architecture.clone(),
                    dry_run: false,
                    no_deps: false,
                    selection_reason: None,
                    sandbox_mode: super::SandboxMode::Always,
                    allow_downgrade: false,
                    convert_to_ccs: false,
                    ownership: None,
                    yes: true,
                    from_source: None,
                    repository_provenance: None,
                },
            )
            .await
        }
        PlannedOp::Remove {
            package,
            version,
            architecture,
        } => {
            super::cmd_remove(
                package,
                db_path,
                version.clone(),
                architecture.clone(),
                super::SandboxMode::Always,
                false,
            )
            .await
        }
        PlannedOp::Restore {
            package,
            version,
            architecture,
        } => {
            super::cmd_restore(
                package,
                db_path,
                root,
                version.clone(),
                architecture.clone(),
                false,
                false,
            )
            .await
        }
    }
}

fn insert_history_row(
    conn: &rusqlite::Connection,
    action: &conary_core::automation::PendingAction,
    status: &str,
    error_message: Option<&str>,
) -> Result<()> {
    let packages_json = serde_json::to_string(&action.packages)?;
    conn.execute(
        "INSERT INTO automation_history (action_id, category, packages, status, error_message)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            action.id,
            category_key(action.category),
            packages_json,
            status,
            error_message
        ],
    )?;
    Ok(())
}

async fn execute_actions(
    conn: &rusqlite::Connection,
    actions: &[conary_core::automation::PendingAction],
    db_path: &str,
    root: &str,
) -> Result<(usize, usize, usize)> {
    let planner = ActionExecutor::new();
    let mut applied = 0;
    let mut failed = 0;
    let mut partial = 0;

    for action in actions {
        println!("  Applying: {}", action.summary);
        let plan = match planner.plan(action) {
            Ok(plan) => plan,
            Err(e) => {
                let message = e.to_string();
                insert_history_row(conn, action, "failed", Some(&message))?;
                let row = format!("{}: {}", action.summary, message);
                crate::ui::row(crate::ui::Status::Fail, &[&row]);
                failed += 1;
                continue;
            }
        };

        let mut succeeded_ops = 0usize;
        let mut errors = Vec::new();
        for op in &plan.ops {
            if let Err(e) = execute_planned_op(op, db_path, root).await {
                errors.push(format!("{}: {}", format_planned_op(op), e));
            } else {
                succeeded_ops += 1;
            }
        }

        let (status, error_message) = if succeeded_ops == plan.ops.len() {
            applied += 1;
            ("applied", None)
        } else if succeeded_ops == 0 {
            failed += 1;
            ("failed", Some(errors.join("; ")))
        } else {
            partial += 1;
            ("partial", Some(errors.join("; ")))
        };

        insert_history_row(conn, action, status, error_message.as_deref())?;

        match error_message {
            Some(message) => println!("  [{}] {}", status.to_uppercase(), message),
            None => crate::ui::row(crate::ui::Status::Ok, &[&action.summary]),
        }
    }

    Ok((applied, failed, partial))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AutomationHistoryRow {
    action_id: String,
    category: String,
    packages: Vec<String>,
    status: String,
    error_message: Option<String>,
    applied_at: String,
}

fn query_automation_history(
    conn: &rusqlite::Connection,
    limit: usize,
    category: Option<&str>,
    status: Option<&str>,
    since: Option<&str>,
) -> Result<Vec<AutomationHistoryRow>> {
    let mut sql = String::from(
        "SELECT action_id, category, packages, status, error_message, applied_at
         FROM automation_history",
    );
    let mut clauses = Vec::new();
    let mut params = Vec::new();

    if let Some(category) = category {
        clauses.push("category = ?");
        params.push(category.to_string());
    }
    if let Some(status) = status {
        clauses.push("status = ?");
        params.push(status.to_string());
    }
    if let Some(since) = since {
        clauses.push("applied_at >= ?");
        params.push(since.to_string());
    }

    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }

    sql.push_str(" ORDER BY applied_at DESC, id DESC LIMIT ?");
    params.push(limit.to_string());

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;

    rows.map(|row| {
        let (action_id, category, packages_json, status, error_message, applied_at) = row?;
        let packages = match packages_json {
            Some(raw) => serde_json::from_str::<Vec<String>>(&raw)
                .with_context(|| format!("automation history {action_id} has corrupt packages"))?,
            None => Vec::new(),
        };
        Ok(AutomationHistoryRow {
            action_id,
            category,
            packages,
            status,
            error_message,
            applied_at,
        })
    })
    .collect()
}

fn load_automation_config_from_path(path: &Path) -> Result<AutomationConfig> {
    if !path.exists() {
        return Ok(AutomationConfig::default());
    }
    let model = load_model(Some(path))?;
    Ok(model.automation)
}

fn ensure_table_item(parent: &mut Item, key: &str) -> Result<()> {
    let table = parent
        .as_table_like_mut()
        .ok_or_else(|| anyhow::anyhow!("expected TOML table when updating automation config"))?;

    let needs_table = match table.get(key) {
        Some(item) => !item.is_table(),
        None => true,
    };

    if needs_table {
        table.insert(key, Item::Table(Table::new()));
    }

    Ok(())
}

fn normalize_category_key(category: &str) -> Option<&'static str> {
    match category {
        "security" => Some("security"),
        "orphans" => Some("orphans"),
        "updates" => Some("updates"),
        "major_upgrades" | "major-upgrades" => Some("major_upgrades"),
        "repair" | "integrity" => Some("repair"),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn update_automation_config_file(
    path: &Path,
    _db_path: Option<&str>,
    mode: Option<&str>,
    enable: Option<&str>,
    disable: Option<&str>,
    interval: Option<&str>,
    enable_ai: bool,
    disable_ai: bool,
) -> Result<()> {
    let raw = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        "[model]\nversion = 1\n".to_string()
    };
    let mut doc = raw.parse::<DocumentMut>()?;

    ensure_table_item(doc.as_item_mut(), "automation")?;

    if let Some(mode) = mode {
        doc["automation"]["mode"] = value(mode);
    }
    if let Some(interval) = interval {
        doc["automation"]["check_interval"] = value(interval);
    }
    if let Some(category) = enable.and_then(normalize_category_key) {
        ensure_table_item(&mut doc["automation"], category)?;
        doc["automation"][category]["mode"] = value("auto");
    }
    if let Some(category) = disable.and_then(normalize_category_key) {
        ensure_table_item(&mut doc["automation"], category)?;
        doc["automation"][category]["mode"] = value("disabled");
    }
    if enable_ai || disable_ai {
        ensure_table_item(&mut doc["automation"], "ai_assist")?;
        doc["automation"]["ai_assist"]["enabled"] = value(enable_ai && !disable_ai);
    }

    std::fs::write(path, doc.to_string())?;
    Ok(())
}

fn build_status_json(summary: &AutomationSummary, config: &AutomationConfig) -> serde_json::Value {
    serde_json::json!({
        "total": summary.total,
        "security_updates": summary.security_updates,
        "available_updates": summary.available_updates,
        "orphaned_packages": summary.orphaned_packages,
        "major_upgrades": summary.major_upgrades,
        "integrity_issues": summary.integrity_issues,
        "mode": format!("{:?}", config.mode),
        "check_interval": config.check_interval,
    })
}

/// Show automation status
pub async fn cmd_automation_status(db_path: &str, format: &str, verbose: bool) -> Result<()> {
    let conn = open_db(db_path)?;

    // Load model to get automation config
    let config = if model_exists(None) {
        let model = load_model(None)?;
        model.automation.clone()
    } else {
        AutomationConfig::default()
    };

    let checker = AutomationChecker::new(&conn, &config);
    let results = checker.run_all()?;

    let summary = AutomationSummary {
        total: results.total(),
        security_updates: results.security.len(),
        available_updates: results.updates.len(),
        orphaned_packages: results.orphans.len(),
        major_upgrades: results.major_upgrades.len(),
        integrity_issues: results.integrity.len(),
    };

    match format {
        "json" => {
            let json = build_status_json(&summary, &config);
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
        _ => {
            println!("=== Automation Status ===\n");
            println!("Mode: {:?}", config.mode);
            println!("Check interval: {}", config.check_interval);
            println!();
            println!("{}", summary.status_line());

            if verbose && summary.total > 0 {
                println!();
                if !results.security.is_empty() {
                    println!("Security updates:");
                    for action in &results.security {
                        println!("  - {}", action.summary);
                    }
                }
                if !results.updates.is_empty() {
                    println!("Available updates:");
                    for action in &results.updates {
                        println!("  - {}", action.summary);
                    }
                }
                if !results.major_upgrades.is_empty() {
                    println!("Major upgrades:");
                    for action in &results.major_upgrades {
                        println!("  - {}", action.summary);
                    }
                }
                if !results.orphans.is_empty() {
                    println!("Orphaned packages:");
                    for action in &results.orphans {
                        println!("  - {}", action.summary);
                    }
                }
                if !results.integrity.is_empty() {
                    println!("Integrity issues:");
                    for action in &results.integrity {
                        println!("  - {}", action.summary);
                    }
                }
            }

            if !results.errors.is_empty() {
                println!("\nWarnings:");
                for err in &results.errors {
                    println!("  - {}", err);
                }
            }
        }
    }

    Ok(())
}

/// Check for automation actions
pub async fn cmd_automation_check(
    db_path: &str,
    _root: &str,
    categories: Option<Vec<String>>,
    quiet: bool,
) -> Result<()> {
    let conn = open_db(db_path)?;

    let config = if model_exists(None) {
        let model = load_model(None)?;
        model.automation.clone()
    } else {
        AutomationConfig::default()
    };

    let checker = AutomationChecker::new(&conn, &config);
    let results = checker.run_all()?;

    // Filter by categories if specified
    let filter = category_filter(categories);

    let show_category = |cat: &AutomationCategory| -> bool {
        match &filter {
            Some(cats) => cats.contains(cat),
            None => true,
        }
    };

    if quiet {
        if results.total() > 0 {
            anyhow::bail!("found {} actionable item(s)", results.total());
        }
        return Ok(());
    }

    println!("Found {} actionable item(s):", results.total());
    println!();

    if !results.security.is_empty() && show_category(&AutomationCategory::Security) {
        println!("[SECURITY] {} security update(s)", results.security.len());
        for action in &results.security {
            println!("  - {}", action.summary);
        }
        println!();
    }

    if !results.updates.is_empty() && show_category(&AutomationCategory::Updates) {
        println!("[UPDATES] {} package update(s)", results.updates.len());
        for action in &results.updates {
            println!("  - {}", action.summary);
        }
        println!();
    }

    if !results.major_upgrades.is_empty() && show_category(&AutomationCategory::MajorUpgrades) {
        println!(
            "[MAJOR UPGRADES] {} major upgrade(s)",
            results.major_upgrades.len()
        );
        for action in &results.major_upgrades {
            println!("  - {}", action.summary);
        }
        println!();
    }

    if !results.orphans.is_empty() && show_category(&AutomationCategory::Orphans) {
        println!("[ORPHANS] {} orphaned package(s)", results.orphans.len());
        for action in &results.orphans {
            println!("  - {}", action.summary);
        }
        println!();
    }

    if !results.integrity.is_empty() && show_category(&AutomationCategory::Repair) {
        println!("[INTEGRITY] {} issue(s)", results.integrity.len());
        for action in &results.integrity {
            println!("  - {}", action.summary);
        }
        println!();
    }

    if results.total() > 0 {
        println!("Run 'conary automation apply' to review and apply these changes.");
    }

    Ok(())
}

/// Apply pending automation actions
pub async fn cmd_automation_apply(
    db_path: &str,
    root: &str,
    yes: bool,
    categories: Option<Vec<String>>,
    dry_run: bool,
) -> Result<()> {
    let conn = open_db(db_path)?;

    let config = if model_exists(None) {
        let model = load_model(None)?;
        model.automation.clone()
    } else {
        AutomationConfig::default()
    };

    let checker = AutomationChecker::new(&conn, &config);
    let results = checker.run_all()?;

    if results.total() == 0 {
        println!("No pending automation actions.");
        return Ok(());
    }

    let category_filter = category_filter(categories);

    let mut manager = AutomationManager::new(config.clone());
    let all_actions: Vec<_> = results
        .all_actions()
        .into_iter()
        .filter(|a| match &category_filter {
            Some(cats) => cats.contains(&a.category),
            None => true,
        })
        .cloned()
        .collect();

    for action in &all_actions {
        manager.register_action(action.clone());
    }

    if dry_run {
        println!("Dry run - would apply {} action(s):", all_actions.len());
        for action in &all_actions {
            println!(
                "  - [{}] {}",
                action.category.display_name(),
                action.summary
            );
            let plan = ActionExecutor::new().plan(action)?;
            for op in &plan.ops {
                println!("      {}", format_planned_op(op));
            }
        }
        return Ok(());
    }

    if yes {
        println!("Applying {} action(s)...", all_actions.len());
        let (applied, failed, partial) =
            execute_actions(&conn, &all_actions, db_path, root).await?;

        println!();
        println!(
            "Complete: {} applied, {} failed, {} partial",
            applied, failed, partial
        );
        if failed > 0 || partial > 0 {
            anyhow::bail!(
                "automation apply completed with {} failed action(s) and {} partial action(s)",
                failed,
                partial
            );
        }
        return Ok(());
    }

    // Interactive mode
    let prompt = AutomationPrompt::detect();
    let summary = manager.summary();

    match prompt.show_summary(&summary)? {
        SummaryResponse::ApplyAll => {
            println!("Applying all actions...");
            let (applied, failed, partial) =
                execute_actions(&conn, &all_actions, db_path, root).await?;
            println!();
            println!(
                "Complete: {} applied, {} failed, {} partial",
                applied, failed, partial
            );
            if failed > 0 || partial > 0 {
                anyhow::bail!(
                    "automation apply completed with {} failed action(s) and {} partial action(s)",
                    failed,
                    partial
                );
            }
        }
        SummaryResponse::ReviewCategory(category) => {
            let actions = manager.pending_by_category(category);
            println!("Reviewing {} action(s)...", actions.len());
            let actions: Vec<_> = actions.into_iter().cloned().collect();
            let (applied, failed, partial) =
                execute_actions(&conn, &actions, db_path, root).await?;
            println!();
            println!(
                "Complete: {} applied, {} failed, {} partial",
                applied, failed, partial
            );
            if failed > 0 || partial > 0 {
                anyhow::bail!(
                    "automation apply completed with {} failed action(s) and {} partial action(s)",
                    failed,
                    partial
                );
            }
        }
        SummaryResponse::ShowDetails => {
            for action in manager.pending_actions() {
                println!();
                println!("--- {} ---", action.category.display_name());
                println!("{}", action.summary);
                for detail in &action.details {
                    println!("  {}", detail);
                }
            }
        }
        SummaryResponse::Configure => {
            println!("Run 'conary automation configure --show' to view current settings.");
        }
        SummaryResponse::Exit => {
            println!("No changes made.");
        }
    }

    Ok(())
}

/// Configure automation settings
#[allow(clippy::too_many_arguments)]
pub async fn cmd_automation_configure(
    _db_path: &str,
    show: bool,
    mode: Option<String>,
    enable: Option<String>,
    disable: Option<String>,
    interval: Option<String>,
    enable_ai: bool,
    disable_ai: bool,
) -> Result<()> {
    let model_path = Path::new(DEFAULT_MODEL_PATH);

    if show
        || (mode.is_none()
            && enable.is_none()
            && disable.is_none()
            && interval.is_none()
            && !enable_ai
            && !disable_ai)
    {
        let config = load_automation_config_from_path(model_path)?;
        println!("=== Automation Configuration ===\n");
        println!("Configuration file: {}\n", DEFAULT_MODEL_PATH);

        println!("Current settings:");
        println!("  Global mode: {:?}", config.mode);
        println!("  Check interval: {}", config.check_interval);
        println!();
        println!("Category overrides:");
        println!(
            "  Security: {}",
            config
                .security
                .mode
                .map(|mode| format!("{mode:?}"))
                .unwrap_or_else(|| "(inherits global)".to_string())
        );
        println!(
            "  Orphans: {}",
            config
                .orphans
                .mode
                .map(|mode| format!("{mode:?}"))
                .unwrap_or_else(|| "(inherits global)".to_string())
        );
        println!(
            "  Updates: {}",
            config
                .updates
                .mode
                .map(|mode| format!("{mode:?}"))
                .unwrap_or_else(|| "(inherits global)".to_string())
        );
        println!(
            "  Major upgrades: {}",
            config
                .major_upgrades
                .mode
                .map(|mode| format!("{mode:?}"))
                .unwrap_or_else(|| "(inherits global)".to_string())
        );
        println!(
            "  Repair: {}",
            config
                .repair
                .mode
                .map(|mode| format!("{mode:?}"))
                .unwrap_or_else(|| "(inherits global)".to_string())
        );
        println!();
        println!(
            "Deferred assistant config: {}",
            if config.ai_assist.enabled {
                "enabled"
            } else {
                "disabled"
            }
        );
        println!();
        println!("To modify, edit {} or use:", DEFAULT_MODEL_PATH);
        println!("  conary automation configure --mode auto");
        println!("  conary automation configure --enable security");
        println!("  conary automation configure --enable-ai");
        return Ok(());
    }

    update_automation_config_file(
        model_path,
        None,
        mode.as_deref(),
        enable.as_deref(),
        disable.as_deref(),
        interval.as_deref(),
        enable_ai,
        disable_ai,
    )?;

    println!("Updated automation configuration at {}", DEFAULT_MODEL_PATH);
    println!("Restart any running foreground automation daemon to pick up the new settings.");

    Ok(())
}

/// Run automation daemon
pub async fn cmd_automation_daemon(db_path: &str, _root: &str, pidfile: &str) -> Result<()> {
    let _conn = open_db(db_path)?;

    let config = if model_exists(None) {
        let model = load_model(None)?;
        model.automation.clone()
    } else {
        AutomationConfig::default()
    };

    // Write PID file
    if let Err(e) = std::fs::write(pidfile, std::process::id().to_string()) {
        tracing::warn!("Could not write PID file {}: {}", pidfile, e);
    }

    println!("Starting automation daemon (foreground mode)...");
    println!("PID: {}", std::process::id());
    println!("PID file: {}", pidfile);
    println!("Check interval: {}", config.check_interval);
    println!("Press Ctrl+C to stop.");
    println!();

    let mut daemon = AutomationDaemon::new(config.clone());
    println!("Status: {}", daemon.scheduler().status_line());
    println!();

    // Run the daemon loop (Ctrl+C will terminate the process)
    println!("Daemon running. Waiting for scheduled checks...\n");
    loop {
        if daemon.scheduler().should_run() && daemon.scheduler().within_window() {
            println!(
                "[{}] Running scheduled automation check...",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
            );

            // Run the actual check
            let checker = AutomationChecker::new(&_conn, &config);
            match checker.run_all() {
                Ok(results) => {
                    let summary = AutomationSummary {
                        total: results.total(),
                        security_updates: results.security.len(),
                        available_updates: results.updates.len(),
                        orphaned_packages: results.orphans.len(),
                        major_upgrades: results.major_upgrades.len(),
                        integrity_issues: results.integrity.len(),
                    };

                    if summary.total > 0 {
                        println!("  Found: {}", summary.status_line());
                    } else {
                        println!("  System up to date");
                    }
                }
                Err(e) => {
                    println!("  Error: {}", e);
                }
            }

            daemon.record_check();
            println!("  {}", daemon.scheduler().status_line());
            println!();
        }

        // Sleep briefly then check again
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
}

/// Show automation history recorded by `conary automation apply`.
pub async fn cmd_automation_history(
    db_path: &str,
    limit: usize,
    category: Option<String>,
    status: Option<String>,
    since: Option<String>,
) -> Result<()> {
    let conn = open_db(db_path)?;
    let rows = query_automation_history(
        &conn,
        limit,
        category.as_deref(),
        status.as_deref(),
        since.as_deref(),
    )?;

    if rows.is_empty() {
        println!("No automation history.");
        return Ok(());
    }

    println!("=== Automation History ===\n");
    for row in rows {
        let packages = if row.packages.is_empty() {
            "-".to_string()
        } else {
            row.packages.join(", ")
        };
        println!(
            "{}  {:<8}  {:<15}  {}",
            row.applied_at, row.status, row.category, packages
        );
        if let Some(error) = row.error_message {
            println!("  error: {}", error);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;
