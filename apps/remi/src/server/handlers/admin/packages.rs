// apps/remi/src/server/handlers/admin/packages.rs
//! Admin handlers for publishing custom CCS packages into Remi metadata.

use super::{check_scope, validate_path_param};
use crate::server::ServerState;
use crate::server::auth::{Scope, TokenScopes, json_error};
use crate::server::publication::{ReviewArtifactInput, decision_refusal, write_review_artifact};
use axum::{
    extract::{Path, Query, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use conary_core::ccs::convert::ScriptletBundleSummary;
use conary_core::db::models::{CONVERSION_VERSION, ScriptletSummaryForPublication};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;

/// Maximum allowed upload size (512 MB).
const MAX_UPLOAD_SIZE: u64 = 512 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct ReviewArtifactQuery {
    pub version: String,
    pub arch: Option<String>,
}

#[derive(Debug)]
enum ReviewArtifactLookup {
    Found(String),
    Stale,
    Ambiguous,
    Missing,
}

#[derive(Debug)]
struct ReviewArtifactRow {
    conversion_version: i32,
    review_artifact_path: Option<String>,
}

#[derive(Serialize)]
struct PublishPackageResponse {
    distro: String,
    package: String,
    version: String,
    path: String,
    size: u64,
    content_hash: String,
}

fn safe_ccs_filename(name: &str, version: &str) -> String {
    let sanitize = |value: &str| {
        value
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>()
    };
    format!("{}-{}.ccs", sanitize(name), sanitize(version))
}

fn chunk_path(chunk_dir: &FsPath, hash: &str) -> PathBuf {
    crate::server::handlers::cas_object_path(chunk_dir, hash)
}

/// Atomically replace existing DB record with new one inside a single transaction.
///
/// Returns the old `ConvertedPackage` (if any) so the caller can clean up stale
/// files on disk *after* the transaction commits.
async fn atomic_replace_record(
    db_path: PathBuf,
    distro: String,
    package_name: String,
    package_version: String,
    package_architecture: Option<String>,
    content_hash: String,
    size: i64,
    final_ccs_path: String,
    scriptlet_summary: ScriptletBundleSummary,
) -> anyhow::Result<Option<conary_core::db::models::ConvertedPackage>> {
    tokio::task::spawn_blocking(move || {
        let mut conn = crate::server::open_runtime_db(&db_path)?;
        conary_core::db::transaction(&mut conn, |tx| {
            // Find existing record (if any) before deleting
            let existing =
                conary_core::db::models::ConvertedPackage::find_by_package_identity_with_arch(
                    tx,
                    &distro,
                    &package_name,
                    Some(&package_version),
                    package_architecture.as_deref(),
                )?;

            // Delete old record inside the transaction
            if let Some(ref existing) = existing {
                conary_core::db::models::ConvertedPackage::delete_by_checksum(
                    tx,
                    &existing.original_checksum,
                )?;
            }

            // Insert new record inside the same transaction
            let mut converted = conary_core::db::models::ConvertedPackage::new_server(
                distro.clone(),
                package_name.clone(),
                package_version.clone(),
                "ccs".to_string(),
                format!("upload:{}:{}", distro, content_hash),
                "full".to_string(),
                std::slice::from_ref(&content_hash),
                size,
                content_hash.clone(),
                final_ccs_path,
            );
            converted.package_architecture = package_architecture;
            converted.set_scriptlet_metadata(&scriptlet_summary)?;
            converted.insert(tx)?;

            Ok(existing)
        })
        .map_err(|e| anyhow::anyhow!("database transaction failed: {e}"))
    })
    .await
    .map_err(|e| anyhow::anyhow!("failed to join blocking db task: {e}"))?
}

fn matching_review_artifact_rows(
    conn: &rusqlite::Connection,
    distro: &str,
    package: &str,
    version: &str,
    architecture: Option<&str>,
) -> anyhow::Result<Vec<ReviewArtifactRow>> {
    let mut rows = Vec::new();
    if let Some(architecture) = architecture {
        let mut stmt = conn.prepare(
            "SELECT conversion_version, review_artifact_path
             FROM converted_packages
             WHERE distro = ?1 AND package_name = ?2 AND package_version = ?3
               AND package_architecture = ?4
             ORDER BY converted_at DESC",
        )?;
        let iter = stmt.query_map(
            rusqlite::params![distro, package, version, architecture],
            |row| {
                Ok(ReviewArtifactRow {
                    conversion_version: row.get(0)?,
                    review_artifact_path: row.get(1)?,
                })
            },
        )?;
        for row in iter {
            rows.push(row?);
        }
    } else {
        let mut stmt = conn.prepare(
            "SELECT conversion_version, review_artifact_path
             FROM converted_packages
             WHERE distro = ?1 AND package_name = ?2 AND package_version = ?3
             ORDER BY converted_at DESC",
        )?;
        let iter = stmt.query_map(rusqlite::params![distro, package, version], |row| {
            Ok(ReviewArtifactRow {
                conversion_version: row.get(0)?,
                review_artifact_path: row.get(1)?,
            })
        })?;
        for row in iter {
            rows.push(row?);
        }
    }
    Ok(rows)
}

fn classify_review_artifact_rows(
    rows: Vec<ReviewArtifactRow>,
    architecture: Option<&str>,
) -> ReviewArtifactLookup {
    if rows.is_empty() {
        return ReviewArtifactLookup::Missing;
    }

    let current: Vec<_> = rows
        .into_iter()
        .filter(|row| row.conversion_version >= CONVERSION_VERSION)
        .collect();
    if current.is_empty() {
        return ReviewArtifactLookup::Stale;
    }
    if architecture.is_none() && current.len() > 1 {
        return ReviewArtifactLookup::Ambiguous;
    }

    current
        .into_iter()
        .find_map(|row| row.review_artifact_path)
        .map(ReviewArtifactLookup::Found)
        .unwrap_or(ReviewArtifactLookup::Missing)
}

pub async fn get_scriptlet_review_artifact(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((distro, package)): Path<(String, String)>,
    Query(query): Query<ReviewArtifactQuery>,
    scopes: Option<axum::Extension<TokenScopes>>,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }
    if let Some(err) = validate_path_param(&distro, "distro") {
        return err;
    }
    if let Some(err) = validate_path_param(&package, "package") {
        return err;
    }
    if query.version.is_empty() {
        return json_error(400, "Version is required", "INVALID_PARAMETER");
    }

    let (db_path, cache_dir) = {
        let guard = state.read().await;
        (guard.config.db_path.clone(), guard.config.cache_dir.clone())
    };
    let lookup = tokio::task::spawn_blocking({
        let distro = distro.clone();
        let package = package.clone();
        let version = query.version.clone();
        let arch = query.arch.clone();
        move || -> anyhow::Result<ReviewArtifactLookup> {
            let conn = crate::server::open_runtime_db(&db_path)?;
            let rows =
                matching_review_artifact_rows(&conn, &distro, &package, &version, arch.as_deref())?;
            Ok(classify_review_artifact_rows(rows, arch.as_deref()))
        }
    })
    .await;

    let path = match lookup {
        Ok(Ok(ReviewArtifactLookup::Found(path))) => PathBuf::from(path),
        Ok(Ok(ReviewArtifactLookup::Stale)) => {
            return json_error(
                409,
                "Converted package needs reconversion",
                "STALE_CONVERSION",
            );
        }
        Ok(Ok(ReviewArtifactLookup::Ambiguous)) => {
            return json_error(
                409,
                "Architecture is required for this package/version",
                "AMBIGUOUS_ARCHITECTURE",
            );
        }
        Ok(Ok(ReviewArtifactLookup::Missing)) => {
            return json_error(404, "Review artifact not found", "NOT_FOUND");
        }
        Ok(Err(error)) => {
            tracing::error!("Failed to query review artifact row: {error}");
            return json_error(500, "Failed to query review artifact", "DB_ERROR");
        }
        Err(error) => {
            tracing::error!("Failed to join review artifact lookup task: {error}");
            return json_error(500, "Failed to query review artifact", "INTERNAL_ERROR");
        }
    };

    let metadata = match tokio::fs::metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return json_error(404, "Review artifact file not found on disk", "NOT_FOUND");
        }
        Err(error) => {
            tracing::warn!(
                "Failed to inspect review artifact {}: {}",
                path.display(),
                error
            );
            return json_error(500, "Failed to inspect review artifact", "IO_ERROR");
        }
    };
    if !metadata.is_file() {
        return json_error(403, "Review artifact path is invalid", "FORBIDDEN");
    }

    match crate::server::publication::validate_review_artifact_path(&cache_dir, &path) {
        Ok(true) => {}
        Ok(false) => {
            return json_error(403, "Review artifact path is invalid", "FORBIDDEN");
        }
        Err(error) => {
            tracing::warn!("Review artifact path validation failed: {error}");
            return json_error(403, "Review artifact path is invalid", "FORBIDDEN");
        }
    }

    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            bytes,
        )
            .into_response(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            json_error(404, "Review artifact file not found on disk", "NOT_FOUND")
        }
        Err(error) => {
            tracing::warn!(
                "Failed to read review artifact {}: {}",
                path.display(),
                error
            );
            json_error(500, "Failed to read review artifact", "IO_ERROR")
        }
    }
}

