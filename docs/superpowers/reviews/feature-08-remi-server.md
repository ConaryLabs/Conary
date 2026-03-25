## Feature 8: Remi Server -- Review Findings

### Summary

The Remi server is a well-structured, production-grade HTTP service spanning ~28,600 lines
across 57 Rust files. The architecture follows sound patterns: shared `admin_service` layer
for business logic, `thiserror` for error types, auth middleware on the external admin API,
governor-based rate limiting, and DB-backed LRU eviction. Security posture is strong -- SSRF
protection, input validation, path traversal guards, parameterised SQL, and Cloudflare IP
verification are all present. No P0 auth bypass or data-loss bugs were found. The findings
below are improvement opportunities, not shipping blockers.

---

### P0 -- Critical

No P0 findings.

---

### P1 -- Incorrect Behaviour / Missing Validation

**[P1] [security]: Direct `sha2` imports bypass the project's hashing abstraction**
- Files: `conary-server/src/server/conversion.rs:18`,
  `conary-server/src/server/handlers/derivations.rs:24`,
  `conary-server/src/server/handlers/profiles.rs:19`,
  `conary-server/src/server/handlers/admin/packages.rs:14`
- Issue: Four server files import `sha2::{Digest, Sha256}` directly instead of using
  `conary_core::hash::sha256()` or `conary_core::hash::Hasher`. This fragments hashing
  logic and makes it possible for a future algorithm migration (e.g., switching to
  BLAKE3) to miss these call sites.
- Impact: Inconsistency risk during hash algorithm changes; no immediate correctness bug.
- Fix: Replace `sha2::Sha256` usage with `conary_core::hash::sha256()` for one-shot
  hashing, or `conary_core::hash::Hasher` for streaming. The `conversion.rs`
  `calculate_checksum` function is already streaming via `std::io::copy` and could use
  `hash::hash_reader`.

**[P1] [correctness]: `store_chunk` does not verify content hash before writing**
- File: `conary-server/src/server/cache.rs:111`
- Issue: `ChunkCache::store_chunk(hash, data)` trusts the caller-supplied hash and writes
  to the CAS path derived from it without verifying `sha256(data) == hash`. If a caller
  passes mismatched hash/data, corrupted chunks are silently persisted. The
  `pull_through_fetch` codepath in `chunks.rs:457` does verify, but `store_chunk` itself
  does not, leaving other call sites (future or current) unprotected.
- Impact: Silent data corruption if any code path supplies wrong hash.
- Fix: Add an optional `verify: bool` parameter (or a separate `store_chunk_verified`
  method) that computes the hash and rejects mismatches. Alternatively, always verify --
  SHA-256 of a 64KB chunk is ~10us.

**[P1] [security]: `put_derivation` and `put_seed` use inline token check, missing rate limiting and audit logging**
- Files: `conary-server/src/server/handlers/derivations.rs:194`,
  `conary-server/src/server/handlers/seeds.rs:97`,
  `conary-server/src/server/handlers/profiles.rs:90`
- Issue: These PUT endpoints are on the **public** router (`:8080`), not the external
  admin router (`:8082`). They use the inline `require_admin_token()` helper for auth,
  which works, but they bypass the admin rate limiter, audit logging middleware, and ban
  list enforcement that protect `:8082` endpoints. An attacker can brute-force bearer
  tokens on these endpoints without triggering rate limits.
- Impact: Auth brute-force on the public port is rate-limited only by the general
  `rate_limit_middleware` (100 rps burst 200), not the auth-failure-specific 5/min limit
  that protects the admin API.
- Fix: Move these write endpoints to the external admin router, or apply the auth-failure
  rate limiter to the public router for endpoints that call `require_admin_token`.

**[P1] [correctness]: `TOUCH_CACHE` grows unbounded in auth middleware**
- File: `conary-server/src/server/auth.rs:31`
- Issue: `TOUCH_CACHE` is a `LazyLock<Mutex<HashMap<i64, Instant>>>` that inserts a new
  entry for every unique token ID that authenticates. There is no eviction or cleanup.
  Over a long server lifetime with many rotated tokens, this map grows without bound.
  While each entry is small (i64 + Instant = ~24 bytes), it is a principle violation --
  the server contracts guarantee bounded memory.
