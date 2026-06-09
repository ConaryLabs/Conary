// src/commands/model.rs

//! System Model Commands
//!
//! Commands for declarative system state management using model files.

mod apply;
mod context;
mod presentation;
mod publish;

use std::path::Path;
use std::process;

use super::open_db;
use super::replatform_rendering::render_replatform_execution_plan;
use anyhow::{Context, Result, anyhow};
use conary_core::db::models::RemoteCollection;
use conary_core::filesystem::CasStore;
use conary_core::model::parser::SystemModel;
use conary_core::model::remote::fetch_remote_collection;
use conary_core::model::{
    DiffAction, capture_current_state, parse_trove_spec, replatform_execution_plan,
    snapshot_to_model,
};
use rusqlite::Connection;
use tracing::debug;

pub use apply::ApplyOptions;
use apply::{
    apply_derived_packages, apply_metadata_changes, apply_package_changes,
    apply_replatform_changes, apply_source_policy_changes,
};
#[cfg(test)]
use context::compute_model_diff;
use context::{load_model, load_model_and_diff};
use presentation::{
    is_replatform_action, is_source_policy_action, model_check_drift_headline,
    print_source_policy_and_replatform, render_replatform_summary, source_policy_replatform_note,
    source_policy_summary,
};
pub use publish::cmd_model_publish;

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
    let model_bytes = std::fs::read(model_path)
        .with_context(|| format!("Failed to read model file '{}'", model_path.display()))?;
    lock.metadata.model_hash = format!("sha256:{}", conary_core::hash::sha256(&model_bytes));
    Ok(lock)
}

/// Create a derived package definition from a model specification
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

