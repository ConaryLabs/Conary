// src/daemon/client.rs

//! Daemon client for CLI forwarding
//!
//! Provides a client that connects to the running daemon via Unix socket.
//! Used by CLI commands to forward operations when a daemon is running.
//!
//! # Example
//!
//! ```ignore
//! use crate::daemon::client::DaemonClient;
//!
//! // Try to connect to daemon
//! if let Ok(client) = DaemonClient::connect() {
//!     // Forward install command to daemon
//!     let job = client.install(&["nginx"], Default::default())?;
//!     println!("Job queued: {}", job.job_id);
//!
//!     // Wait for completion with progress
//!     client.wait_for_job(&job.job_id, |event| {
//!         println!("{:?}", event);
//!     })?;
//! } else {
//!     // Daemon not running, execute directly
//!     // ...
//! }
//! ```

use crate::daemon::{DaemonConfig, DaemonError, DaemonEvent};
use crate::Result;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Daemon client for connecting to conaryd
pub struct DaemonClient {
    /// Path to the Unix socket
    socket_path: PathBuf,
    /// Connection timeout
    timeout: Duration,
}

/// Response from creating a transaction
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CreateTransactionResponse {
    pub job_id: String,
    pub status: String,
    pub queue_position: usize,
    pub location: String,
}

/// Transaction details
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TransactionDetails {
    pub id: String,
    pub idempotency_key: Option<String>,
    pub kind: String,
    pub status: String,
    pub spec: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub error: Option<DaemonError>,
    pub requested_by_uid: Option<u32>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub queue_position: Option<usize>,
}

/// Options for package installation
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct InstallOptions {
    pub allow_downgrade: bool,
    pub skip_deps: bool,
}

/// Options for package removal
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RemoveOptions {
    pub cascade: bool,
    pub remove_orphans: bool,
}

/// Options for package updates
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct UpdateOptions {
    pub security_only: bool,
}

/// HTTP response from daemon
struct HttpResponse {
    status_code: u16,
    #[allow(dead_code)]
    headers: Vec<(String, String)>,
    body: String,
}

impl DaemonClient {
    /// Create a new client with default socket path
    pub fn new() -> Self {
        Self {
            socket_path: PathBuf::from(DaemonConfig::default().socket_path),
            timeout: Duration::from_secs(30),
        }
    }

