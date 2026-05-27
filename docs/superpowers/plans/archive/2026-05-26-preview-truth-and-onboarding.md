# Preview Truth And Onboarding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Conary's public preview story truthful, easy to try, and hard to overread.

**Architecture:** Treat docs truth as part of the product surface. Fix stale
claims, wire public docs/site/changelog into truth checks, add a bounded
five-minute adoption path, explain the live-mutation acknowledgement boundary,
account for Remi cold-start latency, and either implement true single-package
adoption dry-run or make the unsupported route explicit and tested.

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
  Remi cold-start caveat, live-mutation acknowledgement explanation, and
  five-minute preview quickstart.
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
  Before Plan B structured metadata lands, wording must describe current
  warning-only behavior plainly instead of promising future history/status
  fields.
- If single-package adoption dry-run is not implemented in this pass, the
  refusal text and docs must deliberately route testers to the system-wide
  dry-run path.
- Remi cold-start conversion latency is part of tester onboarding, not a hidden
  service detail.
- Source-selection defaults should be documented before behavior changes are
  considered.
- Include `CHANGELOG.md` and all public site routes in docs truth checks so this
  exact drift class cannot recur quietly. The baseline sweep must include stale
  schema/version claims, not only the compare page.

---

### Task 1: Baseline Truth Sweep

**Files:**
- Read: `README.md`
- Read: `CHANGELOG.md`
- Read: `docs/modules/conaryd.md`
- Read: `site/src/routes`
- Read: `apps/conary/src/dispatch.rs`

- [x] **Step 1: Confirm the conaryd route claim is not stale**

Run:

```bash
rg -n "501 Not Implemented|package install/remove/update" CHANGELOG.md docs/modules/conaryd.md README.md
```

Expected: current-state docs and changelog language agree that conaryd package
routes queue package jobs rather than returning blanket `501 Not Implemented`.
If the stale claim remains, fix it in Task 2.

- [x] **Step 2: Confirm the current single-package dry-run route**

Run:

```bash
rg -n "single-package adoption dry-run" apps/conary/src/dispatch.rs apps/conary/src/cli/system.rs
```

Expected before the fix: dispatch rejects single-package adoption dry-run and
CLI help says it is unsupported.

- [x] **Step 3: Run the existing docs truth gate**

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

- [x] **Step 1: Ensure the changelog conaryd bullet is current**

If the stale bullet remains, replace it with:

```markdown
- Clarified that conaryd package install/remove/update routes queue daemon
  package jobs while preserving the CLI's explicit live-host mutation
  acknowledgement boundary.
```

- [x] **Step 2: Revise README atomicity wording**

Replace the absolute "If something fails, your system stays exactly as it was"
claim with wording equivalent to:

```markdown
**Atomic package state and generation selection.** Install, remove, and update
operations commit package DB/file state as changesets, and generation rollback
switches complete system states. Legacy RPM/DEB/Arch post-scriptlets can still
fail after package files are installed or removed. Until the scriptlet trust
plan lands structured degradation metadata, treat those as warning-only
post-scriptlet side effects rather than part of the same rollback boundary.
```

Keep the tone user-facing, not defensive.

- [x] **Step 3: Verify the stale text is gone**

Run:

```bash
rg -n "501 Not Implemented|If something fails, your system stays exactly as it was" CHANGELOG.md README.md
```

Expected: no matches in active current-state prose. Historical release entries
may remain only if they describe old releases accurately.

### Task 3: Add The Bounded Five-Minute Preview Path

**Files:**
- Modify: `README.md`
- Modify: `site/src/routes/install/+page.svelte`

- [x] **Step 1: Add a `Five-Minute Preview` subsection**

Add the sequence below before or at the top of the existing quickstart. The
default path must be safe to complete in one sitting on a VM or non-critical
host; do not call full-system CAS-backed adoption "five minute" unless the
implementation PR records timing and disk bounds that prove it.

```bash
conary system init
conary repo add remi https://remi.conary.io
conary repo sync
conary system adopt --system --dry-run
conary system adopt --status
```

If the quickstart includes an apply step, prefer a measured metadata-only or
scoped adoption path first:

