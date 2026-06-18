// conary-core/src/repository/sync.rs

//! Repository synchronization
//!
//! Functions for synchronizing repository metadata from remote sources,
//! including native format support for Arch, Debian, and Fedora repositories.

use crate::db::models::{
    PackageDelta, Repository, RepositoryPackage, RepositoryPackageKey, RepositoryPackageKeyStatus,
};
use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

use super::client::RepositoryClient;
use super::gpg::{GpgVerifier, MetadataSignatureVerifier};
use super::metadata::{
    PackageSecurityAdvisoryMetadata, RepositoryMetadata, SecurityAdvisorySourceMetadata,
};
use super::registry::{self, RepositoryFormat};
use super::static_repo::sync::fetch_static_sync_snapshot;
use native::{
    convert_requirement_groups, distro_flavor_to_db, normalized_repository_capabilities,
    persist_native_sync_rows, persist_synced_package_rows, version_scheme_to_db,
};
#[cfg(test)]
use remi::remi_sync_row;
use remi::{
    fetch_and_persist_canonical_map, fetch_canonical_map_snapshot, fetch_remi_sync_rows,
    persist_canonical_map, sync_repository_remi,
};
#[cfg(test)]
use types::{CanonicalMapResponse, RemiPackageEntry};
use types::{
    JsonPackageDelta, JsonRepositorySyncSnapshot, RepositorySyncSnapshot, SyncedPackageRow,
};

mod native;
mod remi;
pub(in crate::repository) mod types;

/// Get current timestamp as ISO 8601 string
pub fn current_timestamp() -> String {
    use chrono::Utc;
    Utc::now().to_rfc3339()
}

/// Parse ISO 8601 timestamp to Unix seconds
pub fn parse_timestamp(timestamp: &str) -> Result<u64> {
    use chrono::DateTime;

    let dt = DateTime::parse_from_rfc3339(timestamp)
        .map_err(|e| Error::ParseError(format!("Invalid timestamp: {e}")))?;

    u64::try_from(dt.timestamp()).map_err(|_| {
        Error::ParseError(format!(
            "Timestamp is before Unix epoch (negative): {timestamp}"
        ))
    })
}

async fn run_blocking_sync<F, T>(f: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|error| Error::InternalError(format!("blocking sync task failed: {error}")))?
}

/// Rebase a download URL from metadata source to content source
///
/// If the repository has a content_url configured, this function rebases
/// the download URL from the metadata URL to the content URL.
///
/// For example:
/// - metadata_url: "https://your-server.com/fedora/39/metadata"
/// - content_url: "https://mirror.local/fedora"
/// - download_url: "https://your-server.com/fedora/39/metadata/Packages/foo.rpm"
/// - Result: "https://mirror.local/fedora/Packages/foo.rpm"
///
/// Security note: The checksum from trusted metadata is verified after download,
/// so even if content_url points to an untrusted source, the downloaded content
/// is validated against the hash from the signed metadata.
fn rebase_download_url(
    download_url: &str,
    metadata_url: &str,
    content_url: Option<&str>,
) -> String {
    match content_url {
        Some(content_base) => {
            // Normalize URLs by removing trailing slashes for consistent matching
            let metadata_base = metadata_url.trim_end_matches('/');
            let content_base = content_base.trim_end_matches('/');

            if let Some(relative) = download_url.strip_prefix(metadata_base) {
                // Ensure proper path joining - relative should start with /
                // This handles cases like:
                //   metadata_base: "http://foo.com/repo"
                //   relative: "/Packages/foo.rpm" or "Packages/foo.rpm"
                let relative = relative.trim_start_matches('/');
                format!("{}/{}", content_base, relative)
            } else {
                // URL doesn't match metadata base - return as-is
                // This handles absolute URLs in metadata that point elsewhere
                download_url.to_string()
            }
        }
        None => download_url.to_string(),
    }
}

async fn fetch_repository_native_snapshot(
    repo: &Repository,
    format: RepositoryFormat,
    keyring_dir: &Path,
) -> Result<RepositorySyncSnapshot> {
    info!(
        "Syncing repository {} using native {:?} format",
        repo.name, format
    );

    // Create and use parser from registry
    let metadata_signature_verifier = if repo.gpg_check {
        Some(MetadataSignatureVerifier::new(
            keyring_dir.to_path_buf(),
            repo.name.clone(),
            true,
        ))
    } else {
        None
    };
    let parser =
        registry::create_parser(format, &repo.name, &repo.url, metadata_signature_verifier)?;
    let packages = parser.sync_metadata(&repo.url).await?;

    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;

    if let Some(ref content_url) = repo.content_url {
        info!(
            "Repository {} uses reference mirror - rebasing download URLs to {}",
            repo.name, content_url
        );
    }

    // Convert package metadata to repository rows plus normalized capability rows.
    let synced_packages: Vec<SyncedPackageRow> = packages
        .into_iter()
        .map(|pkg_meta| {
            let (provides, requirements) = normalized_repository_capabilities(&pkg_meta);
            // Convert parsers::Dependency to Vec<String>
            let deps_json = if !pkg_meta.dependencies.is_empty() {
                let dep_strings: Vec<String> = pkg_meta
                    .dependencies
                    .iter()
                    .map(|dep| {
                        if let Some(constraint) = &dep.constraint {
                            format!("{} {constraint}", dep.name)
                        } else {
                            dep.name.clone()
                        }
                    })
                    .collect();
                Some(serde_json::to_string(&dep_strings).unwrap_or_default())
            } else {
                None
            };

            // Rebase download URL if content_url is configured (reference mirror)
            let download_url = rebase_download_url(
                &pkg_meta.download_url,
                &repo.url,
                repo.content_url.as_deref(),
            );

            let mut repo_pkg = RepositoryPackage::new(
                repo_id,
                pkg_meta.name,
                pkg_meta.version,
                pkg_meta.checksum,
                pkg_meta.size as i64,
                download_url,
            );

            repo_pkg.architecture = pkg_meta.architecture;
            repo_pkg.description = pkg_meta.description;
            repo_pkg.dependencies = deps_json;
            repo_pkg.metadata = match &pkg_meta.extra_metadata {
                serde_json::Value::Null => None,
                value => Some(value.to_string()),
            };

            // Persist package origin metadata from parser
            repo_pkg.distro = pkg_meta.source_distro.map(distro_flavor_to_db);
            repo_pkg.version_scheme = pkg_meta.version_scheme.map(version_scheme_to_db);

            // Convert parser-level requirement groups to DB models
            let (req_groups, req_group_clauses) =
                convert_requirement_groups(0, &pkg_meta.requirements);

            SyncedPackageRow {
                package: repo_pkg,
                provides,
                requirements,
                requirement_groups: req_groups,
                requirement_group_clauses: req_group_clauses,
            }
        })
        .collect();
    Ok(RepositorySyncSnapshot::NativeRows(synced_packages))
}

/// Synchronize repository using native metadata format parsers
async fn sync_repository_native(
    conn: &Connection,
    repo: &mut Repository,
    format: RepositoryFormat,
) -> Result<usize> {
    let keyring_dir = keyring_dir_for_connection(conn)?;
    let snapshot = fetch_repository_native_snapshot(repo, format, &keyring_dir).await?;
    let count = persist_repository_sync_snapshot(conn, repo, snapshot)?;

    info!(
        "Synchronized {} packages from repository {}",
        count, repo.name
    );
    Ok(count)
}

fn keyring_dir_for_connection(conn: &Connection) -> Result<PathBuf> {
    let mut stmt = conn.prepare("PRAGMA database_list")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
    })?;

    for row in rows {
        let (name, file) = row?;
        if name == "main" && !file.is_empty() {
            return Ok(crate::db::paths::keyring_dir(&file));
        }
    }

    Ok(crate::db::paths::keyring_dir("/var/lib/conary/conary.db"))
}

async fn fetch_repository_sync_snapshot(
    repo: &Repository,
    keyring_dir: &Path,
) -> Result<RepositorySyncSnapshot> {
    if repo.default_strategy.as_deref() == Some("remi") {
        return fetch_remi_sync_rows(repo)
            .await
            .map(RepositorySyncSnapshot::NativeRows);
    }

    let format = registry::detect_repository_format(&repo.name, &repo.url);

    if format != RepositoryFormat::Json {
        match fetch_repository_native_snapshot(repo, format, keyring_dir).await {
            Ok(snapshot) => return Ok(snapshot),
            Err(e) => {
                warn!("Native format sync failed: {}, falling back to JSON", e);
            }
        }
    }

    fetch_repository_json_snapshot(repo).await
}

