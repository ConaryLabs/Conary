// conary-core/src/db/generation_backup_chain.rs

//! Self-contained, bounded SQLite base-plus-changeset recovery authority.

use crate::db::generation_delta::{GenerationDbBaselineToken, GenerationDbDelta};
use crate::db::generation_snapshot::{
    GenerationDbSnapshotProvenance, GenerationDbSnapshotProvider,
};
use crate::filesystem::durable::{
    sync_parent_directory, write_file_atomic_with_mode, write_json_atomic_with_mode,
};
use crate::hash::sha256_reader_hex;
use crate::{Error, Result};
use chrono::Utc;
use rusqlite::session::ConflictAction;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub const GENERATION_DB_CHAIN_FORMAT: &str = "conary.generation-db-backup.v3";
pub const GENERATION_DB_CHAIN_MANIFEST_VERSION: u32 = 3;
pub const MAX_GENERATION_DB_DELTAS: usize = 32;
pub const GENERATION_DB_CHAIN_MANIFEST_FILE: &str = "conary-db-backup.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenerationDbBackupIdentity {
    pub generation_number: i64,
    pub state_number: i64,
    pub transaction_high_water_mark: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationDbBaseArtifact {
    pub file: String,
    pub sha256: String,
    pub sqlite_page_count: i64,
    pub logical_size: u64,
    pub snapshot: GenerationDbSnapshotProvenance,
    pub result_baseline: GenerationDbBaselineToken,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationDbDeltaArtifact {
    pub file: String,
    pub sha256: String,
    pub payload_bytes: u64,
    pub source_baseline: GenerationDbBaselineToken,
    pub result_baseline: GenerationDbBaselineToken,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationDbChainManifest {
    pub format: String,
    pub manifest_version: u32,
    pub schema_epoch: String,
    pub db_schema_version: i32,
    pub generation_number: i64,
    pub state_number: i64,
    pub created_at: String,
    pub transaction_high_water_mark: Option<i64>,
    pub base: GenerationDbBaseArtifact,
    pub deltas: Vec<GenerationDbDeltaArtifact>,
    pub final_baseline: GenerationDbBaselineToken,
    pub chain_sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenerationDbChainMetrics {
    pub payload_bytes_written: u64,
    pub linked_artifacts: usize,
    pub provider_elapsed_ns: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerationDbChainRecord {
    pub manifest_path: PathBuf,
    pub manifest: GenerationDbChainManifest,
    pub metrics: GenerationDbChainMetrics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenerationDbChainFallbackReason {
    ChainLimitReached,
    EmptyDelta,
    SourceBaselineChanged,
    SchemaChanged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GenerationDbChainAppend {
    Appended(Box<GenerationDbChainRecord>),
    NeedsBase(GenerationDbChainFallbackReason),
}

pub fn create_generation_db_chain_base(
    source: &Connection,
    source_path: impl AsRef<Path>,
    state_dir: impl AsRef<Path>,
    schema_epoch: &str,
    db_schema_version: i32,
    identity: GenerationDbBackupIdentity,
) -> Result<GenerationDbChainRecord> {
    let state_dir = state_dir.as_ref();
    std::fs::create_dir_all(state_dir)?;
    let temp = state_dir.join(".conary.db.base.tmp");
    remove_if_exists(&temp)?;
    let snapshot = GenerationDbSnapshotProvider::default().snapshot(source, source_path, &temp)?;
    let snapshot_connection = Connection::open_with_flags(
        &temp,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let observed_schema_version = crate::db::schema::get_schema_version(&snapshot_connection)?;
    if observed_schema_version != db_schema_version {
        return Err(Error::RecoveryFailed(format!(
            "generation DB base schema version changed during snapshot: expected={db_schema_version} observed={observed_schema_version}"
        )));
    }
    let sqlite_page_count =
        snapshot_connection.query_row("PRAGMA page_count", [], |row| row.get(0))?;
    let final_baseline = crate::db::generation_delta::read_baseline_token(&snapshot_connection)?;
    drop(snapshot_connection);
    let sha256 = sha256_file(&temp)?;
    let file = format!("conary.db.base-{sha256}.sqlite");
    let base_path = state_dir.join(&file);
    std::fs::rename(&temp, &base_path)?;
    sync_parent_directory(&base_path)?;
    let payload_bytes_written = snapshot.provenance.payload_bytes_written;
    let provider_elapsed_ns = snapshot.metrics.provider_elapsed_ns;
    let base = GenerationDbBaseArtifact {
        file,
        sha256,
        sqlite_page_count,
        logical_size: snapshot.provenance.logical_size,
        snapshot: snapshot.provenance,
        result_baseline: final_baseline.clone(),
    };
    let chain_sha256 = calculate_chain_sha256(&base, &[]);
    let manifest = GenerationDbChainManifest {
        format: GENERATION_DB_CHAIN_FORMAT.to_string(),
        manifest_version: GENERATION_DB_CHAIN_MANIFEST_VERSION,
        schema_epoch: schema_epoch.to_string(),
        db_schema_version,
        generation_number: identity.generation_number,
        state_number: identity.state_number,
        created_at: Utc::now().to_rfc3339(),
        transaction_high_water_mark: identity.transaction_high_water_mark,
        base,
        deltas: Vec::new(),
        final_baseline,
        chain_sha256,
    };
    persist_manifest(state_dir, &manifest)?;
    prune_unreferenced_artifacts(state_dir, &manifest)?;
    Ok(GenerationDbChainRecord {
        manifest_path: state_dir.join(GENERATION_DB_CHAIN_MANIFEST_FILE),
        manifest,
        metrics: GenerationDbChainMetrics {
            payload_bytes_written,
            linked_artifacts: 0,
            provider_elapsed_ns: Some(provider_elapsed_ns),
        },
    })
}

pub fn append_generation_db_chain_delta(
    prior_state_dir: impl AsRef<Path>,
    state_dir: impl AsRef<Path>,
    delta: &GenerationDbDelta,
    schema_epoch: &str,
    db_schema_version: i32,
    identity: GenerationDbBackupIdentity,
) -> Result<GenerationDbChainAppend> {
    let prior_state_dir = prior_state_dir.as_ref();
    let state_dir = state_dir.as_ref();
    let prior = read_generation_db_chain(prior_state_dir)?;
    validate_manifest_structure(&prior)?;
    if prior.schema_epoch != schema_epoch || prior.db_schema_version != db_schema_version {
        return Ok(GenerationDbChainAppend::NeedsBase(
            GenerationDbChainFallbackReason::SchemaChanged,
        ));
    }
    if prior.final_baseline != *delta.source_baseline() {
        return Ok(GenerationDbChainAppend::NeedsBase(
            GenerationDbChainFallbackReason::SourceBaselineChanged,
        ));
    }
    if prior.deltas.len() >= MAX_GENERATION_DB_DELTAS {
        return Ok(GenerationDbChainAppend::NeedsBase(
            GenerationDbChainFallbackReason::ChainLimitReached,
        ));
    }
    if delta.payload_bytes() == 0 || delta.source_baseline() == delta.result_baseline() {
        return Ok(GenerationDbChainAppend::NeedsBase(
            GenerationDbChainFallbackReason::EmptyDelta,
        ));
    }

    std::fs::create_dir_all(state_dir)?;
    let mut linked_artifacts = 0;
    link_artifact(prior_state_dir, state_dir, &prior.base.file)?;
    linked_artifacts += 1;
    for artifact in &prior.deltas {
        link_artifact(prior_state_dir, state_dir, &artifact.file)?;
        linked_artifacts += 1;
    }

    let file = format!(
        "conary.db.delta-{:03}-{}.changeset",
        prior.deltas.len() + 1,
        delta.sha256()
    );
    write_file_atomic_with_mode(&state_dir.join(&file), delta.bytes(), 0o600)?;
    let artifact = GenerationDbDeltaArtifact {
        file,
        sha256: delta.sha256().to_string(),
        payload_bytes: delta.payload_bytes(),
        source_baseline: delta.source_baseline().clone(),
        result_baseline: delta.result_baseline().clone(),
    };
    let mut deltas = prior.deltas;
    deltas.push(artifact);
    let chain_sha256 = calculate_chain_sha256(&prior.base, &deltas);
    let manifest = GenerationDbChainManifest {
        format: GENERATION_DB_CHAIN_FORMAT.to_string(),
        manifest_version: GENERATION_DB_CHAIN_MANIFEST_VERSION,
        schema_epoch: schema_epoch.to_string(),
        db_schema_version,
        generation_number: identity.generation_number,
        state_number: identity.state_number,
        created_at: Utc::now().to_rfc3339(),
        transaction_high_water_mark: identity.transaction_high_water_mark,
        base: prior.base,
        deltas,
        final_baseline: delta.result_baseline().clone(),
        chain_sha256,
    };
    persist_manifest(state_dir, &manifest)?;
    prune_unreferenced_artifacts(state_dir, &manifest)?;
    Ok(GenerationDbChainAppend::Appended(Box::new(
        GenerationDbChainRecord {
            manifest_path: state_dir.join(GENERATION_DB_CHAIN_MANIFEST_FILE),
            manifest,
            metrics: GenerationDbChainMetrics {
                payload_bytes_written: delta.payload_bytes(),
                linked_artifacts,
                provider_elapsed_ns: None,
            },
        },
    )))
}

pub fn read_generation_db_chain(state_dir: impl AsRef<Path>) -> Result<GenerationDbChainManifest> {
    let path = state_dir.as_ref().join(GENERATION_DB_CHAIN_MANIFEST_FILE);
    let bytes = std::fs::read(&path)?;
    serde_json::from_slice(&bytes).map_err(Into::into)
}

pub fn find_generation_db_chain_by_baseline(
    generations_dir: impl AsRef<Path>,
    before_generation: i64,
    baseline: &GenerationDbBaselineToken,
) -> Result<Option<PathBuf>> {
    let generations_dir = generations_dir.as_ref();
    if !generations_dir.exists() {
        return Ok(None);
    }
    let mut candidates = Vec::new();
    for entry in std::fs::read_dir(generations_dir)? {
        let entry = entry?;
        let Some(number) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i64>().ok())
        else {
            continue;
        };
        if number < before_generation {
            candidates.push((number, entry.path()));
        }
    }
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
    for (_, generation_dir) in candidates {
        let state_dir = generation_dir.join("state");
        let manifest_path = state_dir.join(GENERATION_DB_CHAIN_MANIFEST_FILE);
        if !manifest_path.exists() {
            continue;
        }
        let manifest = read_generation_db_chain(&state_dir)?;
        validate_manifest_structure(&manifest)?;
        if manifest.final_baseline == *baseline {
            return Ok(Some(generation_dir));
        }
    }
    Ok(None)
}

pub fn materialize_and_verify_generation_db_chain(
    state_dir: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> Result<GenerationDbChainManifest> {
    let state_dir = state_dir.as_ref();
    let destination = destination.as_ref();
    let manifest = read_generation_db_chain(state_dir)?;
    verify_manifest_payloads(state_dir, &manifest)?;
    remove_if_exists(destination)?;
    std::fs::copy(state_dir.join(&manifest.base.file), destination)?;
    File::open(destination)?.sync_all()?;
    let connection = Connection::open(destination)?;
    crate::db::generation_delta::configure_mutation_epoch(&connection)?;
    connection.execute_batch(
        "PRAGMA journal_mode = DELETE;
         PRAGMA synchronous = FULL;
         PRAGMA foreign_keys = OFF;",
    )?;
    for artifact in &manifest.deltas {
        let bytes = std::fs::read(state_dir.join(&artifact.file))?;
        let mut input = bytes.as_slice();
        connection.apply_strm(&mut input, None::<fn(&str) -> bool>, |_conflict, _item| {
            ConflictAction::SQLITE_CHANGESET_ABORT
        })?;
        connection.execute(
            "UPDATE generation_db_mutation_epoch SET token = ?1 WHERE singleton = 1",
            [&artifact.result_baseline.mutation_token],
        )?;
    }
    let observed = crate::db::generation_delta::read_baseline_token(&connection)?;
    if observed != manifest.final_baseline {
        return Err(Error::RecoveryFailed(format!(
            "materialized generation DB baseline mismatch: manifest={:?} database={observed:?}",
            manifest.final_baseline
        )));
    }
    let integrity: String = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(Error::RecoveryFailed(format!(
            "materialized generation DB integrity check failed: {integrity}"
        )));
    }
    let foreign_key_errors: i64 =
        connection.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if foreign_key_errors != 0 {
        return Err(Error::RecoveryFailed(format!(
            "materialized generation DB has {foreign_key_errors} foreign-key violations"
        )));
    }
    drop(connection);
    File::open(destination)?.sync_all()?;
    sync_parent_directory(destination)?;
    Ok(manifest)
}

fn persist_manifest(state_dir: &Path, manifest: &GenerationDbChainManifest) -> Result<()> {
    validate_manifest_structure(manifest)?;
    write_json_atomic_with_mode(
        &state_dir.join(GENERATION_DB_CHAIN_MANIFEST_FILE),
        manifest,
        0o600,
    )
}

fn prune_unreferenced_artifacts(
    state_dir: &Path,
    manifest: &GenerationDbChainManifest,
) -> Result<()> {
    let retained = std::iter::once(manifest.base.file.as_str())
        .chain(manifest.deltas.iter().map(|delta| delta.file.as_str()))
        .collect::<BTreeSet<_>>();
    for entry in std::fs::read_dir(state_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let unreferenced_v3 = (name.starts_with("conary.db.base-")
            || name.starts_with("conary.db.delta-"))
            && !retained.contains(name);
        let retired_v2 = matches!(name, "conary.db.backup" | "conary.db.backup.sha256");
        if unreferenced_v3 || retired_v2 {
            remove_if_exists(&entry.path())?;
        }
    }
    Ok(())
}

fn validate_manifest_structure(manifest: &GenerationDbChainManifest) -> Result<()> {
    if manifest.format != GENERATION_DB_CHAIN_FORMAT
        || manifest.manifest_version != GENERATION_DB_CHAIN_MANIFEST_VERSION
    {
        return Err(Error::RecoveryFailed(format!(
            "unsupported generation DB chain format/version: {}/{}",
            manifest.format, manifest.manifest_version
        )));
    }
    if manifest.deltas.len() > MAX_GENERATION_DB_DELTAS {
        return Err(Error::RecoveryFailed(format!(
            "generation DB delta chain exceeds limit {}: {}",
            MAX_GENERATION_DB_DELTAS,
            manifest.deltas.len()
        )));
    }
    if manifest.generation_number < 0
        || manifest.state_number < 0
        || manifest.generation_number != manifest.state_number
    {
        return Err(Error::RecoveryFailed(format!(
            "generation DB chain has invalid generation/state identity: generation={} state={}",
            manifest.generation_number, manifest.state_number
        )));
    }
    validate_artifact_name(&manifest.base.file, "conary.db.base-")?;
    validate_sha256(&manifest.base.sha256)?;
    let expected_base_file = format!("conary.db.base-{}.sqlite", manifest.base.sha256);
    if manifest.base.file != expected_base_file {
        return Err(Error::RecoveryFailed(format!(
            "generation DB base filename does not bind its digest: {}",
            manifest.base.file
        )));
    }
    if manifest.base.logical_size == 0 || manifest.base.sqlite_page_count <= 0 {
        return Err(Error::RecoveryFailed(
            "generation DB base has invalid SQLite size metadata".to_string(),
        ));
    }
    manifest.base.snapshot.validate()?;
    if manifest.base.snapshot.logical_size != manifest.base.logical_size {
        return Err(Error::RecoveryFailed(
            "generation DB base logical size disagrees with snapshot provenance".to_string(),
        ));
    }
    validate_baseline_token(&manifest.base.result_baseline)?;
    let mut expected_source = &manifest.base.result_baseline;
    for (index, artifact) in manifest.deltas.iter().enumerate() {
        validate_artifact_name(&artifact.file, "conary.db.delta-")?;
        validate_sha256(&artifact.sha256)?;
        let expected_file = format!(
            "conary.db.delta-{:03}-{}.changeset",
            index + 1,
            artifact.sha256
        );
        if artifact.file != expected_file {
            return Err(Error::RecoveryFailed(format!(
                "generation DB delta filename does not bind order and digest: {}",
                artifact.file
            )));
        }
        if artifact.payload_bytes == 0 {
            return Err(Error::RecoveryFailed(format!(
                "generation DB delta {} has zero payload bytes",
                index + 1
            )));
        }
        validate_baseline_token(&artifact.source_baseline)?;
        validate_baseline_token(&artifact.result_baseline)?;
        if artifact.source_baseline != *expected_source {
            return Err(Error::RecoveryFailed(format!(
                "generation DB delta {} does not continue the prior baseline",
                index + 1
            )));
        }
        if artifact.result_baseline == artifact.source_baseline {
            return Err(Error::RecoveryFailed(format!(
                "generation DB delta {} does not advance the baseline",
                index + 1
            )));
        }
        expected_source = &artifact.result_baseline;
    }
    validate_baseline_token(&manifest.final_baseline)?;
    if *expected_source != manifest.final_baseline {
        return Err(Error::RecoveryFailed(
            "generation DB chain final baseline does not match its payload sequence".to_string(),
        ));
    }
    let expected_chain = calculate_chain_sha256(&manifest.base, &manifest.deltas);
    if expected_chain != manifest.chain_sha256 {
        return Err(Error::RecoveryFailed(format!(
            "generation DB chain identity mismatch: manifest={} calculated={expected_chain}",
            manifest.chain_sha256
        )));
    }
    Ok(())
}

fn verify_manifest_payloads(state_dir: &Path, manifest: &GenerationDbChainManifest) -> Result<()> {
    validate_manifest_structure(manifest)?;
    verify_artifact(
        &state_dir.join(&manifest.base.file),
        &manifest.base.sha256,
        Some(manifest.base.logical_size),
    )?;
    for artifact in &manifest.deltas {
        verify_artifact(
            &state_dir.join(&artifact.file),
            &artifact.sha256,
            Some(artifact.payload_bytes),
        )?;
    }
    Ok(())
}

fn verify_artifact(path: &Path, expected_sha256: &str, expected_size: Option<u64>) -> Result<()> {
    if let Some(expected) = expected_size {
        let actual = path.metadata()?.len();
        if actual != expected {
            return Err(Error::RecoveryFailed(format!(
                "generation DB chain artifact size mismatch at {}: expected={expected} actual={actual}",
                path.display()
            )));
        }
    }
    let actual = sha256_file(path)?;
    if actual != expected_sha256 {
        return Err(Error::ChecksumMismatch {
            expected: expected_sha256.to_string(),
            actual,
        });
    }
    Ok(())
}

fn calculate_chain_sha256(
    base: &GenerationDbBaseArtifact,
    deltas: &[GenerationDbDeltaArtifact],
) -> String {
    let mut identity = format!(
        "{GENERATION_DB_CHAIN_FORMAT}\0{}\0{}",
        base.sha256, base.result_baseline.mutation_token
    )
    .into_bytes();
    for delta in deltas {
        identity.push(0);
        identity.extend_from_slice(delta.sha256.as_bytes());
        identity.push(0);
        identity.extend_from_slice(delta.source_baseline.mutation_token.as_bytes());
        identity.push(0);
        identity.extend_from_slice(delta.result_baseline.mutation_token.as_bytes());
    }
    crate::hash::sha256(&identity)
}

fn validate_artifact_name(file: &str, prefix: &str) -> Result<()> {
    let path = Path::new(file);
    if !file.starts_with(prefix)
        || path.is_absolute()
        || path.components().count() != 1
        || !matches!(
            path.components().next(),
            Some(std::path::Component::Normal(_))
        )
    {
        return Err(Error::InvalidPath(format!(
            "generation DB chain artifact must be a plain {prefix} filename: {file}"
        )));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::RecoveryFailed(format!(
            "invalid generation DB SHA-256 identity: {value}"
        )));
    }
    Ok(())
}

fn validate_baseline_token(token: &GenerationDbBaselineToken) -> Result<()> {
    uuid::Uuid::parse_str(&token.mutation_token).map_err(|_| {
        Error::RecoveryFailed(format!(
            "invalid generation DB mutation token: {}",
            token.mutation_token
        ))
    })?;
    Ok(())
}

fn link_artifact(source_dir: &Path, destination_dir: &Path, file: &str) -> Result<()> {
    let source = source_dir.join(file);
    let destination = destination_dir.join(file);
    if destination.exists() {
        let source_metadata = source.metadata()?;
        let destination_metadata = destination.metadata()?;
        if source_metadata.dev() == destination_metadata.dev()
            && source_metadata.ino() == destination_metadata.ino()
        {
            return Ok(());
        }
        return Err(Error::RecoveryFailed(format!(
            "generation DB chain artifact already exists with different identity: {}",
            destination.display()
        )));
    }
    std::fs::hard_link(&source, &destination)?;
    sync_parent_directory(&destination)
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    sha256_reader_hex(&mut file).map_err(Error::Io)
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => sync_parent_directory(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::Io(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::generation_delta::{
        GenerationDbDeltaCapture, GenerationDbDeltaRecorder, create_mutation_epoch_triggers,
        read_baseline_token,
    };

    fn fixture(path: &Path) -> Connection {
        let connection = Connection::open(path).unwrap();
        crate::db::generation_delta::configure_mutation_epoch(&connection).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE generation_db_mutation_epoch (
                     singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                     token TEXT NOT NULL CHECK(length(token) = 36)
                 );
                 INSERT INTO generation_db_mutation_epoch (singleton, token)
                 VALUES (1, '00000000-0000-0000-0000-000000000000');
                 CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
                 INSERT INTO schema_version (version) VALUES (39);
                 CREATE TABLE authority (
                     id INTEGER PRIMARY KEY,
                     value TEXT NOT NULL
                 );
                 INSERT INTO authority (id, value) VALUES (1, 'base');",
            )
            .unwrap();
        create_mutation_epoch_triggers(&connection).unwrap();
        connection
    }

    fn identity(number: i64) -> GenerationDbBackupIdentity {
        GenerationDbBackupIdentity {
            generation_number: number,
            state_number: number,
            transaction_high_water_mark: None,
        }
    }

    #[test]
    fn first_generation_zero_is_a_valid_chain_identity() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("live.sqlite");
        let state_dir = temp.path().join("generation-0-state");
        let source = fixture(&db_path);

        let record = create_generation_db_chain_base(
            &source,
            &db_path,
            &state_dir,
            "fixture-v1",
            39,
            identity(0),
        )
        .unwrap();

        assert_eq!(record.manifest.generation_number, 0);
        assert_eq!(record.manifest.state_number, 0);
        validate_manifest_structure(&record.manifest).unwrap();
    }

    #[test]
    fn delta_generation_is_self_contained_and_reconstructs_exact_rows() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("live.sqlite");
        let base_state = temp.path().join("generation-1-state");
        let delta_state = temp.path().join("generation-2-state");
        let recovered = temp.path().join("recovered.sqlite");
        let source = fixture(&db_path);
        let base = create_generation_db_chain_base(
            &source,
            &db_path,
            &base_state,
            "fixture-v1",
            39,
            identity(1),
        )
        .unwrap();
        let mut recorder = GenerationDbDeltaRecorder::begin_against(
            &source,
            &db_path,
            base.manifest.final_baseline.clone(),
        )
        .unwrap();
        source
            .execute_batch(
                "UPDATE authority SET value = 'updated' WHERE id = 1;
                 INSERT INTO authority (id, value) VALUES (2, 'inserted');",
            )
            .unwrap();
        let GenerationDbDeltaCapture::Captured(delta) = recorder.capture().unwrap() else {
            panic!("exact base unexpectedly required a full snapshot");
        };
        let GenerationDbChainAppend::Appended(appended) = append_generation_db_chain_delta(
            &base_state,
            &delta_state,
            &delta,
            "fixture-v1",
            39,
            identity(2),
        )
        .unwrap() else {
            panic!("short exact chain unexpectedly required a new base");
        };
        assert_eq!(
            appended.metrics.payload_bytes_written,
            delta.payload_bytes()
        );
        assert_eq!(appended.metrics.linked_artifacts, 1);
        assert_eq!(appended.manifest.deltas.len(), 1);

        std::fs::remove_dir_all(&base_state).unwrap();
        materialize_and_verify_generation_db_chain(&delta_state, &recovered).unwrap();
        let recovered = Connection::open(recovered).unwrap();
        let rows = recovered
            .prepare("SELECT id, value FROM authority ORDER BY id")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![(1, "updated".to_string()), (2, "inserted".to_string())]
        );
        assert_eq!(
            read_baseline_token(&recovered).unwrap(),
            appended.manifest.final_baseline
        );
    }

    #[test]
    fn manifest_and_delta_tampering_fail_before_replay() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("live.sqlite");
        let base_state = temp.path().join("generation-1-state");
        let delta_state = temp.path().join("generation-2-state");
        let source = fixture(&db_path);
        let base = create_generation_db_chain_base(
            &source,
            &db_path,
            &base_state,
            "fixture-v1",
            39,
            identity(1),
        )
        .unwrap();
        let recorder = GenerationDbDeltaRecorder::begin_against(
            &source,
            &db_path,
            base.manifest.final_baseline,
        )
        .unwrap();
        source
            .execute("INSERT INTO authority (id, value) VALUES (2, 'delta')", [])
            .unwrap();
        let GenerationDbDeltaCapture::Captured(delta) = recorder.finish().unwrap() else {
            panic!("exact base unexpectedly required a full snapshot");
        };
        let GenerationDbChainAppend::Appended(record) = append_generation_db_chain_delta(
            &base_state,
            &delta_state,
            &delta,
            "fixture-v1",
            39,
            identity(2),
        )
        .unwrap() else {
            panic!("short exact chain unexpectedly required a new base");
        };
        std::fs::write(
            delta_state.join(&record.manifest.deltas[0].file),
            b"tampered",
        )
        .unwrap();

        let error = materialize_and_verify_generation_db_chain(
            &delta_state,
            temp.path().join("recovered.sqlite"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("size mismatch"));
    }

    #[test]
    fn manifest_rejects_a_delta_baseline_gap_even_with_a_matching_chain_digest() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("live.sqlite");
        let base_state = temp.path().join("generation-1-state");
        let delta_state = temp.path().join("generation-2-state");
        let source = fixture(&db_path);
        let base = create_generation_db_chain_base(
            &source,
            &db_path,
            &base_state,
            "fixture-v1",
            39,
            identity(1),
        )
        .unwrap();
        let recorder = GenerationDbDeltaRecorder::begin_against(
            &source,
            &db_path,
            base.manifest.final_baseline,
        )
        .unwrap();
        source
            .execute("INSERT INTO authority (id, value) VALUES (2, 'delta')", [])
            .unwrap();
        let GenerationDbDeltaCapture::Captured(delta) = recorder.finish().unwrap() else {
            panic!("exact base unexpectedly required a full snapshot");
        };
        let GenerationDbChainAppend::Appended(record) = append_generation_db_chain_delta(
            &base_state,
            &delta_state,
            &delta,
            "fixture-v1",
            39,
            identity(2),
        )
        .unwrap() else {
            panic!("short exact chain unexpectedly required a new base");
        };
        let mut manifest = record.manifest;
        manifest.deltas[0].source_baseline = GenerationDbBaselineToken {
            mutation_token: uuid::Uuid::new_v4().to_string(),
        };
        manifest.chain_sha256 = calculate_chain_sha256(&manifest.base, &manifest.deltas);

        let error = validate_manifest_structure(&manifest).unwrap_err();
        assert!(error.to_string().contains("does not continue"));
    }

    #[test]
    fn current_schema_triggers_admit_exact_chain_replay() {
        let temp = tempfile::tempdir().unwrap();
        let (database, source) = crate::db::testing::create_test_db();
        let base_state = temp.path().join("generation-1-state");
        let delta_state = temp.path().join("generation-2-state");
        let recovered_path = temp.path().join("recovered.sqlite");
        let base = create_generation_db_chain_base(
            &source,
            database.path(),
            &base_state,
            crate::db::schema::SCHEMA_EPOCH,
            crate::db::schema::SCHEMA_VERSION,
            identity(1),
        )
        .unwrap();
        let recorder = GenerationDbDeltaRecorder::begin_against(
            &source,
            database.path(),
            base.manifest.final_baseline,
        )
        .unwrap();
        source
            .execute(
                "UPDATE schema_identity SET created_at = 'chain-replay-fixture'",
                [],
            )
            .unwrap();
        let GenerationDbDeltaCapture::Captured(delta) = recorder.finish().unwrap() else {
            panic!("current-schema mutation unexpectedly required a full snapshot");
        };
        let GenerationDbChainAppend::Appended(_) = append_generation_db_chain_delta(
            &base_state,
            &delta_state,
            &delta,
            crate::db::schema::SCHEMA_EPOCH,
            crate::db::schema::SCHEMA_VERSION,
            identity(2),
        )
        .unwrap() else {
            panic!("current-schema chain unexpectedly required a new base");
        };

        materialize_and_verify_generation_db_chain(&delta_state, &recovered_path).unwrap();
        let recovered = Connection::open(recovered_path).unwrap();
        assert_eq!(
            recovered
                .query_row("SELECT created_at FROM schema_identity", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            "chain-replay-fixture"
        );
    }

    #[test]
    fn thirty_third_delta_forces_a_new_base_before_any_link_work() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("live.sqlite");
        let base_state = temp.path().join("generation-1-state");
        let next_state = temp.path().join("generation-34-state");
        let source = fixture(&db_path);
        let mut manifest = create_generation_db_chain_base(
            &source,
            &db_path,
            &base_state,
            "fixture-v1",
            39,
            identity(1),
        )
        .unwrap()
        .manifest;
        let mut source_baseline = manifest.base.result_baseline.clone();
        for index in 0..MAX_GENERATION_DB_DELTAS {
            let result_baseline = GenerationDbBaselineToken {
                mutation_token: uuid::Uuid::new_v4().to_string(),
            };
            let bytes = format!("delta-{index}").into_bytes();
            let sha256 = crate::hash::sha256(&bytes);
            manifest.deltas.push(GenerationDbDeltaArtifact {
                file: format!("conary.db.delta-{:03}-{sha256}.changeset", index + 1),
                sha256,
                payload_bytes: bytes.len() as u64,
                source_baseline,
                result_baseline: result_baseline.clone(),
            });
            source_baseline = result_baseline;
        }
        manifest.final_baseline = source_baseline.clone();
        manifest.chain_sha256 = calculate_chain_sha256(&manifest.base, &manifest.deltas);
        persist_manifest(&base_state, &manifest).unwrap();
        let next = GenerationDbDelta::from_bytes(
            b"next-delta".to_vec(),
            source_baseline,
            GenerationDbBaselineToken {
                mutation_token: uuid::Uuid::new_v4().to_string(),
            },
        );

        assert_eq!(
            append_generation_db_chain_delta(
                &base_state,
                &next_state,
                &next,
                "fixture-v1",
                39,
                identity(34),
            )
            .unwrap(),
            GenerationDbChainAppend::NeedsBase(GenerationDbChainFallbackReason::ChainLimitReached)
        );
        assert!(!next_state.exists());
    }
}
