// apps/conary/src/commands/try_session/session/tests.rs
//! Try-session lifecycle, refresh, keep, rollback, and liveness tests.

use std::path::{Path, PathBuf};

use conary_core::ccs::manifest::CcsManifest;
use conary_core::db::models::{TrySession, TrySessionMode};
use conary_core::transaction::TransactionEngine;

use super::super::test_support::*;
use super::super::{TryRefreshRequest, TryWatchMarkerRequest};
use super::*;

#[test]
fn activated_no_command_session_records_boot_without_launcher_pid() -> anyhow::Result<()> {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-a");
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    create_current_generation_link(&fixture.root, 1);
    let package = fixture.write_package(
        "try-activated-no-command",
        CcsManifest::new_minimal("try-activated-no-command", "1.0.0"),
    );

    let outcome = begin_activated_try(&fixture, &package)?;

    let stored = stored_session(&fixture, &outcome.session_id);
    assert_eq!(stored.launcher_boot_id.as_deref(), Some("boot-a"));
    assert_eq!(stored.launcher_pid, None);
    Ok(())
}

#[test]
fn namespace_try_start_rejects_unsupported_declarative_hook_classes_before_session()
-> anyhow::Result<()> {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    for (manifest, expected) in [
        (manifest_with_systemd_hook(), "hooks.systemd"),
        (manifest_with_tmpfiles_hook(), "hooks.tmpfiles"),
        (manifest_with_sysctl_hook(), "hooks.sysctl"),
        (manifest_with_alternative_hook(), "hooks.alternatives"),
    ] {
        let fixture = TryRuntimeFixture::new();
        let package = fixture.write_package("try-unsupported-hook", manifest);

        let err = begin_namespace_try(&fixture, &package)
            .expect_err("unsupported declarative hook class should fail before session opens");
        let message = err.to_string();
        assert!(message.contains(expected), "{message}");
        assert!(message.contains("M2"), "{message}");
        assert!(
            TrySession::find_active_or_orphaned(&fixture.open())?.is_none(),
            "try start must fail before creating an open session"
        );
    }
    Ok(())
}

#[test]
fn namespace_try_start_creates_active_session_and_copied_artifact() -> anyhow::Result<()> {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    let original_package =
        fixture.write_package("try-demo", CcsManifest::new_minimal("try-demo", "1.0.0"));

    let outcome = begin_namespace_try(&fixture, &original_package)?;

    let session = stored_session(&fixture, &outcome.session_id);
    assert_eq!(
        session.status,
        conary_core::db::models::TrySessionStatus::Active
    );
    assert_eq!(session.mode, TrySessionMode::Namespace);
    assert_eq!(session.package_name.as_deref(), Some("try-demo"));
    assert_eq!(session.package_version.as_deref(), Some("1.0.0"));
    assert_eq!(
        session.package_signing_key,
        fixture.signing_key.public_key_base64()
    );
    assert_eq!(session.try_generation_id, Some(outcome.try_generation_id));
    assert_ne!(Path::new(&session.package_path), original_package.as_path());
    assert_eq!(
        Path::new(&session.package_path),
        outcome.copied_package_path
    );
    assert!(outcome.copied_package_path.exists());
    assert!(outcome.copied_db_path.exists());
    assert!(outcome.install_root.exists());
    assert!(outcome.work_dir.starts_with(fixture.root.join("try")));

    let copied = conary_core::db::open(&outcome.copied_db_path)?;
    let copied_session = TrySession::find_by_id(&copied, &outcome.session_id)?.unwrap();
    assert_eq!(
        copied_session.try_generation_id,
        Some(outcome.try_generation_id)
    );
    Ok(())
}

#[test]
fn namespace_try_start_with_active_session_errors_with_active_id() -> anyhow::Result<()> {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    let first_package =
        fixture.write_package("try-first", CcsManifest::new_minimal("try-first", "1.0.0"));
    let second_package = fixture.write_package(
        "try-second",
        CcsManifest::new_minimal("try-second", "1.0.0"),
    );
    let first = begin_namespace_try(&fixture, &first_package)?;

    let err = begin_namespace_try(&fixture, &second_package)
        .expect_err("second open try session should fail");
    let message = err.to_string();
    assert!(message.contains(&first.session_id), "{message}");
    assert!(
        message.contains("active or orphaned try session"),
        "{message}"
    );
    Ok(())
}