- Impact: Slow memory leak proportional to the number of distinct token IDs seen.
- Fix: Add a periodic cleanup that removes entries older than `TOUCH_DEBOUNCE_SECS`, or
  use an LRU cache with a max capacity.

---

### P2 -- Improvement Opportunities

**[P2] [duplication]: CAS path computation duplicated across cache, chunk_gc, conversion, and handlers**
- Files: `conary-server/src/server/cache.rs:87` (`chunk_path`),
  `conary-server/src/server/chunk_gc.rs:130` (`chunk_path`),
  `conary-server/src/server/conversion.rs:517` (inline `split_at(2)`),
  `conary-server/src/server/handlers/mod.rs:36` (`cas_object_path`)
- Issue: The `hash[0:2]/hash[2:]` path derivation is reimplemented in four places.
  While all implementations agree today, the duplication invites future divergence.
- Fix: Extract a shared `fn cas_path(root: &Path, hash: &str) -> PathBuf` in a shared
  module (e.g., `conary_core::filesystem::cas`) and call it everywhere.

**[P2] [code-quality]: `scan_chunk_hashes` duplicated between `chunks.rs` and `chunk_gc.rs`**
- Files: `conary-server/src/server/handlers/chunks.rs:855` (async version),
  `conary-server/src/server/chunk_gc.rs:89` (sync version using `walkdir`)
- Issue: Two implementations of the same logic (scan CAS directory, extract hashes, skip
  `.tmp` files). The async version uses a manual stack-based walk; the sync version uses
  `walkdir`. Both work but maintaining two is unnecessary.
- Fix: Keep the `walkdir` version (simpler, used from `spawn_blocking`) and call it from
  both sites via `spawn_blocking`.

**[P2] [code-quality]: `extract_hash_from_path` duplicated**
- Files: `conary-server/src/server/handlers/chunks.rs:891`,
  `conary-server/src/server/chunk_gc.rs:120`
- Issue: Identical function in two files.
- Fix: Move to the shared `handlers/mod.rs` or a `cas` utility module.

**[P2] [security]: Negative cache is a `HashMap` behind `RwLock` with no size bound**
- File: `conary-server/src/server/negative_cache.rs:27`
- Issue: The negative cache is an unbounded `HashMap<String, NegativeEntry>`. An attacker
  can generate unlimited unique "not found" keys (e.g., random package names) to grow
  this map without limit. The cleanup loop only removes expired entries, not excess ones.
- Impact: Memory exhaustion under sustained probing with unique keys.
- Fix: Add a max capacity (e.g., 100K entries). When full, skip insertion or evict the
  oldest entry.

**[P2] [security]: Public rate limiter `RateLimiter` also uses unbounded `HashMap`**
- File: `conary-server/src/server/security.rs:22`
- Issue: The hand-rolled token bucket `RateLimiter` stores per-IP state in an unbounded
  `HashMap`. The 5-minute cleanup helps, but a burst of unique IPs in a short window
  can still spike memory. The `TODO` comment on line 16 acknowledges this and suggests
  migrating to `governor`.
- Impact: Memory spike under IP-distributed attacks.
- Fix: Complete the migration to `governor::DefaultKeyedRateLimiter` (which has built-in
  GC via `retain_recent`), matching what the admin API already uses.

**[P2] [correctness]: `BanList` uses `HashMap<String, Instant>` -- string keys waste memory**
- File: `conary-server/src/server/security.rs:77`
- Issue: IP addresses are stored as `String` keys. Using `IpAddr` as the key type would
  save 20-30 bytes per entry (avoiding heap allocation for the string representation)
  and make type errors impossible.
- Fix: Change to `HashMap<IpAddr, Instant>`.

**[P2] [architecture]: `ServerState` has too many fields (26+)**
- File: `conary-server/src/server/mod.rs:182`
- Issue: `ServerState` is a god-struct with 26+ fields. Every new feature adds another
  `pub` field. The `RwLock<ServerState>` is acquired on most requests, meaning all
  readers block while any writer (e.g., updating search engine) holds the lock.
- Fix: Group related fields into sub-structs (e.g., `FederationState`, `SearchState`)
  with their own locking. This reduces contention and improves readability.

**[P2] [code-quality]: `conversion.rs` reads entire file into memory for hashing**
- File: `conary-server/src/server/conversion.rs:663`
- Issue: `build_from_recipe` reads the entire CCS package into a `Vec<u8>` to hash it
  (`tokio::fs::read(&cook_result.package_path)`). For large packages, this is wasteful
  when `calculate_checksum` already does streaming I/O.
