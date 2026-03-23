# Codebase Review Remediation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix all findings from the full codebase review — 20 Critical, 52 Important, 48 Minor issues across 485 files / 208K lines.

**Architecture:** Fixes are organized into 6 phases by priority. Each phase is independently deployable. Within phases, tasks are ordered by file dependency (shared modules first). One false positive was identified and excluded (CCS verify.rs blob hash reconstruction — `rsplit('/')` is actually correct for both `./` and non-`./` paths).

**Tech Stack:** Rust 1.94, Edition 2024, rusqlite, tokio, axum, ed25519-dalek, resolvo, bollard

**Commit convention:** Conventional Commits per CLAUDE.md — `fix:`, `security:`, `refactor:`, `test:`

---

## Phase 1: Security-Critical Fixes (P0)

These must be fixed before any release. Trust/verification gaps, input validation, auth bypass.

---

### Task 1: CCS PolicyChain.apply — return Replace when content was modified

**Files:**
- Modify: `conary-core/src/ccs/policy.rs:140-168`
- Modify: `conary-core/src/ccs/builder.rs:222-234`
- Test: `conary-core/src/ccs/policy.rs` (inline tests)

**Context:** `PolicyChain::apply()` always returns `PolicyAction::Keep` even when a policy returned `Replace` and modified `current_content`. The builder checks the action to decide whether to rehash — so policy-modified files get stale hashes.

- [ ] **Step 1: Write failing test**

Add to the existing `#[cfg(test)]` module in `policy.rs`. Note the actual signature: `apply(&self, entry: &mut FileEntry, content: Vec<u8>, source_path: &Path, config: &BuildPolicyConfig)`:

```rust
#[test]
fn test_policy_chain_returns_replace_when_content_modified() {
    use super::*;
    use std::path::Path;
    // Create a chain with FixShebangsPolicy which modifies #!/usr/local paths
    let mut chain = PolicyChain::new();
    chain.add(Box::new(FixShebangsPolicy));
    let content = b"#!/usr/local/bin/python3\nprint('hello')".to_vec();
    let original = content.clone();
    let mut entry = FileEntry {
        path: "usr/bin/script.py".into(),
        hash: String::new(),
        size: content.len() as u64,
        mode: 0o755,
        file_type: FileType::Regular,
        component: None,
        chunks: vec![],
    };
    let config = BuildPolicyConfig::default();
    let (action, new_content) = chain.apply(&mut entry, content, Path::new("usr/bin/script.py"), &config).unwrap();
    // If content was modified, action MUST be Replace
    if new_content != original {
        assert!(matches!(action, PolicyAction::Replace(_)),
            "PolicyChain must return Replace when content was modified, got Keep");
    }
}
```

- [ ] **Step 2: Run test, verify it fails**

Run: `cargo test -p conary-core policy_chain_returns_replace -- --nocapture`
Expected: FAIL — action is `Keep` even though content was modified.

- [ ] **Step 3: Fix PolicyChain::apply**

In `conary-core/src/ccs/policy.rs`, modify the `apply` method (lines 129-169). The actual signature is `apply(&self, entry: &mut FileEntry, content: Vec<u8>, source_path: &Path, config: &BuildPolicyConfig)`. Add a `was_replaced` tracker:

```rust
pub fn apply(
    &self,
    entry: &mut FileEntry,
    content: Vec<u8>,
    source_path: &Path,
    config: &BuildPolicyConfig,
) -> Result<(PolicyAction, Vec<u8>)> {
    let mut current_content = content;
    let mut was_replaced = false;

    for policy in &self.policies {
        let ctx = PolicyContext {
            source_path,
            entry,
            content: &current_content,
            config,
        };

        match policy.apply(&ctx)? {
            PolicyAction::Keep => {}
            PolicyAction::Replace(new_content) => {
                current_content = new_content;
                was_replaced = true;
            }
            PolicyAction::Skip => {
                return Ok((PolicyAction::Skip, current_content));
            }
            PolicyAction::Reject(msg) => {
                return Err(PolicyError::Violation {
                    policy: policy.name().to_string(),
                    message: msg,
                }
                .into());
            }
        }
    }

    if was_replaced {
        // Signal to caller that content was modified and needs rehashing.
        // The Replace payload is unused by the builder — it uses the second tuple element.
        Ok((PolicyAction::Replace(Vec::new()), current_content))
    } else {
        Ok((PolicyAction::Keep, current_content))
    }
}
```

- [ ] **Step 4: Run test, verify it passes**

Run: `cargo test -p conary-core policy_chain -- --nocapture`
Expected: All policy tests PASS.

- [ ] **Step 5: Run full conary-core tests**

Run: `cargo test -p conary-core`
Expected: All tests pass. Builder now correctly rehashes when PolicyChain returns Replace.

- [ ] **Step 6: Commit**

```
security: fix PolicyChain.apply returning Keep when content was modified

PolicyChain::apply() always returned PolicyAction::Keep even when a
policy replaced file content. The builder uses this action to decide
whether to rehash — causing stale hashes for policy-modified files
(shebangs, compressed manpages). Track whether any Replace occurred
and return Replace when content was modified.
```

---

### Task 2: CCS signature verification — prevent self-signed package acceptance

**Files:**
- Modify: `conary-core/src/ccs/verify.rs:320-350`
- Test: `conary-core/src/ccs/verify.rs` (inline tests)

**Context:** Signature verification decodes the public key from `PackageSignature` embedded *inside the package itself*, then only checks if that key is in the trusted list. When `trusted_keys` is empty (the default `TrustPolicy`), any self-signed package returns `Valid`.

- [ ] **Step 1: Write failing test**

Note: `SignatureStatus::Untrusted` is a struct variant: `Untrusted { key_id: Option<String> }`.

```rust
#[test]
fn test_verify_untrusted_when_no_trusted_keys() {
    // A package with a valid self-signature but no trusted keys configured
    // should return Untrusted, not Valid
    let policy = TrustPolicy::default(); // allow_unsigned=true, trusted_keys=empty
    assert!(policy.trusted_keys.is_empty());

    // Create a minimal signed package scenario and verify
    // The exact test setup depends on existing test helpers —
    // see existing verify tests for how to construct test packages.
    // Key assertion: when trusted_keys is empty, even a valid signature
    // should produce SignatureStatus::Untrusted { key_id: ... }
}
```

- [ ] **Step 2: Implement fix**

In `verify.rs`, modify the signature verification logic. After verifying the signature is mathematically correct, check the trust list:
```rust
if trusted_keys.is_empty() {
    // No trust anchors configured — signature is valid but untrusted
    return Ok(SignatureStatus::Untrusted { key_id: Some(sig.key_id.clone()) });
}
if !trusted_keys.iter().any(|k| k == &sig.public_key) {
    return Ok(SignatureStatus::Untrusted { key_id: Some(sig.key_id.clone()) });
}
// Key is in trusted list — signature is valid AND trusted
Ok(SignatureStatus::Valid {
    key_id: Some(sig.key_id.clone()),
    timestamp: sig.timestamp.clone(),
})
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p conary-core verify -- --nocapture`
Expected: PASS. Existing tests may need adjustment if they relied on empty trusted_keys returning Valid.

- [ ] **Step 4: Commit**

```
security: prevent self-signed CCS packages from returning Valid status

When trusted_keys is empty, signature verification now returns
Untrusted instead of Valid. A package can embed its own keypair and
self-sign, which would previously pass verification with no trust
anchors configured. Packages must now have their signing key in the
trusted list to achieve Valid status.
```

---

### Task 3: Self-update — add Ed25519 signature verification

**Files:**
- Modify: `conary-core/src/self_update.rs:197-278`
- Test: `conary-core/src/self_update.rs` (inline tests)

**Context:** The self-update pipeline verifies SHA-256 integrity but not cryptographic signatures. A compromised update channel could serve a malicious binary with a matching hash. The project already has `ed25519_dalek`.

