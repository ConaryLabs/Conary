// apps/remi/src/server/delta_manifests.rs
//! Delta manifests for efficient package updates
//!
//! Pre-computes the set difference in chunks between package versions so
//! clients only download new chunks when upgrading. Results are persisted
//! in the `delta_manifests` table for fast lookup.

use anyhow::{Context, Result, bail};
use conary_core::db::models::{ConvertedPackage, RemiCatalogResource, RemiCatalogResourceKind};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use tracing::{debug, info};

/// A pre-computed delta between two versions of a package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaManifest {
    pub id: Option<i64>,
    /// Public profile label retained for the legacy delta cache schema. The
    /// exact profile revision supplied to the operation is the authority.
    pub source_profile: String,
    pub package_name: String,
    pub from_version: String,
    pub to_version: String,
    /// JSON array of chunk hashes present in to_version but not from_version
    pub new_chunks: Vec<String>,
    /// JSON array of chunk hashes present in from_version but not to_version
    pub removed_chunks: Vec<String>,
    /// Total download size of new chunks in bytes
    pub download_size: u64,
    /// Full package size of to_version in bytes
    pub full_size: u64,
    pub computed_at: Option<String>,
}

/// API response for a delta query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaResponse {
    pub from_version: String,
    pub to_version: String,
    pub new_chunks: Vec<String>,
    pub removed_chunks: Vec<String>,
    pub download_size: u64,
    pub full_size: u64,
    pub savings_percent: f64,
}

impl DeltaManifest {
    /// Convert to an API response
    pub fn to_response(&self) -> DeltaResponse {
        let savings_percent = if self.full_size > 0 {
            let saved = self.full_size.saturating_sub(self.download_size);
            (saved as f64 / self.full_size as f64) * 100.0
        } else {
            0.0
        };

        DeltaResponse {
            from_version: self.from_version.clone(),
            to_version: self.to_version.clone(),
            new_chunks: self.new_chunks.clone(),
            removed_chunks: self.removed_chunks.clone(),
            download_size: self.download_size,
            full_size: self.full_size,
            savings_percent,
        }
    }
}

#[derive(Debug, Clone)]
struct ProfileRevisionContext {
    source_profile: String,
    profile_revision_sha256: String,
}

/// Resolve one exact profile revision through its durable content-addressed
/// resource. The resource metadata is the only source of the public profile
/// label used by the legacy `delta_manifests.source_profile` cache column.
fn profile_revision_context(
    conn: &Connection,
    profile_revision_sha256: &str,
) -> Result<ProfileRevisionContext> {
    let resource =
        RemiCatalogResource::find_by_sha256(conn, profile_revision_sha256)?.ok_or_else(|| {
            anyhow::anyhow!("profile revision resource {profile_revision_sha256} is not registered")
        })?;
    if resource.kind != RemiCatalogResourceKind::ProfileRevision {
        bail!(
            "resource {} is {:?}, expected profile revision",
            resource.resource_sha256,
            resource.kind
        );
    }
    if !resource.durable {
        bail!(
            "profile revision resource {} is not durable",
            resource.resource_sha256
        );
    }
    if conary_core::repository::supported_profiles::profile_by_public_id(&resource.source_profile)
        .is_none()
    {
        bail!(
            "profile revision {} carries unsupported source profile '{}'",
            resource.resource_sha256,
            resource.source_profile
        );
    }

    Ok(ProfileRevisionContext {
        source_profile: resource.source_profile,
        profile_revision_sha256: resource.resource_sha256,
    })
}