#[test]
fn try_generation_build_leaves_current_link_and_writes_live_runtime_artifacts() -> anyhow::Result<()>
{
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    create_current_generation_link(&fixture.root, 77);
    let before_current = std::fs::read_link(fixture.root.join("current"))?;
    let package = fixture.write_package(
        "try-artifacts",
        CcsManifest::new_minimal("try-artifacts", "1.0.0"),
    );

    let outcome = begin_namespace_try(&fixture, &package)?;

    assert_eq!(
        std::fs::read_link(fixture.root.join("current"))?,
        before_current
    );
    assert!(
        fixture
            .root
            .join(format!("generations/{}", outcome.try_generation_id))
            .join(conary_core::generation::metadata::GENERATION_METADATA_FILE)
            .exists(),
        "try generation must be built under live runtime generations/"
    );
    assert!(
        has_cas_object(&fixture.root),
        "try transaction must write CAS objects under live runtime objects/"
    );
    assert!(
        !outcome.work_dir.join("objects").exists()
            && !outcome.work_dir.join("generations").exists(),
        "throwaway work dir must not become the runtime artifact root: objects={}, generations={}",
        outcome.work_dir.join("objects").exists(),
        outcome.work_dir.join("generations").exists()
    );
    Ok(())
}

#[test]
fn activated_try_publishes_generation_records_previous_and_marks_mode() -> anyhow::Result<()> {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    create_current_generation_link(&fixture.root, 7);
    let package = fixture.write_package(
        "try-activated",
        CcsManifest::new_minimal("try-activated", "1.0.0"),
    );

    let outcome = begin_activated_try(&fixture, &package)?;

    let session = stored_session(&fixture, &outcome.session_id);
    assert_eq!(session.mode, TrySessionMode::Activated);
    assert_eq!(session.previous_generation_id, Some(7));
    assert_eq!(session.try_generation_id, Some(outcome.try_generation_id));
    assert_eq!(
        conary_core::generation::mount::current_generation(&fixture.root)?,
        Some(outcome.try_generation_id)
    );
    Ok(())
}

#[test]
fn activated_rollback_uses_copied_package_after_original_is_deleted() -> anyhow::Result<()> {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    create_current_generation_link(&fixture.root, 5);
    let package = fixture.write_package(
        "try-rollback-activated",
        CcsManifest::new_minimal("try-rollback-activated", "1.0.0"),
    );
    let outcome = begin_activated_try(&fixture, &package)?;
    std::fs::remove_file(&package)?;

    rollback_active_try_session(&fixture.db_path_string)?;

    let session = stored_session(&fixture, &outcome.session_id);
    assert_eq!(
        session.status,
        conary_core::db::models::TrySessionStatus::RolledBack
    );
    assert_eq!(
        conary_core::generation::mount::current_generation(&fixture.root)?,
        Some(5)
    );
    assert!(
        !outcome.work_dir.exists(),
        "rollback must remove try work dir"
    );
    Ok(())
}

#[test]
fn namespace_rollback_marks_rolled_back_and_removes_work_dir() -> anyhow::Result<()> {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    create_current_generation_link(&fixture.root, 2);
    let package = fixture.write_package(
        "try-rollback",
        CcsManifest::new_minimal("try-rollback", "1.0.0"),
    );
    let outcome = begin_namespace_try(&fixture, &package)?;

    rollback_active_try_session(&fixture.db_path_string)?;

    let session = stored_session(&fixture, &outcome.session_id);
    assert_eq!(
        session.status,
        conary_core::db::models::TrySessionStatus::RolledBack
    );
    assert!(
        !outcome.work_dir.exists(),
        "rollback must remove try work dir"
    );
    assert!(
        !fixture
            .root
            .join(format!("generations/{}", outcome.try_generation_id))
            .exists(),
        "unkept inactive try generation should be removed"
    );
    assert_eq!(
        conary_core::generation::mount::current_generation(&fixture.root)?,
        Some(2)
    );
    Ok(())
}

