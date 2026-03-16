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
- Modify: 41 command files — replace 146 bare `db::open()?` calls

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

### Task 2: Promote `Trove::find_one_by_name()` usage

> **Review correction:** The plan originally proposed a new `find_installed_trove()` wrapper, but `Trove::find_one_by_name()` already exists in conary-core and is only used in 3 places. 9 files (not ~15) have "package not found" patterns with 3 different approaches (`.ok_or_else()`, `bail!()`, `.is_empty()` checks). Some files like `provenance.rs` have custom helpers with different return types — leave those as-is.

**Files:**
- Modify: 5-6 command files where `Trove::find_one_by_name()` can replace manual lookup + error

- [ ] Identify files doing manual `Trove::find_by_name()` + `.first()` / `.is_empty()` + error construction
- [ ] Replace with `Trove::find_one_by_name()` where the pattern matches (simple name -> single trove lookup)
- [ ] Leave alone: files with custom return types (e.g., `provenance.rs`'s `find_trove()` returning `Option<(i64, String, String)>`)
- [ ] Verify: `cargo build && cargo test`
- [ ] Commit: `refactor(cli): use Trove::find_one_by_name() instead of manual lookup patterns`

---

### Task 3: Add missing SQL indexes

> **Review correction:** 2 of the 4 originally proposed indexes are redundant. `idx_deps_trove` duplicates v1's `idx_dependencies_trove_id`. `idx_provides_kind_cap` duplicates v23's `idx_provides_kind_capability`. Only 2 new indexes needed.

**Files:**
- Modify: `conary-core/src/db/schema.rs` — bump to v52
- Modify: `conary-core/src/db/migrations.rs` — add migration

- [ ] Add migration v52:
```sql
CREATE INDEX IF NOT EXISTS idx_provides_trove_cap ON provides(trove_id, capability);
CREATE INDEX IF NOT EXISTS idx_repo_req_pkg_kind ON repository_requirements(repository_package_id, kind);
```
- [ ] Bump SCHEMA_VERSION to 52
- [ ] Add v52 match arm in `apply_migration()`
- [ ] Verify: `cargo test -p conary-core db`
- [ ] Commit: `perf(db): add composite indexes for resolver and sync hot paths`

---

### Task 4: Batch load dependencies in resolver

> **Review correction:** The N+1 pattern exists in 3 locations, not just `load_removal_data()`: also `load_installed_packages()` (provider.rs:304) and `resolver/graph.rs:202`. Address all three.

**Files:**
- Modify: `conary-core/src/db/models/dependency.rs` — add batch method
- Modify: `conary-core/src/resolver/provider.rs` — use batch in `load_removal_data()` and `load_installed_packages()`
- Modify: `conary-core/src/resolver/graph.rs` — use batch in dependency graph construction

- [ ] Add `DependencyEntry::find_by_troves(conn, &[i64])` batch method using `WHERE trove_id IN (...)`:
```rust
pub fn find_by_troves(conn: &Connection, trove_ids: &[i64]) -> Result<HashMap<i64, Vec<Self>>> {
    // Build parameterized IN clause, execute single query, group by trove_id
}
```
- [ ] Replace per-solvable `DependencyEntry::find_by_trove()` loop in `load_removal_data()`
- [ ] Replace same pattern in `load_installed_packages()` (provider.rs:304)
- [ ] Replace same pattern in `resolver/graph.rs:202`
- [ ] Verify: `cargo test -p conary-core resolver`
- [ ] Commit: `perf(resolver): batch load dependencies to eliminate N+1 queries`

---

### Task 5: Replace format!() SQL with const strings

**Files:**
- Modify: 16 model files in `conary-core/src/db/models/` (41 instances total)

**Priority order by query frequency:**
1. `dependency.rs` (1) — resolver hot path
2. `trove.rs` (2) — most-queried model
3. `file_entry.rs` (5) — file deployment
4. `state.rs` (4) — generation management
5. `changeset.rs` (3) — transaction tracking
6. `label.rs` (5), `trigger.rs` (4), `derived.rs` (3), `config.rs` (3), `chunk_access.rs` (3)
7. Remaining: `component_dependency.rs` (2), `repository.rs` (2), `redirect.rs` (1), `federation_peer.rs` (1), `converted.rs` (1), `component.rs` (1)

- [ ] For each model file, replace `format!("SELECT {COLUMNS} FROM table WHERE ...")` with inline string literals in `conn.prepare()`:
```rust
// Before:
let sql = format!("SELECT {DEP_COLUMNS} FROM dependencies WHERE trove_id = ?1");
let mut stmt = conn.prepare(&sql)?;

// After:
let mut stmt = conn.prepare(
    "SELECT id, trove_id, depends_on_name, depends_on_version, \
     dependency_type, version_constraint, kind FROM dependencies WHERE trove_id = ?1"
)?;
```
- [ ] Remove unused `COLUMNS` constants after all their usages are inlined
- [ ] Verify: `cargo test -p conary-core`
- [ ] Commit: `perf(db): use const SQL strings instead of format!() allocation`

---

## Tier 2: Moderate Refactors (2-3 hours each)

### Task 6: Reduce constraint clones in resolver

> **Review correction:** Rc/Arc adds pointer overhead and reference counting — overkill for this case. ConaryConstraint is 40-80+ bytes and cloned twice per `intern_version_set()` call plus once per `intern_repo_version_set()`. The right approach is a constraint ID pool (like the existing name interning pattern).

**Files:**
- Modify: `conary-core/src/resolver/provider.rs`

- [ ] Add a constraint arena/pool similar to name interning:
```rust
// Store constraints in a Vec, reference by index
constraints: Vec<ConaryConstraint>,
constraint_to_id: HashMap<ConaryConstraint, ConstraintId>,
```
- [ ] In `intern_version_set()`, intern the constraint and store ConstraintId instead of cloning
- [ ] In `intern_repo_version_set()`, same pattern
- [ ] Update `version_sets` to store `(NameId, ConstraintId)` instead of `(NameId, ConaryConstraint)`
- [ ] Fix `intern_all_dependency_version_sets()` line 610+ which clones all deps
- [ ] Verify: `cargo test -p conary-core resolver`
- [ ] Commit: `perf(resolver): intern constraints to eliminate cloning in version set interning`

---

### Task 7: Reduce string allocations in SAT solver

**Files:**
- Modify: `conary-core/src/resolver/sat.rs`
- Modify: `conary-core/src/resolver/provider.rs`

**Hotspots (sat.rs lines 68-92, transitive dependency loading loop):**
- Clone 1: `requests.iter().map(|(n, _)| n.clone())` into HashSet
- Clone 2: `loaded_names.iter().cloned()` into Vec
- Clone 3: `loaded_names.insert(n.clone())` in filter
- Clone 4: `canonical_equivalents(n).iter().cloned()`
- Clone 5: `loaded_names.insert(n.clone())` again in equiv filter

**provider.rs `intern_name()` double-clone (lines 165-173):**
- Clone 1: `self.names.push(name.to_string())`
- Clone 2: `self.name_to_id.insert(name.to_string(), id)`

- [ ] In transitive dep loop: use `HashSet<&str>` borrowing from provider's interned names where possible
- [ ] Pre-allocate `to_load` and `new_names` based on estimated dependency counts
- [ ] In `intern_name()`: allocate once, clone once (or use entry API)
- [ ] Verify: `cargo test -p conary-core resolver`
- [ ] Commit: `perf(resolver): reduce string allocations in SAT dependency loading`

---

### Task 8: Break up cmd_install (1,179 lines)

**Files:**
- Modify: `src/commands/install/mod.rs` (lines 228-1407)
- Directory already has helper modules: `batch.rs`, `conversion.rs`, `dependencies.rs`, `dep_resolution.rs`, `execute.rs`, `prepare.rs`, `resolve.rs`, `scriptlets.rs`, `system_pm.rs`, `blocklist.rs`, `dep_mode.rs`

**Logical sections to extract:**
1. Lines 248-297: Option destructuring, DB open, policy construction
2. Lines 298-407: Canonical resolution, component spec parsing, blocklist/adoption checks
3. Lines 409-551: Already-installed promotion, progress tracker, package resolution, format detection
4. Lines 553-903: Dependency analysis (build edges, filter, resolve)
5. Lines 904-1100: File extraction, component selection, capabilities inference, scriptlet setup
6. Lines 1100-1407: Transaction execution + rollback

- [ ] Extract sub-functions from `cmd_install`:
  - `resolve_package_source()` — canonical resolution + policy ranking (~sections 2-3)
  - `analyze_dependencies()` — dependency graph construction + resolution (~section 4)
  - `prepare_installation()` — file extraction, components, capabilities, scriptlets (~section 5)
  - `execute_transaction()` — DB transaction, file deployment, rollback (~section 6)
- [ ] Each extracted function should be < 300 lines
- [ ] Keep `cmd_install` as orchestrator calling the sub-functions
- [ ] Verify: `cargo build && cargo test`
- [ ] Commit: `refactor(install): extract sub-functions from 1,179-line cmd_install`

---

### Task 9: Make RepositoryClient timeouts configurable

> **Review correction:** Timeouts are already differentiated across modules (30s general, 60s chunks, 300s polling). The real issue is that `RepositoryClient::new()` hardcodes `HTTP_TIMEOUT = 30s` with no override mechanism.

**Files:**
- Modify: `conary-core/src/repository/client.rs`

- [ ] Add `TimeoutConfig` struct:
```rust
pub struct TimeoutConfig {
    pub metadata: Duration,   // Default 10s — repo index, package metadata
    pub download: Duration,   // Default 300s — file/package downloads
    pub default: Duration,    // Default 30s — everything else
}
```
- [ ] Add builder method to `RepositoryClient`:
```rust
pub fn with_timeouts(mut self, config: TimeoutConfig) -> Self
```
- [ ] Use per-request timeout override instead of client-level timeout where needed
- [ ] Verify: `cargo build -p conary-core`
- [ ] Commit: `feat(repo): make RepositoryClient timeouts configurable`

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
- [ ] Document `open_db()` helper and `Trove::find_one_by_name()` convention
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
1. Task 1: open_db() helper (15 min, 41 files / 146 calls)
2. Task 3: SQL indexes (15 min, 2 new indexes)
3. Task 2: Promote find_one_by_name() (30 min, 5-6 files)
4. Task 4: Batch load deps (1 hour, 3 N+1 sites)
5. Task 5: Const SQL strings (1-2 hours, 41 instances / 16 files)

**Do next (moderate effort, good payoff):**
6. Task 9: Configurable timeouts (30 min)
7. Task 10: Document standards (30 min)
8. Task 11: Dead code audit (1 hour)

**Do when time allows (larger refactors):**
9. Task 6: Constraint interning (2-3 hours)
10. Task 7: String allocations (2-3 hours)
11. Task 8: Break up cmd_install (2-3 hours)

## Success Criteria

- `cargo test` passes with zero failures
- `cargo clippy -- -D warnings` clean
- No bare `db::open()?` in command handlers
- Schema at v52 with 2 new indexes
- `load_removal_data()` and siblings use batch query
- All `#[allow(dead_code)]` documented or removed
