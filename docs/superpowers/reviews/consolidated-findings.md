# Consolidated Code Review Findings

## Statistics
- Total findings: 155
- By severity: P0=16, P1=39, P2=60, P3=40
- By category: Correctness=68, Security=26, Quality=34, Architecture=12, Slop=5, Idiomatic=1, Performance=3, Convention=6

## Cross-Cutting Patterns

### XC-1: CAS path computation duplicated across the codebase
The `hash[..2]/hash[2..]` two-level CAS directory path derivation is reimplemented in 8+ locations instead of using a shared function like `CasStore::hash_to_path()`.
- `conary-core/src/ccs/builder.rs:549-553`
- `conary-core/src/ccs/chunking.rs:47-49`
- `conary-core/src/ccs/archive_reader.rs:158-168`
- `conary-core/src/derivation/install.rs:222-229`
- `conary-server/src/server/cache.rs:87`
- `conary-server/src/server/chunk_gc.rs:130`
- `conary-server/src/server/conversion.rs:517`
- `conary-server/src/server/handlers/mod.rs:36`
- **Fix:** Extract a shared `crate::filesystem::cas::object_path(root, hash)` and call it everywhere.

### XC-2: Whole-file reads for hashing instead of streaming I/O
Multiple modules read entire files into memory to compute checksums, risking OOM on large files. The codebase has `hash::hash_reader` for streaming but it is not used consistently.
- `conary-core/src/packages/deb.rs:155-199` (DEB data.tar, 2GB guard)
- `conary-core/src/recipe/kitchen/archive.rs:49` (source archive checksum)
- `conary-core/src/recipe/cache.rs:418` (CCS cache integrity)
- `conary-core/src/trust/verify.rs:222` (TUF target file verification)
- `conary-core/src/filesystem/cas.rs:401` (hardlink_from_existing hashing)
- `conary-server/src/server/conversion.rs:663` (recipe build output)
- `conary-core/src/ccs/chunking.rs:153-157` (chunk_file reads whole file)
- **Fix:** Replace `fs::read()` / `read_to_end()` with `BufReader` + `hash::hash_reader` pattern already used in `compose.rs:erofs_image_hash()`.

### XC-3: `expect()` calls on production paths that should propagate errors
Multiple modules use `.expect()` in non-test code paths where `Result` propagation is available.
- `conary-core/src/resolver/provider/mod.rs:122,141,165,176,186,194` (pool u32 overflow)
- `conary-core/src/provenance/mod.rs:79-80` (dna_hash hex decode)
- `conary-core/src/recipe/kitchen/archive.rs:76-77,119` (UTF-8 path expect)
- `src/commands/progress.rs:39,49,74,135,193,202,234,281,289,347,391,400,424` (progress bar templates)
- `src/commands/repo.rs:204` (progress bar template)
- **Fix:** Replace `expect()` with `.map_err()?` in library code; centralize template creation in CLI helpers.

### XC-4: `anyhow::Result` used instead of `crate::Result` with `thiserror`
Several modules break the project convention of typed errors via `thiserror`, making it impossible for callers to match on specific error variants.
- `conary-core/src/ccs/signing.rs:8`
- `conary-core/src/ccs/verify.rs`
- `conary-core/src/generation/composefs.rs:11`
- `conary-core/src/generation/metadata.rs:8`
- **Fix:** Replace `anyhow::Result` with `crate::Result` and add domain-specific error variants.

### XC-5: Direct `sha2` imports bypassing the project's `hash` module
Server-side code imports `sha2::{Digest, Sha256}` directly instead of using the `conary_core::hash` abstraction, fragmenting hashing logic.
- `conary-server/src/server/conversion.rs:18`
- `conary-server/src/server/handlers/derivations.rs:24`
- `conary-server/src/server/handlers/profiles.rs:19`
- `conary-server/src/server/handlers/admin/packages.rs:14`
- **Fix:** Replace with `conary_core::hash::sha256()` or `conary_core::hash::Hasher`.

### XC-6: Stale numbers across documentation (version, line count, test count, schema version)
Multiple documentation files contain numbers that have drifted from reality.
- `README.md:5` -- version badge says v0.6.0, Cargo.toml is v0.7.0
- `README.md:58,528` -- claims schema v56, actual is v57
- `README.md:9,58` -- claims 174K+ lines, actual is 211K
- `README.md:513` -- claims ~260 unit tests
- `CLAUDE.md:8` -- claims ~269 unit tests, actual is ~2,600
- `deploy/FORGE.md:36` -- claims 37-test suite
- **Fix:** Single pass to update all numbers to current values.

### XC-7: Unimplemented features that silently succeed instead of returning errors
Commands and config fields that are accepted but do nothing, misleading users.
- `src/commands/self_update.rs:40-43` -- `--version` flag ignored, installs latest
- `src/main.rs:1654` -- daemon `--foreground` flag silently discarded
- `src/commands/state.rs:217` -- `state revert` prints plan but never applies, returns Ok
- `src/commands/automation.rs:491-517` -- `automation history` always says "no history"
- `src/cli/mod.rs:605-607` -- `Export --oci` flag always true, no alternative
- `conary-server/src/federation/config.rs:270` -- `allowed_peers` parsed but never enforced
- `conary-server/src/daemon/mod.rs:640-650` -- TCP listener bound but never accepts
- **Fix:** Either implement the feature or return an error/hide the flag.

### XC-8: `serde_json::Display` / manual Display impls duplicating serde rename strings
Multiple types have manual `Display` impls that duplicate `#[serde(rename_all)]` strings, risking divergence.
- `conary-core/src/ccs/lockfile.rs:176-185` (DependencyKind)
- `conary-core/src/provenance/signature.rs:190-201` (SignatureScope)
- **Fix:** Use `strum::Display` derive or add tests asserting Display matches serde output.

### XC-9: Unsafe `as` casts for integer conversions
Multiple locations use `as` casts that can silently truncate or wrap, particularly `u64 as usize` on 32-bit and `i64 as u64` for potentially negative values.
- `conary-core/src/ccs/package.rs:304,306,446,486-487` (size/mode casts)
- `conary-core/src/ccs/chunking.rs:155` (Vec capacity)
- `conary-server/src/server/conversion.rs:607` (i64 total_size as u64)
- `conary-test/src/server/wal.rs:66` (COUNT i64 as u64)
- `conary-test/src/server/wal.rs:134` (usize as u64)
- `conary-test/src/server/state.rs` / `remi_client.rs` (u64 vs i64 run_id)
- **Fix:** Use `TryFrom` with proper error handling or `unwrap_or(0)`.

### XC-10: Unbounded in-memory collections that grow without limit
Several caches and maps lack size bounds, enabling memory exhaustion under sustained load.
- `conary-server/src/server/auth.rs:31` (TOUCH_CACHE HashMap)
- `conary-server/src/server/negative_cache.rs:27` (HashMap no max capacity)
- `conary-server/src/server/security.rs:22` (RateLimiter HashMap)
- **Fix:** Add max-capacity limits, use LRU eviction, or migrate to `governor`.

---

## P0 -- Critical

### P0-1. [Security] Hardcoded authentication token in tracked service file
- **Domain:** Feature 0 (Repo Presentation)
- **File:** `deploy/conary-test.service:9,11`
- **Finding:** Bearer token `d7975d...` hardcoded in a git-tracked systemd service file. Will be public when posted.
- **Fix:** Move to `EnvironmentFile=` pointing to an untracked path, rotate the token immediately.

