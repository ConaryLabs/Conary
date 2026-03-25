## Feature 3: Filesystem & Generations -- Review Findings

### Summary

The filesystem, generation, and delta modules form the immutable deployment
layer of Conary. Overall code quality is high: CAS operations are crash-safe
with atomic stores, path traversal is well-guarded by `safe_join()` and
`sanitize_path()`, and the three-way /etc merge logic is correct and
thoroughly tested. The most significant findings are a TOCTOU race in
`safe_join()` that silently degrades to no defense-in-depth, an unused
`digest` field in `MountOptions` that gives a false sense of fsverity
enforcement, and a non-atomic metadata write in `GenerationMetadata::write_to()`
that could leave a truncated JSON file after a crash.

---

### P0 -- Data Loss / Security / Production Crash

**[P0] [correctness]: `build_generation_from_db` has a race between generation number reservation and state creation**
- File: `conary-core/src/generation/builder.rs:346-397`
- Issue: `next_state_number()` reads `MAX(state_number)` at line 346, then the
  generation directory is created, EROFS built, and `create_snapshot()` calls
  `next_state_number()` *again* at `state.rs:380`. If another process creates a
  state between these two calls, the `debug_assert_eq!` at line 394 fires in
  debug builds (silent in release), and the metadata.json records a generation
  number that does not match the actual DB state number. The symlink and
  directory name would point to the wrong generation.
- Impact: Generation number mismatch causes the `current` symlink to reference
  a directory whose metadata disagrees with the DB. GC could delete CAS objects
  that the "real" generation still needs.
- Fix: Wrap the entire `build_generation_from_db` function body in
  `db::with_transaction(conn, |tx| { ... })` to serialize the state number
  reservation with the snapshot creation. Alternatively, have
  `create_snapshot()` accept an explicit state number instead of re-computing
  it.

**[P0] [correctness]: `etc_merge` fallback path follows symlinks via `is_file()`**
- File: `conary-core/src/generation/etc_merge.rs:302-304`
- Issue: The `scan_dir_recursive` function correctly uses `symlink_metadata()`
  to avoid following symlinks (line 272), but the fallback path in
  `upper_file_hash()` at line 303 uses `abs_path.is_file()`, which follows
  symlinks. A crafted symlink in the overlay upper directory (e.g.,
  `etc/resolv.conf -> /etc/shadow`) could cause the merge logic to read and
  hash a file outside the overlay boundary.
- Impact: Information disclosure -- the hash of an arbitrary file is computed
  and compared, potentially leaking whether a file has specific content.
  In the Conflict variant, the hash is stored in the MergePlan and could be
  logged.
- Fix: Replace `abs_path.is_file()` with
  `abs_path.symlink_metadata().is_ok_and(|m| m.is_file())` to avoid following
  symlinks in the fallback path.

---

### P1 -- Incorrect Behavior / Silent Failure

**[P1] [correctness]: `MountOptions.digest` field is never used**
- File: `conary-core/src/generation/mount.rs:34`
- Issue: The `digest` field on `MountOptions` is documented as "Passed as
  `digest=` mount option when present" but `to_mount_args()` never reads it.
  The `digest=<hex>` option is what tells the kernel to verify the EROFS image
  integrity at mount time. Without it, even when `verity: true` is set, the
  kernel trusts whatever image is at the path without checking its digest.
- Impact: fs-verity image integrity is not enforced at mount time. An attacker
  who can replace the EROFS image on disk could mount a malicious image even
  when the caller requested verity.
- Fix: Add to `to_mount_args()`:
  ```rust
  if let Some(ref digest) = self.digest {
      opts.push(format!("digest={digest}"));
  }
  ```

