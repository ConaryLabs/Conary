// src/commands/model/apply.rs

use std::path::Path;

use crate::commands::replatform_rendering::render_replatform_blocked_reason;
use crate::commands::{InstallOptions, SandboxMode, cmd_install, cmd_remove};
use anyhow::{Context, Result, anyhow};
use conary_core::db::models::{
    DerivedOverride, DerivedPackage, DerivedPatch, DistroPin, Repository, Trove, VersionPolicy,
    settings,
};
use conary_core::derived::{build_from_definition, persist_build_artifact};
use conary_core::filesystem::CasStore;
use conary_core::hash::sha256;
use conary_core::model::parser::SystemModel;
use conary_core::model::{DiffAction, ModelDerivedPackage, replatform_execution_plan};
use conary_core::repository::versioning::{VersionScheme, infer_version_scheme};
use conary_core::repository::{
    SETTINGS_KEY_ALLOWED_DISTROS, SETTINGS_KEY_SELECTION_MODE, resolution_policy::SelectionMode,
};
use rusqlite::Connection;
#[cfg(test)]
use std::cell::Cell;
use tracing::{debug, info};

#[cfg(test)]
thread_local! {
    static REPLATFORM_METADATA_FAILPOINT: Cell<bool> = const { Cell::new(false) };
}

/// Options for `cmd_model_apply`, replacing the former 8-argument signature.
pub struct ApplyOptions<'a> {
    pub model_path: &'a str,
    pub db_path: &'a str,
    #[allow(dead_code)] // reserved for future chroot support
    pub root: &'a str,
    pub dry_run: bool,
    pub skip_optional: bool,
    pub strict: bool,
    pub autoremove: bool,
    pub offline: bool,
}

/// Apply source-policy actions from the filtered action list.
///
/// Returns the number of changes applied.
pub(super) fn apply_source_policy_changes(
    conn: &Connection,
    actions: &[&DiffAction],
) -> Result<usize> {
    let mut count = 0;
    for action in actions {
        match action {
            DiffAction::SetSourcePin { distro, strength } => {
                let strength = strength.as_deref().unwrap_or("guarded");
                DistroPin::set(conn, distro, strength)?;
                println!("Updated source policy pin: {} ({})", distro, strength);
                count += 1;
            }
            DiffAction::ClearSourcePin => {
                DistroPin::remove(conn)?;
                println!("Cleared source policy pin");
                count += 1;
            }
            DiffAction::SetSelectionMode { mode } => {
                settings::set(
                    conn,
                    SETTINGS_KEY_SELECTION_MODE,
                    selection_mode_value(*mode),
                )?;
                println!(
                    "Updated source policy selection mode: {}",
                    selection_mode_value(*mode)
                );
                count += 1;
            }
            DiffAction::ClearSelectionMode => {
                settings::delete(conn, SETTINGS_KEY_SELECTION_MODE)?;
                println!("Cleared source policy selection mode");
                count += 1;
            }
            DiffAction::SetAllowedDistros { distros } => {
                settings::set(
                    conn,
                    SETTINGS_KEY_ALLOWED_DISTROS,
                    &serde_json::to_string(distros)?,
                )?;
                println!("Updated allowed source distros: {}", distros.join(", "));
                count += 1;
            }
            DiffAction::ClearAllowedDistros => {
                settings::delete(conn, SETTINGS_KEY_ALLOWED_DISTROS)?;
                println!("Cleared allowed source distros");
                count += 1;
            }
            _ => {}
        }
    }
    Ok(count)
}

fn selection_mode_value(mode: SelectionMode) -> &'static str {
    match mode {
        SelectionMode::Policy => "policy",
        SelectionMode::Latest => "latest",
    }
}

