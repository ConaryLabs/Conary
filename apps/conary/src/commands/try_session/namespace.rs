// apps/conary/src/commands/try_session/namespace.rs
//! Try-session namespace root exposure, mounts, and declarative hook execution.

use anyhow::{Context, Result, bail};
use conary_core::ccs::manifest::CcsManifest;
use conary_core::db::models::FileEntry;
use conary_core::runtime_root::ConaryRuntimeRoot;
use rusqlite::Connection;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use super::util::remove_path_if_exists;

pub(super) fn apply_declarative_try_hooks(manifest: &CcsManifest, root: &Path) -> Result<()> {
    if root == Path::new("/") {
        bail!("refusing to execute try hooks against the host root");
    }
    if !manifest.hooks.has_declarative_hooks() {
        return Ok(());
    }

    let mut executor = conary_core::ccs::HookExecutor::new(root);
    executor
        .execute_pre_hooks(&manifest.hooks)
        .context("failed to execute try declarative pre-hooks")?;
    let results = executor.execute_post_hooks_with_results(&manifest.hooks);
    let failures = results
        .failures()
        .map(|failure| {
            format!(
                "{} '{}' failed: {}",
                failure.hook_type,
                failure.name,
                failure.error.as_deref().unwrap_or("unknown error")
            )
        })
        .collect::<Vec<_>>();
    if !failures.is_empty() {
        bail!(
            "failed to execute try declarative post-hooks: {}",
            failures.join("; ")
        );
    }
    Ok(())
}

pub(super) fn root_relative_path(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    let relative = if path.is_absolute() {
        path.strip_prefix("/").unwrap_or(path)
    } else {
        path
    };
    if relative.as_os_str().is_empty() {
        bail!("empty try root path {path:?}");
    }
    if relative.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        bail!("unsafe try hook effects path {path:?}");
    }
    Ok(relative.to_path_buf())
}

pub(super) fn hook_account_entry_exists(
    generation_root: &Path,
    etc_state_root: &Path,
    relative_file: &str,
    name: &str,
) -> bool {
    [generation_root, etc_state_root]
        .iter()
        .any(|root| passwd_like_file_contains_name(&root.join(relative_file), name))
}

fn passwd_like_file_contains_name(path: &Path, name: &str) -> bool {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    contents
        .lines()
        .any(|line| line.split(':').next() == Some(name))
}

pub(super) fn promotable_try_hook_root(
    runtime_root: &ConaryRuntimeRoot,
    try_generation_id: i64,
) -> Result<PathBuf> {
    let root = runtime_root
        .etc_state_dir()
        .join(try_generation_id.to_string());
    std::fs::create_dir_all(&root)
        .with_context(|| format!("failed to create try hook root {}", root.display()))?;
    Ok(root)
}

pub(super) fn expose_try_namespace_root(
    runtime_root: &ConaryRuntimeRoot,
    work_dir: &Path,
    copied_conn: &Connection,
    try_generation_id: i64,
    hook_upperdir: &Path,
) -> Result<PathBuf> {
    let namespace_root = work_dir.join("namespace-root");
    if std::env::var_os("CONARY_TEST_SKIP_GENERATION_MOUNT").is_some() {
        materialize_test_try_namespace_root(copied_conn, runtime_root, hook_upperdir)?;
        recreate_path_symlink(hook_upperdir, &namespace_root)?;
        return Ok(namespace_root);
    }

    let generation_dir = runtime_root.generation_path(try_generation_id);
    let metadata =
        conary_core::generation::metadata::GenerationMetadata::read_from(&generation_dir)
            .map_err(|error| anyhow::anyhow!(error))
            .with_context(|| {
                format!(
                    "failed to read try generation metadata from {}",
                    generation_dir.display()
                )
            })?;
    let lower_root = work_dir.join("generation-root");
    let overlay_workdir = work_dir.join("namespace-work");
    std::fs::create_dir_all(&lower_root)
        .with_context(|| format!("failed to create try lower root {}", lower_root.display()))?;
    std::fs::create_dir_all(&namespace_root).with_context(|| {
        format!(
            "failed to create try namespace root {}",
            namespace_root.display()
        )
    })?;
    std::fs::create_dir_all(&overlay_workdir).with_context(|| {
        format!(
            "failed to create try namespace overlay workdir {}",
            overlay_workdir.display()
        )
    })?;

    let mount_options = conary_core::generation::mount::MountOptions {
        image_path: generation_dir.join(conary_core::generation::metadata::EROFS_IMAGE_NAME),
        basedir: runtime_root.objects_dir(),
        mount_point: lower_root.clone(),
        verity: metadata.fsverity_enabled,
        digest: metadata
            .fsverity_enabled
            .then(|| metadata.erofs_verity_digest.clone())
            .flatten(),
        upperdir: None,
        workdir: None,
    };
    conary_core::generation::mount::mount_generation(&mount_options)
        .map_err(|error| anyhow::anyhow!(error))
        .with_context(|| {
            format!(
                "failed to mount try generation {} at {}",
                try_generation_id,
                lower_root.display()
            )
        })?;
    if let Err(error) = mount_try_namespace_overlay(
        &lower_root,
        hook_upperdir,
        &overlay_workdir,
        &namespace_root,
    ) {
        let _ = conary_core::generation::mount::unmount_generation(&lower_root);
        return Err(error);
    }

    Ok(namespace_root)
}

