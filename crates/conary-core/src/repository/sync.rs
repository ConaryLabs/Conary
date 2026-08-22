// conary-core/src/repository/sync.rs

//! Repository synchronization
//!
//! Functions for synchronizing repository metadata from remote sources,
//! including native format support for Arch, Debian, and Fedora repositories.

use crate::db::models::{
    PackageDelta, Repository, RepositoryPackage, RepositoryPackageKey, RepositoryProvide,
};
use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::info;

use super::client::RepositoryClient;
use super::metadata::{
    PackageSecurityAdvisoryMetadata, RepositoryMetadata, SecurityAdvisorySourceMetadata,
};
use super::registry::{self, RepositoryFormat};
use super::static_repo::sync::fetch_static_sync_snapshot;
use super::trust::openpgp::PreparedOpenPgpTrust;
use super::versioning::VersionScheme;
use native::persist_native_sync_rows;
pub(in crate::repository) use native::{
    capability_kind_to_db, convert_requirement_groups, persist_synced_package_rows,
    synced_package_row,
};
use remi::{sync_repository_remi, sync_repository_remi_from_db_path};
use types::{
    JsonPackageDelta, JsonRepositorySyncSnapshot, RepositorySyncSnapshot, SyncedPackageRow,
};

mod immutable_catalog;
mod native;
mod projection_cache;
mod remi;
mod support;
pub(in crate::repository) mod types;

pub use immutable_catalog::{fetch_native_source_catalog, stream_native_source_catalog};
pub use remi::{
    PROFILE_SYNC_HEARTBEAT_INTERVAL, ProfileSyncFailureCategory, ProfileSyncFailureStage,
    ProfileSyncRun, ProfileSyncRunMember, ProfileSyncRunRecovery, abort_profile_sync_run,
    acknowledge_profile_sync_candidate_cleanup, begin_profile_sync_run,
    begin_profile_sync_run_with_input, begin_profile_sync_run_with_members,
    heartbeat_profile_sync_run, ready_profile_sync_run, record_profile_sync_run_member,
    recover_expired_profile_sync_runs,
};
pub use support::{current_timestamp, parse_timestamp};
use support::{has_trusted_root, rebase_download_url, run_blocking_sync};

/// Process-local authority for repository synchronization writes.
///
/// Path-based synchronization fetches remote metadata without holding this
/// authority, then acquires it for each short SQLite persistence phase. The
/// application supplies the authority so every database-writing subsystem in
/// that process can share one owner.
pub trait RepositoryWriteAuthority: Clone + Send + Sync + 'static {
    fn execute<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T>;
}

async fn run_blocking_write<W, T>(
    authority: W,
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T>
where
    W: RepositoryWriteAuthority,
    T: Send + 'static,
{
    run_blocking_sync(move || authority.execute(operation)).await
}

pub(super) async fn fetch_repository_native_snapshot(
    repo: &Repository,
    keyring_dir: &Path,
) -> Result<RepositorySyncSnapshot> {
    let parser = prepare_repository_native_parser(repo, keyring_dir).await?;
    let mut sink = crate::repository::parsers::CollectingRepositorySnapshotSink::create()?;
    let snapshot = parser.ingest_snapshot(&repo.url, &mut sink).await?;
    let (packages, authenticated_objects) = sink.finish();

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
            synced_package_row(
                repo_id,
                repo.source_profile.as_deref(),
                &repo.url,
                repo.content_url.as_deref(),
                pkg_meta,
            )
        })
        .collect();
    Ok(RepositorySyncSnapshot::NativeRows {
        packages: synced_packages,
        snapshot,
        authenticated_objects,
    })
}

pub(super) async fn prepare_repository_native_parser(
    repo: &Repository,
    keyring_dir: &Path,
) -> Result<registry::AnyParser> {
    let parser_config = repo.require_parser_config()?;
    repo.validate_stream_binding()?;
    let source_policy = repo.require_source_policy()?;
    let repository_identity = repo.repository_identity.as_deref().ok_or_else(|| {
        Error::ConfigError(format!(
            "repository '{}' has no exact repository identity",
            repo.name
        ))
    })?;
    info!(
        "Syncing repository {} as exact source {}/{} using exact {} parser configuration",
        repo.name,
        source_policy.source_identity,
        repository_identity,
        parser_config.format().as_str()
    );

    let trust =
        PreparedOpenPgpTrust::prepare(&repo.name, keyring_dir, repo.require_trust_policy()?)
            .await?;
    registry::create_parser(parser_config, trust)
}

