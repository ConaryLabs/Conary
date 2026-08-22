// crates/conary-core/src/repository/universe/client.rs

//! Verified transfer and fenced activation of one complete client universe.

use std::collections::BTreeMap;
use std::fs;
use std::io::BufReader;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::error::{Error, Result};
use crate::repository::client::RepositoryClient;
use crate::trust::client::{
    TufClient, TufUpdateSnapshot, TufUpdateState, metadata_hash_for_persistence,
};
use crate::trust::{RootMetadata, Signed, SnapshotMetadata, TargetsMetadata, TimestampMetadata};

use super::{
    ClientUniverseIndex, RemiUniverseManifestV1, build_client_universe_index,
    normalize_remi_endpoint, verify_remi_universe_manifest_target,
};

const MAX_UNIVERSE_MANIFEST_SIZE: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemiUniverseSyncOutcome {
    Unchanged {
        manifest_sha256: String,
        sequence: u64,
        package_count: u64,
    },
    Activated {
        manifest_sha256: String,
        sequence: u64,
        package_count: u64,
        downloaded_objects: usize,
        reused_objects: usize,
    },
}

struct ClientSyncState {
    endpoint: String,
    fencing_epoch: i64,
    active_manifest_sha256: Option<String>,
    active_sequence: Option<u64>,
    active_package_count: Option<u64>,
    tuf: TufUpdateState,
}

struct VerifiedCandidate {
    manifest: RemiUniverseManifestV1,
    manifest_sha256: String,
    manifest_json: String,
    tuf: TufUpdateSnapshot,
    objects: BTreeMap<String, DownloadedObject>,
    index: ClientUniverseIndex,
    downloaded_objects: usize,
    reused_objects: usize,
}

#[derive(Debug, Clone)]
struct DownloadedObject {
    path: PathBuf,
    size: u64,
    kind: &'static str,
}

pub async fn sync_remi_universe(db_path: &Path, endpoint: &str) -> Result<RemiUniverseSyncOutcome> {
    let endpoint = normalize_remi_endpoint(endpoint)?;
    let state = {
        let conn = crate::db::open_fast(db_path)?;
        begin_sync(&conn, &endpoint)?
    };
    let tuf_client = TufClient::new_static(
        0,
        &state.endpoint,
        Some(&format!("{}/v1/universe/tuf", state.endpoint)),
    )
    .map_err(|error| Error::TrustError(error.to_string()))?;
    let tuf = tuf_client
        .fetch_update_snapshot(state.tuf.clone())
        .await
        .map_err(|error| Error::TrustError(error.to_string()))?;
    let client = RepositoryClient::new()?;
    let (manifest, manifest_sha256, manifest_json) =
        fetch_manifest(&client, &state.endpoint, &tuf).await?;
    reject_rollback_or_fork(&state, &manifest, &manifest_sha256)?;

    if state.active_manifest_sha256.as_deref() == Some(manifest_sha256.as_str()) {
        let package_count = state.active_package_count.ok_or_else(|| {
            Error::ConflictError("active universe has no attached index package count".to_string())
        })?;
        let conn = crate::db::open_fast(db_path)?;
        persist_unchanged_metadata(&conn, &state, &manifest, &tuf)?;
        return Ok(RemiUniverseSyncOutcome::Unchanged {
            manifest_sha256,
            sequence: manifest.sequence,
            package_count,
        });
    }

    let roots = client_storage_roots(db_path)?;
    let (objects, downloaded_objects, reused_objects) = fetch_objects(
        &client,
        &state.endpoint,
        state.fencing_epoch,
        &manifest,
        &tuf,
        &roots.objects,
    )
    .await?;
    let canonical_object = objects
        .get(&manifest.canonical_map.sha256)
        .ok_or_else(|| Error::NotFound("verified universe omits its canonical map".to_string()))?;
    let catalog_objects = objects
        .iter()
        .filter(|(_, object)| object.kind == "catalog")
        .map(|(sha256, object)| (sha256.clone(), object.path.clone()))
        .collect::<BTreeMap<_, _>>();
    let operational = crate::db::open_fast(db_path)?;
    let index = build_client_universe_index(
        &operational,
        &manifest,
        &canonical_object.path,
        &catalog_objects,
        &roots.indices,
    )?;
    drop(operational);
    let candidate = VerifiedCandidate {
        manifest,
        manifest_sha256,
        manifest_json,
        tuf,
        objects,
        index,
        downloaded_objects,
        reused_objects,
    };
    let conn = crate::db::open_fast(db_path)?;
    activate_candidate(&conn, &state, &candidate)?;
    collect_unreachable_files(&conn, &roots, &candidate.manifest)?;
    Ok(RemiUniverseSyncOutcome::Activated {
        manifest_sha256: candidate.manifest_sha256,
        sequence: candidate.manifest.sequence,
        package_count: candidate.index.package_count,
        downloaded_objects: candidate.downloaded_objects,
        reused_objects: candidate.reused_objects,
    })
}

