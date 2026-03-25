## Feature 5: Supply Chain Security -- Review Findings

### Summary

The supply chain security feature spans three modules (28 files, ~4800 lines):
TUF trust (7 files), provenance tracking (7 files), and capability declarations
with enforcement (14 files). The TUF implementation is sound and follows the
spec well -- root rotation, signature thresholds, version monotonicity, expiration,
and type-field checks are all present and correct. The capability enforcement
via landlock and seccomp is well-engineered. There are no P0 security
vulnerabilities. The main findings are around defense-in-depth gaps (verify_file
reading entire files into memory, string-based path matching in deny conflict
detection), two expect() calls in production provenance code, and several code
quality improvements.

---

### P0 -- Critical

None found.

---

### P1 -- Important

**[P1] [security]: `verify_file` reads entire file into memory without size limit**
- File: `conary-core/src/trust/verify.rs:222`
- Issue: `verify_file()` calls `std::fs::read(path)` with no size bound. If a
  target file is very large (e.g., multi-GB package), this will OOM the process.
  The TUF client has `MAX_TUF_METADATA_SIZE` for metadata fetches but the
  file verification path has no guard.
- Impact: DoS via OOM when verifying a very large target file.
- Fix: Use streaming hash via `BufReader` + chunked reads (consistent with the
  codebase's `hash::hash_reader` pattern noted in anti-patterns), or add a
  `MetaFile.length` check before reading.

**[P1] [security]: `find_deny_conflicts` uses string `starts_with` instead of path component matching**
- File: `conary-core/src/capability/enforcement/landlock_enforce.rs:139-145`
- Issue: `deny_path.starts_with(read_path)` is a string comparison, not a path
  component comparison. A deny path `/etcetera/secret` would incorrectly be
  flagged as conflicting with an allow path `/etc`. This is the opposite
  direction from the resolver's `path_matches` (which correctly uses
  `Path::starts_with`), so the two modules are inconsistent.
- Impact: False-positive deny conflict warnings in Warn/Audit mode; in Enforce
  mode, false `DenyConflict` errors that prevent legitimate capability policies.
- Fix: Use `std::path::Path::new(deny_path).starts_with(read_path)` for
  component-based matching, matching the resolver's pattern.

**[P1] [correctness]: `expect()` calls in production path (`Provenance::dna_hash`)**
- File: `conary-core/src/provenance/mod.rs:79-80`
- Issue: Two `expect()` calls in `dna_hash()`:
  `hex::decode(&hex).expect("SHA-256 hex is always valid")` and
  `DnaHash::from_bytes(&bytes).expect("SHA-256 always produces 32 bytes")`.
  While the invariant holds for SHA-256, this is a non-test production path.
  If the `Hasher` implementation ever changes (e.g., returning a different
  format), this panics instead of returning an error.
- Impact: Panic in production if the hasher invariant is violated.
- Fix: Change `dna_hash()` to return `Result<DnaHash, DnaHashError>` or at
  minimum document the infallibility with a safety comment. The method is
  called from non-test code.

**[P1] [correctness]: `verify_snapshot_consistency` silently passes when snapshot lacks root.json or targets.json entries**
- File: `conary-core/src/trust/verify.rs:132-159`
- Issue: The function uses `if let Some(root_meta) = snapshot.meta.get("root.json") && ...`
  which silently passes if the snapshot does not contain a root.json entry at all.
  Per the TUF spec, a snapshot MUST contain version information for root.json
  and targets.json. A malicious server could serve a snapshot without these
  entries to bypass consistency checks.
- Impact: Weakened mix-and-match attack protection. An attacker who controls the
  server can omit root.json/targets.json entries from snapshot to avoid version
  pinning checks.
- Fix: Make the presence of root.json (and targets.json when `expected_targets_version`
  is `Some`) mandatory. Return `ConsistencyError` if the entry is missing.

**[P1] [security]: Root rotation loop has no upper bound on iterations**
- File: `conary-core/src/trust/client.rs:218-254`
- Issue: `check_root_rotation` probes for `{version+1}.root.json` in an
  unbounded loop. A malicious server could serve an infinite sequence of
  valid root metadata (each signed by the previous root key, with incrementing
  versions) to force the client into an unbounded loop, consuming time and
  database writes.