**[P1] [architecture]: `composefs.rs` and `metadata.rs` use `anyhow::Result` instead of `crate::Result`**
- File: `conary-core/src/generation/composefs.rs:11`, `conary-core/src/generation/metadata.rs:8`
- Issue: These two files use `anyhow::Result` for their public APIs
  (`preflight_composefs`, `GenerationMetadata::write_to`, `read_from`), while
  every other module in `conary-core` uses `crate::error::Result` with
  `thiserror`. This creates an error type mismatch at call sites -- callers
  must convert `anyhow::Error` to `crate::Error`, and the structured error
  variants are lost.
- Impact: Callers cannot match on specific error variants (e.g.,
  distinguishing "file not found" from "JSON parse error" in
  `GenerationMetadata::read_from`). This also breaks the project convention
  established in CLAUDE.md ("all errors use thiserror").
- Fix: Replace `anyhow::Result` with `crate::Result` and `anyhow!()` with
  the appropriate `crate::error::Error` variant. Add specific error variants
  if needed (e.g., `Error::ComposeFsNotSupported`).

**[P1] [correctness]: `etc_merge` does not handle overlayfs whiteout files**
- File: `conary-core/src/generation/etc_merge.rs:261-286`
- Issue: `scan_dir_recursive` skips symlinks and processes regular files, but
  does not handle overlayfs whiteout entries (character device `0/0` named
  `.wh.<filename>` or the opaque directory marker `.wh..wh..opq`). When a user
  deletes a file from the overlay, the overlay creates a whiteout character
  device. The merge logic does not detect this, so a user-deleted /etc file
  would not appear in the merge plan at all -- it would silently reappear in
  the next generation.
- Impact: User deletions of /etc files are silently reverted on generation
  transitions.
- Fix: In `scan_dir_recursive`, detect whiteout entries (char device with
  major/minor 0/0, or filenames starting with `.wh.`) and record them in the
  scanned map with a sentinel value (e.g., a special "deleted" hash). Then
  in `classify`, treat a whiteout as "user deleted" similar to
  `OrphanedUserFile`.

**[P1] [correctness]: `safe_join()` silently skips defense-in-depth when path does not exist**
- File: `conary-core/src/filesystem/path.rs:122-132`
- Issue: The `if let (Ok(...), Ok(...))` guard means that if either
  `canonicalize()` fails (e.g., the path does not exist yet, which is the
  common case when deploying new files), the entire defense-in-depth check is
  silently skipped. The function relies solely on `sanitize_path()`, which is
  solid but the comment says "catches any edge cases we might have missed."
  Those edge cases are precisely the ones where the path does not yet exist.
- Impact: If `sanitize_path()` has a bug, the defense-in-depth layer provides
  no protection for the most common use case (creating new files).
- Fix: This is not trivially fixable since `canonicalize` requires the path
  to exist. Consider using a manual normalization that resolves `..` against
  the known root without requiring filesystem access. At minimum, add a
  comment clarifying that the defense-in-depth only works for existing paths,
  or log a debug warning when canonicalize fails so operators know the
  fallback is in effect.

---

### P2 -- Improvement Opportunity / Minor Inconsistency

**[P2] [correctness]: `GenerationMetadata::write_to()` is not crash-safe**
- File: `conary-core/src/generation/metadata.rs:71-76`
- Issue: `write_to()` calls `std::fs::write()` directly, which is not atomic.
  A crash during the write could leave a truncated `.conary-gen.json`. The
  EROFS image write in `builder.rs` correctly uses temp-file + fsync + rename,
  but the metadata write does not.
- Impact: After a crash, `GenerationMetadata::read_from()` could fail to
  deserialize the truncated JSON, leaving the generation directory in an
  inconsistent state (valid EROFS image but unreadable metadata).
- Fix: Use the same temp + fsync + rename pattern used in `builder.rs:259-293`.

**[P2] [security]: TOCTOU in `CasStore::atomic_store()` existence check**
- File: `conary-core/src/filesystem/cas.rs:103-104`
- Issue: `path.exists()` is checked before writing, but another process could
  delete the file between the check and the read. This is mostly harmless for
  CAS (duplicate writes are idempotent) but the `Ok(false)` return is
  misleading -- it claims content "already existed" when it may have been
  deleted.
