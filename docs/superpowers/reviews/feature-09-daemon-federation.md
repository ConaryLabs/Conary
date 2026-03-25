## Feature 9: Daemon & Federation -- Review Findings

### Summary

The daemon and federation modules are well-structured with solid architectural foundations: the `SystemLock` flock-based exclusion is correct, the circuit breaker implementation is sound, and the request coalescer properly uses DashMap entry API to avoid races. However, there are several P0/P1 findings including a blocking database call on the async executor in the enhancement background worker, a TCP listener that is bound but never served (dead configuration), the `allowed_peers` config field that is declared but never enforced, and an mDNS spoofing vector. The daemon auth model is well-designed with SO_PEERCRED extraction and PID-reuse detection for supplementary groups.

---

### P0 -- Data Loss, Security Hole, Production Crash

**[P0] [security] Federation `allowed_peers` config is parsed but never enforced**
- File: `conary-server/src/federation/config.rs:270`
- Issue: `FederationConfig.allowed_peers: Option<Vec<String>>` is deserialized from TOML and stored, but no code in `Federation::new()`, `fetch_chunk_inner()`, `chunk_exists()`, or any other fetch path ever checks it. An operator who sets `allowed_peers` believing they are restricting which peers can be contacted will have a false sense of security -- all peers are contacted regardless.
- Impact: Security bypass. An operator's explicit peer restriction configuration is silently ignored.
- Fix: Either enforce `allowed_peers` in `Federation::new()` (filter peers on registration) and in `start_mdns_discovery()` (filter discovered peers), or remove the field and document that `tier_allowlists` is the correct mechanism. The `tier_allowlists` field IS enforced (via `select_peers_hierarchical_filtered`), but `fetch_chunk_inner()` currently calls `select_peers_hierarchical()` (unfiltered) instead.

**[P0] [security] mDNS discovery trusts any peer on the LAN without verification**
- File: `conary-server/src/federation/mdns.rs:306-337` and `conary-server/src/federation/mod.rs:319-337`
- Issue: Any device on the LAN can announce `_conary-cas._tcp.local.` and get automatically added to the peer registry via the mDNS callback. The discovered peer's `tier` is taken directly from its self-reported TXT record (`tier` property). A rogue device can claim to be a `RegionHub` or `CellHub` and receive chunk fetch traffic. Additionally, `tier_allowlists` are not checked during mDNS peer addition -- they are only checked during `select_peers_hierarchical_filtered()`, which is not the code path used by `fetch_chunk_inner()`.
- Impact: On a compromised or untrusted LAN, a malicious actor can inject a peer that serves tampered chunks. While chunks are SHA-256 verified after download, this still enables DoS (serving garbage that fails verification), traffic interception (observing which chunks are requested), and resource exhaustion.
- Fix: (1) Check `tier_allowlists` and/or `allowed_peers` before adding discovered peers in the mDNS callback. (2) Use `select_peers_hierarchical_filtered()` (not `select_peers_hierarchical()`) in `fetch_chunk_inner()`. (3) Consider requiring a shared secret or signature in the mDNS TXT record for hub announcements.

**[P0] [correctness] Enhancement background worker calls `open_db()` on the async executor**
- File: `conary-server/src/daemon/enhance.rs:301`
- Issue: `enhancement_background_worker()` calls `state.open_db()` directly (not inside `spawn_blocking`). `open_db` calls `db::open_fast()` which opens a SQLite connection and runs pragmas -- this is synchronous I/O that blocks the tokio executor thread. In contrast, `execute_enhance_job()` correctly uses `spawn_blocking` for its DB access.
- Impact: Under load, blocking the async executor can cause latency spikes or deadlocks across all concurrent async tasks on the same runtime. Since the worker loops forever checking every N seconds, this repeatedly blocks the executor.
- Fix: Wrap the pending-count check in `tokio::task::spawn_blocking`, matching the pattern used everywhere else in the daemon:
  ```rust
  let state_clone = state.clone();
  let pending_count = tokio::task::spawn_blocking(move || {
      match state_clone.open_db() {
          Ok(conn) => get_pending_by_priority(&conn, 1).map(|ids| ids.len()).unwrap_or(0),
          Err(_) => 0,
      }
  }).await.unwrap_or(0);
  ```

---

### P1 -- Incorrect Behavior, Silent Failure, Missing Validation

