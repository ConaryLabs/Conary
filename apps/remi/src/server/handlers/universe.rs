// apps/remi/src/server/handlers/universe.rs

//! Public read-only transport for the signed endpoint-wide Remi universe.

use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use conary_core::repository::catalog::CATALOG_FILE_NAME;
use conary_core::trust::{RootMetadata, Signed};
use rusqlite::OptionalExtension;
use tokio::sync::RwLock;
use tokio_util::io::ReaderStream;

use crate::server::ServerState;
use crate::server::universe_publish::{
    UNIVERSE_CANONICAL_MAP_FILE, UNIVERSE_MANIFEST_FILE, UNIVERSE_ROOT_FILE,
    UNIVERSE_SNAPSHOT_FILE, UNIVERSE_TARGETS_FILE, UNIVERSE_TIMESTAMP_FILE, universe_bundle_path,
};

use super::open_handler_db;

#[derive(Debug)]
struct PublicFile {
    path: PathBuf,
    content_type: &'static str,
    cache_control: &'static str,
    expected_sha256: Option<String>,
}

/// GET `/v1/universe/tuf/{name}`.
pub async fn get_metadata(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
) -> Response {
    let (db_path, catalog_dir) = {
        let guard = state.read().await;
        (
            guard.config.db_path.clone(),
            guard.config.catalog_dir.clone(),
        )
    };
    let resolved =
        tokio::task::spawn_blocking(move || resolve_metadata_file(&db_path, &catalog_dir, &name))
            .await;
    respond_to_resolution(resolved).await
}

/// GET `/v1/universe/targets/universe/{manifest}.json`.
pub async fn get_manifest_target(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(manifest): Path<String>,
) -> Response {
    let Some(sha256) = manifest.strip_suffix(".json").map(str::to_string) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if !is_sha256(&sha256) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let (db_path, catalog_dir) = {
        let guard = state.read().await;
        (
            guard.config.db_path.clone(),
            guard.config.catalog_dir.clone(),
        )
    };
    let resolved = tokio::task::spawn_blocking(move || {
        resolve_manifest_target(&db_path, &catalog_dir, &sha256)
    })
    .await;
    respond_to_resolution(resolved).await
}

/// GET `/v1/universe/targets/objects/sha256/{sha256}`.
pub async fn get_object_target(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(sha256): Path<String>,
) -> Response {
    if !is_sha256(&sha256) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let (db_path, catalog_dir) = {
        let guard = state.read().await;
        (
            guard.config.db_path.clone(),
            guard.config.catalog_dir.clone(),
        )
    };
    let resolved =
        tokio::task::spawn_blocking(move || resolve_object_target(&db_path, &catalog_dir, &sha256))
            .await;
    respond_to_resolution(resolved).await
}

