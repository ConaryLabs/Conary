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
                cmd_remove(&package, &db_path, None, None, SandboxMode::Always, purge).await?;
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
mod tests {
    use super::*;
    use crate::daemon::{DaemonConfig, SystemLock};
    use conary_core::db::models::{FileEntry, InstallSource, Trove, TroveType};
    use conary_core::payload::{
        PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, ResolvedPayloadNode,
    };
    use std::sync::atomic::AtomicBool;
    use tempfile::TempDir;

    static TEST_GENERATION_MOUNT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct TestGenerationMountSkipGuard {
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl TestGenerationMountSkipGuard {
        fn acquire() -> Self {
            let guard = TEST_GENERATION_MOUNT_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            unsafe {
                std::env::set_var("CONARY_TEST_SKIP_GENERATION_MOUNT", "1");
            }
            Self { _guard: guard }
        }
    }

    impl Drop for TestGenerationMountSkipGuard {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var("CONARY_TEST_SKIP_GENERATION_MOUNT");
            }
        }
    }

    fn regular_file_entry(
        db_path: &std::path::Path,
        path: &str,
        content: &[u8],
        permissions: u32,
        trove_id: i64,
    ) -> FileEntry {
        let runtime_root =
            conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path.to_path_buf());
        let sha256 = conary_core::filesystem::CasStore::new(runtime_root.objects_dir())
            .unwrap()
            .store(content)
            .unwrap();
        FileEntry::new(
            path.to_string(),
            ResolvedPayloadNode::from_numeric_source(test_payload_node(permissions)).unwrap(),
            Some(PayloadContentAuthority {
                sha256,
                size: content.len() as u64,
            }),
            trove_id,
        )
    }

    fn directory_file_entry(path: &str, permissions: u32, trove_id: i64) -> FileEntry {
        let mut node = test_payload_node(permissions);
        node.kind = PayloadNodeKind::Directory;
        node.mode = libc::S_IFDIR | permissions;
        FileEntry::new(
            path.to_string(),
            ResolvedPayloadNode::from_numeric_source(node).unwrap(),
            None,
            trove_id,
        )
    }

    fn test_payload_node(permissions: u32) -> PayloadNode {
        let mut node = PayloadNode::regular(permissions);
        node.user = PayloadIdentity::Numeric {
            id: u64::from(nix::unistd::geteuid().as_raw()),
        };
        node.group = PayloadIdentity::Numeric {
            id: u64::from(nix::unistd::getegid().as_raw()),
        };
        node
    }

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
        (
            Arc::new(DaemonState::new(config, system_lock).unwrap()),
            temp_dir,
        )
    }

    fn seed_test_bootable_runtime(state: &DaemonState) {
        let boot = state.config.root.join("boot");
        std::fs::create_dir_all(boot.join("EFI/BOOT")).unwrap();
        std::fs::write(boot.join("vmlinuz-test-kernel"), b"test-kernel").unwrap();
        std::fs::write(boot.join("initramfs-test-kernel.img"), b"test-initramfs").unwrap();
        std::fs::write(boot.join("EFI/BOOT/BOOTX64.EFI"), b"test-efi").unwrap();

        let conn = conary_core::db::open(&state.config.db_path).unwrap();
        let mut trove = Trove::new_with_source(
            "test-runtime-base".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        trove.architecture = Some("x86_64".to_string());
        let trove_id = trove.insert(&conn).unwrap();
        let mut sbin = directory_file_entry("/sbin", 0o755, trove_id);
        sbin.insert(&conn).unwrap();
        let mut init = regular_file_entry(
            &state.config.db_path,
            "/sbin/init",
            b"test init binary",
            0o755,
            trove_id,
        );
        init.insert(&conn).unwrap();
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
        assert!(message.contains("--dry-run"), "{message}");
        assert!(message.contains("--yes"), "{message}");
        assert!(
            !message.contains("--allow-live-system-mutation"),
            "{message}"
        );
        assert!(message.contains("conaryd install"), "{message}");
    }

    #[test]
    fn package_executor_accepts_apply_intent_without_old_ack() {
        assert!(require_live_ack("conaryd install", false, MutationIntent::Apply).is_ok());
    }

    #[test]
    fn package_executor_rejects_removed_ack_field() {
        let spec = serde_json::json!([
            {
                "type": "install",
                "packages": ["fixture"],
                "allow_live_system_mutation": true
            }
        ]);

        let err = parse_operations(spec).expect_err("removed acknowledgement field must fail");
        assert!(
            format!("{err:#}").contains("unknown field `allow_live_system_mutation`"),
            "{err:#}"
        );
    }

    #[test]
    fn package_executor_rejects_removed_lifecycle_bypass_field() {
        for spec in [
            serde_json::json!([
                {
                    "type": "install",
                    "packages": ["fixture"],
                    "no_scripts": true
                }
            ]),
            serde_json::json!([
                {
                    "type": "remove",
                    "packages": ["fixture"],
                    "no_scripts": true
                }
            ]),
        ] {
            let err = parse_operations(spec).expect_err("removed lifecycle bypass field must fail");
            assert!(
                format!("{err:#}").contains("unknown field `no_scripts`"),
                "{err:#}"
            );
        }

        let err = serde_json::from_value::<crate::daemon::routes::PackageOperationRequest>(
            serde_json::json!({
                "packages": ["fixture"],
                "options": {
                    "no_scripts": true
                }
            }),
        )
        .expect_err("convenience package request must reject the lifecycle bypass field");
        assert!(
            err.to_string().contains("unknown field `no_scripts`"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn package_executor_runs_remove_through_cli_contract() {
        let _mount_skip = TestGenerationMountSkipGuard::acquire();
        let (state, _temp_dir) = create_test_state();
        seed_test_bootable_runtime(&state);
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
                conary_core::repository::versioning::VersionScheme::Conary,
            );
            let trove_id = trove.insert(&conn).unwrap();
            for path in ["/usr", "/usr/bin"] {
                let mut directory = directory_file_entry(path, 0o755, trove_id);
                directory.insert(&conn).unwrap();
            }
            let mut file = regular_file_entry(
                &state.config.db_path,
                "/usr/bin/fixture",
                b"fixture",
                0o755,
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
                "apply_intent": true
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
        assert_eq!(std::fs::read_to_string(&payload).unwrap(), "fixture");

        let conn = conary_core::db::open(&state.config.db_path).unwrap();
        assert!(Trove::find_by_name(&conn, "fixture").unwrap().is_empty());
        let runtime_root = conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(
            state.config.db_path.clone(),
        );
        let generation = conary_core::generation::mount::current_generation(runtime_root.root())
            .unwrap()
            .expect("successful daemon mutation must publish a selected generation");
        let artifact = conary_core::generation::artifact::load_generation_artifact(
            &runtime_root.generation_path(generation),
        )
        .unwrap();
        assert!(
            artifact
                .generation_root
                .entries
                .iter()
                .all(|entry| entry.path != "/usr/bin/fixture")
        );
        assert!(
            conary_core::db::models::GenerationPublication::pending_recoverable(&conn)
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn package_executor_reports_committed_mutation_with_pending_publication() {
        let _mount_skip = TestGenerationMountSkipGuard::acquire();
        let (state, _temp_dir) = create_test_state();
        std::fs::create_dir_all(state.config.root.join("boot")).unwrap();

        {
            let conn = conary_core::db::open(&state.config.db_path).unwrap();
            let mut trove = Trove::new_with_source(
                "fixture".to_string(),
                "1.0.0".to_string(),
                TroveType::Package,
                InstallSource::Repository,
                conary_core::repository::versioning::VersionScheme::Conary,
            );
            let trove_id = trove.insert(&conn).unwrap();
            for path in ["/usr", "/usr/bin"] {
                let mut directory = directory_file_entry(path, 0o755, trove_id);
                directory.insert(&conn).unwrap();
            }
            let mut file = regular_file_entry(
                &state.config.db_path,
                "/usr/bin/fixture",
                b"fixture",
                0o755,
                trove_id,
            );
            file.insert(&conn).unwrap();
        }

        let err = execute_package_job(
            state.clone(),
            "job-remove-pending-publication",
            JobKind::Remove,
            serde_json::json!([
                {
                    "type": "remove",
                    "packages": ["fixture"],
                    "cascade": false,
                    "remove_orphans": false,
                    "apply_intent": true
                }
            ]),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap_err();

        let message = format!("{err:#}");
        assert!(message.contains("mutation committed"), "{message}");
        assert!(message.contains("generation publication"), "{message}");
        assert!(
            message.contains("conary system generation publish --yes"),
            "{message}"
        );
        let conn = conary_core::db::open(&state.config.db_path).unwrap();
        assert!(Trove::find_by_name(&conn, "fixture").unwrap().is_empty());
        assert_eq!(
            conary_core::db::models::GenerationPublication::pending_recoverable(&conn)
                .unwrap()
                .len(),
            1
        );
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