/// Synchronize repository metadata by opening short-lived database connections
/// around blocking persistence phases.
pub async fn sync_repository_from_db_path(db_path: PathBuf, repo: Repository) -> Result<usize> {
    info!("Synchronizing repository: {}", repo.name);

    if is_static_repository(&repo) {
        return sync_static_repository_from_db_path(db_path, repo).await;
    }

    if repo.tuf_enabled {
        let repo_id = repo
            .id
            .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;
        let tuf_client =
            crate::trust::client::TufClient::new(repo_id, &repo.url, repo.tuf_root_url.as_deref())
                .map_err(|e| Error::TrustError(e.to_string()))?;

        let state_db_path = db_path.clone();
        let update_state = run_blocking_sync(move || {
            let conn = crate::db::open_fast(&state_db_path)?;
            tuf_client
                .load_update_state(&conn)
                .map_err(|e| Error::TrustError(e.to_string()))
        })
        .await?;

        let tuf_client =
            crate::trust::client::TufClient::new(repo_id, &repo.url, repo.tuf_root_url.as_deref())
                .map_err(|e| Error::TrustError(e.to_string()))?;
        let update_snapshot = tuf_client
            .fetch_update_snapshot(update_state)
            .await
            .map_err(|e| Error::TrustError(e.to_string()))?;

        let persist_db_path = db_path.clone();
        let verified = run_blocking_sync(move || {
            let conn = crate::db::open_fast(&persist_db_path)?;
            tuf_client
                .persist_update_snapshot(&conn, update_snapshot)
                .map_err(|e| Error::TrustError(e.to_string()))
        })
        .await?;

        info!(
            "TUF verified: root v{}, targets v{}, {} targets",
            verified.root_version,
            verified.targets_version,
            verified.targets.len()
        );
    }

    let keyring_dir = crate::db::paths::keyring_dir(&db_path.display().to_string());
    let snapshot = fetch_repository_sync_snapshot(&repo, &keyring_dir).await?;

    let persist_repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;
    let persist_db_path = db_path.clone();
    let count = run_blocking_sync(move || {
        let conn = crate::db::open_fast(&persist_db_path)?;
        let mut repo = Repository::find_by_id(&conn, persist_repo_id)?.ok_or_else(|| {
            Error::NotFound(format!(
                "Repository {persist_repo_id} not found during sync"
            ))
        })?;
        persist_repository_sync_snapshot(&conn, &mut repo, snapshot)
    })
    .await?;

    if let Some(ref remi_endpoint) = repo.default_strategy_endpoint {
        match fetch_canonical_map_snapshot(remi_endpoint).await {
            Ok(map) => {
                let canonical_db_path = db_path.clone();
                match run_blocking_sync(move || {
                    let conn = crate::db::open_fast(&canonical_db_path)?;
                    persist_canonical_map(&conn, &map)
                })
                .await
                {
                    Ok(mapping_count) => {
                        info!("Synced {} canonical mappings from Remi", mapping_count);
                    }
                    Err(e) => {
                        debug!("Failed to persist canonical map: {}", e);
                    }
                }
            }
            Err(e) => {
                debug!("Failed to fetch canonical map: {}", e);
            }
        }
    }

    Ok(count)
}

/// Synchronize repository metadata with the database
pub async fn sync_repository(conn: &Connection, repo: &mut Repository) -> Result<usize> {
    info!("Synchronizing repository: {}", repo.name);

    if is_static_repository(repo) {
        return sync_repository_static(conn, repo).await;
    }

    // TUF verification phase (before any metadata processing)
    if repo.tuf_enabled {
        let repo_id = repo
            .id
            .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;

        let tuf_client =
            crate::trust::client::TufClient::new(repo_id, &repo.url, repo.tuf_root_url.as_deref())
                .map_err(|e| Error::TrustError(e.to_string()))?;

        let verified = tuf_client
            .update(conn)
            .await
            .map_err(|e| Error::TrustError(e.to_string()))?;

        info!(
            "TUF verified: root v{}, targets v{}, {} targets",
            verified.root_version,
            verified.targets_version,
            verified.targets.len()
        );
    }

    // Route to Remi-native sync if strategy is "remi"
    let count = if repo.default_strategy.as_deref() == Some("remi") {
        sync_repository_remi(conn, repo).await?
    } else {
        // Detect repository format using registry
        let format = registry::detect_repository_format(&repo.name, &repo.url);

        // Try native format first if detected
        let native_result = if format != RepositoryFormat::Json {
            match sync_repository_native(conn, repo, format).await {
                Ok(count) => Some(count),
                Err(e) => {
                    warn!("Native format sync failed: {}, falling back to JSON", e);
                    None
                }
            }
        } else {
            None
        };

        if let Some(count) = native_result {
            count
        } else {
            // Fall back to JSON metadata format
            sync_repository_json_fallback(conn, repo).await?
        }
    };

    // After package sync completes, fetch the canonical map from Remi (non-fatal)
    if let Some(ref remi_endpoint) = repo.default_strategy_endpoint {
        match fetch_and_persist_canonical_map(conn, remi_endpoint).await {
            Ok(mapping_count) => {
                info!("Synced {} canonical mappings from Remi", mapping_count);
            }
            Err(e) => {
                debug!("Failed to fetch canonical map: {}", e);
            }
        }
    }

    Ok(count)
}

fn is_static_repository(repo: &Repository) -> bool {
    repo.default_strategy.as_deref() == Some("static")
}

fn static_repin_command(repo: &Repository) -> String {
    format!(
        "conary repo add {} {} --fingerprint <root-key-id> --replace",
        repo.name, repo.url
    )
}

fn static_trust_not_established_error(repo: &Repository) -> Error {
    Error::TrustError(format!(
        "Static repository trust is not established; run {}",
        static_repin_command(repo)
    ))
}

fn map_static_trust_error(repo: &Repository, error: impl std::fmt::Display) -> Error {
    let message = error.to_string();
    if message.contains("No trusted root") {
        static_trust_not_established_error(repo)
    } else {
        Error::TrustError(message)
    }
}

async fn sync_static_repository_from_db_path(db_path: PathBuf, repo: Repository) -> Result<usize> {
    if !repo.tuf_enabled {
        return Err(static_trust_not_established_error(&repo));
    }

    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;
    let tuf_client = crate::trust::client::TufClient::new_static(
        repo_id,
        &repo.url,
        repo.tuf_root_url.as_deref(),
    )
    .map_err(|error| Error::TrustError(error.to_string()))?;

    let state_db_path = db_path.clone();
    let update_state = match run_blocking_sync(move || {
        let conn = crate::db::open_fast(&state_db_path)?;
        tuf_client
            .load_update_state(&conn)
            .map_err(|error| Error::TrustError(error.to_string()))
    })
    .await
    {
        Ok(state) => state,
        Err(error) => return Err(map_static_trust_error(&repo, error)),
    };

    let tuf_client = crate::trust::client::TufClient::new_static(
        repo_id,
        &repo.url,
        repo.tuf_root_url.as_deref(),
    )
    .map_err(|error| Error::TrustError(error.to_string()))?;
    let update_snapshot = tuf_client
        .fetch_update_snapshot(update_state)
        .await
        .map_err(|error| Error::TrustError(error.to_string()))?;

    let persist_db_path = db_path.clone();
    let verified = run_blocking_sync(move || {
        let conn = crate::db::open_fast(&persist_db_path)?;
        tuf_client
            .persist_update_snapshot(&conn, update_snapshot)
            .map_err(|error| Error::TrustError(error.to_string()))
    })
    .await?;

    info!(
        "TUF verified: root v{}, targets v{}, {} targets",
        verified.root_version,
        verified.targets_version,
        verified.targets.len()
    );

    let snapshot = fetch_static_sync_snapshot(&repo, &verified).await?;
    let persist_db_path = db_path.clone();
    run_blocking_sync(move || {
        let conn = crate::db::open_fast(&persist_db_path)?;
        let mut repo = Repository::find_by_id(&conn, repo_id)?.ok_or_else(|| {
            Error::NotFound(format!("Repository {repo_id} not found during static sync"))
        })?;
        persist_repository_sync_snapshot(&conn, &mut repo, snapshot)
    })
    .await
}

async fn sync_repository_static(conn: &Connection, repo: &mut Repository) -> Result<usize> {
    if !repo.tuf_enabled {
        return Err(static_trust_not_established_error(repo));
    }

    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;

    let tuf_client = crate::trust::client::TufClient::new_static(
        repo_id,
        &repo.url,
        repo.tuf_root_url.as_deref(),
    )
    .map_err(|error| Error::TrustError(error.to_string()))?;

    let verified = match tuf_client.update(conn).await {
        Ok(verified) => verified,
        Err(error) => return Err(map_static_trust_error(repo, error)),
    };

    info!(
        "TUF verified: root v{}, targets v{}, {} targets",
        verified.root_version,
        verified.targets_version,
        verified.targets.len()
    );

    let snapshot = fetch_static_sync_snapshot(repo, &verified).await?;
    let count = persist_repository_sync_snapshot(conn, repo, snapshot)?;

    info!(
        "Synchronized {} packages from static repository {}",
        count, repo.name
    );
    Ok(count)
}