- Impact: Minimal for CAS operations (idempotent by design), but the early
  return means the content is not re-stored if it was deleted between the
  check and the return. A concurrent GC could delete a CAS object that
  `atomic_store` then reports as "already exists."
- Fix: After the `path.exists()` early return, this is benign since any
  consumer would also fail to read the missing object and trigger a re-fetch.
  Document this explicitly with a comment. Alternatively, remove the existence
  check and let the rename handle the race (if the file exists, rename
  overwrites atomically).

**[P2] [code-quality]: Duplicate delta code between `generation/delta.rs` and `delta/`**
- File: `conary-core/src/generation/delta.rs` and `conary-core/src/delta/generator.rs`
- Issue: Both modules implement zstd dictionary compression/decompression with
  nearly identical logic: `EncoderDictionary::copy`, `Encoder::with_prepared_dictionary`,
  `Decoder::with_prepared_dictionary`, 64KB read buffer, size limit check. The
  generation delta module uses 512 MiB limit while the CAS delta module uses
  2 GiB limit.
- Impact: Bug fixes or improvements to the compression logic must be applied
  in two places.
- Fix: Extract a shared `zstd_dict_compress(data, dictionary, level)` /
  `zstd_dict_decompress(data, dictionary, max_output)` pair into a utility
  module (e.g., `conary-core/src/compression/dict.rs`) and have both modules
  call it.

**[P2] [code-quality]: `accept_package_paths()` has a dead binding**
- File: `conary-core/src/generation/etc_merge.rs:81-83`
- Issue: The line `let _ = a;` is dead code. The `a` binding from the filter
  closure is unused and explicitly silenced.
- Impact: Minor readability issue.
- Fix: Replace the `.map(|(p, a)| { let _ = a; p.as_path() })` with
  `.map(|(p, _)| p.as_path())`.

**[P2] [architecture]: `DeltaGenerator.cas` is `pub(crate)` exposing internal state**
- File: `conary-core/src/delta/generator.rs:22`
- Issue: `pub(crate) cas: CasStore` exposes the internal CAS store, which is
  used directly in tests (`generator.cas.store(...)`). This leaks the
  implementation detail that the generator owns a CAS store.
- Impact: Test coupling -- tests depend on the generator's internal storage
  rather than using the public API.
- Fix: Make `cas` private and add a `#[cfg(test)]` accessor, or restructure
  the test to use a shared CAS store passed to both generator and the test
  setup.

**[P2] [code-quality]: `hardlink_from_existing()` reads entire file into memory for hashing**
- File: `conary-core/src/filesystem/cas.rs:401`
- Issue: `fs::read(existing_path)` loads the entire file into memory to
  compute the hash. For large files (e.g., kernel images, debug symbols), this
  is wasteful. The file is read once for hashing and never actually copied
  (the hardlink avoids the copy).
- Impact: Memory pressure during adoption of large files.
- Fix: Use streaming hash via `crate::hash::hash_reader()` with a
  `BufReader`, then hardlink. Only fall back to `fs::read` + `store()` if the
  hardlink fails and a copy is needed.

**[P2] [code-quality]: `GENERATION_FORMAT` constant mismatch with rules doc**
- File: `conary-core/src/generation/metadata.rs:16` vs `.claude/rules/generation.md`
- Issue: The code defines `GENERATION_FORMAT` as `"composefs"` but the rules
  document says it is `"composefs-erofs-v1"`. The metadata
  `backwards_compat` test at line 220 shows the `format` field defaults to
  `""` for old generations, so the value matters for format detection.
- Impact: Documentation and code disagree. New developers will be confused.
- Fix: Update the rules doc to match the code (`"composefs"`), or update the
  code if the more descriptive string was intended.

---

### P3 -- Style / Nitpick

**[P3] [style]: `detect_kernel_version_from_troves` test is a tautology**
- File: `conary-core/src/generation/builder.rs:610-612`
- Issue: `assert!(result.is_some() || result.is_none())` is always true.
  The test name says "does not panic" which is valid, but the assertion adds
  no value.
