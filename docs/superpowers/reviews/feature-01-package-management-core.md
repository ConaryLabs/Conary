## Feature 1: Package Management Core -- Review Findings

### Summary

The package management core is a well-structured 172K-line Rust codebase with a solid
database-first architecture, consistent parameterized query usage (no SQL injection), and
good test coverage on model CRUD operations. The codebase is largely production-ready with
proper error handling, security guards (CPIO size limits, path sanitization, SSRF
prevention, decompression bomb protection), and a cleanly layered architecture.

The primary concerns are: (1) a version parsing ambiguity for versions containing
multiple hyphens, (2) several production-path `expect()` calls in the resolver pool
that would panic on sufficiently large package sets, and (3) column-index coupling
between COLUMNS constants and `from_row` implementations that is one refactoring away
from a silent misread.

---

### P0 -- Critical

#### 1. RpmVersion::parse splits on first hyphen, mishandling multi-hyphen versions
- **File:** `conary-core/src/version/mod.rs:45`
- **Category:** Correctness
- **Finding:** `RpmVersion::parse` uses `rest.find('-')` to split version from release.
  This means a version string like `"1.2.3-beta-1.fc43"` yields version `"1.2.3"` and
  release `"beta-1.fc43"`, when the correct RPM split for some upstream versions is
  different. More critically, for Debian-style versions fed through this parser
  (e.g. `"2.0-1ubuntu3"` where the hyphen separates upstream from revision), the parser
  does the right thing -- but for RPM versions with hyphens in the upstream component
  (which are technically invalid per RPM spec but appear in the wild via `%version`
  macros), the first-hyphen split is the only correct behavior. This is **not** a bug
  per se for RPM versions, but the function is named generically and is used as the
  fallback parser for non-RPM version strings in the resolver graph
  (`InstalledPackageVersion::satisfies_legacy` at `graph.rs:84`). When a Debian or Arch
  version with multiple hyphens is compared via this RPM fallback, the split produces
  wrong epoch/version/release decomposition and version ordering breaks silently.
- **Fix:** Either (a) guard `satisfies_legacy` so it is never called for non-RPM
  schemes (the native comparison path already exists via `repo_version_satisfies`), or
  (b) document that `RpmVersion::parse` is RPM-only and add a compile-time guard.

#### 2. Resolver pool `expect()` panics on u32 overflow in production path
- **File:** `conary-core/src/resolver/provider/mod.rs:122,141,165,176,186,194`
- **Category:** Correctness
- **Finding:** The resolver provider uses `u32::try_from(...).expect("pool overflow")`
  for all interning operations (names, version sets, solvables, strings). These are
  production paths -- `solve_install` calls them for every package during dependency
  resolution. While 4 billion entries is unlikely in practice, the panic contract
  means a sufficiently large or pathological repository database causes an unrecoverable
  crash rather than a clean error. The `expect` messages are descriptive but the behavior
  is still process termination.
- **Fix:** Replace `expect` with `map_err(|_| Error::...)` and propagate the error.
  The resolver functions already return `Result`, so this is a signature-compatible
  change.

---

### P1 -- Important

#### 3. Column-index coupling in `from_row` implementations is fragile
- **File:** `conary-core/src/db/models/trove.rs:329-376`, `repository.rs:201-222`,
  `file_entry.rs:289-302`, `changeset.rs:153-170`
- **Category:** Architecture
- **Finding:** Every model defines a `COLUMNS` constant as a comma-separated string
  and a `from_row` method that indexes columns by ordinal position (0, 1, 2, ...).
  Adding a column to `COLUMNS` without updating the corresponding ordinal in `from_row`
  (or vice versa) causes a silent misread or runtime type error. With 17 columns on
  `Trove` and 19 on `RepositoryPackage`, this is an active maintenance risk. The
  pattern is repeated across 15+ model files.
- **Fix:** Consider a `derive` macro or a helper that generates both the column list
  and the row-mapping function. Short of that, add a compile-time test per model that
  asserts `COLUMNS.split(", ").count()` equals the number of `row.get(N)` calls. The
  test in `schema.rs` already validates table existence but not column alignment.

