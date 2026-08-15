// apps/remi/src/server/r2_durability.rs

//! Typed local-versus-R2 durability inventory and missing-object backfill.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::r2::{R2ChunkObject, R2Store};

pub const R2_DURABILITY_SCHEMA_V1: u32 = 1;
const FAILURE_SAMPLE_LIMIT: usize = 10;
pub const DEFAULT_BACKFILL_CONCURRENCY: usize = 16;
pub const MAX_BACKFILL_CONCURRENCY: usize = 64;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum R2DurabilityMode {
    Plan,
    Apply,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChunkStoreInventory {
    pub objects: usize,
    pub bytes: u64,
    pub required_present: usize,
    pub required_missing: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum R2DurabilityOutcome {
    PlanReady,
    PlanBlocked,
    AppliedComplete,
    AppliedIncomplete,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct R2DurabilityFailure {
    pub hash: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct R2DurabilityReport {
    pub schema_version: u32,
    pub mode: R2DurabilityMode,
    pub outcome: R2DurabilityOutcome,
    pub r2_complete: bool,
    pub required_objects: usize,
    pub required_bytes: u64,
    pub local: ChunkStoreInventory,
    pub r2: ChunkStoreInventory,
    pub planned_uploads: usize,
    pub planned_upload_bytes: u64,
    pub attempted_uploads: usize,
    pub uploaded_objects: usize,
    pub uploaded_bytes: u64,
    pub failed_uploads: usize,
    pub missing_from_both: usize,
    pub missing_from_both_samples: Vec<String>,
    pub failure_samples: Vec<R2DurabilityFailure>,
}

#[async_trait]
pub trait DurableChunkStore: Send + Sync {
    async fn list_chunk_objects(&self) -> Result<Vec<R2ChunkObject>>;
    async fn put_chunk(&self, hash: &str, data: &[u8]) -> Result<()>;
}

#[async_trait]
impl DurableChunkStore for R2Store {
    async fn list_chunk_objects(&self) -> Result<Vec<R2ChunkObject>> {
        R2Store::list_chunk_objects(self).await
    }

    async fn put_chunk(&self, hash: &str, data: &[u8]) -> Result<()> {
        R2Store::put_chunk(self, hash, data).await
    }
}

#[derive(Debug)]
struct UploadOutcome {
    hash: String,
    bytes: u64,
    error: Option<String>,
}

/// Inventory R2 against local CAS and persisted package-object authority.
///
/// `Plan` is read-only. `Apply` uploads only objects absent from R2 (or whose
/// stored size disagrees with the verified local object), then lists R2 again
/// and derives completion from that post-apply state.
pub async fn run_r2_durability<S: DurableChunkStore + ?Sized + 'static>(
    db_path: &Path,
    objects_dir: &Path,
    store: Arc<S>,
    mode: R2DurabilityMode,
    concurrency: usize,
) -> Result<R2DurabilityReport> {
    if !(1..=MAX_BACKFILL_CONCURRENCY).contains(&concurrency) {
        bail!("R2 backfill concurrency must be between 1 and {MAX_BACKFILL_CONCURRENCY}");
    }

    let db_path = db_path.to_path_buf();
    let objects_dir = objects_dir.to_path_buf();
    let required = tokio::task::spawn_blocking(move || required_objects(&db_path))
        .await
        .context("join R2 required-object inventory task")??;
    let local_objects_dir = objects_dir.clone();
    let local = tokio::task::spawn_blocking(move || scan_local(&local_objects_dir))
        .await
        .context("join local chunk inventory task")??;
    let initial_r2 = map_r2_objects(store.list_chunk_objects().await?)?;
    let planned = planned_uploads(&local, &initial_r2);

    let mut attempted_uploads = 0usize;
    let mut uploaded_objects = 0usize;
    let mut uploaded_bytes = 0u64;
    let mut failure_samples = Vec::new();
    let mut failed_uploads = 0usize;

    if mode == R2DurabilityMode::Apply {
        attempted_uploads = planned.len();
        let outcomes = stream::iter(planned.keys().cloned())
            .map(|hash| {
                let store = Arc::clone(&store);
                let objects_dir = objects_dir.clone();
                async move { upload_verified_local(store, objects_dir, hash).await }
            })
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await;

        for outcome in outcomes {
            if let Some(error) = outcome.error {
                failed_uploads += 1;
                if failure_samples.len() < FAILURE_SAMPLE_LIMIT {
                    failure_samples.push(R2DurabilityFailure {
                        hash: outcome.hash,
                        error,
                    });
                }
            } else {
                uploaded_objects += 1;
                uploaded_bytes = uploaded_bytes.saturating_add(outcome.bytes);
            }
        }
    }

    let final_r2 = if mode == R2DurabilityMode::Apply {
        map_r2_objects(store.list_chunk_objects().await?)?
    } else {
        initial_r2
    };
    let missing_from_both = required
        .keys()
        .filter(|hash| !local.contains_key(*hash) && !final_r2.contains_key(*hash))
        .cloned()
        .collect::<Vec<_>>();
    let r2_complete = required
        .iter()
        .all(|(hash, expected_size)| final_r2.get(hash).is_some_and(|size| size == expected_size));
    let outcome = match (mode, r2_complete) {
        (R2DurabilityMode::Plan, true) => R2DurabilityOutcome::PlanReady,
        (R2DurabilityMode::Plan, false) => R2DurabilityOutcome::PlanBlocked,
        (R2DurabilityMode::Apply, true) => R2DurabilityOutcome::AppliedComplete,
        (R2DurabilityMode::Apply, false) => R2DurabilityOutcome::AppliedIncomplete,
    };

    Ok(R2DurabilityReport {
        schema_version: R2_DURABILITY_SCHEMA_V1,
        mode,
        outcome,
        r2_complete,
        required_objects: required.len(),
        required_bytes: required.values().copied().sum(),
        local: inventory(&local, &required),
        r2: inventory(&final_r2, &required),
        planned_uploads: planned.len(),
        planned_upload_bytes: planned.values().copied().sum(),
        attempted_uploads,
        uploaded_objects,
        uploaded_bytes,
        failed_uploads,
        missing_from_both: missing_from_both.len(),
        missing_from_both_samples: missing_from_both
            .into_iter()
            .take(FAILURE_SAMPLE_LIMIT)
            .collect(),
        failure_samples,
    })
}

fn inventory(
    actual: &BTreeMap<String, u64>,
    required: &BTreeMap<String, u64>,
) -> ChunkStoreInventory {
    let required_present = required
        .iter()
        .filter(|(hash, size)| {
            actual
                .get(*hash)
                .is_some_and(|actual_size| actual_size == *size)
        })
        .count();
    ChunkStoreInventory {
        objects: actual.len(),
        bytes: actual.values().copied().sum(),
        required_present,
        required_missing: required.len().saturating_sub(required_present),
    }
}

fn planned_uploads(
    local: &BTreeMap<String, u64>,
    r2: &BTreeMap<String, u64>,
) -> BTreeMap<String, u64> {
    local
        .iter()
        .filter(|(hash, size)| r2.get(*hash).is_none_or(|r2_size| r2_size != *size))
        .map(|(hash, size)| (hash.clone(), *size))
        .collect()
}

async fn upload_verified_local<S: DurableChunkStore + ?Sized>(
    store: Arc<S>,
    objects_dir: PathBuf,
    hash: String,
) -> UploadOutcome {
    let path = chunk_path(&objects_dir, &hash);
    let data = match tokio::fs::read(&path).await {
        Ok(data) => data,
        Err(error) => {
            return UploadOutcome {
                hash,
                bytes: 0,
                error: Some(format!("read local object: {error}")),
            };
        }
    };
    let bytes = data.len() as u64;
    let actual_hash = conary_core::hash::sha256(&data);
    if actual_hash != hash {
        return UploadOutcome {
            hash,
            bytes,
            error: Some(format!("local object digest is {actual_hash}")),
        };
    }
    let error = store
        .put_chunk(&hash, &data)
        .await
        .err()
        .map(|error| format!("R2 PUT failed: {error}"));
    UploadOutcome { hash, bytes, error }
}

fn map_r2_objects(objects: Vec<R2ChunkObject>) -> Result<BTreeMap<String, u64>> {
    let mut mapped = BTreeMap::new();
    for object in objects {
        validate_hash(&object.hash).context("invalid object under the R2 chunk prefix")?;
        if mapped
            .insert(object.hash.clone(), object.size_bytes)
            .is_some()
        {
            bail!("R2 inventory repeated chunk {}", object.hash);
        }
    }
    Ok(mapped)
}

fn scan_local(objects_dir: &Path) -> Result<BTreeMap<String, u64>> {
    let mut objects = BTreeMap::new();
    for hash in super::chunk_gc::scan_local_chunks(objects_dir)? {
        validate_hash(&hash).context("invalid object in the local chunk directory")?;
        let path = chunk_path(objects_dir, &hash);
        let size = std::fs::metadata(&path)
            .with_context(|| format!("stat local chunk {}", path.display()))?
            .len();
        if objects.insert(hash.clone(), size).is_some() {
            bail!("local inventory repeated chunk {hash}");
        }
    }
    Ok(objects)
}

fn required_objects(db_path: &Path) -> Result<BTreeMap<String, u64>> {
    let conn = crate::server::open_runtime_db(db_path)?;
    let mut required = BTreeMap::new();
    collect_transport_objects(
        &conn,
        "SELECT id, transport_json FROM converted_packages WHERE transport_json IS NOT NULL",
        "converted package",
        &mut required,
    )?;
    collect_transport_objects(
        &conn,
        "SELECT id, transport_json FROM native_package_publications \
         WHERE status = 'public' AND transport_json IS NOT NULL",
        "native package publication",
        &mut required,
    )?;
    Ok(required)
}

fn collect_transport_objects(
    conn: &Connection,
    sql: &str,
    owner: &str,
    required: &mut BTreeMap<String, u64>,
) -> Result<()> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, transport_json) = row?;
        let transport =
            serde_json::from_str::<conary_core::ccs::CcsTransportEnvelopeV1>(&transport_json)
                .with_context(|| format!("{owner} {id} has malformed transport authority"))?;
        for object in transport.objects {
            validate_hash(&object.sha256)
                .with_context(|| format!("{owner} {id} has invalid object identity"))?;
            if let Some(previous) = required.insert(object.sha256.clone(), object.size)
                && previous != object.size
            {
                bail!(
                    "{owner} {id} gives chunk {} size {}, contradicting prior size {previous}",
                    object.sha256,
                    object.size
                );
            }
        }
    }
    Ok(())
}