- Impact: DoS against the TUF client during sync.
- Fix: Add a `MAX_ROOT_ROTATIONS` constant (TUF reference implementations
  typically use 1024) and break with an error if exceeded.

---

### P2 -- Improvement

**[P2] [correctness]: `load_capabilities` swallows `QueryReturnedNoRows` as `None`**
- File: `conary-core/src/capability/mod.rs:103-110`
- Issue: `load_capabilities` uses `.ok()` to convert any rusqlite error
  (including `QueryReturnedNoRows`) into `None`. This also silently swallows
  genuine database errors (corruption, locked database, schema mismatch).
- Impact: Silent failure -- a database error looks like "no capabilities declared".
- Fix: Use `.optional()` from `rusqlite::OptionalExtension` (already imported in
  client.rs) to properly distinguish "no rows" from actual errors, and propagate
  `Database` errors.

**[P2] [correctness]: `load_capabilities_by_name` also swallows DB errors via `.ok()`**
- File: `conary-core/src/capability/mod.rs:125-131`
- Issue: Same pattern as above -- the trove_id lookup uses `.ok()` which swallows
  all errors. A locked database returns `PackageNotFound` instead of a database error.
- Impact: Misleading error messages when database is inaccessible.
- Fix: Same as above -- use `.optional()`.

**[P2] [duplication]: `TufMetadataFields` trait is pure boilerplate**
- File: `conary-core/src/trust/client.rs:563-602`
- Issue: Four identical `impl TufMetadataFields` blocks each returning
  `self.version` and `&self.expires`. This is a macro candidate.
- Impact: Maintenance burden -- adding a new metadata type requires a new impl.
- Fix: Use a derive macro or a simple `macro_rules!` to generate the impls.

**[P2] [code-quality]: `canonical_json` wrapper in `trust/keys.rs` is a thin delegation**
- File: `conary-core/src/trust/keys.rs:27-30`
- Issue: `canonical_json` in keys.rs just calls `crate::json::canonical_json`
  and maps the error. This is fine for now but the doc comment says
  "Delegates to [] for the shared implementation" -- the link target is empty.
- Impact: Broken doc link. Minor confusion for readers.
- Fix: Fix the doc comment to reference `crate::json::canonical_json`.

**[P2] [security]: `ceremony::generate_role_key` writes private key to disk with default permissions**
- File: `conary-core/src/trust/ceremony.rs:22-23`
- Issue: `save_to_files` is called without explicitly setting restrictive
  permissions on the private key file. Depending on the `save_to_files`
  implementation and umask, the private key could be world-readable.
- Impact: Private TUF signing key potentially readable by other users on the
  build system.
- Fix: Verify that `save_to_files` sets 0600 permissions on the private key,
  or add explicit `std::fs::set_permissions` after creation.

**[P2] [code-quality]: `check_root_rotation` persists each intermediate root individually**
- File: `conary-core/src/trust/client.rs:241-243`
- Issue: Each intermediate root during rotation is persisted with `persist_root`
  and `persist_root_keys` outside of the final transaction. If the process
  crashes mid-rotation, the database could have partially-applied root state.
  The final `persist_metadata` for the current root is done later inside the
  main transaction, but the intermediate writes are not.
- Impact: Inconsistent TUF state after crash during root rotation.
- Fix: Wrap the entire rotation loop in a transaction, or at minimum document
  that intermediate persistence is intentional for recoverability.

**[P2] [ai-slop]: Over-commented obvious code in provenance module**
- File: `conary-core/src/provenance/build.rs` (various), `content.rs`, `source.rs`
- Issue: Many trivial methods like `add_dependency`, `set_host_attestation`,
  `add_chunk`, `add_patch` have doc comments that literally restate the method
  name ("Add a build dependency", "Set host attestation"). These provide no
  information beyond what the type signature already communicates.
- Impact: Noise that obscures meaningful documentation.
- Fix: Remove or condense trivial doc comments; keep comments only where they
  explain non-obvious behavior.

**[P2] [code-quality]: `SignatureScope::Display` duplicates `serde(rename_all)` logic**
- File: `conary-core/src/provenance/signature.rs:190-201`
- Issue: Manual `Display` impl reproduces the same strings as
  `#[serde(rename_all = "lowercase")]`. If a variant is added, both must be
  updated independently.
