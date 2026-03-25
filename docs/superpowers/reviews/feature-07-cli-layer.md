## Feature 7: CLI Layer -- Review Findings

### Summary

The CLI layer is well-structured across ~35,000 lines with 210 command handlers,
28 CLI definition files, and 12 integration test files. The architecture is sound:
clean separation between CLI definitions (`src/cli/`) and command implementations
(`src/commands/`), consistent use of the `open_db()` helper, and correct file
headers on all 85+ source files. The main concerns are: (1) a self-update flow
that accepts `--version` but silently ignores it while still performing an update,
(2) the Daemon `--foreground` flag is accepted but silently discarded, (3) significant
dead code behind `#[allow(dead_code)]` that should be cleaned up or tracked, and
(4) the `Export --oci` flag defaults to true and has no other format, making it
misleading to users.

---

### P0 -- Critical

**[P0] [correctness]: self-update `--version` flag silently ignored -- installs latest anyway**
- File: `src/commands/self_update.rs:40-43`
- Issue: When the user passes `--version 0.5.0`, the command prints a `[NOT YET IMPLEMENTED]`
  warning but then proceeds to download and install the **latest** version anyway. A user
  requesting a specific version likely does not expect to receive a different one. This is
  silent data mutation -- the binary gets replaced with an unintended version.
- Impact: User explicitly requests version X, gets version Y installed. If the latest has
  a breaking change or bug, the user has no recourse. This is worse than refusing the flag
  entirely.
- Fix: Either bail with an error when `--version` is specified (since it is not implemented),
  or hide the flag. Do not proceed with a different version than what was requested.

---

### P1 -- Incorrect Behavior / Significant Issues

**[P1] [correctness]: Daemon `--foreground` flag accepted but silently discarded**
- File: `src/main.rs:1654` (`foreground: _`)
- Issue: The `--foreground` flag is bound to `_` (wildcard), meaning it is parsed by clap
  but never passed to `DaemonConfig` or used in any logic. The daemon always runs in the
  foreground regardless of the flag's value.
- Impact: User expects `conary daemon` (without `--foreground`) to daemonize, but it never
  does. The flag creates a false expectation.
- Fix: Either wire `foreground` into `DaemonConfig` and implement daemonization, or remove
  the flag from the CLI definition and document that the daemon always runs in the foreground
  (users can use systemd).

**[P1] [correctness]: `Export --oci` flag always true, no alternative formats**
- File: `src/cli/mod.rs:605-607`
- Issue: The `--oci` flag has `default_value_t = true` and is bound as `oci: bool`, but
  `--no-oci` would set it to false -- yet there is no non-OCI export path. In `main.rs:1863`,
  the value is bound as `oci: _` and ignored entirely.
- Impact: A user passing `--no-oci` sees no error but gets OCI output anyway. This is
  confusing for anyone reading `--help`.
- Fix: Remove the `--oci` flag entirely. If future formats are planned, use a `--format`
  enum with `oci` as the only current variant (like `ccs inspect --format`).

**[P1] [code-quality]: 22 `#[allow(dead_code)]` annotations in non-test command code**
- Files: Multiple -- `install/resolve.rs`, `install/batch.rs`, `install/execute.rs`,
  `install/blocklist.rs`, `install/dep_resolution.rs`, `install/system_pm.rs`,
  `install/scriptlets.rs`, `install/mod.rs`, `federation.rs`, `model.rs`,
  `derived.rs:393`, `adopt/convert.rs:27`
- Issue: 22 instances of `#[allow(dead_code)]` in production command code. Some are
  annotated "TODO: wire into X" (`derived.rs:393`, `adopt/convert.rs:27`), meaning they
  are stubs that were never connected. Others are "reserved for future" with no tracking.
- Impact: Accumulated dead code makes the codebase harder to review and reason about.
  Unconnected stubs (`cmd_derive_mark_stale`, `BatchConvertOptions`) give the impression
  of completeness while features are actually missing.
- Fix: For each `#[allow(dead_code)]`:
  - If the function is needed soon, create a tracking issue and reference it in the comment.
  - If it is speculative, remove it. Dead code can be resurrected from git history.
  - The `install/` module has the highest concentration -- consider a focused cleanup pass.

**[P1] [correctness]: `cmd_automation_history` is a complete stub**
- File: `src/commands/automation.rs:491-517`
- Issue: `cmd_automation_history` accepts `db_path`, `limit`, `category`, `status`, and
  `since` parameters but ignores all of them except for printing filter labels. It always
  prints "No automation history recorded yet." The TODO comment at line 489 confirms this.
- Impact: Users running `conary automation history` get misleading output -- the command
  appears to work but returns no data regardless of system state.