- [ ] **Step 1: Add signature field to LatestVersionInfo**

In `self_update.rs`, add `pub signature: Option<String>` to `LatestVersionInfo`. This is the hex-encoded detached Ed25519 signature over the SHA-256 hash.

- [ ] **Step 2: Add public key constant**

Add a compile-time pinned public key for update verification:
```rust
/// Ed25519 public key for verifying self-update signatures.
/// This key is pinned at compile time — updates to the key require a code release.
const UPDATE_SIGNING_KEY: &str = "PLACEHOLDER_GENERATE_REAL_KEY";
```

Note: The actual key will be generated during release setup. For now, use a placeholder and make verification skip when the key is the placeholder (development mode).

- [ ] **Step 3: Add verify_update_signature function**

```rust
fn verify_update_signature(sha256_hash: &str, signature_hex: &str) -> Result<bool> {
    use ed25519_dalek::{Signature, VerifyingKey, Verifier};
    if UPDATE_SIGNING_KEY == "PLACEHOLDER_GENERATE_REAL_KEY" {
        // Development mode — skip verification
        return Ok(true);
    }
    let key_bytes = hex::decode(UPDATE_SIGNING_KEY)?;
    let verifying_key = VerifyingKey::from_bytes(&key_bytes.try_into()
        .map_err(|_| anyhow::anyhow!("invalid update signing key length"))?)?;
    let sig_bytes = hex::decode(signature_hex)?;
    let signature = Signature::from_bytes(&sig_bytes.try_into()
        .map_err(|_| anyhow::anyhow!("invalid signature length"))?);
    Ok(verifying_key.verify(sha256_hash.as_bytes(), &signature).is_ok())
}
```

- [ ] **Step 4: Integrate into download pipeline**

In `stream_update_to_disk`, after SHA-256 verification passes, call `verify_update_signature` if a signature is present in the response. If verification fails, delete the downloaded file and return an error.

- [ ] **Step 5: Write tests and run**

Test that: (a) valid signature passes, (b) tampered hash fails, (c) missing signature is accepted in permissive mode but warns.

Run: `cargo test -p conary-core self_update -- --nocapture`

- [ ] **Step 6: Commit**

```
security: add Ed25519 signature verification to self-update pipeline

The self-update download now verifies an Ed25519 detached signature
over the SHA-256 hash before applying updates. Uses a compile-time
pinned public key. Currently in development mode (placeholder key)
until the release signing infrastructure is set up.
```

---

### Task 4: CLI path traversal in `ccs shell` and `ccs run`

**Files:**
- Modify: `src/commands/ccs/runtime.rs:84-85`
- Modify: `src/commands/ccs/install.rs` (extract shared sanitize function if needed)
- Test: `src/commands/ccs/runtime.rs` (inline tests)

**Context:** `ccs shell` and `ccs run` join file paths from packages to a temp directory without sanitization. `ccs/install.rs` already has `sanitize_package_relative_path` — use it here.

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_path_traversal_rejected() {
    // Paths with .. components must be rejected
    let temp = tempfile::tempdir().unwrap();
    let malicious_path = "../../etc/shadow";
    let rel = malicious_path.strip_prefix('/').unwrap_or(malicious_path);
    let result = sanitize_package_relative_path(rel);
    assert!(result.is_err(), "path traversal should be rejected");
}
```

- [ ] **Step 2: Apply sanitization in runtime.rs**

At line 84-85, before `temp_path.join(rel_path)`, add:
```rust
let rel_path = sanitize_package_relative_path(rel_path)
    .with_context(|| format!("unsafe path in package: {}", rel_path))?;
```

Import `sanitize_package_relative_path` from `ccs/install.rs` (may need to make it `pub` or extract to a shared module).

- [ ] **Step 3: Run tests**

Run: `cargo test ccs_runtime -- --nocapture` and `cargo test -p conary-core`

- [ ] **Step 4: Commit**

```
security: sanitize file paths in ccs shell/run to prevent traversal

Applied sanitize_package_relative_path to file paths before joining
to temp directory in ccs shell and ccs run commands. A malicious
package with paths like ../../etc/shadow could previously write
outside the temp directory.
```

---

### Task 5: Server package upload auth bypass

**Files:**
- Modify: `conary-server/src/server/handlers/admin/packages.rs:83-87`
- Test: `conary-server/src/server/handlers/admin/packages.rs` (inline tests)

**Context:** When `scopes.is_none()`, the entire auth check is skipped. The external admin router should always have scopes populated by the auth middleware.

- [ ] **Step 1: Fix the scope check**

Replace the `if scopes.is_some() && ...` pattern with:
```rust
if let Some(err) = check_scope(&scopes, Scope::Admin) {
    return err;
}
```

This returns 401 when scopes is None (unauthenticated) and 403 when scope is insufficient.

- [ ] **Step 2: Write test for unauthenticated upload rejection**

Add a test that sends a package upload request without a token and expects 401.

- [ ] **Step 3: Audit all handlers for the same pattern**

Grep for `scopes.is_some() &&` across all admin handlers. Based on review, only `packages.rs:83` has this bug — other handlers (repos, federation, ci, tokens, test_data, audit, artifacts) already call `check_scope` unconditionally. Verify this is still the case:

Run: `grep -rn "scopes.is_some()" conary-server/src/`

- [ ] **Step 4: Run server tests**

Run: `cargo test -p conary-server`

- [ ] **Step 5: Commit**

```
security: fix inverted scope check in package upload handler

The scope check skipped authentication when scopes was None
(unauthenticated request). Now properly returns 401 for missing
auth and 403 for insufficient scope. Audited all admin handlers
for the same pattern.
```

---

### Task 6: Server artifact upload size limit

**Files:**
- Modify: `conary-server/src/server/handlers/admin/artifacts.rs:123-149`
- Modify: `conary-server/src/server/handlers/admin/packages.rs:124-148` (same pattern)
- Modify: `conary-server/src/server/routes.rs` (apply DefaultBodyLimit to external admin router)

**Context:** The upload handlers stream request body without a size cap, and the external admin router does not apply `DefaultBodyLimit`.

- [ ] **Step 1: Add size guard to upload loop**

In `artifacts.rs` upload handler, add a running size check:
```rust
const MAX_UPLOAD_SIZE: u64 = 512 * 1024 * 1024; // 512 MB
let mut total_size: u64 = 0;
while let Some(chunk) = stream.next().await {
    let chunk = chunk?;
    total_size += chunk.len() as u64;
    if total_size > MAX_UPLOAD_SIZE {
        return Err(StatusCode::PAYLOAD_TOO_LARGE.into_response());
    }
    file.write_all(&chunk).await?;
}
```

Apply same pattern in `packages.rs`.

- [ ] **Step 2: Apply DefaultBodyLimit to external admin router**

In `routes.rs`, add `.layer(DefaultBodyLimit::max(512 * 1024 * 1024))` to the external admin router.

- [ ] **Step 3: Write test**

Test that uploading > 512MB returns 413.

- [ ] **Step 4: Commit**

```
security: add upload size limits to artifact and package handlers

Added MAX_UPLOAD_SIZE (512MB) guard to upload streaming loops and
applied DefaultBodyLimit to the external admin router. Prevents
authenticated users from exhausting disk via unbounded uploads.
```

---

### Task 7: DB metadata.rs — eliminate SQL injection surface

**Files:**
- Modify: `conary-core/src/db/models/metadata.rs:7-18`

**Context:** `get_metadata()` and `set_metadata()` interpolate a `table: &str` parameter into SQL via `format!`. Replace with an enum.

- [ ] **Step 1: Replace dynamic table name with enum**

```rust
#[derive(Debug, Clone, Copy)]
pub enum MetadataTable {
    Server,
    Client,
}

impl MetadataTable {
    fn as_str(self) -> &'static str {
        match self {
            Self::Server => "server_metadata",
            Self::Client => "client_metadata",
        }
    }
}

