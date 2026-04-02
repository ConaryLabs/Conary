# Lintian Memory

## Codebase Patterns

### Remi handlers (apps/remi/src/server/handlers/)
- Handler pattern: scope check -> param validation -> get db_path -> spawn_blocking -> match Result<Result<T>>
- `admin_service.rs` shared by HTTP handlers and MCP tools
- Three routers: public (:8080), internal admin (:8081), external admin (:8082 auth+rate-limit+audit)

### conary-test crate (test infrastructure)
- Declarative TOML manifests + Podman containers via bollard + MCP server (23 tools)
- ContainerBackend trait: BollardBackend (real), MockBackend (test), NullBackend (QEMU-only)
- Service layer (service.rs) shared by HTTP handlers and MCP tools (same pattern as Remi admin_service)

### CLI / Daemon / Federation / Derivation / CCS / Trust
- Tests in same file as code, `tempfile` for FS, `:memory:` SQLite for DB
- CLI: 28 defs, options structs pattern; stub commands must bail!() not Ok(())
- Daemon: SO_PEERCRED auth (cached at connect time), RFC 7807 errors (intentionally not thiserror)
- Daemon GET endpoints (events, transactions, packages, metrics) have NO auth -- auth_gate only gates POST/PUT/DELETE
- Daemon broadcast channel: single global channel, no per-user filtering on SSE streams
- Daemon router has no DefaultBodyLimit (Remi has 16MB); no SSE connection cap
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
- **.ok() on rusqlite queries**: use .optional() to distinguish "no rows" from DB errors
- **verify() instead of verify_strict()**: Production Ed25519 must use verify_strict()
- **CAS store() return hash not checked**: substituter.rs stores network data without verifying returned hash
- **retrieve_unchecked on untrusted path**: update.rs uses unchecked retrieve on delta-reconstructed data
- **LocalCacheFetcher no hash verify on read**: chunk_fetcher.rs trusts disk, never re-hashes
- **ChunkStore::get_chunk no hash verify**: chunking.rs reads chunks without integrity check
- **hash_to_path vs object_path validation gap**: hash_to_path accepts len>=2, object_path requires len>=4
- **GC has no transaction lock**: generation GC can race with in-flight transactions
- **Socket bind-then-chmod TOCTOU**: socket.rs creates socket then sets permissions; use umask or fchmod
- **Raw error in DaemonError::internal**: wraps rusqlite errors into client-visible RFC 7807 responses

## Security (confirmed good)
- Parameterized SQL, safe_join() path traversal, atomic CAS store
- TUF: verify_strict, keyid dedup, type_field checks, size limits
- CCS: per-entry/cumulative/manifest size limits, hook input validation
- Remi: layered auth, pull-through hash verification, SSRF protection via validate_url_scheme
- Container default: 5 namespace isolations + resource limits
- Self-update: SHA-256 inline verify during streaming download
- Body limits: 16 MiB global, 512 MiB uploads, 4 MiB CAS PUT; SSE 100-conn semaphore

## Security (needs attention)
- **Metadata sync unsigned**: GPG only on pkg downloads, not metadata; TUF defaults off
- **HTTPS downgrade via redirect**: reqwest follows HTTPS->HTTP redirects; validate_url_scheme only checks initial URL
- **Self-update: download_url from server used unvalidated; no download size cap**

## Concurrency / Transaction Patterns
- Only install/mod.rs and install/batch.rs use TransactionEngine::begin()
- CCS install, remove, adopt, rollback do NOT acquire the file lock
- build_generation_from_db has own .generation-build.lock for number allocation
- update_current_symlink uses fixed "current.tmp" name -- not unique per process

## Doc Drift Patterns
- Schema version drifts across docs when migrations added (db.md, portage.md, sbuild.md)
- DB function is `db::transaction()` not `with_transaction()`; `format_bytes` in util.rs not models/
- DB migrations split into v1_v20.rs, v21_v40.rs, v41_current.rs