### P0-2. [Correctness] RpmVersion::parse fallback used for non-RPM version comparison
- **Domain:** Feature 1 (Package Management Core)
- **File:** `conary-core/src/version/mod.rs:45`, `conary-core/src/resolver/graph.rs:84`
- **Finding:** `RpmVersion::parse` splits on first hyphen. When used as the fallback parser for non-RPM versions (Debian, Arch) via `satisfies_legacy`, multi-hyphen versions produce wrong epoch/version/release decomposition and version ordering breaks silently.
- **Fix:** Guard `satisfies_legacy` so it is never called for non-RPM schemes, or document `RpmVersion::parse` as RPM-only with a compile-time guard.

### P0-3. [Correctness] Resolver pool `expect()` panics on u32 overflow in production path
- **Domain:** Feature 1 (Package Management Core)
- **File:** `conary-core/src/resolver/provider/mod.rs:122,141,165,176,186,194`
- **Finding:** Six `u32::try_from(...).expect("pool overflow")` calls in production code paths. Pathological repository data causes unrecoverable crash.
- **Fix:** Replace `expect` with `map_err(|_| Error::...)` and propagate. All callers already return `Result`.

### P0-4. [Correctness/Security] ChunkStore::store_chunk TOCTOU race leading to silent data corruption
- **Domain:** Feature 2 (CCS Native Format)
- **File:** `conary-core/src/ccs/chunking.rs:202-222`
- **Finding:** Deterministic temp file name `path.with_extension("tmp")` means concurrent stores to the same chunk overwrite each other's temp file.
- **Fix:** Use unique temp file names (e.g., `tempfile::NamedTempFile` in the same directory) and rename atomically.

### P0-5. [Security] Signature verification uses `verify()` instead of `verify_strict()`
- **Domain:** Feature 2 (CCS Native Format)
- **File:** `conary-core/src/ccs/verify.rs:284-288`
- **Finding:** Production verification path uses non-strict Ed25519 verification that accepts malleable signatures. The signing test code uses `verify_strict` but production does not.
- **Fix:** Change to `verifying_key.verify_strict(manifest_raw, &signature)`.

### P0-6. [Security] Hook execute_script runs arbitrary shell content without sanitization
- **Domain:** Feature 2 (CCS Native Format)
- **File:** `conary-core/src/ccs/hooks/mod.rs:365-392`
- **Finding:** `execute_script()` passes package manifest `post_install`/`pre_remove` scripts directly to `/bin/sh -c` with no sandboxing, allowlisting, or user confirmation on live root.
- **Fix:** At minimum warn the user; better: run inside `crate::container::Sandbox`; best: reject script hooks from untrusted sources.

### P0-7. [Correctness] Generation number race between reservation and state creation
- **Domain:** Feature 3 (Filesystem & Generations)
- **File:** `conary-core/src/generation/builder.rs:346-397`
- **Finding:** `next_state_number()` called twice without a wrapping transaction. Concurrent state creation causes generation number mismatch; GC could delete CAS objects the "real" generation still needs.
- **Fix:** Wrap `build_generation_from_db` in a database transaction, or have `create_snapshot()` accept an explicit state number.

### P0-8. [Security] `etc_merge` fallback follows symlinks via `is_file()`
- **Domain:** Feature 3 (Filesystem & Generations)
- **File:** `conary-core/src/generation/etc_merge.rs:302-304`
- **Finding:** `upper_file_hash()` uses `abs_path.is_file()` which follows symlinks. A crafted symlink in the overlay upper directory could cause hash computation of files outside the overlay boundary.
- **Fix:** Replace with `abs_path.symlink_metadata().is_ok_and(|m| m.is_file())`.

### P0-9. [Security] Chroot build path does not escape workdir in command string
- **Domain:** Feature 4 (Source Building)
- **File:** `conary-core/src/recipe/kitchen/cook.rs:509-513`
- **Finding:** `run_build_step_direct` interpolates `chroot_workdir` into shell command without escaping. Shell metacharacters in workdir enable command injection as root inside chroot.
- **Fix:** Apply same single-quote escaping used in `run_build_step_isolated` (line 455).

### P0-10. [Security] PKGBUILD checksum algorithm mismatch -- non-sha256 silently labeled sha256
- **Domain:** Feature 4 (Source Building)
- **File:** `conary-core/src/recipe/pkgbuild.rs:130-147`
- **Finding:** Regardless of which checksum array was matched (sha512sums, b2sums, md5sums), the value is always prefixed with `"sha256:"`. Non-sha256 PKGBUILDs produce recipes that can never build.
- **Fix:** Track which algorithm was found and prefix appropriately, or reject non-sha256 with a warning.

### P0-11. [Security] Self-update signature verification unconditionally bypassed
- **Domain:** Feature 6 (Cross-Distro & Extensibility)
- **File:** `conary-core/src/self_update.rs:79`
- **Finding:** `TRUSTED_UPDATE_KEYS` is empty (`&[]`), so production verification always fails. `cfg!(test)` bypass accepts any payload in test builds. The entire signature verification apparatus provides zero protection. SHA-256 checksum is the only integrity check; a compromised CDN could serve a matching pair.
- **Fix:** Add release signing public key to `TRUSTED_UPDATE_KEYS`, remove `cfg!(test)` bypass, require signature when keys exist.

### P0-12. [Correctness] Self-update `--version` flag silently ignored -- installs latest
- **Domain:** Feature 7 (CLI Layer)
- **File:** `src/commands/self_update.rs:40-43`
- **Finding:** User passes `--version 0.5.0`, command prints `[NOT YET IMPLEMENTED]` warning but proceeds to install the latest version. Silent data mutation -- binary replaced with unintended version.
- **Fix:** Bail with error when `--version` is specified, or hide the flag.

### P0-13. [Security] Federation `allowed_peers` config parsed but never enforced
- **Domain:** Feature 9 (Daemon & Federation)
- **File:** `conary-server/src/federation/config.rs:270`
- **Finding:** `allowed_peers: Option<Vec<String>>` is deserialized but never checked in any fetch path. Operators who set this field have a false sense of security.
- **Fix:** Enforce in `Federation::new()` and `start_mdns_discovery()`, or remove the field.

### P0-14. [Security] mDNS discovery trusts any peer on the LAN without verification
- **Domain:** Feature 9 (Daemon & Federation)
- **File:** `conary-server/src/federation/mdns.rs:306-337`, `conary-server/src/federation/mod.rs:319-337`
- **Finding:** Any LAN device can announce `_conary-cas._tcp.local.` and get auto-added to peer registry. Self-reported `tier` in TXT record is trusted. Enables DoS, traffic interception, and resource exhaustion.
- **Fix:** Check allowlists before adding discovered peers; use `select_peers_hierarchical_filtered()` in `fetch_chunk_inner()`; consider shared secret in mDNS TXT records.

### P0-15. [Correctness] Enhancement background worker blocks async executor with sync DB call
- **Domain:** Feature 9 (Daemon & Federation)
- **File:** `conary-server/src/daemon/enhance.rs:301`
- **Finding:** `enhancement_background_worker()` calls `state.open_db()` directly on the async executor, blocking the tokio thread. Loops forever, repeatedly blocking.
- **Fix:** Wrap in `tokio::task::spawn_blocking`.

### P0-16. [Correctness] conary-test crate does not compile -- missing module declarations and type conversions
- **Domain:** Feature 10 (Test Infrastructure)
- **Files:** `conary-test/src/engine/mod.rs:1-10`, `conary-test/src/container/mod.rs:1-12`, `conary-test/src/server/handlers.rs:57,98,183,222`, `conary-test/src/server/mcp.rs:548`
- **Finding:** Missing `pub(crate) mod container_setup;` in engine/mod.rs, missing `#[cfg(test)] pub(crate) mod mock;` in container/mod.rs, missing `From<ConaryTestError> for StructuredError` impl, and `anyhow_to_mcp` type mismatches in 5 MCP call sites. Total: 10 compilation errors.
- **Fix:** Add module declarations, implement type conversions, fix map_err call sites.

---

## P1 -- Important

