// conary-core/src/repository/error_helpers.rs

//! Extension trait for adding contextual information to errors in repository
//! operations.
//!
//! The repository module has many `.map_err(|e| Error::DownloadError(format!(...)))`
//! chains that follow the same patterns. This trait provides concise helpers:
//!
//! ```ignore
//! // Before:
//! response.bytes().map_err(|e| Error::DownloadError(format!("Failed to fetch {}: {}", url, e)))?;
//!
//! // After:
//! response.bytes().download_context(url)?;
//! ```

use crate::error::{Error, Result};

/// Builds an actionable diagnostic for HTTP client construction failures.
///
/// Reqwest builder failures are terse in minimal roots (`builder error`), but
/// testers need to know which host files are expected before networking starts.
pub(crate) fn http_client_builder_error_message(error: impl std::fmt::Display) -> String {
    format!(
        "Failed to create HTTP client: {error}. In a minimal chroot or BusyBox-style root, \
         check that resolver files such as /etc/resolv.conf are present and that TLS trust \
         roots exist under /etc/ssl/certs, usually from the ca-certificates package."
    )
}

/// Extension trait for adding download/parse/sync context to errors.
///
/// Implemented on `Result<T, E>` where `E: Display`, so it works with
/// `reqwest::Error`, `std::io::Error`, `serde_json::Error`, etc.
#[allow(dead_code)]
pub(crate) trait ResultExt<T> {
    /// Wrap the error with download context, including the URL.
    ///
    /// Produces `Error::DownloadError("Failed to download {url}: {original}")`.
    fn download_context(self, url: &str) -> Result<T>;

    /// Wrap the error with parse context, including the format name.
    ///
    /// Produces `Error::ParseError("Failed to parse {format}: {original}")`.
    fn parse_context(self, format: &str) -> Result<T>;

    /// Wrap the error with sync context, including the repository name.
    ///
    /// Produces `Error::DownloadError("Failed to sync {repo}: {original}")`.
    fn sync_context(self, repo: &str) -> Result<T>;

    /// Wrap the error with I/O context, including a description of the operation.
    ///
    /// Produces `Error::IoError("{operation}: {original}")`.
    fn io_context(self, operation: &str) -> Result<T>;
}

impl<T, E: std::fmt::Display> ResultExt<T> for std::result::Result<T, E> {
    fn download_context(self, url: &str) -> Result<T> {
        self.map_err(|e| Error::DownloadError(format!("Failed to download {url}: {e}")))
    }

    fn parse_context(self, format: &str) -> Result<T> {
        self.map_err(|e| Error::ParseError(format!("Failed to parse {format}: {e}")))
    }

    fn sync_context(self, repo: &str) -> Result<T> {
        self.map_err(|e| Error::DownloadError(format!("Failed to sync {repo}: {e}")))
    }

    fn io_context(self, operation: &str) -> Result<T> {
        self.map_err(|e| Error::IoError(format!("{operation}: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_download_context() {
        let err: std::result::Result<(), &str> = Err("connection refused");
        let result: Result<()> = err.download_context("https://example.com/pkg.rpm");
        let e = result.unwrap_err();
        assert!(e.to_string().contains("Failed to download"));
        assert!(e.to_string().contains("https://example.com/pkg.rpm"));
        assert!(e.to_string().contains("connection refused"));
    }

    #[test]
    fn test_parse_context() {
        let err: std::result::Result<(), &str> = Err("unexpected token");
        let result: Result<()> = err.parse_context("primary.xml");
        let e = result.unwrap_err();
        assert!(e.to_string().contains("Failed to parse"));
        assert!(e.to_string().contains("primary.xml"));
    }

    #[test]
    fn test_sync_context() {
        let err: std::result::Result<(), &str> = Err("timeout");
        let result: Result<()> = err.sync_context("fedora-updates");
        let e = result.unwrap_err();
        assert!(e.to_string().contains("Failed to sync"));
        assert!(e.to_string().contains("fedora-updates"));
    }

    #[test]
    fn test_io_context() {
        let err: std::result::Result<(), &str> = Err("permission denied");
        let result: Result<()> = err.io_context("create output file");
        let e = result.unwrap_err();
        assert!(e.to_string().contains("create output file"));
        assert!(e.to_string().contains("permission denied"));
    }

    #[test]
    fn test_ok_passes_through() {
        let ok: std::result::Result<i32, &str> = Ok(42);
        let result: Result<i32> = ok.download_context("https://example.com");
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn http_client_builder_error_message_mentions_minimal_runtime_inputs() {
        let message = http_client_builder_error_message("builder error");

        assert!(message.contains("Failed to create HTTP client: builder error"));
        assert!(message.contains("minimal chroot"));
        assert!(message.contains("/etc/resolv.conf"));
        assert!(message.contains("/etc/ssl/certs"));
        assert!(message.contains("ca-certificates"));
    }
}
