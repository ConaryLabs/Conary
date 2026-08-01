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

use std::collections::{BTreeSet, VecDeque};
use std::io;
use std::sync::{Arc, Mutex};

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

/// A newline-free guest stream is malformed console output. Keep its pending
/// line bounded independently from the retained head-and-tail log, and fail the
/// test instead of silently discarding token-matching authority.
const MAX_PENDING_LINE_BYTES: usize = MAX_CAPTURED_BYTES;

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

/// Expected tokens, matched against console lines as they arrive.
///
/// Matching has to happen on the live stream rather than over the retained
/// buffer. The buffer is bounded, so a chatty guest drops its middle — and a
/// token that genuinely appeared in the dropped span is then indistinguishable
/// from one that was never emitted, which would report a false failure. A match
/// is a fact about the moment it was observed, not about what survived.
#[derive(Default)]
struct TokenMatcher {
    pending: Vec<String>,
    matched: BTreeSet<String>,
}

impl TokenMatcher {
    fn new(tokens: &[String]) -> Self {
        Self {
            pending: tokens.to_vec(),
            matched: BTreeSet::new(),
        }
    }

    fn observe_line(&mut self, line: &str) {
        self.pending.retain(|token| {
            if line.contains(token.as_str()) {
                self.matched.insert(token.clone());
                false
            } else {
                true
            }
        });
    }
}

/// Readers draining a spawned QEMU process's stdout and stderr.
pub(super) struct ConsoleCapture {
    stdout: Option<JoinHandle<io::Result<Vec<u8>>>>,
    stderr: Option<JoinHandle<io::Result<Vec<u8>>>>,
    matcher: Arc<Mutex<TokenMatcher>>,
}

/// What the readers collected once the process is gone.
pub(super) struct CapturedConsole {
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
    /// Expected tokens seen on the live stream, whether or not the retained
    /// buffer still holds them.
    pub(super) matched: BTreeSet<String>,
}

impl ConsoleCapture {
    /// Take the child's pipes and start draining them immediately, watching for
    /// `expect_output` tokens as lines arrive.
    ///
    /// Call this between spawning QEMU and doing anything that waits on the
    /// guest. The handles are removed from the child, so a later
    /// `wait_with_output` would see empty streams — use [`Self::finish`].
    pub(super) fn attach(child: &mut Child, tokens: &[String]) -> Self {
        let matcher = Arc::new(Mutex::new(TokenMatcher::new(tokens)));
        let stdout = child.stdout.take().map(|mut pipe| {
            let matcher = Arc::clone(&matcher);
            tokio::spawn(async move { drain(&mut pipe, matcher).await })
        });
        let stderr = child.stderr.take().map(|mut pipe| {
            let matcher = Arc::clone(&matcher);
            tokio::spawn(async move { drain(&mut pipe, matcher).await })
        });
        Self {
            stdout,
            stderr,
            matcher,
        }
    }

    /// Collect both streams. The readers end when the pipes close, which
    /// happens once the process exits, so kill the child before awaiting this.
    pub(super) async fn finish(mut self) -> Result<CapturedConsole> {
        let stdout = join(self.stdout.take(), "stdout").await?;
        let stderr = join(self.stderr.take(), "stderr").await?;
        let matched = self
            .matcher
            .lock()
            .expect("console token matcher is not poisoned")
            .matched
            .clone();
        Ok(CapturedConsole {
            stdout,
            stderr,
            matched,
        })
    }
}

async fn join(handle: Option<JoinHandle<io::Result<Vec<u8>>>>, stream: &str) -> Result<Vec<u8>> {
    match handle {
        Some(handle) => handle
            .await
            .with_context(|| format!("QEMU console {stream} reader panicked"))?
            .with_context(|| format!("QEMU console {stream} read failed")),
        None => Ok(Vec::new()),
    }
}

async fn drain<R>(pipe: &mut R, matcher: Arc<Mutex<TokenMatcher>>) -> io::Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut log = BoundedLog::new();
    let mut buffer = vec![0u8; READ_CHUNK_BYTES];
    // Tokens are matched per line, so a token split across two reads still
    // matches once its line completes.
    let mut line = Vec::new();
    loop {
        match pipe.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => {
                let chunk = &buffer[..read];
                log.extend(chunk);
                for &byte in chunk {
                    if byte == b'\n' {
                        observe(&matcher, &line);
                        line.clear();
                    } else {
                        if line.len() == MAX_PENDING_LINE_BYTES {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!(
                                    "console line exceeds {MAX_PENDING_LINE_BYTES} bytes without a newline"
                                ),
                            ));
                        }
                        line.push(byte);
                    }
                }
            }
            Err(error) => return Err(error),
        }
    }
    // A guest that powers off without a trailing newline still emitted its last
    // line.
    if !line.is_empty() {
        observe(&matcher, &line);
    }
    Ok(log.into_bytes())
}

