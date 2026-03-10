// src/commands/model.rs

//! System Model Commands
//!
//! Commands for declarative system state management using model files.

use std::path::Path;

use anyhow::{Result, anyhow};
use conary_core::db;
use conary_core::db::models::{CollectionMember, RemoteCollection, Repository, Trove, TroveType};
use conary_core::db::models::{
    DerivedOverride, DerivedPackage, DerivedPatch, DerivedStatus, VersionPolicy,
};
use conary_core::derived::build_from_definition;
use conary_core::filesystem::CasStore;
use conary_core::hash::sha256;
use conary_core::model::parser::SystemModel;
use conary_core::model::remote::fetch_remote_collection;
use conary_core::model::{
    DiffAction, ModelDerivedPackage, ModelDiff, SystemState, capture_current_state, compute_diff,
    compute_diff_with_includes_offline, parse_model_file, parse_trove_spec, snapshot_to_model,
};
use rusqlite::Connection;
use tracing::{debug, info};

fn load_model(model_path: &Path) -> Result<SystemModel> {
    if !model_path.exists() {
        return Err(anyhow!("Model file not found: {}", model_path.display()));
    }
    Ok(parse_model_file(model_path)?)
}

fn load_model_and_diff(
    model_path: &Path,
    db_path: &str,
    offline: bool,
    announce_includes: bool,
) -> Result<(SystemModel, Connection, ModelDiff)> {
    let model = load_model(model_path)?;
    let conn = db::open(db_path)?;
    let state = capture_current_state(&conn)?;
    let diff = compute_model_diff(&model, &state, &conn, offline, announce_includes)?;
    Ok((model, conn, diff))
}

fn compute_model_diff(
    model: &SystemModel,
    state: &SystemState,
    conn: &Connection,
    offline: bool,
    announce: bool,
) -> Result<ModelDiff> {
    if model.has_includes() {
        if announce {
            let mode = if offline { " (offline mode)" } else { "" };
            println!(
                "Resolving {} remote include(s){}...",
                model.include.models.len(),
                mode
            );
        }
        Ok(compute_diff_with_includes_offline(
            model, state, conn, offline,
        )?)
    } else {
        Ok(compute_diff(model, state))
    }
}

fn collect_lock_data(
    model: &SystemModel,
    conn: &Connection,
) -> Result<Vec<(String, String, conary_core::model::remote::CollectionData)>> {
    let mut lock_data = Vec::new();
    for spec in &model.include.models {
        let (name, label) = parse_trove_spec(spec)?;
        let label_str = label.as_deref().unwrap_or("");
        if let Some(cached) = RemoteCollection::find_cached(conn, &name, Some(label_str))
            .map_err(|e| anyhow!("Database error: {}", e))?
        {
            let data: conary_core::model::remote::CollectionData =
                serde_json::from_str(&cached.data_json)
                    .map_err(|e| anyhow!("Corrupt cache entry for '{}': {}", name, e))?;
            lock_data.push((name, label_str.to_string(), data));
        } else {
            return Err(anyhow!(
                "No cached data for '{}' after resolution -- this should not happen",
                spec
            ));
        }
    }
    Ok(lock_data)
}

fn build_lock_from_data(
    lock_data: &[(String, String, conary_core::model::remote::CollectionData)],
    model_path: &Path,
) -> Result<conary_core::model::lockfile::ModelLock> {
    let refs: Vec<(String, String, &conary_core::model::remote::CollectionData)> = lock_data
        .iter()
        .map(|(n, l, d)| (n.clone(), l.clone(), d))
        .collect();
    let mut lock = conary_core::model::lockfile::ModelLock::from_resolved(&refs);
    let model_bytes = std::fs::read(model_path)?;
    lock.metadata.model_hash = format!("sha256:{}", conary_core::hash::sha256(&model_bytes));
    Ok(lock)
}

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
        return existing.id.ok_or_else(|| {
            anyhow!(
                "Derived package '{}' exists but has no database id",
                model_derived.name
            )
        });
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
    let mut derived = DerivedPackage::new(model_derived.name.clone(), model_derived.from.clone());
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

        let mut patch = DerivedPatch::new(derived_id, (order + 1) as i32, patch_name, patch_hash);
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

            let mut ov = DerivedOverride::new_replace(derived_id, target_path.clone(), source_hash);
            ov.source_path = Some(source_path.clone());
            ov.insert(conn)?;

            // Store in CAS
            cas.store(&content)?;
        }
    }

    Ok(derived_id)
}