#[test]
fn namespace_rollback_leaves_session_retryable_when_work_dir_removal_fails() -> anyhow::Result<()> {
    let _env_lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    create_current_generation_link(&fixture.root, 2);
    let package = fixture.write_package(
        "try-rollback-workdir-fail",
        CcsManifest::new_minimal("try-rollback-workdir-fail", "1.0.0"),
    );
    let outcome = begin_namespace_try(&fixture, &package)?;
    let _fail_guard = EnvVarGuard::set("CONARY_TEST_TRY_REMOVE_DIR_FAIL", &outcome.work_dir);

    let err = rollback_active_try_session(&fixture.db_path_string)
        .expect_err("rollback should fail before marking rolled_back when work dir cleanup fails");
    let message = format!("{err:#}");
    assert!(
        message.contains("forced try directory removal failure"),
        "{message}"
    );
    assert!(
        message.contains(&outcome.work_dir.display().to_string()),
        "{message}"
    );
    assert_eq!(
        stored_session(&fixture, &outcome.session_id).status,
        conary_core::db::models::TrySessionStatus::Active
    );
    assert!(
        outcome.work_dir.exists(),
        "failed cleanup must leave work dir for retry"
    );
    Ok(())
}

#[test]
fn namespace_watch_start_writes_marker_before_session_is_keepable() -> anyhow::Result<()> {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    let package = fixture.write_package(
        "watch-demo",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.0"),
    );

    let outcome = begin_try_session(TryStartRequest {
        db_path: &fixture.db_path_string,
        package_path: &package,
        trust_policy: &fixture.trust_policy,
        activate: false,
        command: None,
        watch_marker: Some(TryWatchMarkerRequest {
            operation_id: "watch-1",
        }),
    })?;

    let marker = outcome.work_dir.join(".conary-try-watch-session.json");
    let marker_text = std::fs::read_to_string(&marker)?;
    assert!(
        marker_text.contains("\"operation_id\":\"watch-1\""),
        "{marker_text}"
    );

    let err = keep_active_try_session(&fixture.db_path_string).unwrap_err();
    assert!(
        err.to_string().contains("watch-created try session"),
        "{err:#}"
    );
    Ok(())
}

#[test]
fn watch_marker_write_failure_does_not_leave_active_session() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    let package = fixture.write_package(
        "watch-demo",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.0"),
    );

    let _guard = EnvVarGuard::set("CONARY_TEST_TRY_WATCH_MARKER_FAIL", "1");
    let err = begin_try_session(TryStartRequest {
        db_path: &fixture.db_path_string,
        package_path: &package,
        trust_policy: &fixture.trust_policy,
        activate: false,
        command: None,
        watch_marker: Some(TryWatchMarkerRequest {
            operation_id: "watch-1",
        }),
    })
    .unwrap_err();

    assert!(
        err.to_string().contains("failed to write try watch marker"),
        "{err:#}"
    );
    let conn = fixture.open();
    assert!(
        TrySession::find_active_or_orphaned(&conn)
            .unwrap()
            .is_none()
    );
}

#[test]
fn refresh_try_session_updates_generation_after_staging_succeeds() -> anyhow::Result<()> {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    let first = fixture.write_package(
        "watch-demo-a",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.0"),
    );
    let second = fixture.write_package(
        "watch-demo-b",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.1"),
    );
    let started = begin_try_session(TryStartRequest {
        db_path: &fixture.db_path_string,
        package_path: &first,
        trust_policy: &fixture.trust_policy,
        activate: false,
        command: None,
        watch_marker: Some(TryWatchMarkerRequest {
            operation_id: "watch-1",
        }),
    })?;

    let refreshed = refresh_try_session(TryRefreshRequest {
        db_path: &fixture.db_path_string,
        session_id: &started.session_id,
        expected_try_generation_id: started.try_generation_id,
        package_path: &second,
        trust_policy: &fixture.trust_policy,
    })?;

    assert_eq!(refreshed.previous_generation_id, started.try_generation_id);
    assert!(refreshed.try_generation_id > started.try_generation_id);
    assert_eq!(refreshed.cleanup_error, None);
    let conn = fixture.open();
    let session = TrySession::find_by_id(&conn, &started.session_id)?.unwrap();
    assert_eq!(session.try_generation_id, Some(refreshed.try_generation_id));
    assert_eq!(Path::new(&session.work_dir), started.work_dir);
    Ok(())
}

