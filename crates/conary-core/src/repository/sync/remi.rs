// conary-core/src/repository/sync/remi.rs

use crate::db::models::{
    CanonicalPackage, PackageImplementation, Repository, RepositoryPackage, RepositoryProvide,
    RepositoryRequirement,
};
use crate::error::{Error, Result};
use crate::repository::client::RepositoryClient;
use crate::repository::registry;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use tracing::{debug, info};

use super::native::{
    SyncedPackageRow, extract_extra_metadata_provides, persist_native_sync_rows,
    split_on_version_op,
};
/// Response from Remi metadata API (`GET /v1/{distro}/metadata`)
#[derive(Debug, serde::Deserialize)]
pub(super) struct RemiMetadataResponse {
    packages: Vec<RemiPackageEntry>,
}

/// Individual package entry from Remi metadata
#[derive(Debug, serde::Deserialize)]
pub(super) struct RemiPackageEntry {
    pub(super) name: String,
    pub(super) version: String,
    #[allow(dead_code)] // Present in wire format; not used by sync logic
    pub(super) converted: bool,
    pub(super) dependencies: Option<Vec<String>>,
    pub(super) metadata: Option<serde_json::Value>,
}

/// Response from Remi canonical map API (`GET /v1/canonical/map`)
#[derive(Debug, serde::Deserialize)]
pub(super) struct CanonicalMapResponse {
    #[allow(dead_code)] // Wire format field; only entries is consumed
    pub(super) version: u32,
    #[allow(dead_code)] // Wire format field; only entries is consumed
    pub(super) generated_at: String,
    pub(super) entries: Vec<CanonicalMapEntry>,
}

/// A single entry in the canonical map response
#[derive(Debug, serde::Deserialize)]
pub(super) struct CanonicalMapEntry {
    pub(super) canonical: String,
    pub(super) implementations: HashMap<String, String>,
}

pub(super) fn remi_sync_row(
    repo_id: i64,
    endpoint: String,
    distro: String,
    entry: RemiPackageEntry,
    system_arch: &str,
) -> SyncedPackageRow {
    let download_url = format!("{endpoint}/v1/{distro}/packages/{}/download", entry.name);

    let mut package = RepositoryPackage::new(
        repo_id,
        entry.name.clone(),
        entry.version.clone(),
        "remi:server-verified".to_string(),
        0,
        download_url,
    );
    package.architecture = Some(system_arch.to_string());
    package.dependencies = entry
        .dependencies
        .as_ref()
        .map(|deps| serde_json::to_string(deps).unwrap_or_default());
    package.metadata = entry.metadata.as_ref().map(|value| value.to_string());

    let metadata = entry.metadata.unwrap_or(serde_json::Value::Null);

    let scheme_str = match distro.as_str() {
        distro if distro.starts_with("ubuntu") || distro.starts_with("debian") => {
            Some("debian".to_string())
        }
        distro if distro.starts_with("arch") => Some("arch".to_string()),
        _ => Some("rpm".to_string()),
    };

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

    SyncedPackageRow {
        package,
        provides,
        requirements,
        requirement_groups: Vec::new(),
        requirement_group_clauses: Vec::new(),
    }
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
pub(super) async fn sync_repository_remi(
    conn: &Connection,
    repo: &mut Repository,
) -> Result<usize> {
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
    let bytes = client.download_to_bytes(&metadata_url).await?;
    let response: RemiMetadataResponse = serde_json::from_slice(&bytes).map_err(|error| {
        Error::ParseError(format!(
            "Failed to parse Remi metadata from {}: {}",
            metadata_url, error
        ))
    })?;

    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;

    let system_arch = registry::detect_system_arch();
    let mut seen = HashSet::new();
    let synced_packages: Vec<SyncedPackageRow> = response
        .packages
        .into_iter()
        .filter_map(|entry| {
            let key = (
                entry.name.clone(),
                entry.version.clone(),
                system_arch.clone(),
            );
            if !seen.insert(key) {
                return None;
            }
            Some(remi_sync_row(
                repo_id,
                endpoint.to_string(),
                distro.to_string(),
                entry,
                &system_arch,
            ))
        })
        .collect();

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

/// Fetch the canonical package map from a Remi endpoint and persist it locally.
///
/// Downloads the full canonical map from `{endpoint}/v1/canonical/map` and upserts
/// each entry into `canonical_packages` and `package_implementations`. This is
/// non-fatal: callers should log failures at debug level and continue.
pub(super) async fn fetch_and_persist_canonical_map(
    conn: &Connection,
    endpoint: &str,
) -> Result<u64> {
    let url = format!("{}/v1/canonical/map", endpoint.trim_end_matches('/'));
    debug!("Fetching canonical map from {}", url);

    let client = RepositoryClient::new()?;
    let bytes = client.download_to_bytes(&url).await?;

    let map: CanonicalMapResponse = serde_json::from_slice(&bytes).map_err(|error| {
        Error::ParseError(format!("Failed to parse canonical map from {url}: {error}"))
    })?;

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