- Fix: Either implement the DB query (the `automation_actions` table exists in the schema),
  or mark the command as `#[command(hide = true)]` until it is functional.

**[P1] [correctness]: `cmd_state_restore` is a stub that shows a plan but never applies it**
- File: `src/commands/state.rs:217`
- Issue: The `state revert` command computes a restoration plan and prints it, but then
  prints `[NOT YET IMPLEMENTED]` and exits successfully. It does not return an error, so
  scripted usage (`conary system state revert 5 && echo "restored"`) would falsely report
  success.
- Impact: Silent no-op on a destructive operation. A user reverting state after a bad
  install would believe the revert succeeded.
- Fix: Return an error (non-zero exit) when `dry_run` is false and the apply is not
  implemented. Something like:
  `anyhow::bail!("State restore apply is not yet implemented. Use 'conary rollback' for individual changesets.")`

---

### P2 -- Improvements / Minor Inconsistencies

**[P2] [architecture]: `cmd_repo_add` takes 12 positional parameters**
- File: `src/commands/repo.rs` (called from `src/main.rs:617-632`)
- Issue: `cmd_repo_add` accepts 12 individual parameters instead of an options struct.
  Compare with `cmd_install` which uses `InstallOptions`, or `cmd_model_apply` which uses
  `ApplyOptions`.
- Impact: Maintenance burden -- adding a new repo option requires changing 3 places
  (CLI def, main.rs dispatch, function signature). Easy to mix up parameter order.
- Fix: Create a `RepoAddOptions` struct, similar to `InstallOptions`.

**[P2] [conventions]: `expect()` used for progress bar template strings in production code**
- File: `src/commands/progress.rs` (lines 39, 49, 74, 135, 193, 202, 234, 281, 289, 347, 391, 400, 424)
  and `src/commands/repo.rs:204`
- Issue: 14 uses of `.expect("Invalid spinner template")` on `ProgressStyle::template()`.
  Per project conventions, `expect()` should only be used in tests and infallible cases.
  These ARE infallible (literal string templates), but the pattern is noisy and could be
  replaced with a helper.
- Impact: No runtime risk (templates are compile-time constants), but inconsistent with
  the codebase policy documented in CLAUDE.md.
- Fix: Create a `fn progress_style(template: &str) -> ProgressStyle` helper in
  `progress.rs` that wraps the expect, reducing noise and centralizing the pattern.

**[P2] [code-quality]: `distro list` is hardcoded**
- File: `src/commands/distro.rs:66-75`
- Issue: The TODO at line 66 notes this should be driven from the database/registry.
  Currently hardcodes 5 distros including `ubuntu-oracular` (Ubuntu 24.10) which is
  already end-of-life as of July 2025.
- Impact: Stale data shown to users. Adding a new distro requires a code change and
  release instead of a registry update.
- Fix: Query the `distros` table or canonical registry. Fall back to the hardcoded list
  if the DB is not initialized.

**[P2] [consistency]: Inconsistent async patterns across commands**
- Files: Various in `src/commands/`
- Issue: Many commands are `pub async fn` but contain no `.await` calls -- they are
  synchronous functions wrapped in async. Examples include nearly all the query commands,
  state commands, trigger commands, label commands, etc. Meanwhile, generation commands
  like `cmd_generation_build` and `cmd_generation_switch` are synchronous (non-async)
  and are called directly without `.await` in main.rs.
- Impact: Minor -- tokio handles this fine. But it means the function signatures are
  inconsistent, making it harder to know which commands actually need async.
- Fix: This is a long-term cleanup. The current approach works and changing it would be
  churn. Document the convention: all `cmd_*` functions are async for uniformity even
  if they don't use it, to allow future async additions without signature changes.

**[P2] [code-quality]: `no_capture` field in `InstallOptions` has confusing semantics**
- File: `src/commands/install/mod.rs:86`
- Issue: The field is named `no_capture` but in the CLI definition (`src/cli/mod.rs:193`)
  it means "disable scriptlet capture during CCS conversion." The name reads as "no state
  capture after install" which is a completely different concept. The `InstallOptions`
  default is `false` (capture enabled), which is correct, but the naming is misleading.
- Impact: Maintainer confusion. Someone reading `no_capture: true` would need to check
  the CLI help to understand what it means.
- Fix: Rename to `no_scriptlet_capture` or `skip_capture` with a doc comment.

**[P2] [correctness]: `remove.rs:325` prints redundant file count**
- File: `src/commands/remove.rs:325`
- Issue: `println!("  Files removed: {}/{}", removed_count, regular_files.len())` --
  `removed_count` is defined as `regular_files.len()` on line 286, so this always prints
  `N/N`. In the composefs-native model, individual file removal tracking is not applicable
  (confirmed by the comment on line 329), making this fraction meaningless.
