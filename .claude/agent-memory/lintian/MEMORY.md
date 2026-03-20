# Lintian Memory

## Codebase Patterns

### Server handlers (conary-server/src/server/handlers/)
- `mod.rs` has shared helpers: `validate_name`, `serialize_json`, `json_response`, `human_bytes`
- Handler pattern: scope check -> param validation -> get db_path -> spawn_blocking for DB -> match Result<Result<T>>
- Write operations publish events via `state.read().await.publish_event(event_type, json_data)`
- `admin_service.rs` shared business logic for both HTTP handlers and MCP tools
- Rate limiters stored as axum Extension (not in RwLock ServerState)

### Testing patterns
- `test_app()` creates temp DB, seeds bootstrap token, returns `(Router, PathBuf)`
- `rebuild_app()` for multi-step tests (oneshot consumes the router)
- Tests in same file as code, `tempfile` for FS, `:memory:` SQLite for DB

### Architecture
- Schema v54, function-dispatch migrations (`migrate_v{N}`)
- `db::open_fast()` skips migrations for server hot paths
- `audit_log` uses free functions (insert/query/purge), not struct methods — differs from Trove/Repository pattern

### Derivation module
- Inner loop of 114-package bootstrap pipeline — efficiency matters here
- `canonical_string` pattern: deterministic hash inputs via sorted BTreeMap + newline-delimited format
- `expand_variables()` in recipe_hash.rs intentionally diverges from Recipe::substitute() for hash determinism
- Topological sort in stages.rs intentionally different from recipe::graph (BTreeMap determinism + stage scoping)

## Anti-Patterns to Flag

- **Direct sha2 imports**: crate::hash::sha256() and hash::Hasher should be the standard path. Flag `use sha2::` in reviews.
- **Whole-file reads for hashing**: use streaming I/O (hash::hash_reader or BufReader + 8KB chunks) for files that could be large
- **CAS two-level walk duplication**: gc.rs, fsverity.rs, export.rs, system.rs all walk CAS dirs with inconsistent filtering — should share iterator
- **Debug format for serialization**: `format!("{:?}", enum_value)` is fragile for anything stored or hashed. Use Display or serde.

## Security (confirmed good)
- All DB queries use parameterized ?1 bindings (no SQL injection)
- Path traversal guarded via safe_join() and sanitize_path()
- CAS atomic_store uses PID+counter temp names (race-safe)
- CPIO parser has MAX_FILE_SIZE and MAX_NAME_SIZE guards
