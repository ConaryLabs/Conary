# Release Evidence And Supportability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the limited preview supportable: every artifact or source-build
path has a clear evidence/provenance story, every beta report has useful
diagnostics, and every tester sees a path to contribute.

**Architecture:** Add a canonical release artifact matrix and checks around it, keep local QEMU/KVM evidence honest while remote validation is paused, add an allowlist-only support-bundle flow, and create contributor-facing beta intake paths without adding heavy governance.

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
- Modify `.github/workflows/release-build.yml` if a documented binary artifact
  lacks checksum/signature/SBOM/provenance sidecars.
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
- The support bundle must be allowlist-only. Redaction is a backstop for
  allowed command output, not permission to collect raw logs, environment
  dumps, filesystem trees, or arbitrary files.
- Do not include `conary.db` by default. The first support bundle should include
  integrity/status summaries and require an explicit opt-in before copying any
  database file.
- Remote Forge validation remains explicitly paused until a KVM-capable runner
  exists; do not imply hosted boot evidence is green.
- The contributor funnel should start with validation and docs tasks, not
  kernel/boot/trust code.
- A minimum Plan C slice must land before the first public tester post:
  artifact/source expectation matrix, reviewed support-bundle flow, beta issue
  template, and evidence command block. The full matrix can deepen before
  widened beta, but first testers need a support path on day one.

---

### Task 0: Minimum First-Tester Support Slice

**Files:**
- Modify: `docs/operations/release-artifact-matrix.md`
- Modify: `.github/ISSUE_TEMPLATE/bug_report.md`
- Create: `.github/ISSUE_TEMPLATE/beta_feedback.md`
- Create or modify: support-bundle script/command chosen in Task 3
- Modify: `docs/INTEGRATION-TESTING.md`

- [x] **Step 1: Decide binary or source-build first post**

Before the tester post, publish one of these states:

```text
binary path: artifact URL, checksum, signature status, SBOM/provenance status,
known caveats, and verification command

source-build path: exact source commit, build commands, expected clean-VM build
time, supported distros, and known caveats
```

- [x] **Step 2: Land the minimum support loop**

The first tester post must link:

```text
support bundle command
beta feedback template
release/source expectation matrix
evidence command block
```

### Task 1: Release Artifact Matrix

**Files:**
- Create: `docs/operations/release-artifact-matrix.md`
- Modify: `README.md`
- Modify: `ROADMAP.md`

- [x] **Step 1: Create the matrix doc**

Use this initial table:

```markdown
# Release Artifact Matrix

| Product | Artifact classes | Required evidence | Preview support |
| --- | --- | --- | --- |
| `conary` | binary, `.ccs`, `.rpm`, `.deb`, `.pkg.tar.zst`, or source-build fallback | checksums, signature status, SBOM/provenance status, release-matrix row, smoke help output | limited preview |
| `remi` | binary/container/deploy bundle or source-build fallback | checksums, signature status, SBOM/provenance status, health check, admin-origin config review | service operator preview |
| `conaryd` | binary/package artifacts or source-build fallback | checksums, signature status, SBOM/provenance status, Unix-socket auth check, package-job queue smoke | local daemon preview |
| `conary-test` | binary/package artifacts or source-build fallback | checksums, signature status, SBOM/provenance status, suite inventory parse, fixture manifest check | validation tooling |
```

- [x] **Step 2: Add provenance columns**

Include columns for:

```text
release workflow
source commit
binary download URL or package repository URL
SLSA/provenance sidecar
SBOM path
known caveats
source-build fallback and expected build time if no binary is published
```

- [x] **Step 3: Link it from README and ROADMAP**

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

- [x] **Step 1: Parse product names from docs**

If creating a new script, start with product-name checks, then require each row
to carry concrete URLs/paths or an explicit `source-build-only` caveat:

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

- [x] **Step 2: Cross-check release matrix entries and sidecars**

Read the existing release matrix source used by `scripts/check-release-matrix.sh`
and assert every documented product still exists there. For every row that
claims a binary artifact, verify the documented checksum, signature status,
SBOM/provenance status, URL/path, and caveat fields are non-empty. If the
workflow does not produce those sidecars yet, add workflow tasks or mark the row
`source-build-only` until it does.

- [x] **Step 3: Add the check to PR gate**

If the new script is separate, add it to the docs/release validation section in
`.github/workflows/pr-gate.yml`.