fn begin_sync(conn: &Connection, endpoint: &str) -> Result<ClientSyncState> {
    require_single_enabled_endpoint(conn, endpoint)?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let row = tx
        .query_row(
            "SELECT trusted_root_json, root_version, fencing_epoch,
                    timestamp_json, snapshot_json, targets_json
             FROM remi_client_universe_trust WHERE endpoint = ?1",
            [endpoint],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            Error::TrustError(format!(
                "Remi endpoint {endpoint} has no enrolled universe metadata root"
            ))
        })?;
    let (root_json, root_version, prior_fence, timestamp_json, snapshot_json, targets_json) = row;
    let trusted_root: Signed<RootMetadata> = serde_json::from_str(&root_json)?;
    if i64::try_from(trusted_root.signed.version).ok() != Some(root_version) {
        return Err(Error::ConflictError(
            "enrolled universe root version disagrees with its metadata".to_string(),
        ));
    }
    let timestamp = parse_optional::<TimestampMetadata>(timestamp_json.as_deref())?;
    let snapshot = parse_optional::<SnapshotMetadata>(snapshot_json.as_deref())?;
    let targets = parse_optional::<TargetsMetadata>(targets_json.as_deref())?;
    if timestamp.is_some() != snapshot.is_some() || snapshot.is_some() != targets.is_some() {
        return Err(Error::ConflictError(
            "stored universe TUF roles are incomplete".to_string(),
        ));
    }
    let fencing_epoch = prior_fence.checked_add(1).ok_or_else(|| {
        Error::InternalError("client universe fencing epoch overflow".to_string())
    })?;
    tx.execute(
        "UPDATE remi_client_universe_trust SET fencing_epoch = ?1 WHERE endpoint = ?2",
        params![fencing_epoch, endpoint],
    )?;
    // The package count belongs to the attached private index and cannot be
    // joined from operational SQLite. Read the active identity here and the
    // count through the connection-local authority view below.
    let active_identity = tx
        .query_row(
            "SELECT manifest_sha256, sequence FROM remi_active_client_universe
             WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    let active_package_count = if active_identity.is_some() {
        tx.query_row(
            "SELECT COUNT(*) FROM resolved_repository_packages package
             JOIN repositories repository ON repository.id = package.repository_id
             WHERE repository.default_strategy = 'remi'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .map(|count| {
            u64::try_from(count).map_err(|_| {
                Error::ConflictError("active universe package count is negative".to_string())
            })
        })
        .transpose()?
    } else {
        None
    };
    let (active_manifest_sha256, active_sequence) = match active_identity {
        Some((sha256, sequence)) => (
            Some(sha256),
            Some(u64::try_from(sequence).map_err(|_| {
                Error::ConflictError("active universe sequence is negative".to_string())
            })?),
        ),
        None => (None, None),
    };
    tx.commit()?;
    let stored_timestamp_hash = timestamp
        .as_ref()
        .map(metadata_hash_for_persistence)
        .transpose()
        .map_err(|error| Error::TrustError(error.to_string()))?;
    Ok(ClientSyncState {
        endpoint: endpoint.to_string(),
        fencing_epoch,
        active_manifest_sha256,
        active_sequence,
        active_package_count,
        tuf: TufUpdateState {
            trusted_root,
            stored_timestamp_version: timestamp.as_ref().map(|role| role.signed.version),
            stored_timestamp_hash,
            stored_snapshot_version: snapshot.as_ref().map(|role| role.signed.version),
            stored_targets_version: targets.as_ref().map(|role| role.signed.version),
            stored_snapshot: snapshot,
            stored_targets: targets,
        },
    })
}

fn parse_optional<T>(json: Option<&str>) -> Result<Option<Signed<T>>>
where
    T: serde::de::DeserializeOwned,
{
    json.map(serde_json::from_str)
        .transpose()
        .map_err(Into::into)
}

async fn fetch_manifest(
    client: &RepositoryClient,
    endpoint: &str,
    tuf: &TufUpdateSnapshot,
) -> Result<(RemiUniverseManifestV1, String, String)> {
    let manifest_paths = tuf
        .signed_targets
        .signed
        .targets
        .keys()
        .filter(|path| path.starts_with("universe/") && path.ends_with(".json"))
        .cloned()
        .collect::<Vec<_>>();
    let [manifest_path] = manifest_paths.as_slice() else {
        return Err(Error::TrustError(format!(
            "verified universe targets contain {} manifest paths instead of one",
            manifest_paths.len()
        )));
    };
    let target = tuf
        .signed_targets
        .signed
        .targets
        .get(manifest_path)
        .ok_or_else(|| {
            Error::TrustError("verified universe manifest target vanished".to_string())
        })?;
    if target.length > MAX_UNIVERSE_MANIFEST_SIZE {
        return Err(Error::TrustError(format!(
            "universe manifest is {} bytes; limit is {MAX_UNIVERSE_MANIFEST_SIZE}",
            target.length
        )));
    }
    let url = format!("{endpoint}/v1/universe/targets/{manifest_path}");
    let bytes = client
        .download_to_bytes_with_limit(&url, target.length)
        .await?;
    let verified =
        verify_remi_universe_manifest_target(&bytes, &tuf.signed_targets.signed.targets)?;
    if verified.manifest.expires_at <= chrono::Utc::now() {
        return Err(Error::TrustError(format!(
            "Remi universe manifest {} expired at {}",
            verified.manifest_sha256, verified.manifest.expires_at
        )));
    }
    let root_bytes = crate::json::canonical_json(&tuf.current_root).map_err(Error::ParseError)?;
    if crate::hash::sha256(&root_bytes) != verified.manifest.metadata_root_sha256 {
        return Err(Error::TrustError(
            "universe manifest metadata-root digest disagrees with verified TUF root".to_string(),
        ));
    }
    let manifest_json = String::from_utf8(bytes)
        .map_err(|error| Error::ParseError(format!("universe manifest is not UTF-8: {error}")))?;
    Ok((verified.manifest, verified.manifest_sha256, manifest_json))
}

fn reject_rollback_or_fork(
    state: &ClientSyncState,
    manifest: &RemiUniverseManifestV1,
    manifest_sha256: &str,
) -> Result<()> {
    if let Some(active_sequence) = state.active_sequence {
        if manifest.sequence < active_sequence {
            return Err(Error::TrustError(format!(
                "Remi universe sequence {} rolls back active sequence {active_sequence}",
                manifest.sequence
            )));
        }
        if manifest.sequence == active_sequence
            && state.active_manifest_sha256.as_deref() != Some(manifest_sha256)
        {
            return Err(Error::TrustError(format!(
                "Remi universe sequence {} forks the active manifest",
                manifest.sequence
            )));
        }
    }
    Ok(())
}

struct StorageRoots {
    objects: PathBuf,
    indices: PathBuf,
}

fn client_storage_roots(db_path: &Path) -> Result<StorageRoots> {
    let database = db_path.canonicalize()?;
    let parent = database
        .parent()
        .ok_or_else(|| Error::InvalidPath("database has no parent directory".to_string()))?;
    let root = ensure_private_directory(&parent.join("remi-universes"))?;
    let object_parent = ensure_private_directory(&root.join("objects"))?;
    let objects = ensure_private_directory(&object_parent.join("sha256"))?;
    let indices = ensure_private_directory(&root.join("indices"))?;
    Ok(StorageRoots { objects, indices })
}

fn ensure_private_directory(path: &Path) -> Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                return Err(Error::InvalidPath(format!(
                    "client universe path {} must be a real directory",
                    path.display()
                )));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(path)?,
        Err(error) => return Err(error.into()),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(path.canonicalize()?)
}

