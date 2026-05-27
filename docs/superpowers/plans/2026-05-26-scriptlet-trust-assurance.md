# Scriptlet Trust Assurance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Conary's scriptlet trust story precise, test-backed, and visible to operators before the preview widens.

**Architecture:** Start with an assurance audit of protected live-root execution, then either update stale docs or harden the implementation. Add regression tests around unsafe root-transition capabilities, record legacy direct-scriptlet degradation structurally, and design a narrow capability model for packages that need real host integration.

**Tech Stack:** Rust scriptlet/container code, seccomp capability declarations, rusqlite changeset metadata, Markdown security docs, focused cargo tests.

---

## Scope

This plan implements Plan B from
`docs/superpowers/specs/2026-05-26-limited-preview-release-hardening-design.md`.
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
  outcomes if needed.
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
  after package file state changes, but they must be visible as degraded side
  effects in history/status.
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

- [ ] **Step 1: Locate live-root protected execution**

Run:

```bash
rg -n "execute_sandbox_live|pivot_root|chroot|SandboxMode|namespace|seccomp" crates/conary-core/src/container/mod.rs crates/conary-core/src/scriptlet/mod.rs crates/conary-core/src/capability/declaration.rs
```

- [ ] **Step 2: Classify the actual root-transition mechanism**

Record the answer in the implementation PR description:

```text
protected live-root root transition: pivot_root | chroot | other
target-root/offline install transition: chroot | other
scriptlet syscall profile allows chroot: yes | no
```

If the profile allows `chroot`, stop and either remove it from protected
scriptlet execution or update this plan with the exact compatibility reason.

- [ ] **Step 3: Classify hardened-kernel and container failure modes**

Run the sandbox preflight or focused sandbox test in environments where
namespace creation can be restricted:

```text
standard local host
rootless or restricted container
hardened kernel with unprivileged user namespaces disabled
```

The implementation must turn namespace setup failures into one clear error
that names the missing capability and the choices:

```text
Protected scriptlet sandboxing requires mount and user namespace support.
Enable the required kernel/container namespace support, run inside a VM, or
rerun with --sandbox=never only if you intentionally accept direct host
mutation.
```

- [ ] **Step 4: Add an assurance note to the security doc**

Add a short section:

```markdown
## Assurance Notes

Protected live-root scriptlets do not receive the `chroot` syscall in the
scriptlet seccomp profile. Target-root build/install flows may still use
chroot-style execution for alternate roots; that is not the protected live-root
sandbox boundary.
```

Adjust the wording if the audit proves a different current fact.

### Task 2: Add Regression Tests For Unsafe Root-Transition Drift

**Files:**
- Modify: `crates/conary-core/src/capability/declaration.rs`
- Modify: `crates/conary-core/src/container/mod.rs`
- Modify if needed: `crates/conary-core/src/scriptlet/mod.rs`

- [ ] **Step 1: Add a scriptlet profile test**

Add or keep a unit test equivalent to:

```rust
#[test]
fn scriptlet_profile_does_not_allow_chroot() {
    let profile = SyscallProfile::parse("scriptlet").unwrap();
    assert!(
        !profile.syscalls.iter().any(|syscall| syscall == "chroot"),
        "protected scriptlets must not regain the classic chroot escape primitive"
    );
}
```

- [ ] **Step 2: Add a protected-mode setup test**

If the container module exposes enough structure, add a test that asserts
protected live-root setup uses the hardened path and fails closed when the
required namespace operations are unavailable.

- [ ] **Step 3: Add readable namespace-preflight diagnostics**

If current errors are generic `Operation not permitted` or low-level namespace
errors, add a focused helper that maps them to the protected-sandbox diagnostic
from Task 1.

- [ ] **Step 4: Run focused tests**

Run:

```bash
cargo test -p conary-core capability::declaration
cargo test -p conary-core container
```

Expected: both pass.

### Task 3: Make Direct Scriptlet Degradation Structured

**Files:**
- Modify: `apps/conary/src/commands/changeset_metadata.rs`
- Modify: `apps/conary/src/commands/install/scriptlets.rs`
- Modify: `apps/conary/src/commands/remove.rs`
- Modify: `apps/conary/src/commands/install/mod.rs`
- Test: package-local unit tests or `apps/conary/tests/scriptlet_harness/`

- [ ] **Step 1: Add metadata shape**

Extend the existing metadata envelope with entries shaped like:

```json
{
  "kind": "scriptlet_warning",
  "phase": "post-install",
  "package": "example",
  "sandbox_mode": "never",
  "message": "post-install scriptlet failed after package files were installed"
}
```

- [ ] **Step 2: Return structured post-scriptlet outcomes**

Change post-install and post-remove helpers so callers can append metadata
when a warning-only scriptlet fails. Keep existing warning output.

- [ ] **Step 3: Add regression coverage**

Add a test with a package whose post-install scriptlet exits nonzero. Assert:

- package file state is installed if that remains the chosen behavior;
- command output warns;
- history or changeset metadata records `scriptlet_warning`;
- README atomicity wording remains accurate.

- [ ] **Step 4: Run focused tests**

Run:

```bash
cargo test -p conary --test scriptlet_harness
cargo test -p conary commands::changeset_metadata
```

Expected: tests pass or the harness target name is adjusted to the existing
scriptlet test target.

### Task 4: Design Capability-Scoped Host Integration

**Files:**
- Modify: `docs/specs/ccs-format-v1.md`
- Modify: `crates/conary-core/src/ccs/manifest.rs`
- Modify: `crates/conary-core/src/capability/declaration.rs`

- [ ] **Step 1: Define the minimal capability vocabulary**

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

- [ ] **Step 2: Validate unknown capabilities fail closed**

Add manifest parsing tests where an unknown capability produces a clear error:

```text
unknown scriptlet capability 'pam-live-edit'; declare a supported capability or run with --sandbox=never explicitly
```

- [ ] **Step 3: Fail closed until enforcement exists**

It is acceptable for this task to parse and validate declarations while
leaving enforcement as a follow-up, but install execution must fail closed for
packages declaring unenforced capabilities unless the operator uses
`--sandbox=never` explicitly. The error should say:

```text
scriptlet capability declarations are present but enforcement is not available;
rerun with --sandbox=never only if you intentionally accept direct host mutation
```

### Task 5: Update Security Docs And Public Atomicity Claims

**Files:**
- Modify: `docs/SCRIPTLET_SECURITY.md`
- Modify: `README.md`
- Modify: `docs/conaryopedia-v2.md`

- [ ] **Step 1: Separate protected, target-root, and direct execution**

Use three distinct headings:

```markdown
### Protected Live-Root Execution
### Target-Root Execution
### Direct Legacy Execution
```

- [ ] **Step 2: State the degradation rule**

Add:

```markdown
Post-install and post-remove scriptlets from legacy packages can fail after
package file state has changed. Conary records those failures as degraded
scriptlet side effects and surfaces them in history/status; use
`--sandbox=never` only when you intentionally accept direct host mutation.
```

- [ ] **Step 3: Run docs truth and audit**

Run:

```bash
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --check
cargo fmt --check
```

Expected: all pass.
