// src/commands/model.rs

//! System Model Commands
//!
//! Commands for declarative system state management using model files.

use std::path::Path;

use anyhow::{anyhow, Result};
use conary::db;
use conary::db::models::{DerivedOverride, DerivedPackage, DerivedPatch, DerivedStatus, VersionPolicy};
use conary::derived::build_from_definition;
use conary::filesystem::CasStore;
use conary::hash::sha256;
use conary::db::models::{CollectionMember, Repository, Trove, TroveType};
use conary::model::{
    capture_current_state, compute_diff, compute_diff_with_includes, parse_model_file,
    snapshot_to_model, DiffAction, ModelDerivedPackage,
};
use rusqlite::Connection;
use tracing::info;

/// Create a derived package definition from a model specification
fn create_derived_from_model(
    conn: &Connection,
    model_derived: &ModelDerivedPackage,
    model_dir: &Path,
    cas: &CasStore,
) -> Result<i64> {
    // Check if already exists
    if let Some(existing) = DerivedPackage::find_by_name(conn, &model_derived.name)? {
        info!(
            "Derived package '{}' already exists, updating",
            model_derived.name
        );
        // Return existing ID, patches/overrides will be checked separately
        return Ok(existing.id.unwrap());
    }

    // Parse version policy
    let version_policy = if model_derived.version == "inherit" {
        VersionPolicy::Inherit
    } else if model_derived.version.starts_with('+') {
        VersionPolicy::Suffix(model_derived.version.clone())
    } else {
        VersionPolicy::Specific(model_derived.version.clone())
    };

    // Create the derived package
    let mut derived = DerivedPackage::new(
        model_derived.name.clone(),
        model_derived.from.clone(),
    );
    derived.version_policy = version_policy;
    derived.model_source = Some(model_dir.display().to_string());

    let derived_id = derived.insert(conn)?;
    info!(
        "Created derived package '{}' with id={}",
        model_derived.name, derived_id
    );

    // Add patches
    for (order, patch_path) in model_derived.patches.iter().enumerate() {
        let full_path = model_dir.join(patch_path);
        if !full_path.exists() {
            return Err(anyhow!(
                "Patch file not found: {} (for derived package '{}')",
                full_path.display(),
                model_derived.name
            ));
        }

        let patch_content = std::fs::read(&full_path)?;
        let patch_hash = sha256(&patch_content);
        let patch_name = Path::new(patch_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("patch")
            .to_string();

        let mut patch = DerivedPatch::new(
            derived_id,
            (order + 1) as i32,
            patch_name,
            patch_hash,
        );
        patch.insert(conn)?;

        // Store in CAS
        cas.store(&patch_content)?;
    }

    // Add file overrides
    for (target_path, source_path) in &model_derived.override_files {
        if source_path.is_empty() || source_path == "REMOVE" {
            // File removal
            let mut ov = DerivedOverride::new_remove(derived_id, target_path.clone());
            ov.insert(conn)?;
        } else {
            // File replacement
            let full_source = model_dir.join(source_path);
            if !full_source.exists() {
                return Err(anyhow!(
                    "Override source file not found: {} (for derived package '{}')",
                    full_source.display(),
                    model_derived.name
                ));
            }

            let content = std::fs::read(&full_source)?;
            let source_hash = sha256(&content);

            let mut ov = DerivedOverride::new_replace(
                derived_id,
                target_path.clone(),
                source_hash,
            );
            ov.source_path = Some(source_path.clone());
            ov.insert(conn)?;

            // Store in CAS
            cas.store(&content)?;
        }
    }

    Ok(derived_id)
}

/// Build a derived package and return success/failure
fn build_derived_package(
    conn: &Connection,
    name: &str,
    cas: &CasStore,
) -> Result<()> {
    let mut derived = DerivedPackage::find_by_name(conn, name)?
        .ok_or_else(|| anyhow!("Derived package '{}' not found", name))?;

    // Build the derived package
    let result = build_from_definition(conn, &derived, cas);

    match result {
        Ok(build_result) => {
            println!("  Built '{}': {} files, {} patches applied",
                name,
                build_result.files.len(),
                build_result.patches_applied.len()
            );
            derived.set_status(conn, DerivedStatus::Built)?;
            Ok(())
        }
        Err(e) => {
            let error_msg = e.to_string();
            derived.mark_error(conn, &error_msg)?;
            Err(anyhow!("Build failed for '{}': {}", name, error_msg))
        }
    }
}

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

    // Compute diff, resolving includes if present
    let diff = if model.has_includes() {
        println!("Resolving {} remote include(s)...", model.include.models.len());
        compute_diff_with_includes(&model, &state, &conn)?
    } else {
        compute_diff(&model, &state)
    };

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

    // Compute diff, resolving includes if present
    let diff = if model.has_includes() {
        println!("Resolving {} remote include(s)...", model.include.models.len());
        compute_diff_with_includes(&model, &state, &conn)?
    } else {
        compute_diff(&model, &state)
    };

    if diff.is_empty() {
        println!("System is already in sync with model - no changes needed");
        return Ok(());
    }

    // Filter actions based on options
    let actions: Vec<_> = diff
        .actions
        .iter()
        .filter(|a| {
            if skip_optional
                && let DiffAction::Install { optional, .. } = a
            {
                return !optional;
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

    println!("Applying changes...");
    println!();

    // Set up CAS for derived package operations
    let db_path_obj = Path::new(db_path);
    let objects_dir = db_path_obj.parent().unwrap_or(Path::new(".")).join("objects");
    let cas = CasStore::new(&objects_dir)?;

    // Get model directory for resolving relative paths
    let model_dir = model_path.parent().unwrap_or(Path::new("."));

    // Track results
    let mut errors: Vec<String> = Vec::new();
    let mut derived_built = 0;
    let mut derived_rebuilt = 0;

    // Collect different action types
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

    // Process removes first (before derived packages that might depend on them)
    if !removes.is_empty() {
        println!("Packages to remove: {}", removes.join(", "));
        println!("  [NOTE: Package removal not yet implemented - run manually]");
        println!();
    }

    // Process regular installs (needed before derived packages)
    if !installs.is_empty() {
        println!("Packages to install: {}", installs.join(", "));
        println!("  [NOTE: Package installation not yet implemented - run manually]");
        println!();
    }

    // Process derived package actions
    for action in &actions {
        match action {
            DiffAction::BuildDerived { name, parent, needs_parent } => {
                println!("Building derived package '{}'...", name);

                if *needs_parent {
                    println!("  [WARNING: Parent '{}' needs to be installed first]", parent);
                    errors.push(format!(
                        "Cannot build '{}': parent '{}' not installed",
                        name, parent
                    ));
                    continue;
                }

                // Find the derived package definition in the model
                let model_def = model.derive.iter().find(|d| d.name == *name);

                if let Some(def) = model_def {
                    // Create the derived package definition in DB
                    match create_derived_from_model(&conn, def, model_dir, &cas) {
                        Ok(_id) => {
                            // Now build it
                            match build_derived_package(&conn, name, &cas) {
                                Ok(()) => {
                                    derived_built += 1;
                                }
                                Err(e) => {
                                    errors.push(format!("Build '{}': {}", name, e));
                                }
                            }
                        }
                        Err(e) => {
                            errors.push(format!("Create definition '{}': {}", name, e));
                        }
                    }
                } else {
                    errors.push(format!(
                        "Derived package '{}' not found in model file",
                        name
                    ));
                }
            }

            DiffAction::RebuildDerived { name, parent: _ } => {
                println!("Rebuilding derived package '{}'...", name);

                match build_derived_package(&conn, name, &cas) {
                    Ok(()) => {
                        derived_rebuilt += 1;
                    }
                    Err(e) => {
                        errors.push(format!("Rebuild '{}': {}", name, e));
                    }
                }
            }

            _ => {
                // Other actions (Install, Remove, Pin, etc.) handled above or not yet implemented
            }
        }
    }

    if autoremove {
        println!();
        println!("Autoremove: [NOTE: Not yet implemented - run 'conary autoremove' manually]");
    }

    // Summary
    println!();
    println!("Summary:");

    if derived_built > 0 {
        println!("  Derived packages built: {}", derived_built);
    }
    if derived_rebuilt > 0 {
        println!("  Derived packages rebuilt: {}", derived_rebuilt);
    }
    if !installs.is_empty() {
        println!("  Packages to install (manual): {}", installs.len());
    }
    if !removes.is_empty() {
        println!("  Packages to remove (manual): {}", removes.len());
    }

    if !errors.is_empty() {
        println!();
        println!("Errors ({}):", errors.len());
        for err in &errors {
            println!("  - {}", err);
        }
        return Err(anyhow!("{} error(s) during apply", errors.len()));
    }

    if derived_built > 0 || derived_rebuilt > 0 {
        println!();
        println!("Derived packages processed successfully.");
    }

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

    // Compute diff, resolving includes if present
    let diff = if model.has_includes() {
        compute_diff_with_includes(&model, &state, &conn)?
    } else {
        compute_diff(&model, &state)
    };

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
    toml_content.push('\n');

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

/// Publish a system model as a versioned collection to a local repository
///
/// This creates a collection (group trove) from the model file and stores it
/// in a local repository. Other systems can then include this collection
/// using the `[include]` directive in their model files.
pub fn cmd_model_publish(
    model_path: &str,
    name: &str,
    version: &str,
    repo_name: &str,
    description: Option<&str>,
    db_path: &str,
) -> Result<()> {
    // Check if model file exists
    let model_path = Path::new(model_path);
    if !model_path.exists() {
        return Err(anyhow!("Model file not found: {}", model_path.display()));
    }

    // Load the model
    let model = parse_model_file(model_path)?;

    // Ensure group- prefix
    let group_name = if name.starts_with("group-") {
        name.to_string()
    } else {
        format!("group-{}", name)
    };

    println!("Publishing model as collection '{}'...", group_name);

    // Open database
    let mut conn = db::open(db_path)?;

    // Get repository and verify it's local
    let repo = Repository::find_by_name(&conn, repo_name)?
        .ok_or_else(|| anyhow!("Repository '{}' not found", repo_name))?;

    let repo_url = &repo.url;
    if !repo_url.starts_with("file://") && !repo_url.starts_with('/') {
        return Err(anyhow!(
            "Publishing only supported to local repositories. '{}' is remote (URL: {})",
            repo_name,
            repo_url
        ));
    }

    // Get the repository path
    let repo_path = repo_url.strip_prefix("file://").unwrap_or(repo_url);
    let repo_dir = Path::new(repo_path);

    // Verify repository path exists and is writable
    if !repo_dir.exists() {
        return Err(anyhow!("Repository path does not exist: {}", repo_path));
    }
    if !repo_dir.is_dir() {
        return Err(anyhow!("Repository path is not a directory: {}", repo_path));
    }

    // Check write permission by attempting to create a temp file
    let test_path = repo_dir.join(".conary_write_test");
    std::fs::write(&test_path, b"test")
        .map_err(|e| anyhow!("No write permission to repository {}: {}", repo_path, e))?;
    std::fs::remove_file(&test_path)?;

    // Check if collection already exists
    let existing = Trove::find_by_name(&conn, &group_name)?;
    if !existing.is_empty() {
        // Check if it's a collection
        if existing.iter().any(|t| t.trove_type == TroveType::Collection) {
            return Err(anyhow!(
                "Collection '{}' already exists. Use a different name or remove the existing one.",
                group_name
            ));
        }
    }

    // Create the collection in the database
    db::transaction(&mut conn, |tx| {
        // Create the collection trove
        let mut trove = Trove::new(
            group_name.clone(),
            version.to_string(),
            TroveType::Collection,
        );
        trove.description = description.map(|s| s.to_string());
        trove.selection_reason = Some(format!("Published from {}", model_path.display()));
        let collection_id = trove.insert(tx)?;

        info!("Created collection '{}' with id={}", group_name, collection_id);

        // Add members from the model's install list
        for pkg_name in &model.config.install {
            let version_constraint = model.pin.get(pkg_name).cloned();
            let is_optional = model.optional.packages.contains(pkg_name);

            let mut member = CollectionMember::new(collection_id, pkg_name.clone());
            if let Some(v) = version_constraint {
                member = member.with_version(v);
            }
            if is_optional {
                member = member.optional();
            }
            member.insert(tx)?;
        }

        // Also add optional packages that aren't in the install list
        for pkg_name in &model.optional.packages {
            if !model.config.install.contains(pkg_name) {
                let mut member = CollectionMember::new(collection_id, pkg_name.clone())
                    .optional();
                if let Some(v) = model.pin.get(pkg_name) {
                    member = member.with_version(v.clone());
                }
                member.insert(tx)?;
            }
        }

        Ok(collection_id)
    })?;

    // Count members for summary
    let member_count = model.config.install.len() + model.optional.packages.iter()
        .filter(|p| !model.config.install.contains(*p))
        .count();
    let optional_count = model.optional.packages.len();
    let pinned_count = model.pin.len();

    println!();
    println!("Published {} v{} to repository '{}'", group_name, version, repo_name);
    println!("  Members: {} package(s)", member_count);
    if optional_count > 0 {
        println!("  Optional: {} package(s)", optional_count);
    }
    if pinned_count > 0 {
        println!("  Pinned: {} package(s)", pinned_count);
    }
    if !model.config.exclude.is_empty() {
        println!("  Exclude: {} package(s)", model.config.exclude.len());
    }
    println!();
    println!("Other systems can now include this collection:");
    println!("  [include]");
    println!("  models = [\"{}@{}:stable\"]", group_name, repo_name);

    Ok(())
}