```bash
conary --allow-live-system-mutation system adopt --system
conary system adopt --status
conary system unadopt --all --dry-run
conary --allow-live-system-mutation system unadopt --all
```

If the chosen apply path uses `--full`, record clean-VM timing, package count,
and disk growth in the implementation PR and state the expectation in the
quickstart.

If release binaries are not yet published for the current tag, keep the build
steps as a developer path but label them that way.

- [x] **Step 2: Decide binary-vs-source tester expectations**

Before publishing the tester post, choose one of these explicit paths:

```text
release-binary path: publish and link a binary/package artifact for each
supported preview distro, with the minimum artifact/provenance matrix from
Plan C already published

source-build path: time the build-from-source path on a clean VM and state the
expected compile time honestly in the quickstart
```

- [x] **Step 3: Explain the live-mutation acknowledgement flag at first use**

Add one sentence near the first command that uses the flag:

```markdown
`--allow-live-system-mutation` is intentionally long: it marks the exact point
where the preview moves from inspection into changing the active host.
```

Do not add a shorter alias in this task unless the CLI review chooses one
explicitly and adds matching command-risk tests.

- [x] **Step 4: Put the escape hatch near the first mutating command**

Add one sentence immediately after the first adoption apply command:

```markdown
Before selecting a Conary generation, `conary --allow-live-system-mutation
system unadopt --all` removes Conary tracking without deleting native package
files.
```

- [x] **Step 5: Mirror the same flow on the install page**

Update the site install page so the first path is adoption-led and reversible,
not takeover-led and not generation-export-led.

### Task 4: Account For Remi Cold-Start Latency

**Files:**
- Modify: `README.md`
- Modify: `site/src/routes/install/+page.svelte`
- Modify or create only if chosen: `scripts/remi-prewarm-preview.sh`

- [x] **Step 1: Time the cold path**

On a clean VM or clean Remi cache, time the first supported install path:

```bash
time conary install nginx --dry-run
```

Record the distro, package, cache state, and elapsed time in the implementation
PR.

- [x] **Step 2: Choose the preview mitigation**

Pick one. Caveat-only is acceptable only if the measured cold path is below the
threshold chosen in the implementation PR; use 30 seconds as the starting
threshold unless the release notes justify another number.

```text
documented caveat: quickstart says first use may spend time converting legacy
packages through Remi, allowed only below the threshold

pre-warm command: add a script or command that warms a small package set before
the tester tries install/remove flows

server pre-conversion: pre-convert a top package set for Fedora 44, Ubuntu
26.04 LTS, and Arch before the tester post
```

- [x] **Step 3: Keep the scope small**

The initial warm set should be no larger than packages used in the quickstart
and integration smoke paths, for example:

```text
nginx
curl
openssl
sqlite
```

### Task 5: Sharpen The Ecosystem Comparison

**Files:**
- Modify: `README.md`
- Modify: `site/src/routes/compare/+page.svelte`

- [x] **Step 1: Replace the generic Nix paragraph**

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

- [x] **Step 2: Adjust comparison table wording**

Change blunt table labels such as `Atomic transactions` to distinguish:

```text
Package-state transaction boundary
Bootable generation rollback
Native distro adoption/unadoption
```

- [x] **Step 3: Check site copy for replacement overclaims**

Run:

```bash
rg -n "replace.*(apt|dnf|pacman)|universal package manager|all distros|risk-free" site README.md ROADMAP.md
```

Expected: any remaining "risk-free" wording is specifically tied to adoption
and unadoption, not takeover or generation switching.

### Task 6: Extend Docs Truth Checks

**Files:**
- Modify: `scripts/check-doc-truth.sh`
- Modify: `scripts/test-doc-truth.sh`

- [x] **Step 1: Add changelog and all site paths to the truth scan**

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

- [x] **Step 2: Add explicit stale conaryd detection**

Add a check that reports an error if `CHANGELOG.md` or active site copy says
conaryd package mutation routes still return blanket `501 Not Implemented`.

- [x] **Step 3: Update fixture tests**

Add one failing fixture in `scripts/test-doc-truth.sh` where `CHANGELOG.md`
contains:

