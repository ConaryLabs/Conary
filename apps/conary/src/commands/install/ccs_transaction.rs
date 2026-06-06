// apps/conary/src/commands/install/ccs_transaction.rs
//! Direct CCS package transaction install adapter.
//!
//! This module owns the direct CCS transaction entry point and CCS-specific
//! manifest selection, hook-status, and capability-gate helpers. Shared install
//! transaction mechanics stay in `install/mod.rs`.

use super::legacy_replay::{
    LegacyReplayExecutionScope, build_legacy_replay_audit_for_install,
    execute_legacy_replay_plan_entries, legacy_post_replay_warnings, require_legacy_replay_success,
};
use super::scriptlets::build_execution_mode;
use super::{
    ComponentSelection, ExtractionResult, InstallPhase, InstallProgress, InstallSemantics,
    LegacyReplayOptions, RepositoryInstallProvenance, ScriptletContext, TransactionContext,
    UpgradeCheck, check_upgrade_status, execute_install_transaction,
    finalize_install_without_snapshot, merge_old_upgrade_legacy_replay_state,
    plan_ccs_fresh_install_legacy_replay, plan_ccs_old_installed_upgrade_legacy_replay,
    preflight_extracted_live_root_file_ownership, prepare_install_environment_before_scriptlets,
    run_pre_install_phase, show_dry_run_summary,
};
use anyhow::{Context, Result};
use conary_core::components::{ComponentClassifier, ComponentType, should_run_scriptlets};
use conary_core::db::models::{Changeset, ChangesetStatus};
use conary_core::dependencies::LanguageDepDetector;
use conary_core::packages::PackageFormat;
use conary_core::scriptlet::SandboxMode;
use std::collections::HashMap;
use std::path::Path;
use tracing::{info, warn};

pub(crate) struct CcsTransactionInstallOptions<'a> {
    pub db_path: &'a str,
    pub root: &'a str,
    pub dry_run: bool,
    pub defer_generation: bool,
    pub no_scripts: bool,
    pub sandbox_mode: SandboxMode,
    pub allow_downgrade: bool,
    pub reinstall: bool,
    pub selection_reason: Option<&'a str>,
    pub component_selection: ComponentSelection,
    pub selected_manifest_components: Option<Vec<String>>,
    pub repository_provenance: Option<RepositoryInstallProvenance>,
    pub legacy_replay: LegacyReplayOptions,
}

pub(crate) struct CcsTransactionInstallResult {
    pub changeset_id: i64,
    pub post_commit_warnings: Vec<String>,
}

fn extract_and_classify_ccs_manifest_files(
    pkg: &conary_core::ccs::CcsPackage,
    selected_component_names: &[String],
    root_path: &Path,
    progress: &InstallProgress,
) -> Result<ExtractionResult> {
    progress.set_phase(pkg.name(), InstallPhase::Extracting);
    info!(
        "Extracting CCS file contents for manifest components: {:?}",
        selected_component_names
    );

    let selected_component_set: std::collections::HashSet<&str> = selected_component_names
        .iter()
        .map(String::as_str)
        .collect();
    let selected_entries: Vec<_> = pkg
        .file_entries()
        .iter()
        .filter(|file| selected_component_set.contains(file.component.as_str()))
        .collect();
    let selected_paths: std::collections::HashSet<&str> = selected_entries
        .iter()
        .map(|file| file.path.as_str())
        .collect();

    let extracted_files: Vec<_> = if selected_paths.is_empty() {
        Vec::new()
    } else {
        pkg.extract_file_contents()?
            .into_iter()
            .filter(|file| selected_paths.contains(file.path.as_str()))
            .collect()
    };
    if extracted_files.is_empty() && !selected_entries.is_empty() {
        anyhow::bail!(
            "No files matched the selected CCS components: {}",
            selected_component_names.join(", ")
        );
    }

    let extracted_files =
        crate::commands::ccs::normalize_ccs_extracted_files(root_path, extracted_files)?;

    let mut component_names_by_path = HashMap::new();
    for file in &selected_entries {
        let normalized_path =
            crate::commands::ccs::normalize_ccs_package_path(root_path, file.path.as_str())?;
        component_names_by_path
            .entry(normalized_path)
            .or_insert_with(|| file.component.clone());
    }

    let file_paths: Vec<String> = extracted_files.iter().map(|f| f.path.clone()).collect();
    let classified = ComponentClassifier::classify_all(&file_paths);
    let installed_component_types: Vec<ComponentType> = classified.keys().copied().collect();
    let mut installed_component_names: Vec<String> = selected_entries
        .iter()
        .map(|file| file.component.clone())
        .collect();
    installed_component_names.sort();
    installed_component_names.dedup();

    let language_provides = LanguageDepDetector::detect_all_provides(&file_paths);
    if !language_provides.is_empty() {
        info!(
            "Detected {} language-specific provides from CCS components",
            language_provides.len()
        );
    }

    Ok(ExtractionResult {
        extracted_files,
        classified,
        component_names_by_path: Some(component_names_by_path),
        installed_component_names: Some(installed_component_names),
        ccs_pre_remove_script: None,
        installed_component_types,
        skipped_components: Vec::new(),
        language_provides,
    })
}