### P1-1. [Architecture] Column-index coupling in `from_row` implementations across 15+ model files
- **Domain:** Feature 1 (Package Management Core)
- **Files:** `conary-core/src/db/models/trove.rs:329-376`, `repository.rs:201-222`, `file_entry.rs:289-302`, `changeset.rs:153-170`
- **Finding:** COLUMNS constant and `from_row` ordinal indexing are manually synchronized. Adding a column without updating ordinals causes silent data misread.
- **Fix:** Add compile-time column-count assertions per model or use a derive macro.

### P1-2. [Security] `find_by_reason` uses user-controlled glob pattern in SQL LIKE
- **Domain:** Feature 1 (Package Management Core)
- **File:** `conary-core/src/db/models/trove.rs:452-464`
- **Finding:** `%` and `_` in input become unescaped SQL wildcards. Currently only called with hardcoded patterns, but API is `pub`.
- **Fix:** Mark `pub(crate)` or escape `%` and `_` before the `*` -> `%` conversion.

### P1-3. [Security] `RepositoryPackage::search` uses unescaped LIKE pattern from user input
- **Domain:** Feature 1 (Package Management Core)
- **File:** `conary-core/src/db/models/repository.rs:381-394`
- **Finding:** User input wrapped in `%...%` without escaping SQL wildcards.
- **Fix:** Escape `%` and `_` in pattern input before wrapping.

### P1-4. [Correctness] `parse_timestamp` silently clamps negative Unix timestamps to 0
- **Domain:** Feature 1 (Package Management Core)
- **File:** `conary-core/src/repository/sync.rs:41`
- **Finding:** `u64::try_from(dt.timestamp()).unwrap_or(0)` causes repository to always appear stale for pre-1970 timestamps.
- **Fix:** Return an error for negative timestamps.

### P1-5. [Correctness] `RetryConfig::with_retry` panics if `max_attempts` is 0
- **Domain:** Feature 1 (Package Management Core)
- **File:** `conary-core/src/repository/retry.rs:120,150`
- **Finding:** `last_err.expect("max_attempts must be >= 1")` panics when `max_attempts` is 0 and the loop body never executes.
- **Fix:** Clamp `max_attempts` to `max(1, ...)` in the constructor.

### P1-6. [Correctness] Silent truncation via `as` casts on 32-bit platforms
- **Domain:** Feature 2 (CCS Native Format)
- **Files:** `conary-core/src/ccs/package.rs:304,306,446,486-487`, `conary-core/src/ccs/chunking.rs:155`
- **Finding:** `u64 as usize` truncates silently on 32-bit. `u64 as i64` and `u32 as i32` can overflow for large values.
- **Fix:** Use `TryFrom` with error handling or `#[cfg(target_pointer_width = "64")]` gate.

### P1-7. [Correctness] `verify_content_hashes` uses heuristic instead of `file_type` for directory detection
- **Domain:** Feature 2 (CCS Native Format)
- **File:** `conary-core/src/ccs/verify.rs:349`
- **Finding:** `file.size == 0 && file.hash.is_empty()` matches both directories and empty files with empty hashes.
- **Fix:** Use `file.file_type == FileType::Directory`.

### P1-8. [Correctness] `parse_octal_mode` fails on input "0"
- **Domain:** Feature 2 (CCS Native Format)
- **File:** `conary-core/src/ccs/manifest.rs:792-798`
- **Finding:** `strip_prefix('0')` on `"0"` produces `""`, then `from_str_radix("", 8)` errors. Inconsistent with `directory.rs` which has a guard.
- **Fix:** Add empty-string guard: `let mode_str = if mode_str.is_empty() { "0" } else { mode_str };`.

### P1-9. [Correctness] Fragile coupling between chunking hex encoding and hash module
- **Domain:** Feature 2 (CCS Native Format)
- **File:** `conary-core/src/ccs/builder.rs:251`
- **Finding:** Chunked path uses `hex::encode(chunk.hash)`, non-chunked uses `hash::sha256()`. Both must produce identical lowercase hex; no documented invariant.
- **Fix:** Use a single function for both paths. Document lowercase hex invariant.

### P1-10. [Security] `converter.rs` does not use `safe_join` for path construction
- **Domain:** Feature 2 (CCS Native Format)
- **File:** `conary-core/src/ccs/convert/converter.rs:418`
- **Finding:** `temp_dir.join(rel_path)` where `rel_path` comes from legacy packages. `../` in paths could write outside temp directory.
- **Fix:** Use `crate::filesystem::path::safe_join(temp_dir, rel_path)`.

### P1-11. [Correctness] sysctl `only_if_lower` semantics lost when writing to sysctl.d config
- **Domain:** Feature 2 (CCS Native Format)
- **File:** `conary-core/src/ccs/hooks/sysctl.rs:60-67`
- **Finding:** Config file just writes `key=value` with a misleading comment; sysctl.d has no conditional application. Value applied unconditionally on first boot.
- **Fix:** Either skip writing `only_if_lower` entries to sysctl.d or generate a boot-time oneshot that does the comparison.

### P1-12. [Correctness] `MountOptions.digest` field is never used -- fsverity not enforced
- **Domain:** Feature 3 (Filesystem & Generations)
- **File:** `conary-core/src/generation/mount.rs:34`
- **Finding:** `digest` field documented as "Passed as digest= mount option" but `to_mount_args()` never reads it. Attacker who replaces EROFS image on disk can mount it even with `verity: true`.
- **Fix:** Add `if let Some(ref digest) = self.digest { opts.push(format!("digest={digest}")); }` to `to_mount_args()`.

### P1-13. [Correctness] `etc_merge` does not handle overlayfs whiteout files
- **Domain:** Feature 3 (Filesystem & Generations)
- **File:** `conary-core/src/generation/etc_merge.rs:261-286`
- **Finding:** User deletions of /etc files (overlayfs whiteout entries) are not detected. Deleted files silently reappear in next generation.
- **Fix:** Detect `.wh.*` whiteout entries in scan, classify as user deletion.

### P1-14. [Correctness] `safe_join()` defense-in-depth silently skipped when path doesn't exist
- **Domain:** Feature 3 (Filesystem & Generations)
- **File:** `conary-core/src/filesystem/path.rs:122-132`
- **Finding:** If `canonicalize()` fails (common for new files during deployment), the entire defense-in-depth check is silently skipped. Relies solely on `sanitize_path()`.
- **Fix:** Use manual normalization that resolves `..` without filesystem access, or log a debug warning when canonicalize fails.

### P1-15. [Correctness] md5 accepted by `verify_file_checksum` but rejected by recipe parser
- **Domain:** Feature 4 (Source Building)
- **File:** `conary-core/src/recipe/kitchen/archive.rs:58` vs `conary-core/src/recipe/parser.rs:40-47`
- **Finding:** archive.rs accepts `md5` as a valid algorithm; parser rejects anything not `sha256:` or `xxh128:`. Inconsistency between verification and validation.
- **Fix:** Remove md5 arm from `verify_file_checksum`.

### P1-16. [Correctness] PKGBUILD `source=()` with unquoted values not fully parsed
- **Domain:** Feature 4 (Source Building)
- **File:** `conary-core/src/recipe/pkgbuild.rs:316-345`
- **Finding:** `extract_array` regex requires quote characters; fallback `trim_matches` strips quotes but not parentheses. Some legitimate PKGBUILDs may produce malformed entries.
- **Fix:** Test with unquoted PKGBUILD arrays; handle parentheses in fallback.

### P1-17. [Correctness] `extract_array` compiles regex on every call (7+ times per PKGBUILD)
- **Domain:** Feature 4 (Source Building)
- **File:** `conary-core/src/recipe/pkgbuild.rs:318-319`
- **Finding:** Fresh `Regex::new` on every invocation for each array type.
- **Fix:** Cache compiled regexes or accept as-is for interactive tool.

