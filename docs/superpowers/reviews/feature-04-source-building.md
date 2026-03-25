## Feature 4: Source Building -- Review Findings

### Summary

The source building pipeline (recipe, derivation, bootstrap, derived) is architecturally
solid: derivation IDs are properly content-addressed with injection-resistant canonical
serialization, the Kitchen provides layered build isolation, and the bootstrap pipeline
has clean stage management with atomic checkpointing. The main concerns are
whole-file reads for checksum verification (memory exhaustion on large archives),
command injection surface in the chroot build path, and a checksum validation gap
where md5 is accepted despite the parser rejecting it.

---

### P0 -- Data Loss / Security / Production Crash

**[P0] security: chroot build path does not escape workdir in command string**
- File: `conary-core/src/recipe/kitchen/cook.rs:509-513`
- Issue: In `run_build_step_direct` with a sysroot configured, the `chroot_workdir`
  path is interpolated directly into a shell command string (`cd {} && {}`). Unlike
  the isolated path (line 455) which shell-escapes the workdir with single quotes,
  the chroot path uses bare `Display` formatting. A workdir containing shell
  metacharacters (spaces, semicolons, backticks) would break the command or allow
  injection.
- Impact: A crafted recipe `workdir` field could inject arbitrary shell commands
  inside the chroot as root.
- Fix: Apply the same single-quote escaping used in `run_build_step_isolated`:
  ```rust
  let escaped = format!("'{}'", chroot_workdir.display().to_string().replace('\'', "'\\''"));
  let script = format!("cd {} && {}", escaped, command);
  ```

**[P0] security: PKGBUILD checksum algorithm mismatch -- sha512/b2 silently labeled sha256**
- File: `conary-core/src/recipe/pkgbuild.rs:130-147`
- Issue: `convert_pkgbuild` checks for `sha256sums`, then falls back to `sha512sums`,
  `b2sums`, and `md5sums`. But regardless of which algorithm was found, the checksum
  is always prefixed with `"sha256:"` (line 143). If the source PKGBUILD uses
  SHA-512 checksums, the converted recipe will have `sha256:<sha512-hex>`, which will
  never verify correctly -- but more dangerously, if `md5sums` is used, the short
  hash prefixed with `sha256:` will pass format validation and reach the download
  path where `verify_file_checksum` will compute a SHA-256 of the file and compare
  it against an MD5 value, always returning false. This is a silent build failure,
  not a security hole, but it masks the real problem.
- Impact: All PKGBUILD conversions using non-sha256 checksums produce recipes that
  can never build. Users will get cryptic checksum mismatch errors.
- Fix: Track which algorithm was found and prefix appropriately:
  `sha256:` / `sha512:` / `b2:` / `md5:`. Or reject non-sha256 with a warning
  suggesting the user provide a sha256 checksum.

---

### P1 -- Incorrect Behavior / Silent Failure / Missing Validation

**[P1] correctness: md5 accepted by archive.rs but rejected by recipe parser**
- File: `conary-core/src/recipe/kitchen/archive.rs:58` vs `conary-core/src/recipe/parser.rs:40-47`
- Issue: `verify_file_checksum` in archive.rs accepts `md5` as a valid algorithm
  (line 58), but `validate_recipe` rejects any checksum that does not start with
  `sha256:` or `xxh128:`. This inconsistency means md5 checksums pass verification
  but fail validation -- or if validation is skipped (e.g., programmatic recipe
  construction), md5 is silently accepted.
- Impact: Confusing behavior; md5 should be explicitly rejected everywhere since
  it is cryptographically broken.
- Fix: Remove the md5 arm from `verify_file_checksum` or add `md5:` to the parser's
  accepted prefixes. The former is recommended for security.

**[P1] correctness: whole-file read for checksum verification (memory exhaustion)**
- File: `conary-core/src/recipe/kitchen/archive.rs:49`
- Issue: `verify_file_checksum` reads the entire file into memory with `fs::read(path)`.
  Source archives can be hundreds of MB (GCC is 80MB, LLVM is 160MB). In the bootstrap
  pipeline this is called for every package.
- Impact: High memory consumption, potential OOM on memory-constrained build machines.
- Fix: Use streaming I/O via `crate::hash::hash_reader` with a `BufReader`, matching
  the pattern already used in `compose.rs:erofs_image_hash()`.

**[P1] correctness: whole-file read for cache integrity verification**
- File: `conary-core/src/recipe/cache.rs:418`
- Issue: Same pattern as above -- `checksum_file` reads the entire cached CCS package
  into memory to compute its SHA-256.
