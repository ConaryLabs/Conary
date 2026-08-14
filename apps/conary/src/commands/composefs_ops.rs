// src/commands/composefs_ops.rs

//! Shared composefs-native operations for CLI commands.
//!
//! Every package mutation (install, remove, restore, rollback) ends with
//! the same atomic generation publication sequence:
//!
//! 1. `build_generation_from_db` -- build EROFS image from current DB state
//! 2. Exact typed config transaction materialization into a generation-local
//!    `/etc` overlay
//! 3. `enable_generation_rootfs_verity` -- make runtime metadata truthful when
//!    the backing filesystem supports fs-verity
//! 4. `update_current_symlink` -- point `/conary/current` at the next-boot generation

use std::path::PathBuf;

use rusqlite::Connection;
use tracing::info;

use crate::commands::generation::builder::enable_generation_rootfs_verity;
use conary_core::config_transaction::GenerationConfigTransaction;
use conary_core::db::models::{GenerationPublication, SystemState};
use conary_core::generation::root_manifest::CapturedSelectedRoot;
use conary_core::runtime_root::ConaryRuntimeRoot;

/// Rebuild the EROFS generation from current DB state and publish it.
///
/// This is the composefs-native publication step that follows every DB
/// mutation (install, remove, restore, rollback).  It:
///
/// 1. Builds a new EROFS image from all installed packages in the DB
/// 2. Materializes every durable typed config transaction into an unpublished
///    generation-local upper directory
/// 3. Enables fs-verity for the generation image when not explicitly skipped
/// 4. Leaves current-link publication to the caller
///
/// The runtime root is derived through `ConaryRuntimeRoot`, so the default DB
/// at `/var/lib/conary/conary.db` still stores boot-visible generation state
/// under `/conary` while non-default test DB paths stay self-contained.
///
/// Returns the new generation number on success.
fn runtime_root_for_db_path(db_path: &str) -> ConaryRuntimeRoot {
    ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path))
}

fn boot_root_for_generation_build(runtime_root: &ConaryRuntimeRoot) -> PathBuf {
    if std::env::var_os("CONARY_TEST_SKIP_GENERATION_MOUNT").is_some() {
        let test_boot = runtime_root.root().join("boot");
        if test_boot.is_dir() {
            return test_boot;
        }
    }

    PathBuf::from("/boot")
}

#[derive(Debug)]
pub(crate) struct BuiltGeneration {
    pub generation_number: i64,
    pub state_number: i64,
}

fn build_generation_from_installed_state(
    conn: &Connection,
    db_path: &str,
    summary: &str,
    config_transactions: &[GenerationConfigTransaction],
) -> anyhow::Result<BuiltGeneration> {
    build_generation_for_input(conn, db_path, summary, config_transactions, None)
}

pub(crate) fn build_captured_generation_for_publication(
    conn: &Connection,
    db_path: &str,
    summary: &str,
    config_transactions: &[GenerationConfigTransaction],
    captured: CapturedSelectedRoot,
) -> anyhow::Result<BuiltGeneration> {
    build_generation_for_input(conn, db_path, summary, config_transactions, Some(captured))
}

fn build_generation_for_input(
    conn: &Connection,
    db_path: &str,
    summary: &str,
    config_transactions: &[GenerationConfigTransaction],
    captured: Option<CapturedSelectedRoot>,
) -> anyhow::Result<BuiltGeneration> {
    if let Some(error) = forced_generation_rebuild_failure() {
        return Err(error);
    }

    let runtime_root = runtime_root_for_db_path(db_path);
    let generations_dir = runtime_root.generations_dir();
    let boot_root = boot_root_for_generation_build(&runtime_root);
    let activation = conary_core::generation::builder::GenerationActivation::Inactive;
    let (gen_num, build_result) = match captured {
        Some(captured) => conary_core::generation::builder::
            build_generation_from_captured_root_with_boot_root_and_activation(
                conn,
                &generations_dir,
                summary,
                &boot_root,
                activation,
                captured,
            ),
        None => conary_core::generation::builder::
            build_generation_from_db_with_boot_root_and_activation(
                conn,
                &generations_dir,
                summary,
                &boot_root,
                activation,
            ),
    }
    .map_err(|e| anyhow::anyhow!("Failed to build EROFS generation: {e}"))?;

    info!(
        "Built generation {gen_num} ({} bytes, {} CAS objects)",
        build_result.image_size, build_result.cas_objects_referenced
    );

    seed_generation_mutable_state(&runtime_root, gen_num)?;
    crate::commands::generation::config_transaction::materialize(
        &runtime_root,
        gen_num,
        config_transactions,
    )?;

    if std::env::var_os("CONARY_TEST_SKIP_GENERATION_MOUNT").is_some() {
        info!(
            "Skipping generation fs-verity enablement because CONARY_TEST_SKIP_GENERATION_MOUNT is set"
        );
        return Ok(BuiltGeneration {
            generation_number: gen_num,
            state_number: gen_num,
        });
    }

    let gen_dir = generations_dir.join(gen_num.to_string());
    enable_generation_rootfs_verity(&gen_dir, &build_result.image_path).map_err(|e| {
        anyhow::anyhow!(
            "Failed to enable fs-verity on generation {gen_num} image {}: {e}",
            build_result.image_path.display()
        )
    })?;

    Ok(BuiltGeneration {
        generation_number: gen_num,
        state_number: gen_num,
    })
}

