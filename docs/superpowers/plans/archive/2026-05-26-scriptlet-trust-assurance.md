# Scriptlet Trust Assurance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Make Conary's scriptlet trust story precise, test-backed, and visible to operators before the preview widens.

**Architecture:** Start with an assurance audit of protected live-root execution,
then either update stale docs or harden the implementation. Add pre-mutation
sandbox setup checks, typed scriptlet outcomes, regression tests around unsafe
root-transition capabilities, structured degradation metadata for true script
exit failures, and a narrow capability model for packages that need real host
integration.

**Tech Stack:** Rust scriptlet/container code, seccomp capability declarations, rusqlite changeset metadata, Markdown security docs, focused cargo tests.

---

## Implementation Result

Completed on 2026-05-26.

- Protected live-root scriptlets install the enforce-mode `scriptlet` seccomp
  profile, and the profile remains covered by a regression test that excludes
  `chroot`.
- Protected live-root scriptlets run a pre-mutation preflight for namespace,
  private-layer, interpreter, and enforcement readiness.
- Scriptlet execution now returns typed outcomes with `failure_kind`,
  `requested_sandbox_mode`, and `effective_sandbox`.
- Warning-only post-install/post-remove failures are limited to true nonzero
  script exits, stored as `scriptlet_warning` changeset metadata, and surfaced
  by `conary history`.
- CCS manifests accept the narrow `[[scriptlets.capabilities]]` vocabulary,
  preserve it through package archive parsing, reject unknown declarations, and
  fail closed at install time until enforcement exists unless the operator
  chooses `--sandbox=never`.
- Security, README, Conaryopedia, CCS format, roadmap, and docs-audit metadata
  were updated to match the implemented behavior.

## Scope

This plan implements Plan B from
`docs/superpowers/specs/archive/2026-05-26-limited-preview-release-hardening-design.md`.
It does not replace the entire scriptlet system and does not make
`--sandbox=never` safe. It makes protected mode and direct-mode failure
semantics honest.

## File Structure

- Modify `docs/SCRIPTLET_SECURITY.md`: exact protected-mode guarantees and
  direct-mode limits.
- Modify `crates/conary-core/src/container/mod.rs`: protected root-transition
  implementation or explicit assertion helpers.
- Modify `crates/conary-core/src/capability/declaration.rs`: ensure scriptlet
  syscall profile excludes `chroot` and captures allowed root-transition
  behavior.
- Modify `crates/conary-core/src/scriptlet/mod.rs`: expose structured execution
  outcomes and live-root sandbox preflight if needed.
- Modify `apps/conary/src/commands/install/scriptlets.rs`,
  `apps/conary/src/commands/remove.rs`, and upgrade scriptlet call sites:
  record warning-only post-scriptlet failures in structured metadata.
- Modify `apps/conary/src/commands/changeset_metadata.rs`: add a scriptlet
  warning/degradation envelope next to existing adoption/publication metadata.
- Modify `docs/specs/ccs-format-v1.md` and
  `crates/conary-core/src/ccs/manifest.rs` only for the
  capability-declaration design slice.
- Add or extend tests under `crates/conary-core/src/container/mod.rs`,
  `crates/conary-core/src/capability/declaration.rs`, and
  `apps/conary/tests/scriptlet_harness/`.

## Review-Tightened Decisions

- If protected live-root scriptlets still rely on raw `chroot`, this plan must
  harden implementation before docs wording.
- If protected live-root scriptlets already use a safer root transition, this
  plan must remove stale `chroot` wording and add tests that prevent regression.
- Direct execution remains an explicit legacy escape hatch; do not imply it is
  sandboxed.
- Post-install/post-remove legacy scriptlet failures may remain warning-only
  after package file state changes only when the sandbox was set up correctly
  and the script process itself exited nonzero. Sandbox setup/enforcement
  failures must be detected before file/DB mutation or fail the command.
- The security story must distinguish `requested_sandbox_mode` from
  `effective_sandbox`; `auto` can choose direct execution for low-risk
  live-root scripts and must not be described as protected.
- Capability declarations should be narrow and declarative. They should not be
  a synonym for "run the whole script unsandboxed."
- If capability declarations parse before enforcement exists, install paths
  must fail closed unless the operator explicitly chooses direct execution with
  `--sandbox=never`.

---

### Task 1: Protected Sandbox Assurance Audit

**Files:**
- Read: `docs/SCRIPTLET_SECURITY.md`
- Read: `crates/conary-core/src/container/mod.rs`
- Read: `crates/conary-core/src/capability/declaration.rs`
- Read: `crates/conary-core/src/scriptlet/mod.rs`

- [x] **Step 1: Locate live-root protected execution**