pub fn get_metadata(conn: &Connection, table: MetadataTable, key: &str) -> Result<Option<String>> {
    let sql = format!("SELECT value FROM {} WHERE key = ?1", table.as_str());
    // ... rest unchanged
}
```

- [ ] **Step 2: Update all callers**

Find all call sites with `get_metadata(conn, "server_metadata", ...)` and replace with `get_metadata(conn, MetadataTable::Server, ...)`.

Run: `grep -rn 'get_metadata\|set_metadata' conary-core/src/ conary-server/src/`

- [ ] **Step 3: Run tests**

Run: `cargo test -p conary-core` and `cargo test -p conary-server`

- [ ] **Step 4: Commit**

```
security: replace dynamic SQL table name with enum in metadata.rs

get_metadata() and set_metadata() accepted &str table names that
were interpolated into SQL via format!(). Replaced with a
MetadataTable enum that maps to static strings, eliminating the
injection surface entirely.
```

---

### Task 8: Landlock deny enforcement — fail on conflicts in Enforce mode

**Files:**
- Modify: `conary-core/src/capability/enforcement/landlock_enforce.rs:144-152`
- Test: `conary-core/src/capability/enforcement/landlock_enforce.rs` (inline tests)

**Context:** When a deny path conflicts with an allowed parent, the code logs a warning but returns `Ok(())` in Enforce mode. This creates a false sense of security.

- [ ] **Step 1: Return error when deny conflicts detected in Enforce mode**

After `check_deny_conflicts()`, if `deny_conflicts > 0` and mode is `Enforce`:
```rust
if deny_conflicts > 0 && mode == EnforcementMode::Enforce {
    return Err(EnforcementError::DenyConflict {
        count: deny_conflicts,
        message: "Deny paths conflict with allowed parents — landlock cannot enforce denials".into(),
    }.into());
}
```

Add `DenyConflict` variant to `EnforcementError` if it doesn't exist.

- [ ] **Step 2: Add deny_conflicts_count to EnforcementReport**

```rust
pub deny_conflicts_count: usize,
```

- [ ] **Step 3: Write test and run**

Test that Enforce mode with conflicting deny/allow returns an error.

Run: `cargo test -p conary-core landlock -- --nocapture`

- [ ] **Step 4: Commit**

```
security: fail landlock enforcement when deny paths conflict with allows

Landlock cannot enforce deny rules when the denied path falls under
an allowed parent directory. In Enforce mode, this now returns an
error instead of silently continuing. In Warn/Audit mode, the
conflict count is reported in EnforcementReport.
```

---

### Task 9: Seccomp — configurable mode for scriptlets, fail on apply error

**Files:**
- Modify: `conary-core/src/scriptlet/mod.rs:464-473` (pre_exec apply_filter)
- Modify: `conary-core/src/scriptlet/mod.rs:732` (build_scriptlet_seccomp hardcoded Warn)
- Test: `conary-core/src/scriptlet/mod.rs` (inline tests)

**Context:** (1) `build_scriptlet_seccomp()` hardcodes `EnforcementMode::Warn`, so seccomp never blocks syscalls. (2) If `apply_filter()` fails in `pre_exec`, the code warns and returns `Ok(())`, running the scriptlet unsandboxed.

- [ ] **Step 1: Accept EnforcementMode parameter**

Change `build_scriptlet_seccomp()` signature to accept `mode: EnforcementMode` instead of hardcoding `Warn`.

- [ ] **Step 2: Propagate apply_filter failure in Enforce mode**

In the `pre_exec` closure, when `apply_filter` fails:
```rust
if mode == EnforcementMode::Enforce {
    return Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        "seccomp filter application failed — refusing to run scriptlet unsandboxed",
    ));
}
// In Warn mode, log and continue (existing behavior)
```

- [ ] **Step 3: Update callers to pass mode from policy**

Thread the enforcement mode from the capability policy configuration through to the scriptlet executor.

- [ ] **Step 4: Run tests**

Run: `cargo test -p conary-core scriptlet -- --nocapture`

- [ ] **Step 5: Commit**

```
security: make scriptlet seccomp mode configurable, fail on apply error

build_scriptlet_seccomp() now accepts an EnforcementMode parameter
instead of hardcoding Warn. In Enforce mode, seccomp filter
application failure now returns an error instead of silently running
the scriptlet with no syscall restrictions.
```

---

### Task 10: CLI detect_package_format — stop misidentifying zstd/xz as Arch

**Files:**
- Modify: `src/commands/mod.rs:232-239`
- Test: `src/commands/mod.rs` (inline tests)

**Context:** Magic-byte fallback matches ANY zstd/xz file as Arch package. Should require Arch-specific markers.

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_zstd_file_not_detected_as_arch() {
    // A plain .tar.zst file should NOT be detected as Arch
    let temp = tempfile::NamedTempFile::new().unwrap();
    // Write zstd magic bytes
    std::fs::write(temp.path(), &[0x28, 0xB5, 0x2F, 0xFD, 0, 0, 0, 0]).unwrap();
    let result = detect_package_format(temp.path());
    assert!(!matches!(result, Ok(Some(PackageFormat::Arch))),
        "plain zstd file should not be detected as Arch package");
}
```

- [ ] **Step 2: Tighten magic-byte detection**

Remove the zstd/xz magic-byte fallback for Arch detection. Arch packages should only be detected by their `.pkg.tar.{zst,xz}` extension. The magic-byte path should return `None` (unknown format) for bare compressed files.

- [ ] **Step 3: Run tests**

Run: `cargo test detect_package -- --nocapture`

- [ ] **Step 4: Commit**

```
fix: stop misidentifying zstd/xz archives as Arch packages

The magic-byte fallback matched any zstd or xz compressed file as
an Arch package. Arch packages are now detected only by their
.pkg.tar.{zst,xz} extension. Plain compressed files return None.
```

---

## Phase 2: Data Integrity & Correctness (P1)

---

### Task 11: RepositoryPackage::insert() — add missing columns

**Files:**
- Modify: `conary-core/src/db/models/repository.rs:311-334`
- Test: `conary-core/src/db/models/repository.rs` (inline tests)

**Context:** `insert()` inserts 15 columns, missing `distro` and `version_scheme`. `BATCH_INSERT_SQL` correctly inserts 17.

- [ ] **Step 1: Write failing test**

Use the existing `testing::create_test_db()` helper from `conary-core/src/db/mod.rs`. Create a test repo inline since `create_test_repo` is not in the shared testing module. Note: `RepositoryPackage::new` size param is `i64`.

```rust
#[test]
fn test_insert_preserves_distro_and_version_scheme() {
    let conn = testing::create_test_db();
    // Create a repo first (required FK)
    let mut repo = Repository::new("test-repo".into(), "http://example.com".into());
    repo.insert(&conn).unwrap();
    let mut pkg = RepositoryPackage::new(
        repo.id.unwrap(), "test-pkg".into(), "1.0".into(),
        "abc123".into(), 100_i64, "http://example.com/test.rpm".into(),
    );
    pkg.distro = Some("fedora43".into());
    pkg.version_scheme = Some("rpm".into());
    pkg.insert(&conn).unwrap();
    let found = RepositoryPackage::find_by_name(&conn, "test-pkg").unwrap();
    assert_eq!(found[0].distro.as_deref(), Some("fedora43"));
    assert_eq!(found[0].version_scheme.as_deref(), Some("rpm"));
}
```

- [ ] **Step 2: Fix insert() to include all 17 columns**

Update the INSERT statement in `insert()` to match `BATCH_INSERT_SQL`:
```sql
INSERT INTO repository_packages
(repository_id, name, version, architecture, description, checksum, size,
 download_url, dependencies, metadata, is_security_update, severity, cve_ids,
 advisory_id, advisory_url, distro, version_scheme)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
```

Add `&self.distro` and `&self.version_scheme` to the params.

- [ ] **Step 3: Run tests, commit**

Run: `cargo test -p conary-core repository -- --nocapture`