### P1-18. [Correctness] `convert_pkgbuild_url` double-replaces when pkgname appears in version
- **Domain:** Feature 4 (Source Building)
- **File:** `conary-core/src/recipe/pkgbuild.rs:413-452`
- **Finding:** Name replacement before version replacement can corrupt URL if package name is a substring of the version.
- **Fix:** Apply version replacement before name replacement, or use single-pass strategy.

### P1-19. [Security] `find_deny_conflicts` uses string `starts_with` instead of path component matching
- **Domain:** Feature 5 (Supply Chain Security)
- **File:** `conary-core/src/capability/enforcement/landlock_enforce.rs:139-145`
- **Finding:** `/etcetera/secret` falsely flagged as conflicting with `/etc` allow path. String comparison instead of `Path::starts_with`.
- **Fix:** Use `std::path::Path::new(deny_path).starts_with(read_path)`.

### P1-20. [Correctness] `expect()` calls in production `Provenance::dna_hash()`
- **Domain:** Feature 5 (Supply Chain Security)
- **File:** `conary-core/src/provenance/mod.rs:79-80`
- **Finding:** Two `expect()` calls in non-test production path for hex decode and byte conversion.
- **Fix:** Return `Result<DnaHash, DnaHashError>` or document infallibility.

### P1-21. [Security] `verify_snapshot_consistency` silently passes when entries are missing
- **Domain:** Feature 5 (Supply Chain Security)
- **File:** `conary-core/src/trust/verify.rs:132-159`
- **Finding:** Per TUF spec, snapshot MUST contain root.json and targets.json entries. Missing entries are silently ignored, weakening mix-and-match attack protection.
- **Fix:** Make presence of root.json and targets.json mandatory; return `ConsistencyError` if absent.

### P1-22. [Security] Root rotation loop has no upper bound on iterations
- **Domain:** Feature 5 (Supply Chain Security)
- **File:** `conary-core/src/trust/client.rs:218-254`
- **Finding:** Unbounded loop probing `{version+1}.root.json`. Malicious server can force infinite loop consuming time and DB writes.
- **Fix:** Add `MAX_ROOT_ROTATIONS` constant (typically 1024) and break with error.

### P1-23. [Security] `verify_file` passes `require_hash: false` -- hash check is optional
- **Domain:** Feature 5 (Supply Chain Security)
- **File:** `conary-core/src/trust/verify.rs:229`
- **Finding:** A target with no hash in TUF targets metadata passes verification without content check.
- **Fix:** Change to `require_hash: true` for target file verification.

### P1-24. [Correctness] Canonical JSON may not be deterministic under `serde_json/preserve_order`
- **Domain:** Feature 6 (Cross-Distro & Extensibility)
- **File:** `conary-core/src/json.rs:36-41`
- **Finding:** `sort_json_keys` collects into `serde_json::Map` which is `IndexMap` under `preserve_order` feature, preserving non-sorted insertion order. Would silently break signature verification.
- **Fix:** Collect into explicit `BTreeMap<String, serde_json::Value>` then convert.

### P1-25. [Correctness] Remote collection content hash uses non-canonical serialization
- **Domain:** Feature 6 (Cross-Distro & Extensibility)
- **File:** `conary-core/src/model/remote.rs:282-283,533`
- **Finding:** `serde_json::to_vec` does not guarantee key order for `HashMap<String, String>`. Hash verification is fragile across platforms/versions.
- **Fix:** Use `crate::json::canonical_json` for both publisher and verifier paths.

### P1-26. [Correctness] Daemon `--foreground` flag accepted but silently discarded
- **Domain:** Feature 7 (CLI Layer)
- **File:** `src/main.rs:1654`
- **Finding:** Flag bound to `_` wildcard; daemon always runs in foreground regardless.
- **Fix:** Wire into `DaemonConfig` or remove the flag.

### P1-27. [Correctness] `Export --oci` flag always true, no alternative formats
- **Domain:** Feature 7 (CLI Layer)
- **File:** `src/cli/mod.rs:605-607`, `src/main.rs:1863`
- **Finding:** `--no-oci` accepted but gets OCI output anyway. Flag value ignored entirely.
- **Fix:** Remove the flag; use `--format` enum if future formats are planned.

### P1-28. [Code Quality] 22 `#[allow(dead_code)]` annotations in non-test command code
- **Domain:** Feature 7 (CLI Layer)
- **Files:** `install/resolve.rs`, `install/batch.rs`, `install/execute.rs`, `install/blocklist.rs`, `install/dep_resolution.rs`, `install/system_pm.rs`, `install/scriptlets.rs`, `install/mod.rs`, `federation.rs`, `model.rs`, `derived.rs:393`, `adopt/convert.rs:27`
- **Finding:** 22 instances of dead code with `#[allow(dead_code)]`, some annotated "TODO: wire into X" but never connected.
- **Fix:** Remove speculative dead code; track needed code with issues.

### P1-29. [Correctness] `cmd_automation_history` is a complete stub returning success
- **Domain:** Feature 7 (CLI Layer)
- **File:** `src/commands/automation.rs:491-517`
- **Finding:** Ignores all parameters, always prints "No automation history recorded yet." The `automation_actions` table exists in schema.
- **Fix:** Implement DB query or mark command as `#[command(hide = true)]`.

### P1-30. [Correctness] `cmd_state_restore` is a stub that shows a plan but never applies
- **Domain:** Feature 7 (CLI Layer)
- **File:** `src/commands/state.rs:217`
- **Finding:** Returns `Ok(())` after printing `[NOT YET IMPLEMENTED]`. Scripted usage falsely reports success for a destructive operation.
- **Fix:** Return error when apply is not implemented.

### P1-31. [Correctness] `store_chunk` does not verify content hash before writing
- **Domain:** Feature 8 (Remi Server)
- **File:** `conary-server/src/server/cache.rs:111`
- **Finding:** Trusts caller-supplied hash without verifying `sha256(data) == hash`. Mismatched hash/data silently persisted.
- **Fix:** Always verify or add `store_chunk_verified` method.

### P1-32. [Security] PUT endpoints on public router bypass admin rate limiting and audit
- **Domain:** Feature 8 (Remi Server)
- **Files:** `conary-server/src/server/handlers/derivations.rs:194`, `seeds.rs:97`, `profiles.rs:90`
- **Finding:** PUT endpoints on `:8080` use inline token check but bypass auth-failure rate limiter (5/min) protecting `:8082`. Brute-force tokens at 100 rps.
- **Fix:** Move to external admin router or apply auth-failure rate limiter to public router.

### P1-33. [Correctness] `TOUCH_CACHE` grows unbounded in auth middleware
- **Domain:** Feature 8 (Remi Server)
- **File:** `conary-server/src/server/auth.rs:31`
- **Finding:** HashMap with no eviction or cleanup. Entry per unique token ID over server lifetime.
- **Fix:** Periodic cleanup of entries older than `TOUCH_DEBOUNCE_SECS` or LRU with max capacity.

### P1-34. [Correctness] TCP listener bound but never accepts connections
- **Domain:** Feature 9 (Daemon & Federation)
- **File:** `conary-server/src/daemon/mod.rs:640-650,675-727`
- **Finding:** `SocketManager::bind()` creates TCP listener when `enable_tcp` is true, but `run_daemon()` only uses Unix listener. Port shows as listening but connections hang forever.
- **Fix:** Implement TCP accept loop or return error when `enable_tcp` is set.

### P1-35. [Correctness] `fetch_chunk_inner()` does not apply `tier_allowlists` filtering
- **Domain:** Feature 9 (Daemon & Federation)
- **File:** `conary-server/src/federation/mod.rs:406`
- **Finding:** Calls `select_peers_hierarchical()` (unfiltered) instead of `select_peers_hierarchical_filtered()`. Per-tier endpoint restrictions not enforced during chunk fetching.
- **Fix:** Call `select_peers_hierarchical_filtered()` with `self.config.tier_allowlists`.