- Impact: CCS packages can be large; this wastes memory when streaming is available.
- Fix: Use `hash::hash_reader` with `BufReader` as done elsewhere in the codebase.

**[P1] correctness: PKGBUILD `source=()` with unquoted values not parsed**
- File: `conary-core/src/recipe/pkgbuild.rs:316-345`
- Issue: `extract_array` first tries to match quoted values, then falls back to
  whitespace-split for unquoted values. However, PKGBUILD arrays can contain
  entries like `source=(https://url.com/file.tar.gz)` with no quotes at all.
  The regex `["']([^"']+)["']` requires at least one quote character. The fallback
  handles this via `split_whitespace`, but the `trim_matches` on line 334 only
  strips quotes, not parentheses -- so the URL would retain embedded parens from
  the regex capture.
- Impact: Some legitimate PKGBUILDs with unquoted URLs may produce malformed
  source entries. Edge case but real-world PKGBUILDs exist in this format.
- Fix: Test with unquoted PKGBUILD arrays and ensure the fallback path handles them.

**[P1] correctness: `extract_array` compiles regex on every call**
- File: `conary-core/src/recipe/pkgbuild.rs:318-319`
- Issue: `extract_array` builds a new `Regex` from a runtime-constructed pattern
  on every invocation. While `Regex::new` is not free, the real concern is that
  multiple calls (source, sha256sums, sha512sums, b2sums, md5sums, depends,
  makedepends -- 7+ calls per PKGBUILD) each compile a fresh regex.
- Impact: Minor performance issue, not a bug. Acceptable for a conversion tool
  used interactively.
- Fix: Accept as-is or cache compiled regexes per array name.

**[P1] correctness: `convert_pkgbuild_url` double-replaces when pkgname appears in version**
- File: `conary-core/src/recipe/pkgbuild.rs:413-452`
- Issue: The function first replaces `$pkgname`/`${pkgname}` with `%(name)s`, then
  on line 452 replaces the literal `pkgver` value with `%(version)s`. If the package
  name happens to be a substring of the version (unlikely but possible, e.g., package
  "2" version "2.0"), the name replacement on line 440 could corrupt the URL. The
  `pkgname.len() >= 3` guard mitigates this for very short names but not all cases.
- Impact: Edge case, unlikely in practice, but the function has no test for this.
- Fix: Apply version replacement before name replacement, or use a single-pass
  replacement strategy.

---

### P2 -- Improvement Opportunity / Minor Inconsistency

**[P2] anti-pattern: CAS path computation duplicated in install.rs**
- File: `conary-core/src/derivation/install.rs:222-229`
- Issue: `cas_object_path` implements the `hash[..2]/hash[2..]` two-level CAS
  path computation. This same pattern is duplicated in gc.rs, substituter.rs,
  builder.rs, and chunking.rs (per agent memory).
- Impact: Maintenance burden, risk of divergence if the layout changes.
- Fix: Use `CasStore::hash_to_path()` or extract a shared free function.

**[P2] code-quality: `archive.rs` panics on non-UTF-8 paths**
- File: `conary-core/src/recipe/kitchen/archive.rs:76-77`
- Issue: `extract_archive` calls `.expect("archive path must be valid utf-8")` and
  `.expect("dest path must be valid utf-8")` on line 76-77. Similarly, `apply_patch`
  on line 119 uses `.expect("patch path must be valid utf-8")`.
- Impact: Non-UTF-8 paths (rare on Linux but possible with mount points or locales)
  would panic in production.
- Fix: Return an error instead of panicking, consistent with how `download_file`
  handles non-UTF-8 on line 14-16.

**[P2] code-quality: `build_script_hash` does not include `script_file` field**
- File: `conary-core/src/derivation/recipe_hash.rs:60-93`
- Issue: The `build_script_hash` function hashes `setup`, `configure`, `make`,
  `install`, `check`, `post_install`, environment, and workdir. But it does not
  hash the `script_file` field (Recipe.build.script_file), which is an alternative
  to inline commands. If a recipe uses `script_file`, the build script hash would
  be empty/constant regardless of the script content.
- Impact: Two recipes with different `script_file` paths would produce the same
  derivation ID, causing false cache hits.
