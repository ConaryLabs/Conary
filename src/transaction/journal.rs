// src/transaction/journal.rs

//! Append-only transaction journal for crash recovery
//!
//! The journal provides a durable record of transaction progress. Each record
//! is written as a single line with CRC32 checksum for integrity verification.
//!
//! Format: `{crc32_hex}|{json}\n`
//!
//! Phase barriers use fsync to ensure durability before proceeding to
//! destructive operations.

use crate::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use super::{FileType, PlannedOperation, TransactionState};

/// A record in the transaction journal
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum JournalRecord {
    /// Transaction started
    Begin {
        tx_uuid: String,
        root: PathBuf,
        db_path: PathBuf,
        description: String,
        timestamp: DateTime<Utc>,
    },

    /// Full operation plan
    Plan {
        operations: Vec<PlannedOperation>,
        package_name: String,
        package_version: String,
        is_upgrade: bool,
        old_version: Option<String>,
    },

    /// Content prepared in CAS
    Prepared { files_in_cas: usize, total_bytes: u64 },

    /// Pre-scriptlet executed
    PreScriptComplete { exit_code: i32, duration_ms: u64 },

    /// Single file backed up
    Backup {
        path: PathBuf,
        backup_path: PathBuf,
        old_type: FileType,
        old_hash: Option<String>,
        old_mode: u32,
        old_size: u64,
    },

    /// All backups complete barrier
    BackupsComplete { count: usize },

    /// Single file staged
    Stage {
        path: PathBuf,
        stage_path: PathBuf,
        new_hash: String,
        new_mode: u32,
        new_type: FileType,
    },

    /// All staging complete barrier
    StagingComplete { count: usize },

    /// Filesystem changes applied
    FsApplied {
        files_added: usize,
        files_replaced: usize,
        files_removed: usize,
        dirs_created: usize,
    },

    /// About to commit DB (correlation key for recovery)
    DbCommitIntent { tx_uuid: String },

    /// DB transaction committed
    DbApplied { changeset_id: i64, trove_id: i64 },

    /// Post-action result
    PostAction {
        action_type: PostActionType,
        name: String,
        success: bool,
        exit_code: Option<i32>,
        error: Option<String>,
    },

    /// Transaction complete
    Done { duration_ms: u64, success: bool },
}

/// Type of post-install action
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PostActionType {
    PostInstall,
    PostRemove,
    Trigger,
}

impl JournalRecord {
    /// Get the transaction state this record represents
    pub fn to_state(&self) -> TransactionState {
        match self {
            Self::Begin { .. } => TransactionState::New,
            Self::Plan { .. } => TransactionState::Planned,
            Self::Prepared { .. } => TransactionState::Prepared,
            Self::PreScriptComplete { .. } => TransactionState::PreScriptsComplete,
            Self::Backup { .. } | Self::BackupsComplete { .. } => TransactionState::BackedUp,
            Self::Stage { .. } | Self::StagingComplete { .. } => TransactionState::Staged,
            Self::FsApplied { .. } | Self::DbCommitIntent { .. } => TransactionState::FsApplied,
            Self::DbApplied { .. } => TransactionState::DbApplied,
            Self::PostAction { .. } => TransactionState::PostScriptsComplete,
            Self::Done { .. } => TransactionState::Done,
        }
    }

    /// Check if this is a phase barrier record
    pub fn is_barrier(&self) -> bool {
        matches!(
            self,
            Self::Begin { .. }
                | Self::Plan { .. }
                | Self::Prepared { .. }
                | Self::PreScriptComplete { .. }
                | Self::BackupsComplete { .. }
                | Self::StagingComplete { .. }
                | Self::FsApplied { .. }
                | Self::DbCommitIntent { .. }
                | Self::DbApplied { .. }
                | Self::Done { .. }
        )
    }
}

/// Append-only transaction journal with fsync barriers
pub struct TransactionJournal {
    path: PathBuf,
    file: File,
    tx_uuid: String,
    sequence: u64,
}

