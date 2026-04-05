# Codebase Simplification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Simplify ~226k lines of Rust across 7 crates — remove dead code, decompose large files, clarify complex logic, deduplicate within modules.

**Architecture:** 12 independent chunks executed in 4 waves of 3 parallel worktree agents, each wave merged before the next starts. A final Phase 2 pass on merged `main` removes cross-visible dead APIs. See `docs/superpowers/specs/2026-04-03-codebase-simplification-design.md` for the full design spec.

**Tech Stack:** Rust, cargo clippy/test/fmt, git worktrees

---

## Wave Overview

| Wave | Chunks | Total ~Lines | Purpose |
|------|--------|-------------|---------|
| 1 (pilot) | 5, 8, 11 | 23k | Validate approach on smallest chunks |
| 2 | 3, 4, 12 | 50k | Medium chunks, cross-crate safe |
| 3 | 1, 2, 10 | 77k | Core data + remi server |
| 4 | 6, 7, 9 | 84k | Heaviest chunks, chunk 7 handles deferred lib.rs notes |
| Phase 2 | cross-chunk | all | Dead pub APIs, unused re-exports, trait bloat |

**Gate rule:** Do not start wave N+1 until all wave N branches are merged to `main` and verification passes on the merged result.

## Safety Rules (applies to ALL chunk tasks)

Every agent must follow these rules. Violations should be caught in review.

1. **Never remove `pub` or `pub(crate)` items.** Only remove private/module-internal dead code. Cross-visible cleanup is deferred to Phase 2.
2. **Respect non-textual liveness.** Preserve: serde-derived fields, macro-consumed items (rmcp, axum, clap), items with explanatory `#[allow(dead_code)]` comments, re-exports in lib.rs/mod.rs.
3. **Do not edit files outside the chunk.** If a file isn't listed in the chunk's scope, don't touch it.
4. **Preserve all behavior.** No semantic changes, no new dependencies, no API changes.
5. **Preserve file header path comments** (per AGENTS.md convention).
6. **Update tests only if internal signatures change.** Don't add/remove test coverage — just keep existing tests compiling.

## Stop Conditions (applies to ALL chunk tasks)

An agent should **stop and defer** to Phase 2 or ask a human when:

- An item appears dead but is `pub` or `pub(crate)` — defer to Phase 2
- An item has `#[derive(Serialize, Deserialize)]` and no textual callers — leave it
- A struct field is consumed by a framework macro — leave it
- A file decomposition would require changing `pub mod` in `lib.rs` — defer with `DEFERRED-PH2:` tag (removing pub mod lines is pub item removal)
- Removing code causes a test to fail and the fix isn't obvious — revert the removal and move on
- A function is complex enough that simplifying it risks changing behavior — leave it

When deferring something to Phase 2, include a line in the commit message:
`DEFERRED-PH2: <description>` (e.g., `DEFERRED-PH2: module trigger appears fully dead, evaluate pub mod removal in lib.rs`). Phase 2 will grep for this tag.

---

## Task 0: Baseline Verification

**Purpose:** Confirm `main` is green before any simplification work starts.

- [ ] **Step 1: Run the full verification suite on main**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --exclude conary-test --verbose
cargo test -p conary-test --verbose
cargo test --doc --workspace --verbose
```

- [ ] **Step 2: Verify feature-gated builds and tests**

```bash
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo clippy -p conary --all-targets --features experimental -- -D warnings
cargo clippy -p conaryd --all-targets --features polkit -- -D warnings
cargo test -p conary --features experimental --verbose
```

- [ ] **Step 3: Record baseline**

Note the commit hash. All chunk branches will be created from this point.

```bash
git rev-parse HEAD
```

Expected: all commands pass. If any fail, fix before proceeding.

---

## Wave 1: Pilot (Chunks 5, 8, 11)

Three smallest chunks. Run in parallel worktrees. Review results before scaling.

### Task 1: Chunk 5 — core-resolver (9k lines)

**Scope:** `crates/conary-core/src/resolver/`, `crates/conary-core/src/dependencies/`, `crates/conary-core/src/flavor/`, `crates/conary-core/src/version/`
**Branch:** `simplify/core-resolver`
**Dead code markers:** 0
**Feature gates:** none

**Known decomposition targets:**
- `resolver/provider/mod.rs` — 1728 lines (priority split target)
- `resolver/sat.rs` — 1518 lines (priority split target)
- `version/mod.rs` — 740 lines
- `flavor/mod.rs` — 721 lines
- `resolver/canonical.rs` — 664 lines
- `dependencies/detection.rs` — 589 lines
- `dependencies/classes.rs` — 563 lines
- `resolver/component_resolver.rs` — 540 lines

- [ ] **Step 1: Create worktree and branch**

```bash
git worktree add .claude/worktrees/simplify-core-resolver -b simplify/core-resolver
```

- [ ] **Step 2: Survey the chunk**

Read every file in scope. For each file over 500 lines, note:
- Functions over 80 lines (decomposition candidates)
- Repeated logic patterns (dedup candidates)
- Deep nesting / complex conditionals (clarity candidates)
- Private functions/types with no callers within the chunk (dead code candidates — but verify they aren't called from outside the chunk via `grep -rn 'function_name' /home/peter/Conary/` from the repo root)

- [ ] **Step 3: Remove private dead code**

For each dead private item found in the survey:
1. Confirm it has zero callers in the entire codebase: `grep -rn 'item_name' /home/peter/Conary/crates/ /home/peter/Conary/apps/`
2. Remove it
3. Remove any now-unused imports

- [ ] **Step 4: Quick verification**

```bash
cd /home/peter/Conary/.claude/worktrees/simplify-core-resolver
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 5: Decompose large files**

