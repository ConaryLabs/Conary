// conary-core/src/container/execution/process_wait.rs

//! Deadline-aware child waiting and cleanup shared by sandbox monitors.
//!
//! The PID-namespace monitor calls this after `fork()` and before `exec()`, so
//! this module must stay free of logging, global locks, and subprocess APIs.

use nix::errno::Errno;
use nix::sys::signal::{Signal, kill};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::time::{Duration, Instant};

const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChildWaitOutcome {
    Exited(i32),
    Signaled(Signal),
    TimedOut,
}

pub(super) fn wait_for_child_until(
    child: Pid,
    deadline: Instant,
) -> std::result::Result<ChildWaitOutcome, Errno> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            terminate_and_reap(child)?;
            return Ok(ChildWaitOutcome::TimedOut);
        }
        match waitpid(child, Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(_, code)) => return Ok(ChildWaitOutcome::Exited(code)),
            Ok(WaitStatus::Signaled(_, signal, _)) => {
                return Ok(ChildWaitOutcome::Signaled(signal));
            }
            Ok(WaitStatus::StillAlive) | Ok(_) => {}
            Err(Errno::EINTR) => continue,
            Err(error) => return Err(error),
        }

        sleep_without_runtime_state(WAIT_POLL_INTERVAL.min(deadline.duration_since(now)));
    }
}

fn sleep_without_runtime_state(duration: Duration) {
    let timeout_ms = duration.as_millis().clamp(1, i32::MAX as u128) as i32;
    // SAFETY: a poll with no descriptors is the kernel sleep primitive. It
    // touches no inherited userspace synchronization state.
    unsafe {
        libc::poll(std::ptr::null_mut(), 0, timeout_ms);
    }
}

/// Send SIGKILL and synchronously consume the child's terminal wait status.
pub(super) fn terminate_and_reap(child: Pid) -> std::result::Result<(), Errno> {
    match kill(child, Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => {}
        Err(error) => return Err(error),
    }

    loop {
        match waitpid(child, None) {
            Ok(WaitStatus::Exited(_, _)) | Ok(WaitStatus::Signaled(_, _, _)) => return Ok(()),
            Ok(_) | Err(Errno::EINTR) => {}
            // Another exact owner may have won a terminal-status race. Either
            // way there is no child left for this process to leak as a zombie.
            Err(Errno::ECHILD) => return Ok(()),
            Err(error) => return Err(error),
        }
    }
}

/// Wait until a pipe can be read without exceeding the sandbox deadline.
pub(super) fn wait_until_readable(
    fd: BorrowedFd<'_>,
    deadline: Instant,
) -> std::result::Result<bool, Errno> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Ok(false);
        }
        let remaining = deadline.duration_since(now);
        let timeout_ms = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
        let mut descriptor = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `descriptor` points to one initialized pollfd for the duration
        // of the call. poll only writes its `revents` field.
        let result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
        if result > 0 {
            return Ok(true);
        }
        if result == 0 {
            continue;
        }
        let error = Errno::last();
        if error != Errno::EINTR {
            return Err(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::unistd::{ForkResult, pipe};
    use std::os::fd::AsFd;

    #[test]
    fn timeout_kills_and_reaps_child() {
        // SAFETY: the child performs only async-signal-safe pause/_exit calls.
        match unsafe { nix::unistd::fork() }.expect("test fork should succeed") {
            ForkResult::Child => loop {
                unsafe { libc::pause() };
            },
            ForkResult::Parent { child } => {
                let outcome =
                    wait_for_child_until(child, Instant::now() + Duration::from_millis(20))
                        .expect("deadline wait should succeed");
                assert_eq!(outcome, ChildWaitOutcome::TimedOut);
                assert_eq!(
                    waitpid(child, Some(WaitPidFlag::WNOHANG)),
                    Err(Errno::ECHILD)
                );
            }
        }
    }

    #[test]
    fn unreadable_handshake_pipe_honors_deadline() {
        let (read_fd, _write_fd) = pipe().expect("test pipe should open");
        let readable =
            wait_until_readable(read_fd.as_fd(), Instant::now() + Duration::from_millis(20))
                .expect("poll should succeed");
        assert!(!readable);
    }
}
