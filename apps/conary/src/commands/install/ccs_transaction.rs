// apps/conary/src/commands/install/ccs_transaction.rs
//! Direct CCS package transaction install adapter.
//!
//! This module owns the direct CCS transaction entry point and CCS-specific
//! manifest selection, hook-status, and capability-gate helpers. Shared install
//! transaction mechanics stay in `install/mod.rs`.

use super::ccs_removal_hooks::CcsRemovalHookPlan;
use super::native_events::{NativeInstallInput, PreparedNativeTransaction};
use super::{
    ExtractionResult, FinalizeInstallOutput, InstallIntent, InstallPhase, InstallProgress,
    InstallSemantics, RepositoryInstallProvenance, TransactionContext, UpgradeCheck,
    build_execution_mode, check_upgrade_status,
    execute_install_transaction_in_selected_root_with_post_graph,
    finalize_install_without_snapshot, preflight_extracted_file_ownership,
    runtime_requirement_count,
};
use anyhow::{Context, Result};
use conary_core::ccs::native_lifecycle::SourceFormat;
use conary_core::components::ComponentType;
use conary_core::packages::PackageFormat;
use conary_core::scriptlet::SandboxMode;
use std::collections::HashMap;
use std::path::Path;
use tracing::info;

pub(crate) struct CcsTransactionInstallOptions<'a> {
    pub db_path: &'a str,
    pub root: &'a str,
    pub dry_run: bool,
    pub defer_generation: bool,
    pub quiet: bool,
    pub sandbox_mode: SandboxMode,
    pub allow_downgrade: bool,
    pub intent: InstallIntent,
    pub reinstall: bool,
    pub selection_reason: Option<&'a str>,
    pub selected_manifest_components: Option<Vec<String>>,
    pub repository_provenance: Option<RepositoryInstallProvenance>,
}

pub(crate) struct CcsTransactionInstallResult {
    pub trove_id: Option<i64>,
    pub changeset_id: i64,
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

    let payload_files: Vec<_> = if selected_paths.is_empty() {
        Vec::new()
    } else {
        pkg.package_payload()?
            .into_files()
            .into_iter()
            .filter(|file| selected_paths.contains(file.path.as_str()))
            .collect()
    };
    if payload_files.is_empty() && !selected_entries.is_empty() {
        anyhow::bail!(
            "No files matched the selected CCS components: {}",
            selected_component_names.join(", ")
        );
    }

    let extracted_files =
        crate::commands::ccs::normalize_ccs_payload_files(root_path, payload_files)?;

    let mut component_names_by_path = HashMap::new();
    let mut classified: HashMap<ComponentType, Vec<String>> = HashMap::new();
    for file in &selected_entries {
        let normalized_path =
            crate::commands::ccs::normalize_ccs_package_path(root_path, file.path.as_str())?;
        component_names_by_path
            .entry(normalized_path.clone())
            .or_insert_with(|| file.component.clone());
        if let Some(component_type) = ComponentType::parse(&file.component) {
            classified
                .entry(component_type)
                .or_default()
                .push(normalized_path);
        }
    }

    let mut installed_component_names: Vec<String> = selected_entries
        .iter()
        .map(|file| file.component.clone())
        .collect();
    installed_component_names.sort();
    installed_component_names.dedup();
    let installed_component_types = installed_component_names
        .iter()
        .filter_map(|name| ComponentType::parse(name))
        .collect();

    Ok(ExtractionResult {
        extracted_files,
        classified,
        component_names_by_path: Some(component_names_by_path),
        installed_component_names: Some(installed_component_names),
        ccs_remove_hook: None,
        installed_component_types,
        skipped_components: Vec::new(),
        language_provides: Vec::new(),
    })
}

fn show_ccs_dry_run_summary(pkg: &conary_core::ccs::CcsPackage, extraction: &ExtractionResult) {
    let component_names = extraction
        .installed_component_names
        .as_deref()
        .unwrap_or_default();
    println!(
        "\nWould install package: {} version {}",
        pkg.name(),
        pkg.version()
    );
    println!("  Architecture: {}", pkg.architecture().unwrap_or("none"));
    println!(
        "  Components to install: {} ({} files)",
        if component_names.is_empty() {
            "(metadata-only)".to_string()
        } else {
            component_names.join(", ")
        },
        extraction.extracted_files.len()
    );
    println!("  Dependencies: {}", runtime_requirement_count(pkg));
    println!("\nDry run complete. No changes made.");
}

