## Feature 6: Cross-Distro & Extensibility -- Review Findings

### Summary

Feature 6 spans ~6,500 lines across 27 Rust source files covering canonical name mapping
(Repology, AppStream, discovery heuristics), the system model (TOML parser, diff engine,
remote includes, lockfile, signing, replatform), automation (scheduling, checks, actions),
component classification, flavor specs, labels, triggers, scriptlets, container isolation,
hashing, self-update, canonical JSON, utilities, progress tracking, and the centralized
error type. The code is generally well-structured with thorough test coverage and correct
security primitives (Ed25519 signing, SHA-256 download verification, namespace isolation,
path traversal guards). I found one P0 (self-update signature bypass in production), two
P1s (canonical JSON not truly canonical under `serde_json` default features; content hash
verification uses non-canonical serialization), and a handful of P2/P3 improvements.

---

### Strengths

- **Thorough test coverage.** Nearly every module has in-file `#[cfg(test)] mod tests`
  with both happy-path and edge-case tests. `signing.rs` tests round-trip, wrong-key,
  tampered-data, and invalid-length scenarios. `self_update.rs` tests pre-release semver
  ordering, extract-from-tar, and atomic rename.

- **Ed25519 signing chain is clean.** `model/signing.rs` delegates canonical JSON
  production to `crate::json::canonical_json`, avoiding a second hand-rolled
  implementation. Key loading supports both raw-32-byte and hex-64-char formats with
  proper validation.

- **Self-update download verifies SHA-256 inline.** `stream_update_to_disk` hashes while
  writing, avoiding TOCTOU between download and verify. If the checksum fails, the
  temp file is deleted before returning the error.

- **Container isolation defaults to maximum security.** `ContainerConfig::default()`
  enables PID, UTS, IPC, mount, and network namespace isolation with resource limits.
  The `pristine()` config for bootstrap builds mounts zero host paths by default.

- **Trigger handler path traversal is guarded.** `handler_exists_in_root` uses
  `crate::filesystem::path::safe_join()` for absolute-path handlers, returning `false`
  on traversal attempts (`trigger/mod.rs:376`).

- **Rules engine anchors regexes.** `anchor_regex` in `canonical/rules.rs` wraps
  user-provided patterns in `^(?:...)$`, preventing substring matches that could
  produce incorrect canonical names.

- **Automation defaults are safe.** Global mode is `Suggest`, AI mode is `Advisory`,
  major upgrades default to `require_approval: true`. The `ActionExecutor::execute`
  placeholder logs a warning and takes no action, which is correct for unfinished code.

- **Hash module provides clean abstraction.** `hash.rs` unifies SHA-256, XXH128, and
  MD5 behind `Hasher`/`hash_bytes`/`hash_reader` with streaming support, and separates
  cryptographic from non-cryptographic use cases clearly in the docs.

---

### Issues

#### P0 -- Security

**[P0] [security]: Self-update signature verification is unconditionally bypassed**
- File: `conary-core/src/self_update.rs:79`
- Issue: `verify_update_signature()` returns `Ok(())` when `cfg!(test)` is true. This is
  the public function used by the update pipeline. However, the `TRUSTED_UPDATE_KEYS`
  array is empty (`&[]`), which means the `verify_update_signature_with_keys` path would
  return `Err(Untrusted)` for any signature. The net effect is that in production builds,
  **no signature verification can pass** -- the function is dead code unless keys are
  added. Meanwhile, in test builds, any payload is accepted. This is a design concern
  rather than an active exploit, but the empty key list means the entire signature
  verification apparatus provides zero protection until keys are populated. If a release
  were shipped before keys are added, any MITM'd update payload would install successfully
  since the caller would skip verification when the field is `None`.
- Impact: Self-update integrity relies solely on SHA-256 checksum. A compromised CDN
  serving both the `/latest` JSON and the CCS file could serve a matching pair.
