// apps/conary/src/commands/try_session/namespace/tests.rs
//! Tests for try-session namespace materialization and declarative hooks.

use std::path::{Path, PathBuf};

use conary_core::ccs::manifest::{
    AlternativeHook, CcsManifest, DirectoryHook, SysctlHook, UserHook,
};

use super::super::test_support::*;
use super::super::{TryStartRequest, begin_try_session, rollback_active_try_session};
use super::*;

#[test]
fn declarative_try_hooks_refuse_host_root() {
    let manifest = manifest_with_declarative_hook();

    let err = apply_declarative_try_hooks(&manifest, Path::new("/"))
        .expect_err("try hooks must not run against host root");

    assert!(err.to_string().contains("host root"));
}

#[test]
fn declarative_try_hooks_abort_post_hooks_when_pre_hooks_fail() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let mut manifest = CcsManifest::new_minimal("bad-pre-hook", "1.0.0");
    manifest.hooks.users.push(UserHook {
        name: "BadName!".to_string(),
        system: true,
        home: None,
        shell: Some("/usr/sbin/nologin".to_string()),
        group: None,
        reversible: None,
    });
    manifest.hooks.sysctl.push(SysctlHook {
        key: "kernel.modules_disabled".to_string(),
        value: "1".to_string(),
        reversible: None,
    });

    let err = apply_declarative_try_hooks(&manifest, temp.path())
        .expect_err("pre-hook failure should abort try hook execution");
    let message = format!("{err:#}");

    assert!(
        message.contains("failed to execute try declarative pre-hooks"),
        "{message}"
    );
    assert!(
        !temp.path().join("etc/sysctl.d").exists(),
        "post-hook sysctl config must not be written after pre-hook failure"
    );
    Ok(())
}

#[test]
fn declarative_try_hooks_collect_post_hook_failures() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let mut manifest = CcsManifest::new_minimal("bad-post-hooks", "1.0.0");
    manifest.hooks.sysctl.push(SysctlHook {
        key: "kernel.modules_disabled".to_string(),
        value: "1".to_string(),
        reversible: None,
    });
    manifest.hooks.alternatives.push(AlternativeHook {
        link: "/usr/bin/demo".to_string(),
        name: "bad/name".to_string(),
        path: "/usr/bin/demo".to_string(),
        priority: 50,
        reversible: None,
    });

    let executor = conary_core::ccs::HookExecutor::new(temp.path(), Default::default());
    let results = executor.execute_post_hooks_with_results(&manifest.hooks);
    let failures = results.failures().collect::<Vec<_>>();
    assert_eq!(failures.len(), 1);
    assert_eq!(
        failures[0].hook_type,
        conary_core::ccs::HookType::Capability
    );
    assert_eq!(failures[0].name, "host-integration");
    assert!(
        failures[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("Alternative name contains invalid characters"))
    );

    let err = apply_declarative_try_hooks(&manifest, temp.path())
        .expect_err("post-hook failures should be collected");
    let message = format!("{err:#}");

    assert!(
        message.contains("failed to execute try declarative post-hooks"),
        "{message}"
    );
    assert!(
        message.contains("capability 'host-integration' failed"),
        "{message}"
    );
    assert!(
        message.contains("Alternative name contains invalid characters: bad/name"),
        "{message}"
    );
    Ok(())
}

#[test]
fn namespace_declarative_hooks_write_to_live_etc_state_not_workdir() -> anyhow::Result<()> {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    let package = fixture.write_package("try-hooks", manifest_with_declarative_hook());

    let outcome = begin_namespace_try(&fixture, &package)?;

    assert!(
        fixture
            .root
            .join(format!(
                "etc-state/{}/var/lib/declarative",
                outcome.try_generation_id
            ))
            .is_dir(),
        "declarative hook effects must land in live etc-state upperdir"
    );
    assert!(
        !outcome.work_dir.join("root/var/lib/declarative").exists(),
        "throwaway install scratch root must not be the only hook effect location"
    );
    Ok(())
}

