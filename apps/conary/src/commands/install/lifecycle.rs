// src/commands/install/lifecycle.rs

use super::scriptlets::{
    build_execution_mode, get_old_package_scriptlets, preflight_install_scriptlets,
    preflight_old_remove_scriptlets, run_old_post_remove, run_old_pre_remove, run_post_install,
    run_pre_install,
};
use super::{
    ComponentSelection, InstallPhase, InstallProgress, InstallSemantics, InstallTransactionResult,
    run_triggers,
};
use crate::commands::create_state_snapshot;
use anyhow::{Context, Result};
use conary_core::components::{ComponentClassifier, ComponentType, should_run_scriptlets};
use conary_core::db::models::DerivedPackage;
use conary_core::dependencies::{LanguageDep, LanguageDepDetector};
use conary_core::packages::PackageFormat;
use conary_core::packages::traits::ExtractedFile;
use conary_core::scriptlet::{ExecutionMode, PackageFormat as ScriptletPackageFormat, SandboxMode};
use std::collections::HashMap;
use std::path::Path;
use tracing::{info, warn};

/// Context for scriptlet execution phases.
pub(super) struct ScriptletContext<'a> {
    pub(super) root: &'a str,
    pub(super) no_scripts: bool,
    pub(super) sandbox_mode: SandboxMode,
    pub(super) semantics: InstallSemantics,
    pub(super) old_trove: Option<&'a conary_core::db::models::Trove>,
}

/// State captured during pre-install scriptlet phase, needed for post-install.
pub(super) struct PreScriptletState {
    scriptlet_format: ScriptletPackageFormat,
    execution_mode: ExecutionMode,
    old_package_scriptlets: Vec<conary_core::db::models::ScriptletEntry>,
    run_scriptlets: bool,
}

/// Result of file extraction and component classification.
pub(super) struct ExtractionResult {
    pub(super) extracted_files: Vec<ExtractedFile>,
    pub(super) classified: HashMap<ComponentType, Vec<String>>,
    pub(super) component_names_by_path: Option<HashMap<String, String>>,
    pub(super) installed_component_names: Option<Vec<String>>,
    pub(super) ccs_pre_remove_script: Option<String>,
    pub(super) installed_component_types: Vec<ComponentType>,
    pub(super) skipped_components: Vec<&'static str>,
    pub(super) language_provides: Vec<LanguageDep>,
}

pub(super) fn mark_upgraded_parent_deriveds_stale(
    conn: &rusqlite::Connection,
    parent_name: &str,
    old_version: Option<&str>,
    new_version: &str,
) {
    match DerivedPackage::mark_stale_if_parent_changed(conn, parent_name, old_version, new_version)
    {
        Ok(count) if count > 0 => {
            info!(
                "Marked {} derived package(s) stale after {} changed from {} to {}",
                count,
                parent_name,
                old_version.unwrap_or("unknown"),
                new_version
            );
        }
        Ok(_) => {}
        Err(e) => {
            warn!(
                "Failed to mark derived packages stale for upgraded parent {}: {}",
                parent_name, e
            );
        }
    }
}

/// Display a dry-run summary showing what would be installed.
pub(super) fn show_dry_run_summary(
    pkg: &dyn PackageFormat,
    component_selection: &ComponentSelection,
) {
    // For dry run, classify files to show component info
    let dry_run_paths: Vec<String> = pkg.files().iter().map(|f| f.path.clone()).collect();
    let dry_run_classified = ComponentClassifier::classify_all(&dry_run_paths);
    let dry_run_available: Vec<_> = dry_run_classified.keys().collect();
    let dry_run_selected: Vec<_> = dry_run_available
        .iter()
        .filter(|c| component_selection.should_install(***c))
        .collect();
    let dry_run_skipped: Vec<_> = dry_run_available
        .iter()
        .filter(|c| !component_selection.should_install(***c))
        .collect();

    let selected_file_count: usize = dry_run_classified
        .iter()
        .filter(|(c, _)| component_selection.should_install(**c))
        .map(|(_, files)| files.len())
        .sum();

    println!(
        "\nWould install package: {} version {}",
        pkg.name(),
        pkg.version()
    );
    println!("  Architecture: {}", pkg.architecture().unwrap_or("none"));
    println!(
        "  Components to install: {} ({} files)",
        dry_run_selected
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        selected_file_count
    );
    if !dry_run_skipped.is_empty() {
        println!(
            "  Components skipped: {} (use {}:all to include)",
            dry_run_skipped
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            pkg.name()
        );
    }
    println!("  Dependencies: {}", pkg.dependencies().len());
    println!("\nDry run complete. No changes made.");
}