/// Synchronize repository using native metadata format parsers
async fn sync_repository_native(conn: &Connection, repo: &mut Repository) -> Result<usize> {
    let keyring_dir = keyring_dir_for_connection(conn)?;
    let snapshot = fetch_repository_native_snapshot(repo, &keyring_dir).await?;
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
    match repo.require_parser_config()?.format() {
        RepositoryFormat::Json => fetch_repository_json_snapshot(repo).await,
        RepositoryFormat::Arch
        | RepositoryFormat::Debian
        | RepositoryFormat::Fedora
        | RepositoryFormat::Eopkg => fetch_repository_native_snapshot(repo, keyring_dir).await,
        RepositoryFormat::Unspecified => Err(Error::InitError(format!(
            "repository '{}' has an unspecified parser configuration",
            repo.name
        ))),
    }
}

/// Synchronize repository metadata by opening short-lived database connections
/// around blocking persistence phases.
pub async fn sync_repository_from_db_path<W>(
    db_path: PathBuf,
    repo: Repository,
    write_authority: W,
) -> Result<usize>
where
    W: RepositoryWriteAuthority,
{
    info!("Synchronizing repository: {}", repo.name);

    if is_static_repository(&repo) {
        return sync_static_repository_from_db_path(db_path, repo, write_authority).await;
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
        let verified = run_blocking_write(write_authority.clone(), move || {
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

    let count = if repo.default_strategy.as_deref() == Some("remi") {
        sync_repository_remi_from_db_path(db_path.clone(), repo.clone(), write_authority.clone())
            .await?
    } else {
        let keyring_dir = crate::db::paths::keyring_dir(&db_path.display().to_string());
        let snapshot = fetch_repository_sync_snapshot(&repo, &keyring_dir).await?;

        let persist_repo_id = repo
            .id
            .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;
        let persist_db_path = db_path.clone();
        run_blocking_write(write_authority.clone(), move || {
            let conn = crate::db::open_fast(&persist_db_path)?;
            let mut repo = Repository::find_by_id(&conn, persist_repo_id)?.ok_or_else(|| {
                Error::NotFound(format!(
                    "Repository {persist_repo_id} not found during sync"
                ))
            })?;
            persist_repository_sync_snapshot(&conn, &mut repo, snapshot)
        })
        .await?
    };

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
        match repo.require_parser_config()?.format() {
            RepositoryFormat::Json => sync_repository_json(conn, repo).await?,
            RepositoryFormat::Arch
            | RepositoryFormat::Debian
            | RepositoryFormat::Fedora
            | RepositoryFormat::Eopkg => sync_repository_native(conn, repo).await?,
            RepositoryFormat::Unspecified => {
                return Err(Error::InitError(format!(
                    "repository '{}' has an unspecified parser configuration",
                    repo.name
                )));
            }
        }
    };

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

async fn sync_static_repository_from_db_path<W>(
    db_path: PathBuf,
    repo: Repository,
    write_authority: W,
) -> Result<usize>
where
    W: RepositoryWriteAuthority,
{
    if !repo.tuf_enabled {
        return Err(static_trust_not_established_error(&repo));
    }

    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;
    let trust_db_path = db_path.clone();
    let trust_root_exists = run_blocking_sync(move || {
        let conn = crate::db::open_fast(&trust_db_path)?;
        has_trusted_root(&conn, repo_id)
    })
    .await?;
    if !trust_root_exists {
        return Err(static_trust_not_established_error(&repo));
    }

    let tuf_client = crate::trust::client::TufClient::new_static(
        repo_id,
        &repo.url,
        repo.tuf_root_url.as_deref(),
    )
    .map_err(|error| Error::TrustError(error.to_string()))?;

    let state_db_path = db_path.clone();
    let update_state = run_blocking_sync(move || {
        let conn = crate::db::open_fast(&state_db_path)?;
        tuf_client
            .load_update_state(&conn)
            .map_err(|error| Error::TrustError(error.to_string()))
    })
    .await?;

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
    let verified = run_blocking_write(write_authority.clone(), move || {
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
    run_blocking_write(write_authority, move || {
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
    if !has_trusted_root(conn, repo_id)? {
        return Err(static_trust_not_established_error(repo));
    }

    let tuf_client = crate::trust::client::TufClient::new_static(
        repo_id,
        &repo.url,
        repo.tuf_root_url.as_deref(),
    )
    .map_err(|error| Error::TrustError(error.to_string()))?;

    let verified = tuf_client
        .update(conn)
        .await
        .map_err(|error| Error::TrustError(error.to_string()))?;

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
        // Rebase download URL if content_url is configured (reference mirror)
        let download_url = rebase_download_url(
            &pkg_meta.download_url,
            &repo.url,
            repo.content_url.as_deref(),
        );

        let version_scheme = pkg_meta.version_scheme;
        if let Some(release) = pkg_meta.release.as_deref() {
            crate::repository::versioning::validate_package_release(release).map_err(|error| {
                Error::ParseError(format!(
                    "repository package '{}' has invalid CCS release '{}': {error}",
                    pkg_meta.name, release
                ))
            })?;
        }
        let mut repo_pkg = RepositoryPackage::new(
            repo_id,
            pkg_meta.name.clone(),
            pkg_meta.version.clone(),
            pkg_meta.version_scheme,
            pkg_meta.checksum.clone(),
            pkg_meta.size,
            download_url,
        );
        repo_pkg.package_release = pkg_meta.release.unwrap_or_default();
        repo_pkg.architecture = pkg_meta.architecture;
        repo_pkg.debian_multi_arch = if version_scheme == VersionScheme::Debian {
            Some(pkg_meta.debian_multi_arch.unwrap_or_default())
        } else {
            pkg_meta.debian_multi_arch
        };
        repo_pkg.description = pkg_meta.description;
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

        let mut all_groups = pkg_meta.requirements;
        all_groups.extend(pkg_meta.relations);
        for group in &all_groups {
            crate::repository::requirement::validate_requirement_group(group, version_scheme)
                .map_err(Error::ParseError)?;
        }
        let (requirement_groups, requirement_group_clauses) =
            convert_requirement_groups(0, &all_groups);
        packages.push(SyncedPackageRow {
            package: repo_pkg,
            provides: vec![RepositoryProvide::new(
                0,
                pkg_meta.name.clone(),
                Some(pkg_meta.version.clone()),
                "package".to_string(),
                Some(pkg_meta.name.clone()),
                version_scheme,
            )],
            requirement_groups,
            requirement_group_clauses,
        });

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

    Ok(RepositorySyncSnapshot::JsonContract(
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
        RepositorySyncSnapshot::NativeRows {
            packages,
            snapshot,
            authenticated_objects: _,
        } => persist_native_sync_rows(conn, repo, packages, snapshot),
        RepositorySyncSnapshot::StaticRows {
            packages,
            package_keys,
        } => persist_static_sync_rows(conn, repo, packages, package_keys),
        RepositorySyncSnapshot::JsonContract(snapshot) => {
            let repo_id = repo
                .id
                .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;
            let count = snapshot.packages.len();

            let tx = conn.unchecked_transaction()?;

            persist_synced_package_rows(&tx, repo_id, snapshot.packages)?;

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

            mark_repository_revision_published(repo);
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
    let tx = conn.unchecked_transaction()?;

    persist_synced_package_rows(&tx, repo_id, synced_packages)?;
    RepositoryPackageKey::replace_for_repository_in_transaction(&tx, repo_id, &package_keys)?;
    link_canonical_ids(&tx, repo_id)?;

    mark_repository_revision_published(repo);
    repo.update(&tx)?;

    tx.commit()?;

    Ok(count)
}

/// Synchronize a repository whose declared metadata contract is Conary JSON.
async fn sync_repository_json(conn: &Connection, repo: &mut Repository) -> Result<usize> {
    let snapshot = fetch_repository_json_snapshot(repo).await?;
    persist_repository_sync_snapshot(conn, repo, snapshot)
}

/// Check if repository metadata needs refresh
pub fn needs_sync(repo: &Repository) -> bool {
    let Some(last_checked_at) = &repo.last_checked_at else {
        return true;
    };

    let Ok(last_checked_at_time) = parse_timestamp(last_checked_at) else {
        return true;
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    now.saturating_sub(last_checked_at_time) > repo.metadata_expire as u64
}

fn mark_repository_revision_published(repo: &mut Repository) {
    let timestamp = current_timestamp();
    repo.last_checked_at = Some(timestamp.clone());
    repo.last_changed_at = Some(timestamp.clone());
    repo.last_validated_at = Some(timestamp.clone());
    repo.last_published_at = Some(timestamp);
}

/// Link repository_packages to their canonical identity.
///
/// For each package in the given repo, looks up a matching entry in
/// package_implementations by (distro_name, distro) and sets canonical_id.
/// Called after batch_insert during sync, and by `conary canonical rebuild`.
pub fn link_canonical_ids(conn: &Connection, repo_id: i64) -> Result<usize> {
    let source_profile: Option<String> = conn
        .query_row(
            "SELECT source_profile FROM repositories WHERE id = ?1",
            [repo_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();

    let Some(profile) = source_profile else {
        return Ok(0);
    };

    let updated = conn.execute(
        "UPDATE repository_packages SET canonical_id = (
            SELECT pi.canonical_id FROM package_implementations pi
            WHERE pi.distro_name = repository_packages.name
              AND pi.distro = ?1
            LIMIT 1
        ) WHERE repository_id = ?2 AND canonical_id IS NULL",
        params![profile, repo_id],
    )?;

    if updated > 0 {
        info!("Linked {updated} packages to canonical identity for repo {repo_id}");
    }

    Ok(updated)
}

#[cfg(test)]
include!("sync/tests.rs");