- Fix: (1) Add the release signing public key to `TRUSTED_UPDATE_KEYS`. (2) Make
  `check_for_update` / the CLI caller require a non-None `signature` field when
  `TRUSTED_UPDATE_KEYS` is non-empty, failing the update if the server doesn't provide
  one. (3) Remove the `cfg!(test)` bypass -- use `verify_update_signature_with_keys`
  directly in tests with a test key list.

---

#### P1 -- Correctness

**[P1] [correctness]: Canonical JSON may not be deterministic under default serde_json features**
- File: `conary-core/src/json.rs:36-41`
- Issue: `sort_json_keys` collects into `serde_json::Map<String, serde_json::Value>`.
  By default, `serde_json::Map` is backed by `BTreeMap`, which preserves insertion order
  lexicographically. However, if `serde_json` is compiled with the `preserve_order`
  feature (which uses `IndexMap`), the `.collect()` from a non-sorted iterator would
  produce keys in insertion order from the `.iter()` call on the source map. The code
  iterates the source map and collects -- since the source may be an `IndexMap`, the
  iteration order depends on insertion order, not sorted order. The `collect()` into
  `serde_json::Map` would then preserve that non-sorted order if `preserve_order` is
  active.
- Impact: If any dependency or feature flag activates `serde_json/preserve_order`,
  canonical JSON output would silently become non-deterministic, breaking signature
  verification for model collections and TUF metadata.
- Fix: Collect into a `BTreeMap<String, serde_json::Value>` explicitly, then convert to
  `serde_json::Map` via `.into_iter().collect()`. This guarantees sorted order regardless
  of serde_json feature flags.

**[P1] [correctness]: Remote collection content hash verification uses non-canonical serialization**
- File: `conary-core/src/model/remote.rs:282-283`
- Issue: When verifying the content hash of a fetched remote collection, the code clones
  the deserialized `CollectionData`, sets `content_hash = ""`, re-serializes with
  `serde_json::to_vec`, and hashes. But `serde_json::to_vec` does not guarantee key
  order -- it depends on the `Serialize` implementation for `HashMap<String, String>`
  (the `pins` field), which iterates in arbitrary order. The publisher in
  `build_collection_data_from_model` (line 533) also uses `serde_json::to_vec`. If the
  HashMap iteration order differs between the publisher and the verifier (different
  platforms, Rust versions, or just different insertion history), the hashes will
  diverge and verification will fail spuriously -- or worse, match when it shouldn't.
- Impact: Content hash verification is fragile across platforms/versions. Could cause
  spurious "content hash mismatch" errors or miss real tampering.
- Fix: Use `crate::json::canonical_json(&verification_data)` instead of
  `serde_json::to_vec` for both the publisher and verifier paths. This ensures
  deterministic serialization. The publisher in `build_collection_data_from_model`
  (line 533) should also use `canonical_json`.

---

#### P2 -- Improvement

**[P2] [correctness]: `DiffAction::package()` returns distro name for `SetSourcePin`, not a package name**
- File: `conary-core/src/model/diff.rs:130`
- Issue: `DiffAction::SetSourcePin { distro, .. } => distro` -- the `package()` method
  is documented as returning "the package name this action affects" but for
  `SetSourcePin` it returns the distro name (e.g., "arch"). Callers that filter or
  display by `action.package()` would get confusing results.
- Impact: Display/reporting confusion; no data corruption.
- Fix: Return a sentinel like `"<source-policy>"` (matching `ClearSourcePin`) or change
  the return type to `Option<&str>`.

**[P2] [code-quality]: `ActionBuilder::build()` uses `Debug` format for action ID generation**
- File: `conary-core/src/automation/action.rs:88-89`
- Issue: `format!("{:?}-{}-{}", self.category, ...)` uses `Debug` formatting of the
  `AutomationCategory` enum for the action ID. If the enum variant name changes (e.g.,
  renamed during refactoring), all existing action IDs become incompatible with any
  persisted state that references them.
- Impact: Fragile ID generation; breaks if enum variants are renamed.
- Fix: Use `self.category.display_name()` or a dedicated stable string representation.

