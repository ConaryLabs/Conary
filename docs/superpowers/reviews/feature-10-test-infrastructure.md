## Feature 10: Test Infrastructure -- Review Findings

### Summary

The conary-test crate is a well-structured declarative test engine with Podman containers, WAL-backed result streaming, MCP integration, and a rich assertion vocabulary. The architecture is clean: config parsing is well-separated from execution, the `ContainerBackend` trait enables proper unit testing, and the WAL provides genuine resilience for result delivery. However, **the crate does not compile on main** due to a recent refactor that extracted `container_setup.rs` without adding the module declaration, and removed a `From<ConaryTestError> for StructuredError` impl that handlers still reference. There is also a significant code duplication issue where container initialization logic exists in three places. Beyond the build breakage, the codebase is solid -- the assertion engine, variable substitution, and container lifecycle management are all well-tested.

---

### P0: Build Breakage / Data Loss / Security

**[P0] [correctness]: Crate does not compile -- missing module declarations for `container_setup` and `mock`**
- File: `conary-test/src/engine/mod.rs:1-10`, `conary-test/src/container/mod.rs:1-12`
- Issue: Two module files exist on disk but are not declared in their parent `mod.rs`:
  1. `engine/container_setup.rs` -- created as an untracked file, referenced by `runner.rs:430` as `crate::engine::container_setup::initialize_container_state`. Causes `E0433`.
  2. `container/mock.rs` -- added in commit `c277f77` but `container/mod.rs` was never updated. Referenced by test code in `executor.rs:578`, `runner.rs:664`, and `container_coordinator.rs:159`.
- Impact: The entire `conary-test` crate cannot compile. No tests can run, no server can start. CI is broken.
- Fix: Add `pub(crate) mod container_setup;` to `engine/mod.rs` and `#[cfg(test)] pub(crate) mod mock;` to `container/mod.rs`.

**[P0] [correctness]: Missing `From<ConaryTestError> for StructuredError` impl**
- File: `conary-test/src/server/handlers.rs:57,98,183,222`
- Issue: Four handler error paths call `StructuredError::from(e)` where `e: ConaryTestError`, but no `From<ConaryTestError> for StructuredError` implementation exists anywhere in the crate. This causes `E0308: expected StructuredError, found ConaryTestError` at each call site.
- Impact: Build failure; the HTTP server cannot compile.
- Fix: Add an `impl From<ConaryTestError> for StructuredError` that maps each `ConaryTestError` variant to the appropriate `StructuredError` builder (e.g., `Container` -> `infrastructure`, `AssertionFailed` -> `assertion`, `Config` -> `config`, etc.).

**[P0] [correctness]: MCP service.rs `map_err(anyhow_to_mcp)` applied to `ConaryTestError` results**
- File: `conary-test/src/server/mcp.rs:548` (and 4 other sites)
- Issue: `anyhow_to_mcp` accepts `anyhow::Error` but is passed to `.map_err()` on `Result<_, ConaryTestError>`. Five `E0631` type mismatch errors.
- Impact: Build failure; the MCP server cannot compile.
- Fix: Either change `anyhow_to_mcp` to accept `impl std::fmt::Display` or convert to `anyhow::Error` first, or add an `impl From<ConaryTestError> for McpError` conversion.

---

### P1: Incorrect Behavior / Significant Code Smell

**[P1] [architecture]: Container initialization logic duplicated in three places**
- File: `conary-test/src/engine/container_setup.rs:16-83`, `conary-test/src/server/service.rs:379-451`, and inline in `runner.rs` (pre-extraction)
- Issue: `container_setup::initialize_container_state()` was extracted specifically to deduplicate this logic, but `service.rs::initialize_container()` (lines 379-451) is a near-identical copy that was not updated to call the shared function. The only difference is that the service version always adds the distro repo while the shared one gates on `add_distro_repo`.
- Impact: Bug fixes applied to one path silently diverge from the other. A maintainer seeing the shared function in `container_setup.rs` would assume it's the only implementation.
- Fix: Replace `service.rs::initialize_container()` with a call to `container_setup::initialize_container_state(config, distro, true, backend, container_id).await`.

**[P1] [correctness]: `cleanup_containers` only removes containers with the `conary-test` label, but containers are never labeled**
- File: `conary-test/src/server/service.rs:700-741`
- Issue: The cleanup endpoint filters containers by `label=conary-test`, but the container creation in `execute_run()` (service.rs:234-243) and `ContainerCoordinator` never add this label to `ContainerConfig`. The bollard `ContainerCreateBody` at `lifecycle.rs:199` doesn't set any labels.
- Impact: The `/v1/cleanup` endpoint and `cleanup_containers` MCP tool will never find any containers to clean up, silently returning `{"removed": 0}`. Orphaned containers accumulate.
- Fix: Add `labels: Some(HashMap::from([("conary-test".to_string(), "true".to_string())]))` to the `ContainerCreateBody` in `BollardBackend::create()`, or add a `labels` field to `ContainerConfig`.