/// Apply executable replatform transactions through the shared install path.
///
/// Returns `(executed, errors)`.
pub(super) async fn apply_replatform_changes(
    db_path: &str,
    root: &str,
    actions: &[&DiffAction],
) -> Result<(usize, Vec<String>)> {
    let conn = rusqlite::Connection::open(db_path)?;
    let owned_actions = actions
        .iter()
        .map(|action| (*action).clone())
        .collect::<Vec<_>>();
    let Some(plan) = replatform_execution_plan(&conn, &owned_actions)? else {
        return Ok((0, Vec::new()));
    };
    drop(conn);

    let mut executed = 0usize;
    let mut errors = Vec::new();

    for transaction in plan.transactions {
        if !transaction.executable {
            let reason = if !transaction.blocked_reasons.is_empty() {
                transaction
                    .blocked_reasons
                    .iter()
                    .map(render_replatform_blocked_reason)
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                transaction
                    .blocked_reason
                    .as_ref()
                    .map(render_replatform_blocked_reason)
                    .unwrap_or("unknown replatform block")
                    .to_string()
            };
            errors.push(format!(
                "Replatform '{}' blocked: {}",
                transaction.package, reason
            ));
            continue;
        }

        let Some(repository) = transaction.install_repository.clone() else {
            errors.push(format!(
                "Replatform '{}' executable plan missing repository metadata",
                transaction.package
            ));
            continue;
        };

        let current_source = {
            let conn = rusqlite::Connection::open(db_path)?;
            find_current_replatform_source(&conn, &transaction)?
                .or_else(|| transaction.current_distro.clone())
                .unwrap_or_else(|| "unknown source".to_string())
        };

        let selection_reason = format!(
            "Replatformed from {} to {} by model apply",
            current_source, transaction.target_distro
        );

        match cmd_install(
            &transaction.package,
            InstallOptions {
                db_path,
                root,
                version: Some(transaction.target_version.clone()),
                repo: Some(repository),
                architecture: transaction.architecture.clone(),
                dry_run: false,
                no_deps: false,
                no_scripts: false,
                selection_reason: Some(selection_reason.as_str()),
                sandbox_mode: SandboxMode::None,
                allow_downgrade: true,
                convert_to_ccs: false,
                no_capture: true,
                force: false,
                dep_mode: None,
                yes: true,
                from_distro: None,
            },
        )
        .await
        {
            Ok(()) => {
                let conn = rusqlite::Connection::open(db_path)?;
                match finalize_replatform_provenance(&conn, &transaction, &selection_reason) {
                    Ok(()) => {
                        println!(
                            "Executed replatform replacement: {} -> {} {}",
                            transaction.package,
                            transaction.target_distro,
                            transaction.target_version
                        );
                        executed += 1;
                    }
                    Err(err) => {
                        let marker = format!("Replatform partial failure after install: {}", err);
                        let failure = format!(
                            "Replatform '{}': failed to finalize replatform metadata: {}",
                            transaction.package, err
                        );
                        if let Err(marker_err) =
                            mark_replatform_partial_failure(&conn, &transaction, &marker)
                        {
                            errors.push(format!(
                                "{failure}; additionally failed to record partial failure state: {marker_err}"
                            ));
                        } else {
                            errors.push(failure);
                        }
                    }
                }
            }
            Err(err) => errors.push(format!("Replatform '{}': {}", transaction.package, err)),
        }
    }

    Ok((executed, errors))
}

fn finalize_replatform_provenance(
    conn: &Connection,
    transaction: &conary_core::model::ReplatformExecutionTransaction,
    selection_reason: &str,
) -> Result<()> {
    maybe_fail_replatform_metadata_for_test()?;
    let repository_name = transaction
        .install_repository
        .as_deref()
        .ok_or_else(|| anyhow!("missing install repository metadata"))?;
    let repository = Repository::find_by_name(conn, repository_name)?
        .ok_or_else(|| anyhow!("missing repository '{repository_name}' for replatform install"))?;
    let repository_id = repository
        .id
        .ok_or_else(|| anyhow!("repository '{repository_name}' missing id"))?;
    let version_scheme = infer_version_scheme(&repository)
        .ok_or_else(|| anyhow!("unable to infer version scheme for '{repository_name}'"))?;
    let installed = find_installed_replatform_trove(conn, transaction)?;
    let installed_id = installed.id.ok_or_else(|| {
        anyhow!(
            "installed replatform trove '{}' missing id",
            transaction.package
        )
    })?;

    Trove::update_replatform_metadata(
        conn,
        installed_id,
        &transaction.target_distro,
        version_scheme_to_str(version_scheme),
        repository_id,
        selection_reason,
    )?;

    Ok(())
}

