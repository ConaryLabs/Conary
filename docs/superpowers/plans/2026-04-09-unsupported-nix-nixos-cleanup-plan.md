# Unsupported Nix/NixOS Cleanup Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove unsupported NixOS deployment/support messaging from active docs and site surfaces while preserving honest Conary-vs-Nix/NixOS comparison language and leaving implementation details untouched.

**Architecture:** Execute this cleanup in two passes. First, remove the retired NixOS operator path from `deploy/` by deleting the dead deployment artifact and stripping support-language from the Cloudflare operator guide. Second, review the remaining comparison-oriented docs and site pages, editing only the lines that blur comparison into implied support, then close with a repo-wide grep and site syntax check to prove the remaining `nix`/`nixos` matches are deliberate comparison or implementation-only cases.

**Tech Stack:** Markdown, Svelte, Bash, `rg`, `git ls-files`, `git rm`, `git diff --check`, and `npm --prefix site run check`.

**Commit Convention:** Each commit in this plan should reference `docs/superpowers/plans/2026-04-09-unsupported-nix-nixos-cleanup-plan.md` in the commit body.

---

## Scope Guard

- Remove support, deployment, or integration references to Nix/NixOS from active docs.
- Keep legitimate comparison-only mentions in `README.md`, `docs/conaryopedia-v2.md`, site pages, and CLI help when they explain Conary's model or UX.
- Do not touch `Cargo.toml`, `Cargo.lock`, Rust source that imports the `nix` crate, internal `Nix-style` comments, or tests that prove `nixos` is unsupported.
- Treat `apps/conary/src/cli/ccs.rs` and `apps/conary/src/commands/install/mod.rs` as verification-only unless they drift into implying support.
- For docs with YAML frontmatter, bump `last_updated` and `revision` whenever their content changes.
- Delete `deploy/nixos/remi.nix`; do not archive it or replace it with a new deployment path in this plan.

## File Map

| File | Responsibility |
|------|----------------|
| `deploy/CLOUDFLARE.md` | Active operator doc that currently points at the retired NixOS deployment path and needs support-language cleanup |
| `deploy/nixos/remi.nix` | Dead NixOS Remi deployment artifact that should be deleted from the tracked tree |
| `README.md` | Root product comparison doc; keep Nix comparisons, remove any wording that reads like supported Nix/NixOS operation |
| `docs/conaryopedia-v2.md` | Canonical long-form doc with conceptual Nix/NixOS analogies; keep only comparison-oriented wording |
| `site/src/routes/+page.svelte` | Homepage comparison teaser; verify the Nix column remains comparison-only |
| `site/src/routes/features/+page.svelte` | Features page with `nix shell` analogy; keep only if it remains clearly comparative |
| `site/src/routes/compare/+page.svelte` | Primary comparison page; preserve honest Nix/NixOS comparison sections without implying support |
| `apps/conary/src/cli/ccs.rs` | Verification-only CLI help strings that compare `ccs shell`/`ccs run` to `nix-shell`, `nix develop`, or `nix run` |
| `apps/conary/src/commands/install/mod.rs` | Verification-only negative test proving `nixos` is not a supported distro flavor |
| `Cargo.toml` and `Cargo.lock` | Verification-only evidence that the Rust `nix` dependency remains intentionally untouched |

## Chunk 1: Remove Unsupported Operator Surface

### Task 1: Delete the retired NixOS deploy path and clean the operator guide

**Files:**
- Modify: `deploy/CLOUDFLARE.md`
- Delete: `deploy/nixos/remi.nix`

- [ ] **Step 1: Re-read the current deploy references**

Check `deploy/CLOUDFLARE.md` and `deploy/nixos/remi.nix` together and confirm the exact unsupported breadcrumbs being removed:
- the prerequisite that points operators at `deploy/nixos/remi.nix`
- the NixOS-only `r2.accessKeyFile` / `r2.secretKeyFile` note
- the tracked `deploy/nixos/remi.nix` module itself

- [ ] **Step 2: Edit `deploy/CLOUDFLARE.md` to remove unsupported NixOS guidance**

Requirements:
- remove the `deploy/nixos/remi.nix` prerequisite breadcrumb
- keep the supported/current Remi setup guidance intact
- replace NixOS-specific secret handling text with generic supported guidance only when needed
- do not invent a new deployment path or promise host support the repo does not actually document elsewhere

- [ ] **Step 3: Delete the retired deployment artifact**

Run:

```bash
git rm deploy/nixos/remi.nix
```

- [ ] **Step 4: Verify the operator surface is clean**

Run:

```bash
rg -n -i '\bnixos\b|\bnix\b' deploy/CLOUDFLARE.md deploy/nixos || true
git ls-files deploy/nixos
```

Expected:
- `deploy/CLOUDFLARE.md` no longer contains `nix` or `nixos`
- `git ls-files deploy/nixos` prints nothing

- [ ] **Step 5: Commit**

```bash
git add deploy/CLOUDFLARE.md
git commit -m "docs: remove unsupported nixos deploy path" -m "Part of docs/superpowers/plans/2026-04-09-unsupported-nix-nixos-cleanup-plan.md"
```

## Chunk 2: Preserve Comparison Copy, Remove Support Implications

### Task 2: Review and tighten retained Markdown comparisons

**Files:**
- Modify if needed: `README.md`
- Modify if needed: `docs/conaryopedia-v2.md`

- [ ] **Step 1: Re-read the current comparison mentions**

