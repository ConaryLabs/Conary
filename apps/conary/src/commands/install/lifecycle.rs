// src/commands/install/lifecycle.rs

use super::{
    ComponentSelection, InstallPhase, InstallProgress, InstallTransactionResult, run_triggers,
};
use crate::commands::create_state_snapshot;
use anyhow::{Context, Result};
use conary_core::ccs::manifest::ScriptHook;
use conary_core::components::ComponentType;
use conary_core::db::models::DerivedPackage;
use conary_core::dependencies::LanguageDep;
use conary_core::packages::PackageFormat;
use conary_core::packages::traits::ExtractedFile;
use conary_core::repository::dependency_model::RepositoryRequirementKind;
use std::collections::HashMap;
use std::path::Path;
use tracing::{info, warn};

pub(super) struct FinalizeInstallOutput<'a> {
    progress: &'a InstallProgress,
    quiet: bool,
}

impl<'a> FinalizeInstallOutput<'a> {
    pub(super) fn new(progress: &'a InstallProgress, quiet: bool) -> Self {
        Self { progress, quiet }
    }
}

/// Result of file extraction and exact component assignment.
pub(super) struct ExtractionResult {
    pub(super) extracted_files: Vec<ExtractedFile>,
    pub(super) classified: HashMap<ComponentType, Vec<String>>,
    pub(super) component_names_by_path: Option<HashMap<String, String>>,
    pub(super) installed_component_names: Option<Vec<String>>,
    pub(super) ccs_remove_hook: Option<ScriptHook>,
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
) -> Result<()> {
    require_lossless_native_component_selection(component_selection)?;
    let selected_file_count = pkg.files().len();

    println!(
        "\nWould install package: {} version {}",
        pkg.name(),
        pkg.version()
    );
    println!("  Architecture: {}", pkg.architecture().unwrap_or("none"));
    println!(
        "  Components to install: {} ({} files)",
        ComponentType::Runtime.as_str(),
        selected_file_count
    );
    println!("  Dependencies: {}", runtime_requirement_count(pkg));
    println!("\nDry run complete. No changes made.");
    Ok(())
}

/// Extract a native package without inventing component boundaries.
///
/// Native RPM, Debian, and Arch packages do not expose Conary's component
/// contract. Their complete payload is therefore one lossless `runtime`
/// component unless a future package parser provides explicit typed metadata.
pub(super) fn extract_and_classify_files(
    pkg: &dyn PackageFormat,
    component_selection: &ComponentSelection,
    progress: &InstallProgress,
) -> Result<ExtractionResult> {
    require_lossless_native_component_selection(component_selection)?;
    // Extract and install
    progress.set_phase(pkg.name(), InstallPhase::Extracting);
    info!("Extracting file contents from package...");
    let extracted_files = pkg
        .extract_file_contents()
        .with_context(|| format!("Failed to extract files from package '{}'", pkg.name()))?;
    info!("Extracted {} files", extracted_files.len());

    let file_paths: Vec<String> = extracted_files.iter().map(|f| f.path.clone()).collect();
    let classified = HashMap::from([(ComponentType::Runtime, file_paths.clone())]);
    let installed_component_types = vec![ComponentType::Runtime];

    info!(
        "Installing all {} native package files as one lossless component: runtime",
        extracted_files.len(),
    );

    Ok(ExtractionResult {
        extracted_files,
        classified,
        component_names_by_path: None,
        installed_component_names: None,
        ccs_remove_hook: None,
        installed_component_types,
        skipped_components: Vec::new(),
        language_provides: Vec::new(),
    })
}

fn require_lossless_native_component_selection(selection: &ComponentSelection) -> Result<()> {
    if let ComponentSelection::Specific(components) = selection
        && (components.is_empty()
            || components
                .iter()
                .any(|component| *component != ComponentType::Runtime))
    {
        anyhow::bail!(
            "native packages without explicit Conary component metadata expose one lossless :runtime component; selective path-derived components are not supported"
        );
    }
    Ok(())
}

/// Run post-install triggers and print the final summary.
pub(super) fn finalize_install_without_snapshot(
    conn: &rusqlite::Connection,
    pkg: &dyn PackageFormat,
    extraction: &ExtractionResult,
    root: &str,
    tx_result: &InstallTransactionResult,
    output: FinalizeInstallOutput<'_>,
) -> Result<()> {
    output
        .progress
        .set_phase(pkg.name(), InstallPhase::Triggers);
    if !tx_result.triggers_executed {
        let file_paths: Vec<String> = extraction
            .extracted_files
            .iter()
            .map(|f| f.path.clone())
            .collect();
        run_triggers(conn, Path::new(root), tx_result.changeset_id, &file_paths)?;
    }

    output
        .progress
        .finish(&format!("Installed {} {}", pkg.name(), pkg.version()));

    if !output.quiet {
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
        println!("  Dependencies: {}", runtime_requirement_count(pkg));
        if !extraction.language_provides.is_empty() {
            println!(
                "  Provides: {} (language-specific capabilities)",
                extraction.language_provides.len()
            );
        }
    }

    Ok(())
}

pub(super) fn runtime_requirement_count(pkg: &dyn PackageFormat) -> usize {
    pkg.requirements()
        .iter()
        .filter(|requirement| {
            matches!(
                requirement.kind,
                RepositoryRequirementKind::Depends | RepositoryRequirementKind::PreDepends
            )
        })
        .count()
}

pub(super) fn finalize_install(
    conn: &rusqlite::Connection,
    pkg: &dyn PackageFormat,
    extraction: &ExtractionResult,
    root: &str,
    tx_result: &InstallTransactionResult,
    progress: &InstallProgress,
) -> Result<()> {
    finalize_install_without_snapshot(
        conn,
        pkg,
        extraction,
        root,
        tx_result,
        FinalizeInstallOutput::new(progress, false),
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
