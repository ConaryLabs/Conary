// src/commands/model/diff.rs

use std::path::Path;

use super::context::load_model_and_diff;
use super::presentation::{
    is_replatform_action, is_source_policy_action, print_source_policy_and_replatform,
    render_replatform_summary,
};
use anyhow::Result;
use conary_core::model::DiffAction;

/// Show what changes are needed to reach the model state
pub async fn cmd_model_diff(model_path: &str, db_path: &str, offline: bool) -> Result<()> {
    let model_path = Path::new(model_path);
    let (_model, conn, diff) = load_model_and_diff(model_path, db_path, offline, true).await?;
    let summary = diff.summary();

    if diff.is_empty() {
        println!("System is in sync with model - no changes needed");
        return Ok(());
    }

    println!("Changes needed to reach model state:");
    println!();

    // Group actions by type for cleaner output
    let installs: Vec<_> = diff
        .actions
        .iter()
        .filter(|a| matches!(a, DiffAction::Install { .. }))
        .collect();
    let removes: Vec<_> = diff
        .actions
        .iter()
        .filter(|a| matches!(a, DiffAction::Remove { .. }))
        .collect();
    let replatforms: Vec<_> = diff
        .actions
        .iter()
        .filter(|a| is_replatform_action(a))
        .collect();
    let others: Vec<_> = diff
        .actions
        .iter()
        .filter(|a| {
            !matches!(a, DiffAction::Install { .. } | DiffAction::Remove { .. })
                && !is_source_policy_action(a)
                && !is_replatform_action(a)
        })
        .collect();
    let source_policy: Vec<_> = diff
        .actions
        .iter()
        .filter(|a| is_source_policy_action(a))
        .collect();

    if !installs.is_empty() {
        println!("To install ({}):", installs.len());
        for action in &installs {
            println!("  + {}", action.description());
        }
        println!();
    }

    if !removes.is_empty() {
        println!("To remove ({}):", removes.len());
        for action in &removes {
            println!("  - {}", action.description());
        }
        println!();
    }

    if !source_policy.is_empty() {
        println!("Source policy changes ({}):", source_policy.len());
        for action in &source_policy {
            println!("  ~ {}", action.description());
        }
        println!();
    }

    if !replatforms.is_empty() {
        println!("Replatform proposals ({}):", replatforms.len());
        for action in &replatforms {
            println!("  > {}", action.description());
        }
        println!();
    }

    if !others.is_empty() {
        println!("Other changes ({}):", others.len());
        for action in &others {
            println!("  * {}", action.description());
        }
        println!();
    }

    // Print warnings
    if !diff.warnings.is_empty() {
        println!("Warnings:");
        for warning in &diff.warnings {
            println!("  ! {}", warning);
        }
        println!();
    }

    print_source_policy_and_replatform(&conn, &diff)?;

    println!(
        "Summary: {} install(s), {} remove(s), {} source policy change(s), {} other change(s)",
        summary.installs, summary.removes, summary.source_policy_changes, summary.other_changes
    );
    if let Some(replatform) = render_replatform_summary(&summary) {
        println!("{}", replatform);
    }

    Ok(())
}