#[test]
fn refresh_missing_session_is_typed_divergence_before_package_access() {
    let fixture = TryRuntimeFixture::new();
    let unused_package = fixture.root.join("unused.ccs");

    let err = refresh_try_session(TryRefreshRequest {
        db_path: &fixture.db_path_string,
        session_id: "missing-watch-session",
        expected_try_generation_id: 41,
        package_path: &unused_package,
        trust_policy: &fixture.trust_policy,
    })
    .unwrap_err();

    assert!(err.is::<TryRefreshSessionDiverged>(), "{err:#}");
    assert!(!unused_package.exists());
}

#[test]
fn refresh_try_session_cas_miss_preserves_previous_generation() -> anyhow::Result<()> {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    let first = fixture.write_package(
        "watch-demo-a",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.0"),
    );
    let second = fixture.write_package(
        "watch-demo-b",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.1"),
    );
    let started = begin_try_session(TryStartRequest {
        db_path: &fixture.db_path_string,
        package_path: &first,
        trust_policy: &fixture.trust_policy,
        activate: false,
        command: None,
        watch_marker: Some(TryWatchMarkerRequest {
            operation_id: "watch-1",
        }),
    })?;
    {
        let conn = fixture.open();
        let session = TrySession::find_by_id(&conn, &started.session_id)?.unwrap();
        session.mark_orphaned(&conn)?;
    }

    let err = refresh_try_session(TryRefreshRequest {
        db_path: &fixture.db_path_string,
        session_id: &started.session_id,
        expected_try_generation_id: started.try_generation_id,
        package_path: &second,
        trust_policy: &fixture.trust_policy,
    })
    .unwrap_err();

    assert!(err.is::<TryRefreshSessionDiverged>(), "{err:#}");
    let conn = fixture.open();
    let session = TrySession::find_by_id(&conn, &started.session_id)?.unwrap();
    assert_eq!(session.try_generation_id, Some(started.try_generation_id));
    assert_eq!(
        session.status,
        conary_core::db::models::TrySessionStatus::Orphaned
    );
    Ok(())
}

#[test]
fn refresh_try_session_cleans_staging_after_generation_build_failure() -> anyhow::Result<()> {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    let first = fixture.write_package(
        "watch-demo-a",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.0"),
    );
    let second = fixture.write_package(
        "watch-demo-b",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.1"),
    );
    let started = begin_try_session(TryStartRequest {
        db_path: &fixture.db_path_string,
        package_path: &first,
        trust_policy: &fixture.trust_policy,
        activate: false,
        command: None,
        watch_marker: Some(TryWatchMarkerRequest {
            operation_id: "watch-1",
        }),
    })?;
    let _guard = EnvVarGuard::set(
        "CONARY_TEST_FAIL_GENERATION_REBUILD",
        "forced watch refresh generation failure",
    );

    let err = refresh_try_session(TryRefreshRequest {
        db_path: &fixture.db_path_string,
        session_id: &started.session_id,
        expected_try_generation_id: started.try_generation_id,
        package_path: &second,
        trust_policy: &fixture.trust_policy,
    })
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("forced watch refresh generation failure"),
        "{err:#}"
    );
    let conn = fixture.open();
    let session = TrySession::find_by_id(&conn, &started.session_id)?.unwrap();
    assert_eq!(session.try_generation_id, Some(started.try_generation_id));
    assert!(
        std::fs::read_dir(&started.work_dir)?
            .filter_map(|entry| entry.ok())
            .all(|entry| !entry.file_name().to_string_lossy().starts_with("refresh-")),
        "failed refresh staging directory should be cleaned"
    );
    Ok(())
}

