// src/commands/model.rs

//! System Model Commands
//!
//! Commands for declarative system state management using model files.

use std::path::Path;

use anyhow::{anyhow, Result};
use conary::db;
use conary::model::{
    capture_current_state, compute_diff, parse_model_file, snapshot_to_model,
    DiffAction,
};

/// Show what changes are needed to reach the model state
pub fn cmd_model_diff(model_path: &str, db_path: &str) -> Result<()> {
    // Check if model file exists
    let model_path = Path::new(model_path);
    if !model_path.exists() {
        eprintln!("Error: Model file not found: {}", model_path.display());
        eprintln!("Create a model file or use 'conary model-snapshot' to capture current state");
        return Err(anyhow!("Model file not found"));
    }

    // Load the model
    let model = match parse_model_file(model_path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Error parsing model file: {}", e);
            return Err(e.into());
        }
    };

    // Open database and capture current state
    let conn = db::open(db_path)?;
    let state = capture_current_state(&conn)?;

    // Compute diff
    let diff = compute_diff(&model, &state);

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
    let others: Vec<_> = diff
        .actions
        .iter()
        .filter(|a| !matches!(a, DiffAction::Install { .. } | DiffAction::Remove { .. }))
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

    println!(
        "Summary: {} install(s), {} remove(s), {} other change(s)",
        installs.len(),
        removes.len(),
        others.len()
    );

    Ok(())
}

/// Apply the system model to reach the desired state
pub fn cmd_model_apply(
    model_path: &str,
    db_path: &str,
    _root: &str,
    dry_run: bool,
    skip_optional: bool,
    strict: bool,
    autoremove: bool,
) -> Result<()> {
    // Check if model file exists
    let model_path = Path::new(model_path);
    if !model_path.exists() {
        eprintln!("Error: Model file not found: {}", model_path.display());
        return Err(anyhow!("Model file not found"));
    }

    // Load the model
    let model = parse_model_file(model_path)?;

    // Open database and capture current state
    let conn = db::open(db_path)?;
    let state = capture_current_state(&conn)?;

    // Compute diff
    let diff = compute_diff(&model, &state);

    if diff.is_empty() {
        println!("System is already in sync with model - no changes needed");
        return Ok(());
    }

    // Filter actions based on options
    let actions: Vec<_> = diff
        .actions
        .iter()
        .filter(|a| {
            if skip_optional {
                if let DiffAction::Install { optional, .. } = a {
                    return !optional;
                }
            }
            if !strict {
                // In non-strict mode, skip MarkDependency actions
                if matches!(a, DiffAction::MarkDependency { .. }) {
                    return false;
                }
            }
            true
        })
        .collect();

    if actions.is_empty() {
        println!("No applicable changes after filtering");
        return Ok(());
    }

    println!("Model apply plan:");
    println!();

    for action in &actions {
        let prefix = match action {
            DiffAction::Install { .. } => "+",
            DiffAction::Remove { .. } => "-",
            _ => "*",
        };
        println!("  {} {}", prefix, action.description());
    }
    println!();

    if dry_run {
        println!("[Dry run - no changes made]");
        return Ok(());
    }

    // TODO: Actually apply the changes using install/remove commands
    // For now, we just show what would be done
    println!("Applying changes...");

    let installs: Vec<_> = actions
        .iter()
        .filter_map(|a| match a {
            DiffAction::Install { package, .. } => Some(package.as_str()),
            _ => None,
        })
        .collect();

    let removes: Vec<_> = actions
        .iter()
        .filter_map(|a| match a {
            DiffAction::Remove { package, .. } => Some(package.as_str()),
            _ => None,
        })
        .collect();

    if !removes.is_empty() {
        println!("Would remove: {}", removes.join(", "));
        // TODO: Call remove command for each package
    }

    if !installs.is_empty() {
        println!("Would install: {}", installs.join(", "));
        // TODO: Call install command for each package
    }

    if autoremove {
        println!("Would run autoremove to clean up orphaned dependencies");
        // TODO: Call autoremove command
    }

    println!();
    println!("[NOTE: Full apply implementation pending - showing plan only]");
    println!("To apply manually, run the install/remove commands shown above.");

    Ok(())
}

/// Check if system state matches the model
pub fn cmd_model_check(
    model_path: &str,
    db_path: &str,
    verbose: bool,
) -> Result<()> {
    // Check if model file exists
    let model_path = Path::new(model_path);
    if !model_path.exists() {
        eprintln!("Error: Model file not found: {}", model_path.display());
        return Err(anyhow!("Model file not found"));
    }

    // Load the model
    let model = parse_model_file(model_path)?;

    // Open database and capture current state
    let conn = db::open(db_path)?;
    let state = capture_current_state(&conn)?;

    // Compute diff
    let diff = compute_diff(&model, &state);

    if diff.is_empty() {
        println!("OK: System matches model");
        return Ok(());
    }

    // System doesn't match model
    if verbose {
        println!("DRIFT: System does not match model");
        println!();
        for action in &diff.actions {
            println!("  {}", action.description());
        }
        println!();
        println!(
            "Total: {} difference(s)",
            diff.actions.len()
        );
    } else {
        println!(
            "DRIFT: {} difference(s) from model",
            diff.actions.len()
        );
        println!("Run with --verbose for details, or 'model-diff' for full output");
    }

    // Return error to indicate drift
    std::process::exit(1);
}

/// Create a model file from current system state
pub fn cmd_model_snapshot(
    output_path: &str,
    db_path: &str,
    description: Option<&str>,
) -> Result<()> {
    // Open database and capture current state
    let conn = db::open(db_path)?;
    let state = capture_current_state(&conn)?;

    // Create model from state
    let model = snapshot_to_model(&state);

    // Generate TOML
    let mut toml_content = String::new();

    // Add header comment
    toml_content.push_str("# Conary System Model\n");
    toml_content.push_str("# Generated from current system state\n");
    if let Some(desc) = description {
        toml_content.push_str(&format!("# Description: {}\n", desc));
    }
    toml_content.push_str(&format!(
        "# Generated at: {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    ));
    toml_content.push_str("#\n");
    toml_content.push_str("# Edit this file to define your desired system state.\n");
    toml_content.push_str("# Then run 'conary model-apply' to sync the system.\n");
    toml_content.push_str("\n");

    // Add model content
    toml_content.push_str(&model.to_toml()?);

    // Write to file
    std::fs::write(output_path, &toml_content)?;

    println!("Model snapshot written to: {}", output_path);
    println!();
    println!("Captured:");
    println!("  - {} explicit package(s)", model.config.install.len());
    println!("  - {} pinned package(s)", model.pin.len());
    println!();
    println!("Edit the file to customize, then run:");
    println!("  conary model-diff -m {}   # Preview changes", output_path);
    println!("  conary model-apply -m {}  # Apply changes", output_path);

    Ok(())
}
