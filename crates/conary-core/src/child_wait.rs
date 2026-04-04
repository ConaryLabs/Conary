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
    let stdout_reader = spawn_reader(child.stdout.take());
    let stderr_reader = spawn_reader(child.stderr.take());
    let start = Instant::now();

    loop {
        if let Some(status) = child.try_wait()? {
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

    let _ = child.kill();
    let status = child.wait().ok();

    Ok(ChildWaitOutput {
        status,
        stdout: join_reader(stdout_reader),
        stderr: join_reader(stderr_reader),
        timed_out: true,
    })
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