- Fix: Hash the `script_file` path (and ideally the file's content hash) as part
  of `build_script_hash`.

**[P2] code-quality: `expand_variables` does not expand `%(destdir)s`**
- File: `conary-core/src/derivation/recipe_hash.rs:27-45`
- Issue: `expand_variables` expands `%(name)s` and `%(version)s` but not
  `%(destdir)s`. The doc comment on `Recipe::substitute` (format.rs:49) shows
  `%(destdir)s` as a built-in variable. Since `build_script_hash` passes the
  result through `expand_variables` rather than `Recipe::substitute`, any
  `%(destdir)s` references in build commands remain unexpanded in the hash input.
  This is intentional per agent memory ("diverges from Recipe::substitute() for
  hash determinism"), but means two recipes identical except for destdir usage
  patterns would hash differently.
- Impact: Documented intentional divergence. Not a bug, but worth noting for
  anyone maintaining this code.

**[P2] code-quality: `output_hash` v1 does not include file permissions**
- File: `conary-core/src/derivation/output.rs:78-96`
- Issue: `compute_output_hash` (v1) only hashes `path` and `hash` for files.
  Two derivation outputs with identical file contents but different permissions
  (e.g., 0o755 vs 0o644 on a binary) would produce the same output hash. The
  `compute_output_hash_v2` on line 103 fixes this, but `OutputManifest::new()`
  still uses v1 by default.
- Impact: Permission-only changes would not be detected by the derivation cache.
- Fix: Switch `OutputManifest::new()` to use v2, or plan a migration path.

**[P2] code-quality: `graph.rs` find_cycles may report duplicate cycles**
- File: `conary-core/src/recipe/graph.rs:220-266`
- Issue: `find_cycles` uses DFS and reports a cycle every time the recursion
  stack hits an already-visited node. For complex graphs this can report the same
  cycle from multiple entry points (e.g., A->B->C->A reported from each of A, B, C).
- Impact: `suggest_bootstrap_edges` might add redundant suggestions, but the
  bootstrap edge set is a `HashSet` so duplicates are harmless.
- Fix: De-duplicate cycles before returning, or accept as-is since downstream
  handles duplicates.

**[P2] improvement: `PKGBUILD` `convert_function_body` replaces `$srcdir` with `.`**
- File: `conary-core/src/recipe/pkgbuild.rs:461`
- Issue: `$srcdir` is replaced with `.` (current directory). In the Conary Kitchen,
  the working directory is already the source directory, so this is correct. But
  if the converted recipe is used outside the Kitchen, `.` may not be meaningful.
- Impact: Minor documentation gap -- the replacement is noted in code but not in
  the warnings returned to the user.

**[P2] improvement: `provenance_capture.rs` patches not sorted before DNA hash**
- File: `conary-core/src/recipe/kitchen/provenance_capture.rs:217`
- Issue: The comment says "Patches (sorted for determinism)" but patches are
  iterated in insertion order (`self.patches` is a `Vec`), not sorted. Build deps
  are explicitly sorted on line 231, but patches are not.
- Impact: If patches are applied in a different order (unlikely since recipes
  define a fixed order), the DNA hash would differ. Low risk since patch order
  is deterministic from the recipe, but the comment is misleading.
- Fix: Either sort patches by source/hash before hashing, or update the comment
  to say "Patches (in recipe order)".

---

### P3 -- Style / Naming / Minor Improvement

**[P3] style: `VAR_RE` regex does not handle multi-line values**
- File: `conary-core/src/recipe/pkgbuild.rs:52`
- Issue: The variable regex `^([a-zA-Z_]...)=["']?([^"'\n]*)["']?$` does not
  handle quoted values that span multiple lines. This is documented as a limitation.
- Impact: None beyond what the module doc already states.

**[P3] style: `CROSS_TOOLS_ORDER` uses string matching for `libstdc++` -> `libstdcxx.toml`**
- File: `conary-core/src/bootstrap/cross_tools.rs:202`
- Issue: The `replace("++", "xx")` mapping is implicit. If someone adds a recipe
  with `+` in its name, this replacement would be surprising.
- Impact: Cosmetic, unlikely to cause real issues.

**[P3] style: `BuildStage` and `Stage` are separate enums with overlapping semantics**
- File: `conary-core/src/recipe/format.rs:436` and `conary-core/src/derivation/build_order.rs:24`
- Issue: `BuildStage` (Stage0/Stage1/Stage2/Final) is used by the recipe system,
  while `Stage` (Toolchain/Foundation/System/Customization) is used by the derivation
  build order. Both classify packages into build phases but use different taxonomies.
- Impact: Potential confusion for contributors. Both are needed (different abstraction
  levels), but could benefit from cross-referencing documentation.

**[P3] style: `BootstrapStage::all()` returns a static reference to a fixed array**
- File: `conary-core/src/bootstrap/stages.rs:32-41`
- Issue: This is fine idiomatically, but `strum`-style derive macros could generate
  this automatically. Not a bug, just a note for potential simplification.

---

### Cross-Domain Notes

**[Cross-Domain] `conary-core/src/container/` -- ContainerConfig referenced by cook.rs**
- The `ContainerConfig::pristine_for_bootstrap()` and `ContainerConfig::pristine()`
  methods are called from `cook.rs:368-376` but live outside the review scope.
  The Kitchen's security depends on these methods correctly configuring namespace
  isolation. A separate review of the container module should verify the mount
  namespace setup.

**[Cross-Domain] `conary-core/src/filesystem/path.rs` -- `safe_join()` used by install.rs**
- `install.rs:216` correctly uses `safe_join()` for path traversal protection.
  This is the right pattern. The `derived/builder.rs:347-378` also has its own
  path traversal check via `canonicalize()` + `starts_with()`, which is correct
  but uses a different approach than `safe_join()`. Consider standardizing.

---

### Strengths

1. **Derivation ID canonical serialization** (`derivation/id.rs`): The
   `validate_inputs` function rejects newlines and colons in field positions where
   they could corrupt the canonical format. This is textbook injection prevention
   for content-addressed systems. Tests cover all the injection vectors.

2. **Build environment lifecycle management** (`derivation/environment.rs`):
   Both `BuildEnvironment` and `MutableEnvironment` implement `Drop` with unmount
   cleanup, and `MutableEnvironment` tracks seed identity via a `.seed_id` marker
   file for stale-upper-dir detection. This is production-quality mount management.

3. **ChrootEnv mount tracking** (`bootstrap/chroot_env.rs`): The mount vector
   with reverse-order teardown in `Drop` is a clean RAII pattern that prevents
   mount leaks even on panic.

4. **Derived package path traversal protection** (`derived/builder.rs:350-378`):
   The `apply_single_file_patch` function canonicalizes both the work directory and
   the patch target, then verifies `starts_with`. This blocks `..` traversal in
   patch file paths.

5. **Test coverage**: Every module has in-file `#[cfg(test)]` tests. The
   derivation module has a shared `test_helpers.rs` to avoid recipe construction
   boilerplate. Coverage is comprehensive for the determinism properties.

---

### Recommendations

1. **Fix the chroot command injection** (P0): The direct `format!("cd {} && {}",
   chroot_workdir, command)` path in `cook.rs` needs the same shell-escaping
   applied to the isolated path. This is the highest-priority fix.

2. **Switch to streaming checksum verification** (P1): Replace `fs::read()` in
   `archive.rs:verify_file_checksum` and `cache.rs:checksum_file` with
   `hash::hash_reader` + `BufReader`. The pattern already exists in
   `compose.rs:erofs_image_hash()` -- just replicate it.

3. **Fix PKGBUILD checksum algorithm labeling** (P0): Track which checksum array
   was matched and use the correct prefix. Alternatively, reject non-sha256
   with a warning since `verify_file_checksum` cannot verify sha512 or b2 anyway.

---

### Assessment

**Ready to merge?** No -- with fixes for P0 items.

**Reasoning:** Two P0 findings need attention before this code handles untrusted
input: the chroot path injection in `cook.rs` and the PKGBUILD checksum algorithm
mislabeling. The streaming checksum fix (P1) is strongly recommended for the
bootstrap pipeline where 80-160MB archives are common. The remaining P2/P3 items
are improvement opportunities that can be addressed incrementally.

---

### Work Breakdown

1. **[P0] Fix chroot workdir shell escaping in cook.rs** -- Apply single-quote escaping
   to `chroot_workdir` in `run_build_step_direct`, matching the pattern on line 455.
   Add a test with a workdir containing a space.

2. **[P0] Fix PKGBUILD checksum algorithm prefix** -- Track matched algorithm in
   `convert_pkgbuild`. Use correct prefix or reject non-sha256. Add tests for
   sha512sums and b2sums PKGBUILDs.

3. **[P1] Remove md5 from verify_file_checksum** -- Delete the md5 arm in archive.rs
   or return an explicit "md5 is not supported" error.

4. **[P1] Streaming checksum verification** -- Replace `fs::read` with `BufReader` +
   `hash_reader` in `archive.rs:verify_file_checksum` and `cache.rs:checksum_file`.

5. **[P2] Hash script_file in build_script_hash** -- Add the `script_file` path to
   the hash computation in `recipe_hash.rs`.

6. **[P2] Eliminate CAS path duplication in install.rs** -- Replace `cas_object_path`
   with the shared `CasStore::hash_to_path()` or a free function.

7. **[P2] Fix expect() calls in archive.rs** -- Replace `.expect()` on lines 76-77
   and 119 with proper error returns.

8. **[P2] Switch OutputManifest::new() to v2 hash** -- Or document the migration plan
   for existing cached derivations.
