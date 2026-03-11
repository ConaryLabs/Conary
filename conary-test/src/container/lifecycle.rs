// conary-test/src/container/lifecycle.rs

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use bollard::Docker;
use bollard::container::{
    Config, DownloadFromContainerOptions, LogsOptions, RemoveContainerOptions,
    StopContainerOptions, UploadToContainerOptions,
};
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::image::BuildImageOptions;
use bollard::models::HostConfig;
use bytes::Bytes;
use futures::StreamExt;
use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;
use std::time::Duration;
use tracing::{debug, warn};

use super::backend::{ContainerBackend, ContainerConfig, ContainerId, ExecResult, VolumeMount};

/// Container backend powered by bollard (Docker/Podman API).
pub struct BollardBackend {
    docker: Docker,
}

impl BollardBackend {
    /// Connect to the local Docker/Podman socket using defaults.
    pub fn new() -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()
            .context("failed to connect to container runtime")?;
        Ok(Self { docker })
    }

    /// Format volume mounts as Docker bind strings (`host:container[:ro]`).
    fn format_binds(volumes: &[VolumeMount]) -> Vec<String> {
        volumes
            .iter()
            .map(|v| {
                if v.read_only {
                    format!("{}:{}:ro", v.host_path, v.container_path)
                } else {
                    format!("{}:{}", v.host_path, v.container_path)
                }
            })
            .collect()
    }

    /// Format environment variables as `KEY=VALUE` strings.
    fn format_env(env: &HashMap<String, String>) -> Vec<String> {
        env.iter().map(|(k, v)| format!("{k}={v}")).collect()
    }
}

#[async_trait]
impl ContainerBackend for BollardBackend {
    async fn build_image(
        &self,
        dockerfile: &Path,
        tag: &str,
        build_args: HashMap<String, String>,
    ) -> Result<String> {
        let context_dir = dockerfile
            .parent()
            .context("dockerfile has no parent directory")?;

        let dockerfile_name = dockerfile
            .file_name()
            .context("dockerfile has no filename")?
            .to_string_lossy()
            .to_string();

        // Create a tar archive of the build context directory.
        let tar_bytes = {
            let mut tar_builder = tar::Builder::new(Vec::new());
            tar_builder
                .append_dir_all(".", context_dir)
                .context("failed to tar build context")?;
            tar_builder
                .into_inner()
                .context("failed to finalize tar archive")?
        };

        let options = BuildImageOptions {
            dockerfile: dockerfile_name.as_str(),
            t: tag,
            rm: true,
            forcerm: true,
            buildargs: build_args
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect(),
            ..Default::default()
        };

        let mut stream = self
            .docker
            .build_image(options, None, Some(Bytes::from(tar_bytes)));

        while let Some(result) = stream.next().await {
            match result {
                Ok(info) => {
                    if let Some(stream_msg) = &info.stream {
                        debug!(target: "build", "{}", stream_msg.trim_end());
                    }
                    if let Some(error) = &info.error {
                        bail!("image build failed: {error}");
                    }
                }
                Err(e) => return Err(e).context("image build stream error"),
            }
        }

        debug!("image built: {tag}");
        Ok(tag.to_string())
    }

    async fn create(&self, config: ContainerConfig) -> Result<ContainerId> {
        let binds = Self::format_binds(&config.volumes);
        let env = Self::format_env(&config.env);

        let host_config = HostConfig {
            binds: if binds.is_empty() { None } else { Some(binds) },
            privileged: Some(config.privileged),
            network_mode: Some(config.network_mode.clone()),
            ..Default::default()
        };

        let container_config = Config {
            image: Some(config.image.as_str()),
            cmd: Some(vec!["sleep", "86400"]),
            env: if env.is_empty() {
                None
            } else {
                Some(env.iter().map(String::as_str).collect())
            },
            host_config: Some(host_config),
            ..Default::default()
        };

        let response = self
            .docker
            .create_container::<&str, &str>(None, container_config)
            .await
            .context("failed to create container")?;

        debug!(id = %response.id, "container created");
        Ok(response.id)
    }

    async fn start(&self, id: &ContainerId) -> Result<()> {
        self.docker
            .start_container::<String>(id, None)
            .await
            .context("failed to start container")?;
        debug!(id = %id, "container started");
        Ok(())
    }

