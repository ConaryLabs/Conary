// src/commands/model/apply.rs

mod derived;
mod packages;

use std::path::Path;

use super::context::load_model_and_diff;
use super::presentation::{
    is_replatform_action, print_source_policy_and_replatform, render_replatform_summary,
};
use crate::commands::install::cmd_install_replatform;
use crate::commands::replatform_rendering::{
    render_replatform_blocked_reason, render_replatform_execution_plan,
};
use crate::commands::{InstallOptions, SandboxMode};
use anyhow::{Context, Result, anyhow};
use conary_core::db::models::{Repository, RepositoryPackage, Trove, TroveType};
use conary_core::filesystem::CasStore;
use conary_core::model::parser::SystemModel;
use conary_core::model::{DiffAction, replatform_execution_plan};
use derived::{build_derived_package, create_derived_from_model};
use packages::{apply_package_changes, validate_package_changes};
use rusqlite::Connection;
#[cfg(test)]
use std::cell::Cell;
use tracing::debug;

#[cfg(test)]
thread_local! {
    static REPLATFORM_METADATA_FAILPOINT: Cell<bool> = const { Cell::new(false) };
}

/// Options for `cmd_model_apply`, replacing the former 8-argument signature.
pub struct ApplyOptions<'a> {
    pub model_path: &'a str,
    pub db_path: &'a str,
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
            _ => "*",
        };
        println!("  {} {}", prefix, action.description());
    }
    println!();

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
        validate_package_changes(db_path, &actions).await?;
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

    // Phase 1: executable replatform replacements
    let (replatform_executed, replatform_errors) =
        apply_replatform_changes(db_path, root, &actions).await?;

    // Phase 2: package changes (install/remove/update execution)
    let (package_applied, package_errors) =
        apply_package_changes(db_path, root, &actions, strict).await?;

    // Phase 3: derived packages
    let (derived_built, derived_rebuilt, mut errors) =
        apply_derived_packages(&conn, &actions, &model, model_dir, &cas);
    errors.extend(replatform_errors);
    errors.extend(package_errors);

    // Phase 4: metadata changes (pin/unpin, mark explicit/dependency, update)
    let (metadata_applied, metadata_errors) = apply_metadata_changes(&conn, &actions);
    errors.extend(metadata_errors);

    if autoremove {
        println!();
        if let Err(e) =
            crate::commands::cmd_autoremove(db_path, false, crate::commands::SandboxMode::Always)
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
                .or_else(|| transaction.current_source_identity.clone())
                .unwrap_or_else(|| "unknown source".to_string())
        };

        let selection_reason = format!(
            "Replatformed from {} to {} by model apply",
            current_source, transaction.target_source_identity
        );
        let exact_release = {
            let repository_package_id = transaction
                .install_repository_package_id
                .context("executable replatform plan has no exact repository package id")?;
            let conn = rusqlite::Connection::open(db_path)?;
            let package = RepositoryPackage::find_by_id(&conn, repository_package_id)?
                .context("exact replatform repository package row disappeared")?;
            if package.name != transaction.package
                || package.version != transaction.target_version
                || package.architecture != transaction.architecture
            {
                anyhow::bail!("exact replatform repository package row drifted after planning");
            }
            package.package_release
        };

        match cmd_install_replatform(
            &transaction.package,
            InstallOptions {
                db_path,
                root,
                version: Some(transaction.target_version.clone()),
                package_release: Some(exact_release),
                repo: Some(repository),
                architecture: transaction.architecture.clone(),
                dry_run: false,
                no_deps: false,
                selection_reason: Some(selection_reason.as_str()),
                sandbox_mode: SandboxMode::Always,
                allow_downgrade: true,
                convert_to_ccs: false,
                ownership: None,
                yes: true,
                from_profile: None,
                repository_provenance: None,
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
                            transaction.target_source_identity,
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
    format!("Replatform '{package}': {err}")
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
    let installed = find_installed_replatform_trove(conn, transaction)?;
    let version_scheme = installed.version_scheme;
    let installed_id = installed.id.ok_or_else(|| {
        anyhow!(
            "installed replatform trove '{}' missing id",
            transaction.package
        )
    })?;

    Trove::update_replatform_metadata(
        conn,
        installed_id,
        repository.source_profile.as_deref(),
        version_scheme,
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
        .filter(|trove| {
            trove.version == transaction.target_version && trove.trove_type == TroveType::Package
        })
        .collect::<Vec<_>>();

    select_replatform_variant(
        &matches,
        transaction.architecture.as_deref(),
        &transaction.package,
    )?
    .cloned()
    .ok_or_else(|| {
        anyhow!(
            "installed replatform trove '{}' not found",
            transaction.package
        )
    })
}

fn find_current_replatform_source(
    conn: &Connection,
    transaction: &conary_core::model::ReplatformExecutionTransaction,
) -> Result<Option<String>> {
    let matches = Trove::find_by_name(conn, &transaction.package)?
        .into_iter()
        .filter(|trove| {
            trove.version == transaction.current_version && trove.trove_type == TroveType::Package
        })
        .collect::<Vec<_>>();

    let Some(trove) = select_replatform_variant(
        &matches,
        transaction.current_architecture.as_deref(),
        &transaction.package,
    )?
    else {
        return Ok(None);
    };
    if let Some(repository_id) = trove.installed_from_repository_id
        && let Some(repository) = Repository::find_by_id(conn, repository_id)?
    {
        return Ok(repository
            .resolution_source_identity()?
            .map(str::to_string)
            .or_else(|| trove.source_profile.clone()));
    }
    Ok(trove.source_profile.clone())
}

fn select_replatform_variant<'a>(
    matches: &'a [Trove],
    expected_architecture: Option<&str>,
    package: &str,
) -> Result<Option<&'a Trove>> {
    let exact = matches
        .iter()
        .filter(|trove| trove.architecture.as_deref() == expected_architecture)
        .collect::<Vec<_>>();
    match exact.as_slice() {
        [trove] => return Ok(Some(*trove)),
        [] => {}
        _ => {
            return Err(anyhow!(
                "replatform package '{}' has multiple exact architecture matches for {}",
                package,
                format_replatform_architecture(expected_architecture),
            ));
        }
    }

    if expected_architecture.is_none() {
        return match matches {
            [] => Ok(None),
            [trove] => Ok(Some(trove)),
            _ => Err(anyhow!(
                "replatform package '{}' has multiple installed variants but no architecture selector",
                package,
            )),
        };
    }

    let architecture_independent = matches
        .iter()
        .filter(|trove| trove.architecture.is_none())
        .collect::<Vec<_>>();
    match architecture_independent.as_slice() {
        [trove] => Ok(Some(*trove)),
        [] => Ok(None),
        _ => Err(anyhow!(
            "replatform package '{}' has multiple architecture-independent matches for {}",
            package,
            format_replatform_architecture(expected_architecture),
        )),
    }
}

