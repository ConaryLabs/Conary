// conary-test/src/container/mock.rs
//
// Shared mock ContainerBackend for unit tests across executor, runner, and
// coordinator modules.  A single configurable struct replaces four nearly
// identical inline implementations.

use crate::container::backend::{
    ContainerBackend, ContainerConfig, ContainerId, ContainerInspection, ExecResult, ImageInfo,
};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// FailOn -- error-injection selector
// ---------------------------------------------------------------------------

/// Which backend operation should fail when using `MockBackend::failing_on`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FailOn {
    Create,
    Start,
    Stop,
}

// ---------------------------------------------------------------------------
// MockBackend
// ---------------------------------------------------------------------------

/// Configurable in-memory ContainerBackend for unit tests.
///
/// # Usage
///
/// ```rust
/// // Simple: supply a list of ExecResults returned in FIFO order.
/// let mock = MockBackend::new(vec![ExecResult { exit_code: 0, .. }]);
///
/// // With a custom inspection response:
/// let mock = MockBackend::new(vec![]).with_inspection(ContainerInspection { .. });
///
/// // With detached-exec log lines and result:
/// let mock = MockBackend::new(vec![])
///     .with_detached_exec("exec-1", vec!["line 1", "line 2"], ExecResult { exit_code: 0, .. });
///
/// // With failure injection:
/// let mock = MockBackend::failing_on(FailOn::Start);
/// ```
///
/// Accessor methods (`exec_calls`, `created_containers`, `stopped_containers`,
/// `removed_containers`, `inspected_containers`, `detached_calls`,
/// `killed_execs`) allow test assertions after the fact.
pub struct MockBackend {
    // ---------- exec queue ----------
    exec_results: Mutex<Vec<ExecResult>>,
    exec_calls: Mutex<Vec<Vec<String>>>,

    // ---------- container tracking ----------
    created_containers: Mutex<Vec<ContainerConfig>>,
    stopped_containers: Mutex<Vec<ContainerId>>,
    removed_containers: Mutex<Vec<ContainerId>>,
    inspected_containers: Mutex<Vec<ContainerId>>,
    inspection_response: ContainerInspection,

    // ---------- detached exec ----------
    detached_calls: Mutex<Vec<Vec<String>>>,
    log_sequences: Mutex<HashMap<String, Vec<String>>>,
    detached_results: Mutex<HashMap<String, ExecResult>>,
    killed_execs: Mutex<Vec<(String, String)>>,

    // ---------- error injection ----------
    fail_on: Option<FailOn>,

    // ---------- container id prefix ----------
    id_prefix: &'static str,
}

impl MockBackend {
    /// Create a mock that drains `exec_results` in FIFO order.  When the queue
    /// is empty `exec` returns a success result with empty output.
    pub fn new(results: Vec<ExecResult>) -> Self {
        Self {
            exec_results: Mutex::new(results),
            exec_calls: Mutex::new(Vec::new()),
            created_containers: Mutex::new(Vec::new()),
            stopped_containers: Mutex::new(Vec::new()),
            removed_containers: Mutex::new(Vec::new()),
            inspected_containers: Mutex::new(Vec::new()),
            inspection_response: ContainerInspection::default(),
            detached_calls: Mutex::new(Vec::new()),
            log_sequences: Mutex::new(HashMap::new()),
            detached_results: Mutex::new(HashMap::new()),
            killed_execs: Mutex::new(Vec::new()),
            fail_on: None,
            id_prefix: "mock-ctr",
        }
    }

    /// Create a mock that injects a failure on the specified operation.
    /// All other operations succeed normally.  The exec queue is empty.
    pub fn failing_on(op: FailOn) -> Self {
        let mut m = Self::new(Vec::new());
        m.fail_on = Some(op);
        m
    }

    // ---------- builder helpers ----------

    /// Override the inspection response returned by `inspect_container`.
    pub fn with_inspection(mut self, response: ContainerInspection) -> Self {
        self.inspection_response = response;
        self
    }

    /// Override the container-id prefix (default: `"mock-ctr"`).
    pub fn with_id_prefix(mut self, prefix: &'static str) -> Self {
        self.id_prefix = prefix;
        self
    }

    /// Register pre-canned log lines and a final result for a detached exec ID.
    pub fn with_detached_exec(self, exec_id: &str, logs: Vec<&str>, result: ExecResult) -> Self {
        self.log_sequences.lock().unwrap().insert(
            exec_id.to_string(),
            logs.into_iter().map(String::from).collect(),
        );
        self.detached_results
            .lock()
            .unwrap()
            .insert(exec_id.to_string(), result);
        self
    }

    // ---------- call accessors ----------

    /// Commands passed to `exec`, in order.
    pub fn exec_calls(&self) -> Vec<Vec<String>> {
        self.exec_calls.lock().unwrap().clone()
    }

