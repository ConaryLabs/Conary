// apps/remi/src/server/native_publish/persistence.rs

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use conary_core::db::models::{
    NativePackagePublication, NativePublicationStatus, Repository, RepositoryPackage,
};
use conary_core::trust::{
    MetaFile, Signed, SnapshotMetadata, TUF_SPEC_VERSION, TargetDescription, TargetsMetadata,
    sign_tuf_metadata,
};
use rusqlite::{OptionalExtension, params};
use tokio::sync::RwLock;

use crate::server::ServerState;
use crate::server::handlers::tuf::{load_release_tuf_key, refresh_timestamp_for_distro_in_conn};
use crate::server::native_publish::storage::PromotedNativeArtifact;
use crate::server::native_publish::{
    NativePublishError, NativePublishErrorCode, VerifiedNativeArtifact,
};

#[derive(Debug)]
struct SupersededNativePublication {
    package_path: PathBuf,
    content_hash: String,
    target_path: String,
}

pub(crate) async fn commit_native_publication(
    state: &Arc<RwLock<ServerState>>,
    distro: &str,
    artifact: VerifiedNativeArtifact,
    promoted: PromotedNativeArtifact,
) -> Result<(), NativePublishError> {
    let (db_path, keys_dir) = {
        let guard = state.read().await;
        let keys_dir = guard
            .config
            .release_publish
            .repository_keys_dir
            .clone()
            .ok_or_else(|| {
                NativePublishError::internal(
                    NativePublishErrorCode::MetadataCommitFailed,
                    "release_publish.repository_keys_dir is not configured",
                )
            })?;
        (guard.config.db_path.clone(), keys_dir)
    };
    let distro = distro.to_string();

    tokio::task::spawn_blocking(move || {
        commit_native_publication_blocking(&db_path, &keys_dir, &distro, artifact, promoted)
    })
    .await
    .map_err(|error| {
        NativePublishError::internal(
            NativePublishErrorCode::MetadataCommitFailed,
            format!("join native publication metadata task: {error}"),
        )
    })?
    .map_err(|error| {
        NativePublishError::internal(
            NativePublishErrorCode::MetadataCommitFailed,
            format!("commit native publication metadata: {error:#}"),
        )
    })
}

pub fn commit_native_publication_blocking(
    db_path: &Path,
    keys_dir: &Path,
    distro: &str,
    artifact: VerifiedNativeArtifact,
    promoted: PromotedNativeArtifact,
) -> Result<()> {
    let mut conn = crate::server::open_runtime_db(db_path)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let repo_id = ensure_release_repository(&tx, distro)?;
    let repo_package_id = upsert_repository_projection(&tx, repo_id, distro, &artifact, &promoted)?;
    let superseded = supersede_active_native_identity(&tx, distro, &artifact)?;
    delete_superseded_tuf_targets(&tx, repo_id, &superseded)?;
    insert_native_publication(&tx, repo_id, repo_package_id, distro, &artifact, &promoted)?;
    refresh_release_tuf_metadata(&tx, keys_dir, distro, repo_id, &artifact, &promoted)?;
    tx.commit()?;

    for old in superseded {
        PromotedNativeArtifact::cleanup_package_path_blocking(&old.package_path);
    }
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

fn upsert_repository_projection(
    conn: &rusqlite::Connection,
    repo_id: i64,
    distro: &str,
    artifact: &VerifiedNativeArtifact,
    _promoted: &PromotedNativeArtifact,
) -> Result<i64> {
    let metadata = serde_json::json!({
        "source_kind": "native-ccs",
        "native": true,
        "identity": {
            "name": &artifact.name,
            "version": &artifact.version,
            "release": &artifact.package_release,
            "architecture": &artifact.architecture,
        },
        "trust": {
            "status": "verified",
            "hardening_level": "hermetic",
        }
    })
    .to_string();
    let download_url = format!("/v1/chunks/{}", artifact.content_hash);
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM repository_packages
             WHERE repository_id = ?1 AND name = ?2 AND version = ?3
               AND package_release = ?4 AND architecture = ?5",
            params![
                repo_id,
                artifact.name,
                artifact.version,
                artifact.package_release,
                artifact.architecture,
            ],
            |row| row.get(0),
        )
        .optional()?;

    if let Some(id) = existing {
        conn.execute(
            "UPDATE repository_packages
             SET checksum = ?1, size = ?2, download_url = ?3, metadata = ?4,
                 description = ?5, distro = ?6
             WHERE id = ?7",
            params![
                artifact.content_hash,
                i64::try_from(artifact.total_size).context("native artifact size exceeds i64")?,
                download_url,
                metadata,
                "Remi native CCS release artifact",
                distro,
                id,
            ],
        )?;
        return Ok(id);
    }

    let mut package = RepositoryPackage::new(
        repo_id,
        artifact.name.clone(),
        artifact.version.clone(),
        artifact.content_hash.clone(),
        i64::try_from(artifact.total_size).context("native artifact size exceeds i64")?,
        download_url,
    );
    package.package_release = artifact.package_release.clone();
    package.architecture = Some(artifact.architecture.clone());
    package.description = Some("Remi native CCS release artifact".to_string());
    package.metadata = Some(metadata);
    package.distro = Some(distro.to_string());
    package.insert(conn).map_err(anyhow::Error::from)
}