```
fix: add missing distro and version_scheme to RepositoryPackage::insert()

insert() only inserted 15 columns while batch_insert() correctly
inserted all 17. The distro and version_scheme fields were silently
dropped, causing NULL values for scheme-aware version comparison.
```

---

### Task 12: TroveType::Redirect — remove unused variant

**Files:**
- Modify: `conary-core/src/db/models/trove.rs:13-18`

**Context:** `TroveType::Redirect` violates the CHECK constraint on the `troves` table. The variant is unused — the `redirects` table (migration v28) handles redirects separately.

- [ ] **Step 1: Verify Redirect is unused in code and DB**

Run: `grep -rn 'TroveType::Redirect' conary-core/src/ src/`
Also check if any database rows contain the string "redirect" in the type column. Note: `TroveType` derives `strum::Display` and `strum::EnumString`, so removing the variant will break `FromStr` for the string "redirect".

- [ ] **Step 2: Remove the variant if unused, handle FromStr gracefully**

If no code references `TroveType::Redirect` and no DB rows contain it:
- Remove the variant from the enum
- Verify `FromStr` ("redirect") now returns an error (strum will do this automatically)
- If DB rows COULD contain "redirect" from past usage, add a migration to convert them first

If `Redirect` IS referenced somewhere, add a migration (v57) to recreate the troves table with the updated CHECK constraint including `'redirect'`.

- [ ] **Step 3: Run tests, commit**

```
fix: remove unused TroveType::Redirect that violates CHECK constraint

The Redirect variant would fail the troves table CHECK constraint
which only allows 'package', 'component', 'collection'. The
redirects table (v28 migration) handles redirects separately.
Verified no code or DB rows reference this variant.
```

---

### Task 13: Derived package override path validation

**Files:**
- Modify: `conary-core/src/derived/builder.rs:538-556`
- Test: `conary-core/src/derived/builder.rs` (inline tests)

**Context:** `validate_override_target` rejects absolute paths, but `DerivedFile.path` stores absolute paths. The documented API shows absolute paths in override specs.

- [ ] **Step 1: Fix validation to accept absolute paths**

Change `validate_override_target` to accept absolute paths and instead check for `..` components and null bytes:
```rust
fn validate_override_target(path: &str) -> Result<()> {
    if path.contains('\0') {
        return Err(anyhow!("Override target contains null byte"));
    }
    if path.split('/').any(|c| c == "..") {
        return Err(anyhow!("Override target contains path traversal: {}", path));
    }
    Ok(())
}
```

- [ ] **Step 2: Add test that calls build() with overrides**

The existing test only constructs `DerivedSpec` without calling `build()`. Add a test that exercises the full pipeline with absolute-path overrides.

- [ ] **Step 3: Run tests, commit**

```
fix: allow absolute paths in derived package overrides

validate_override_target rejected paths starting with /, but
DerivedFile.path stores absolute paths and the documented API
uses them. Now accepts absolute paths while still rejecting
path traversal (.. components) and null bytes.
```

---

### Task 14: CCS symlink hash inconsistency

**Files:**
- Modify: `conary-core/src/ccs/builder.rs:349-353` (symlink content format)
- Modify: `conary-core/src/ccs/package.rs:314-318, 537-542`
- Test: inline tests

**Context:** Builder hashes `"symlink:{target}"`, CasStore hashes raw target, extractor returns raw target bytes. Unify on one convention.

- [ ] **Step 1: Choose convention — raw target path**

The CAS store and extractor use raw target path. Change the builder to match:
```rust
// In builder.rs collect_file, for symlinks:
let symlink_content = target_str.as_bytes().to_vec();
let hash = self.cas.compute_hash(&symlink_content);
```

Remove the `format!("symlink:{}", target_str)` prefix.

- [ ] **Step 2: Write test verifying round-trip consistency**

Build a package with a symlink, extract it, verify the symlink hash matches.

- [ ] **Step 3: Run tests, commit**

```
fix: unify symlink hash convention across CCS builder and extractor

Builder used "symlink:{target}" prefix when hashing symlinks,
but CasStore and the extractor hashed the raw target path.
Unified on raw target path to ensure manifest hashes match
what verification and extraction compute.
```

---

### Task 15: Resolver — find_repo_package_by_id O(1) lookup

**Files:**
- Modify: `conary-core/src/resolver/provider.rs:948-955`
- Modify: `conary-core/src/db/models/repository.rs` (add find_by_id method)
- Test: inline tests

**Context:** `find_repo_package_by_id` calls `list_all(conn)` then linear scans. Called per-dependency during SAT resolution.

- [ ] **Step 1: Add RepositoryPackage::find_by_id**

In `repository.rs`:
```rust
pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
    let sql = format!("SELECT {} FROM repository_packages WHERE id = ?1", Self::COLUMNS);
    let mut stmt = conn.prepare(&sql)?;
    let pkg = stmt.query_row([id], Self::from_row).optional()?;
    Ok(pkg)
}
```

- [ ] **Step 2: Replace list_all + linear scan**

In `provider.rs:948-955`:
```rust
fn find_repo_package_by_id(&self, id: i64) -> Option<RepositoryPackage> {
    RepositoryPackage::find_by_id(self.conn, id).ok().flatten()
}
```

- [ ] **Step 3: Run tests, commit**

```
perf: replace O(N) full-table scan with O(1) lookup in resolver

find_repo_package_by_id loaded all RepositoryPackages to find one
by ID. Replaced with a direct SQL query. This function is called
per-dependency during SAT resolution, so the improvement is
O(D*N) -> O(D) where D=dependencies and N=total repo packages.
```

---

### Task 16: Resolver — escape LIKE wildcards in capability search

**Files:**
- Modify: `conary-core/src/resolver/provider.rs:436-441`
- Test: inline tests

**Context:** `format!("%{capability}%")` passes unescaped SQL LIKE wildcards.

- [ ] **Step 1: Add LIKE escaping**

```rust
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

// In find_repo_providers fallback:
let pattern = format!("%{}%", escape_like(&capability));
// Add ESCAPE '\' to the SQL:
"... LIKE ?1 ESCAPE '\\' ..."
```

- [ ] **Step 2: Write test, run, commit**

```
fix: escape LIKE wildcards in resolver capability search

Capability names containing % or _ characters would cause
false matches in the LIKE-based fallback provider search.
```

---

### Task 17: Trigger execution order — preserve topological sort

**Files:**
- Modify: `conary-core/src/db/models/trigger.rs:562`
- Test: inline tests

**Context:** `get_execution_order` re-sorts by priority after topological sort, destroying dependency ordering.

- [ ] **Step 1: Replace full re-sort with level-aware sort**

Option A (simplest): Remove the secondary sort entirely — topological sort already respects dependencies.

Option B (if priority matters): Group triggers by topological level, sort within each level by priority, then flatten.

- [ ] **Step 2: Write test with conflicting priority/dependency**

Create triggers where A depends on B, but A has higher priority. Verify B still executes first.

- [ ] **Step 3: Run tests, commit**

```
fix: preserve topological ordering in trigger execution

The secondary sort by priority destroyed the topological ordering.
A trigger with high priority but a dependency on a lower-priority
trigger would incorrectly execute first.
```

---

### Task 18: Resolve_install — handle pre-existing cycles

**Files:**
- Modify: `conary-core/src/resolver/engine.rs:99-107`
- Test: inline tests

**Context:** `resolve_install` calls `topological_sort()` on the full graph. Pre-existing cycles (glibc<->glibc-common) cause all installs to fail.

- [ ] **Step 1: Use detect_cycle_involving instead**

Replace the full-graph cycle check with a targeted check for cycles involving only the newly added package:
```rust
// Instead of:
// self.graph.topological_sort()?;
// Use:
if let Some(cycle) = self.graph.detect_cycle_involving(&package_name) {
    return Err(ResolverError::CyclicDependency(cycle));
}
```

- [ ] **Step 2: Write test with pre-existing cycle**

