// conary-test/src/engine/container_coordinator.rs

use anyhow::{Context, Result};
use tracing::{debug, warn};

use crate::config::manifest::ResourceConstraints;
use crate::container::backend::{ContainerBackend, ContainerConfig, ContainerId};

/// Orchestrates container lifecycle for test execution.
///
/// Tracks all created containers and guarantees cleanup via `teardown_all`,
/// even if individual tests fail. Optionally verifies resource constraints
/// after container creation by inspecting the running container.
pub struct ContainerCoordinator<'a> {
    backend: &'a dyn ContainerBackend,
    tracked: Vec<ContainerId>,
}

impl<'a> ContainerCoordinator<'a> {
    pub fn new(backend: &'a dyn ContainerBackend) -> Self {
        Self {
            backend,
            tracked: Vec::new(),
        }
    }

    /// Create and start a container, optionally verifying resource constraints.
    ///
    /// The container ID is tracked for cleanup via `teardown_all`.
    pub async fn setup_container(
        &mut self,
        config: &ContainerConfig,
        resources: Option<&ResourceConstraints>,
    ) -> Result<ContainerId> {
        let id = self
            .backend
            .create(config.clone())
            .await
            .context("coordinator: failed to create container")?;

        self.tracked.push(id.clone());

        self.backend
            .start(&id)
            .await
            .context("coordinator: failed to start container")?;

        if let Some(constraints) = resources {
            self.verify_resources(&id, constraints).await?;
        }

        debug!(id = %id, "coordinator: container ready");
        Ok(id)
    }

    /// Stop and remove a single container, removing it from the tracked list.
    pub async fn teardown_container(&mut self, id: &ContainerId) -> Result<()> {
        if let Err(err) = self.backend.stop(id).await {
            warn!(id = %id, error = %err, "coordinator: failed to stop container");
        }
        if let Err(err) = self.backend.remove(id).await {
            warn!(id = %id, error = %err, "coordinator: failed to remove container");
        }

        self.tracked.retain(|tracked_id| tracked_id != id);
        debug!(id = %id, "coordinator: container torn down");
        Ok(())
    }

    /// Tear down all tracked containers. Logs warnings on failure but does not
    /// propagate errors, ensuring best-effort cleanup.
    pub async fn teardown_all(&mut self) {
        let ids: Vec<ContainerId> = self.tracked.drain(..).collect();
        for id in &ids {
            if let Err(err) = self.backend.stop(id).await {
                warn!(id = %id, error = %err, "coordinator: failed to stop container during cleanup");
            }
            if let Err(err) = self.backend.remove(id).await {
                warn!(id = %id, error = %err, "coordinator: failed to remove container during cleanup");
            }
        }
        debug!(count = ids.len(), "coordinator: teardown_all complete");
    }

    /// Returns the number of currently tracked containers.
    pub fn tracked_count(&self) -> usize {
        self.tracked.len()
    }

    /// Drain all tracked container IDs without stopping/removing them.
    /// Used by cleanup guards that take ownership of the IDs.
    pub fn drain_tracked(&mut self) -> Vec<ContainerId> {
        self.tracked.drain(..).collect()
    }

    /// Run an async closure with guaranteed cleanup. `teardown_all` is
    /// called regardless of whether `f` returns `Ok`, `Err`, or panics
    /// (in the panic case, containers are drained and cleaned up by the
    /// caller).
    pub async fn with_cleanup<F, Fut, T>(&mut self, f: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let result = f().await;
        self.teardown_all().await;
        result
    }

