---
paths:
  - "conary-core/src/repository/**"
---

# Repository Module

Manages remote package repositories: HTTP fetching with retry, metadata sync,
format-specific parsers, mirror selection, GPG verification, and the Remi client
for CCS chunk operations.

## Key Types
- `RepositoryClient` -- HTTP client wrapper with `RetryPolicy` (exponential backoff + jitter)
- `RepositoryParser` -- trait for format-specific metadata parsers (Arch, Debian)
- `MirrorHealthTracker` / `MirrorHealth` -- per-mirror latency and error tracking
- `MirrorSelector` / `MirrorStrategy` -- intelligent mirror selection
- `RemiClient` / `AsyncRemiClient` -- client for Remi CCS server (async variant feature-gated)
- `PackageResolver` / `PackageSource` -- resolution with `ResolutionOptions`
- `SubstituterChain` -- binary substituter pipeline

## Constants
- `HTTP_TIMEOUT` -- 30 seconds default
- `STREAM_BUFFER_SIZE` -- 8 KB for streaming downloads
- `MAX_BYTES_RESPONSE_SIZE` -- 100 MB for in-memory downloads
- `DEFAULT_CACHE_TTL_SECS` -- from `db::models::remote_collection`

## Invariants
- `validate_url_scheme()` rejects non-HTTP(S) URLs (SSRF prevention)
- `RetryPolicy` defaults: 3 retries, 1s base delay, 30s max, 0.25 jitter
- All downloads stream in chunks -- never buffer entire response in memory
- `chunk_fetcher` module is feature-gated behind `--features server`

## Gotchas
- `parsers/` contains format-specific metadata parsers (arch.rs, debian.rs), not package parsers
- `remi.rs` has both sync `RemiClient` and async `AsyncRemiClient` (server feature only)
- `registry.rs` handles format detection and parser creation
- `metalink.rs` parses both XML metalink files and HTTP metalink headers

## Files
- `client.rs` -- `RepositoryClient`, `RetryPolicy`, `validate_url_scheme()`
- `sync.rs` -- `sync_repository()`, `needs_sync()`, timestamp helpers
- `parsers/` -- `arch.rs`, `debian.rs` format-specific metadata parsers
- `mirror_health.rs` -- per-mirror tracking
- `remi.rs` -- Remi CCS server client
- `download.rs` -- `download_package()`, checksum verification
- `resolution.rs` -- package resolution logic
- `gpg.rs` -- GPG signature verification