#### 4. `find_by_reason` uses user-controlled glob pattern in SQL LIKE
- **File:** `conary-core/src/db/models/trove.rs:452-464`
- **Category:** Security
- **Finding:** `Trove::find_by_reason` converts a glob-style `*` to SQL `%` before
  passing the pattern to a parameterized LIKE query. The pattern itself is passed via
  `?1` binding so there is no SQL injection. However, the public API accepts arbitrary
  `&str` patterns, and SQL `LIKE` interprets `%` and `_` as wildcards. If an external
  caller passes user input containing `%` or `_`, those characters become unescaped
  wildcards, potentially returning more rows than intended. Currently the three callers
  (`find_dependencies_installed`, `find_collection_installed`,
  `find_explicitly_installed`) use hardcoded patterns, so this is not exploitable today.
- **Fix:** Either mark the function `pub(crate)` to prevent external misuse, or
  escape `%` and `_` in the input before the `*` -> `%` conversion.

#### 5. `RepositoryPackage::search` uses unescaped LIKE pattern from user input
- **File:** `conary-core/src/db/models/repository.rs:381-394`
- **Category:** Security
- **Finding:** `RepositoryPackage::search` wraps user input in `%...%` for a LIKE
  query. If the user enters `%` or `_`, those are interpreted as SQL wildcards. The
  query is parameterized (no injection), but the wildcards make the search match more
  broadly than intended. For a package search CLI command this is low-severity but
  still semantically incorrect.
- **Fix:** Escape `%` and `_` in the `pattern` input before wrapping with `%...%`.

#### 6. `parse_timestamp` silently clamps negative Unix timestamps to 0
- **File:** `conary-core/src/repository/sync.rs:41`
- **Category:** Correctness
- **Finding:** `parse_timestamp` uses `u64::try_from(dt.timestamp()).unwrap_or(0)`.
  If a timestamp parses to a date before 1970, this silently returns 0 rather than
  erroring. This could cause `needs_sync` to always consider the repository stale
  (since 0 is always older than `metadata_expire`). Unlikely in practice but
  semantically wrong.
- **Fix:** Return an error for negative timestamps rather than clamping.

#### 7. `RetryConfig::with_retry` panics if `max_attempts` is 0
- **File:** `conary-core/src/repository/retry.rs:120,150`
- **Category:** Correctness
- **Finding:** Both sync and async `with_retry` functions end with
  `Err(last_err.expect("max_attempts must be >= 1"))`. If `max_attempts` is 0, the
  loop body never executes, `last_err` stays `None`, and `expect` panics. While the
  `RetryConfig::default()` sets 3, a caller can construct a config with 0 attempts.
- **Fix:** Add `debug_assert!(config.max_attempts >= 1)` at function entry, or
  clamp `max_attempts` to `max(1, ...)` in the constructor.

---

### P2 -- Improvement

#### 8. DEB parser reads entire data.tar into memory
- **File:** `conary-core/src/packages/deb.rs:155-199`
- **Category:** Performance
- **Finding:** `extract_ar_members` reads the entire control.tar and data.tar into
  `Vec<u8>` in memory. The `MAX_DEB_MEMBER_SIZE` guard (2 GB) prevents catastrophic
  OOM, but a 500 MB data.tar will still use 500 MB of heap. The RPM parser similarly
  reads the entire file into memory (with a 4 GB guard), so this is consistent within
  the codebase but worth noting for large packages.
- **Fix:** Consider streaming the data.tar extraction rather than buffering. This is
  not urgent but would improve memory behavior for large packages.

#### 9. `db_dir` default fallback to `/var/lib/conary` for bare filename
- **File:** `conary-core/src/db/paths.rs:7-13`
- **Category:** Correctness
- **Finding:** `db_dir("conary.db")` returns `/var/lib/conary` because `parent()`
  returns `""` (empty path), which is filtered out. This means if someone passes a
  relative filename without a directory, all CAS/keyring/temp paths silently point to
  the system default rather than the current directory. This is probably intentional
  for production (where DB paths are always absolute), but could surprise a developer
  testing with a relative path.