fn mount_try_namespace_overlay(
    lower_root: &Path,
    hook_upperdir: &Path,
    overlay_workdir: &Path,
    namespace_root: &Path,
) -> Result<()> {
    let options = format!(
        "lowerdir={},upperdir={},workdir={}",
        lower_root.display(),
        hook_upperdir.display(),
        overlay_workdir.display()
    );
    let status = std::process::Command::new("mount")
        .args([
            "-t",
            "overlay",
            "overlay",
            "-o",
            &options,
            &namespace_root.to_string_lossy(),
        ])
        .status()
        .context("failed to execute try namespace overlay mount")?;
    if status.success() {
        return Ok(());
    }
    bail!(
        "failed to mount try namespace overlay at {} with lower {} and upper {}",
        namespace_root.display(),
        lower_root.display(),
        hook_upperdir.display()
    )
}

pub(super) fn teardown_try_namespace_mounts(work_dir: &Path) -> Result<()> {
    unmount_try_path_if_mounted(&work_dir.join("namespace-root"))?;
    unmount_try_path_if_mounted(&work_dir.join("generation-root"))?;
    Ok(())
}

fn unmount_try_path_if_mounted(path: &Path) -> Result<()> {
    if !try_path_is_mounted(path)? {
        return Ok(());
    }
    run_try_unmount(path)
}

fn try_path_is_mounted(path: &Path) -> Result<bool> {
    let mountinfo = read_try_mountinfo()?;
    Ok(mountinfo.lines().any(|line| {
        line.split_whitespace()
            .nth(4)
            .map(decode_mountinfo_path)
            .as_deref()
            == Some(path)
    }))
}

fn read_try_mountinfo() -> Result<String> {
    #[cfg(test)]
    if let Some(path) = std::env::var_os("CONARY_TEST_TRY_MOUNTINFO_PATH") {
        return std::fs::read_to_string(&path).with_context(|| {
            format!(
                "failed to read try mountinfo {}",
                Path::new(&path).display()
            )
        });
    }

    std::fs::read_to_string("/proc/self/mountinfo").context("failed to read /proc/self/mountinfo")
}

fn decode_mountinfo_path(raw: &str) -> PathBuf {
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 3 < bytes.len()
            && bytes[index + 1].is_ascii_digit()
            && bytes[index + 2].is_ascii_digit()
            && bytes[index + 3].is_ascii_digit()
        {
            let value = ((bytes[index + 1] - b'0') << 6)
                | ((bytes[index + 2] - b'0') << 3)
                | (bytes[index + 3] - b'0');
            decoded.push(value);
            index += 4;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(OsString::from_vec(decoded))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(&decoded).into_owned())
    }
}