fn find_installed_replatform_trove(
    conn: &Connection,
    transaction: &conary_core::model::ReplatformExecutionTransaction,
) -> Result<Trove> {
    let matches = Trove::find_by_name(conn, &transaction.package)?
        .into_iter()
        .filter(|trove| trove.version == transaction.target_version)
        .collect::<Vec<_>>();

    if let Some(expected_arch) = transaction.architecture.as_deref()
        && let Some(installed) = matches.iter().find(|trove| {
            trove.architecture.as_deref() == Some(expected_arch)
                || trove.architecture.as_deref().is_none()
        })
    {
        return Ok(installed.clone());
    }

    match matches.as_slice() {
        [installed] => Ok(installed.clone()),
        [] => Err(anyhow!(
            "installed replatform trove '{}' not found",
            transaction.package
        )),
        _ => Err(anyhow!(
            "installed replatform trove '{}' is ambiguous after install",
            transaction.package
        )),
    }
}

fn find_current_replatform_source(
    conn: &Connection,
    transaction: &conary_core::model::ReplatformExecutionTransaction,
) -> Result<Option<String>> {
    let matches = Trove::find_by_name(conn, &transaction.package)?
        .into_iter()
        .filter(|trove| trove.version == transaction.current_version)
        .collect::<Vec<_>>();

    if let Some(expected_arch) = transaction.current_architecture.as_deref()
        && let Some(installed) = matches.iter().find(|trove| {
            trove.architecture.as_deref() == Some(expected_arch)
                || trove.architecture.as_deref().is_none()
        })
    {
        return Ok(installed.source_distro.clone());
    }

    Ok(matches
        .into_iter()
        .next()
        .and_then(|trove| trove.source_distro))
}

fn version_scheme_to_str(scheme: VersionScheme) -> &'static str {
    match scheme {
        VersionScheme::Rpm => "rpm",
        VersionScheme::Debian => "debian",
        VersionScheme::Arch => "arch",
    }
}

fn mark_replatform_partial_failure(
    conn: &Connection,
    transaction: &conary_core::model::ReplatformExecutionTransaction,
    selection_reason: &str,
) -> Result<()> {
    let installed = find_installed_replatform_trove(conn, transaction)?;
    let installed_id = installed.id.ok_or_else(|| {
        anyhow!(
            "installed replatform trove '{}' missing id",
            transaction.package
        )
    })?;
    Trove::update_selection_reason(conn, installed_id, selection_reason)?;
    Ok(())
}

#[cfg(test)]
pub(super) fn set_replatform_metadata_failpoint_for_test(enabled: bool) {
    REPLATFORM_METADATA_FAILPOINT.with(|failpoint| failpoint.set(enabled));
}

#[cfg(test)]
fn maybe_fail_replatform_metadata_for_test() -> Result<()> {
    if REPLATFORM_METADATA_FAILPOINT.with(Cell::get) {
        return Err(anyhow!("injected replatform metadata failure"));
    }
    Ok(())
}

#[cfg(not(test))]
fn maybe_fail_replatform_metadata_for_test() -> Result<()> {
    Ok(())
}

