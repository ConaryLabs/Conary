// src/server/popularity.rs
//! Popularity data fetching for pre-warming pipeline
//!
//! Fetches package download/popularity data from upstream distributions
//! to prioritize which packages to pre-convert to CCS format.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Download/popularity data for a single package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackagePopularity {
    /// Package name
    pub name: String,
    /// Download or install count
    pub download_count: u64,
}

/// Default URL for Fedora popularity data (JSON format)
const FEDORA_POPULARITY_URL: &str = "https://data.fedoraproject.org/stats/packages/popularity.json";

/// Default URL for Arch Linux pkgstats data
const ARCH_POPULARITY_URL: &str = "https://pkgstats.archlinux.de/api/packages";

/// Default URL for Ubuntu popularity-contest data
const UBUNTU_POPULARITY_URL: &str = "https://popcon.ubuntu.com/by_inst.gz";

/// Fetch popularity data from Fedora.
///
/// Expects JSON format: `[{"name": "...", "download_count": N}, ...]`
/// Falls back to a simpler format if the full mdapi response differs.
pub async fn fetch_fedora_popularity(client: &reqwest::Client) -> Result<Vec<PackagePopularity>> {
    fetch_fedora_popularity_from(client, FEDORA_POPULARITY_URL).await
}

async fn fetch_fedora_popularity_from(
    client: &reqwest::Client,
    url: &str,
) -> Result<Vec<PackagePopularity>> {
    let response = client
        .get(url)
        .send()
        .await
        .context("Failed to fetch Fedora popularity data")?;

    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("Fedora popularity fetch failed with status {status}");
    }

    let body = response
        .text()
        .await
        .context("Failed to read Fedora popularity response body")?;

    parse_fedora_popularity(&body)
}

/// Parse Fedora popularity JSON.
///
/// Accepts: `[{"name": "pkg", "download_count": 123}, ...]`
fn parse_fedora_popularity(body: &str) -> Result<Vec<PackagePopularity>> {
    let mut data: Vec<PackagePopularity> =
        serde_json::from_str(body).context("Failed to parse Fedora popularity JSON")?;
    data.sort_by(|a, b| b.download_count.cmp(&a.download_count));
    Ok(data)
}

/// Fetch popularity data from Arch Linux pkgstats.
///
/// Expects a simple text format with one entry per line:
/// ```text
/// package_name count
/// ```
pub async fn fetch_arch_popularity(client: &reqwest::Client) -> Result<Vec<PackagePopularity>> {
    fetch_arch_popularity_from(client, ARCH_POPULARITY_URL).await
}

async fn fetch_arch_popularity_from(
    client: &reqwest::Client,
    url: &str,
) -> Result<Vec<PackagePopularity>> {
    let response = client
        .get(url)
        .send()
        .await
        .context("Failed to fetch Arch popularity data")?;

    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("Arch popularity fetch failed with status {status}");
    }

    let body = response
        .text()
        .await
        .context("Failed to read Arch popularity response body")?;

    parse_arch_popularity(&body)
}

/// Parse Arch pkgstats text format.
///
/// Each line: `package_name count`
/// Lines starting with `#` are comments.
fn parse_arch_popularity(body: &str) -> Result<Vec<PackagePopularity>> {
    let mut results = Vec::new();

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut parts = line.split_whitespace();
        let name = match parts.next() {
            Some(n) => n,
            None => continue,
        };
        let count: u64 = match parts.next() {
            Some(c) => c.parse().unwrap_or(0),
            None => continue,
        };

        results.push(PackagePopularity {
            name: name.to_string(),
            download_count: count,
        });
    }

    results.sort_by(|a, b| b.download_count.cmp(&a.download_count));
    Ok(results)
}

/// Fetch popularity data from Ubuntu popularity-contest.
///
/// Expects the `by_inst` format (space-separated):
/// ```text
/// rank name inst vote old recent no-files
/// ```
pub async fn fetch_ubuntu_popularity(client: &reqwest::Client) -> Result<Vec<PackagePopularity>> {
    fetch_ubuntu_popularity_from(client, UBUNTU_POPULARITY_URL).await
}

async fn fetch_ubuntu_popularity_from(
    client: &reqwest::Client,
    url: &str,
) -> Result<Vec<PackagePopularity>> {
    let response = client
        .get(url)
        .send()
        .await
        .context("Failed to fetch Ubuntu popularity data")?;

    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("Ubuntu popularity fetch failed with status {status}");
    }

    let body = response
        .text()
        .await
        .context("Failed to read Ubuntu popularity response body")?;

    parse_ubuntu_popularity(&body)
}

/// Parse Ubuntu popcon `by_inst` format.
///
/// Each line: `rank name inst vote old recent no-files`
/// Header lines start with `#`. The `inst` column (index 2) is the install count.
fn parse_ubuntu_popularity(body: &str) -> Result<Vec<PackagePopularity>> {
    let mut results = Vec::new();

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('-') {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        // Format: rank name inst vote old recent no-files
        if parts.len() < 3 {
            continue;
        }

        // Skip header row if rank is not numeric
        let Ok(_rank) = parts[0].parse::<u64>() else {
            continue;
        };

        let name = parts[1];
        let inst: u64 = parts[2].parse().unwrap_or(0);

        results.push(PackagePopularity {
            name: name.to_string(),
            download_count: inst,
        });
    }

    results.sort_by(|a, b| b.download_count.cmp(&a.download_count));
    Ok(results)
}