**[P1] [correctness] TCP listener is bound but never accepts connections**
- File: `conary-server/src/daemon/mod.rs:640-650` (bind) vs `mod.rs:675-727` (accept loop)
- Issue: `SocketManager::bind()` creates and binds a TCP listener when `config.enable_tcp` is true. However, `run_daemon()` only takes and uses the Unix listener (`socket_manager.take_unix_listener()`). The TCP listener is never taken from the SocketManager and no accept loop is created for it. Any user who configures `--tcp 127.0.0.1:7890` will see the port bound (netstat shows it listening) but connections will hang forever because nobody is accepting them.
- Impact: Silent feature failure. Users believe TCP access is working because the port is listening, but connections never complete.
- Fix: Either implement a second accept loop for TCP connections (using `tokio::select!` to multiplex both listeners), or remove the TCP bind from `SocketManager::bind()` until the feature is complete, returning an error if `enable_tcp` is set.

**[P1] [correctness] `fetch_chunk_inner()` does not apply `tier_allowlists` filtering**
- File: `conary-server/src/federation/mod.rs:406`
- Issue: `fetch_chunk_inner()` calls `self.router.select_peers_hierarchical(hash, &all_peers)` which does NOT filter by allowlists. The `select_peers_hierarchical_filtered()` method exists and works correctly, but is never called from any fetch path. The `tier_allowlists` config is only exercised in unit tests.
- Impact: Per-tier endpoint restrictions configured by the operator are not enforced during chunk fetching. This is distinct from the `allowed_peers` P0 above -- even the more granular `tier_allowlists` mechanism is unused in practice.
- Fix: Change the call in `fetch_chunk_inner()` to:
  ```rust
  let selection = self.router.select_peers_hierarchical_filtered(
      hash, &all_peers, &self.config.tier_allowlists
  );
  ```
  Similarly update `chunk_exists()`.

**[P1] [correctness] `lock.rs` `open_lock_file` uses `File::create` which truncates existing file**
- File: `conary-server/src/daemon/lock.rs:94`
- Issue: `File::create` opens with `O_WRONLY | O_CREAT | O_TRUNC`. If a stale lock file exists from a crashed daemon (lock released because process died, but file remains), this is fine. However, `is_held()` at line 110 uses `File::open` (read-only), then `try_lock_exclusive`, then `unlock`. The issue is that `is_held()` acquires and immediately releases the lock. If two processes call `is_held()` and `try_acquire()` concurrently, there is a TOCTOU window between `is_held` returning false and the caller acting on that information. This is not a bug in `SystemLock` itself (the lock acquisition is atomic), but it makes `is_held()` unreliable for external callers.
- Impact: Minor -- the daemon itself uses `try_acquire()` which is atomic. But CLI code using `is_daemon_running()` (which calls `is_held`) may get stale information.
- Fix: Document that `is_held()` is best-effort/advisory and that the only reliable check is attempting `try_acquire()`. Consider using `OpenOptions::new().create(true).write(true).open()` instead of `File::create` to avoid the truncate.

**[P1] [correctness] `get_daemon_pid` comment contradicts code**
- File: `conary-server/src/daemon/mod.rs:579-587`
- Issue: The doc comment says "so we skip that check" about `is_daemon_running()`, but the code calls `is_daemon_running()` via `.filter(|_| is_daemon_running())`. The comment claims the check is redundant and skipped, but the code does the opposite.
- Impact: Misleading documentation. The function also opens the lock file twice: once in `holder_pid` and once in `is_daemon_running`, which is wasteful.
- Fix: Either (a) remove the `.filter(|_| is_daemon_running())` to match the comment (trusting that a PID file + valid PID implies the daemon is running), or (b) rewrite the comment to explain why the double-check is intentional.

**[P1] [correctness] `dry_run_handler` requires write-level auth for a read-only operation**
- File: `conary-server/src/daemon/routes.rs:980-984`
- Issue: The dry-run endpoint calls `require_auth` with `action_for_job_kind(determine_job_kind(...))`, which maps Install/Remove/Update operations to their corresponding write actions. But a dry-run should be read-only -- it only plans what would happen without executing. This means a non-root user without PolicyKit authorization cannot even preview an operation.
- Impact: Usability issue for unprivileged users who want to see what an install/remove would do before elevating privileges.
- Fix: Use `Action::Query` for dry-run operations, or create a dedicated `Action::DryRun` that is marked read-only.

