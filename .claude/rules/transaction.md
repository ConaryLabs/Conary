---
paths:
  - "conary-core/src/transaction/**"
---

# Transaction Module

Composefs-native transaction engine. Every transaction follows:
resolve -> fetch -> DB commit -> EROFS build -> mount. No journal, no backup,
no staging. The database is the source of truth; everything after DB commit
is re-derivable.

## Key Types
- `TransactionEngine` -- orchestrates the full transaction lifecycle (lock, CAS, recover)
- `TransactionConfig` -- root, db_path, objects_dir, generations_dir, etc_state_dir, mount_point
- `TransactionState` -- enum: `New`, `Resolved`, `Fetched`, `Committed`, `Built`, `Mounted`, `Done`
- `TransactionPlanner` -- VFS-based conflict detection before any changes
- `PlannedOperation` -- single file operation (path, op_type, hash, mode)
- `ConflictInfo` -- conflict types: `FileOwnedByOther`, `UntrackedFileExists`, `DirectoryBlocksFile`
- `TransactionResult` -- generation_number, duration_ms, packages_changed

## Transaction Lifecycle
```
NEW -> RESOLVED -> FETCHED -> COMMITTED -> BUILT -> MOUNTED -> DONE
```
Point of no return is `Committed` -- the DB has the new package state. Building
the EROFS image and mounting it are idempotent operations that can be retried.

## Recovery

Recovery replaces the old journal-based roll-forward/roll-back with a 4-step
fallback strategy:

1. Read `/conary/current` symlink; if the target EROFS image is valid, mount it.
2. If the image is missing or truncated, rebuild from DB state via
   `build_generation_from_db()`.
3. If the DB is corrupted, scan `generations/` descending by number and try
   each intact EROFS image.
4. If nothing works, return `RecoveryFailed`.

## Invariants
- File lock via `fs2::FileExt` prevents concurrent transactions
- Before `Committed`: discard the transaction with no side effects
- After `Committed`: recovery means re-deriving the EROFS image and remounting
- DB is the single source of truth for package state
- EROFS images are validated via magic number check at byte offset 1024

## Gotchas
- No journal, no backup directory, no staging area -- the old 10-state pipeline is gone
- `planner.rs` is kept for VfsTree-based conflict detection (preflight only)
- Compatibility types (`PackageInfo`, `TransactionOperations`, `ExtractedFile`,
  `FsApplyResult`) are preserved for CLI install/batch consumers during migration
- `TransactionConfig::from_paths()` derives `objects_dir` and `generations_dir`
  from the database directory

## Files
- `mod.rs` -- `TransactionEngine` (~890 lines), `TransactionState`, `TransactionConfig`,
  recovery logic, EROFS validation, compatibility types
- `planner.rs` -- `TransactionPlanner`, `PlannedOperation`, `ConflictInfo`, `BackupInfo`
  (VfsTree conflict detection retained from old system)
