// apps/conaryd/src/daemon/package_ops/tests.rs

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
    conary_core::ccs::HostCapabilityInventory::default()
        .persist(&conn)
        .unwrap();
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
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(state.config.db_path.clone());
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
