# Code Simplification Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix all systemic code quality and efficiency issues found during the simplify review — quick wins first, then deeper refactors.

**Architecture:** Three tiers: quick wins (helpers, indexes, batching), moderate refactors (function extraction, allocation reduction), and larger structural improvements. Each tier is independently committable.

**Tech Stack:** Rust 1.94, SQLite (rusqlite), resolvo.

---

## Tier 1: Quick Wins (1-2 hours each)

### Task 1: Create `open_db()` helper and unify db::open calls

**Files:**
- Modify: `src/commands/mod.rs` — add helper
- Modify: ~40 command files — replace bare `db::open()?`

- [ ] Add to `src/commands/mod.rs`:
```rust
pub fn open_db(path: &str) -> Result<rusqlite::Connection> {
    conary_core::db::open(path).context("Failed to open package database")
}
```
- [ ] Find all bare `conary_core::db::open(db_path)?` without `.context()` across `src/commands/`
- [ ] Replace with `open_db(db_path)?`
- [ ] Verify: `cargo build && cargo test`
- [ ] Commit: `refactor(cli): unify db::open calls with open_db() helper`

---

### Task 2: Extract "package not found" helper

**Files:**
- Modify: `src/commands/mod.rs` — add helper
- Modify: ~15 query/adopt command files

- [ ] Add to `src/commands/mod.rs`:
```rust
pub fn find_installed_trove(conn: &Connection, name: &str) -> Result<Trove> {
    let troves = Trove::find_by_name(conn, name)?;
    troves.into_iter().next().ok_or_else(|| {
        anyhow::anyhow!("Package '{}' is not installed", name)
    })
}
```
- [ ] Replace repeated `troves.first().ok_or_else(|| anyhow!("Package '{}' not found"))` patterns
- [ ] Verify: `cargo build && cargo test`
- [ ] Commit: `refactor(cli): extract find_installed_trove() helper`

---

### Task 3: Add missing SQL indexes

**Files:**
- Modify: `conary-core/src/db/schema.rs` — bump to v52
- Modify: `conary-core/src/db/migrations.rs` — add migration

- [ ] Add migration v52:
```sql
CREATE INDEX IF NOT EXISTS idx_provides_trove_cap ON provides(trove_id, capability);
CREATE INDEX IF NOT EXISTS idx_provides_kind_cap ON provides(kind, capability);
CREATE INDEX IF NOT EXISTS idx_deps_trove ON dependencies(trove_id);
CREATE INDEX IF NOT EXISTS idx_repo_req_pkg_kind ON repository_requirements(repository_package_id, kind);
```
- [ ] Bump SCHEMA_VERSION to 52
- [ ] Add v52 match arm in `apply_migration()`
- [ ] Verify: `cargo test -p conary-core db`
- [ ] Commit: `perf(db): add composite indexes for resolver and sync hot paths`

---

### Task 4: Batch load dependencies in load_removal_data()

**Files:**
- Modify: `conary-core/src/resolver/provider.rs`

- [ ] In `load_removal_data()`, replace per-solvable `DependencyEntry::find_by_trove()` calls with a single batch query:
```rust
// Load ALL dependencies for ALL installed troves in one query
let trove_ids: Vec<i64> = self.solvables.iter()
    .filter_map(|s| s.trove_id)
    .collect();
let all_deps = DependencyEntry::find_by_troves(self.conn, &trove_ids)?;
// Group into HashMap<i64, Vec<DependencyEntry>>
```
- [ ] Add `DependencyEntry::find_by_troves(conn, ids)` batch method if it doesn't exist
- [ ] Verify: `cargo test -p conary-core resolver`
- [ ] Commit: `perf(resolver): batch load dependencies in load_removal_data()`

---

### Task 5: Replace format!() SQL with const strings

**Files:**
- Modify: ~25 model files in `conary-core/src/db/models/`

- [ ] For each model file that has a `COLUMNS` constant used in `format!()`:
  - Replace `format!("SELECT {COLUMNS} FROM table WHERE id = ?1")` with a const string
  - Example: `const SELECT_BY_ID: &str = "SELECT id, name, ... FROM triggers WHERE id = ?1";`
- [ ] Start with the most-queried models: `trove.rs`, `file_entry.rs`, `dependency.rs`, `provide_entry.rs`
- [ ] Verify: `cargo test -p conary-core`
- [ ] Commit: `perf(db): use const SQL strings instead of format!() allocation`