/// Validate every current conversion candidate, including its durable exact
/// profile-revision pin. A missing, stale, or mismatched pin is corruption and
/// must fail closed rather than silently dropping the row.
fn current_conversions(
    conn: &Connection,
    profile_revision_sha256: &str,
    package_name: &str,
) -> Result<Vec<ConvertedPackage>> {
    let conversions = ConvertedPackage::find_current_conversions(
        conn,
        profile_revision_sha256,
        Some(package_name),
    )?;
    for converted in &conversions {
        let id = converted.id.context("current conversion row has no ID")?;
        let pin = ConvertedPackage::require_conversion_pin(conn, id)?;
        if pin.profile_revision_sha256 != profile_revision_sha256 {
            bail!(
                "current conversion {id} carries pin revision {}, expected {profile_revision_sha256}",
                pin.profile_revision_sha256
            );
        }
    }
    Ok(conversions)
}

/// Get signed CCS object identities and sizes for a converted package version.
///
/// Both identities and sizes come from the persisted authenticated transport
/// descriptor; `chunk_access` is cache bookkeeping, never delta authority.
fn get_version_chunks(
    conn: &Connection,
    profile_revision_sha256: &str,
    package_name: &str,
    version: &str,
) -> Result<(BTreeMap<String, u64>, u64)> {
    match find_current_conversion(conn, profile_revision_sha256, package_name, version)? {
        Some(converted) => {
            let artifact = converted.repository_artifact()?;
            let objects = artifact
                .transport
                .objects
                .into_iter()
                .map(|object| (object.sha256, object.size))
                .collect::<BTreeMap<_, _>>();
            let total = objects.values().try_fold(0_u64, |total, size| {
                total
                    .checked_add(*size)
                    .context("delta object-size sum overflow")
            })?;
            Ok((objects, total))
        }
        None => Ok((BTreeMap::new(), 0)),
    }
}

fn find_current_conversion(
    conn: &Connection,
    profile_revision_sha256: &str,
    package_name: &str,
    version: &str,
) -> Result<Option<ConvertedPackage>> {
    for converted in current_conversions(conn, profile_revision_sha256, package_name)? {
        if converted.repository_artifact()?.package_version == version {
            converted.scriptlet_summary()?;
            return Ok(Some(converted));
        }
    }
    Ok(None)
}

fn current_conversion_versions(
    conn: &Connection,
    profile_revision_sha256: &str,
    package_name: &str,
) -> Result<HashSet<String>> {
    let mut versions = HashSet::new();
    for converted in current_conversions(conn, profile_revision_sha256, package_name)? {
        converted.scriptlet_summary()?;
        versions.insert(converted.repository_artifact()?.package_version.to_string());
    }
    Ok(versions)
}

fn calculate_chunk_delta(
    from_chunks: &BTreeMap<String, u64>,
    to_chunks: &BTreeMap<String, u64>,
) -> Result<(Vec<String>, Vec<String>, u64)> {
    let from_set: BTreeSet<&str> = from_chunks.keys().map(String::as_str).collect();
    let to_set: BTreeSet<&str> = to_chunks.keys().map(String::as_str).collect();

    let new_chunks: Vec<String> = to_set
        .difference(&from_set)
        .map(|s| (*s).to_string())
        .collect();
    let removed_chunks: Vec<String> = from_set
        .difference(&to_set)
        .map(|s| (*s).to_string())
        .collect();
    let download_size = new_chunks
        .iter()
        .map(|hash| {
            to_chunks
                .get(hash)
                .copied()
                .context("delta object lost signed size authority")
        })
        .sum::<Result<u64>>()?;

    Ok((new_chunks, removed_chunks, download_size))
}

