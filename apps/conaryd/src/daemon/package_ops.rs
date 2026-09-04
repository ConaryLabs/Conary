// apps/conaryd/src/daemon/package_ops.rs
//! Daemon execution for package install, remove, and update jobs.

use crate::daemon::routes::TransactionOperation;
use crate::daemon::{DaemonEvent, DaemonState, JobKind};
use anyhow::{Context, Result, bail};
use conary::commands::{InstallOptions, SandboxMode, cmd_install, cmd_remove, cmd_update};
use conary::live_host_safety::{
    LiveMutationClass, LiveMutationRequest, MutationIntent, require_mutation_intent,
};
use serde::Serialize;
use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Serialize)]
pub struct PackageJobResult {
    pub operations: Vec<PackageOperationResult>,
}

#[derive(Debug, Serialize)]
pub struct PackageOperationResult {
    pub operation: String,
    pub packages: Vec<String>,
    pub dry_run: bool,
    pub status: String,
}

#[derive(Debug, Clone)]
enum PackageCommand {
    Install {
        packages: Vec<String>,
        allow_downgrade: bool,
        skip_deps: bool,
        dry_run: bool,
        yes: bool,
        apply_intent: bool,
    },
    Remove {
        packages: Vec<String>,
        cascade: bool,
        remove_orphans: bool,
        purge: bool,
        apply_intent: bool,
    },
    Update {
        packages: Vec<String>,
        security_only: bool,
        dry_run: bool,
        yes: bool,
        apply_intent: bool,
    },
}

pub async fn execute_package_job(
    state: Arc<DaemonState>,
    job_id: &str,
    kind: JobKind,
    spec: serde_json::Value,
    cancel_token: Arc<AtomicBool>,
) -> Result<PackageJobResult> {
    let operations = parse_operations(spec)?;
    ensure_kind_matches(kind, &operations)?;

    let total = operation_unit_count(&operations);
    let mut completed = 0_u64;
    let mut results = Vec::with_capacity(operations.len());

    for operation in operations {
        ensure_not_cancelled(&cancel_token)?;
        let phase = phase_for_operation(&operation).to_string();
        state.emit(DaemonEvent::JobPhase {
            job_id: job_id.to_string(),
            phase: phase.clone(),
        });

        let packages = packages_for_operation(&operation).to_vec();
        let dry_run = operation_dry_run(&operation);
        state.emit(DaemonEvent::JobProgress {
            job_id: job_id.to_string(),
            current: completed,
            total,
            message: format!("{phase} {}", format_packages(&packages)),
        });

        execute_one(&state, &operation).await?;

        completed += packages.len().max(1) as u64;
        state.emit(DaemonEvent::JobProgress {
            job_id: job_id.to_string(),
            current: completed,
            total,
            message: format!("completed {phase} {}", format_packages(&packages)),
        });

        results.push(PackageOperationResult {
            operation: phase,
            packages,
            dry_run,
            status: "completed".to_string(),
        });
    }

    Ok(PackageJobResult {
        operations: results,
    })
}

fn parse_operations(spec: serde_json::Value) -> Result<Vec<TransactionOperation>> {
    serde_json::from_value(spec).context("Failed to parse daemon package job specification")
}

fn ensure_kind_matches(kind: JobKind, operations: &[TransactionOperation]) -> Result<()> {
    for operation in operations {
        let operation_kind = match operation {
            TransactionOperation::Install { .. } => JobKind::Install,
            TransactionOperation::Remove { .. } => JobKind::Remove,
            TransactionOperation::Update { .. } => JobKind::Update,
        };

        if operation_kind != kind {
            bail!(
                "Package job kind '{}' cannot execute '{}' operation",
                kind.as_str(),
                operation_kind.as_str()
            );
        }
    }

    Ok(())
}

async fn execute_one(state: &DaemonState, operation: &TransactionOperation) -> Result<()> {
    let db_path = state.config.db_path.to_string_lossy().into_owned();
    let root = state.config.root.to_string_lossy().into_owned();
    let command = PackageCommand::from(operation);

    tokio::task::spawn_blocking(move || -> Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("Failed to build package executor runtime")?;
        runtime.block_on(run_cli_command(command, db_path, root))
    })
    .await
    .context("Package executor task join failed")?
}

impl From<&TransactionOperation> for PackageCommand {
    fn from(operation: &TransactionOperation) -> Self {
        match operation {
            TransactionOperation::Install {
                packages,
                allow_downgrade,
                skip_deps,
                dry_run,
                yes,
                apply_intent,
            } => Self::Install {
                packages: packages.clone(),
                allow_downgrade: *allow_downgrade,
                skip_deps: *skip_deps,
                dry_run: *dry_run,
                yes: *yes,
                apply_intent: *apply_intent,
            },
            TransactionOperation::Remove {
                packages,
                cascade,
                remove_orphans,
                purge,
                apply_intent,
            } => Self::Remove {
                packages: packages.clone(),
                cascade: *cascade,
                remove_orphans: *remove_orphans,
                purge: *purge,
                apply_intent: *apply_intent,
            },
            TransactionOperation::Update {
                packages,
                security_only,
                dry_run,
                yes,
                apply_intent,
            } => Self::Update {
                packages: packages.clone(),
                security_only: *security_only,
                dry_run: *dry_run,
                yes: *yes,
                apply_intent: *apply_intent,
            },
        }
    }
}

