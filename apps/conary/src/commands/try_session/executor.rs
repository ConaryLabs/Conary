// apps/conary/src/commands/try_session/executor.rs
//! Try-session command launcher and launcher-liveness bookkeeping.

use anyhow::{Context, Result, bail};
use conary_core::db::models::TrySession;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use super::current_boot_id;

struct RunningTryCommand {
    child: Child,
    pid: i64,
    boot_id: String,
    label: &'static str,
}

pub(super) fn run_try_command_for_session(
    command: &[&str],
    namespace_root: &Path,
    activated: bool,
    live_conn: &rusqlite::Connection,
    copied_conn: &rusqlite::Connection,
    live_session: &TrySession,
    copied_session: &TrySession,
) -> Result<()> {
    let mut running = spawn_try_command(command, namespace_root, activated)?;
    let record_result = (|| -> Result<()> {
        live_session.set_launcher(live_conn, running.pid, &running.boot_id)?;
        copied_session.set_launcher(copied_conn, running.pid, &running.boot_id)?;
        Ok(())
    })();
    if let Err(error) = record_result {
        let _ = running.child.kill();
        let _ = running.child.wait();
        return Err(error.context("failed to record try launcher liveness before waiting"));
    }

    let wait_result = wait_try_command(&mut running);
    let clear_result = clear_try_launcher(live_conn, &live_session.id)
        .and_then(|()| clear_try_launcher(copied_conn, &copied_session.id));

    match wait_result {
        Ok(()) => clear_result,
        Err(error) => {
            if let Err(clear_error) = clear_result {
                return Err(error.context(format!(
                    "also failed to clear try launcher liveness after exit: {clear_error}"
                )));
            }
            Err(error)
        }
    }
}

#[cfg(test)]
pub(super) fn launch_try_command(
    command: &[&str],
    namespace_root: &Path,
    activated: bool,
) -> Result<(i64, String)> {
    let mut running = spawn_try_command(command, namespace_root, activated)?;
    let pid = running.pid;
    let boot_id = running.boot_id.clone();
    wait_try_command(&mut running)?;
    Ok((pid, boot_id))
}

fn spawn_try_command(
    command: &[&str],
    namespace_root: &Path,
    activated: bool,
) -> Result<RunningTryCommand> {
    if command.is_empty() {
        bail!("try launcher command cannot be empty");
    }
    let boot_id = current_boot_id();
    if let Some(test_launcher) = std::env::var_os("CONARY_TEST_TRY_LAUNCHER") {
        let child = Command::new(test_launcher)
            .arg(namespace_root)
            .args(command)
            .spawn()
            .context("failed to start CONARY_TEST_TRY_LAUNCHER")?;
        return Ok(running_try_command(
            child,
            boot_id,
            "CONARY_TEST_TRY_LAUNCHER",
        ));
    }
    if activated {
        let child = Command::new(command[0])
            .args(&command[1..])
            .spawn()
            .with_context(|| format!("failed to start activated try command {}", command[0]))?;
        return Ok(running_try_command(child, boot_id, "activated try command"));
    }
    let Some(bwrap) = find_command("bwrap") else {
        bail!(
            "bubblewrap is required for namespace try; `conary try --activate` is the M1b fallback for host-global testing and mutates the host-global current generation"
        );
    };
    let child = Command::new(bwrap)
        .arg("--unshare-all")
        .arg("--die-with-parent")
        .arg("--proc")
        .arg("/proc")
        .arg("--dev")
        .arg("/dev")
        .arg("--ro-bind")
        .arg(namespace_root)
        .arg("/")
        .arg("--chdir")
        .arg("/")
        .arg("--")
        .args(command)
        .spawn()
        .context("failed to start bubblewrap namespace try launcher")?;
    Ok(running_try_command(
        child,
        boot_id,
        "bubblewrap namespace try launcher",
    ))
}

fn running_try_command(child: Child, boot_id: String, label: &'static str) -> RunningTryCommand {
    RunningTryCommand {
        pid: i64::from(child.id()),
        child,
        boot_id,
        label,
    }
}

fn wait_try_command(running: &mut RunningTryCommand) -> Result<()> {
    let status = running
        .child
        .wait()
        .with_context(|| format!("failed to wait for {}", running.label))?;
    if !status.success() {
        bail!("{} exited with status {status}", running.label);
    }
    Ok(())
}

fn clear_try_launcher(conn: &rusqlite::Connection, session_id: &str) -> Result<()> {
    let session = TrySession::find_by_id(conn, session_id)?
        .ok_or_else(|| anyhow::anyhow!("try session {session_id} not found"))?;
    Ok(session.clear_launcher(conn)?)
}