- Fix: Remove the assertion or replace with a comment: `// Smoke test: just
  verify no panic`.

**[P3] [style]: `VfsNode` hash stored as `String` rather than a newtype**
- File: `conary-core/src/filesystem/vfs/mod.rs:49`
- Issue: The `hash` field in `NodeKind::File` is a plain `String`. A newtype
  wrapper (e.g., `ContentHash(String)`) would make it impossible to
  accidentally pass a non-hash string.
- Impact: Type safety improvement only; no runtime bug.
- Fix: Consider introducing a `ContentHash` newtype if the VFS is used in
  more places.

**[P3] [style]: `sanitize_filename` does not check for null bytes**
- File: `conary-core/src/filesystem/path.rs:154-174`
- Issue: `sanitize_path` checks for null bytes (line 48) but
  `sanitize_filename` does not. A filename with an embedded null byte could
  truncate at C API boundaries.
- Impact: Low -- `sanitize_filename` is used for single components that
  typically come from package metadata, not raw user input. But for
  consistency with `sanitize_path`, the check should be present.
- Fix: Add `if name.contains('\0') { return Err(...); }` before the other
  checks.

**[P3] [style]: `composefs_rs_eval.rs` is test-only but not gated behind `#[cfg(test)]` at module level**
- File: `conary-core/src/generation/composefs_rs_eval.rs`
- Issue: The entire file is `#[cfg(test)] mod tests { ... }`, but the module
  declaration in `mod.rs` is `#[cfg(feature = "composefs-rs")] pub mod composefs_rs_eval;`.
  When the feature is enabled in production, the module is compiled (to an
  empty item). This is harmless but unconventional.
- Impact: None at runtime; minor compile-time overhead.
- Fix: Add `#[cfg(test)]` to the module declaration in `mod.rs` alongside the
  feature gate.

---

### Cross-Domain Notes

**[Cross-Domain: DB] Race in `build_generation_from_db`**: The P0 finding above
originates in `generation/builder.rs` but the root cause is that
`SystemState::next_state_number()` in `db/models/state.rs:91` is a
non-transactional read. The fix requires changes in the DB module
(accepting an explicit state number or wrapping in a transaction).

**[Cross-Domain: CLI] GC safety depends on CLI caller correctness**: The
`gc_cas_objects()` function in `generation/gc.rs` trusts the caller to
provide the correct set of surviving state IDs. The CLI caller in
`src/commands/generation/commands.rs:236-268` maps generation numbers to
state IDs correctly, but there is no lock preventing a concurrent
`build_generation_from_db` from creating a new generation whose CAS objects
are not in the "surviving" set. A file-based lock on the generations
directory would prevent concurrent builds and GC from racing.

---

### Strengths

1. **CAS crash safety** (`cas.rs:94-126`): The `atomic_store` implementation
   is textbook correct -- temp file with PID+counter naming, fsync data, rename,
   fsync parent directory. The naming scheme avoids cross-process races.

2. **Path traversal defense** (`path.rs:43-85`): `sanitize_path()` correctly
   handles null bytes, `..` traversal, leading slashes, and dot components.
   The test suite covers all the attack vectors including multi-level traversal
   (`usr/../../../etc/passwd`).

3. **Three-way /etc merge logic** (`etc_merge.rs:166-248`): The `classify()`
   function is clean and correct. The edge cases (user matches new package,
   user matches old base, user-created files) are all handled with clear
   comments. The test suite is comprehensive with 10 test cases covering
   every MergeAction variant.

4. **VFS arena allocation** (`vfs/mod.rs`): The arena + HashMap design is
   cache-friendly and provides O(1) path lookup. The reparent implementation
   correctly handles cycle detection, path index updates, and orphan cleanup.

