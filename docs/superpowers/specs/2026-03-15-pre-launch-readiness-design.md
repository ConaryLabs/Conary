---
last_updated: 2026-03-15
revision: 3
summary: Fix all readiness gaps before public announcement -- failing tests, missing commands, UX, consistency
---

# Pre-Launch Readiness Sweep

## Problem Statement

The project is feature-complete for a v0.5.0 announcement, but a readiness
audit found gaps across three tiers that would embarrass the project if
discovered by new users, contributors, or reviewers.

## Gaps to Fix

### Block 1: Showstoppers

**1. Six failing unit tests (5 in conary-core + 1 integration)**

These fail on a clean `cargo test` run:

- `bootstrap::toolchain::tests::test_toolchain_tool_paths` -- the tool()
  method now falls back to unprefixed binaries, but the test still asserts
  the prefixed path exists. Update the test to match the new fallback logic.
- `bootstrap::toolchain::tests::test_toolchain_env` -- same root cause as
  above; env() builds paths using tool() which changed behavior.
- `container::tests::test_container_config_pristine_for_bootstrap` -- the
  pristine_for_bootstrap() function now mounts host essentials (/usr/bin,
  /lib64, /tmp). The test asserts an empty bind_mounts list. Update to
  expect the new mounts. Also update `is_pristine()` (line 303) to accept
  these specific host paths as valid for "pristine bootstrap" containers.
  **Also fix the integration test** in `tests/target_root.rs:154` which
  fails for the same reason.
- `filesystem::vfs::tests::test_remove_directory_with_children` --
  **Production bug**, not just a test issue. The `remove()` method at
  `conary-core/src/filesystem/vfs/mod.rs:427-460` processes the
  `to_remove` vector sequentially, setting `parent = None` on each node.
  Later `get_path(id)` calls for descendant nodes fail because the parent
  chain is already broken. **Fix:** collect all paths via `get_path()`
  BEFORE mutating any nodes, then remove them from the path_index.
- `db::tests::test_open_rejects_corrupt_wal_sidecar` -- **Production bug**
  in `validate_wal_file()` at `conary-core/src/db/mod.rs:43`. For files
  without an extension (e.g., `/foo/bar`), the function constructs the WAL
  path as `/foo/bar.-wal` instead of `/foo/bar-wal`. The test writes
  corruption to `*.db-wal` but validation looks for `*.-wal` — they never
  match. **Fix the path construction** in `validate_wal_file()` to handle
  extensionless files, then verify the test passes.

**2. `conary capability enforce` and `audit` commands hidden**

README shows `conary capability enforce nginx` and `conary capability audit
nginx` as features. Both commands **already exist and are fully implemented**
at `src/commands/capability.rs:307-526`, but are hidden from --help via
`#[command(hide = true)]` in `src/cli/capability.rs:86-108`.

**Fix:**
- Un-hide both `audit` and `run` commands in the CLI definition
- Add `enforce` as a visible alias for `run` (or rename `run` to `enforce`
  and keep `run` as a hidden alias for backwards compatibility)
- Verify both commands work end-to-end with an installed package

**3. First-run UX: "Database not found" error**

When a user runs `conary install foo` without `conary system init`, they
get: `"Database not found at path: /var/lib/conary/conary.db"`

Fix: add a centralized error mapping in `src/main.rs` (or the top-level
error handler) that catches `Error::DatabaseNotFound` and enhances it:
```
Error: Database not initialized.
Run 'conary system init' to set up the package database.
```

Do NOT scatter this across 50+ call sites. Handle it once at the top-level
error handler where anyhow errors are formatted for the user.

**4. Duplicate repo add: raw SQLite error**

`conary repo add remi https://...` when remi already exists gives:
`"UNIQUE constraint failed: repositories.name"`

Fix: catch the unique constraint error in the repo add command and return:
```
Error: Repository 'remi' already exists.
Use 'conary repo list' to see configured repositories.
```

Location: `src/commands/repo.rs` in the add handler. Catch
`rusqlite::Error` with `ErrorCode::ConstraintViolation` and map to a
user-friendly message.

### Block 2: Consistency

**5. Version sync**

- `conary-server/Cargo.toml` is at 0.4.0 -- leave at 0.4.0 (separate
  version track per CLAUDE.md, `server-v` tag prefix). Bumping without a
  server-specific change would violate semver. Add a comment in the TOML
  explaining the separate version track.
- `conary-test/Cargo.toml` is at 0.2.0 -- same rationale, add comment.

**6. ROADMAP update**

Update ROADMAP.md to reflect:
- Phase 4 tests (T160-T255) completed
- Bootstrap image generation working (31 packages + qcow2)
- Test infrastructure overhaul complete (Remi API, WAL, 24 MCP tools)
- Mark completed items with [COMPLETE]

**7. README accuracy**

- Mark system generations as "functional but limited real-world testing"
- Mark bootstrap as "working (31 packages built from source, qcow2 image
  generation), full distro bootstrap in progress"
- Update capability section to use `conary capability enforce` (not just
  install-time policy) and `conary capability audit` — both are real
  commands once un-hidden