### P1-36. [Correctness] `DaemonJob::from_row` silently nullifies invalid spec JSON
- **Domain:** Feature 9 (Daemon & Federation)
- **File:** `conary-server/src/daemon/jobs.rs:306-307`
- **Finding:** `serde_json::from_str(&spec_json).unwrap_or(serde_json::Value::Null)` silently converts corrupt JSON to null. Job behaves unpredictably when executed.
- **Fix:** Return `FromSqlConversionFailure` error like kind/status parsing does.

### P1-37. [Correctness] `dry_run_handler` requires write-level auth for read-only operation
- **Domain:** Feature 9 (Daemon & Federation)
- **File:** `conary-server/src/daemon/routes.rs:980-984`
- **Finding:** Unprivileged users cannot preview operations without PolicyKit authorization for the write action.
- **Fix:** Use `Action::Query` or `Action::DryRun` for dry-run operations.

### P1-38. [Architecture] Container initialization logic duplicated in three places
- **Domain:** Feature 10 (Test Infrastructure)
- **Files:** `conary-test/src/engine/container_setup.rs:16-83`, `conary-test/src/server/service.rs:379-451`
- **Finding:** `initialize_container_state` was extracted to deduplicate, but `service.rs::initialize_container()` is a near-identical copy not updated to call the shared function.
- **Fix:** Replace `service.rs::initialize_container()` with a call to the shared function.

### P1-39. [Correctness] `cleanup_containers` filters by label but containers are never labeled
- **Domain:** Feature 10 (Test Infrastructure)
- **File:** `conary-test/src/server/service.rs:700-741`
- **Finding:** Cleanup endpoint filters by `label=conary-test`, but container creation never adds this label. Cleanup always returns `{"removed": 0}`. Orphaned containers accumulate.
- **Fix:** Add `labels: Some(HashMap::from([("conary-test", "true")]))` to `ContainerCreateBody`.

---

## P2 -- Medium

### P2-1. [Correctness] infrastructure.md references nonexistent `erofs` release group
- **Domain:** Feature 0 (Repo Presentation)
- **File:** `.claude/rules/infrastructure.md:62`
- **Fix:** Remove `erofs|` from the release.sh docs.

### P2-2. [Correctness] architecture.md says "6-phase pipeline" but CLAUDE.md says "8-stage"
- **Domain:** Feature 0 (Repo Presentation)
- **File:** `.claude/rules/architecture.md:45`
- **Fix:** Align to README's description.

### P2-3. [Security] Three RUSTSEC advisories suppressed without explanation
- **Domain:** Feature 0 (Repo Presentation)
- **File:** `.github/workflows/ci.yml:78-81`
- **Fix:** Add comment above each `--ignore` explaining why safe to suppress.

### P2-4. [Correctness] README Building section says "~260 unit tests"
- **Domain:** Feature 0 (Repo Presentation)
- **File:** `README.md:513`
- **Fix:** Consolidate to one accurate number across all files.

### P2-5. [Quality] conary-test Cargo.toml missing authors and license fields
- **Domain:** Feature 0 (Repo Presentation)
- **File:** `conary-test/Cargo.toml:1-7`
- **Fix:** Add `authors` and `license` fields.

### P2-6. [Slop] README comparison table claims features that are aspirational
- **Domain:** Feature 0 (Repo Presentation)
- **File:** `README.md:62-84`
- **Fix:** Add "(alpha)" or "(experimental)" annotations to untested features.

### P2-7. [Quality] GitHub Actions CI caches are suboptimal
- **Domain:** Feature 0 (Repo Presentation)
- **File:** `.github/workflows/ci.yml:28-41`
- **Fix:** Use single cache action with `restore-keys` fallback.

### P2-8. [Correctness] E2E workflow Phase 3 does not build `conary` binary
- **Domain:** Feature 0 (Repo Presentation)
- **File:** `.forgejo/workflows/e2e.yaml:81`
- **Fix:** Add `cargo build` step before `cargo build -p conary-test` in Phase 3.

### P2-9. [Performance] DEB parser reads entire data.tar into memory
- **Domain:** Feature 1 (Package Management Core)
- **File:** `conary-core/src/packages/deb.rs:155-199`
- **Fix:** Consider streaming extraction.

### P2-10. [Correctness] `db_dir` default fallback to `/var/lib/conary` for bare filename
- **Domain:** Feature 1 (Package Management Core)
- **File:** `conary-core/src/db/paths.rs:7-13`
- **Fix:** Document this behavior.

### P2-11. [Correctness] `format_permissions` does not handle setuid/setgid/sticky bits
- **Domain:** Feature 1 (Package Management Core)
- **File:** `conary-core/src/db/models/file_entry.rs:211-242`
- **Fix:** Handle 04000, 02000, 01000 mode bits.

### P2-12. [Quality] `RepositoryPackage::search` does not filter by `enabled = 1`
- **Domain:** Feature 1 (Package Management Core)
- **File:** `conary-core/src/db/models/repository.rs:381-394`
- **Fix:** Add `WHERE r.enabled = 1` join.

### P2-13. [Security] `validate_wal_file` TOCTOU between `exists()` check and `open()`
- **Domain:** Feature 1 (Package Management Core)
- **File:** `conary-core/src/db/mod.rs:43-81`
- **Fix:** Open directly, handle `ErrorKind::NotFound`.

### P2-14. [Quality] Migration code uses `IF NOT EXISTS` inconsistently
- **Domain:** Feature 1 (Package Management Core)
- **File:** `conary-core/src/db/migrations/v1_v20.rs`, `v21_v40.rs`, `v41_current.rs`
- **Fix:** Pick one style and document the convention.

### P2-15. [Quality] `ConaryProvider` uses `HashMap<u32, ...>` losing type safety
- **Domain:** Feature 1 (Package Management Core)
- **File:** `conary-core/src/resolver/provider/mod.rs:72`
- **Fix:** Key on `SolvableId` instead of `u32`.

### P2-16. [Quality] archive_reader doesn't verify object path hex length
- **Domain:** Feature 2 (CCS Native Format)
- **File:** `conary-core/src/ccs/archive_reader.rs:158-168`
- **Fix:** Add `if hash.len() != 64 { warn!(...); continue; }`.

### P2-17. [Quality] Policy chain Replace action carries empty Vec (type-level lie)
- **Domain:** Feature 2 (CCS Native Format)
- **File:** `conary-core/src/ccs/policy.rs:170-174`
- **Fix:** Introduce `PolicyAction::Modified` or return `(action, content)` tuple.

### P2-18. [Quality] Redundant file reads during OCI layer creation
- **Domain:** Feature 2 (CCS Native Format)
- **File:** `conary-core/src/ccs/export/oci.rs:233-234`
- **Fix:** Hash the uncompressed tar during `create_layer_tarball` by teeing to a hasher.

### P2-19. [Quality] Manifest validation is minimal (name/version non-empty only)
- **Domain:** Feature 2 (CCS Native Format)
- **File:** `conary-core/src/ccs/manifest.rs:93-101`
- **Fix:** Add validation for name format, hook path absoluteness, mode string validity.

### P2-20. [Quality] OCI export creates non-reproducible images due to `Utc::now()`
- **Domain:** Feature 2 (CCS Native Format)
- **File:** `conary-core/src/ccs/export/oci.rs:428`
- **Fix:** Use `SOURCE_DATE_EPOCH` or `LAYER_MTIME` for config timestamp.