async fn respond_to_resolution(
    resolved: std::result::Result<Result<Option<PublicFile>>, tokio::task::JoinError>,
) -> Response {
    match resolved {
        Ok(Ok(Some(file))) => serve_public_file(file).await,
        Ok(Ok(None)) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(error)) => {
            tracing::error!(%error, "failed to resolve signed Remi universe file");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(error) => {
            tracing::error!(%error, "signed Remi universe resolver task failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn serve_public_file(file: PublicFile) -> Response {
    let metadata = match tokio::fs::symlink_metadata(&file.path).await {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Ok(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tracing::error!(path = %file.path.display(), "durable universe file is missing");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        Err(error) => {
            tracing::error!(%error, path = %file.path.display(), "cannot inspect universe file");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let opened = match tokio::fs::File::open(&file.path).await {
        Ok(opened) => opened,
        Err(error) => {
            tracing::error!(%error, path = %file.path.display(), "cannot open universe file");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, file.content_type)
        .header(header::CONTENT_LENGTH, metadata.len().to_string())
        .header(header::CACHE_CONTROL, file.cache_control);
    if let Some(sha256) = file.expected_sha256 {
        builder = builder.header(header::ETAG, format!("\"sha256:{sha256}\""));
    }
    builder
        .body(Body::from_stream(ReaderStream::new(opened)))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn resolve_metadata_file(
    db_path: &FsPath,
    catalog_dir: &FsPath,
    requested: &str,
) -> Result<Option<PublicFile>> {
    let conn = open_handler_db(db_path)?;
    let active = conn
        .query_row(
            "SELECT manifest_sha256 FROM remi_active_universe_revision WHERE singleton = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(active) = active else {
        return Ok(None);
    };
    require_sha256(&active, "active universe manifest")?;
    let bundle = universe_bundle_path(catalog_dir, &active);
    let file_name = match requested {
        "root.json" => UNIVERSE_ROOT_FILE,
        "targets.json" => UNIVERSE_TARGETS_FILE,
        "snapshot.json" => UNIVERSE_SNAPSHOT_FILE,
        "timestamp.json" => UNIVERSE_TIMESTAMP_FILE,
        versioned if versioned.ends_with(".root.json") => {
            let version = versioned
                .strip_suffix(".root.json")
                .context("versioned universe root suffix")?
                .parse::<u64>()
                .context("versioned universe root is not numeric")?;
            let bytes = std::fs::read(bundle.join(UNIVERSE_ROOT_FILE))?;
            let root: Signed<RootMetadata> =
                serde_json::from_slice(&bytes).context("parse active universe root")?;
            if root.signed.version != version {
                return resolve_historical_root(&conn, catalog_dir, version);
            }
            UNIVERSE_ROOT_FILE
        }
        _ => return Ok(None),
    };
    Ok(Some(PublicFile {
        path: bundle.join(file_name),
        content_type: "application/json",
        cache_control: "no-cache",
        expected_sha256: None,
    }))
}

fn resolve_historical_root(
    conn: &rusqlite::Connection,
    catalog_dir: &FsPath,
    requested_version: u64,
) -> Result<Option<PublicFile>> {
    let mut statement = conn.prepare(
        "SELECT manifest_sha256 FROM remi_universe_revisions
         WHERE durable = 1 ORDER BY sequence DESC",
    )?;
    let revisions = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for revision in revisions {
        require_sha256(&revision, "historical universe manifest")?;
        let path = universe_bundle_path(catalog_dir, &revision).join(UNIVERSE_ROOT_FILE);
        let bytes = std::fs::read(&path)?;
        let root: Signed<RootMetadata> =
            serde_json::from_slice(&bytes).context("parse historical universe root")?;
        if root.signed.version == requested_version {
            return Ok(Some(PublicFile {
                path,
                content_type: "application/json",
                cache_control: "public, max-age=31536000, immutable",
                expected_sha256: Some(conary_core::hash::sha256(&bytes)),
            }));
        }
    }
    Ok(None)
}

fn resolve_manifest_target(
    db_path: &FsPath,
    catalog_dir: &FsPath,
    manifest_sha256: &str,
) -> Result<Option<PublicFile>> {
    let conn = open_handler_db(db_path)?;
    let exists = conn
        .query_row(
            "SELECT 1 FROM remi_universe_revisions
             WHERE manifest_sha256 = ?1 AND durable = 1",
            [manifest_sha256],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    Ok(exists.then(|| PublicFile {
        path: universe_bundle_path(catalog_dir, manifest_sha256).join(UNIVERSE_MANIFEST_FILE),
        content_type: "application/json",
        cache_control: "public, max-age=31536000, immutable",
        expected_sha256: Some(manifest_sha256.to_string()),
    }))
}

fn resolve_object_target(
    db_path: &FsPath,
    catalog_dir: &FsPath,
    object_sha256: &str,
) -> Result<Option<PublicFile>> {
    let conn = open_handler_db(db_path)?;
    if let Some(manifest_sha256) = conn
        .query_row(
            "SELECT manifest_sha256 FROM remi_universe_revisions
             WHERE canonical_map_sha256 = ?1 AND durable = 1
             ORDER BY sequence DESC LIMIT 1",
            [object_sha256],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        require_sha256(&manifest_sha256, "canonical-map universe manifest")?;
        return Ok(Some(PublicFile {
            path: universe_bundle_path(catalog_dir, &manifest_sha256)
                .join(UNIVERSE_CANONICAL_MAP_FILE),
            content_type: "application/json",
            cache_control: "public, max-age=31536000, immutable",
            expected_sha256: Some(object_sha256.to_string()),
        }));
    }
    let profile = conn
        .query_row(
            "SELECT member.source_profile, member.profile_revision_sha256
             FROM remi_universe_profile_revisions member
             JOIN remi_universe_revisions universe
               ON universe.manifest_sha256 = member.manifest_sha256
             WHERE member.catalog_sha256 = ?1 AND universe.durable = 1
             ORDER BY universe.sequence DESC LIMIT 1",
            [object_sha256],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((source_profile, profile_revision_sha256)) = profile else {
        return Ok(None);
    };
    require_sha256(&profile_revision_sha256, "profile revision")?;
    Ok(Some(PublicFile {
        path: catalog_dir
            .join("profiles")
            .join(source_profile)
            .join(profile_revision_sha256)
            .join(CATALOG_FILE_NAME),
        content_type: "application/vnd.sqlite3",
        cache_control: "public, max-age=31536000, immutable",
        expected_sha256: Some(object_sha256.to_string()),
    }))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn require_sha256(value: &str, label: &str) -> Result<()> {
    if !is_sha256(value) {
        bail!("{label} is not one lowercase SHA-256 digest");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_digest_paths_are_strict_lowercase_sha256() {
        assert!(is_sha256(&"a".repeat(64)));
        assert!(!is_sha256(&"A".repeat(64)));
        assert!(!is_sha256(&"a".repeat(63)));
        assert!(!is_sha256("../root.json"));
    }
}