Install a package on a system where glibc and glibc-common have a mutual dependency. Verify the install succeeds.

- [ ] **Step 3: Run tests, commit**

```
fix: allow installs on systems with pre-existing dependency cycles

resolve_install ran topological_sort on the full graph, failing
when pre-existing cycles existed (e.g., glibc <-> glibc-common).
Now only checks for cycles involving the newly installed package.
```

---

### Task 19: build_script_hash — include all build sections

**Files:**
- Modify: `conary-core/src/derivation/recipe_hash.rs:58-76`
- Test: inline tests

**Context:** `build_script_hash` only hashes `configure`, `make`, `install`, `check`. Missing `setup`, `post_install`, `environment`, `workdir`.

- [ ] **Step 1: Add missing fields**

```rust
let sections: [(&str, &Option<String>); 6] = [
    ("setup", &recipe.build.setup),
    ("configure", &recipe.build.configure),
    ("make", &recipe.build.make),
    ("install", &recipe.build.install),
    ("post_install", &recipe.build.post_install),
    ("check", &recipe.build.check),
];
// After sections loop, add environment and workdir:
let mut env_keys: Vec<_> = recipe.build.environment.keys().collect();
env_keys.sort();
for key in env_keys {
    let val = &recipe.build.environment[key];
    hasher.update(format!("env:{key}={val}\n").as_bytes());
}
if let Some(ref wd) = recipe.build.workdir {
    hasher.update(format!("workdir:{wd}\n").as_bytes());
}
```

- [ ] **Step 2: Write test verifying different setup/environment produce different hashes**

- [ ] **Step 3: Run tests, commit**

```
fix: include setup, post_install, environment, workdir in build_script_hash

Changes to these fields would not change the derivation ID,
breaking content-addressability. Two builds with different
environment variables would produce the same derivation ID
but potentially different outputs.
```

---

### Task 20: EROFS atomic writes

**Files:**
- Modify: `conary-core/src/generation/builder.rs:260-266`

**Context:** `build_erofs_image` uses `std::fs::write()` which is not atomic. Partial writes could survive recovery validation.

- [ ] **Step 1: Replace with temp+fsync+rename**

```rust
let temp_path = image_path.with_extension("erofs.tmp");
let mut file = std::fs::File::create(&temp_path)?;
file.write_all(&erofs_data)?;
file.sync_all()?;
std::fs::rename(&temp_path, &image_path)?;
// fsync parent dir for durability
if let Some(parent) = image_path.parent() {
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
}
```

- [ ] **Step 2: Run tests, commit**

```
fix: use atomic write for EROFS image generation

std::fs::write() is not atomic — a crash during write could leave
a partially written root.erofs that passes the recovery magic
check but fails at mount time. Now uses temp+fsync+rename.
```

---

### Task 21: build_generation_from_db — create state after image

**Files:**
- Modify: `conary-core/src/generation/builder.rs:316-386`

**Context:** DB state snapshot is created before EROFS image build. If image build fails, the DB has an orphaned state with no image.

- [ ] **Step 1: Reorder — build image first, then create state**

Move the state snapshot creation (step 2 in the function) to after successful EROFS image build (step 5).

- [ ] **Step 2: Run tests, commit**

```
fix: create generation state snapshot after successful image build

State was created before image build, leaving orphaned DB records
if image building failed. Now the state is only recorded after
the EROFS image is successfully built and written atomically.
```

---

## Phase 3: Hardening & Defense-in-Depth (P2)

---

### Task 22: Decompression bomb — detect truncation

**Files:**
- Modify: `conary-core/src/compression/mod.rs:164-175`
- Test: inline tests

- [ ] **Step 1: Read one extra byte to detect truncation**

Use `take()` on a mutable reference so the original decoder is not consumed, then read `MAX + 1` bytes:

```rust
let mut buf = Vec::new();
let mut limited = (&mut decoder).take(MAX_DECOMPRESS_SIZE + 1);
limited.read_to_end(&mut buf)?;
if buf.len() as u64 > MAX_DECOMPRESS_SIZE {
    return Err(anyhow!("decompressed data exceeds {} byte limit", MAX_DECOMPRESS_SIZE));
}
```

- [ ] **Step 2: Run tests, commit**

```
fix: detect and reject truncated decompression output

The decompression guard silently truncated data at
MAX_DECOMPRESS_SIZE. Now reads one extra byte to detect
truncation and returns an error.
```

---

### Task 23: CCS parse/inspect — add size limits for metadata entries

**Files:**
- Modify: `conary-core/src/ccs/package.rs:405-438`
- Modify: `conary-core/src/ccs/inspector.rs:41-74`

- [ ] **Step 1: Add MAX_MANIFEST_SIZE and MAX_COMPONENT_SIZE constants**

```rust
const MAX_MANIFEST_SIZE: u64 = 16 * 1024 * 1024; // 16 MB
const MAX_COMPONENT_SIZE: u64 = 64 * 1024 * 1024; // 64 MB
```

- [ ] **Step 2: Apply limits before read_to_string/read_to_end**

Check `entry.header().size()?` before reading metadata entries.

- [ ] **Step 3: Run tests, commit**

```
security: add size limits to CCS metadata parsing

MANIFEST and component JSON entries were read without size bounds
during parse and inspect. A malicious .ccs could include an
extremely large metadata entry causing OOM.
```

---

### Task 24: /etc merge — don't follow symlinks

**Files:**
- Modify: `conary-core/src/generation/etc_merge.rs:261-277`

- [ ] **Step 1: Use symlink_metadata instead of is_dir/is_file**

Replace `path.is_dir()` with `path.symlink_metadata()?.is_dir()` and skip symlinks.

- [ ] **Step 2: Run tests, commit**

```
security: prevent /etc merge from following symlinks outside overlay

scan_dir_recursive used is_dir()/is_file() which follow symlinks.
A symlink pointing outside the overlay could cause the merge logic
to read and compare files outside its intended scope.
```

---

### Task 25: GC SQL variable limit — batch for large state lists

**Files:**
- Modify: `conary-core/src/generation/gc.rs:48-52`

- [ ] **Step 1: Batch the IN clause or use json_each**

```rust
// Use json_each for unbounded lists:
let json_array = serde_json::to_string(surviving_state_ids)?;
let sql = "SELECT DISTINCT f.sha256_hash FROM files f \
    JOIN troves t ON f.trove_id = t.id \
    JOIN state_members sm ON sm.trove_name = t.name AND sm.trove_version = t.version \
    WHERE sm.state_id IN (SELECT value FROM json_each(?1))";
conn.prepare(&sql)?.query_map([&json_array], ...)?;
```

- [ ] **Step 2: Run tests, commit**

```
fix: use json_each for GC state ID queries to avoid SQLite variable limit

The IN clause used one parameter per state ID, which could exceed
SQLITE_MAX_VARIABLE_NUMBER for systems with many generations.
```

---

### Task 26: CAS iterator — fix temp file skip pattern

**Files:**
- Modify: `conary-core/src/filesystem/cas.rs:328-330` (CasIterator)

- [ ] **Step 1: Align skip pattern with temp file naming convention**

Temp files use format `{hash}.tmp.{pid}.{counter}`. The iterator should check `.contains(".tmp.")`:

```rust
if name_str.starts_with('.') || name_str.contains(".tmp.") || name_str.ends_with(".tmp") {
    continue;
}
```

- [ ] **Step 2: Run tests, commit**

```
fix: align CAS iterator skip pattern with temp file naming convention

CasIterator checked ends_with(".tmp") but temp files use the format
hash.tmp.pid.counter (ending with a digit). Orphaned temps could
appear as valid CAS objects, affecting GC correctness.
```

---

### Task 27: parse_octal_mode — return error instead of silent fallback

**Files:**
- Modify: `conary-core/src/ccs/manifest.rs:790-796`

- [ ] **Step 1: Return Result instead of defaulting to 0o755**

