// apps/remi/src/server/handlers/packages/serving.rs

//! Exact immutable-revision lookup for converted package serving.

use super::{PackageManifest, PackageQuery, ScriptletPackageMetadata};
use crate::server::catalog_authority::CatalogAuthority;
use anyhow::Result;
use axum::http::StatusCode;
use std::path::Path;

/// Resolve the exact immutable profile revision selected by the active
/// catalog pointer. A request that cannot establish that authority must go
/// through the normal repository-readiness path; it must never fall back to a
/// source-profile-only conversion row.
pub(super) async fn active_profile_revision_for_request(
    catalog_authority: CatalogAuthority,
    distro: &str,
) -> Option<String> {
    let profile_id = conary_core::repository::supported_profiles::profile_for_remi_route(distro)?
        .id()
        .to_string();
    let lookup_profile_id = profile_id.clone();
    match tokio::task::spawn_blocking(move || {
        catalog_authority
            .open_active_profile(&lookup_profile_id)
            .map(|catalog| catalog.profile_revision_sha256().to_string())
    })
    .await
    {
        Ok(Ok(revision)) => Some(revision),
        Ok(Err(error)) => {
            tracing::debug!(profile = %profile_id, "active immutable profile unavailable: {error}");
            None
        }
        Err(error) => {
            tracing::debug!(profile = %profile_id, "active immutable profile task failed: {error}");
            None
        }
    }
}

pub(super) async fn converted_manifest_for_request(
    db_path: &Path,
    catalog_authority: &CatalogAuthority,
    distro: &str,
    name: &str,
    query: &PackageQuery,
) -> Result<ConvertedManifestLookup, (StatusCode, &'static str)> {
    let Some(profile_revision_sha256) =
        active_profile_revision_for_request(catalog_authority.clone(), distro).await
    else {
        return Ok(ConvertedManifestLookup::Missing);
    };
    let check_db = db_path.to_path_buf();
    let check_distro = distro.to_string();
    let check_name = name.to_string();
    let check_version = query.version.clone();
    let check_arch = query.arch.clone();
    match tokio::task::spawn_blocking(move || {
        check_converted(
            &check_db,
            &profile_revision_sha256,
            &check_distro,
            &check_name,
            check_version.as_deref(),
            check_arch.as_deref(),
        )
    })
    .await
    {
        Ok(Ok(lookup)) => Ok(lookup),
        Ok(Err(error)) => {
            tracing::error!("Database error checking conversion: {}", error);
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Database error"))
        }
        Err(error) => {
            tracing::error!("Blocking task failed: {}", error);
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Internal error"))
        }
    }
}

pub(super) enum ConvertedManifestLookup {
    Ready(Box<PackageManifest>),
    Missing,
}

/// Check whether an exact-revision conversion has a durable pin and a
/// readable CCS artifact, then build its public manifest.
pub(super) fn check_converted(
    db_path: &Path,
    profile_revision_sha256: &str,
    distro: &str,
    name: &str,
    version: Option<&str>,
    architecture: Option<&str>,
) -> Result<ConvertedManifestLookup, anyhow::Error> {
    use conary_core::db::models::ConvertedPackage;

    // Startup already validated the current schema, so this hot path can skip it.
    let conn = conary_core::db::open_fast(db_path)?;
    let existing = ConvertedPackage::find_by_package_identity_with_arch(
        &conn,
        profile_revision_sha256,
        name,
        version,
        architecture,
    )?;

    if let Some(converted) = existing {
        if !converted.repository_conversion_is_current_for_revision(profile_revision_sha256)? {
            return Ok(ConvertedManifestLookup::Missing);
        }
        let id = converted.id.ok_or_else(|| {
            anyhow::anyhow!("repository converted package has no database identity")
        })?;
        ConvertedPackage::require_conversion_pin(&conn, id)?;
        let artifact = converted.repository_artifact()?;
        let ccs_path = Path::new(artifact.ccs_path);
        if ccs_path.exists() {
            let scriptlet_summary = converted.scriptlet_summary()?;
            let manifest = PackageManifest {
                name: artifact.package_name.to_string(),
                version: artifact.package_version.to_string(),
                release: None,
                distro: distro.to_string(),
                transport: artifact.transport,
                total_size: artifact.total_size,
                content_hash: artifact.content_hash.to_string(),
                native: false,
                converted: true,
                source_kind: Some("converted".to_string()),
                scriptlets: Some(ScriptletPackageMetadata::from(&scriptlet_summary)),
            };

            return Ok(ConvertedManifestLookup::Ready(Box::new(manifest)));
        }
    }

    Ok(ConvertedManifestLookup::Missing)
}

pub(super) enum ConvertedDownloadLookup {
    Ready(std::path::PathBuf),
    Missing,
}

/// Resolve a downloadable CCS path only from a current exact-revision row
/// with a valid durable conversion pin.
pub(super) fn converted_ccs_path_for_download(
    db_path: &Path,
    profile_revision_sha256: &str,
    name: &str,
    version: Option<&str>,
    architecture: Option<&str>,
) -> Result<ConvertedDownloadLookup, anyhow::Error> {
    use conary_core::db::models::ConvertedPackage;

    let conn = conary_core::db::open_fast(db_path)?;
    let Some(converted) = ConvertedPackage::find_by_package_identity_with_arch(
        &conn,
        profile_revision_sha256,
        name,
        version,
        architecture,
    )?
    else {
        return Ok(ConvertedDownloadLookup::Missing);
    };

    if !converted.repository_conversion_is_current_for_revision(profile_revision_sha256)? {
        return Ok(ConvertedDownloadLookup::Missing);
    }
    let id = converted
        .id
        .ok_or_else(|| anyhow::anyhow!("repository converted package has no database identity"))?;
    ConvertedPackage::require_conversion_pin(&conn, id)?;
    converted.scriptlet_summary()?;
    let artifact = converted.repository_artifact()?;
    let ccs_path = std::path::PathBuf::from(artifact.ccs_path);
    if ccs_path.exists() {
        Ok(ConvertedDownloadLookup::Ready(ccs_path))
    } else {
        Ok(ConvertedDownloadLookup::Missing)
    }
}
