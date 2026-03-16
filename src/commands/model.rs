// src/commands/model.rs

//! System Model Commands
//!
//! Commands for declarative system state management using model files.

use std::path::Path;
use std::process;

use super::open_db;
use anyhow::{Context, Result, anyhow};
use conary_core::db;
use conary_core::db::models::{
    CollectionMember, RemoteCollection, Repository, SystemAffinity, Trove, TroveType,
};
use conary_core::db::models::{
    DerivedOverride, DerivedPackage, DerivedPatch, DerivedStatus, DistroPin, VersionPolicy,
};
use conary_core::derived::build_from_definition;
use conary_core::filesystem::CasStore;
use conary_core::hash::sha256;
use conary_core::model::parser::SystemModel;
use conary_core::model::remote::fetch_remote_collection;
use conary_core::model::{
    DiffAction, ModelDerivedPackage, ModelDiff, ModelDiffSummary, ReplatformEstimate,
    ReplatformExecutionPlan, ReplatformStatus, SystemState, VisibleRealignmentProposal,
    capture_current_state, compute_diff, compute_diff_with_includes_offline, parse_model_file,
    parse_trove_spec, planned_replatform_actions, replatform_estimate_from_affinities,
    replatform_execution_plan, snapshot_to_model, source_policy_replatform_snapshot,
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
    let conn = open_db(db_path)?;
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
    let mut diff = if model.has_includes() {
        if announce {
            let mode = if offline { " (offline mode)" } else { "" };
            println!(
                "Resolving {} remote include(s){}...",
                model.include.models.len(),
                mode
            );
        }
        compute_diff_with_includes_offline(model, state, conn, offline)?
    } else {
        compute_diff(model, state)
    };
    diff.replatform_estimate = compute_replatform_estimate(&diff, &SystemAffinity::list(conn)?);
    if let Some(target_distro) = diff.actions.iter().find_map(|action| match action {
        DiffAction::SetSourcePin { distro, .. } => Some(distro.as_str()),
        _ => None,
    }) {
        let snapshot = source_policy_replatform_snapshot(conn, target_distro)?;
        diff.visible_realignment_candidates =
            Some(conary_core::model::VisibleRealignmentCandidates {
                target_distro: snapshot.target_distro.clone(),
                candidate_count: snapshot.visible_realignment_candidates,
            });
        diff.visible_realignment_proposals = Some(snapshot.visible_realignment_proposals);
        let planned_actions = planned_replatform_actions(
            &conary_core::model::SourcePolicyReplatformSnapshot {
                target_distro: target_distro.to_string(),
                estimate: diff.replatform_estimate.clone(),
                visible_realignment_candidates: diff
                    .visible_realignment_proposals
                    .as_ref()
                    .map(|items| items.len())
                    .unwrap_or(0),
                visible_realignment_proposals: diff
                    .visible_realignment_proposals
                    .clone()
                    .unwrap_or_default(),
            },
            state,
        );
        if diff.structural_change_count() == 0 {
            diff.actions.extend(planned_actions);
        }
    }
    Ok(diff)
}

fn is_source_policy_action(action: &DiffAction) -> bool {
    matches!(
        action,
        DiffAction::SetSourcePin { .. } | DiffAction::ClearSourcePin
    )
}

fn is_replatform_action(action: &DiffAction) -> bool {
    matches!(action, DiffAction::ReplatformReplace { .. })
}

fn source_policy_summary(diff: &ModelDiff) -> Option<String> {
    if !diff.has_source_policy_changes() {
        return None;
    }

    Some(match diff.replatform_status() {
        Some(ReplatformStatus::PackageConvergencePlanned { .. }) => {
            "This is a source-policy transition with package convergence. Review the package plan carefully before applying."
                .to_string()
        }
        Some(ReplatformStatus::PendingWithEstimate(_)) | Some(ReplatformStatus::PolicyOnlyPending) => {
            "This is a source-policy-only transition. Applying it updates Conary's preferred package source policy without replacing packages yet."
                .to_string()
        }
        None => return None,
    })
}

fn source_policy_replatform_estimate(
    estimate: Option<&ReplatformEstimate>,
    has_structural_changes: bool,
) -> Option<String> {
    if has_structural_changes {
        return None;
    }
    estimate.map(|estimate| {
        format!(
        "Estimated replatform scope: about {} installed package(s) already align with {}, and about {} package(s) may need source realignment.",
        estimate.aligned_packages, estimate.target_distro, estimate.packages_to_realign
    )
    })
}