/// Compute the delta manifest between two versions of a package.
///
/// Queries signed object references for both versions from `converted_packages`,
/// computes the set difference (new_chunks = in to_version but not from_version,
/// removed_chunks = in from_version but not to_version), calculates download_size
/// from descriptor-owned sizes, and inserts the result into `delta_manifests`.
pub fn compute_delta(
    conn: &Connection,
    profile_revision_sha256: &str,
    package_name: &str,
    from_version: &str,
    to_version: &str,
) -> Result<DeltaManifest> {
    let profile = profile_revision_context(conn, profile_revision_sha256)?;
    debug!(
        "Computing delta for {}/{} (revision {}): {} -> {}",
        profile.source_profile,
        package_name,
        profile.profile_revision_sha256,
        from_version,
        to_version
    );

    // Get chunks for both versions
    let (from_chunks, _from_size) = get_version_chunks(
        conn,
        &profile.profile_revision_sha256,
        package_name,
        from_version,
    )?;
    let (to_chunks, to_size) = get_version_chunks(
        conn,
        &profile.profile_revision_sha256,
        package_name,
        to_version,
    )?;
    let (new_chunks, removed_chunks, download_size) =
        calculate_chunk_delta(&from_chunks, &to_chunks)?;

    let new_chunks_json = serde_json::to_string(&new_chunks)?;
    let removed_chunks_json = serde_json::to_string(&removed_chunks)?;

    // Insert or replace into delta_manifests
    conn.execute(
        "INSERT OR REPLACE INTO delta_manifests
         (source_profile, package_name, from_version, to_version, new_chunks, removed_chunks, download_size, full_size)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            &profile.source_profile,
            package_name,
            from_version,
            to_version,
            &new_chunks_json,
            &removed_chunks_json,
            download_size as i64,
            to_size as i64,
        ],
    )?;

    let id = conn.last_insert_rowid();

    info!(
        "Delta computed for {}/{} (revision {}): {} -> {} ({} new, {} removed, {} bytes to download vs {} full)",
        profile.source_profile,
        package_name,
        profile.profile_revision_sha256,
        from_version,
        to_version,
        new_chunks.len(),
        removed_chunks.len(),
        download_size,
        to_size
    );

    Ok(DeltaManifest {
        id: Some(id),
        source_profile: profile.source_profile,
        package_name: package_name.to_string(),
        from_version: from_version.to_string(),
        to_version: to_version.to_string(),
        new_chunks,
        removed_chunks,
        download_size,
        full_size: to_size,
        computed_at: None,
    })
}

/// Compute deltas for all adjacent version pairs of a package.
///
/// Finds all converted versions, sorts them with scheme-aware comparison
/// (RPM/Debian/Arch), then computes deltas between each adjacent pair
/// (v1->v2, v2->v3, ...).
pub fn compute_deltas_for_package(
    conn: &Connection,
    profile_revision_sha256: &str,
    package_name: &str,
) -> Result<Vec<DeltaManifest>> {
    use conary_core::repository::supported_profiles::{SupportedProfile, profile_by_public_id};
    use conary_core::repository::versioning::{compare_repo_versions, validate_repo_version};

    let profile = profile_revision_context(conn, profile_revision_sha256)?;

    // Version ordering comes from the exact profile resource bound to this
    // revision, never from a caller-provided route label.
    let scheme = profile_by_public_id(&profile.source_profile)
        .map(SupportedProfile::version_scheme)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "unsupported public source profile '{}' for delta computation",
                profile.source_profile
            )
        })?;

    // Get all current converted versions for this package.
    let mut versions: Vec<String> =
        current_conversion_versions(conn, &profile.profile_revision_sha256, package_name)?
            .into_iter()
            .collect();

    // Validate before entering Rust's infallible sort callback. The callback
    // cannot observe an invalid version after this boundary.
    for version in &versions {
        validate_repo_version(scheme, version)?;
    }
    versions.sort_by(|a, b| {
        compare_repo_versions(scheme, a, b).expect("delta versions were validated before sorting")
    });

    if versions.len() < 2 {
        debug!(
            "Package {}/{} (revision {}) has {} versions, need at least 2 for deltas",
            profile.source_profile,
            profile.profile_revision_sha256,
            package_name,
            versions.len()
        );
        return Ok(Vec::new());
    }

    let mut deltas = Vec::new();
    for pair in versions.windows(2) {
        let from_version = &pair[0];
        let to_version = &pair[1];

        deltas.push(
            compute_delta(
                conn,
                &profile.profile_revision_sha256,
                package_name,
                from_version,
                to_version,
            )
            .with_context(|| {
                format!(
                    "compute adjacent delta {}/{}/{} {from_version} -> {to_version}",
                    profile.source_profile, profile.profile_revision_sha256, package_name
                )
            })?,
        );
    }

    info!(
        "Computed {} deltas for {}/{} (revision {})",
        deltas.len(),
        profile.source_profile,
        profile.profile_revision_sha256,
        package_name
    );
    Ok(deltas)
}

