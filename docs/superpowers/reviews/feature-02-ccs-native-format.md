## Feature 2: CCS Native Format -- Review Findings

### Summary

The CCS module (37 files, ~6,500 SLOC) is well-structured with clean module boundaries, good test coverage, and solid security fundamentals. The crypto path uses Ed25519 correctly with proper trust policy enforcement. The most significant findings are: (1) a TOCTOU race in `ChunkStore::store_chunk` that can silently corrupt data, (2) the signature verification uses `verify()` instead of `verify_strict()` which accepts malleable signatures, (3) hook execution runs arbitrary scripts via `/bin/sh -c` without sanitization of the script content, and (4) several `as` casts that silently truncate on 32-bit platforms.

### P0 -- Critical

#### 1. ChunkStore::store_chunk has TOCTOU race leading to silent data corruption
- **File:** `conary-core/src/ccs/chunking.rs:202-222`
- **Category:** Correctness / Security
- **Finding:** `store_chunk()` checks `if path.exists()` then writes to a temp file and renames. Two concurrent processes storing different content for the same hash (collision or bug) race between the existence check and the rename. The temp file name `path.with_extension("tmp")` is deterministic, so concurrent stores to the same chunk overwrite each other's temp file. Compare with CAS `atomic_store` which uses PID+counter for temp names.
- **Fix:** Use a unique temp file name (e.g., `tempfile::NamedTempFile` in the same directory) and rename atomically. The `exists()` check is fine as an optimization but the temp file collision is the real bug.

#### 2. Signature verification uses verify() instead of verify_strict()
- **File:** `conary-core/src/ccs/verify.rs:284-288`
- **Category:** Security (Crypto)
- **Finding:** `verifying_key.verify(manifest_raw, &signature)` uses the non-strict verification that accepts malleable signatures (signatures where the S component is not reduced). The signing code in `signing.rs:187` test uses `verify_strict` but the production verification path does not. Ed25519 signature malleability allows an attacker to produce a different valid signature for the same message without knowing the private key, which could bypass deduplication or audit logging that keys on signature bytes.
- **Fix:** Change `verifying_key.verify(manifest_raw, &signature)` to `verifying_key.verify_strict(manifest_raw, &signature)` in `verify_signature()`.

