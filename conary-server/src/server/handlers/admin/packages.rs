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

async fn remove_existing_record(
    db_path: PathBuf,
    distro: String,
    package: String,
    version: String,
) -> anyhow::Result<Option<conary_core::db::models::ConvertedPackage>> {
    tokio::task::spawn_blocking(move || {
        let conn = conary_core::db::open(&db_path)?;
        if let Some(existing) = conary_core::db::models::ConvertedPackage::find_by_package_identity(
            &conn,
            &distro,
            &package,
            Some(&version),
        )? {
            conary_core::db::models::ConvertedPackage::delete_by_checksum(
                &conn,
                &existing.original_checksum,
            )?;
            Ok(Some(existing))
        } else {
            Ok(None)
        }
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

    let packages_dir = cache_dir.join("packages");
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

    use sha2::Digest as _;
    let mut size = 0u64;
    let mut hasher = sha2::Sha256::new();
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
                hasher.update(&bytes);
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
    let content_hash = format!("{:x}", hasher.finalize());
    let ccs_filename = safe_ccs_filename(&package_name, &package_version);
    let final_ccs_path = packages_dir.join(&ccs_filename);

    let existing = match remove_existing_record(
        db_path.clone(),
        distro.clone(),
        package_name.clone(),
        package_version.clone(),
    )
    .await
    {
        Ok(existing) => existing,
        Err(err) => {
            tracing::error!(
                "Failed to clear existing converted record for {}/{}/{}: {}",
                distro,
                package_name,
                package_version,
                err
            );
            let _ = tokio::fs::remove_file(&temp_path).await;
            return json_error(500, "Failed to update package metadata", "DB_ERROR");
        }
    };

    if let Some(existing) = &existing
        && let Some(path) = &existing.ccs_path
        && path != &final_ccs_path.to_string_lossy()
    {
        let _ = tokio::fs::remove_file(path).await;
    }

    if let Err(err) = tokio::fs::rename(&temp_path, &final_ccs_path).await {
        tracing::error!(
            "Failed to move uploaded package {} into place: {}",
            final_ccs_path.display(),
            err
        );
        let _ = tokio::fs::remove_file(&temp_path).await;
        return json_error(500, "Failed to publish package", "IO_ERROR");
    }

    let chunk_file_path = chunk_path(&chunk_dir, &content_hash);
    if let Some(parent) = chunk_file_path.parent()
        && let Err(err) = tokio::fs::create_dir_all(parent).await
    {
        tracing::error!("Failed to create chunk dir {}: {}", parent.display(), err);
        return json_error(500, "Failed to create chunk storage", "IO_ERROR");
    }
    if tokio::fs::try_exists(&chunk_file_path).await.ok() != Some(true)
        && let Err(err) = tokio::fs::copy(&final_ccs_path, &chunk_file_path).await
    {
        tracing::error!(
            "Failed to copy uploaded package into chunk store {}: {}",
            chunk_file_path.display(),
            err
        );
        return json_error(500, "Failed to store package chunk", "IO_ERROR");
    }

    let store_result = tokio::task::spawn_blocking({
        let db_path = db_path.clone();
        let distro = distro.clone();
        let package_name = package_name.clone();
        let package_version = package_version.clone();
        let content_hash = content_hash.clone();
        let final_ccs_path = final_ccs_path.clone();
        move || -> anyhow::Result<()> {
            let conn = conary_core::db::open(&db_path)?;
            let mut converted = conary_core::db::models::ConvertedPackage::new_server(
                distro.clone(),
                package_name.clone(),
                package_version.clone(),
                "ccs".to_string(),
                format!("upload:{}:{}", distro, content_hash),
                "full".to_string(),
                std::slice::from_ref(&content_hash),
                size as i64,
                content_hash.clone(),
                final_ccs_path.to_string_lossy().to_string(),
            );
            converted.insert(&conn)?;
            Ok(())
        }
    })
    .await;

    match store_result {
        Ok(Ok(())) => (
            StatusCode::CREATED,
            axum::Json(PublishPackageResponse {
                distro,
                package: package_name,
                version: package_version,
                path: final_ccs_path.to_string_lossy().to_string(),
                size,
                content_hash,
            }),
        )
            .into_response(),
        Ok(Err(err)) => {
            tracing::error!("Failed to store uploaded package metadata: {}", err);
            json_error(500, "Failed to store package metadata", "DB_ERROR")
        }
        Err(err) => {
            tracing::error!("Failed to join blocking package store task: {}", err);
            json_error(500, "Failed to store package metadata", "INTERNAL_ERROR")
        }
    }
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
