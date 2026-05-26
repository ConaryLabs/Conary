# Preview Truth And Onboarding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Conary's public preview story truthful, easy to try, and hard to overread.

**Architecture:** Treat docs truth as part of the product surface. Fix stale claims, wire public docs/site/changelog into truth checks, add a safe five-minute adoption path, and either implement true single-package adoption dry-run or make the unsupported route explicit and tested.

**Tech Stack:** Markdown, Svelte site copy, Bash docs-truth scripts, Rust CLI dispatch/tests, existing docs-audit inventory and ledger.

---

## Scope

This plan implements Plan A from
`docs/superpowers/specs/2026-05-26-limited-preview-release-hardening-design.md`.
It does not change scriptlet containment, release artifact generation, or
generation DB recovery except where public docs must stop overclaiming them.

## File Structure

- Modify `CHANGELOG.md`: replace stale conaryd `501 Not Implemented` wording.
- Modify `README.md`: sharpen positioning, atomicity wording, Nix comparison,
  and five-minute preview quickstart.
- Modify `ROADMAP.md`: keep the review-derived queue current after this plan
  lands.
- Modify `site/src/routes/+page.svelte`, `site/src/routes/install/+page.svelte`,
  `site/src/routes/features/+page.svelte`, and
  `site/src/routes/compare/+page.svelte`: align public site copy with the
  adoption-led preview.
- Modify `scripts/check-doc-truth.sh` and `scripts/test-doc-truth.sh`: include
  changelog and site truth checks.
- Modify `apps/conary/src/dispatch.rs`, `apps/conary/src/cli/system.rs`, and
  `apps/conary/tests/live_host_mutation_safety.rs`: implement or deliberately
  route single-package adoption dry-run.
- Modify `docs/modules/source-selection.md` and source-selection tests only if
  this plan changes preview defaults.
- Modify docs-audit inventory and ledger files.

## Review-Tightened Decisions

- The preview headline is "Nix-like safety on the distro you already use,"
  not "universal replacement today."
- Keep build-from-source instructions for developers, but the tester quickstart
  must prefer release artifacts once they exist.
- Atomicity wording must not hide warning-only legacy post-scriptlet behavior.
- If single-package adoption dry-run is not implemented in this pass, the
  refusal text and docs must deliberately route testers to the system-wide
  dry-run path.
- Include `CHANGELOG.md` and public site copy in docs truth checks so this
  exact drift class cannot recur quietly.

---

### Task 1: Baseline Truth Sweep

**Files:**
- Read: `README.md`
- Read: `CHANGELOG.md`
- Read: `docs/modules/conaryd.md`
- Read: `site/src/routes/compare/+page.svelte`
- Read: `apps/conary/src/dispatch.rs`

- [ ] **Step 1: Confirm the stale conaryd claim still exists**

Run:

```bash
rg -n "501 Not Implemented|package install/remove/update" CHANGELOG.md docs/modules/conaryd.md README.md
```

Expected before the fix: `CHANGELOG.md` still contains the stale `501 Not
Implemented` claim, while `docs/modules/conaryd.md` describes queued package
jobs.

- [ ] **Step 2: Confirm the current single-package dry-run route**

Run:

```bash
rg -n "single-package adoption dry-run" apps/conary/src/dispatch.rs apps/conary/src/cli/system.rs
```

Expected before the fix: dispatch rejects single-package adoption dry-run and
CLI help says it is unsupported.

- [ ] **Step 3: Run the existing docs truth gate**

Run:

```bash
bash scripts/check-doc-truth.sh
```

Expected before the fix: pass, proving the stale changelog/site class is not
covered yet.

### Task 2: Fix Public Truth Drift

**Files:**
- Modify: `CHANGELOG.md`
- Modify: `README.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Update the changelog conaryd bullet**

Replace the stale bullet with:

```markdown
- Clarified that conaryd package install/remove/update routes queue daemon
  package jobs while preserving the CLI's explicit live-host mutation
  acknowledgement boundary.
