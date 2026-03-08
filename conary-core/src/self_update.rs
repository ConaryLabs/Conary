// conary-core/src/self_update.rs

//! Self-update logic for the conary binary
//!
//! Checks Remi for newer versions and handles downloading, verifying,
//! and atomically replacing the running binary.

use crate::db::models::settings;
use crate::error::{Error, Result};
use rusqlite::Connection;
use serde::Deserialize;

/// Default update channel URL
pub const DEFAULT_UPDATE_CHANNEL: &str = "https://packages.conary.io/v1/ccs/conary";

/// Settings key for the update channel override
const SETTINGS_KEY_UPDATE_CHANNEL: &str = "update-channel";

/// Response from the /latest endpoint
#[derive(Debug, Clone, Deserialize)]
pub struct LatestVersionInfo {
    pub version: String,
    pub download_url: String,
    pub sha256: String,
    pub size: u64,
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
    },
    /// Already at the latest version
    UpToDate {
        version: String,
    },
}

/// Get the update channel URL from settings or fall back to default
pub fn get_update_channel(conn: &Connection) -> Result<String> {
    match settings::get(conn, SETTINGS_KEY_UPDATE_CHANNEL)? {
        Some(url) => Ok(url),
        None => Ok(DEFAULT_UPDATE_CHANNEL.to_string()),
    }
}

/// Set a custom update channel URL
pub fn set_update_channel(conn: &Connection, url: &str) -> Result<()> {
    settings::set(conn, SETTINGS_KEY_UPDATE_CHANNEL, url)
}

/// Compare two semver version strings. Returns true if `remote` is newer than `current`.
pub fn is_newer(current: &str, remote: &str) -> bool {
    let parse = |v: &str| -> (u64, u64, u64) {
        let parts: Vec<&str> = v.split('.').collect();
        let major = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
        let minor = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let patch = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
        (major, minor, patch)
    };
    parse(remote) > parse(current)
}

/// Check for available updates by querying the update channel
pub fn check_for_update(
    channel_url: &str,
    current_version: &str,
) -> Result<VersionCheckResult> {
    use reqwest::blocking::Client;
    use std::time::Duration;

    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| Error::IoError(format!("Failed to create HTTP client: {e}")))?;

    let url = format!("{channel_url}/latest");
    let response = client
        .get(&url)
        .header("User-Agent", format!("conary/{current_version}"))
        .send()
        .map_err(|e| Error::IoError(format!("Failed to check for updates: {e}")))?;

    if !response.status().is_success() {
        return Err(Error::IoError(format!(
            "Update check failed: HTTP {}",
            response.status()
        )));
    }

    let info: LatestVersionInfo = response
        .json()
        .map_err(|e| Error::ParseError(format!("Invalid update response: {e}")))?;

    if is_newer(current_version, &info.version) {
        Ok(VersionCheckResult::UpdateAvailable {
            current: current_version.to_string(),
            latest: info.version,
            download_url: info.download_url,
            sha256: info.sha256,
            size: info.size,
        })
    } else {
        Ok(VersionCheckResult::UpToDate {
            version: current_version.to_string(),
        })
    }
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

    fn create_test_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        crate::db::schema::migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn test_get_update_channel_default() {
        let conn = create_test_db();
        let channel = get_update_channel(&conn).unwrap();
        assert_eq!(channel, DEFAULT_UPDATE_CHANNEL);
    }

    #[test]
    fn test_set_update_channel() {
        let conn = create_test_db();
        set_update_channel(&conn, "https://internal.example.com/conary").unwrap();
        let channel = get_update_channel(&conn).unwrap();
        assert_eq!(channel, "https://internal.example.com/conary");
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
}
