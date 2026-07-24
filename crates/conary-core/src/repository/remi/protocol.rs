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
    pub status: String,
    pub distro: String,
    pub package: String,
    pub version: Option<String>,
    pub architecture: Option<String>,
    pub progress: Option<u8>,
    pub error: Option<String>,
    pub manifest: Option<PackageManifest>,
    pub publication: Option<PublicationGateReport>,
}

/// Scriptlet publication report returned by Remi when a conversion cannot be
/// served publicly.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PublicationGateReport {
    #[serde(default)]
    pub publication_status: String,
    #[serde(default)]
    pub scriptlet_fidelity: String,
    #[serde(default)]
    pub target_compatibility: String,
    #[serde(default)]
    pub summary_valid: bool,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub reason_codes: Vec<String>,
    #[serde(default)]
    pub blocked_reason_codes: Vec<String>,
    #[serde(default)]
    pub review_reason_codes: Vec<String>,
    #[serde(default)]
    pub unknown_command_evidence: Vec<crate::ccs::legacy_scriptlets::UnknownCommandEvidence>,
    #[serde(default)]
    pub blocked_classes: Vec<String>,
    #[serde(default)]
    pub boot_security_intents: Vec<crate::ccs::legacy_scriptlets::BootSecurityIntentEvidence>,
    #[serde(default)]
    pub evidence_digest: Option<String>,
    #[serde(default)]
    pub curation_evidence_digest: Option<String>,
    #[serde(default)]
    pub review_artifact_available: bool,
}

#[derive(Debug, Deserialize)]
struct PublicationRefusalResponse {
    status: String,
    message: String,
    distro: String,
    package: String,
    version: Option<String>,
    scriptlets: PublicationGateReport,
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
}

impl RemiClientCore {
    /// Create a new core, validating and normalizing the base URL.
    pub(super) fn new(base_url: &str) -> Result<Self> {
        crate::repository::client::validate_url_scheme(base_url)?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
        })
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
            403 | 409 => {
                if let Ok(refusal) = serde_json::from_str::<PublicationRefusalResponse>(&body) {
                    Error::DownloadError(format_publication_refusal_response(&refusal))
                } else {
                    Error::DownloadError(format!("Remi returned HTTP {}: {}", status, body))
                }
            }
            _ => Error::DownloadError(format!("Remi returned HTTP {}: {}", status, body)),
        }
    }
}

pub(super) fn terminal_publication_status_error(status: &JobStatus) -> Option<Error> {
    match status.status.as_str() {
        "review-required" | "blocked" => Some(Error::DownloadError(format_publication_refusal(
            &status.status,
            &status.distro,
            &status.package,
            status.version.as_deref(),
            status.publication.as_ref(),
            None,
        ))),
        _ => None,
    }
}

fn format_publication_refusal_response(refusal: &PublicationRefusalResponse) -> String {
    format_publication_refusal(
        &refusal.status,
        &refusal.distro,
        &refusal.package,
        refusal.version.as_deref(),
        Some(&refusal.scriptlets),
        Some(refusal.message.as_str()),
    )
}

fn format_publication_refusal(
    status: &str,
    distro: &str,
    package: &str,
    version: Option<&str>,
    report: Option<&PublicationGateReport>,
    server_message: Option<&str>,
) -> String {
    let package_ref = match version {
        Some(version) => format!("{distro}/{package} {version}"),
        None => format!("{distro}/{package}"),
    };
    let status_summary = match status {
        "blocked" => "blocked by Remi's legacy scriptlet publication policy",
        "review-required" => "held for scriptlet review before public serving",
        other => {
            return format!("Remi returned terminal conversion status {other} for {package_ref}");
        }
    };

    let mut message = format!("Remi refused to serve {package_ref}: {status_summary}.");
    if let Some(server_message) = server_message.filter(|message| !message.trim().is_empty()) {
        message.push(' ');
        message.push_str(server_message.trim());
    } else if let Some(report_message) = report
        .map(|report| report.message.trim())
        .filter(|message| !message.is_empty())
    {
        message.push(' ');
        message.push_str(report_message);
    }

    if let Some(report) = report {
        if !report.blocked_classes.is_empty() {
            message.push_str(" blocked classes: ");
            message.push_str(&report.blocked_classes.join(", "));
            message.push('.');
        }
        if !report.reason_codes.is_empty() {
            message.push_str(" reason codes: ");
            message.push_str(&report.reason_codes.join(", "));
            message.push('.');
        }
        if !report.unknown_command_evidence.is_empty() {
            message.push_str(" unknown command evidence: ");
            message.push_str(
                &report
                    .unknown_command_evidence
                    .iter()
                    .map(|evidence| {
                        let mut shape = evidence.command.clone();
                        if !evidence.argv.is_empty() {
                            shape.push(' ');
                            shape.push_str(&evidence.argv.join(" "));
                        }
                        if let Some(phase) = &evidence.phase {
                            shape.push_str(" [");
                            shape.push_str(phase);
                            shape.push(']');
                        }
                        shape
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            message.push('.');
        }
        if report_contains_boot_security_class(report) {
            message.push_str(
                " kernel/initramfs/SELinux scriptlets are boot- or security-critical and are not supported by the Remi public preview yet; Conary will not bypass them with --no-scripts or raw legacy replay.",
            );
        }
        if !report.boot_security_intents.is_empty() {
            message.push_str(" Boot/security scriptlet evidence:");
            for intent in &report.boot_security_intents {
                let args = if intent.argv.is_empty() {
                    String::new()
                } else {
                    format!(" {}", intent.argv.join(" "))
                };
                message.push_str(&format!(
                    " {}: {}{}.",
                    intent.class_id, intent.command, args
                ));
            }
        }
    } else {
        message.push_str(" Remi did not include a scriptlet publication report.");
    }

    message
}

fn report_contains_boot_security_class(report: &PublicationGateReport) -> bool {
    report
        .blocked_classes
        .iter()
        .chain(report.reason_codes.iter())
        .chain(report.blocked_reason_codes.iter())
        .any(|value| {
            matches!(
                value.as_str(),
                "selinux"
                    | "initramfs"
                    | "kernel-module"
                    | "blocked-class-selinux"
                    | "blocked-class-initramfs"
                    | "blocked-class-kernel-module"
            )
        })
}
