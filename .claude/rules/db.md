---
paths:
  - "conary-core/src/db/**"
---

# Database Module

All runtime state lives in SQLite. No config files for runtime state -- this is a core
architectural invariant. The schema is at version v57 with 71 tables across 57 migrations.
Connection management uses `rusqlite::Connection` directly (no pool).

## Key Types
- `Connection` -- rusqlite connection, opened via `db::open()` or `db::init()`
- `Trove` -- core package record (name, version, type, architecture)
- `Changeset` -- atomic transaction record with `ChangesetStatus`
- `FileEntry` -- file owned by a trove (path, hash, mode, size)
- `Repository` / `RepositoryPackage` -- remote repo metadata
- `StateEngine` / `SystemState` -- declarative system state tracking
- `TriggerEngine` / `Trigger` -- post-install trigger definitions
- `AdminToken` -- admin API token (name, token_hash, scopes, created_at, last_used_at)
- `AuditEntry` -- admin audit log entry (token_name, action, method, path, status_code, request/response bodies, duration_ms)
- `RepositoryProvide` -- normalized provider record for dependency resolution
- `RepositoryRequirement` -- normalized requirement record for dependency resolution
- `RepositoryRequirementGroup` -- OR-group of requirements for a package

## Invariants
- Every `db::open()` and `db::init()` sets WAL mode, `synchronous=NORMAL`, `foreign_keys=ON`, `busy_timeout=5000`
- `schema::migrate(&conn)` runs on every open -- migrations are idempotent
- Use `db::transaction()` for multi-statement operations (auto-commit/rollback)
- Path helpers live in `db::paths` -- `objects_dir()`, `keyring_dir()`, `temp_dir()` all derive from `db_path`

## Gotchas
- Models are split across 37 files in `models/` -- each re-exported from `models/mod.rs`
- `format_size()` in `models/mod.rs` delegates to `crate::util::format_size()` (canonical location is `util.rs`)
- `CONVERSION_VERSION` constant lives in `models/converted.rs`
- `DEFAULT_CACHE_TTL_SECS` lives in `models/remote_collection.rs`
- Schema version must be bumped in migrations when adding tables

## Files
- `mod.rs` -- `init()`, `open()`, `open_fast()`, `transaction()`, connection setup
- `schema.rs` -- migration runner with function-dispatch migrations (migrate_v1 through migrate_v57)
- `migrations/` -- migration functions split across `v1_v20.rs`, `v21_v40.rs`, `v41_current.rs`
- `paths.rs` -- centralized path derivation (db_dir, objects_dir, keyring_dir, temp_dir)
- `models/` -- 37 model files, one per table group