#[test]
fn namespace_command_sees_generation_files_and_hook_upperdir() -> anyhow::Result<()> {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir()?;
    let launcher = temp.path().join("launcher.sh");
    let seen_root = temp.path().join("seen-root");
    std::fs::write(
        &launcher,
        "#!/bin/sh\nroot=\"$1\"\nif [ ! -f \"$root/usr/bin/try-launch-root\" ]; then echo missing package file >&2; exit 43; fi\nif [ ! -d \"$root/var/lib/declarative\" ]; then echo missing hook dir >&2; exit 44; fi\nprintf '%s\\n' \"$root\" > \"$TRY_SEEN_ROOT_FILE\"\n",
    )?;
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&launcher)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&launcher, permissions)?;
    }
    let _launcher_guard = EnvVarGuard::set("CONARY_TEST_TRY_LAUNCHER", &launcher);
    let _seen_guard = EnvVarGuard::set("TRY_SEEN_ROOT_FILE", &seen_root);
    let fixture = TryRuntimeFixture::new();
    let mut manifest = CcsManifest::new_minimal("try-launch-root", "1.0.0");
    manifest.hooks.directories.push(DirectoryHook {
        path: "/var/lib/declarative".to_string(),
        mode: "0755".to_string(),
        owner: "root".to_string(),
        group: "root".to_string(),
        cleanup: None,
        reversible: None,
    });
    let package = fixture.write_package("try-launch-root", manifest);
    let command = ["/usr/bin/try-launch-root"];

    let outcome = begin_try_session(TryStartRequest {
        db_path: &fixture.db_path_string,
        package_path: &package,
        trust_policy: &fixture.trust_policy,
        activate: false,
        command: Some(&command),
        watch_marker: None,
    })?;

    let launcher_root = PathBuf::from(std::fs::read_to_string(seen_root)?.trim());
    assert_eq!(launcher_root, outcome.namespace_root);
    assert_ne!(outcome.namespace_root, outcome.install_root);
    assert!(
        outcome
            .namespace_root
            .join("usr/bin/try-launch-root")
            .is_file(),
        "namespace root must expose installed package files"
    );
    assert!(
        fixture
            .root
            .join(format!(
                "etc-state/{}/var/lib/declarative",
                outcome.try_generation_id
            ))
            .is_dir(),
        "hook writes must land in the live etc-state upperdir"
    );
    Ok(())
}

#[test]
fn activated_declarative_hooks_use_promotable_etc_state_before_publish() -> anyhow::Result<()> {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    create_current_generation_link(&fixture.root, 3);
    let package = fixture.write_package("try-activated-hooks", manifest_with_declarative_hook());

    let outcome = begin_activated_try(&fixture, &package)?;

    assert!(
        fixture
            .root
            .join(format!(
                "etc-state/{}/var/lib/declarative",
                outcome.try_generation_id
            ))
            .is_dir(),
        "activated declarative hooks must use the promotable generation upperdir"
    );
    assert_eq!(
        conary_core::generation::mount::current_generation(&fixture.root)?,
        Some(outcome.try_generation_id)
    );
    Ok(())
}

#[test]
fn namespace_rollback_unmounts_namespace_before_generation_root() -> anyhow::Result<()> {
    let _env_lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    create_current_generation_link(&fixture.root, 2);
    let package = fixture.write_package(
        "try-rollback-unmount",
        CcsManifest::new_minimal("try-rollback-unmount", "1.0.0"),
    );
    let outcome = begin_namespace_try(&fixture, &package)?;
    let mountinfo = fixture.root.join("try-mountinfo");
    let unmount_log = fixture.root.join("try-unmount.log");
    let namespace_root = outcome.work_dir.join("namespace-root");
    let generation_root = outcome.work_dir.join("generation-root");
    write_try_mountinfo(&mountinfo, &[&namespace_root, &generation_root])?;
    let _mountinfo_guard = EnvVarGuard::set("CONARY_TEST_TRY_MOUNTINFO_PATH", &mountinfo);
    let _unmount_guard = EnvVarGuard::set("CONARY_TEST_TRY_UMOUNT_LOG", &unmount_log);

    rollback_active_try_session(&fixture.db_path_string)?;

    let unmounted = std::fs::read_to_string(unmount_log)?
        .lines()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    assert_eq!(unmounted, vec![namespace_root, generation_root]);
    assert_eq!(
        stored_session(&fixture, &outcome.session_id).status,
        conary_core::db::models::TrySessionStatus::RolledBack
    );
    assert!(
        !outcome.work_dir.exists(),
        "rollback must remove try work dir after unmounting"
    );
    Ok(())
}