    /// Create a client with a custom socket path
    pub fn with_socket_path<P: AsRef<Path>>(socket_path: P) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            timeout: Duration::from_secs(30),
        }
    }

    /// Set connection timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Try to connect to the daemon
    ///
    /// Returns Ok(client) if daemon is running and accessible.
    /// Returns Err if daemon is not running or connection fails.
    pub fn connect() -> Result<Self> {
        let client = Self::new();
        client.check_connection()?;
        Ok(client)
    }

    /// Try to connect to a specific socket path
    pub fn connect_to<P: AsRef<Path>>(socket_path: P) -> Result<Self> {
        let client = Self::with_socket_path(socket_path);
        client.check_connection()?;
        Ok(client)
    }

    /// Check if the daemon is running and accessible
    pub fn check_connection(&self) -> Result<()> {
        let response = self.request("GET", "/health", None)?;
        if response.status_code == 200 {
            Ok(())
        } else {
            Err(crate::Error::IoError(format!(
                "Daemon health check failed with status {}",
                response.status_code
            )))
        }
    }

    /// Check if the daemon is running (without connecting)
    pub fn is_daemon_running(&self) -> bool {
        self.socket_path.exists()
            && self.check_connection().is_ok()
    }

    /// Install packages
    pub fn install(
        &self,
        packages: &[&str],
        options: InstallOptions,
    ) -> Result<CreateTransactionResponse> {
        let body = serde_json::json!({
            "packages": packages,
            "options": options
        });

        let response = self.request(
            "POST",
            "/v1/packages/install",
            Some(&body.to_string()),
        )?;

        self.parse_response(response)
    }

    /// Remove packages
    pub fn remove(
        &self,
        packages: &[&str],
        options: RemoveOptions,
    ) -> Result<CreateTransactionResponse> {
        let body = serde_json::json!({
            "packages": packages,
            "options": options
        });

        let response = self.request(
            "POST",
            "/v1/packages/remove",
            Some(&body.to_string()),
        )?;

        self.parse_response(response)
    }

    /// Update packages
    pub fn update(
        &self,
        packages: &[&str],
        options: UpdateOptions,
    ) -> Result<CreateTransactionResponse> {
        let body = serde_json::json!({
            "packages": packages,
            "options": options
        });

        let response = self.request(
            "POST",
            "/v1/packages/update",
            Some(&body.to_string()),
        )?;

        self.parse_response(response)
    }

    /// Get transaction details
    pub fn get_transaction(&self, job_id: &str) -> Result<TransactionDetails> {
        let response = self.request(
            "GET",
            &format!("/v1/transactions/{}", job_id),
            None,
        )?;

        self.parse_response(response)
    }

    /// Cancel a transaction
    pub fn cancel_transaction(&self, job_id: &str) -> Result<()> {
        let response = self.request(
            "DELETE",
            &format!("/v1/transactions/{}", job_id),
            None,
        )?;

        if response.status_code == 204 || response.status_code == 200 {
            Ok(())
        } else {
            self.parse_error(response)
        }
    }

    /// Wait for a job to complete, calling the callback with progress events
    ///
    /// Returns the final transaction details when the job completes.
    pub fn wait_for_job<F>(
        &self,
        job_id: &str,
        mut on_event: F,
    ) -> Result<TransactionDetails>
    where
        F: FnMut(DaemonEvent),
    {
        // Connect to SSE stream for this job
        let mut stream = UnixStream::connect(&self.socket_path)?;
        stream.set_read_timeout(Some(Duration::from_secs(300)))?;

        // Send HTTP request for SSE
        let request = format!(
            "GET /v1/transactions/{}/stream HTTP/1.1\r\n\
             Host: localhost\r\n\
             Accept: text/event-stream\r\n\
             Cache-Control: no-cache\r\n\
             Connection: keep-alive\r\n\
             \r\n",
            job_id
        );
        stream.write_all(request.as_bytes())?;

        // Read response headers
        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line)?;

        // Parse status
        let status_code: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(500);

        if status_code != 200 {
            return Err(crate::Error::IoError(format!(
                "SSE stream failed with status {}",
                status_code
            )));
        }

        // Skip headers until empty line
        loop {
            let mut header = String::new();
            reader.read_line(&mut header)?;
            if header.trim().is_empty() {
                break;
            }
        }

        // Read SSE events
        let mut event_type = String::new();
        let mut event_data = String::new();

        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let line = line.trim_end();

                    if line.starts_with("event:") {
                        event_type = line[6..].trim().to_string();
                    } else if line.starts_with("data:") {
                        event_data = line[5..].trim().to_string();
                    } else if line.is_empty() && !event_data.is_empty() {
                        // Event complete, process it
                        if let Ok(event) = serde_json::from_str::<DaemonEvent>(&event_data) {
                            // Check for terminal states
                            let is_terminal = matches!(
                                &event,
                                DaemonEvent::JobCompleted { .. }
                                    | DaemonEvent::JobFailed { .. }
                                    | DaemonEvent::JobCancelled { .. }
                            );

                            on_event(event);

                            if is_terminal {
                                break;
                            }
                        }

                        event_type.clear();
                        event_data.clear();
                    } else if line.starts_with(":") {
                        // Comment/keepalive, ignore
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // Timeout, check job status
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }

        // Get final transaction details
        self.get_transaction(job_id)
    }

    /// Poll for job completion (without SSE)
    ///
    /// Polls the job status at the given interval until it completes.
    pub fn poll_job(
        &self,
        job_id: &str,
        poll_interval: Duration,
    ) -> Result<TransactionDetails> {
        loop {
            let details = self.get_transaction(job_id)?;

            match details.status.as_str() {
                "completed" | "failed" | "cancelled" => {
                    return Ok(details);
                }
                _ => {
                    std::thread::sleep(poll_interval);
                }
            }
        }
    }

    /// Make an HTTP request to the daemon
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Result<HttpResponse> {
        let mut stream = UnixStream::connect(&self.socket_path)?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;

        // Build HTTP request
        let content_length = body.map(|b| b.len()).unwrap_or(0);
        let mut request = format!(
            "{} {} HTTP/1.1\r\n\
             Host: localhost\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n",
            method, path, content_length
        );

        if let Some(body) = body {
            request.push_str(body);
        }

        // Send request
        stream.write_all(request.as_bytes())?;

        // Read response
        let mut response = String::new();
        stream.read_to_string(&mut response)?;

        // Parse response
        self.parse_http_response(&response)
    }

    /// Parse HTTP response
    fn parse_http_response(&self, response: &str) -> Result<HttpResponse> {
        let mut lines = response.lines();

        // Parse status line
        let status_line = lines.next().ok_or_else(|| {
            crate::Error::IoError("Empty response from daemon".to_string())
        })?;

        let status_code: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(500);

        // Parse headers
        let mut headers = Vec::new();

        for line in &mut lines {
            if line.is_empty() {
                break;
            }

            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_lowercase();
                let value = value.trim().to_string();
                headers.push((key, value));
            }
        }

        // Get body (everything after headers)
        let body: String = lines.collect::<Vec<_>>().join("\n");

        Ok(HttpResponse {
            status_code,
            headers,
            body,
        })
    }

    /// Parse successful response body
    fn parse_response<T: serde::de::DeserializeOwned>(
        &self,
        response: HttpResponse,
    ) -> Result<T> {
        if response.status_code >= 200 && response.status_code < 300 {
            serde_json::from_str(&response.body).map_err(|e| {
                crate::Error::IoError(format!("Failed to parse response: {}", e))
            })
        } else {
            self.parse_error(response)
        }
    }

    /// Parse error response
    fn parse_error<T>(&self, response: HttpResponse) -> Result<T> {
        // Try to parse as DaemonError
        if let Ok(error) = serde_json::from_str::<DaemonError>(&response.body) {
            Err(crate::Error::IoError(format!(
                "Daemon error ({}): {}",
                error.status, error.detail
            )))
        } else {
            Err(crate::Error::IoError(format!(
                "Request failed with status {}: {}",
                response.status_code, response.body
            )))
        }
    }
}