Run:

```bash
rg -n "execute_sandbox_live|pivot_root|chroot|SandboxMode|namespace|seccomp" crates/conary-core/src/container/mod.rs crates/conary-core/src/scriptlet/mod.rs crates/conary-core/src/capability/declaration.rs
```

- [x] **Step 2: Classify the actual root-transition mechanism**

Record the answer in the implementation PR description with separate cases:

```text
protected live-root root/no-userns: pivot_root required | other
protected live-root unprivileged-userns: chroot after non-host-root mapping | other
protected live-root chroot fallback in enforce mode: fatal | allowed
target-root/offline install transition: chroot/container | other
scriptlet syscall profile allows chroot: yes | no
live-root protected seccomp profile installed: yes | no
```

If the profile allows `chroot`, stop and either remove it from protected
scriptlet execution or update this plan with the exact compatibility reason.
If live-root protected mode does not install the scriptlet seccomp profile,
either add it or make the docs explicit that live-root protection relies on
namespace and mount isolation rather than seccomp.

- [x] **Step 3: Classify hardened-kernel and container failure modes**

Run the sandbox preflight or focused sandbox test in environments where
namespace creation can be restricted:

```text
standard local host
rootless or restricted container
hardened kernel with unprivileged user namespaces disabled
```

The implementation must turn namespace setup failures into one clear error that
names the missing capability. The primary remediation must be safer
environmental setup, not direct execution:

```text
Protected scriptlet sandboxing requires mount and user namespace support.
Enable the required kernel/container namespace support or run inside a VM.
Dangerous legacy direct execution is available only with --sandbox=never plus
the live-host mutation acknowledgement, and it records effective_sandbox=direct.
```

- [x] **Step 4: Add an assurance note to the security doc**

Add a short section:

```markdown
## Assurance Notes

Protected live-root scriptlets do not receive the `chroot` syscall in any
enforced live-root seccomp profile. When unprivileged user namespaces are used,
setup may enter the prepared root with chroot after root maps to a non-host
UID/GID; that is distinct from allowing the scriptlet process to call `chroot`.
Target-root build/install flows may still use chroot-style execution for
alternate roots; that is not the protected live-root sandbox boundary.
```

Adjust the wording if the audit proves a different current fact.

### Task 2: Add Pre-Mutation Sandbox Setup And Outcome Typing

**Files:**
- Modify: `crates/conary-core/src/scriptlet/mod.rs`
- Modify: `crates/conary-core/src/container/mod.rs`
- Modify: `apps/conary/src/commands/install/scriptlets.rs`
- Modify: `apps/conary/src/commands/remove.rs`

- [x] **Step 1: Add a protected live-root preflight**

Before package file/DB mutation for packages with runnable live-root scriptlets,
verify that protected sandbox setup can create the required namespaces,
writable layers, and enforcement handles. If preflight fails, abort before
mutating package state.

- [x] **Step 2: Add typed outcomes**

Return or record outcomes equivalent to:

```rust
enum ScriptletOutcome {
    Skipped,
    ScriptExited { code: Option<i32> },
    SandboxSetupUnavailable { message: String },
    EnforcementSetupFailed { message: String },
}
```

Only `ScriptExited` in post phases may degrade to warning-only after package
state changes. Setup and enforcement failures must fail closed.

- [x] **Step 3: Record requested and effective sandbox mode**

Structured metadata must include:

```text
requested_sandbox_mode
effective_sandbox
phase
failure_kind
```

This prevents `--sandbox=auto` direct execution from being confused with
protected sandboxing.

### Task 3: Add Regression Tests For Unsafe Root-Transition Drift

**Files:**
- Modify: `crates/conary-core/src/capability/declaration.rs`
- Modify: `crates/conary-core/src/container/mod.rs`
- Modify if needed: `crates/conary-core/src/scriptlet/mod.rs`

- [x] **Step 1: Add a scriptlet profile test**

Add or keep a unit test equivalent to:

```rust
#[test]
fn scriptlet_profile_does_not_allow_chroot() {
    let profile = SyscallProfile::parse("scriptlet").unwrap();
    assert!(
        !profile.allowed_syscalls().iter().any(|syscall| syscall == "chroot"),
        "protected scriptlets must not regain the classic chroot escape primitive"
    );
}
```

- [x] **Step 2: Assert live-root enforcement matches the docs**

If protected live-root mode claims seccomp enforcement, assert that
`live_sandbox_config()` installs `SyscallCapabilities { profile:
Some("scriptlet"), ... }` or the local equivalent. If it intentionally does not
use seccomp, assert the docs say so explicitly.

- [x] **Step 3: Add a protected-mode setup test**

