// conary-core/src/repository/remi/acquisition_error.rs

use crate::error::Error;

/// Exact failure authority for one ready-package acquisition attempt.
///
/// Retry disposition is derived from these variants and the HTTP status, never
/// from the human-readable diagnostics produced by [`std::fmt::Display`].
#[derive(Debug, thiserror::Error)]
pub(super) enum ReadyPackageAcquisitionError {
    #[error("request transport failed for {url}: {source}")]
    Transport {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("{source}")]
    HttpResponse {
        status: u16,
        #[source]
        source: Error,
    },
    #[error("response stream failed: {source}")]
    ResponseStream {
        #[source]
        source: reqwest::Error,
    },
    #[error(transparent)]
    Fatal(#[from] Error),
}

impl ReadyPackageAcquisitionError {
    pub(super) fn transport(url: &str, source: reqwest::Error) -> Self {
        Self::Transport {
            url: url.to_string(),
            source,
        }
    }

    pub(super) fn http_response(status: u16, source: Error) -> Self {
        Self::HttpResponse { status, source }
    }

    pub(super) fn response_stream(source: reqwest::Error) -> Self {
        Self::ResponseStream { source }
    }

    pub(super) fn is_retryable(&self) -> bool {
        match self {
            Self::Transport { .. } | Self::ResponseStream { .. } => true,
            Self::HttpResponse { status, .. } => is_transient_http_status(*status),
            Self::Fatal(_) => false,
        }
    }

    pub(super) fn into_repository_error(self) -> Error {
        match self {
            Self::Transport { url, source } => {
                Error::DownloadError(format!("Failed to download {url}: {source}"))
            }
            Self::HttpResponse { source, .. } | Self::Fatal(source) => source,
            Self::ResponseStream { source } => {
                Error::DownloadError(format!("response stream: {source}"))
            }
        }
    }
}

fn is_transient_http_status(status: u16) -> bool {
    status == 408 || status == 429 || (500..=599).contains(&status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http_failure(status: u16, message: &str) -> ReadyPackageAcquisitionError {
        ReadyPackageAcquisitionError::http_response(
            status,
            Error::DownloadError(message.to_string()),
        )
    }

    #[test]
    fn exact_http_status_controls_retry_disposition() {
        for status in [408, 429, 500, 503, 599] {
            assert!(
                http_failure(status, "diagnostic wording is irrelevant").is_retryable(),
                "HTTP {status} should be retryable"
            );
        }

        for status in [400, 401, 403, 404, 409, 600] {
            assert!(
                !http_failure(status, "Remi returned transient HTTP 503").is_retryable(),
                "HTTP {status} should not be retryable"
            );
        }
    }

    #[test]
    fn fatal_download_wording_cannot_enable_retry() {
        let error = ReadyPackageAcquisitionError::Fatal(Error::DownloadError(
            "Failed to download response stream: Remi returned transient HTTP 503".to_string(),
        ));

        assert!(!error.is_retryable());
    }

    #[test]
    fn typed_http_disposition_ignores_diagnostic_wording() {
        let first = http_failure(500, "first wording");
        let second = http_failure(500, "completely different wording");

        assert_eq!(first.is_retryable(), second.is_retryable());
    }

    #[test]
    fn validation_parse_and_local_failures_are_never_retryable() {
        let failures = vec![
            Error::ChecksumMismatch {
                expected: "expected".to_string(),
                actual: "actual".to_string(),
            },
            Error::ParseError("malformed accepted response".to_string()),
            Error::IoError("local output failed".to_string()),
            Error::DownloadError("invalid CCS body".to_string()),
        ];

        for failure in failures {
            assert!(!ReadyPackageAcquisitionError::Fatal(failure).is_retryable());
        }
    }
}