fn versions_have_current_conversions(
    conn: &Connection,
    profile_revision_sha256: &str,
    package_name: &str,
    from_version: &str,
    to_version: &str,
) -> Result<bool> {
    let versions = current_conversion_versions(conn, profile_revision_sha256, package_name)?;
    Ok(versions.contains(from_version) && versions.contains(to_version))
}

/// Look up a pre-computed delta manifest.
pub fn get_delta(
    conn: &Connection,
    profile_revision_sha256: &str,
    package_name: &str,
    from_version: &str,
    to_version: &str,
) -> Result<Option<DeltaManifest>> {
    let profile = profile_revision_context(conn, profile_revision_sha256)?;
    let row = conn
        .query_row(
            "SELECT id, source_profile, package_name, from_version, to_version,
                    new_chunks, removed_chunks, download_size, full_size, computed_at
             FROM delta_manifests
             WHERE source_profile = ?1 AND package_name = ?2
               AND from_version = ?3 AND to_version = ?4",
            params![
                &profile.source_profile,
                package_name,
                from_version,
                to_version
            ],
            |row| {
                let new_chunks_json: String = row.get(5)?;
                let removed_chunks_json: String = row.get(6)?;

                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    new_chunks_json,
                    removed_chunks_json,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            },
        )
        .optional()?;

    match row {
        Some((
            id,
            source_profile,
            pkg,
            from_v,
            to_v,
            new_json,
            rem_json,
            dl_size,
            full_size,
            computed,
        )) => {
            if !versions_have_current_conversions(
                conn,
                &profile.profile_revision_sha256,
                &pkg,
                &from_v,
                &to_v,
            )? {
                return Ok(None);
            }

            let new_chunks: Vec<String> = serde_json::from_str(&new_json).with_context(|| {
                format!(
                    "delta manifest {id} for {source_profile}/{pkg} {from_v} -> {to_v} has corrupt new_chunks"
                )
            })?;
            let removed_chunks: Vec<String> =
                serde_json::from_str(&rem_json).with_context(|| {
                    format!(
                        "delta manifest {id} for {source_profile}/{pkg} {from_v} -> {to_v} has corrupt removed_chunks"
                    )
                })?;
            let download_size = u64::try_from(dl_size).with_context(|| {
                format!("delta manifest {id} has negative download_size {dl_size}")
            })?;
            let full_size = u64::try_from(full_size).with_context(|| {
                format!("delta manifest {id} has negative full_size {full_size}")
            })?;

            // The legacy cache key has no revision column. Revalidate its
            // complete signed object set and sizes against the exact current
            // revision before returning it, so activation cannot reuse a
            // delta computed from an older profile revision.
            let (from_chunks, _) =
                get_version_chunks(conn, &profile.profile_revision_sha256, &pkg, &from_v)?;
            let (to_chunks, expected_full_size) =
                get_version_chunks(conn, &profile.profile_revision_sha256, &pkg, &to_v)?;
            let (expected_new_chunks, expected_removed_chunks, expected_download_size) =
                calculate_chunk_delta(&from_chunks, &to_chunks)?;
            if new_chunks != expected_new_chunks
                || removed_chunks != expected_removed_chunks
                || download_size != expected_download_size
                || full_size != expected_full_size
            {
                return Ok(None);
            }

            Ok(Some(DeltaManifest {
                id: Some(id),
                source_profile,
                package_name: pkg,
                from_version: from_v,
                to_version: to_v,
                new_chunks,
                removed_chunks,
                download_size,
                full_size,
                computed_at: computed,
            }))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests;