fn observe(matcher: &Arc<Mutex<TokenMatcher>>, line: &[u8]) {
    let text = String::from_utf8_lossy(line);
    if let Ok(mut matcher) = matcher.lock() {
        matcher.observe_line(&text);
    }
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

        let capture = ConsoleCapture::attach(&mut child, &[]);

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

    /// A token that really was emitted must count even when the bounded buffer
    /// has since dropped it. Matching only over the retained text would report a
    /// false failure here, and a truncation marker sitting where the token was
    /// is indistinguishable from the guest never printing it.
    #[tokio::test]
    async fn a_token_dropped_from_the_buffer_still_counts_as_matched() {
        // The token has to land in the dropped *middle*: the head is retained
        // in full, so anything printed early survives by construction. Bury it
        // under more than HEAD_BYTES before and more than TAIL_BYTES after.
        let filler = HEAD_BYTES + (HEAD_BYTES / 8);
        let script = format!(
            "head -c {filler} /dev/zero | tr '\\0' 'a'; echo; \
             echo GENERATION-PUBLISHED-OK; \
             head -c {filler} /dev/zero | tr '\\0' 'b'; echo"
        );
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn writer");

        let capture = ConsoleCapture::attach(&mut child, &["GENERATION-PUBLISHED-OK".to_string()]);
        let _ = child.wait().await;
        let console = capture.finish().await.expect("collect console");

        let retained = String::from_utf8_lossy(&console.stdout);
        assert!(
            retained.contains("dropped"),
            "this test is only meaningful if the buffer truncated; it did not"
        );
        assert!(
            !retained.contains("GENERATION-PUBLISHED-OK"),
            "the token should have been dropped from the retained buffer"
        );
        assert!(
            console.matched.contains("GENERATION-PUBLISHED-OK"),
            "the token was emitted and must be recorded as matched"
        );
    }

    #[tokio::test]
    async fn a_token_never_emitted_is_not_matched() {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("echo something-else")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn writer");

        let capture = ConsoleCapture::attach(&mut child, &["NEVER-PRINTED".to_string()]);
        let _ = child.wait().await;
        let console = capture.finish().await.expect("collect console");

        assert!(console.matched.is_empty());
    }

    /// Reads land on arbitrary boundaries, so a token can be split across two
    /// of them. Line buffering is what makes the match independent of where the
    /// kernel happened to cut the stream.
    #[test]
    fn a_token_split_across_reads_matches_once_its_line_completes() {
        let mut matcher = TokenMatcher::new(&["SPLIT-TOKEN".to_string()]);

        matcher.observe_line("prefix SPLIT-TOKEN suffix");

        assert!(matcher.matched.contains("SPLIT-TOKEN"));
        assert!(matcher.pending.is_empty());
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

        let capture = ConsoleCapture::attach(&mut child, &[]);
        let _ = child.wait().await;
        let console = capture.finish().await.expect("collect console");

        assert_eq!(console.stdout, b"hello console");
        assert_eq!(console.stderr, b"to stderr");
    }

    #[tokio::test]
    async fn a_newline_free_stream_is_bounded_and_fails_loudly() {
        let matcher = Arc::new(Mutex::new(TokenMatcher::default()));
        let mut reader =
            tokio::io::repeat(b'x').take((MAX_PENDING_LINE_BYTES as u64).saturating_add(1));

        let error = drain(&mut reader, matcher).await.unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("without a newline"));
    }

    struct ReadErrorAfterPrefix {
        emitted_prefix: bool,
    }

    impl tokio::io::AsyncRead for ReadErrorAfterPrefix {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buffer: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            if !self.emitted_prefix {
                buffer.put_slice(b"partial console");
                self.emitted_prefix = true;
                return std::task::Poll::Ready(Ok(()));
            }

            std::task::Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "injected console read failure",
            )))
        }
    }

    #[tokio::test]
    async fn a_console_read_error_cannot_be_mistaken_for_clean_eof() {
        let matcher = Arc::new(Mutex::new(TokenMatcher::default()));
        let mut reader = ReadErrorAfterPrefix {
            emitted_prefix: false,
        };

        let error = drain(&mut reader, matcher).await.unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert!(error.to_string().contains("injected console read failure"));
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