#[test]
fn namespace_rollback_leaves_session_retryable_when_unmount_fails() -> anyhow::Result<()> {
    let _env_lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let fixture = TryRuntimeFixture::new();
    create_current_generation_link(&fixture.root, 2);
    let package = fixture.write_package(
        "try-rollback-unmount-fail",
        CcsManifest::new_minimal("try-rollback-unmount-fail", "1.0.0"),
    );
    let outcome = begin_namespace_try(&fixture, &package)?;
    let mountinfo = fixture.root.join("try-mountinfo");
    let unmount_log = fixture.root.join("try-unmount.log");
    let namespace_root = outcome.work_dir.join("namespace-root");
    let generation_root = outcome.work_dir.join("generation-root");
    write_try_mountinfo(&mountinfo, &[&namespace_root, &generation_root])?;
    let _mountinfo_guard = EnvVarGuard::set("CONARY_TEST_TRY_MOUNTINFO_PATH", &mountinfo);
    let _unmount_guard = EnvVarGuard::set("CONARY_TEST_TRY_UMOUNT_LOG", &unmount_log);
    let _fail_guard = EnvVarGuard::set("CONARY_TEST_TRY_UMOUNT_FAIL", &namespace_root);

    let err = rollback_active_try_session(&fixture.db_path_string)
        .expect_err("rollback should fail before marking rolled_back when unmount fails");
    let message = format!("{err:#}");
    assert!(
        message.contains("forced try namespace unmount failure"),
        "{message}"
    );
    assert!(message.contains("namespace-root"), "{message}");
    assert_eq!(
        stored_session(&fixture, &outcome.session_id).status,
        conary_core::db::models::TrySessionStatus::Active
    );
    assert!(
        outcome.work_dir.exists(),
        "failed cleanup must leave work dir for retry"
    );
    assert!(
        fixture
            .root
            .join(format!("generations/{}", outcome.try_generation_id))
            .exists(),
        "failed cleanup must leave generation artifacts for retry"
    );
    Ok(())
}

#[test]
fn switch_stable_namespace_root_restores_previous_on_forced_failure() -> anyhow::Result<()> {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir()?;
    let stable = temp.path().join("namespace-root");
    let previous = temp.path().join("previous-root");
    let staged = temp.path().join("namespace-root.next");
    std::fs::create_dir_all(&previous)?;
    std::fs::create_dir_all(staged.parent().unwrap())?;
    std::fs::create_dir_all(&staged)?;
    std::fs::write(previous.join("marker"), "old")?;
    std::fs::write(staged.join("marker"), "new")?;
    recreate_path_symlink(&previous, &stable)?;

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let _fail_guard = EnvVarGuard::set("CONARY_TEST_TRY_REFRESH_FAIL_NAMESPACE_SWITCH", "1");
    let exposure = StagedNamespaceExposure {
        generation_id: 2,
        next_namespace_root: staged,
        stable_namespace_root: stable.clone(),
        previous_namespace_root: temp.path().join("namespace-root.previous"),
        generation_root: temp.path().join("generation-root-2"),
        namespace_workdir: temp.path().join("namespace-work-2"),
    };
    let err = switch_stable_namespace_root(exposure, 1).unwrap_err();

    assert!(
        err.to_string()
            .contains("failed to switch stable try namespace"),
        "{err:#}"
    );
    assert_eq!(std::fs::read_link(&stable)?, previous);
    Ok(())
}

#[test]
fn teardown_try_namespace_mounts_removes_watch_generation_paths() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    for name in [
        "namespace-root.next",
        "namespace-root.previous",
        "generation-root-41",
        "namespace-work-41",
        "generation-root",
        "namespace-work",
    ] {
        std::fs::create_dir_all(temp.path().join(name))?;
    }

    teardown_try_namespace_mounts(temp.path())?;

    for name in [
        "namespace-root.next",
        "namespace-root.previous",
        "generation-root-41",
        "namespace-work-41",
        "generation-root",
        "namespace-work",
    ] {
        assert!(!temp.path().join(name).exists(), "{name} should be removed");
    }
    Ok(())
}

#[test]
fn namespace_switch_commit_removes_superseded_generation_paths() -> anyhow::Result<()> {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir()?;
    let stable = temp.path().join("namespace-root");
    let previous = temp.path().join("previous-root");
    let staged = temp.path().join("namespace-root.next");
    std::fs::create_dir_all(&previous)?;
    std::fs::create_dir_all(&staged)?;
    recreate_path_symlink(&previous, &stable)?;
    for name in [
        "generation-root-1",
        "namespace-work-1",
        "generation-root",
        "namespace-work",
    ] {
        std::fs::create_dir_all(temp.path().join(name))?;
    }

    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let exposure = StagedNamespaceExposure {
        generation_id: 2,
        next_namespace_root: staged,
        stable_namespace_root: stable,
        previous_namespace_root: temp.path().join("namespace-root.previous"),
        generation_root: temp.path().join("generation-root-2"),
        namespace_workdir: temp.path().join("namespace-work-2"),
    };

    let switch = switch_stable_namespace_root(exposure, 1)?;
    switch.commit()?;

    for name in [
        "generation-root-1",
        "namespace-work-1",
        "generation-root",
        "namespace-work",
    ] {
        assert!(!temp.path().join(name).exists(), "{name} should be removed");
    }
    Ok(())
}