Review the active Nix/NixOS mentions in:
- `README.md` comparison table, comparison paragraph, and `nix shell` analogy
- `docs/conaryopedia-v2.md` `configuration.nix`, `nix-shell`, `nix run`, and system-model analogies

Classify each match as one of:
- allowed comparison
- misleading support/deployment implication

- [ ] **Step 2: Edit only the misleading lines**

Requirements:
- keep honest comparison tables and conceptual analogies
- remove or rewrite any line that sounds like Conary supports Nix or NixOS as a deployment, runtime, or operator surface
- if a file is already comparison-only, leave it unchanged

- [ ] **Step 3: Verify the remaining Markdown matches are comparison-only**

Run:

```bash
rg -n -i '\bnixos\b|\bnix\b' README.md docs/conaryopedia-v2.md
```

Expected:
- matches may remain
- every remaining match is plainly a feature comparison, conceptual analogy, or UX analogy rather than support guidance

- [ ] **Step 4: Commit if either file changed**

If `README.md` or `docs/conaryopedia-v2.md` changed:

```bash
git add README.md docs/conaryopedia-v2.md
git commit -m "docs: clarify nix comparison copy" -m "Part of docs/superpowers/plans/2026-04-09-unsupported-nix-nixos-cleanup-plan.md"
```

If neither file changed, skip this commit and record that both files were verified as comparison-only.

### Task 3: Review and tighten retained site comparisons

**Files:**
- Modify if needed: `site/src/routes/+page.svelte`
- Modify if needed: `site/src/routes/features/+page.svelte`
- Modify if needed: `site/src/routes/compare/+page.svelte`

- [ ] **Step 1: Re-read the current site copy**

Focus on:
- the homepage comparison teaser table
- the features-page `nix shell` analogy
- the compare page's `vs. Nix` and `vs. NixOS Generations` sections

Allowed outcomes:
- `Nix` comparison columns stay
- `vs. Nix` and `vs. NixOS Generations` sections stay if they remain explicitly comparative
- any operator, deployment, or integration breadcrumb gets removed

- [ ] **Step 2: Edit only copy that blurs comparison into support**

Requirements:
- preserve honest comparison framing
- keep the site metadata aligned with comparison framing
- do not strip legitimate comparison content just because it mentions `Nix` or `NixOS`

- [ ] **Step 3: Run the site-specific verification**

Run:

```bash
rg -n -i '\bnixos\b|\bnix\b' site/src/routes/+page.svelte site/src/routes/features/+page.svelte site/src/routes/compare/+page.svelte
npm --prefix site run check
```

Expected:
- grep output remains limited to comparison-oriented copy
- `npm --prefix site run check` passes

- [ ] **Step 4: Commit if any site file changed**

If any site file changed:

```bash
git add site/src/routes/+page.svelte site/src/routes/features/+page.svelte site/src/routes/compare/+page.svelte
git commit -m "docs(site): clarify nix comparison boundaries" -m "Part of docs/superpowers/plans/2026-04-09-unsupported-nix-nixos-cleanup-plan.md"
```

If no site files changed, skip this commit and record that the pages were verified as comparison-only.

### Task 4: Run the final support-boundary verification sweep

**Files:**
- Verify-only: `deploy/CLOUDFLARE.md`
- Verify-only: `README.md`
- Verify-only: `docs/conaryopedia-v2.md`
- Verify-only: `site/src/routes/+page.svelte`
- Verify-only: `site/src/routes/features/+page.svelte`
- Verify-only: `site/src/routes/compare/+page.svelte`
- Verify-only: `apps/conary/src/cli/ccs.rs`
- Verify-only: `apps/conary/src/commands/install/mod.rs`
- Verify-only: `Cargo.toml`
- Verify-only: `Cargo.lock`

- [ ] **Step 1: Run the final repo-local grep**

Run:

```bash
rg -n -i '\bnixos\b|\bnix\b' README.md docs/conaryopedia-v2.md deploy/CLOUDFLARE.md site/src/routes/+page.svelte site/src/routes/features/+page.svelte site/src/routes/compare/+page.svelte apps/conary/src/cli/ccs.rs apps/conary/src/commands/install/mod.rs Cargo.toml Cargo.lock
git ls-files deploy/nixos
```

Expected:
- `deploy/CLOUDFLARE.md` has no `nix`/`nixos` matches
- `git ls-files deploy/nixos` prints nothing
- remaining matches are limited to:
  - comparison copy in `README.md`, `docs/conaryopedia-v2.md`, and the site pages
  - CLI help analogies in `apps/conary/src/cli/ccs.rs`
  - the negative unsupported test in `apps/conary/src/commands/install/mod.rs`
  - the Rust `nix` dependency entries in `Cargo.toml` and `Cargo.lock`

- [ ] **Step 2: Run the final whitespace and patch sanity checks**

Run:

```bash
git diff --check
git status --short
```

Expected:
- `git diff --check` exits successfully
- `git status --short` shows only the expected modified/deleted tracked files plus any pre-existing untracked local scratch files

- [ ] **Step 3: Create a final cleanup commit if the verification sweep required more edits**

If Task 4 caused additional content edits beyond the earlier commits:

```bash
git add deploy/CLOUDFLARE.md README.md docs/conaryopedia-v2.md site/src/routes/+page.svelte site/src/routes/features/+page.svelte site/src/routes/compare/+page.svelte
git commit -m "docs: finalize unsupported nix cleanup" -m "Part of docs/superpowers/plans/2026-04-09-unsupported-nix-nixos-cleanup-plan.md"
```

If the verification sweep was read-only, do not create an extra commit.
