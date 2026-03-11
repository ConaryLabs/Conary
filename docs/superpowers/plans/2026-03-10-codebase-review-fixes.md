# Codebase Review Fix Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix 200 findings from full codebase review, prioritized by severity.

**Architecture:** Fixes are organized into 5 phases by severity tier. Each phase contains independent tasks that can be parallelized via subagents. Tasks within a phase have no dependencies on each other. Later phases may depend on earlier ones (noted where applicable).

**Tech Stack:** Rust 1.94, rusqlite, tokio, axum, resolvo, thiserror

---

## Phase 1: P0 Critical Fixes (29 findings)

These are security holes, data loss risks, and production crashes. Ship ASAP.

---

### Task 1: Fix upsert `last_insert_rowid()` bug (DB)

**Files:**
- Modify: `conary-core/src/db/models/config.rs:155-169`
- Modify: `conary-core/src/db/models/label.rs:402-418`
- Modify: `conary-core/src/db/models/trigger.rs:310-322`

**Problem:** When `INSERT ... ON CONFLICT DO UPDATE` takes the conflict branch, `last_insert_rowid()` returns the ID from the *last successful INSERT*, not the updated row. `self.id` gets set to a stale/wrong value.

- [ ] **Step 1: Fix `ConfigFile::upsert()` in config.rs**

Replace the `INSERT OR REPLACE` / `ON CONFLICT DO UPDATE` + `last_insert_rowid()` pattern with a follow-up SELECT to get the actual row ID:

```rust
// After the INSERT ... ON CONFLICT DO UPDATE statement:
let id: i64 = conn.query_row(
    "SELECT id FROM config_files WHERE path = ?1",
    [&self.path],
    |row| row.get(0),
)?;
self.id = Some(id);
Ok(id)
```

- [ ] **Step 2: Apply same fix to `LabelPathEntry::upsert()` in label.rs:416**

Use `SELECT id FROM label_path_entries WHERE ...` with the unique key columns.

- [ ] **Step 3: Apply same fix to `ChangesetTrigger::insert()` in trigger.rs:319**

Use `SELECT id FROM changeset_triggers WHERE ...` with the unique key columns.

- [ ] **Step 4: Run tests**

Run: `cargo test -p conary-core -- db::models`
Expected: All model tests pass.

- [ ] **Step 5: Commit**

```
fix(db): use SELECT after upsert instead of last_insert_rowid
```

---

### Task 2: Fix migration runner transaction passthrough (DB)

**Files:**
- Modify: `conary-core/src/db/schema.rs:67-68`

**Problem:** Migration creates `tx = conn.unchecked_transaction()` but passes `conn` to `apply_migration` and `set_schema_version` instead of `&tx`. Works by accident but is brittle.

- [ ] **Step 1: Pass `&tx` instead of `conn`**

```rust
// Line 67-68, change:
let tx = conn.unchecked_transaction()?;
match apply_migration(conn, version).and_then(|()| set_schema_version(conn, version)) {
// To:
let tx = conn.unchecked_transaction()?;
match apply_migration(&tx, version).and_then(|()| set_schema_version(&tx, version)) {
```

