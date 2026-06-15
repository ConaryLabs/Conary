// apps/remi/src/server/release_publish.rs
//! Remi release artifact upload, gate enforcement, and public metadata commit.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    Json,
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use conary_core::ccs::CcsPackage;
use conary_core::ccs::convert::ScriptletBundleSummary;
use conary_core::db::models::{ConvertedPackage, Repository, RepositoryPackage};
use conary_core::packages::traits::PackageFormat;
use conary_core::repository::static_repo::publish_gate::{
    AcceptedStaticSignerSet, TrustedArtifactSigner, format_publish_gate_failures,
    verify_static_artifact_publish_eligibility,
};
use conary_core::trust::{
    MetaFile, Signed, SnapshotMetadata, TUF_SPEC_VERSION, TargetDescription, TargetsMetadata,
    sign_tuf_metadata,
};
use futures::StreamExt;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;

use crate::server::ServerState;
use crate::server::handlers::tuf::{load_release_tuf_key, refresh_timestamp_for_distro_in_conn};

const MAX_RELEASE_UPLOAD_SIZE: u64 = 512 * 1024 * 1024;
const RELEASE_PUBLISH_POLICY_DIGEST: &str = "m2-static-publish-policy-v1";

#[derive(Debug, Serialize)]
pub struct ReleaseUploadResponse {
    status: &'static str,
    distro: String,
    package: String,
    version: String,
    path: String,
    size: u64,
    content_hash: String,
}

#[derive(Debug)]
struct ReleaseUploadError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ReleaseUploadError {
    fn bad_request(message: impl Into<String>, code: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message: message.into(),
        }
    }

    fn unprocessable(message: impl Into<String>, code: &'static str) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>, code: &'static str) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code,
            message: message.into(),
        }
    }
}

struct StagedRelease {
    path: PathBuf,
    size: u64,
}

struct ReleaseArtifact {
    name: String,
    version: String,
    architecture: Option<String>,
    content_hash: String,
    scriptlet_summary: ScriptletBundleSummary,
}

struct PromotedRelease {
    package_path: PathBuf,
    chunk_path: PathBuf,
    target_path: String,
}

pub async fn handle_release_upload(
    state: Arc<RwLock<ServerState>>,
    distro: String,
    request: Request,
) -> Response {
    match release_upload_inner(state, distro, request).await {
        Ok(response) => (StatusCode::CREATED, Json(response)).into_response(),
        Err(error) => release_upload_error_response(error),
    }
}

async fn release_upload_inner(
    state: Arc<RwLock<ServerState>>,
    distro: String,
    request: Request,
) -> Result<ReleaseUploadResponse, ReleaseUploadError> {
    let staged = stage_release_body(&state, request).await?;
    let result = release_upload_after_stage(&state, &distro, &staged).await;
    let _ = tokio::fs::remove_file(&staged.path).await;
    result
}

async fn release_upload_after_stage(
    state: &Arc<RwLock<ServerState>>,
    distro: &str,
    staged: &StagedRelease,
) -> Result<ReleaseUploadResponse, ReleaseUploadError> {
    let accepted = accepted_release_signers(state).await?;
    let lint = verify_static_artifact_publish_eligibility(
        &staged.path,
        &accepted,
        RELEASE_PUBLISH_POLICY_DIGEST,
    )
    .map_err(|error| {
        ReleaseUploadError::unprocessable(
            format!("release artifact gate failed: {error}"),
            "GATE_ERROR",
        )
    })?;
    if !lint.is_passed() {
        return Err(ReleaseUploadError::unprocessable(
            format_publish_gate_failures(&lint),
            "PUBLISH_GATE_FAILED",
        ));
    }

    let artifact = inspect_release_artifact(staged).await?;
    let promoted = promote_release_artifact(state, distro, staged, &artifact).await?;
    let commit = commit_release_metadata(state, distro, &artifact, &promoted, staged.size).await;
    if let Err(error) = commit {
        promoted.cleanup_public_objects().await;
        return Err(error);
    }

    Ok(ReleaseUploadResponse {
        status: "created",
        distro: distro.to_string(),
        package: artifact.name,
        version: artifact.version,
        path: promoted.package_path.to_string_lossy().to_string(),
        size: staged.size,
        content_hash: artifact.content_hash,
    })
}