pub(crate) fn check_ccs_upgrade_status(
    conn: &rusqlite::Connection,
    pkg: &conary_core::ccs::CcsPackage,
    semantics: &InstallSemantics,
    allow_downgrade: bool,
    intent: InstallIntent,
    reinstall: bool,
) -> Result<UpgradeCheck> {
    let existing = conary_core::db::models::Trove::find_by_name(conn, pkg.name())?;

    for trove in &existing {
        if trove.architecture == pkg.architecture().map(|s: &str| s.to_string())
            && trove.version == pkg.version()
            && trove.package_release.as_deref() == pkg.package_release()
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

    check_upgrade_status(conn, pkg, semantics, allow_downgrade, intent)
}

/// Recover the package-manager semantics carried through a converted CCS
/// archive. CCS is a transport here; it must not erase the source package
/// manager's version, config, or directory-materialization contract.
pub(crate) fn install_semantics_for_ccs_manifest(
    manifest: &conary_core::ccs::manifest::CcsManifest,
) -> Result<InstallSemantics> {
    let Some(native) = manifest.native_lifecycle.as_ref() else {
        return Ok(InstallSemantics::ccs(manifest.package.version_scheme));
    };
    native
        .validate()
        .context("converted CCS native lifecycle contract is invalid")?;
    let format = match native.source_format {
        SourceFormat::Rpm => super::PackageFormatType::Rpm,
        SourceFormat::Deb => super::PackageFormatType::Deb,
        SourceFormat::Arch => super::PackageFormatType::Arch,
    };
    let semantics = InstallSemantics::native_package(format);
    if manifest.package.version_scheme != semantics.version_scheme {
        anyhow::bail!(
            "converted CCS source format '{}' requires package version scheme '{}', got '{}'",
            native.source_format.as_str(),
            semantics.version_scheme.as_str(),
            manifest.package.version_scheme.as_str()
        );
    }
    Ok(semantics)
}

pub(super) fn ccs_has_pre_hooks(hooks: &conary_core::ccs::manifest::Hooks) -> bool {
    !hooks.users.is_empty() || !hooks.groups.is_empty() || !hooks.directories.is_empty()
}

pub(super) fn ccs_has_post_hooks(hooks: &conary_core::ccs::manifest::Hooks) -> bool {
    !hooks.services.is_empty()
        || !hooks.systemd.is_empty()
        || !hooks.tmpfiles.is_empty()
        || !hooks.sysctl.is_empty()
        || !hooks.alternatives.is_empty()
        || hooks.post_install.is_some()
}

pub(super) fn ccs_requires_host_capability_inventory(
    hooks: &conary_core::ccs::manifest::Hooks,
) -> bool {
    !hooks.services.is_empty()
        || !hooks.systemd.is_empty()
        || !hooks.tmpfiles.is_empty()
        || !hooks.sysctl.is_empty()
}

fn ccs_lifecycle_dry_run_entries(
    manifest: &conary_core::ccs::manifest::CcsManifest,
) -> Vec<String> {
    use conary_core::ccs::manifest::ServiceAction;

    let hooks = &manifest.hooks;
    let mut entries = Vec::new();
    entries.extend(
        hooks
            .groups
            .iter()
            .map(|hook| format!("group:{}", hook.name)),
    );
    entries.extend(hooks.users.iter().map(|hook| format!("user:{}", hook.name)));
    entries.extend(
        hooks
            .directories
            .iter()
            .map(|hook| format!("directory:{}", hook.path)),
    );
    entries.extend(hooks.services.iter().map(|hook| {
        let action = match &hook.action {
            ServiceAction::Enable => "enable",
            ServiceAction::Disable => "disable",
            ServiceAction::Start => "start",
            ServiceAction::Stop => "stop",
            ServiceAction::Reload => "reload",
            ServiceAction::Restart => "restart",
            ServiceAction::TryRestart => "try-restart",
            ServiceAction::ReloadOrRestart => "reload-or-restart",
            ServiceAction::ReloadOrTryRestart => "reload-or-try-restart",
        };
        format!("service:{}:{action}", hook.name)
    }));
    entries.extend(hooks.systemd.iter().map(|hook| {
        format!(
            "systemd:{}:{}",
            hook.unit,
            if hook.enable { "enable" } else { "disable" }
        )
    }));
    entries.extend(
        hooks
            .tmpfiles
            .iter()
            .map(|hook| format!("tmpfiles:{}:{}", hook.entry_type, hook.path)),
    );
    entries.extend(
        hooks
            .sysctl
            .iter()
            .map(|hook| format!("sysctl:{}", hook.key)),
    );
    entries.extend(
        hooks
            .alternatives
            .iter()
            .map(|hook| format!("alternative:{}:{}", hook.name, hook.path)),
    );
    if hooks.post_install.is_some() {
        entries.push("post-install:/bin/sh:sandboxed-target-root:planned".to_string());
    }
    if hooks.pre_remove.is_some() {
        entries.push("pre-remove:/bin/sh:sandboxed-target-root:persisted".to_string());
    }
    entries.extend(
        manifest
            .scriptlets
            .capabilities
            .iter()
            .map(|capability| format!("script-capability:{}", capability.name)),
    );
    entries
}

fn show_ccs_lifecycle_dry_run(manifest: &conary_core::ccs::manifest::CcsManifest) {
    let entries = ccs_lifecycle_dry_run_entries(manifest);
    let summary = if entries.is_empty() {
        "none".to_string()
    } else {
        entries.join(", ")
    };
    crate::ui::field("Lifecycle", &summary);
}

pub(super) fn enforce_ccs_scriptlet_capability_gate(
    pkg: &conary_core::ccs::CcsPackage,
) -> Result<()> {
    if !pkg.manifest().scriptlets.has_capability_declarations() {
        return Ok(());
    }

    anyhow::bail!(
        "scriptlet capability declarations are present but enforcement is not available; \
         enable supported capability enforcement or run inside a VM."
    );
}

pub(crate) fn install_ccs_package_transactionally(
    conn: &mut rusqlite::Connection,
    pkg: &conary_core::ccs::CcsPackage,
    opts: CcsTransactionInstallOptions<'_>,
) -> Result<CcsTransactionInstallResult> {
    install_ccs_package_transactionally_inner(conn, pkg, opts, None)
}

pub(crate) fn install_ccs_package_transactionally_in_selected_root(
    conn: &mut rusqlite::Connection,
    pkg: &conary_core::ccs::CcsPackage,
    opts: CcsTransactionInstallOptions<'_>,
    selected_root: &mut crate::commands::generation::selected_root::SelectedRootSession,
) -> Result<CcsTransactionInstallResult> {
    install_ccs_package_transactionally_inner(conn, pkg, opts, Some(selected_root))
}

fn install_ccs_package_transactionally_inner(
    conn: &mut rusqlite::Connection,
    pkg: &conary_core::ccs::CcsPackage,
    opts: CcsTransactionInstallOptions<'_>,
    selected_root: Option<&mut crate::commands::generation::selected_root::SelectedRootSession>,
) -> Result<CcsTransactionInstallResult> {
    let caller_owned_selected_root = selected_root.is_some();
    let progress = InstallProgress::single("Installing");
    let semantics = install_semantics_for_ccs_manifest(pkg.manifest())?;
    enforce_ccs_scriptlet_capability_gate(pkg)?;
    // A caller-owned selected root already owns the runtime mutation lock.
    // Standalone real installs acquire it before resolving upgrade authority;
    // dry runs remain read-only and do not serialize with mutations.
    let locked_root = if !opts.dry_run && !caller_owned_selected_root {
        Some(crate::commands::generation::selected_root::LockedRuntimeRoot::acquire(opts.db_path)?)
    } else {
        None
    };
    let upgrade = check_ccs_upgrade_status(
        conn,
        pkg,
        &semantics,
        opts.allow_downgrade,
        opts.intent,
        opts.reinstall,
    )?;
    let old_trove = match &upgrade {
        UpgradeCheck::FreshInstall => None,
        UpgradeCheck::AlreadyInstalled(trove) => {
            anyhow::bail!(
                "Package {} version {} ({}) is already installed",
                trove.name,
                trove.version,
                trove.architecture.as_deref().unwrap_or("no-arch")
            )
        }
        UpgradeCheck::Upgrade(trove)
        | UpgradeCheck::Downgrade(trove)
        | UpgradeCheck::Replatform(trove) => Some(trove.as_ref()),
    };
    // Dry-run remains filesystem-read-only. Every real CCS mutation receives
    // either its caller-owned try root or a freshly materialized selected root.
    let mut owned_selected_root = locked_root
        .map(|locked_root| {
            locked_root.materialize(conn, format!("Install {}-{}", pkg.name(), pkg.version()))
        })
        .transpose()?;
    let selected_root = match selected_root {
        Some(selected_root) => Some(selected_root),
        None => owned_selected_root.as_mut(),
    };
    let transaction_root = selected_root.as_deref().map_or_else(
        || opts.root.to_string(),
        |session| session.selected_root().to_string_lossy().into_owned(),
    );

    let mut extraction =
        if let Some(selected_manifest_components) = opts.selected_manifest_components.as_deref() {
            extract_and_classify_ccs_manifest_files(
                pkg,
                selected_manifest_components,
                Path::new(&transaction_root),
                &progress,
            )?
        } else {
            let mut selected_manifest_components: Vec<String> =
                pkg.components().keys().cloned().collect();
            selected_manifest_components.sort();
            extract_and_classify_ccs_manifest_files(
                pkg,
                &selected_manifest_components,
                Path::new(&transaction_root),
                &progress,
            )?
        };
    extraction.ccs_remove_hook = pkg.manifest().hooks.pre_remove.clone();

    let relation_plan =
        conary_core::transaction::plan_package_relations(conn, pkg, semantics.version_scheme)
            .context("Failed to plan CCS package conflicts and replacements")?;
    conary_core::transaction::validate_package_relation_plan(conn, &relation_plan)
        .context("CCS package conflicts and replacements cannot be applied")?;
    let native_lifecycle_bundle = pkg.manifest().native_lifecycle.as_ref();
    let resolution_capabilities = pkg.resolution_capabilities()?;
    let native_transaction = PreparedNativeTransaction::prepare_install(
        conn,
        NativeInstallInput {
            package_name: pkg.name(),
            package_version: pkg.version(),
            package_arch: pkg.architecture(),
            version_scheme: semantics.version_scheme,
            provides: &resolution_capabilities,
            new_bundle: native_lifecycle_bundle,
            old_trove,
            relation_removals: &relation_plan.removals,
            relation_deconfigurations: &relation_plan.deconfigurations,
            paths: extraction
                .extracted_files
                .iter()
                .map(|file| file.path.clone())
                .collect(),
        },
    )?;
    let hooks = &pkg.manifest().hooks;
    // CCS hooks are package-scoped authority. Component names do not infer
    // lifecycle ownership, so every package install runs all declared hooks.
    let host_capabilities = if ccs_requires_host_capability_inventory(hooks) {
        conary_core::ccs::HostCapabilityInventory::load_required(conn)
            .context("CCS lifecycle host capability preflight failed")?
    } else {
        conary_core::ccs::HostCapabilityInventory::default()
    };
    let mut hook_executor =
        conary_core::ccs::HookExecutor::new(Path::new(&transaction_root), host_capabilities);
    if ccs_has_pre_hooks(hooks) || ccs_has_post_hooks(hooks) {
        hook_executor
            .preflight_hooks(hooks)
            .context("CCS lifecycle host capability preflight failed")?;
    }

    if opts.dry_run {
        show_ccs_lifecycle_dry_run(pkg.manifest());
        show_ccs_dry_run_summary(pkg, &extraction);
        for removal in &relation_plan.removals {
            println!(
                "  Would remove {} {} ({})",
                removal.package_name,
                removal.package_version,
                removal.kind.as_str()
            );
        }
        return Ok(CcsTransactionInstallResult {
            trove_id: None,
            changeset_id: 0,
        });
    }

    let ccs_removal_hook_plan =
        CcsRemovalHookPlan::prepare(conn, old_trove, relation_plan.removals.iter())?;

    let selected_component_names =
        if let Some(selected) = opts.selected_manifest_components.as_ref() {
            selected.clone()
        } else {
            let mut names: Vec<String> = pkg.components().keys().cloned().collect();
            names.sort();
            names
        };
    crate::commands::ccs::validate_ccs_payload_paths(
        Path::new(&transaction_root),
        pkg,
        &selected_component_names,
    )?;
    preflight_extracted_file_ownership(conn, pkg, &extraction, &relation_plan.removals)?;
    let normalized_file_capabilities = crate::commands::ccs::normalize_ccs_file_capabilities(
        Path::new(&transaction_root),
        &pkg.manifest().file_capabilities,
    )?;
    let repository_enrollments = pkg
        .v3_authority()
        .map(|authority| authority.lifecycle.repository_enrollments.as_slice())
        .unwrap_or_default();
    conary_core::repository::enrollment::validate_payload_bindings(
        repository_enrollments,
        &extraction.extracted_files,
    )?;
    let tx_ctx = TransactionContext {
        db_path: opts.db_path,
        root: &transaction_root,
        semantics,
        selection_reason: opts.selection_reason,
        old_trove_to_upgrade: old_trove,
        ccs_capabilities: pkg.manifest().capabilities.as_ref(),
        ccs_file_capabilities: Some(&normalized_file_capabilities),
        // Deferral is an ownership contract for try sessions: that caller
        // captures the exact root after this transaction. Normal installs
        // always persist and publish their selected-root authority here.
        defer_generation: opts.defer_generation && caller_owned_selected_root,
        repository_provenance: opts.repository_provenance,
        native_lifecycle_bundle,
        repository_enrollments,
        relation_removals: &relation_plan.removals,
        relation_deconfigurations: &relation_plan.deconfigurations,
        retain_replaced_payload_until_lifecycle: native_transaction
            .requires_upgrade_payload_boundary(),
    };
    super::preflight_generation_file_capabilities_for_install(&tx_ctx, &extraction)?;

    let native_execution_mode = build_execution_mode(old_trove.map(|trove| trove.version.as_str()));
    native_transaction.preflight(Path::new(&transaction_root), &native_execution_mode)?;
    let preflighted_ccs_removal_hooks =
        ccs_removal_hook_plan.preflight(Path::new(&transaction_root), opts.sandbox_mode)?;
    preflighted_ccs_removal_hooks.execute()?;
    if ccs_has_pre_hooks(hooks) {
        info!("Executing CCS pre-install hooks");
        hook_executor
            .execute_pre_hooks(hooks)
            .context("CCS pre-install hook failed")?;
    }

    let selected_root =
        selected_root.context("real CCS install has no selected-root transaction")?;
    let tx_result = execute_install_transaction_in_selected_root_with_post_graph(
        conn,
        pkg,
        &extraction,
        &tx_ctx,
        &progress,
        selected_root,
        &native_transaction,
        &native_execution_mode,
        || {
            if ccs_has_post_hooks(hooks) {
                execute_ccs_post_hooks(pkg.name(), pkg.version(), hooks, &hook_executor)?;
            }
            Ok(ccs_activation_requests(
                pkg.name(),
                pkg.version(),
                &hook_executor,
            ))
        },
    )?;
    finalize_install_without_snapshot(
        conn,
        pkg,
        &extraction,
        opts.root,
        &tx_result,
        FinalizeInstallOutput::new(&progress, opts.quiet),
    )?;
    Ok(CcsTransactionInstallResult {
        trove_id: Some(tx_result.trove_id),
        changeset_id: tx_result.changeset_id,
    })
}

pub(super) fn execute_ccs_post_hooks(
    package_name: &str,
    package_version: &str,
    hooks: &conary_core::ccs::manifest::Hooks,
    hook_executor: &conary_core::ccs::HookExecutor,
) -> Result<()> {
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
        anyhow::bail!(
            "post-install hooks failed for {} {}: {}",
            package_name,
            package_version,
            failures.join("; ")
        );
    }
    Ok(())
}