fn check_ccs_upgrade_status(
    conn: &rusqlite::Connection,
    pkg: &conary_core::ccs::CcsPackage,
    semantics: &InstallSemantics,
    allow_downgrade: bool,
    reinstall: bool,
) -> Result<UpgradeCheck> {
    let existing = conary_core::db::models::Trove::find_by_name(conn, pkg.name())?;

    for trove in &existing {
        if trove.architecture == pkg.architecture().map(|s: &str| s.to_string())
            && trove.version == pkg.version()
        {
            if reinstall {
                info!("Reinstalling {} version {}", pkg.name(), pkg.version());
                return Ok(UpgradeCheck::Upgrade(Box::new(trove.clone())));
            }
            return Err(anyhow::anyhow!(
                "Package {} version {} ({}) is already installed",
                pkg.name(),
                pkg.version(),
                pkg.architecture().unwrap_or("no-arch")
            ));
        }
    }

    check_upgrade_status(conn, pkg, semantics, allow_downgrade)
}

fn mark_ccs_changeset_post_hooks_failed(
    conn: &rusqlite::Connection,
    changeset_id: i64,
    warning: &str,
) {
    match Changeset::find_by_id(conn, changeset_id) {
        Ok(Some(mut changeset)) => {
            if let Err(error) = changeset.update_status(conn, ChangesetStatus::PostHooksFailed) {
                warn!(
                    changeset_id,
                    "Failed to mark changeset after CCS post-install hook failure: {}", error
                );
            } else {
                warn!(
                    changeset_id,
                    "Marked applied changeset as post_hooks_failed: {}", warning
                );
            }
        }
        Ok(None) => warn!(
            changeset_id,
            "Could not mark CCS post-hook failure because the changeset no longer exists"
        ),
        Err(error) => warn!(
            changeset_id,
            "Failed to load changeset after CCS post-install hook failure: {}", error
        ),
    }
}

fn ccs_has_pre_hooks(hooks: &conary_core::ccs::manifest::Hooks) -> bool {
    !hooks.users.is_empty() || !hooks.groups.is_empty() || !hooks.directories.is_empty()
}

fn ccs_has_post_hooks(hooks: &conary_core::ccs::manifest::Hooks) -> bool {
    !hooks.systemd.is_empty()
        || !hooks.tmpfiles.is_empty()
        || !hooks.sysctl.is_empty()
        || !hooks.alternatives.is_empty()
        || hooks.post_install.is_some()
}

fn enforce_ccs_scriptlet_capability_gate(
    pkg: &conary_core::ccs::CcsPackage,
    no_scripts: bool,
    sandbox_mode: SandboxMode,
) -> Result<()> {
    if no_scripts || !pkg.manifest().scriptlets.has_capability_declarations() {
        return Ok(());
    }

    if sandbox_mode == SandboxMode::None {
        return Ok(());
    }

    anyhow::bail!(
        "scriptlet capability declarations are present but enforcement is not available; \
         enable supported capability enforcement or run inside a VM. Dangerous legacy direct \
         execution requires --sandbox=never plus the live-host mutation acknowledgement and \
         records effective_sandbox=direct."
    );
}