async fn fetch_objects(
    client: &RepositoryClient,
    endpoint: &str,
    fencing_epoch: i64,
    manifest: &RemiUniverseManifestV1,
    tuf: &TufUpdateSnapshot,
    objects_root: &Path,
) -> Result<(BTreeMap<String, DownloadedObject>, usize, usize)> {
    let descriptors = manifest
        .profiles
        .iter()
        .map(|profile| {
            (
                profile.catalog.sha256.clone(),
                profile.catalog.size,
                "catalog",
            )
        })
        .chain(std::iter::once((
            manifest.canonical_map.sha256.clone(),
            manifest.canonical_map.size,
            "canonical_map",
        )))
        .collect::<Vec<_>>();
    let mut objects = BTreeMap::new();
    let mut downloaded = 0;
    let mut reused = 0;
    for (sha256, size, kind) in descriptors {
        let target_path = format!("objects/sha256/{sha256}");
        let target = tuf
            .signed_targets
            .signed
            .targets
            .get(&target_path)
            .ok_or_else(|| Error::TrustError(format!("TUF omits object {target_path}")))?;
        if target.length != size
            || target.hashes.len() != 1
            || target.hashes.get("sha256").map(String::as_str) != Some(sha256.as_str())
        {
            return Err(Error::TrustError(format!(
                "TUF object {target_path} disagrees with manifest authority"
            )));
        }
        let path = objects_root.join(&sha256);
        if verify_file_identity(&path, &sha256, size).is_ok() {
            reused += 1;
        } else {
            let url = format!("{endpoint}/v1/universe/targets/{target_path}");
            let candidate_path =
                objects_root.join(format!(".candidate-{fencing_epoch}-{sha256}.download"));
            let identity = match client
                .download_file_with_identity_limit(&url, &candidate_path, size)
                .await
            {
                Ok(identity) => identity,
                Err(error) => {
                    let _ = fs::remove_file(&candidate_path);
                    let _ = fs::remove_file(candidate_path.with_extension("tmp"));
                    return Err(error);
                }
            };
            if identity.sha256 != sha256 || identity.size != size {
                let _ = fs::remove_file(&candidate_path);
                return Err(Error::ChecksumMismatch {
                    expected: format!("{sha256}:{size}"),
                    actual: format!("{}:{}", identity.sha256, identity.size),
                });
            }
            fs::set_permissions(&candidate_path, fs::Permissions::from_mode(0o400))?;
            fs::rename(&candidate_path, &path)?;
            fs::File::open(objects_root)?.sync_all()?;
            downloaded += 1;
        }
        objects.insert(sha256, DownloadedObject { path, size, kind });
    }
    Ok((objects, downloaded, reused))
}