- **Fix:** Document this behavior in the function doc comment.

#### 10. `format_permissions` does not handle setuid/setgid/sticky bits
- **File:** `conary-core/src/db/models/file_entry.rs:211-242`
- **Category:** Correctness
- **Finding:** The `format_permissions` method renders `rwx` but ignores
  setuid (04000), setgid (02000), and sticky (01000) bits. A setuid binary would
  display as `---x------` instead of `---s------`. For a package manager this matters
  when users inspect installed files.
- **Fix:** Handle the special mode bits in the execute positions (s/S for setuid/setgid,
  t/T for sticky).

#### 11. `RepositoryPackage::search` does not deduplicate across repos
- **File:** `conary-core/src/db/models/repository.rs:381-394`
- **Category:** Code Quality
- **Finding:** The search query joins with `repositories` but does not filter by
  `enabled = 1`. A search will return packages from disabled repositories, which is
  inconsistent with `list_all` and `find_security_updates` which both filter on enabled.
- **Fix:** Add `JOIN repositories r ON rp.repository_id = r.id WHERE r.enabled = 1`
  to the search query.

#### 12. `validate_wal_file` TOCTOU between `exists()` check and `open()`
- **File:** `conary-core/src/db/mod.rs:43-81`
- **Category:** Security
- **Finding:** `validate_wal_file` checks `wal_path.exists()`, then opens the file.
  Between the check and the open, the file could be replaced (e.g., symlink attack).
  In practice, the WAL file lives next to the database which is already a trusted
  location, so the risk is minimal. The subsequent SQLite open will also validate
  the WAL independently.
- **Fix:** Open the file directly without the existence check, handling
  `ErrorKind::NotFound` as the "file doesn't exist" case.

#### 13. `detect_format` uses `file.read()` without checking full buffer fill
- **File:** `conary-core/src/packages/registry.rs:52`
- **Category:** Correctness
- **Finding:** `file.read(&mut magic)` returns `Ok(n)` where `n` can be less than 8.
  The code does check `n >= 4` and `n >= 7` before comparing magic bytes, so this is
  not a bug -- it is correctly handled. Just noting the pattern is correct.
- **Fix:** None needed.

#### 14. Migration code uses `IF NOT EXISTS` inconsistently
- **File:** `conary-core/src/db/migrations/v1_v20.rs`, `v21_v40.rs`, `v41_current.rs`
- **Category:** Code Quality
- **Finding:** Early migrations (v1-v20) do NOT use `CREATE TABLE IF NOT EXISTS` or
  `CREATE INDEX IF NOT EXISTS`, while later migrations (v41+) do. Since each migration
  runs in a transaction and the version is tracked, `IF NOT EXISTS` is technically
  unnecessary -- the migration should never re-run. The inconsistency is cosmetic but
  could be confusing for contributors.
- **Fix:** Pick one style (without `IF NOT EXISTS` is cleaner since it catches
  accidental double-application) and document the convention.

#### 15. `RepositoryPackage::find_in_enabled_repos_with_metadata_like` is a coarse filter
- **File:** `conary-core/src/db/models/repository.rs:464-480`
- **Category:** Code Quality
- **Finding:** The doc comment correctly states this is a "coarse pre-filter" and
  callers must re-check. The implementation wraps the name in `%...%` which will
  match partial strings (e.g., searching for `lib` would match `libX11` in the JSON).
  This is intentional and documented, so not a bug.
- **Fix:** Consider using the normalized `repository_provides` table instead of LIKE
  on the JSON blob, now that v49 migrations have populated it. This would be both
  more correct and faster.

#### 16. `ConaryProvider` uses `HashMap<u32, Vec<SolverDep>>` for dependencies
- **File:** `conary-core/src/resolver/provider/mod.rs:72`
- **Category:** Code Quality
- **Finding:** The dependencies map is keyed by `u32` (the raw SolvableId value)
  rather than `SolvableId` directly. This loses type safety -- a typo using a
  `NameId.0` instead of `SolvableId.0` would compile without error.
