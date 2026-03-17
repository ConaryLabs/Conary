# Code Simplification Implementation Plan [COMPLETE]

> All 11 tasks across 3 tiers implemented. 10 commits, all tests passing, clippy clean.

**Goal:** Fix all systemic code quality and efficiency issues found during the simplify review — quick wins first, then deeper refactors.

**Architecture:** Three tiers: quick wins (helpers, indexes, batching), moderate refactors (function extraction, allocation reduction), and larger structural improvements. Each tier is independently committable.

**Tech Stack:** Rust 1.94, SQLite (rusqlite), resolvo.

---

## Tier 1: Quick Wins [COMPLETE]

### Task 1: Create `open_db()` helper and unify db::open calls [COMPLETE]

Commit: `fdfa217` — 146 calls unified across 48 files.

- [x] Added `open_db()` to `src/commands/mod.rs`
- [x] Replaced all bare `conary_core::db::open(db_path)?` without `.context()`
- [x] Exceptions kept: test code (`.unwrap()`), one closure returning `conary_core::Result`

---

### Task 2: Promote `Trove::find_one_by_name()` usage [COMPLETE]

Commit: `a678cff` — 6 patterns replaced in 4 files.

- [x] Replaced manual `find_by_name()` + `.first()` / `.is_empty()` in: `update.rs`, `query/dependency.rs`, `query/deptree.rs`, `restore.rs`
- [x] Left alone: files iterating all troves, custom helpers, control-flow branches

---

### Task 3: Add missing SQL indexes [COMPLETE]

Commit: `e9268fc` — Schema v52, 2 new composite indexes.

- [x] `idx_provides_trove_cap` on `provides(trove_id, capability)`
- [x] `idx_repo_req_pkg_kind` on `repository_requirements(repository_package_id, kind)`
- [x] Skipped 2 redundant indexes (already existed from v1 and v23)

---

### Task 4: Batch load dependencies in resolver [COMPLETE]

Commit: `3fd1359` — N+1 eliminated in 3 call sites.

- [x] Added `DependencyEntry::find_by_troves()` with chunking at 500 IDs
- [x] Updated `load_installed_packages()`, `load_removal_data()`, `build_from_db()`

---

### Task 5: Replace format!() SQL with const strings [COMPLETE]

Commit: `df5aab4` — 41 `format!()` removed, 24 constants eliminated across 16 files.

- [x] All model files inlined, unused COLUMNS constants removed

---

## Tier 2: Moderate Refactors [COMPLETE]

### Task 6+7: Reduce cloning in resolver [COMPLETE]

Commit: `6ff8079` — Combined constraint and string allocation improvements.

- [x] `intern_version_set()`: removed extra clone (move instead of clone)
- [x] `intern_name()`: single `to_string()` + clone instead of two `to_string()`
- [x] `intern_all_dependency_version_sets()`: `mem::take` instead of cloning all SolverDep entries
- [x] Added `new_dependency_names(known)` to skip already-loaded names in transitive loop
- Note: Full constraint ID pool (plan's original approach) deferred — diminishing returns vs. complexity

---

### Task 8: Break up cmd_install (1,179 lines) [COMPLETE]

Commit: `b1b982d` — 1,179 lines reduced to 238-line orchestrator + 12 helper functions.

Extracted functions:
- `build_resolution_policy()`, `resolve_canonical_name()`, `parse_component_and_validate()`
- `try_promote_existing_dep()`, `resolve_and_parse_package()`
- `handle_dependencies()`, `handle_dep_adoptions()`, `handle_dep_installs()`
- `check_unresolvable_deps()`, `show_dry_run_summary()`
- `extract_and_classify_files()`, `run_pre_install_phase()`
- `execute_install_transaction()`, `finalize_install()`

---

### Task 9: Make RepositoryClient timeouts configurable [COMPLETE]

Commit: `beb4ea8` — `TimeoutConfig` with per-request timeouts.

- [x] `TimeoutConfig` struct: metadata (30s), download (300s), connect (30s)
- [x] `RepositoryClient::with_timeouts()` constructor
- [x] Per-request timeouts via `RequestBuilder::timeout()`

---

## Tier 3: Documentation + Cleanup [COMPLETE]

### Task 10: Document output standards in CLI rules [COMPLETE]

Commit: `7341567`

- [x] Output formatting standards (println, tracing, eprintln)
- [x] Function size guideline (< 300 lines)
- [x] `open_db()` and `find_one_by_name()` conventions documented

---

### Task 11: Audit and document dead code [COMPLETE]

Commit: `f9eb721` — 27 markers audited across 12 files.

- [x] 1 truly dead struct removed (`BenchmarkResult`)
- [x] 5 struct-level annotations narrowed to field-level
- [x] 13 kept annotations given explanatory comments
- [x] 9 already had adequate documentation

---

## Success Criteria [ALL MET]

- [x] `cargo test` passes with zero failures
- [x] `cargo clippy -- -D warnings` clean
- [x] No bare `db::open()?` in command handlers
- [x] Schema at v52 with 2 new indexes
- [x] `load_removal_data()` and siblings use batch query
- [x] All `#[allow(dead_code)]` documented or removed