fn verify_file_identity(path: &Path, expected_sha256: &str, expected_size: u64) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(Error::InvalidPath(format!(
            "universe object {} must be a regular file",
            path.display()
        )));
    }
    if metadata.permissions().mode() & 0o277 != 0 {
        return Err(Error::InvalidPath(format!(
            "universe object {} must be immutable and private",
            path.display()
        )));
    }
    if metadata.len() != expected_size {
        return Err(Error::ChecksumMismatch {
            expected: format!("{expected_size} bytes"),
            actual: format!("{} bytes", metadata.len()),
        });
    }
    let mut reader = BufReader::new(fs::File::open(path)?);
    let actual = crate::hash::sha256_reader_hex(&mut reader)?;
    if actual != expected_sha256 {
        return Err(Error::ChecksumMismatch {
            expected: expected_sha256.to_string(),
            actual,
        });
    }
    Ok(())
}

fn persist_unchanged_metadata(
    conn: &Connection,
    state: &ClientSyncState,
    manifest: &RemiUniverseManifestV1,
    tuf: &TufUpdateSnapshot,
) -> Result<()> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    require_current_fence(&tx, state)?;
    let active = tx.query_row(
        "SELECT manifest_sha256, sequence FROM remi_active_client_universe WHERE singleton = 1",
        [],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )?;
    if active
        != (
            manifest.manifest_sha256()?,
            checked_i64(manifest.sequence, "universe sequence")?,
        )
    {
        return Err(Error::ConflictError(
            "active universe changed during unchanged metadata refresh".to_string(),
        ));
    }
    persist_tuf_state(&tx, state, tuf)?;
    mark_endpoint_checked(&tx, &state.endpoint, false)?;
    tx.commit()?;
    Ok(())
}

