# Lintian Memory

## Codebase Patterns

### Server handler conventions (conary-server/src/server/handlers/)
- `mod.rs` has shared helpers: `validate_name`, `serialize_json`, `json_response`, `human_bytes`, `find_repository_for_distro`
- `admin/` is a directory (mod.rs, tokens.rs, ci.rs, events.rs, repos.rs, federation.rs, audit.rs)
- Handler pattern: scope check -> path param validation -> get db_path from state -> spawn_blocking for DB ops -> match on Result<Result<T>> (join error + db error)
- Write operations publish events via `state.read().await.publish_event(event_type, json_data)`
- `json_error(status_code, message, error_code)` from `auth.rs` for consistent error responses
- External admin router on :8082 uses auth_middleware; localhost admin on :8081 has no auth
- `admin_service.rs` provides shared business logic used by both HTTP handlers and MCP tools
- `forgejo.rs` provides shared Forgejo/CI client (get, post, get_text)
- `Scope` enum replaces raw string scopes; `validate_scopes()` rejects unknown scope strings
- Rate limiters stored as axum Extension (not in RwLock ServerState)
- `extract_ip()` in rate_limit.rs is the single source for client IP extraction
- Federation peer management uses `federation_peer` DB model in conary-core

### Testing patterns
- `test_app()` creates temp DB, seeds a bootstrap token, returns `(Router, PathBuf)`
- `rebuild_app()` helper for multi-step tests (oneshot consumes the router)

### Known issues (still open)
- Body field validation sometimes weaker than path param validation (e.g., create_repo validates length but not character set)
- Forgejo repo path `/repos/peter/Conary` hardcoded 12+ times -- should be constant or config
- `is_enabled` vs `enabled` field naming inconsistency between REST and MCP APIs for peers
- `forgejo_url` and `forgejo_token` redundantly stored in both AdminSection config and ServerState
- `validate_path_param` exists in both admin/mod.rs and mcp.rs (different error types, acceptable)

## Architecture Notes
- `conary_core::db::models::Repository` has: new(), insert(), update(), delete(), find_by_name(), list_all(), list_enabled()
- Repository.id is Option<i64> -- None before insert, Some after
- `ServerState` behind `Arc<RwLock<>>`, config.db_path used to open fresh connections per request
- External admin API scopes: admin, ci:read, ci:trigger, repos:read, repos:write, federation:read, federation:write
- Schema version is v48 (admin_audit_log added in v48)
- `audit_log` model uses free functions (insert/query/purge) not struct methods -- differs from Trove/Repository pattern
- MCP endpoint at /mcp on :8082 requires admin scope
- SSE broadcast channel bounded at 1024 -- adequate for admin API volume

## Performance Notes
- `db::open_fast()` skips migrations for server hot paths (open_fast vs open)
- Governor rate limiter DashMaps cleaned every 5 min via `run_limiter_cleanup()` (retain_recent + shrink_to_fit)
- auth_middleware spawns background `touch()` on every authenticated request (open + write) -- no debouncing