```markdown
conaryd package install/remove/update routes return `501 Not Implemented`
```

Expected failure text:

```text
claims conaryd package execution is still blanket 501
```

- [x] **Step 4: Add stale site/status detection**

Add negative fixtures and real checks for stale site claims found in the
baseline sweep, including:

```text
schema version older than crates/conary-core/src/db/schema.rs
every install builds EROFS
under a minute
atomically absorbs/takes over native packages without explicit takeover
```

- [x] **Step 5: Run the truth tests**

Run:

```bash
bash scripts/test-doc-truth.sh
bash scripts/check-doc-truth.sh
```

Expected: both pass.

### Task 7: Decide Single-Package Adoption Dry-Run

**Files:**
- Modify: `apps/conary/src/dispatch.rs`
- Modify: `apps/conary/src/commands/adopt/packages.rs`
- Modify: `apps/conary/tests/live_host_mutation_safety.rs`
- Modify: `README.md`

- [x] **Step 1: Prefer implementation if package discovery can be reused safely**

If `cmd_adopt_packages` can expose the same package query and CAS/file-count
summary without DB or CAS mutation, add a dry-run branch that prints:

```text
Would adopt package(s): curl
Mode: metadata-only
Native package-manager authority remains unchanged.
No Conary DB, CAS, or host files were modified.
```

- [x] **Step 2: Add the non-mutating regression test**

Add an integration test that runs:

```bash
conary system adopt curl --dry-run
```

and asserts no trove rows or CAS objects are added.

- [x] **Step 3: Keep the deliberate refusal if implementation is unsafe**

If the preview cannot be implemented without mutation in this slice, keep the
refusal but update docs and tests so the first-run route says:

```text
Single-package adoption dry-run is not implemented yet; use `conary system
adopt --system --dry-run` for the safe preview path.
```

### Task 8: Document Preview Source-Selection Defaults

**Files:**
- Modify: `docs/modules/source-selection.md`
- Read: `crates/conary-core/src/repository/effective_policy.rs`
- Read: `apps/conary/src/commands/model.rs`
- Test only if a harmful mismatch is found: `cargo test -p conary-core repository::effective_policy`

- [x] **Step 1: Confirm current defaults**

Run:

```bash
rg -n "latest-anywhere|SelectionMode::Latest|SelectionMode::Policy|balanced" docs/modules/source-selection.md crates/conary-core/src/repository apps/conary/src/commands/model.rs
```

- [x] **Step 2: Document the current dual default before changing behavior**

If the current behavior is:

```text
model-backed configuration defaults to balanced/latest-anywhere
runtime policy falls back to policy/native-affinity
```

then document that clearly and do not change behavior in this plan. Only change
defaults if the code contradicts docs or current behavior actively undermines
the adoption-led preview.

- [x] **Step 3: Add or update tests**

If behavior changes, add a test proving default update selection does not
switch from a package's current native source to another distro source without
explicit policy.

### Task 9: Audit First-Run `system init` Failure Modes

**Files:**
- Modify: `apps/conary/src/commands/system/init.rs` or the current init module
- Modify: `README.md`
- Test: focused CLI or command unit tests

- [x] **Step 1: Locate init implementation and tests**

Run:

```bash
rg -n "system init|cmd_.*init|composefs|disk space|/conary" apps/conary/src apps/conary/tests crates/conary-core/src
```

- [x] **Step 2: Check common first-run failures**

Verify error messages for:

```text
/conary already exists from a prior attempt
insufficient disk space for CAS/generation work
missing composefs or kernel support
permission denied creating /conary
```

- [x] **Step 3: Add targeted improvements**

Each failure should name the failing path or requirement and the safest next
action. Avoid suggesting destructive cleanup unless the command can prove the
state is disposable.

### Task 10: Docs-Audit And Verification

**Files:**
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [x] **Step 1: Regenerate inventory after staging new tracked docs**

Run:

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [x] **Step 2: Add ledger rows for changed planning docs**

Every new or edited active planning doc must have one ledger row with
`status=verified` and a concrete disposition.

- [x] **Step 3: Run verification**

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-doc-truth.sh
git diff --check
cargo fmt --check
```

Expected: all pass.