fn activate_candidate(
    conn: &Connection,
    state: &ClientSyncState,
    candidate: &VerifiedCandidate,
) -> Result<()> {
    verify_file_identity(
        &candidate.index.path,
        &candidate.index.sha256,
        candidate.index.size,
    )?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    require_current_fence(&tx, state)?;
    let current = tx
        .query_row(
            "SELECT endpoint, manifest_sha256, sequence FROM remi_active_client_universe
             WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    let expected = state.active_manifest_sha256.as_ref().map(|sha256| {
        (
            state.endpoint.as_str(),
            sha256.as_str(),
            checked_i64(
                state.active_sequence.expect("manifest and sequence pair"),
                "sequence",
            )
            .expect("stored sequence already fit SQLite"),
        )
    });
    if current
        .as_ref()
        .map(|(endpoint, sha256, sequence)| (endpoint.as_str(), sha256.as_str(), *sequence))
        != expected
    {
        return Err(Error::ConflictError(
            "active universe changed while a replacement was being built".to_string(),
        ));
    }
    require_single_enabled_endpoint(&tx, &state.endpoint)?;
    for (sha256, object) in &candidate.objects {
        verify_file_identity(&object.path, sha256, object.size)?;
        tx.execute(
            "INSERT OR IGNORE INTO remi_client_universe_objects (
                 endpoint, sha256, size, object_kind, local_path, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &state.endpoint,
                sha256,
                checked_i64(object.size, "object size")?,
                object.kind,
                object.path.to_string_lossy(),
                candidate.manifest.generated_at.timestamp(),
            ],
        )?;
        let stored = tx.query_row(
            "SELECT size, object_kind, local_path
             FROM remi_client_universe_objects
             WHERE endpoint = ?1 AND sha256 = ?2",
            params![&state.endpoint, sha256],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;
        let expected = (
            checked_i64(object.size, "object size")?,
            object.kind.to_string(),
            object.path.to_string_lossy().into_owned(),
        );
        if stored != expected {
            return Err(Error::ConflictError(format!(
                "stored universe object {sha256} disagrees with verified immutable identity"
            )));
        }
    }
    tx.execute(
        "INSERT INTO remi_client_universe_revisions (
             endpoint, manifest_sha256, sequence, manifest_json, index_sha256,
             index_size, index_path, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            &state.endpoint,
            &candidate.manifest_sha256,
            checked_i64(candidate.manifest.sequence, "universe sequence")?,
            &candidate.manifest_json,
            &candidate.index.sha256,
            checked_i64(candidate.index.size, "index size")?,
            candidate.index.path.to_string_lossy(),
            candidate.manifest.generated_at.timestamp(),
        ],
    )?;
    tx.execute(
        "INSERT INTO remi_active_client_universe (
             singleton, endpoint, manifest_sha256, sequence, fencing_epoch, activated_at
         ) VALUES (1, ?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(singleton) DO UPDATE SET
             endpoint = excluded.endpoint,
             manifest_sha256 = excluded.manifest_sha256,
             sequence = excluded.sequence,
             fencing_epoch = excluded.fencing_epoch,
             activated_at = excluded.activated_at",
        params![
            &state.endpoint,
            &candidate.manifest_sha256,
            checked_i64(candidate.manifest.sequence, "universe sequence")?,
            state.fencing_epoch,
            chrono::Utc::now().timestamp(),
        ],
    )?;
    delete_retired_mutable_remi_authority(&tx, &state.endpoint)?;
    persist_tuf_state(&tx, state, &candidate.tuf)?;
    mark_endpoint_checked(&tx, &state.endpoint, true)?;
    tx.commit()?;
    Ok(())
}

