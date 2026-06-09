// src/commands/model/check.rs

use std::path::Path;
use std::process;

use super::context::load_model_and_diff;
use super::presentation::{model_check_drift_headline, source_policy_replatform_note};
use anyhow::Result;

/// Check if system state matches the model
pub async fn cmd_model_check(
    model_path: &str,
    db_path: &str,
    verbose: bool,
    offline: bool,
) -> Result<()> {
    let model_path = Path::new(model_path);
    let (_model, _conn, diff) = load_model_and_diff(model_path, db_path, offline, false).await?;

    if diff.is_empty() {
        println!("OK: System matches model");
        return Ok(());
    }

    // System doesn't match model - report drift and exit with non-zero code
    if verbose {
        println!("DRIFT: System does not match model");
        println!();
        for action in &diff.actions {
            println!("  {}", action.description());
        }
        if let Some(estimate) = source_policy_replatform_note(&diff) {
            println!();
            println!("  {}", estimate);
        }
        println!();
        println!("Total: {} difference(s)", diff.actions.len());
    } else {
        println!("{}", model_check_drift_headline(&diff));
        println!("Run with --verbose for details, or 'model-diff' for full output");
    }

    // Exit with code 2 to distinguish drift (expected check failure) from
    // runtime errors (code 1). This avoids an anyhow error message on stderr
    // that duplicates the structured output already printed above.
    drop(_conn);
    process::exit(2)
}
