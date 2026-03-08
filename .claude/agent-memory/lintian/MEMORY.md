# Lintian Memory

## Codebase Patterns

### Server handler conventions (conary-server/src/server/handlers/)
- `mod.rs` has shared helpers: `validate_name`, `serialize_json`, `json_response`, `human_bytes`, `find_repository_for_distro`
- `admin.rs` has its own `validate_path_param` (private) and `check_scope` -- validate_path_param should be in mod.rs
- Handler pattern: scope check -> path param validation -> get db_path from state -> spawn_blocking for DB ops -> match on Result<Result<T>> (join error + db error)
- Write operations publish events via `state.read().await.publish_event(event_type, json_data)`
- `json_error(status_code, message, error_code)` from `auth.rs` for consistent error responses
- External admin router on :8082 uses auth_middleware; localhost admin on :8081 has no auth

### Testing patterns
- `test_app()` creates temp DB, seeds a bootstrap token, returns `(Router, PathBuf)`
- `rebuild_app()` helper for multi-step tests (oneshot consumes the router)
- Router can be cloned before oneshot to avoid rebuilds (not currently done)

### Recurring issues found
- Body field validation sometimes weaker than path param validation (e.g., create_repo validates length but not character set)
- `expect()` used in non-test spawn_blocking code (delete_repo:827) -- should propagate errors
- MCP tools (all 16) have NO scope checking -- critical auth bypass (P1)
- Scope strings are raw literals with no validation whitelist -- typos create dead tokens
- SSE event filter uses exact match but events use dotted names (e.g., "repo.created") -- filter never matches
- Major duplication between admin.rs handlers and mcp.rs tools -- identical business logic, no service layer
- `validate_path_param` duplicated in admin.rs and mcp.rs; `validate_name` in mod.rs is a weaker variant
- `forgejo_get`/`forgejo_post` duplicated in admin.rs and mcp.rs -- needs shared client module
- `auth::hash_token` reimplements `conary_core::hash::sha256` with direct sha2/hex deps
- IP extraction logic appears 3 times: rate_limit.rs (fn), auth.rs (inline), audit.rs (inline)
- Federation peer SQL is inline in admin.rs + mcp.rs -- no model file exists (unlike admin_token/audit_log)
- Forgejo repo path `/repos/peter/Conary` hardcoded 12+ times -- should be constant or config
- `is_enabled` vs `enabled` field naming inconsistency between REST and MCP APIs for peers

## Architecture Notes
- `conary_core::db::models::Repository` has: new(), insert(), update(), delete(), find_by_name(), list_all(), list_enabled()
- Repository.id is Option<i64> -- None before insert, Some after
- `ServerState` behind `Arc<RwLock<>>`, config.db_path used to open fresh connections per request
- External admin API scopes: admin, ci:read, ci:trigger, repos:read, repos:write, federation:read, federation:write
- Schema version is v48 (admin_audit_log added in v48) -- CLAUDE.md still says v45
- `audit_log` model uses free functions (insert/query/purge) not struct methods -- differs from Trove/Repository pattern
- `forgejo_url` and `forgejo_token` redundantly stored in both AdminSection config and ServerState
- `.claude/rules/db.md` needs manual updates when schema version changes

## Performance Issues (confirmed 2026-03-07)
- `db::open()` calls `schema::migrate()` every time -- runs version check SQL + info logging on every request-path open
- Middleware stack (rate_limit -> auth -> audit) acquires state RwLock 3-5 times per request for immutable data (db_path, rate_limiters, forgejo config)
- auth_middleware spawns background `touch()` on every authenticated request (open + write) -- no debouncing
- Governor rate limiter DashMaps have no GC -- entries per unique IP never expire
- SSE broadcast channel bounded at 1024 (mod.rs:258) -- adequate for admin API volume