### P2-21. [Correctness] `GenerationMetadata::write_to()` is not crash-safe
- **Domain:** Feature 3 (Filesystem & Generations)
- **File:** `conary-core/src/generation/metadata.rs:71-76`
- **Fix:** Use temp + fsync + rename pattern.

### P2-22. [Security] TOCTOU in `CasStore::atomic_store()` existence check
- **Domain:** Feature 3 (Filesystem & Generations)
- **File:** `conary-core/src/filesystem/cas.rs:103-104`
- **Fix:** Document explicitly or remove existence check and let rename handle the race.

### P2-23. [Quality] Duplicate delta code between `generation/delta.rs` and `delta/generator.rs`
- **Domain:** Feature 3 (Filesystem & Generations)
- **Files:** `conary-core/src/generation/delta.rs`, `conary-core/src/delta/generator.rs`
- **Fix:** Extract shared `zstd_dict_compress`/`zstd_dict_decompress` utility.

### P2-24. [Quality] `accept_package_paths()` has a dead binding `let _ = a;`
- **Domain:** Feature 3 (Filesystem & Generations)
- **File:** `conary-core/src/generation/etc_merge.rs:81-83`
- **Fix:** Replace with `.map(|(p, _)| p.as_path())`.

### P2-25. [Architecture] `DeltaGenerator.cas` is `pub(crate)` exposing internal state
- **Domain:** Feature 3 (Filesystem & Generations)
- **File:** `conary-core/src/delta/generator.rs:22`
- **Fix:** Make private; add `#[cfg(test)]` accessor.

### P2-26. [Quality] `hardlink_from_existing()` reads entire file for hashing
- **Domain:** Feature 3 (Filesystem & Generations)
- **File:** `conary-core/src/filesystem/cas.rs:401`
- **Fix:** Use streaming hash via `hash_reader` with `BufReader`.

### P2-27. [Quality] `GENERATION_FORMAT` constant mismatch with rules doc
- **Domain:** Feature 3 (Filesystem & Generations)
- **File:** `conary-core/src/generation/metadata.rs:16` vs `.claude/rules/generation.md`
- **Fix:** Align documentation and code.

### P2-28. [Quality] `build_script_hash` does not include `script_file` field
- **Domain:** Feature 4 (Source Building)
- **File:** `conary-core/src/derivation/recipe_hash.rs:60-93`
- **Fix:** Hash the `script_file` path and content.

### P2-29. [Quality] `expand_variables` does not expand `%(destdir)s`
- **Domain:** Feature 4 (Source Building)
- **File:** `conary-core/src/derivation/recipe_hash.rs:27-45`
- **Fix:** Documented intentional divergence; update comment.

### P2-30. [Quality] `output_hash` v1 does not include file permissions
- **Domain:** Feature 4 (Source Building)
- **File:** `conary-core/src/derivation/output.rs:78-96`
- **Fix:** Switch `OutputManifest::new()` to v2 or plan migration path.

### P2-31. [Quality] `graph.rs find_cycles` may report duplicate cycles
- **Domain:** Feature 4 (Source Building)
- **File:** `conary-core/src/recipe/graph.rs:220-266`
- **Fix:** De-duplicate cycles before returning, or accept as-is since downstream handles duplicates.

### P2-32. [Quality] `PKGBUILD` `convert_function_body` replaces `$srcdir` with `.`
- **Domain:** Feature 4 (Source Building)
- **File:** `conary-core/src/recipe/pkgbuild.rs:461`
- **Fix:** Document the replacement in user-facing warnings.

### P2-33. [Correctness] `provenance_capture.rs` patches not sorted before DNA hash despite comment
- **Domain:** Feature 4 (Source Building)
- **File:** `conary-core/src/recipe/kitchen/provenance_capture.rs:217`
- **Fix:** Sort patches or update comment to say "in recipe order".

### P2-34. [Quality] `archive.rs` panics on non-UTF-8 paths
- **Domain:** Feature 4 (Source Building)
- **File:** `conary-core/src/recipe/kitchen/archive.rs:76-77,119`
- **Fix:** Return error instead of panicking.

### P2-35. [Correctness] `load_capabilities` swallows all DB errors as `None` via `.ok()`
- **Domain:** Feature 5 (Supply Chain Security)
- **File:** `conary-core/src/capability/mod.rs:103-110`
- **Fix:** Use `.optional()` to distinguish "no rows" from actual errors.

### P2-36. [Correctness] `load_capabilities_by_name` also swallows DB errors via `.ok()`
- **Domain:** Feature 5 (Supply Chain Security)
- **File:** `conary-core/src/capability/mod.rs:125-131`
- **Fix:** Same as above.

### P2-37. [Quality] `TufMetadataFields` trait is pure boilerplate (4 identical impls)
- **Domain:** Feature 5 (Supply Chain Security)
- **File:** `conary-core/src/trust/client.rs:563-602`
- **Fix:** Use `macro_rules!` to generate impls.

### P2-38. [Quality] Broken doc link in `trust/keys.rs` canonical_json delegation
- **Domain:** Feature 5 (Supply Chain Security)
- **File:** `conary-core/src/trust/keys.rs:27-30`
- **Fix:** Fix doc comment to reference `crate::json::canonical_json`.

### P2-39. [Security] `ceremony::generate_role_key` writes private key with default permissions
- **Domain:** Feature 5 (Supply Chain Security)
- **File:** `conary-core/src/trust/ceremony.rs:22-23`
- **Fix:** Verify `save_to_files` sets 0600 on private key, or add explicit `set_permissions`.

### P2-40. [Quality] `check_root_rotation` persists intermediate roots non-transactionally
- **Domain:** Feature 5 (Supply Chain Security)
- **File:** `conary-core/src/trust/client.rs:241-243`
- **Fix:** Wrap rotation loop in a transaction or document intentional intermediate persistence.

### P2-41. [Slop] Over-commented obvious code in provenance module
- **Domain:** Feature 5 (Supply Chain Security)
- **File:** `conary-core/src/provenance/build.rs`, `content.rs`, `source.rs`
- **Fix:** Remove trivial doc comments that restate method names.

### P2-42. [Correctness] `DiffAction::package()` returns distro name for `SetSourcePin`
- **Domain:** Feature 6 (Cross-Distro & Extensibility)
- **File:** `conary-core/src/model/diff.rs:130`
- **Fix:** Return sentinel `"<source-policy>"` or `Option<&str>`.

### P2-43. [Quality] `ActionBuilder::build()` uses `Debug` format for action ID
- **Domain:** Feature 6 (Cross-Distro & Extensibility)
- **File:** `conary-core/src/automation/action.rs:88-89`
- **Fix:** Use stable string representation instead of `Debug`.

### P2-44. [Security] `handler_exists_in_root` does not use `safe_join` for non-absolute PATH search
- **Domain:** Feature 6 (Cross-Distro & Extensibility)
- **File:** `conary-core/src/trigger/mod.rs:393-397`
- **Fix:** Use `safe_join` for PATH-search branch.

### P2-45. [Correctness] `is_major_upgrade` false positives for epoch-prefixed versions
- **Domain:** Feature 6 (Cross-Distro & Extensibility)
- **File:** `conary-core/src/automation/check.rs:521-534`
- **Fix:** Strip epoch prefix before extracting major version component.

### P2-46. [Quality] Duplicate `reqwest::Client` construction across 5 modules
- **Domain:** Feature 6 (Cross-Distro & Extensibility)
- **Files:** `canonical/client.rs:34`, `canonical/repology.rs:211`, `model/remote.rs:452`, `self_update.rs:221`, `self_update.rs:317`
- **Fix:** Extract shared `crate::http::client()` builder.

### P2-47. [Architecture] `model/mod.rs` has dual error systems (ModelError vs crate::Error)
- **Domain:** Feature 6 (Cross-Distro & Extensibility)
- **File:** `conary-core/src/model/mod.rs:102-133`
- **Fix:** Migrate `ModelError` into `crate::error::Error` or implement `From<ModelError>`.