#### 3. Hook execute_script runs arbitrary shell content without sanitization
- **File:** `conary-core/src/ccs/hooks/mod.rs:365-392`
- **Category:** Security
- **Finding:** `execute_script()` takes a `script: &str` from the package manifest and passes it directly to `/bin/sh -c`. The `post_install` and `pre_remove` ScriptHook fields in the TOML manifest are freeform strings. A malicious package can embed any shell command. When `root != "/"`, chroot is used which provides some isolation, but on live root this is command injection from package metadata. There is no sandboxing, no allowlisting, and no user confirmation.
- **Fix:** At minimum, warn the user before executing script hooks (like RPM's `%pretrans` prompts). Better: run script hooks inside the namespace sandbox (`crate::container::Sandbox`) the same way `capture.rs` does. Long-term: the declarative hooks exist precisely to avoid this -- consider rejecting `post_install`/`pre_remove` script hooks from untrusted sources.

### P1 -- Important

#### 4. Silent truncation via `as` casts on 32-bit platforms
- **File:** `conary-core/src/ccs/package.rs:446`, `conary-core/src/ccs/chunking.rs:155`
- **Category:** Correctness
- **Finding:** `Vec::with_capacity(file.size as usize)` where `file.size` is `u64`. On a 32-bit platform, a file > 4GB silently truncates the capacity to a wrong value. Similarly in `chunk_file()`: `Vec::with_capacity(size as usize)`. While Conary targets 64-bit Linux, these are latent bugs.
- **Fix:** Use `usize::try_from(file.size).unwrap_or(usize::MAX)` or gate on `#[cfg(target_pointer_width = "64")]`.

#### 5. package.rs convert_files: `file.size as i64` and `file.mode as i32` can overflow
- **File:** `conary-core/src/ccs/package.rs:304,306`
- **Category:** Correctness
- **Finding:** `PackageFile.size` is `i64` and `PackageFile.mode` is `i32`. Casting `u64` to `i64` overflows for files > 8 EiB (theoretical) and casting `u32` to `i32` overflows for modes with high bits set (e.g., setuid 04755 = 2541 decimal, which fits, but raw `st_mode` can include file type bits above 0o7777 which would overflow). Similar casts appear at lines 486-487 in `extract_file_contents`.
- **Fix:** Use `i64::try_from(file.size).expect("file size exceeds i64")` or mask mode to `file.mode & 0o7777`.

#### 6. verify_content_hashes uses size==0 && hash.is_empty() to detect directories
- **File:** `conary-core/src/ccs/verify.rs:349`
- **Category:** Correctness
- **Finding:** The directory detection heuristic `file.size == 0 && file.hash.is_empty()` will also match genuinely empty regular files that happen to have an empty hash string (which shouldn't happen normally but would be a parsing bug). The `FileEntry` has an explicit `file_type` field -- use it instead of the heuristic.
- **Fix:** Change `if file.size == 0 && file.hash.is_empty()` to `if file.file_type == FileType::Directory`.

#### 7. parse_octal_mode strips leading '0' ambiguously for "0" input
- **File:** `conary-core/src/ccs/manifest.rs:792-798`
- **Category:** Correctness
- **Finding:** For input `"0"`, `strip_prefix('0')` produces `""`, then `u32::from_str_radix("", 8)` returns an error. The function is used in `builder.rs` for directory hooks and tmpfiles hooks. The `directory.rs` hook has its own mode parsing that adds `let mode_str = if mode_str.is_empty() { "0" } else { mode_str };` to handle this, but `parse_octal_mode` in manifest.rs does not, creating an inconsistency. The test in `directory.rs:113` passes because it uses the directory.rs parser, not `parse_octal_mode`.
- **Fix:** Add the same empty-string guard to `parse_octal_mode`: `let mode_str = if mode_str.is_empty() { "0" } else { mode_str };`.

#### 8. Chunking uses hex::encode of raw hash but builder expects hex string from hash::sha256
- **File:** `conary-core/src/ccs/builder.rs:251`
- **Category:** Correctness
- **Finding:** In the chunking path, chunk hashes are computed as `hex::encode(chunk.hash)` where `chunk.hash` is `[u8; 32]` from `crate::hash::sha256_bytes`. The non-chunked path uses `hash::sha256()` which returns a hex string. Both should produce identical hex strings for the same input, but this relies on `hex::encode` and `hash::sha256` using the same hex encoding (lowercase). If either ever changed casing, chunk verification would break silently. This is a fragile coupling.
- **Fix:** Use a single function for both paths. Either always use `hash::sha256()` on `&chunk.data` or always use `hex::encode(hash::sha256_bytes(...))`. Document the lowercase hex invariant.

#### 9. OCI export creates non-reproducible images due to `chrono::Utc::now()`
- **File:** `conary-core/src/ccs/export/oci.rs:428`
- **Category:** Quality
- **Finding:** `create_config()` embeds `chrono::Utc::now()` in the OCI config `created` field, making the image non-reproducible. The layer itself uses `LAYER_MTIME` for reproducibility, but the config and history break it. The config hash changes on every build, which defeats content-addressable dedup of images.
- **Fix:** Use `SOURCE_DATE_EPOCH` (like the builder does) or `LAYER_MTIME` for the config timestamp when reproducibility is desired.

#### 10. sysctl only_if_lower comment says "not enforced" for target root but it is silently written
- **File:** `conary-core/src/ccs/hooks/sysctl.rs:60-67`
- **Category:** Correctness
- **Finding:** When writing sysctl config for target root with `only_if_lower: true`, the config file just writes `key=value` with a comment saying "Only apply if current value is lower", but the sysctl.d config format doesn't support conditional application. On first boot, the value will be applied unconditionally, ignoring the `only_if_lower` semantics. The comment is misleading -- it looks like documentation for a human, not a machine-readable directive.
- **Fix:** Either don't write `only_if_lower` entries to sysctl.d (they can't be expressed), or generate a systemd-tmpfiles or oneshot service that does the comparison at boot. At minimum, add a warning when `only_if_lower` is used with a non-live root.

### P2 -- Improvement

#### 11. archive_reader doesn't verify object path hex length before concatenating
- **File:** `conary-core/src/ccs/archive_reader.rs:158-168`
- **Category:** Quality
- **Finding:** The object path parsing validates each component is hex but doesn't verify the concatenated hash is exactly 64 characters (SHA-256). A malformed archive could inject short or overly long hash strings into the blobs map, which would be silently accepted.
- **Fix:** Add `if hash.len() != 64 { warn!(...); continue; }` after the format concatenation.

#### 12. CAS path computation duplication in builder.rs
- **File:** `conary-core/src/ccs/builder.rs:549-553`
- **Category:** Quality
- **Finding:** `hash.split_at(2)` for the two-level CAS directory structure is duplicated in builder.rs, chunking.rs (Chunk::cas_path), and archive_reader.rs. Per the memory anti-patterns, this should use `CasStore::hash_to_path()` or a shared free function.
- **Fix:** Extract to a shared `crate::filesystem::cas_path(hash: &str) -> PathBuf` or similar.

#### 13. Policy chain Replace action carries empty Vec
- **File:** `conary-core/src/ccs/policy.rs:170-174`
- **Category:** Quality
- **Finding:** When the policy chain signals content was replaced, it returns `PolicyAction::Replace(Vec::new())` -- an empty vector. The caller in `builder.rs:226-234` pattern-matches on `PolicyAction::Replace(_)` to know content changed, ignoring the inner value. This is a type-level lie: `Replace` carries a `Vec<u8>` that means "new content" everywhere else, but here it's meaningless. It's also confusing for future maintainers.
- **Fix:** Introduce `PolicyAction::Modified` (no payload) or change `Replace` to not carry data and have the chain return `(action, content)` tuple (which it already does -- the data is in `current_content`).

#### 14. Redundant file reads during OCI layer creation
- **File:** `conary-core/src/ccs/export/oci.rs:233-234`
- **Category:** Quality
- **Finding:** `sha256_hex_uncompressed` creates the entire uncompressed tar in memory just to hash it, after `create_layer_tarball` already created the compressed version in memory. This means the tar is built twice. For large packages this doubles memory usage and build time.
- **Fix:** Hash the uncompressed tar during `create_layer_tarball` by teeing the uncompressed stream to a hasher before compression.

#### 15. Manifest validation is minimal
- **File:** `conary-core/src/ccs/manifest.rs:93-101`
- **Category:** Quality
- **Finding:** `validate()` only checks that name and version are non-empty. It doesn't validate: name contains only valid characters (no path separators, no null bytes), version is a reasonable format, hook paths are absolute, mode strings are valid octal, etc. Invalid data passes through and causes errors later in the pipeline.
- **Fix:** Add validation for package name format (alphanumeric + hyphens), hook path absoluteness, and mode string validity at parse time.

#### 16. Chunker reads entire file into memory
- **File:** `conary-core/src/ccs/chunking.rs:153-157`
- **Category:** Quality
- **Finding:** `chunk_file()` reads the entire file into memory with `read_to_end`. The comment acknowledges this: "For files up to a few hundred MB this is fine. For very large files, we'd want a streaming approach." Since CCS packages could contain large database files or firmware blobs, this is a practical concern.
- **Fix:** Use FastCDC's streaming mode or memory-map the file. Not urgent but worth a TODO.

#### 17. convert/converter.rs write_files_to_temp doesn't use safe_join
- **File:** `conary-core/src/ccs/convert/converter.rs:418`
- **Category:** Security
- **Finding:** `temp_dir.join(rel_path)` where `rel_path` comes from `file.path.strip_prefix('/')`. If a legacy package contains a file with `../` in its path (a known attack vector in archives), this could write outside the temp directory. The `strip_prefix('/')` helps but doesn't prevent `../../etc/shadow` style paths.
- **Fix:** Use `crate::filesystem::path::safe_join(temp_dir, rel_path)` per the codebase convention documented in the memory anti-patterns.

### P3 -- Nitpick

#### 18. signing.rs uses anyhow::Result but other modules use thiserror
- **File:** `conary-core/src/ccs/signing.rs:8`
- **Category:** Architecture
- **Finding:** `signing.rs` and `verify.rs` use `anyhow::Result` for their public APIs while most of the CCS module uses `thiserror` for typed errors. This makes it harder for callers to match on specific error types from signing operations.
- **Fix:** Consider a `SigningError` thiserror enum for consistency. Low priority since the signing API is small.

#### 19. DependencyKind has manual Display impl instead of using strum or similar
- **File:** `conary-core/src/ccs/lockfile.rs:176-185`
- **Category:** Quality
- **Finding:** `DependencyKind` has a manual `Display` impl that duplicates the `#[serde(rename_all = "lowercase")]` strings. If someone adds a variant and forgets to update Display, the formats diverge.
- **Fix:** Minor; consider deriving Display from the serde rename or using `strum::Display`.

#### 20. EnhancementResult_ naming with trailing underscore
- **File:** `conary-core/src/ccs/enhancement/mod.rs:164`
- **Category:** Quality
- **Finding:** The type `EnhancementResult_` has a trailing underscore to avoid conflicting with the `EnhancementResult` type alias for `Result<T, EnhancementError>`. This is awkward naming. The convention in Rust is to give the alias a different name.
- **Fix:** Rename the alias to `EnhResult` or the struct to `EnhancementOutcome`.

#### 21. mock.rs log format vulnerable to argument injection
- **File:** `conary-core/src/ccs/convert/mock.rs:71-74`
- **Category:** Security
- **Finding:** The mock tool script uses `echo "CALL:{name} $@" >> {log_file}` where `$@` is unquoted. If a scriptlet argument contains a newline, the parse_capture_log parser could be confused. This is inside a sandbox so the risk is low, but the log parsing is fragile.
- **Fix:** Quote `$@` or use a structured format (JSON per line).

#### 22. legacy/mod.rs sysctl_commands is_safe check is stricter than sysctl.rs validate_sysctl_value
- **File:** `conary-core/src/ccs/legacy/mod.rs:208-214`
- **Category:** Quality
- **Finding:** `sysctl_commands()` validates keys/values with `is_safe()` which rejects slashes (valid in sysctl keys like `net/ipv4/ip_forward`). But `sysctl.rs::validate_sysctl_key` accepts slashes. This means a key with slashes passes the hooks module but gets shell-escaped in the legacy generator, potentially changing behavior.
- **Fix:** Align `is_safe` to include `/` for keys, matching `validate_sysctl_key`.

### Cross-Domain Notes

- **CAS path duplication** (finding 12): The `hash[..2]/hash[2..]` pattern in `builder.rs:549-553` and `chunking.rs:47-49` duplicates logic from `filesystem/cas.rs`. This is noted in the lintian memory as a known anti-pattern. The fix belongs to a shared utility module, not this feature alone.

- **converter.rs path joining** (finding 17): Uses bare `Path::join` instead of `safe_join()` from `crate::filesystem::path`. This is a cross-domain concern (filesystem module owns the safe join logic).

### Strengths

- **Ed25519 trust model is sound** (`verify.rs:290-306`): Self-signed packages correctly return `Untrusted` when no trusted keys are configured. The test suite at lines 628-690 explicitly covers this attack scenario.

- **Archive reader has proper size guards** (`archive_reader.rs:20-29`): Per-entry (512 MB), cumulative (4 GB), manifest (16 MiB), and component (64 MiB) size limits prevent zip bomb style attacks.

- **Hook input validation** (`alternatives.rs:14-44`, `sysctl.rs:15-37`): Alternative names and paths are validated for injection characters before being passed to system commands. Sysctl keys and values are validated for newlines and special characters.

- **CDC implementation is clean** (`chunking.rs`): The FastCDC wrapper is well-tested with property-based assertions (content stability, minimal chunk drift on small changes). The delta calculation is correct.

- **Policy chain architecture** (`policy.rs`): The trait-based policy system with `apply()` returning an enum is a clean design. The `StripBinariesPolicy` correctly handles both system `strip` and native fallback, and silently degrades rather than failing the build.

- **Conversion fidelity tracking** (`fidelity.rs`): The fidelity level system gives users clear visibility into what was lost during legacy package conversion, with severity-based degradation.

### Assessment

**Ready to merge?** With fixes

**Reasoning:** P0 items 1 (chunk store TOCTOU), 2 (verify_strict), and 3 (unsandboxed script hooks) need fixing before this code handles untrusted packages. The chunk store race can cause silent data corruption under concurrent writes. The malleable signature issue weakens the crypto model. The script hook execution is the most concerning for production -- any CCS package with `post_install` or `pre_remove` script hooks gets arbitrary code execution on live root with no sandboxing.

### Work Breakdown

1. **[P0] Fix ChunkStore::store_chunk TOCTOU** -- Use unique temp file names (tempfile crate) in `chunking.rs:214-220`. ~15 min.

2. **[P0] Use verify_strict for Ed25519** -- Single line change in `verify.rs:284`. Add test for malleable signature rejection. ~10 min.

3. **[P0] Sandbox or warn for script hooks** -- In `hooks/mod.rs:execute_script`, add user confirmation or sandbox execution via `crate::container::Sandbox`. This is a design decision. ~2 hours for sandbox approach, ~30 min for warning.

4. **[P1] Fix parse_octal_mode for "0" input** -- Add empty-string guard in `manifest.rs:796`. ~5 min.

5. **[P1] Fix directory detection in verify_content_hashes** -- Use `file.file_type` instead of heuristic in `verify.rs:349`. ~5 min.

6. **[P1] Guard `as` casts for 32-bit safety** -- Fix casts in `package.rs:304,306,446,486-487` and `chunking.rs:155`. ~15 min.

7. **[P1] Use safe_join in converter write_files_to_temp** -- Replace `temp_dir.join(rel_path)` with `safe_join` in `converter.rs:419`. ~5 min.

8. **[P2] Validate object path hash length in archive_reader** -- Add `hash.len() == 64` check in `archive_reader.rs:165`. ~5 min.

9. **[P2] Extract shared CAS path function** -- Deduplicate `hash[..2]/hash[2..]` across builder.rs, chunking.rs, archive_reader.rs. ~30 min.

10. **[P2] Fix sysctl only_if_lower for target root** -- Either skip writing or generate a boot-time oneshot service. ~30 min.

11. **[P2] Make OCI export reproducible** -- Use `SOURCE_DATE_EPOCH` or fixed timestamp for config. ~15 min.
