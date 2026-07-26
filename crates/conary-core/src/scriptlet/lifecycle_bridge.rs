// conary-core/src/scriptlet/lifecycle_bridge.rs

//! Private, byte-exact transport between selected-root lifecycle shims and Conary.
//!
//! This module owns only transport. Package-format command grammars, state
//! transitions, and mutation authority belong to typed handlers above it.

#[path = "lifecycle_bridge/protocol.rs"]
mod protocol;
#[path = "lifecycle_bridge/staging.rs"]
mod staging;

use super::process::{TargetCommandBindMount, TargetRootScript};
use crate::error::{Error, Result, ScriptletFailureKind};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const BRIDGE_CHANNEL_FD: i32 = 7;
const BRIDGE_LOCK_READ_FD: i32 = 8;
const BRIDGE_LOCK_WRITE_FD: i32 = 9;
const MINIMUM_SOURCE_FD: i32 = 32;
const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(5);

/// One exact command invocation received from a selected-root lifecycle shim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleBridgeRequest {
    command: Vec<u8>,
    arguments: Vec<Vec<u8>>,
}

impl LifecycleBridgeRequest {
    pub fn command(&self) -> &[u8] {
        &self.command
    }

    pub fn arguments(&self) -> &[Vec<u8>] {
        &self.arguments
    }
}

/// Exact process-visible result returned to a selected-root lifecycle shim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleBridgeResponse {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: u8,
}

impl LifecycleBridgeResponse {
    pub fn new(stdout: Vec<u8>, stderr: Vec<u8>, exit_code: u8) -> Self {
        Self {
            stdout,
            stderr,
            exit_code,
        }
    }

    pub fn stdout(&self) -> &[u8] {
        &self.stdout
    }

    pub fn stderr(&self) -> &[u8] {
        &self.stderr
    }

    pub fn exit_code(&self) -> u8 {
        self.exit_code
    }
}

/// A typed handler failure, distinct from a command's exact non-zero result.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct LifecycleBridgeHandlerError {
    message: String,
}

impl LifecycleBridgeHandlerError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Typed lifecycle semantics plugged into the private bridge transport.
pub trait LifecycleBridgeHandler: Send + Sync + 'static {
    fn handle(
        &self,
        request: &LifecycleBridgeRequest,
    ) -> std::result::Result<LifecycleBridgeResponse, LifecycleBridgeHandlerError>;
}

/// A Conary-owned shim and the exact selected-root path it replaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleBridgeEndpoint {
    target: PathBuf,
    shim: Vec<u8>,
}

impl LifecycleBridgeEndpoint {
    pub fn new(target: impl Into<PathBuf>, shim: impl Into<Vec<u8>>) -> Self {
        Self {
            target: target.into(),
            shim: shim.into(),
        }
    }

    pub fn target(&self) -> &Path {
        &self.target
    }
}

/// Per-executor bridge configuration.
#[derive(Clone)]
pub struct LifecycleBridgeConfig {
    endpoints: Vec<LifecycleBridgeEndpoint>,
    handler: Arc<dyn LifecycleBridgeHandler>,
    io_timeout: Duration,
}

impl LifecycleBridgeConfig {
    pub fn new(
        endpoints: Vec<LifecycleBridgeEndpoint>,
        handler: impl LifecycleBridgeHandler,
    ) -> Self {
        Self::with_shared_handler(endpoints, Arc::new(handler))
    }

    pub fn with_shared_handler(
        endpoints: Vec<LifecycleBridgeEndpoint>,
        handler: Arc<dyn LifecycleBridgeHandler>,
    ) -> Self {
        Self {
            endpoints,
            handler,
            io_timeout: DEFAULT_IO_TIMEOUT,
        }
    }

    /// Bound an incomplete request frame or blocked response write.
    ///
    /// The executor's lifecycle timeout remains the outer bound for a shim
    /// waiting on a typed handler.
    pub fn with_io_timeout(mut self, timeout: Duration) -> Self {
        self.io_timeout = timeout;
        self
    }

    pub fn endpoints(&self) -> &[LifecycleBridgeEndpoint] {
        &self.endpoints
    }

    pub(super) fn shared_handler(&self) -> Arc<dyn LifecycleBridgeHandler> {
        Arc::clone(&self.handler)
    }

    pub(super) fn io_timeout(&self) -> Duration {
        self.io_timeout
    }
}

/// POSIX-shell transport functions shared by executable and sourced shims.
pub fn lifecycle_bridge_shell_library() -> &'static str {
    protocol::SHELL_LIBRARY
}

/// Generic executable shim that forwards its exact `$0` and argv.
pub fn executable_bridge_shim() -> &'static [u8] {
    protocol::EXECUTABLE_SHIM.as_bytes()
}

pub(super) struct LifecycleBridgeSession {
    child_fds: Option<BridgeChildFds>,
    shutdown: UnixStream,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<std::result::Result<(), String>>>,
    mounts: Vec<TargetCommandBindMount>,
    targets: staging::BridgeTargetStaging,
}

struct BridgeChildFds {
    channel: OwnedFd,
    lock_read: OwnedFd,
    lock_write: OwnedFd,
}