async fn stage_release_body(
    state: &Arc<RwLock<ServerState>>,
    request: Request,
) -> Result<StagedRelease, ReleaseUploadError> {
    let cache_dir = state.read().await.config.cache_dir.clone();
    let staging_dir = cache_dir.join("releases").join("staging");
    tokio::fs::create_dir_all(&staging_dir)
        .await
        .map_err(|error| {
            ReleaseUploadError::internal(
                format!("create release staging directory: {error}"),
                "IO_ERROR",
            )
        })?;

    let path = staging_dir.join(format!("release-{}.ccs", uuid::Uuid::new_v4().simple()));
    let mut file = tokio::fs::File::create(&path).await.map_err(|error| {
        ReleaseUploadError::internal(format!("create staged release body: {error}"), "IO_ERROR")
    })?;

    let mut size = 0u64;
    let mut stream = request.into_body().into_data_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|error| {
            ReleaseUploadError::bad_request(format!("invalid upload body: {error}"), "INVALID_BODY")
        })?;
        size += bytes.len() as u64;
        if size > MAX_RELEASE_UPLOAD_SIZE {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(ReleaseUploadError {
                status: StatusCode::PAYLOAD_TOO_LARGE,
                code: "PAYLOAD_TOO_LARGE",
                message: "Upload exceeds maximum size (512 MB)".to_string(),
            });
        }
        if let Err(error) = file.write_all(&bytes).await {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(ReleaseUploadError::internal(
                format!("write staged release body: {error}"),
                "IO_ERROR",
            ));
        }
    }

    if size == 0 {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(ReleaseUploadError::bad_request(
            "Package body must not be empty",
            "INVALID_BODY",
        ));
    }

    if let Err(error) = file.flush().await {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(ReleaseUploadError::internal(
            format!("flush staged release body: {error}"),
            "IO_ERROR",
        ));
    }

    Ok(StagedRelease { path, size })
}

async fn accepted_release_signers(
    state: &Arc<RwLock<ServerState>>,
) -> Result<AcceptedStaticSignerSet, ReleaseUploadError> {
    let trusted: Vec<TrustedArtifactSigner> = state
        .read()
        .await
        .config
        .release_publish
        .trusted_build_attestation_signers
        .iter()
        .map(|signer| TrustedArtifactSigner {
            key_id: signer.key_id.clone(),
            public_key: signer.public_key.clone(),
        })
        .collect();

    AcceptedStaticSignerSet::from_trusted_artifact_signers(&trusted).map_err(|error| {
        ReleaseUploadError::unprocessable(error.to_string(), "PUBLISH_GATE_FAILED")
    })
}

async fn inspect_release_artifact(
    staged: &StagedRelease,
) -> Result<ReleaseArtifact, ReleaseUploadError> {
    let path = staged.path.clone();
    tokio::task::spawn_blocking(move || -> Result<ReleaseArtifact> {
        let package = CcsPackage::parse(path.to_str().context("release path is not UTF-8")?)?;
        let name = package.name().to_string();
        let version = package.version().to_string();
        let architecture = package.architecture().map(str::to_string);
        let mut reader = std::fs::File::open(&path)?;
        let content_hash = conary_core::hash::sha256_reader_hex(&mut reader)?;
        let scriptlet_summary = package
            .manifest()
            .legacy_scriptlets
            .as_ref()
            .map(|bundle| {
                ScriptletBundleSummary::from_bundle(bundle, bundle.evidence_digest.clone())
            })
            .unwrap_or_default();
        Ok(ReleaseArtifact {
            name,
            version,
            architecture,
            content_hash,
            scriptlet_summary,
        })
    })
    .await
    .map_err(|error| {
        ReleaseUploadError::internal(
            format!("join release inspection task: {error}"),
            "INTERNAL_ERROR",
        )
    })?
    .map_err(|error| {
        ReleaseUploadError::bad_request(
            format!("Uploaded file is not a valid CCS package: {error}"),
            "INVALID_CCS",
        )
    })
}