- [x] **Step 4: Add workflow follow-up when matrix claims outpace artifacts**

If `.github/workflows/release-build.yml` only signs/checksums a subset of the
matrix, either extend the workflow for the missing products or downgrade those
rows to source-build fallback before the first tester post.

### Task 3: Allowlist-Only Support Bundle Flow

**Files:**
- Prefer create: `scripts/conary-support-bundle.sh`
- Or modify: `apps/conary/src/cli/system.rs`
- Or create: `apps/conary/src/commands/diagnostics.rs`
- Modify: `README.md`

- [x] **Step 1: Start with a script unless a CLI command is already nearby**

Create `scripts/conary-support-bundle.sh` that writes to a local directory:

```bash
target_dir="${1:-target/conary-support-bundle}"
mkdir -p "$target_dir"
```

Collect only these predefined command outputs:

```text
conary --version
conary system adopt --status
conary system generation list
conary system generation pending
conary repo list
uname -a
lsb_release -a or /etc/os-release
```

Do not collect raw filesystem logs, process environments, shell history,
package payloads, arbitrary `/etc` files, home-directory paths, or recursive
directory listings.

Explicitly exclude by default:

```text
/var/lib/conary/conary.db or any live conary.db path
/etc/conary/trust
private keys
SSH keys
ignored local access docs
full package payloads
```

For database troubleshooting, collect only:

```bash
sqlite3 "$CONARY_DB" "PRAGMA integrity_check;"
sqlite3 "$CONARY_DB" "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name;"
```

Do not copy the DB into the bundle unless a future `--include-db` option is
added with a separate redaction/review warning.

- [x] **Step 2: Use redaction only as a backstop**

Pipe only the allowlisted command output through a redactor that masks:

```text
Authorization: ...
token=...
password=...
secret=...
/home/<user>/.ssh/...
Bearer ...
X-Remi-Admin-Token: ...
https://user:password@example
?access_token=...
```

If the script ever needs a non-allowlisted source, add a new explicit command
entry and test rather than widening the collector.

- [x] **Step 3: Add privacy wording**

Document:

```markdown
The support bundle is local-only. Review it before attaching it to an issue.
Do not include `/etc/conary/trust`, private keys, SSH keys, or host-local
credential files.
```

- [x] **Step 4: Add support-bundle privacy tests**

Add tests or shell fixtures proving:

```text
conary.db is not copied by default
raw logs and environment dumps are not collected
repository URLs with embedded credentials are redacted
the reviewed-before-attach reminder is present
```

### Task 4: Beta Feedback Intake

**Files:**
- Create: `.github/ISSUE_TEMPLATE/beta_feedback.md`
- Modify: `.github/ISSUE_TEMPLATE/bug_report.md`
- Modify: `CONTRIBUTING.md`

- [x] **Step 1: Add beta feedback template**

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

- [x] **Step 2: Replace raw-log bug report guidance**

Update `.github/ISSUE_TEMPLATE/bug_report.md` so it prefers the reviewed support
bundle over pasted raw `RUST_LOG=debug` output. Keep raw debug logs as
maintainer-requested follow-up only, with warnings not to include credentials,
`/etc/conary/trust`, DB files, or unreviewed environment dumps.

- [x] **Step 3: Add good first validation tasks**

In `CONTRIBUTING.md`, add a short section listing concrete entry points:

```text
apps/conary/tests/live_host_mutation_safety.rs: command-risk and dry-run tests
apps/conary/tests/cli_daily_ux.rs: preview output and diagnostic clarity
docs/SCRIPTLET_SECURITY.md: sandbox wording and evidence references
site/src/routes/install/+page.svelte: tester quickstart copy
scripts/check-doc-truth.sh: docs drift checks
docs/modules/source-selection.md: source-policy wording
```

### Task 5: Evidence Refresh Checklist

**Files:**
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/operations/release-artifact-matrix.md`

- [x] **Step 1: Add a release evidence command block**

Add:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p conary-test -- list
bash scripts/check-doc-truth.sh
bash scripts/check-release-matrix.sh
bash scripts/release-cargo-audit.sh
```

- [x] **Step 2: Keep QEMU evidence dated**

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

- [x] **Step 1: Run checks**

Run:

```bash
bash scripts/check-release-matrix.sh
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --check
cargo fmt --check
```

Expected: all pass.