fn supersede_active_native_identity(
    conn: &rusqlite::Connection,
    distro: &str,
    artifact: &VerifiedNativeArtifact,
) -> Result<Vec<SupersededNativePublication>> {
    let mut stmt = conn.prepare(
        "SELECT id, package_path, content_hash, target_path
         FROM native_package_publications
         WHERE status = 'public' AND distro = ?1 AND name = ?2 AND version = ?3
           AND package_release = ?4 AND architecture = ?5",
    )?;
    let rows = stmt.query_map(
        params![
            distro,
            artifact.name,
            artifact.version,
            artifact.package_release,
            artifact.architecture,
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                SupersededNativePublication {
                    package_path: PathBuf::from(row.get::<_, String>(1)?),
                    content_hash: row.get(2)?,
                    target_path: row.get(3)?,
                },
            ))
        },
    )?;

    let mut superseded = Vec::new();
    for row in rows {
        let (id, old) = row?;
        conn.execute(
            "UPDATE native_package_publications
             SET status = 'superseded',
                 superseded_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
             WHERE id = ?1",
            params![id],
        )?;
        superseded.push(old);
    }
    Ok(superseded)
}

fn delete_superseded_tuf_targets(
    conn: &rusqlite::Connection,
    repo_id: i64,
    superseded: &[SupersededNativePublication],
) -> Result<()> {
    for old in superseded {
        conn.execute(
            "DELETE FROM tuf_targets
             WHERE repository_id = ?1 AND (target_path = ?2 OR sha256 = ?3)",
            params![repo_id, old.target_path, old.content_hash],
        )?;
    }
    Ok(())
}

fn insert_native_publication(
    conn: &rusqlite::Connection,
    repo_id: i64,
    repo_package_id: i64,
    distro: &str,
    artifact: &VerifiedNativeArtifact,
    promoted: &PromotedNativeArtifact,
) -> Result<()> {
    let chunk_hashes_json = serde_json::to_string(std::slice::from_ref(&artifact.content_hash))?;
    let mut publication = NativePackagePublication {
        id: None,
        repository_id: repo_id,
        repository_package_id: repo_package_id,
        distro: distro.to_string(),
        name: artifact.name.clone(),
        version: artifact.version.clone(),
        package_release: artifact.package_release.clone(),
        architecture: artifact.architecture.clone(),
        package_kind: artifact.package_kind.clone(),
        authority_format_version: artifact.authority_format_version,
        status: NativePublicationStatus::Public,
        content_hash: artifact.content_hash.clone(),
        chunk_hashes_json,
        total_size: i64::try_from(artifact.total_size)
            .context("native artifact size exceeds i64")?,
        package_path: promoted.package_path.to_string_lossy().to_string(),
        target_path: promoted.target_path.clone(),
        trust_status: "verified".to_string(),
    };
    publication.insert(conn).map_err(anyhow::Error::from)?;
    Ok(())
}

fn refresh_release_tuf_metadata(
    conn: &rusqlite::Connection,
    keys_dir: &Path,
    distro: &str,
    repo_id: i64,
    artifact: &VerifiedNativeArtifact,
    promoted: &PromotedNativeArtifact,
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
            promoted.target_path,
            artifact.content_hash,
            i64::try_from(artifact.total_size).context("native artifact size exceeds i64")?,
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