- **Fix:** Key the map on `SolvableId` instead of `u32`.

---

### P3 -- Nitpick

#### 17. `models/mod.rs` `format_size` is a one-line delegation
- **File:** `conary-core/src/db/models/mod.rs:98-100`
- **Category:** Code Quality
- **Finding:** `format_size` in models/mod.rs delegates to `crate::util::format_size`.
  The callers (`RepositoryPackage::size_human`, `FileEntry::size_human`) could call
  `crate::util::format_size` directly, removing this wrapper.
- **Fix:** Remove the wrapper and call `crate::util::format_size` directly from callers.

#### 18. `TroveType`, `InstallSource`, `InstallReason` define redundant `as_str` methods
- **File:** `conary-core/src/db/models/trove.rs:22,44,76`
- **Category:** Idiomatic Rust
- **Finding:** All three enums derive `AsRefStr` from strum which provides `.as_ref()`.
  They then define an `as_str` method that delegates to `self.as_ref()`. This is noted
  as "backwards compatibility" but adds unnecessary surface area. `as_ref()` returns
  `&str` and is equivalent.
- **Fix:** Deprecate the `as_str` methods and migrate callers to `.as_ref()`.

#### 19. `CompressionFormat::None` should be renamed to avoid shadowing `Option::None`
- **File:** `conary-core/src/compression/mod.rs:38`
- **Category:** Code Quality
- **Finding:** `CompressionFormat::None` shadows `Option::None` in match arms. The
  code works correctly because `CompressionFormat::None` is always fully qualified,
  but `Uncompressed` would be clearer.
- **Fix:** Rename to `CompressionFormat::Uncompressed` if a breaking rename is acceptable.

#### 20. Several unused `tracing::debug` import in migrations
- **File:** `conary-core/src/db/migrations/v21_v40.rs:6`, `v41_current.rs:6`
- **Category:** Code Quality
- **Finding:** `debug` is imported but only used in some migration functions. The later
  migrations (v46+) do not use `debug!` and only use `info!`.
- **Fix:** Remove unused `debug` import or add `#[allow(unused_imports)]`.

---

### Cross-Domain Notes

#### [Feature 2 - CAS/Filesystem] CAS path computation duplication
The `db/paths.rs::objects_dir()` computes the CAS root, but other modules
(install, gc, substituter, builder, chunking) independently compute `hash[..2]/hash[2..]`
subdirectory paths. This is tracked in the lintian memory as an anti-pattern.

#### [Feature 3 - Generation/EROFS] Transaction module depends on generation module
`transaction/mod.rs` imports `crate::generation::metadata::EROFS_IMAGE_NAME` and
`crate::filesystem::CasStore`. This coupling is architectural and intentional but means
the transaction module cannot be tested without the full generation stack.

---

### Strengths

1. **Parameterized queries everywhere**: All 69 tables worth of queries use `?1` bindings.
   No string interpolation of user data into SQL. This is confirmed across 44 model files
   and all migration scripts.

2. **Comprehensive security guards in parsers**: CPIO has `MAX_NAME_SIZE` (4 KiB) and
   `MAX_FILE_SIZE` (512 MB). DEB has `MAX_DEB_MEMBER_SIZE` (2 GB). RPM has a 4 GB
   file size check. Compression has `MAX_DECOMPRESS_SIZE` (2 GiB) decompression bomb
   protection. Path normalization uses `sanitize_path()` and `sanitize_filename()`.

3. **Schema versioning with function dispatch**: The migration system
   (`schema.rs:apply_migration`) maps version numbers to functions deterministically.
   Each migration runs in its own transaction with automatic rollback on failure.

4. **Normalized dependency tables**: The v49-v51 migrations added first-class
   `repository_provides`, `repository_requirements`, and
   `repository_requirement_groups` tables, replacing JSON-blob scanning with proper
   relational queries for the resolver.

5. **Version comparison correctness**: The `rpmvercmp` implementation in
   `version/mod.rs` correctly handles leading zeros, mixed alpha/numeric segments,
   and the RPM rule that digits always beat alpha segments. The test suite covers
   edge cases including empty epochs and cross-epoch comparison.

