---
last_updated: 2026-03-15
revision: 1
summary: Fix all readiness gaps before public announcement -- failing tests, missing commands, UX, consistency
---

# Pre-Launch Readiness Sweep

## Problem Statement

The project is feature-complete for a v0.5.0 announcement, but a readiness
audit found gaps across three tiers that would embarrass the project if
discovered by new users, contributors, or reviewers.

## Gaps to Fix

### Block 1: Showstoppers

**1. Five failing unit tests**

These fail on a clean `cargo test` run:

- `bootstrap::toolchain::tests::test_toolchain_tool_paths` -- the tool()
  method now falls back to unprefixed binaries, but the test still asserts
  the prefixed path exists. Update the test to match the new fallback logic.
- `bootstrap::toolchain::tests::test_toolchain_env` -- same root cause as
  above; env() builds paths using tool() which changed behavior.
- `container::tests::test_container_config_pristine_for_bootstrap` -- the
  pristine_for_bootstrap() function now mounts host essentials (/usr/bin,
  /lib64, /tmp). The test asserts an empty bind_mounts list. Update to
  expect the new mounts.
- `filesystem::vfs::tests::test_remove_directory_with_children` -- edge
  case in VFS directory removal. Debug and fix.
- `db::tests::test_open_rejects_corrupt_wal_sidecar` -- WAL corruption
  detection test is fragile. Fix the test setup or assertion.

**2. `conary capability enforce` command missing**

README shows `conary capability enforce nginx` as a feature. The command
does not exist in the CLI. Two options:

- **(A) Implement it**: Wire the existing `CapabilityDeclaration` +
  `CapabilityPolicy` into a CLI command that loads a package's declared
  capabilities and evaluates them against the policy. Output: which caps
  are allowed/prompted/denied.
- **(B) Remove from README**: Mark capability enforcement as
  "install-time only" and remove the `enforce` example.

Decision: **(A) Implement it** -- the policy engine exists, this is just a
thin CLI wrapper. The `capability audit` command (also in README) should
work similarly: run the package in audit mode and report what capabilities
it actually uses.

Implementation:
- `conary capability enforce <package>` -- loads package's
  CapabilityDeclaration from DB, evaluates against CapabilityPolicy,
  prints tier for each capability.
- `conary capability audit <package>` -- if the hidden `audit` command
  exists in CLI definition, verify it works. If stub, implement basic
  version that shows declared capabilities + policy evaluation.

**3. First-run UX: "Database not found" error**

When a user runs `conary install foo` without `conary system init`, they
get: `"Database not found at path: /var/lib/conary/conary.db"`

Fix: detect the missing-DB case and print:
```
Error: Database not initialized.
Run 'conary system init' to set up the package database.
```

Location: `conary-core/src/db/mod.rs` in the `open()` or `open_fast()`
function. Check if file exists before attempting to open; if not, return a
typed error that the CLI layer can catch and enhance with the hint.

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

- `conary-server/Cargo.toml` is at 0.4.0 -- bump to 0.5.0
- `conary-test/Cargo.toml` is at 0.2.0 -- this is fine (separate version
  track per CLAUDE.md), but add a comment in the TOML explaining why

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
- Ensure capability section accurately describes install-time policy (not
  runtime enforcement, which is aspirational)
- Add a "Project Status" section or badge showing test count (249 tests,
  248 passing)

### Block 3: Polish

**8. Error messages**

Beyond the two specific fixes above, do a sweep of common error paths:
- `conary repo sync` with no repos configured
- `conary remove <not-installed-package>`
- `conary ccs install <nonexistent-file>`
- `conary ccs install <file> --db-path <missing-db>`

Each should produce a clear, actionable error message. No raw SQLite
errors, no panics, no "No such file or directory" without context.

**9. `cargo test` fully green**

After fixing the 5 specific failures, run the full suite and fix any
other failures or warnings. Target: `cargo test` exits 0 with no
failures.

**10. Site content check**

Read `site/src/` to verify that any marketing copy on conary.io matches
the current feature set. Flag aspirational claims.

## Implementation Order

1. Fix 5 failing unit tests (unblocks "cargo test passes")
2. Fix first-run UX (DB not found hint)
3. Fix duplicate repo add error
4. Implement `capability enforce` command
5. Version sync + ROADMAP update
6. README accuracy sweep
7. Error message polish sweep
8. Site content check
9. Final `cargo test && cargo clippy` verification

## Success Criteria

- `cargo test` exits 0 with no failures
- `cargo clippy -- -D warnings` passes
- A new user can: `cargo build`, `conary system init`, `conary repo sync`,
  `conary install tree` without hitting any confusing errors
- README makes no claims the code can't back up
- ROADMAP reflects actual project state