No signature changes needed -- `Transaction` derefs to `Connection`.

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-core -- db::schema`
Expected: PASS

- [ ] **Step 3: Commit**

```
fix(db): pass transaction ref to migration functions instead of bare connection
```

---

### Task 3: Fix hash slice panic in install path (CLI)

**Files:**
- Modify: `src/commands/install/mod.rs:1030-1034`
- Reference: `src/commands/install/batch.rs:379` (has the correct guard)

**Problem:** `&hash[0..2]` panics if hash is shorter than 2 chars. The batch installer has a guard; the single-install path does not.

- [ ] **Step 1: Add length guard before hash slicing**

Before line 1030, add:

```rust
for (path, hash, size, mode) in &file_hashes {
    if hash.len() < 3 {
        warn!("Skipping file with short hash: {} (hash={})", path, hash);
        continue;
    }
    // existing code continues...
```

- [ ] **Step 2: Run tests**

Run: `cargo test -- commands::install`
Expected: PASS

- [ ] **Step 3: Commit**

```
fix(install): guard against short hash before slicing in single-install path
```

---

### Task 4: Fix `remove` crash safety -- DB commit before file deletion (CLI)

**Files:**
- Modify: `src/commands/remove.rs:210-280`

**Problem:** Files are deleted BEFORE the DB commit. A crash between file removal and DB commit leaves files gone but the package still recorded as installed.

- [ ] **Step 1: Reverse the order -- commit DB changes first, then remove files**

Move the DB transaction block (lines 272-278+) to BEFORE the file removal loop (line 217). Update the changeset to record intent, commit the DB to mark the package as removed, THEN remove files. Failed file removals become a cleanup issue, not a data consistency issue.

- [ ] **Step 2: Update the TODO comment at line 213**

Replace the TODO with a comment explaining the new order and why DB-first is safer.

- [ ] **Step 3: Run tests**

Run: `cargo test -- commands::remove`
Expected: PASS

- [ ] **Step 4: Commit**

```
fix(remove): commit DB changes before file deletion for crash safety
```

---

### Task 5: Fix `cache_max_bytes` integer overflow (CLI)

**Files:**
- Modify: `src/main.rs:420`

**Problem:** `max_cache_gb * 1024 * 1024 * 1024` can wrap on large values.

- [ ] **Step 1: Use checked multiplication**

```rust
let cache_max_bytes = max_cache_gb
    .checked_mul(1024 * 1024 * 1024)
    .unwrap_or(u64::MAX);
```

- [ ] **Step 2: Run tests**

Run: `cargo test -- main`
Expected: PASS

- [ ] **Step 3: Commit**

```
fix(cli): use checked multiplication for cache_max_bytes to prevent overflow
```

---

### Task 6: Fix `unwrap()` on `canonical.id` in install path (CLI)

**Files:**
- Modify: `src/commands/install/mod.rs:211`

**Problem:** `canonical.id.unwrap()` panics on corrupt DB data.

- [ ] **Step 1: Replace unwrap with error propagation**

```rust
// Change:
canonical.id.unwrap()
// To:
canonical.id.ok_or_else(|| anyhow::anyhow!("Canonical package has no ID"))?
```

- [ ] **Step 2: Commit**

```
fix(install): replace unwrap on canonical.id with proper error
```

---

### Task 7: Fix TUF root rotation ordering (Trust)

**Files:**
- Modify: `conary-core/src/trust/client.rs:63-175` (the `update` method)

**Problem:** Per TUF spec 5.3, root rotation must happen BEFORE timestamp/snapshot verification. Currently it happens after (step 3), meaning compromised old keys can verify metadata for one update cycle.

- [ ] **Step 1: Move root rotation check before timestamp fetch**

Restructure the `update()` method:
1. First, probe for new root versions via `{version+1}.root.json`
2. Verify the root chain
3. THEN fetch and verify timestamp using the (possibly new) root keys
4. Continue with snapshot/targets as before

Remove the TODO comment at line 111-114.

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-core -- trust`
Expected: PASS

- [ ] **Step 3: Commit**

```
security(trust): fix TUF root rotation to happen before metadata verification per spec 5.3
```

---

### Task 8: Wrap TUF persist in transaction (Trust)

**Files:**
- Modify: `conary-core/src/trust/client.rs:164-174`

**Problem:** Four `persist_metadata` calls are not transactional. Crash between them leaves inconsistent TUF state.

- [ ] **Step 1: Wrap persist calls in transaction**

```rust
// Replace lines 164-174 with:
conary_core::db::with_transaction(conn, |tx| {
    self.persist_metadata(tx, "timestamp", &signed_timestamp)?;
    if snapshot_changed {
        self.persist_metadata(tx, "snapshot", &signed_snapshot)?;
    }
    if targets_changed {
        self.persist_metadata(tx, "targets", &signed_targets)?;
        self.persist_targets(tx, &signed_targets.signed)?;
    }
    Ok(())
})?;
```

Remove the TODO comment at line 165.

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-core -- trust`
Expected: PASS

- [ ] **Step 3: Commit**

```
fix(trust): wrap TUF persist operations in database transaction
```

---

### Task 9: Fix `canonical_json()` expect calls (Trust)

**Files:**
- Modify: `conary-core/src/trust/keys.rs:31-35`
- Modify: `conary-core/src/trust/mod.rs` (add error variant if needed)

**Problem:** `expect()` in production path for JSON serialization.

- [ ] **Step 1: Change return type to `Result`**

```rust
pub fn canonical_json<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, TrustError> {
    let json_value = serde_json::to_value(value)
        .map_err(|e| TrustError::SerializationError(e.to_string()))?;
    let sorted = sort_json_value(&json_value);
    serde_json::to_vec(&sorted)
        .map_err(|e| TrustError::SerializationError(e.to_string()))
}
```

- [ ] **Step 2: Add `SerializationError` variant to `TrustError` if it doesn't exist**

- [ ] **Step 3: Update all callers of `canonical_json` to propagate the error with `?`**

Check `keys.rs` (`compute_key_id`, `sign_tuf_metadata`) and any other callers.

- [ ] **Step 4: Run tests**

Run: `cargo test -p conary-core -- trust`
Expected: PASS

- [ ] **Step 5: Commit**

```
fix(trust): make canonical_json return Result instead of panicking
```

---

### Task 10: Fix `unreachable!()` in resolver provider (Resolver)

**Files:**
- Modify: `conary-core/src/resolver/provider.rs:362-365`

**Problem:** `resolve_condition()` panics with `unreachable!()`. If resolvo ever calls it, the process dies.

- [ ] **Step 1: Return a safe default instead of panicking**

```rust
fn resolve_condition(&self, _condition: ConditionId) -> Condition {
    // ConaryProvider does not use conditions; return a permissive default
    Condition::default()
}
```

If `Condition::default()` is not available, check the resolvo API for the appropriate neutral value.

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-core -- resolver`
Expected: PASS

- [ ] **Step 3: Commit**

```
fix(resolver): replace unreachable!() with safe default in resolve_condition
```

---

### Task 11: Fix `RemiClient::new()` missing URL scheme validation (Repository)

**Files:**
- Modify: `conary-core/src/repository/remi.rs:122-131`

**Problem:** Constructor accepts any URL without calling `validate_url_scheme()`, bypassing SSRF prevention.

- [ ] **Step 1: Add URL scheme validation**

At the start of `RemiClient::new()`:

```rust
crate::repository::client::validate_url_scheme(&base_url)?;
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-core -- repository::remi`
Expected: PASS

- [ ] **Step 3: Commit**

```
security(repository): validate URL scheme in RemiClient constructor
```

---

### Task 12: Fix unbounded CCS archive extraction (CCS)

**Files:**
- Modify: `conary-core/src/ccs/package.rs:292-329`

**Problem:** `extract_all_content()` reads every archive object into memory with no size limit. Malicious package = OOM.

- [ ] **Step 1: Add per-entry and cumulative size limits**

```rust
const MAX_ENTRY_SIZE: u64 = 512 * 1024 * 1024; // 512 MB per entry
const MAX_TOTAL_SIZE: u64 = 4 * 1024 * 1024 * 1024; // 4 GB total

pub fn extract_all_content(&self) -> Result<HashMap<String, Vec<u8>>> {
    // ... existing setup ...
    let mut total_bytes: u64 = 0;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_size = entry.header().size()?;

        if entry_size > MAX_ENTRY_SIZE {
            return Err(anyhow!("Archive entry exceeds maximum size: {} bytes", entry_size));
        }
        total_bytes += entry_size;
        if total_bytes > MAX_TOTAL_SIZE {
            return Err(anyhow!("Archive total extraction exceeds {} bytes", MAX_TOTAL_SIZE));
        }

        // ... rest of extraction logic ...
    }
```

- [ ] **Step 2: Add hex validation for object hash paths**

After reconstructing the hash from `prefix` and `suffix` (line 314), validate:

```rust
if let Some((prefix, suffix)) = path_str.split_once('/') {
    if !prefix.chars().all(|c| c.is_ascii_hexdigit())
        || !suffix.chars().all(|c| c.is_ascii_hexdigit())
    {
        warn!("Skipping non-hex object path: {}", path_str);
        continue;
    }
    let hash = format!("{}{}", prefix, suffix);
    // ...
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p conary-core -- ccs::package`
Expected: PASS

- [ ] **Step 4: Commit**

```
security(ccs): add size limits and hex validation to archive extraction
```

---

### Task 13: Fix DEB parser unbounded memory (Packages)

**Files:**
- Modify: `conary-core/src/packages/deb.rs:150-182`

**Problem:** `extract_ar_members()` reads both control and data tarballs entirely into memory. Large `.deb` packages (800MB+) = OOM.

- [ ] **Step 1: Add size limit check on AR entry before reading**

Before `read_to_end`, check the AR entry header size against `MAX_EXTRACTION_FILE_SIZE` from `common.rs`:

```rust
let entry_size = entry.header().size();
if entry_size > MAX_EXTRACTION_FILE_SIZE as u64 {
    return Err(anyhow!("DEB archive entry too large: {} bytes", entry_size));
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-core -- packages::deb`
Expected: PASS

- [ ] **Step 3: Commit**

```
security(packages): add size limit to DEB archive member extraction
```

---

### Task 14: Fix shell injection in bootstrap stage0 (Bootstrap)

**Files:**
- Modify: `conary-core/src/bootstrap/stage0.rs:264-271`

**Problem:** `work_dir` is interpolated into a shell command via `format!("cd {} && ...")`, enabling shell injection.

- [ ] **Step 1: Use `Command` with `current_dir` instead of shell `cd`**

```rust
// Replace the su -c with cd approach:
cmd = Command::new("su");
cmd.args(["-s", "/bin/bash", &build_user, "-c"])
    .arg(format!("{} ct-ng build", ct_env))
    .current_dir(&self.work_dir);
```

If `su -c` doesn't support `current_dir` (it doesn't -- it runs in the user's home), use `shlex::quote()` on the path:

```rust
let safe_dir = shell_escape::escape(self.work_dir.display().to_string().into());
cmd.arg(format!("cd {} && {} ct-ng build", safe_dir, ct_env));
```

Add `shell-escape` to `conary-core/Cargo.toml` if not already present, or use a simple manual quoting function that wraps in single quotes with internal quote escaping.

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-core -- bootstrap`
Expected: PASS

- [ ] **Step 3: Commit**

```
security(bootstrap): fix shell injection in stage0 ct-ng invocation
```

---

### Task 15: Fix non-atomic bootstrap state file writes (Bootstrap)

**Files:**
- Modify: `conary-core/src/bootstrap/stages.rs:281-286`

**Problem:** `std::fs::write()` is not atomic. Crash mid-write corrupts the state file, losing hours of build progress.

- [ ] **Step 1: Write to temp file then rename atomically**

```rust
fn save(&self) -> Result<()> {
    let content = serde_json::to_string_pretty(&self)?;
    let tmp_path = self.state_file.with_extension("json.tmp");
    std::fs::write(&tmp_path, &content)?;
    std::fs::rename(&tmp_path, &self.state_file)?;
    Ok(())
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-core -- bootstrap::stages`
Expected: PASS

- [ ] **Step 3: Commit**

```
fix(bootstrap): use atomic write-then-rename for state file persistence
```

---

### Task 16: Fix `expand_env_vars()` docstring mismatch (Bootstrap)

**Files:**
- Modify: `conary-core/src/bootstrap/build_helpers.rs:85-96`

**Problem:** Docstring says "falls back to system environment" but implementation returns empty string for missing vars. In a sandboxed build, leaking host env is wrong.

- [ ] **Step 1: Fix the docstring to match the implementation**

```rust
/// Expand environment variables in a string.
///
/// Variables use `${VAR}` syntax. Looks up values in `build_env` only.
/// Variables not found in `build_env` expand to empty string (host
/// environment is intentionally not consulted for build hermiticity).
```

- [ ] **Step 2: Commit**

```
docs(bootstrap): fix expand_env_vars docstring to match hermetic behavior
```

---

### Task 17: Fix double-wait on child process (Container)

**Files:**
- Modify: `conary-core/src/container/mod.rs:507-523`
- Reference: `conary-core/src/trigger/mod.rs:263-280` (correct pattern)

**Problem:** After `wait_timeout` returns `Some(status)`, calling `wait_with_output()` calls `wait()` again, triggering `ECHILD`.

- [ ] **Step 1: Read pipes directly after wait_timeout, don't call wait_with_output**

```rust
match child.wait_timeout(self.config.timeout)? {
    Some(status) => {
        // Read stdout/stderr directly from the child's pipes
        let mut stdout_str = String::new();
        let mut stderr_str = String::new();
        if let Some(mut stdout) = child.stdout.take() {
            use std::io::Read;
            stdout.read_to_string(&mut stdout_str).ok();
        }
        if let Some(mut stderr) = child.stderr.take() {
            use std::io::Read;
            stderr.read_to_string(&mut stderr_str).ok();
        }
        let code = status.code().unwrap_or(-1);
        Ok((code, stdout_str, stderr_str))
    }
    None => {
        let _ = child.kill();
        Err(Error::ScriptletError(format!(
            "Script timed out after {:?}",
            self.config.timeout
        )))
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-core -- container`
Expected: PASS

- [ ] **Step 3: Commit**

```
fix(container): read pipes directly after wait_timeout instead of double-wait
```

---

### Task 18: Fix `expect()` in model signing (Model)

**Files:**
- Modify: `conary-core/src/model/signing.rs:75,77`

**Problem:** `expect()` calls in `canonical_json()` for model signing.

- [ ] **Step 1: Propagate errors instead of panicking**

If Task 9 (trust `canonical_json` fix) is completed first, this may already be resolved since model signing likely calls the same function. If `signing.rs` has its own `canonical_json`, apply the same fix: return `Result` and use `?`.

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-core -- model`
Expected: PASS

- [ ] **Step 3: Commit**

```
fix(model): replace expect() with error propagation in canonical_json
```

---

### Task 19: Fix journal state mapping for FileMoved/FileRemoved (Transaction)

**Files:**
- Modify: `conary-core/src/transaction/journal.rs:129`

**Problem:** `FileMoved` and `FileRemoved` map to `TransactionState::Staged` but they occur during the FS_APPLIED phase. Recovery could roll back when it should roll forward.

- [ ] **Step 1: Fix the mapping**

```rust
// Change line 129 from:
Self::FileMoved { .. } | Self::FileRemoved { .. } => TransactionState::Staged,
// To:
Self::FileMoved { .. } | Self::FileRemoved { .. } => TransactionState::FsApplied,
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-core -- transaction`
Expected: PASS

- [ ] **Step 3: Commit**

```
fix(transaction): map FileMoved/FileRemoved to FsApplied state for correct recovery
```

---

### Task 20: Fix EROFS `Superblock::new()` panic (EROFS)

**Files:**
- Modify: `conary-erofs/src/superblock.rs:91-97`
- Modify: `conary-erofs/src/lib.rs` or create `conary-erofs/src/error.rs`

**Problem:** `assert!()` in public constructor panics on invalid input.

- [ ] **Step 1: Define `ErofsError` type**

```rust
#[derive(Debug, thiserror::Error)]
pub enum ErofsError {
    #[error("invalid block size: must be a power of two >= 512, got {0}")]
    InvalidBlockSize(u32),
    #[error("path traversal detected: {0}")]
    PathTraversal(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
```

- [ ] **Step 2: Convert `Superblock::new()` to return `Result`**

```rust
pub fn new(block_size: u32) -> Result<Self, ErofsError> {
    if !block_size.is_power_of_two() || block_size < 512 {
        return Err(ErofsError::InvalidBlockSize(block_size));
    }
    // ... rest unchanged, wrap in Ok(...)
}
```

- [ ] **Step 3: Update all callers of `Superblock::new()`**

- [ ] **Step 4: Run tests**

Run: `cargo test -p conary-erofs`
Expected: PASS

- [ ] **Step 5: Commit**

```
fix(erofs): define ErofsError type, convert Superblock::new to return Result
```

---

### Task 21: Fix EROFS `normalize_path()` panic (EROFS)

**Files:**
- Modify: `conary-erofs/src/builder.rs:649-651`

**Problem:** `assert!()` on path traversal in public API.

- [ ] **Step 1: Return `Result` instead of panicking**

```rust
fn normalize_path(path: &str) -> Result<String, ErofsError> {
    // ... existing logic, but replace assert! with:
    if component == ".." {
        return Err(ErofsError::PathTraversal(path.to_string()));
    }
    // ... wrap return in Ok(...)
}
```

- [ ] **Step 2: Update `add_file`, `add_symlink`, `add_directory` to propagate the error**

- [ ] **Step 3: Run tests**

Run: `cargo test -p conary-erofs`
Expected: PASS

- [ ] **Step 4: Commit**

```
fix(erofs): convert normalize_path to return Result instead of panicking
```

---

### Task 22: Fix EROFS ZEROS buffer overflow (EROFS)

**Files:**
- Modify: `conary-erofs/src/builder.rs:346,441,458`

**Problem:** `ZEROS` is 4096 bytes but block sizes > 4096 are accepted, causing out-of-bounds slice.

- [ ] **Step 1: Write padding in a loop**

Replace each `writer.write_all(&ZEROS[..pad as usize])` with:

```rust
let mut remaining = pad as usize;
while remaining > 0 {
    let chunk = remaining.min(ZEROS.len());
    writer.write_all(&ZEROS[..chunk])?;
    remaining -= chunk;
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-erofs`
Expected: PASS

- [ ] **Step 3: Commit**

```
fix(erofs): write padding in chunks to handle block sizes > 4096
```

---

### Task 23: Fix blocking I/O in async handler -- `check_converted` (Server)

**Files:**
- Modify: `conary-server/src/server/handlers/packages.rs:170`

**Problem:** `db::open()` called in async context without `spawn_blocking`.

- [ ] **Step 1: Wrap in `spawn_blocking` and use `open_fast`**

```rust
let result = tokio::task::spawn_blocking(move || {
    let conn = conary_core::db::open_fast(&db_path)?;
    // ... existing query logic ...
    Ok::<_, anyhow::Error>(result)
}).await??;
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-server`
Expected: PASS

- [ ] **Step 3: Commit**

```
fix(server): wrap check_converted DB call in spawn_blocking
```

---

### Task 24: Fix blocking file read in `get_latest` (Server)

**Files:**
- Modify: `conary-server/src/server/handlers/self_update.rs:128`

**Problem:** `std::fs::read()` of entire CCS file in async handler.

- [ ] **Step 1: Use `tokio::fs::read` instead**

```rust
// Change:
let data = std::fs::read(&ccs_path)?;
// To:
let data = tokio::fs::read(&ccs_path).await?;
```

- [ ] **Step 2: Commit**

```
fix(server): use tokio::fs::read for async CCS file serving
```

---

### Task 25: Fix OCI blob handler hash normalization (Server)

**Files:**
- Modify: `conary-server/src/server/handlers/oci.rs:322`

**Problem:** `get_blob_inner` skips `normalize_hash()` after `strip_digest_prefix()`, causing cache misses for mixed-case digests.

- [ ] **Step 1: Add normalization**

After `strip_digest_prefix`:

```rust
let hash = normalize_hash(&hash);
```

- [ ] **Step 2: Commit**

```
fix(server): normalize hash in OCI blob handler to prevent cache bypass
```

---

### Task 26: Fix rate limit off-by-one in auth (Server)

**Files:**
- Modify: `conary-server/src/server/auth.rs:160-175,194-198`

**Problem:** Rate limit checked AFTER consuming a token, giving N+1 attempts per window.

- [ ] **Step 1: Check rate limit FIRST, then return 401**

```rust
// For missing token (lines 160-175):
// Check rate limit first
if let Some(limiter) = &state.rate_limiter {
    if !limiter.check_auth_failure(&client_ip) {
        return Err(StatusCode::TOO_MANY_REQUESTS.into_response());
    }
}
// Then return 401
return Err((StatusCode::UNAUTHORIZED, "Missing or invalid Authorization header").into_response());
```

Apply same pattern for invalid-token case at lines 194-198.

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-server -- auth`
Expected: PASS

- [ ] **Step 3: Commit**

```
security(server): fix auth rate limiting to check before consuming token
```

---

### Task 27: Fix localhost admin API authentication (Server)

**Files:**
- Modify: `conary-server/src/server/routes.rs` (internal router setup)

**Problem:** Port 8081 admin API has zero authentication -- any local process gets full admin access.

- [ ] **Step 1: Add IP validation to internal router**

Add middleware or a check that `ConnectInfo` source IP is `127.0.0.1` or `::1`:

```rust
async fn require_localhost(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    if !addr.ip().is_loopback() {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}
```

Apply to the internal router.

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-server -- routes`
Expected: PASS

- [ ] **Step 3: Commit**

```
security(server): add localhost-only check to internal admin API
```

---

### Task 28: Fix daemon TCP read access (Server)

**Files:**
- Modify: `conary-server/src/daemon/routes.rs:186-193`

**Problem:** When TCP is enabled, all read endpoints are accessible without authentication.

- [ ] **Step 1: Add startup warning when TCP is enabled**

```rust
if config.enable_tcp {
    warn!("TCP listener enabled -- read endpoints accessible without authentication. \
           Use only in trusted networks.");
}
```

- [ ] **Step 2: Commit**

```
security(daemon): add warning when TCP listener exposes unauthenticated read endpoints
```

---

### Task 29: Fix `DaemonState::open_db` skipping WAL/pragmas (Server)

**Files:**
- Modify: `conary-server/src/daemon/mod.rs:419-422`

**Problem:** Uses raw `rusqlite::Connection::open` without WAL, busy_timeout, or foreign_keys.

- [ ] **Step 1: Use `conary_core::db::open_fast` instead**

```rust
// Change:
let conn = rusqlite::Connection::open(&self.db_path)?;
// To:
let conn = conary_core::db::open_fast(&self.db_path)?;
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p conary-server -- daemon`
Expected: PASS

- [ ] **Step 3: Commit**

```
fix(daemon): use open_fast for WAL mode and proper pragmas
```

---

## Phase 2: P1 Security Fixes

These are security-adjacent P1 findings. Each is an independent task.

### Task 30: Add path traversal validation to Arch parser
- **File:** `conary-core/src/repository/parsers/arch.rs:198-199`
- **Fix:** Add filename validation matching `debian.rs:151-159` pattern
- **Commit:** `security(repository): add path traversal validation to Arch package parser`

### Task 31: Add package size validation to Arch and Debian parsers
- **File:** `conary-core/src/repository/parsers/arch.rs:189-193`, `debian.rs:138-141`
- **Fix:** Add `MAX_PACKAGE_SIZE` constant, validate size fields
- **Commit:** `security(packages): add package size validation to Arch and Debian parsers`

### Task 32: Validate alternative name/path in CCS hooks
- **File:** `conary-core/src/ccs/hooks/alternatives.rs:47-53`
- **Fix:** Validate `name` contains only `[a-zA-Z0-9_-]`, `path` is absolute without `..`
- **Commit:** `security(ccs): validate alternative name and path in hooks`

### Task 33: Validate sysctl key/value in CCS hooks
- **File:** `conary-core/src/ccs/hooks/sysctl.rs:27-28`
- **Fix:** Validate key matches `[a-zA-Z0-9._/-]+`, value has no newlines
- **Commit:** `security(ccs): validate sysctl key and value before writing config`

### Task 34: Fix CPIO allocation based on untrusted size
- **File:** `conary-core/src/packages/cpio.rs:135`
- **Fix:** Lower `MAX_FILE_SIZE` to 512 MB or use streaming for large entries
- **Commit:** `security(packages): reduce CPIO max file size allocation`

### Task 35: Add checksum verification for remote patches in recipes
- **File:** `conary-core/src/recipe/kitchen/cook.rs` (patch phase)
- **Fix:** Require and verify checksums for all remote patch URLs
- **Commit:** `security(recipe): require checksums for remote patches`

### Task 36: Fix `validate_port_spec` allowing port 0
- **File:** `conary-core/src/capability/declaration.rs:296-318`
- **Fix:** Add `if port_num == 0 { return Err(...) }` after parse
- **Commit:** `fix(capability): reject port 0 in validate_port_spec`

---

## Phase 3: P1 Correctness Fixes

### Task 37: Fix `find_orphans()` transitive dependency walk
- **File:** `conary-core/src/db/models/trove.rs:277-296`
- **Fix:** Use recursive CTE to walk full dependency tree from explicit packages
- **Commit:** `fix(db): use recursive CTE for transitive orphan detection`

### Task 38: Fix `DistroPin::set()` non-atomic delete+insert
- **File:** `conary-core/src/db/models/distro_pin.rs:23-30`
- **Fix:** Wrap in SAVEPOINT or use `INSERT OR REPLACE`
- **Commit:** `fix(db): make DistroPin::set atomic`

### Task 39: Fix `Changeset::update_status` SQL format injection
- **File:** `conary-core/src/db/models/changeset.rs:128-130`
- **Fix:** Use two separate static SQL strings instead of `format!` for column name
- **Commit:** `fix(db): remove format-based SQL column injection in Changeset::update_status`

### Task 40: Fix `audit_log::query` LIMIT format injection
- **File:** `conary-core/src/db/models/audit_log.rs:89`
- **Fix:** Use `LIMIT ?N` with parameter binding
- **Commit:** `fix(db): use parameter binding for LIMIT in audit_log query`

### Task 41: Fix `format_permissions` symlink/directory bitmask
- **File:** `conary-core/src/db/models/file_entry.rs:173-174`
- **Fix:** Use `(mode & 0o170000)` to extract file type, check symlinks first
- **Commit:** `fix(db): fix format_permissions symlink detection bitmask`

### Task 42: Fix `autoremove` fixed-point iteration
- **File:** `src/commands/remove.rs:418-444`
- **Fix:** Loop orphan detection+removal until no more orphans found
- **Commit:** `fix(remove): iterate autoremove to fixed point`

### Task 43: Fix phantom changeset creation in `cmd_update`
- **File:** `src/commands/update.rs:371-377,547-553`
- **Fix:** Only create changeset when actual work exists
- **Commit:** `fix(update): don't create changeset when no updates needed`

### Task 44: Replace all `process::exit(1)` with proper error returns
- **Files:** `src/commands/provenance.rs`, `model.rs`, `automation.rs`, `src/main.rs`
- **Fix:** Return `Err(anyhow!(...))` instead of `process::exit(1)` (11 call sites)
- **Commit:** `fix(cli): replace process::exit calls with proper error returns`

### Task 45: Replace all production `expect()`/`unwrap()` in CLI
- **Files:** `src/commands/registry.rs:37`, `ccs/build.rs:53`, `adopt/hooks.rs:99`, `groups.rs:40`
- **Fix:** Replace each with `ok_or_else(|| anyhow!(...))?`
- **Commit:** `fix(cli): replace expect/unwrap with proper error propagation`

### Task 46: Fix unsafe timestamp cast in provenance
- **File:** `src/commands/provenance.rs:950,952`
- **Fix:** Use `u64::try_from(timestamp).map_err(|_| ...)`
- **Commit:** `fix(provenance): use try_from for timestamp i64->u64 conversion`

### Task 47: Fix `expect()` on Tokio runtime creation
- **File:** `src/main.rs:428,1557,1599,1690`
- **Fix:** Use `?` with `.map_err(|e| anyhow!(...))`
- **Commit:** `fix(cli): replace expect on Tokio runtime with error propagation`

### Task 48: Fix Remi sync hardcoded x86_64 architecture
- **File:** `conary-core/src/repository/sync.rs:240-254`
- **Fix:** Use `registry::detect_system_arch()` dynamically
- **Commit:** `fix(repository): detect system architecture instead of hardcoding x86_64`

### Task 49: Fix `download_chunks()` doc vs implementation mismatch
- **File:** `conary-core/src/repository/remi.rs:290-293`
- **Fix:** Either implement parallel downloads or fix the doc comment
- **Commit:** `docs(repository): fix download_chunks doc to match sequential implementation`

### Task 50: Fix `expect()` in canonical.rs production code
- **File:** `conary-core/src/resolver/canonical.rs:172`
- **Fix:** Replace with `ok_or_else`
- **Commit:** `fix(resolver): replace expect with error propagation in canonical resolution`

### Task 51: Fix `unwrap_or(0)` for canonical_id
- **File:** `conary-core/src/resolver/canonical.rs:54`
- **Fix:** Return error or early-return empty vec when id is None
- **Commit:** `fix(resolver): handle missing canonical_id instead of defaulting to 0`

### Task 52: Fix `unwrap()` in resolver engine
- **File:** `conary-core/src/resolver/engine.rs:72`
- **Fix:** Use `if let Some(target) = ...` pattern
- **Commit:** `fix(resolver): replace unwrap with if-let in dependency graph`

### Task 53: Fix `expect()` in resolver provider pool overflow
- **File:** `conary-core/src/resolver/provider.rs:455`
- **Fix:** Return `Dependencies::Unknown` or propagate error
- **Commit:** `fix(resolver): handle version set pool overflow gracefully`

### Task 54: Fix operator precedence bug in `analyze_package_name()`
- **File:** `conary-core/src/capability/inference/heuristics.rs:209-212`
- **Fix:** Add parentheses to fix `||`/`&&` precedence
- **Commit:** `fix(capability): fix operator precedence in server package detection`

### Task 55: Fix `expect()` in bootstrap production paths
- **File:** `conary-core/src/bootstrap/build_helpers.rs:40-45`, `stage1.rs:303`
- **Fix:** Replace `.expect()` with `ok_or_else`
- **Commit:** `fix(bootstrap): replace expect with error propagation for path validation`

### Task 56: Strengthen BuildCache verify_entry with checksum
- **File:** `conary-core/src/recipe/cache.rs`
- **Fix:** Store expected SHA-256, verify on retrieval
- **Commit:** `fix(recipe): verify cached artifact checksums on retrieval`

### Task 57: Fix `current_stage()` return when all stages complete
- **File:** `conary-core/src/bootstrap/stages.rs:174-181`
- **Fix:** Return error or distinct sentinel when all stages done
- **Commit:** `fix(bootstrap): return error from current_stage when all stages complete`

### Task 58: Replace DefaultHasher with deterministic hash in tmpfiles
- **File:** `conary-core/src/ccs/hooks/tmpfiles.rs:85-90`
- **Fix:** Use FNV or CRC32 instead of `DefaultHasher`
- **Commit:** `fix(ccs): use deterministic hash for tmpfiles config naming`

### Task 59: Fix directory.rs mode "0" edge case
- **File:** `conary-core/src/ccs/hooks/directory.rs:52-56`
- **Fix:** Handle empty string after prefix strip
- **Commit:** `fix(ccs): handle mode "0" in directory hook`

### Task 60: Fix stage2 `compare_dirs()` to check file content
- **File:** `conary-core/src/bootstrap/stage2.rs`
- **Fix:** Hash and compare file contents, not just paths/counts
- **Commit:** `fix(bootstrap): compare file contents in reproducibility check`

### Task 61: Fix diamond include false cycle detection
- **File:** `conary-core/src/model/mod.rs:355-361`
- **Fix:** Use stack-based cycle detection (add on entry, remove on exit)
- **Commit:** `fix(model): fix diamond include false positive in cycle detection`

### Task 62: Fix fork isolation losing child output
- **File:** `conary-core/src/container/mod.rs:565`
- **Fix:** Set up pipe pairs before fork, read from parent side
- **Commit:** `fix(container): capture stdout/stderr in fork-based isolation`

### Task 63: Fix recovery `get_changeset_id_by_uuid` silent failure
- **File:** `conary-core/src/transaction/recovery.rs:424`
- **Fix:** Return `Result<Option<i64>>`
- **Commit:** `fix(transaction): distinguish not-found from error in changeset UUID lookup`

### Task 64: Fix O(n^2) rollback complexity
- **File:** `conary-core/src/transaction/recovery.rs:202-204`
- **Fix:** Pre-build `HashSet<String>` of backup paths
- **Commit:** `perf(transaction): use HashSet for O(1) backup path lookup during rollback`

### Task 65: Fix version comparison RPM compatibility
- **File:** `conary-core/src/version/mod.rs`
- **Fix:** Implement `rpmvercmp`-compatible logic for non-numeric segments
- **Commit:** `fix(version): implement RPM-compatible version comparison`

### Task 66: Fix `VersionConstraint::Exact` epoch/release matching
- **File:** `conary-core/src/version/mod.rs`
- **Fix:** Normalize epoch to 0 and release to empty when missing
- **Commit:** `fix(version): normalize epoch and release for exact version matching`

### Task 67: Fix `self_update::is_newer()` pre-release handling
- **File:** `conary-core/src/self_update.rs`
- **Fix:** Parse and compare pre-release per SemVer rules
- **Commit:** `fix(self_update): handle pre-release versions in is_newer comparison`

### Task 68: Fix `detect_soname` returning filename instead of soname
- **File:** `conary-core/src/dependencies/detection.rs:249-263`
- **Fix:** Strip minor/patch from `.so.X.Y.Z` to approximate soname
- **Commit:** `fix(dependencies): approximate soname by stripping minor/patch version`

### Task 69: Fix `AutomationDaemon` stop requiring `&mut self`
- **File:** `conary-core/src/automation/scheduler.rs`
- **Fix:** Use `Arc<AtomicBool>` for the running flag
- **Commit:** `fix(automation): use AtomicBool for daemon stop flag`

### Task 70: Fix `check_version_constraint` string fallback
- **File:** `conary-core/src/dependencies/classes.rs`
- **Fix:** Return error instead of falling back to string comparison
- **Commit:** `fix(dependencies): error on version parse failure instead of string fallback`

### Task 71: Fix dirent nameoff u16 truncation
- **File:** `conary-erofs/src/dirent.rs:92`
- **Fix:** Add bounds check, return error when `name_offset > u16::MAX`
- **Commit:** `fix(erofs): add bounds check for dirent nameoff u16`

### Task 72: Fix `to_sse()` unwrap in conary-test
- **File:** `conary-test/src/report/stream.rs:57`
- **Fix:** Return `Result` or use fallback
- **Commit:** `fix(test): replace unwrap in to_sse with error handling`

### Task 73: Add eviction to conary-test runs HashMap
- **File:** `conary-test/src/server/state.rs:16`
- **Fix:** Keep last N runs or expire after 1 hour
- **Commit:** `fix(test): add eviction policy to prevent unbounded run history growth`

### Task 74-78: Server P1 fixes
- **74:** `find_missing` hash normalization -- `chunks.rs:543`
- **75:** `update_repo` partial update fix -- `admin_service.rs:389`
- **76:** `purge_audit` date validation -- `admin_service.rs:291`
- **77:** `server_info` path exposure -- `routes.rs`
- **78:** Audit middleware body truncation -- `audit.rs:109`

### Task 79-82: Server infra P1 fixes
- **79:** `ServiceError` -> thiserror -- `admin_service.rs`
- **80:** `ForgejoError` -> thiserror -- `forgejo.rs`
- **81:** Add timeout to Forgejo requests -- `forgejo.rs`
- **82:** Fix MCP error mapping -- `mcp.rs:61-66`
- **83:** Fix `list_all` SQL format injection -- `jobs.rs:163`
- **84:** Fix TOCTOU race in cancel -- `jobs.rs:407-428`

---

## Phase 4: P2 Fixes (70 findings)

Grouped by subsystem. Each is independent. Reference the review reports for full details.

### DB P2 (Tasks 85-91)
- 85: Transaction wrapper for `DownloadStat::insert_batch`
- 86: Doc comment for `RepositoryPackage::batch_insert` transaction requirement
- 87: Fix `ConvertedPackage::new_server` `unwrap_or_default` to `"[]"`
- 88: Log warning on `Trove::from_row` parse fallbacks
- 89: Inconsistent model patterns (free functions vs struct methods) -- document
- 90: `StateDiff::compare` memory optimization -- defer
- 91: `FileEntry::insert_or_replace` ownership semantics review

### CCS + Packages P2 (Tasks 92-97)
- 92: Single-pass `ArchPackage::parse()` (3 decompressions -> 1)
- 93: Cache data tarball in `DebPackage` to avoid double extraction
- 94: Define `BuilderError` thiserror enum for `ccs/builder.rs`
- 95: Validate override target paths in `derived/builder.rs`
- 96: Make `registry.rs` `detect_format` propagate file open errors
- 97: Return `Result` from `archive_utils::get_file_metadata`

### Repo + Resolver P2 (Tasks 98-104)
- 98: Convert `Conflict` enum to thiserror derive
- 99: Add HashMap index for version_set lookup (performance)
- 100: Add transient error retry to `poll_for_completion`
- 101: Add retry logic to `download_chunks`
- 102: Fix `u32` casts in resolver with `try_from`
- 103: Remove dead code in debian.rs alternative handling
- 104: Fix `ConflictingConstraints` trailing newline

### Bootstrap + Recipe + Capability P2 (Tasks 105-113)
- 105: Extract shared `PackageBuildRunner` from stage1/stage2/base
- 106: Eliminate double CCS build in `plate()`
- 107: Stream file hashing in `archive.rs` instead of reading into memory
- 108: Use `LazyLock` for PKGBUILD regex patterns
- 109: Fix PKGBUILD brace-counting to handle strings/comments
- 110: Make RecipeGraph bootstrap edges configurable
- 111: Deduplicate syscall profile lists in `declaration.rs`
- 112: Add aarch64 syscall mappings in `seccomp_enforce.rs`
- 113: Use `Path::starts_with()` in capability resolver

### Model + Transaction + FS P2 (Tasks 114-120)
- 114: Migrate `fsverity.rs` from anyhow to thiserror
- 115: Cache hash computation in transaction planner
- 116: Log warning when deploy_symlink skips existing directory
- 117: Populate `model_hash` in lockfile or remove field
- 118: Clean orphaned journal placeholder files
- 119: Fix `BackupInfo.size` conversion pattern
- 120: Document symlink validation asymmetry in recovery

### Trust + Small Modules P2 (Tasks 121-128)
- 121: Remove duplicate `ConfigError`/`IoError` variants in `error.rs`
- 122: Fix `is_lib_file` `.so` substring false positive
- 123: Preserve IO error context in `verify_file`
- 124: Log warning on `self_update` `fs::read` failure
- 125: Stream self-update download through hasher
- 126: Change `ceremony.rs` to return `TrustResult`
- 127: Replace `expect()` in `repology.rs:262`
- 128: Remove dead `ContentProvenanceBuilder` or integrate it

### CLI P2 (Tasks 129-135)
- 129: Consolidate multiple DB opens in `cmd_install`
- 130: Remove `#![allow(dead_code)]` blanket in `progress.rs`
- 131: Deduplicate `format_bytes` / `human_bytes` to shared utility
- 132: Fix `model check` exit code handling
- 133: Use `ValueEnum` for `SandboxMode` and `DepMode`

### EROFS + Test P2 (Tasks 134-139)
- 134: Remove unused thiserror and tracing deps from conary-erofs
- 135: Extract service layer from conary-test handlers + mcp
- 136: Fix double JSON serialization in conary-test get_run
- 137: Use `suite.status.as_str()` in MCP list_runs
- 138: Use `Vec<TestPackage>` instead of numbered fields in DistroConfig
- 139: Add validation for conflicting assertion fields

### Server P2 (Tasks 140-151)
- 140: Debounce auth token `touch()` calls
- 141: Replace deprecated `rand::thread_rng()`
- 142: Use `Arc<Peer>` in `PeerRegistry::all()`
- 143: Remove duplicate `SO_PEERCRED` extraction
- 144: Fix subdomain wildcard matching bare domain
- 145: Replace `expect()` in `canonical_bytes()`
- 146: Parallelize local cache lookups in `fetch_many`
- 147: Apply limit to `list_transactions` with status filter
- 148: Cache `scan_versions` result with TTL
- 149: Wrap readiness check in `spawn_blocking`
- 150: Only count 401/403 in ban middleware
- 151: Convert remaining errors to thiserror

---

## Phase 5: P3 Style/Cleanup (38 findings)

Low priority. Address during normal development when touching adjacent code.

### DB P3 (3): Move `format_size` utility, remove dead TROVE_COLUMNS_PREFIXED, document changeset.metadata
### CCS P3 (5): Fix unreachable match, compute relative path dynamically, handle user_group UTF-8 error, doc DEB installed_size unit
### Repo P3 (3): Remove unnecessary Debug derive, use `mem::take`, rename `isize_val`
### Bootstrap P3 (6): Fix host PATH leak, fix InferenceCache read lock, fix wellknown `$HOME` paths, fix build tools `.` path, rename PackageMetadataRef, add TODO for unused NameHints fields
### Model P3 (3): Remove dead deployer field, document canonicalize fallback, add try_get_node variant
### Trust P3 (5): Rename DnaHashError::ShortInput, use typed ParseRoleError, add cfg(target_os) guard, use or_default(), document Label wildcard design
### CLI P3 (2): Use ValueEnum for CLI enums, add #[must_use] on query functions
### EROFS P3 (4): Feature-gate dead code, remove unused deps, document cast_possible_truncation, fix RUN_COUNTER test note
### Server P3 (7): Reject empty scopes, extract shared test_app, verify latency cast, add job_id to enhancement events, add Action::Enhance, clean CircuitBreakerRegistry, document DaemonEvent

---

## Execution Notes

- **Phase 1 tasks are all independent** -- dispatch all 29 as parallel subagents
- **Phase 2-3 tasks are independent** -- dispatch in batches of 8-10
- **Run `cargo clippy -- -D warnings` after each phase** to catch regressions
- **Run `cargo test` after each phase** to verify no breakage
- **Commit convention:** Each task gets its own commit with the specified message
- **Do not combine tasks** into single commits -- they should be independently revertable