impl LifecycleBridgeSession {
    pub(super) fn stage(
        staged: &mut TargetRootScript,
        root: &Path,
        config: &LifecycleBridgeConfig,
    ) -> Result<Self> {
        validate_config(config)?;
        let mut targets = staging::BridgeTargetStaging::new(root)?;
        let mut mounts = Vec::with_capacity(config.endpoints.len());
        for (index, endpoint) in config.endpoints.iter().enumerate() {
            let target = targets.prepare(endpoint.target())?;
            let source = staged.create_extra_file(
                &format!("lifecycle.bridge.proxy.{index}"),
                &endpoint.shim,
                0o700,
            )?;
            mounts.push(TargetCommandBindMount { source, target });
        }

        let (server_stream, child_stream) = UnixStream::pair().map_err(bridge_setup_error)?;
        server_stream
            .set_read_timeout(Some(config.io_timeout))
            .map_err(bridge_setup_error)?;
        server_stream
            .set_write_timeout(Some(config.io_timeout))
            .map_err(bridge_setup_error)?;
        let shutdown = server_stream.try_clone().map_err(bridge_setup_error)?;
        let (lock_read, lock_write) =
            nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC).map_err(bridge_setup_error)?;
        write_lock_token(&lock_write)?;

        let child_fds = BridgeChildFds {
            channel: duplicate_source_fd(child_stream.as_raw_fd())?,
            lock_read: duplicate_source_fd(lock_read.as_raw_fd())?,
            lock_write: duplicate_source_fd(lock_write.as_raw_fd())?,
        };
        drop(child_stream);
        drop(lock_read);
        drop(lock_write);

        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let handler = Arc::clone(&config.handler);
        let timeout = config.io_timeout;
        let server = thread::Builder::new()
            .name("conary-lifecycle-bridge".to_string())
            .spawn(move || protocol::serve(server_stream, handler, timeout, &server_stop))
            .map_err(bridge_setup_error)?;

        Ok(Self {
            child_fds: Some(child_fds),
            shutdown,
            stop,
            server: Some(server),
            mounts,
            targets,
        })
    }

    pub(super) fn mounts(&self) -> &[TargetCommandBindMount] {
        &self.mounts
    }

    pub(super) fn configure_command(&self, command: &mut Command) -> Result<()> {
        let child = self.child_fds.as_ref().ok_or_else(|| {
            bridge_error("lifecycle bridge child descriptors were already released")
        })?;
        let mappings = [
            (child.channel.as_raw_fd(), BRIDGE_CHANNEL_FD),
            (child.lock_read.as_raw_fd(), BRIDGE_LOCK_READ_FD),
            (child.lock_write.as_raw_fd(), BRIDGE_LOCK_WRITE_FD),
        ];

        // Descriptor 7 is the bidirectional Unix stream. Descriptors 8 and 9
        // are the read/write ends of a one-token pipe. A shim holds that token
        // through its complete request/response exchange, so forked helpers
        // cannot interleave frames or consume each other's responses.
        //
        // Safety: dup2 is async-signal-safe and this closure runs after fork.
        unsafe {
            command.pre_exec(move || {
                for (source, target) in mappings {
                    if libc::dup2(source, target) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        Ok(())
    }

    pub(super) fn child_spawned(&mut self) {
        self.child_fds.take();
    }

    pub(super) fn finish(mut self) -> Result<()> {
        self.stop_server();
        let server_result = self.join_server();
        let cleanup_result = self.targets.cleanup();
        server_result?;
        cleanup_result
    }

    fn stop_server(&self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.shutdown.shutdown(std::net::Shutdown::Both);
    }

    fn join_server(&mut self) -> Result<()> {
        let Some(server) = self.server.take() else {
            return Ok(());
        };
        match server.join() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message)) => Err(bridge_error(message)),
            Err(_) => Err(bridge_error("lifecycle bridge server thread panicked")),
        }
    }
}

impl Drop for LifecycleBridgeSession {
    fn drop(&mut self) {
        self.stop_server();
        let _ = self.join_server();
    }
}

fn validate_config(config: &LifecycleBridgeConfig) -> Result<()> {
    if config.io_timeout.is_zero() {
        return Err(bridge_error(
            "lifecycle bridge I/O timeout must be positive",
        ));
    }
    let mut targets = std::collections::BTreeSet::new();
    for endpoint in &config.endpoints {
        if endpoint.shim.is_empty() {
            return Err(bridge_error(format!(
                "lifecycle bridge shim for {} is empty",
                endpoint.target.display()
            )));
        }
        if !targets.insert(endpoint.target.clone()) {
            return Err(bridge_error(format!(
                "duplicate lifecycle bridge endpoint {}",
                endpoint.target.display()
            )));
        }
    }
    Ok(())
}

fn duplicate_source_fd(source: i32) -> Result<OwnedFd> {
    let duplicated = unsafe { libc::fcntl(source, libc::F_DUPFD_CLOEXEC, MINIMUM_SOURCE_FD) };
    if duplicated == -1 {
        return Err(bridge_setup_error(std::io::Error::last_os_error()));
    }
    // Safety: fcntl returned a new owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicated) })
}

fn write_lock_token(lock_write: &OwnedFd) -> Result<()> {
    match nix::unistd::write(lock_write, b"\n") {
        Ok(1) => Ok(()),
        Ok(written) => Err(bridge_error(format!(
            "lifecycle bridge lock token write was short: {written} bytes"
        ))),
        Err(error) => Err(bridge_setup_error(error)),
    }
}

fn bridge_setup_error(error: impl std::fmt::Display) -> Error {
    Error::scriptlet(
        ScriptletFailureKind::ProcessSetupFailed,
        format!("cannot prepare lifecycle bridge: {error}"),
    )
}

fn bridge_error(message: impl Into<String>) -> Error {
    Error::scriptlet(ScriptletFailureKind::ContractViolation, message.into())
}

#[cfg(test)]
#[path = "lifecycle_bridge/tests.rs"]
mod tests;
