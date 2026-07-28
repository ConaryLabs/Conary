// conary-test/src/engine/qemu/console.rs
//! Concurrent, bounded capture of a QEMU process's console pipes.
//!
//! QEMU writes the guest serial console to its stdout. A piped stdout that
//! nobody reads holds one kernel pipe buffer — 64 KiB on Linux — and then
//! blocks the writer. A blocked QEMU stops executing the guest, so a guest that
//! prints more than a pipe buffer before the harness can reach it freezes
//! mid-boot and never becomes reachable at all. Waiting on the guest and
//! reading its console are therefore not separable: the reader has to run for
//! the whole life of the process, not at the end of it.
//!
//! Draining without a bound just moves the failure: a guest stuck in a console
//! loop would grow the harness's memory until it died. The capture keeps a head
//! and a tail slice and records how many bytes it dropped between them, because
//! boot failures are diagnosed from the start of the log and command failures
//! from the end.

use std::collections::VecDeque;

use anyhow::{Context, Result};
use tokio::io::AsyncReadExt;
use tokio::process::Child;
use tokio::task::JoinHandle;

/// Total console bytes kept per stream before the middle is dropped.
const MAX_CAPTURED_BYTES: usize = 4 * 1024 * 1024;

/// Bytes kept from the beginning of a stream. Boot-time failures are read from
/// the first lines, so the head is never sacrificed to the tail.
const HEAD_BYTES: usize = MAX_CAPTURED_BYTES / 2;

/// Bytes kept from the end of a stream.
const TAIL_BYTES: usize = MAX_CAPTURED_BYTES - HEAD_BYTES;

const READ_CHUNK_BYTES: usize = 8 * 1024;

/// A stream's retained head, retained tail, and the count between them.
struct BoundedLog {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    dropped: u64,
}

impl BoundedLog {
    fn new() -> Self {
        Self {
            head: Vec::new(),
            tail: VecDeque::new(),
            dropped: 0,
        }
    }

    fn extend(&mut self, chunk: &[u8]) {
        for &byte in chunk {
            if self.head.len() < HEAD_BYTES {
                self.head.push(byte);
                continue;
            }
            if self.tail.len() == TAIL_BYTES {
                self.tail.pop_front();
                self.dropped += 1;
            }
            self.tail.push_back(byte);
        }
    }

    fn into_bytes(self) -> Vec<u8> {
        let mut out = self.head;
        if self.dropped > 0 {
            out.extend_from_slice(
                format!("\n[conary-test: dropped {} console bytes]\n", self.dropped).as_bytes(),
            );
        }
        out.extend(self.tail);
        out
    }
}

/// Readers draining a spawned QEMU process's stdout and stderr.
pub(super) struct ConsoleCapture {
    stdout: Option<JoinHandle<Vec<u8>>>,
    stderr: Option<JoinHandle<Vec<u8>>>,
}

/// What the readers collected once the process is gone.
pub(super) struct CapturedConsole {
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
}

impl ConsoleCapture {
    /// Take the child's pipes and start draining them immediately.
    ///
    /// Call this between spawning QEMU and doing anything that waits on the
    /// guest. The handles are removed from the child, so a later
    /// `wait_with_output` would see empty streams — use [`Self::finish`].
    pub(super) fn attach(child: &mut Child) -> Self {
        let stdout = child
            .stdout
            .take()
            .map(|mut pipe| tokio::spawn(async move { drain(&mut pipe).await }));
        let stderr = child
            .stderr
            .take()
            .map(|mut pipe| tokio::spawn(async move { drain(&mut pipe).await }));
        Self { stdout, stderr }
    }

    /// Collect both streams. The readers end when the pipes close, which
    /// happens once the process exits, so kill the child before awaiting this.
    pub(super) async fn finish(mut self) -> Result<CapturedConsole> {
        Ok(CapturedConsole {
            stdout: join(self.stdout.take(), "stdout").await?,
            stderr: join(self.stderr.take(), "stderr").await?,
        })
    }
}