/// Extract files from the package and classify them into components.
pub(super) fn extract_and_classify_files(
    pkg: &dyn PackageFormat,
    component_selection: &ComponentSelection,
    progress: &InstallProgress,
) -> Result<ExtractionResult> {
    // Extract and install
    progress.set_phase(pkg.name(), InstallPhase::Extracting);
    info!("Extracting file contents from package...");
    let extracted_files = pkg
        .extract_file_contents()
        .with_context(|| format!("Failed to extract files from package '{}'", pkg.name()))?;
    info!("Extracted {} files", extracted_files.len());

    // Classify files into components
    let file_paths: Vec<String> = extracted_files.iter().map(|f| f.path.clone()).collect();
    let all_classified = ComponentClassifier::classify_all(&file_paths);

    // Show what components are available in the package
    let available_components: Vec<ComponentType> = all_classified.keys().copied().collect();
    info!(
        "Package contains {} component types: {:?}",
        available_components.len(),
        available_components
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>()
    );

    // Filter to only selected components
    let classified: HashMap<ComponentType, Vec<String>> = all_classified
        .into_iter()
        .filter(|(comp_type, _)| component_selection.should_install(*comp_type))
        .collect();

    // Build set of paths for selected components
    let selected_paths: std::collections::HashSet<&str> =
        classified.values().flatten().map(|s| s.as_str()).collect();

    // Filter extracted files to only include selected components
    let extracted_files: Vec<_> = extracted_files
        .into_iter()
        .filter(|f| selected_paths.contains(f.path.as_str()))
        .collect();

    let installed_component_types: Vec<ComponentType> = classified.keys().copied().collect();

    // Show what we're actually installing
    let skipped_components: Vec<&str> = available_components
        .iter()
        .filter(|c| !component_selection.should_install(**c))
        .map(|c| c.as_str())
        .collect();

    if !skipped_components.is_empty() {
        info!(
            "Skipping non-default components: {:?} (use package:all to install everything)",
            skipped_components
        );
    }

    info!(
        "Installing {} files from {} component(s): {:?}",
        extracted_files.len(),
        classified.len(),
        installed_component_types
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>()
    );

    // Detect language-specific provides from installed files
    // Do this before the transaction so we can display the count in the summary
    let installed_paths: Vec<String> = extracted_files.iter().map(|f| f.path.clone()).collect();
    let language_provides = LanguageDepDetector::detect_all_provides(&installed_paths);
    if !language_provides.is_empty() {
        info!(
            "Detected {} language-specific provides: {:?}",
            language_provides.len(),
            language_provides
                .iter()
                .take(5)
                .map(|d| d.to_dep_string())
                .collect::<Vec<_>>()
        );
    }

    Ok(ExtractionResult {
        extracted_files,
        classified,
        component_names_by_path: None,
        installed_component_names: None,
        ccs_pre_remove_script: None,
        installed_component_types,
        skipped_components,
        language_provides,
    })
}

/// Run pre-install scriptlets and query old package scriptlets for upgrades.
pub(super) fn run_pre_install_phase(
    conn: &rusqlite::Connection,
    pkg: &dyn PackageFormat,
    installed_component_types: &[ComponentType],
    ctx: &ScriptletContext<'_>,
    progress: &InstallProgress,
) -> Result<PreScriptletState> {
    // Determine package format and execution mode for scriptlet execution
    let scriptlet_format = ctx.semantics.scriptlet_format;
    let execution_mode = build_execution_mode(ctx.old_trove.map(|t| t.version.as_str()));

    // Execute pre-install scriptlet (before any changes)
    // Scriptlets only run when :runtime or :lib is being installed
    let scriptlets = pkg.scriptlets();
    let run_scriptlets = should_run_scriptlets(installed_component_types);

    // Query old package's scriptlets before any scriptlet runs. This lets us
    // preflight both new and old package scriptlets before file/DB mutation.
    let old_trove_id = ctx.old_trove.and_then(|t| t.id);
    let old_package_scriptlets = get_old_package_scriptlets(conn, old_trove_id)?;

    if !ctx.no_scripts && !scriptlets.is_empty() && run_scriptlets {
        preflight_install_scriptlets(
            Path::new(ctx.root),
            pkg.name(),
            pkg.version(),
            scriptlets,
            scriptlet_format,
            &execution_mode,
            ctx.sandbox_mode,
        )?;
        progress.set_phase(pkg.name(), InstallPhase::PreScript);
        run_pre_install(
            Path::new(ctx.root),
            pkg.name(),
            pkg.version(),
            scriptlets,
            scriptlet_format,
            &execution_mode,
            ctx.sandbox_mode,
        )?;
    } else if !ctx.no_scripts && !scriptlets.is_empty() && !run_scriptlets {
        info!(
            "Skipping scriptlets: no :runtime or :lib component being installed (components: {:?})",
            installed_component_types
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
        );
    }

    if !ctx.no_scripts
        && let Some(old_trove) = ctx.old_trove
    {
        preflight_old_remove_scriptlets(
            Path::new(ctx.root),
            &old_trove.name,
            &old_trove.version,
            pkg.version(),
            &old_package_scriptlets,
            scriptlet_format,
            ctx.sandbox_mode,
        )?;
    }

    // For RPM/DEB upgrades: run old package's pre-remove scriptlet
    if !ctx.no_scripts
        && let Some(old_trove) = ctx.old_trove
    {
        run_old_pre_remove(
            Path::new(ctx.root),
            &old_trove.name,
            &old_trove.version,
            pkg.version(),
            &old_package_scriptlets,
            scriptlet_format,
            ctx.sandbox_mode,
        )?;
    }

    Ok(PreScriptletState {
        scriptlet_format,
        execution_mode,
        old_package_scriptlets,
        run_scriptlets,
    })
}