### P2-48. [Architecture] `cmd_repo_add` takes 12 positional parameters
- **Domain:** Feature 7 (CLI Layer)
- **File:** `src/commands/repo.rs` (called from `src/main.rs:617-632`)
- **Fix:** Create `RepoAddOptions` struct.

### P2-49. [Quality] `distro list` is hardcoded with stale data (ubuntu-oracular EOL)
- **Domain:** Feature 7 (CLI Layer)
- **File:** `src/commands/distro.rs:66-75`
- **Fix:** Query `distros` table or canonical registry.

### P2-50. [Quality] Inconsistent async patterns across commands (async fn without await)
- **Domain:** Feature 7 (CLI Layer)
- **Files:** Various in `src/commands/`
- **Fix:** Document convention; this is a long-term cleanup.

### P2-51. [Quality] `no_capture` field has confusing semantics
- **Domain:** Feature 7 (CLI Layer)
- **File:** `src/commands/install/mod.rs:86`
- **Fix:** Rename to `no_scriptlet_capture` or `skip_capture`.

### P2-52. [Correctness] `remove.rs:325` prints redundant file count (N/N always)
- **Domain:** Feature 7 (CLI Layer)
- **File:** `src/commands/remove.rs:325`
- **Fix:** Print just `removed_count` or include directory count.

### P2-53. [Security] Self-update signature verification code flow misleads auditors
- **Domain:** Feature 7 (CLI Layer)
- **File:** `src/commands/self_update.rs:72-80`
- **Fix:** Move signature check after download or add clarifying comment.

### P2-54. [Slop] Bootstrap commands have highly repetitive parameter patterns
- **Domain:** Feature 7 (CLI Layer)
- **File:** `src/cli/bootstrap.rs`
- **Fix:** Extract `BootstrapPhaseArgs` struct with common fields.

### P2-55. [Quality] Negative cache is unbounded HashMap
- **Domain:** Feature 8 (Remi Server)
- **File:** `conary-server/src/server/negative_cache.rs:27`
- **Fix:** Add max capacity (e.g., 100K entries).

### P2-56. [Quality] Public `RateLimiter` uses unbounded HashMap (TODO: migrate to governor)
- **Domain:** Feature 8 (Remi Server)
- **File:** `conary-server/src/server/security.rs:22`
- **Fix:** Complete migration to `governor::DefaultKeyedRateLimiter`.

### P2-57. [Quality] `BanList` uses `String` keys instead of `IpAddr`
- **Domain:** Feature 8 (Remi Server)
- **File:** `conary-server/src/server/security.rs:77`
- **Fix:** Change to `HashMap<IpAddr, Instant>`.

### P2-58. [Architecture] `ServerState` god-struct with 26+ fields behind single RwLock
- **Domain:** Feature 8 (Remi Server)
- **File:** `conary-server/src/server/mod.rs:182`
- **Fix:** Group into sub-structs with independent locking.

### P2-59. [Correctness] `conversion.rs:607` total_size cast from i64 wraps negatives
- **Domain:** Feature 8 (Remi Server)
- **File:** `conary-server/src/server/conversion.rs:607`
- **Fix:** Use `u64::try_from(...).unwrap_or(0)`.

### P2-60. [Quality] `scan_chunk_hashes` duplicated between `chunks.rs` and `chunk_gc.rs`
- **Domain:** Feature 8 (Remi Server)
- **Files:** `conary-server/src/server/handlers/chunks.rs:855`, `conary-server/src/server/chunk_gc.rs:89`
- **Fix:** Keep walkdir version; call from both via `spawn_blocking`.

---

## P3 -- Minor

### P3-1. [Correctness] CLAUDE.md references "69 tables" but verification is difficult
- **Domain:** Feature 0 (Repo Presentation)
- **File:** `CLAUDE.md:62`

### P3-2. [Quality] CLAUDE.md "Tool Selection" section is sparse
- **Domain:** Feature 0 (Repo Presentation)
- **File:** `CLAUDE.md:64-69`

### P3-3. [Correctness] deploy/FORGE.md workflow table says "37-test suite" (stale)
- **Domain:** Feature 0 (Repo Presentation)
- **File:** `deploy/FORGE.md:36`

### P3-4. [Slop] README "What's Next" section is generic filler
- **Domain:** Feature 0 (Repo Presentation)
- **File:** `README.md:536-539`

### P3-5. [Architecture] packaging/dracut duplicated across deploy/ and packaging/
- **Domain:** Feature 0 (Repo Presentation)
- **Files:** `deploy/dracut/`, `packaging/dracut/`

### P3-6. [Quality] `models/mod.rs format_size` is a one-line delegation
- **Domain:** Feature 1 (Package Management Core)
- **File:** `conary-core/src/db/models/mod.rs:98-100`

### P3-7. [Idiomatic] `TroveType`, `InstallSource`, `InstallReason` redundant `as_str` methods
- **Domain:** Feature 1 (Package Management Core)
- **File:** `conary-core/src/db/models/trove.rs:22,44,76`

### P3-8. [Quality] `CompressionFormat::None` shadows `Option::None`
- **Domain:** Feature 1 (Package Management Core)
- **File:** `conary-core/src/compression/mod.rs:38`

### P3-9. [Quality] Unused `tracing::debug` imports in migrations
- **Domain:** Feature 1 (Package Management Core)
- **Files:** `conary-core/src/db/migrations/v21_v40.rs:6`, `v41_current.rs:6`

### P3-10. [Architecture] `signing.rs` uses `anyhow::Result` but other CCS modules use `thiserror`
- **Domain:** Feature 2 (CCS Native Format)
- **File:** `conary-core/src/ccs/signing.rs:8`

### P3-11. [Quality] `DependencyKind` manual Display duplicates serde rename
- **Domain:** Feature 2 (CCS Native Format)
- **File:** `conary-core/src/ccs/lockfile.rs:176-185`

### P3-12. [Quality] `EnhancementResult_` naming with trailing underscore
- **Domain:** Feature 2 (CCS Native Format)
- **File:** `conary-core/src/ccs/enhancement/mod.rs:164`

### P3-13. [Security] mock.rs log format vulnerable to argument injection (inside sandbox)
- **Domain:** Feature 2 (CCS Native Format)
- **File:** `conary-core/src/ccs/convert/mock.rs:71-74`

### P3-14. [Quality] legacy/mod.rs `is_safe` rejects slashes valid in sysctl keys
- **Domain:** Feature 2 (CCS Native Format)
- **File:** `conary-core/src/ccs/legacy/mod.rs:208-214`

### P3-15. [Quality] `detect_kernel_version_from_troves` test is a tautology
- **Domain:** Feature 3 (Filesystem & Generations)
- **File:** `conary-core/src/generation/builder.rs:610-612`

### P3-16. [Quality] `VfsNode` hash stored as `String` rather than newtype
- **Domain:** Feature 3 (Filesystem & Generations)
- **File:** `conary-core/src/filesystem/vfs/mod.rs:49`

### P3-17. [Quality] `sanitize_filename` does not check for null bytes (unlike `sanitize_path`)
- **Domain:** Feature 3 (Filesystem & Generations)
- **File:** `conary-core/src/filesystem/path.rs:154-174`

### P3-18. [Quality] `composefs_rs_eval.rs` is test-only but not gated behind `#[cfg(test)]` at module level
- **Domain:** Feature 3 (Filesystem & Generations)
- **File:** `conary-core/src/generation/composefs_rs_eval.rs`

### P3-19. [Quality] `VAR_RE` regex does not handle multi-line PKGBUILD values (documented limitation)
- **Domain:** Feature 4 (Source Building)
- **File:** `conary-core/src/recipe/pkgbuild.rs:52`

