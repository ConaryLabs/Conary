---
last_updated: 2026-03-14
revision: 1
summary: Design for fixing 7 integration test failures and implementing capability policy engine
---

# Test Failures & Capability Policy Engine Design

## Problem Statement

The conary-test integration suite has 7 failing tests and 2 skipped tests that
need attention:

- **7 failures** across 3 suites (group-j: T112, T114; group-m: T142, T145,
  T148, T149; group-n: T150)
- **2 skips** (T104, T105) waiting on capability policy enforcement
- **Root cause**: `solve_removal()` in `sat.rs` matches dependencies against
  package names only, never consulting the provides table

## Root Cause Analysis

### The `solve_removal()` Bug (6 of 7 failures)

`conary-core/src/resolver/sat.rs:185-194` checks whether a dependency is
satisfied by scanning installed solvables for a **name match**:

```rust
alt.name == dep_name  // Only checks package NAME, not provides
```

Dependencies can be satisfied by **provides/capabilities** from
differently-named packages. The code never queries `ProvideEntry`.

**Group J (T112, T114):** CCS packages depend on virtual capabilities. The
provider package has a different name than the capability. Name-only matching
fails to find the alternative provider, so removal is incorrectly blocked.

- T112: `dep-virtual-consumer` depends on capability `"virtual-cap"`.
  `dep-virtual-provider` provides it, but no package is named `"virtual-cap"`.
- T114: `dep-or-consumer` depends on a capability that both `dep-or-a` and
  `dep-or-b` provide. Removing `dep-or-a` should be safe (dep-or-b remains),
  but name-only matching doesn't find the alternative.

**Group M (T142, T145, T148, T149):** After `system adopt --system` and
`--dep-mode takeover`, adopted packages have RPM-sourced capability deps
(sonames like `"libc.so.6()(64bit)"`). No package is named after a soname.
Every adopted package appears to have unsatisfied deps, so removing ANY
package triggers "40 packages depend on it."

- T142, T145, T149: Cleanup steps try `conary remove <pkg>`, blocked by
  false dependents. Cleanup silently fails (masked by `; true`).
- T148: Cleanup fails, then `ccs install` hits "already installed" error.

### T150: Container Environment Limitation

`kernel_file_deployment` expects files in `/boot/` and `/lib/modules/` after
kernel package install. Kernel RPM scripts detect container environments and
skip boot file deployment. Output is `"0"` (zero files found).

### T104/T105: Capability Policy Not Implemented

`src/commands/ccs/install.rs:385-391` hard-rejects CCS packages with
capability declarations: `"install-time capability enforcement is not yet
supported"`. Needs a real policy engine.

## Design

### 1. Fix `solve_removal()` to Use Provides

**Location:** `conary-core/src/resolver/sat.rs`, `solve_removal()` function.

Replace the inner satisfaction check (lines 182-211) to query provides instead
of matching package names.

**Algorithm:**

```
At start of solve_removal():
  Preload all provides into HashMap<String, Vec<(trove_id, version)>>
  from ProvideEntry table (for performance, avoid per-dep DB queries)

For each dep (dep_name, constraint) of each installed package:
  1. If dep_name is being removed:
     Query provides map for dep_name
     Filter out providers whose trove is in the remove_set
     If any provider remains -> satisfied
  2. If dep_name is NOT being removed:
     Query provides map for dep_name
     If any provider exists -> satisfied
  3. Fall back to fuzzy matching (soname variations, cross-distro)
     via ProvideEntry::generate_capability_variations()
```

This leverages existing fuzzy matching in `ProvideEntry` which handles sonames,
cross-distro variations, case-insensitive matching, and paren-pattern matching.

**Performance:** Preload provides into memory at the start rather than
per-dependency DB queries. The provides table is bounded by installed package
count, so memory impact is small.

### 2. Verify/Fix Adopted Package Provides

**Prerequisite check:** Query the DB on a test container after
`system adopt --system` to verify adopted packages have provide entries.