impl Default for DaemonClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if the daemon is running and return a client if so
///
/// This is a convenience function for CLI commands to check if they
/// should forward to the daemon or execute directly.
pub fn try_connect() -> Option<DaemonClient> {
    DaemonClient::connect().ok()
}

/// Check if we should forward to the daemon
///
/// Returns true if:
/// - The daemon is running
/// - We're not already the daemon process
/// - The CONARY_NO_DAEMON env var is not set
pub fn should_forward_to_daemon() -> bool {
    // Don't forward if env var is set
    if std::env::var("CONARY_NO_DAEMON").is_ok() {
        return false;
    }

    // Check if daemon socket exists and is accessible
    let client = DaemonClient::new();
    client.is_daemon_running()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = DaemonClient::new();
        assert_eq!(
            client.socket_path,
            PathBuf::from("/run/conary/conaryd.sock")
        );
    }

    #[test]
    fn test_client_with_custom_path() {
        let client = DaemonClient::with_socket_path("/tmp/test.sock");
        assert_eq!(client.socket_path, PathBuf::from("/tmp/test.sock"));
    }

    #[test]
    fn test_client_with_timeout() {
        let client = DaemonClient::new().with_timeout(Duration::from_secs(60));
        assert_eq!(client.timeout, Duration::from_secs(60));
    }

    #[test]
    fn test_install_options_default() {
        let options = InstallOptions::default();
        assert!(!options.allow_downgrade);
        assert!(!options.skip_deps);
    }

    #[test]
    fn test_remove_options_default() {
        let options = RemoveOptions::default();
        assert!(!options.cascade);
        assert!(!options.remove_orphans);
    }

    #[test]
    fn test_should_forward_with_env_var() {
        // Save current value
        let prev = std::env::var("CONARY_NO_DAEMON").ok();

        // Set env var
        // SAFETY: Tests are run single-threaded by default, env manipulation is safe
        unsafe {
            std::env::set_var("CONARY_NO_DAEMON", "1");
        }
        assert!(!should_forward_to_daemon());

        // Restore
        // SAFETY: Tests are run single-threaded by default, env manipulation is safe
        unsafe {
            if let Some(val) = prev {
                std::env::set_var("CONARY_NO_DAEMON", val);
            } else {
                std::env::remove_var("CONARY_NO_DAEMON");
            }
        }
    }
}