/// Apply package install/remove actions from the model diff.
///
/// Returns `(applied_count, error_list)`.
pub(super) async fn apply_package_changes(
    db_path: &str,
    root: &str,
    actions: &[&DiffAction],
    strict: bool,
) -> Result<(usize, Vec<String>)> {
    let mut applied = 0usize;
    let mut errors = Vec::new();

    for action in actions {
        if let DiffAction::Remove {
            package,
            current_version,
            architectures,
        } = action
        {
            let iterations = architectures.len().max(1);
            for arch in architectures
                .iter()
                .map(Some)
                .chain(std::iter::once(None))
                .take(iterations)
            {
                match arch {
                    Some(arch) => {
                        println!("Removing {} {} [{}]...", package, current_version, arch)
                    }
                    None => println!("Removing {} {}...", package, current_version),
                }

                match cmd_remove(
                    package,
                    db_path,
                    root,
                    Some(current_version.clone()),
                    false,
                    SandboxMode::Always,
                    false,
                )
                .await
                {
                    Ok(()) => {
                        println!("  Removed {}", package);
                        applied += 1;
                    }
                    Err(e) => {
                        let msg = match arch {
                            Some(arch) => {
                                format!(
                                    "Remove '{}' {} [{}]: {}",
                                    package, current_version, arch, e
                                )
                            }
                            None => format!("Remove '{}' {}: {}", package, current_version, e),
                        };
                        eprintln!("  [FAILED] {}", msg);
                        if strict {
                            anyhow::bail!(msg);
                        }
                        errors.push(msg);
                    }
                }
            }
        }
    }

    for action in actions {
        match action {
            DiffAction::Install { package, pin, .. } => {
                println!(
                    "Installing {}{}...",
                    package,
                    display_pin_suffix(pin.as_deref())
                );
                match cmd_install(
                    package,
                    InstallOptions {
                        db_path,
                        root,
                        version: pin.clone(),
                        repo: None,
                        architecture: None,
                        dry_run: false,
                        no_deps: false,
                        no_scripts: false,
                        selection_reason: Some("Installed by model apply"),
                        sandbox_mode: SandboxMode::Always,
                        allow_downgrade: false,
                        convert_to_ccs: false,
                        no_capture: false,
                        force: false,
                        dep_mode: None,
                        yes: true,
                        from_distro: None,
                    },
                )
                .await
                {
                    Ok(()) => {
                        println!("  Installed {}", package);
                        applied += 1;
                    }
                    Err(e) => {
                        let msg = format!("Install '{}': {}", package, e);
                        eprintln!("  [FAILED] {}", msg);
                        if strict {
                            anyhow::bail!(msg);
                        }
                        errors.push(msg);
                    }
                }
            }
            DiffAction::Update {
                package,
                current_version,
                target_version,
            } => {
                println!(
                    "Updating {} from {} to {}...",
                    package, current_version, target_version
                );
                match cmd_install(
                    package,
                    InstallOptions {
                        db_path,
                        root,
                        version: Some(target_version.clone()),
                        repo: None,
                        architecture: None,
                        dry_run: false,
                        no_deps: false,
                        no_scripts: false,
                        selection_reason: Some("Updated by model apply"),
                        sandbox_mode: SandboxMode::Always,
                        allow_downgrade: true,
                        convert_to_ccs: false,
                        no_capture: false,
                        force: false,
                        dep_mode: None,
                        yes: true,
                        from_distro: None,
                    },
                )
                .await
                {
                    Ok(()) => {
                        println!("  Updated {} to {}", package, target_version);
                        applied += 1;
                    }
                    Err(e) => {
                        let msg = format!(
                            "Update '{}' {} -> {}: {}",
                            package, current_version, target_version, e
                        );
                        eprintln!("  [FAILED] {}", msg);
                        if strict {
                            anyhow::bail!(msg);
                        }
                        errors.push(msg);
                    }
                }
            }
            _ => {}
        }
    }

    Ok((applied, errors))
}

fn display_pin_suffix(pin: Option<&str>) -> String {
    pin.map(|pin| format!(" ({})", pin)).unwrap_or_default()
}

/// Apply derived-package build/rebuild actions.
///
/// Returns `(built, rebuilt, errors)`.
pub(super) fn apply_derived_packages(
    conn: &Connection,
    actions: &[&DiffAction],
    model: &SystemModel,
    model_dir: &Path,
    cas: &CasStore,
) -> (usize, usize, Vec<String>) {
    let mut derived_built = 0usize;
    let mut derived_rebuilt = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for action in actions {
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

                let model_def = model.derive.iter().find(|d| d.name == *name);

                if let Some(def) = model_def {
                    match create_derived_from_model(conn, def, model_dir, cas) {
                        Ok(_id) => match build_derived_package(conn, name, cas) {
                            Ok(()) => derived_built += 1,
                            Err(e) => errors.push(format!("Build '{}': {}", name, e)),
                        },
                        Err(e) => errors.push(format!("Create definition '{}': {}", name, e)),
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

                match build_derived_package(conn, name, cas) {
                    Ok(()) => derived_rebuilt += 1,
                    Err(e) => errors.push(format!("Rebuild '{}': {}", name, e)),
                }
            }
            _ => {}
        }
    }

    (derived_built, derived_rebuilt, errors)
}