**[P1] [correctness]: `has_failed()` / `should_skip()` treats `Skipped` as unsuccessful, creating cascading skips**
- File: `conary-test/src/engine/suite.rs:92-108`
- Issue: `record()` adds both `Failed` and `Skipped` test IDs to `unsuccessful_ids`. If T01 fails, T02 (depends on T01) is skipped. If T03 depends on T02, it is *also* skipped because T02 is in `unsuccessful_ids`. This creates cascading skip chains where a test that merely depends on a skipped test is itself skipped, even though the skipped test may have no bearing on it.
- Impact: A single failed fatal test can cause the entire rest of a suite to be skipped, even for tests that don't depend on the failed one through transitivity. This matches the observed behavior but may not be the intended semantics -- the function is named `has_failed`, not `has_not_passed`.
- Fix: Either rename to `has_not_passed()` to clarify intent, or change `record()` to only add `Failed` (not `Skipped`) to `unsuccessful_ids` if cascading skips are undesired.

**[P1] [correctness]: `retry_delay_ms` is parsed but never used**
- File: `conary-test/src/config/manifest.rs:42` (defined), `conary-test/src/engine/runner.rs` (never referenced)
- Issue: `TestDef.retry_delay_ms` is parsed from TOML and tested for parsing, but the `majority_vote()` function in `runner.rs` never reads it. Retries happen immediately with no delay between attempts.
- Impact: Manifest authors who set `retry_delay_ms = 500` get no delay between retries, defeating the purpose of the field for flaky tests that need time to recover (e.g., port reuse, file lock release).
- Fix: In `majority_vote()`, after a failed attempt, insert `tokio::time::sleep(Duration::from_millis(test_def.retry_delay_ms.unwrap_or(0)))`.

**[P1] [code-quality]: Mock backend `exec_detached` always returns `"exec-1"` regardless of call count**
- File: `conary-test/src/container/mock.rs:229-235`
- Issue: Every call to `exec_detached` returns the hardcoded string `"exec-1"`. If a test exercises multiple detached exec calls (e.g., mock server start + kill_after_log), the second call's logs and result will overwrite or collide with the first's in the `log_sequences`/`detached_results` maps.
- Impact: Unit tests that exercise multiple detached exec paths will get incorrect results. The mock doesn't faithfully simulate real Podman behavior where each exec gets a unique ID.
- Fix: Use an `AtomicU64` counter to generate unique IDs: `format!("exec-{}", self.exec_counter.fetch_add(1, Ordering::Relaxed))`.

---

### P2: Improvement Opportunities / Minor Inconsistencies

**[P2] [architecture]: `run_id` is `u64` in state but `i64` in Remi client**
- File: `conary-test/src/server/state.rs:144` vs `conary-test/src/server/remi_client.rs:79`
- Issue: `AppState::next_run_id()` returns `u64`. `RemiClient::create_run()` returns `i64`. The conversion happens implicitly via `as i64` casts in handlers.rs (e.g., line 82: `id as i64`). If the run counter ever exceeds `i64::MAX`, the cast wraps silently.
- Impact: Low probability in practice (counter starts at 1 and only reaches ~10^18 on 64-bit), but the inconsistency is a code smell and the `as i64` casts are scattered rather than centralized.
- Fix: Either standardize on `i64` throughout (SQLite uses `i64` anyway) or add a safe conversion utility.

**[P2] [code-quality]: `StepOutput` event uses stdout line index as `step`, not actual step index**
- File: `conary-test/src/engine/runner.rs:316`
- Issue: `for (step_idx, line) in exec.stdout.lines().enumerate()` -- `step_idx` is the *line number within stdout*, not the step index from the manifest. The `StepOutput` event field is named `step`, suggesting it corresponds to the manifest step.
- Impact: SSE consumers expecting `step` to identify which manifest step produced the output will get misleading data. Each stdout line gets an incrementing "step" that bears no relation to the actual test step.
- Fix: Track the actual step index in the runner loop and pass it to the event, or rename the field to `line_index`.

**[P2] [code-quality]: `pending_count` casts `i64` to `u64` without bounds check**
- File: `conary-test/src/server/wal.rs:66`
- Issue: `row.get::<_, i64>(0).map(|v| v as u64)`. `COUNT(*)` returns a non-negative value in SQLite, but the `as u64` cast on a potentially negative `i64` would wrap. While SQLite's COUNT(*) is always >= 0, the cast is technically unsound.
- Impact: No practical impact, but it violates the principle of safe integer conversion.
- Fix: Use `u64::try_from(v).unwrap_or(0)`.