**[P2] [security]: `handler_exists_in_root` does not use `safe_join` for non-absolute PATH search**
- File: `conary-core/src/trigger/mod.rs:393-397`
- Issue: For non-absolute handler commands (e.g., "ldconfig"), the function iterates
  `search_paths` and uses `root.join(search_path).join(cmd)`. If `cmd` contains `..`
  components, `Path::join` would resolve them, potentially escaping the target root.
  The absolute-path branch correctly uses `safe_join`, but the PATH-search branch does
  not.
- Impact: A malicious trigger handler name containing `../` could reference files outside
  the target root.
- Fix: Use `crate::filesystem::path::safe_join` for the PATH-search branch too:
  `safe_join(&root.join(search_path), cmd)`.

**[P2] [correctness]: `is_major_upgrade` has false positives for epoch-prefixed versions**
- File: `conary-core/src/automation/check.rs:521-534`
- Issue: The function splits on non-digit characters and takes the first numeric
  component. For RPM versions with epochs like "1:2.0.0", it would extract "1" as the
  major version. Comparing "1:2.0.0" vs "1:3.0.0" would see major=1 vs major=1 and
  report "not a major upgrade", which is wrong (2->3). More subtly, "1:2.0" vs "2.0.0"
  would compare 1 vs 2, reporting a false major upgrade.
- Impact: Incorrect major upgrade detection for epoch-bearing RPM versions.
- Fix: Strip the epoch prefix (everything before the first `:`) before extracting
  the major version component.

**[P2] [code-quality]: Duplicate `reqwest::Client` construction across modules**
- Files: `canonical/client.rs:34`, `canonical/repology.rs:211`, `model/remote.rs:452`,
  `self_update.rs:221`, `self_update.rs:317`
- Issue: Each module constructs its own `reqwest::Client` with similar configuration
  (user-agent, timeout). This duplicates the client setup and risks inconsistent
  timeouts or user-agents.
- Fix: Extract a shared `crate::http::client()` or `crate::repository::default_client()`
  builder.

**[P2] [architecture]: `model/mod.rs` has dual error systems**
- File: `conary-core/src/model/mod.rs:102-133`
- Issue: The model module defines its own `ModelError` enum with `thiserror`, but many
  of the same error categories exist in `crate::error::Error` (e.g., `ParseError`,
  `IoError`, `DatabaseError`, `DownloadError`). The model uses `ModelResult<T>` while
  the rest of the codebase uses `crate::error::Result<T>`. This forces callers to
  `.map_err()` at every boundary.
- Impact: Ergonomic friction; error context lost at conversion boundaries.
- Fix: Consider migrating `ModelError` variants into `crate::error::Error` or
  implementing `From<ModelError> for crate::error::Error`.

---

#### P3 -- Style/Nitpick

**[P3] [convention]: Missing file header on `model/replatform.rs`**
- File: `conary-core/src/model/replatform.rs:1`
- Issue: Header is `// conary-core/src/model/replatform.rs` -- this is correct.
  (All files checked have correct headers.)

**[P3] [code-quality]: `parse_duration` splits at `len()-1` which is fragile for multi-digit units**
- File: `conary-core/src/automation/mod.rs:357`
- Issue: `s.split_at(s.len() - 1)` assumes the unit is exactly 1 byte. This works for
  the current set of units (s, m, h, d, w) but would break if "ms" were ever added.
  Not a bug today, but a maintenance trap.
- Fix: Use `s.trim_end_matches(|c: char| c.is_ascii_alphabetic())` to separate numeric
  and unit parts.

**[P3] [code-quality]: Unnecessary `.to_string()` allocations in discovery strategies**
- File: `conary-core/src/canonical/discovery.rs:56-63` (and similar in all 5 strategies)
- Issue: Each strategy allocates `String` for `canonical_name`, `implementations[].0`,
  `implementations[].1`, and `source` in the inner loop. For large package sets (tens of
  thousands), this creates significant allocation pressure.
- Fix: For performance-critical paths, consider using `Cow<'_, str>` or interned strings.
  For now, this is acceptable since discovery runs infrequently.

