---
paths:
  - "conary-core/src/transaction/**"
---

# Transaction Module

Crash-safe, atomic package operations with journal-based recovery. Provides
backup-before-overwrite semantics, VFS preflight conflict detection, and a
deterministic state machine for recovery at any failure point.

## Key Types
- `TransactionEngine` -- orchestrates the full transaction lifecycle
- `TransactionJournal` -- append-only log with CRC32 checksums per record
- `JournalRecord` -- tagged enum: `Begin`, `Plan`, `Backup`, `Stage`, `FsApplied`, `DbApplied`, etc.
- `TransactionPlanner` -- VFS-based conflict detection before any filesystem changes
- `PlannedOperation` -- single file operation (path, op_type, hash, mode)
- `ConflictInfo` -- conflict types: `FileOwnedByOther`, `UntrackedFileExists`, `DirectoryBlocksFile`
- `RecoveryOutcome` -- `RolledBack`, `RolledForward`, `CompletedPending`, `Corrupted`, `Clean`

## Transaction Lifecycle
```
NEW -> PLANNED -> PREPARED -> PRE_SCRIPTS -> BACKED_UP -> STAGED -> FS_APPLIED -> DB_APPLIED -> POST_SCRIPTS -> DONE
```
Point of no return is `DB_APPLIED` -- roll forward after that.

## Invariants
- Journal format: `{crc32_hex}|{json}\n` -- one record per line, fsync at phase barriers
- Before `DB_APPLIED`: roll back (restore backups, remove staged files)
- After `DB_APPLIED`: roll forward (cleanup temps, archive journal)
- DB is source of truth for whether `DB_APPLIED` succeeded (crash can occur after SQLite commits)
- `move_file_atomic()` handles cross-filesystem moves with copy+fsync+delete fallback (EXDEV)
- File lock via `fs2::FileExt` prevents concurrent transactions

## Gotchas
- `DbCommitIntent` record written before DB commit -- used to detect partial commits
- Ownership (uid/gid) preserved explicitly during cross-filesystem copy (fs::copy does not do this)
- `safe_join()` from filesystem module used for path safety in recovery
- `find_incomplete_journals()` scans journal dir for recovery candidates

## Files
- `mod.rs` -- `TransactionEngine`, `move_file_atomic()`, state machine, lifecycle
- `journal.rs` -- `TransactionJournal`, `JournalRecord`, CRC32 integrity
- `planner.rs` -- `TransactionPlanner`, `PlannedOperation`, `ConflictInfo`, `BackupInfo`
- `recovery.rs` -- `recover_all()`, `RecoveryOutcome`, roll-back/roll-forward logic