    /// Verify that the container's actual resource configuration matches the
    /// requested constraints by inspecting the running container.
    async fn verify_resources(
        &self,
        id: &ContainerId,
        constraints: &ResourceConstraints,
    ) -> Result<()> {
        let inspection = self
            .backend
            .inspect_container(id)
            .await
            .context("coordinator: failed to inspect container for resource verification")?;

        if let Some(expected_mb) = constraints.memory_limit_mb {
            let expected_bytes = expected_mb.saturating_mul(1024 * 1024);
            if inspection
                .memory_limit
                .is_some_and(|actual| actual != expected_bytes)
            {
                warn!(
                    id = %id,
                    expected = expected_bytes,
                    actual = ?inspection.memory_limit,
                    "coordinator: memory limit mismatch"
                );
            }
        }

        if constraints.network_isolated.unwrap_or(false)
            && inspection
                .network_mode
                .as_deref()
                .is_some_and(|mode| mode != "none")
        {
            warn!(
                id = %id,
                actual = ?inspection.network_mode,
                "coordinator: expected network_mode=none for isolated test"
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::backend::{
        ContainerBackend, ContainerConfig, ContainerId, ContainerInspection, ExecResult, ImageInfo,
    };
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::sync::mpsc;

    /// Minimal mock backend for coordinator tests.
    struct CoordinatorMock {
        created: Mutex<Vec<ContainerConfig>>,
        stopped: Mutex<Vec<ContainerId>>,
        removed: Mutex<Vec<ContainerId>>,
        inspected: Mutex<Vec<ContainerId>>,
        inspection_response: ContainerInspection,
    }

    impl CoordinatorMock {
        fn new() -> Self {
            Self {
                created: Mutex::new(Vec::new()),
                stopped: Mutex::new(Vec::new()),
                removed: Mutex::new(Vec::new()),
                inspected: Mutex::new(Vec::new()),
                inspection_response: ContainerInspection::default(),
            }
        }

        fn with_inspection(mut self, inspection: ContainerInspection) -> Self {
            self.inspection_response = inspection;
            self
        }
    }

    #[async_trait]
    impl ContainerBackend for CoordinatorMock {
        async fn build_image(
            &self,
            _dockerfile: &Path,
            _tag: &str,
            _build_args: HashMap<String, String>,
        ) -> Result<String> {
            Ok("mock".to_string())
        }

        async fn create(&self, config: ContainerConfig) -> Result<ContainerId> {
            let mut created = self.created.lock().unwrap();
            created.push(config);
            Ok(format!("coord-ctr-{}", created.len()))
        }

        async fn start(&self, _id: &ContainerId) -> Result<()> {
            Ok(())
        }

        async fn exec(
            &self,
            _id: &ContainerId,
            _cmd: &[&str],
            _timeout: Duration,
        ) -> Result<ExecResult> {
            Ok(ExecResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }

        async fn exec_detached(&self, _id: &ContainerId, _cmd: &[&str]) -> Result<String> {
            Ok("exec-1".to_string())
        }

        async fn exec_logs(&self, _exec_id: &str) -> Result<mpsc::Receiver<String>> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(rx)
        }

        async fn exec_result(&self, _exec_id: &str) -> Result<ExecResult> {
            Ok(ExecResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }

        async fn kill(&self, _id: &ContainerId, _signal: &str) -> Result<()> {
            Ok(())
        }

        async fn kill_exec(&self, _exec_id: &str, _signal: &str) -> Result<()> {
            Ok(())
        }

        async fn stop(&self, id: &ContainerId) -> Result<()> {
            self.stopped.lock().unwrap().push(id.clone());
            Ok(())
        }

        async fn remove(&self, id: &ContainerId) -> Result<()> {
            self.removed.lock().unwrap().push(id.clone());
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
            self.inspected.lock().unwrap().push(id.clone());
            Ok(self.inspection_response.clone())
        }

        async fn list_images(&self) -> Result<Vec<ImageInfo>> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn setup_and_teardown_tracks_container() {
        let mock = CoordinatorMock::new();
        let mut coord = ContainerCoordinator::new(&mock);

        let config = ContainerConfig {
            image: "test:latest".to_string(),
            ..Default::default()
        };

        let id = coord.setup_container(&config, None).await.unwrap();
        assert_eq!(coord.tracked_count(), 1);
        assert_eq!(id, "coord-ctr-1");

        coord.teardown_container(&id).await.unwrap();
        assert_eq!(coord.tracked_count(), 0);

        assert_eq!(mock.stopped.lock().unwrap().as_slice(), ["coord-ctr-1"]);
        assert_eq!(mock.removed.lock().unwrap().as_slice(), ["coord-ctr-1"]);
    }

    #[tokio::test]
    async fn teardown_all_cleans_up_all_tracked() {
        let mock = CoordinatorMock::new();
        let mut coord = ContainerCoordinator::new(&mock);

        let config = ContainerConfig {
            image: "test:latest".to_string(),
            ..Default::default()
        };

        let _id1 = coord.setup_container(&config, None).await.unwrap();
        let _id2 = coord.setup_container(&config, None).await.unwrap();
        let _id3 = coord.setup_container(&config, None).await.unwrap();
        assert_eq!(coord.tracked_count(), 3);

        coord.teardown_all().await;
        assert_eq!(coord.tracked_count(), 0);

        let stopped = mock.stopped.lock().unwrap();
        assert_eq!(stopped.len(), 3);
        assert!(stopped.contains(&"coord-ctr-1".to_string()));
        assert!(stopped.contains(&"coord-ctr-2".to_string()));
        assert!(stopped.contains(&"coord-ctr-3".to_string()));

        let removed = mock.removed.lock().unwrap();
        assert_eq!(removed.len(), 3);
    }

    #[tokio::test]
    async fn setup_with_resources_calls_inspect() {
        let inspection = ContainerInspection {
            memory_limit: Some(512 * 1024 * 1024),
            tmpfs: HashMap::new(),
            network_mode: Some("none".to_string()),
        };
        let mock = CoordinatorMock::new().with_inspection(inspection);
        let mut coord = ContainerCoordinator::new(&mock);

        let config = ContainerConfig {
            image: "test:latest".to_string(),
            memory_limit: Some(512 * 1024 * 1024),
            network_mode: "none".to_string(),
            ..Default::default()
        };

        let constraints = ResourceConstraints {
            memory_limit_mb: Some(512),
            tmpfs_size_mb: None,
            network_isolated: Some(true),
        };

        let id = coord
            .setup_container(&config, Some(&constraints))
            .await
            .unwrap();
        assert_eq!(id, "coord-ctr-1");

        let inspected = mock.inspected.lock().unwrap();
        assert_eq!(inspected.as_slice(), ["coord-ctr-1"]);
    }

    #[tokio::test]
    async fn setup_without_resources_skips_inspect() {
        let mock = CoordinatorMock::new();
        let mut coord = ContainerCoordinator::new(&mock);

        let config = ContainerConfig {
            image: "test:latest".to_string(),
            ..Default::default()
        };

        let _id = coord.setup_container(&config, None).await.unwrap();

        let inspected = mock.inspected.lock().unwrap();
        assert!(inspected.is_empty());
    }

    #[tokio::test]
    async fn teardown_container_not_tracked_is_noop() {
        let mock = CoordinatorMock::new();
        let mut coord = ContainerCoordinator::new(&mock);

        // Teardown a container that was never tracked -- should not panic.
        coord
            .teardown_container(&"nonexistent".to_string())
            .await
            .unwrap();
        assert_eq!(coord.tracked_count(), 0);
    }

    #[tokio::test]
    async fn with_cleanup_tears_down_on_success() {
        let mock = CoordinatorMock::new();
        let mut coord = ContainerCoordinator::new(&mock);

        let config = ContainerConfig {
            image: "test:latest".to_string(),
            ..Default::default()
        };

        let _id = coord.setup_container(&config, None).await.unwrap();
        assert_eq!(coord.tracked_count(), 1);

        let result: Result<String> = coord
            .with_cleanup(|| async { Ok("done".to_string()) })
            .await;
        assert!(result.is_ok());
        assert_eq!(coord.tracked_count(), 0);

        let stopped = mock.stopped.lock().unwrap();
        assert_eq!(stopped.len(), 1);
    }

    #[tokio::test]
    async fn with_cleanup_tears_down_on_error() {
        let mock = CoordinatorMock::new();
        let mut coord = ContainerCoordinator::new(&mock);

        let config = ContainerConfig {
            image: "test:latest".to_string(),
            ..Default::default()
        };

        let _id = coord.setup_container(&config, None).await.unwrap();

        let result: Result<String> = coord
            .with_cleanup(|| async { anyhow::bail!("test error") })
            .await;
        assert!(result.is_err());
        assert_eq!(coord.tracked_count(), 0);

        let stopped = mock.stopped.lock().unwrap();
        assert_eq!(stopped.len(), 1);
    }

    #[tokio::test]
    async fn drain_tracked_empties_list() {
        let mock = CoordinatorMock::new();
        let mut coord = ContainerCoordinator::new(&mock);

        let config = ContainerConfig {
            image: "test:latest".to_string(),
            ..Default::default()
        };

        let _id1 = coord.setup_container(&config, None).await.unwrap();
        let _id2 = coord.setup_container(&config, None).await.unwrap();
        assert_eq!(coord.tracked_count(), 2);

        let drained = coord.drain_tracked();
        assert_eq!(drained.len(), 2);
        assert_eq!(coord.tracked_count(), 0);
    }
}