```rust
fn parse_octal_mode(s: &str) -> Result<u32> {
    u32::from_str_radix(s.trim_start_matches("0o").trim_start_matches('0'), 8)
        .map_err(|_| anyhow!("invalid octal mode: {}", s))
}
```

Update callers to handle the Result.

- [ ] **Step 2: Run tests, commit**

```
fix: return error for invalid octal mode instead of silent 0o755 fallback

A typo in mode strings would silently become 0o755, potentially
making restricted directories world-readable.
```

---

### Task 28: sanitize_path — add null byte check

**Files:**
- Modify: `conary-core/src/filesystem/path.rs:43-80`

- [ ] **Step 1: Add null byte check at the start of sanitize_path**

```rust
if path_str.contains('\0') {
    return Err(Error::PathTraversal("path contains null byte".to_string()));
}
```

- [ ] **Step 2: Run tests, commit**

```
security: reject null bytes in sanitize_path

Null bytes could truncate paths at the C API boundary. Added
defense-in-depth check early in sanitize_path.
```

---

### Task 29: Sandbox fallback — refuse when running as root

**Files:**
- Modify: `conary-core/src/container/mod.rs:426-444`

- [ ] **Step 1: Check for root and refuse fallback**

```rust
if nix::unistd::getuid().is_root() {
    return Err(anyhow!(
        "namespace isolation unavailable while running as root — \
         refusing to execute scriptlet without sandboxing"
    ));
}
// Existing fallback for non-root (resource limits only)
```

- [ ] **Step 2: Run tests, commit**

```
security: refuse sandbox fallback to execute_limited when running as root

When running as root, namespace isolation should always be available.
Falling back to resource-limits-only execution would run untrusted
scriptlets with full root access on the host.
```

---

### Task 30: Bootstrap — remove hardcoded x86_64

**Files:**
- Modify: `conary-core/src/bootstrap/cross_tools.rs:28, 141-151, 312-316`
- Modify: `conary-core/src/bootstrap/temp_tools.rs:160-171`
- Test: inline tests

- [ ] **Step 1: Replace LFS_TGT const with config-derived value**

```rust
fn lfs_tgt(config: &BootstrapConfig) -> String {
    format!("{}-conary-linux-gnu", config.target_arch.triple_prefix())
}
```

Remove the `const LFS_TGT`.

- [ ] **Step 2: Fix verify() to use config arch**

Map `TargetArch` to expected `file(1)` output strings.

- [ ] **Step 3: Replace unsafe set_var with explicit env passing**

Instead of `set_var("LFS_TGT", ...)`, pass the environment via `.envs()` on `Command`:
```rust
let mut env = toolchain.env();
env.insert("LFS_TGT".into(), lfs_tgt(&self.config));
command.envs(&env);
```

Apply same change in `temp_tools.rs`.

- [ ] **Step 4: Run tests, commit**

```
fix: derive bootstrap target from config instead of hardcoding x86_64

LFS_TGT was a const set to x86_64-conary-linux-gnu, ignoring the
configured target_arch. Verification also hardcoded x86-64 checks.
Also replaced unsafe set_var with explicit env passing to child
processes, eliminating UB risk in multi-threaded contexts.
```

---

### Task 31: VersionConstraint::Exact — use compare() for consistency

**Files:**
- Modify: `conary-core/src/version/mod.rs:281-287`
- Test: inline tests

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_exact_constraint_handles_leading_zeros() {
    let constraint = VersionConstraint::Exact(RpmVersion::parse("1.001-1"));
    let version = RpmVersion::parse("1.1-1");
    assert!(constraint.satisfies(&version),
        "Exact should use rpmvercmp semantics where 1.001 == 1.1");
}
```

- [ ] **Step 2: Replace direct comparison with compare()**

```rust
VersionConstraint::Exact(v) => version.compare(v).is_eq(),
```

- [ ] **Step 3: Run tests, commit**

```
fix: use rpmvercmp semantics for VersionConstraint::Exact

Exact used direct string equality while all other constraint
operators used compare(). This meant "1.001" != "1.1" under Exact
but == under >=/<=/etc, creating inconsistent version matching.
```

---

## Phase 4: Server & Infrastructure (P2)

---

### Task 32: Deduplicate artifact path sanitization

**Files:**
- Create: `conary-server/src/server/artifact_paths.rs`
- Modify: `conary-server/src/server/handlers/admin/artifacts.rs`
- Modify: `conary-server/src/server/handlers/artifacts.rs`
- Modify: `conary-server/src/server/mod.rs` (add module)

- [ ] **Step 1: Extract shared module**

Create `conary-server/src/server/artifact_paths.rs` with the required file header (`// conary-server/src/server/artifact_paths.rs`). Move `sanitize_relative_path`, `ArtifactRoot`, `storage_root`, `artifact_root` into it.

- [ ] **Step 2: Update both handlers to import from shared module**

- [ ] **Step 3: Run tests, commit**

```
refactor: deduplicate artifact path sanitization into shared module

sanitize_relative_path and ArtifactRoot were copy-pasted between
admin upload and public serving handlers. Divergence could
introduce a path traversal vulnerability.
```

---

### Task 33: Fix block_on inside spawn_blocking

**Files:**
- Modify: `conary-server/src/server/admin_service.rs:509-516`
- Modify: `conary-server/src/server/conversion.rs:253,389,409`

- [ ] **Step 1: Restructure to be fully async**

Move async operations (sync_repository, download_package, fetch_gpg_key) outside `spawn_blocking`. Use `spawn_blocking` only for the synchronous SQLite portions.

Pattern:
```rust
// Before (broken):
spawn_blocking(|| { handle.block_on(async_fn()) }).await?

// After (correct):
let async_result = async_fn().await?;
spawn_blocking(move || { sync_db_operation(async_result) }).await?
```

- [ ] **Step 2: Run tests, commit**

```
fix: restructure block_on inside spawn_blocking to prevent deadlock

Using Handle::block_on() inside spawn_blocking can deadlock if the
blocking thread pool is saturated. Restructured to run async
operations outside spawn_blocking, using it only for sync SQLite.
```

---

### Task 34: SSE connection limit

**Files:**
- Modify: `conary-server/src/server/handlers/admin/events.rs:18-74`

- [ ] **Step 1: Add semaphore-based connection limit**

```rust
static SSE_SEMAPHORE: LazyLock<tokio::sync::Semaphore> =
    LazyLock::new(|| tokio::sync::Semaphore::new(100)); // max 100 concurrent SSE connections

pub async fn sse_events(...) -> impl IntoResponse {
    let permit = match SSE_SEMAPHORE.try_acquire() {
        Ok(p) => p,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    // ... existing SSE logic, hold permit for duration
}
```

- [ ] **Step 2: Run tests, commit**

```
security: add concurrent connection limit to SSE event stream

An attacker with a valid token could open thousands of SSE
connections, exhausting server resources. Now limited to 100
concurrent connections.
```

---

### Task 35: list_test_runs — push filters into SQL

**Files:**
- Modify: `conary-server/src/server/admin_service.rs:777-788`

- [ ] **Step 1: Build SQL WHERE clause from filters**

```rust
let mut conditions = Vec::new();
let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
if let Some(ref suite) = filter.suite {
    conditions.push(format!("suite = ?{}", params.len() + 1));
    params.push(Box::new(suite.clone()));
}
// ... same for distro, status
```

- [ ] **Step 2: Run tests, commit**

```
fix: push test run filters into SQL instead of post-query filtering

Filters were applied after fetching `limit` rows, causing fewer
results than expected when most rows didn't match.
```

---

### Task 36: TOUCH_CACHE — use std::sync::Mutex

**Files:**
- Modify: `conary-server/src/server/auth.rs:30-31`

- [ ] **Step 1: Replace tokio::sync::Mutex with std::sync::Mutex**

The critical section never crosses an await point.

- [ ] **Step 2: Run tests, commit**

```
refactor: use std::sync::Mutex for TOUCH_CACHE

The critical section (HashMap lookup/insert) is synchronous and
never crosses an await point. std::sync::Mutex is more appropriate
and marginally faster.
```