**[P1] [code-quality] `DaemonJob::from_row` silently nullifies invalid spec JSON**
- File: `conary-server/src/daemon/jobs.rs:306-307`
- Issue: `serde_json::from_str(&spec_json).unwrap_or(serde_json::Value::Null)` silently converts invalid JSON to `null`. The `kind` and `status` fields correctly return errors for invalid values (lines 287-303), but a corrupt `spec_json` column is silently swallowed. A job with a null spec will behave unpredictably when executed.
- Impact: Data corruption in the jobs table is silently hidden rather than surfaced as an error.
- Fix: Return a `FromSqlConversionFailure` error like the kind/status parsing does:
  ```rust
  let spec: serde_json::Value = serde_json::from_str(&spec_json).map_err(|e| {
      rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, e.to_string().into())
  })?;
  ```

---

### P2 -- Improvement Opportunity, Minor Inconsistency

**[P2] [correctness] Circuit breaker `is_open()` takes `&self` and cannot transition to HalfOpen**
- File: `conary-server/src/federation/circuit.rs:71`
- Issue: The `is_open(&self)` method correctly returns `false` when the cooldown elapses (allowing a probe request), but it cannot actually set the state to `HalfOpen` because it takes `&self`. The `CircuitBreakerRegistry` uses DashMap with `RefMut`, so `record_success/record_failure` do get `&mut self`. But the `is_open` check and subsequent `record_success` are not atomic -- the circuit could be checked by one task, then another task's `record_failure` could re-open it before the first task's probe completes. This is documented in the code comments, so it is an acknowledged design limitation rather than a bug.
- Impact: Under high concurrency, multiple probe requests may be sent during half-open state instead of exactly one. This is benign (extra probes are harmless).
- Fix: Consider using a `Mutex<CircuitBreaker>` or adding an atomic `probe_in_progress` flag if strict single-probe semantics are desired.

**[P2] [code-quality] Duplicate `PeerCredentials::is_root()` test**
- File: `conary-server/src/daemon/socket.rs:282-299` and `conary-server/src/daemon/auth.rs:484-499`
- Issue: `test_peer_credentials_is_root()` is defined identically in both `socket.rs` and `auth.rs`.
- Impact: Redundant test code.
- Fix: Remove the duplicate in `socket.rs` (the canonical location for peer credential tests is `auth.rs`).

**[P2] [architecture] `AuditLogger` stores entries in-memory only, not in SQLite**
- File: `conary-server/src/daemon/auth.rs:400-478`
- Issue: The `AuditLogger` stores audit entries in a `Vec<AuditEntry>` that is lost on daemon restart. This violates the "Database-First" principle from CLAUDE.md: "All state lives in SQLite." The Remi admin server has a proper `admin_audit_log` table, but the daemon audit logger does not persist.
- Impact: Audit trail is lost on daemon restart or crash, making post-incident investigation impossible.
- Fix: Either write audit entries to a `daemon_audit_log` table, or redirect to the system journal (which is already happening via `log::info!`/`log::warn!`) and remove the in-memory Vec.

**[P2] [code-quality] `enhancement_background_worker` never terminates**
- File: `conary-server/src/daemon/enhance.rs:296-338`
- Issue: The function loops forever with no mechanism to signal shutdown. The `cancel_token` inside the function is a local `AtomicBool` that is never set to `true` by external code. When the daemon shuts down, this task will be dropped by the tokio runtime (which is fine), but there is no graceful shutdown path.
- Impact: On daemon shutdown, any in-progress enhancement batch may be interrupted mid-operation. Database consistency is likely maintained by SQLite transactions, but this deserves explicit documentation.
- Fix: Accept a shared `CancellationToken` (or `Arc<AtomicBool>`) parameter that the daemon sets during shutdown, and check it in the loop.

**[P2] [code-quality] Unused `Arc` import in `peer.rs`**
- File: `conary-server/src/federation/peer.rs:9`
- Issue: `use std::sync::Arc;` is imported but only used for `PeerRegistry.peers: HashMap<PeerId, Arc<Peer>>`. The `Arc` wrapping seems unnecessary since `PeerRegistry` itself is behind an `RwLock<PeerRegistry>` in `Federation` -- callers always hold the lock when accessing peers, so `Arc<Peer>` provides no benefit over `Peer`. The `all()` method returns `Vec<Arc<Peer>>` but callers (like `fetch_chunk_inner`) immediately call `all_cloned()` which does `(**p).clone()`, paying for both the Arc and the clone.
- Impact: Unnecessary indirection and allocation.
- Fix: Store `Peer` directly in the HashMap and remove the `all()` method, keeping only `all_cloned()`.

