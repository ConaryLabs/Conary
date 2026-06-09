// src/commands/model/apply.rs

use std::path::Path;

use super::context::load_model_and_diff;
use super::presentation::{
    is_replatform_action, is_source_policy_action, print_source_policy_and_replatform,
    render_replatform_summary, source_policy_replatform_note, source_policy_summary,
};
use crate::commands::replatform_rendering::{
    render_replatform_blocked_reason, render_replatform_execution_plan,
};
use crate::commands::{InstallOptions, LegacyReplayOptions, SandboxMode, cmd_install, cmd_remove};
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
                repository_provenance: None,
                legacy_replay: LegacyReplayOptions::default(),
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
            Err(err) => errors.push(format_replatform_install_error(&transaction.package, err)),
        }
    }

    Ok((executed, errors))
}

fn format_replatform_install_error(package: &str, err: anyhow::Error) -> String {
    let error = err.to_string();
    let guidance = if error.contains("LegacyReplayFeatureDisabled") {
        " Safe choices: select a different target distro or wait for adapter coverage."
    } else {
        ""
    };
    format!("Replatform '{package}': {error}{guidance}")
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
                    arch.cloned(),
                    false,
                    SandboxMode::Always,
                    false,
                    LegacyReplayOptions::default(),
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
                        repository_provenance: None,
                        legacy_replay: LegacyReplayOptions::default(),
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
                        repository_provenance: None,
                        legacy_replay: LegacyReplayOptions::default(),
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

#[cfg(test)]
mod tests {
    use super::super::context::compute_model_diff;
    use super::super::test_support::{
        ReplatformMetadataFailpointReset, build_test_ccs_package,
        build_test_ccs_package_with_bundle, legacy_replatform_upgrade_bundle, serve_test_file,
    };
    use super::*;
    use crate::commands::test_helpers::create_test_db;
    use conary_core::db::models::{DistroPin, settings};
    use conary_core::model::capture_current_state;
    use conary_core::model::parser::SystemModel;
    use conary_core::repository::{SETTINGS_KEY_ALLOWED_DISTROS, SETTINGS_KEY_SELECTION_MODE};
    use tempfile::tempdir;

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

        set_replatform_metadata_failpoint_for_test(true);
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