fn validate_hash(hash: &str) -> Result<()> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("chunk hash must be exactly 64 lowercase hexadecimal characters");
    }
    Ok(())
}

fn chunk_path(objects_dir: &Path, hash: &str) -> PathBuf {
    let (prefix, rest) = hash.split_at(2);
    objects_dir.join(prefix).join(rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::ccs::{CcsTransportEnvelopeV1, CcsTransportObjectV1};
    use conary_core::db::models::{ConvertedPackage, EMPTY_REPOSITORY_PROVIDES_DIGEST};
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeStore {
        objects: Mutex<BTreeMap<String, Vec<u8>>>,
        puts: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl DurableChunkStore for FakeStore {
        async fn list_chunk_objects(&self) -> Result<Vec<R2ChunkObject>> {
            Ok(self
                .objects
                .lock()
                .unwrap()
                .iter()
                .map(|(hash, data)| R2ChunkObject {
                    hash: hash.clone(),
                    size_bytes: data.len() as u64,
                })
                .collect())
        }

        async fn put_chunk(&self, hash: &str, data: &[u8]) -> Result<()> {
            self.puts.lock().unwrap().push(hash.to_string());
            self.objects
                .lock()
                .unwrap()
                .insert(hash.to_string(), data.to_vec());
            Ok(())
        }
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, String, Vec<u8>) {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("remi.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();
        let data = b"durable chunk".to_vec();
        let hash = conary_core::hash::sha256(&data);
        let transport = CcsTransportEnvelopeV1 {
            schema_version: conary_core::ccs::transport::CCS_TRANSPORT_SCHEMA_V1,
            manifest_base64: String::new(),
            signature_json: "{}".to_string(),
            debug_toml_base64: None,
            build_attestation_json: None,
            foreign_conversion_boundary_json: None,
            objects: vec![CcsTransportObjectV1 {
                sha256: hash.clone(),
                size: data.len() as u64,
            }],
        };
        let mut converted = ConvertedPackage::new_repository(
            "fedora-44".to_string(),
            "durability-fixture".to_string(),
            "1-1".to_string(),
            "x86_64".to_string(),
            "rpm".to_string(),
            "sha256:fixture".to_string(),
            &transport,
            data.len() as i64,
            "sha256:transport".to_string(),
            "/tmp/fixture.ccs".to_string(),
            EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
        );
        converted.insert(&conn).unwrap();

        let objects_dir = temp.path().join("chunks/objects");
        let path = chunk_path(&objects_dir, &hash);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, &data).unwrap();
        (temp, db_path, objects_dir, hash, data)
    }

    #[tokio::test]
    async fn plan_reports_exact_gap_without_writing() {
        let (_temp, db_path, objects_dir, hash, data) = fixture();
        let store = Arc::new(FakeStore::default());

        let report = run_r2_durability(
            &db_path,
            &objects_dir,
            Arc::clone(&store),
            R2DurabilityMode::Plan,
            2,
        )
        .await
        .unwrap();

        assert_eq!(report.outcome, R2DurabilityOutcome::PlanBlocked);
        assert_eq!(report.planned_uploads, 1);
        assert_eq!(report.planned_upload_bytes, data.len() as u64);
        assert_eq!(report.missing_from_both, 0);
        assert!(store.puts.lock().unwrap().is_empty());
        assert_eq!(report.local.required_present, 1);
        assert_eq!(report.r2.required_missing, 1);
        assert_eq!(report.missing_from_both_samples, Vec::<String>::new());
        assert_eq!(hash.len(), 64);
    }

    #[tokio::test]
    async fn apply_uploads_verified_bytes_and_rechecks_r2() {
        let (_temp, db_path, objects_dir, hash, data) = fixture();
        let store = Arc::new(FakeStore::default());

        let report = run_r2_durability(
            &db_path,
            &objects_dir,
            Arc::clone(&store),
            R2DurabilityMode::Apply,
            2,
        )
        .await
        .unwrap();

        assert_eq!(report.outcome, R2DurabilityOutcome::AppliedComplete);
        assert!(report.r2_complete);
        assert_eq!(report.attempted_uploads, 1);
        assert_eq!(report.uploaded_objects, 1);
        assert_eq!(report.uploaded_bytes, data.len() as u64);
        assert_eq!(store.puts.lock().unwrap().as_slice(), &[hash]);
        assert_eq!(report.r2.required_missing, 0);
    }

    #[tokio::test]
    async fn apply_refuses_corrupt_local_object() {
        let (_temp, db_path, objects_dir, hash, _data) = fixture();
        std::fs::write(chunk_path(&objects_dir, &hash), b"corrupt").unwrap();
        let store = Arc::new(FakeStore::default());

        let report = run_r2_durability(
            &db_path,
            &objects_dir,
            Arc::clone(&store),
            R2DurabilityMode::Apply,
            2,
        )
        .await
        .unwrap();

        assert_eq!(report.outcome, R2DurabilityOutcome::AppliedIncomplete);
        assert!(!report.r2_complete);
        assert_eq!(report.failed_uploads, 1);
        assert_eq!(report.uploaded_objects, 0);
        assert!(report.failure_samples[0].error.contains("digest"));
        assert!(store.puts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn plan_reports_required_object_missing_from_both_stores() {
        let (_temp, db_path, objects_dir, hash, _data) = fixture();
        std::fs::remove_file(chunk_path(&objects_dir, &hash)).unwrap();
        let store = Arc::new(FakeStore::default());

        let report = run_r2_durability(&db_path, &objects_dir, store, R2DurabilityMode::Plan, 2)
            .await
            .unwrap();

        assert_eq!(report.outcome, R2DurabilityOutcome::PlanBlocked);
        assert_eq!(report.planned_uploads, 0);
        assert_eq!(report.missing_from_both, 1);
        assert_eq!(report.missing_from_both_samples, vec![hash]);
    }
}