- Add a "Project Status" section showing: 249 integration tests (248
  passing, 1 environment skip), plus ~1500 unit tests

### Block 3: Polish

**8. Error messages**

Systematic approach: grep for raw `rusqlite::Error` propagation in
`src/commands/` (any `.context("...")?` or bare `?` on DB operations).
Also check for `.unwrap()` in command handlers.

Specific paths to verify:
- `conary repo sync` with no repos configured
- `conary remove <not-installed-package>`
- `conary ccs install <nonexistent-file>`
- `conary ccs install <file> --db-path <missing-db>`

Each should produce a clear, actionable error message. No raw SQLite
errors, no panics, no "No such file or directory" without context.

**9. `cargo test` fully green**

After fixing the 6 specific failures (5 unit + 1 integration), run:
- `cargo test` (all unit + integration tests)
- `cargo test --features server` (server-only tests)
- `cargo test -p conary-test` (test infrastructure tests)

Target: all exit 0 with no failures. The 249 integration tests are
separate (run on Forge), but unit tests must pass locally.

**10. Site content check**

Review `site/src/app.html` and any route components to verify marketing
copy matches current features. Flag aspirational claims.

### Block 4: Progress Bar Coverage

**11. Add progress feedback to long-running operations**

The `indicatif`-based progress system exists (`src/commands/progress.rs`)
with 4 progress types (`InstallProgress`, `RemoveProgress`,
`UpdateProgress`, `AdoptProgress`). These are wired into install, remove,
update, and adopt — but NOT into many other operations that can run for
seconds to hours with zero user feedback.

**Operations that need progress bars (prioritized by user visibility):**

| Command | Duration | Current UX | Fix |
|---------|----------|-----------|-----|
| `repo sync` | 5-60s | Silent until done | Spinner + package count |
| `ccs build` | 10s-5min | Prints final result only | Phase spinner (parse, compress, write) |
| `self-update --check` + download | 5-30s | Silent | Download progress bar |
| `system takeover` | 30s-5min | Prints at end | Phase spinner + package counter |
| `bootstrap stage0/1/2/base` | 30min-4hr | Log lines only | Overall progress bar + per-package status |
| `cook` | 10s-10min | Silent until done | Phase spinner (fetch, configure, build, install) |
| `model apply` | 10s-5min | Silent | Reuse InstallProgress/RemoveProgress |
| `system adopt --system` | 10s-60s | Has AdoptProgress | Already done (verify) |

**Implementation approach:**

- `repo sync`: Add `SyncProgress` with spinner showing repo name + package
  count as metadata downloads. The sync operation already iterates packages
  — wrap in progress updates.
- `ccs build`: Add phase status prints: "Parsing manifest...",
  "Compressing files...", "Writing CCS package...". Lightweight — just
  `println!` at key points, no full progress bar needed.
- `self-update`: Add download progress bar using `indicatif` with
  content-length from HTTP response.
- `system takeover`: Add phase spinner (scanning, converting, recording).
- `bootstrap`: The bootstrap already prints `[1/12] Building zlib...`
  style output via `tracing::info!`. Add an overall progress bar wrapping
  the per-package logs.
- `cook`: Add phase prints at fetch/configure/build/install boundaries.
- `model apply`: Delegate to existing InstallProgress/RemoveProgress for
  the package operations it triggers.

**Also add lightweight feedback to "instant" commands:**

Even fast operations should confirm what happened. No progress bar needed,
but consistent status output:
- `pin`/`unpin`: "Pinned tree v2.1" / "Unpinned tree"
- `repo add`: "Added repository 'fedora-remi'" (already prints this)
- `config backup`: "Backed up /etc/hostname -> backup #3"
- `system gc`: "Garbage collection: freed 12MB (45 orphaned objects)"
- `collection create`: "Created collection 'base-server' with 3 members"

The pattern: every mutating command prints a one-line confirmation of what
it did. Read-only commands (list, search, query) just output their results.

## Implementation Order

1. Fix 6 failing tests (VFS bug is production code, WAL bug is production
   code, rest are test updates)
2. Fix first-run UX (DB not found hint -- centralized)
3. Fix duplicate repo add error
4. Un-hide capability enforce/audit commands
5. Version comments + ROADMAP update
6. README accuracy sweep
7. Error message polish sweep
8. Progress bars for repo sync, ccs build, self-update, takeover, cook
9. Site content check
10. Final `cargo test && cargo test --features server && cargo clippy`

## Success Criteria

- `cargo test` exits 0 with no failures
- `cargo test --features server` exits 0
- `cargo clippy -- -D warnings` passes
- Every long-running command shows progress feedback (spinner or bar)
- Every mutating command prints a confirmation of what it did
- A new user can: `cargo build`, `conary system init`, `conary repo sync`,
  `conary install tree` without hitting any confusing errors
- `conary capability enforce <pkg>` and `conary capability audit <pkg>`
  appear in --help and produce meaningful output
- README makes no claims the code can't back up
- ROADMAP reflects actual project state
