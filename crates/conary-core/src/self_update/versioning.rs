// conary-core/src/self_update/versioning.rs

use crate::error::{Error, Result};
use semver::Version;
use serde::Deserialize;
use url::Url;

/// Maximum size of the `/latest` JSON response before deserialization (1 MiB).
const MAX_SELF_UPDATE_METADATA_SIZE: usize = 1024 * 1024;
/// Self-update artifacts are bounded independently of HTTP metadata.
const MAX_SELF_UPDATE_PACKAGE_SIZE: u64 = 512 * 1024 * 1024;

/// Response from the /latest endpoint
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LatestVersionInfo {
    pub version: String,
    pub download_url: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub signature: Option<String>,
}

/// Result of a version check
#[derive(Debug, Clone, PartialEq)]
pub enum VersionCheckResult {
    /// A newer version is available
    UpdateAvailable {
        current: String,
        latest: String,
        download_url: String,
        sha256: String,
        size: u64,
        signature: Option<String>,
    },
    /// Already at the latest version
    UpToDate { version: String },
}

/// Ensure update metadata cannot redirect downloads to a different origin.
pub fn validate_download_origin(channel_url: &str, download_url: &str) -> Result<()> {
    let channel = Url::parse(channel_url)
        .map_err(|e| Error::ParseError(format!("Invalid update channel URL: {e}")))?;
    let download = Url::parse(download_url)
        .map_err(|e| Error::ParseError(format!("Invalid update download URL: {e}")))?;

    let same_origin = channel.scheme() == download.scheme()
        && channel.host_str() == download.host_str()
        && channel.port_or_known_default() == download.port_or_known_default();

    if !same_origin {
        return Err(Error::DownloadError(format!(
            "Update download URL origin mismatch: {download_url} does not match channel {channel_url}"
        )));
    }

    Ok(())
}

fn build_http_client() -> Result<reqwest::Client> {
    use std::time::Duration;

    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| Error::IoError(format!("Failed to create HTTP client: {e}")))
}

fn resolve_download_url(channel_url: &str, download_url: &str) -> Result<String> {
    let channel = Url::parse(channel_url)
        .map_err(|e| Error::ParseError(format!("Invalid update channel URL: {e}")))?;

    match Url::parse(download_url) {
        Ok(url) => Ok(url.to_string()),
        Err(url::ParseError::RelativeUrlWithoutBase) => channel
            .join(download_url)
            .map(|url| url.to_string())
            .map_err(|e| Error::ParseError(format!("Invalid update download URL: {e}"))),
        Err(e) => Err(Error::ParseError(format!(
            "Invalid update download URL: {e}"
        ))),
    }
}

async fn fetch_version_metadata(
    channel_url: &str,
    metadata_path: &str,
    user_agent: &str,
) -> Result<LatestVersionInfo> {
    let client = build_http_client()?;

    let url = format!("{channel_url}/{metadata_path}");
    let response = client
        .get(&url)
        .header("User-Agent", user_agent)
        .send()
        .await
        .map_err(|e| Error::IoError(format!("Failed to check for updates: {e}")))?;

    if !response.status().is_success() {
        return Err(Error::IoError(format!(
            "Update check failed: HTTP {}",
            response.status()
        )));
    }

    let bytes =
        read_limited_response_bytes(response, MAX_SELF_UPDATE_METADATA_SIZE, "update response")
            .await?;
    let mut info = parse_latest_version_info_bytes(&bytes)?;
    info.download_url = resolve_download_url(channel_url, &info.download_url)?;
    validate_download_origin(channel_url, &info.download_url)?;
    Ok(info)
}

pub async fn fetch_latest_version_info(
    channel_url: &str,
    user_agent: &str,
) -> Result<LatestVersionInfo> {
    fetch_version_metadata(channel_url, "latest", user_agent).await
}

pub async fn fetch_version_info(
    channel_url: &str,
    version: &str,
    user_agent: &str,
) -> Result<LatestVersionInfo> {
    fetch_version_metadata(channel_url, version, user_agent).await
}

/// Check for available updates by querying the update channel
pub async fn check_for_update(
    channel_url: &str,
    current_version: &str,
) -> Result<VersionCheckResult> {
    let info = fetch_latest_version_info(channel_url, &format!("conary/{current_version}")).await?;

    if is_newer(current_version, &info.version)? {
        Ok(VersionCheckResult::UpdateAvailable {
            current: current_version.to_string(),
            latest: info.version,
            download_url: info.download_url,
            sha256: info.sha256,
            size: info.size,
            signature: info.signature,
        })
    } else {
        Ok(VersionCheckResult::UpToDate {
            version: current_version.to_string(),
        })
    }
}