/// Apply package metadata changes: pin/unpin, mark explicit/dependency, update.
///
/// Returns the number of changes applied and any errors encountered.
pub(super) fn apply_metadata_changes(
    conn: &Connection,
    actions: &[&DiffAction],
) -> (usize, Vec<String>) {
    let mut applied = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for action in actions {
        match action {
            DiffAction::Pin { package, pattern } => match Trove::find_one_by_name(conn, package) {
                Ok(Some(trove)) => {
                    if let Some(id) = trove.id {
                        if let Err(e) = Trove::pin(conn, id) {
                            errors.push(format!("Pin '{}': {}", package, e));
                        } else {
                            println!("Pinned '{}' to pattern '{}'", package, pattern);
                            applied += 1;
                        }
                    }
                }
                Ok(None) => {
                    errors.push(format!("Pin '{}': package not installed", package));
                }
                Err(e) => errors.push(format!("Pin '{}': {}", package, e)),
            },
            DiffAction::Unpin { package } => match Trove::find_one_by_name(conn, package) {
                Ok(Some(trove)) => {
                    if let Some(id) = trove.id {
                        if let Err(e) = Trove::unpin(conn, id) {
                            errors.push(format!("Unpin '{}': {}", package, e));
                        } else {
                            println!("Unpinned '{}'", package);
                            applied += 1;
                        }
                    }
                }
                Ok(None) => {
                    errors.push(format!("Unpin '{}': package not installed", package));
                }
                Err(e) => errors.push(format!("Unpin '{}': {}", package, e)),
            },
            DiffAction::MarkExplicit { package } => {
                match Trove::promote_to_explicit(
                    conn,
                    package,
                    Some("Marked explicit by model apply"),
                ) {
                    Ok(true) => {
                        println!("Marked '{}' as explicitly installed", package);
                        applied += 1;
                    }
                    Ok(false) => {
                        debug!("'{}' already explicit or not found", package);
                    }
                    Err(e) => errors.push(format!("MarkExplicit '{}': {}", package, e)),
                }
            }
            DiffAction::MarkDependency { package } => {
                match conn.execute(
                    "UPDATE troves SET install_reason = 'dependency' \
                     WHERE name = ?1 AND install_reason = 'explicit' AND type = 'package'",
                    rusqlite::params![package],
                ) {
                    Ok(rows) if rows > 0 => {
                        println!("Marked '{}' as dependency", package);
                        applied += 1;
                    }
                    Ok(_) => {
                        debug!("'{}' already a dependency or not found", package);
                    }
                    Err(e) => errors.push(format!("MarkDependency '{}': {}", package, e)),
                }
            }
            _ => {}
        }
    }

    (applied, errors)
}

fn create_derived_from_model(
    conn: &Connection,
    model_derived: &ModelDerivedPackage,
    model_dir: &Path,
    cas: &CasStore,
) -> Result<i64> {
    if let Some(existing) = DerivedPackage::find_by_name(conn, &model_derived.name)? {
        info!(
            "Derived package '{}' already exists, updating",
            model_derived.name
        );
        return existing.id.ok_or_else(|| {
            anyhow!(
                "Derived package '{}' exists but has no database id",
                model_derived.name
            )
        });
    }

    let version_policy = if model_derived.version == "inherit" {
        VersionPolicy::Inherit
    } else if model_derived.version.starts_with('+') {
        VersionPolicy::Suffix(model_derived.version.clone())
    } else {
        VersionPolicy::Specific(model_derived.version.clone())
    };

    let mut derived = DerivedPackage::new(model_derived.name.clone(), model_derived.from.clone());
    derived.version_policy = version_policy;
    derived.model_source = Some(model_dir.display().to_string());

    let derived_id = derived.insert(conn)?;
    info!(
        "Created derived package '{}' with id={}",
        model_derived.name, derived_id
    );

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
        cas.store(&patch_content)?;
    }

    for (target_path, source_path) in &model_derived.override_files {
        if source_path.is_empty() || source_path == "REMOVE" {
            let mut ov = DerivedOverride::new_remove(derived_id, target_path.clone());
            ov.insert(conn)?;
        } else {
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
            cas.store(&content)?;
        }
    }

    Ok(derived_id)
}

fn build_derived_package(conn: &Connection, name: &str, cas: &CasStore) -> Result<()> {
    let mut derived = DerivedPackage::find_by_name(conn, name)?
        .ok_or_else(|| anyhow!("Derived package '{}' not found", name))?;

    match build_from_definition(conn, &derived, cas) {
        Ok(build_result) => {
            let build_meta = persist_build_artifact(conn, &mut derived, &build_result, cas)?;
            println!(
                "  Built '{}': {} files, {} patches applied ({})",
                name,
                build_result.files.len(),
                build_result.patches_applied.len(),
                build_meta.artifact_path
            );
            Ok(())
        }
        Err(e) => {
            let error_msg = e.to_string();
            derived.mark_error(conn, &error_msg)?;
            Err(anyhow!("Build failed for '{}': {}", name, error_msg))
        }
    }
}