fn require_current_fence(tx: &Transaction<'_>, state: &ClientSyncState) -> Result<()> {
    let fence = tx.query_row(
        "SELECT fencing_epoch FROM remi_client_universe_trust WHERE endpoint = ?1",
        [&state.endpoint],
        |row| row.get::<_, i64>(0),
    )?;
    if fence != state.fencing_epoch {
        return Err(Error::ConflictError(format!(
            "Remi universe sync lost fencing epoch {}",
            state.fencing_epoch
        )));
    }
    Ok(())
}

fn persist_tuf_state(
    tx: &Transaction<'_>,
    state: &ClientSyncState,
    tuf: &TufUpdateSnapshot,
) -> Result<()> {
    let root_json = canonical_json_string(&tuf.current_root, "root")?;
    let root_sha256 = crate::hash::sha256(root_json.as_bytes());
    let updated = tx.execute(
        "UPDATE remi_client_universe_trust
         SET trusted_root_sha256 = ?1, trusted_root_json = ?2, root_version = ?3,
             timestamp_json = ?4, snapshot_json = ?5, targets_json = ?6
         WHERE endpoint = ?7 AND fencing_epoch = ?8",
        params![
            root_sha256,
            root_json,
            checked_i64(tuf.current_root.signed.version, "root version")?,
            canonical_json_string(&tuf.signed_timestamp, "timestamp")?,
            canonical_json_string(&tuf.signed_snapshot, "snapshot")?,
            canonical_json_string(&tuf.signed_targets, "targets")?,
            &state.endpoint,
            state.fencing_epoch,
        ],
    )?;
    if updated != 1 {
        return Err(Error::ConflictError(
            "universe TUF fence became stale".to_string(),
        ));
    }
    Ok(())
}

fn canonical_json_string(value: &impl serde::Serialize, label: &str) -> Result<String> {
    String::from_utf8(crate::json::canonical_json(value).map_err(Error::ParseError)?)
        .map_err(|error| Error::ParseError(format!("universe {label} is not UTF-8: {error}")))
}

fn require_single_enabled_endpoint(conn: &Connection, endpoint: &str) -> Result<()> {
    let endpoints = conn
        .prepare(
            "SELECT default_strategy_endpoint FROM repositories
             WHERE enabled = 1 AND default_strategy = 'remi'
               AND default_strategy_endpoint IS NOT NULL
             GROUP BY default_strategy_endpoint ORDER BY default_strategy_endpoint",
        )?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if endpoints != [endpoint] {
        return Err(Error::ConflictError(format!(
            "enabled Remi repositories must name exactly enrolled endpoint {endpoint}; found {endpoints:?}"
        )));
    }
    Ok(())
}

fn mark_endpoint_checked(conn: &Connection, endpoint: &str, changed: bool) -> Result<()> {
    let timestamp = crate::repository::current_timestamp();
    conn.execute(
        "UPDATE repositories
         SET last_checked_at = ?1,
             last_validated_at = ?1,
             last_changed_at = CASE WHEN ?2 THEN ?1 ELSE last_changed_at END,
             last_published_at = CASE WHEN ?2 THEN ?1 ELSE last_published_at END
         WHERE enabled = 1 AND default_strategy = 'remi'
           AND default_strategy_endpoint = ?3",
        params![timestamp, changed, endpoint],
    )?;
    Ok(())
}