**[P2] [code-quality]: `purge_dead` returns `deleted as u64` with potential overflow**
- File: `conary-test/src/server/wal.rs:134`
- Issue: `rusqlite::Connection::execute` returns `usize`, and `usize as u64` is fine on 64-bit platforms but technically narrowing on 32-bit.
- Impact: None on the target platform (all servers are 64-bit), but inconsistent with the careful conversion patterns elsewhere.
- Fix: Use `u64::try_from(deleted).unwrap_or(0)`.

**[P2] [architecture]: `config.toml` does not set `containerfile` for any distro**
- File: `tests/integration/remi/config.toml:22-50`
- Issue: None of the three distro configurations (`fedora43`, `ubuntu-noble`, `arch`) include a `containerfile` field. The code falls back to `Containerfile.{distro}` in `service.rs:684`, which works, but it means the config file is incomplete documentation of what the system actually uses.
- Impact: Minor -- the default works. But if someone renames a Containerfile, the config gives no hint of where to update.

**[P2] [correctness]: Assertion `file_exists` and `file_not_exists` in `Assertion` struct are never evaluated**
- File: `conary-test/src/engine/assertions.rs:6-69`
- Issue: The `Assertion` struct has `file_exists` and `file_not_exists` fields (manifest.rs:237-239), and `evaluate_assertion()` handles `exit_code`, `stdout_*`, and `stderr_*` checks -- but it never checks `assertion.file_exists` or `assertion.file_not_exists` within an assertion block. These are only handled as *step types* (TestStep fields), not as assertion checks on the output of a previous step.
- Impact: If a manifest author writes `[test.step.assert]\nfile_exists = "/some/path"`, the assertion silently passes regardless of whether the file exists, because `evaluate_assertion` never checks those fields.
- Fix: Either add file_exists/file_not_exists checks to `evaluate_assertion()` (would need container access), or document that these are step-level-only checks and remove them from the `Assertion` struct to prevent confusion.

**[P2] [code-quality]: `copy_dir_filtered` does not handle nested symlinks pointing outside the source tree**
- File: `conary-test/src/container/image.rs:38-79`
- Issue: Symlinks are recreated verbatim via `std::os::unix::fs::symlink(&link_target, &target)`. If a symlink points to an absolute path (e.g., `/usr/lib/foo`), it will be recreated pointing to the same absolute path in the build context, which likely doesn't exist.
- Impact: Low in practice (the source tree is unlikely to contain absolute symlinks), but for a function called `copy_dir_filtered`, the behavior is surprising.

**[P2] [architecture]: `container_setup.rs` module is `pub` but is an implementation detail**
- File: `conary-test/src/engine/container_setup.rs`
- Issue: Once the module declaration is added, it will be `pub mod container_setup` per convention. The function `initialize_container_state` is only called from `runner.rs` and should be from `service.rs`. There's no reason for it to be part of the public API.
- Fix: Use `pub(crate) mod container_setup;` to limit visibility.

---

### P3: Style / Naming / Minor

**[P3] [style]: `suite.rs` `has_failed()` is misleading given that skipped tests are also tracked**
- File: `conary-test/src/engine/suite.rs:99`
- Issue: The method name suggests it checks only for failure, but `unsuccessful_ids` also includes skipped tests. Consider renaming to `is_unsuccessful()`.

**[P3] [style]: `RUN_COUNTER` global means run IDs collide with Remi's ID space**
- File: `conary-test/src/server/state.rs:25`
- Issue: `AtomicU64::new(1)` resets to 1 on every process restart. The in-memory run IDs and Remi's database IDs are independent sequences that will overlap.
- Impact: Minimal because lookups are scoped (local DashMap vs Remi API), but could confuse operators reading logs that reference both ID spaces.

**[P3] [style]: `error.rs` `Container` source field not wired into error chain**
- File: `conary-test/src/error.rs:8-11`
- Issue: The `source` field on the `Container` variant is `Option<Box<dyn Error + Send + Sync>>` but is not annotated with `#[source]`. The `thiserror`-derived `Error::source()` method returns `None` even when a source error is present, breaking error chain traversal for diagnostics.
- Fix: Add `#[source]` to the field, or restructure to use `#[from]` for automatic conversion.

---

### Cross-Domain Notes

**[Cross-Domain: Feature 8] `initialize_container` in service.rs duplicates logic from container_setup.rs**
- The deduplication intent was clear from commit `1e1b61e`, but the service.rs copy was not updated. This affects Feature 8 (Remi server) insofar as the test infrastructure server code diverges from the shared implementation.