#[test]
fn refresh_try_session_namespace_switch_failure_preserves_stable_files() -> anyhow::Result<()> {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    let first = fixture.write_package(
        "watch-demo-a",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.0"),
    );
    let second = fixture.write_package(
        "watch-demo-b",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.1"),
    );
    let started = begin_try_session(TryStartRequest {
        db_path: &fixture.db_path_string,
        package_path: &first,
        trust_policy: &fixture.trust_policy,
        activate: false,
        command: None,
        watch_marker: Some(TryWatchMarkerRequest {
            operation_id: "watch-1",
        }),
    })?;
    let stable_package_path = started.work_dir.join("package.ccs");
    let stable_db_path = started.work_dir.join("conary.db");
    let stable_package_before = std::fs::read(&stable_package_path)?;
    let stable_db_before = std::fs::read(&stable_db_path)?;
    let namespace_before = std::fs::read_link(started.work_dir.join("namespace-root"))?;
    let _fail_guard = EnvVarGuard::set("CONARY_TEST_TRY_REFRESH_FAIL_NAMESPACE_SWITCH", "1");

    let err = refresh_try_session(TryRefreshRequest {
        db_path: &fixture.db_path_string,
        session_id: &started.session_id,
        expected_try_generation_id: started.try_generation_id,
        package_path: &second,
        trust_policy: &fixture.trust_policy,
    })
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("failed to switch stable try namespace"),
        "{err:#}"
    );
    let conn = fixture.open();
    let session = TrySession::find_by_id(&conn, &started.session_id)?.unwrap();
    assert_eq!(session.try_generation_id, Some(started.try_generation_id));
    assert_eq!(std::fs::read(&stable_package_path)?, stable_package_before);
    assert_eq!(std::fs::read(&stable_db_path)?, stable_db_before);
    assert_eq!(
        std::fs::read_link(started.work_dir.join("namespace-root"))?,
        namespace_before
    );
    Ok(())
}

#[test]
fn refresh_try_session_reports_committed_cleanup_failure_with_new_generation_active()
-> anyhow::Result<()> {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    let first = fixture.write_package(
        "watch-demo-a",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.0"),
    );
    let second = fixture.write_package(
        "watch-demo-b",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.1"),
    );
    let started = begin_try_session(TryStartRequest {
        db_path: &fixture.db_path_string,
        package_path: &first,
        trust_policy: &fixture.trust_policy,
        activate: false,
        command: None,
        watch_marker: Some(TryWatchMarkerRequest {
            operation_id: "watch-1",
        }),
    })?;
    let _cleanup_guard = EnvVarGuard::set(
        "CONARY_TEST_TRY_REFRESH_FAIL_NAMESPACE_COMMIT_CLEANUP",
        "forced cleanup failure",
    );

    let refreshed = refresh_try_session(TryRefreshRequest {
        db_path: &fixture.db_path_string,
        session_id: &started.session_id,
        expected_try_generation_id: started.try_generation_id,
        package_path: &second,
        trust_policy: &fixture.trust_policy,
    })?;

    assert!(
        refreshed
            .cleanup_error
            .as_deref()
            .unwrap_or("")
            .contains("forced cleanup failure")
    );
    let conn = fixture.open();
    let session = TrySession::find_by_id(&conn, &started.session_id)?.unwrap();
    assert_eq!(session.try_generation_id, Some(refreshed.try_generation_id));
    assert!(refreshed.try_generation_id > started.try_generation_id);
    Ok(())
}

