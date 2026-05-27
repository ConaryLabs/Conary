// conary-core/src/db/backup.rs

//! Durable SQLite checkpoint backups for Conary live-state recovery.

use crate::db::schema;
use crate::filesystem::durable::{sync_parent_directory, write_file_atomic, write_json_atomic};
use crate::hash::sha256_reader_hex;
use crate::runtime_root::ConaryRuntimeRoot;
use crate::{Error, Result};
use chrono::{Duration, Utc};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};

const BACKUP_FORMAT: &str = "conary.db-checkpoint.v1";
const GENERATION_BACKUP_FORMAT: &str = "conary.generation-db-backup.v1";
const MANIFEST_VERSION: u32 = 1;
const DEFAULT_RETAIN_COUNT: usize = 5;
const DEFAULT_RETAIN_DAYS: i64 = 14;
const GENERATION_BACKUP_FILE: &str = "conary.db.backup";
const GENERATION_BACKUP_CHECKSUM_FILE: &str = "conary.db.backup.sha256";
const GENERATION_BACKUP_MANIFEST_FILE: &str = "conary-db-backup.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckpointReason {
    PreMutation,
    PostSuccess,
}

impl CheckpointReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreMutation => "pre-mutation",
            Self::PostSuccess => "post-success",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbBackupManifest {
    pub format: String,
    pub manifest_version: u32,
    pub created_at: String,
    pub source_db_path: PathBuf,
    pub backup_file: String,
    pub backup_sha256: String,
    pub reason: CheckpointReason,
    pub db_schema_version: i32,
    pub integrity_check: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbBackupRecord {
    pub backup_path: PathBuf,
    pub manifest_path: PathBuf,
    pub checksum_path: PathBuf,
    pub manifest: DbBackupManifest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupVerification {
    pub backup_path: PathBuf,
    pub db_schema_version: i32,
    pub integrity_check: String,
    pub backup_sha256: String,
    pub sqlite_page_count: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationDbBackupManifest {
    pub format: String,
    pub manifest_version: u32,
    pub db_schema_version: i32,
    pub generation_number: i64,
    pub state_number: i64,
    pub created_at: String,
    pub backup_file: String,
    pub compression: Option<String>,
    pub backup_sha256: String,
    pub sqlite_page_count: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerationDbBackupRecord {
    pub backup_path: PathBuf,
    pub checksum_path: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: GenerationDbBackupManifest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerationDbBackupVerification {
    pub backup_path: PathBuf,
    pub manifest_path: PathBuf,
    pub generation_number: i64,
    pub state_number: i64,
    pub db_schema_version: i32,
    pub integrity_check: String,
    pub backup_sha256: String,
    pub sqlite_page_count: Option<i64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecoveryOptions {
    pub dry_run: bool,
    pub yes: bool,
    pub replace_healthy_db: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryOutcome {
    pub backup_path: PathBuf,
    pub manifest_path: PathBuf,
    pub dry_run: bool,
    pub restored: bool,
    pub quarantined_paths: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GenerationDbRecoveryOptions {
    pub dry_run: bool,
    pub yes: bool,
    pub keep_temp: bool,
    pub replace_healthy_db: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerationDbRecoveryOutcome {
    pub backup_path: PathBuf,
    pub manifest_path: PathBuf,
    pub dry_run: bool,
    pub restored: bool,
    pub verified_temp_path: Option<PathBuf>,
    pub quarantined_paths: Vec<PathBuf>,
}

pub fn backup_dir_for_db_path(db_path: impl AsRef<Path>) -> PathBuf {
    ConaryRuntimeRoot::from_db_path(db_path.as_ref().to_path_buf())
        .root()
        .join("backups")
}

pub fn create_checkpoint(
    db_path: impl AsRef<Path>,
    reason: CheckpointReason,
) -> Result<DbBackupRecord> {
    let db_path = db_path.as_ref();
    if !db_path.exists() {
        return Err(Error::DatabaseNotFound(db_path.display().to_string()));
    }

    let backup_dir = backup_dir_for_db_path(db_path);
    std::fs::create_dir_all(&backup_dir)?;

    let now = Utc::now();
    let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let unique = now
        .timestamp_nanos_opt()
        .unwrap_or_else(|| now.timestamp_micros() * 1_000);
    let stem = format!("conary-db-{timestamp}-{unique}-{}", reason.as_str());
    let tmp_backup_path = backup_dir.join(format!(".{stem}.tmp"));
    let backup_path = backup_dir.join(format!("{stem}.sqlite"));
    let manifest_path = backup_dir.join(format!("{stem}.manifest.json"));
    let checksum_path = backup_dir.join(format!("{stem}.sha256"));

    if tmp_backup_path.exists() {
        std::fs::remove_file(&tmp_backup_path)?;
    }

    let source = Connection::open(db_path)?;
    let tmp_backup_string = tmp_backup_path.to_string_lossy().into_owned();
    source.execute("VACUUM main INTO ?1", [tmp_backup_string.as_str()])?;
    sync_file(&tmp_backup_path)?;
    std::fs::rename(&tmp_backup_path, &backup_path)?;
    sync_parent_directory(&backup_path)?;

    let verification = verify_sqlite_database(&backup_path)?;
    if verification.db_schema_version != schema::SCHEMA_VERSION {
        return Err(Error::RecoveryFailed(format!(
            "refusing to checkpoint schema version {}; supported schema is {}",
            verification.db_schema_version,
            schema::SCHEMA_VERSION
        )));
    }

    let backup_sha256 = sha256_file(&backup_path)?;
    let manifest = DbBackupManifest {
        format: BACKUP_FORMAT.to_string(),
        manifest_version: MANIFEST_VERSION,
        created_at: now.to_rfc3339(),
        source_db_path: db_path.to_path_buf(),
        backup_file: backup_path
            .file_name()
            .ok_or_else(|| Error::InvalidPath(backup_path.display().to_string()))?
            .to_string_lossy()
            .into_owned(),
        backup_sha256: backup_sha256.clone(),
        reason,
        db_schema_version: verification.db_schema_version,
        integrity_check: verification.integrity_check.clone(),
    };

    write_file_atomic(
        &checksum_path,
        format!("{backup_sha256}  {}\n", manifest.backup_file).as_bytes(),
    )?;
    write_json_atomic(&manifest_path, &manifest)?;

    let record = DbBackupRecord {
        backup_path,
        manifest_path,
        checksum_path,
        manifest,
    };
    rotate_backups(&backup_dir, DEFAULT_RETAIN_COUNT, DEFAULT_RETAIN_DAYS)?;
    Ok(record)
}

pub fn list_backups(db_path: impl AsRef<Path>) -> Result<Vec<DbBackupRecord>> {
    let backup_dir = backup_dir_for_db_path(db_path);
    if !backup_dir.exists() {
        return Ok(Vec::new());
    }

    let mut records = Vec::new();
    for entry in std::fs::read_dir(backup_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".manifest.json"))
        {
            continue;
        }

        records.push(read_record_from_manifest(&path)?);
    }

    records.sort_by(|a, b| {
        a.manifest
            .created_at
            .cmp(&b.manifest.created_at)
            .then_with(|| a.manifest_path.cmp(&b.manifest_path))
    });
    Ok(records)
}

pub fn latest_backup(db_path: impl AsRef<Path>) -> Result<Option<DbBackupRecord>> {
    Ok(list_backups(db_path)?.into_iter().next_back())
}

pub fn verify_latest(db_path: impl AsRef<Path>) -> Result<BackupVerification> {
    let record = latest_backup(db_path)?
        .ok_or_else(|| Error::NotFound("no Conary DB backups found".to_string()))?;
    verify_backup(&record)
}

pub fn verify_backup(record: &DbBackupRecord) -> Result<BackupVerification> {
    if record.manifest.format != BACKUP_FORMAT {
        return Err(Error::RecoveryFailed(format!(
            "unsupported DB backup manifest format: {}",
            record.manifest.format
        )));
    }
    if record.manifest.manifest_version != MANIFEST_VERSION {
        return Err(Error::RecoveryFailed(format!(
            "unsupported DB backup manifest version: {}",
            record.manifest.manifest_version
        )));
    }

    let actual_hash = sha256_file(&record.backup_path)?;
    if actual_hash != record.manifest.backup_sha256 {
        return Err(Error::ChecksumMismatch {
            expected: record.manifest.backup_sha256.clone(),
            actual: actual_hash,
        });
    }

    let verification = verify_sqlite_database(&record.backup_path)?;
    if verification.db_schema_version != schema::SCHEMA_VERSION {
        return Err(Error::RecoveryFailed(format!(
            "DB backup schema version {} is not supported by this Conary binary (expected {})",
            verification.db_schema_version,
            schema::SCHEMA_VERSION
        )));
    }
    if verification.db_schema_version != record.manifest.db_schema_version {
        return Err(Error::RecoveryFailed(format!(
            "DB backup schema version changed since manifest creation: manifest={}, backup={}",
            record.manifest.db_schema_version, verification.db_schema_version
        )));
    }

    Ok(BackupVerification {
        backup_path: record.backup_path.clone(),
        db_schema_version: verification.db_schema_version,
        integrity_check: verification.integrity_check,
        backup_sha256: record.manifest.backup_sha256.clone(),
        sqlite_page_count: verification.sqlite_page_count,
    })
}

pub fn recover_latest(
    db_path: impl AsRef<Path>,
    options: RecoveryOptions,
) -> Result<RecoveryOutcome> {
    let db_path = db_path.as_ref();
    let record = latest_backup(db_path)?
        .ok_or_else(|| Error::NotFound("no Conary DB backups found".to_string()))?;
    verify_backup(&record)?;

    if options.dry_run {
        return Ok(RecoveryOutcome {
            backup_path: record.backup_path,
            manifest_path: record.manifest_path,
            dry_run: true,
            restored: false,
            quarantined_paths: Vec::new(),
        });
    }

    if !options.yes {
        return Err(Error::Cancelled(
            "refusing to restore a DB backup without --yes".to_string(),
        ));
    }

    if db_path.exists() && live_db_is_healthy(db_path) && !options.replace_healthy_db {
        return Err(Error::RecoveryFailed(format!(
            "live DB at {} passed integrity checks; refusing to replace it without --replace-healthy-db",
            db_path.display()
        )));
    }

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let quarantine_stamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let mut quarantined_paths = Vec::new();
    for candidate in sqlite_database_paths(db_path) {
        if candidate.exists() {
            let target = quarantine_path(&candidate, &quarantine_stamp)?;
            std::fs::rename(&candidate, &target)?;
            sync_parent_directory(&target)?;
            quarantined_paths.push(target);
        }
    }

    let restore_tmp = restore_temp_path(db_path);
    if restore_tmp.exists() {
        std::fs::remove_file(&restore_tmp)?;
    }
    std::fs::copy(&record.backup_path, &restore_tmp)?;
    sync_file(&restore_tmp)?;
    std::fs::rename(&restore_tmp, db_path)?;
    sync_parent_directory(db_path)?;
    verify_sqlite_database(db_path)?;

    Ok(RecoveryOutcome {
        backup_path: record.backup_path,
        manifest_path: record.manifest_path,
        dry_run: false,
        restored: true,
        quarantined_paths,
    })
}

pub fn create_generation_db_backup(
    db_path: impl AsRef<Path>,
    generation_dir: impl AsRef<Path>,
    generation_number: i64,
    state_number: i64,
) -> Result<GenerationDbBackupRecord> {
    let db_path = db_path.as_ref();
    let generation_dir = generation_dir.as_ref();
    if !db_path.exists() {
        return Err(Error::DatabaseNotFound(db_path.display().to_string()));
    }

    let state_dir = generation_state_backup_dir(generation_dir);
    std::fs::create_dir_all(&state_dir)?;
    let tmp_backup_path = state_dir.join(".conary.db.backup.tmp");
    let backup_path = state_dir.join(GENERATION_BACKUP_FILE);
    let checksum_path = state_dir.join(GENERATION_BACKUP_CHECKSUM_FILE);
    let manifest_path = state_dir.join(GENERATION_BACKUP_MANIFEST_FILE);

    remove_if_exists(&tmp_backup_path)?;
    let source = Connection::open(db_path)?;
    let tmp_backup_string = tmp_backup_path.to_string_lossy().into_owned();
    source.execute("VACUUM main INTO ?1", [tmp_backup_string.as_str()])?;
    sync_file(&tmp_backup_path)?;
    std::fs::rename(&tmp_backup_path, &backup_path)?;
    sync_parent_directory(&backup_path)?;

    let verification = verify_sqlite_database(&backup_path)?;
    if verification.db_schema_version != schema::SCHEMA_VERSION {
        return Err(Error::RecoveryFailed(format!(
            "refusing to write generation DB backup with schema version {}; supported schema is {}",
            verification.db_schema_version,
            schema::SCHEMA_VERSION
        )));
    }

    let manifest = GenerationDbBackupManifest {
        format: GENERATION_BACKUP_FORMAT.to_string(),
        manifest_version: MANIFEST_VERSION,
        db_schema_version: verification.db_schema_version,
        generation_number,
        state_number,
        created_at: Utc::now().to_rfc3339(),
        backup_file: GENERATION_BACKUP_FILE.to_string(),
        compression: None,
        backup_sha256: verification.backup_sha256.clone(),
        sqlite_page_count: verification.sqlite_page_count,
    };

    write_file_atomic(
        &checksum_path,
        format!("{}  {}\n", manifest.backup_sha256, manifest.backup_file).as_bytes(),
    )?;
    write_json_atomic(&manifest_path, &manifest)?;

    let record = GenerationDbBackupRecord {
        backup_path,
        checksum_path,
        manifest_path,
        manifest,
    };
    verify_generation_db_backup_record(&record, None)?;
    Ok(record)
}

pub fn verify_generation_db_backup(
    generation_dir: impl AsRef<Path>,
    current_root: Option<&Path>,
) -> Result<GenerationDbBackupVerification> {
    let record = read_generation_record(generation_dir.as_ref())?;
    verify_generation_db_backup_record(&record, current_root)
}

pub fn recover_generation_db_backup(
    db_path: impl AsRef<Path>,
    generation_dir: impl AsRef<Path>,
    options: GenerationDbRecoveryOptions,
) -> Result<GenerationDbRecoveryOutcome> {
    let db_path = db_path.as_ref();
    let record = read_generation_record(generation_dir.as_ref())?;
    verify_generation_db_backup_record(&record, None)?;

    if options.dry_run {
        let verified_temp_path = verify_generation_backup_copy(&record.backup_path, options)?;
        return Ok(GenerationDbRecoveryOutcome {
            backup_path: record.backup_path,
            manifest_path: record.manifest_path,
            dry_run: true,
            restored: false,
            verified_temp_path,
            quarantined_paths: Vec::new(),
        });
    }

    if !options.yes {
        return Err(Error::Cancelled(
            "refusing to restore a generation DB backup without --yes".to_string(),
        ));
    }

    if db_path.exists() && live_db_is_healthy(db_path) && !options.replace_healthy_db {
        return Err(Error::RecoveryFailed(format!(
            "live DB at {} passed integrity checks; refusing to replace it without --replace-healthy-db",
            db_path.display()
        )));
    }

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let quarantine_stamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let mut quarantined_paths = Vec::new();
    for candidate in sqlite_database_paths(db_path) {
        if candidate.exists() {
            let target = quarantine_path(&candidate, &quarantine_stamp)?;
            std::fs::rename(&candidate, &target)?;
            sync_parent_directory(&target)?;
            quarantined_paths.push(target);
        }
    }

    let restore_tmp = restore_temp_path(db_path);
    remove_if_exists(&restore_tmp)?;
    std::fs::copy(&record.backup_path, &restore_tmp)?;
    sync_file(&restore_tmp)?;
    verify_sqlite_database(&restore_tmp)?;
    std::fs::rename(&restore_tmp, db_path)?;
    sync_parent_directory(db_path)?;
    verify_sqlite_database(db_path)?;

    Ok(GenerationDbRecoveryOutcome {
        backup_path: record.backup_path,
        manifest_path: record.manifest_path,
        dry_run: false,
        restored: true,
        verified_temp_path: None,
        quarantined_paths,
    })
}

fn rotate_backups(backup_dir: &Path, keep_count: usize, keep_days: i64) -> Result<()> {
    let mut verified = Vec::new();
    for entry in std::fs::read_dir(backup_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".manifest.json"))
        {
            continue;
        }
        let record = read_record_from_manifest(&path)?;
        if verify_backup(&record).is_ok() {
            verified.push(record);
        }
    }

    verified.sort_by(|a, b| {
        a.manifest
            .created_at
            .cmp(&b.manifest.created_at)
            .then_with(|| a.manifest_path.cmp(&b.manifest_path))
    });

    let cutoff = Utc::now() - Duration::days(keep_days);
    let mut removal = Vec::new();
    let keep_start = verified.len().saturating_sub(keep_count);
    for (index, record) in verified.iter().enumerate() {
        let created_at = chrono::DateTime::parse_from_rfc3339(&record.manifest.created_at)
            .map(|dt| dt.with_timezone(&Utc))
            .ok();
        if index < keep_start || created_at.is_some_and(|created| created < cutoff) {
            removal.push(record.clone());
        }
    }

    for record in removal {
        remove_if_exists(&record.backup_path)?;
        remove_if_exists(&record.checksum_path)?;
        remove_if_exists(&record.manifest_path)?;
    }

    Ok(())
}

fn read_record_from_manifest(path: &Path) -> Result<DbBackupRecord> {
    let raw = std::fs::read(path)?;
    let manifest: DbBackupManifest = serde_json::from_slice(&raw)?;
    let backup_dir = path
        .parent()
        .ok_or_else(|| Error::InvalidPath(path.display().to_string()))?;
    let backup_path = backup_dir.join(&manifest.backup_file);
    let checksum_path = backup_path.with_extension("sha256");
    Ok(DbBackupRecord {
        backup_path,
        manifest_path: path.to_path_buf(),
        checksum_path,
        manifest,
    })
}

fn verify_sqlite_database(path: &Path) -> Result<BackupVerification> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let integrity_check: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity_check != "ok" {
        return Err(Error::RecoveryFailed(format!(
            "SQLite integrity_check failed for {}: {}",
            path.display(),
            integrity_check
        )));
    }

    Ok(BackupVerification {
        backup_path: path.to_path_buf(),
        db_schema_version: schema::get_schema_version(&conn)?,
        integrity_check,
        backup_sha256: sha256_file(path)?,
        sqlite_page_count: conn
            .query_row("PRAGMA page_count", [], |row| row.get(0))
            .ok(),
    })
}

fn generation_state_backup_dir(generation_dir: &Path) -> PathBuf {
    generation_dir.join("state")
}

fn read_generation_record(generation_dir: &Path) -> Result<GenerationDbBackupRecord> {
    let state_dir = generation_state_backup_dir(generation_dir);
    let manifest_path = state_dir.join(GENERATION_BACKUP_MANIFEST_FILE);
    let raw = std::fs::read(&manifest_path)?;
    let manifest: GenerationDbBackupManifest = serde_json::from_slice(&raw)?;
    let backup_rel = validate_generation_backup_file_name(&manifest.backup_file)?;
    let backup_path = state_dir.join(backup_rel);
    let checksum_path = state_dir.join(GENERATION_BACKUP_CHECKSUM_FILE);
    Ok(GenerationDbBackupRecord {
        backup_path,
        checksum_path,
        manifest_path,
        manifest,
    })
}

fn verify_generation_db_backup_record(
    record: &GenerationDbBackupRecord,
    current_root: Option<&Path>,
) -> Result<GenerationDbBackupVerification> {
    let generation_dir = record
        .manifest_path
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| Error::InvalidPath(record.manifest_path.display().to_string()))?;

    if record.manifest.format != GENERATION_BACKUP_FORMAT {
        return Err(Error::RecoveryFailed(format!(
            "unsupported generation DB backup manifest format: {}",
            record.manifest.format
        )));
    }
    if record.manifest.manifest_version != MANIFEST_VERSION {
        return Err(Error::RecoveryFailed(format!(
            "unsupported generation DB backup manifest version: {}",
            record.manifest.manifest_version
        )));
    }
    if record.manifest.compression.is_some() {
        return Err(Error::RecoveryFailed(
            "compressed generation DB backups are not supported by this recovery path yet"
                .to_string(),
        ));
    }
    if record.manifest.backup_file != GENERATION_BACKUP_FILE {
        return Err(Error::InvalidPath(format!(
            "generation DB backup file must be {GENERATION_BACKUP_FILE}, got {}",
            record.manifest.backup_file
        )));
    }
    validate_generation_directory_number(generation_dir, record.manifest.generation_number)?;

    let artifact =
        crate::generation::artifact::load_generation_artifact_for_activation(generation_dir)?;
    if artifact.generation != record.manifest.generation_number {
        return Err(Error::RecoveryFailed(format!(
            "generation artifact mismatch: backup is for generation {}, artifact declares {}",
            record.manifest.generation_number, artifact.generation
        )));
    }
    if let Some(root) = current_root {
        let selected = crate::generation::mount::current_generation(root)?;
        if selected != Some(record.manifest.generation_number) {
            return Err(Error::RecoveryFailed(format!(
                "selected generation mismatch: /conary/current is {:?}, backup is for generation {}",
                selected, record.manifest.generation_number
            )));
        }
    }

    let actual_hash = sha256_file(&record.backup_path)?;
    if actual_hash != record.manifest.backup_sha256 {
        return Err(Error::ChecksumMismatch {
            expected: record.manifest.backup_sha256.clone(),
            actual: actual_hash,
        });
    }
    verify_checksum_sidecar(record)?;

    let verification = verify_sqlite_database(&record.backup_path)?;
    if verification.db_schema_version != schema::SCHEMA_VERSION {
        return Err(Error::RecoveryFailed(format!(
            "generation DB backup schema version {} is not supported by this Conary binary (expected {})",
            verification.db_schema_version,
            schema::SCHEMA_VERSION
        )));
    }
    if verification.db_schema_version != record.manifest.db_schema_version {
        return Err(Error::RecoveryFailed(format!(
            "generation DB backup schema version changed since manifest creation: manifest={}, backup={}",
            record.manifest.db_schema_version, verification.db_schema_version
        )));
    }
    if verification.sqlite_page_count != record.manifest.sqlite_page_count {
        return Err(Error::RecoveryFailed(format!(
            "generation DB backup page count changed since manifest creation: manifest={:?}, backup={:?}",
            record.manifest.sqlite_page_count, verification.sqlite_page_count
        )));
    }
    verify_generation_publication_state(&record.manifest, &record.backup_path)?;

    Ok(GenerationDbBackupVerification {
        backup_path: record.backup_path.clone(),
        manifest_path: record.manifest_path.clone(),
        generation_number: record.manifest.generation_number,
        state_number: record.manifest.state_number,
        db_schema_version: verification.db_schema_version,
        integrity_check: verification.integrity_check,
        backup_sha256: record.manifest.backup_sha256.clone(),
        sqlite_page_count: verification.sqlite_page_count,
    })
}

fn validate_generation_backup_file_name(file_name: &str) -> Result<&Path> {
    let path = Path::new(file_name);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(Error::InvalidPath(format!(
            "generation DB backup file must be a plain relative filename: {file_name}"
        )));
    }
    Ok(path)
}

fn validate_generation_directory_number(generation_dir: &Path, expected: i64) -> Result<()> {
    let actual = generation_dir
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.parse::<i64>().ok())
        .ok_or_else(|| {
            Error::InvalidPath(format!(
                "generation directory must end in a numeric generation: {}",
                generation_dir.display()
            ))
        })?;
    if actual != expected {
        return Err(Error::RecoveryFailed(format!(
            "generation directory mismatch: directory is {actual}, backup manifest says {expected}"
        )));
    }
    Ok(())
}

fn verify_checksum_sidecar(record: &GenerationDbBackupRecord) -> Result<()> {
    let raw = std::fs::read_to_string(&record.checksum_path)?;
    let expected = format!(
        "{}  {}\n",
        record.manifest.backup_sha256, record.manifest.backup_file
    );
    if raw != expected {
        return Err(Error::RecoveryFailed(format!(
            "generation DB backup checksum sidecar mismatch at {}",
            record.checksum_path.display()
        )));
    }
    Ok(())
}

fn verify_generation_publication_state(
    manifest: &GenerationDbBackupManifest,
    backup_path: &Path,
) -> Result<()> {
    let conn = Connection::open_with_flags(backup_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = conn.prepare(
        "SELECT phase, status, recoverable, state_number
         FROM generation_publications
         WHERE generation_number = ?1
         ORDER BY id DESC",
    )?;
    let mut rows = stmt.query([manifest.generation_number])?;
    while let Some(row) = rows.next()? {
        let phase: String = row.get(0)?;
        let status: String = row.get(1)?;
        let recoverable: i64 = row.get(2)?;
        let state_number: Option<i64> = row.get(3)?;
        if state_number != Some(manifest.state_number) {
            continue;
        }
        let complete = phase == "active_marked" && status == "complete" && recoverable == 0;
        let current_published =
            phase == "current_published" && status == "running" && recoverable == 1;
        if complete || current_published {
            return Ok(());
        }
    }

    Err(Error::RecoveryFailed(format!(
        "generation DB backup has no complete or current_published/running publication state for generation {} state {}",
        manifest.generation_number, manifest.state_number
    )))
}

fn verify_generation_backup_copy(
    backup_path: &Path,
    options: GenerationDbRecoveryOptions,
) -> Result<Option<PathBuf>> {
    let stamp = Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_else(|| Utc::now().timestamp_micros() * 1_000);
    let temp_path =
        std::env::temp_dir().join(format!("conary-generation-db-recover-{stamp}.sqlite"));
    std::fs::copy(backup_path, &temp_path)?;
    sync_file(&temp_path)?;
    verify_sqlite_database(&temp_path)?;
    if options.keep_temp {
        Ok(Some(temp_path))
    } else {
        remove_if_exists(&temp_path)?;
        Ok(None)
    }
}

fn live_db_is_healthy(path: &Path) -> bool {
    verify_sqlite_database(path).is_ok()
}

fn sqlite_database_paths(db_path: &Path) -> [PathBuf; 3] {
    [
        db_path.to_path_buf(),
        sqlite_sidecar_path(db_path, "-wal"),
        sqlite_sidecar_path(db_path, "-shm"),
    ]
}

fn sqlite_sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
    let mut path = OsString::from(db_path.as_os_str());
    path.push(suffix);
    PathBuf::from(path)
}

fn quarantine_path(path: &Path, stamp: &str) -> Result<PathBuf> {
    let file_name = path
        .file_name()
        .ok_or_else(|| Error::InvalidPath(path.display().to_string()))?
        .to_string_lossy();
    Ok(path.with_file_name(format!("{file_name}.recovery-backup.{stamp}")))
}

fn restore_temp_path(path: &Path) -> PathBuf {
    let mut temp = OsString::from(path.as_os_str());
    temp.push(".restore-tmp");
    PathBuf::from(temp)
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    sha256_reader_hex(&mut file).map_err(Error::Io)
}

fn sync_file(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            sync_parent_directory(path)?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::Io(error)),
    }
}
