// conary-test/src/container/build_output.rs

use anyhow::{Context, Result, bail};
use bollard::models::BuildInfo;
use futures::{Stream, StreamExt};
use tracing::debug;

const BUILD_OUTPUT_TAIL_BYTES: usize = 32 * 1024;

#[derive(Default)]
struct BuildOutputTail {
    output: String,
    omitted_bytes: usize,
}

impl BuildOutputTail {
    fn push(&mut self, chunk: &str) {
        self.output.push_str(chunk);
        if self.output.len() <= BUILD_OUTPUT_TAIL_BYTES {
            return;
        }

        let mut drain_through = self.output.len() - BUILD_OUTPUT_TAIL_BYTES;
        while !self.output.is_char_boundary(drain_through) {
            drain_through += 1;
        }
        self.output.drain(..drain_through);
        self.omitted_bytes += drain_through;
    }

    fn failure(&self, summary: impl std::fmt::Display) -> String {
        let retained = self.output.trim_end();
        if retained.is_empty() {
            return summary.to_string();
        }

        let omission = if self.omitted_bytes == 0 {
            String::new()
        } else {
            format!(
                "[... {} earlier Docker build output bytes omitted ...]\n",
                self.omitted_bytes
            )
        };
        format!(
            "{summary}\nDocker build output tail ({BUILD_OUTPUT_TAIL_BYTES}-byte limit):\n{omission}{retained}"
        )
    }
}

pub(super) async fn consume_build_stream<S>(mut stream: S) -> Result<()>
where
    S: Stream<Item = std::result::Result<BuildInfo, bollard::errors::Error>> + Unpin,
{
    let mut output_tail = BuildOutputTail::default();

    while let Some(result) = stream.next().await {
        match result {
            Ok(info) => {
                if let Some(stream_msg) = &info.stream {
                    debug!(target: "build", "{}", stream_msg.trim_end());
                    output_tail.push(stream_msg);
                }
                if let Some(detail) = &info.error_detail {
                    let msg = detail.message.as_deref().unwrap_or("unknown error");
                    bail!(
                        "{}",
                        output_tail.failure(format_args!("image build failed: {msg}"))
                    );
                }
            }
            Err(error) => {
                let context =
                    output_tail.failure(format_args!("image build stream error: {error}"));
                return Err(error).context(context);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{BUILD_OUTPUT_TAIL_BYTES, consume_build_stream};
    use bollard::errors::Error;
    use bollard::models::{BuildInfo, ErrorDetail};
    use futures::stream;

    #[tokio::test]
    async fn build_error_retains_only_the_bounded_output_tail() {
        let discarded_line = "old package-manager output that should be discarded\n";
        let old_output = format!(
            "discard-this-prefix\n{}",
            discarded_line.repeat(BUILD_OUTPUT_TAIL_BYTES / discarded_line.len() + 2)
        );
        let useful_output = "error: failed retrieving file 'core.db' from mirror.example\n";
        let build_stream = stream::iter(vec![
            Ok(BuildInfo {
                stream: Some(old_output),
                ..Default::default()
            }),
            Ok(BuildInfo {
                stream: Some(useful_output.to_string()),
                ..Default::default()
            }),
            Ok(BuildInfo {
                error_detail: Some(ErrorDetail {
                    message: Some("process exited with status 1".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        ]);

        let error = consume_build_stream(build_stream)
            .await
            .expect_err("build should fail");
        let diagnostic = error.to_string();

        assert!(diagnostic.contains("process exited with status 1"));
        assert!(diagnostic.contains(useful_output.trim_end()));
        assert!(diagnostic.contains("earlier Docker build output bytes omitted"));
        assert!(!diagnostic.contains("discard-this-prefix"));
        assert!(diagnostic.len() < BUILD_OUTPUT_TAIL_BYTES + 512);
    }

    #[tokio::test]
    async fn stream_error_includes_preceding_build_output() {
        let build_stream = stream::iter(vec![
            Ok(BuildInfo {
                stream: Some("pacman: failed to synchronize all databases\n".to_string()),
                ..Default::default()
            }),
            Err(Error::DockerStreamError {
                error: "connection reset".to_string(),
            }),
        ]);

        let error = consume_build_stream(build_stream)
            .await
            .expect_err("stream should fail");
        let diagnostic = error.to_string();

        assert!(diagnostic.contains("Docker stream error: connection reset"));
        assert!(diagnostic.contains("pacman: failed to synchronize all databases"));
    }
}