- Impact: Potential divergence between Display and serde serialization.
- Fix: Use `strum::Display` or have Display delegate to serde serialization, or
  at minimum add a test asserting Display matches serde output for all variants.

**[P2] [correctness]: `verify_file` passes `require_hash: false` -- hash check is optional**
- File: `conary-core/src/trust/verify.rs:229`
- Issue: `verify_file` calls `verify_metadata_hash(meta_ref, &content, false)`.
  With `require_hash: false`, if the `MetaFile` has no hash, verification
  silently succeeds. This is dangerous for target file verification where
  the hash should always be present.
- Impact: A target with no hash in the TUF targets metadata would pass
  verification without any content check.
- Fix: Change to `require_hash: true`, or at least make it a parameter so
  callers can opt in to strict mode. The client.rs code correctly uses
  `require_hash: true` for metadata cross-references.

---

### P3 -- Nitpick

**[P3] [naming]: `DnaHashError::InputTooShort` is misleading for inputs that are too long**
- File: `conary-core/src/provenance/dna.rs:12-13`
- Issue: The error says "requires {expected} bytes, got {actual}" but the variant
  is named `InputTooShort`. Passing 64 bytes would trigger `InputTooShort`
  with "requires 32 bytes, got 64" -- the name contradicts the message.
- Fix: Rename to `InvalidLength` or `LengthMismatch`.

**[P3] [style]: `CapabilityPolicy::load` accepts `Option<&str>` instead of `Option<&Path>`**
- File: `conary-core/src/capability/policy.rs:114`
- Issue: The path parameter is `&str` when it should be `&Path` for type safety.
- Fix: Change to `Option<&Path>`.

**[P3] [code-quality]: `IsolationLevel::None` variant name shadows the standard `None`**
- File: `conary-core/src/provenance/build.rs:320`
- Issue: `IsolationLevel::None` can cause confusion with `Option::None` in
  pattern matching contexts.
- Fix: Consider `IsolationLevel::Unrestricted` or `IsolationLevel::Bare`.

**[P3] [code-quality]: `ReproducibilityInfo::add_verifier` overloads "differences" field semantics**
- File: `conary-core/src/provenance/build.rs:305-312`
- Issue: `differences` stores builder_ids that didn't match rather than actual
  differences. The name suggests it would contain descriptions of what differed.
- Fix: Rename to `mismatched_builders` or `non_matching_verifiers`.

**[P3] [style]: Inconsistent use of `pub mod` vs `mod` for submodules**
- File: `conary-core/src/capability/mod.rs:32-35`
- Issue: `declaration` is `mod` (private) while `enforcement`, `inference`,
  `policy`, `resolver` are `pub mod`. The declaration types are re-exported
  individually, which is correct, but the inconsistency is worth noting.
- Fix: This is actually correct (declaration's public API is re-exported via
  `pub use`). No change needed, but a comment noting the deliberate choice
  would help.

**[P3] [code-quality]: `HostAttestation` hostname field is noted as "not used in hash" but this is only in a comment**
- File: `conary-core/src/provenance/build.rs:186`
- Issue: The comment `/// Hostname (for audit, not used in hash)` is the only
  indicator that hostname is excluded from `canonical_bytes()`. There is no
  enforcement of this invariant.
- Fix: Consider adding a test that verifies changing hostname does not change
  `canonical_bytes()` output.

---

### Cross-Domain Notes

**[Cross-Domain] [Feature 6 - CCS]: `ccs::signing::SigningKeyPair` is used throughout trust and provenance**
- The `SigningKeyPair` type from the CCS module is the foundation for all TUF
  key operations. The `save_to_files` method's permission handling (P2 finding)
  lives in the CCS module, not trust.

**[Cross-Domain] [Feature 3 - Derivation]: `crate::json::canonical_json` shared with model signing**
- The canonical JSON implementation is shared between TUF (Feature 5) and model
  collection signing (Feature 3). Changes to `sort_json_keys` affect both
  cryptographic paths.

---

### Strengths

1. **TUF verification completeness** (`verify.rs:29-70`): The signature verification
   correctly handles duplicate key ID deduplication, only supports ed25519
   (rejecting unknown key types), uses `verify_strict` (not `verify`), and
   properly separates role-specific keys via `extract_role_keys`. The doc
   comment on `verify_signatures` explicitly warns callers about the key
   filtering requirement.