### P3-20. [Quality] `CROSS_TOOLS_ORDER` uses implicit `replace("++", "xx")` mapping
- **Domain:** Feature 4 (Source Building)
- **File:** `conary-core/src/bootstrap/cross_tools.rs:202`

### P3-21. [Quality] `BuildStage` and `Stage` are separate enums with overlapping semantics
- **Domain:** Feature 4 (Source Building)
- **Files:** `conary-core/src/recipe/format.rs:436`, `conary-core/src/derivation/build_order.rs:24`

### P3-22. [Quality] `BootstrapStage::all()` could use strum derive
- **Domain:** Feature 4 (Source Building)
- **File:** `conary-core/src/bootstrap/stages.rs:32-41`

### P3-23. [Quality] `DnaHashError::InputTooShort` misleading for inputs that are too long
- **Domain:** Feature 5 (Supply Chain Security)
- **File:** `conary-core/src/provenance/dna.rs:12-13`

### P3-24. [Quality] `CapabilityPolicy::load` accepts `Option<&str>` instead of `Option<&Path>`
- **Domain:** Feature 5 (Supply Chain Security)
- **File:** `conary-core/src/capability/policy.rs:114`

### P3-25. [Quality] `IsolationLevel::None` shadows `Option::None`
- **Domain:** Feature 5 (Supply Chain Security)
- **File:** `conary-core/src/provenance/build.rs:320`

### P3-26. [Quality] `ReproducibilityInfo::add_verifier` "differences" field misleading name
- **Domain:** Feature 5 (Supply Chain Security)
- **File:** `conary-core/src/provenance/build.rs:305-312`

### P3-27. [Quality] Inconsistent `pub mod` vs `mod` for capability submodules
- **Domain:** Feature 5 (Supply Chain Security)
- **File:** `conary-core/src/capability/mod.rs:32-35`

### P3-28. [Quality] `HostAttestation` hostname exclusion from hash not enforced by test
- **Domain:** Feature 5 (Supply Chain Security)
- **File:** `conary-core/src/provenance/build.rs:186`

### P3-29. [Convention] Missing file header verified correct on model/replatform.rs
- **Domain:** Feature 6 (Cross-Distro & Extensibility)
- **File:** `conary-core/src/model/replatform.rs:1`
- Note: All files checked have correct headers.

### P3-30. [Quality] `parse_duration` splits at `len()-1` which is fragile for multi-char units
- **Domain:** Feature 6 (Cross-Distro & Extensibility)
- **File:** `conary-core/src/automation/mod.rs:357`

### P3-31. [Quality] Unnecessary `.to_string()` allocations in discovery strategies
- **Domain:** Feature 6 (Cross-Distro & Extensibility)
- **File:** `conary-core/src/canonical/discovery.rs:56-63`

### P3-32. [Quality] `#[allow(dead_code)]` on `CanonicalMapResponse` fields
- **Domain:** Feature 6 (Cross-Distro & Extensibility)
- **File:** `conary-core/src/canonical/client.rs:16-17`

### P3-33. [Slop] Over-commented obvious patterns in automation/action.rs
- **Domain:** Feature 6 (Cross-Distro & Extensibility)
- **File:** `conary-core/src/automation/action.rs:276-308`

### P3-34. [Convention] Missing comment for Federation Commands section in main.rs
- **Domain:** Feature 7 (CLI Layer)
- **File:** `src/main.rs:1565-1567`

### P3-35. [Convention] `Revert` vs `Rollback` vs `Restore` naming inconsistency
- **Domain:** Feature 7 (CLI Layer)
- **Files:** `src/cli/state.rs:42`, `src/commands/state.rs:189`

### P3-36. [Convention] `system init` prints wrong command name `conary repo-sync`
- **Domain:** Feature 7 (CLI Layer)
- **File:** `src/commands/system.rs:82`

### P3-37. [Convention] `derived.rs:386` prints old command name `conary derive-build`
- **Domain:** Feature 7 (CLI Layer)
- **File:** `src/commands/derived.rs:386`

### P3-38. [Quality] `check_scope` returns `Option<Response>` with inverted semantics
- **Domain:** Feature 8 (Remi Server)
- **File:** `conary-server/src/server/handlers/admin/mod.rs:51`

### P3-39. [Quality] `is_valid_hash` allows uppercase but CAS uses lowercase
- **Domain:** Feature 8 (Remi Server)
- **File:** `conary-server/src/server/handlers/chunks.rs:35`

### P3-40. [Quality] Inconsistent error response patterns (plain text vs JSON)
- **Domain:** Feature 8 (Remi Server)

---

## Per-Domain Summary

**Feature 0 -- Repo Presentation** (21 findings): The biggest concern is a hardcoded authentication token in a tracked service file (P0). Beyond that, the primary risk is stale version/count numbers across README and CLAUDE.md that undermine credibility.

**Feature 1 -- Package Management Core** (20 findings): Well-structured database-first architecture. Main concerns are a version parsing ambiguity when RPM fallback is used for non-RPM packages (P0), `expect()` calls in the resolver pool that panic in production (P0), and fragile column-index coupling in 15+ model files (P1).

**Feature 2 -- CCS Native Format** (22 findings): Solid crypto and archive handling with good size guards. Three P0 findings: chunk store TOCTOU race, non-strict Ed25519 verification, and unsandboxed script hook execution. Multiple `as` cast truncation risks on 32-bit platforms (P1).

**Feature 3 -- Filesystem & Generations** (17 findings): CAS operations are crash-safe with atomic stores, and path traversal is well-guarded. Two P0 findings: generation number race between reservation and state creation, and symlink-following in etc_merge fallback. The unused `digest` field in MountOptions means fsverity is not actually enforced (P1).

**Feature 4 -- Source Building** (16 findings): Architecturally solid derivation system with content-addressed IDs. Two P0 findings: unescaped shell metacharacters in chroot build path, and PKGBUILD checksum algorithm mislabeling. Whole-file reads for checksum verification (P1) are the biggest resource concern.

**Feature 5 -- Supply Chain Security** (16 findings): TUF implementation is fundamentally sound with correct root rotation, signature thresholds, and type-field checks. No P0 findings. Five P1 concerns: unbounded root rotation loop (DoS), silent snapshot consistency bypass, production `expect()` calls, verify_file OOM, and string-based path matching in landlock.

**Feature 6 -- Cross-Distro & Extensibility** (13 findings): Well-tested with clean Ed25519 signing chain. One P0: self-update signature verification bypassed (empty key list). Two P1 findings: canonical JSON determinism risk under `preserve_order` feature, and non-canonical serialization for content hash verification.

**Feature 7 -- CLI Layer** (16 findings): Clean architecture with consistent CLI/command separation. One P0: `--version` flag silently ignored during self-update. Five P1 findings: dead `--foreground` flag, always-true `--oci` flag, dead code accumulation, and stub commands returning success.

**Feature 8 -- Remi Server** (15 findings): Production-grade with strong auth, SSRF protection, and thundering herd prevention. No P0 findings. Four P1 concerns: direct sha2 imports bypassing hash module, unverified content hash in chunk store, public-port write endpoints bypassing rate limiting, and unbounded TOUCH_CACHE.

**Feature 9 -- Daemon & Federation** (16 findings): Sound system lock and circuit breaker designs. Three P0 findings: unenforced `allowed_peers`, unverified mDNS peer injection, and blocking DB call on async executor. Four P1 findings including dead TCP listener, unenforced tier_allowlists, and silent spec JSON corruption.

**Feature 10 -- Test Infrastructure** (13 findings): Well-designed declarative test engine. Three P0 findings (all build breakage): missing module declarations, missing type conversions, and MCP map_err mismatches. Two P1 findings: duplicated container initialization and broken container cleanup labeling.
