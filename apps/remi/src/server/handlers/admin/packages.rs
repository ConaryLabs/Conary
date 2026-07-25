// apps/remi/src/server/handlers/admin/packages.rs
//! Admin handlers for publishing custom CCS packages into Remi metadata.

use super::{check_scope, validate_supported_admin_distro_route};
use crate::server::ServerState;
use crate::server::auth::{Scope, TokenScopes, json_error};
use axum::{
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use conary_core::ccs::convert::ScriptletBundleSummary;
use conary_core::packages::PackageFormat;
use futures::StreamExt;
use serde::Serialize;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;

/// Maximum allowed upload size (512 MB).
const MAX_UPLOAD_SIZE: u64 = 512 * 1024 * 1024;

struct AtomicReplaceInput {
    db_path: PathBuf,
    distro: String,
    package_name: String,
    package_version: String,
    package_architecture: Option<String>,
    content_hash: String,
    size: i64,
    final_ccs_path: String,
    scriptlet_summary: ScriptletBundleSummary,
}

fn update_converted_ccs_path(
    db_path: &FsPath,
    distro: &str,
    package_name: &str,
    package_version: &str,
    package_architecture: Option<&str>,
    ccs_path: &str,
) -> anyhow::Result<()> {
    let conn = crate::server::open_runtime_db(db_path)?;
    let updated = if let Some(architecture) = package_architecture {
        conn.execute(
            "UPDATE converted_packages SET ccs_path = ?1 \
             WHERE distro = ?2 AND package_name = ?3 AND package_version = ?4 \
               AND package_architecture = ?5",
            rusqlite::params![
                ccs_path,
                distro,
                package_name,
                package_version,
                architecture
            ],
        )?
    } else {
        conn.execute(
            "UPDATE converted_packages SET ccs_path = ?1 \
             WHERE distro = ?2 AND package_name = ?3 AND package_version = ?4 \
               AND package_architecture IS NULL",
            rusqlite::params![ccs_path, distro, package_name, package_version],
        )?
    };
    anyhow::ensure!(
        updated == 1,
        "expected one converted package path update for {distro}/{package_name}/{package_version}, updated {updated}"
    );
    Ok(())
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
    input: AtomicReplaceInput,
) -> anyhow::Result<Option<conary_core::db::models::ConvertedPackage>> {
    tokio::task::spawn_blocking(move || {
        let mut conn = crate::server::open_runtime_db(&input.db_path)?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| {
                anyhow::anyhow!("begin converted package metadata transaction: {error}")
            })?;

        // Find existing record (if any) before deleting.
        let existing =
            conary_core::db::models::ConvertedPackage::find_by_package_identity_with_arch(
                &tx,
                &input.distro,
                &input.package_name,
                Some(&input.package_version),
                input.package_architecture.as_deref(),
            )
            .map_err(|error| {
                anyhow::anyhow!("find existing converted package metadata: {error}")
            })?;

        // Delete old record inside the transaction.
        if let Some(ref existing) = existing {
            conary_core::db::models::ConvertedPackage::delete_by_checksum(
                &tx,
                &existing.original_checksum,
            )
            .map_err(|error| {
                anyhow::anyhow!("delete existing converted package metadata: {error}")
            })?;
        }

        // Insert new record inside the same transaction.
        let mut converted = conary_core::db::models::ConvertedPackage::new_repository(
            input.distro.clone(),
            input.package_name.clone(),
            input.package_version.clone(),
            "ccs".to_string(),
            format!("upload:{}:{}", input.distro, input.content_hash),
            std::slice::from_ref(&input.content_hash),
            input.size,
            input.content_hash.clone(),
            input.final_ccs_path,
        );
        converted.package_architecture = input.package_architecture;
        converted
            .set_scriptlet_metadata(&input.scriptlet_summary)
            .map_err(|error| anyhow::anyhow!("serialize scriptlet metadata: {error}"))?;
        converted
            .insert(&tx)
            .map_err(|error| anyhow::anyhow!("insert converted package metadata: {error}"))?;
        if let Some(existing_chunk) =
            conary_core::db::models::ChunkAccess::find_by_hash(&tx, &input.content_hash)?
            && existing_chunk.size_bytes != input.size
        {
            anyhow::bail!(
                "chunk {} size disagrees with persisted CAS authority: {} != {}",
                input.content_hash,
                existing_chunk.size_bytes,
                input.size
            );
        }
        conary_core::db::models::ChunkAccess::new(input.content_hash, input.size)
            .upsert(&tx)
            .map_err(|error| anyhow::anyhow!("persist exact chunk size: {error}"))?;

        tx.commit()
            .map_err(|error| anyhow::anyhow!("commit converted package metadata: {error}"))?;

        Ok(existing)
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
    if let Some(err) = validate_supported_admin_distro_route(&distro) {
        return err;
    }

    let (cache_dir, chunk_dir, db_path, repository_keys_dir) = {
        let guard = state.read().await;
        (
            guard.config.cache_dir.clone(),
            guard.config.chunk_dir.clone(),
            guard.config.db_path.clone(),
            guard.config.release_publish.repository_keys_dir.clone(),
        )
    };
    let Some(repository_keys_dir) = repository_keys_dir else {
        return json_error(
            500,
            "CCS publication authority is not configured",
            "CONFIG_ERROR",
        );
    };
    let public_key_path = repository_keys_dir.join(&distro).join("targets.public");
    let public_key = match tokio::task::spawn_blocking(move || {
        conary_core::ccs::signing::load_public_key(&public_key_path)
    })
    .await
    {
        Ok(Ok(public_key)) => public_key,
        Ok(Err(error)) => {
            tracing::error!("Failed to load CCS publication authority key: {error}");
            return json_error(
                500,
                "CCS publication authority key is unavailable",
                "CONFIG_ERROR",
            );
        }
        Err(error) => {
            tracing::error!("Failed to join CCS authority key task: {error}");
            return json_error(
                500,
                "Failed to load CCS publication authority",
                "INTERNAL_ERROR",
            );
        }
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

    // Step 3: authenticate CCS v2 authority before deriving publication metadata.
    let inspected = match tokio::task::spawn_blocking({
        let temp_path = temp_path.clone();
        move || -> anyhow::Result<conary_core::ccs::CcsPackage> {
            let verified = conary_core::ccs::verify::verify_package(
                &temp_path,
                &conary_core::ccs::verify::TrustPolicy::strict(vec![public_key]),
            )?;
            let path = temp_path
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("temporary CCS path is not valid UTF-8"))?;
            Ok(conary_core::ccs::CcsPackage::from_verified_archive(
                path, &verified,
            )?)
        }
    })
    .await
    {
        Ok(Ok(pkg)) => pkg,
        Ok(Err(err)) => {
            tracing::warn!("Uploaded package has no trusted CCS v2 authority: {}", err);
            let _ = tokio::fs::remove_file(&temp_path).await;
            return json_error(
                400,
                "Uploaded file is not a trusted CCS v2 package",
                "UNTRUSTED_CCS",
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
    let package_architecture = inspected.architecture().map(str::to_string);
    let scriptlet_summary = match inspected.manifest().native_lifecycle.as_ref() {
        Some(bundle) => {
            if let Err(err) = bundle.validate() {
                tracing::warn!(
                    "Uploaded CCS package {}/{} has invalid native lifecycle bundle: {}",
                    package_name,
                    package_version,
                    err
                );
                let _ = tokio::fs::remove_file(&temp_path).await;
                return json_error(
                    400,
                    "Uploaded CCS package has invalid native lifecycle metadata",
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

    // Step 5: Atomic DB transaction -- point at the STAGED path initially.
    // This way, if the final rename fails, the DB points at the staged file
    // which actually contains the correct new bytes (not the old content).
    let staged_path_str = staged_path.to_string_lossy().to_string();
    let existing = match atomic_replace_record(AtomicReplaceInput {
        db_path: db_path.clone(),
        distro: distro.clone(),
        package_name: package_name.clone(),
        package_version: package_version.clone(),
        package_architecture: package_architecture.clone(),
        content_hash: content_hash.clone(),
        size: size as i64,
        final_ccs_path: staged_path_str.clone(),
        scriptlet_summary: scriptlet_summary.clone(),
    })
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
        let arch_for_update = package_architecture.clone();
        let fp = final_path_str.clone();
        let update_result = tokio::task::spawn_blocking(move || {
            update_converted_ccs_path(
                &db_path_for_update,
                &distro_for_update,
                &name_for_update,
                &version_for_update,
                arch_for_update.as_deref(),
                &fp,
            )
        })
        .await;

        if matches!(&update_result, Ok(Ok(()))) {
            serving_path = final_path_str;
        } else {
            match &update_result {
                Ok(Err(error)) => tracing::error!("DB path update failed: {error}"),
                Err(error) => tracing::error!("DB path update task failed: {error}"),
                Ok(Ok(())) => unreachable!("handled successful update"),
            }
            // UPDATE failed -- try to rename back so DB (staged path) stays valid.
            match tokio::fs::rename(&final_ccs_path, &staged_path).await {
                Ok(()) => {
                    // Rename-back succeeded: file is at staged_path, DB points there.
                    tracing::warn!(
                        "DB path update failed; reverted rename, serving from staged path"
                    );
                    serving_path = staged_path_str.clone();
                }
                Err(rename_back_error) => {
                    // Rename-back also failed: file is at final_ccs_path, DB points
                    // at staged_path (which no longer exists). Force-update DB to
                    // final_ccs_path as a last resort.
                    tracing::error!(
                        "DB update failed AND rename-back failed ({rename_back_error}); forcing DB to final path"
                    );
                    let final_str = final_ccs_path.to_string_lossy().to_string();
                    let db2 = db_path.clone();
                    let d2 = distro.clone();
                    let n2 = package_name.clone();
                    let v2 = package_version.clone();
                    let a2 = package_architecture.clone();
                    let fs2 = final_str.clone();
                    let repair_result = tokio::task::spawn_blocking(move || {
                        update_converted_ccs_path(&db2, &d2, &n2, &v2, a2.as_deref(), &fs2)
                    })
                    .await;

                    if matches!(&repair_result, Ok(Ok(()))) {
                        serving_path = final_str;
                    } else {
                        match &repair_result {
                            Ok(Err(error)) => tracing::error!("DB path repair failed: {error}"),
                            Err(error) => tracing::error!("DB path repair task failed: {error}"),
                            Ok(Ok(())) => unreachable!("handled successful repair"),
                        }
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
    }

    // Clean up the old file if it had a different path than what we're serving.
    if let Some(existing) = &existing {
        let artifact = match existing.repository_artifact() {
            Ok(artifact) => artifact,
            Err(error) => {
                tracing::error!("Existing converted artifact is corrupt: {error}");
                return json_error(
                    500,
                    "Existing package metadata is corrupt",
                    "CONVERTED_ARTIFACT_CORRUPT",
                );
            }
        };
        if artifact.ccs_path != serving_path {
            let _ = tokio::fs::remove_file(artifact.ccs_path).await;
        }
    }

    // Populate chunk store from the actual serving path.
    let chunk_file_path = chunk_path(&chunk_dir, &content_hash);
    if let Some(parent) = chunk_file_path.parent()
        && let Err(err) = tokio::fs::create_dir_all(parent).await
    {
        tracing::error!("Failed to create chunk dir {}: {}", parent.display(), err);
        return json_error(500, "Failed to create chunk storage", "IO_ERROR");
    }
    match tokio::fs::try_exists(&chunk_file_path).await {
        Ok(true) => {}
        Ok(false) => {
            if let Err(err) = tokio::fs::copy(&serving_path, &chunk_file_path).await {
                tracing::error!(
                    "Failed to copy package into chunk store {}: {}",
                    chunk_file_path.display(),
                    err
                );
                return json_error(500, "Failed to store package chunk", "IO_ERROR");
            }
        }
        Err(err) => {
            tracing::error!(
                "Failed to inspect package chunk path {}: {}",
                chunk_file_path.display(),
                err
            );
            return json_error(500, "Failed to inspect package chunk storage", "IO_ERROR");
        }
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
mod tests;