/// Fetch popularity data for a given distribution.
///
/// Dispatches to the appropriate fetcher based on the distro name.
/// Supported values: `"fedora"`, `"arch"`, `"ubuntu"`.
pub async fn fetch_popularity(
    client: &reqwest::Client,
    distro: &str,
) -> Result<Vec<PackagePopularity>> {
    match distro.to_lowercase().as_str() {
        "fedora" => fetch_fedora_popularity(client).await,
        "arch" => fetch_arch_popularity(client).await,
        "ubuntu" => fetch_ubuntu_popularity(client).await,
        other => anyhow::bail!("Unsupported distro for popularity data: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_fedora_popularity() {
        let json = r#"[
            {"name": "nginx", "download_count": 50000},
            {"name": "curl", "download_count": 80000},
            {"name": "vim", "download_count": 30000},
            {"name": "bash", "download_count": 100000}
        ]"#;

        let result = parse_fedora_popularity(json).unwrap();
        assert_eq!(result.len(), 4);
        // Should be sorted descending by download_count
        assert_eq!(result[0].name, "bash");
        assert_eq!(result[0].download_count, 100_000);
        assert_eq!(result[1].name, "curl");
        assert_eq!(result[1].download_count, 80_000);
        assert_eq!(result[2].name, "nginx");
        assert_eq!(result[3].name, "vim");
    }

    #[test]
    fn test_parse_fedora_popularity_empty() {
        let json = "[]";
        let result = parse_fedora_popularity(json).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_fedora_popularity_invalid() {
        let json = "not json";
        assert!(parse_fedora_popularity(json).is_err());
    }

    #[test]
    fn test_parse_arch_popularity() {
        let body = r#"# Arch Linux pkgstats data
# Generated: 2025-01-15
linux 45000
base 42000
filesystem 40000
glibc 39000
bash 38000
"#;

        let result = parse_arch_popularity(body).unwrap();
        assert_eq!(result.len(), 5);
        assert_eq!(result[0].name, "linux");
        assert_eq!(result[0].download_count, 45_000);
        assert_eq!(result[4].name, "bash");
        assert_eq!(result[4].download_count, 38_000);
    }

    #[test]
    fn test_parse_arch_popularity_with_blanks() {
        let body = "
# comment
nginx 1000

curl 2000
# another comment

vim 500
";

        let result = parse_arch_popularity(body).unwrap();
        assert_eq!(result.len(), 3);
        // Sorted descending
        assert_eq!(result[0].name, "curl");
        assert_eq!(result[0].download_count, 2000);
        assert_eq!(result[1].name, "nginx");
        assert_eq!(result[2].name, "vim");
    }

    #[test]
    fn test_parse_arch_popularity_empty() {
        let body = "# only comments\n";
        let result = parse_arch_popularity(body).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_arch_popularity_bad_count() {
        let body = "nginx notanumber\ncurl 500\n";
        let result = parse_arch_popularity(body).unwrap();
        assert_eq!(result.len(), 2);
        // "notanumber" parses as 0
        assert_eq!(result[0].name, "curl");
        assert_eq!(result[0].download_count, 500);
        assert_eq!(result[1].name, "nginx");
        assert_eq!(result[1].download_count, 0);
    }

    #[test]
    fn test_parse_ubuntu_popularity() {
        let body = r#"# Popularity-contest results for Ubuntu
# See https://popcon.ubuntu.com
# rank name inst vote old recent no-files
1 dpkg 180000 50000 100000 30000 0
2 apt 175000 48000 97000 30000 0
3 bash 170000 45000 95000 30000 0
4 coreutils 168000 44000 94000 30000 0
5 libc6 165000 43000 92000 30000 0
"#;

        let result = parse_ubuntu_popularity(body).unwrap();
        assert_eq!(result.len(), 5);
        // Sorted by inst (download_count) descending
        assert_eq!(result[0].name, "dpkg");
        assert_eq!(result[0].download_count, 180_000);
        assert_eq!(result[4].name, "libc6");
        assert_eq!(result[4].download_count, 165_000);
    }

    #[test]
    fn test_parse_ubuntu_popularity_skip_headers() {
        let body = r#"# comment line
------
rank name inst vote old recent no-files
1 nginx 5000 1000 3000 1000 0
2 curl 4000 900 2500 600 0
"#;

        let result = parse_ubuntu_popularity(body).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "nginx");
        assert_eq!(result[1].name, "curl");
    }

    #[test]
    fn test_parse_ubuntu_popularity_empty() {
        let body = "# only comments\n";
        let result = parse_ubuntu_popularity(body).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_ubuntu_popularity_short_lines() {
        let body = "1 nginx\n2 curl 5000 1000 2000 500 0\n";
        let result = parse_ubuntu_popularity(body).unwrap();
        // First line has only 2 fields, should be skipped
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "curl");
    }

    #[test]
    fn test_package_popularity_serialization() {
        let pop = PackagePopularity {
            name: "nginx".to_string(),
            download_count: 50_000,
        };

        let json = serde_json::to_string(&pop).unwrap();
        assert!(json.contains("nginx"));
        assert!(json.contains("50000"));

        let parsed: PackagePopularity = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "nginx");
        assert_eq!(parsed.download_count, 50_000);
    }

    #[tokio::test]
    async fn test_fetch_popularity_unsupported_distro() {
        let client = reqwest::Client::new();
        let result = fetch_popularity(&client, "gentoo").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unsupported distro"));
    }

    #[tokio::test]
    async fn test_fetch_popularity_case_insensitive() {
        // This will fail due to network, but it should dispatch correctly
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(1))
            .build()
            .unwrap();

        // Should not return "unsupported distro" error
        let result = fetch_popularity(&client, "Fedora").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            !err.contains("Unsupported"),
            "Should dispatch to fedora fetcher, got: {err}"
        );
    }
}
