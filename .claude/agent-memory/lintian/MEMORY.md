# Lintian Memory

## Codebase Patterns

### Server handler conventions (conary-server/src/server/handlers/)
- `mod.rs` has shared helpers: `validate_name`, `serialize_json`, `json_response`, `human_bytes`, `find_repository_for_distro`
- `admin/` is a directory (mod.rs, tokens.rs, ci.rs, events.rs, repos.rs, federation.rs, audit.rs)
- Handler pattern: scope check -> path param validation -> get db_path from state -> spawn_blocking for DB ops -> match on Result<Result<T>> (join error + db error)
- Write operations publish events via `state.read().await.publish_event(event_type, json_data)`
- `admin_service.rs` provides shared business logic used by both HTTP handlers and MCP tools
- `Scope` enum replaces raw string scopes; `validate_scopes()` rejects unknown scope strings
- Rate limiters stored as axum Extension (not in RwLock ServerState)
- `extract_ip()` in rate_limit.rs is the single source for client IP extraction

### Testing patterns
- `test_app()` creates temp DB, seeds a bootstrap token, returns `(Router, PathBuf)`
- `rebuild_app()` helper for multi-step tests (oneshot consumes the router)

### Architecture notes
- Schema version is v54 (derivation_index table added in v54; uses function dispatch pattern migrate_v{N})
- `ServerState` behind `Arc<RwLock<>>`, config.db_path used to open fresh connections per request
- `audit_log` model uses free functions (insert/query/purge) not struct methods -- differs from Trove/Repository pattern
- `db::open_fast()` skips migrations for server hot paths
- Governor rate limiter DashMaps cleaned every 5 min via `run_limiter_cleanup()`

### Derivation module patterns
- 13 files, ~5600 lines. Inner loop of 114-package bootstrap pipeline.
- `compose_file_entries()` merges file entries from OutputManifests into EROFS input
- `canonical_string()` pattern used by DerivationId and SourceDerivationId for deterministic hashing
- Topological sort in stages.rs is intentionally different from recipe::graph (BTreeMap determinism + stage scoping)
- `expand_variables()` in recipe_hash.rs intentionally diverges from Recipe::substitute() for hash determinism

### conary-test patterns
- `runs` HashMap in AppState grows without bound (no eviction)
- 5 test fixture constructors duplicated, handlers+mcp duplicate 5 operations (no service layer)

## Open Issues

### Ship-blockers
- **Symlink loss in compose**: `compose_file_entries()` only processes `manifest.files`, ignores `manifest.symlinks` -- EROFS images broken for packages with shared lib symlinks
- **output_hash excludes mode/size**: `compute_output_hash()` skips file size and mode, so permission-only changes don't invalidate derivation cache

### Code quality
- `canonical_string()` copy-pasted between DerivationId and SourceDerivationId -- will drift
- `BuildProfile::canonical_string()` has no input validation for colon/newline injection
- glibc appears in both TOOLCHAIN_NAMED and FOUNDATION_PACKAGES arrays (works by accident)
- `SubstituterSection.trust` is a raw String where an enum (check/trust/rebuild) should be used
- SeedSource serialized via Debug format (`format!("{:?}", ...)`) -- fragile
- `DerivationExecutor.cas_dir` is misleadingly named (used as build work dir, not CAS root)

### Code reuse
- 10+ sites import sha2::Sha256 directly instead of using crate::hash utilities -- flag in reviews
- `hex_digest()` in export.rs reimplements `conary_core::hash::sha256()`
- CAS two-level walk pattern duplicated 4x (gc.rs, fsverity.rs, export.rs, system.rs) with inconsistent filtering
- `FsverityEnableArg` struct + `FS_IOC_ENABLE_VERITY` const duplicated in composefs.rs and fsverity.rs
- `detect_kernel_in_sysroot()` (image.rs) is identical to `detect_kernel_version()` (metadata.rs)
- `make_recipe()` test helper duplicated identically between stages.rs and pipeline.rs

### Efficiency (matters at 114-package scale)
- **HIGH**: `capture.rs` buffers entire DESTDIR files before CAS store (GCC = 100+ MB). Needs `CasStore::store_path()` with streaming hash+write.
- **HIGH**: `erofs_image_hash()` reads entire EROFS image into memory (200-400 MB). Needs streaming SHA-256.
- **MEDIUM**: No batch insert in derivation index.rs -- 114 individual INSERT transactions with fsync.
- **MEDIUM**: `canonical_string()` builds Vec<String> then joins; could use single String with push_str.

### System management (from 2026-03-10 audit, still open)
- container/mod.rs double-wait bug (wait_with_output after wait_timeout = ECHILD)
- model/mod.rs diamond includes falsely detected as cycles (visited set never shrinks)
- scriptlet/mod.rs seccomp warn-only, no actual enforcement

### Server
- Forgejo repo path `/repos/peter/Conary` hardcoded 12+ times -- should be constant or config
- `is_enabled` vs `enabled` field naming inconsistency between REST and MCP APIs for peers
- `content_url` not validated as URL in create_repo or update_repo
- `update_repo` handler does not validate `url` via url::Url::parse (create_repo does)
- auth_middleware spawns background `touch()` on every authenticated request -- no debouncing

## Lessons Learned
- No SQL injection via format strings -- all DB queries use parameterized ?1 bindings
- No `unwrap()`/`expect()` in non-test server handler code
- Path traversal properly guarded via safe_join() and sanitize_path()
- CAS atomic_store uses PID+counter temp names -- race-safe across threads/processes
- CPIO parser has proper MAX_FILE_SIZE and MAX_NAME_SIZE guards
- Bootstrap pipeline has 6+ expect() calls in production paths (base.rs) -- should be errors
- `expand_env_vars()` leaks host env into sandboxed bootstrap builds (design issue, has TODO)
- Resolver uses `as u32` for pool indices -- safe at current scale but fragile
