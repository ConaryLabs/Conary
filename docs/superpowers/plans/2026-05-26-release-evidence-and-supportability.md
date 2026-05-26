# Release Evidence And Supportability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the limited preview supportable: every artifact has a clear evidence/provenance story, every beta report has useful diagnostics, and every tester sees a path to contribute.

**Architecture:** Add a canonical release artifact matrix and checks around it, keep local QEMU/KVM evidence honest while remote validation is paused, add a redacted support-bundle flow, and create contributor-facing beta intake paths without adding heavy governance.

**Tech Stack:** Markdown operations docs, Bash release checks, GitHub issue templates, Rust or Bash diagnostic collection, existing release matrix and docs-audit scripts.

---

## Scope

This plan implements Plan C from
`docs/superpowers/specs/2026-05-26-limited-preview-release-hardening-design.md`.
It does not restore remote KVM by itself and does not add always-on telemetry.
All tester diagnostics are explicit and local-first.

## File Structure

- Create `docs/operations/release-artifact-matrix.md`: product/artifact/SBOM/
  provenance/support-status table.
- Modify `scripts/check-release-matrix.sh` or add
  `scripts/check-release-artifacts.sh`: verify documented artifacts match the
  release matrix.
- Modify `.github/ISSUE_TEMPLATE/bug_report.md` and create
  `.github/ISSUE_TEMPLATE/beta_feedback.md`: capture preview-support context.
- Modify `CONTRIBUTING.md`: add contributor quickstart and validation tasks.
- Add either `scripts/conary-support-bundle.sh` or a focused CLI command in
  `apps/conary/src/cli/system.rs` plus implementation under
  `apps/conary/src/commands/diagnostics.rs`.
- Modify `README.md` and `ROADMAP.md`: link release matrix and support-bundle
  docs.
- Modify docs-audit inventory and ledger files.

## Review-Tightened Decisions

- No automatic telemetry in this plan.
- The support bundle must default to local output and redact paths/secrets that
  are likely to include credentials.
- Remote Forge validation remains explicitly paused until a KVM-capable runner
  exists; do not imply hosted boot evidence is green.
- The contributor funnel should start with validation and docs tasks, not
  kernel/boot/trust code.

---

### Task 1: Release Artifact Matrix

**Files:**
- Create: `docs/operations/release-artifact-matrix.md`
- Modify: `README.md`
- Modify: `ROADMAP.md`

- [ ] **Step 1: Create the matrix doc**

Use this initial table:

```markdown
# Release Artifact Matrix

| Product | Artifact classes | Required evidence | Preview support |
| --- | --- | --- | --- |
| `conary` | binary, `.ccs`, `.rpm`, `.deb`, `.pkg.tar.zst` | checksums, signature, SBOM, release-matrix row, smoke help output | limited preview |
| `remi` | binary/container/deploy bundle | checksums, signature, SBOM, health check, admin-origin config review | service operator preview |
| `conaryd` | binary/package artifacts | checksums, signature, SBOM, Unix-socket auth check, package-job queue smoke | local daemon preview |
| `conary-test` | binary/package artifacts | checksums, signature, SBOM, suite inventory parse, fixture manifest check | validation tooling |
```

- [ ] **Step 2: Add provenance columns**

Include columns for:

```text
release workflow
source commit
SLSA/provenance sidecar
SBOM path
known caveats
```

- [ ] **Step 3: Link it from README and ROADMAP**

Add one sentence near release status:

```markdown
Release artifact and provenance expectations are tracked in
`docs/operations/release-artifact-matrix.md`.
```

### Task 2: Artifact Matrix Check

**Files:**
- Modify: `scripts/check-release-matrix.sh`
- Or create: `scripts/check-release-artifacts.sh`
- Modify: `.github/workflows/pr-gate.yml` only if a new script is created

- [ ] **Step 1: Parse product names from docs**

If creating a new script, start with:

```bash
#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

doc="docs/operations/release-artifact-matrix.md"
[[ -f "$doc" ]] || { echo "missing $doc" >&2; exit 1; }

for product in conary remi conaryd conary-test; do
    rg -q "\`$product\`" "$doc" || {
        echo "ERROR: release artifact matrix missing $product" >&2
        exit 1
    }
done
```

- [ ] **Step 2: Cross-check release matrix entries**

Read the existing release matrix source used by `scripts/check-release-matrix.sh`
and assert every documented product still exists there.

- [ ] **Step 3: Add the check to PR gate**

If the new script is separate, add it to the docs/release validation section in
`.github/workflows/pr-gate.yml`.

### Task 3: Support Bundle Flow

**Files:**
- Prefer create: `scripts/conary-support-bundle.sh`
- Or modify: `apps/conary/src/cli/system.rs`
- Or create: `apps/conary/src/commands/diagnostics.rs`
- Modify: `README.md`

- [ ] **Step 1: Start with a script unless a CLI command is already nearby**

Create `scripts/conary-support-bundle.sh` that writes to a local directory:

```bash
target_dir="${1:-target/conary-support-bundle}"
mkdir -p "$target_dir"
```

Collect:

```text
conary --version
conary system adopt --status
conary system generation list
conary system generation pending
conary repo list
uname -a
lsb_release -a or /etc/os-release
```

- [ ] **Step 2: Redact obvious secrets**

Pipe collected text through a redactor that masks:

```text
Authorization: ...
token=...
password=...
secret=...
/home/<user>/.ssh/...
```

- [ ] **Step 3: Add privacy wording**

Document:

```markdown
The support bundle is local-only. Review it before attaching it to an issue.
Do not include `/etc/conary/trust`, private keys, SSH keys, or host-local
credential files.
```

### Task 4: Beta Feedback Intake

**Files:**
- Create: `.github/ISSUE_TEMPLATE/beta_feedback.md`
- Modify: `.github/ISSUE_TEMPLATE/bug_report.md`
- Modify: `CONTRIBUTING.md`

- [ ] **Step 1: Add beta feedback template**

Include fields:

```markdown
## Preview Lane
- [ ] Adoption/unadoption
- [ ] Conary-owned install/remove/update
- [ ] Selected-generation native handoff
- [ ] Generation export
- [ ] Remi conversion
- [ ] conaryd local daemon

## Distro
- Fedora 44
- Ubuntu 26.04 LTS
- Arch

## Safety Context
- VM/snapshot/non-critical host:
- Commands run:
- Support bundle reviewed before attach: yes/no
```

- [ ] **Step 2: Add good first validation tasks**

In `CONTRIBUTING.md`, add a short section listing:

```text
Run a dry-run adoption on a VM.
Run unadopt dry-run/apply and report output clarity.
Try one Conary-owned package install/remove.
Verify docs quickstart on one supported distro.
Improve error messages with tests.
```

### Task 5: Evidence Refresh Checklist

**Files:**
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/operations/release-artifact-matrix.md`

- [ ] **Step 1: Add a release evidence command block**

Add:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p conary-test -- list
bash scripts/check-doc-truth.sh
bash scripts/check-release-matrix.sh
bash scripts/release-cargo-audit.sh
```

- [ ] **Step 2: Keep QEMU evidence dated**

Add a rule:

```markdown
QEMU evidence must include the absolute run date, distro, suite name, and pass
counts. Do not describe local evidence as hosted CI evidence while remote KVM
validation is paused.
```

### Task 6: Verification

**Files:**
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Run checks**

Run:

```bash
bash scripts/check-release-matrix.sh
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --check
cargo fmt --check
```

Expected: all pass.