async fn promote_release_artifact(
    state: &Arc<RwLock<ServerState>>,
    distro: &str,
    staged: &StagedRelease,
    artifact: &ReleaseArtifact,
) -> Result<PromotedRelease, ReleaseUploadError> {
    let (cache_dir, chunk_dir) = {
        let guard = state.read().await;
        (
            guard.config.cache_dir.clone(),
            guard.config.chunk_dir.clone(),
        )
    };
    let packages_dir = cache_dir.join("releases").join("packages").join(distro);
    tokio::fs::create_dir_all(&packages_dir)
        .await
        .map_err(|error| {
            ReleaseUploadError::internal(
                format!("create release package directory: {error}"),
                "IO_ERROR",
            )
        })?;
    let filename = safe_ccs_filename(&artifact.name, &artifact.version, &artifact.content_hash);
    let package_path = packages_dir.join(&filename);
    tokio::fs::copy(&staged.path, &package_path)
        .await
        .map_err(|error| {
            ReleaseUploadError::internal(format!("promote release package: {error}"), "IO_ERROR")
        })?;

    let chunk_path = crate::server::handlers::cas_object_path(&chunk_dir, &artifact.content_hash);
    if let Some(parent) = chunk_path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|error| {
            let package_path = package_path.clone();
            tokio::spawn(async move {
                let _ = tokio::fs::remove_file(package_path).await;
            });
            ReleaseUploadError::internal(
                format!("create release chunk directory: {error}"),
                "IO_ERROR",
            )
        })?;
    }
    tokio::fs::copy(&package_path, &chunk_path)
        .await
        .map_err(|error| {
            let package_path = package_path.clone();
            tokio::spawn(async move {
                let _ = tokio::fs::remove_file(package_path).await;
            });
            ReleaseUploadError::internal(format!("promote release chunk: {error}"), "IO_ERROR")
        })?;

    Ok(PromotedRelease {
        package_path,
        chunk_path,
        target_path: format!("packages/{distro}/{filename}"),
    })
}

async fn commit_release_metadata(
    state: &Arc<RwLock<ServerState>>,
    distro: &str,
    artifact: &ReleaseArtifact,
    promoted: &PromotedRelease,
    size: u64,
) -> Result<(), ReleaseUploadError> {
    let (db_path, keys_dir) = {
        let guard = state.read().await;
        let keys_dir = guard
            .config
            .release_publish
            .repository_keys_dir
            .clone()
            .ok_or_else(|| {
                ReleaseUploadError::internal(
                    "release_publish.repository_keys_dir is not configured",
                    "REPOSITORY_KEYS_MISSING",
                )
            })?;
        (guard.config.db_path.clone(), keys_dir)
    };
    let distro = distro.to_string();
    let artifact = artifact_metadata_for_commit(artifact, promoted, size);

    tokio::task::spawn_blocking(move || {
        commit_release_metadata_blocking(&db_path, &keys_dir, &distro, artifact)
    })
    .await
    .map_err(|error| {
        ReleaseUploadError::internal(
            format!("join release metadata task: {error}"),
            "INTERNAL_ERROR",
        )
    })?
    .map_err(|error| {
        ReleaseUploadError::internal(
            format!("commit release metadata: {error:#}"),
            "METADATA_COMMIT_FAILED",
        )
    })
}

#[derive(Clone)]
struct ReleaseArtifactCommit {
    name: String,
    version: String,
    architecture: Option<String>,
    content_hash: String,
    size: u64,
    package_path: String,
    target_path: String,
    scriptlet_summary: ScriptletBundleSummary,
}

fn artifact_metadata_for_commit(
    artifact: &ReleaseArtifact,
    promoted: &PromotedRelease,
    size: u64,
) -> ReleaseArtifactCommit {
    ReleaseArtifactCommit {
        name: artifact.name.clone(),
        version: artifact.version.clone(),
        architecture: artifact.architecture.clone(),
        content_hash: artifact.content_hash.clone(),
        size,
        package_path: promoted.package_path.to_string_lossy().to_string(),
        target_path: promoted.target_path.clone(),
        scriptlet_summary: artifact.scriptlet_summary.clone(),
    }
}

fn commit_release_metadata_blocking(
    db_path: &Path,
    keys_dir: &Path,
    distro: &str,
    artifact: ReleaseArtifactCommit,
) -> Result<()> {
    let mut conn = crate::server::open_runtime_db(db_path)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let repo_id = ensure_release_repository(&tx, distro)?;
    replace_repository_package(&tx, repo_id, distro, &artifact)?;
    replace_converted_package(&tx, distro, &artifact)?;
    refresh_release_tuf_metadata(&tx, keys_dir, distro, repo_id, &artifact)?;
    tx.commit()?;
    Ok(())
}

