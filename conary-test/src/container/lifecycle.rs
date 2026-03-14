// conary-test/src/container/lifecycle.rs

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use bollard::Docker;
use bollard::container::{
    Config, DownloadFromContainerOptions, KillContainerOptions, LogsOptions,
    RemoveContainerOptions, StopContainerOptions, UploadToContainerOptions,
};
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::image::BuildImageOptions;
use bollard::models::HostConfig;
use bytes::Bytes;
use futures::StreamExt;
use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tracing::{debug, warn};

use super::backend::{
    ContainerBackend, ContainerConfig, ContainerId, ContainerInspection, ExecResult, ImageInfo,
    VolumeMount,
};

struct RunningExec {
    container_id: ContainerId,
    receiver: Option<mpsc::Receiver<String>>,
    result_rx: Option<oneshot::Receiver<ExecResult>>,
    pid: Arc<Mutex<Option<u64>>>,
    pid_ready: Arc<Notify>,
}

/// Container backend powered by bollard (Docker/Podman API).
pub struct BollardBackend {
    docker: Docker,
    running_execs: Arc<Mutex<HashMap<String, RunningExec>>>,
}

impl BollardBackend {
    fn normalize_signal_name(signal: &str) -> &str {
        signal.strip_prefix("SIG").unwrap_or(signal)
    }

    fn find_build_context(dockerfile: &Path) -> Result<(std::path::PathBuf, String)> {
        let dockerfile = dockerfile
            .canonicalize()
            .context("failed to canonicalize dockerfile path")?;

        let mut candidate = dockerfile
            .parent()
            .context("dockerfile has no parent directory")?;

        while let Some(parent) = candidate.parent() {
            if candidate.join("Cargo.toml").is_file()
                || candidate.join(".git").exists()
                || (candidate.join("config.toml").is_file() && candidate.join("runner").is_dir())
            {
                let dockerfile_name = dockerfile
                    .strip_prefix(candidate)
                    .context("failed to derive dockerfile path within build context")?
                    .to_string_lossy()
                    .to_string();
                return Ok((candidate.to_path_buf(), dockerfile_name));
            }
            candidate = parent;
        }

        let context_dir = dockerfile
            .parent()
            .context("dockerfile has no parent directory")?;
        let dockerfile_name = dockerfile
            .file_name()
            .context("dockerfile has no filename")?
            .to_string_lossy()
            .to_string();
        Ok((context_dir.to_path_buf(), dockerfile_name))
    }

    /// Connect to the local Docker/Podman socket using defaults.
    pub fn new() -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()
            .context("failed to connect to container runtime")?;
        Ok(Self {
            docker,
            running_execs: Arc::new(Mutex::new(HashMap::new())),
        })
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