pub(super) fn ccs_activation_requests(
    package_name: &str,
    package_version: &str,
    hook_executor: &conary_core::ccs::HookExecutor,
) -> Vec<conary_core::db::models::NewActivationRequest> {
    hook_executor
        .take_activation_invocations()
        .into_iter()
        .map(|captured| conary_core::db::models::NewActivationRequest {
            source_kind: match captured.source {
                conary_core::ccs::CcsActivationSource::DeclarativeService => {
                    conary_core::db::models::ActivationRequestSourceKind::CcsService
                }
                conary_core::ccs::CcsActivationSource::CapturedRuntime => {
                    conary_core::db::models::ActivationRequestSourceKind::captured_for(
                        &captured.invocation,
                    )
                }
            },
            source_package: package_name.to_string(),
            source_version: package_version.to_string(),
            source_entry: captured.source_entry,
            invocation: captured.invocation,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use conary_core::ccs::manifest::{
        CcsManifest, DirectoryHook, ScriptHook, Service, ServiceAction, SystemdHook,
    };
    use conary_core::ccs::native_lifecycle::{
        NATIVE_LIFECYCLE_SCHEMA_REVISION, NATIVE_LIFECYCLE_SCHEMA_V1, NativeLifecycleBundle,
        ScriptletFidelity, SourceFormat, VersionScheme as NativeVersionScheme,
    };
    use conary_core::repository::dependency_model::DebianMultiArch;
    use conary_core::repository::versioning::VersionScheme;

    fn native_free_bundle(format: SourceFormat, version: &str) -> NativeLifecycleBundle {
        NativeLifecycleBundle {
            schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
            schema_revision: NATIVE_LIFECYCLE_SCHEMA_REVISION,
            source_format: format,
            source_family: format.as_str().to_string(),
            source_profile: None,
            source_release: None,
            source_arch: Some("x86_64".to_string()),
            source_package: format!("{}-transport", format.as_str()),
            source_version: version.to_string(),
            source_checksum: Some(format!("sha256:{}", "1".repeat(64))),
            version_scheme: match format {
                SourceFormat::Rpm => NativeVersionScheme::Rpm,
                SourceFormat::Deb => NativeVersionScheme::Deb,
                SourceFormat::Arch => NativeVersionScheme::Arch,
            },
            conversion_tool: "conary-test".to_string(),
            conversion_tool_version: env!("CARGO_PKG_VERSION").to_string(),
            conversion_policy: "typed-source-abi-test".to_string(),
            evidence_digest: Some(format!("sha256:{}", "2".repeat(64))),
            scriptlet_fidelity: ScriptletFidelity::NativeFree,
            entries: Vec::new(),
        }
    }

    fn reloaded_converted_manifest(format: SourceFormat, version: &str) -> CcsManifest {
        let mut manifest =
            CcsManifest::new_minimal(&format!("{}-transport", format.as_str()), version);
        manifest.package.version_scheme = match format {
            SourceFormat::Rpm => VersionScheme::Rpm,
            SourceFormat::Deb => VersionScheme::Debian,
            SourceFormat::Arch => VersionScheme::Arch,
        };
        manifest.package.debian_multi_arch = match format {
            SourceFormat::Deb => Some(DebianMultiArch::No),
            SourceFormat::Rpm | SourceFormat::Arch => None,
        };
        manifest.native_lifecycle = Some(native_free_bundle(format, version));
        let encoded = toml::to_string_pretty(&manifest).unwrap();
        CcsManifest::parse(&encoded).unwrap()
    }

    #[test]
    fn reloaded_converted_ccs_preserves_exact_source_install_semantics() {
        for (format, version, expected) in [
            (
                SourceFormat::Rpm,
                "1.0-1",
                super::super::PackageFormatType::Rpm,
            ),
            (
                SourceFormat::Deb,
                "1.0-1",
                super::super::PackageFormatType::Deb,
            ),
            (
                SourceFormat::Arch,
                "1.0-1",
                super::super::PackageFormatType::Arch,
            ),
        ] {
            let manifest = reloaded_converted_manifest(format, version);
            let semantics = super::install_semantics_for_ccs_manifest(&manifest).unwrap();
            assert_eq!(
                semantics,
                super::super::InstallSemantics::native_package(expected)
            );
        }
    }

    #[test]
    fn converted_ccs_source_format_rejects_a_mismatched_package_version_scheme() {
        let mut manifest = reloaded_converted_manifest(SourceFormat::Deb, "1.0-1");
        manifest.package.version_scheme = VersionScheme::Rpm;

        let error = super::install_semantics_for_ccs_manifest(&manifest).unwrap_err();

        assert!(
            error.to_string().contains(
                "source format 'deb' requires package version scheme 'debian', got 'rpm'"
            ),
            "{error:#}"
        );
    }

    #[test]
    fn author_native_ccs_keeps_ccs_semantics() {
        let manifest = CcsManifest::new_minimal("author-native", "1.0.0");
        assert_eq!(
            super::install_semantics_for_ccs_manifest(&manifest).unwrap(),
            super::super::InstallSemantics::ccs(VersionScheme::Conary)
        );
    }

    #[test]
    fn ccs_dry_run_always_plans_typed_lifecycle_without_inspecting_script_text() {
        let mut manifest = CcsManifest::new_minimal("lifecycle", "1.0.0");
        manifest.hooks.directories.push(DirectoryHook {
            path: "/var/lib/lifecycle".to_string(),
            mode: "0755".to_string(),
            owner: "root".to_string(),
            group: "root".to_string(),
            cleanup: None,
            reversible: Some(true),
        });
        manifest.hooks.services.push(Service {
            name: "lifecycle".to_string(),
            action: ServiceAction::Restart,
            reversible: Some(false),
        });
        manifest.hooks.systemd.push(SystemdHook {
            unit: "lifecycle.service".to_string(),
            enable: true,
            reversible: Some(true),
        });
        manifest.hooks.post_install = Some(ScriptHook {
            script: "this body is deliberately not classified".to_string(),
            reversible: Some(false),
        });

        assert!(super::ccs_has_post_hooks(&manifest.hooks));
        let entries = super::ccs_lifecycle_dry_run_entries(&manifest);

        assert!(entries.contains(&"directory:/var/lib/lifecycle".to_string()));
        assert!(entries.contains(&"service:lifecycle:restart".to_string()));
        assert!(entries.contains(&"systemd:lifecycle.service:enable".to_string()));
        assert!(
            entries.contains(&"post-install:/bin/sh:sandboxed-target-root:planned".to_string())
        );
        assert!(
            entries
                .iter()
                .all(|entry| !entry.contains("deliberately not classified")
                    && !entry.contains("disabled"))
        );
    }

    #[test]
    fn ccs_systemd_false_is_planned_as_disable_and_services_are_post_hooks() {
        let mut manifest = CcsManifest::new_minimal("service-state", "1.0.0");
        manifest.hooks.systemd.push(SystemdHook {
            unit: "disabled.service".to_string(),
            enable: false,
            reversible: Some(true),
        });
        manifest.hooks.services.push(Service {
            name: "enabled.service".to_string(),
            action: ServiceAction::Enable,
            reversible: Some(true),
        });

        assert!(super::ccs_has_post_hooks(&manifest.hooks));
        let entries = super::ccs_lifecycle_dry_run_entries(&manifest);
        assert!(entries.contains(&"systemd:disabled.service:disable".to_string()));
        assert!(entries.contains(&"service:enabled.service:enable".to_string()));
    }
}

#[cfg(test)]
#[path = "ccs_transaction/converted_directory_tests.rs"]
mod converted_directory_tests;

#[cfg(test)]
#[path = "ccs_transaction/mutation_lock_tests.rs"]
mod mutation_lock_tests;