fn ensure_release_repository(conn: &rusqlite::Connection, distro: &str) -> Result<i64> {
    if let Some((id, tuf_enabled)) = conn
        .query_row(
            "SELECT id, tuf_enabled FROM repositories WHERE name = ?1",
            params![distro],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i32>(1)? != 0)),
        )
        .optional()?
    {
        if !tuf_enabled {
            conn.execute(
                "UPDATE repositories SET tuf_enabled = 1 WHERE id = ?1",
                params![id],
            )?;
        }
        return Ok(id);
    }

    let mut repo = Repository::new(distro.to_string(), format!("remi-release://{distro}"));
    repo.tuf_enabled = true;
    repo.insert(conn).map_err(anyhow::Error::from)
}

fn replace_repository_package(
    conn: &rusqlite::Connection,
    repo_id: i64,
    distro: &str,
    artifact: &ReleaseArtifactCommit,
) -> Result<()> {
    delete_repo_package_identity(conn, repo_id, artifact)?;
    let mut package = RepositoryPackage::new(
        repo_id,
        artifact.name.clone(),
        artifact.version.clone(),
        artifact.content_hash.clone(),
        artifact.size as i64,
        format!("/v1/chunks/{}", artifact.content_hash),
    );
    package.architecture = artifact.architecture.clone();
    package.description = Some("Remi release artifact".to_string());
    package.distro = Some(distro.to_string());
    package.insert(conn).map_err(anyhow::Error::from)?;
    Ok(())
}

fn delete_repo_package_identity(
    conn: &rusqlite::Connection,
    repo_id: i64,
    artifact: &ReleaseArtifactCommit,
) -> Result<()> {
    if let Some(arch) = artifact.architecture.as_deref() {
        conn.execute(
            "DELETE FROM repository_packages
             WHERE repository_id = ?1 AND name = ?2 AND version = ?3 AND architecture = ?4",
            params![repo_id, artifact.name, artifact.version, arch],
        )?;
    } else {
        conn.execute(
            "DELETE FROM repository_packages
             WHERE repository_id = ?1 AND name = ?2 AND version = ?3 AND architecture IS NULL",
            params![repo_id, artifact.name, artifact.version],
        )?;
    }
    Ok(())
}

fn replace_converted_package(
    conn: &rusqlite::Connection,
    distro: &str,
    artifact: &ReleaseArtifactCommit,
) -> Result<()> {
    delete_converted_identity(conn, distro, artifact)?;
    let mut converted = ConvertedPackage::new_server(
        distro.to_string(),
        artifact.name.clone(),
        artifact.version.clone(),
        "ccs".to_string(),
        format!("release:{distro}:{}", artifact.content_hash),
        "full".to_string(),
        std::slice::from_ref(&artifact.content_hash),
        artifact.size as i64,
        artifact.content_hash.clone(),
        artifact.package_path.clone(),
    );
    converted.package_architecture = artifact.architecture.clone();
    converted
        .set_scriptlet_metadata(&artifact.scriptlet_summary)
        .map_err(anyhow::Error::from)?;
    converted.insert(conn).map_err(anyhow::Error::from)?;
    Ok(())
}

fn delete_converted_identity(
    conn: &rusqlite::Connection,
    distro: &str,
    artifact: &ReleaseArtifactCommit,
) -> Result<()> {
    if let Some(arch) = artifact.architecture.as_deref() {
        conn.execute(
            "DELETE FROM converted_packages
             WHERE distro = ?1 AND package_name = ?2 AND package_version = ?3
               AND package_architecture = ?4",
            params![distro, artifact.name, artifact.version, arch],
        )?;
    } else {
        conn.execute(
            "DELETE FROM converted_packages
             WHERE distro = ?1 AND package_name = ?2 AND package_version = ?3
               AND package_architecture IS NULL",
            params![distro, artifact.name, artifact.version],
        )?;
    }
    Ok(())
}