/// Compare two strict SemVer values using the upstream SemVer implementation.
///
/// Invalid local or remote release metadata is an error. Self-update must not
/// reinterpret malformed versions as zeroes or accept abbreviated versions.
pub fn is_newer(current: &str, remote: &str) -> Result<bool> {
    let current = Version::parse(current).map_err(|error| {
        Error::ParseError(format!(
            "Invalid current self-update version {current:?}: {error}"
        ))
    })?;
    let remote = Version::parse(remote).map_err(|error| {
        Error::ParseError(format!(
            "Invalid remote self-update version {remote:?}: {error}"
        ))
    })?;
    Ok(remote > current)
}

fn parse_latest_version_info_bytes(bytes: &[u8]) -> Result<LatestVersionInfo> {
    if bytes.len() > MAX_SELF_UPDATE_METADATA_SIZE {
        return Err(Error::ParseError(format!(
            "Update response too large ({} bytes, max {} bytes)",
            bytes.len(),
            MAX_SELF_UPDATE_METADATA_SIZE
        )));
    }

    let info: LatestVersionInfo = serde_json::from_slice(bytes)
        .map_err(|e| Error::ParseError(format!("Invalid update response: {e}")))?;
    Version::parse(&info.version).map_err(|error| {
        Error::ParseError(format!(
            "Invalid self-update metadata version {:?}: {error}",
            info.version
        ))
    })?;
    if info.sha256.len() != 64 || !info.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::ParseError(
            "Invalid self-update metadata SHA-256: expected exactly 64 hexadecimal characters"
                .to_string(),
        ));
    }
    if info.size == 0 {
        return Err(Error::ParseError(
            "Invalid self-update metadata size: update artifact must not be empty".to_string(),
        ));
    }
    if info.size > MAX_SELF_UPDATE_PACKAGE_SIZE {
        return Err(Error::ParseError(format!(
            "Invalid self-update metadata size: {} bytes exceeds {} byte limit",
            info.size, MAX_SELF_UPDATE_PACKAGE_SIZE
        )));
    }
    if info.download_url.trim().is_empty() {
        return Err(Error::ParseError(
            "Invalid self-update metadata download URL: value must not be empty".to_string(),
        ));
    }
    if info
        .signature
        .as_ref()
        .is_some_and(|signature| signature.trim().is_empty())
    {
        return Err(Error::ParseError(
            "Invalid self-update metadata signature: value must not be empty".to_string(),
        ));
    }
    Ok(info)
}

