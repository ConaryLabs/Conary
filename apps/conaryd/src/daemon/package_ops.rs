// apps/conaryd/src/daemon/package_ops.rs
//! Daemon execution for package install, remove, and update jobs.

use crate::daemon::routes::TransactionOperation;
use crate::daemon::{DaemonEvent, DaemonState, JobKind};
use anyhow::{Context, Result, bail};
use conary::commands::{
    InstallOptions, LegacyReplayOptions, SandboxMode, cmd_install, cmd_remove, cmd_update,
};
use conary::live_host_safety::{
    LiveMutationClass, LiveMutationRequest, require_live_system_mutation_ack,
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
        no_scripts: bool,
        yes: bool,
        allow_live_system_mutation: bool,
    },
    Remove {
        packages: Vec<String>,
        cascade: bool,
        remove_orphans: bool,
        no_scripts: bool,
        purge_files: bool,
        allow_live_system_mutation: bool,
    },
    Update {
        packages: Vec<String>,
        security_only: bool,
        dry_run: bool,
        yes: bool,
        allow_live_system_mutation: bool,
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
                no_scripts,
                yes,
                allow_live_system_mutation,
            } => Self::Install {
                packages: packages.clone(),
                allow_downgrade: *allow_downgrade,
                skip_deps: *skip_deps,
                dry_run: *dry_run,
                no_scripts: *no_scripts,
                yes: *yes,
                allow_live_system_mutation: *allow_live_system_mutation,
            },
            TransactionOperation::Remove {
                packages,
                cascade,
                remove_orphans,
                no_scripts,
                purge_files,
                allow_live_system_mutation,
            } => Self::Remove {
                packages: packages.clone(),
                cascade: *cascade,
                remove_orphans: *remove_orphans,
                no_scripts: *no_scripts,
                purge_files: *purge_files,
                allow_live_system_mutation: *allow_live_system_mutation,
            },
            TransactionOperation::Update {
                packages,
                security_only,
                dry_run,
                yes,
                allow_live_system_mutation,
            } => Self::Update {
                packages: packages.clone(),
                security_only: *security_only,
                dry_run: *dry_run,
                yes: *yes,
                allow_live_system_mutation: *allow_live_system_mutation,
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
            no_scripts,
            yes,
            allow_live_system_mutation,
        } => {
            require_live_ack("conaryd install", dry_run, allow_live_system_mutation)?;
            for package in packages {
                let mut opts = InstallOptions::default();
                opts.db_path = &db_path;
                opts.root = &root;
                opts.dry_run = dry_run;
                opts.no_deps = skip_deps;
                opts.no_scripts = no_scripts;
                opts.sandbox_mode = SandboxMode::Always;
                opts.allow_downgrade = allow_downgrade;
                opts.yes = yes;
                cmd_install(&package, opts).await?;
            }
        }
        PackageCommand::Remove {
            packages,
            cascade,
            remove_orphans,
            no_scripts,
            purge_files,
            allow_live_system_mutation,
        } => {
            if cascade || remove_orphans {
                bail!(
                    "Daemon remove jobs do not support cascade or remove_orphans yet; use explicit remove jobs"
                );
            }
            require_live_ack("conaryd remove", false, allow_live_system_mutation)?;
            for package in packages {
                cmd_remove(
                    &package,
                    &db_path,
                    &root,
                    None,
                    None,
                    no_scripts,
                    SandboxMode::Always,
                    purge_files,
                    LegacyReplayOptions::default(),
                )
                .await?;
            }
        }
        PackageCommand::Update {
            packages,
            security_only,
            dry_run,
            yes,
            allow_live_system_mutation,
        } => {
            require_live_ack("conaryd update", dry_run, allow_live_system_mutation)?;
            if packages.is_empty() {
                cmd_update(
                    None,
                    &db_path,
                    &root,
                    security_only,
                    dry_run,
                    false,
                    SandboxMode::Always,
                    None,
                    yes,
                    None,
                    None,
                    LegacyReplayOptions::default(),
                )
                .await?;
            } else {
                for package in packages {
                    cmd_update(
                        Some(package),
                        &db_path,
                        &root,
                        security_only,
                        dry_run,
                        false,
                        SandboxMode::Always,
                        None,
                        yes,
                        None,
                        None,
                        LegacyReplayOptions::default(),
                    )
                    .await?;
                }
            }
        }
    }

    Ok(())
}