**[P2] [code-quality] Mixed logging crates: `log::` vs `tracing::`**
- File: Multiple daemon files
- Issue: `mod.rs`, `lock.rs`, and `socket.rs` use `log::info!`, `log::warn!`, `log::error!`. `enhance.rs`, `routes.rs`, and auth.rs use `tracing::info!`, `tracing::warn!`. The `conaryd.rs` binary initializes `tracing_subscriber` but the `run_daemon` function uses `log::info!`. While `tracing` provides a compatibility layer for the `log` crate, mixing them means structured fields (e.g., `tracing::warn!(uid = creds.uid, ...)`) are only available in some log lines.
- Impact: Inconsistent log output. Structured fields from `tracing` are lost in `log::` calls.
- Fix: Migrate all `log::` calls in the daemon module to `tracing::`.

**[P2] [ai-slop] Over-commented builder methods**
- File: `conary-server/src/daemon/mod.rs:116-139`
- Issue: `DaemonConfig` builder methods (`with_db_path`, `with_socket_path`, `with_tcp`, `with_idle_timeout`) have doc comments that add no information beyond the method signature. For example, `/// Set the socket path` on `fn with_socket_path`.
- Impact: Comment noise that makes the file harder to scan.
- Fix: Remove trivial comments or convert to `#[doc(hidden)]` for internal-only builder methods.

**[P2] [correctness] `OperationQueue::cancel` has a potential lock-ordering issue**
- File: `conary-server/src/daemon/jobs.rs:434-455`
- Issue: The `cancel` method first takes `current_job.read()`, then `cancel_tokens.read()`, then `queue.lock()`, then `cancel_tokens.write()`. If another code path acquires these locks in a different order, a deadlock could occur. Currently this appears safe because no other method holds `queue` and then acquires `cancel_tokens`, but the ordering is not documented or enforced.
- Impact: Latent deadlock risk as the codebase evolves.
- Fix: Document the lock acquisition order, or consolidate the queue and cancel tokens under a single mutex.

---

### P3 -- Style, Naming, Minor Improvement

**[P3] [style] `DaemonError` is not a `thiserror` type**
- File: `conary-server/src/daemon/mod.rs:213-301`
- Issue: `DaemonError` is a plain struct with manual constructors, not a `thiserror::Error` enum. The codebase convention (CLAUDE.md) says "thiserror for errors." However, `DaemonError` is an RFC 7807 wire format, not a Rust error type, so this is arguably the correct design for its purpose.
- Impact: None if intentional. Worth a brief comment explaining the design choice.
- Fix: Add a comment: `// Note: This is an RFC 7807 wire format, not a Rust error type, hence no thiserror.`

**[P3] [style] `manifest.rs` `ManifestError` uses `thiserror` correctly -- good**
- File: `conary-server/src/federation/manifest.rs:39-59`
- Issue: None. This is a positive note: `ManifestError` correctly uses `thiserror::Error`.

**[P3] [style] `conaryd.rs` default DB path differs from `DaemonConfig` default**
- File: `conary-server/src/bin/conaryd.rs:18` vs `conary-server/src/daemon/mod.rs:104`
- Issue: The `conaryd` binary defaults to `/conary/db/conary.db` while `DaemonConfig::default()` uses `/var/lib/conary/conary.db`. The binary's default always wins at runtime, but this creates confusion when reading `DaemonConfig::default()` in isolation.
- Impact: Minor confusion for developers.
- Fix: Make the defaults consistent, or add a comment in `DaemonConfig::default()` noting that the binary overrides this.

**[P3] [style] `JobId` is `String` but could be a newtype**
- File: `conary-server/src/daemon/mod.rs:143`
- Issue: `pub type JobId = String;` provides no type safety. A trove name or any other String can be passed where a JobId is expected.
- Impact: Minimal given the current scope, but a newtype would catch bugs at compile time as the API grows.
- Fix: Consider `pub struct JobId(pub String)` with `Deref<Target=str>` and `Display`.

---

### Cross-Domain Notes

**[Cross-Domain: conary-core] `ChunkFetcher` trait's `fetch_many` default is sequential**
- File: `conary-server/src/federation/mod.rs:700-769`
- Issue: `FederatedChunkFetcher::fetch_many` implements its own parallel fetch logic using `join_all`. This duplicates the parallelism that could be provided by a default implementation in the `ChunkFetcher` trait. Not a bug, but the federation module is working around a trait limitation.
- Impact: If another `ChunkFetcher` implementation needs parallel fetch_many, it will re-implement the same pattern.

**[Cross-Domain: conary-core] `hash::verify_sha256` used correctly in federation**
- File: `conary-server/src/federation/mod.rs:512`
- Issue: None. Positive note: chunk integrity is verified after download using `conary_core::hash::verify_sha256`, which is the correct codebase pattern.