- Impact: Confusing output -- the denominator adds no information.
- Fix: Print just `"  Files removed: {removed_count}"` or, better, print the total
  including directories: `"  Entries removed: {} files, {} directories"`.

**[P2] [security]: `self-update` signature verification happens before download**
- File: `src/commands/self_update.rs:72-80`
- Issue: The code verifies the signature of the SHA-256 hash from the API response
  *before* downloading the CCS package. This verifies that the server claims the hash
  is signed, but does not verify the actual downloaded content matches. The actual
  content verification happens inside `download_update_with_progress` (which checks
  the downloaded file's hash against `sha256`), so there is no actual vulnerability here.
  But the code flow is misleading -- the comment says "Verify signature before downloading"
  which implies the download might be skipped on failure, but verification of the
  *content* only happens after download.
- Impact: No security gap (the hash is verified on the downloaded content), but the code
  flow misleads auditors.
- Fix: Move the signature check after download, or add a clarifying comment that this
  verifies the server's claimed hash is authentic, while content integrity is checked
  during download.

**[P2] [ai-slop]: Bootstrap commands have highly repetitive parameter patterns**
- File: `src/cli/bootstrap.rs` (CrossTools, TempTools, System, Config, Tier2 variants)
- Issue: Five bootstrap phase commands (`CrossTools`, `TempTools`, `System`, `Config`,
  `Tier2`) share nearly identical parameter sets (work_dir, lfs_root, jobs, verbose,
  skip_verify). The dispatch in `main.rs` also has near-identical patterns for each.
- Impact: Copy-paste smell. Adding a parameter to all phases requires editing 5 CLI
  definitions and 5 dispatch blocks.
- Fix: Extract a `BootstrapPhaseArgs` struct with common fields, and flatten it into
  each variant. This reduces each variant to just the struct flatten plus any
  phase-specific args.

---

### P3 -- Style / Nitpicks

**[P3] [conventions]: Missing comment for Federation Commands section in main.rs**
- File: `src/main.rs:1565-1567`
- Issue: There are two consecutive section comment blocks ("Federation Commands" and
  "Trust Commands") but the Federation commands dispatch actually starts at line 1613.
  The "Federation Commands" header is empty -- the Trust commands are dispatched first.
- Impact: Misleading to someone scanning the file by section headers.
- Fix: Reorder so Trust commands have their own header, or merge the headers.

**[P3] [conventions]: `Revert` vs `Rollback` naming inconsistency**
- File: `src/cli/state.rs:42` (CLI name `Revert`) vs `src/commands/state.rs:189`
  (function `cmd_state_restore`)
- Issue: The CLI subcommand is `revert`, the internal function is `cmd_state_restore`,
  and there is a separate `Rollback` subcommand for changesets. The three terms
  (revert, restore, rollback) all mean slightly different things but overlap enough
  to confuse.
- Impact: Minor user confusion in help text.
- Fix: Align naming -- the CLI says `revert`, so the function should be
  `cmd_state_revert`. Keep `rollback` separate for changeset-level operations.

**[P3] [style]: `system init` prints "Use 'conary repo-sync'" but the command is `conary repo sync`**
- File: `src/commands/system.rs:82`
- Issue: The output says `Use 'conary repo-sync' to download metadata.` but the actual
  command is `conary repo sync` (two words, not hyphenated).
- Impact: User copy-pastes the suggestion and gets an error.
- Fix: Change to `Use 'conary repo sync' to download metadata.`

**[P3] [style]: `derived.rs:386` prints old command name**
- File: `src/commands/derived.rs:386`
- Issue: Prints `Use 'conary derive-build <name>' to rebuild.` but the actual command is
  `conary derive build <name>` (subcommand, not hyphenated).
- Impact: Same as above -- user gets an error when copy-pasting.
- Fix: Change to `Use 'conary derive build <name>' to rebuild.`

---

### Cross-Domain Notes

**[Cross-Domain] [conary-core]: `detect_package_format` in `src/commands/mod.rs:211-240` does file I/O**
- This function opens files and reads magic bytes. It lives in the CLI crate but is
  pure format detection logic. It should live in `conary-core::packages` alongside
  the format parsers. The CLI layer should be a thin wrapper, not contain I/O logic.

**[Cross-Domain] [conary-core]: `PackageFormatType` enum duplicates `conary_core::packages::PackageFormat`**
- File: `src/commands/mod.rs:164-169`
- Issue: The CLI defines its own `PackageFormatType` enum with `Rpm/Deb/Arch` variants.
  `conary_core::packages` already has a similar type. The duplication means format
  detection and format-specific logic are split across crates unnecessarily.

---

### Strengths

1. **Consistent file headers**: Every single `.rs` file (85 in commands, 28 in CLI, 12
   in tests) has the correct `// path/to/file.rs` header. 100% compliance.

2. **Clean CLI/command separation**: CLI definitions in `src/cli/` are pure clap structs
   with no business logic. Command implementations in `src/commands/` call into
   `conary-core`. This separation is maintained rigorously across 210 command handlers.

3. **Shared helpers avoid duplication**: `open_db()`, `create_state_snapshot()`,
   `format_bytes()`, and `hint_unconfigured_source_policy()` in `commands/mod.rs`
   prevent each command from reinventing database opening and state management.

4. **Good help text quality**: CLI definitions have meaningful `///` doc comments that
   serve as `--help` text. Complex commands include detailed `long_about` with examples
   and caveats (e.g., `Cook`, `Adopt`, `Model Apply`, `CCS Install`).

5. **Feature gating is correct**: Server-only commands (`Daemon`, `Remi`, `RemiProxy`,
   `IndexGen`, `Prewarm`, `Server`, `Scan`, `SignTargets`, `RotateKey`) are properly
   behind `#[cfg(feature = "server")]` in both CLI definitions and main.rs dispatch.

6. **Export command has thorough tests**: `src/commands/export.rs` has 8 unit tests
   covering OCI layout structure, manifest validity, layer content, config labels,
   blob integrity, hex digest, and fallback behavior. Well-structured test data setup.

7. **Smart dispatch pattern**: `install @collection` and `update @collection` detect the
   `@` prefix and route to collection-specific handlers. Clean UX design.

8. **Error messages are generally actionable**: Remove checks for pinned packages and
   suggests `conary unpin`. Dependency breakage lists affected packages and suggests
   `conary whatbreaks`. Multiple versions prompts for `--version`.

---

### Issues

Issues are listed above organized by severity (P0 through P3), then by file.

---

### Recommendations

1. **Fix the self-update `--version` flag immediately (P0)**. Either bail with an error
   when `--version` is specified, or remove the flag from the CLI until it is implemented.
   A command that accepts `--version X` but installs version Y is a trust violation.

2. **Audit all `[NOT YET IMPLEMENTED]` stubs for silent success (P1)**. Seven commands
   print a "not yet implemented" message but return `Ok(())`. Any command that accepts
   parameters promising action but takes none should return an error code so scripts
   don't falsely report success. Priority targets: `state revert`, `automation history`.

3. **Create `RepoAddOptions` and `BootstrapPhaseArgs` structs (P2)**. These two areas
   have the most parameter-passing overhead. Extracting structs would reduce the main.rs
   dispatch code and make future parameter additions single-point changes.

---

### Assessment

**Ready to merge?** No -- with fixes for P0 and P1.

**Reasoning:** The P0 self-update `--version` issue is a user trust violation -- the
command does the opposite of what was requested. The P1 stub commands that return success
without acting (state revert, automation history) are incorrect behavior. Everything else
is cleanup-grade. The CLI architecture is solid and the codebase conventions are followed
with high consistency.

---

### Work Breakdown

1. **[P0] self-update --version**: Bail with error when `--version` is specified but
   unimplemented. (1 line change in `self_update.rs`)

2. **[P1] Daemon --foreground**: Either wire into DaemonConfig or remove the flag.
   (Small -- CLI + main.rs change)

3. **[P1] Export --oci**: Remove the flag, add `--format oci` enum if needed later.
   (CLI def + main.rs, ~10 lines)

4. **[P1] Stub commands return errors**: Fix `cmd_state_restore`, `cmd_automation_history`
   to return errors when they cannot fulfill the request. (~5 lines each)

5. **[P2] RepoAddOptions struct**: Extract from `cmd_repo_add` 12-param signature.
   (commands/repo.rs + main.rs, ~30 lines)

6. **[P2] BootstrapPhaseArgs struct**: Extract common bootstrap phase parameters.
   (cli/bootstrap.rs + main.rs, ~40 lines)

7. **[P2] Dead code cleanup pass**: Audit 22 `#[allow(dead_code)]` annotations in
   commands/. Remove speculative code, track needed code with issues.

8. **[P3] Fix stale command names in output**: `repo-sync` -> `repo sync`,
   `derive-build` -> `derive build`. (2 string changes)

9. **[P3] Fix section comment ordering in main.rs**: Federation/Trust header alignment.
   (Comment-only change)