pub(crate) fn build_inactive_generation_for_runtime(
    conn: &Connection,
    runtime_root: &ConaryRuntimeRoot,
    summary: &str,
    captured: CapturedSelectedRoot,
) -> anyhow::Result<BuiltGeneration> {
    if let Some(error) = forced_generation_rebuild_failure() {
        return Err(error);
    }

    let generations_dir = runtime_root.generations_dir();
    let boot_root = boot_root_for_generation_build(runtime_root);
    let (gen_num, build_result) = conary_core::generation::builder::
        build_generation_from_captured_root_with_boot_root_and_activation(
            conn,
            &generations_dir,
            summary,
            &boot_root,
            conary_core::generation::builder::GenerationActivation::Inactive,
            captured,
        )
        .map_err(|e| anyhow::anyhow!("Failed to build inactive try generation: {e}"))?;

    info!(
        "Built inactive try generation {gen_num} ({} bytes, {} CAS objects)",
        build_result.image_size, build_result.cas_objects_referenced
    );

    seed_generation_mutable_state(runtime_root, gen_num)?;
    let transactions = GenerationPublication::pending_recoverable(conn)?
        .into_iter()
        .map(|publication| publication.config_transaction)
        .collect::<Vec<_>>();
    crate::commands::generation::config_transaction::materialize(
        runtime_root,
        gen_num,
        &transactions,
    )?;

    if std::env::var_os("CONARY_TEST_SKIP_GENERATION_MOUNT").is_none() {
        let gen_dir = generations_dir.join(gen_num.to_string());
        enable_generation_rootfs_verity(&gen_dir, &build_result.image_path).map_err(|e| {
            anyhow::anyhow!(
                "Failed to enable fs-verity on try generation {gen_num} image {}: {e}",
                build_result.image_path.display()
            )
        })?;
    }

    Ok(BuiltGeneration {
        generation_number: gen_num,
        state_number: gen_num,
    })
}

fn seed_generation_mutable_state(
    runtime_root: &ConaryRuntimeRoot,
    generation_number: i64,
) -> anyhow::Result<()> {
    let artifact = conary_core::generation::artifact::load_generation_artifact(
        &runtime_root.generation_path(generation_number),
    )
    .map_err(|error| {
        anyhow::anyhow!(
            "failed to load generation {generation_number} mutable-state authority: {error}"
        )
    })?;
    let state_root = runtime_root
        .etc_state_dir()
        .join(generation_number.to_string());
    if state_root.exists() {
        std::fs::remove_dir_all(&state_root).map_err(|error| {
            anyhow::anyhow!(
                "failed to replace unpublished generation state {}: {error}",
                state_root.display()
            )
        })?;
    }
    std::fs::create_dir_all(&state_root)?;
    let cas = conary_core::filesystem::CasStore::new(runtime_root.objects_dir())?;
    conary_core::generation::root_manifest::materialize_config_state_upper(
        &artifact.mutable_state,
        &cas,
        &state_root,
    )
    .map_err(|error| {
        anyhow::anyhow!(
            "failed to materialize generation {generation_number} mutable state: {error}"
        )
    })
}

pub(crate) fn publish_generation_link(db_path: &str, gen_num: i64) -> anyhow::Result<()> {
    let runtime_root = runtime_root_for_db_path(db_path);
    conary_core::generation::mount::update_current_symlink(runtime_root.root(), gen_num)
        .map_err(|e| anyhow::anyhow!("Failed to update current symlink: {e}"))?;
    Ok(())
}

pub(crate) fn mark_generation_state_active(conn: &Connection, gen_num: i64) -> anyhow::Result<()> {
    let state = SystemState::find_by_number(conn, gen_num)
        .map_err(|e| anyhow::anyhow!("Failed to load system state {gen_num}: {e}"))?
        .ok_or_else(|| {
            anyhow::anyhow!("System state {gen_num} not found after generation build")
        })?;
    state
        .set_active(conn)
        .map_err(|e| anyhow::anyhow!("Failed to mark system state {gen_num} active: {e}"))
}