fn compute_replatform_estimate(
    diff: &ModelDiff,
    affinities: &[SystemAffinity],
) -> Option<ReplatformEstimate> {
    if diff.structural_change_count() > 0 {
        return None;
    }

    let target_distro = diff.actions.iter().find_map(|action| match action {
        DiffAction::SetSourcePin { distro, .. } => Some(distro.clone()),
        _ => None,
    })?;

    replatform_estimate_from_affinities(affinities, &target_distro)
}

fn source_policy_replatform_note(diff: &ModelDiff) -> Option<String> {
    match diff.replatform_status() {
        Some(ReplatformStatus::PendingWithEstimate(estimate)) => {
            source_policy_replatform_estimate(Some(&estimate), false)
        }
        Some(ReplatformStatus::PolicyOnlyPending) => Some(
            "Replatform estimate unavailable: no source affinity data yet. Run a repo sync or refresh affinity data first."
                .to_string(),
        ),
        _ => None,
    }
}

fn model_check_drift_headline(diff: &ModelDiff) -> String {
    match diff.replatform_status() {
        Some(ReplatformStatus::PendingWithEstimate(estimate)) => format!(
            "DRIFT: source policy pending replatform estimate for {} (about {} package(s) may need realignment)",
            estimate.target_distro, estimate.packages_to_realign
        ),
        Some(ReplatformStatus::PolicyOnlyPending) => {
            match diff.summary().visible_realignment_candidates {
                Some(candidates) => format!(
                    "DRIFT: source policy changed; replatform planning is still pending ({} visible package candidate(s))",
                    candidates
                ),
                None => {
                    "DRIFT: source policy changed; replatform planning is still pending".to_string()
                }
            }
        }
        Some(ReplatformStatus::PackageConvergencePlanned { structural_changes }) => format!(
            "DRIFT: source policy transition with {} planned package change(s)",
            structural_changes
        ),
        None => format!("DRIFT: {} difference(s) from model", diff.actions.len()),
    }
}

fn render_replatform_summary(summary: &ModelDiffSummary) -> Option<String> {
    if let Some(packages) = summary.replatform_pending_packages {
        return Some(format!(
            "  Replatform pending estimate: {} package(s) may need realignment",
            packages
        ));
    }

    if let Some(changes) = summary.planned_package_convergence {
        return Some(format!(
            "  Planned package convergence changes: {}",
            changes
        ));
    }

    if let Some(candidates) = summary.visible_realignment_candidates {
        return Some(format!(
            "  Visible package-level realignment candidates: {}",
            candidates
        ));
    }

    None
}

fn render_realignment_proposal_preview(proposals: &[VisibleRealignmentProposal]) -> Option<String> {
    if proposals.is_empty() {
        return None;
    }

    let preview: Vec<String> = proposals
        .iter()
        .take(3)
        .map(|proposal| {
            let mut rendered = format!(
                "{} -> {} {}",
                proposal.package, proposal.target_distro, proposal.target_version
            );
            if let Some(arch) = &proposal.architecture {
                rendered.push_str(&format!(" [{}]", arch));
            }
            rendered
        })
        .collect();

    let mut line = format!("  Visible realignment proposals: {}", preview.join(", "));
    if proposals.len() > preview.len() {
        line.push_str(&format!(", +{} more", proposals.len() - preview.len()));
    }
    Some(line)
}

fn render_replatform_execution_plan(plan: &ReplatformExecutionPlan) -> String {
    let mut lines = vec![format!(
        "Planned replatform transactions ({}):",
        plan.transactions.len()
    )];

    for transaction in &plan.transactions {
        let current = transaction
            .current_distro
            .as_deref()
            .unwrap_or("unknown source");
        let status = if transaction.executable {
            "executable"
        } else {
            "blocked"
        };
        let mut line = format!(
            "  > [{}] remove {} {} from {}, install {} {}",
            status,
            transaction.package,
            transaction.current_version,
            current,
            transaction.target_distro,
            transaction.target_version
        );
        if let Some(arch) = &transaction.architecture {
            line.push_str(&format!(" [{}]", arch));
        }
        match (
            transaction.install_repository.as_deref(),
            transaction.install_repository_package_id,
        ) {
            (Some(repo), Some(pkg_id)) => {
                line.push_str(&format!(" via {} [repo-pkg:{}]", repo, pkg_id));
                if let Some(route) = &transaction.install_route {
                    line.push_str(&format!(" [route:{}]", route));
                }
                if let Some(reason) = transaction.blocked_reason.as_ref() {
                    line.push_str(&format!(" [{}]", render_replatform_blocked_reason(reason)));
                }
            }
            _ => {
                let reason = transaction
                    .blocked_reason
                    .as_ref()
                    .map(render_replatform_blocked_reason)
                    .unwrap_or("pending repo/package resolution");
                line.push_str(&format!(" [{}]", reason));
            }
        }
        if !transaction.unresolved_dependencies.is_empty() {
            line.push_str(&format!(
                " [deps:{}]",
                transaction.unresolved_dependencies.join(", ")
            ));
        }
        lines.push(line);
    }

    lines.join("\n")
}