fn refresh_release_tuf_metadata(
    conn: &rusqlite::Connection,
    keys_dir: &Path,
    distro: &str,
    repo_id: i64,
    artifact: &ReleaseArtifactCommit,
) -> Result<()> {
    let targets_key = load_release_tuf_key(keys_dir, distro, "targets")?;
    let snapshot_key = load_release_tuf_key(keys_dir, distro, "snapshot")?;

    let targets_version = next_tuf_metadata_version(conn, repo_id, "targets")?;
    conn.execute(
        "INSERT OR REPLACE INTO tuf_targets
         (repository_id, target_path, sha256, length, custom_json, targets_version)
         VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
        params![
            repo_id,
            artifact.target_path,
            artifact.content_hash,
            artifact.size as i64,
            targets_version as i64,
        ],
    )?;
    let targets = TargetsMetadata {
        type_field: "targets".to_string(),
        spec_version: TUF_SPEC_VERSION.to_string(),
        version: targets_version,
        expires: chrono::Utc::now() + chrono::Duration::days(30),
        targets: load_tuf_targets(conn, repo_id)?,
    };
    let signed_targets = Signed {
        signatures: vec![sign_tuf_metadata(&targets_key, &targets).map_err(anyhow::Error::from)?],
        signed: targets,
    };
    persist_signed_targets(conn, repo_id, &signed_targets)?;

    let targets_json = serde_json::to_string(&signed_targets)?;
    let snapshot_version = next_tuf_metadata_version(conn, repo_id, "snapshot")?;
    let mut target_hashes = BTreeMap::new();
    target_hashes.insert(
        "sha256".to_string(),
        conary_core::hash::sha256(targets_json.as_bytes()),
    );
    let mut meta = BTreeMap::new();
    meta.insert(
        "targets.json".to_string(),
        MetaFile {
            version: targets_version,
            length: Some(targets_json.len() as u64),
            hashes: Some(target_hashes),
        },
    );
    let snapshot = SnapshotMetadata {
        type_field: "snapshot".to_string(),
        spec_version: TUF_SPEC_VERSION.to_string(),
        version: snapshot_version,
        expires: chrono::Utc::now() + chrono::Duration::days(7),
        meta,
    };
    let signed_snapshot = Signed {
        signatures: vec![sign_tuf_metadata(&snapshot_key, &snapshot).map_err(anyhow::Error::from)?],
        signed: snapshot,
    };
    persist_signed_snapshot(conn, repo_id, &signed_snapshot)?;

    refresh_timestamp_for_distro_in_conn(conn, keys_dir, distro)?;
    Ok(())
}

fn next_tuf_metadata_version(conn: &rusqlite::Connection, repo_id: i64, role: &str) -> Result<u64> {
    let current: Option<i64> = conn
        .query_row(
            "SELECT version FROM tuf_metadata WHERE repository_id = ?1 AND role = ?2",
            params![repo_id, role],
            |row| row.get(0),
        )
        .optional()?;
    Ok(current.unwrap_or(0) as u64 + 1)
}

fn load_tuf_targets(
    conn: &rusqlite::Connection,
    repo_id: i64,
) -> Result<BTreeMap<String, TargetDescription>> {
    let mut stmt = conn.prepare(
        "SELECT target_path, sha256, length FROM tuf_targets
         WHERE repository_id = ?1 ORDER BY target_path",
    )?;
    let rows = stmt.query_map(params![repo_id], |row| {
        let path: String = row.get(0)?;
        let sha256: String = row.get(1)?;
        let length: i64 = row.get(2)?;
        let mut hashes = BTreeMap::new();
        hashes.insert("sha256".to_string(), sha256);
        Ok((
            path,
            TargetDescription {
                length: length as u64,
                hashes,
            },
        ))
    })?;
    let mut targets = BTreeMap::new();
    for row in rows {
        let (path, description) = row?;
        targets.insert(path, description);
    }
    Ok(targets)
}

fn persist_signed_targets(
    conn: &rusqlite::Connection,
    repo_id: i64,
    signed: &Signed<TargetsMetadata>,
) -> Result<()> {
    persist_signed_metadata(
        conn,
        repo_id,
        "targets",
        signed.signed.version,
        signed.signed.expires.to_rfc3339(),
        serde_json::to_string(signed)?,
    )
}

fn persist_signed_snapshot(
    conn: &rusqlite::Connection,
    repo_id: i64,
    signed: &Signed<SnapshotMetadata>,
) -> Result<()> {
    persist_signed_metadata(
        conn,
        repo_id,
        "snapshot",
        signed.signed.version,
        signed.signed.expires.to_rfc3339(),
        serde_json::to_string(signed)?,
    )
}

fn persist_signed_metadata(
    conn: &rusqlite::Connection,
    repo_id: i64,
    role: &str,
    version: u64,
    expires_at: String,
    signed_json: String,
) -> Result<()> {
    let metadata_hash = conary_core::hash::sha256(signed_json.as_bytes());
    conn.execute(
        "INSERT OR REPLACE INTO tuf_metadata
         (repository_id, role, version, metadata_hash, signed_metadata, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            repo_id,
            role,
            version as i64,
            metadata_hash,
            signed_json,
            expires_at,
        ],
    )?;
    Ok(())
}