pub fn rebuild_and_mount_from_installed_state(
    conn: &Connection,
    db_path: &str,
    summary: &str,
) -> anyhow::Result<i64> {
    if !GenerationPublication::pending_recoverable(conn)?.is_empty() {
        anyhow::bail!(
            "cannot rebuild from installed package state while exact selected-root publication is pending; publish the pending generation first"
        );
    }
    let transactions = Vec::new();
    let built = build_generation_from_installed_state(conn, db_path, summary, &transactions)?;
    publish_generation_link(db_path, built.generation_number)?;
    crate::commands::generation::config_transaction::project_persisted_statuses(
        conn,
        &transactions,
    )?;
    mark_generation_state_active(conn, built.generation_number)?;

    info!(
        "Generation {} built and selected for next boot",
        built.generation_number
    );
    Ok(built.generation_number)
}

fn forced_generation_rebuild_failure() -> Option<anyhow::Error> {
    std::env::var_os("CONARY_TEST_FAIL_GENERATION_REBUILD").map(|message| {
        let message = message.to_string_lossy();
        if message.is_empty() {
            anyhow::anyhow!("forced generation rebuild failure for test")
        } else {
            anyhow::anyhow!("forced generation rebuild failure for test: {message}")
        }
    })
}

#[cfg(test)]
pub(crate) struct TestMountSkipGuard {
    _guard: std::sync::MutexGuard<'static, ()>,
    previous: Option<std::ffi::OsString>,
}

#[cfg(test)]
static TEST_MOUNT_SKIP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// This variable controls behavior across the whole unit-test process. Every
// mutation must stay behind TEST_MOUNT_SKIP_LOCK.
#[cfg(test)]
pub(crate) const TEST_MOUNT_SKIP_ENV: &str = "CONARY_TEST_SKIP_GENERATION_MOUNT";

#[cfg(test)]
fn lock_test_mount_skip() -> std::sync::MutexGuard<'static, ()> {
    TEST_MOUNT_SKIP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
fn replace_test_mount_skip(value: Option<&std::ffi::OsStr>) -> Option<std::ffi::OsString> {
    let previous = std::env::var_os(TEST_MOUNT_SKIP_ENV);
    unsafe {
        if let Some(value) = value {
            std::env::set_var(TEST_MOUNT_SKIP_ENV, value);
        } else {
            std::env::remove_var(TEST_MOUNT_SKIP_ENV);
        }
    }
    previous
}

#[cfg(test)]
fn test_mount_skip_guard_for(value: Option<&std::ffi::OsStr>) -> TestMountSkipGuard {
    let guard = lock_test_mount_skip();
    let previous = replace_test_mount_skip(value);
    TestMountSkipGuard {
        _guard: guard,
        previous,
    }
}

#[cfg(test)]
pub(crate) fn test_mount_skip_guard() -> TestMountSkipGuard {
    test_mount_skip_guard_for(Some(std::ffi::OsStr::new("1")))
}

#[cfg(test)]
pub(crate) fn test_mount_skip_clear_guard() -> TestMountSkipGuard {
    test_mount_skip_guard_for(None)
}

#[cfg(test)]
impl Drop for TestMountSkipGuard {
    fn drop(&mut self) {
        replace_test_mount_skip(self.previous.as_deref());
    }
}

#[cfg(test)]
pub(crate) struct TestGenerationRebuildFailureGuard {
    _guard: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
static TEST_GENERATION_REBUILD_FAILURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn test_forced_generation_rebuild_failure_guard(
    message: &str,
) -> TestGenerationRebuildFailureGuard {
    let guard = TEST_GENERATION_REBUILD_FAILURE_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("CONARY_TEST_FAIL_GENERATION_REBUILD", message);
    }
    TestGenerationRebuildFailureGuard { _guard: guard }
}

#[cfg(test)]
impl Drop for TestGenerationRebuildFailureGuard {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("CONARY_TEST_FAIL_GENERATION_REBUILD");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    struct MountSkipEnvReset(Option<std::ffi::OsString>);

    impl MountSkipEnvReset {
        fn replace_with(value: Option<&std::ffi::OsStr>) -> Self {
            let _guard = lock_test_mount_skip();
            Self(replace_test_mount_skip(value))
        }
    }

    impl Drop for MountSkipEnvReset {
        fn drop(&mut self) {
            let _guard = lock_test_mount_skip();
            replace_test_mount_skip(self.0.as_deref());
        }
    }

