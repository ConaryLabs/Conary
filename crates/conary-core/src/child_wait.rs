// crates/conary-core/src/child_wait.rs

use std::io::{self, Read};
use std::process::{Child, ExitStatus};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub(crate) struct ChildWaitOutput {
    pub(crate) status: Option<ExitStatus>,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) timed_out: bool,
}

pub(crate) fn wait_with_output(
    child: &mut Child,
    timeout: Duration,
) -> io::Result<ChildWaitOutput> {
    wait_with_output_inner(child, timeout, false)
}

/// Wait for a child that established itself as a process-group leader.
///
/// The entire group is terminated on timeout and after the leader exits so
/// lifecycle descendants cannot survive the transaction or keep capture pipes
/// open indefinitely.
pub(crate) fn wait_with_output_process_group(
    child: &mut Child,
    timeout: Duration,
) -> io::Result<ChildWaitOutput> {
    wait_with_output_inner(child, timeout, true)
}

fn wait_with_output_inner(
    child: &mut Child,
    timeout: Duration,
    process_group: bool,
) -> io::Result<ChildWaitOutput> {
    let stdout_reader = spawn_reader(child.stdout.take());
    let stderr_reader = spawn_reader(child.stderr.take());
    let start = Instant::now();

    loop {
        if let Some(status) = child.try_wait()? {
            if process_group {
                kill_process_group(child.id());
            }
            return Ok(ChildWaitOutput {
                status: Some(status),
                stdout: join_reader(stdout_reader),
                stderr: join_reader(stderr_reader),
                timed_out: false,
            });
        }

        if start.elapsed() >= timeout {
            break;
        }

        let sleep_for = timeout
            .saturating_sub(start.elapsed())
            .min(CHILD_POLL_INTERVAL);
        thread::sleep(sleep_for);
    }

    if process_group {
        kill_process_group(child.id());
    }
    let _ = child.kill();
    let status = child.wait().ok();

    Ok(ChildWaitOutput {
        status,
        stdout: join_reader(stdout_reader),
        stderr: join_reader(stderr_reader),
        timed_out: true,
    })
}

fn kill_process_group(leader: u32) {
    let Ok(leader) = i32::try_from(leader) else {
        return;
    };
    let _ = nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(leader),
        nix::sys::signal::Signal::SIGKILL,
    );
}

fn spawn_reader<R>(reader: Option<R>) -> Option<JoinHandle<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    reader.map(|mut reader| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = reader.read_to_end(&mut buf);
            buf
        })
    })
}

fn join_reader(handle: Option<JoinHandle<Vec<u8>>>) -> Vec<u8> {
    handle
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt as _;
    use std::process::{Command, Stdio};

    #[test]
    fn process_group_wait_reaps_background_descendants_after_leader_exit() {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 30 &"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Safety: setpgid is an async-signal-safe syscall and the closure does
        // not access shared process state.
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == 0 {
                    Ok(())
                } else {
                    Err(io::Error::last_os_error())
                }
            });
        }
        let mut child = command.spawn().expect("spawn process-group leader");

        let start = Instant::now();
        let output = wait_with_output_process_group(&mut child, Duration::from_secs(2))
            .expect("wait for process group");

        assert!(!output.timed_out);
        assert!(output.status.is_some_and(|status| status.success()));
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "background descendant kept capture pipes open"
        );
    }
}