fn json_repository_sync_snapshot(
    repo: &Repository,
    metadata: RepositoryMetadata,
) -> Result<RepositorySyncSnapshot> {
    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;
    let trusted_advisory_source =
        trusted_json_advisory_source(repo, metadata.security_advisory_source.as_ref())?;

    let mut packages = Vec::new();
    let mut delta_rows = Vec::new();

    for pkg_meta in metadata.packages {
        let deps_json = pkg_meta
            .dependencies
            .as_ref()
            .map(|deps| serde_json::to_string(deps).unwrap_or_default());

        // Rebase download URL if content_url is configured (reference mirror)
        let download_url = rebase_download_url(
            &pkg_meta.download_url,
            &repo.url,
            repo.content_url.as_deref(),
        );

        let mut repo_pkg = RepositoryPackage::new(
            repo_id,
            pkg_meta.name.clone(),
            pkg_meta.version.clone(),
            pkg_meta.checksum.clone(),
            pkg_meta.size,
            download_url,
        );

        repo_pkg.architecture = pkg_meta.architecture;
        repo_pkg.description = pkg_meta.description;
        repo_pkg.dependencies = deps_json;
        if let (Some(source), Some(advisory)) =
            (trusted_advisory_source, pkg_meta.security_advisory.as_ref())
        {
            let normalized = apply_trusted_package_security_advisory(
                &mut repo_pkg,
                advisory,
                &source.name,
                &source.trust,
            )?;
            repo_pkg.metadata = Some(
                serde_json::json!({
                    "security_advisory": normalized,
                })
                .to_string(),
            );
        }

        packages.push(repo_pkg);

        // Store delta metadata if available
        if let Some(delta_infos) = pkg_meta.delta_from {
            for delta_info in delta_infos {
                delta_rows.push(JsonPackageDelta {
                    package_name: pkg_meta.name.clone(),
                    from_version: delta_info.from_version,
                    to_version: pkg_meta.version.clone(),
                    from_hash: delta_info.from_hash,
                    to_hash: pkg_meta.checksum.clone(),
                    delta_url: delta_info.delta_url,
                    delta_size: delta_info.delta_size,
                    delta_checksum: delta_info.delta_checksum,
                    target_size: pkg_meta.size,
                });
            }
        }
    }

    Ok(RepositorySyncSnapshot::JsonFallback(
        JsonRepositorySyncSnapshot {
            packages,
            deltas: delta_rows,
        },
    ))
}

async fn fetch_repository_json_snapshot(repo: &Repository) -> Result<RepositorySyncSnapshot> {
    let client = RepositoryClient::new()?;
    let metadata = client.fetch_metadata(&repo.url).await?;
    json_repository_sync_snapshot(repo, metadata)
}

fn trusted_json_advisory_source<'a>(
    repo: &Repository,
    source: Option<&'a SecurityAdvisorySourceMetadata>,
) -> Result<Option<&'a SecurityAdvisorySourceMetadata>> {
    if !repo.security_advisory_support.is_supported() {
        return Ok(None);
    }

    let source = source.ok_or_else(|| {
        Error::ConfigError(format!(
            "Repository '{}' is marked as supported for security advisories but did not publish a trusted security advisory source",
            repo.name
        ))
    })?;

    if source.name.trim().is_empty() {
        return Err(Error::ConfigError(format!(
            "Repository '{}' published an empty security advisory source name",
            repo.name
        )));
    }

    if !source.trust.eq_ignore_ascii_case("trusted") {
        return Err(Error::ConfigError(format!(
            "Repository '{}' published security advisory source '{}' with unsupported trust '{}'",
            repo.name, source.name, source.trust
        )));
    }

    Ok(Some(source))
}

fn apply_trusted_package_security_advisory(
    package: &mut RepositoryPackage,
    advisory: &PackageSecurityAdvisoryMetadata,
    default_source: &str,
    default_source_trust: &str,
) -> Result<serde_json::Value> {
    let source = advisory.source.as_deref().unwrap_or(default_source).trim();
    if source.is_empty() {
        return Err(Error::ConfigError(format!(
            "Security advisory '{}' for package '{}' has an empty source",
            advisory.id, package.name
        )));
    }

    let source_trust = advisory
        .source_trust
        .as_deref()
        .unwrap_or(default_source_trust)
        .trim()
        .to_ascii_lowercase();
    if source_trust != "trusted" {
        return Err(Error::ConfigError(format!(
            "Security advisory '{}' for package '{}' is not from a trusted source",
            advisory.id, package.name
        )));
    }

    if advisory.id.trim().is_empty() {
        return Err(Error::ConfigError(format!(
            "Security advisory for package '{}' is missing an advisory id",
            package.name
        )));
    }

    let fixed_version = advisory
        .fixed_version
        .as_deref()
        .unwrap_or(&package.version);
    if fixed_version != package.version {
        return Err(Error::ConfigError(format!(
            "Security advisory '{}' for package '{}' fixed_version '{}' does not match package version '{}'",
            advisory.id, package.name, fixed_version, package.version
        )));
    }

    let severity = advisory.severity.as_deref().and_then(normalize_severity);
    let cves: Vec<String> = advisory
        .cves
        .iter()
        .map(|cve| cve.trim())
        .filter(|cve| !cve.is_empty())
        .map(ToOwned::to_owned)
        .collect();

    package.is_security_update = true;
    package.severity = severity.clone();
    package.cve_ids = if cves.is_empty() {
        None
    } else {
        Some(cves.join(","))
    };
    package.advisory_id = Some(advisory.id.trim().to_string());
    package.advisory_url = advisory.url.clone();

    Ok(serde_json::json!({
        "id": advisory.id.trim(),
        "source": source,
        "source_trust": source_trust,
        "severity": severity,
        "cves": cves,
        "fixed_version": fixed_version,
        "url": advisory.url,
    }))
}

fn normalize_severity(severity: &str) -> Option<String> {
    let severity = severity.trim().to_ascii_lowercase();
    match severity.as_str() {
        "" => None,
        "high" => Some("important".to_string()),
        "medium" => Some("moderate".to_string()),
        _ => Some(severity),
    }
}

fn persist_repository_sync_snapshot(
    conn: &Connection,
    repo: &mut Repository,
    snapshot: RepositorySyncSnapshot,
) -> Result<usize> {
    match snapshot {
        RepositorySyncSnapshot::NativeRows(synced_packages) => {
            let mut repo_packages: Vec<RepositoryPackage> = synced_packages
                .iter()
                .map(|row| row.package.clone())
                .collect();
            persist_native_sync_rows(conn, repo, &mut repo_packages, synced_packages)
        }
        RepositorySyncSnapshot::StaticRows {
            packages,
            package_keys,
        } => persist_static_sync_rows(conn, repo, packages, package_keys),
        RepositorySyncSnapshot::JsonFallback(snapshot) => {
            let repo_id = repo
                .id
                .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;
            let count = snapshot.packages.len();

            let tx = conn.unchecked_transaction()?;

            RepositoryPackage::delete_by_repository(&tx, repo_id)?;

            for mut repo_pkg in snapshot.packages {
                repo_pkg.insert(&tx)?;
            }

            let mut delta_count = 0;
            for delta in snapshot.deltas {
                let mut db_delta = PackageDelta::new(
                    delta.package_name,
                    delta.from_version,
                    delta.to_version,
                    delta.from_hash,
                    delta.to_hash,
                    delta.delta_url,
                    delta.delta_size,
                    delta.delta_checksum,
                    delta.target_size,
                );
                db_delta.insert(&tx)?;
                delta_count += 1;
            }

            link_canonical_ids(&tx, repo_id)?;

            repo.last_sync = Some(current_timestamp());
            repo.update(&tx)?;

            tx.commit()?;

            info!(
                "Synchronized {} packages and {} deltas from repository {}",
                count, delta_count, repo.name
            );
            Ok(count)
        }
    }
}

fn persist_static_sync_rows(
    conn: &Connection,
    repo: &mut Repository,
    synced_packages: Vec<SyncedPackageRow>,
    package_keys: Vec<RepositoryPackageKey>,
) -> Result<usize> {
    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;
    let count = synced_packages.len();
    let mut repo_packages: Vec<RepositoryPackage> = synced_packages
        .iter()
        .map(|row| row.package.clone())
        .collect();

    let tx = conn.unchecked_transaction()?;

    persist_synced_package_rows(&tx, repo_id, &mut repo_packages, synced_packages)?;
    replace_package_keys_for_repository(&tx, repo_id, &package_keys)?;
    link_canonical_ids(&tx, repo_id)?;

    repo.last_sync = Some(current_timestamp());
    repo.update(&tx)?;

    tx.commit()?;

    Ok(count)
}