    #[test]
    fn forced_generation_rebuild_failure_reads_test_env_message() {
        let _guard = test_forced_generation_rebuild_failure_guard("slice-d forced failure");

        let error = forced_generation_rebuild_failure()
            .expect("test env should force generation rebuild failure");

        assert!(error.to_string().contains("slice-d forced failure"));
    }

    #[test]
    fn verity_warning_text_is_backed_by_mount_helper() {
        use conary_core::generation::mount::{GenerationMountOutcome, verity_downgrade_warning};

        let warning = verity_downgrade_warning(
            true,
            GenerationMountOutcome::ComposefsPlain,
            Path::new("/conary/generations/7/root.erofs"),
        )
        .unwrap();
        assert!(warning.contains("downgraded"));
    }

    #[test]
    fn runtime_root_for_default_db_path_uses_boot_visible_runtime_root() {
        let runtime_root = runtime_root_for_db_path("/var/lib/conary/conary.db");

        assert_eq!(runtime_root.root(), Path::new("/conary"));
        assert_eq!(
            runtime_root.db_path(),
            Path::new("/var/lib/conary/conary.db")
        );
        assert_eq!(runtime_root.objects_dir(), PathBuf::from("/conary/objects"));
        assert_eq!(
            runtime_root.generations_dir(),
            PathBuf::from("/conary/generations")
        );
    }

    #[test]
    fn runtime_root_for_test_db_path_stays_self_contained() {
        let runtime_root = runtime_root_for_db_path("/tmp/test-conary/conary.db");

        assert_eq!(runtime_root.root(), Path::new("/tmp/test-conary"));
        assert_eq!(
            runtime_root.generations_dir(),
            PathBuf::from("/tmp/test-conary/generations")
        );
    }

    #[test]
    fn boot_root_for_generation_build_prefers_self_contained_test_boot_when_requested() {
        let _guard = test_mount_skip_guard();
        let temp = tempfile::TempDir::new().unwrap();
        let runtime_root = conary_core::runtime_root::ConaryRuntimeRoot::for_test_root(temp.path());
        std::fs::create_dir_all(temp.path().join("boot")).unwrap();

        assert_eq!(
            boot_root_for_generation_build(&runtime_root),
            temp.path().join("boot")
        );
    }

    #[test]
    fn mount_skip_guards_restore_exact_prior_state() {
        let reset = MountSkipEnvReset::replace_with(Some(std::ffi::OsStr::new("prior-test-value")));

        {
            let _guard = test_mount_skip_guard();
            assert_eq!(
                std::env::var_os(TEST_MOUNT_SKIP_ENV),
                Some(std::ffi::OsString::from("1"))
            );
        }
        {
            let _guard = lock_test_mount_skip();
            assert_eq!(
                std::env::var_os(TEST_MOUNT_SKIP_ENV),
                Some(std::ffi::OsString::from("prior-test-value"))
            );
        }

        {
            let _guard = test_mount_skip_clear_guard();
            assert_eq!(std::env::var_os(TEST_MOUNT_SKIP_ENV), None);
        }
        {
            let _guard = lock_test_mount_skip();
            assert_eq!(
                std::env::var_os(TEST_MOUNT_SKIP_ENV),
                Some(std::ffi::OsString::from("prior-test-value"))
            );
        }

        drop(reset);
    }

    #[test]
    fn mount_skip_guards_serialize_competing_mutations() {
        let first = test_mount_skip_guard();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let _second = test_mount_skip_clear_guard();
            acquired_tx
                .send(std::env::var_os(TEST_MOUNT_SKIP_ENV))
                .unwrap();
        });

        started_rx.recv().unwrap();
        assert!(matches!(
            TEST_MOUNT_SKIP_LOCK.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        assert!(matches!(
            acquired_rx.recv_timeout(std::time::Duration::from_millis(100)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));

        drop(first);
        // Serialization is what the two assertions above prove, and they hold
        // regardless of scheduling because `first` is still held. This last wait
        // only proves liveness -- the worker does eventually acquire -- so its
        // bound is an anti-hang backstop, not a contract. `TEST_MOUNT_SKIP_LOCK`
        // is a plain `std::sync::Mutex` with no fairness guarantee, and every
        // install test in this process contends for it, so the worker can be
        // passed over for as long as those tests keep the guard. Do not tighten
        // this into a latency assertion: it would fail whenever the suite grows
        // a test that holds the guard across more transactions.
        assert_eq!(
            acquired_rx
                .recv_timeout(std::time::Duration::from_secs(120))
                .unwrap(),
            None
        );
        worker.join().unwrap();
    }
}