5. **Generation delta compression** (`generation/delta.rs`): The zstd
   dictionary approach is well-suited for EROFS metadata-only images. The
   MAX_OUTPUT_SIZE guard prevents decompression bombs, and the roundtrip
   tests verify byte-for-byte correctness.

6. **fsverity integration** (`fsverity.rs`): The kernel ioctl struct is
   validated with a compile-time size assertion, EEXIST is handled gracefully,
   and the `enable_fsverity_on_cas` function delegates to the shared
   `CasStore::iter_objects()` iterator.

---

### Recommendations

1. **Fix the generation number race (P0)**: Wrap `build_generation_from_db` in
   a database transaction, or pass the reserved state number explicitly to
   `create_snapshot()`. This is the highest-priority fix as it can cause
   generation/CAS mismatch leading to data loss during GC.

2. **Wire up the `digest` field in MountOptions (P1)**: Without the `digest=`
   mount option, fsverity provides no image integrity guarantee. Either
   implement it or remove the field and the `verity` flag to avoid giving
   callers a false sense of security.

3. **Standardize on `crate::Result` throughout `generation/` (P1)**: Convert
   `composefs.rs` and `metadata.rs` from `anyhow::Result` to `crate::Result`.
   This enables structured error matching at call sites and aligns with the
   project convention.

---

### Assessment

**Ready to merge?** No -- with fixes for the P0 and P1 items.

**Reasoning:** The generation number race (P0) can cause CAS data loss in
concurrent scenarios, and the `etc_merge` symlink-following fallback (P0)
is a security issue. The unused `digest` field (P1) means fsverity
enforcement is not actually working. Once these are addressed, the module
is solid.

---

### Work Breakdown

1. **[P0] Fix generation number race in `build_generation_from_db`**
   - Files: `conary-core/src/generation/builder.rs`, `conary-core/src/db/models/state.rs`
   - Tasks: Add explicit state number parameter to `create_snapshot()` or wrap in transaction
   - Test: Add concurrent build test that exercises the race

2. **[P0] Fix symlink-following in `etc_merge` fallback path**
   - Files: `conary-core/src/generation/etc_merge.rs`
   - Tasks: Replace `is_file()` with `symlink_metadata().is_ok_and(|m| m.is_file())`
   - Test: Add test with symlink in upper directory pointing outside overlay

3. **[P1] Wire up `MountOptions.digest` field**
   - Files: `conary-core/src/generation/mount.rs`
   - Tasks: Add `digest=` to `to_mount_args()` when present
   - Test: Add unit test verifying digest appears in mount args

4. **[P1] Convert `composefs.rs` and `metadata.rs` to `crate::Result`**
   - Files: `conary-core/src/generation/composefs.rs`, `conary-core/src/generation/metadata.rs`
   - Tasks: Replace `anyhow::Result` with `crate::Result`, add error variants if needed

5. **[P1] Handle overlayfs whiteout files in `etc_merge`**
   - Files: `conary-core/src/generation/etc_merge.rs`
   - Tasks: Detect `.wh.*` whiteout entries in scan, classify as user deletion
   - Test: Add test with whiteout file in upper directory

6. **[P2] Make `GenerationMetadata::write_to()` crash-safe**
   - Files: `conary-core/src/generation/metadata.rs`
   - Tasks: Use temp + fsync + rename pattern

7. **[P2] Extract shared zstd dictionary compression utility**
   - Files: `conary-core/src/generation/delta.rs`, `conary-core/src/delta/generator.rs`, `conary-core/src/delta/applier.rs`
   - Tasks: Create shared `compression::dict` module, update both callers

8. **[P2] Use streaming hash in `hardlink_from_existing()`**
   - Files: `conary-core/src/filesystem/cas.rs`
   - Tasks: Replace `fs::read()` with `hash_reader()` + `BufReader`

9. **[P3] Add null byte check to `sanitize_filename()`**
   - Files: `conary-core/src/filesystem/path.rs`

10. **[P3] Clean up dead binding in `accept_package_paths()`**
    - Files: `conary-core/src/generation/etc_merge.rs`
