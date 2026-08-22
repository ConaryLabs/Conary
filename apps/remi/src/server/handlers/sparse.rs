// apps/remi/src/server/handlers/sparse.rs
//! Read-only per-package browsing documents.

use crate::server::ServerState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use conary_core::db::models::{NativePackagePublication, normalize_native_architecture};
use conary_core::repository::remi_metadata::REMI_SPARSE_MIN_PACKAGE_SIZE;
pub use conary_core::repository::remi_metadata::{
    RemiSparseIndexEntry as SparseIndexEntry, RemiSparseVersionEntry as SparseVersionEntry,
};
use rusqlite::Connection;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::server::catalog_authority::CatalogAuthority;
use crate::server::profile_catalog::ProfileCatalog;

/// GET /v1/index/{distro}/{name}
///
/// Returns a sparse index entry for a single package, including all versions
/// and their conversion status. Designed to be CDN-cacheable.
///
/// When federation is enabled, merges entries from upstream Remi peers.
pub async fn get_sparse_entry(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, name)): Path<(String, String)>,
) -> Response {
    if let Err(e) = super::validate_distro_and_name(&distro, &name) {
        return e;
    }

    let state_guard = state.read().await;
    let db_path = state_guard.config.db_path.clone();
    let catalog_authority = state_guard.catalog_authority.clone();
    let fed_config = state_guard.federated_config.clone();
    let fed_cache = state_guard.federated_cache.clone();
    let http_client = state_guard.http_client.clone();
    drop(state_guard);

    // Use federated builder if federation is enabled
    if let (Some(config), Some(cache)) = (fed_config, fed_cache) {
        let result = crate::server::federated_index::build_federated_sparse_entry(
            catalog_authority,
            &db_path,
            &distro,
            &name,
            &config,
            &cache,
            &http_client,
        )
        .await;

        return match result {
            Ok(Some(entry)) => {
                let json = match super::serialize_json(&entry, "federated sparse entry") {
                    Ok(j) => j,
                    Err(e) => return e,
                };
                super::json_response(json, 60)
            }
            Ok(None) => (StatusCode::NOT_FOUND, "Package not found").into_response(),
            Err(e) => {
                tracing::error!("Failed to build federated sparse entry: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
            }
        };
    }

    // Non-federated path: local only
    let result = tokio::task::spawn_blocking(move || {
        build_sparse_entry(&catalog_authority, &db_path, &distro, &name)
    })
    .await;

    match result {
        Ok(Ok(Some(entry))) => {
            let json = match super::serialize_json(&entry, "sparse index entry") {
                Ok(j) => j,
                Err(e) => return e,
            };
            super::json_response(json, 60)
        }
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, "Package not found").into_response(),
        Ok(Err(e)) => {
            tracing::error!("Failed to build sparse entry: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
        Err(e) => {
            tracing::error!("Blocking task failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}

/// Build a sparse index entry for a specific package, aggregating across all
/// repos for the distro (e.g. arch-core + arch-extra).
pub(super) fn build_sparse_entry(
    catalog_authority: &CatalogAuthority,
    db_path: &std::path::Path,
    distro: &str,
    name: &str,
) -> Result<Option<SparseIndexEntry>, anyhow::Error> {
    build_sparse_entry_with_revision(catalog_authority, db_path, distro, name)
        .map(|(_, entry)| entry)
}

pub(crate) fn build_sparse_entry_with_revision(
    catalog_authority: &CatalogAuthority,
    db_path: &std::path::Path,
    distro: &str,
    name: &str,
) -> Result<(String, Option<SparseIndexEntry>), anyhow::Error> {
    let conn = Connection::open(db_path)?;
    let source_profile =
        conary_core::repository::supported_profiles::profile_for_remi_route(distro)
            .ok_or_else(|| anyhow::anyhow!("unsupported public route '{distro}'"))?;

    let pinned = catalog_authority.open_active_profile(source_profile.id())?;
    let catalog = ProfileCatalog::new(&pinned);
    let revision_sha256 = catalog.profile_revision_sha256().to_string();
    let minimum_size = u64::try_from(REMI_SPARSE_MIN_PACKAGE_SIZE)
        .map_err(|_| anyhow::anyhow!("sparse minimum package size is negative"))?;
    let packages = catalog.find_downloadable_package_records_by_name(name, minimum_size)?;
    let mut publications =
        NativePackagePublication::find_active(&conn, source_profile.id(), name, None, None, None)?;
    validate_unique_publications(&publications)?;
    if packages.is_empty() && publications.is_empty() {
        return Ok((revision_sha256, None));
    }

    let mut versions = Vec::with_capacity(packages.len() + publications.len());
    for package in packages {
        let publication_index = publications.iter().position(|publication| {
            publication.version == package.version
                && publication.package_release == package.package_release
                && normalize_native_architecture(Some(&publication.architecture))
                    == normalize_native_architecture(package.architecture.as_deref())
        });
        let content_hash = if let Some(index) = publication_index {
            let publication = publications.remove(index);
            if publication.content_hash != package.checksum {
                anyhow::bail!(
                    "public native publication content hash '{}' disagrees with immutable \
                     catalog package '{}' checksum '{}'",
                    publication.content_hash,
                    package.package_key_sha256,
                    package.checksum
                );
            }
            Some(publication.content_hash)
        } else {
            None
        };
        let projected = catalog.project_package(&package)?;
        versions.push(SparseVersionEntry {
            version: projected.version,
            release: projected.release,
            provides: projected.provides,
            requirement_groups: projected.requirement_groups,
            architecture: projected.architecture,
            size: projected.size,
            converted: false,
            content_hash,
        });
    }
    for publication in publications {
        if publication.total_size < REMI_SPARSE_MIN_PACKAGE_SIZE {
            anyhow::bail!(
                "public native publication {}-{}.{} has invalid size {}",
                publication.version,
                publication.package_release,
                publication.architecture,
                publication.total_size
            );
        }
        versions.push(SparseVersionEntry {
            version: publication.version,
            release: (!publication.package_release.is_empty())
                .then_some(publication.package_release),
            provides: Vec::new(),
            requirement_groups: Vec::new(),
            architecture: Some(normalize_native_architecture(Some(
                &publication.architecture,
            ))),
            size: publication.total_size,
            converted: false,
            content_hash: Some(publication.content_hash),
        });
    }
    sort_sparse_versions(&mut versions);

    Ok((
        revision_sha256,
        Some(SparseIndexEntry {
            name: name.to_string(),
            distro: distro.to_string(),
            versions,
        }),
    ))
}

fn validate_unique_publications(
    publications: &[NativePackagePublication],
) -> Result<(), anyhow::Error> {
    let mut identities = std::collections::BTreeSet::new();
    for publication in publications {
        let identity = (
            &publication.version,
            &publication.package_release,
            normalize_native_architecture(Some(&publication.architecture)),
        );
        if !identities.insert(identity) {
            anyhow::bail!(
                "multiple public native publications share package identity {}-{}.{}",
                publication.version,
                publication.package_release,
                publication.architecture
            );
        }
    }
    Ok(())
}

fn sort_sparse_versions(versions: &mut [SparseVersionEntry]) {
    versions.sort_by(|left, right| {
        (
            &left.version,
            &left.release,
            &left.architecture,
            &left.content_hash,
        )
            .cmp(&(
                &right.version,
                &right.release,
                &right.architecture,
                &right.content_hash,
            ))
    });
}

#[cfg(test)]
mod tests;
