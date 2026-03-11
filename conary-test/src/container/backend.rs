// conary-test/src/container/backend.rs

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

/// Opaque container identifier returned by the backend.
pub type ContainerId = String;

/// Result of executing a command inside a container.
#[derive(Debug, Clone)]
pub struct ExecResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// A host-to-container volume mount.
#[derive(Debug, Clone)]
pub struct VolumeMount {
    pub host_path: String,
    pub container_path: String,
    pub read_only: bool,
}

/// Configuration for creating a new container.
#[derive(Debug, Clone)]
pub struct ContainerConfig {
    pub image: String,
    pub env: HashMap<String, String>,
    pub volumes: Vec<VolumeMount>,
    pub privileged: bool,
    pub network_mode: String,
    pub tmpfs: HashMap<String, String>,
    pub memory_limit: Option<i64>,
}

impl Default for ContainerConfig {
    fn default() -> Self {
        Self {
            image: String::new(),
            env: HashMap::new(),
            volumes: Vec::new(),
            privileged: false,
            network_mode: "bridge".to_string(),
            tmpfs: HashMap::new(),
            memory_limit: None,
        }
    }
}

/// Abstraction over container runtimes (Docker, Podman).
///
/// All methods return `anyhow::Result` so callers get rich error context
/// without coupling to a specific backend error type.
#[async_trait]
pub trait ContainerBackend: Send + Sync {
    /// Build an image from a Dockerfile/Containerfile.
    async fn build_image(
        &self,
        dockerfile: &Path,
        tag: &str,
        build_args: HashMap<String, String>,
    ) -> Result<String>;

    /// Create a container (does not start it).
    async fn create(&self, config: ContainerConfig) -> Result<ContainerId>;

    /// Start a previously created container.
    async fn start(&self, id: &ContainerId) -> Result<()>;

    /// Execute a command inside a running container.
    async fn exec(&self, id: &ContainerId, cmd: &[&str], timeout: Duration) -> Result<ExecResult>;

    /// Stop a running container.
    async fn stop(&self, id: &ContainerId) -> Result<()>;

    /// Remove a container (force-kills if still running).
    async fn remove(&self, id: &ContainerId) -> Result<()>;

    /// Copy a file out of the container (returns raw bytes).
    async fn copy_from(&self, id: &ContainerId, path: &str) -> Result<Vec<u8>>;

    /// Copy data into the container at the given path.
    async fn copy_to(&self, id: &ContainerId, path: &str, data: &[u8]) -> Result<()>;

    /// Retrieve all logs (stdout + stderr) from the container.
    async fn logs(&self, id: &ContainerId) -> Result<String>;
}