```

- [ ] **Step 2: Revise README atomicity wording**

Replace the absolute "If something fails, your system stays exactly as it was"
claim with wording equivalent to:

```markdown
**Atomic package state and generation selection.** Install, remove, and update
operations commit package DB/file state as changesets, and generation rollback
switches complete system states. Legacy RPM/DEB/Arch post-scriptlets can still
fail after package files are installed or removed; Conary must report those as
degraded side effects rather than pretending they are part of the same rollback
boundary.
```

Keep the tone user-facing, not defensive.

- [ ] **Step 3: Verify the stale text is gone**

Run:

```bash
rg -n "501 Not Implemented|If something fails, your system stays exactly as it was" CHANGELOG.md README.md
```

Expected: no matches in active current-state prose. Historical release entries
may remain only if they describe old releases accurately.

### Task 3: Add The Five-Minute Preview Path

**Files:**
- Modify: `README.md`
- Modify: `site/src/routes/install/+page.svelte`

- [ ] **Step 1: Add a `Five-Minute Preview` subsection**

Add the sequence below before or at the top of the existing quickstart:

```bash
conary system init
conary repo add remi https://remi.conary.io
conary repo sync
conary system adopt --system --dry-run
conary --allow-live-system-mutation system adopt --system --full
conary system adopt --status
conary system unadopt --all --dry-run
conary --allow-live-system-mutation system unadopt --all
```

If release binaries are not yet published for the current tag, keep the build
steps as a developer path but label them that way.

- [ ] **Step 2: Put the escape hatch near the first mutating command**

Add one sentence immediately after the first adoption apply command:

```markdown
Before selecting a Conary generation, `conary --allow-live-system-mutation
system unadopt --all` removes Conary tracking without deleting native package
files.
```

- [ ] **Step 3: Mirror the same flow on the install page**

Update the site install page so the first path is adoption-led and reversible,
not takeover-led and not generation-export-led.

### Task 4: Sharpen The Ecosystem Comparison

**Files:**
- Modify: `README.md`
- Modify: `site/src/routes/compare/+page.svelte`

- [ ] **Step 1: Replace the generic Nix paragraph**

Use this content as the core comparison:

```markdown
If you already run NixOS and like it, Conary is probably not trying to pull
you away. Conary's near-term bet is different: keep Fedora, Ubuntu, or Arch as
the base system, let Conary adopt and CAS-back what is already installed, and
move into Conary-owned generations only when the user explicitly chooses that
authority boundary. The trade-off is maturity and package count; Nix wins
there today. Conary wins only if the migration path is safer and easier to
try.
```

- [ ] **Step 2: Adjust comparison table wording**

Change blunt table labels such as `Atomic transactions` to distinguish:

```text
Package-state transaction boundary
Bootable generation rollback
Native distro adoption/unadoption
```

- [ ] **Step 3: Check site copy for replacement overclaims**

Run:

```bash
rg -n "replace.*(apt|dnf|pacman)|universal package manager|all distros|risk-free" site README.md ROADMAP.md
```

Expected: any remaining "risk-free" wording is specifically tied to adoption
and unadoption, not takeover or generation switching.

### Task 5: Extend Docs Truth Checks

**Files:**
- Modify: `scripts/check-doc-truth.sh`
- Modify: `scripts/test-doc-truth.sh`

- [ ] **Step 1: Add changelog and site paths to the truth scan**

Extend `PRODUCT_DOC_PATHS` in `scripts/check-doc-truth.sh`:

```bash
PRODUCT_DOC_PATHS=(
    "README.md"
    "ROADMAP.md"
    "CHANGELOG.md"
    "docs/ARCHITECTURE.md"
    "docs/conaryopedia-v2.md"
    "docs/modules"
    "docs/operations"
    "site/src/routes"
)
```

- [ ] **Step 2: Add explicit stale conaryd detection**

Add a check that reports an error if `CHANGELOG.md` or active site copy says
conaryd package mutation routes still return blanket `501 Not Implemented`.

- [ ] **Step 3: Update fixture tests**

Add one failing fixture in `scripts/test-doc-truth.sh` where `CHANGELOG.md`
contains:

```markdown
conaryd package install/remove/update routes return `501 Not Implemented`
```

Expected failure text:

```text
claims conaryd package execution is still blanket 501
```

- [ ] **Step 4: Run the truth tests**

Run:

```bash
bash scripts/test-doc-truth.sh
bash scripts/check-doc-truth.sh
```

Expected: both pass.

### Task 6: Decide Single-Package Adoption Dry-Run

**Files:**
- Modify: `apps/conary/src/dispatch.rs`
- Modify: `apps/conary/src/commands/adopt/packages.rs`
- Modify: `apps/conary/tests/live_host_mutation_safety.rs`
- Modify: `README.md`

- [ ] **Step 1: Prefer implementation if package discovery can be reused safely**

If `cmd_adopt_packages` can expose the same package query and CAS/file-count
summary without DB or CAS mutation, add a dry-run branch that prints:

```text
Would adopt package(s): curl
Mode: metadata-only
Native package-manager authority remains unchanged.
No Conary DB, CAS, or host files were modified.
```

- [ ] **Step 2: Add the non-mutating regression test**

Add an integration test that runs:

```bash
conary system adopt curl --dry-run
```

and asserts no trove rows or CAS objects are added.

- [ ] **Step 3: Keep the deliberate refusal if implementation is unsafe**

If the preview cannot be implemented without mutation in this slice, keep the
refusal but update docs and tests so the first-run route says:

```text
Single-package adoption dry-run is not implemented yet; use `conary system
adopt --system --dry-run` for the safe preview path.
```

### Task 7: Revisit Preview Source-Selection Defaults

**Files:**
- Modify: `docs/modules/source-selection.md`
- Modify only if behavior changes: `crates/conary-core/src/repository/effective_policy.rs`
- Modify only if behavior changes: `apps/conary/src/commands/model.rs`
- Test only if behavior changes: `cargo test -p conary-core repository::effective_policy`

- [ ] **Step 1: Confirm current defaults**

Run:

```bash
rg -n "latest-anywhere|SelectionMode::Latest|SelectionMode::Policy|balanced" docs/modules/source-selection.md crates/conary-core/src/repository apps/conary/src/commands/model.rs
```

- [ ] **Step 2: Choose a preview-safe default**

For the limited preview, prefer native/source affinity unless the user opts
into newest-anywhere behavior. If code already behaves this way, update docs
to say so. If code defaults to `latest-anywhere`, change the preview preset to
policy/native-affinity and require explicit opt-in for cross-source latest.

- [ ] **Step 3: Add or update tests**

Add a test proving default update selection does not switch from a package's
current native source to another distro source without explicit policy.

### Task 8: Docs-Audit And Verification

**Files:**
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Regenerate inventory after staging new tracked docs**

Run:

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] **Step 2: Add ledger rows for changed planning docs**

Every new or edited active planning doc must have one ledger row with
`status=verified` and a concrete disposition.

- [ ] **Step 3: Run verification**

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-doc-truth.sh
git diff --check
cargo fmt --check
```

Expected: all pass.