impl PromotedRelease {
    async fn cleanup_public_objects(&self) {
        let _ = tokio::fs::remove_file(&self.package_path).await;
        let _ = tokio::fs::remove_file(&self.chunk_path).await;
    }
}

fn release_upload_error_response(error: ReleaseUploadError) -> Response {
    (
        error.status,
        Json(serde_json::json!({
            "error": error.message,
            "code": error.code,
        })),
    )
        .into_response()
}

fn safe_ccs_filename(name: &str, version: &str, content_hash: &str) -> String {
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
    let hash_prefix = content_hash.get(..12).unwrap_or(content_hash);
    format!("{}-{}-{hash_prefix}.ccs", sanitize(name), sanitize(version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request};
    use conary_core::ccs::attestation::{
        BUILD_ATTESTATION_SCHEMA_V1, BuildAttestationPayload, canonical_json_hash,
        compute_build_output_identity, sign_build_attestation,
    };
    use conary_core::ccs::builder::{CcsBuilder, write_ccs_package, write_signed_ccs_package};
    use conary_core::ccs::manifest_provenance::ManifestProvenance;
    use conary_core::ccs::signing::SigningKeyPair;
    use conary_core::db::schema;
    use conary_core::recipe::hermetic::{
        BuildInputIdentity, BuilderEnvironmentIdentity, BuilderEnvironmentKind, DependencyLock,
        DivergenceReport, EcosystemPolicyReport, HERMETIC_EVIDENCE_SCHEMA_V1,
        HermeticBuildEvidence, RecipeIdentity, ReproducibilityRecord, SourceIdentity,
    };
    use tower::ServiceExt;

    struct ReleaseFixture {
        _temp: tempfile::TempDir,
        app: axum::Router,
        db_path: PathBuf,
        chunk_dir: PathBuf,
    }

    impl ReleaseFixture {
        fn new(trusted: Vec<crate::server::config::TrustedBuildAttestationSigner>) -> Self {
            Self::new_with_tuf_roles(trusted, &["targets", "snapshot", "timestamp"])
        }

        fn new_with_tuf_roles(
            trusted: Vec<crate::server::config::TrustedBuildAttestationSigner>,
            tuf_roles: &[&str],
        ) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let db_path = temp.path().join("remi.db");
            let chunk_dir = temp.path().join("chunks");
            let cache_dir = temp.path().join("cache");
            let keys_dir = temp.path().join("keys");
            std::fs::create_dir_all(&chunk_dir).unwrap();
            std::fs::create_dir_all(&cache_dir).unwrap();
            write_tuf_role_keys(&keys_dir, "test-distro", tuf_roles);

            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
                .unwrap();
            schema::migrate(&conn).unwrap();
            drop(conn);

            let release_publish = crate::server::config::ReleasePublishSection {
                repository_keys_dir: Some(keys_dir),
                trusted_build_attestation_signers: trusted,
            };
            let config = crate::server::ServerConfig {
                db_path: db_path.clone(),
                chunk_dir: chunk_dir.clone(),
                cache_dir,
                release_publish,
                ..Default::default()
            };
            let state = Arc::new(RwLock::new(
                crate::server::ServerState::new(config).expect("test server state"),
            ));
            let app = crate::server::routes::create_external_admin_router(state, None);
            seed_admin_token(&db_path);

            Self {
                _temp: temp,
                app,
                db_path,
                chunk_dir,
            }
        }

        async fn upload_release(&self, bytes: Vec<u8>) -> Response {
            self.app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/v1/admin/releases/test-distro")
                        .header("Authorization", "Bearer test-admin-token-12345")
                        .body(Body::from(bytes))
                        .unwrap(),
                )
                .await
                .unwrap()
        }

        fn converted_package_row_exists(&self, package: &str) -> bool {
            let conn = rusqlite::Connection::open(&self.db_path).unwrap();
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM converted_packages
                     WHERE distro = 'test-distro' AND package_name = ?1",
                    params![package],
                    |row| row.get(0),
                )
                .unwrap();
            count > 0
        }

        fn public_package_detail_exists(&self, package: &str) -> bool {
            let conn = rusqlite::Connection::open(&self.db_path).unwrap();
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM repository_packages rp
                     JOIN repositories r ON rp.repository_id = r.id
                     WHERE r.name = 'test-distro' AND rp.name = ?1",
                    params![package],
                    |row| row.get(0),
                )
                .unwrap();
            count > 0
        }

        fn public_chunk_exists(&self, content_hash: &str) -> bool {
            crate::server::handlers::cas_object_path(&self.chunk_dir, content_hash).exists()
        }

        fn tuf_target_exists(&self, package: &str) -> bool {
            let conn = rusqlite::Connection::open(&self.db_path).unwrap();
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM tuf_targets
                     WHERE target_path LIKE ?1",
                    params![format!("%{package}%")],
                    |row| row.get(0),
                )
                .unwrap();
            count > 0
        }
    }

    #[tokio::test]
    async fn release_upload_empty_trusted_signers_fail_closed() {
        let signer = SigningKeyPair::generate().with_key_id("publisher");
        let artifact = attested_release_artifact(&signer, "hello", "1.0.0");
        let fixture = ReleaseFixture::new(Vec::new());

        let response = fixture.upload_release(artifact.bytes).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = response_text(response).await;
        assert!(body.contains("no trusted release signers configured"));
        assert_no_public_state(&fixture, "hello", &artifact.content_hash);
    }

    #[tokio::test]
    async fn remi_release_parity_rejected_upload_leaves_no_public_state() {
        let signer = SigningKeyPair::generate().with_key_id("publisher");
        let trusted_other = SigningKeyPair::generate().with_key_id("other");
        let artifact = attested_release_artifact(&signer, "hello", "1.0.0");
        let fixture = ReleaseFixture::new(vec![trusted_signer(&trusted_other)]);

        let response = fixture.upload_release(artifact.bytes).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_no_public_state(&fixture, "hello", &artifact.content_hash);
    }

    #[tokio::test]
    async fn remi_release_parity_commit_failure_after_promotion_cleans_public_objects() {
        let signer = SigningKeyPair::generate().with_key_id("publisher");
        let artifact = attested_release_artifact(&signer, "hello", "1.0.0");
        let fixture = ReleaseFixture::new_with_tuf_roles(
            vec![trusted_signer(&signer)],
            &["targets", "timestamp"],
        );

        let response = fixture.upload_release(artifact.bytes).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_no_public_state(&fixture, "hello", &artifact.content_hash);
    }

    #[tokio::test]
    async fn release_upload_with_accepted_signer_publishes_public_metadata() {
        let signer = SigningKeyPair::generate().with_key_id("publisher");
        let artifact = attested_release_artifact(&signer, "hello", "1.0.0");
        let fixture = ReleaseFixture::new(vec![trusted_signer(&signer)]);

        let response = fixture.upload_release(artifact.bytes).await;
        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "{}",
            response_text(response).await
        );
        assert!(fixture.converted_package_row_exists("hello"));
        assert!(fixture.public_package_detail_exists("hello"));
        assert!(fixture.public_chunk_exists(&artifact.content_hash));
        assert!(fixture.tuf_target_exists("hello"));
    }

    fn assert_no_public_state(fixture: &ReleaseFixture, package: &str, content_hash: &str) {
        assert!(!fixture.converted_package_row_exists(package));
        assert!(!fixture.public_package_detail_exists(package));
        assert!(!fixture.public_chunk_exists(content_hash));
        assert!(!fixture.tuf_target_exists(package));
    }

    async fn response_text(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn trusted_signer(
        key: &SigningKeyPair,
    ) -> crate::server::config::TrustedBuildAttestationSigner {
        crate::server::config::TrustedBuildAttestationSigner {
            key_id: key.key_id().unwrap_or("publisher").to_string(),
            public_key: key.public_key_base64(),
        }
    }

    fn seed_admin_token(db_path: &Path) {
        let token = "test-admin-token-12345";
        let hash = crate::server::auth::hash_token(token);
        let conn = rusqlite::Connection::open(db_path).unwrap();
        conary_core::db::models::admin_token::create(&conn, "test-admin", &hash, "admin").unwrap();
    }

    fn write_tuf_role_keys(keys_dir: &Path, distro: &str, roles: &[&str]) {
        let distro_dir = keys_dir.join(distro);
        std::fs::create_dir_all(&distro_dir).unwrap();
        for role in roles {
            SigningKeyPair::generate()
                .with_key_id(role)
                .save_to_files(
                    &distro_dir.join(format!("{role}.private")),
                    &distro_dir.join(format!("{role}.public")),
                )
                .unwrap();
        }
    }

    struct TestArtifact {
        bytes: Vec<u8>,
        content_hash: String,
    }

    fn attested_release_artifact(
        signer: &SigningKeyPair,
        name: &str,
        version: &str,
    ) -> TestArtifact {
        let temp = tempfile::tempdir().unwrap();
        let source_dir = temp.path().join("source");
        std::fs::create_dir_all(source_dir.join("usr/share")).unwrap();
        std::fs::write(source_dir.join("usr/share/payload"), b"release payload").unwrap();

        let evidence = sample_hermetic_evidence_for_tests(name, version);
        let mut manifest = conary_core::ccs::CcsManifest::new_minimal(name, version);
        manifest.provenance = Some(ManifestProvenance {
            origin_class: Some("native-built".to_string()),
            hardening_level: Some("hermetic".to_string()),
            hermetic_evidence: Some(evidence.clone()),
            ..Default::default()
        });
        let mut result = CcsBuilder::new(manifest, &source_dir).build().unwrap();
        let identity_path = temp.path().join("identity.ccs");
        write_ccs_package(&result, &identity_path).unwrap();
        let identity_package = CcsPackage::parse(identity_path.to_str().unwrap()).unwrap();
        let output_identity = compute_build_output_identity(&identity_package).unwrap();
        let payload = BuildAttestationPayload {
            schema_version: BUILD_ATTESTATION_SCHEMA_V1,
            origin_class: output_identity.origin_class.clone(),
            hardening_level: output_identity.hardening_level.clone(),
            build_input: evidence.build_input.clone(),
            dependency_lock: evidence.dependency_lock.clone(),
            hermetic_evidence_hash: canonical_json_hash(&evidence).unwrap(),
            output_identity,
            build_command_risk_report_hash: canonical_json_hash(&evidence.command_risk).unwrap(),
            scriptlet_risk_report_hash: None,
            conversion_boundary_hash: None,
            publish_policy_digest: RELEASE_PUBLISH_POLICY_DIGEST.to_string(),
            command_risk_classifier_version: evidence.command_risk.classifier_version.clone(),
            sandbox_profile: "kitchen-pristine-network-none".to_string(),
            seccomp_profile: Some("scriptlet-v1".to_string()),
            builder_identity: "remi-release-test-builder".to_string(),
            conary_version: "test".to_string(),
            issued_at: "2026-06-14T00:00:00Z".to_string(),
        };
        result
            .manifest
            .provenance
            .as_mut()
            .unwrap()
            .build_attestation = Some(sign_build_attestation(payload, signer).unwrap());
        let package_path = temp.path().join("release.ccs");
        write_signed_ccs_package(&result, &package_path, signer).unwrap();
        let bytes = std::fs::read(package_path).unwrap();
        let content_hash = conary_core::hash::sha256(&bytes);
        TestArtifact {
            bytes,
            content_hash,
        }
    }

    fn sample_hermetic_evidence_for_tests(name: &str, version: &str) -> HermeticBuildEvidence {
        HermeticBuildEvidence {
            schema_version: HERMETIC_EVIDENCE_SCHEMA_V1,
            build_input: BuildInputIdentity {
                recipe: RecipeIdentity::GeneratedRecipe {
                    generator: "remi-release-test".to_string(),
                    canonical_hash: conary_core::hash::sha256_prefixed(
                        format!("{name}:{version}").as_bytes(),
                    ),
                    inference_trace_hash: conary_core::hash::sha256_prefixed(b"test"),
                },
                source: SourceIdentity::Archive {
                    url: "https://example.invalid/source.tar.gz".to_string(),
                    checksum: "sha256:source".to_string(),
                },
                additional_sources: Vec::new(),
                patches: Vec::new(),
                local_tree: None,
                ecosystem_dependencies: Vec::new(),
                builder_environment: BuilderEnvironmentIdentity {
                    kind: BuilderEnvironmentKind::Pristine,
                    sysroot_hash: Some("sha256:sysroot".to_string()),
                    toolchain_hash: None,
                    diagnostics: Vec::new(),
                },
            },
            dependency_lock: DependencyLock::default(),
            ecosystem_policy: EcosystemPolicyReport::clean("test"),
            command_risk: conary_core::recipe::hermetic::BuildCommandRiskReport::clean(),
            reproducibility: ReproducibilityRecord {
                source_date_epoch: Some(1),
                path_remap_count: 1,
                env_keys: vec!["SOURCE_DATE_EPOCH".to_string()],
            },
            divergence: DivergenceReport::default(),
            diagnostics: Vec::new(),
        }
    }
}