fn render_replatform_blocked_reason(
    reason: &conary_core::model::ReplatformBlockedReason,
) -> &'static str {
    match reason {
        conary_core::model::ReplatformBlockedReason::MissingRepositoryMetadata => {
            "missing repository metadata"
        }
        conary_core::model::ReplatformBlockedReason::MissingRepositoryPackageId => {
            "missing repository package id"
        }
        conary_core::model::ReplatformBlockedReason::AnyVersionRouteOnly => {
            "only any-version install route"
        }
        conary_core::model::ReplatformBlockedReason::MissingVersionedInstallRoute => {
            "missing versioned install route"
        }
        conary_core::model::ReplatformBlockedReason::MissingInstallRoute => "missing install route",
        conary_core::model::ReplatformBlockedReason::UnsatisfiedTargetDependencies => {
            "unsatisfied target dependencies"
        }
        conary_core::model::ReplatformBlockedReason::ArchitectureMismatch => {
            "architecture mismatch"
        }
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
    let model_bytes = std::fs::read(model_path)
        .with_context(|| format!("Failed to read model file '{}'", model_path.display()))?;
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

        let patch_content = std::fs::read(&full_path)
            .with_context(|| format!("Failed to read patch file '{}'", full_path.display()))?;
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

            let content = std::fs::read(&full_source).with_context(|| {
                format!(
                    "Failed to read override source file '{}'",
                    full_source.display()
                )
            })?;
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
    let (_model, conn, diff) = load_model_and_diff(model_path, db_path, offline, true)?;
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

    if let Some(summary) = source_policy_summary(&diff) {
        println!("{}", summary);
        println!();
    }

    if let Some(estimate) = source_policy_replatform_note(&diff) {
        println!("{}", estimate);
        println!();
    }

    println!(
        "Summary: {} install(s), {} remove(s), {} source policy change(s), {} other change(s)",
        summary.installs, summary.removes, summary.source_policy_changes, summary.other_changes
    );
    if let Some(replatform) = render_replatform_summary(&summary) {
        println!("{}", replatform);
    }
    if let Some(plan) = replatform_execution_plan(&conn, &diff.actions)? {
        println!("{}", render_replatform_execution_plan(&plan));
    } else if let Some(proposals) = diff.visible_realignment_proposals.as_ref()
        && let Some(preview) = render_realignment_proposal_preview(proposals)
    {
        println!("{}", preview);
    }

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
    let diff_summary = diff.summary();

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
        &actions
            .iter()
            .map(|action| (*action).clone())
            .collect::<Vec<_>>(),
    )? {
        println!("{}", render_replatform_execution_plan(&plan));
        println!();
        println!(
            "Replatform replacement actions are planning-only in this slice. Review them here; automatic replacement execution is still pending."
        );
        println!();
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

    for action in &actions {
        match action {
            DiffAction::SetSourcePin { distro, strength } => {
                let strength = strength.as_deref().unwrap_or("guarded");
                DistroPin::set(&conn, distro, strength)?;
                println!("Updated source policy pin: {} ({})", distro, strength);
            }
            DiffAction::ClearSourcePin => {
                DistroPin::remove(&conn)?;
                println!("Cleared source policy pin");
            }
            _ => {}
        }
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
    if diff_summary.source_policy_changes > 0 {
        println!(
            "  Source policy changes applied: {}",
            diff_summary.source_policy_changes
        );
    }
    if let Some(replatform) = render_replatform_summary(&diff_summary) {
        println!("{}", replatform);
    }
    if let Some(plan) = replatform_execution_plan(&conn, &diff.actions)? {
        println!("{}", render_replatform_execution_plan(&plan));
    } else if let Some(proposals) = diff.visible_realignment_proposals.as_ref()
        && let Some(preview) = render_realignment_proposal_preview(proposals)
    {
        println!("{}", preview);
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
    process::exit(2)
}

/// Create a model file from current system state
pub fn cmd_model_snapshot(
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
pub fn cmd_model_remote_diff(model_path: &str, db_path: &str, refresh: bool) -> Result<()> {
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
pub fn cmd_model_lock(model_path: &str, output: Option<&str>, db_path: &str) -> Result<()> {
    let model_path = Path::new(model_path);
    let model = load_model(model_path)?;
    let conn = open_db(db_path)?;

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
    let mut conn = open_db(db_path)?;

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
    use conary_core::db::models::{
        DistroPin, InstallSource, LabelEntry, PackageResolution, PrimaryStrategy, Repository,
        RepositoryPackage, ResolutionStrategy, Trove,
    };
    use conary_core::db::schema;
    use conary_core::model::ReplatformBlockedReason;
    use conary_core::model::parser::SystemModel;
    use tempfile::{NamedTempFile, tempdir};

    fn create_test_db() -> (NamedTempFile, String) {
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().display().to_string();
        let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        drop(conn);
        (temp_file, db_path)
    }

    fn seed_mixed_replatform_fixture(conn: &rusqlite::Connection) {
        let mut fedora_repo = Repository::new(
            "fedora".to_string(),
            "https://example.test/fedora".to_string(),
        );
        fedora_repo.default_strategy_distro = Some("fedora-43".to_string());
        let fedora_repo_id = fedora_repo.insert(conn).unwrap();

        let mut arch_repo = Repository::new(
            "arch-core".to_string(),
            "https://example.test/arch".to_string(),
        );
        arch_repo.default_strategy = Some("legacy".to_string());
        arch_repo.default_strategy_distro = Some("arch".to_string());
        let arch_repo_id = arch_repo.insert(conn).unwrap();

        let mut fedora_label = LabelEntry::new(
            "fedora".to_string(),
            "f43".to_string(),
            "stable".to_string(),
        );
        fedora_label.insert(conn).unwrap();
        fedora_label
            .set_repository(conn, Some(fedora_repo_id))
            .unwrap();

        for (name, version) in [("vim", "9.0.1"), ("bash", "5.1.0"), ("zsh", "5.8.0")] {
            let mut trove = Trove::new_with_source(
                name.to_string(),
                version.to_string(),
                TroveType::Package,
                InstallSource::Repository,
            );
            trove.architecture = Some("x86_64".to_string());
            trove.label_id = fedora_label.id;
            trove.insert(conn).unwrap();
        }

        for (name, version) in [("vim", "9.1.0"), ("bash", "5.2.0"), ("zsh", "5.9.1")] {
            let mut pkg = RepositoryPackage::new(
                arch_repo_id,
                name.to_string(),
                version.to_string(),
                format!("sha256:{name}"),
                123,
                format!("https://example.test/arch/{name}.pkg.tar.zst"),
            );
            pkg.architecture = Some("x86_64".to_string());
            pkg.insert(conn).unwrap();
        }

        let mut exact_resolution = PackageResolution::new(
            arch_repo_id,
            "vim".to_string(),
            vec![ResolutionStrategy::Binary {
                url: "https://example.test/arch/vim-9.1.0.ccs".to_string(),
                checksum: "sha256:vim-exact".to_string(),
                delta_base: None,
            }],
        );
        exact_resolution.primary_strategy = PrimaryStrategy::Binary;
        exact_resolution.version = Some("9.1.0".to_string());
        exact_resolution.insert(conn).unwrap();

        let mut any_version_resolution = PackageResolution::new(
            arch_repo_id,
            "bash".to_string(),
            vec![ResolutionStrategy::Binary {
                url: "https://example.test/arch/bash-latest.ccs".to_string(),
                checksum: "sha256:bash-any".to_string(),
                delta_base: None,
            }],
        );
        any_version_resolution.primary_strategy = PrimaryStrategy::Binary;
        any_version_resolution.insert(conn).unwrap();
    }

    #[test]
    fn test_version_matches_constraint_exact() {
        assert!(version_matches_constraint("1.24.0", "1.24.0"));
        assert!(!version_matches_constraint("1.24.1", "1.24.0"));
    }

    #[test]
    fn test_source_policy_summary_for_policy_only_transition() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });

        let summary = source_policy_summary(&diff).unwrap();

        assert!(summary.contains("source-policy-only transition"));
        assert!(summary.contains("without replacing packages yet"));
    }

    #[test]
    fn test_source_policy_summary_for_transition_with_package_changes() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });
        diff.actions.push(DiffAction::Install {
            package: "kernel".to_string(),
            pin: None,
            optional: false,
        });

        let summary = source_policy_summary(&diff).unwrap();

        assert!(summary.contains("source-policy transition with package convergence"));
    }

    #[test]
    fn test_source_policy_summary_policy_only_stays_conservative() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::ClearSourcePin);

        let summary = source_policy_summary(&diff).unwrap();

        assert!(summary.contains("without replacing packages yet"));
        assert!(!summary.contains("replaced"));
    }

    #[test]
    fn test_source_policy_replatform_estimate_uses_affinity_counts() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });
        let affinities = vec![
            SystemAffinity {
                distro: "arch".to_string(),
                package_count: 10,
                percentage: 25.0,
            },
            SystemAffinity {
                distro: "fedora-43".to_string(),
                package_count: 30,
                percentage: 75.0,
            },
        ];

        let estimate = compute_replatform_estimate(&diff, &affinities).unwrap();

        assert_eq!(estimate.target_distro, "arch");
        assert_eq!(estimate.aligned_packages, 10);
        assert_eq!(estimate.packages_to_realign, 30);
        assert_eq!(estimate.total_packages, 40);
    }

    #[test]
    fn test_source_policy_replatform_estimate_handles_missing_affinity_data() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });

        let estimate = compute_replatform_estimate(&diff, &[]);

        assert!(estimate.is_none());
    }

    #[test]
    fn test_source_policy_replatform_note_falls_back_when_affinity_missing() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });

        let note = source_policy_replatform_note(&diff).unwrap();

        assert!(note.contains("no source affinity data yet"));
    }

    #[test]
    fn test_model_check_drift_headline_for_pending_estimate() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });
        diff.replatform_estimate = Some(ReplatformEstimate {
            target_distro: "arch".to_string(),
            aligned_packages: 10,
            packages_to_realign: 30,
            total_packages: 40,
        });

        let headline = model_check_drift_headline(&diff);

        assert!(headline.contains("pending replatform estimate for arch"));
        assert!(headline.contains("30 package(s) may need realignment"));
    }

    #[test]
    fn test_model_check_drift_headline_for_policy_only_pending() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::ClearSourcePin);

        let headline = model_check_drift_headline(&diff);

        assert!(headline.contains("source policy changed"));
        assert!(headline.contains("replatform planning is still pending"));
    }

    #[test]
    fn test_model_check_drift_headline_mentions_visible_candidates_when_estimate_missing() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });
        diff.visible_realignment_candidates =
            Some(conary_core::model::VisibleRealignmentCandidates {
                target_distro: "arch".to_string(),
                candidate_count: 5,
            });

        let headline = model_check_drift_headline(&diff);

        assert!(headline.contains("source policy changed"));
        assert!(headline.contains("5 visible package candidate(s)"));
    }

    #[test]
    fn test_model_check_drift_headline_for_package_convergence() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });
        diff.actions.push(DiffAction::Install {
            package: "kernel".to_string(),
            pin: None,
            optional: false,
        });

        let headline = model_check_drift_headline(&diff);

        assert!(headline.contains("source policy transition"));
        assert!(headline.contains("1 planned package change(s)"));
    }

    #[test]
    fn test_compute_model_diff_surfaces_mixed_replatform_execution_states() {
        let (_temp, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        seed_mixed_replatform_fixture(&conn);

        let model: SystemModel = toml::from_str(
            r#"
[model]
version = 1

[system]
profile = "balanced/latest-anywhere"

[system.pin]
distro = "arch"
strength = "strict"
"#,
        )
        .unwrap();

        let state = capture_current_state(&conn).unwrap();
        let diff = compute_model_diff(&model, &state, &conn, false, false).unwrap();

        assert_eq!(
            diff.actions
                .iter()
                .filter(|action| matches!(action, DiffAction::ReplatformReplace { .. }))
                .count(),
            3
        );

        let plan = replatform_execution_plan(&conn, &diff.actions)
            .unwrap()
            .expect("expected replatform execution plan");

        assert_eq!(plan.transactions.len(), 3);

        let bash = plan
            .transactions
            .iter()
            .find(|transaction| transaction.package == "bash")
            .expect("expected bash transaction");
        assert!(!bash.executable);
        assert_eq!(bash.install_route.as_deref(), Some("resolution:binary"));
        assert_eq!(
            bash.blocked_reason,
            Some(ReplatformBlockedReason::AnyVersionRouteOnly)
        );

        let vim = plan
            .transactions
            .iter()
            .find(|transaction| transaction.package == "vim")
            .expect("expected vim transaction");
        assert!(vim.executable);
        assert_eq!(vim.install_route.as_deref(), Some("resolution:binary"));
        assert_eq!(vim.blocked_reason, None);

        let zsh = plan
            .transactions
            .iter()
            .find(|transaction| transaction.package == "zsh")
            .expect("expected zsh transaction");
        assert!(!zsh.executable);
        assert_eq!(zsh.install_route.as_deref(), Some("default:legacy"));
        assert_eq!(
            zsh.blocked_reason,
            Some(ReplatformBlockedReason::MissingVersionedInstallRoute)
        );
    }

    #[test]
    fn test_render_replatform_summary_for_pending_estimate() {
        let summary = ModelDiffSummary {
            installs: 0,
            removes: 0,
            source_policy_changes: 1,
            other_changes: 0,
            warnings: 1,
            replatform_pending_packages: Some(30),
            planned_package_convergence: None,
            visible_realignment_candidates: None,
        };

        let rendered = render_replatform_summary(&summary).unwrap();

        assert!(rendered.contains("Replatform pending estimate"));
        assert!(rendered.contains("30 package(s) may need realignment"));
    }

    #[test]
    fn test_render_replatform_summary_for_visible_candidates() {
        let summary = ModelDiffSummary {
            installs: 0,
            removes: 0,
            source_policy_changes: 1,
            other_changes: 0,
            warnings: 0,
            replatform_pending_packages: None,
            planned_package_convergence: None,
            visible_realignment_candidates: Some(5),
        };

        let rendered = render_replatform_summary(&summary).unwrap();

        assert!(rendered.contains("Visible package-level realignment candidates"));
        assert!(rendered.contains("5"));
    }

    #[test]
    fn test_render_realignment_proposal_preview_lists_examples() {
        let proposals = vec![
            conary_core::model::VisibleRealignmentProposal {
                package: "bash".to_string(),
                current_distro: Some("fedora-43".to_string()),
                target_distro: "arch".to_string(),
                target_version: "5.2.0".to_string(),
                architecture: Some("x86_64".to_string()),
                target_repository: Some("arch-core".to_string()),
                target_repository_package_id: Some(11),
            },
            conary_core::model::VisibleRealignmentProposal {
                package: "vim".to_string(),
                current_distro: Some("fedora-43".to_string()),
                target_distro: "arch".to_string(),
                target_version: "9.1.0".to_string(),
                architecture: Some("x86_64".to_string()),
                target_repository: Some("arch-core".to_string()),
                target_repository_package_id: Some(22),
            },
            conary_core::model::VisibleRealignmentProposal {
                package: "zsh".to_string(),
                current_distro: Some("fedora-43".to_string()),
                target_distro: "arch".to_string(),
                target_version: "5.9.1".to_string(),
                architecture: Some("x86_64".to_string()),
                target_repository: Some("arch-core".to_string()),
                target_repository_package_id: Some(33),
            },
            conary_core::model::VisibleRealignmentProposal {
                package: "curl".to_string(),
                current_distro: Some("fedora-43".to_string()),
                target_distro: "arch".to_string(),
                target_version: "8.8.0".to_string(),
                architecture: Some("x86_64".to_string()),
                target_repository: Some("arch-core".to_string()),
                target_repository_package_id: Some(44),
            },
        ];

        let rendered = render_realignment_proposal_preview(&proposals).unwrap();

        assert!(rendered.contains("bash -> arch 5.2.0"));
        assert!(rendered.contains("vim -> arch 9.1.0"));
        assert!(rendered.contains("zsh -> arch 5.9.1"));
        assert!(rendered.contains("+1 more"));
    }

    #[test]
    fn test_render_replatform_execution_plan_lists_transactions() {
        let plan = ReplatformExecutionPlan {
            transactions: vec![
                conary_core::model::ReplatformExecutionTransaction {
                    package: "bash".to_string(),
                    current_distro: Some("fedora-43".to_string()),
                    target_distro: "arch".to_string(),
                    current_version: "5.1.0".to_string(),
                    current_architecture: Some("x86_64".to_string()),
                    target_version: "5.2.0".to_string(),
                    architecture: Some("x86_64".to_string()),
                    install_repository: Some("arch-core".to_string()),
                    install_repository_package_id: Some(11),
                    install_route: Some("default:legacy".to_string()),
                    unresolved_dependencies: Vec::new(),
                    executable: false,
                    blocked_reason: Some(
                        conary_core::model::ReplatformBlockedReason::MissingVersionedInstallRoute,
                    ),
                },
                conary_core::model::ReplatformExecutionTransaction {
                    package: "vim".to_string(),
                    current_distro: Some("fedora-43".to_string()),
                    target_distro: "arch".to_string(),
                    current_version: "9.0.1".to_string(),
                    current_architecture: Some("x86_64".to_string()),
                    target_version: "9.1.0".to_string(),
                    architecture: Some("x86_64".to_string()),
                    install_repository: Some("arch-core".to_string()),
                    install_repository_package_id: Some(22),
                    install_route: Some("default:legacy".to_string()),
                    unresolved_dependencies: Vec::new(),
                    executable: false,
                    blocked_reason: Some(
                        conary_core::model::ReplatformBlockedReason::MissingVersionedInstallRoute,
                    ),
                },
            ],
        };

        let rendered = render_replatform_execution_plan(&plan);

        assert!(rendered.contains("Planned replatform transactions (2):"));
        assert!(
            rendered.contains("[blocked] remove bash 5.1.0 from fedora-43, install arch 5.2.0")
        );
        assert!(rendered.contains(
            "via arch-core [repo-pkg:11] [route:default:legacy] [missing versioned install route]"
        ));
        assert!(rendered.contains("[blocked] remove vim 9.0.1 from fedora-43, install arch 9.1.0"));
        assert!(rendered.contains(
            "via arch-core [repo-pkg:22] [route:default:legacy] [missing versioned install route]"
        ));
    }

    #[test]
    fn test_render_replatform_execution_plan_lists_block_reason() {
        let plan = ReplatformExecutionPlan {
            transactions: vec![conary_core::model::ReplatformExecutionTransaction {
                package: "vim".to_string(),
                current_distro: Some("fedora-43".to_string()),
                target_distro: "arch".to_string(),
                current_version: "9.0.1".to_string(),
                current_architecture: Some("x86_64".to_string()),
                target_version: "9.1.0".to_string(),
                architecture: Some("x86_64".to_string()),
                install_repository: None,
                install_repository_package_id: None,
                install_route: None,
                unresolved_dependencies: Vec::new(),
                executable: false,
                blocked_reason: Some(
                    conary_core::model::ReplatformBlockedReason::MissingRepositoryMetadata,
                ),
            }],
        };

        let rendered = render_replatform_execution_plan(&plan);

        assert!(rendered.contains("[blocked]"));
        assert!(rendered.contains("[missing repository metadata]"));
    }

    #[test]
    fn test_render_replatform_execution_plan_lists_any_version_reason_with_route() {
        let plan = ReplatformExecutionPlan {
            transactions: vec![conary_core::model::ReplatformExecutionTransaction {
                package: "vim".to_string(),
                current_distro: Some("fedora-43".to_string()),
                target_distro: "arch".to_string(),
                current_version: "9.0.1".to_string(),
                current_architecture: Some("x86_64".to_string()),
                target_version: "9.1.0".to_string(),
                architecture: Some("x86_64".to_string()),
                install_repository: Some("arch-core".to_string()),
                install_repository_package_id: Some(22),
                install_route: Some("resolution:binary".to_string()),
                unresolved_dependencies: Vec::new(),
                executable: false,
                blocked_reason: Some(
                    conary_core::model::ReplatformBlockedReason::AnyVersionRouteOnly,
                ),
            }],
        };

        let rendered = render_replatform_execution_plan(&plan);

        assert!(rendered.contains("[route:resolution:binary] [only any-version install route]"));
    }

    #[test]
    fn test_render_replatform_execution_plan_lists_executable_transaction() {
        let plan = ReplatformExecutionPlan {
            transactions: vec![conary_core::model::ReplatformExecutionTransaction {
                package: "vim".to_string(),
                current_distro: Some("fedora-43".to_string()),
                target_distro: "arch".to_string(),
                current_version: "9.0.1".to_string(),
                current_architecture: Some("x86_64".to_string()),
                target_version: "9.1.0".to_string(),
                architecture: Some("x86_64".to_string()),
                install_repository: Some("arch-core".to_string()),
                install_repository_package_id: Some(22),
                install_route: Some("resolution:binary".to_string()),
                unresolved_dependencies: Vec::new(),
                executable: true,
                blocked_reason: None,
            }],
        };

        let rendered = render_replatform_execution_plan(&plan);

        assert!(
            rendered.contains("[executable] remove vim 9.0.1 from fedora-43, install arch 9.1.0")
        );
        assert!(rendered.contains("via arch-core [repo-pkg:22] [route:resolution:binary]"));
        assert!(!rendered.contains("missing install route"));
        assert!(!rendered.contains("only any-version install route"));
    }

    #[test]
    fn test_render_replatform_execution_plan_lists_unresolved_dependencies() {
        let plan = ReplatformExecutionPlan {
            transactions: vec![conary_core::model::ReplatformExecutionTransaction {
                package: "vim".to_string(),
                current_distro: Some("fedora-43".to_string()),
                target_distro: "arch".to_string(),
                current_version: "9.0.1".to_string(),
                current_architecture: Some("x86_64".to_string()),
                target_version: "9.1.0".to_string(),
                architecture: Some("x86_64".to_string()),
                install_repository: Some("arch-core".to_string()),
                install_repository_package_id: Some(22),
                install_route: Some("resolution:binary".to_string()),
                unresolved_dependencies: vec!["libmagic (>= 1.0)".to_string()],
                executable: false,
                blocked_reason: Some(
                    conary_core::model::ReplatformBlockedReason::UnsatisfiedTargetDependencies,
                ),
            }],
        };

        let rendered = render_replatform_execution_plan(&plan);

        assert!(rendered.contains("[unsatisfied target dependencies]"));
        assert!(rendered.contains("[deps:libmagic (>= 1.0)]"));
    }

    #[test]
    fn test_planned_replatform_actions_promote_proposals_into_actions() {
        let snapshot = conary_core::model::SourcePolicyReplatformSnapshot {
            target_distro: "arch".to_string(),
            estimate: Some(ReplatformEstimate {
                target_distro: "arch".to_string(),
                aligned_packages: 10,
                packages_to_realign: 2,
                total_packages: 12,
            }),
            visible_realignment_candidates: 2,
            visible_realignment_proposals: vec![
                conary_core::model::VisibleRealignmentProposal {
                    package: "vim".to_string(),
                    current_distro: Some("fedora-43".to_string()),
                    target_distro: "arch".to_string(),
                    target_version: "9.1.0".to_string(),
                    architecture: Some("x86_64".to_string()),
                    target_repository: Some("arch-core".to_string()),
                    target_repository_package_id: Some(22),
                },
                conary_core::model::VisibleRealignmentProposal {
                    package: "bash".to_string(),
                    current_distro: Some("fedora-43".to_string()),
                    target_distro: "arch".to_string(),
                    target_version: "5.2.0".to_string(),
                    architecture: Some("x86_64".to_string()),
                    target_repository: Some("arch-core".to_string()),
                    target_repository_package_id: Some(11),
                },
            ],
        };
        let mut state = SystemState::new();
        state.installed.insert(
            "vim".to_string(),
            conary_core::model::InstalledPackage {
                name: "vim".to_string(),
                version: "9.0.1".to_string(),
                architecture: Some("x86_64".to_string()),
                explicit: true,
                label: Some("fedora@f43:stable".to_string()),
            },
        );
        state.installed.insert(
            "bash".to_string(),
            conary_core::model::InstalledPackage {
                name: "bash".to_string(),
                version: "5.1.0".to_string(),
                architecture: Some("x86_64".to_string()),
                explicit: true,
                label: Some("fedora@f43:stable".to_string()),
            },
        );

        let actions = planned_replatform_actions(&snapshot, &state);

        assert!(actions.iter().any(|action| {
            matches!(
                action,
                DiffAction::ReplatformReplace {
                    package,
                    current_distro,
                    target_distro,
                    current_version,
                    current_architecture,
                    target_version,
                    target_repository,
                    target_repository_package_id,
                    ..
                } if package == "vim"
                    && current_distro.as_deref() == Some("fedora-43")
                    && target_distro == "arch"
                    && current_version == "9.0.1"
                    && current_architecture.as_deref() == Some("x86_64")
                    && target_version == "9.1.0"
                    && target_repository.as_deref() == Some("arch-core")
                    && *target_repository_package_id == Some(22)
            )
        }));
        assert_eq!(actions.len(), 2);
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
            source_pin: None,
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

    #[test]
    fn test_model_snapshot_writes_effective_source_policy() {
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
        .unwrap();

        let content = std::fs::read_to_string(&output_path).unwrap();
        assert!(content.contains("[system]"));
        assert!(content.contains("profile = \"balanced/latest-anywhere\""));
        assert!(content.contains("[system.pin]"));
        assert!(content.contains("distro = \"arch\""));
        assert!(content.contains("strength = \"strict\""));
    }

    #[test]
    fn test_model_apply_updates_source_policy_without_package_changes() {
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

        cmd_model_apply(
            model_path.to_str().unwrap(),
            &db_path,
            "/",
            false,
            false,
            false,
            false,
            true,
        )
        .unwrap();

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let pin = DistroPin::get_current(&conn).unwrap().unwrap();
        assert_eq!(pin.distro, "arch");
        assert_eq!(pin.mixing_policy, "strict");
    }
}