#[test]
fn namespace_keep_publishes_try_generation_and_marks_kept() -> anyhow::Result<()> {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    let package = fixture.write_package("try-keep", CcsManifest::new_minimal("try-keep", "1.0.0"));
    let outcome = begin_namespace_try(&fixture, &package)?;

    keep_active_try_session(&fixture.db_path_string)?;

    let session = stored_session(&fixture, &outcome.session_id);
    assert_eq!(
        session.status,
        conary_core::db::models::TrySessionStatus::Kept
    );
    assert_eq!(
        conary_core::generation::mount::current_generation(&fixture.root)?,
        Some(outcome.try_generation_id)
    );
    let installed: String = fixture.open().query_row(
        "SELECT name FROM troves WHERE name = 'try-keep'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(installed, "try-keep");
    Ok(())
}

#[test]
fn namespace_keep_removes_stale_sidecars_before_promoted_db_reopen() -> anyhow::Result<()> {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    let package = fixture.write_package(
        "try-keep-sidecars",
        CcsManifest::new_minimal("try-keep-sidecars", "1.0.0"),
    );
    let outcome = begin_namespace_try(&fixture, &package)?;
    std::fs::write(sqlite_sidecar_path(&outcome.copied_db_path, "-wal"), b"")?;
    std::fs::write(sqlite_sidecar_path(&outcome.copied_db_path, "-shm"), b"")?;
    std::fs::write(sqlite_sidecar_path(&fixture.db_path, "-wal"), b"")?;
    std::fs::write(sqlite_sidecar_path(&fixture.db_path, "-shm"), b"")?;

    keep_active_try_session(&fixture.db_path_string)?;

    assert!(!sqlite_sidecar_path(&outcome.copied_db_path, "-wal").exists());
    assert!(!sqlite_sidecar_path(&outcome.copied_db_path, "-shm").exists());
    assert!(!sqlite_sidecar_path(&fixture.db_path, "-wal").exists());
    assert!(!sqlite_sidecar_path(&fixture.db_path, "-shm").exists());
    assert!(conary_core::db::open(&fixture.db_path).is_ok());
    Ok(())
}

#[test]
fn db_promotion_syncs_parent_after_quarantine_and_final_rename() -> anyhow::Result<()> {
    let _env_lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir()?;
    let live_db = temp.path().join("conary.db");
    let copied_db = temp.path().join("try/conary.db");
    let sync_log = temp.path().join("sync-parent.log");
    std::fs::create_dir_all(copied_db.parent().unwrap())?;
    std::fs::write(&live_db, b"live")?;
    std::fs::write(sqlite_sidecar_path(&live_db, "-wal"), b"wal")?;
    std::fs::write(sqlite_sidecar_path(&live_db, "-shm"), b"shm")?;
    std::fs::write(&copied_db, b"copy")?;
    let _sync_guard = EnvVarGuard::set("CONARY_TEST_TRY_SYNC_PARENT_LOG", &sync_log);

    replace_live_db_with_session_copy(&live_db, &copied_db)?;

    assert_eq!(std::fs::read(&live_db)?, b"copy");
    assert_eq!(std::fs::read(&copied_db)?, b"copy");
    let synced = std::fs::read_to_string(sync_log)?
        .lines()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    assert_eq!(synced.len(), 4, "{synced:?}");
    let synced_names = synced
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(synced_names[0].starts_with("conary.db.try-promote-"));
    assert!(synced_names[0].ends_with(".old"));
    assert!(synced_names[1].starts_with("conary.db-wal.try-promote-"));
    assert!(synced_names[1].ends_with(".old"));
    assert!(synced_names[2].starts_with("conary.db-shm.try-promote-"));
    assert!(synced_names[2].ends_with(".old"));
    assert_eq!(synced[3], live_db);
    Ok(())
}

#[test]
fn namespace_keep_holds_runtime_lock_until_session_is_marked() -> anyhow::Result<()> {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    let package = fixture.write_package(
        "try-keep-lock",
        CcsManifest::new_minimal("try-keep-lock", "1.0.0"),
    );
    let outcome = begin_namespace_try(&fixture, &package)?;

    keep_active_try_session_with_probe(&fixture.db_path_string, || {
        let mut config =
            build_try_transaction_config(&fixture.runtime_root(), fixture.db_path.clone());
        config.lock_timeout_secs = 0;
        let mut engine = TransactionEngine::new(config).unwrap();
        assert!(
            engine.begin().is_err(),
            "namespace keep must still hold the live runtime lock while marking the session"
        );
    })?;

    assert_eq!(
        stored_session(&fixture, &outcome.session_id).status,
        conary_core::db::models::TrySessionStatus::Kept
    );
    Ok(())
}

#[test]
fn activated_keep_holds_runtime_lock_while_marking_kept() -> anyhow::Result<()> {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    create_current_generation_link(&fixture.root, 11);
    let package = fixture.write_package(
        "try-activated-keep-lock",
        CcsManifest::new_minimal("try-activated-keep-lock", "1.0.0"),
    );
    let outcome = begin_activated_try(&fixture, &package)?;

    keep_active_try_session_with_probe(&fixture.db_path_string, || {
        let mut config =
            build_try_transaction_config(&fixture.runtime_root(), fixture.db_path.clone());
        config.lock_timeout_secs = 0;
        let mut engine = TransactionEngine::new(config).unwrap();
        assert!(
            engine.begin().is_err(),
            "activated keep must hold the runtime mutation lock while marking the session"
        );
    })?;

    assert_eq!(
        stored_session(&fixture, &outcome.session_id).status,
        conary_core::db::models::TrySessionStatus::Kept
    );
    Ok(())
}

#[test]
fn namespace_keep_restores_live_db_after_post_backup_failure() -> anyhow::Result<()> {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let _fail_guard =
        EnvVarGuard::set("CONARY_TEST_TRY_KEEP_FAIL_AFTER_BACKUP", "after-db-promote");
    let fixture = TryRuntimeFixture::new();
    let package = fixture.write_package(
        "try-restore-live-db",
        CcsManifest::new_minimal("try-restore-live-db", "1.0.0"),
    );
    let outcome = begin_namespace_try(&fixture, &package)?;

    let err = keep_active_try_session(&fixture.db_path_string)
        .expect_err("forced post-backup failure should abort keep");
    let error_chain = format!("{err:#}");
    assert!(
        error_chain.contains("forced try keep failure"),
        "{error_chain}"
    );
    assert!(
        error_chain.contains("restored live DB checkpoint"),
        "{error_chain}"
    );

    let conn = fixture.open();
    let installed_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM troves WHERE name = 'try-restore-live-db'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(installed_count, 0, "live DB must be restored from backup");
    let session = TrySession::find_by_id(&conn, &outcome.session_id)?.unwrap();
    assert_eq!(
        session.status,
        conary_core::db::models::TrySessionStatus::Active
    );
    Ok(())
}

#[test]
fn namespace_keep_restores_current_link_after_post_link_failure() -> anyhow::Result<()> {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fail_guard = EnvVarGuard::set(
        "CONARY_TEST_TRY_KEEP_FAIL_AFTER_BACKUP",
        "after-current-link",
    );
    let fixture = TryRuntimeFixture::new();
    create_current_generation_link(&fixture.root, 7);
    let package = fixture.write_package(
        "try-restore-current-link",
        CcsManifest::new_minimal("try-restore-current-link", "1.0.0"),
    );
    let outcome = begin_namespace_try(&fixture, &package)?;
    assert_ne!(
        conary_core::generation::mount::current_generation(&fixture.root)?,
        Some(outcome.try_generation_id)
    );

    let err = keep_active_try_session(&fixture.db_path_string)
        .expect_err("forced post-link failure should abort keep");
    let error_chain = format!("{err:#}");
    assert!(
        error_chain.contains("forced try keep failure"),
        "{error_chain}"
    );
    assert!(
        error_chain.contains("restored live DB checkpoint"),
        "{error_chain}"
    );

    assert_eq!(
        conary_core::generation::mount::current_generation(&fixture.root)?,
        Some(7),
        "current generation link must be restored after post-link keep failure"
    );
    let conn = fixture.open();
    let installed_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM troves WHERE name = 'try-restore-current-link'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(installed_count, 0);
    assert_eq!(
        stored_session(&fixture, &outcome.session_id).status,
        conary_core::db::models::TrySessionStatus::Active
    );
    assert!(
        outcome.copied_db_path.exists(),
        "failed keep must preserve copied DB so the session can be retried"
    );

    drop(fail_guard);
    keep_active_try_session(&fixture.db_path_string)?;

    assert_eq!(
        conary_core::generation::mount::current_generation(&fixture.root)?,
        Some(outcome.try_generation_id),
        "retrying keep after a recovered failure should promote the try generation"
    );
    assert_eq!(
        stored_session(&fixture, &outcome.session_id).status,
        conary_core::db::models::TrySessionStatus::Kept
    );
    Ok(())
}

#[test]
fn namespace_keep_restores_current_link_even_when_db_restore_fails() -> anyhow::Result<()> {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let _fail_guard = EnvVarGuard::set(
        "CONARY_TEST_TRY_KEEP_FAIL_AFTER_BACKUP",
        "after-current-link",
    );
    let _restore_fail_guard = EnvVarGuard::set("CONARY_TEST_TRY_RESTORE_DB_FAIL", "1");
    let fixture = TryRuntimeFixture::new();
    create_current_generation_link(&fixture.root, 7);
    let package = fixture.write_package(
        "try-restore-link-after-db-restore-fails",
        CcsManifest::new_minimal("try-restore-link-after-db-restore-fails", "1.0.0"),
    );
    let outcome = begin_namespace_try(&fixture, &package)?;

    let err = keep_active_try_session(&fixture.db_path_string)
        .expect_err("forced DB restore failure should keep promotion failed");
    let error_chain = format!("{err:#}");
    assert!(
        error_chain.contains("failed to restore live DB checkpoint"),
        "{error_chain}"
    );

    assert_eq!(
        conary_core::generation::mount::current_generation(&fixture.root)?,
        Some(7),
        "current generation link must be restored even when DB checkpoint restore fails"
    );
    assert!(
        outcome.copied_db_path.exists(),
        "failed keep must preserve copied DB even when DB restore fails"
    );
    Ok(())
}

#[test]
fn namespace_keep_fails_when_declarative_hook_effect_is_not_promotable() -> anyhow::Result<()> {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    let package = fixture.write_package("try-hook-verify", manifest_with_declarative_hook());
    let outcome = begin_namespace_try(&fixture, &package)?;
    std::fs::remove_dir_all(fixture.root.join(format!(
        "etc-state/{}/var/lib/declarative",
        outcome.try_generation_id
    )))?;

    let err = keep_active_try_session(&fixture.db_path_string)
        .expect_err("keep should reject missing promotable hook effects");
    let message = err.to_string();
    assert!(message.contains("hook effects"), "{message}");
    assert!(message.contains("rollback"), "{message}");
    assert_eq!(
        stored_session(&fixture, &outcome.session_id).status,
        conary_core::db::models::TrySessionStatus::Active
    );
    Ok(())
}

#[test]
fn keep_time_hook_verification_checks_user_group_effects() -> anyhow::Result<()> {
    let fixture = TryRuntimeFixture::new();
    let runtime_root = fixture.runtime_root();
    let package = fixture.write_package("try-user-group-verify", manifest_with_user_group_hooks());
    let package_path = package.to_string_lossy().into_owned();
    let session = TrySession {
        id: "try-user-group-session".to_string(),
        package_path,
        package_signing_key: fixture.signing_key.public_key_base64(),
        package_name: Some("user-group-hooks".to_string()),
        package_version: Some("1.0.0".to_string()),
        previous_generation_id: None,
        try_generation_id: Some(42),
        launcher_pid: None,
        launcher_boot_id: None,
        status: conary_core::db::models::TrySessionStatus::Active,
        mode: TrySessionMode::Namespace,
        work_dir: fixture.root.join("try/work").to_string_lossy().into_owned(),
        last_error: None,
        started_at: None,
        updated_at: None,
        completed_at: None,
    };

    let err = verify_namespace_try_hook_effects(&session, &runtime_root, 42)
        .expect_err("missing user/group hook effects should fail keep verification");
    let message = err.to_string();
    assert!(message.contains("hook effects"), "{message}");
    assert!(message.contains("rollback"), "{message}");

    let etc = fixture.root.join("etc-state/42/etc");
    std::fs::create_dir_all(&etc)?;
    std::fs::write(etc.join("group"), "trygroup:x:999:\n")?;
    std::fs::write(
        etc.join("passwd"),
        "tryuser:x:999:999::/nonexistent:/usr/sbin/nologin\n",
    )?;

    verify_namespace_try_hook_effects(&session, &runtime_root, 42)?;
    Ok(())
}

#[test]
fn proc_liveness_probe_rejects_non_positive_pids() {
    assert!(!Path::new("/proc/0").exists() || !try_launcher_pid_is_alive(0));
    assert!(!try_launcher_pid_is_alive(-1));
}
