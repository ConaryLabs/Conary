// conary-core/src/self_update/versioning.rs

use crate::error::{Error, Result};
use serde::Deserialize;
use url::Url;

/// Maximum size of the `/latest` JSON response before deserialization (1 MiB).
const MAX_SELF_UPDATE_METADATA_SIZE: usize = 1024 * 1024;

/// Response from the /latest endpoint
#[derive(Debug, Clone, Deserialize)]
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

pub async fn fetch_latest_version_info(
    channel_url: &str,
    user_agent: &str,
) -> Result<LatestVersionInfo> {
    use std::time::Duration;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| Error::IoError(format!("Failed to create HTTP client: {e}")))?;

    let url = format!("{channel_url}/latest");
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
    let info = parse_latest_version_info_bytes(&bytes)?;
    validate_download_origin(channel_url, &info.download_url)?;
    Ok(info)
}

/// Check for available updates by querying the update channel
pub async fn check_for_update(
    channel_url: &str,
    current_version: &str,
) -> Result<VersionCheckResult> {
    let info = fetch_latest_version_info(channel_url, &format!("conary/{current_version}")).await?;

    if is_newer(current_version, &info.version) {
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

/// Compare two semver version strings. Returns true if `remote` is newer than `current`.
///
/// Handles pre-release versions per SemVer rules:
/// - A pre-release version (e.g., `1.0.0-alpha.1`) is always older than its
///   release counterpart (`1.0.0`).
/// - Pre-release identifiers are compared left-to-right: numeric identifiers
///   are compared as integers, alphanumeric identifiers are compared
///   lexicographically, and numeric identifiers always sort before
///   alphanumeric ones.
pub fn is_newer(current: &str, remote: &str) -> bool {
    let parse = |v: &str| -> ((u64, u64, u64), Option<Vec<String>>) {
        let parts: Vec<&str> = v.split('.').collect();
        let major = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
        let minor = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);

        let patch_str = parts.get(2).copied().unwrap_or("0");
        let (patch_num, prerelease) = if let Some(dash_pos) = patch_str.find('-') {
            let patch = patch_str[..dash_pos].parse().unwrap_or(0);
            let mut pre_parts: Vec<String> = patch_str[dash_pos + 1..]
                .split('.')
                .map(String::from)
                .collect();
            for part in parts.iter().skip(3) {
                pre_parts.push(part.to_string());
            }
            (patch, Some(pre_parts))
        } else {
            (patch_str.parse().unwrap_or(0), None)
        };

        ((major, minor, patch_num), prerelease)
    };

    let (remote_ver, remote_pre) = parse(remote);
    let (current_ver, current_pre) = parse(current);

    match remote_ver.cmp(&current_ver) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => match (&current_pre, &remote_pre) {
            (None, None) => false,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (Some(cur), Some(rem)) => compare_prerelease(rem, cur) == std::cmp::Ordering::Greater,
        },
    }
}

fn compare_prerelease(a: &[String], b: &[String]) -> std::cmp::Ordering {
    for (ai, bi) in a.iter().zip(b.iter()) {
        let a_num = ai.parse::<u64>();
        let b_num = bi.parse::<u64>();
        let ord = match (a_num, b_num) {
            (Ok(an), Ok(bn)) => an.cmp(&bn),
            (Ok(_), Err(_)) => std::cmp::Ordering::Less,
            (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
            (Err(_), Err(_)) => ai.cmp(bi),
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }

    a.len().cmp(&b.len())
}

fn parse_latest_version_info_bytes(bytes: &[u8]) -> Result<LatestVersionInfo> {
    if bytes.len() > MAX_SELF_UPDATE_METADATA_SIZE {
        return Err(Error::ParseError(format!(
            "Update response too large ({} bytes, max {} bytes)",
            bytes.len(),
            MAX_SELF_UPDATE_METADATA_SIZE
        )));
    }

    serde_json::from_slice(bytes)
        .map_err(|e| Error::ParseError(format!("Invalid update response: {e}")))
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

    #[test]
    fn test_is_newer() {
        assert!(is_newer("0.1.0", "0.2.0"));
        assert!(is_newer("0.1.0", "0.1.1"));
        assert!(is_newer("0.1.0", "1.0.0"));
        assert!(!is_newer("0.2.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("1.0.0", "0.9.9"));
    }

    #[test]
    fn test_is_newer_edge_cases() {
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(is_newer("0.99.99", "1.0.0"));
        assert!(is_newer("1", "2"));
        assert!(is_newer("1.0", "1.1"));
    }

    #[test]
    fn test_is_newer_prerelease() {
        assert!(!is_newer("1.0.0", "1.0.0-alpha.1"));
        assert!(is_newer("1.0.0-alpha.1", "1.0.0"));
        assert!(is_newer("1.0.0-alpha.1", "1.0.0-alpha.2"));
        assert!(is_newer("1.0.0-alpha.1", "1.0.0-beta.1"));
        assert!(!is_newer("1.0.0-beta.1", "1.0.0-alpha.1"));
        assert!(!is_newer("1.0.0-alpha.1", "1.0.0-alpha.1"));
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
        assert!(is_newer("1.9.9", "2.0.0"));
        assert!(!is_newer("2.0.0", "1.9.9"));
        assert!(is_newer("1.0.9", "1.1.0"));
        assert!(!is_newer("1.1.0", "1.0.9"));
    }
}