**[P3] [code-quality]: `#[allow(dead_code)]` on `CanonicalMapResponse` fields**
- File: `conary-core/src/canonical/client.rs:16-17`
- Issue: `version` and `generated_at` are deserialized but never read. They should
  either be used (e.g., logged or version-checked) or documented as intentionally
  ignored for forward compatibility.
- Fix: Add a comment explaining these fields are kept for protocol evolution, or remove
  the `#[allow(dead_code)]` and prefix with `_`.

**[P3] [ai-slop]: Over-commented obvious patterns in automation/action.rs**
- File: `conary-core/src/automation/action.rs:276-308`
- Issue: The `execute` method has TODO comments and a match that returns
  `Ok(ActionStatus::Completed)` for every branch identically. The match arms are
  purely decorative -- every category does the same thing.
- Fix: Replace with a single `self.executed.push(action.id.clone()); Ok(ActionStatus::Completed)` after the match, with a single TODO.

---

### Cross-Domain Notes

**[Cross-Domain] content hash used in lockfile depends on non-canonical serialization**
- Affects: Feature 6 (model/lockfile.rs) depends on Feature 6 (model/remote.rs)
- The lockfile stores `content_hash` from `CollectionData`, which is computed via
  `serde_json::to_vec` (non-deterministic). This hash is then checked in
  `ModelLock::check_drift`. If the hash computation is fixed to use canonical JSON,
  existing lockfiles will show false drift on first check.

---

### Recommendations

1. **Populate `TRUSTED_UPDATE_KEYS` and remove the `cfg!(test)` bypass** in
   `self_update.rs`. This is the most critical item -- the self-update signature
   verification infrastructure exists but provides zero protection until keys are added.
   In the meantime, require signatures when the key list is non-empty and reject updates
   without signatures.

2. **Switch content hash computation to `canonical_json`** in both
   `model/remote.rs:282` (verifier) and `model/remote.rs:533` (publisher). This
   eliminates platform-dependent HashMap iteration order from the hash input, making
   content hash verification reliable across environments.

3. **Guard `canonical_json` against `serde_json/preserve_order`** by explicitly
   collecting into `BTreeMap` in `sort_json_keys`. Add a compile-time assertion or
   test that verifies determinism under both feature flag states.

---

### Assessment

**Ready to merge?** With fixes

**Reasoning:** The codebase is well-tested and architecturally sound, but the self-update
signature bypass (P0) and the non-deterministic content hash verification (P1) need to
be addressed before this code ships to production systems. The P0 is mitigated by the
fact that keys haven't been populated yet (so the feature is effectively disabled), but
the architecture should be corrected now before a release introduces the real keys.

---

### Work Breakdown

1. **[P0] Self-update signature hardening**
   - Add release signing Ed25519 public key to `TRUSTED_UPDATE_KEYS`
   - Remove `cfg!(test)` bypass from `verify_update_signature`
   - Make the CLI reject updates without signatures when trusted keys exist
   - Files: `conary-core/src/self_update.rs`

2. **[P1] Canonical JSON determinism**
   - Collect into explicit `BTreeMap` in `sort_json_keys`
   - Add test that fails under `preserve_order` to prevent regression
   - Files: `conary-core/src/json.rs`

3. **[P1] Content hash canonical serialization**
   - Replace `serde_json::to_vec` with `canonical_json` in verification path
   - Replace `serde_json::to_vec` with `canonical_json` in publisher path
   - Files: `conary-core/src/model/remote.rs`

4. **[P2] Trigger handler PATH search safe_join**
   - Use `safe_join` for non-absolute handler names in `handler_exists_in_root`
   - Files: `conary-core/src/trigger/mod.rs`

5. **[P2] Fix DiffAction::package() for SetSourcePin**
   - Return sentinel string instead of distro name
   - Files: `conary-core/src/model/diff.rs`

6. **[P2] Fix is_major_upgrade epoch handling**
   - Strip epoch prefix before major version extraction
   - Files: `conary-core/src/automation/check.rs`