async fn join(handle: Option<JoinHandle<Vec<u8>>>, stream: &str) -> Result<Vec<u8>> {
    match handle {
        Some(handle) => handle
            .await
            .with_context(|| format!("QEMU console {stream} reader panicked")),
        None => Ok(Vec::new()),
    }
}

async fn drain<R>(pipe: &mut R) -> Vec<u8>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut log = BoundedLog::new();
    let mut buffer = vec![0u8; READ_CHUNK_BYTES];
    loop {
        match pipe.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => log.extend(&buffer[..read]),
            // A read error ends the capture; the process outcome is reported
            // separately and losing the console must not fail the test run.
            Err(_) => break,
        }
    }
    log.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;
    use std::time::Duration;
    use tokio::process::Command;

    /// The property the pipe-buffer deadlock violated: a process that writes
    /// more than one kernel pipe buffer keeps *executing*.
    ///
    /// The discriminator is a side effect outside the pipe — a sentinel file
    /// the writer touches only after its 256 KiB of output. Asserting on
    /// captured bytes alone would not discriminate: an undrained writer
    /// unblocks as soon as anything finally reads, so the bytes still arrive in
    /// the end. What never arrives is the guest's *progress* while the harness
    /// is waiting on it, which is why QEMU froze mid-boot and TGE01 timed out
    /// after 902 s having captured exactly 65,536 bytes.
    #[tokio::test]
    async fn a_writer_past_one_pipe_buffer_keeps_executing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sentinel = dir.path().join("past-the-buffer");
        let script = format!(
            "i=0; while [ $i -lt 256 ]; do printf '%1024d' 0; i=$((i+1)); done; \
             : > {}; sleep 30",
            sentinel.display()
        );

        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn writer");

        let capture = ConsoleCapture::attach(&mut child);

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !sentinel.exists() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let reached = sentinel.exists();
        let _ = child.start_kill();
        let _ = child.wait().await;
        let console = capture.finish().await.expect("collect console");

        assert!(
            reached,
            "writer stalled after one pipe buffer: it never got past 256 KiB of output"
        );
        assert!(
            console.stdout.len() > 64 * 1024,
            "captured {} bytes, expected more than one pipe buffer",
            console.stdout.len()
        );
    }

    #[tokio::test]
    async fn a_short_stream_is_captured_whole_with_no_marker() {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("printf 'hello console'; printf 'to stderr' >&2")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn writer");

        let capture = ConsoleCapture::attach(&mut child);
        let _ = child.wait().await;
        let console = capture.finish().await.expect("collect console");

        assert_eq!(console.stdout, b"hello console");
        assert_eq!(console.stderr, b"to stderr");
    }

    #[test]
    fn the_bound_keeps_both_ends_and_counts_what_it_dropped() {
        let mut log = BoundedLog::new();
        let overflow = 4096usize;
        let total = MAX_CAPTURED_BYTES + overflow;
        // Distinguishable ends: the first byte written and the last.
        let mut written = vec![b'm'; total];
        written[0] = b'H';
        written[total - 1] = b'T';
        log.extend(&written);

        let out = log.into_bytes();
        let text = String::from_utf8_lossy(&out);

        assert_eq!(out[0], b'H', "head must survive");
        assert_eq!(out[out.len() - 1], b'T', "tail must survive");
        assert!(
            text.contains(&format!("dropped {overflow} console bytes")),
            "the drop count must be exact and stated"
        );
    }

    #[test]
    fn a_stream_inside_the_bound_is_not_marked_as_dropped() {
        let mut log = BoundedLog::new();
        log.extend(&vec![b'x'; MAX_CAPTURED_BYTES]);

        let out = log.into_bytes();

        assert_eq!(out.len(), MAX_CAPTURED_BYTES);
        assert!(!String::from_utf8_lossy(&out).contains("dropped"));
    }
}