---

### Strengths

1. **SystemLock design** (`lock.rs`): The flock-based exclusive lock with separate PID file is a correct Unix pattern. The `try_acquire`/`is_held` split allows both blocking and non-blocking usage. Drop cleanup is proper.

2. **Circuit breaker with jitter** (`circuit.rs`): The implementation correctly prevents thundering herd on recovery with randomized cooldowns. The DashMap-based registry provides lock-free per-peer access. The documented limitation about `is_open(&self)` not transitioning state is honest engineering.

3. **Request coalescer** (`coalesce.rs`): Uses DashMap's entry API for atomic check-then-insert, correctly avoiding the double-fetch race. The retry logic when a leader drops without sending is a thoughtful edge case handler.

4. **Auth model** (`auth.rs`): SO_PEERCRED extraction with PID-reuse detection (cross-validating `/proc/{pid}/status` UID against the socket credential UID) is a security-conscious design that most daemon implementations skip.

5. **Defense-in-depth auth gate** (`routes.rs:199-212`): The `auth_gate_middleware` rejects all mutating requests without credentials at the router level, ensuring that a handler missing its own `require_auth()` call is still protected.

6. **Hierarchical peer routing** (`router.rs`): Clean separation of rendezvous hashing per tier with proper BinaryHeap-based top-K selection. The `HierarchicalSelection` type with `iter_with_tier()` is ergonomic.

7. **Signed manifests** (`manifest.rs`): Proper canonical serialization (BTreeMap for determinism), Ed25519 signature verification, configurable trust policies, and tamper detection tests.

8. **Comprehensive test coverage**: Every file has in-module tests covering the core logic. The circuit breaker, router, manifest signing, and job queue tests are particularly thorough.

---

### Recommendations

1. **Fix the tier_allowlists enforcement gap (P0 + P1)**: Change `fetch_chunk_inner()` and `chunk_exists()` to use `select_peers_hierarchical_filtered()` with `self.config.tier_allowlists`. Either wire up or remove `allowed_peers`. Filter mDNS-discovered peers against allowlists before adding to registry.

2. **Fix the blocking DB call in the background worker (P0)**: Wrap `state.open_db()` in `tokio::task::spawn_blocking` in `enhancement_background_worker()` at line 301.

3. **Resolve the TCP listener gap (P1)**: Either implement TCP accept or error when `enable_tcp` is set. A bound but unserved port is worse than a failed bind.

---

### Assessment

**Ready to merge?** No -- with fixes for the three P0 items.

**Reasoning:** The P0 items -- unenforced `allowed_peers` config, unfiltered mDNS peer injection, and blocking DB on async executor -- represent a security gap (operators believe they have peer restrictions that don't exist), a potential DoS vector (LAN spoofing), and a correctness issue (async executor starvation). The P1 items (dead TCP listener, unfiltered tier_allowlists, misleading doc comment) should be addressed before shipping to users but are not blockers for internal development.

---

### Work Breakdown

1. **[P0] Enforce peer allowlists in fetch paths**
   - Files: `conary-server/src/federation/mod.rs`
   - Change `fetch_chunk_inner()` line 406 to call `select_peers_hierarchical_filtered()` with `self.config.tier_allowlists`
   - Change `chunk_exists()` to use filtered selection
   - Either enforce `allowed_peers` or remove the field from `FederationConfig`
   - Add mDNS allowlist filtering in the discovery callback

2. **[P0] Fix blocking DB in enhancement background worker**
   - File: `conary-server/src/daemon/enhance.rs:301`
   - Wrap `state.open_db()` in `tokio::task::spawn_blocking`

3. **[P1] Resolve TCP listener dead code**
   - File: `conary-server/src/daemon/mod.rs`
   - Option A: Implement TCP accept loop alongside Unix accept
   - Option B: Return error when `enable_tcp` is set (mark as unimplemented)

4. **[P1] Fix silent spec JSON corruption in DaemonJob::from_row**
   - File: `conary-server/src/daemon/jobs.rs:306-307`
   - Return error instead of defaulting to `Value::Null`

5. **[P2] Migrate log:: to tracing:: in daemon module**
   - Files: `mod.rs`, `lock.rs`, `socket.rs`
   - Replace `log::info!`/`log::warn!`/`log::error!` with `tracing::` equivalents

6. **[P2] Add graceful shutdown to enhancement background worker**
   - File: `conary-server/src/daemon/enhance.rs`
   - Accept a `CancellationToken` parameter
   - Check it in the main loop alongside the sleep