6. **Consistent batch insert pattern**: All high-volume insert operations
   (`Trove::batch_insert`, `FileEntry::batch_insert`, `RepositoryPackage::batch_insert`,
   `RepositoryProvide::batch_insert`) use `prepare_cached` for statement reuse,
   eliminating the 5-minute sync regression documented in the batch_insert commit.

---

### Recommendations

1. **Replace `expect()` in resolver pool interning with proper `Result` propagation.**
   These are the only production-path panics in the review scope. Six call sites in
   `provider/mod.rs` need to be converted. The change is mechanically simple since all
   callers already return `Result`.

2. **Add column-count assertions per model.** A one-line test per model file:
   `assert_eq!(Self::COLUMNS.split(", ").count(), 17)` catches column/ordinal drift
   at compile-test time rather than at runtime with corrupt data.

3. **Guard `satisfies_legacy` against non-RPM schemes.** The fallback to RPM version
   parsing for non-RPM installed packages is the most likely source of subtle
   version-ordering bugs as the multi-distro resolver matures. Add a scheme check
   that routes to `repo_version_satisfies` for Debian and Arch packages.

---

### Assessment

**Ready to merge?** Yes, with fixes for P0-1 and P0-2.

**Reasoning:** The codebase is mature, well-tested, and follows its stated conventions
consistently. The two P0 findings are unlikely to trigger in normal usage (the resolver
pool would need >4 billion entries, and the RPM fallback only fires for non-RPM packages
without scheme annotation), but they represent correctness violations that should be
fixed before the multi-distro resolver sees production load. The P1 and P2 findings are
improvements that can be addressed incrementally.

---

### Work Breakdown

These tasks are structured for `emerge` consumption. Each is independent unless noted.

1. **[P0] resolver: Replace expect() with Result in provider pool interning**
   - Files: `conary-core/src/resolver/provider/mod.rs`
   - 6 call sites: lines 122, 141, 165, 176, 186, 194
   - Convert `expect("...")` to `.map_err(|_| Error::InitError("..."))?`
   - Run `cargo test -p conary-core` to verify

2. **[P0] version: Guard satisfies_legacy against non-RPM schemes**
   - Files: `conary-core/src/resolver/graph.rs` (InstalledPackageVersion::satisfies_legacy)
   - Add scheme check: if scheme != Rpm, use `repo_version_satisfies` instead
   - Add test case: Debian version with multiple hyphens compared correctly

3. **[P1] db/models: Add column-count assertions to model tests**
   - Files: All model files with COLUMNS constant (~15 files)
   - Add `#[test] fn column_count() { assert_eq!(Self::COLUMNS.split(", ").count(), N); }`
   - One test per model struct

4. **[P1] trove: Make find_by_reason pub(crate) or escape LIKE wildcards**
   - File: `conary-core/src/db/models/trove.rs`
   - Escape `%` and `_` in input before `*` -> `%` conversion
   - Add test with `%` in pattern input

5. **[P1] repository: Add enabled filter to search query**
   - File: `conary-core/src/db/models/repository.rs`
   - Add `JOIN repositories r ON rp.repository_id = r.id WHERE r.enabled = 1` to search
   - Add test verifying disabled repo packages are excluded from search

6. **[P1] retry: Guard against max_attempts == 0**
   - File: `conary-core/src/repository/retry.rs`
   - Clamp `max_attempts` to `max(1, ...)` in both sync and async `with_retry`
   - Add test for 0-attempts case

7. **[P2] file_entry: Handle setuid/setgid/sticky in format_permissions**
   - File: `conary-core/src/db/models/file_entry.rs`
   - Add handling for mode bits 04000, 02000, 01000
   - Add tests for setuid binary, sticky directory

8. **[P2] resolver/provider: Key dependencies map on SolvableId not u32**
   - File: `conary-core/src/resolver/provider/mod.rs`
   - Change `HashMap<u32, Vec<SolverDep>>` to `HashMap<SolvableId, Vec<SolverDep>>`
   - Same for `removal_deps` field