pub async fn upload_package(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(distro): Path<String>,
    scopes: Option<axum::Extension<TokenScopes>>,
    request: Request,
) -> Response {
    if let Some(err) = check_scope(&scopes, Scope::Admin) {
        return err;
    }
    if let Some(err) = validate_path_param(&distro, "distro") {
        return err;
    }

    let (cache_dir, chunk_dir, db_path) = {
        let guard = state.read().await;
        (
            guard.config.cache_dir.clone(),
            guard.config.chunk_dir.clone(),
            guard.config.db_path.clone(),
        )
    };

    let packages_dir = cache_dir.join("packages").join(&distro);
    if let Err(err) = tokio::fs::create_dir_all(&packages_dir).await {
        tracing::error!(
            "Failed to create package cache dir {}: {}",
            packages_dir.display(),
            err
        );
        return json_error(500, "Failed to create package cache directory", "IO_ERROR");
    }

    let temp_path = packages_dir.join(format!("upload-{}.ccs", uuid::Uuid::new_v4().simple()));
    let mut file = match tokio::fs::File::create(&temp_path).await {
        Ok(file) => file,
        Err(err) => {
            tracing::error!(
                "Failed to create temp package {}: {}",
                temp_path.display(),
                err
            );
            return json_error(500, "Failed to store package", "IO_ERROR");
        }
    };

    // Step 1: Stream upload body to temp file (no hashing during streaming)
    let mut size = 0u64;
    let mut stream = request.into_body().into_data_stream();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => {
                size += bytes.len() as u64;
                if size > MAX_UPLOAD_SIZE {
                    let _ = tokio::fs::remove_file(&temp_path).await;
                    return json_error(
                        413,
                        "Upload exceeds maximum size (512 MB)",
                        "PAYLOAD_TOO_LARGE",
                    );
                }
                if let Err(err) = file.write_all(&bytes).await {
                    tracing::error!(
                        "Failed writing package upload {}: {}",
                        temp_path.display(),
                        err
                    );
                    let _ = tokio::fs::remove_file(&temp_path).await;
                    return json_error(500, "Failed to store package", "IO_ERROR");
                }
            }
            Err(err) => {
                tracing::warn!("Failed reading package upload body: {}", err);
                let _ = tokio::fs::remove_file(&temp_path).await;
                return json_error(400, "Invalid upload body", "INVALID_BODY");
            }
        }
    }

    if size == 0 {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return json_error(400, "Package body must not be empty", "INVALID_BODY");
    }

    if let Err(err) = file.flush().await {
        tracing::error!(
            "Failed to flush temp package {}: {}",
            temp_path.display(),
            err
        );
        let _ = tokio::fs::remove_file(&temp_path).await;
        return json_error(500, "Failed to finalize package", "IO_ERROR");
    }
    drop(file);

    // Step 2: Hash the temp file using centralized conary_core::hash module
    let content_hash = match tokio::task::spawn_blocking({
        let temp_path = temp_path.clone();
        move || {
            let mut reader = std::fs::File::open(&temp_path)?;
            conary_core::hash::sha256_reader_hex(&mut reader)
        }
    })
    .await
    {
        Ok(Ok(hash)) => hash,
        Ok(Err(err)) => {
            tracing::error!("Failed to hash uploaded package: {}", err);
            let _ = tokio::fs::remove_file(&temp_path).await;
            return json_error(500, "Failed to hash package", "IO_ERROR");
        }
        Err(err) => {
            tracing::error!("Failed to join hash task: {}", err);
            let _ = tokio::fs::remove_file(&temp_path).await;
            return json_error(500, "Failed to hash package", "INTERNAL_ERROR");
        }
    };

    // Step 3: CCS inspection
    let inspected = match tokio::task::spawn_blocking({
        let temp_path = temp_path.clone();
        move || conary_core::ccs::inspector::InspectedPackage::from_file(&temp_path)
    })
    .await
    {
        Ok(Ok(pkg)) => pkg,
        Ok(Err(err)) => {
            tracing::warn!("Uploaded package is not a valid CCS archive: {}", err);
            let _ = tokio::fs::remove_file(&temp_path).await;
            return json_error(
                400,
                "Uploaded file is not a valid CCS package",
                "INVALID_CCS",
            );
        }
        Err(err) => {
            tracing::error!("Failed to inspect uploaded package: {}", err);
            let _ = tokio::fs::remove_file(&temp_path).await;
            return json_error(500, "Failed to inspect package", "INTERNAL_ERROR");
        }
    };

    let package_name = inspected.name().to_string();
    let package_version = inspected.version().to_string();
    let package_architecture = inspected
        .manifest
        .package
        .platform
        .as_ref()
        .and_then(|platform| platform.arch.as_deref())
        .map(str::to_string);
    let mut scriptlet_summary = match inspected.manifest.legacy_scriptlets.as_ref() {
        Some(bundle) => {
            if let Err(err) = bundle.validate() {
                tracing::warn!(
                    "Uploaded CCS package {}/{} has invalid legacy scriptlet bundle: {}",
                    package_name,
                    package_version,
                    err
                );
                let _ = tokio::fs::remove_file(&temp_path).await;
                return json_error(
                    400,
                    "Uploaded CCS package has invalid legacy scriptlet metadata",
                    "INVALID_SCRIPTLETS",
                );
            }
            ScriptletBundleSummary::from_bundle(bundle, bundle.evidence_digest.clone())
        }
        None => ScriptletBundleSummary::default(),
    };
    let ccs_filename = safe_ccs_filename(&package_name, &package_version);
    let final_ccs_path = packages_dir.join(&ccs_filename);

    // Step 4: Stage file to a .new suffix so the old file is untouched
    // until the DB transaction succeeds. This prevents the case where a
    // same-version re-upload overwrites the old content but the DB update
    // fails, leaving the old DB row pointing at new bytes.
    let staged_path = final_ccs_path.with_extension("ccs.new");
    if let Err(err) = tokio::fs::rename(&temp_path, &staged_path).await {
        tracing::error!(
            "Failed to stage uploaded package {}: {}",
            staged_path.display(),
            err
        );
        let _ = tokio::fs::remove_file(&temp_path).await;
        return json_error(500, "Failed to publish package", "IO_ERROR");
    }

    let mut review_artifact_path = None;
    let decision = crate::server::publication::classify_summary(ScriptletSummaryForPublication {
        summary: scriptlet_summary.clone(),
        valid: true,
    });
    if let Some(refusal) = decision_refusal(decision) {
        let mut report = match refusal {
            crate::server::publication::PublicationRefusal::ReviewRequired(report)
            | crate::server::publication::PublicationRefusal::Blocked(report) => report,
        };
        report.review_artifact_available = true;
        let artifact_path = match write_review_artifact(
            &cache_dir,
            ReviewArtifactInput {
                distro: &distro,
                package: &package_name,
                version: &package_version,
                architecture: package_architecture.as_deref(),
                original_format: "ccs",
                conversion_fidelity: "full",
                conversion_version: CONVERSION_VERSION,
                ccs_content_hash: &content_hash,
                ccs_total_size: size,
                publication: report,
            },
        ) {
            Ok(path) => path,
            Err(err) => {
                tracing::error!(
                    "Failed to write scriptlet review artifact for {}/{}/{}: {}",
                    distro,
                    package_name,
                    package_version,
                    err
                );
                let _ = tokio::fs::remove_file(&staged_path).await;
                return json_error(500, "Failed to write scriptlet review artifact", "IO_ERROR");
            }
        };
        scriptlet_summary.review_artifact_path = Some(artifact_path.to_string_lossy().to_string());
        review_artifact_path = Some(artifact_path);
    }

    // Step 5: Atomic DB transaction -- point at the STAGED path initially.
    // This way, if the final rename fails, the DB points at the staged file
    // which actually contains the correct new bytes (not the old content).
    let staged_path_str = staged_path.to_string_lossy().to_string();
    let existing = match atomic_replace_record(
        db_path.clone(),
        distro.clone(),
        package_name.clone(),
        package_version.clone(),
        package_architecture.clone(),
        content_hash.clone(),
        size as i64,
        staged_path_str.clone(),
        scriptlet_summary.clone(),
    )
    .await
    {
        Ok(existing) => existing,
        Err(err) => {
            tracing::error!(
                "Failed to update package metadata for {}/{}/{}: {}",
                distro,
                package_name,
                package_version,
                err
            );
            // DB failed -- remove the staged file, old file is untouched.
            let _ = tokio::fs::remove_file(&staged_path).await;
            if let Some(path) = review_artifact_path.as_ref() {
                let _ = tokio::fs::remove_file(path).await;
            }
            return json_error(500, "Failed to update package metadata", "DB_ERROR");
        }
    };

    // Step 6: DB succeeded (pointing at staged path). Try to rename to the
    // canonical location. Track `serving_path` -- the path the DB actually
    // points to -- and use it consistently for all subsequent operations.
    let serving_path: String;

    if let Err(err) = tokio::fs::rename(&staged_path, &final_ccs_path).await {
        // Rename failed. DB still points at staged_path which has correct content.
        tracing::warn!(
            "Rename to final path failed ({}), serving from staged path",
            err,
        );
        serving_path = staged_path_str.clone();
    } else {
        // Rename succeeded. Update DB to point at the canonical path.
        // If this UPDATE fails, rename back so the DB (staged path) is
        // consistent with the filesystem.
        let final_path_str = final_ccs_path.to_string_lossy().to_string();
        let db_path_for_update = db_path.clone();
        let distro_for_update = distro.clone();
        let name_for_update = package_name.clone();
        let version_for_update = package_version.clone();
        let arch_for_update = package_architecture.clone();
        let fp = final_path_str.clone();
        let update_ok = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let conn = crate::server::open_runtime_db(&db_path_for_update)?;
            if let Some(arch) = arch_for_update {
                conn.execute(
                    "UPDATE converted_packages SET ccs_path = ?1 \
                     WHERE distro = ?2 AND package_name = ?3 AND package_version = ?4 \
                       AND package_architecture = ?5",
                    rusqlite::params![
                        fp,
                        distro_for_update,
                        name_for_update,
                        version_for_update,
                        arch
                    ],
                )?;
            } else {
                conn.execute(
                    "UPDATE converted_packages SET ccs_path = ?1 \
                     WHERE distro = ?2 AND package_name = ?3 AND package_version = ?4 \
                       AND package_architecture IS NULL",
                    rusqlite::params![fp, distro_for_update, name_for_update, version_for_update],
                )?;
            }
            Ok(())
        })
        .await;

        if update_ok.is_ok() && update_ok.unwrap().is_ok() {
            serving_path = final_path_str;
        } else {
            // UPDATE failed -- try to rename back so DB (staged path) stays valid.
            if tokio::fs::rename(&final_ccs_path, &staged_path)
                .await
                .is_ok()
            {
                // Rename-back succeeded: file is at staged_path, DB points there.
                tracing::warn!("DB path update failed; reverted rename, serving from staged path");
                serving_path = staged_path_str.clone();
            } else {
                // Rename-back also failed: file is at final_ccs_path, DB points
                // at staged_path (which no longer exists). Force-update DB to
                // final_ccs_path as a last resort.
                tracing::error!(
                    "DB update failed AND rename-back failed; forcing DB to final path"
                );
                let final_str = final_ccs_path.to_string_lossy().to_string();
                let db2 = db_path.clone();
                let d2 = distro.clone();
                let n2 = package_name.clone();
                let v2 = package_version.clone();
                let a2 = package_architecture.clone();
                let fs2 = final_str.clone();
                let repair_ok = tokio::task::spawn_blocking(move || -> bool {
                    let Ok(conn) = crate::server::open_runtime_db(&db2) else {
                        return false;
                    };
                    if let Some(arch) = a2 {
                        conn.execute(
                            "UPDATE converted_packages SET ccs_path = ?1 \
                             WHERE distro = ?2 AND package_name = ?3 AND package_version = ?4 \
                               AND package_architecture = ?5",
                            rusqlite::params![fs2, d2, n2, v2, arch],
                        )
                        .is_ok()
                    } else {
                        conn.execute(
                            "UPDATE converted_packages SET ccs_path = ?1 \
                             WHERE distro = ?2 AND package_name = ?3 AND package_version = ?4 \
                               AND package_architecture IS NULL",
                            rusqlite::params![fs2, d2, n2, v2],
                        )
                        .is_ok()
                    }
                })
                .await
                .unwrap_or(false);

                if repair_ok {
                    serving_path = final_str;
                } else {
                    // All three attempts failed: DB points at vanished staged
                    // path, we cannot fix it. Return 500 rather than lying.
                    tracing::error!(
                        "All DB repair attempts failed for {}/{}/{}; row is inconsistent",
                        distro,
                        package_name,
                        package_version
                    );
                    return json_error(
                        500,
                        "Package uploaded but metadata repair failed; re-upload to fix",
                        "DB_REPAIR_FAILED",
                    );
                }
            }
        }
    }

    // Clean up the old file if it had a different path than what we're serving.
    if let Some(existing) = &existing
        && let Some(path) = &existing.ccs_path
        && *path != serving_path
    {
        let _ = tokio::fs::remove_file(path).await;
    }

    // Populate chunk store from the actual serving path.
    let chunk_file_path = chunk_path(&chunk_dir, &content_hash);
    if let Some(parent) = chunk_file_path.parent()
        && let Err(err) = tokio::fs::create_dir_all(parent).await
    {
        tracing::error!("Failed to create chunk dir {}: {}", parent.display(), err);
        return json_error(500, "Failed to create chunk storage", "IO_ERROR");
    }
    if tokio::fs::try_exists(&chunk_file_path).await.ok() != Some(true)
        && let Err(err) = tokio::fs::copy(&serving_path, &chunk_file_path).await
    {
        tracing::error!(
            "Failed to copy package into chunk store {}: {}",
            chunk_file_path.display(),
            err
        );
        return json_error(500, "Failed to store package chunk", "IO_ERROR");
    }

    (
        StatusCode::CREATED,
        axum::Json(PublishPackageResponse {
            distro,
            package: package_name,
            version: package_version,
            path: serving_path,
            size,
            content_hash,
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use crate::server::handlers::admin::test_helpers::{rebuild_app, test_app};
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, StatusCode};
    use conary_core::ccs::convert::ScriptletBundleSummary;
    use conary_core::db::models::{CONVERSION_VERSION, ConvertedPackage};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::Builder;
    use tower::ServiceExt;

    fn minimal_ccs(name: &str, version: &str) -> Vec<u8> {
        let manifest = conary_core::ccs::manifest::CcsManifest::new_minimal(name, version);
        let manifest_toml = toml::to_string(&manifest).expect("serialize manifest");
        let component = serde_json::json!({
            "name": "runtime",
            "files": [],
            "hash": "empty",
            "size": 0
        })
        .to_string();

        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut tar = Builder::new(encoder);

        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(manifest_toml.len() as u64);
        header.set_cksum();
        tar.append_data(&mut header, "MANIFEST.toml", manifest_toml.as_bytes())
            .expect("write manifest");

        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(component.len() as u64);
        header.set_cksum();
        tar.append_data(&mut header, "components/runtime.json", component.as_bytes())
            .expect("write component");

        tar.into_inner()
            .expect("finish tar")
            .finish()
            .expect("finish gzip")
    }

    async fn assert_status(response: axum::response::Response, expected: StatusCode) {
        let status = response.status();
        if status != expected {
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            panic!(
                "unexpected status {status}, expected {expected}; body: {}",
                String::from_utf8_lossy(&body)
            );
        }
    }

    #[tokio::test]
    async fn test_upload_package_registers_converted_record() {
        let (app, db_path) = test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/admin/packages/fedora")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(Body::from(minimal_ccs("fixture-demo", "1.0.0")))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_status(response, StatusCode::CREATED).await;

        let conn = conary_core::db::open(&db_path).unwrap();
        let found = conary_core::db::models::ConvertedPackage::find_by_package_identity(
            &conn,
            "fedora",
            "fixture-demo",
            Some("1.0.0"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(found.original_format, "ccs");
        assert_eq!(
            found.total_size,
            Some(minimal_ccs("fixture-demo", "1.0.0").len() as i64)
        );
    }

    #[tokio::test]
    async fn test_upload_package_allows_same_fixture_for_multiple_distros() {
        let (app, db_path) = test_app().await;
        let body = minimal_ccs("fixture-demo", "1.0.0");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/admin/packages/fedora")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(Body::from(body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_status(response, StatusCode::CREATED).await;

        let app2 = rebuild_app(&db_path);
        let response = app2
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/admin/packages/ubuntu")
                    .header("Authorization", "Bearer test-admin-token-12345")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_status(response, StatusCode::CREATED).await;

        let conn = conary_core::db::open(&db_path).unwrap();
        assert!(
            conary_core::db::models::ConvertedPackage::find_by_package_identity(
                &conn,
                "fedora",
                "fixture-demo",
                Some("1.0.0"),
            )
            .unwrap()
            .is_some()
        );
        assert!(
            conary_core::db::models::ConvertedPackage::find_by_package_identity(
                &conn,
                "ubuntu",
                "fixture-demo",
                Some("1.0.0"),
            )
            .unwrap()
            .is_some()
        );
    }

    #[tokio::test]
    async fn test_upload_package_rejects_unauthenticated() {
        let (app, _db_path) = test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/admin/packages/fedora")
                    .body(Body::from(minimal_ccs("fixture-demo", "1.0.0")))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn admin_review_artifact_requires_admin_scope() {
        let (app, _db_path) = test_app().await;

        let response = tower::ServiceExt::oneshot(
            app,
            Request::builder()
                .uri("/v1/admin/packages/fedora/pkg/scriptlet-review?version=1.0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn admin_review_artifact_rejects_paths_outside_review_root() {
        let (app, db_path) = test_app().await;
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            "pkg".to_string(),
            "1.0".to_string(),
            "rpm".to_string(),
            "sha256:source".to_string(),
            "high".to_string(),
            &["abc".to_string()],
            3,
            "sha256:content".to_string(),
            "/tmp/pkg.ccs".to_string(),
        );
        let mut summary = ScriptletBundleSummary {
            publication_status: "private-review".to_string(),
            scriptlet_fidelity: "review-required".to_string(),
            target_compatibility: "review-required".to_string(),
            review_reason_codes: vec!["review-class-debconf".to_string()],
            ..Default::default()
        };
        summary.review_artifact_path = Some("/etc/passwd".to_string());
        converted.set_scriptlet_metadata(&summary).unwrap();
        converted.insert(&conn).unwrap();

        let response = tower::ServiceExt::oneshot(
            app,
            Request::builder()
                .uri("/v1/admin/packages/fedora/pkg/scriptlet-review?version=1.0")
                .header("Authorization", "Bearer test-admin-token-12345")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn admin_upload_with_blocked_bundle_stores_non_public_metadata() {
        let (app, db_path) = test_app().await;
        let archive = blocked_scriptlet_ccs_fixture();

        let response = tower::ServiceExt::oneshot(
            app,
            Request::builder()
                .method(Method::POST)
                .uri("/v1/admin/packages/fedora")
                .header("Authorization", "Bearer test-admin-token-12345")
                .body(Body::from(archive))
                .unwrap(),
        )
        .await
        .unwrap();

        assert_status(response, StatusCode::CREATED).await;

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let converted = ConvertedPackage::find_by_package_identity(
            &conn,
            "fedora",
            "blocked-scriptlet-fixture",
            Some("1.0"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(converted.publication_status, "blocked");
        assert!(converted.review_artifact_path.is_some());
        let expected_digest = conary_core::hash::sha256_prefixed(b"fixture-evidence");
        assert_eq!(
            converted.evidence_digest.as_deref(),
            Some(expected_digest.as_str())
        );
        let artifact_path = std::path::PathBuf::from(converted.review_artifact_path.unwrap());
        let artifact: serde_json::Value =
            serde_json::from_slice(&std::fs::read(artifact_path).unwrap()).unwrap();
        assert_eq!(artifact["schema"], "conary.remi.scriptlet-review.v1");
        assert_eq!(
            artifact["publication"]["evidence_digest"].as_str(),
            Some(expected_digest.as_str())
        );
        assert!(
            !serde_json::to_string(&artifact)
                .unwrap()
                .contains("review_artifact_path")
        );
    }

    #[tokio::test]
    async fn admin_review_artifact_lookup_is_arch_specific_and_reports_stale_rows() {
        let (app, db_path) = test_app().await;
        seed_review_artifact_row(
            &db_path,
            "pkg",
            "1.0",
            Some("x86_64"),
            "current.json",
            false,
        );
        seed_review_artifact_row(&db_path, "pkg", "1.0", Some("aarch64"), "stale.json", true);

        let current = tower::ServiceExt::oneshot(
            app.clone(),
            Request::builder()
                .uri("/v1/admin/packages/fedora/pkg/scriptlet-review?version=1.0&arch=x86_64")
                .header("Authorization", "Bearer test-admin-token-12345")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(current.status(), StatusCode::OK);

        let stale = tower::ServiceExt::oneshot(
            app,
            Request::builder()
                .uri("/v1/admin/packages/fedora/pkg/scriptlet-review?version=1.0&arch=aarch64")
                .header("Authorization", "Bearer test-admin-token-12345")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(stale.status(), StatusCode::CONFLICT);
    }

    fn blocked_scriptlet_ccs_fixture() -> Vec<u8> {
        use conary_core::ccs::builder::{CcsBuilder, write_ccs_package};
        use conary_core::ccs::legacy_scriptlets::{
            DecisionCounts, ForeignReplayPolicy, LegacyScriptletBundle, PublicationPolicy,
            PublicationStatus, ScriptletFidelity, SourceFormat, TargetCompatibility, VersionScheme,
        };

        let temp = tempfile::tempdir().unwrap();
        let mut manifest = conary_core::ccs::manifest::CcsManifest::new_minimal(
            "blocked-scriptlet-fixture",
            "1.0",
        );
        manifest.legacy_scriptlets = Some(LegacyScriptletBundle {
            schema: conary_core::ccs::legacy_scriptlets::LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
            schema_revision: 1,
            source_format: SourceFormat::Rpm,
            source_family: "rpm".to_string(),
            source_distro: Some("fedora".to_string()),
            source_release: None,
            source_arch: Some("x86_64".to_string()),
            source_package: "blocked-scriptlet-fixture".to_string(),
            source_version: "1.0".to_string(),
            source_checksum: Some(conary_core::hash::sha256_prefixed(b"fixture-source")),
            version_scheme: VersionScheme::Rpm,
            conversion_tool: "test".to_string(),
            conversion_tool_version: "test".to_string(),
            conversion_policy: "publication-gate-test".to_string(),
            adapter_registry_digest: None,
            target_policy_digest: None,
            evidence_digest: Some(conary_core::hash::sha256_prefixed(b"fixture-evidence")),
            target_compatibility: TargetCompatibility::Blocked,
            allowed_targets: Vec::new(),
            foreign_replay_policy: ForeignReplayPolicy::Deny,
            publication_policy: PublicationPolicy::Blocked,
            publication_status: PublicationStatus::Blocked,
            scriptlet_fidelity: ScriptletFidelity::Blocked,
            decision_counts: DecisionCounts::default(),
            unsupported_class_counts: std::collections::BTreeMap::new(),
            entries: Vec::new(),
            extra: std::collections::BTreeMap::new(),
        });

        std::fs::write(temp.path().join("payload.txt"), b"fixture").unwrap();
        let path = temp.path().join("blocked.ccs");
        let result = CcsBuilder::new(manifest, temp.path())
            .build()
            .expect("fixture build");
        write_ccs_package(&result, &path).expect("fixture CCS package");
        std::fs::read(path).expect("fixture bytes")
    }

    fn seed_review_artifact_row(
        db_path: &std::path::Path,
        package: &str,
        version: &str,
        architecture: Option<&str>,
        artifact_name: &str,
        stale: bool,
    ) {
        let cache_dir = db_path.parent().unwrap().join("cache");
        let artifact_dir = crate::server::publication::review_artifact_root(&cache_dir)
            .join("fedora")
            .join(package)
            .join(version)
            .join(architecture.unwrap_or("noarch"));
        std::fs::create_dir_all(&artifact_dir).unwrap();
        let artifact_path = artifact_dir.join(artifact_name);
        std::fs::write(
            &artifact_path,
            serde_json::json!({
                "schema": "conary.remi.scriptlet-review.v1",
                "package": package,
                "version": version,
                "architecture": architecture,
            })
            .to_string(),
        )
        .unwrap();

        let conn = rusqlite::Connection::open(db_path).unwrap();
        let mut converted = ConvertedPackage::new_server(
            "fedora".to_string(),
            package.to_string(),
            version.to_string(),
            "rpm".to_string(),
            format!("sha256:source-{package}-{version}-{artifact_name}"),
            "high".to_string(),
            &["abc".to_string()],
            3,
            format!("sha256:content-{package}-{version}-{artifact_name}"),
            format!("/tmp/{package}-{version}-{artifact_name}.ccs"),
        );
        converted.package_architecture = architecture.map(str::to_string);
        if stale {
            converted.conversion_version = CONVERSION_VERSION - 1;
        }
        let mut summary = ScriptletBundleSummary {
            publication_status: "private-review".to_string(),
            scriptlet_fidelity: "review-required".to_string(),
            target_compatibility: "review-required".to_string(),
            review_reason_codes: vec!["review-class-debconf".to_string()],
            ..Default::default()
        };
        summary.review_artifact_path = Some(artifact_path.to_string_lossy().to_string());
        converted.set_scriptlet_metadata(&summary).unwrap();
        converted.insert(&conn).unwrap();
    }
}