2. **Root rotation follows TUF spec** (`client.rs:212-254`): Root is verified
   against both old and new keys (`verify_root`), version monotonicity is
   enforced, expiration is checked, and the type field is validated. This is
   the most security-critical path and it is implemented correctly.

3. **Metadata type-field checking** (`client.rs:552-560`): `verify_type_field`
   prevents a server from serving the wrong metadata type (e.g., returning
   targets.json when snapshot.json is requested). This is a subtle TUF
   requirement that many implementations miss.

4. **Size limits on metadata fetch** (`client.rs:256-318`): The
   `MAX_TUF_METADATA_SIZE` (10MB) is checked both via Content-Length header
   and post-download body size, preventing DoS via oversized metadata.

5. **Landlock enforcement design** (`landlock_enforce.rs`): The deny-conflict
   detection is a thoughtful addition that prevents silent enforcement failures.
   The three-mode system (Audit/Warn/Enforce) enables progressive deployment.
   The ordering constraint (landlock before seccomp) is correctly documented and
   implemented.

6. **Seccomp profile composition** (`declaration.rs:432-611`): The
   `compose_profile` pattern with shared base sets avoids duplication and makes
   it easy to audit which syscalls each profile allows. The explicit exclusion
   of `chroot` from the scriptlet profile (with a security comment explaining
   why) shows security-conscious design.

7. **Canonical bytes trait** (`provenance/mod.rs:107-109`): The `CanonicalBytes`
   trait with sorted collections for deterministic hashing is well-implemented
   across all provenance layers. Each implementation sorts by the appropriate
   key (key_id for signatures, name for dependencies, hash for patches).

8. **Capability inference tier system** (`inference/mod.rs:392-494`): The
   four-tier inference system (wellknown, heuristic, config scan, binary
   analysis) with progressive confidence scoring is well-architected. The
   merge logic correctly prefers higher-confidence results.

---

### Recommendations

1. **Add a root rotation iteration limit** to `check_root_rotation` (P1). This
   is the highest-priority fix -- an unbounded loop in a network-facing code
   path is a DoS risk. A constant like `MAX_ROOT_ROTATIONS = 1024` with an
   error return is the standard approach.

2. **Make `verify_snapshot_consistency` strict** about required entries (P1).
   The snapshot MUST contain root.json and targets.json version pins per the
   TUF spec. Silently passing when they are absent weakens the security model.

3. **Replace `expect()` in `Provenance::dna_hash()`** with proper error handling
   (P1). This is called in production paths and should not panic. Either return
   `Result<DnaHash, DnaHashError>` or use `unwrap_or_else(|| unreachable!())`
   with a comment documenting the invariant.

---

### Assessment

**Ready to merge?** With fixes

**Reasoning:** The TUF implementation is fundamentally sound and follows the spec
well. No P0 vulnerabilities were found. However, the five P1 findings --
unbounded root rotation loop, silent snapshot consistency bypass, expect() in
production code, verify_file OOM, and string-based path matching -- should be
addressed before this code is relied upon in production trust decisions. The
remaining P2/P3 findings are code quality improvements that can be addressed
incrementally.

---

### Work Breakdown

- **Task 1** [P1]: Add `MAX_ROOT_ROTATIONS` constant and loop guard to `TufClient::check_root_rotation` in `trust/client.rs`
- **Task 2** [P1]: Make `verify_snapshot_consistency` require root.json and targets.json entries in the snapshot
- **Task 3** [P1]: Convert `Provenance::dna_hash()` to return `Result<DnaHash, DnaHashError>` and update callers
- **Task 4** [P1]: Replace `std::fs::read()` in `verify_file` with streaming hash or add size limit
- **Task 5** [P1]: Fix `find_deny_conflicts` to use `Path::starts_with` instead of string `starts_with`
- **Task 6** [P2]: Replace `.ok()` with `.optional()` in `load_capabilities` and `load_capabilities_by_name`
- **Task 7** [P2]: Change `verify_file` to use `require_hash: true` by default
- **Task 8** [P2]: Verify `save_to_files` sets 0600 permissions on private key files
- **Task 9** [P2]: Fix broken doc link in `trust/keys.rs:25` canonical_json delegation comment
- **Task 10** [P3]: Rename `DnaHashError::InputTooShort` to `InvalidLength`