pub(crate) fn install_ccs_package_transactionally(
    conn: &mut rusqlite::Connection,
    pkg: &conary_core::ccs::CcsPackage,
    opts: CcsTransactionInstallOptions<'_>,
) -> Result<CcsTransactionInstallResult> {
    let progress = InstallProgress::single("Installing");
    let semantics = InstallSemantics::ccs();
    enforce_ccs_scriptlet_capability_gate(pkg, opts.no_scripts, opts.sandbox_mode)?;
    let upgrade =
        check_ccs_upgrade_status(conn, pkg, &semantics, opts.allow_downgrade, opts.reinstall)?;
    let old_trove = match &upgrade {
        UpgradeCheck::FreshInstall => None,
        UpgradeCheck::Upgrade(trove) | UpgradeCheck::Downgrade(trove) => Some(trove.as_ref()),
    };

    let mut extraction =
        if let Some(selected_manifest_components) = opts.selected_manifest_components.as_deref() {
            extract_and_classify_ccs_manifest_files(
                pkg,
                selected_manifest_components,
                Path::new(opts.root),
                &progress,
            )?
        } else {
            let mut selected_manifest_components: Vec<String> =
                pkg.components().keys().cloned().collect();
            selected_manifest_components.sort();
            extract_and_classify_ccs_manifest_files(
                pkg,
                &selected_manifest_components,
                Path::new(opts.root),
                &progress,
            )?
        };
    extraction.ccs_pre_remove_script = pkg
        .manifest()
        .hooks
        .pre_remove
        .as_ref()
        .map(|hook| hook.script.clone());

    let legacy_bundle = pkg.manifest().legacy_scriptlets.as_ref();
    let mut legacy_replay_state =
        plan_ccs_fresh_install_legacy_replay(conn, legacy_bundle, &opts, old_trove.is_some())?;
    let old_legacy_replay_state =
        plan_ccs_old_installed_upgrade_legacy_replay(conn, old_trove, &opts)?;
    merge_old_upgrade_legacy_replay_state(&mut legacy_replay_state, old_legacy_replay_state);

    if opts.dry_run {
        show_dry_run_summary(pkg, &opts.component_selection);
        return Ok(CcsTransactionInstallResult {
            changeset_id: 0,
            post_commit_warnings: Vec::new(),
        });
    }

    let hooks = &pkg.manifest().hooks;
    let should_run_ccs_hooks =
        !opts.no_scripts && should_run_scriptlets(&extraction.installed_component_types);
    if !opts.no_scripts
        && !should_run_ccs_hooks
        && (ccs_has_pre_hooks(hooks) || ccs_has_post_hooks(hooks))
    {
        info!(
            "Skipping CCS install hooks for non-runtime component selection: {:?}",
            extraction.installed_component_types
        );
    }

    let selected_component_names =
        if let Some(selected) = opts.selected_manifest_components.as_ref() {
            selected.clone()
        } else {
            let mut names: Vec<String> = pkg.components().keys().cloned().collect();
            names.sort();
            names
        };
    crate::commands::ccs::validate_ccs_payload_paths(
        Path::new(opts.root),
        pkg,
        &selected_component_names,
    )?;
    let execution_path =
        prepare_install_environment_before_scriptlets(conn, opts.db_path, opts.root)?;
    preflight_extracted_live_root_file_ownership(conn, pkg, &extraction, execution_path)?;
    let legacy_execution_mode = build_execution_mode(old_trove.map(|trove| trove.version.as_str()));
    let old_legacy_pre_outcomes = if let Some(old_trove) = old_trove {
        execute_legacy_replay_plan_entries(
            LegacyReplayExecutionScope {
                root: Path::new(opts.root),
                package_name: &old_trove.name,
                package_version: &old_trove.version,
                mode: &legacy_execution_mode,
                sandbox_mode: opts.sandbox_mode,
                old_version: Some(&old_trove.version),
                new_version: Some(pkg.version()),
            },
            legacy_replay_state.old_bundle_to_replay.as_ref(),
            legacy_replay_state.old_bundle_pre_remove_plan.as_ref(),
        )?
    } else {
        Vec::new()
    };
    require_legacy_replay_success(&old_legacy_pre_outcomes)?;

    let legacy_pre_outcomes = execute_legacy_replay_plan_entries(
        LegacyReplayExecutionScope {
            root: Path::new(opts.root),
            package_name: pkg.name(),
            package_version: pkg.version(),
            mode: &legacy_execution_mode,
            sandbox_mode: opts.sandbox_mode,
            old_version: old_trove.map(|trove| trove.version.as_str()),
            new_version: Some(pkg.version()),
        },
        legacy_bundle,
        legacy_replay_state.new_bundle_pre_plan.as_ref(),
    )?;
    require_legacy_replay_success(&legacy_pre_outcomes)?;

    let mut hook_executor = conary_core::ccs::HookExecutor::new(Path::new(opts.root));
    let mut pre_hooks_ran = false;
    if should_run_ccs_hooks && ccs_has_pre_hooks(hooks) {
        info!("Executing CCS pre-install hooks");
        pre_hooks_ran = true;
        if let Err(error) = hook_executor.execute_pre_hooks(hooks) {
            if let Err(revert_error) = hook_executor.revert_pre_hooks() {
                warn!(
                    "Failed to revert CCS pre-install hooks after pre-hook error: {}",
                    revert_error
                );
            }
            return Err(error).context("CCS pre-install hook failed");
        }
    }

    let scriptlet_ctx = ScriptletContext {
        root: opts.root,
        no_scripts: opts.no_scripts,
        sandbox_mode: opts.sandbox_mode,
        semantics,
        old_trove,
    };
    let pre_state = run_pre_install_phase(
        conn,
        pkg,
        &extraction.installed_component_types,
        &scriptlet_ctx,
        &progress,
    )?;

    let tx_ctx = TransactionContext {
        db_path: opts.db_path,
        root: opts.root,
        semantics,
        selection_reason: opts.selection_reason,
        old_trove_to_upgrade: old_trove,
        ccs_manifest_provides: Some(&pkg.manifest().provides),
        ccs_capabilities: pkg.manifest().capabilities.as_ref(),
        execution_path,
        defer_generation: opts.defer_generation,
        repository_provenance: opts.repository_provenance,
        legacy_replay: opts.legacy_replay,
        accepted_legacy_bundle: legacy_replay_state.accepted_bundle_to_persist.as_ref(),
    };
    let tx_result = match execute_install_transaction(conn, pkg, &extraction, &tx_ctx, &progress) {
        Ok(result) => result,
        Err(error) => {
            if pre_hooks_ran && let Err(revert_error) = hook_executor.revert_pre_hooks() {
                warn!(
                    "Failed to revert CCS pre-install hooks after install failure: {}",
                    revert_error
                );
            }
            return Err(error);
        }
    };

    finalize_install_without_snapshot(
        conn,
        pkg,
        &extraction,
        &scriptlet_ctx,
        &pre_state,
        &tx_result,
        &progress,
    )?;

    let mut post_commit_warnings = Vec::new();
    let old_legacy_post_outcomes = if let Some(old_trove) = old_trove {
        execute_legacy_replay_plan_entries(
            LegacyReplayExecutionScope {
                root: Path::new(opts.root),
                package_name: &old_trove.name,
                package_version: &old_trove.version,
                mode: &legacy_execution_mode,
                sandbox_mode: opts.sandbox_mode,
                old_version: Some(&old_trove.version),
                new_version: Some(pkg.version()),
            },
            legacy_replay_state.old_bundle_to_replay.as_ref(),
            legacy_replay_state.old_bundle_post_remove_plan.as_ref(),
        )?
    } else {
        Vec::new()
    };
    let old_legacy_post_warnings =
        legacy_post_replay_warnings(pkg.name(), &old_legacy_post_outcomes)?;
    if !old_legacy_post_warnings.is_empty() {
        crate::commands::append_scriptlet_warning_metadata(
            conn,
            tx_result.changeset_id,
            old_legacy_post_warnings.clone(),
        )?;
        post_commit_warnings.extend(
            old_legacy_post_warnings
                .into_iter()
                .map(|warning| warning.message),
        );
    }

    if should_run_ccs_hooks && ccs_has_post_hooks(hooks) {
        info!("Executing CCS post-install hooks");
        let results = hook_executor.execute_post_hooks_with_results(hooks);
        let failures = results
            .failures()
            .map(|failure| {
                format!(
                    "{} '{}' failed: {}",
                    failure.hook_type,
                    failure.name,
                    failure.error.as_deref().unwrap_or("unknown error")
                )
            })
            .collect::<Vec<_>>();
        if !failures.is_empty() {
            let warning = format!(
                "Post-install hooks failed for {} {} after commit: {}",
                pkg.name(),
                pkg.version(),
                failures.join("; ")
            );
            warn!(
                changeset_id = tx_result.changeset_id,
                package = pkg.name(),
                version = pkg.version(),
                "CCS post-install hooks failed after DB commit: {}",
                warning
            );
            mark_ccs_changeset_post_hooks_failed(conn, tx_result.changeset_id, &warning);
            eprintln!("WARNING: {warning}");
            post_commit_warnings.push(warning);
        }
    }
    let legacy_post_outcomes = execute_legacy_replay_plan_entries(
        LegacyReplayExecutionScope {
            root: Path::new(opts.root),
            package_name: pkg.name(),
            package_version: pkg.version(),
            mode: &legacy_execution_mode,
            sandbox_mode: opts.sandbox_mode,
            old_version: old_trove.map(|trove| trove.version.as_str()),
            new_version: Some(pkg.version()),
        },
        legacy_bundle,
        legacy_replay_state.new_bundle_post_plan.as_ref(),
    )?;
    let legacy_post_warnings = legacy_post_replay_warnings(pkg.name(), &legacy_post_outcomes)?;
    if !legacy_post_warnings.is_empty() {
        crate::commands::append_scriptlet_warning_metadata(
            conn,
            tx_result.changeset_id,
            legacy_post_warnings.clone(),
        )?;
        post_commit_warnings.extend(
            legacy_post_warnings
                .into_iter()
                .map(|warning| warning.message),
        );
    }
    if let Some(audit) = build_legacy_replay_audit_for_install(
        &legacy_replay_state,
        &old_legacy_pre_outcomes,
        &legacy_pre_outcomes,
        &old_legacy_post_outcomes,
        &legacy_post_outcomes,
    ) {
        crate::commands::append_legacy_replay_audit_metadata(conn, tx_result.changeset_id, audit)?;
    }

    Ok(CcsTransactionInstallResult {
        changeset_id: tx_result.changeset_id,
        post_commit_warnings,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn ccs_transaction_install_preflights_live_root_ownership_before_hooks_and_scriptlets() {
        let source = include_str!("ccs_transaction.rs");
        let install_start = source
            .find("install_ccs_package_transactionally")
            .expect("install_ccs_package_transactionally should exist");
        let test_module_start = source[install_start..]
            .find("#[cfg(test)]")
            .unwrap_or(source[install_start..].len());
        let install_source = &source[install_start..install_start + test_module_start];

        let extraction_pos = install_source
            .find("extract_and_classify_ccs_manifest_files")
            .expect("CCS transaction install should extract files");
        let preflight_pos = install_source
            .find("preflight_extracted_live_root_file_ownership(")
            .expect("CCS transaction install should preflight live-root ownership");
        let ccs_hook_pos = install_source
            .find("hook_executor.execute_pre_hooks")
            .expect("CCS transaction install should run pre-hooks");
        let scriptlet_pos = install_source
            .find("run_pre_install_phase(")
            .expect("CCS transaction install should run pre-install scriptlets");

        assert!(
            extraction_pos < preflight_pos
                && preflight_pos < ccs_hook_pos
                && preflight_pos < scriptlet_pos,
            "CCS transaction installs must preflight live-root ownership before hooks and scriptlets"
        );
    }
}