async fn run_cli_command(command: PackageCommand, db_path: String, root: String) -> Result<()> {
    match command {
        PackageCommand::Install {
            packages,
            allow_downgrade,
            skip_deps,
            dry_run,
            yes,
            apply_intent,
        } => {
            require_live_ack(
                "conaryd install",
                dry_run,
                MutationIntent::from_apply_intent(apply_intent),
            )?;
            for package in packages {
                let mut opts = InstallOptions::default();
                opts.db_path = &db_path;
                opts.root = &root;
                opts.dry_run = dry_run;
                opts.no_deps = skip_deps;
                opts.sandbox_mode = SandboxMode::Always;
                opts.allow_downgrade = allow_downgrade;
                opts.yes = yes;
                cmd_install(&package, opts).await?;
                if !dry_run {
                    require_generation_publication_complete(&db_path)?;
                }
            }
        }
        PackageCommand::Remove {
            packages,
            cascade,
            remove_orphans,
            purge,
            apply_intent,
        } => {
            if cascade || remove_orphans {
                bail!(
                    "Daemon remove jobs do not support cascade or remove_orphans yet; use explicit remove jobs"
                );
            }
            require_live_ack(
                "conaryd remove",
                false,
                MutationIntent::from_apply_intent(apply_intent),
            )?;
            for package in packages {
                cmd_remove(&package, &db_path, None, None, SandboxMode::Always, purge)?;
                require_generation_publication_complete(&db_path)?;
            }
        }
        PackageCommand::Update {
            packages,
            security_only,
            dry_run,
            yes,
            apply_intent,
        } => {
            require_live_ack(
                "conaryd update",
                dry_run,
                MutationIntent::from_apply_intent(apply_intent),
            )?;
            if packages.is_empty() {
                cmd_update(
                    None,
                    &db_path,
                    &root,
                    security_only,
                    dry_run,
                    SandboxMode::Always,
                    None,
                    yes,
                    None,
                    None,
                )
                .await?;
                if !dry_run {
                    require_generation_publication_complete(&db_path)?;
                }
            } else {
                for package in packages {
                    cmd_update(
                        Some(package),
                        &db_path,
                        &root,
                        security_only,
                        dry_run,
                        SandboxMode::Always,
                        None,
                        yes,
                        None,
                        None,
                    )
                    .await?;
                    if !dry_run {
                        require_generation_publication_complete(&db_path)?;
                    }
                }
            }
        }
    }

    Ok(())
}

fn require_generation_publication_complete(db_path: &str) -> Result<()> {
    let conn = conary_core::db::open(db_path)
        .with_context(|| format!("Failed to inspect generation publication state in {db_path}"))?;
    let debts = conary_core::db::models::GenerationPublication::pending_recoverable(&conn)
        .context("Failed to inspect pending generation publication state")?;
    let Some(debt) = debts.last() else {
        return Ok(());
    };
    let detail = debt
        .last_error
        .as_deref()
        .unwrap_or("publication has not recorded a failure detail");
    bail!(
        "Package database mutation committed, but generation publication {} remains {} at phase {}: {}. Run: conary system generation publish --yes",
        debt.id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "<unknown>".to_string()),
        debt.status,
        debt.phase,
        detail
    );
}

fn require_live_ack(
    command_label: &'static str,
    dry_run: bool,
    intent: MutationIntent,
) -> Result<()> {
    require_mutation_intent(&LiveMutationRequest {
        command_label: Cow::Borrowed(command_label),
        class: LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
        dry_run,
        intent,
    })
}

fn ensure_not_cancelled(cancel_token: &AtomicBool) -> Result<()> {
    if cancel_token.load(Ordering::Relaxed) {
        bail!("Package job was cancelled");
    }

    Ok(())
}

fn operation_unit_count(operations: &[TransactionOperation]) -> u64 {
    operations
        .iter()
        .map(|operation| packages_for_operation(operation).len().max(1) as u64)
        .sum()
}

fn phase_for_operation(operation: &TransactionOperation) -> &'static str {
    match operation {
        TransactionOperation::Install { .. } => "install",
        TransactionOperation::Remove { .. } => "remove",
        TransactionOperation::Update { .. } => "update",
    }
}

fn operation_dry_run(operation: &TransactionOperation) -> bool {
    match operation {
        TransactionOperation::Install { dry_run, .. }
        | TransactionOperation::Update { dry_run, .. } => *dry_run,
        TransactionOperation::Remove { .. } => false,
    }
}

fn packages_for_operation(operation: &TransactionOperation) -> &[String] {
    match operation {
        TransactionOperation::Install { packages, .. }
        | TransactionOperation::Remove { packages, .. }
        | TransactionOperation::Update { packages, .. } => packages,
    }
}

fn format_packages(packages: &[String]) -> String {
    if packages.is_empty() {
        "all packages".to_string()
    } else {
        packages.join(", ")
    }
}

#[cfg(test)]
#[path = "package_ops/tests.rs"]
mod tests;