fn run_try_unmount(path: &Path) -> Result<()> {
    #[cfg(test)]
    if let Some(fail_path) = std::env::var_os("CONARY_TEST_TRY_UMOUNT_FAIL")
        && Path::new(&fail_path) == path
    {
        bail!(
            "forced try namespace unmount failure for {}",
            path.display()
        );
    }

    #[cfg(test)]
    if let Some(log_path) = std::env::var_os("CONARY_TEST_TRY_UMOUNT_LOG") {
        use std::io::Write as _;

        let mut log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| {
                format!(
                    "failed to open try unmount log {}",
                    Path::new(&log_path).display()
                )
            })?;
        writeln!(log, "{}", path.display())?;
        return Ok(());
    }

    conary_core::generation::mount::unmount_generation(path)
        .map_err(|error| anyhow::anyhow!(error))
        .with_context(|| format!("failed to unmount try namespace path {}", path.display()))
}

fn materialize_test_try_namespace_root(
    copied_conn: &Connection,
    runtime_root: &ConaryRuntimeRoot,
    hook_upperdir: &Path,
) -> Result<()> {
    std::fs::create_dir_all(hook_upperdir).with_context(|| {
        format!(
            "failed to create test try namespace root {}",
            hook_upperdir.display()
        )
    })?;
    for entry in FileEntry::find_all_ordered(copied_conn).map_err(|error| anyhow::anyhow!(error))? {
        if conary_core::generation::metadata::is_excluded(&entry.path) {
            continue;
        }
        let relative = root_relative_path(&entry.path)?;
        let destination = hook_upperdir.join(relative);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create parent directory for test try namespace file {}",
                    destination.display()
                )
            })?;
        }
        remove_path_if_exists(&destination)?;
        if let Some(target) = &entry.symlink_target {
            create_symlink(target.as_ref(), &destination)?;
            continue;
        }

        let object_path =
            conary_core::filesystem::object_path(&runtime_root.objects_dir(), &entry.sha256_hash)
                .map_err(|error| anyhow::anyhow!(error))
                .with_context(|| format!("failed to locate CAS object {}", entry.sha256_hash))?;
        if let Err(_error) = std::fs::hard_link(&object_path, &destination) {
            std::fs::copy(&object_path, &destination).with_context(|| {
                format!(
                    "failed to copy CAS object {} to test try namespace file {}",
                    object_path.display(),
                    destination.display()
                )
            })?;
        }
        set_file_mode(&destination, entry.permissions)?;
    }

    for (link, target) in conary_core::generation::metadata::ROOT_SYMLINKS {
        let link_path = hook_upperdir.join(link);
        if link_path.exists() || std::fs::symlink_metadata(&link_path).is_ok() {
            continue;
        }
        create_symlink((*target).as_ref(), &link_path)?;
    }

    Ok(())
}

fn recreate_path_symlink(target: &Path, link: &Path) -> Result<()> {
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }
    remove_path_if_exists(link)?;
    create_symlink(target, link)
}

fn create_symlink(target: &Path, link: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).with_context(|| {
            format!(
                "failed to create symlink {} -> {}",
                link.display(),
                target.display()
            )
        })?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = target;
        let _ = link;
        bail!("try namespace root materialization requires symlink support")
    }
}

fn set_file_mode(path: &Path, permissions: i32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = (permissions as u32) & 0o7777;
        let mut file_permissions = std::fs::metadata(path)?.permissions();
        file_permissions.set_mode(mode);
        std::fs::set_permissions(path, file_permissions)
            .with_context(|| format!("failed to set permissions on {}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        let _ = permissions;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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
            only_if_lower: false,
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
            only_if_lower: false,
            reversible: None,
        });
        manifest.hooks.alternatives.push(AlternativeHook {
            name: "bad/name".to_string(),
            path: "/usr/bin/demo".to_string(),
            priority: 50,
            reversible: None,
        });

        let err = apply_declarative_try_hooks(&manifest, temp.path())
            .expect_err("post-hook failures should be collected");
        let message = format!("{err:#}");

        assert!(
            message.contains("failed to execute try declarative post-hooks"),
            "{message}"
        );
        assert!(
            message.contains("sysctl 'kernel.modules_disabled' failed"),
            "{message}"
        );
        assert!(
            message.contains("alternatives 'bad/name' failed"),
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
            activate: false,
            allow_irreversible: false,
            command: Some(&command),
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
        let package =
            fixture.write_package("try-activated-hooks", manifest_with_declarative_hook());

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
}