/// Build a derived package and return success/failure
fn build_derived_package(conn: &Connection, name: &str, cas: &CasStore) -> Result<()> {
    let mut derived = DerivedPackage::find_by_name(conn, name)?
        .ok_or_else(|| anyhow!("Derived package '{}' not found", name))?;

    // Build the derived package
    let result = build_from_definition(conn, &derived, cas);

    match result {
        Ok(build_result) => {
            println!(
                "  Built '{}': {} files, {} patches applied",
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
pub fn cmd_model_diff(model_path: &str, db_path: &str, offline: bool) -> Result<()> {
    let model_path = Path::new(model_path);
    let (_model, _conn, diff) = load_model_and_diff(model_path, db_path, offline, true)?;

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
#[allow(clippy::too_many_arguments)]
pub fn cmd_model_apply(
    model_path: &str,
    db_path: &str,
    _root: &str,
    dry_run: bool,
    skip_optional: bool,
    strict: bool,
    autoremove: bool,
    offline: bool,
) -> Result<()> {
    let model_path = Path::new(model_path);
    let (model, conn, diff) = load_model_and_diff(model_path, db_path, offline, true)?;

    if diff.is_empty() {
        println!("System is already in sync with model - no changes needed");
        return Ok(());
    }

    // Filter actions based on options
    let actions: Vec<_> = diff
        .actions
        .iter()
        .filter(|a| {
            if skip_optional && let DiffAction::Install { optional, .. } = a {
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
    let objects_dir = db_path_obj
        .parent()
        .unwrap_or(Path::new("."))
        .join("objects");
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
            DiffAction::BuildDerived {
                name,
                parent,
                needs_parent,
            } => {
                println!("Building derived package '{}'...", name);

                if *needs_parent {
                    println!(
                        "  [WARNING: Parent '{}' needs to be installed first]",
                        parent
                    );
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
    offline: bool,
) -> Result<()> {
    let model_path = Path::new(model_path);
    let (_model, _conn, diff) = load_model_and_diff(model_path, db_path, offline, false)?;

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
        println!("Total: {} difference(s)", diff.actions.len());
    } else {
        println!("DRIFT: {} difference(s) from model", diff.actions.len());
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

/// Compare local state against remote model collections
///
/// Fetches each remote include from the model (with optional forced refresh),
/// then compares remote collection members against installed packages to
/// detect drift.
pub fn cmd_model_remote_diff(model_path: &str, db_path: &str, refresh: bool) -> Result<()> {
    let model_path = Path::new(model_path);
    let model = load_model(model_path)?;
    let conn = db::open(db_path)?;
    let state = capture_current_state(&conn)?;

    if !model.has_includes() {
        println!("No remote includes in model");
        return Ok(());
    }

    let include_specs = &model.include.models;
    println!("Remote drift report:");
    println!();

    let mut total_drift = 0u32;
    let mut collections_checked = 0u32;

    for spec in include_specs {
        let (name, label) = parse_trove_spec(spec)?;

        let label_str = match &label {
            Some(l) => l.as_str(),
            None => {
                eprintln!("  Skipping '{}': no label for remote fetch", name);
                continue;
            }
        };

        // Purge cache if refresh requested
        if refresh {
            let purged =
                RemoteCollection::purge_by_name(&conn, &name, Some(label_str)).unwrap_or(0);
            if purged > 0 {
                debug!(name = %name, label = %label_str, "Purged {} cache entries", purged);
            }
        }

        // Fetch the remote collection
        let collection = match fetch_remote_collection(&conn, &name, label_str, false) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("  Failed to fetch '{}': {}", spec, e);
                continue;
            }
        };

        collections_checked += 1;

        // Compare remote members against local state
        let mut missing: Vec<String> = Vec::new();
        let mut version_drift: Vec<(String, String, String)> = Vec::new();

        for member in &collection.members {
            if let Some(installed) = state.installed.get(&member.name) {
                // Package is installed — check version constraint
                if let Some(constraint) = &member.version_constraint
                    && !version_matches_constraint(&installed.version, constraint)
                {
                    version_drift.push((
                        member.name.clone(),
                        constraint.clone(),
                        installed.version.clone(),
                    ));
                }
            } else {
                // Package not installed
                let suffix = if member.is_optional {
                    " (optional)"
                } else {
                    " (required)"
                };
                missing.push(format!("{}{}", member.name, suffix));
            }
        }

        let drift_count = missing.len() + version_drift.len();

        if drift_count > 0 {
            println!(
                "  {} ({}):",
                spec,
                format_version_info(&conn, &name, label_str)
            );

            if !missing.is_empty() {
                println!("    Missing locally:");
                for entry in &missing {
                    println!("      - {}", entry);
                }
            }

            if !version_drift.is_empty() {
                println!("    Version constraint drift:");
                for (pkg, constraint, installed) in &version_drift {
                    println!(
                        "      - {}: remote pins {}, installed {}",
                        pkg, constraint, installed
                    );
                }
            }

            println!();
        }

        total_drift += drift_count as u32;
    }

    println!(
        "Summary: {} collection(s) checked, {} drift(s) found",
        collections_checked, total_drift
    );

    if total_drift > 0 {
        std::process::exit(1);
    }

    Ok(())
}

/// Check if an installed version satisfies a version constraint pattern
///
/// Supports glob-style patterns (e.g. "1.24.*") and prefix comparisons.
fn version_matches_constraint(installed: &str, constraint: &str) -> bool {
    if constraint == installed {
        return true;
    }

    // Glob-style: "1.24.*" matches "1.24.0", "1.24.3", etc.
    if let Some(prefix) = constraint.strip_suffix(".*") {
        return installed == prefix || installed.starts_with(&format!("{}.", prefix));
    }

    // Prefix match: "1.24" matches "1.24.0"
    if installed.starts_with(constraint) && installed[constraint.len()..].starts_with('.') {
        return true;
    }

    false
}

/// Get version info string for display from cached collection data
fn format_version_info(conn: &Connection, name: &str, label: &str) -> String {
    if let Ok(Some(cached)) = RemoteCollection::find_cached(conn, name, Some(label))
        && let Some(version) = &cached.version
    {
        return format!("v{}", version);
    }
    "unknown version".to_string()
}

/// Lock remote include hashes for reproducibility
///
/// Resolves all remote includes and records their content hashes
/// in a model.lock file, preventing silent upstream changes.
pub fn cmd_model_lock(model_path: &str, output: Option<&str>, db_path: &str) -> Result<()> {
    let model_path = Path::new(model_path);
    let model = load_model(model_path)?;
    let conn = db::open(db_path)?;

    if !model.has_includes() {
        println!("No remote includes to lock");
        return Ok(());
    }

    let _resolved = conary_core::model::resolve_includes(&model, &conn)?;

    let lock_data = collect_lock_data(&model, &conn)?;
    let lock = build_lock_from_data(&lock_data, model_path)?;

    let lock_path = if let Some(out) = output {
        std::path::PathBuf::from(out)
    } else {
        let model_dir = model_path.parent().unwrap_or(Path::new("."));
        model_dir.join("model.lock")
    };

    lock.save(&lock_path)?;

    println!(
        "Locked {} collection(s) to {}",
        lock.collections.len(),
        lock_path.display()
    );
    for coll in &lock.collections {
        println!(
            "  {} ({}) - {} members, hash: {}",
            coll.name, coll.label, coll.member_count, coll.content_hash
        );
    }

    Ok(())
}

/// Update locked remote includes
///
/// Force-refreshes all remote includes, compares against the existing lock
/// file, and updates the lock with new hashes. Reports what changed.
pub fn cmd_model_update(model_path: &str, db_path: &str) -> Result<()> {
    let model_path = Path::new(model_path);
    let model = load_model(model_path)?;
    let conn = db::open(db_path)?;

    let model_dir = model_path.parent().unwrap_or(Path::new("."));
    let lock_path = model_dir.join("model.lock");

    if !lock_path.exists() {
        return Err(anyhow!(
            "No lock file found at {}. Run 'conary model lock' first.",
            lock_path.display()
        ));
    }

    let old_lock = conary_core::model::lockfile::ModelLock::load(&lock_path)?;

    if !model.has_includes() {
        println!("No remote includes to update");
        return Ok(());
    }

    // Force-refresh each include by purging cache first
    for spec in &model.include.models {
        let (name, label) = parse_trove_spec(spec)?;
        if let Some(label_str) = &label {
            let _ = RemoteCollection::purge_by_name(&conn, &name, Some(label_str));
        }
    }

    let _resolved = conary_core::model::resolve_includes(&model, &conn)?;

    let lock_data = collect_lock_data(&model, &conn)?;
    let current_hashes: Vec<(String, String, String)> = lock_data
        .iter()
        .map(|(n, l, d)| (n.clone(), l.clone(), d.content_hash.clone()))
        .collect();

    let drifts = old_lock.check_drift(&current_hashes);

    let new_lock = build_lock_from_data(&lock_data, model_path)?;
    new_lock.save(&lock_path)?;

    // Report results
    let changed = drifts.len();
    println!(
        "Updated {} collection(s), {} changed",
        new_lock.collections.len(),
        changed
    );

    if !drifts.is_empty() {
        println!();
        println!("Changes detected:");
        for drift in &drifts {
            println!(
                "  {} ({}): {} -> {}",
                drift.name, drift.label, drift.locked_hash, drift.current_hash
            );
        }
    }

    Ok(())
}

/// Publish a system model as a versioned collection to a repository
///
/// Supports both local (file://) and remote (http/https) repositories.
/// For remote repos, the collection is sent via HTTP PUT to the Remi
/// server's admin API.
#[allow(clippy::too_many_arguments)]
pub fn cmd_model_publish(
    model_path: &str,
    name: &str,
    version: &str,
    repo_name: &str,
    description: Option<&str>,
    db_path: &str,
    force: bool,
    sign_key_path: Option<&str>,
) -> Result<()> {
    let model_path = Path::new(model_path);
    let model = load_model(model_path)?;

    // Ensure group- prefix
    let group_name = if name.starts_with("group-") {
        name.to_string()
    } else {
        format!("group-{}", name)
    };

    println!("Publishing model as collection '{}'...", group_name);

    // Open database
    let mut conn = db::open(db_path)?;

    // Get repository
    let repo = Repository::find_by_name(&conn, repo_name)?
        .ok_or_else(|| anyhow!("Repository '{}' not found", repo_name))?;

    let repo_url = &repo.url;
    let is_remote = repo_url.starts_with("http://") || repo_url.starts_with("https://");

    // Load signing key if provided
    let signing_key = if let Some(key_path) = sign_key_path {
        let key = conary_core::model::signing::load_signing_key(Path::new(key_path))
            .map_err(|e| anyhow!("Failed to load signing key: {e}"))?;
        let key_id = conary_core::model::signing::key_id(&key.verifying_key());
        println!("  Signing with key: {}", key_id);
        Some(key)
    } else {
        None
    };

    if is_remote {
        // Remote publish via HTTP PUT
        let data = conary_core::model::remote::build_collection_data_from_model(
            &model,
            &group_name,
            version,
        );

        // Sign if key provided
        if let Some(ref key) = signing_key {
            let signature = conary_core::model::signing::sign_collection(&data, key)
                .map_err(|e| anyhow!("{}", e))?;
            let key_id = conary_core::model::signing::key_id(&key.verifying_key());
            println!(
                "  Signed collection ({} bytes, key {})",
                signature.len(),
                key_id
            );

            // Store signature in cache so the server endpoint can serve it
            let mut sig_cache = conary_core::db::models::RemoteCollection::new(
                group_name.clone(),
                Some(repo_name.to_string()),
                String::new(),
                serde_json::to_string(&data).unwrap_or_default(),
                "2099-12-31T23:59:59".to_string(),
            );
            sig_cache.version = Some(version.to_string());
            sig_cache.signature = Some(signature);
            sig_cache.signer_key_id = Some(key_id);
            let _ = sig_cache.upsert(&conn);
        }

        conary_core::model::remote::publish_remote_collection(repo_url, &data, force)
            .map_err(|e| anyhow!("{}", e))?;

        let member_count = data.members.len();
        println!();
        println!(
            "Published {} v{} to remote repository '{}'",
            group_name, version, repo_name
        );
        println!("  Members: {} package(s)", member_count);
    } else {
        // Local publish (existing logic)
        if !repo_url.starts_with("file://") && !repo_url.starts_with('/') {
            return Err(anyhow!(
                "Repository URL scheme not supported: '{}'. Use file://, http://, or https://",
                repo_url
            ));
        }

        let repo_path = repo_url.strip_prefix("file://").unwrap_or(repo_url);
        let repo_dir = Path::new(repo_path);

        if !repo_dir.exists() {
            return Err(anyhow!("Repository path does not exist: {}", repo_path));
        }
        if !repo_dir.is_dir() {
            return Err(anyhow!("Repository path is not a directory: {}", repo_path));
        }

        // Check write permission
        let test_path = repo_dir.join(".conary_write_test");
        std::fs::write(&test_path, b"test")
            .map_err(|e| anyhow!("No write permission to repository {}: {}", repo_path, e))?;
        std::fs::remove_file(&test_path)?;

        // Check if collection already exists
        let existing = Trove::find_by_name(&conn, &group_name)?;
        if !existing.is_empty()
            && existing
                .iter()
                .any(|t| t.trove_type == TroveType::Collection)
        {
            if force {
                for t in &existing {
                    if t.trove_type == TroveType::Collection
                        && let Some(id) = t.id
                    {
                        CollectionMember::delete_all_for_collection(&conn, id)?;
                        Trove::delete(&conn, id)?;
                    }
                }
            } else {
                return Err(anyhow!(
                    "Collection '{}' already exists. Use --force to overwrite.",
                    group_name
                ));
            }
        }

        // Create the collection in the database
        db::transaction(&mut conn, |tx| {
            let mut trove = Trove::new(
                group_name.clone(),
                version.to_string(),
                TroveType::Collection,
            );
            trove.description = description.map(|s| s.to_string());
            trove.selection_reason = Some(format!("Published from {}", model_path.display()));
            let collection_id = trove.insert(tx)?;

            info!(
                "Created collection '{}' with id={}",
                group_name, collection_id
            );

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

            for pkg_name in &model.optional.packages {
                if !model.config.install.contains(pkg_name) {
                    let mut member =
                        CollectionMember::new(collection_id, pkg_name.clone()).optional();
                    if let Some(v) = model.pin.get(pkg_name) {
                        member = member.with_version(v.clone());
                    }
                    member.insert(tx)?;
                }
            }

            Ok(collection_id)
        })?;

        let member_count = model.config.install.len()
            + model
                .optional
                .packages
                .iter()
                .filter(|p| !model.config.install.contains(*p))
                .count();
        let optional_count = model.optional.packages.len();
        let pinned_count = model.pin.len();

        println!();
        println!(
            "Published {} v{} to repository '{}'",
            group_name, version, repo_name
        );
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
    }

    println!();
    println!("Other systems can now include this collection:");
    println!("  [include]");
    println!("  models = [\"{}@{}:stable\"]", group_name, repo_name);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_matches_constraint_exact() {
        assert!(version_matches_constraint("1.24.0", "1.24.0"));
        assert!(!version_matches_constraint("1.24.1", "1.24.0"));
    }

    #[test]
    fn test_version_matches_constraint_glob() {
        assert!(version_matches_constraint("1.24.0", "1.24.*"));
        assert!(version_matches_constraint("1.24.3", "1.24.*"));
        assert!(version_matches_constraint("1.24", "1.24.*"));
        assert!(!version_matches_constraint("1.25.0", "1.24.*"));
        assert!(!version_matches_constraint("2.24.0", "1.24.*"));
    }

    #[test]
    fn test_version_matches_constraint_prefix() {
        assert!(version_matches_constraint("1.24.0", "1.24"));
        assert!(!version_matches_constraint("1.25.0", "1.24"));
    }

    #[test]
    fn test_remote_diff_detects_missing() {
        use conary_core::db::models::RemoteCollection;
        use conary_core::db::schema;
        use conary_core::model::SystemState;
        use std::collections::{HashMap, HashSet};
        use tempfile::NamedTempFile;

        // Create test DB and populate cache
        let temp_file = NamedTempFile::new().unwrap();
        let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();

        // Create a cached remote collection with members
        let collection_data = serde_json::json!({
            "name": "group-test",
            "version": "1.0",
            "members": [
                {"name": "nginx", "version_constraint": "1.24.*", "is_optional": false},
                {"name": "redis", "version_constraint": null, "is_optional": false},
                {"name": "memcached", "version_constraint": null, "is_optional": true}
            ],
            "includes": [],
            "pins": {},
            "exclude": [],
            "content_hash": "sha256:test123",
            "published_at": "2026-01-01T00:00:00Z"
        });

        let mut cache_entry = RemoteCollection::new(
            "group-test".to_string(),
            Some("myrepo:stable".to_string()),
            "sha256:test123".to_string(),
            serde_json::to_string(&collection_data).unwrap(),
            "2099-12-31T23:59:59".to_string(),
        );
        cache_entry.version = Some("1.0".to_string());
        cache_entry.upsert(&conn).unwrap();

        // Create a system state with only nginx installed
        let state = SystemState {
            installed: HashMap::from([(
                "nginx".to_string(),
                conary_core::model::InstalledPackage {
                    name: "nginx".to_string(),
                    version: "1.24.2".to_string(),
                    architecture: None,
                    explicit: true,
                    label: None,
                },
            )]),
            explicit: HashSet::from(["nginx".to_string()]),
            pinned: HashSet::new(),
        };

        // Fetch the collection from cache
        let fetched = conary_core::model::remote::fetch_remote_collection(
            &conn,
            "group-test",
            "myrepo:stable",
            false,
        )
        .unwrap();

        // Simulate the drift detection logic from cmd_model_remote_diff
        let mut missing = Vec::new();
        let mut version_drift = Vec::new();

        for member in &fetched.members {
            if let Some(installed) = state.installed.get(&member.name) {
                if let Some(constraint) = &member.version_constraint
                    && !version_matches_constraint(&installed.version, constraint)
                {
                    version_drift.push(member.name.clone());
                }
            } else {
                missing.push(member.name.clone());
            }
        }

        // redis and memcached should be missing
        assert_eq!(missing.len(), 2);
        assert!(missing.contains(&"redis".to_string()));
        assert!(missing.contains(&"memcached".to_string()));

        // nginx 1.24.2 matches constraint 1.24.* so no version drift
        assert!(version_drift.is_empty());
    }
}