/// Apply the system model to reach the desired state.
pub async fn cmd_model_apply(opts: ApplyOptions<'_>) -> Result<()> {
    let ApplyOptions {
        model_path,
        db_path,
        root,
        dry_run,
        skip_optional,
        strict,
        autoremove,
        offline,
    } = opts;

    let model_path = Path::new(model_path);
    let (model, conn, diff) = load_model_and_diff(model_path, db_path, offline, true).await?;
    let diff_summary = diff.summary();

    if diff.is_empty() {
        println!("System is already in sync with model - no changes needed");
        return Ok(());
    }

    // Filter actions based on options
    let actions: Vec<&DiffAction> = diff
        .actions
        .iter()
        .filter(|a| {
            if skip_optional && let DiffAction::Install { optional, .. } = a {
                return !optional;
            }
            if !strict && matches!(a, DiffAction::MarkDependency { .. }) {
                return false;
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
            a if is_replatform_action(a) => ">",
            a if is_source_policy_action(a) => "~",
            _ => "*",
        };
        println!("  {} {}", prefix, action.description());
    }
    println!();

    if let Some(summary) = source_policy_summary(&diff) {
        println!("{}", summary);
        println!();
    }

    if let Some(estimate) = source_policy_replatform_note(&diff) {
        println!("{}", estimate);
        println!();
    }

    if let Some(plan) = replatform_execution_plan(
        &conn,
        &actions.iter().map(|a| (*a).clone()).collect::<Vec<_>>(),
    )? {
        println!("{}", render_replatform_execution_plan(&plan));
        println!();
        let executable = plan.transactions.iter().filter(|tx| tx.executable).count();
        let blocked = plan.transactions.len().saturating_sub(executable);
        if executable == 0 {
            println!(
                "No executable replatform transactions are available in this plan yet. Review the blocked reasons above; those package replacements remain pending."
            );
            println!();
        } else if blocked == 0 {
            println!(
                "Executable replatform transactions will be applied through the shared install path."
            );
            println!();
        } else {
            println!(
                "Executable replatform transactions will be applied through the shared install path; blocked ones will remain pending and be reported as errors."
            );
            println!();
        }
    }

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

    // Phase 1: source policy changes
    apply_source_policy_changes(&conn, &actions)?;

    // Phase 2: executable replatform replacements
    let (replatform_executed, replatform_errors) =
        apply_replatform_changes(db_path, root, &actions).await?;

    // Phase 3: package changes (install/remove/update execution)
    let (package_applied, package_errors) =
        apply_package_changes(db_path, root, &actions, strict).await?;

    // Phase 4: derived packages
    let (derived_built, derived_rebuilt, mut errors) =
        apply_derived_packages(&conn, &actions, &model, model_dir, &cas);
    errors.extend(replatform_errors);
    errors.extend(package_errors);

    // Phase 5: metadata changes (pin/unpin, mark explicit/dependency, update)
    let (metadata_applied, metadata_errors) = apply_metadata_changes(&conn, &actions);
    errors.extend(metadata_errors);

    if autoremove {
        println!();
        if let Err(e) = crate::commands::cmd_autoremove(
            db_path,
            root,
            false,
            false,
            crate::commands::SandboxMode::Always,
            crate::commands::LegacyReplayOptions::default(),
        )
        .await
        {
            errors.push(format!("Autoremove: {}", e));
        }
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
    if package_applied > 0 {
        println!("  Package changes applied: {}", package_applied);
    }
    if replatform_executed > 0 {
        println!(
            "  Replatform replacements executed: {}",
            replatform_executed
        );
    }
    if metadata_applied > 0 {
        println!("  Metadata changes applied: {}", metadata_applied);
    }
    if diff_summary.source_policy_changes > 0 {
        println!(
            "  Source policy changes applied: {}",
            diff_summary.source_policy_changes
        );
    }
    if let Some(replatform) = render_replatform_summary(&diff_summary) {
        println!("{}", replatform);
    }
    print_source_policy_and_replatform(&conn, &diff)?;

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

/// Create a model file from current system state
pub async fn cmd_model_snapshot(
    output_path: &str,
    db_path: &str,
    description: Option<&str>,
) -> Result<()> {
    // Open database and capture current state
    let conn = open_db(db_path)?;
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
pub async fn cmd_model_remote_diff(model_path: &str, db_path: &str, refresh: bool) -> Result<()> {
    let model_path = Path::new(model_path);
    let model = load_model(model_path)?;
    let conn = open_db(db_path)?;
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
        let collection = match fetch_remote_collection(&conn, &name, label_str, false).await {
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
            if let Some(installed) = state.get_package(&member.name) {
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
        return Err(anyhow!(
            "remote drift detected: {} drift(s) found",
            total_drift
        ));
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
pub async fn cmd_model_lock(model_path: &str, output: Option<&str>, db_path: &str) -> Result<()> {
    let model_path = Path::new(model_path);
    let model = load_model(model_path)?;
    let conn = open_db(db_path)?;

    if !model.has_includes() {
        println!("No remote includes to lock");
        return Ok(());
    }

    let _resolved = conary_core::model::resolve_includes(&model, &conn).await?;

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
pub async fn cmd_model_update(model_path: &str, db_path: &str) -> Result<()> {
    let model_path = Path::new(model_path);
    let model = load_model(model_path)?;
    let conn = open_db(db_path)?;

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

    let _resolved = conary_core::model::resolve_includes(&model, &conn).await?;

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

#[cfg(test)]
mod tests {
    use super::super::test_helpers::create_test_db;
    use super::*;
    use conary_core::ccs::legacy_scriptlets::{
        DecisionCounts, ForeignReplayPolicy, LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle,
        LegacyScriptletEntry, LifecyclePath, NativeInvocation, PublicationPolicy,
        PublicationStatus, ScriptletDecision, ScriptletFidelity, SourceFormat, TargetCompatibility,
        TransactionOrder, VersionScheme,
    };
    use conary_core::db::models::{DistroPin, settings};
    use conary_core::model::parser::SystemModel;
    use conary_core::repository::{SETTINGS_KEY_ALLOWED_DISTROS, SETTINGS_KEY_SELECTION_MODE};
    use std::collections::BTreeMap;
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    fn build_test_ccs_package(dir: &Path, name: &str, version: &str) -> PathBuf {
        build_test_ccs_package_with_bundle(dir, name, version, None)
    }

    fn build_test_ccs_package_with_bundle(
        dir: &Path,
        name: &str,
        version: &str,
        legacy_scriptlets: Option<LegacyScriptletBundle>,
    ) -> PathBuf {
        use conary_core::ccs::builder::write_ccs_package;
        use conary_core::ccs::manifest::Platform;
        use conary_core::ccs::{BuildResult, CcsManifest, ComponentData, FileEntry, FileType};
        use conary_core::hash;

        let binary_content = format!("#!/bin/sh\necho {name} {version}\n").into_bytes();
        let binary_hash = hash::sha256(&binary_content);
        let init_content = format!("#!/bin/sh\nexec /usr/bin/{name}\n").into_bytes();
        let init_hash = hash::sha256(&init_content);
        let files = vec![
            FileEntry {
                path: format!("/usr/bin/{name}"),
                hash: binary_hash.clone(),
                size: binary_content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
            FileEntry {
                path: "/usr/sbin/init".to_string(),
                hash: init_hash.clone(),
                size: init_content.len() as u64,
                mode: 0o100755,
                component: "runtime".to_string(),
                file_type: FileType::Regular,
                target: None,
                chunks: None,
            },
        ];
        let component_size = files.iter().map(|file| file.size).sum();
        let package_path = dir.join(format!("{name}-{version}.ccs"));
        let mut manifest = CcsManifest::new_minimal(name, version);
        manifest.package.platform = Some(Platform {
            os: "linux".to_string(),
            arch: Some("x86_64".to_string()),
            libc: "gnu".to_string(),
            abi: None,
        });
        manifest.legacy_scriptlets = legacy_scriptlets;
        let result = BuildResult {
            manifest,
            components: HashMap::from([(
                "runtime".to_string(),
                ComponentData {
                    name: "runtime".to_string(),
                    files: files.clone(),
                    hash: format!("{name}-runtime"),
                    size: component_size,
                },
            )]),
            files,
            blobs: HashMap::from([(binary_hash, binary_content), (init_hash, init_content)]),
            total_size: 0,
            chunked: false,
            chunk_stats: None,
        };
        write_ccs_package(&result, &package_path).unwrap();
        package_path
    }

    fn legacy_replatform_upgrade_bundle(package: &str, version: &str) -> LegacyScriptletBundle {
        let entry = legacy_replatform_upgrade_entry();
        LegacyScriptletBundle {
            schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
            schema_revision: 1,
            source_format: SourceFormat::Rpm,
            source_family: "fedora-rhel".to_string(),
            source_distro: Some("fedora".to_string()),
            source_release: Some("44".to_string()),
            source_arch: Some("x86_64".to_string()),
            source_package: package.to_string(),
            source_version: version.to_string(),
            source_checksum: None,
            version_scheme: VersionScheme::Rpm,
            conversion_tool: "remi".to_string(),
            conversion_tool_version: "0.8.0".to_string(),
            conversion_policy: "goal6-model-test".to_string(),
            adapter_registry_digest: None,
            target_policy_digest: None,
            evidence_digest: Some(conary_core::hash::sha256_prefixed(
                format!("{package}-{version}-legacy-replatform").as_bytes(),
            )),
            target_compatibility: TargetCompatibility::SourceNative,
            allowed_targets: vec!["rpm/fedora/44/x86_64".to_string()],
            foreign_replay_policy: ForeignReplayPolicy::Deny,
            publication_policy: PublicationPolicy::LocalOnly,
            publication_status: PublicationStatus::Public,
            scriptlet_fidelity: ScriptletFidelity::LegacyReplay,
            decision_counts: DecisionCounts {
                replaced: 0,
                legacy: 1,
                blocked: 0,
                review: 0,
                extra: BTreeMap::new(),
            },
            unsupported_class_counts: BTreeMap::new(),
            entries: vec![entry],
            extra: BTreeMap::new(),
        }
    }

    fn legacy_replatform_upgrade_entry() -> LegacyScriptletEntry {
        let body = "echo replay-replatform-upgrade\n";
        LegacyScriptletEntry {
            id: "rpm:%pre".to_string(),
            native_slot: "%pre".to_string(),
            phase: LifecyclePath::PreUpgrade,
            lifecycle_paths: vec!["upgrade:new-pre".to_string()],
            interpreter: "/bin/sh".to_string(),
            interpreter_args: Vec::new(),
            body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
            body: body.to_string(),
            body_encoding: None,
            native_invocation: NativeInvocation::default(),
            transaction_order: TransactionOrder {
                position: "before-payload".to_string(),
                before: Vec::new(),
                after: Vec::new(),
                extra: BTreeMap::new(),
            },
            timeout_ms: 30_000,
            sandbox: None,
            capabilities: Vec::new(),
            decision: ScriptletDecision::Legacy,
            reason_code: "legacy-replay-required".to_string(),
            human_reason: Some("test fixture".to_string()),
            evidence_digest: Some(conary_core::hash::sha256_prefixed(
                b"rpm:%pre:echo replay-replatform-upgrade",
            )),
            source_evidence_refs: vec!["capture:rpm:%pre".to_string()],
            effects: Vec::new(),
            unknown_commands: Vec::new(),
            blocked_classes: Vec::new(),
            rpm_trigger: None,
            deb_maintainer: None,
            arch_install: None,
            residual_replay: None,
            extra: BTreeMap::new(),
        }
    }

    fn serve_test_file(file_path: PathBuf) -> (String, std::thread::JoinHandle<()>) {
        let filename = file_path.file_name().unwrap().to_string_lossy().to_string();
        let bytes = std::fs::read(&file_path).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            stream.write_all(headers.as_bytes()).unwrap();
            stream.write_all(&bytes).unwrap();
        });
        (format!("http://{addr}/{filename}"), handle)
    }

    struct ReplatformMetadataFailpointReset;

    impl Drop for ReplatformMetadataFailpointReset {
        fn drop(&mut self) {
            super::apply::set_replatform_metadata_failpoint_for_test(false);
        }
    }

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

    #[tokio::test]
    async fn test_remote_diff_detects_missing() {
        use conary_core::db::models::RemoteCollection;
        use conary_core::model::SystemState;
        use std::collections::{HashMap, HashSet};

        // Create test DB and populate cache
        let (_temp_file, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();

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
                vec![conary_core::model::InstalledPackage {
                    name: "nginx".to_string(),
                    version: "1.24.2".to_string(),
                    architecture: None,
                    explicit: true,
                    pinned: false,
                    label: None,
                }],
            )]),
            explicit: HashSet::from(["nginx".to_string()]),
            pinned: HashSet::new(),
            source_pin: None,
            selection_mode: None,
            allowed_distros: Vec::new(),
        };

        // Fetch the collection from cache
        let fetched = conary_core::model::remote::fetch_remote_collection(
            &conn,
            "group-test",
            "myrepo:stable",
            false,
        )
        .await
        .unwrap();

        // Simulate the drift detection logic from cmd_model_remote_diff
        let mut missing = Vec::new();
        let mut version_drift = Vec::new();

        for member in &fetched.members {
            if let Some(installed) = state.get_package(&member.name) {
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

    #[tokio::test]
    async fn test_model_snapshot_writes_effective_source_policy() {
        let (_temp_file, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        DistroPin::set(&conn, "arch", "strict").unwrap();
        drop(conn);

        let temp_dir = tempdir().unwrap();
        let output_path = temp_dir.path().join("system.toml");

        cmd_model_snapshot(
            output_path.to_str().unwrap(),
            &db_path,
            Some("snapshot test"),
        )
        .await
        .unwrap();

        let content = std::fs::read_to_string(&output_path).unwrap();
        assert!(content.contains("[system]"));
        assert!(content.contains("profile = \"balanced/latest-anywhere\""));
        assert!(content.contains("[system.pin]"));
        assert!(content.contains("distro = \"arch\""));
        assert!(content.contains("strength = \"strict\""));
    }

    #[tokio::test]
    async fn test_model_apply_updates_source_policy_without_package_changes() {
        let (_temp_file, db_path) = create_test_db();
        let model_dir = tempdir().unwrap();
        let model_path = model_dir.path().join("system.toml");
        std::fs::write(
            &model_path,
            r#"
[model]
version = 1

[system.pin]
distro = "arch"
strength = "strict"
"#,
        )
        .unwrap();

        cmd_model_apply(ApplyOptions {
            model_path: model_path.to_str().unwrap(),
            db_path: &db_path,
            root: "/",
            dry_run: false,
            skip_optional: false,
            strict: false,
            autoremove: false,
            offline: true,
        })
        .await
        .unwrap();

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let pin = DistroPin::get_current(&conn).unwrap().unwrap();
        assert_eq!(pin.distro, "arch");
        assert_eq!(pin.mixing_policy, "strict");
    }

    #[tokio::test]
    async fn test_model_apply_updates_selection_mode_without_package_changes() {
        let (_temp_file, db_path) = create_test_db();
        let model_dir = tempdir().unwrap();
        let model_path = model_dir.path().join("system.toml");
        std::fs::write(
            &model_path,
            r#"
[model]
version = 1

[system]
selection_mode = "latest"
"#,
        )
        .unwrap();

        cmd_model_apply(ApplyOptions {
            model_path: model_path.to_str().unwrap(),
            db_path: &db_path,
            root: "/",
            dry_run: false,
            skip_optional: false,
            strict: false,
            autoremove: false,
            offline: true,
        })
        .await
        .unwrap();

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        assert_eq!(
            settings::get(&conn, SETTINGS_KEY_SELECTION_MODE).unwrap(),
            Some("latest".to_string())
        );
        assert!(DistroPin::get_current(&conn).unwrap().is_none());
    }

    #[tokio::test]
    async fn test_model_apply_updates_allowed_distros_without_package_changes() {
        let (_temp_file, db_path) = create_test_db();
        let model_dir = tempdir().unwrap();
        let model_path = model_dir.path().join("system.toml");
        std::fs::write(
            &model_path,
            r#"
[model]
version = 1

[system]
allowed_distros = ["arch"]
"#,
        )
        .unwrap();

        cmd_model_apply(ApplyOptions {
            model_path: model_path.to_str().unwrap(),
            db_path: &db_path,
            root: "/",
            dry_run: false,
            skip_optional: false,
            strict: false,
            autoremove: false,
            offline: true,
        })
        .await
        .unwrap();

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        assert_eq!(
            settings::get(&conn, SETTINGS_KEY_ALLOWED_DISTROS).unwrap(),
            Some("[\"arch\"]".to_string())
        );
        assert!(DistroPin::get_current(&conn).unwrap().is_none());
    }

    #[tokio::test]
    async fn test_model_apply_executes_replatform_replacement_when_route_is_executable() {
        use conary_core::db::models::{
            DistroPin, InstallSource, LabelEntry, PackageResolution, PrimaryStrategy, Repository,
            RepositoryPackage, ResolutionStrategy, Trove, TroveType,
        };

        let (_temp_file, db_path) = create_test_db();
        let temp_dir = tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        std::fs::create_dir_all(&install_root).unwrap();

        let package_path = build_test_ccs_package(temp_dir.path(), "vim", "9.1.0");
        let package_checksum = conary_core::hash::sha256(&std::fs::read(&package_path).unwrap());
        let (package_url, _server_handle) = serve_test_file(package_path.clone());

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        DistroPin::set(&conn, "fedora-44", "strict").unwrap();

        let mut fedora_repo = Repository::new(
            "fedora".to_string(),
            "https://example.test/fedora".to_string(),
        );
        fedora_repo.default_strategy_distro = Some("fedora-44".to_string());
        let fedora_repo_id = fedora_repo.insert(&conn).unwrap();

        let mut arch_repo = Repository::new(
            "arch-core".to_string(),
            "https://example.test/arch".to_string(),
        );
        arch_repo.default_strategy_distro = Some("arch".to_string());
        let arch_repo_id = arch_repo.insert(&conn).unwrap();

        let mut fedora_label = LabelEntry::new(
            "fedora".to_string(),
            "f43".to_string(),
            "stable".to_string(),
        );
        fedora_label.repository_id = Some(fedora_repo_id);
        let fedora_label_id = fedora_label.insert(&conn).unwrap();

        let mut installed = Trove::new_with_source(
            "vim".to_string(),
            "9.0.1".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        installed.label_id = Some(fedora_label_id);
        installed.architecture = Some("x86_64".to_string());
        installed.source_distro = Some("fedora-44".to_string());
        installed.version_scheme = Some("rpm".to_string());
        installed.installed_from_repository_id = Some(fedora_repo_id);
        installed.insert(&conn).unwrap();

        let mut arch_pkg = RepositoryPackage::new(
            arch_repo_id,
            "vim".to_string(),
            "9.1.0".to_string(),
            package_checksum.clone(),
            std::fs::metadata(&package_path)
                .unwrap()
                .len()
                .try_into()
                .unwrap(),
            package_url.clone(),
        );
        arch_pkg.architecture = Some("x86_64".to_string());
        arch_pkg.insert(&conn).unwrap();

        let mut exact_resolution = PackageResolution::new(
            arch_repo_id,
            "vim".to_string(),
            vec![ResolutionStrategy::Binary {
                url: package_url,
                checksum: package_checksum,
                delta_base: None,
            }],
        );
        exact_resolution.version = Some("9.1.0".to_string());
        exact_resolution.primary_strategy = PrimaryStrategy::Binary;
        exact_resolution.insert(&conn).unwrap();
        drop(conn);

        let model_path = temp_dir.path().join("system.toml");
        std::fs::write(
            &model_path,
            r#"
[model]
version = 1

[system.pin]
distro = "arch"
strength = "strict"
"#,
        )
        .unwrap();

        let result = cmd_model_apply(ApplyOptions {
            model_path: model_path.to_str().unwrap(),
            db_path: &db_path,
            root: install_root.to_str().unwrap(),
            dry_run: false,
            skip_optional: false,
            strict: false,
            autoremove: false,
            offline: true,
        });

        let _mount_skip = crate::commands::composefs_ops::test_mount_skip_guard();
        let result = result.await;

        result.unwrap();

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let installed_troves = Trove::find_by_name(&conn, "vim").unwrap();
        assert_eq!(installed_troves.len(), 1);
        let installed = &installed_troves[0];
        assert_eq!(installed.version, "9.1.0");
        assert_eq!(installed.source_distro.as_deref(), Some("arch"));
        assert_eq!(installed.version_scheme.as_deref(), Some("arch"));
        assert_eq!(installed.installed_from_repository_id, Some(arch_repo_id));
        assert_eq!(
            installed.selection_reason.as_deref(),
            Some("Replatformed from fedora-44 to arch by model apply")
        );
        assert_eq!(
            DistroPin::get_current(&conn).unwrap().unwrap().distro,
            "arch"
        );
    }

    #[tokio::test]
    async fn test_model_apply_replatform_legacy_replay_failure_names_safe_choices() {
        use conary_core::db::models::{
            InstallSource, LabelEntry, PackageResolution, PrimaryStrategy, Repository,
            RepositoryPackage, ResolutionStrategy, Trove, TroveType,
        };

        let (_temp_file, db_path) = create_test_db();
        let temp_dir = tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        std::fs::create_dir_all(&install_root).unwrap();

        let package_path = build_test_ccs_package_with_bundle(
            temp_dir.path(),
            "vim",
            "9.1.0",
            Some(legacy_replatform_upgrade_bundle("vim", "9.1.0")),
        );
        let package_checksum = conary_core::hash::sha256(&std::fs::read(&package_path).unwrap());
        let (package_url, _server_handle) = serve_test_file(package_path.clone());

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        DistroPin::set(&conn, "fedora-44", "strict").unwrap();

        let mut fedora_repo = Repository::new(
            "fedora".to_string(),
            "https://example.test/fedora".to_string(),
        );
        fedora_repo.default_strategy_distro = Some("fedora-44".to_string());
        let fedora_repo_id = fedora_repo.insert(&conn).unwrap();

        let mut arch_repo = Repository::new(
            "arch-core".to_string(),
            "https://example.test/arch".to_string(),
        );
        arch_repo.default_strategy_distro = Some("arch".to_string());
        let arch_repo_id = arch_repo.insert(&conn).unwrap();

        let mut fedora_label = LabelEntry::new(
            "fedora".to_string(),
            "f43".to_string(),
            "stable".to_string(),
        );
        fedora_label.repository_id = Some(fedora_repo_id);
        let fedora_label_id = fedora_label.insert(&conn).unwrap();

        let mut installed = Trove::new_with_source(
            "vim".to_string(),
            "9.0.1".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        installed.label_id = Some(fedora_label_id);
        installed.architecture = Some("x86_64".to_string());
        installed.source_distro = Some("fedora-44".to_string());
        installed.version_scheme = Some("rpm".to_string());
        installed.installed_from_repository_id = Some(fedora_repo_id);
        installed.insert(&conn).unwrap();

        let mut arch_pkg = RepositoryPackage::new(
            arch_repo_id,
            "vim".to_string(),
            "9.1.0".to_string(),
            package_checksum.clone(),
            std::fs::metadata(&package_path)
                .unwrap()
                .len()
                .try_into()
                .unwrap(),
            package_url.clone(),
        );
        arch_pkg.architecture = Some("x86_64".to_string());
        arch_pkg.insert(&conn).unwrap();

        let mut exact_resolution = PackageResolution::new(
            arch_repo_id,
            "vim".to_string(),
            vec![ResolutionStrategy::Binary {
                url: package_url,
                checksum: package_checksum,
                delta_base: None,
            }],
        );
        exact_resolution.version = Some("9.1.0".to_string());
        exact_resolution.primary_strategy = PrimaryStrategy::Binary;
        exact_resolution.insert(&conn).unwrap();

        let model: SystemModel = toml::from_str(
            r#"
[model]
version = 1

[system.pin]
distro = "arch"
strength = "strict"
"#,
        )
        .unwrap();

        let state = capture_current_state(&conn).unwrap();
        let diff = compute_model_diff(&model, &state, &conn, true, false)
            .await
            .unwrap();
        let action_refs = diff.actions.iter().collect::<Vec<_>>();
        apply_source_policy_changes(&conn, &action_refs).unwrap();
        drop(conn);

        let _mount_skip = crate::commands::composefs_ops::test_mount_skip_guard();
        let (executed, errors) =
            apply_replatform_changes(&db_path, install_root.to_str().unwrap(), &action_refs)
                .await
                .unwrap();

        assert_eq!(executed, 0);
        assert_eq!(errors.len(), 1);
        let error = &errors[0];
        assert!(error.contains("Replatform 'vim'"), "{error}");
        assert!(error.contains("LegacyReplayFeatureDisabled"), "{error}");
        assert!(
            error.contains("select a different target distro"),
            "{error}"
        );
        assert!(error.contains("wait for adapter coverage"), "{error}");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let installed_troves = Trove::find_by_name(&conn, "vim").unwrap();
        assert_eq!(installed_troves.len(), 1);
        assert_eq!(installed_troves[0].version, "9.0.1");
    }

    #[tokio::test]
    async fn test_model_apply_rolls_back_or_reports_partial_failure_during_replatform() {
        use conary_core::db::models::{
            InstallSource, LabelEntry, PackageResolution, PrimaryStrategy, Repository,
            RepositoryPackage, ResolutionStrategy, Trove, TroveType,
        };

        let (_temp_file, db_path) = create_test_db();
        let temp_dir = tempdir().unwrap();
        let install_root = temp_dir.path().join("root");
        std::fs::create_dir_all(&install_root).unwrap();

        let package_path = build_test_ccs_package(temp_dir.path(), "vim", "9.1.0");
        let package_checksum = conary_core::hash::sha256(&std::fs::read(&package_path).unwrap());
        let (package_url, _server_handle) = serve_test_file(package_path.clone());

        let conn = rusqlite::Connection::open(&db_path).unwrap();

        let mut fedora_repo = Repository::new(
            "fedora".to_string(),
            "https://example.test/fedora".to_string(),
        );
        fedora_repo.default_strategy_distro = Some("fedora-44".to_string());
        let fedora_repo_id = fedora_repo.insert(&conn).unwrap();

        let mut arch_repo = Repository::new(
            "arch-core".to_string(),
            "https://example.test/arch".to_string(),
        );
        arch_repo.default_strategy_distro = Some("arch".to_string());
        let arch_repo_id = arch_repo.insert(&conn).unwrap();

        let mut fedora_label = LabelEntry::new(
            "fedora".to_string(),
            "f43".to_string(),
            "stable".to_string(),
        );
        fedora_label.repository_id = Some(fedora_repo_id);
        let fedora_label_id = fedora_label.insert(&conn).unwrap();

        let mut installed = Trove::new_with_source(
            "vim".to_string(),
            "9.0.1".to_string(),
            TroveType::Package,
            InstallSource::Repository,
        );
        installed.label_id = Some(fedora_label_id);
        installed.architecture = Some("x86_64".to_string());
        installed.source_distro = Some("fedora-44".to_string());
        installed.version_scheme = Some("rpm".to_string());
        installed.installed_from_repository_id = Some(fedora_repo_id);
        installed.insert(&conn).unwrap();

        let mut arch_pkg = RepositoryPackage::new(
            arch_repo_id,
            "vim".to_string(),
            "9.1.0".to_string(),
            package_checksum.clone(),
            std::fs::metadata(&package_path)
                .unwrap()
                .len()
                .try_into()
                .unwrap(),
            package_url.clone(),
        );
        arch_pkg.architecture = Some("x86_64".to_string());
        arch_pkg.insert(&conn).unwrap();

        let mut exact_resolution = PackageResolution::new(
            arch_repo_id,
            "vim".to_string(),
            vec![ResolutionStrategy::Binary {
                url: package_url,
                checksum: package_checksum,
                delta_base: None,
            }],
        );
        exact_resolution.version = Some("9.1.0".to_string());
        exact_resolution.primary_strategy = PrimaryStrategy::Binary;
        exact_resolution.insert(&conn).unwrap();

        let model: SystemModel = toml::from_str(
            r#"
[model]
version = 1

[system.pin]
distro = "arch"
strength = "strict"
"#,
        )
        .unwrap();

        let state = capture_current_state(&conn).unwrap();
        let diff = compute_model_diff(&model, &state, &conn, true, false)
            .await
            .unwrap();
        drop(conn);

        super::apply::set_replatform_metadata_failpoint_for_test(true);
        let _reset = ReplatformMetadataFailpointReset;

        let action_refs = diff.actions.iter().collect::<Vec<_>>();
        let _mount_skip = crate::commands::composefs_ops::test_mount_skip_guard();
        let (executed, errors) =
            apply_replatform_changes(&db_path, install_root.to_str().unwrap(), &action_refs)
                .await
                .unwrap();

        assert_eq!(executed, 0);
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].contains("failed to finalize replatform metadata"),
            "expected explicit execution failure, got: {}",
            errors[0]
        );
        assert!(
            !errors[0].contains("blocked"),
            "execution failure should not be reported as blocked: {}",
            errors[0]
        );

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let installed_troves = Trove::find_by_name(&conn, "vim").unwrap();
        assert_eq!(installed_troves.len(), 1);
        let installed = &installed_troves[0];
        assert_eq!(installed.version, "9.1.0");
        assert_eq!(installed.source_distro.as_deref(), Some("arch"));
        assert_eq!(installed.version_scheme.as_deref(), Some("arch"));
        assert_eq!(installed.installed_from_repository_id, Some(arch_repo_id));
        assert_eq!(
            installed.selection_reason.as_deref(),
            Some("Replatform partial failure after install: injected replatform metadata failure")
        );
    }
}
