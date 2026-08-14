// conary-core/src/db/generation_snapshot.rs

//! Typed, crash-retryable SQLite snapshots for generation publication.

use crate::filesystem::durable::sync_parent_directory;
use crate::{Error, Result};
use rusqlite::{Connection, MAIN_DB};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Exact mechanism that created a generation database snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GenerationDbSnapshotMethod {
    Reflink,
    SqliteOnlineBackup,
    FullCopy,
}

impl GenerationDbSnapshotMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reflink => "reflink",
            Self::SqliteOnlineBackup => "sqlite-online-backup",
            Self::FullCopy => "full-copy",
        }
    }
}

/// Typed reason that a faster snapshot provider could not be selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GenerationDbSnapshotFallbackReason {
    WalCheckpointBusy,
    PlatformUnsupported,
    FilesystemUnsupported,
    CrossDevice,
    OnlineBackupUnavailable,
}

impl GenerationDbSnapshotFallbackReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WalCheckpointBusy => "wal-checkpoint-busy",
            Self::PlatformUnsupported => "platform-unsupported",
            Self::FilesystemUnsupported => "filesystem-unsupported",
            Self::CrossDevice => "cross-device",
            Self::OnlineBackupUnavailable => "online-backup-unavailable",
        }
    }
}

/// One provider that was considered but could not create the snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationDbSnapshotFallback {
    pub method: GenerationDbSnapshotMethod,
    pub reason: GenerationDbSnapshotFallbackReason,
}

/// Exact WAL state established before a raw main-database snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationDbWalCheckpoint {
    pub journal_mode: String,
    pub log_frames: i64,
    pub checkpointed_frames: i64,
}

/// Persisted provider provenance for a generation database snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationDbSnapshotProvenance {
    pub method: GenerationDbSnapshotMethod,
    pub fallbacks: Vec<GenerationDbSnapshotFallback>,
    pub wal_checkpoint: Option<GenerationDbWalCheckpoint>,
    /// Logical SQLite payload bytes copied by the selected provider.
    ///
    /// Reflink shares existing extents and therefore records zero payload
    /// bytes even though the destination has a non-zero logical length.
    pub payload_bytes_written: u64,
    pub logical_size: u64,
}