impl TransactionJournal {
    /// Create a new journal for a transaction
    pub fn create(journal_dir: &Path, tx_uuid: &str) -> Result<Self> {
        fs::create_dir_all(journal_dir)?;

        let path = journal_dir.join(format!("tx-{}.journal", tx_uuid));
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)?;

        Ok(Self {
            path,
            file,
            tx_uuid: tx_uuid.to_string(),
            sequence: 0,
        })
    }

    /// Create a placeholder journal (used when replacing ownership)
    pub fn create_placeholder() -> Result<Self> {
        // Create a temp file that will be dropped
        let path = std::env::temp_dir().join(format!("conary-placeholder-{}.journal", uuid::Uuid::new_v4()));
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)?;

        Ok(Self {
            path,
            file,
            tx_uuid: String::new(),
            sequence: 0,
        })
    }

    /// Open an existing journal for recovery
    pub fn open(path: PathBuf) -> Result<Self> {
        // Extract tx_uuid from filename
        let filename = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| crate::Error::IoError("Invalid journal filename".to_string()))?;

        let tx_uuid = filename
            .strip_prefix("tx-")
            .ok_or_else(|| crate::Error::IoError("Invalid journal filename format".to_string()))?
            .to_string();

        // Count existing records
        let sequence = {
            let file = File::open(&path)?;
            BufReader::new(file).lines().count() as u64
        };

        let file = OpenOptions::new().append(true).open(&path)?;

        Ok(Self {
            path,
            file,
            tx_uuid,
            sequence,
        })
    }

    /// Get the transaction UUID
    pub fn tx_uuid(&self) -> &str {
        &self.tx_uuid
    }

    /// Get the journal file path
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Write a record to the journal (does NOT fsync)
    pub fn write(&mut self, record: JournalRecord) -> Result<()> {
        self.sequence += 1;
        let json = serde_json::to_string(&record).map_err(|e| {
            crate::Error::IoError(format!("Failed to serialize journal record: {}", e))
        })?;
        let crc = crc32fast::hash(json.as_bytes());
        writeln!(self.file, "{:08x}|{}", crc, json)?;
        Ok(())
    }

    /// Write a record and fsync (for phase barriers)
    pub fn write_barrier(&mut self, record: JournalRecord) -> Result<()> {
        self.write(record)?;
        self.file.flush()?;
        self.file.sync_all()?;
        Ok(())
    }

    /// Read all valid records from journal
    pub fn read_all(&self) -> Result<Vec<JournalRecord>> {
        let file = File::open(&self.path)?;
        let reader = BufReader::new(file);
        let mut records = Vec::new();

        for (line_num, line_result) in reader.lines().enumerate() {
            let line = line_result?;
            if line.is_empty() {
                continue;
            }

            // Parse format: {crc32}|{json}
            let parts: Vec<&str> = line.splitn(2, '|').collect();
            if parts.len() != 2 {
                log::warn!("Malformed journal line {}: missing delimiter", line_num + 1);
                continue;
            }

            let expected_crc = u32::from_str_radix(parts[0], 16).map_err(|_| {
                crate::Error::IoError(format!(
                    "Invalid CRC32 at line {}: {}",
                    line_num + 1,
                    parts[0]
                ))
            })?;

            let actual_crc = crc32fast::hash(parts[1].as_bytes());
            if expected_crc != actual_crc {
                log::warn!(
                    "CRC mismatch at line {}: expected {:08x}, got {:08x}",
                    line_num + 1,
                    expected_crc,
                    actual_crc
                );
                // Stop reading at first corrupted record
                break;
            }

            let record: JournalRecord = serde_json::from_str(parts[1]).map_err(|e| {
                crate::Error::IoError(format!(
                    "Failed to parse journal record at line {}: {}",
                    line_num + 1,
                    e
                ))
            })?;

            records.push(record);
        }

        Ok(records)
    }

    /// Get the last phase barrier state reached
    pub fn last_phase(&self) -> Result<TransactionState> {
        let records = self.read_all()?;

        // Find the last barrier record
        let last_barrier = records.iter().rev().find(|r| r.is_barrier());

        match last_barrier {
            Some(record) => Ok(record.to_state()),
            None => Ok(TransactionState::New),
        }
    }

    /// Get the transaction UUID from records (for validation)
    pub fn get_tx_uuid_from_records(&self) -> Result<Option<String>> {
        let records = self.read_all()?;
        for record in records {
            if let JournalRecord::Begin { tx_uuid, .. } = record {
                return Ok(Some(tx_uuid));
            }
        }
        Ok(None)
    }

    /// Archive the journal after successful completion
    pub fn archive(self) -> Result<()> {
        let archive_dir = self
            .path
            .parent()
            .unwrap_or(Path::new("."))
            .join("archive");
        fs::create_dir_all(&archive_dir)?;

        let archive_path = archive_dir.join(self.path.file_name().unwrap());
        fs::rename(&self.path, archive_path)?;

        Ok(())
    }

    /// Delete the journal (for aborted transactions)
    pub fn delete(self) -> Result<()> {
        if self.path.exists() {
            fs::remove_file(&self.path)?;
        }
        Ok(())
    }
}