fn replace_package_keys_for_repository(
    conn: &Connection,
    repository_id: i64,
    keys: &[RepositoryPackageKey],
) -> Result<()> {
    for key in keys {
        if key.repository_id != repository_id {
            return Err(Error::InternalError(format!(
                "repository_id mismatch for repository package key: expected {repository_id}, got {}",
                key.repository_id
            )));
        }
    }

    conn.execute(
        "DELETE FROM repository_package_keys WHERE repository_id = ?1",
        [repository_id],
    )?;

    let mut insert_with_default_synced_at = conn.prepare(
        "INSERT INTO repository_package_keys (repository_id, public_key, key_id, status)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    let mut insert_with_synced_at = conn.prepare(
        "INSERT INTO repository_package_keys
            (repository_id, public_key, key_id, status, synced_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;

    for key in keys {
        let status = match key.status {
            RepositoryPackageKeyStatus::Active => "active",
            RepositoryPackageKeyStatus::Retired => "retired",
        };

        if let Some(synced_at) = &key.synced_at {
            insert_with_synced_at.execute(params![
                key.repository_id,
                &key.public_key,
                &key.key_id,
                status,
                synced_at,
            ])?;
        } else {
            insert_with_default_synced_at.execute(params![
                key.repository_id,
                &key.public_key,
                &key.key_id,
                status,
            ])?;
        }
    }

    Ok(())
}

/// JSON metadata fallback sync path (used when native format sync is unavailable)
async fn sync_repository_json_fallback(conn: &Connection, repo: &mut Repository) -> Result<usize> {
    let snapshot = fetch_repository_json_snapshot(repo).await?;
    persist_repository_sync_snapshot(conn, repo, snapshot)
}

/// Check if repository metadata needs refresh
pub fn needs_sync(repo: &Repository) -> bool {
    let Some(last_sync) = &repo.last_sync else {
        return true;
    };

    let Ok(last_sync_time) = parse_timestamp(last_sync) else {
        return true;
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    now.saturating_sub(last_sync_time) > repo.metadata_expire as u64
}

/// Attempt to fetch and import GPG key if configured for the repository
///
/// This function should be called before sync when gpg_check is enabled.
/// It will:
/// 1. Check if gpg_key_url is configured
/// 2. Check if key already exists (skip if so)
/// 3. Download and import the key
///
/// # Arguments
/// * `repo` - Repository with gpg_key_url to fetch from
/// * `keyring_dir` - Directory to store GPG keys
///
/// # Returns
/// * `Ok(Some(fingerprint))` - Key was fetched and imported
/// * `Ok(None)` - No key URL configured, or key already exists
/// * `Err(_)` - Failed to fetch or import key
pub async fn maybe_fetch_gpg_key(repo: &Repository, keyring_dir: &Path) -> Result<Option<String>> {
    let Some(key_url) = &repo.gpg_key_url else {
        debug!("No gpg_key_url configured for repository '{}'", repo.name);
        return Ok(None);
    };

    // Create verifier
    let verifier = GpgVerifier::new(keyring_dir.to_path_buf())?;

    // Skip if key already exists
    if verifier.has_key(&repo.name) {
        debug!(
            "GPG key already exists for repository '{}', skipping fetch",
            repo.name
        );
        return Ok(None);
    }

    // Reject insecure HTTP URLs for GPG key fetching
    if key_url.starts_with("http://") {
        return Err(Error::ConfigError(format!(
            "GPG key URL for repository '{}' uses insecure http:// scheme. \
             Use https:// instead: {}",
            repo.name, key_url
        )));
    }

    info!(
        "Fetching GPG key for repository '{}' from {}",
        repo.name, key_url
    );

    // Download the key
    let client = RepositoryClient::new()?;
    let key_data = client.download_to_bytes(key_url).await.map_err(|e| {
        Error::DownloadError(format!(
            "Failed to fetch GPG key for '{}': {}",
            repo.name, e
        ))
    })?;

    // Import the key
    let fingerprint = verifier.import_key(&key_data, &repo.name)?;

    info!(
        "Imported GPG key for repository '{}' (fingerprint: {})",
        repo.name, fingerprint
    );

    Ok(Some(fingerprint))
}

/// Link repository_packages to their canonical identity.
///
/// For each package in the given repo, looks up a matching entry in
/// package_implementations by (distro_name, distro) and sets canonical_id.
/// Called after batch_insert during sync, and by `conary canonical rebuild`.
pub fn link_canonical_ids(conn: &Connection, repo_id: i64) -> Result<usize> {
    let repo_distro: Option<String> = conn
        .query_row(
            "SELECT COALESCE(default_strategy_distro, name) FROM repositories WHERE id = ?1",
            [repo_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();

    let Some(distro) = repo_distro else {
        return Ok(0);
    };

    let updated = conn.execute(
        "UPDATE repository_packages SET canonical_id = (
            SELECT pi.canonical_id FROM package_implementations pi
            WHERE pi.distro_name = repository_packages.name
              AND pi.distro = ?1
            LIMIT 1
        ) WHERE repository_id = ?2 AND canonical_id IS NULL",
        params![distro, repo_id],
    )?;

    if updated > 0 {
        info!("Linked {updated} packages to canonical identity for repo {repo_id}");
    }

    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::signing::SigningKeyPair;
    use crate::db::models::{
        RepositoryPackage, RepositoryPackageKey, RepositoryProvide, RepositoryRequirement,
        RepositoryRequirementGroup as DbRequirementGroup, SecurityAdvisorySupport,
    };
    use crate::db::schema::migrate;
    use crate::hash::sha256;
    use crate::repository::dependency_model::{
        self as dep_model, ConditionalRequirementBehavior, RepositoryDependencyFlavor,
        RepositoryRequirementKind,
    };
    use crate::repository::metadata::RepositoryMetadata as JsonRepositoryMetadata;
    use crate::repository::parsers::{Dependency, PackageMetadata};
    use crate::repository::versioning::VersionScheme;
    use crate::trust::metadata::{TargetDescription, VerifiedTufState};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::path::Path;

    const STATIC_PACKAGE_PATH: &str = "packages/acme-widget/acme-widget-1.4.2-1-x86_64.ccs";

    struct StaticSyncFixture {
        _tempdir: tempfile::TempDir,
        repo: Repository,
        package_bytes: Vec<u8>,
        active_key: String,
        retired_key: String,
    }

    impl StaticSyncFixture {
        fn new() -> Self {
            let tempdir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(tempdir.path().join("packages/acme-widget")).unwrap();
            std::fs::create_dir_all(tempdir.path().join("keys")).unwrap();

            let package_bytes = b"static ccs payload".to_vec();
            std::fs::write(tempdir.path().join(STATIC_PACKAGE_PATH), &package_bytes).unwrap();

            let active_key = SigningKeyPair::generate().public_key_base64();
            let retired_key = SigningKeyPair::generate().public_key_base64();

            let mut repo = Repository::new(
                "static-test".to_string(),
                tempdir.path().display().to_string(),
            );
            repo.default_strategy = Some("static".to_string());
            repo.tuf_enabled = true;

            let mut fixture = Self {
                _tempdir: tempdir,
                repo,
                package_bytes,
                active_key,
                retired_key,
            };
            fixture.write_static_files();
            fixture
        }

        fn root(&self) -> &Path {
            self._tempdir.path()
        }

        fn insert_repo(&mut self, conn: &Connection) -> i64 {
            self.repo.insert(conn).unwrap();
            self.repo.id.unwrap()
        }

        fn write_static_files(&mut self) {
            let package_sha = sha256(&self.package_bytes);
            let index = json!({
                "schema": 1,
                "name": "acme-tools",
                "index_version": 7,
                "generated": "2026-06-10T18:00:00Z",
                "packages": [{
                    "name": "acme-widget",
                    "version": "1.4.2",
                    "release": "1",
                    "arch": "x86_64",
                    "path": STATIC_PACKAGE_PATH,
                    "sha256": package_sha,
                    "size": self.package_bytes.len() as u64,
                    "description": "Widget frobnicator",
                    "dependencies": ["libfoo >= 2.0", "libbar"]
                }]
            });
            std::fs::write(
                self.root().join("index.json"),
                serde_json::to_vec(&index).unwrap(),
            )
            .unwrap();

            let keys = json!({
                "schema": 1,
                "keys": [
                    {
                        "algorithm": "ed25519",
                        "public_key": self.active_key,
                        "key_id": "active-key",
                        "status": "active"
                    },
                    {
                        "algorithm": "ed25519",
                        "public_key": self.retired_key,
                        "key_id": "retired-key",
                        "status": "retired"
                    }
                ]
            });
            std::fs::write(
                self.root().join("keys/package-keys.json"),
                serde_json::to_vec(&keys).unwrap(),
            )
            .unwrap();
        }

        fn verified(&self) -> VerifiedTufState {
            let mut targets = BTreeMap::new();
            targets.insert(
                "index.json".to_string(),
                target_for_bytes(&std::fs::read(self.root().join("index.json")).unwrap()),
            );
            targets.insert(
                "keys/package-keys.json".to_string(),
                target_for_bytes(
                    &std::fs::read(self.root().join("keys/package-keys.json")).unwrap(),
                ),
            );
            targets.insert(
                STATIC_PACKAGE_PATH.to_string(),
                target_for_bytes(&self.package_bytes),
            );

            VerifiedTufState {
                root_version: 1,
                targets_version: 7,
                snapshot_version: 7,
                timestamp_version: 7,
                targets,
            }
        }
    }

    fn target_for_bytes(bytes: &[u8]) -> TargetDescription {
        let mut hashes = BTreeMap::new();
        hashes.insert("sha256".to_string(), sha256(bytes));
        TargetDescription {
            length: bytes.len() as u64,
            hashes,
        }
    }

    fn static_repo(name: &str, url: &str) -> Repository {
        let mut repo = Repository::new(name.to_string(), url.to_string());
        repo.default_strategy = Some("static".to_string());
        repo
    }

    fn assert_repin_error(error: impl std::fmt::Display, name: &str, url: &str) {
        let error = error.to_string();
        assert!(error.contains("Static repository trust is not established"));
        assert!(error.contains(&format!(
            "conary repo add {name} {url} --fingerprint <root-key-id> --replace"
        )));
    }

    #[tokio::test]
    async fn static_repo_without_tuf_enabled_hard_fails_before_native_or_json_fetch() {
        let (_temp, conn) = crate::db::testing::create_test_db();
        let mut repo = static_repo("static-test", "file:///definitely/missing/static-repo");
        repo.insert(&conn).unwrap();

        let err = sync_repository(&conn, &mut repo).await.unwrap_err();

        assert_repin_error(err, "static-test", "file:///definitely/missing/static-repo");
    }

    #[tokio::test]
    async fn static_repo_db_path_without_tuf_enabled_hard_fails_before_native_or_json_fetch() {
        let (temp, _conn) = crate::db::testing::create_test_db();
        let repo = static_repo("static-test", "file:///definitely/missing/static-repo");

        let err = sync_repository_from_db_path(temp.path().to_path_buf(), repo)
            .await
            .unwrap_err();

        assert_repin_error(err, "static-test", "file:///definitely/missing/static-repo");
    }

    #[tokio::test]
    async fn static_repo_without_trusted_root_hard_fails_with_repin_command() {
        let (_temp, conn) = crate::db::testing::create_test_db();
        let mut repo = static_repo("static-test", "file:///definitely/missing/static-repo");
        repo.tuf_enabled = true;
        repo.insert(&conn).unwrap();

        let err = sync_repository(&conn, &mut repo).await.unwrap_err();

        assert_repin_error(err, "static-test", "file:///definitely/missing/static-repo");
    }

    #[tokio::test]
    async fn static_repo_db_path_without_trusted_root_hard_fails_with_repin_command() {
        let (temp, conn) = crate::db::testing::create_test_db();
        let mut repo = static_repo("static-test", "file:///definitely/missing/static-repo");
        repo.tuf_enabled = true;
        repo.insert(&conn).unwrap();

        let err = sync_repository_from_db_path(temp.path().to_path_buf(), repo)
            .await
            .unwrap_err();

        assert_repin_error(err, "static-test", "file:///definitely/missing/static-repo");
    }

    #[test]
    fn static_sync_snapshot_persists_packages_keys_and_normalized_rows_atomically() {
        let (_temp, conn) = crate::db::testing::create_test_db();
        let mut fixture = StaticSyncFixture::new();
        let repo_id = fixture.insert_repo(&conn);
        let snapshot = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(
                crate::repository::static_repo::sync::fetch_static_sync_snapshot(
                    &fixture.repo,
                    &fixture.verified(),
                ),
            )
            .unwrap();

        let count = persist_repository_sync_snapshot(&conn, &mut fixture.repo, snapshot).unwrap();

        assert_eq!(count, 1);
        assert!(fixture.repo.last_sync.is_some());

        let packages = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "acme-widget");
        assert_eq!(packages[0].version, "1.4.2");
        assert_eq!(
            packages[0].dependencies.as_deref(),
            Some(r#"["libfoo >= 2.0","libbar"]"#)
        );
        assert!(
            packages[0]
                .metadata
                .as_deref()
                .is_some_and(|metadata| metadata.contains(r#""release":"1""#))
        );

        let package_id = packages[0].id.unwrap();
        let provides = RepositoryProvide::find_by_repository_package(&conn, package_id).unwrap();
        assert!(provides.iter().any(|provide| {
            provide.capability == "acme-widget"
                && provide.version.as_deref() == Some("1.4.2")
                && provide.raw.as_deref() == Some("acme-widget")
        }));

        let requirements =
            RepositoryRequirement::find_by_repository_package(&conn, package_id).unwrap();
        assert!(requirements.iter().any(|requirement| {
            requirement.capability == "libfoo"
                && requirement.version_constraint.as_deref() == Some(">= 2.0")
                && requirement.raw.as_deref() == Some("libfoo >= 2.0")
        }));

        let trusted = RepositoryPackageKey::trusted_keys_for_repository(&conn, repo_id).unwrap();
        assert_eq!(trusted.len(), 1);
        assert!(trusted.contains(&fixture.active_key));

        let stored_keys_and_statuses: Vec<(String, String)> = conn
            .prepare(
                "SELECT public_key, status FROM repository_package_keys
                 WHERE repository_id = ?1 ORDER BY status",
            )
            .unwrap()
            .query_map([repo_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            stored_keys_and_statuses,
            vec![
                (fixture.active_key.clone(), "active".to_string()),
                (fixture.retired_key.clone(), "retired".to_string())
            ]
        );
    }

    #[test]
    fn test_rebase_download_url_no_content_url() {
        // When no content_url, the download_url should be unchanged
        let download_url = "https://example.com/fedora/Packages/foo-1.0.rpm";
        let metadata_url = "https://example.com/fedora";

        let result = rebase_download_url(download_url, metadata_url, None);
        assert_eq!(result, download_url);
    }

    #[test]
    fn test_rebase_download_url_with_content_url() {
        // Rebase from metadata URL to content URL
        let download_url = "https://metadata.example.com/fedora/Packages/foo-1.0.rpm";
        let metadata_url = "https://metadata.example.com/fedora";
        let content_url = "https://mirror.local/fedora";

        let result = rebase_download_url(download_url, metadata_url, Some(content_url));
        assert_eq!(result, "https://mirror.local/fedora/Packages/foo-1.0.rpm");
    }

    #[test]
    fn test_rebase_download_url_trailing_slashes() {
        // Handle trailing slashes correctly - no double slashes in output
        let download_url = "https://metadata.example.com/fedora/Packages/foo-1.0.rpm";
        let metadata_url = "https://metadata.example.com/fedora/";
        let content_url = "https://mirror.local/fedora/";

        let result = rebase_download_url(download_url, metadata_url, Some(content_url));
        assert_eq!(result, "https://mirror.local/fedora/Packages/foo-1.0.rpm");
        // Verify no double slashes
        assert!(
            !result.contains("//P"),
            "Should not have double slashes before path"
        );
    }

    #[test]
    fn test_rebase_download_url_no_leading_slash() {
        // Handle case where relative path has no leading slash
        let download_url = "https://metadata.example.com/fedora/Packages/foo-1.0.rpm";
        let metadata_url = "https://metadata.example.com/fedora";
        let content_url = "https://mirror.local/content";

        let result = rebase_download_url(download_url, metadata_url, Some(content_url));
        assert_eq!(result, "https://mirror.local/content/Packages/foo-1.0.rpm");
    }

    #[test]
    fn test_rebase_download_url_ubuntu_example() {
        // Real-world example: Ubuntu with local metadata but archive.ubuntu.com content
        let download_url = "https://your-server.com/ubuntu/pool/main/n/nginx/nginx_1.24.0.deb";
        let metadata_url = "https://your-server.com/ubuntu";
        let content_url = "https://archive.ubuntu.com/ubuntu";

        let result = rebase_download_url(download_url, metadata_url, Some(content_url));
        assert_eq!(
            result,
            "https://archive.ubuntu.com/ubuntu/pool/main/n/nginx/nginx_1.24.0.deb"
        );
    }

    #[test]
    fn test_rebase_download_url_different_base() {
        // When download_url doesn't match metadata_url prefix, return as-is
        let download_url = "https://other-server.com/packages/foo.rpm";
        let metadata_url = "https://metadata.example.com/fedora";
        let content_url = "https://mirror.local/fedora";

        let result = rebase_download_url(download_url, metadata_url, Some(content_url));
        // Can't rebase - different base, return as-is
        assert_eq!(result, "https://other-server.com/packages/foo.rpm");
    }

    #[test]
    fn test_persist_native_sync_rows_writes_normalized_capabilities() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let mut repo = Repository::new(
            "arch-core".to_string(),
            "https://example.com/arch".to_string(),
        );
        repo.insert(&conn).unwrap();
        let repo_id = repo.id.unwrap();

        let mut pkg_meta = PackageMetadata::new(
            "ripgrep".to_string(),
            "14.1.0-1".to_string(),
            "abc123".to_string(),
            1234,
            "https://example.com/arch/pool/ripgrep.pkg.tar.zst".to_string(),
        );
        pkg_meta.architecture = Some("x86_64".to_string());
        pkg_meta.dependencies = vec![Dependency::runtime_versioned(
            "glibc".to_string(),
            ">= 2.39".to_string(),
        )];
        pkg_meta.extra_metadata = json!({
            "arch_provides": ["rg=14.1.0-1"],
        });

        let (provides, requirements) = normalized_repository_capabilities(&pkg_meta);
        let synced_packages = vec![SyncedPackageRow {
            package: RepositoryPackage::new(
                repo_id,
                pkg_meta.name.clone(),
                pkg_meta.version.clone(),
                pkg_meta.checksum.clone(),
                pkg_meta.size as i64,
                pkg_meta.download_url.clone(),
            ),
            provides,
            requirements,
            requirement_groups: Vec::new(),
            requirement_group_clauses: Vec::new(),
        }];
        let mut repo_packages: Vec<RepositoryPackage> = synced_packages
            .iter()
            .map(|row| row.package.clone())
            .collect();

        let count = persist_native_sync_rows(&conn, &mut repo, &mut repo_packages, synced_packages)
            .unwrap();
        assert_eq!(count, 1);

        let stored_packages = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
        assert_eq!(stored_packages.len(), 1);
        let repository_package_id = stored_packages[0].id.unwrap();

        let stored_provides =
            RepositoryProvide::find_by_repository_package(&conn, repository_package_id).unwrap();
        assert_eq!(stored_provides.len(), 2);
        assert!(stored_provides.iter().any(|provide| {
            provide.capability == "ripgrep"
                && provide.version.as_deref() == Some("14.1.0-1")
                && provide.raw.as_deref() == Some("ripgrep")
        }));
        assert!(stored_provides.iter().any(|provide| {
            provide.capability == "rg"
                && provide.version.as_deref() == Some("14.1.0-1")
                && provide.raw.as_deref() == Some("rg=14.1.0-1")
        }));

        let stored_requirements =
            RepositoryRequirement::find_by_repository_package(&conn, repository_package_id)
                .unwrap();
        assert_eq!(stored_requirements.len(), 1);
        assert_eq!(stored_requirements[0].capability, "glibc");
        assert_eq!(
            stored_requirements[0].version_constraint.as_deref(),
            Some(">= 2.39")
        );
        assert_eq!(stored_requirements[0].dependency_type, "runtime");
        assert_eq!(stored_requirements[0].raw.as_deref(), Some("glibc >= 2.39"));
    }

    #[test]
    fn test_remi_package_entry_builds_normalized_capabilities() {
        let entry = RemiPackageEntry {
            name: "kernel-core".to_string(),
            version: "6.19.6-200.fc44".to_string(),
            release: None,
            converted: false,
            architecture: Some("x86_64".to_string()),
            dependencies: Some(vec![
                "kernel-modules-core-uname-r = 6.19.6-200.fc44.x86_64".to_string(),
                "glibc >= 2.39".to_string(),
            ]),
            metadata: Some(json!({
                "rpm_provides": [
                    "kernel-core-uname-r = 6.19.6-200.fc44.x86_64",
                    "kernel-core = 6.19.6-200.fc44"
                ]
            })),
        };

        let row = remi_sync_row(
            7,
            "https://remi.conary.io".to_string(),
            "fedora".to_string(),
            entry,
        );

        assert!(row.provides.iter().any(|provide| {
            provide.capability == "kernel-core-uname-r"
                && provide.version.as_deref() == Some("6.19.6-200.fc44.x86_64")
                && provide.raw.as_deref() == Some("kernel-core-uname-r = 6.19.6-200.fc44.x86_64")
        }));
        assert!(row.requirements.iter().any(|requirement| {
            requirement.capability == "kernel-modules-core-uname-r"
                && requirement.version_constraint.as_deref() == Some("= 6.19.6-200.fc44.x86_64")
                && requirement.raw.as_deref()
                    == Some("kernel-modules-core-uname-r = 6.19.6-200.fc44.x86_64")
        }));
        assert!(row.requirements.iter().any(|requirement| {
            requirement.capability == "glibc"
                && requirement.version_constraint.as_deref() == Some(">= 2.39")
        }));
    }

    #[test]
    fn test_remi_package_entry_marks_trusted_security_advisory() {
        let entry = RemiPackageEntry {
            name: "openssl".to_string(),
            version: "3.2.1-1.fc44".to_string(),
            release: None,
            converted: false,
            architecture: Some("x86_64".to_string()),
            dependencies: None,
            metadata: Some(json!({
                "security_advisory": {
                    "id": "FEDORA-2026-0001",
                    "source": "remi",
                    "source_trust": "trusted",
                    "severity": "critical",
                    "cves": ["CVE-2026-0001", "CVE-2026-0002"],
                    "fixed_version": "3.2.1-1.fc44",
                    "url": "https://security.example.test/FEDORA-2026-0001"
                }
            })),
        };

        let row = remi_sync_row(
            7,
            "https://remi.conary.io".to_string(),
            "fedora".to_string(),
            entry,
        );

        assert!(row.package.is_security_update);
        assert_eq!(row.package.severity.as_deref(), Some("critical"));
        assert_eq!(
            row.package.cve_ids.as_deref(),
            Some("CVE-2026-0001,CVE-2026-0002")
        );
        assert_eq!(row.package.advisory_id.as_deref(), Some("FEDORA-2026-0001"));
        assert_eq!(
            row.package.advisory_url.as_deref(),
            Some("https://security.example.test/FEDORA-2026-0001")
        );

        let metadata: serde_json::Value =
            serde_json::from_str(row.package.metadata.as_deref().unwrap()).unwrap();
        assert_eq!(
            metadata["security_advisory"]["fixed_version"],
            "3.2.1-1.fc44"
        );
        assert_eq!(metadata["security_advisory"]["source_trust"], "trusted");
    }

    #[test]
    fn test_json_fallback_persists_trusted_advisory_metadata() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let mut repo = Repository::new(
            "fedora-security".to_string(),
            "https://example.com/fedora".to_string(),
        );
        repo.security_advisory_support = SecurityAdvisorySupport::Supported;
        repo.insert(&conn).unwrap();
        let repo_id = repo.id.unwrap();

        let metadata: JsonRepositoryMetadata = serde_json::from_value(json!({
            "name": "fedora-security",
            "version": "1",
            "security_advisory_source": {
                "name": "conary-json",
                "trust": "trusted"
            },
            "packages": [
                {
                    "name": "openssl",
                    "version": "3.2.1-1.fc44",
                    "architecture": "x86_64",
                    "description": "TLS toolkit",
                    "checksum": "sha256:openssl-fixed",
                    "size": 4096,
                    "download_url": "https://example.com/fedora/openssl-3.2.1-1.fc44.ccs",
                    "dependencies": [],
                    "security_advisory": {
                        "id": "FEDORA-2026-0001",
                        "severity": "critical",
                        "cves": ["CVE-2026-0001"],
                        "fixed_version": "3.2.1-1.fc44",
                        "url": "https://security.example.test/FEDORA-2026-0001"
                    }
                }
            ]
        }))
        .unwrap();

        let snapshot = json_repository_sync_snapshot(&repo, metadata).unwrap();
        assert_eq!(
            persist_repository_sync_snapshot(&conn, &mut repo, snapshot).unwrap(),
            1
        );

        let stored = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
        assert_eq!(stored.len(), 1);
        let package = &stored[0];
        assert!(package.is_security_update);
        assert_eq!(package.severity.as_deref(), Some("critical"));
        assert_eq!(package.cve_ids.as_deref(), Some("CVE-2026-0001"));
        assert_eq!(package.advisory_id.as_deref(), Some("FEDORA-2026-0001"));
        assert_eq!(
            package.advisory_url.as_deref(),
            Some("https://security.example.test/FEDORA-2026-0001")
        );

        let package_metadata: serde_json::Value =
            serde_json::from_str(package.metadata.as_deref().unwrap()).unwrap();
        assert_eq!(
            package_metadata["security_advisory"]["fixed_version"],
            "3.2.1-1.fc44"
        );
        assert_eq!(
            package_metadata["security_advisory"]["source"],
            "conary-json"
        );
        assert_eq!(
            package_metadata["security_advisory"]["source_trust"],
            "trusted"
        );
    }

    #[test]
    fn test_json_fallback_supported_repo_requires_trusted_advisory_source() {
        let mut repo = Repository::new(
            "fedora-security".to_string(),
            "https://example.com/fedora".to_string(),
        );
        repo.id = Some(42);
        repo.security_advisory_support = SecurityAdvisorySupport::Supported;

        let metadata: JsonRepositoryMetadata = serde_json::from_value(json!({
            "name": "fedora-security",
            "version": "1",
            "packages": [
                {
                    "name": "openssl",
                    "version": "3.2.1-1.fc44",
                    "architecture": "x86_64",
                    "description": "TLS toolkit",
                    "checksum": "sha256:openssl-fixed",
                    "size": 4096,
                    "download_url": "https://example.com/fedora/openssl-3.2.1-1.fc44.ccs",
                    "dependencies": [],
                    "security_advisory": {
                        "id": "FEDORA-2026-0001",
                        "severity": "critical",
                        "cves": ["CVE-2026-0001"],
                        "fixed_version": "3.2.1-1.fc44"
                    }
                }
            ]
        }))
        .unwrap();

        let error = json_repository_sync_snapshot(&repo, metadata).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("trusted security advisory source"),
            "{error}"
        );
    }

    #[test]
    fn test_sync_persists_distro_and_version_scheme() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let mut repo = Repository::new(
            "fedora-updates".to_string(),
            "https://example.com/fedora".to_string(),
        );
        repo.insert(&conn).unwrap();
        let repo_id = repo.id.unwrap();

        let mut pkg_meta = PackageMetadata::new(
            "bash".to_string(),
            "5.2.37-1.fc44".to_string(),
            "deadbeef".to_string(),
            2048,
            "https://example.com/fedora/bash.rpm".to_string(),
        );
        pkg_meta.source_distro = Some(RepositoryDependencyFlavor::Rpm);
        pkg_meta.version_scheme = Some(VersionScheme::Rpm);

        let (provides, requirements) = normalized_repository_capabilities(&pkg_meta);
        let synced = vec![SyncedPackageRow {
            package: {
                let mut p = RepositoryPackage::new(
                    repo_id,
                    pkg_meta.name.clone(),
                    pkg_meta.version.clone(),
                    pkg_meta.checksum.clone(),
                    pkg_meta.size as i64,
                    pkg_meta.download_url.clone(),
                );
                p.distro = pkg_meta.source_distro.map(distro_flavor_to_db);
                p.version_scheme = pkg_meta.version_scheme.map(version_scheme_to_db);
                p
            },
            provides,
            requirements,
            requirement_groups: Vec::new(),
            requirement_group_clauses: Vec::new(),
        }];
        let mut repo_packages: Vec<RepositoryPackage> =
            synced.iter().map(|row| row.package.clone()).collect();

        persist_native_sync_rows(&conn, &mut repo, &mut repo_packages, synced).unwrap();

        let stored = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].distro.as_deref(), Some("rpm"));
        assert_eq!(stored[0].version_scheme.as_deref(), Some("rpm"));
    }

    #[test]
    fn test_sync_persists_debian_origin_metadata() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let mut repo = Repository::new(
            "debian-main".to_string(),
            "https://example.com/debian".to_string(),
        );
        repo.insert(&conn).unwrap();
        let repo_id = repo.id.unwrap();

        let mut pkg_meta = PackageMetadata::new(
            "postfix".to_string(),
            "3.9.1-1".to_string(),
            "aabbccdd".to_string(),
            512,
            "https://example.com/debian/postfix.deb".to_string(),
        );
        pkg_meta.source_distro = Some(RepositoryDependencyFlavor::Deb);
        pkg_meta.version_scheme = Some(VersionScheme::Debian);

        let (provides, requirements) = normalized_repository_capabilities(&pkg_meta);
        let synced = vec![SyncedPackageRow {
            package: {
                let mut p = RepositoryPackage::new(
                    repo_id,
                    pkg_meta.name.clone(),
                    pkg_meta.version.clone(),
                    pkg_meta.checksum.clone(),
                    pkg_meta.size as i64,
                    pkg_meta.download_url.clone(),
                );
                p.distro = pkg_meta.source_distro.map(distro_flavor_to_db);
                p.version_scheme = pkg_meta.version_scheme.map(version_scheme_to_db);
                p
            },
            provides,
            requirements,
            requirement_groups: Vec::new(),
            requirement_group_clauses: Vec::new(),
        }];
        let mut repo_packages: Vec<RepositoryPackage> =
            synced.iter().map(|row| row.package.clone()).collect();

        persist_native_sync_rows(&conn, &mut repo, &mut repo_packages, synced).unwrap();

        let stored = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].distro.as_deref(), Some("deb"));
        assert_eq!(stored[0].version_scheme.as_deref(), Some("debian"));
    }

    #[test]
    fn test_sync_persists_arch_origin_metadata() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let mut repo = Repository::new(
            "arch-core".to_string(),
            "https://example.com/arch".to_string(),
        );
        repo.insert(&conn).unwrap();
        let repo_id = repo.id.unwrap();

        let mut pkg_meta = PackageMetadata::new(
            "ripgrep".to_string(),
            "14.1.0-1".to_string(),
            "abc123".to_string(),
            1234,
            "https://example.com/arch/ripgrep.pkg.tar.zst".to_string(),
        );
        pkg_meta.source_distro = Some(RepositoryDependencyFlavor::Arch);
        pkg_meta.version_scheme = Some(VersionScheme::Arch);

        let (provides, requirements) = normalized_repository_capabilities(&pkg_meta);
        let synced = vec![SyncedPackageRow {
            package: {
                let mut p = RepositoryPackage::new(
                    repo_id,
                    pkg_meta.name.clone(),
                    pkg_meta.version.clone(),
                    pkg_meta.checksum.clone(),
                    pkg_meta.size as i64,
                    pkg_meta.download_url.clone(),
                );
                p.distro = pkg_meta.source_distro.map(distro_flavor_to_db);
                p.version_scheme = pkg_meta.version_scheme.map(version_scheme_to_db);
                p
            },
            provides,
            requirements,
            requirement_groups: Vec::new(),
            requirement_group_clauses: Vec::new(),
        }];
        let mut repo_packages: Vec<RepositoryPackage> =
            synced.iter().map(|row| row.package.clone()).collect();

        persist_native_sync_rows(&conn, &mut repo, &mut repo_packages, synced).unwrap();

        let stored = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].distro.as_deref(), Some("arch"));
        assert_eq!(stored[0].version_scheme.as_deref(), Some("arch"));
    }

    #[test]
    fn test_sync_persists_requirement_groups_with_alternatives() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let mut repo = Repository::new(
            "debian-main".to_string(),
            "https://example.com/debian".to_string(),
        );
        repo.insert(&conn).unwrap();
        let repo_id = repo.id.unwrap();

        // Simulate a Debian package with an OR dependency: default-mta | mail-transport-agent
        let or_group = dep_model::RepositoryRequirementGroup::alternatives(
            RepositoryRequirementKind::Depends,
            vec![
                dep_model::RepositoryRequirementClause::name_only("default-mta".to_string()),
                dep_model::RepositoryRequirementClause::name_only(
                    "mail-transport-agent".to_string(),
                ),
            ],
        )
        .with_native_text("default-mta | mail-transport-agent".to_string());

        let simple_group = dep_model::RepositoryRequirementGroup::simple(
            RepositoryRequirementKind::Depends,
            dep_model::RepositoryRequirementClause::versioned(
                "libc6".to_string(),
                ">= 2.34".to_string(),
            ),
        );

        let mut pkg_meta = PackageMetadata::new(
            "postfix".to_string(),
            "3.9.1-1".to_string(),
            "aabbcc".to_string(),
            4096,
            "https://example.com/debian/postfix.deb".to_string(),
        );
        pkg_meta.source_distro = Some(RepositoryDependencyFlavor::Deb);
        pkg_meta.version_scheme = Some(VersionScheme::Debian);
        pkg_meta.requirements = vec![or_group, simple_group];

        let (provides, flat_reqs) = normalized_repository_capabilities(&pkg_meta);
        let (req_groups, req_group_clauses) = convert_requirement_groups(0, &pkg_meta.requirements);

        let synced = vec![SyncedPackageRow {
            package: {
                let mut p = RepositoryPackage::new(
                    repo_id,
                    pkg_meta.name.clone(),
                    pkg_meta.version.clone(),
                    pkg_meta.checksum.clone(),
                    pkg_meta.size as i64,
                    pkg_meta.download_url.clone(),
                );
                p.distro = pkg_meta.source_distro.map(distro_flavor_to_db);
                p.version_scheme = pkg_meta.version_scheme.map(version_scheme_to_db);
                p
            },
            provides,
            requirements: flat_reqs,
            requirement_groups: req_groups,
            requirement_group_clauses: req_group_clauses,
        }];
        let mut repo_packages: Vec<RepositoryPackage> =
            synced.iter().map(|row| row.package.clone()).collect();

        persist_native_sync_rows(&conn, &mut repo, &mut repo_packages, synced).unwrap();

        let stored = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
        assert_eq!(stored.len(), 1);
        let pkg_id = stored[0].id.unwrap();

        // Verify requirement groups were persisted
        let groups = DbRequirementGroup::find_by_repository_package(&conn, pkg_id).unwrap();
        assert_eq!(groups.len(), 2);

        // First group: OR alternative
        let or = groups
            .iter()
            .find(|g| g.native_text.as_deref() == Some("default-mta | mail-transport-agent"));
        assert!(or.is_some(), "OR group should be persisted");
        let or = or.unwrap();
        assert_eq!(or.kind, "depends");
        assert_eq!(or.behavior, "hard");

        // Verify the OR group has two clauses
        let or_clauses = RepositoryRequirement::find_by_group(&conn, or.id.unwrap()).unwrap();
        assert_eq!(or_clauses.len(), 2);
        assert!(or_clauses.iter().any(|c| c.capability == "default-mta"));
        assert!(
            or_clauses
                .iter()
                .any(|c| c.capability == "mail-transport-agent")
        );

        // Second group: simple versioned dependency
        let simple = groups.iter().find(|g| g.native_text.is_none());
        assert!(simple.is_some(), "simple group should be persisted");
        let simple = simple.unwrap();
        let simple_clauses =
            RepositoryRequirement::find_by_group(&conn, simple.id.unwrap()).unwrap();
        assert_eq!(simple_clauses.len(), 1);
        assert_eq!(simple_clauses[0].capability, "libc6");
        assert_eq!(
            simple_clauses[0].version_constraint.as_deref(),
            Some(">= 2.34")
        );
    }

    #[test]
    fn test_sync_persists_conditional_requirement_behavior() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let mut repo = Repository::new(
            "fedora".to_string(),
            "https://example.com/fedora".to_string(),
        );
        repo.insert(&conn).unwrap();
        let repo_id = repo.id.unwrap();

        // Simulate a conditional RPM rich dependency
        let conditional_group = dep_model::RepositoryRequirementGroup::simple(
            RepositoryRequirementKind::Depends,
            dep_model::RepositoryRequirementClause::versioned(
                "systemd".to_string(),
                ">= 255".to_string(),
            ),
        )
        .with_behavior(ConditionalRequirementBehavior::Conditional)
        .with_native_text("(systemd >= 255 if systemd-resolved)".to_string());

        let mut pkg_meta = PackageMetadata::new(
            "resolved-client".to_string(),
            "1.0-1.fc44".to_string(),
            "ff00ff".to_string(),
            256,
            "https://example.com/fedora/resolved-client.rpm".to_string(),
        );
        pkg_meta.source_distro = Some(RepositoryDependencyFlavor::Rpm);
        pkg_meta.version_scheme = Some(VersionScheme::Rpm);
        pkg_meta.requirements = vec![conditional_group];

        let (provides, flat_reqs) = normalized_repository_capabilities(&pkg_meta);
        let (req_groups, req_group_clauses) = convert_requirement_groups(0, &pkg_meta.requirements);

        let synced = vec![SyncedPackageRow {
            package: {
                let mut p = RepositoryPackage::new(
                    repo_id,
                    pkg_meta.name.clone(),
                    pkg_meta.version.clone(),
                    pkg_meta.checksum.clone(),
                    pkg_meta.size as i64,
                    pkg_meta.download_url.clone(),
                );
                p.distro = pkg_meta.source_distro.map(distro_flavor_to_db);
                p.version_scheme = pkg_meta.version_scheme.map(version_scheme_to_db);
                p
            },
            provides,
            requirements: flat_reqs,
            requirement_groups: req_groups,
            requirement_group_clauses: req_group_clauses,
        }];
        let mut repo_packages: Vec<RepositoryPackage> =
            synced.iter().map(|row| row.package.clone()).collect();

        persist_native_sync_rows(&conn, &mut repo, &mut repo_packages, synced).unwrap();

        let stored = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
        let pkg_id = stored[0].id.unwrap();

        let groups = DbRequirementGroup::find_by_repository_package(&conn, pkg_id).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].kind, "depends");
        assert_eq!(groups[0].behavior, "conditional");
        assert_eq!(
            groups[0].native_text.as_deref(),
            Some("(systemd >= 255 if systemd-resolved)")
        );

        let clauses = RepositoryRequirement::find_by_group(&conn, groups[0].id.unwrap()).unwrap();
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].capability, "systemd");
        assert_eq!(clauses[0].version_constraint.as_deref(), Some(">= 255"));
    }

    #[test]
    fn test_canonical_map_deserialization_and_persist() {
        use crate::db::models::{CanonicalPackage, PackageImplementation};

        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        // Simulate the JSON response from GET /v1/canonical/map
        let json = json!({
            "version": 1,
            "generated_at": "2026-03-16T00:00:00Z",
            "entries": [
                {
                    "canonical": "firefox",
                    "implementations": {
                        "fedora": "firefox",
                        "debian": "firefox-esr",
                        "arch": "firefox"
                    }
                },
                {
                    "canonical": "openssl",
                    "implementations": {
                        "fedora": "openssl",
                        "ubuntu": "libssl3"
                    }
                }
            ]
        });

        let map: CanonicalMapResponse = serde_json::from_value(json).unwrap();
        assert_eq!(map.entries.len(), 2);

        // Persist the map entries using the same logic as fetch_and_persist_canonical_map
        let tx = conn.unchecked_transaction().unwrap();
        let mut count = 0u64;

        for entry in &map.entries {
            let mut canonical =
                CanonicalPackage::new(entry.canonical.clone(), "package".to_string());
            let Some(canonical_id) = canonical.insert_or_ignore(&tx).unwrap() else {
                continue;
            };

            for (distro, distro_name) in &entry.implementations {
                let mut imp = PackageImplementation::new(
                    canonical_id,
                    distro.clone(),
                    distro_name.clone(),
                    "remi".to_string(),
                );
                imp.insert_or_ignore(&tx).unwrap();
                count += 1;
            }
        }
        tx.commit().unwrap();

        assert_eq!(count, 5);

        // Verify canonical packages were persisted
        let firefox = CanonicalPackage::find_by_name(&conn, "firefox")
            .unwrap()
            .unwrap();
        assert_eq!(firefox.kind, "package");

        let openssl = CanonicalPackage::find_by_name(&conn, "openssl")
            .unwrap()
            .unwrap();
        assert_eq!(openssl.kind, "package");

        // Verify implementations
        let ff_impls =
            PackageImplementation::find_by_canonical(&conn, firefox.id.unwrap()).unwrap();
        assert_eq!(ff_impls.len(), 3);
        let debian_impl = ff_impls.iter().find(|i| i.distro == "debian").unwrap();
        assert_eq!(debian_impl.distro_name, "firefox-esr");
        assert_eq!(debian_impl.source, "remi");

        let ssl_impls =
            PackageImplementation::find_by_canonical(&conn, openssl.id.unwrap()).unwrap();
        assert_eq!(ssl_impls.len(), 2);
        let ubuntu_impl = ssl_impls.iter().find(|i| i.distro == "ubuntu").unwrap();
        assert_eq!(ubuntu_impl.distro_name, "libssl3");

        // Second ingest is idempotent -- no duplicate rows
        let tx2 = conn.unchecked_transaction().unwrap();
        for entry in &map.entries {
            let mut canonical =
                CanonicalPackage::new(entry.canonical.clone(), "package".to_string());
            let Some(canonical_id) = canonical.insert_or_ignore(&tx2).unwrap() else {
                continue;
            };

            for (distro, distro_name) in &entry.implementations {
                let mut imp = PackageImplementation::new(
                    canonical_id,
                    distro.clone(),
                    distro_name.clone(),
                    "remi".to_string(),
                );
                imp.insert_or_ignore(&tx2).unwrap();
            }
        }
        tx2.commit().unwrap();

        let ff_impls2 =
            PackageImplementation::find_by_canonical(&conn, firefox.id.unwrap()).unwrap();
        assert_eq!(
            ff_impls2.len(),
            3,
            "No duplicate implementations after re-ingest"
        );
    }

    #[test]
    fn test_link_canonical_ids_populates_from_implementations() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        conn.execute(
            "INSERT INTO canonical_packages (name, kind) VALUES ('firefox-web', 'package')",
            [],
        )
        .unwrap();
        let canonical_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO package_implementations (canonical_id, distro, distro_name, source)
             VALUES (?1, 'fedora-41', 'firefox', 'curated')",
            [canonical_id],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority, default_strategy_distro)
             VALUES ('fedora-41', 'https://example.com', 1, 10, 'fedora-41')",
            [],
        )
        .unwrap();
        let repo_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url)
             VALUES (?1, 'firefox', '125.0', 'sha256:abc', 1024, 'https://example.com/firefox.rpm')",
            [repo_id],
        )
        .unwrap();
        let pkg_id = conn.last_insert_rowid();

        let count = link_canonical_ids(&conn, repo_id).unwrap();
        assert_eq!(count, 1);

        let cid: Option<i64> = conn
            .query_row(
                "SELECT canonical_id FROM repository_packages WHERE id = ?1",
                [pkg_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cid, Some(canonical_id));
    }

    #[test]
    fn test_link_canonical_ids_skips_already_linked() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        conn.execute(
            "INSERT INTO canonical_packages (name, kind) VALUES ('test', 'package')",
            [],
        )
        .unwrap();
        let canonical_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('test-repo', 'https://example.com', 1, 10)",
            [],
        )
        .unwrap();
        let repo_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, canonical_id)
             VALUES (?1, 'test-pkg', '1.0', 'sha256:x', 100, 'https://example.com/x', ?2)",
            rusqlite::params![repo_id, canonical_id],
        )
        .unwrap();

        let count = link_canonical_ids(&conn, repo_id).unwrap();
        assert_eq!(count, 0);
    }
}