async fn read_limited_response_bytes(
    mut response: reqwest::Response,
    limit: usize,
    context: &str,
) -> Result<Vec<u8>> {
    if let Some(content_length) = response.content_length()
        && content_length > limit as u64
    {
        return Err(Error::ParseError(format!(
            "{context} too large ({} bytes, max {} bytes)",
            content_length, limit
        )));
    }

    let mut bytes = Vec::new();
    let mut total = 0usize;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| Error::IoError(format!("Failed to read {context}: {e}")))?
    {
        total += chunk.len();
        if total > limit {
            return Err(Error::ParseError(format!(
                "{context} too large ({} bytes, max {} bytes)",
                total, limit
            )));
        }
        bytes.extend_from_slice(&chunk);
    }

    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn spawn_metadata_server(body: String) -> (String, Arc<Mutex<Option<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen_path = Arc::new(Mutex::new(None));
        let seen_path_task = Arc::clone(&seen_path);

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 4096];
            let bytes_read = stream.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..bytes_read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/")
                .to_string();
            *seen_path_task.lock().unwrap() = Some(path);

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });

        (format!("http://{addr}"), seen_path)
    }

    #[test]
    fn test_is_newer() {
        assert!(is_newer("0.1.0", "0.2.0").unwrap());
        assert!(is_newer("0.1.0", "0.1.1").unwrap());
        assert!(is_newer("0.1.0", "1.0.0").unwrap());
        assert!(!is_newer("0.2.0", "0.1.0").unwrap());
        assert!(!is_newer("0.1.0", "0.1.0").unwrap());
        assert!(!is_newer("1.0.0", "0.9.9").unwrap());
    }

    #[test]
    fn test_is_newer_edge_cases() {
        assert!(!is_newer("1.0.0", "1.0.0").unwrap());
        assert!(is_newer("0.99.99", "1.0.0").unwrap());
        assert!(is_newer("1", "2.0.0").is_err());
        assert!(is_newer("1.0.0", "1.1").is_err());
    }

    #[test]
    fn test_is_newer_prerelease() {
        assert!(!is_newer("1.0.0", "1.0.0-alpha.1").unwrap());
        assert!(is_newer("1.0.0-alpha.1", "1.0.0").unwrap());
        assert!(is_newer("1.0.0-alpha.1", "1.0.0-alpha.2").unwrap());
        assert!(is_newer("1.0.0-alpha.1", "1.0.0-beta.1").unwrap());
        assert!(!is_newer("1.0.0-beta.1", "1.0.0-alpha.1").unwrap());
        assert!(!is_newer("1.0.0-alpha.1", "1.0.0-alpha.1").unwrap());
    }

    #[test]
    fn test_validate_download_origin_accepts_same_origin() {
        validate_download_origin(
            "https://remi.conary.io/v1/ccs/conary",
            "https://remi.conary.io/releases/conary-0.7.0.ccs",
        )
        .unwrap();
    }

    #[test]
    fn test_validate_download_origin_rejects_different_host() {
        let err = validate_download_origin(
            "https://remi.conary.io/v1/ccs/conary",
            "https://evil.example/releases/conary-0.7.0.ccs",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("origin mismatch"));
    }

    #[test]
    fn test_validate_download_origin_rejects_different_scheme() {
        let err = validate_download_origin(
            "https://remi.conary.io/v1/ccs/conary",
            "http://remi.conary.io/releases/conary-0.7.0.ccs",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("origin mismatch"));
    }

    #[test]
    fn test_parse_latest_version_info_rejects_large_response() {
        let oversized = vec![b'{'; MAX_SELF_UPDATE_METADATA_SIZE + 1];
        let err = parse_latest_version_info_bytes(&oversized).unwrap_err();
        assert!(err.to_string().contains("too large"));
    }

    #[test]
    fn parse_latest_version_info_rejects_inexact_authority_fields() {
        let valid = r#"{
            "version":"1.2.3",
            "download_url":"/v1/ccs/conary/1.2.3/download",
            "sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "size":12345
        }"#;
        parse_latest_version_info_bytes(valid.as_bytes()).unwrap();

        for (label, invalid) in [
            ("version", valid.replace("\"1.2.3\"", "\"latest\"")),
            (
                "SHA-256",
                valid.replace(
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                    "missing",
                ),
            ),
            (
                "artifact must not be empty",
                valid.replace("\"size\":12345", "\"size\":0"),
            ),
            (
                "exceeds",
                valid.replace(
                    "\"size\":12345",
                    &format!("\"size\":{}", MAX_SELF_UPDATE_PACKAGE_SIZE + 1),
                ),
            ),
            (
                "unknown field",
                valid.replace("\"size\":12345", "\"size\":12345,\"legacy\":true"),
            ),
        ] {
            let error = parse_latest_version_info_bytes(invalid.as_bytes()).unwrap_err();
            assert!(
                error.to_string().contains(label),
                "expected {label:?} in {error}"
            );
        }
    }

    #[test]
    fn test_version_check_result_variants() {
        let up_to_date = VersionCheckResult::UpToDate {
            version: "0.1.0".to_string(),
        };
        assert_eq!(
            up_to_date,
            VersionCheckResult::UpToDate {
                version: "0.1.0".to_string()
            }
        );

        let update = VersionCheckResult::UpdateAvailable {
            current: "0.1.0".to_string(),
            latest: "0.2.0".to_string(),
            download_url: "https://example.com/conary-0.2.0.ccs".to_string(),
            sha256: "abc123".to_string(),
            size: 12_000_000,
            signature: None,
        };
        match &update {
            VersionCheckResult::UpdateAvailable {
                current, latest, ..
            } => {
                assert_eq!(current, "0.1.0");
                assert_eq!(latest, "0.2.0");
            }
            _ => panic!("Expected UpdateAvailable"),
        }
    }

    #[test]
    fn test_is_newer_major_minor_patch() {
        assert!(is_newer("1.9.9", "2.0.0").unwrap());
        assert!(!is_newer("2.0.0", "1.9.9").unwrap());
        assert!(is_newer("1.0.9", "1.1.0").unwrap());
        assert!(!is_newer("1.1.0", "1.0.9").unwrap());
    }

    #[tokio::test]
    async fn test_fetch_version_info_uses_version_endpoint() {
        let body = r#"{
            "version":"1.2.3",
            "download_url":"/v1/ccs/conary/1.2.3/download",
            "sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "size":12345
        }"#
        .to_string();
        let (base_url, seen_path) = spawn_metadata_server(body).await;
        let channel_url = format!("{base_url}/v1/ccs/conary");

        let info = fetch_version_info(&channel_url, "1.2.3", "conary/test")
            .await
            .unwrap();

        assert_eq!(info.version, "1.2.3");
        assert_eq!(
            seen_path.lock().unwrap().as_deref(),
            Some("/v1/ccs/conary/1.2.3")
        );
    }

    #[tokio::test]
    async fn test_fetch_latest_version_info_resolves_relative_download_url() {
        let body = r#"{
            "version":"1.2.4",
            "download_url":"/v1/ccs/conary/1.2.4/download",
            "sha256":"fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210",
            "size":67890
        }"#
        .to_string();
        let (base_url, _seen_path) = spawn_metadata_server(body).await;
        let channel_url = format!("{base_url}/v1/ccs/conary");

        let info = fetch_latest_version_info(&channel_url, "conary/test")
            .await
            .unwrap();

        assert_eq!(
            info.download_url,
            format!("{base_url}/v1/ccs/conary/1.2.4/download")
        );
    }
}
