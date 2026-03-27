// conary-server/src/server/handlers/admin/packages.rs
//! Admin handlers for publishing custom CCS packages into Remi metadata.

use super::{check_scope, validate_path_param};
use crate::server::ServerState;
use crate::server::auth::{Scope, TokenScopes, json_error};
use axum::{
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use serde::Serialize;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;

/// Maximum allowed upload size (512 MB).
const MAX_UPLOAD_SIZE: u64 = 512 * 1024 * 1024;

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
    content_hash: String,
    size: i64,
    final_ccs_path: String,
) -> anyhow::Result<Option<conary_core::db::models::ConvertedPackage>> {
    tokio::task::spawn_blocking(move || {
        let mut conn = conary_core::db::open(&db_path)?;
        conary_core::db::transaction(&mut conn, |tx| {
            // Find existing record (if any) before deleting
            let existing = conary_core::db::models::ConvertedPackage::find_by_package_identity(
                tx,
                &distro,
                &package_name,
                Some(&package_version),
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
            converted.insert(tx)?;

            Ok(existing)
        })
        .map_err(|e| anyhow::anyhow!("database transaction failed: {e}"))
    })
    .await
    .map_err(|e| anyhow::anyhow!("failed to join blocking db task: {e}"))?
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

    // Step 5: Atomic DB transaction -- point at the STAGED path initially.
    // This way, if the final rename fails, the DB points at the staged file
    // which actually contains the correct new bytes (not the old content).
    let staged_path_str = staged_path.to_string_lossy().to_string();
    let existing = match atomic_replace_record(
        db_path.clone(),
        distro.clone(),
        package_name.clone(),
        package_version.clone(),
        content_hash.clone(),
        size as i64,
        staged_path_str.clone(),
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
        let fp = final_path_str.clone();
        let update_ok = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let conn = conary_core::db::open(&db_path_for_update)?;
            conn.execute(
                "UPDATE converted_packages SET ccs_path = ?1 \
                 WHERE distro = ?2 AND package_name = ?3 AND package_version = ?4",
                rusqlite::params![fp, distro_for_update, name_for_update, version_for_update],
            )?;
            Ok(())
        })
        .await;

        if update_ok.is_ok() && update_ok.unwrap().is_ok() {
            serving_path = final_path_str;
        } else {
            // UPDATE failed -- try to rename back so DB (staged path) stays valid.
            if tokio::fs::rename(&final_ccs_path, &staged_path).await.is_ok() {
                // Rename-back succeeded: file is at staged_path, DB points there.
                tracing::warn!("DB path update failed; reverted rename, serving from staged path");
                serving_path = staged_path_str.clone();
            } else {
                // Rename-back also failed: file is at final_ccs_path, DB points
                // at staged_path (which no longer exists). Force-update DB to
                // final_ccs_path as a last resort.
                tracing::error!("DB update failed AND rename-back failed; forcing DB to final path");
                let final_str = final_ccs_path.to_string_lossy().to_string();
                let db2 = db_path.clone();
                let d2 = distro.clone();
                let n2 = package_name.clone();
                let v2 = package_version.clone();
                let fs2 = final_str.clone();
                let repair_ok = tokio::task::spawn_blocking(move || -> bool {
                    let Ok(conn) = conary_core::db::open(&db2) else { return false };
                    conn.execute(
                        "UPDATE converted_packages SET ccs_path = ?1 \
                         WHERE distro = ?2 AND package_name = ?3 AND package_version = ?4",
                        rusqlite::params![fs2, d2, n2, v2],
                    ).is_ok()
                }).await.unwrap_or(false);

                if repair_ok {
                    serving_path = final_str;
                } else {
                    // All three attempts failed: DB points at vanished staged
                    // path, we cannot fix it. Return 500 rather than lying.
                    tracing::error!(
                        "All DB repair attempts failed for {}/{}/{}; row is inconsistent",
                        distro, package_name, package_version
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
    use crate::server::handlers::admin::test_helpers::test_app;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
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

        assert_eq!(response.status(), StatusCode::CREATED);

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
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = app
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
        assert_eq!(response.status(), StatusCode::CREATED);

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
}
