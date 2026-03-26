# Lintian Memory

## Codebase Patterns

### Server handlers (conary-server/src/server/handlers/)
- Handler pattern: scope check -> param validation -> get db_path -> spawn_blocking -> match Result<Result<T>>
- `admin_service.rs` shared by HTTP handlers and MCP tools
- Three routers: public (:8080), internal admin (:8081), external admin (:8082 auth+rate-limit+audit)
- Admin handlers: tokens, ci, repos, federation, audit, events, artifacts, packages, test_data

### conary-test crate (test infrastructure)
- Declarative TOML manifests + Podman containers via bollard + MCP server (23 tools)
- ContainerBackend trait: BollardBackend (real), MockBackend (test), NullBackend (QEMU-only)
- WAL (wal.rs): SQLite buffer for results when Remi unreachable; flush/purge/mark_retry
- Service layer (service.rs) shared by HTTP handlers and MCP tools (same pattern as conary-server)
- Container cleanup via label filter -- containers must be labeled at creation
- engine/container_setup.rs: shared init logic for runners and service code

### CLI / Daemon / Federation / Derivation / CCS / Trust
- Tests in same file as code, `tempfile` for FS, `:memory:` SQLite for DB
- CLI: 28 defs, options structs pattern; stub commands must bail!() not Ok(())
- Daemon: SO_PEERCRED auth, RFC 7807 errors (intentionally not thiserror)
- Federation: hierarchical routing, RendezvousRouter (FNV-1a), RequestCoalescer (DashMap singleflight)
- Derivation: 19 files, canonical_string pattern, graph.rs shared topo sort, Kitchen hermetic model
- CCS: archive_reader single-pass tar, CBOR BinaryManifest preferred
- TUF: Ed25519 only, verify_strict required, canonical JSON via json::canonical_json
- Model has own ModelError; Self-update TRUSTED_UPDATE_KEYS is empty pre-release
- resolver/provider/ is a directory (5 files), not a single provider.rs

## Anti-Patterns to Flag

- **Direct sha2 imports**: use crate::hash module
- **CAS path/walk duplication**: share CasStore::hash_to_path() and iterator
- **Sysroot path joining without safe_join()**: use filesystem::path::safe_join()
- **Debug format for serialization**: fragile for stored/hashed values
- **.ok() on rusqlite queries**: use .optional() to distinguish "no rows" from DB errors
- **reqwest::Client duplication**: 5+ modules build their own Client
- **.expect() on paths in non-test code**: return errors instead
- **Stub commands returning Ok(())**: must bail!() not Ok(())
- **verify() instead of verify_strict()**: Production Ed25519 must use verify_strict()
- **Unbounded in-memory caches**: NegativeCache, TOUCH_CACHE, RateLimiter lack max capacity

## Security (confirmed good)
- Parameterized SQL, safe_join() path traversal, atomic CAS store
- TUF: verify_strict, keyid dedup, type_field checks, size limits
- CCS: per-entry/cumulative/manifest size limits, hook input validation
- Remi: SSRF protection, layered auth, pull-through hash verification
- Container default: 5 namespace isolations + resource limits
- Self-update: SHA-256 inline verify during streaming download

## Doc Drift Patterns
- Schema version drifts across docs when migrations added (db.md, portage.md, sbuild.md)
- DB function is `db::transaction()` not `with_transaction()`; `format_bytes` in util.rs not models/
- composefs feature flag is `composefs-rs` but crate name is `composefs` v0.3
- conary-erofs crate was removed; old docs may still reference it
- DB migrations split into v1_v20.rs, v21_v40.rs, v41_current.rs