fn find_command(command: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(command))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use conary_core::ccs::manifest::CcsManifest;
    use conary_core::db::models::TrySession;

    use super::super::test_support::*;
    use super::super::{TryStartRequest, begin_try_session};
    use super::*;

    #[test]
    fn namespace_launcher_executes_bubblewrap_when_available() -> anyhow::Result<()> {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir()?;
        let bin_dir = temp.path().join("bin");
        std::fs::create_dir_all(&bin_dir)?;
        let bwrap = bin_dir.join("bwrap");
        let args_file = temp.path().join("bwrap.args");
        let pid_file = temp.path().join("bwrap.pid");
        std::fs::write(
            &bwrap,
            "#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$BWRAP_PID_FILE\"\nprintf '%s\\n' \"$@\" > \"$BWRAP_ARGS_FILE\"\n",
        )?;
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&bwrap)?.permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&bwrap, permissions)?;
        }
        let _path_guard = EnvVarGuard::set("PATH", &bin_dir);
        let _args_guard = EnvVarGuard::set("BWRAP_ARGS_FILE", &args_file);
        let _pid_guard = EnvVarGuard::set("BWRAP_PID_FILE", &pid_file);
        let namespace_root = temp.path().join("namespace-root");
        std::fs::create_dir_all(&namespace_root)?;

        let (pid, _) = launch_try_command(&["/bin/echo", "hello"], &namespace_root, false)?;

        let args = std::fs::read_to_string(args_file)?;
        assert!(args.contains("--ro-bind"), "{args}");
        assert!(
            args.contains(&namespace_root.display().to_string()),
            "{args}"
        );
        assert!(args.contains("/bin/echo"), "{args}");
        assert!(args.contains("hello"), "{args}");
        let child_pid: i64 = std::fs::read_to_string(pid_file)?.trim().parse()?;
        assert_eq!(pid, child_pid, "launcher must return the spawned child PID");
        assert_ne!(
            pid,
            i64::from(std::process::id()),
            "launcher must not record the conary parent process PID"
        );
        Ok(())
    }

    #[test]
    fn try_command_records_child_liveness_before_wait_and_clears_after_exit() -> anyhow::Result<()>
    {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
        let temp = tempfile::tempdir()?;
        let launcher = temp.path().join("launcher.sh");
        let pid_file = temp.path().join("launcher.pid");
        let release_file = temp.path().join("release");
        std::fs::write(
            &launcher,
            "#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$TRY_PID_FILE\"\nwhile [ ! -f \"$TRY_RELEASE_FILE\" ]; do sleep 0.05; done\n",
        )?;
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&launcher)?.permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&launcher, permissions)?;
        }
        let _launcher_guard = EnvVarGuard::set("CONARY_TEST_TRY_LAUNCHER", &launcher);
        let _pid_guard = EnvVarGuard::set("TRY_PID_FILE", &pid_file);
        let _release_guard = EnvVarGuard::set("TRY_RELEASE_FILE", &release_file);
        let _boot_guard = EnvVarGuard::set("CONARY_TEST_BOOT_ID", "boot-launcher");
        let fixture = TryRuntimeFixture::new();
        let package = fixture.write_package(
            "try-launch-liveness",
            CcsManifest::new_minimal("try-launch-liveness", "1.0.0"),
        );
        let db_path_string = fixture.db_path_string.clone();
        let package_for_thread = package.clone();

        let handle = std::thread::spawn(move || {
            let command = ["/bin/true"];
            begin_try_session(TryStartRequest {
                db_path: &db_path_string,
                package_path: package_for_thread.as_path(),
                activate: false,
                allow_irreversible: false,
                command: Some(&command),
            })
        });

        let child_pid = poll_until(std::time::Duration::from_secs(5), || {
            std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|value| value.trim().parse::<i64>().ok())
        })
        .ok_or_else(|| anyhow::anyhow!("launcher did not write child PID"))?;

        let live_session = poll_until(std::time::Duration::from_secs(5), || {
            TrySession::find_active_or_orphaned(&fixture.open())
                .ok()
                .flatten()
                .filter(|session| session.launcher_pid.is_some())
        })
        .ok_or_else(|| anyhow::anyhow!("live DB never recorded launcher liveness"))?;
        assert_eq!(live_session.launcher_pid, Some(child_pid));
        assert_ne!(
            live_session.launcher_pid,
            Some(i64::from(std::process::id()))
        );
        assert_eq!(
            live_session.launcher_boot_id.as_deref(),
            Some("boot-launcher")
        );

        let copied_db_path = PathBuf::from(&live_session.work_dir).join("conary.db");
        let copied_session = poll_until(std::time::Duration::from_secs(5), || {
            conary_core::db::open(&copied_db_path)
                .ok()
                .and_then(|conn| {
                    TrySession::find_by_id(&conn, &live_session.id)
                        .ok()
                        .flatten()
                        .filter(|session| session.launcher_pid == Some(child_pid))
                })
        })
        .ok_or_else(|| anyhow::anyhow!("copied DB never recorded launcher liveness"))?;
        assert_eq!(
            copied_session.launcher_boot_id,
            live_session.launcher_boot_id
        );

        std::fs::write(&release_file, b"release")?;
        let outcome = handle
            .join()
            .map_err(|_| anyhow::anyhow!("try launcher thread panicked"))??;

        let live_after = stored_session(&fixture, &outcome.session_id);
        assert_eq!(live_after.launcher_pid, None);
        assert_eq!(live_after.launcher_boot_id, None);
        let copied = conary_core::db::open(&outcome.copied_db_path)?;
        let copied_after = TrySession::find_by_id(&copied, &outcome.session_id)?.unwrap();
        assert_eq!(copied_after.launcher_pid, None);
        assert_eq!(copied_after.launcher_boot_id, None);
        Ok(())
    }

    fn poll_until<T>(
        timeout: std::time::Duration,
        mut probe: impl FnMut() -> Option<T>,
    ) -> Option<T> {
        let start = std::time::Instant::now();
        loop {
            if let Some(value) = probe() {
                return Some(value);
            }
            if start.elapsed() >= timeout {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
}