---

### Task 37: self_update get_latest — cache hash computation

**Files:**
- Modify: `conary-server/src/server/handlers/self_update.rs:162`

- [ ] **Step 1: Compute sha256/size when scanning versions, cache in VERSIONS_CACHE**

Instead of reading the full CCS file on every `/v1/ccs/conary/latest` request, compute the hash during `scan_versions` and cache it alongside the version list.

- [ ] **Step 2: Run tests, commit**

```
perf: cache CCS package hash in version scan instead of per-request

get_latest read the entire CCS file on every request to compute
SHA-256. Now computed during version scanning and cached.
```

---

### Task 38: audit.rs — add missing route mappings

**Files:**
- Modify: `conary-server/src/server/audit.rs:25-73`

- [ ] **Step 1: Add mappings for test-data, packages, artifacts**

```rust
"test-data" | "test-runs" | "test-health" => "test",
"test-fixtures" | "test-artifacts" => "artifact",
"packages" => "package",
```

- [ ] **Step 2: Run tests, commit**

```
fix: add missing route mappings in audit derive_action

Several admin routes (test-runs, test-health, packages) were
falling through to "unknown" in audit log entries.
```

---

### Task 39: Consolidate rate limiting (optional)

**Files:**
- Modify: `conary-server/src/server/security.rs`
- Modify: `conary-server/src/server/routes.rs`

**Note:** This is a larger refactor. Consider deferring if time-constrained.

- [ ] **Step 1: Migrate public rate limiting from hand-rolled to governor-based**

Replace the `RwLock<HashMap>` token-bucket implementation with the governor-based approach already used for admin routes. This eliminates write-lock contention on every public request.

- [ ] **Step 2: Run tests, commit**

```
refactor: consolidate on governor-based rate limiting

Replaced the hand-rolled RwLock<HashMap> token-bucket rate limiter
for public routes with the governor-based approach already used
for admin routes. Eliminates write-lock contention.
```

---

## Phase 5: Trust, Canonical, Automation (P2-P3)

---

### Task 40: verify_metadata_hash — strict mode

**Files:**
- Modify: `conary-core/src/trust/verify.rs:162-174`

- [ ] **Step 1: Add require_hash parameter**

```rust
pub fn verify_metadata_hash(
    meta_ref: &MetaFileRef,
    actual_hash: &str,
    require_hash: bool,
) -> Result<()> {
    if let Some(ref hashes) = meta_ref.hashes {
        // ... existing verification
    } else if require_hash {
        return Err(TufError::MissingHash.into());
    }
    Ok(())
}
```

- [ ] **Step 2: Use require_hash=true for snapshot->targets and timestamp->snapshot**

- [ ] **Step 3: Run tests, commit**

```
security: add strict mode to verify_metadata_hash

Missing hashes in TUF metadata references were silently accepted.
Now callers can require hashes, preventing downgrade attacks where
a compromised snapshot omits hashes.
```

---

### Task 41: Policy file TOCTOU — remove exists() check

**Files:**
- Modify: `conary-core/src/capability/policy.rs:120-126`

- [ ] **Step 1: Replace exists()+read with try-read**

```rust
match std::fs::read_to_string(&candidate_path) {
    Ok(content) => return parse_policy(&content),
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
    Err(e) => return Err(e.into()),
}
```

- [ ] **Step 2: Run tests, commit**

```
security: eliminate TOCTOU in capability policy loading

Replaced exists() check followed by read_to_string() with a
single read attempt, handling NotFound in the error path.
```

---

### Task 42: Hardcoded User-Agent version strings

**Files:**
- Modify: `conary-core/src/canonical/repology.rs:175`
- Modify: `conary-core/src/canonical/client.rs:33`

- [ ] **Step 1: Use CARGO_PKG_VERSION**

```rust
const USER_AGENT: &str = concat!("conary/", env!("CARGO_PKG_VERSION"));
```

Replace hardcoded `"conary/0.6.0"` in both files.

- [ ] **Step 2: Run tests, commit**

```
fix: derive User-Agent version from Cargo.toml instead of hardcoding

"conary/0.6.0" was hardcoded and would drift on next release.
Now uses env!("CARGO_PKG_VERSION") at compile time.
```

---

### Task 43: Repology — add Debian mappings

**Files:**
- Modify: `conary-core/src/canonical/repology.rs:137-160`

- [ ] **Step 1: Add Debian entries to repo_to_distro and distro_to_repo**

```rust
"debian_12" => Some("debian"),
"debian_13" => Some("debian"),
"debian_unstable" => Some("debian"),
```

- [ ] **Step 2: Run tests, commit**

```
feat: add Debian repository mappings to Repology canonical sync

Repology sync silently skipped all Debian package data due to
missing repo-to-distro mappings.
```

---

### Task 44: Automation daemon shutdown latency

**Files:**
- Modify: `conary-core/src/automation/scheduler.rs:220-239`

- [ ] **Step 1: Replace thread::sleep with Condvar-based wait**

```rust
use std::sync::{Condvar, Mutex};

pub struct AutomationDaemon {
    stop: Arc<AtomicBool>,
    notify: Arc<(Mutex<()>, Condvar)>,
}

impl AutomationDaemon {
    pub fn run(&self) {
        loop {
            if self.stop.load(Ordering::Relaxed) { break; }
            // ... check logic ...
            let (lock, cvar) = &*self.notify;
            let guard = lock.lock().unwrap();
            let _ = cvar.wait_timeout(guard, Duration::from_secs(60)).unwrap();
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.notify.1.notify_one(); // Wake immediately
    }
}
```

- [ ] **Step 2: Run tests, commit**

```
fix: use Condvar for automation daemon shutdown instead of thread::sleep

thread::sleep blocked for up to 60 seconds before observing the
stop signal. Now uses Condvar::wait_timeout so stop() wakes the
daemon immediately.
```

---

### Task 45: Pipeline StageStarted before cutoff check

**Files:**
- Modify: `conary-core/src/derivation/pipeline.rs:352-365`

- [ ] **Step 1: Move cutoff check before StageStarted event**

- [ ] **Step 2: Run tests, commit**

```
fix: check --up-to cutoff before emitting StageStarted event

A stage beyond the cutoff would emit StageStarted but never
StageCompleted, producing confusing output.
```

---

### Task 46: validate_recipe — detect placeholder checksums

**Files:**
- Modify: `conary-core/src/recipe/parser.rs:40-47`

- [ ] **Step 1: Add placeholder detection**

```rust
if checksum.contains("VERIFY_BEFORE_BUILD") || checksum.contains("FIXME") {
    return Err(anyhow!("placeholder checksum detected: {}", checksum));
}
```

- [ ] **Step 2: Run tests, commit**

```
fix: detect placeholder checksums in validate_recipe

sha256:VERIFY_BEFORE_BUILD_abc would pass validation (starts with
sha256:) but fail at build time. Catch placeholders early.
```

---

## Phase 6: Cleanup & Minor Fixes (P3)

---

### Task 47: CLI cleanup batch

**Files:**
- Modify: `src/commands/remove.rs:215-216, 288-289` (dead install_root, misleading counts)
- Modify: `src/commands/restore.rs:74-86` (dead force branch)
- Modify: `src/commands/export.rs:374, 434` (hardcoded amd64)
- Modify: `src/commands/install/mod.rs:168, 178` (classify_dep_type, dead_code report_provides_check)

- [ ] **Step 1: Remove dead install_root binding in remove.rs**
- [ ] **Step 2: Remove failed_count and simplify output in remove.rs**
- [ ] **Step 3: Collapse redundant force branch in restore.rs**
- [ ] **Step 4: Replace hardcoded amd64 with runtime arch in export.rs**

```rust
fn oci_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "riscv64" => "riscv64",
        other => other,
    }
}
```