fn delete_retired_mutable_remi_authority(conn: &Connection, endpoint: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM repository_packages
         WHERE repository_id IN (
             SELECT id FROM repositories
             WHERE default_strategy = 'remi' AND default_strategy_endpoint = ?1
         )",
        [endpoint],
    )?;
    conn.execute(
        "DELETE FROM package_implementations WHERE source = 'remi'",
        [],
    )?;
    conn.execute(
        "DELETE FROM canonical_packages
         WHERE NOT EXISTS (
             SELECT 1 FROM package_implementations
             WHERE canonical_id = canonical_packages.id
         )",
        [],
    )?;
    Ok(())
}

fn collect_unreachable_files(
    conn: &Connection,
    roots: &StorageRoots,
    active: &RemiUniverseManifestV1,
) -> Result<()> {
    let reachable = active
        .profiles
        .iter()
        .map(|profile| profile.catalog.sha256.as_str())
        .chain(std::iter::once(active.canonical_map.sha256.as_str()))
        .collect::<std::collections::BTreeSet<_>>();
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let (active_manifest_sha256, active_index) = tx.query_row(
        "SELECT active.manifest_sha256, revision.index_path
         FROM remi_active_client_universe active
         JOIN remi_client_universe_revisions revision
           ON revision.endpoint = active.endpoint
          AND revision.manifest_sha256 = active.manifest_sha256
         WHERE active.singleton = 1",
        [],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    if active_manifest_sha256 != active.manifest_sha256()? {
        tx.commit()?;
        return Ok(());
    }
    let retired_indices = tx
        .prepare(
            "SELECT revision.index_path
             FROM remi_client_universe_revisions revision
             WHERE NOT EXISTS (
                 SELECT 1 FROM remi_active_client_universe active
                 WHERE active.endpoint = revision.endpoint
                   AND active.manifest_sha256 = revision.manifest_sha256
             ) ORDER BY revision.endpoint, revision.sequence",
        )?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for path in &retired_indices {
        require_gc_path(Path::new(path), &roots.indices, "universe index")?;
    }
    tx.execute(
        "DELETE FROM remi_client_universe_revisions
         WHERE NOT EXISTS (
             SELECT 1 FROM remi_active_client_universe active
             WHERE active.endpoint = remi_client_universe_revisions.endpoint
               AND active.manifest_sha256 = remi_client_universe_revisions.manifest_sha256
         )",
        [],
    )?;
    let mut statement = tx.prepare(
        "SELECT endpoint, sha256, local_path FROM remi_client_universe_objects
         ORDER BY endpoint, sha256",
    )?;
    let objects = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut retired_objects = Vec::new();
    for (endpoint, sha256, path) in objects {
        if reachable.contains(sha256.as_str()) {
            continue;
        }
        let path = PathBuf::from(path);
        require_gc_path(&path, &roots.objects, "universe object")?;
        tx.execute(
            "DELETE FROM remi_client_universe_objects WHERE endpoint = ?1 AND sha256 = ?2",
            params![endpoint, sha256],
        )?;
        retired_objects.push(path);
    }
    drop(statement);
    tx.commit()?;
    for path in retired_objects
        .into_iter()
        .chain(retired_indices.into_iter().map(PathBuf::from))
    {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    for entry in fs::read_dir(&roots.indices)? {
        let path = entry?.path();
        if path.to_string_lossy() != active_index {
            require_gc_path(&path, &roots.indices, "orphan universe index")?;
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

fn require_gc_path(path: &Path, root: &Path, label: &str) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        Error::InvalidPath(format!("{label} path {} has no parent", path.display()))
    })?;
    if parent != root || path.file_name().is_none() {
        return Err(Error::InvalidPath(format!(
            "{label} path {} is outside {}",
            path.display(),
            root.display()
        )));
    }
    Ok(())
}

fn checked_i64(value: u64, label: &str) -> Result<i64> {
    i64::try_from(value)
        .map_err(|_| Error::ConfigError(format!("universe {label} exceeds SQLite integer range")))
}

#[cfg(test)]
#[path = "client/tests.rs"]
mod tests;