fn format_replatform_architecture(architecture: Option<&str>) -> String {
    architecture
        .map(|architecture| format!("architecture '{architecture}'"))
        .unwrap_or_else(|| "architecture-independent identity".to_string())
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
            DiffAction::MarkExplicit { package } => match Trove::find_one_by_name(conn, package) {
                Ok(Some(trove)) => match trove.id {
                    Some(trove_id) => match Trove::promote_to_explicit(
                        conn,
                        trove_id,
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
                    },
                    None => errors.push(format!(
                        "MarkExplicit '{}': installed trove has no persisted ID",
                        package
                    )),
                },
                Ok(None) => debug!("'{}' not installed", package),
                Err(e) => errors.push(format!("MarkExplicit '{}': {}", package, e)),
            },
            DiffAction::MarkDependency { package } => {
                match Trove::find_one_by_name(conn, package) {
                    Ok(Some(trove)) => match trove.id {
                        Some(trove_id) => match conn.execute(
                            "UPDATE troves SET install_reason = 'dependency' \
                             WHERE id = ?1 AND install_reason = 'explicit' AND type = 'package'",
                            rusqlite::params![trove_id],
                        ) {
                            Ok(rows) if rows > 0 => {
                                println!("Marked '{}' as dependency", package);
                                applied += 1;
                            }
                            Ok(_) => {
                                debug!("'{}' already a dependency", package);
                            }
                            Err(e) => errors.push(format!("MarkDependency '{}': {}", package, e)),
                        },
                        None => errors.push(format!(
                            "MarkDependency '{}': installed trove has no persisted ID",
                            package
                        )),
                    },
                    Ok(None) => debug!("'{}' not installed", package),
                    Err(e) => errors.push(format!("MarkDependency '{}': {}", package, e)),
                }
            }
            _ => {}
        }
    }

    (applied, errors)
}

#[cfg(test)]
mod dry_run_tests;
#[cfg(test)]
mod tests;