    async fn exec(&self, id: &ContainerId, cmd: &[&str], timeout: Duration) -> Result<ExecResult> {
        let exec_opts = CreateExecOptions {
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            cmd: Some(cmd.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        };

        let exec_instance = self
            .docker
            .create_exec(id, exec_opts)
            .await
            .context("failed to create exec")?;

        let mut stdout = String::new();
        let mut stderr = String::new();

        let exec_result = self
            .docker
            .start_exec(&exec_instance.id, None)
            .await
            .context("failed to start exec")?;

        if let StartExecResults::Attached { mut output, .. } = exec_result {
            let collect_future = async {
                while let Some(Ok(msg)) = output.next().await {
                    match msg {
                        bollard::container::LogOutput::StdOut { message } => {
                            stdout.push_str(&String::from_utf8_lossy(&message));
                        }
                        bollard::container::LogOutput::StdErr { message } => {
                            stderr.push_str(&String::from_utf8_lossy(&message));
                        }
                        _ => {}
                    }
                }
            };

            if tokio::time::timeout(timeout, collect_future).await.is_err() {
                warn!(id = %id, cmd = ?cmd, "exec timed out");
                return Ok(ExecResult {
                    exit_code: -1,
                    stdout,
                    stderr: format!("{stderr}\n[timed out after {}s]", timeout.as_secs()),
                });
            }
        }

        // Inspect for exit code.
        let inspect = self
            .docker
            .inspect_exec(&exec_instance.id)
            .await
            .context("failed to inspect exec")?;

        let exit_code = inspect.exit_code.unwrap_or(-1);

        Ok(ExecResult {
            exit_code: i32::try_from(exit_code).unwrap_or(-1),
            stdout,
            stderr,
        })
    }

    async fn stop(&self, id: &ContainerId) -> Result<()> {
        self.docker
            .stop_container(id, Some(StopContainerOptions { t: 10 }))
            .await
            .context("failed to stop container")?;
        debug!(id = %id, "container stopped");
        Ok(())
    }

    async fn remove(&self, id: &ContainerId) -> Result<()> {
        self.docker
            .remove_container(
                id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
            .context("failed to remove container")?;
        debug!(id = %id, "container removed");
        Ok(())
    }

    async fn copy_from(&self, id: &ContainerId, path: &str) -> Result<Vec<u8>> {
        let options = Some(DownloadFromContainerOptions { path });

        let mut stream = self.docker.download_from_container(id, options);
        let mut tar_bytes = Vec::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("failed to read container archive stream")?;
            tar_bytes.extend_from_slice(&chunk);
        }

        // The response is a tar archive; extract the first file.
        let mut archive = tar::Archive::new(tar_bytes.as_slice());
        let mut entries = archive.entries().context("failed to read tar entries")?;

        if let Some(entry) = entries.next() {
            let mut entry = entry.context("failed to read tar entry")?;
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .context("failed to read file from tar")?;
            Ok(data)
        } else {
            bail!("no files found in container archive at {path}");
        }
    }

    async fn copy_to(&self, id: &ContainerId, path: &str, data: &[u8]) -> Result<()> {
        // Determine destination directory and filename.
        let dest_path = std::path::Path::new(path);
        let dir = dest_path
            .parent()
            .map_or("/", |p| p.to_str().unwrap_or("/"));
        let filename = dest_path
            .file_name()
            .context("path has no filename")?
            .to_string_lossy();

        // Build a tar archive containing the single file.
        let tar_bytes = {
            let mut header = tar::Header::new_gnu();
            header.set_path(filename.as_ref())?;
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();

            let mut builder = tar::Builder::new(Vec::new());
            builder.append(&header, data)?;
            builder.into_inner().context("failed to finalize tar")?
        };

        let options = Some(UploadToContainerOptions {
            path: dir,
            no_overwrite_dir_non_dir: "",
        });

        self.docker
            .upload_to_container(id, options, Bytes::from(tar_bytes))
            .await
            .context("failed to upload to container")?;

        debug!(id = %id, path = %path, "file copied to container");
        Ok(())
    }

    async fn logs(&self, id: &ContainerId) -> Result<String> {
        let options = Some(LogsOptions::<String> {
            stdout: true,
            stderr: true,
            ..Default::default()
        });

        let mut stream = self.docker.logs(id, options);
        let mut output = String::new();

        while let Some(result) = stream.next().await {
            match result {
                Ok(log) => output.push_str(&log.to_string()),
                Err(e) => {
                    warn!(id = %id, error = %e, "error reading container logs");
                    break;
                }
            }
        }

        Ok(output)
    }
}
