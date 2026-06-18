// conary-core/src/repository/sync/remi.rs

use crate::db::models::{
    CanonicalPackage, PackageImplementation, Repository, RepositoryPackage, RepositoryProvide,
    RepositoryRequirement,
};
use crate::error::{Error, Result};
use crate::repository::client::RepositoryClient;
use crate::repository::metadata::PackageSecurityAdvisoryMetadata;
use crate::repository::retry::RetryConfig;
use rusqlite::Connection;
use std::collections::HashSet;
use tracing::{debug, info, warn};

use super::apply_trusted_package_security_advisory;
use super::native::{
    extract_extra_metadata_provides, persist_native_sync_rows, split_on_version_op,
};
use super::types::{
    CanonicalMapSnapshot, RemiMetadataResponse, RemiPackageEntry, SyncedPackageRow,
};

pub(super) fn remi_sync_row(
    repo_id: i64,
    endpoint: String,
    distro: String,
    entry: RemiPackageEntry,
) -> Result<SyncedPackageRow> {
    let architecture = entry.architecture.clone();
    let package_release = entry
        .release
        .clone()
        .or_else(|| {
            entry
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.pointer("/identity/release"))
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    let mut query = vec![format!("version={}", urlencoding::encode(&entry.version))];
    if !package_release.is_empty() {
        query.push(format!("release={}", urlencoding::encode(&package_release)));
    }
    if let Some(architecture) = architecture.as_deref() {
        query.push(format!("arch={}", urlencoding::encode(architecture)));
    }
    let download_url = format!(
        "{endpoint}/v1/{distro}/packages/{}/download?{}",
        urlencoding::encode(&entry.name),
        query.join("&")
    );

    let mut package = RepositoryPackage::new(
        repo_id,
        entry.name.clone(),
        entry.version.clone(),
        "remi:server-verified".to_string(),
        0,
        download_url,
    );
    package.package_release = package_release;
    package.architecture = architecture;
    package.dependencies = entry
        .dependencies
        .as_ref()
        .map(|deps| serde_json::to_string(deps).unwrap_or_default());

    let mut metadata = entry.metadata.unwrap_or(serde_json::Value::Null);
    if let Some(advisory_value) = metadata.get("security_advisory").cloned() {
        match serde_json::from_value::<PackageSecurityAdvisoryMetadata>(advisory_value) {
            Ok(advisory) => {
                match apply_trusted_package_security_advisory(
                    &mut package,
                    &advisory,
                    "remi",
                    "unknown",
                ) {
                    Ok(normalized) => {
                        if let Some(object) = metadata.as_object_mut() {
                            object.insert("security_advisory".to_string(), normalized);
                        }
                    }
                    Err(error) => {
                        warn!(
                            "Ignoring untrusted Remi security advisory metadata for {} {}: {}",
                            entry.name, entry.version, error
                        );
                    }
                }
            }
            Err(error) => {
                warn!(
                    "Ignoring malformed Remi security advisory metadata for {} {}: {}",
                    entry.name, entry.version, error
                );
            }
        }
    }
    package.metadata = match metadata {
        serde_json::Value::Null => None,
        ref value => Some(value.to_string()),
    };

    let route = crate::repository::supported_profiles::route_by_slug(&distro)
        .ok_or_else(|| Error::ConfigError(format!("unsupported Remi distro route: {distro}")))?;
    let profile_id = route.public_profile_ids().first().ok_or_else(|| {
        Error::ConfigError(format!("no public profile for Remi distro route: {distro}"))
    })?;
    let profile = crate::repository::supported_profiles::profile_by_public_id(profile_id)
        .ok_or_else(|| {
            Error::ConfigError(format!(
                "profile disappeared for Remi distro route: {distro}"
            ))
        })?;
    let scheme = profile.version_scheme();
    let scheme_str = Some(match scheme {
        crate::repository::versioning::VersionScheme::Rpm => "rpm".to_string(),
        crate::repository::versioning::VersionScheme::Debian => "debian".to_string(),
        crate::repository::versioning::VersionScheme::Arch => "arch".to_string(),
    });
    package.distro = Some(distro.clone());
    package.version_scheme = scheme_str.clone();

    let mut self_provide = RepositoryProvide::new(
        0,
        entry.name.clone(),
        Some(entry.version.clone()),
        "package".to_string(),
        Some(entry.name.clone()),
    );
    if let Some(ref scheme) = scheme_str {
        self_provide = self_provide.with_version_scheme(scheme.clone());
    }

    let mut provides = vec![self_provide];
    provides.extend(extract_extra_metadata_provides(&metadata).into_iter().map(
        |(capability, version, raw)| {
            let mut provide =
                RepositoryProvide::new(0, capability, version, "package".to_string(), Some(raw));
            if let Some(ref scheme) = scheme_str {
                provide = provide.with_version_scheme(scheme.clone());
            }
            provide
        },
    ));

    let requirements = entry
        .dependencies
        .unwrap_or_default()
        .into_iter()
        .map(|raw| {
            let (capability, version_constraint) = parse_raw_dependency_entry(&raw);
            RepositoryRequirement::new(
                0,
                capability,
                version_constraint,
                "package".to_string(),
                "runtime".to_string(),
                Some(raw),
            )
        })
        .collect();

    Ok(SyncedPackageRow {
        package,
        provides,
        requirements,
        requirement_groups: Vec::new(),
        requirement_group_clauses: Vec::new(),
    })
}

pub(super) fn parse_raw_dependency_entry(entry: &str) -> (String, Option<String>) {
    match split_on_version_op(entry) {
        Some((name, op, version)) => (name, Some(format!("{op} {version}"))),
        None => (entry.trim().to_string(), None),
    }
}

/// Synchronize repository directly from a Remi metadata API
///
/// For repos with `default_strategy = "remi"`, fetches the package index from
/// the Remi server's `/v1/{distro}/metadata` endpoint instead of parsing
/// traditional repo formats (repomd.xml, Packages, etc.).
pub(super) async fn fetch_remi_sync_rows(repo: &Repository) -> Result<Vec<SyncedPackageRow>> {
    let distro = repo.default_strategy_distro.as_deref().ok_or_else(|| {
        Error::ConfigError(format!(
            "Repository '{}' has strategy 'remi' but no distro configured (use --remi-distro)",
            repo.name
        ))
    })?;

    let endpoint = repo
        .default_strategy_endpoint
        .as_deref()
        .unwrap_or(&repo.url)
        .trim_end_matches('/');

    let metadata_url = format!("{endpoint}/v1/{distro}/metadata");
    info!(
        "Syncing repository {} from Remi metadata: {}",
        repo.name, metadata_url
    );

    let client = RepositoryClient::new()?;
    let response =
        fetch_remi_metadata_with_retry(&client, &metadata_url, &RetryConfig::quick()).await?;

    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;

    let mut seen = HashSet::new();
    let mut synced_packages = Vec::new();
    for entry in response.packages {
        let key = (
            entry.name.clone(),
            entry.version.clone(),
            entry.release.clone(),
            entry.architecture.clone(),
        );
        if !seen.insert(key) {
            continue;
        }
        synced_packages.push(remi_sync_row(
            repo_id,
            endpoint.to_string(),
            distro.to_string(),
            entry,
        )?);
    }

    Ok(synced_packages)
}

pub(super) fn persist_remi_sync_rows(
    conn: &Connection,
    repo: &mut Repository,
    synced_packages: Vec<SyncedPackageRow>,
) -> Result<usize> {
    let mut repo_packages: Vec<RepositoryPackage> = synced_packages
        .iter()
        .map(|row| row.package.clone())
        .collect();
    let count = persist_native_sync_rows(conn, repo, &mut repo_packages, synced_packages)?;

    info!(
        "Synchronized {} packages from Remi repository {}",
        count, repo.name
    );
    Ok(count)
}

pub(super) async fn sync_repository_remi(
    conn: &Connection,
    repo: &mut Repository,
) -> Result<usize> {
    let synced_packages = fetch_remi_sync_rows(repo).await?;
    persist_remi_sync_rows(conn, repo, synced_packages)
}

async fn fetch_remi_metadata_with_retry(
    client: &RepositoryClient,
    metadata_url: &str,
    retry_policy: &RetryConfig,
) -> Result<RemiMetadataResponse> {
    let max_attempts = retry_policy.max_attempts.max(1);
    let mut last_error = None;

    for attempt in 1..=max_attempts {
        match fetch_remi_metadata_once(client, metadata_url).await {
            Ok(response) => return Ok(response),
            Err(error) => {
                if attempt < max_attempts {
                    let delay = retry_policy.delay_for_attempt(attempt);
                    warn!(
                        "Remi metadata fetch attempt {}/{} failed: {}; retrying in {:?}",
                        attempt, max_attempts, error, delay
                    );
                    tokio::time::sleep(delay).await;
                }
                last_error = Some(error);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        Error::DownloadError(format!(
            "Failed to fetch Remi metadata from {metadata_url}: no attempts were made"
        ))
    }))
}

async fn fetch_remi_metadata_once(
    client: &RepositoryClient,
    metadata_url: &str,
) -> Result<RemiMetadataResponse> {
    let bytes = client.download_to_bytes(metadata_url).await?;
    serde_json::from_slice(&bytes).map_err(|error| {
        Error::ParseError(format!(
            "Failed to parse Remi metadata from {}: {}",
            metadata_url, error
        ))
    })
}

/// Fetch the canonical package map from a Remi endpoint and persist it locally.
///
/// Downloads the full canonical map from `{endpoint}/v1/canonical/map` and upserts
/// each entry into `canonical_packages` and `package_implementations`. This is
/// non-fatal: callers should log failures at debug level and continue.
pub(super) async fn fetch_canonical_map_snapshot(endpoint: &str) -> Result<CanonicalMapSnapshot> {
    let url = format!("{}/v1/canonical/map", endpoint.trim_end_matches('/'));
    debug!("Fetching canonical map from {}", url);

    let client = RepositoryClient::new()?;
    let bytes = client.download_to_bytes(&url).await?;

    serde_json::from_slice(&bytes).map_err(|error| {
        Error::ParseError(format!("Failed to parse canonical map from {url}: {error}"))
    })
}

pub(super) fn persist_canonical_map(conn: &Connection, map: &CanonicalMapSnapshot) -> Result<u64> {
    let tx = conn.unchecked_transaction()?;
    let mut count = 0u64;

    for entry in &map.entries {
        let mut canonical = CanonicalPackage::new(entry.canonical.clone(), "package".to_string());
        let Some(canonical_id) = canonical.insert_or_ignore(&tx)? else {
            continue;
        };

        for (distro, distro_name) in &entry.implementations {
            let mut implementation = PackageImplementation::new(
                canonical_id,
                distro.clone(),
                distro_name.clone(),
                "remi".to_string(),
            );
            implementation.insert_or_ignore(&tx)?;
            count += 1;
        }
    }

    tx.commit()?;
    Ok(count)
}

pub(super) async fn fetch_and_persist_canonical_map(
    conn: &Connection,
    endpoint: &str,
) -> Result<u64> {
    let map = fetch_canonical_map_snapshot(endpoint).await?;
    persist_canonical_map(conn, &map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remi_entry_for_tests(name: &str, version: &str) -> RemiPackageEntry {
        RemiPackageEntry {
            name: name.to_string(),
            version: version.to_string(),
            release: None,
            converted: false,
            architecture: Some("x86_64".to_string()),
            dependencies: None,
            metadata: None,
        }
    }

    #[test]
    fn remi_sync_row_preserves_wire_architecture() {
        let row = remi_sync_row(
            7,
            "http://remi.test".to_string(),
            "fedora".to_string(),
            RemiPackageEntry {
                name: "qemu-img".to_string(),
                version: "2:10.1.0-7.fc44".to_string(),
                release: None,
                converted: false,
                architecture: Some("x86_64".to_string()),
                dependencies: None,
                metadata: None,
            },
        )
        .unwrap();

        assert_eq!(row.package.architecture.as_deref(), Some("x86_64"));
    }

    #[test]
    fn remi_sync_row_preserves_release_and_exact_download_url() {
        let row = remi_sync_row(
            7,
            "https://remi.example.test".to_string(),
            "fedora".to_string(),
            RemiPackageEntry {
                name: "hello".to_string(),
                version: "1.0.0".to_string(),
                release: Some("2".to_string()),
                converted: false,
                architecture: Some("noarch".to_string()),
                dependencies: None,
                metadata: None,
            },
        )
        .unwrap();

        assert_eq!(row.package.package_release, "2");
        assert_eq!(
            row.package.download_url,
            "https://remi.example.test/v1/fedora/packages/hello/download?version=1.0.0&release=2&arch=noarch"
        );
    }

    #[test]
    fn remi_sync_row_records_requested_distro_and_version_scheme() {
        let row = remi_sync_row(
            7,
            "http://remi.test".to_string(),
            "ubuntu".to_string(),
            RemiPackageEntry {
                name: "nano".to_string(),
                version: "8.7.1-1".to_string(),
                release: None,
                converted: false,
                architecture: Some("amd64".to_string()),
                dependencies: None,
                metadata: None,
            },
        )
        .unwrap();

        assert_eq!(row.package.distro.as_deref(), Some("ubuntu"));
        assert_eq!(row.package.version_scheme.as_deref(), Some("debian"));
    }

    #[test]
    fn remi_sync_row_rejects_public_profile_id_as_route_slug() {
        for public_id in ["fedora-44", "ubuntu-26.04"] {
            let err = remi_sync_row(
                1,
                "https://remi.example.test".to_string(),
                public_id.to_string(),
                remi_entry_for_tests("bash", "5.2.0"),
            )
            .unwrap_err();

            assert!(err.to_string().contains("unsupported Remi distro route"));
        }
    }

    #[test]
    fn remi_sync_row_accepts_route_slug_and_uses_profile_scheme() {
        let row = remi_sync_row(
            1,
            "https://remi.example.test".to_string(),
            "ubuntu".to_string(),
            remi_entry_for_tests("bash", "5.2.0"),
        )
        .unwrap();

        assert_eq!(row.package.distro.as_deref(), Some("ubuntu"));
        assert_eq!(row.package.version_scheme.as_deref(), Some("debian"));
    }

    #[tokio::test]
    async fn remi_metadata_fetch_retries_truncated_json() {
        use crate::repository::retry::RetryConfig;
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use std::time::Duration;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let attempts = Arc::new(AtomicUsize::new(0));
        let server_attempts = attempts.clone();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                loop {
                    let read = stream.read(&mut buf).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }

                let attempt = server_attempts.fetch_add(1, Ordering::SeqCst);
                let body = if attempt == 0 {
                    r#"{"packages":[{"name":"qemu-img""#
                } else {
                    r#"{"packages":[{"name":"qemu-img","version":"2:10.1.0-7.fc44","converted":false,"architecture":"x86_64"}]}"#
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let retry = RetryConfig {
            max_attempts: 2,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            jitter_factor: 0.0,
        };
        let client = RepositoryClient::new().unwrap();
        let metadata = fetch_remi_metadata_with_retry(
            &client,
            &format!("http://{addr}/v1/fedora/metadata"),
            &retry,
        )
        .await
        .unwrap();

        assert_eq!(metadata.packages.len(), 1);
        assert_eq!(metadata.packages[0].name, "qemu-img");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        server.await.unwrap();
    }
}