---

### Strengths

1. **Well-designed `ContainerBackend` trait** (`container/backend.rs`): Clean async trait with comprehensive operations. The `NullBackend` for QEMU-only suites is elegant. The `MockBackend` with configurable failure injection (`FailOn` enum) enables thorough unit testing of error paths.

2. **Robust assertion validation** (`config/manifest.rs:254-312`): The `Assertion::validate()` method catches contradictory assertions at parse time (e.g., `exit_code=0` AND `exit_code_not=0`), preventing tests that can never pass. This is defense-in-depth that saves debugging time.

3. **WAL-backed result delivery** (`server/wal.rs`): SQLite-backed write-ahead log with proper FIFO ordering, retry counting, dead-letter purging, and in-memory test coverage. The `flush()` function handles corrupt payloads gracefully by removing them rather than retrying forever.

4. **Comprehensive test coverage**: Nearly every module has in-file `#[cfg(test)] mod tests` with meaningful assertions. The test count is high and the tests cover error paths, not just happy paths (e.g., `create_fails_container_not_tracked`, `stop_fails_remove_still_called`).

5. **Variable substitution system** (`engine/variables.rs`): Clean separation between variable building, manifest overrides, and expansion. The `expand_assertion()` function ensures variables in assertion fields are expanded, which is easy to forget.

6. **Error taxonomy** (`error_taxonomy.rs`): Machine-parseable error codes with categories, transient flags, and remediation hints. The builder pattern with `with_hint()` and `with_details()` is ergonomic. The `IntoResponse` impl maps categories to appropriate HTTP status codes.

7. **Container coordinator cleanup guarantees** (`engine/container_coordinator.rs`): `teardown_all()` continues cleanup even if individual stop/remove calls fail. The `with_cleanup()` method ensures teardown on both success and error paths.

---

### Recommendations

1. **Fix the build immediately**: Add `pub(crate) mod container_setup;` to `engine/mod.rs`, implement `From<ConaryTestError> for StructuredError`, and fix the `anyhow_to_mcp` type mismatches in `mcp.rs`. The crate cannot compile without these changes.

2. **Eliminate the `initialize_container` duplication in `service.rs`**: Replace the 70-line copy with a one-line call to `container_setup::initialize_container_state(config, distro, true, backend, container_id)`. This was the explicit goal of commit `1e1b61e` and simply wasn't completed.

3. **Add container labels for cleanup**: Without the `conary-test` label on created containers, the cleanup endpoint is a no-op. Add labels to `ContainerConfig` and the bollard `create` call. This is the difference between containers accumulating until disk exhaustion versus being cleanable.

---

### Assessment

**Ready to merge?** No

**Reasoning:** The crate has 10 compilation errors on main, including a missing module declaration, a missing type conversion impl, and function signature mismatches. These are all from a recent refactor (commit `1e1b61e`) that was not completed. Until these are fixed, no integration tests can be built or run. The container cleanup endpoint is also silently broken (no labels), which is a resource leak risk in production. Fix the P0 build issues, then address the P1 cleanup labeling and code duplication before merging.

---

### Work Breakdown

1. **[CRITICAL] Fix build -- module declarations**: Add `pub(crate) mod container_setup;` to `engine/mod.rs` and `#[cfg(test)] pub(crate) mod mock;` to `container/mod.rs`
2. **[CRITICAL] Fix build -- `From<ConaryTestError> for StructuredError`**: Implement the conversion in `error_taxonomy.rs` mapping each variant to the appropriate category
3. **[CRITICAL] Fix build -- MCP `map_err` type mismatches**: Fix 5 call sites in `mcp.rs` where `anyhow_to_mcp` is applied to `ConaryTestError` results
4. **[HIGH] Deduplicate `initialize_container`**: Replace `service.rs:379-451` with call to `container_setup::initialize_container_state`
5. **[HIGH] Add container labels for cleanup**: Add `conary-test` label to `ContainerConfig` / `ContainerCreateBody`
6. **[HIGH] Implement `retry_delay_ms`**: Add sleep between retry attempts in `majority_vote()`
7. **[MEDIUM] Fix mock backend exec ID uniqueness**: Use atomic counter instead of hardcoded `"exec-1"`
8. **[MEDIUM] Fix `StepOutput` event step index**: Track actual step index, not stdout line number
9. **[MEDIUM] Fix assertion `file_exists`/`file_not_exists` gap**: Either implement in `evaluate_assertion` or remove from `Assertion` struct
10. **[LOW] Fix `has_failed` naming / semantics**: Clarify whether skipped tests should cascade
