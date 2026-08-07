// conary-core/src/repository/remi/protocol.rs

use super::*;

/// Response when package needs conversion (202 Accepted)
#[derive(Debug, Deserialize)]
pub struct ConversionAccepted {
    pub status: String,
    pub job_id: String,
    pub poll_url: String,
    pub eta_seconds: Option<u32>,
}

/// Job status response from polling endpoint
#[derive(Debug, Deserialize)]
pub struct JobStatus {
    pub job_id: String,
    pub status: ConversionJobState,
    pub distro: String,
    pub package: String,
    pub version: Option<String>,
    pub architecture: Option<String>,
    pub progress: Option<u8>,
    pub error: Option<String>,
    pub manifest: Option<PackageManifest>,
}

/// Exact lifecycle states published by Remi's job-status endpoint.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConversionJobState {
    Pending,
    Converting,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum JobPollDecision<'a> {
    Wait,
    Ready,
    Failed(&'a str),
}

impl JobStatus {
    pub(super) fn poll_decision(&self) -> JobPollDecision<'_> {
        match self.status {
            ConversionJobState::Pending | ConversionJobState::Converting => JobPollDecision::Wait,
            ConversionJobState::Ready => JobPollDecision::Ready,
            ConversionJobState::Failed => {
                JobPollDecision::Failed(self.error.as_deref().unwrap_or("Unknown error"))
            }
        }
    }
}

/// Package manifest with chunk list
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PackageManifest {
    pub name: String,
    pub version: String,
    pub distro: String,
    pub chunks: Vec<ChunkRef>,
    pub total_size: u64,
    pub content_hash: String,
}

/// Reference to a chunk in the CAS
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChunkRef {
    pub hash: String,
    pub size: u64,
    pub offset: u64,
}

/// Shared core logic for Remi clients.
///
/// Handles URL construction, HTTP status mapping, and job status parsing.
/// Used by both `RemiClient` (sync) and `AsyncRemiClient` (async) to avoid
/// duplicating these operations.
pub(super) struct RemiClientCore {
    pub(super) base_url: String,
    pub(super) poll_interval: Duration,
}

impl RemiClientCore {
    /// Create a new core, validating and normalizing the base URL.
    pub(super) fn new(base_url: &str) -> Result<Self> {
        crate::repository::client::validate_url_scheme(base_url)?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            poll_interval: POLL_INTERVAL,
        })
    }

    pub(super) async fn wait_for_next_poll(&self) {
        tokio::time::sleep(self.poll_interval).await;
    }

    /// Construct a package URL with optional version and architecture query parameters.
    pub(super) fn package_url(
        &self,
        distro: &str,
        name: &str,
        version: Option<&str>,
        release: Option<&str>,
        architecture: Option<&str>,
    ) -> String {
        let encoded_distro = urlencoding::encode(distro);
        let encoded_name = urlencoding::encode(name);
        let base = format!(
            "{}/v1/{encoded_distro}/packages/{encoded_name}",
            self.base_url
        );
        let mut query = Vec::new();
        if let Some(v) = version {
            let encoded_version = urlencoding::encode(v);
            query.push(format!("version={encoded_version}"));
        }
        if let Some(release) = release {
            let encoded_release = urlencoding::encode(release);
            query.push(format!("release={encoded_release}"));
        }
        if let Some(arch) = architecture {
            let encoded_arch = urlencoding::encode(arch);
            query.push(format!("arch={encoded_arch}"));
        }
        if query.is_empty() {
            base
        } else {
            format!("{base}?{}", query.join("&"))
        }
    }

    /// Construct a direct download URL.
    pub(super) fn download_url(
        &self,
        distro: &str,
        name: &str,
        version: Option<&str>,
        release: Option<&str>,
        architecture: Option<&str>,
    ) -> String {
        let package_url = self.package_url(distro, name, version, release, architecture);
        if let Some((path, query)) = package_url.split_once('?') {
            format!("{path}/download?{query}")
        } else {
            format!("{package_url}/download")
        }
    }

    /// Construct a job poll URL.
    pub(super) fn job_url(&self, job_id: &str) -> String {
        format!("{}/v1/jobs/{}", self.base_url, job_id)
    }

    /// Construct a chunk URL.
    pub(super) fn chunk_url(&self, hash: &str) -> String {
        format!("{}/v1/chunks/{}", self.base_url, hash)
    }

    /// Construct a health check URL.
    pub(super) fn health_url(&self) -> String {
        format!("{}/health", self.base_url)
    }

    /// Map non-success HTTP status codes to domain errors.
    pub(super) fn map_http_error(
        &self,
        status: u16,
        body: String,
        name: &str,
        distro: &str,
    ) -> Error {
        match status {
            404 => Error::NotFound(format!(
                "Package '{}' not found in {} repositories",
                name, distro
            )),
            503 => {
                Error::DownloadError("Remi conversion queue is full, try again later".to_string())
            }
            _ => Error::DownloadError(format!("Remi returned HTTP {}: {}", status, body)),
        }
    }
}