- Fix: Use `calculate_checksum` for the hash, then stream the file for the copy.

**[P2] [code-quality]: `conversion.rs:607` total_size cast from i64 silently truncates negatives**
- File: `conary-server/src/server/conversion.rs:607`
- Issue: `existing.total_size.unwrap_or(repo_pkg.size) as u64` -- `repo_pkg.size` is
  `i64` (from SQLite). If the DB value is negative (data corruption), this wraps to a
  very large `u64`.
- Fix: Use `u64::try_from(...).unwrap_or(0)` for defensive conversion.

---

### P3 -- Style / Nitpicks

**[P3] [convention]: Missing file header comment on `chunk_gc.rs`**
- File: `conary-server/src/server/chunk_gc.rs:1`
- Issue: The file starts with `// conary-server/src/server/chunk_gc.rs` then a blank line
  before the `//!` doc comment. This matches convention. (False alarm -- checked and OK.)

**[P3] [naming]: `check_scope` returns `Option<Response>` with inverted semantics**
- File: `conary-server/src/server/handlers/admin/mod.rs:51`
- Issue: `check_scope` returns `None` on success and `Some(error)` on failure. This
  requires `if let Some(err) = check_scope(...)` at every call site, which reads
  backwards. A `Result<(), Response>` return type would be more idiomatic.
- Fix: Change to `fn require_scope(...) -> Result<(), Response>` and use `?` at call sites.

**[P3] [style]: `is_valid_hash` allows uppercase hex but CAS uses lowercase**
- File: `conary-server/src/server/handlers/chunks.rs:35`
- Issue: `is_valid_hash` accepts uppercase hex (`A-F`), but `normalize_hash` on line 44
  lowercases it. The acceptance of uppercase is intentional and handled, but a stricter
  validator that only accepts lowercase would eliminate the normalize step.
- Fix: Optional -- current approach is fine. Document the intentional normalization.

**[P3] [style]: Inconsistent error response patterns across public and admin handlers**
- Issue: Public endpoints return `(StatusCode, &str).into_response()` while admin
  endpoints use `json_error(status, msg, code)`. Both are valid but the inconsistency
  means clients need to handle both plain-text and JSON error bodies.
- Fix: Standardize on JSON error responses for all endpoints. Low priority.

**[P3] [code-quality]: `only_init` logic in `remi.rs` is fragile**
- File: `conary-server/src/bin/remi.rs:52`
- Issue: `only_init` is true when `--init` is passed without any other flags. But if a
  new CLI flag is added in the future, someone must remember to add it to this compound
  boolean. A better approach would be a dedicated `--init-only` flag.
- Fix: Add `#[arg(long)]` `init_only: bool` or restructure to subcommands.

---

### Cross-Domain Notes

**[Cross-Domain] [conary-core]: `hash::sha256()` should be the standard path for all server hashing**
- The four `use sha2::` imports in the server crate (see P1 above) are a conary-core
  convention issue. The fix belongs in the server crate but the convention is defined by
  conary-core's `hash` module.

**[Cross-Domain] [conary-core]: `ChunkAccess::get_lru_chunks` should exclude protected chunks**
- File: `conary-core/src/db/models/chunk_access.rs` (not in review scope)
- Issue: In `cache.rs:302`, `get_lru_chunks` is called to find eviction candidates. If
  this query does not filter `WHERE protected = 0`, protected chunks could be returned
  and attempted for deletion. The deletion would fail (file exists but is protected), but
  it wastes I/O. Confirming whether the core query filters correctly is outside this
  review scope.

---

### Strengths

1. **Auth architecture is sound** (`conary-server/src/server/auth.rs`,
   `conary-server/src/server/routes.rs:626-793`): The external admin API has layered
   defense: bearer token auth, typed `Scope` enum, per-operation scope checks in every
   handler, rate limiting (governor), audit logging, and MCP requires explicit admin
   scope. The internal admin API on `:8081` is enforced at the connection level via
   `require_localhost` middleware. No auth bypass paths found.