/// Run post-install scriptlets, triggers, and print the final summary.
pub(super) fn finalize_install_without_snapshot(
    conn: &rusqlite::Connection,
    pkg: &dyn PackageFormat,
    extraction: &ExtractionResult,
    scriptlet_ctx: &ScriptletContext<'_>,
    pre_state: &PreScriptletState,
    tx_result: &InstallTransactionResult,
    progress: &InstallProgress,
    quiet: bool,
) -> Result<()> {
    let mut scriptlet_warnings = Vec::new();

    // For RPM/DEB upgrades: run old package's post-remove scriptlet
    if !scriptlet_ctx.no_scripts
        && let Some(old_trove) = scriptlet_ctx.old_trove
    {
        scriptlet_warnings.extend(run_old_post_remove(
            Path::new(scriptlet_ctx.root),
            &old_trove.name,
            &old_trove.version,
            pkg.version(),
            &pre_state.old_package_scriptlets,
            pre_state.scriptlet_format,
            scriptlet_ctx.sandbox_mode,
        )?);
    }

    // Execute post-install scriptlet (after files are deployed)
    let scriptlets = pkg.scriptlets();
    if !scriptlet_ctx.no_scripts && !scriptlets.is_empty() && pre_state.run_scriptlets {
        progress.set_phase(pkg.name(), InstallPhase::PostScript);
        scriptlet_warnings.extend(run_post_install(
            Path::new(scriptlet_ctx.root),
            pkg.name(),
            pkg.version(),
            scriptlets,
            pre_state.scriptlet_format,
            &pre_state.execution_mode,
            scriptlet_ctx.sandbox_mode,
        )?);
    }

    if !scriptlet_warnings.is_empty() {
        crate::commands::append_scriptlet_warning_metadata(
            conn,
            tx_result.changeset_id,
            scriptlet_warnings,
        )?;
    }

    progress.set_phase(pkg.name(), InstallPhase::Triggers);
    let file_paths: Vec<String> = extraction
        .extracted_files
        .iter()
        .map(|f| f.path.clone())
        .collect();
    run_triggers(
        conn,
        Path::new(scriptlet_ctx.root),
        tx_result.changeset_id,
        &file_paths,
    );

    progress.finish(&format!("Installed {} {}", pkg.name(), pkg.version()));

    if !quiet {
        // Show what components were available vs installed
        let skipped_info = if !extraction.skipped_components.is_empty() {
            format!(" (skipped: {})", extraction.skipped_components.join(", "))
        } else {
            String::new()
        };

        println!(
            "Installed package: {} version {}",
            pkg.name(),
            pkg.version()
        );
        println!("  Architecture: {}", pkg.architecture().unwrap_or("none"));
        println!("  Files installed: {}", extraction.extracted_files.len());
        println!(
            "  Components: {}{}",
            extraction
                .installed_component_types
                .iter()
                .map(|c| format!(":{}", c.as_str()))
                .collect::<Vec<_>>()
                .join(", "),
            skipped_info
        );
        println!("  Dependencies: {}", pkg.dependencies().len());
        if !extraction.language_provides.is_empty() {
            println!(
                "  Provides: {} (language-specific capabilities)",
                extraction.language_provides.len()
            );
        }
    }

    Ok(())
}

pub(super) fn finalize_install(
    conn: &rusqlite::Connection,
    pkg: &dyn PackageFormat,
    extraction: &ExtractionResult,
    scriptlet_ctx: &ScriptletContext<'_>,
    pre_state: &PreScriptletState,
    tx_result: &InstallTransactionResult,
    progress: &InstallProgress,
) -> Result<()> {
    finalize_install_without_snapshot(
        conn,
        pkg,
        extraction,
        scriptlet_ctx,
        pre_state,
        tx_result,
        progress,
        false,
    )?;
    if let Err(error) = create_state_snapshot(
        conn,
        tx_result.changeset_id,
        &format!("Install {}", pkg.name()),
    ) {
        crate::commands::append_deferred_follow_up_metadata(
            conn,
            tx_result.changeset_id,
            crate::commands::DeferredFollowUp {
                kind: "state_snapshot".to_string(),
                status: "failed".to_string(),
                message: error.to_string(),
                retry_command: Some(format!(
                    "conary system state create \"Install {}\"",
                    pkg.name()
                )),
            },
        )?;
        warn!(
            changeset_id = tx_result.changeset_id,
            "Package mutation completed, but state snapshot was deferred: {}", error
        );
        eprintln!("WARNING: package mutation completed, but state snapshot was deferred: {error}");
    }
    Ok(())
}