impl GenerationDbSnapshotProvenance {
    pub fn validate(&self) -> Result<()> {
        if self.logical_size == 0 {
            return Err(Error::RecoveryFailed(
                "generation DB snapshot has zero logical size".to_string(),
            ));
        }
        if let Some(checkpoint) = &self.wal_checkpoint
            && (checkpoint.log_frames < 0
                || checkpoint.checkpointed_frames < 0
                || checkpoint.log_frames != checkpoint.checkpointed_frames)
        {
            return Err(Error::RecoveryFailed(format!(
                "generation DB snapshot records an incomplete WAL checkpoint: {checkpoint:?}"
            )));
        }

        let expected_fallbacks: &[GenerationDbSnapshotMethod] = match self.method {
            GenerationDbSnapshotMethod::Reflink => &[],
            GenerationDbSnapshotMethod::SqliteOnlineBackup => {
                &[GenerationDbSnapshotMethod::Reflink]
            }
            GenerationDbSnapshotMethod::FullCopy => &[
                GenerationDbSnapshotMethod::Reflink,
                GenerationDbSnapshotMethod::SqliteOnlineBackup,
            ],
        };
        let actual_fallbacks = self
            .fallbacks
            .iter()
            .map(|fallback| fallback.method)
            .collect::<Vec<_>>();
        if actual_fallbacks != expected_fallbacks {
            return Err(Error::RecoveryFailed(format!(
                "generation DB snapshot provider order is invalid: selected={:?} fallbacks={actual_fallbacks:?}",
                self.method
            )));
        }

        match self.method {
            GenerationDbSnapshotMethod::Reflink => {
                if self.wal_checkpoint.is_none() || self.payload_bytes_written != 0 {
                    return Err(Error::RecoveryFailed(
                        "reflink generation DB snapshot requires a WAL checkpoint and zero payload bytes written"
                            .to_string(),
                    ));
                }
            }
            GenerationDbSnapshotMethod::SqliteOnlineBackup => {
                if self.payload_bytes_written != self.logical_size {
                    return Err(Error::RecoveryFailed(format!(
                        "SQLite online backup payload byte count {} does not match logical size {}",
                        self.payload_bytes_written, self.logical_size
                    )));
                }
            }
            GenerationDbSnapshotMethod::FullCopy => {
                if self.wal_checkpoint.is_none() || self.payload_bytes_written != self.logical_size
                {
                    return Err(Error::RecoveryFailed(
                        "full-copy generation DB snapshot requires a WAL checkpoint and complete payload copy"
                            .to_string(),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Runtime measurements kept out of persisted generation authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenerationDbSnapshotMetrics {
    pub provider_elapsed_ns: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerationDbSnapshot {
    pub path: PathBuf,
    pub provenance: GenerationDbSnapshotProvenance,
    pub metrics: GenerationDbSnapshotMetrics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GenerationDbSnapshotProviderOutcome {
    Created { payload_bytes_written: u64 },
    Unsupported(GenerationDbSnapshotFallbackReason),
}

/// One concrete snapshot mechanism behind the provider-selection boundary.
trait SnapshotProvider {
    fn method(&self) -> GenerationDbSnapshotMethod;

    /// Whether this provider reads the SQLite main file directly and therefore
    /// requires a complete, synchronized WAL checkpoint first.
    fn requires_checkpointed_main(&self) -> bool;

    fn create(
        &self,
        source: &Connection,
        source_path: &Path,
        destination: &Path,
    ) -> Result<GenerationDbSnapshotProviderOutcome>;
}

#[derive(Debug, Default)]
struct ReflinkSnapshot;

impl SnapshotProvider for ReflinkSnapshot {
    fn method(&self) -> GenerationDbSnapshotMethod {
        GenerationDbSnapshotMethod::Reflink
    }

    fn requires_checkpointed_main(&self) -> bool {
        true
    }

    fn create(
        &self,
        _source: &Connection,
        source_path: &Path,
        destination: &Path,
    ) -> Result<GenerationDbSnapshotProviderOutcome> {
        create_reflink(source_path, destination)
    }
}

#[derive(Debug, Default)]
struct SqliteBackupSnapshot;

impl SnapshotProvider for SqliteBackupSnapshot {
    fn method(&self) -> GenerationDbSnapshotMethod {
        GenerationDbSnapshotMethod::SqliteOnlineBackup
    }

    fn requires_checkpointed_main(&self) -> bool {
        false
    }

    fn create(
        &self,
        source: &Connection,
        _source_path: &Path,
        destination: &Path,
    ) -> Result<GenerationDbSnapshotProviderOutcome> {
        source.backup(MAIN_DB, destination, None)?;
        let bytes = destination.metadata()?.len();
        Ok(GenerationDbSnapshotProviderOutcome::Created {
            payload_bytes_written: bytes,
        })
    }
}

#[derive(Debug, Default)]
struct FullCopyFallback;

impl SnapshotProvider for FullCopyFallback {
    fn method(&self) -> GenerationDbSnapshotMethod {
        GenerationDbSnapshotMethod::FullCopy
    }

    fn requires_checkpointed_main(&self) -> bool {
        true
    }

    fn create(
        &self,
        _source: &Connection,
        source_path: &Path,
        destination: &Path,
    ) -> Result<GenerationDbSnapshotProviderOutcome> {
        let bytes = std::fs::copy(source_path, destination)?;
        Ok(GenerationDbSnapshotProviderOutcome::Created {
            payload_bytes_written: bytes,
        })
    }
}

/// Ordered provider selector used by normal generation publication.
pub struct GenerationDbSnapshotProvider {
    providers: Vec<Box<dyn SnapshotProvider>>,
}

impl Default for GenerationDbSnapshotProvider {
    fn default() -> Self {
        Self {
            providers: vec![
                Box::new(ReflinkSnapshot),
                Box::new(SqliteBackupSnapshot),
                Box::new(FullCopyFallback),
            ],
        }
    }
}

impl GenerationDbSnapshotProvider {
    /// Create one exact SQLite snapshot at `destination`.
    ///
    /// The caller owns mutation serialization for `source_path`. Providers
    /// that clone or copy the main file are admitted only after a successful
    /// truncate checkpoint and explicit synchronization. SQLite online backup
    /// remains exact when that checkpoint is temporarily busy.
    pub fn snapshot(
        &self,
        source: &Connection,
        source_path: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> Result<GenerationDbSnapshot> {
        let source_path = source_path.as_ref();
        let destination = destination.as_ref();
        validate_source_connection_path(source, source_path)?;
        let started = Instant::now();
        let mut fallbacks = Vec::new();
        let mut checkpoint: Option<GenerationDbWalCheckpoint> = None;
        let mut checkpoint_busy = false;

        for provider in &self.providers {
            if provider.requires_checkpointed_main() && checkpoint.is_none() && !checkpoint_busy {
                match checkpoint_and_sync(source, source_path)? {
                    CheckpointOutcome::Complete(value) => checkpoint = Some(value),
                    CheckpointOutcome::Busy => checkpoint_busy = true,
                }
            }
            if provider.requires_checkpointed_main() && checkpoint.is_none() {
                fallbacks.push(GenerationDbSnapshotFallback {
                    method: provider.method(),
                    reason: GenerationDbSnapshotFallbackReason::WalCheckpointBusy,
                });
                continue;
            }

            remove_partial(destination)?;
            match provider.create(source, source_path, destination)? {
                GenerationDbSnapshotProviderOutcome::Created {
                    payload_bytes_written,
                } => {
                    let mut permissions = destination.metadata()?.permissions();
                    permissions.set_mode(0o600);
                    std::fs::set_permissions(destination, permissions)?;
                    File::open(destination)?.sync_all()?;
                    sync_parent_directory(destination)?;
                    let logical_size = destination.metadata()?.len();
                    let provider_elapsed_ns =
                        started.elapsed().as_nanos().try_into().unwrap_or(u64::MAX);
                    let provenance = GenerationDbSnapshotProvenance {
                        method: provider.method(),
                        fallbacks,
                        wal_checkpoint: checkpoint,
                        payload_bytes_written,
                        logical_size,
                    };
                    provenance.validate()?;
                    return Ok(GenerationDbSnapshot {
                        path: destination.to_path_buf(),
                        provenance,
                        metrics: GenerationDbSnapshotMetrics {
                            provider_elapsed_ns,
                        },
                    });
                }
                GenerationDbSnapshotProviderOutcome::Unsupported(reason) => {
                    remove_partial(destination)?;
                    fallbacks.push(GenerationDbSnapshotFallback {
                        method: provider.method(),
                        reason,
                    });
                }
            }
        }

        Err(Error::RecoveryFailed(format!(
            "no generation DB snapshot provider could create an exact snapshot: {fallbacks:?}"
        )))
    }

    #[cfg(test)]
    fn with_providers(providers: Vec<Box<dyn SnapshotProvider>>) -> Self {
        Self { providers }
    }
}

enum CheckpointOutcome {
    Complete(GenerationDbWalCheckpoint),
    Busy,
}

fn checkpoint_and_sync(source: &Connection, source_path: &Path) -> Result<CheckpointOutcome> {
    let journal_mode: String = source.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        File::open(source_path)?.sync_all()?;
        sync_parent_directory(source_path)?;
        return Ok(CheckpointOutcome::Complete(GenerationDbWalCheckpoint {
            journal_mode,
            log_frames: 0,
            checkpointed_frames: 0,
        }));
    }
    let (busy, log_frames, checkpointed_frames): (i64, i64, i64) =
        source.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    if busy != 0 || log_frames != checkpointed_frames {
        return Ok(CheckpointOutcome::Busy);
    }

    File::open(source_path)?.sync_all()?;
    let wal_path = sqlite_sidecar_path(source_path, "-wal");
    if wal_path.exists() {
        File::open(&wal_path)?.sync_all()?;
    }
    sync_parent_directory(source_path)?;

    Ok(CheckpointOutcome::Complete(GenerationDbWalCheckpoint {
        journal_mode,
        log_frames,
        checkpointed_frames,
    }))
}

fn validate_source_connection_path(source: &Connection, source_path: &Path) -> Result<()> {
    let connected_path: String = source.query_row(
        "SELECT file FROM pragma_database_list WHERE name = 'main'",
        [],
        |row| row.get(0),
    )?;
    let connected = Path::new(&connected_path).canonicalize()?;
    let requested = source_path.canonicalize()?;
    if connected != requested {
        return Err(Error::RecoveryFailed(format!(
            "generation DB snapshot source mismatch: connection={} requested={}",
            connected.display(),
            requested.display()
        )));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn create_reflink(
    source_path: &Path,
    destination: &Path,
) -> Result<GenerationDbSnapshotProviderOutcome> {
    let source = File::open(source_path)?;
    let destination_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(destination)?;
    // SAFETY: both descriptors remain open for the ioctl; the destination was
    // created empty and exclusively for this snapshot attempt.
    let result = unsafe {
        libc::ioctl(
            destination_file.as_raw_fd(),
            libc::FICLONE,
            source.as_raw_fd(),
        )
    };
    if result == 0 {
        return Ok(GenerationDbSnapshotProviderOutcome::Created {
            payload_bytes_written: 0,
        });
    }

    let error = std::io::Error::last_os_error();
    let reason = match error.raw_os_error() {
        Some(libc::EXDEV) => Some(GenerationDbSnapshotFallbackReason::CrossDevice),
        Some(libc::EOPNOTSUPP) | Some(libc::ENOTTY) | Some(libc::EINVAL) | Some(libc::ENOSYS) => {
            Some(GenerationDbSnapshotFallbackReason::FilesystemUnsupported)
        }
        _ => None,
    };
    match reason {
        Some(reason) => Ok(GenerationDbSnapshotProviderOutcome::Unsupported(reason)),
        None => Err(Error::Io(error)),
    }
}

#[cfg(not(target_os = "linux"))]
fn create_reflink(
    _source_path: &Path,
    _destination: &Path,
) -> Result<GenerationDbSnapshotProviderOutcome> {
    Ok(GenerationDbSnapshotProviderOutcome::Unsupported(
        GenerationDbSnapshotFallbackReason::PlatformUnsupported,
    ))
}

fn sqlite_sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
    let mut path = db_path.as_os_str().to_os_string();
    path.push(suffix);
    PathBuf::from(path)
}

fn remove_partial(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct UnsupportedProvider {
        method: GenerationDbSnapshotMethod,
        checkpointed: bool,
        reason: GenerationDbSnapshotFallbackReason,
    }

    impl SnapshotProvider for UnsupportedProvider {
        fn method(&self) -> GenerationDbSnapshotMethod {
            self.method
        }

        fn requires_checkpointed_main(&self) -> bool {
            self.checkpointed
        }

        fn create(
            &self,
            _source: &Connection,
            _source_path: &Path,
            _destination: &Path,
        ) -> Result<GenerationDbSnapshotProviderOutcome> {
            Ok(GenerationDbSnapshotProviderOutcome::Unsupported(
                self.reason,
            ))
        }
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("source.sqlite");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE authority (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO authority (value) VALUES ('committed through WAL');",
        )
        .unwrap();
        (temp, db_path, conn)
    }

    #[test]
    fn default_chain_snapshots_exact_committed_wal_state() {
        let (temp, db_path, conn) = fixture();
        let destination = temp.path().join("snapshot.sqlite");

        let snapshot = GenerationDbSnapshotProvider::default()
            .snapshot(&conn, &db_path, &destination)
            .unwrap();

        let copied =
            Connection::open_with_flags(&snapshot.path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let value: String = copied
            .query_row("SELECT value FROM authority", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "committed through WAL");
        assert_eq!(
            snapshot.provenance.logical_size,
            destination.metadata().unwrap().len()
        );
        assert!(snapshot.provenance.wal_checkpoint.is_some());
    }

    #[test]
    fn rollback_journal_source_records_synchronized_zero_wal_frames() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("source.sqlite");
        let destination = temp.path().join("snapshot.sqlite");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE authority (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO authority (value) VALUES ('committed');",
        )
        .unwrap();

        let snapshot = GenerationDbSnapshotProvider::default()
            .snapshot(&conn, &db_path, &destination)
            .unwrap();
        let checkpoint = snapshot.provenance.wal_checkpoint.unwrap();

        assert_eq!(checkpoint.journal_mode, "delete");
        assert_eq!(checkpoint.log_frames, 0);
        assert_eq!(checkpoint.checkpointed_frames, 0);
        assert_eq!(
            Connection::open(destination)
                .unwrap()
                .query_row("SELECT value FROM authority", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "committed"
        );
    }

    #[test]
    fn unsupported_reflink_falls_back_to_sqlite_online_backup() {
        let (temp, db_path, conn) = fixture();
        let destination = temp.path().join("snapshot.sqlite");
        let providers = GenerationDbSnapshotProvider::with_providers(vec![
            Box::new(UnsupportedProvider {
                method: GenerationDbSnapshotMethod::Reflink,
                checkpointed: true,
                reason: GenerationDbSnapshotFallbackReason::FilesystemUnsupported,
            }),
            Box::new(SqliteBackupSnapshot),
            Box::new(FullCopyFallback),
        ]);

        let snapshot = providers.snapshot(&conn, &db_path, &destination).unwrap();

        assert_eq!(
            snapshot.provenance.method,
            GenerationDbSnapshotMethod::SqliteOnlineBackup
        );
        assert_eq!(
            snapshot.provenance.fallbacks,
            vec![GenerationDbSnapshotFallback {
                method: GenerationDbSnapshotMethod::Reflink,
                reason: GenerationDbSnapshotFallbackReason::FilesystemUnsupported,
            }]
        );
        assert_eq!(
            snapshot.provenance.payload_bytes_written,
            destination.metadata().unwrap().len()
        );
    }

    #[test]
    fn busy_wal_checkpoint_uses_online_backup_without_losing_committed_frames() {
        let (temp, db_path, conn) = fixture();
        let reader = Connection::open(&db_path).unwrap();
        reader.execute_batch("BEGIN").unwrap();
        let _: i64 = reader
            .query_row("SELECT COUNT(*) FROM authority", [], |row| row.get(0))
            .unwrap();
        conn.execute(
            "INSERT INTO authority (value) VALUES ('committed after reader snapshot')",
            [],
        )
        .unwrap();
        let destination = temp.path().join("snapshot.sqlite");

        let snapshot = GenerationDbSnapshotProvider::default()
            .snapshot(&conn, &db_path, &destination)
            .unwrap();

        assert_eq!(
            snapshot.provenance.method,
            GenerationDbSnapshotMethod::SqliteOnlineBackup
        );
        assert_eq!(
            snapshot.provenance.fallbacks,
            vec![GenerationDbSnapshotFallback {
                method: GenerationDbSnapshotMethod::Reflink,
                reason: GenerationDbSnapshotFallbackReason::WalCheckpointBusy,
            }]
        );
        let copied =
            Connection::open_with_flags(&destination, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let count: i64 = copied
            .query_row("SELECT COUNT(*) FROM authority", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn full_copy_is_used_only_after_both_faster_providers_are_unavailable() {
        let (temp, db_path, conn) = fixture();
        let destination = temp.path().join("snapshot.sqlite");
        let providers = GenerationDbSnapshotProvider::with_providers(vec![
            Box::new(UnsupportedProvider {
                method: GenerationDbSnapshotMethod::Reflink,
                checkpointed: true,
                reason: GenerationDbSnapshotFallbackReason::FilesystemUnsupported,
            }),
            Box::new(UnsupportedProvider {
                method: GenerationDbSnapshotMethod::SqliteOnlineBackup,
                checkpointed: false,
                reason: GenerationDbSnapshotFallbackReason::OnlineBackupUnavailable,
            }),
            Box::new(FullCopyFallback),
        ]);

        let snapshot = providers.snapshot(&conn, &db_path, &destination).unwrap();

        assert_eq!(
            snapshot.provenance.method,
            GenerationDbSnapshotMethod::FullCopy
        );
        assert_eq!(snapshot.provenance.fallbacks.len(), 2);
        assert_eq!(
            snapshot.provenance.payload_bytes_written,
            destination.metadata().unwrap().len()
        );
    }

    #[test]
    fn source_connection_must_name_the_exact_database_path() {
        let (temp, db_path, conn) = fixture();
        let other_path = temp.path().join("other.sqlite");
        Connection::open(&other_path).unwrap();

        let error = GenerationDbSnapshotProvider::default()
            .snapshot(&conn, &other_path, temp.path().join("snapshot.sqlite"))
            .unwrap_err();

        assert!(error.to_string().contains("source mismatch"));
        assert!(db_path.exists());
    }

    #[test]
    fn reflink_provider_is_byte_exact_when_the_filesystem_supports_it() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        std::fs::write(&source, b"reflink snapshot authority").unwrap();

        match create_reflink(&source, &destination).unwrap() {
            GenerationDbSnapshotProviderOutcome::Unsupported(reason) => assert!(matches!(
                reason,
                GenerationDbSnapshotFallbackReason::PlatformUnsupported
                    | GenerationDbSnapshotFallbackReason::FilesystemUnsupported
                    | GenerationDbSnapshotFallbackReason::CrossDevice
            )),
            GenerationDbSnapshotProviderOutcome::Created {
                payload_bytes_written,
            } => {
                assert_eq!(payload_bytes_written, 0);
                assert_eq!(
                    std::fs::read(&destination).unwrap(),
                    b"reflink snapshot authority"
                );
                std::fs::write(&destination, b"copy-on-write destination").unwrap();
                assert_eq!(
                    std::fs::read(&source).unwrap(),
                    b"reflink snapshot authority"
                );
            }
        }
    }

    #[test]
    fn persisted_provenance_rejects_impossible_provider_order() {
        let provenance = GenerationDbSnapshotProvenance {
            method: GenerationDbSnapshotMethod::FullCopy,
            fallbacks: vec![GenerationDbSnapshotFallback {
                method: GenerationDbSnapshotMethod::Reflink,
                reason: GenerationDbSnapshotFallbackReason::FilesystemUnsupported,
            }],
            wal_checkpoint: Some(GenerationDbWalCheckpoint {
                journal_mode: "wal".to_string(),
                log_frames: 0,
                checkpointed_frames: 0,
            }),
            payload_bytes_written: 4096,
            logical_size: 4096,
        };

        let error = provenance.validate().unwrap_err();
        assert!(error.to_string().contains("provider order is invalid"));
    }
}