---

## Tier 2: Moderate Refactors (2-3 hours each)

### Task 6: Reduce constraint clones in resolver

**Files:**
- Modify: `conary-core/src/resolver/provider.rs`

- [ ] In `intern_version_set()` and related functions, reduce `ConaryConstraint` cloning:
  - Use `Rc<ConaryConstraint>` or `Arc<ConaryConstraint>` for shared ownership
  - Or store constraints in an arena and use indices
- [ ] Profile before/after with a 200-package install to measure improvement
- [ ] Verify: `cargo test -p conary-core resolver`
- [ ] Commit: `perf(resolver): reduce constraint cloning in version set interning`

---

### Task 7: Reduce string allocations in SAT solver

**Files:**
- Modify: `conary-core/src/resolver/sat.rs`
- Modify: `conary-core/src/resolver/provider.rs`

- [ ] In the transitive dependency loading loop:
  - Track "already discovered" names and only process new ones
  - Use `&str` references where possible instead of cloning Strings
  - Pre-allocate collections based on estimated sizes
- [ ] In `intern_name()`: avoid double-clone for new names
- [ ] Verify: `cargo test -p conary-core resolver`
- [ ] Commit: `perf(resolver): reduce string allocations in SAT dependency loading`

---

### Task 8: Break up cmd_install (1,179 lines)

**Files:**
- Modify: `src/commands/install/mod.rs`
- Create: `src/commands/install/preconditions.rs` (optional)

- [ ] Extract sub-functions from `cmd_install`:
  - `resolve_package_source()` — canonical resolution + policy ranking
  - `check_install_preconditions()` — adoption check, upgrade validation
  - `execute_package_install()` — file deployment, DB transaction
- [ ] Each extracted function should be < 200 lines
- [ ] Keep `cmd_install` as orchestrator calling the sub-functions
- [ ] Verify: `cargo build && cargo test`
- [ ] Commit: `refactor(install): extract sub-functions from 1,179-line cmd_install`

---

### Task 9: Differentiate HTTP timeouts

**Files:**
- Modify: `conary-core/src/repository/client.rs`

- [ ] Set metadata fetch timeout to 10 seconds
- [ ] Set file/chunk download timeout to 300 seconds
- [ ] Make configurable via `RepositoryClient` builder or config
- [ ] Verify: `cargo build -p conary-core`
- [ ] Commit: `fix(repo): differentiate HTTP timeouts for metadata vs downloads`

---

## Tier 3: Documentation + Cleanup

### Task 10: Document output standards in CLI rules

**Files:**
- Modify: `.claude/rules/cli.md`

- [ ] Add output formatting standards:
  - `println!()` for user-facing results only
  - `tracing::info!()` / `warn!()` for diagnostics
  - `eprintln!()` only for usage errors
- [ ] Document function size guideline (< 300 lines)
- [ ] Document `open_db()` and `find_installed_trove()` helpers
- [ ] Commit: `docs: add CLI output and function size standards`

---

### Task 11: Audit and document dead code

**Files:**
- Modify: Various files with `#[allow(dead_code)]`

- [ ] For each `#[allow(dead_code)]`:
  - If truly unused and not planned: remove
  - If planned for future phase: add comment with phase reference
  - If needed for public API: keep with explanation
- [ ] Verify: `cargo build && cargo clippy -- -D warnings`
- [ ] Commit: `chore: audit dead code markers, remove unused, document planned`

---

## Implementation Order

**Do first (highest impact, lowest effort):**
1. Task 1: open_db() helper (15 min)
2. Task 3: SQL indexes (30 min)
3. Task 2: find_installed_trove() helper (30 min)
4. Task 4: Batch load deps (1 hour)
5. Task 5: Const SQL strings (1-2 hours)

**Do next (moderate effort, good payoff):**
6. Task 9: HTTP timeouts (30 min)
7. Task 10: Document standards (30 min)
8. Task 11: Dead code audit (1 hour)

**Do when time allows (larger refactors):**
9. Task 6: Constraint clones (2-3 hours)
10. Task 7: String allocations (2-3 hours)
11. Task 8: Break up cmd_install (2-3 hours)

## Success Criteria

- `cargo test` passes with zero failures
- `cargo clippy -- -D warnings` clean
- No bare `db::open()?` in command handlers
- Schema at v52 with 4 new indexes
- `load_removal_data()` uses batch query
- All `#[allow(dead_code)]` documented or removed