- [ ] **Step 5: Remove or integrate report_provides_check**
- [ ] **Step 6: Tighten classify_dep_type parenthesis check**
- [ ] **Step 7: Run tests, commit**

```
refactor: CLI cleanup — remove dead code, fix hardcoded arch

Removed dead install_root binding, vestigial failed_count,
redundant force branch in restore. Replaced hardcoded "amd64"
in OCI export with runtime architecture detection. Removed
dead report_provides_check function.
```

---

### Task 48: DB & core cleanup batch

**Files:**
- Modify: `conary-core/src/packages/archive_utils.rs:47` (u64->i64 cast)
- Modify: `conary-core/src/delta/metrics.rs:48-53` (negative savings_percentage)
- Modify: `conary-core/src/filesystem/vfs/mod.rs:427` (remove compact() reference)
- Modify: `conary-core/src/automation/mod.rs:332` (parse_duration empty string)
- Modify: `conary-core/src/automation/check.rs:231` (document write side-effect)

- [ ] **Step 1: Fix u64->i64 in archive_utils**

```rust
i64::try_from(meta.len()).unwrap_or(i64::MAX)
```

- [ ] **Step 2: Clamp savings_percentage to 0.0 minimum**
- [ ] **Step 3: Remove dead compact() reference in VFS**
- [ ] **Step 4: Return error for empty string in parse_duration**
- [ ] **Step 5: Add doc comment about write side-effect in check.rs**
- [ ] **Step 6: Run tests, commit**

```
fix: minor correctness fixes in core types — casts, bounds, dead references

Fixed silent u64->i64 truncation in archive_utils, clamped negative
savings_percentage, removed dead compact() reference, improved
parse_duration empty string handling.
```

---

### Task 49: Package parser hardening batch

**Files:**
- Modify: `conary-core/src/packages/arch.rs:176-192` (brace extraction)
- Modify: `conary-core/src/packages/deb.rs:82` (control parser robustness)
- Modify: `conary-core/src/packages/dpkg_query.rs:336-354` (provides parsing)
- Modify: `conary-core/src/packages/rpm.rs:301` (extract memory bound)

- [ ] **Step 1: Fix Arch extract_function to skip braces in strings/comments**

Handle `#` comments (skip to EOL) and single/double quoted strings.

- [ ] **Step 2: Fix DEB parse_control — validate field name has no whitespace**

```rust
if let Some((key, value)) = line.split_once(':') {
    if key.contains(char::is_whitespace) {
        // Continuation of previous field, not a new field
        current_value.push_str(line);
        continue;
    }
    // ... existing logic
}
```

- [ ] **Step 3: Fix dpkg_query provides parsing — use line position**
- [ ] **Step 4: Add file size check before std::fs::read in RPM extract_file_contents**

```rust
const MAX_RPM_SIZE: u64 = 4 * 1024 * 1024 * 1024; // 4 GB
let meta = std::fs::metadata(self.meta.package_path())?;
if meta.len() > MAX_RPM_SIZE {
    return Err(anyhow!("RPM file too large: {} bytes", meta.len()));
}
```

- [ ] **Step 5: Run tests, commit**

```
fix: harden package parsers against edge cases

Arch: skip braces inside strings/comments in extract_function.
DEB: validate control field names don't contain whitespace.
dpkg: use line position instead of comma detection for provides.
RPM: add file size check before reading entire file into memory.
```

---

### Task 50: Server minor fixes batch

**Files:**
- Modify: `conary-server/src/server/handlers/mod.rs:139-150` (validate_name charset)
- Modify: `conary-server/src/server/handlers/self_update.rs:74-118` (scan_versions sync I/O)
- Modify: `conary-core/src/model/parser.rs:319-326` (RollbackTrigger command tokenization)

- [ ] **Step 1: Tighten validate_name to ASCII-safe charset**
- [ ] **Step 2: Wrap scan_versions in spawn_blocking**
- [ ] **Step 3: Change RollbackTrigger.command to Vec<String>**

Or add a `tokenized_command()` method using `shlex::split`.

- [ ] **Step 4: Run tests, commit**

```
fix: server minor hardening — name validation, sync I/O, command tokenization

Tightened validate_name to ASCII-safe charset. Wrapped sync
scan_versions in spawn_blocking. Added tokenized_command() helper
to RollbackTrigger to prevent shell injection.
```

---

### Task 51: Test infrastructure cleanup

**Files:**
- Modify: `conary-test/src/container/backend.rs:25,55` (memory_limit type)
- Modify: `conary-test/src/engine/suite.rs:93-94` (rename failed_ids)
- Modify: `conary-test/src/server/remi_client.rs:183-195` (URL encoding)
- Modify: `conary-test/src/server/remi_client.rs:330` (unsafe env var)
- Modify: `conary-test/src/server/service.rs:376-448` (dedup initialization)

- [ ] **Step 1: Unify memory_limit to i64 everywhere** (matches bollard API)
- [ ] **Step 2: Rename failed_ids to unsuccessful_ids, clarify semantics**
- [ ] **Step 3: Use urlencoding::encode() for query parameters**
- [ ] **Step 4: Replace unsafe env::remove_var with #[serial_test::serial]**
- [ ] **Step 5: Extract shared container_initialization function**
- [ ] **Step 6: Run tests, commit**

```
refactor: test infrastructure cleanup — types, naming, safety

Unified memory_limit to i64, renamed failed_ids to unsuccessful_ids,
added URL encoding for query params, replaced unsafe env var removal
with serial_test, deduplicated container initialization logic.
```

---

### Task 52: Remaining minor fixes

**Files:**
- Modify: `conary-core/src/repository/gpg.rs:117` (streaming GPG verify)
- Modify: `conary-core/src/resolver/engine.rs:178-188` (constraint selection)
- Modify: `conary-core/src/resolver/sat.rs:186-250` (removal index)
- Modify: `conary-core/src/canonical/appstream.rs:218` (transaction error)
- Modify: `conary-core/src/provenance/signature.rs:109` (verified field)
- Modify: `conary-core/src/recipe/format.rs:58` (BTreeMap for variables)

- [ ] **Step 1: GPG — use streaming verification with VerifierBuilder**
- [ ] **Step 2: Resolver — keep strictest constraint in find_missing_dependencies**
- [ ] **Step 3: SAT — build name->solvable HashMap index for solve_removal**
- [ ] **Step 4: Appstream — use ? operator for transaction error handling**
- [ ] **Step 5: Provenance — add verified: bool field to Signature**
- [ ] **Step 6: Recipe — use BTreeMap for variables for deterministic substitution**
- [ ] **Step 7: Run tests, commit**

```
fix: batch of minor correctness and performance improvements

GPG: streaming verification instead of reading entire file.
Resolver: keep strictest constraint for duplicate deps.
SAT: HashMap index reduces solve_removal from O(S²) to O(S).
Appstream: proper transaction error propagation.
Provenance: add verified field to Signature.
Recipe: BTreeMap for deterministic variable substitution.
```

---

## Summary

| Phase | Tasks | Priority | Est. Complexity |
|-------|-------|----------|----------------|
| 1: Security-Critical | 10 | P0 | Medium-High |
| 2: Data Integrity | 9 | P1 | Medium |
| 3: Hardening | 9 | P2 | Medium |
| 4: Server | 8 | P2 | Medium |
| 5: Trust/Canonical | 7 | P2-P3 | Low-Medium |
| 6: Cleanup | 6 | P3 | Low |
| **Total** | **52** | | |

**Parallelism opportunities:** Within each phase, most tasks touch different files and can be dispatched as parallel subagents. Phase 1 tasks 1-10 are all independent. Phase 6 tasks can all run in parallel.

**Testing strategy:** Each task includes inline tests per project convention. After each phase, run the full test suite: `cargo test` and `cargo clippy -- -D warnings`.

**False positive excluded:** CCS verify.rs blob hash reconstruction (finding from review) — verified that `rsplit('/')` correctly extracts prefix/suffix for both `./objects/` and `objects/` paths.