For each file over 1000 lines (provider/mod.rs, sat.rs):
1. Identify logical groupings of functions that can be extracted into submodules
2. Create the submodule file with a path comment header
3. Move the functions, add `pub(crate)` or `pub(super)` as needed
4. Add `mod submodule_name;` to the parent module
5. Update imports in the remaining file

For files 500-1000 lines, evaluate but only split if there's a natural boundary.

- [ ] **Step 6: Verify after decomposition**

```bash
cd /home/peter/Conary/.claude/worktrees/simplify-core-resolver
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --exclude conary-test --verbose
```

- [ ] **Step 7: Clarify code**

Walk through each file and apply:
- Early returns to reduce nesting depth
- Guard clauses to flatten conditionals
- Idiomatic Rust patterns (e.g., `if let` chains to `let ... else`, iterator chains where clearer than loops)
- Deduplicate repeated logic within the chunk (extract private helpers)
- Improve unclear names (local only — don't rename pub items)

- [ ] **Step 8: Full verification**

```bash
cd /home/peter/Conary/.claude/worktrees/simplify-core-resolver
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --exclude conary-test --verbose
cargo test -p conary-test --verbose
cargo test --doc --workspace --verbose
```

- [ ] **Step 9: Commit**

Commit with a message describing what was simplified. Use one commit per logical group (dead code removal, decomposition, clarity) or one combined commit if the changes are cohesive.

```bash
git add -A
git commit -m "simplify: core-resolver — [describe what changed]

- Dead code removed: [count] items
- Files decomposed: [list]
- Clarity improvements: [summary]

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

### Task 2: Chunk 8 — cli-dispatch (6k lines)

**Scope:** `apps/conary/src/cli/`, `apps/conary/src/dispatch.rs`, `apps/conary/src/app.rs`, `apps/conary/src/main.rs`, `apps/conary/src/live_host_safety.rs`
**Branch:** `simplify/cli-dispatch`
**Dead code markers:** 0
**Feature gates:** `experimental` — must verify with `--features experimental`

**Known decomposition targets:**
- `dispatch.rs` — 1831 lines, **1 function** (the single worst decomposition target in the codebase; this is essentially a giant match/dispatch tree)
- `cli/mod.rs` — 678 lines

**Critical note on dispatch.rs:** This file is one enormous function. Decomposing it means extracting match arms or logical groups of arms into separate functions, not splitting the file. The function likely dispatches CLI subcommands — group related arms into handler functions (e.g., `dispatch_install_commands()`, `dispatch_system_commands()`).

- [ ] **Step 1: Create worktree and branch**

```bash
git worktree add .claude/worktrees/simplify-cli-dispatch -b simplify/cli-dispatch
```

- [ ] **Step 2: Survey the chunk**

Read all files. Pay special attention to `dispatch.rs` — understand the match structure, identify logical groupings of arms, note any dead arms or unreachable branches.

- [ ] **Step 3: Remove private dead code**

Same process as Task 1 Step 3. Verify each removal with `grep -rn` from repo root.

- [ ] **Step 4: Quick verification**

```bash
cd /home/peter/Conary/.claude/worktrees/simplify-cli-dispatch
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p conary --all-targets --features experimental -- -D warnings
```

- [ ] **Step 5: Decompose dispatch.rs**

1. Read the entire function to understand the dispatch structure
2. Identify groups of related match arms (install commands, system commands, repo commands, etc.)
3. Extract each group into a named function in the same file
4. The main dispatch function becomes a thin router calling the group functions
5. If the file is still over 1000 lines after extraction, consider splitting group functions into a `dispatch/` module directory

- [ ] **Step 6: Decompose cli/mod.rs if warranted**

Only split if there's a natural boundary (e.g., argument parsing vs validation vs help text).

- [ ] **Step 7: Verify after decomposition**

```bash
cd /home/peter/Conary/.claude/worktrees/simplify-cli-dispatch
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p conary --all-targets --features experimental -- -D warnings
cargo test --workspace --exclude conary-test --verbose
cargo test -p conary --features experimental --verbose
```

- [ ] **Step 8: Clarify code**

Same clarity pass as Task 1 Step 7. Focus especially on the newly extracted dispatch functions — each should read top-to-bottom without deep nesting.

- [ ] **Step 9: Full verification**

```bash
cd /home/peter/Conary/.claude/worktrees/simplify-cli-dispatch
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p conary --all-targets --features experimental -- -D warnings
cargo test --workspace --exclude conary-test --verbose
cargo test -p conary-test --verbose
cargo test -p conary --features experimental --verbose
cargo test --doc --workspace --verbose
```

- [ ] **Step 10: Commit**

Same format as Task 1 Step 9, branch `simplify/cli-dispatch`.

### Task 3: Chunk 11 — conaryd (8k lines)

**Scope:** `apps/conaryd/src/daemon/`, `apps/conaryd/src/lib.rs`, `apps/conaryd/src/bin/conaryd.rs`
**Branch:** `simplify/conaryd`
**Dead code markers:** 2 (`daemon/lock.rs`, `daemon/client.rs`)
**Feature gates:** `polkit` — must verify with `--features polkit`

**Known decomposition targets:**
- `daemon/routes.rs` — 2160 lines (2nd largest file in codebase)
- `daemon/mod.rs` — 1009 lines
- `daemon/auth.rs` — 774 lines
- `daemon/routes/transactions.rs` — 642 lines
- `daemon/jobs.rs` — 640 lines
- `daemon/client.rs` — 586 lines

- [ ] **Step 1: Create worktree and branch**

```bash
git worktree add .claude/worktrees/simplify-conaryd -b simplify/conaryd
```

- [ ] **Step 2: Survey the chunk**

Read all files. Note the 2 dead-code markers in `lock.rs` and `client.rs`. Check if those marked items are truly dead (grep from repo root — other crates may import conaryd types).

- [ ] **Step 3: Remove private dead code**

Remove the 2 `#[allow(dead_code)]` items if confirmed dead. Check for additional unmarked dead code. Verify each removal with `grep -rn` from repo root.

- [ ] **Step 4: Quick verification**

```bash
cd /home/peter/Conary/.claude/worktrees/simplify-conaryd
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p conaryd --all-targets --features polkit -- -D warnings
```

- [ ] **Step 5: Decompose large files**

Priority target: `daemon/routes.rs` (2160 lines). This likely contains route handlers that can be split by resource (e.g., `routes/jobs.rs`, `routes/status.rs`, `routes/system.rs`). Check if a `routes/` subdirectory already exists (it does — `routes/transactions.rs`), and extend the pattern.

Also decompose `daemon/mod.rs` (1009 lines) if there are logical splits.

- [ ] **Step 6: Verify after decomposition**

```bash
cd /home/peter/Conary/.claude/worktrees/simplify-conaryd
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p conaryd --all-targets --features polkit -- -D warnings
cargo test --workspace --exclude conary-test --verbose
```

- [ ] **Step 7: Clarify code**

Same clarity pass. Focus on the route handlers — they often accumulate nested error handling that can be flattened with early returns.

- [ ] **Step 8: Full verification**

```bash
cd /home/peter/Conary/.claude/worktrees/simplify-conaryd
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p conaryd --all-targets --features polkit -- -D warnings
cargo test --workspace --exclude conary-test --verbose
cargo test -p conary-test --verbose
cargo test --doc --workspace --verbose
```

- [ ] **Step 9: Commit**

Same format as Task 1 Step 9, branch `simplify/conaryd`.

### Task 4: Wave 1 Review and Merge

**Purpose:** Review pilot results, assess quality, merge to main.

- [ ] **Step 1: Review each branch**

For each of the 3 branches (`simplify/core-resolver`, `simplify/cli-dispatch`, `simplify/conaryd`):
1. Read the diff: `git diff main...simplify/<branch>`
2. Check that no `pub` or `pub(crate)` items were removed
3. Check that no files outside the chunk's scope were modified
4. Check that commit messages describe what changed

- [ ] **Step 2: Merge first branch**

Pick the smallest/simplest branch to merge first. Rebase from inside the
worktree (git refuses to checkout a branch that's already checked out in
another worktree).

```bash
cd /home/peter/Conary/.claude/worktrees/simplify-<chunk>
git rebase main
# Re-run full verification suite from the worktree (all commands from
# Task 0 Step 1 + Step 2, including feature-gated checks)
cd /home/peter/Conary
git status              # confirm main is checked out and working tree is clean
git merge simplify/<branch> --no-ff
```

- [ ] **Step 3: Merge second branch**

```bash
cd /home/peter/Conary/.claude/worktrees/simplify-<chunk>
git rebase main  # now includes the first merge
# Resolve any conflicts
# Re-run full verification suite from the worktree
cd /home/peter/Conary
git status              # confirm main, clean
git merge simplify/<branch> --no-ff
```

- [ ] **Step 4: Merge third branch**

Same process — rebase from worktree, verify, confirm `git status` shows main
and clean, merge.

- [ ] **Step 5: Verify merged main**

Run the complete verification matrix on main after all three merges:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo clippy -p conary --all-targets --features experimental -- -D warnings
cargo clippy -p conaryd --all-targets --features polkit -- -D warnings
cargo test --workspace --exclude conary-test --verbose
cargo test -p conary-test --verbose
cargo test -p conary --features experimental --verbose
cargo test --doc --workspace --verbose
```

- [ ] **Step 6: Clean up worktrees**

```bash
git worktree remove .claude/worktrees/simplify-core-resolver
git worktree remove .claude/worktrees/simplify-cli-dispatch
git worktree remove .claude/worktrees/simplify-conaryd
git branch -d simplify/core-resolver simplify/cli-dispatch simplify/conaryd
```

- [ ] **Step 7: Pilot retrospective**

Before starting Wave 2, assess:
- Did the agents stay within scope?
- Were the safety rules sufficient?
- How much dead code was actually found?
- Were the decomposition splits clean?
- Any unexpected issues to feed into Wave 2 instructions?

---

## Wave 2: Medium Chunks (Chunks 3, 4, 12)

### Task 5: Chunk 3 — core-repository (19k lines)

**Scope:** `crates/conary-core/src/repository/`, `crates/conary-core/src/canonical/`, `crates/conary-core/tests/canonical.rs`
**Branch:** `simplify/core-repository`
**Dead code markers:** 6 (`repository/sync.rs`(3), `canonical/client.rs`(2), `repository/error_helpers.rs`(1))
**Feature gates:** none

**Known decomposition targets (17 files over 500 lines):**
- `repository/sync.rs` — 1825 lines (priority)
- `repository/resolution.rs` — 1133 lines
- `repository/remi.rs` — 1127 lines
- `repository/dependencies.rs` — 1096 lines
- `repository/parsers/fedora.rs` — 1092 lines
- `repository/chunk_fetcher.rs` — 795 lines
- `repository/selector.rs` — 767 lines
- `repository/versioning.rs` — 739 lines
- `repository/metalink.rs` — 704 lines
- `repository/client.rs` — 700 lines
- `canonical/repology.rs` — 668 lines
- `repository/parsers/arch.rs` — 664 lines
- `canonical/appstream.rs` — 659 lines
- `repository/parsers/debian.rs` — 639 lines
- `repository/mirror_health.rs` — 555 lines
- `repository/substituter.rs` — 525 lines
- `repository/download.rs` — 515 lines

- [ ] **Step 1: Create worktree and branch**

```bash
git worktree add .claude/worktrees/simplify-core-repository -b simplify/core-repository
```

- [ ] **Step 2: Survey** — Read all files, note dead code and decomposition targets as in Task 1.

- [ ] **Step 3: Remove private dead code** — 6 known markers plus any additional. Verify with `grep -rn` from repo root.

- [ ] **Step 4: Quick verification**

```bash
cd /home/peter/Conary/.claude/worktrees/simplify-core-repository
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 5: Decompose large files** — Priority: `sync.rs` (1825L), `resolution.rs` (1133L), `remi.rs` (1127L), `dependencies.rs` (1096L). The `parsers/` subdirectory already demonstrates the pattern for splitting by backend.

- [ ] **Step 6: Verify after decomposition**

```bash
cd /home/peter/Conary/.claude/worktrees/simplify-core-repository
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --exclude conary-test --verbose
```

- [ ] **Step 7: Clarify code** — Same pass as Task 1.

- [ ] **Step 8: Full verification** — Same as Task 1 Step 8, from the worktree.

- [ ] **Step 9: Commit** — Same format, branch `simplify/core-repository`.

### Task 6: Chunk 4 — core-build (17k lines)

**Scope:** `crates/conary-core/src/derivation/`, `crates/conary-core/src/recipe/`, `crates/conary-core/src/derived/`, `crates/conary-core/src/scriptlet/`, `crates/conary-core/tests/derivation_e2e.rs`, `crates/conary-core/examples/sign_hash.rs`
**Branch:** `simplify/core-build`
**Dead code markers:** 4 (all in `recipe/kitchen/provenance_capture.rs`)
**Feature gates:** none

**Known decomposition targets (15 files over 500 lines):**
- `scriptlet/mod.rs` — 1326 lines (priority)
- `recipe/cache.rs` — 1202 lines (priority)
- `derivation/executor.rs` — 928 lines
- `derivation/pipeline.rs` — 912 lines
- `recipe/graph.rs` — 874 lines
- `derived/builder.rs` — 865 lines
- `recipe/format.rs` — 844 lines
- `recipe/kitchen/cook.rs` — 841 lines
- `recipe/pkgbuild.rs` — 637 lines
- `recipe/kitchen/mod.rs` — 608 lines
- `derivation/recipe_hash.rs` — 582 lines
- `derivation/compose.rs` — 544 lines
- `derivation/seed.rs` — 527 lines
- `derivation/substituter.rs` — 516 lines
- `derivation/environment.rs` — 515 lines

- [ ] **Steps 1-9: Same structure as Task 5**, adapted for scope. Branch `simplify/core-build`.

### Task 7: Chunk 12 — conary-test (14k lines)

**Scope:** `apps/conary-test/src/` (all), `crates/conary-mcp/src/`
**Branch:** `simplify/conary-test`
**Dead code markers:** 2 (`server/mcp.rs`, `server/remi_client.rs`)
**Feature gates:** none

**Known decomposition targets (9 files over 500 lines):**
- `engine/runner.rs` — 1633 lines (priority)
- `cli.rs` — 1334 lines (priority)
- `server/mcp.rs` — 1156 lines
- `server/service.rs` — 1026 lines
- `engine/executor.rs` — 970 lines
- `container/lifecycle.rs` — 788 lines
- `config/manifest.rs` — 560 lines
- `server/handlers.rs` — 548 lines
- `config/mod.rs` — 526 lines

**Note on conary-mcp:** Only 83 lines. Quick read, likely minimal work.

- [ ] **Steps 1-9: Same structure as Task 5**, adapted for scope. Branch `simplify/conary-test`.

### Task 8: Wave 2 Review and Merge

Same process as Task 4: review diffs, then for each branch rebase from inside
its worktree, re-run the full verification matrix (including all feature-gated
checks), merge to main from the main worktree. Merge in order of smallest to
largest. Clean up worktrees and branches after all three are merged. Run the
full verification matrix on merged main.

---

## Wave 3: Core Data + Remi (Chunks 1, 2, 10)

### Task 9: Chunk 1 — core-db (19k lines)

**Scope:** `crates/conary-core/src/db/`
**Branch:** `simplify/core-db`
**Dead code markers:** 0
**Feature gates:** none

**Known decomposition targets (16 files over 500 lines):**
- `db/migrations/v41_current.rs` — 1162 lines
- `db/migrations/v1_v20.rs` — 1049 lines
- `db/migrations/v21_v40.rs` — 952 lines
- `db/models/trigger.rs` — 949 lines
- `db/models/derived.rs` — 851 lines
- `db/models/state.rs` — 744 lines
- `db/models/resolution.rs` — 708 lines
- `db/models/repository.rs` — 706 lines
- `db/models/label.rs` — 701 lines
- `db/models/provide_entry.rs` — 640 lines
- `db/models/trove.rs` — 622 lines
- `db/models/converted.rs` — 614 lines
- `db/models/config.rs` — 574 lines
- `db/models/subpackage.rs` — 568 lines
- `db/models/repository_requirement.rs` — 528 lines
- `db/schema.rs` — 523 lines

**Caution — migration files:** `db/migrations/` files contain schema migration SQL. These should be evaluated conservatively — splitting is fine if there are logical groupings, but do not change migration logic or ordering. Migration functions that run sequentially must stay in order.

- [ ] **Steps 1-9: Same structure as Task 5**, adapted for scope. Branch `simplify/core-db`.

### Task 10: Chunk 2 — core-model-ccs (24k lines)

**Scope:** `crates/conary-core/src/model/`, `crates/conary-core/src/ccs/`
**Branch:** `simplify/core-model-ccs`
**Dead code markers:** 0
**Feature gates:** none

**Known decomposition targets (19 files over 500 lines):**
- `model/replatform.rs` — 1739 lines (priority)
- `model/parser.rs` — 1677 lines (priority)
- `model/diff.rs` — 1178 lines
- `ccs/manifest.rs` — 1147 lines
- `ccs/builder.rs` — 1045 lines
- `ccs/policy.rs` — 961 lines
- `ccs/convert/legacy_provenance.rs` — 947 lines
- `model/remote.rs` — 848 lines
- `ccs/verify.rs` — 773 lines
- `ccs/chunking.rs` — 772 lines
- `ccs/legacy/mod.rs` — 731 lines
- `ccs/hooks/mod.rs` — 724 lines
- `ccs/convert/converter.rs` — 686 lines
- `ccs/enhancement/runner.rs` — 655 lines
- `ccs/package.rs` — 653 lines
- `ccs/convert/analyzer.rs` — 626 lines
- `model/mod.rs` — 575 lines
- `ccs/lockfile.rs` — 559 lines
- `ccs/binary_manifest.rs` — 534 lines

- [ ] **Steps 1-9: Same structure as Task 5**, adapted for scope. Branch `simplify/core-model-ccs`.

### Task 11: Chunk 10 — remi (34k lines)

**Scope:** `apps/remi/src/server/`, `apps/remi/src/federation/`, `apps/remi/src/trust.rs`, `apps/remi/src/lib.rs`, `apps/remi/src/bin/remi.rs`
**Branch:** `simplify/remi`
**Dead code markers:** 3 (`server/mcp.rs`, `server/handlers/seeds.rs`, `server/handlers/mod.rs`)
**Feature gates:** none

**Known decomposition targets (30 files over 500 lines):**
- `server/conversion.rs` — 1661 lines (priority)
- `server/admin_service.rs` — 1090 lines (priority; only 3 functions — very long functions)
- `server/handlers/chunks.rs` — 1050 lines
- `server/config.rs` — 1042 lines
- `federation/mod.rs` — 1005 lines
- `server/mod.rs` — 932 lines
- `server/handlers/oci.rs` — 910 lines
- `server/lite.rs` — 899 lines
- `server/test_db.rs` — 850 lines
- `server/cache.rs` — 784 lines
- `server/mcp.rs` — 782 lines
- (19 more files between 500-770 lines)

**Note on rmcp/axum liveness:** This crate uses `rmcp` for MCP tool generation and `axum` for HTTP routing. Fields annotated with `#[allow(dead_code)]` in `server/mcp.rs` are consumed by generated code — do not remove them. Apply Rule 2 carefully throughout.

- [ ] **Steps 1-9: Same structure as Task 5**, adapted for scope. Branch `simplify/remi`.

### Task 12: Wave 3 Review and Merge

Same process as Task 4: review diffs, rebase each branch from inside its
worktree, re-run full verification matrix (including all feature-gated checks),
merge to main one at a time. These are large chunks — review carefully. Clean
up worktrees and branches after all three are merged. Run the full verification
matrix on merged main.

---

## Wave 4: Heaviest Chunks (Chunks 6, 7, 9)

### Task 13: Chunk 6 — core-system (25k lines)

**Scope:** `crates/conary-core/src/capability/`, `crates/conary-core/src/bootstrap/`, `crates/conary-core/src/generation/`, `crates/conary-core/src/container/`, `crates/conary-core/src/filesystem/`, `crates/conary-core/benches/erofs_build.rs`
**Branch:** `simplify/core-system`
**Dead code markers:** 17 (concentrated in `bootstrap/`: `build_helpers.rs`(7), `tier2.rs`(4), `final_system.rs`(2), `image.rs`(2), `capability/inference/heuristics.rs`(1), `capability/resolver.rs`(1))
**Feature gates:** `composefs-rs` — must verify with BOTH default features AND `--no-default-features`

**Known decomposition targets (18 files over 500 lines):**
- `container/mod.rs` — 2135 lines (priority — largest in chunk)
- `bootstrap/image.rs` — 1954 lines (priority)
- `capability/inference/wellknown.rs` — 1442 lines
- `filesystem/cas.rs` — 1116 lines
- `generation/builder.rs` — 1025 lines
- `capability/inference/mod.rs` — 1025 lines
- (12 more files between 500-813 lines)

**Note on bootstrap dead code:** The 14 markers in `bootstrap/` represent scaffolded placeholder code. These are the highest-value dead code removal targets in the codebase — verify each is truly dead, then remove.

- [ ] **Step 1: Create worktree and branch**

```bash
git worktree add .claude/worktrees/simplify-core-system -b simplify/core-system
```

- [ ] **Steps 2-7: Same structure as Task 1** (survey, remove, verify, decompose, verify, clarify).

- [ ] **Step 8: Full verification with feature gates**

```bash
cd /home/peter/Conary/.claude/worktrees/simplify-core-system
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo test --workspace --exclude conary-test --verbose
cargo test -p conary-test --verbose
cargo test --doc --workspace --verbose
```

- [ ] **Step 9: Commit** — Same format, branch `simplify/core-system`.

### Task 14: Chunk 7 — core-supporting (19k lines)

**Scope:** `crates/conary-core/src/packages/`, `crates/conary-core/src/components/`, `crates/conary-core/src/trust/`, `crates/conary-core/src/provenance/`, `crates/conary-core/src/automation/`, `crates/conary-core/src/transaction/`, `crates/conary-core/src/delta/`, `crates/conary-core/src/trigger/`, `crates/conary-core/src/compression/`, `crates/conary-core/src/hash.rs`, `crates/conary-core/src/json.rs`, `crates/conary-core/src/label.rs`, `crates/conary-core/src/util.rs`, `crates/conary-core/src/self_update.rs`, `crates/conary-core/src/error.rs`, `crates/conary-core/src/lib.rs`, `crates/conary-core/src/progress.rs`, `crates/conary-core/src/federation_discovery.rs`
**Branch:** `simplify/core-supporting`
**Dead code markers:** 0
**Feature gates:** `experimental` — automation module is behind this flag

**Known decomposition targets (18 files over 500 lines):**
- `transaction/mod.rs` — 958 lines
- `self_update.rs` — 957 lines
- `automation/check.rs` — 853 lines
- `packages/arch.rs` — 756 lines
- `transaction/planner.rs` — 733 lines
- `components/classifier.rs` — 684 lines
- `packages/deb.rs` — 682 lines
- `packages/dpkg_query.rs` — 661 lines
- `hash.rs` — 630 lines
- `packages/rpm_query.rs` — 629 lines
- `trust/client.rs` — 627 lines
- `federation_discovery.rs` — 595 lines
- `components/filters.rs` — 583 lines
- `progress.rs` — 561 lines
- `trigger/mod.rs` — 544 lines
- `packages/rpm.rs` — 531 lines
- `trust/verify.rs` — 526 lines
- `packages/pacman_query.rs` — 520 lines

**Special responsibility — lib.rs:** This chunk owns `lib.rs` for structural edits (adding `mod` lines for new submodules created during decomposition within chunk 7's own modules). However, **removing `pub mod` lines is removing pub items**, which Phase 1 forbids. Any deferred notes from other chunks about dead modules are forwarded to Phase 2, not acted on here.

- [ ] **Step 1: Create worktree and branch**

```bash
git worktree add .claude/worktrees/simplify-core-supporting -b simplify/core-supporting
```

- [ ] **Steps 2-7: Same structure as Task 1** (survey, remove, verify, decompose, verify, clarify).

- [ ] **Step 8: Collect deferred lib.rs notes for Phase 2**

Check previous chunk merge commits for deferred notes about dead modules:
```bash
git log --grep="DEFERRED-PH2" --oneline main
```

Do NOT act on these — `pub mod` removal is pub item removal, which is Phase 2
work. Record any findings in this chunk's commit message with the same
`DEFERRED-PH2:` prefix so Phase 2 can find them all in one grep.

- [ ] **Step 9: Full verification**

```bash
cd /home/peter/Conary/.claude/worktrees/simplify-core-supporting
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p conary --all-targets --features experimental -- -D warnings
cargo test --workspace --exclude conary-test --verbose
cargo test -p conary-test --verbose
cargo test -p conary --features experimental --verbose
cargo test --doc --workspace --verbose
```

- [ ] **Step 10: Commit** — Same format, branch `simplify/core-supporting`.

### Task 15: Chunk 9 — cli-commands (40k lines)

**Scope:** `apps/conary/src/commands/`, `apps/conary/tests/`, `apps/conary/build.rs`
**Branch:** `simplify/cli-commands`
**Dead code markers:** 26 (most in codebase — `commands/federation.rs`(5), `commands/install/resolve.rs`(4), `commands/install/batch.rs`(3), `commands/install/dep_resolution.rs`(2), `tests/inference_benchmark.rs`(2), +9 files with 1 each)
**Feature gates:** `experimental` — must verify with `--features experimental`

**Known decomposition targets (23 files over 500 lines):**
- `commands/model.rs` — 2208 lines (priority — largest file in the crate)
- `commands/install/mod.rs` — 1979 lines (priority)
- `commands/ccs/install.rs` — 1830 lines (priority)
- `commands/bootstrap/mod.rs` — 1481 lines
- `commands/update.rs` — 1314 lines
- `tests/features.rs` — 1269 lines
- `commands/provenance.rs` — 1252 lines
- `commands/system.rs` — 1150 lines (7 fns, 164 lines/fn avg)
- `tests/conversion_integration.rs` — 1104 lines
- `commands/generation/takeover.rs` — 952 lines
- `commands/install/batch.rs` — 924 lines
- `commands/export.rs` — 875 lines
- `commands/generation/commands.rs` — 869 lines
- (10 more files between 500-826 lines)

**This is the largest and densest chunk.** 26 dead code markers + 23 large files. Prioritize dead code removal first (highest ROI), then decompose the top 5 largest files, then clarity pass.

- [ ] **Step 1: Create worktree and branch**

```bash
git worktree add .claude/worktrees/simplify-cli-commands -b simplify/cli-commands
```

- [ ] **Steps 2-7: Same structure as Task 1** (survey, remove, verify, decompose, verify, clarify).

- [ ] **Step 8: Full verification**

```bash
cd /home/peter/Conary/.claude/worktrees/simplify-cli-commands
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p conary --all-targets --features experimental -- -D warnings
cargo test --workspace --exclude conary-test --verbose
cargo test -p conary-test --verbose
cargo test -p conary --features experimental --verbose
cargo test --doc --workspace --verbose
```

- [ ] **Step 9: Commit** — Same format, branch `simplify/cli-commands`.

### Task 16: Wave 4 Review and Merge

Same process as Task 4. Merge order recommendation: chunk 7 first (owns lib.rs, may have deferred work), then 6, then 9 (largest).

After merging all three, run full verification on main:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo clippy -p conary --all-targets --features experimental -- -D warnings
cargo clippy -p conaryd --all-targets --features polkit -- -D warnings
cargo test --workspace --exclude conary-test --verbose
cargo test -p conary-test --verbose
cargo test -p conary --features experimental --verbose
cargo test --doc --workspace --verbose
```

---

## Task 17: Phase 2 — Cross-Chunk Simplification

**Prerequisite:** All 12 Phase 1 branches merged to `main`, full verification green.
**Branch:** `simplify/cross-chunk`

- [ ] **Step 1: Create branch**

```bash
git checkout -b simplify/cross-chunk main
```

- [ ] **Step 2: Collect Phase 1 deferred notes**

```bash
git log --grep="DEFERRED-PH2" --oneline main
```

These are the items previous chunk agents flagged for cross-chunk evaluation.

- [ ] **Step 3: Find dead pub items in conary-core**

For every `pub` and `pub(crate)` item (`fn`, `struct`, `enum`, `trait`, `type`, `const`) in `crates/conary-core/src/`:

1. Extract the item name
2. Search the **entire codebase** including tests, benches, examples, and doctests:
   ```bash
   grep -rn 'item_name' /home/peter/Conary/apps/ /home/peter/Conary/crates/
   ```
3. If zero textual callers, check non-textual liveness before marking as dead:
   - Is it a field on a `#[derive(Serialize, Deserialize)]` struct? → keep
   - Is it consumed by macro-generated code (rmcp, axum, clap)? → keep
   - Does it have `#[allow(dead_code)]` with an explanatory comment? → keep
   - Is it a re-export used in doctests or doc examples? → keep
4. Only if zero callers AND no non-textual liveness → removal candidate

Focus on `db::models` (most exported surface) and modules that had heavy Phase 1 cleanup.

- [ ] **Step 4: Remove confirmed dead pub items**

For each item confirmed dead (textual + non-textual checks passed):
1. Remove the item
2. Remove any now-orphaned imports
3. Run `cargo clippy --workspace --all-targets -- -D warnings` after each batch
4. Run `cargo test --doc --workspace` to catch broken doc examples

- [ ] **Step 5: Clean up lib.rs (pub mod + pub use)**

Two categories in `crates/conary-core/src/lib.rs`:

1. **Dead `pub mod` lines** — check DEFERRED-PH2 notes collected in Step 2.
   For each noted module, verify it has zero callers in the entire codebase:
   `grep -rn 'module_name' /home/peter/Conary/apps/ /home/peter/Conary/crates/`.
   If truly dead, remove the `pub mod` line and the module's files.
2. **Unused `pub use` re-exports** — for each re-exported item, verify it has
   callers outside the crate. Remove unused re-exports.

- [ ] **Step 6: Identify shared trait/type bloat**

Look for:
- Trait bounds that are wider than any impl needs
- Generic parameters that are always the same concrete type
- Type aliases that add indirection without clarity

Simplify where safe. This is surgical — don't refactor, just trim.

- [ ] **Step 7: Survey cross-crate duplication (document only)**

Compare patterns between:
- `apps/conaryd/src/` and `apps/remi/src/` (both are servers)
- `apps/conary/src/` and `apps/conaryd/src/` (both talk to the daemon DB)
- Config loading across all app crates

Write findings to `docs/superpowers/specs/cross-crate-duplication-findings.md`.
Do NOT refactor — that's a separate effort. The doc is the deliverable.

- [ ] **Step 8: Full verification**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo clippy -p conary --all-targets --features experimental -- -D warnings
cargo clippy -p conaryd --all-targets --features polkit -- -D warnings
cargo test --workspace --exclude conary-test --verbose
cargo test -p conary-test --verbose
cargo test -p conary --features experimental --verbose
cargo test --doc --workspace --verbose
```

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "simplify: cross-chunk cleanup — dead pub APIs and unused re-exports

- Removed [N] dead pub items from conary-core
- Removed [N] unused re-exports from lib.rs
- Trimmed [N] over-specified trait bounds
- Cross-crate duplication findings: docs/superpowers/specs/cross-crate-duplication-findings.md

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 10: Merge to main**

```bash
git checkout main
git merge simplify/cross-chunk --no-ff
git branch -d simplify/cross-chunk
```

Final verification on merged main (same as Step 7).