2. **SSRF protection is thorough** (`conary-server/src/server/conversion.rs:698-881`):
   The `fetch_url` function validates URL scheme, hostname (blocking localhost, metadata
   endpoints, internal domains), resolves DNS and checks all IPs against private ranges,
   then re-validates after redirects. This is textbook SSRF prevention with tests for
   every blocked category.

3. **Thundering herd prevention** (`conary-server/src/server/handlers/chunks.rs:338-499`):
   The pull-through cache uses `DashMap` + broadcast channels for request coalescing,
   with a proper `InflightGuard` drop guard to prevent waiters from hanging if the fetch
   future is cancelled. This is a non-trivial concurrency pattern implemented correctly.

4. **Clean admin service layer** (`conary-server/src/server/admin_service.rs`): Business
   logic is cleanly separated from HTTP framing. Both HTTP handlers and MCP tools call
   the same service functions, eliminating logic duplication. The `blocking()` helper
   properly flattens the `JoinError` / domain-error nesting.

5. **Comprehensive test coverage**: Every module has `#[cfg(test)] mod tests` with
   meaningful assertions. The `test_app()` helper creates a complete test harness with
   a seeded admin token. Auth rejection, scope enforcement, and CRUD operations are all
   tested at the handler level via `tower::ServiceExt::oneshot`.

6. **Input validation at every boundary**: Package names (`validate_name`), distro names
   (`validate_distro_and_name`), path parameters (`validate_path_param`), chunk hashes
   (`is_valid_hash` + `normalize_hash`), artifact paths (`sanitize_relative_path`), and
   URL parameters are all validated before use. No raw user input reaches SQL or the
   filesystem without sanitization.

---

### Recommendations

1. **Unify CAS path computation and chunk scanning**: Extract the `hash[0:2]/hash[2..]`
   pattern and the directory walk into shared functions. This eliminates four duplications
   and prevents future divergence. Estimated effort: 1-2 hours.

2. **Bound all in-memory caches**: Add max-capacity limits to `NegativeCache`,
   `TOUCH_CACHE`, and the public `RateLimiter` HashMap. Alternatively, complete the
   migration of the public rate limiter to `governor` (the TODO on line 16 of
   `security.rs` already calls for this). Estimated effort: 2-3 hours.

3. **Move write endpoints off the public router**: `PUT /v1/derivations`,
   `PUT /v1/seeds`, and `PUT /v1/profiles` should live behind the external admin router
   where they get rate limiting, audit logging, and auth-failure tracking for free.
   This is the highest-value security improvement. Estimated effort: 1 hour (route
   moves, no logic changes).

---

### Assessment

**Ready to merge?** Yes

**Reasoning:** No P0 findings. The P1 issues are real but non-urgent -- the `sha2`
imports are a convention violation not a correctness bug, the `store_chunk` hash
verification is defense-in-depth (callers currently verify), the unbounded `TOUCH_CACHE`
leaks slowly, and the public-port write endpoints are auth-protected even if they miss
rate limiting. The codebase demonstrates strong security awareness, clean separation of
concerns, and thorough testing. The P2 items are genuine improvements but none block
shipping.

---

### Work Breakdown

1. **[small] Replace direct `sha2` imports with `conary_core::hash`**: Touch 4 files,
   replace `Sha256::new()` / `Sha256::digest()` with `conary_core::hash::sha256()` or
   streaming equivalent.

2. **[small] Extract shared CAS path function**: Create
   `conary_core::filesystem::cas::object_path(root, hash)` and update 4 call sites.

3. **[small] Deduplicate `extract_hash_from_path` and `scan_chunk_hashes`**: Keep the
   `walkdir` version in `chunk_gc.rs`, export it, remove the async version in
   `chunks.rs`.

4. **[small] Move PUT endpoints to admin router**: Relocate derivation/seed/profile
   write routes from the public router to `create_external_admin_router`.

5. **[medium] Bound in-memory caches**: Add max capacity to `NegativeCache` and
   `TOUCH_CACHE`. Migrate public `RateLimiter` to governor.

6. **[medium] Refactor `ServerState` into sub-structs**: Group federation, search,
   analytics, and canonical fields into sub-structs with independent locking.

7. **[small] Add hash verification to `ChunkCache::store_chunk`**: Compute
   `sha256(data)` and reject mismatches before writing.

8. **[small] Change `check_scope` to return `Result`**: Rename to `require_scope`,
   return `Result<(), Response>`, update all call sites to use `?`.