    fn collect_line_fragments(buffer: &mut String, chunk: &str) -> Vec<String> {
        buffer.push_str(chunk);
        let mut lines = Vec::new();
        while let Some(idx) = buffer.find('\n') {
            let line = buffer.drain(..=idx).collect::<String>();
            lines.push(line.trim_end_matches('\n').to_string());
        }
        lines
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
        let (context_dir, dockerfile_name) = Self::find_build_context(dockerfile)?;

        // Create a tar archive of the build context directory.
        let tar_bytes = {
            let mut tar_builder = tar::Builder::new(Vec::new());
            tar_builder
                .append_dir_all(".", &context_dir)
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

        // Podman rejects an empty X-Registry-Config header on /build, while
        // bollard emits that header when credentials=None. Pass an explicit
        // empty config map so the header serializes to "{}" instead.
        let mut stream =
            self.docker
                .build_image(options, Some(HashMap::new()), Some(Bytes::from(tar_bytes)));

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
            ..Default::default()
        };
        let mut host_config = host_config;
        if !config.tmpfs.is_empty() {
            host_config.tmpfs = Some(config.tmpfs.clone());
        }
        if let Some(mem) = config.memory_limit {
            host_config.memory = Some(mem);
        }
        host_config.network_mode = Some(config.network_mode.clone());

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

    async fn exec_detached(&self, id: &ContainerId, cmd: &[&str]) -> Result<String> {
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

        let exec_id = exec_instance.id.clone();
        let docker = self.docker.clone();
        let container_id = id.clone();
        let (tx, rx) = mpsc::channel(128);
        let (result_tx, result_rx) = oneshot::channel();
        let pid = Arc::new(Mutex::new(None));
        let pid_ready = Arc::new(Notify::new());
        let pid_for_task = Arc::clone(&pid);
        let pid_ready_for_task = Arc::clone(&pid_ready);
        let exec_id_for_task = exec_id.clone();

        tokio::spawn(async move {
            let mut stdout = String::new();
            let mut stderr = String::new();
            let mut stdout_buffer = String::new();
            let mut stderr_buffer = String::new();

            let exec_result = async {
                let start_result = docker
                    .start_exec(&exec_id_for_task, None::<StartExecOptions>)
                    .await
                    .context("failed to start exec")?;

                if let StartExecResults::Attached { mut output, .. } = start_result {
                    while let Some(msg) = output.next().await {
                        match msg.context("failed to read exec output")? {
                            bollard::container::LogOutput::StdOut { message } => {
                                let text = String::from_utf8_lossy(&message).to_string();
                                stdout.push_str(&text);
                                for line in Self::collect_line_fragments(&mut stdout_buffer, &text)
                                {
                                    if let Some(value) = line
                                        .strip_prefix("__CONARY_TEST_PID__=")
                                        .and_then(|pid_text| pid_text.parse::<u64>().ok())
                                    {
                                        *pid_for_task.lock().await = Some(value);
                                        pid_ready_for_task.notify_waiters();
                                        continue;
                                    }
                                    if tx.send(line).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            bollard::container::LogOutput::StdErr { message } => {
                                let text = String::from_utf8_lossy(&message).to_string();
                                stderr.push_str(&text);
                                for line in Self::collect_line_fragments(&mut stderr_buffer, &text)
                                {
                                    if tx.send(line).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }

                if !stdout_buffer.is_empty() {
                    if let Some(value) = stdout_buffer
                        .strip_prefix("__CONARY_TEST_PID__=")
                        .and_then(|pid_text| pid_text.parse::<u64>().ok())
                    {
                        *pid_for_task.lock().await = Some(value);
                        pid_ready_for_task.notify_waiters();
                    } else {
                        let _ = tx.send(stdout_buffer.clone()).await;
                    }
                }
                if !stderr_buffer.is_empty() {
                    let _ = tx.send(stderr_buffer.clone()).await;
                }

                let inspect = docker
                    .inspect_exec(&exec_id_for_task)
                    .await
                    .context("failed to inspect exec")?;
                let exit_code = inspect.exit_code.unwrap_or(-1);

                Ok::<ExecResult, anyhow::Error>(ExecResult {
                    exit_code: i32::try_from(exit_code).unwrap_or(-1),
                    stdout: stdout.replace(
                        &format!(
                            "__CONARY_TEST_PID__={}\n",
                            pid_for_task.lock().await.unwrap_or_default()
                        ),
                        "",
                    ),
                    stderr,
                })
            }
            .await;

            let result = match exec_result {
                Ok(result) => result,
                Err(err) => ExecResult {
                    exit_code: -1,
                    stdout,
                    stderr: err.to_string(),
                },
            };

            let _ = result_tx.send(result);
        });

        self.running_execs.lock().await.insert(
            exec_id.clone(),
            RunningExec {
                container_id,
                receiver: Some(rx),
                result_rx: Some(result_rx),
                pid,
                pid_ready,
            },
        );

        Ok(exec_id)
    }

    async fn exec_logs(&self, exec_id: &str) -> Result<mpsc::Receiver<String>> {
        let mut running_execs = self.running_execs.lock().await;
        let running = running_execs
            .get_mut(exec_id)
            .with_context(|| format!("unknown exec id: {exec_id}"))?;
        running
            .receiver
            .take()
            .with_context(|| format!("exec logs already taken: {exec_id}"))
    }

    async fn exec_result(&self, exec_id: &str) -> Result<ExecResult> {
        let result_rx = {
            let mut running_execs = self.running_execs.lock().await;
            let running = running_execs
                .get_mut(exec_id)
                .with_context(|| format!("unknown exec id: {exec_id}"))?;
            running
                .result_rx
                .take()
                .with_context(|| format!("exec result already taken: {exec_id}"))?
        };

        let result = result_rx
            .await
            .with_context(|| format!("failed to await exec result for {exec_id}"))?;
        self.running_execs.lock().await.remove(exec_id);
        Ok(result)
    }

    async fn kill(&self, id: &ContainerId, signal: &str) -> Result<()> {
        self.docker
            .kill_container(
                id,
                Some(KillContainerOptions {
                    signal: signal.to_string(),
                }),
            )
            .await
            .context("failed to kill container")?;
        Ok(())
    }

    async fn kill_exec(&self, exec_id: &str, signal: &str) -> Result<()> {
        let (container_id, pid, pid_ready) = {
            let running_execs = self.running_execs.lock().await;
            let running = running_execs
                .get(exec_id)
                .with_context(|| format!("unknown exec id: {exec_id}"))?;
            (
                running.container_id.clone(),
                Arc::clone(&running.pid),
                Arc::clone(&running.pid_ready),
            )
        };

        if pid.lock().await.is_none() {
            pid_ready.notified().await;
        }
        let target_pid = pid
            .lock()
            .await
            .as_ref()
            .copied()
            .with_context(|| format!("exec pid not available for {exec_id}"))?;

        let signal_name = Self::normalize_signal_name(signal);
        let signal_cmd = format!("kill -s {signal_name} {target_pid}");
        let result = self
            .exec(
                &container_id,
                &["sh", "-c", &signal_cmd],
                Duration::from_secs(10),
            )
            .await?;
        if result.exit_code != 0 {
            bail!("failed to signal exec {exec_id}: {}", result.stderr.trim());
        }
        Ok(())
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

    async fn inspect_container(&self, id: &ContainerId) -> Result<ContainerInspection> {
        let info = self
            .docker
            .inspect_container(id, None)
            .await
            .context("failed to inspect container")?;

        let host_config = info.host_config.as_ref();

        let memory_limit = host_config
            .and_then(|hc| hc.memory)
            .and_then(|m| u64::try_from(m).ok())
            .filter(|&m| m > 0);

        let tmpfs = host_config
            .and_then(|hc| hc.tmpfs.clone())
            .unwrap_or_default();

        let network_mode = host_config.and_then(|hc| hc.network_mode.clone());

        Ok(ContainerInspection {
            memory_limit,
            tmpfs,
            network_mode,
        })
    }

    async fn list_images(&self) -> Result<Vec<ImageInfo>> {
        let images = self
            .docker
            .list_images(Some(bollard::image::ListImagesOptions::<String> {
                all: true,
                ..Default::default()
            }))
            .await
            .context("failed to list images")?;

        let result = images
            .into_iter()
            .map(|img| ImageInfo {
                id: img.id,
                tags: img.repo_tags,
                size: u64::try_from(img.size).unwrap_or(0),
            })
            .collect();

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::BollardBackend;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn build_context_prefers_workspace_root() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("conary-test-build-context-{unique}"));
        let dockerfile = root.join("tests/integration/remi/containers/Containerfile.fedora43");
        let dockerfile_parent = dockerfile.parent().expect("dockerfile parent");

        fs::create_dir_all(dockerfile_parent).expect("create dockerfile directory");
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n")
            .expect("write workspace cargo");
        fs::write(&dockerfile, "FROM scratch\n").expect("write dockerfile");

        let (context_dir, dockerfile_name) =
            BollardBackend::find_build_context(&dockerfile).expect("resolve build context");

        assert_eq!(context_dir, root);
        assert_eq!(
            dockerfile_name,
            "tests/integration/remi/containers/Containerfile.fedora43"
        );

        fs::remove_dir_all(&context_dir).expect("cleanup temp build context");
    }

    #[test]
    fn normalize_signal_name_strips_sig_prefix() {
        assert_eq!(BollardBackend::normalize_signal_name("SIGKILL"), "KILL");
        assert_eq!(BollardBackend::normalize_signal_name("SIGTERM"), "TERM");
        assert_eq!(BollardBackend::normalize_signal_name("KILL"), "KILL");
    }

    #[tokio::test]
    #[ignore] // Requires podman/docker runtime
    async fn smoke_test_real_container() {
        use crate::container::backend::{ContainerBackend, ContainerConfig};
        use std::time::Duration;

        let backend = BollardBackend::new().expect("connect to container runtime");

        let config = ContainerConfig {
            image: "docker.io/library/alpine:latest".to_string(),
            ..Default::default()
        };

        let id = backend.create(config).await.expect("create container");
        backend.start(&id).await.expect("start container");

        let result = backend
            .exec(&id, &["echo", "hello"], Duration::from_secs(10))
            .await
            .expect("exec echo");
        assert_eq!(result.exit_code, 0);
        assert!(
            result.stdout.trim().contains("hello"),
            "stdout should contain 'hello', got: {}",
            result.stdout
        );

        backend.stop(&id).await.expect("stop container");
        backend.remove(&id).await.expect("remove container");
    }
}