If the container module exposes enough structure, add a test that asserts
protected live-root setup uses the hardened path and fails closed when the
required namespace operations are unavailable.

- [x] **Step 4: Add readable namespace-preflight diagnostics**

If current errors are generic `Operation not permitted` or low-level namespace
errors, add a focused helper that maps them to the protected-sandbox diagnostic
from Task 1.

- [x] **Step 5: Run focused tests**

Run:

```bash
cargo test -p conary-core capability::declaration
cargo test -p conary-core container
```

Expected: both pass.

### Task 4: Make Direct Scriptlet Degradation Structured

**Files:**
- Modify: `apps/conary/src/commands/changeset_metadata.rs`
- Modify: `apps/conary/src/commands/install/scriptlets.rs`
- Modify: `apps/conary/src/commands/remove.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Test: package-local unit tests or `apps/conary/tests/scriptlet_harness/`

- [x] **Step 1: Add metadata shape**

Extend the existing metadata envelope with entries shaped like:

```json
{
  "kind": "scriptlet_warning",
  "phase": "post-install",
  "package": "example",
  "failure_kind": "ScriptExited",
  "requested_sandbox_mode": "auto",
  "effective_sandbox": "direct",
  "message": "post-install scriptlet failed after package files were installed"
}
```

- [x] **Step 2: Return structured post-scriptlet outcomes**

Change post-install and post-remove helpers so callers can append metadata
when a warning-only scriptlet fails. Keep existing warning output.

- [x] **Step 3: Add regression coverage**

Add tests for post-install, upgrade old post-remove, and remove post-remove.
For each, cover both a script process that exits nonzero and a sandbox setup
failure. Assert:

- package file state is installed if that remains the chosen behavior;
- command output warns only for `ScriptExited` post-phase failures;
- setup/enforcement failures fail closed before mutation;
- history or changeset metadata records `scriptlet_warning`;
- README atomicity wording remains accurate.

- [x] **Step 4: Run focused tests**

Run:

```bash
cargo test -p conary --test scriptlet_harness
cargo test -p conary commands::changeset_metadata
```

Expected: tests pass or the harness target name is adjusted to the existing
scriptlet test target.

### Task 5: Design Capability-Scoped Host Integration

**Files:**
- Modify: `docs/specs/ccs-format-v1.md`
- Modify: `crates/conary-core/src/ccs/manifest.rs`
- Modify: `crates/conary-core/src/capability/declaration.rs`

- [x] **Step 1: Define the minimal capability vocabulary**

Start with only these capabilities:

```toml
[[scriptlets.capabilities]]
name = "systemd-service-registration"
paths = ["/etc/systemd/system"]

[[scriptlets.capabilities]]
name = "tmpfiles-registration"
paths = ["/usr/lib/tmpfiles.d", "/etc/tmpfiles.d"]

[[scriptlets.capabilities]]
name = "dbus-service-registration"
paths = ["/usr/share/dbus-1/system-services", "/etc/dbus-1/system.d"]
```

- [x] **Step 2: Validate unknown capabilities fail closed**

Add manifest parsing tests where an unknown capability produces a clear error:

```text
unknown scriptlet capability 'pam-live-edit'; declare a supported capability or run in a VM until enforcement exists
```

- [x] **Step 3: Fail closed until enforcement exists**

It is acceptable for this task to parse and validate declarations while leaving
enforcement as a follow-up, but install execution must fail closed for packages
declaring unenforced capabilities unless the operator chooses the documented
dangerous direct-execution path. The error should say:

```text
scriptlet capability declarations are present but enforcement is not available;
enable supported capability enforcement or run inside a VM. Dangerous legacy
direct execution requires --sandbox=never plus the live-host mutation
acknowledgement and records effective_sandbox=direct.
```

### Task 6: Update Security Docs And Public Atomicity Claims

**Files:**
- Modify: `docs/SCRIPTLET_SECURITY.md`
- Modify: `README.md`
- Modify: `docs/conaryopedia-v2.md`

- [x] **Step 1: Separate protected, target-root, and direct execution**

Use three distinct headings:

```markdown
### Protected Live-Root Execution
### Target-Root Execution
### Direct Legacy Execution
```

- [x] **Step 2: State the degradation rule**

Add:

```markdown
Post-install and post-remove scriptlets from legacy packages can fail after
package file state has changed only when the sandbox setup succeeded and the
script process itself exited nonzero. Conary records those failures as degraded
scriptlet side effects and surfaces them in history/status. Sandbox setup and
enforcement failures fail closed before mutation.
```

- [x] **Step 3: Run docs truth and audit**

Run:

```bash
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --check
cargo fmt --check
```

Expected: all pass.