**If provides are missing:** Add provide recording to the adoption code path,
importing from RPM metadata (sonames, file provides, virtual capabilities).
This is the same data conary already parses during `repo sync` for repository
packages.

**If provides are already recorded:** No change needed -- the `solve_removal()`
fix alone handles everything.

### 3. CCS Install `--reinstall` Flag

**Location:** `src/commands/ccs/install.rs:374-390` and CLI definition.

- Add `--reinstall` flag to the `ccs install` CLI
- When set, skip the same-version "already installed" bail
- Proceed through normal install flow (overwrite files, re-record in DB)
- The existing upgrade path (different version) is unaffected
- Named `--reinstall` (not `--force`) for clarity of intent

Scope: ~15 lines across CLI definition and `ccs/install.rs`.

### 4. Move T150 to QEMU Test Suite

- Remove T150 from `phase3-group-n-container.toml`
- Create `phase3-group-n-qemu.toml` using existing QEMU boot step support
  (`conary-test/src/engine/qemu.rs`)
- Test flow: boot QEMU VM from base image, install kernel package, verify
  `/boot/` and `/lib/modules/` contents
- Fallback: if QEMU support isn't ready for full test execution, skip T150
  with reason `"requires bare-metal or VM environment"`

### 5. Capability Policy Engine

**Architecture:** Three-tier policy model for install-time capability
enforcement.

**Policy tiers:**

| Tier | Behavior | Example capabilities |
|------|----------|---------------------|
| Allowed | Install proceeds silently | `cap-dac-read-search` |
| Prompt | Requires user confirmation; rejected in non-interactive mode unless `--allow-capabilities` | `cap-net-raw`, `cap-sys-ptrace` |
| Denied | Always rejected | `cap-sys-admin` (in constrained envs) |

**Default policy:** Built-in defaults with sensible tier assignments. Users
override via `/etc/conary/capability-policy.toml` or
`--capability-policy <path>` flag.

**Enforcement point:** Replace the hard-reject in `ccs/install.rs:385-391`:

1. Parse declared capabilities from CCS manifest
2. Load policy (built-in defaults + user overrides)
3. For each capability: check tier -> allow / prompt / deny
4. Denied -> reject with clear message (T105 scenario)
5. Prompt -> ask user or reject in non-interactive mode (T104 scenario)
6. All allowed -> proceed with install

**Policy file format:**

```toml
[capabilities]
allowed = ["cap-dac-read-search", "cap-chown"]
prompt = ["cap-net-raw", "cap-sys-ptrace", "cap-net-bind-service"]
denied = ["cap-sys-admin", "cap-sys-rawio"]

# Unlisted capabilities default to "prompt"
default_tier = "prompt"
```

**Audit:** Record capability decisions in install history -- what was requested,
what was approved/denied, interactive vs flag-based approval.

## Test Coverage

After implementation, the following tests should pass:

| Test | Issue | Fix |
|------|-------|-----|
| T112 | Alternative provider not found | solve_removal provides check |
| T114 | OR dependency alternative not found | solve_removal provides check |
| T142 | Takeover removal blocked | solve_removal + adopted provides |
| T145 | Takeover removal blocked | solve_removal + adopted provides |
| T148 | Cleanup fails then "already installed" | solve_removal (cleanup works) |
| T149 | Takeover removal blocked | solve_removal + adopted provides |
| T150 | Container kernel deploy | QEMU migration |
| T104 | Capability policy missing | Policy engine (prompt tier) |
| T105 | Capability policy missing | Policy engine (denied tier) |

## Implementation Order

1. Fix `solve_removal()` (highest leverage -- unblocks 6 tests)
2. Verify/fix adopted package provides (may be unnecessary)
3. CCS `--reinstall` flag (small, isolated)
4. T150 QEMU migration (test infrastructure)
5. Capability policy engine (largest piece, standalone feature)