    /// Container configs passed to `create`, in order.
    pub fn created_containers(&self) -> Vec<ContainerConfig> {
        self.created_containers.lock().unwrap().clone()
    }

    /// Container IDs passed to `stop`, in order.
    pub fn stopped_containers(&self) -> Vec<ContainerId> {
        self.stopped_containers.lock().unwrap().clone()
    }

    /// Container IDs passed to `remove`, in order.
    pub fn removed_containers(&self) -> Vec<ContainerId> {
        self.removed_containers.lock().unwrap().clone()
    }

    /// Container IDs passed to `inspect_container`, in order.
    pub fn inspected_containers(&self) -> Vec<ContainerId> {
        self.inspected_containers.lock().unwrap().clone()
    }

    /// Commands passed to `exec_detached`, in order.
    pub fn detached_calls(&self) -> Vec<Vec<String>> {
        self.detached_calls.lock().unwrap().clone()
    }

    /// `(exec_id, signal)` pairs passed to `kill_exec`, in order.
    pub fn killed_execs(&self) -> Vec<(String, String)> {
        self.killed_execs.lock().unwrap().clone()
    }
}

#[async_trait]
impl ContainerBackend for MockBackend {
    async fn build_image(
        &self,
        _dockerfile: &Path,
        _tag: &str,
        _build_args: HashMap<String, String>,
    ) -> Result<String> {
        Ok("mock-image".to_string())
    }

    async fn create(&self, config: ContainerConfig) -> Result<ContainerId> {
        if self.fail_on == Some(FailOn::Create) {
            anyhow::bail!("create failed");
        }
        let mut created = self.created_containers.lock().unwrap();
        created.push(config);
        Ok(format!("{}-{}", self.id_prefix, created.len()))
    }

    async fn start(&self, _id: &ContainerId) -> Result<()> {
        if self.fail_on == Some(FailOn::Start) {
            anyhow::bail!("start failed");
        }
        Ok(())
    }

    async fn exec(
        &self,
        _id: &ContainerId,
        cmd: &[&str],
        _timeout: Duration,
    ) -> Result<ExecResult> {
        self.exec_calls
            .lock()
            .unwrap()
            .push(cmd.iter().map(|s| (*s).to_string()).collect());
        let mut results = self.exec_results.lock().unwrap();
        if results.is_empty() {
            Ok(ExecResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        } else {
            Ok(results.remove(0))
        }
    }

    async fn exec_detached(&self, _id: &ContainerId, cmd: &[&str]) -> Result<String> {
        self.detached_calls
            .lock()
            .unwrap()
            .push(cmd.iter().map(|s| (*s).to_string()).collect());
        Ok("exec-1".to_string())
    }

    async fn exec_logs(&self, exec_id: &str) -> Result<mpsc::Receiver<String>> {
        let mut rx_logs = self
            .log_sequences
            .lock()
            .unwrap()
            .remove(exec_id)
            .unwrap_or_default();
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(async move {
            for line in rx_logs.drain(..) {
                if tx.send(line).await.is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }

    async fn exec_result(&self, exec_id: &str) -> Result<ExecResult> {
        self.detached_results
            .lock()
            .unwrap()
            .remove(exec_id)
            .ok_or_else(|| anyhow::anyhow!("missing detached result for {exec_id}"))
    }

    async fn kill(&self, _id: &ContainerId, _signal: &str) -> Result<()> {
        Ok(())
    }

    async fn kill_exec(&self, exec_id: &str, signal: &str) -> Result<()> {
        self.killed_execs
            .lock()
            .unwrap()
            .push((exec_id.to_string(), signal.to_string()));
        Ok(())
    }

    async fn stop(&self, id: &ContainerId) -> Result<()> {
        // Always record the attempt even when we will fail.
        self.stopped_containers.lock().unwrap().push(id.clone());
        if self.fail_on == Some(FailOn::Stop) {
            anyhow::bail!("stop failed");
        }
        Ok(())
    }

    async fn remove(&self, id: &ContainerId) -> Result<()> {
        self.removed_containers.lock().unwrap().push(id.clone());
        Ok(())
    }

    async fn copy_from(&self, _id: &ContainerId, _path: &str) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }

    async fn copy_to(&self, _id: &ContainerId, _path: &str, _data: &[u8]) -> Result<()> {
        Ok(())
    }

    async fn logs(&self, _id: &ContainerId) -> Result<String> {
        Ok(String::new())
    }

    async fn inspect_container(&self, id: &ContainerId) -> Result<ContainerInspection> {
        self.inspected_containers.lock().unwrap().push(id.clone());
        Ok(self.inspection_response.clone())
    }

    async fn list_images(&self) -> Result<Vec<ImageInfo>> {
        Ok(Vec::new())
    }
}
