---
last_updated: 2026-03-14
revision: 3
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

`conary-core/src/resolver/sat.rs` lines 182-212 check whether a dependency is
satisfied by scanning installed solvables for a **name match**. The bug appears
in **both branches** of the satisfaction check:

- **Lines 185-195** (dep NOT being removed): `alt.name == dep_name`
- **Lines 197-207** (dep being removed): `alt.name == dep_name` with additional
  `!remove_set.contains()` filter

```rust
alt.name == dep_name  // Only checks package NAME, not provides
```

Both branches must be patched. Dependencies can be satisfied by
**provides/capabilities** from differently-named packages. The code never
queries `ProvideEntry`.

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

`src/commands/ccs/install.rs:364-369` hard-rejects CCS packages with
capability declarations: `"install-time capability enforcement is not yet
supported"`. Needs a real policy engine.

## Design

### 1. Fix `solve_removal()` to Use Provides

**Location:** `conary-core/src/resolver/sat.rs`, `solve_removal()` function.

Replace the inner satisfaction check (both branches at lines 185-195 and
197-207) to query provides instead of matching package names.

**Provides cache location:** Load the provides cache into `ConaryProvider`
alongside installed packages (via `load_installed_packages()` or a new
`load_installed_provides()` method). This keeps all DB access in the provider
and maintains the existing pattern where `sat.rs` operates on in-memory data
only. The cache is a `HashMap<String, Vec<(i64, Option<String>)>>` mapping
capability name to `(trove_id, version)` pairs.

**Algorithm:**

```
In ConaryProvider::load_installed_provides():
  Query all ProvideEntry rows for installed troves
  Build HashMap<capability_name, Vec<(trove_id, version)>>
  Also index capability variations (via generate_capability_variations())

In solve_removal(), for each dep (dep_name, constraint):
  1. If dep_name is being removed:
     Query provider's provides cache for dep_name
     Filter out providers whose trove is in the remove_set
     If any provider remains -> satisfied
  2. If dep_name is NOT being removed:
     Query provider's provides cache for dep_name
     If any provider exists -> satisfied
  3. Fall back to fuzzy matching (soname variations, cross-distro)
     via the pre-indexed capability variations
```

This leverages existing fuzzy matching in `ProvideEntry` which handles sonames,
cross-distro variations, case-insensitive matching, and paren-pattern matching.

**Performance:** Preload provides into `ConaryProvider` at the start rather
than per-dependency DB queries. On a fully adopted Fedora system, the provides
table may contain 20,000-50,000 entries (sonames, file provides, virtual
capabilities across 400+ packages). Each entry is a small string, so memory
impact remains manageable.

### 2. Verify Adopted Package Provides Granularity

The adoption code paths (`system.rs:403-425` and `packages.rs:265-287`) do
record provides via `ProvideEntry::new()` during `system adopt`. The install
command's `mod.rs:1280-1295` records provides for packages installed via
`--dep-mode takeover`. So provides data should exist.

**The real risk is granularity mismatch**, not missing data. RPM provides
strings like `"libc.so.6(GLIBC_2.34)(64bit)"` may not match dependency names
like `"libc.so.6"` without fuzzy matching. The `solve_removal()` fix must use
`generate_capability_variations()` to bridge this gap.

**Verification step:** After implementing the `solve_removal()` fix, run the
group-m tests. If they still fail, inspect the provides and dependencies tables
to identify granularity mismatches and tune the fuzzy matching accordingly.
This step is likely unnecessary but serves as a safety net.

### 3. CCS Install `--reinstall` Flag

**Location:** `src/commands/ccs/install.rs:374-390` and CLI definition.

- Add `--reinstall` flag to the `ccs install` CLI
- When set, skip the same-version "already installed" bail
- Proceed through normal install flow (overwrite files, re-record in DB)
- The existing upgrade path (different version) is unaffected
- Named `--reinstall` (not `--force`) for clarity of intent

Scope: ~15 lines across CLI definition and `ccs/install.rs`.

**Note on T148:** T148 has two independent fix paths. The primary path is the
`solve_removal()` fix (step 1), which makes the cleanup `remove` step succeed
so the "already installed" check is never hit. If the `solve_removal()` fix
does not fully resolve T148's cleanup (e.g., due to provides granularity
mismatch), the `--reinstall` flag serves as a fallback -- the test manifest
cleanup step could be updated to use `ccs install --reinstall` instead of
relying on remove + reinstall.

### 4. Move T150 to QEMU Test Suite

- Remove T150 from `phase3-group-n-container.toml`
- Move T150 into the existing `phase3-group-n-qemu.toml` (which already
  contains T156-T159). T150 keeps its test ID.
- Adapt T150 to use `qemu_boot` step type (matching the existing QEMU tests)
  instead of `conary` and `run` steps
- Test flow: boot QEMU VM from base image, install kernel package, verify
  `/boot/` and `/lib/modules/` contents
- Fallback: if QEMU support isn't ready for full test execution, skip T150
  with reason `"requires bare-metal or VM environment"`
- **Dependency chain:** T151, T153, and T154 in `phase3-group-n-container.toml`
  have `depends_on = ["T150"]`. These tests (generation BLS entry, generation
  rollback boot entries, bootloader config paths) also require kernel/boot
  file deployment. They should move to the QEMU manifest alongside T150, or
  have their dependency updated if they can run independently.

### 5. Capability Policy Engine

**Architecture:** Three-tier policy model for install-time Linux capability
(`CAP_*`) enforcement.

**Relationship to existing module:** The CCS manifest field gated at
`install.rs:364-369` is `capabilities: Option<CapabilityDeclaration>`
(manifest.rs:68). `CapabilityDeclaration` contains `NetworkCapabilities`,
`FilesystemCapabilities`, and `SyscallCapabilities` -- higher-level
declarations, not raw Linux `CAP_*` constants.

The policy engine evaluates these declarations and **infers** required Linux
capabilities. For example: `network.bind_ports` containing ports < 1024 implies
`CAP_NET_BIND_SERVICE`; raw socket access implies `CAP_NET_RAW`; filesystem
paths outside the package prefix may imply `CAP_DAC_OVERRIDE`. The inference
mapping is part of the policy engine implementation. The test fixtures (T104,
T105) declare capabilities that map to specific Linux `CAP_*` requirements
which the policy then allows, prompts, or denies.

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
| T148 | Cleanup fails then "already installed" | solve_removal (primary); --reinstall (fallback) |
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