/// Find incomplete transaction journals in a directory
pub fn find_incomplete_journals(journal_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut journals = Vec::new();

    if !journal_dir.exists() {
        return Ok(journals);
    }

    for entry in fs::read_dir(journal_dir)? {
        let entry = entry?;
        let path = entry.path();

        // Only look at .journal files (not in archive subdirectory)
        if path.is_file()
            && path.extension().is_some_and(|e| e == "journal")
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("tx-"))
        {
            // Check if journal is complete
            let journal = TransactionJournal::open(path.clone())?;
            let records = journal.read_all()?;

            // A journal is incomplete if it doesn't have a Done record
            let has_done = records
                .iter()
                .any(|r| matches!(r, JournalRecord::Done { .. }));

            if !has_done {
                journals.push(path);
            }
        }
    }

    Ok(journals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_journal_create_and_write() {
        let temp_dir = TempDir::new().unwrap();
        let tx_uuid = "test-uuid-12345";

        let mut journal = TransactionJournal::create(temp_dir.path(), tx_uuid).unwrap();

        // Write a begin record
        journal
            .write(JournalRecord::Begin {
                tx_uuid: tx_uuid.to_string(),
                root: PathBuf::from("/"),
                db_path: PathBuf::from("/var/lib/conary/conary.db"),
                description: "Test transaction".to_string(),
                timestamp: Utc::now(),
            })
            .unwrap();

        // Write a barrier
        journal
            .write_barrier(JournalRecord::Prepared {
                files_in_cas: 10,
                total_bytes: 1024,
            })
            .unwrap();

        // Read back
        let records = journal.read_all().unwrap();
        assert_eq!(records.len(), 2);
        assert!(matches!(records[0], JournalRecord::Begin { .. }));
        assert!(matches!(records[1], JournalRecord::Prepared { .. }));
    }

    #[test]
    fn test_journal_crc_verification() {
        let temp_dir = TempDir::new().unwrap();
        let tx_uuid = "test-uuid-crc";

        let mut journal = TransactionJournal::create(temp_dir.path(), tx_uuid).unwrap();

        journal
            .write(JournalRecord::Begin {
                tx_uuid: tx_uuid.to_string(),
                root: PathBuf::from("/"),
                db_path: PathBuf::from("/test.db"),
                description: "CRC test".to_string(),
                timestamp: Utc::now(),
            })
            .unwrap();

        // Read and verify
        let records = journal.read_all().unwrap();
        assert_eq!(records.len(), 1);
    }

    #[test]
    fn test_journal_last_phase() {
        let temp_dir = TempDir::new().unwrap();
        let tx_uuid = "test-uuid-phase";

        let mut journal = TransactionJournal::create(temp_dir.path(), tx_uuid).unwrap();

        journal
            .write_barrier(JournalRecord::Begin {
                tx_uuid: tx_uuid.to_string(),
                root: PathBuf::from("/"),
                db_path: PathBuf::from("/test.db"),
                description: "Phase test".to_string(),
                timestamp: Utc::now(),
            })
            .unwrap();

        assert_eq!(journal.last_phase().unwrap(), TransactionState::New);

        journal
            .write_barrier(JournalRecord::Plan {
                operations: vec![],
                package_name: "test".to_string(),
                package_version: "1.0".to_string(),
                is_upgrade: false,
                old_version: None,
            })
            .unwrap();

        assert_eq!(journal.last_phase().unwrap(), TransactionState::Planned);

        journal
            .write_barrier(JournalRecord::DbApplied {
                changeset_id: 1,
                trove_id: 1,
            })
            .unwrap();

        assert_eq!(journal.last_phase().unwrap(), TransactionState::DbApplied);
    }

    #[test]
    fn test_journal_open_existing() {
        let temp_dir = TempDir::new().unwrap();
        let tx_uuid = "test-uuid-open";

        // Create and write
        {
            let mut journal = TransactionJournal::create(temp_dir.path(), tx_uuid).unwrap();
            journal
                .write(JournalRecord::Begin {
                    tx_uuid: tx_uuid.to_string(),
                    root: PathBuf::from("/"),
                    db_path: PathBuf::from("/test.db"),
                    description: "Open test".to_string(),
                    timestamp: Utc::now(),
                })
                .unwrap();
        }

        // Reopen
        let journal_path = temp_dir.path().join(format!("tx-{}.journal", tx_uuid));
        let journal = TransactionJournal::open(journal_path).unwrap();

        assert_eq!(journal.tx_uuid(), tx_uuid);
        assert_eq!(journal.read_all().unwrap().len(), 1);
    }

    #[test]
    fn test_find_incomplete_journals() {
        let temp_dir = TempDir::new().unwrap();

        // Create incomplete journal
        {
            let mut journal = TransactionJournal::create(temp_dir.path(), "incomplete-1").unwrap();
            journal
                .write(JournalRecord::Begin {
                    tx_uuid: "incomplete-1".to_string(),
                    root: PathBuf::from("/"),
                    db_path: PathBuf::from("/test.db"),
                    description: "Incomplete".to_string(),
                    timestamp: Utc::now(),
                })
                .unwrap();
        }

        // Create complete journal
        {
            let mut journal = TransactionJournal::create(temp_dir.path(), "complete-1").unwrap();
            journal
                .write(JournalRecord::Begin {
                    tx_uuid: "complete-1".to_string(),
                    root: PathBuf::from("/"),
                    db_path: PathBuf::from("/test.db"),
                    description: "Complete".to_string(),
                    timestamp: Utc::now(),
                })
                .unwrap();
            journal
                .write(JournalRecord::Done {
                    duration_ms: 100,
                    success: true,
                })
                .unwrap();
        }

        let incomplete = find_incomplete_journals(temp_dir.path()).unwrap();
        assert_eq!(incomplete.len(), 1);
        assert!(incomplete[0].to_string_lossy().contains("incomplete-1"));
    }

    #[test]
    fn test_journal_archive() {
        let temp_dir = TempDir::new().unwrap();
        let tx_uuid = "test-archive";

        let mut journal = TransactionJournal::create(temp_dir.path(), tx_uuid).unwrap();
        journal
            .write(JournalRecord::Done {
                duration_ms: 100,
                success: true,
            })
            .unwrap();

        let original_path = journal.path().to_path_buf();
        journal.archive().unwrap();

        assert!(!original_path.exists());
        assert!(temp_dir
            .path()
            .join("archive")
            .join(format!("tx-{}.journal", tx_uuid))
            .exists());
    }

    #[test]
    fn test_record_to_state() {
        assert_eq!(
            JournalRecord::Begin {
                tx_uuid: String::new(),
                root: PathBuf::new(),
                db_path: PathBuf::new(),
                description: String::new(),
                timestamp: Utc::now(),
            }
            .to_state(),
            TransactionState::New
        );

        assert_eq!(
            JournalRecord::DbApplied {
                changeset_id: 1,
                trove_id: 1
            }
            .to_state(),
            TransactionState::DbApplied
        );

        assert_eq!(
            JournalRecord::Done {
                duration_ms: 0,
                success: true
            }
            .to_state(),
            TransactionState::Done
        );
    }
}