fn require_live_ack(
    command_label: &'static str,
    dry_run: bool,
    allow_live_system_mutation: bool,
) -> Result<()> {
    require_live_system_mutation_ack(
        allow_live_system_mutation,
        &LiveMutationRequest {
            command_label: Cow::Borrowed(command_label),
            class: LiveMutationClass::CurrentlyLiveEvenWithRootArguments,
            dry_run,
        },
    )
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
mod tests {
    use super::*;
    use crate::daemon::{DaemonConfig, SystemLock};
    use conary_core::db::models::{FileEntry, InstallSource, Trove, TroveType};
    use std::sync::atomic::AtomicBool;
    use tempfile::TempDir;

    fn create_test_state() -> (Arc<DaemonState>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let db_path = root.join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        let lock_path = temp_dir.path().join("daemon.lock");
        let config = DaemonConfig {
            db_path,
            root,
            lock_path: lock_path.clone(),
            ..Default::default()
        };

        let system_lock = SystemLock::try_acquire(&lock_path)
            .unwrap()
            .expect("test daemon lock should be acquirable");
        (Arc::new(DaemonState::new(config, system_lock)), temp_dir)
    }

    #[tokio::test]
    async fn package_executor_refuses_live_mutation_without_ack() {
        let (state, _temp_dir) = create_test_state();
        let spec = serde_json::json!([
            {
                "type": "install",
                "packages": ["fixture"],
                "allow_downgrade": false,
                "skip_deps": false
            }
        ]);

        let err = execute_package_job(
            state,
            "job-install-refusal",
            JobKind::Install,
            spec,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap_err();

        let message = format!("{err:#}");
        assert!(
            message.contains("--allow-live-system-mutation"),
            "{message}"
        );
        assert!(message.contains("conaryd install"), "{message}");
    }

    #[tokio::test]
    async fn package_executor_runs_remove_through_cli_contract() {
        let (state, _temp_dir) = create_test_state();
        let payload = state.config.root.join("usr/bin/fixture");
        std::fs::create_dir_all(payload.parent().unwrap()).unwrap();
        std::fs::write(&payload, "fixture").unwrap();

        {
            let conn = conary_core::db::open(&state.config.db_path).unwrap();
            let mut trove = Trove::new_with_source(
                "fixture".to_string(),
                "1.0.0".to_string(),
                TroveType::Package,
                InstallSource::Repository,
            );
            let trove_id = trove.insert(&conn).unwrap();
            let mut file = FileEntry::new(
                "/usr/bin/fixture".to_string(),
                "0".repeat(64),
                "fixture".len() as i64,
                0o100755,
                trove_id,
            );
            file.insert(&conn).unwrap();
        }

        let spec = serde_json::json!([
            {
                "type": "remove",
                "packages": ["fixture"],
                "cascade": false,
                "remove_orphans": false,
                "no_scripts": true,
                "allow_live_system_mutation": true
            }
        ]);

        let result = execute_package_job(
            state.clone(),
            "job-remove-fixture",
            JobKind::Remove,
            spec,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();

        assert_eq!(result.operations.len(), 1);
        assert_eq!(result.operations[0].operation, "remove");
        assert_eq!(result.operations[0].packages, vec!["fixture"]);
        assert!(!payload.exists());

        let conn = conary_core::db::open(&state.config.db_path).unwrap();
        assert!(Trove::find_by_name(&conn, "fixture").unwrap().is_empty());
    }

    #[tokio::test]
    async fn package_executor_accepts_update_dry_run_without_live_ack() {
        let (state, _temp_dir) = create_test_state();
        let spec = serde_json::json!([
            {
                "type": "update",
                "packages": [],
                "security_only": false,
                "dry_run": true
            }
        ]);

        let result = execute_package_job(
            state,
            "job-update-dry-run",
            JobKind::Update,
            spec,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();

        assert_eq!(result.operations.len(), 1);
        assert_eq!(result.operations[0].operation, "update");
        assert!(result.operations[0].dry_run);
        assert!(result.operations[0].packages.is_empty());
    }
}
