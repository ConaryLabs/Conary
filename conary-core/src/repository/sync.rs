// conary-core/src/repository/sync.rs

//! Repository synchronization
//!
//! Functions for synchronizing repository metadata from remote sources,
//! including native format support for Arch, Debian, and Fedora repositories.

use crate::db::models::{
    PackageDelta, Repository, RepositoryPackage, RepositoryProvide, RepositoryRequirement,
    RepositoryRequirementGroup as DbRequirementGroup,
};
use crate::error::{Error, Result};
use crate::repository::dependency_model::{
    ConditionalRequirementBehavior, RepositoryDependencyFlavor, RepositoryRequirementKind,
};
use crate::repository::parsers::{DependencyType, PackageMetadata};
use crate::repository::versioning::VersionScheme;
use rusqlite::Connection;
use std::collections::HashSet;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

use super::client::RepositoryClient;
use super::gpg::GpgVerifier;
use super::registry::{self, RepositoryFormat};

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

    Ok(u64::try_from(dt.timestamp()).unwrap_or(0))
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

/// Synchronize repository using native metadata format parsers
fn sync_repository_native(
    conn: &Connection,
    repo: &mut Repository,
    format: RepositoryFormat,
) -> Result<usize> {
    info!(
        "Syncing repository {} using native {:?} format",
        repo.name, format
    );

    // Create and use parser from registry
    let parser = registry::create_parser(format, &repo.name, &repo.url)?;
    let packages = parser.sync_metadata(&repo.url)?;

    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;

    // Check if we need to rebase download URLs (reference mirror pattern)
    let needs_rebase = repo.content_url.is_some();
    if needs_rebase {
        info!(
            "Repository {} uses reference mirror - rebasing download URLs to {}",
            repo.name,
            repo.content_url.as_deref().unwrap_or("")
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
    let mut repo_packages: Vec<RepositoryPackage> = synced_packages
        .iter()
        .map(|row| row.package.clone())
        .collect();

    let count = persist_native_sync_rows(conn, repo, &mut repo_packages, synced_packages)?;

    info!(
        "Synchronized {} packages from repository {}",
        count, repo.name
    );
    Ok(count)
}

/// A single synced package row with all its normalized capability data.
struct SyncedPackageRow {
    package: RepositoryPackage,
    provides: Vec<RepositoryProvide>,
    requirements: Vec<RepositoryRequirement>,
    requirement_groups: Vec<DbRequirementGroup>,
    requirement_group_clauses: Vec<Vec<RepositoryRequirement>>,
}

fn persist_native_sync_rows(
    conn: &Connection,
    repo: &mut Repository,
    repo_packages: &mut [RepositoryPackage],
    synced_packages: Vec<SyncedPackageRow>,
) -> Result<usize> {
    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;
    let count = synced_packages.len();

    // Use a transaction for bulk operations (much faster than individual inserts)
    let tx = conn.unchecked_transaction()?;

    // Delete old package entries for this repository (cascades to provides/requirements/groups)
    RepositoryPackage::delete_by_repository(&tx, repo_id)?;

    // Batch insert all packages using prepared statement and retain generated IDs
    RepositoryPackage::batch_insert_with_ids(&tx, repo_packages)?;

    let mut repo_provides = Vec::new();
    let mut repo_requirements = Vec::new();
    let mut all_groups: Vec<DbRequirementGroup> = Vec::new();
    let mut all_group_clauses: Vec<Vec<RepositoryRequirement>> = Vec::new();

    for (pkg, row) in repo_packages.iter().zip(synced_packages.into_iter()) {
        let Some(repository_package_id) = pkg.id else {
            return Err(Error::InitError(
                "Inserted repository package missing generated ID".to_string(),
            ));
        };

        repo_provides.extend(row.provides.into_iter().map(|mut provide| {
            provide.repository_package_id = repository_package_id;
            provide
        }));
        repo_requirements.extend(row.requirements.into_iter().map(|mut requirement| {
            requirement.repository_package_id = repository_package_id;
            requirement
        }));

        // Fix up group package IDs and collect for batch insert
        for mut group in row.requirement_groups {
            group.repository_package_id = repository_package_id;
            all_groups.push(group);
        }
        for mut clauses in row.requirement_group_clauses {
            for clause in &mut clauses {
                clause.repository_package_id = repository_package_id;
            }
            all_group_clauses.push(clauses);
        }
    }

    RepositoryProvide::batch_insert(&tx, &repo_provides)?;
    RepositoryRequirement::batch_insert(&tx, &repo_requirements)?;

    // Insert requirement groups and link their clauses
    DbRequirementGroup::batch_insert_with_ids(&tx, &mut all_groups)?;
    let mut grouped_clauses = Vec::new();
    for (group, clauses) in all_groups.iter().zip(all_group_clauses.into_iter()) {
        let group_id = group.id.ok_or_else(|| {
            Error::InitError("Inserted requirement group missing generated ID".to_string())
        })?;
        grouped_clauses.extend(
            clauses
                .into_iter()
                .map(|clause| clause.with_group(group_id)),
        );
    }
    RepositoryRequirement::batch_insert(&tx, &grouped_clauses)?;

    // Update last_sync timestamp
    repo.last_sync = Some(current_timestamp());
    repo.update(&tx)?;

    tx.commit()?;

    Ok(count)
}

fn normalized_repository_capabilities(
    pkg_meta: &PackageMetadata,
) -> (Vec<RepositoryProvide>, Vec<RepositoryRequirement>) {
    let mut provides = vec![RepositoryProvide::new(
        0,
        pkg_meta.name.clone(),
        Some(pkg_meta.version.clone()),
        "package".to_string(),
        Some(pkg_meta.name.clone()),
    )];

    provides.extend(
        extract_extra_metadata_provides(&pkg_meta.extra_metadata)
            .into_iter()
            .map(|(capability, version, raw)| {
                RepositoryProvide::new(0, capability, version, "package".to_string(), Some(raw))
            }),
    );

    let requirements = pkg_meta
        .dependencies
        .iter()
        .map(|dep| {
            let raw = if let Some(constraint) = dep.constraint.as_deref() {
                if constraint.is_empty() {
                    dep.name.clone()
                } else {
                    format!("{} {}", dep.name, constraint)
                }
            } else {
                dep.name.clone()
            };

            let dependency_type = match dep.dep_type {
                DependencyType::Runtime => "runtime",
                DependencyType::Optional => "optional",
                DependencyType::Build => "build",
            };

            let version_constraint = dep
                .constraint
                .as_ref()
                .and_then(|constraint| (!constraint.is_empty()).then(|| constraint.clone()));

            RepositoryRequirement::new(
                0,
                dep.name.clone(),
                version_constraint,
                "package".to_string(),
                dependency_type.to_string(),
                Some(raw),
            )
        })
        .collect();

    (provides, requirements)
}

fn extract_extra_metadata_provides(
    metadata: &serde_json::Value,
) -> Vec<(String, Option<String>, String)> {
    let mut parsed = Vec::new();

    for key in ["rpm_provides", "deb_provides", "arch_provides"] {
        let Some(entries) = metadata.get(key).and_then(|value| value.as_array()) else {
            continue;
        };

        for raw in entries.iter().filter_map(|value| value.as_str()) {
            let (capability, version) = parse_metadata_provide_entry(raw);
            parsed.push((capability, version, raw.to_string()));
        }
    }

    parsed
}

fn parse_metadata_provide_entry(entry: &str) -> (String, Option<String>) {
    const OPS: [&str; 5] = ["<=", ">=", "=", "<", ">"];

    for op in OPS {
        if let Some((name, version)) = entry.split_once(op) {
            let name = name.trim();
            let version = version.trim();
            if name.is_empty() || version.is_empty() {
                continue;
            }
            return (name.to_string(), Some(version.to_string()));
        }
    }

    (entry.trim().to_string(), None)
}

/// Convert a `RepositoryDependencyFlavor` to its database string representation.
fn distro_flavor_to_db(flavor: RepositoryDependencyFlavor) -> String {
    match flavor {
        RepositoryDependencyFlavor::Rpm => "rpm".to_string(),
        RepositoryDependencyFlavor::Deb => "deb".to_string(),
        RepositoryDependencyFlavor::Arch => "arch".to_string(),
    }
}

/// Convert a `VersionScheme` to its database string representation.
fn version_scheme_to_db(scheme: VersionScheme) -> String {
    match scheme {
        VersionScheme::Rpm => "rpm".to_string(),
        VersionScheme::Debian => "debian".to_string(),
        VersionScheme::Arch => "arch".to_string(),
    }
}

/// Convert a `RepositoryRequirementKind` to its database string representation.
fn requirement_kind_to_db(kind: RepositoryRequirementKind) -> String {
    match kind {
        RepositoryRequirementKind::Depends => "depends".to_string(),
        RepositoryRequirementKind::PreDepends => "pre_depends".to_string(),
        RepositoryRequirementKind::Optional => "optional".to_string(),
        RepositoryRequirementKind::Build => "build".to_string(),
        RepositoryRequirementKind::Conflict => "conflict".to_string(),
        RepositoryRequirementKind::Breaks => "breaks".to_string(),
    }
}

/// Convert a `ConditionalRequirementBehavior` to its database string representation.
fn behavior_to_db(behavior: ConditionalRequirementBehavior) -> String {
    match behavior {
        ConditionalRequirementBehavior::Hard => "hard".to_string(),
        ConditionalRequirementBehavior::Conditional => "conditional".to_string(),
        ConditionalRequirementBehavior::UnsupportedRich => "unsupported_rich".to_string(),
    }
}

/// Convert parser-level requirement groups into DB model groups and their linked clauses.
///
/// Returns `(groups, clauses)` where each clause has a placeholder `group_id` of 0
/// that will be fixed up after the groups are inserted with real IDs.
fn convert_requirement_groups(
    repository_package_id: i64,
    groups: &[crate::repository::dependency_model::RepositoryRequirementGroup],
) -> (Vec<DbRequirementGroup>, Vec<Vec<RepositoryRequirement>>) {
    let mut db_groups = Vec::with_capacity(groups.len());
    let mut clause_batches = Vec::with_capacity(groups.len());

    for group in groups {
        let mut db_group = DbRequirementGroup::new(
            repository_package_id,
            requirement_kind_to_db(group.kind),
            behavior_to_db(group.behavior),
        );
        db_group.description = group.description.clone();
        db_group.native_text = group.native_text.clone();

        let clauses: Vec<RepositoryRequirement> = group
            .alternatives
            .iter()
            .map(|clause| {
                let dep_type = match group.kind {
                    RepositoryRequirementKind::Optional => "optional",
                    RepositoryRequirementKind::Build => "build",
                    _ => "runtime",
                };
                // group_id placeholder 0 -- fixed up after group insert
                RepositoryRequirement::new(
                    repository_package_id,
                    clause.name.clone(),
                    clause.version_constraint.clone(),
                    "package".to_string(),
                    dep_type.to_string(),
                    clause.native_text.clone(),
                )
            })
            .collect();

        db_groups.push(db_group);
        clause_batches.push(clauses);
    }

    (db_groups, clause_batches)
}

/// Response from Remi metadata API (`GET /v1/{distro}/metadata`)
#[derive(serde::Deserialize)]
struct RemiMetadataResponse {
    packages: Vec<RemiPackageEntry>,
}

/// Individual package entry from Remi metadata
#[derive(serde::Deserialize)]
struct RemiPackageEntry {
    name: String,
    version: String,
    #[allow(dead_code)]
    converted: bool,
    dependencies: Option<Vec<String>>,
    metadata: Option<serde_json::Value>,
}

fn remi_sync_row(
    repo_id: i64,
    endpoint: String,
    distro: String,
    entry: RemiPackageEntry,
) -> SyncedPackageRow {
    let system_arch = registry::detect_system_arch();
    let download_url = format!("{endpoint}/v1/{distro}/packages/{}/download", entry.name);

    let mut pkg = RepositoryPackage::new(
        repo_id,
        entry.name.clone(),
        entry.version.clone(),
        "remi:server-verified".to_string(),
        0,
        download_url,
    );
    pkg.architecture = Some(system_arch);
    pkg.dependencies = entry
        .dependencies
        .as_ref()
        .map(|deps| serde_json::to_string(deps).unwrap_or_default());
    pkg.metadata = entry.metadata.as_ref().map(|value| value.to_string());

    let metadata = entry.metadata.unwrap_or(serde_json::Value::Null);
    let mut provides = vec![RepositoryProvide::new(
        0,
        entry.name.clone(),
        Some(entry.version.clone()),
        "package".to_string(),
        Some(entry.name.clone()),
    )];
    provides.extend(extract_extra_metadata_provides(&metadata).into_iter().map(
        |(capability, version, raw)| {
            RepositoryProvide::new(0, capability, version, "package".to_string(), Some(raw))
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
        package: pkg,
        provides,
        requirements,
        requirement_groups: Vec::new(),
        requirement_group_clauses: Vec::new(),
    }
}

fn parse_raw_dependency_entry(entry: &str) -> (String, Option<String>) {
    const OPS: [&str; 5] = ["<=", ">=", "=", "<", ">"];

    for op in OPS {
        if let Some((name, version)) = entry.split_once(op) {
            let name = name.trim();
            let version = version.trim();
            if name.is_empty() || version.is_empty() {
                continue;
            }
            return (name.to_string(), Some(format!("{op} {version}")));
        }
    }

    (entry.trim().to_string(), None)
}

/// Synchronize repository directly from a Remi metadata API
///
/// For repos with `default_strategy = "remi"`, fetches the package index from
/// the Remi server's `/v1/{distro}/metadata` endpoint instead of parsing
/// traditional repo formats (repomd.xml, Packages, etc.).
fn sync_repository_remi(conn: &Connection, repo: &mut Repository) -> Result<usize> {
    let distro = repo.default_strategy_distro.as_deref().ok_or_else(|| {
        Error::ConfigError(format!(
            "Repository '{}' has strategy 'remi' but no distro configured (use --remi-distro)",
            repo.name
        ))
    })?;

    // Prefer explicit endpoint, fall back to repo URL itself
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
    let bytes = client.download_to_bytes(&metadata_url)?;
    let response: RemiMetadataResponse = serde_json::from_slice(&bytes).map_err(|e| {
        Error::ParseError(format!(
            "Failed to parse Remi metadata from {}: {}",
            metadata_url, e
        ))
    })?;

    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;

    // Deduplicate by (name, version, arch) — Remi metadata may contain duplicates
    let mut seen = HashSet::new();
    let synced_packages: Vec<SyncedPackageRow> = response
        .packages
        .into_iter()
        .filter_map(|entry| {
            let system_arch = registry::detect_system_arch();
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

/// Synchronize repository metadata with the database
pub fn sync_repository(conn: &Connection, repo: &mut Repository) -> Result<usize> {
    info!("Synchronizing repository: {}", repo.name);

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
            .map_err(|e| Error::TrustError(e.to_string()))?;

        info!(
            "TUF verified: root v{}, targets v{}, {} targets",
            verified.root_version,
            verified.targets_version,
            verified.targets.len()
        );
    }

    // Route to Remi-native sync if strategy is "remi"
    if repo.default_strategy.as_deref() == Some("remi") {
        return sync_repository_remi(conn, repo);
    }

    // Detect repository format using registry
    let format = registry::detect_repository_format(&repo.name, &repo.url);

    // Try native format first if detected
    if format != RepositoryFormat::Json {
        match sync_repository_native(conn, repo, format) {
            Ok(count) => return Ok(count),
            Err(e) => {
                warn!("Native format sync failed: {}, falling back to JSON", e);
            }
        }
    }

    // Fall back to JSON metadata format
    let client = RepositoryClient::new()?;
    let metadata = client.fetch_metadata(&repo.url)?;

    let repo_id = repo
        .id
        .ok_or_else(|| Error::InitError("Repository has no ID".to_string()))?;

    // Use a transaction for the delete + insert (atomic fallback sync)
    let tx = conn.unchecked_transaction()?;

    // Delete old package entries for this repository
    RepositoryPackage::delete_by_repository(&tx, repo_id)?;

    // Insert new package metadata
    let mut count = 0;
    let mut delta_count = 0;

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

        repo_pkg.insert(&tx)?;
        count += 1;

        // Store delta metadata if available
        if let Some(deltas) = pkg_meta.delta_from {
            for delta_info in deltas {
                let mut delta = PackageDelta::new(
                    pkg_meta.name.clone(),
                    delta_info.from_version,
                    pkg_meta.version.clone(),
                    delta_info.from_hash,
                    pkg_meta.checksum.clone(),
                    delta_info.delta_url,
                    delta_info.delta_size,
                    delta_info.delta_checksum,
                    pkg_meta.size,
                );

                delta.insert(&tx)?;
                delta_count += 1;
            }
        }
    }

    // Update last_sync timestamp
    repo.last_sync = Some(current_timestamp());
    repo.update(&tx)?;

    tx.commit()?;

    info!(
        "Synchronized {} packages and {} deltas from repository {}",
        count, delta_count, repo.name
    );
    Ok(count)
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
pub fn maybe_fetch_gpg_key(repo: &Repository, keyring_dir: &Path) -> Result<Option<String>> {
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
    let key_data = client.download_to_bytes(key_url).map_err(|e| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{
        RepositoryProvide, RepositoryRequirement, RepositoryRequirementGroup as DbRequirementGroup,
    };
    use crate::db::schema::migrate;
    use crate::repository::dependency_model::{
        self as dep_model, ConditionalRequirementBehavior, RepositoryRequirementKind,
    };
    use crate::repository::parsers::Dependency;
    use serde_json::json;

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
            version: "6.19.6-200.fc43".to_string(),
            converted: false,
            dependencies: Some(vec![
                "kernel-modules-core-uname-r = 6.19.6-200.fc43.x86_64".to_string(),
                "glibc >= 2.39".to_string(),
            ]),
            metadata: Some(json!({
                "rpm_provides": [
                    "kernel-core-uname-r = 6.19.6-200.fc43.x86_64",
                    "kernel-core = 6.19.6-200.fc43"
                ]
            })),
        };

        let row = remi_sync_row(
            7,
            "https://packages.conary.io".to_string(),
            "fedora".to_string(),
            entry,
        );

        assert!(row.provides.iter().any(|provide| {
            provide.capability == "kernel-core-uname-r"
                && provide.version.as_deref() == Some("6.19.6-200.fc43.x86_64")
                && provide.raw.as_deref() == Some("kernel-core-uname-r = 6.19.6-200.fc43.x86_64")
        }));
        assert!(row.requirements.iter().any(|requirement| {
            requirement.capability == "kernel-modules-core-uname-r"
                && requirement.version_constraint.as_deref() == Some("= 6.19.6-200.fc43.x86_64")
                && requirement.raw.as_deref()
                    == Some("kernel-modules-core-uname-r = 6.19.6-200.fc43.x86_64")
        }));
        assert!(row.requirements.iter().any(|requirement| {
            requirement.capability == "glibc"
                && requirement.version_constraint.as_deref() == Some(">= 2.39")
        }));
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
            "5.2.37-1.fc43".to_string(),
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
            "1.0-1.fc43".to_string(),
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
}
