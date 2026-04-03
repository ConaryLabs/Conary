# Remi Canonical Hostname Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `https://remi.conary.io` the canonical public package-service hostname across active code, docs, CI, and frontend links while leaving `https://packages.conary.io` operational as a compatibility alias outside the repository.

**Architecture:** Treat this as one atomic default-alignment sweep rather than a staged refactor. Update runtime defaults and their tests first, then align CI verification and test-harness configuration, then rewrite active docs and frontend copy to teach only the new canonical host. Do not change the current two-site deployment shape and do not delete historical archive references.

**Tech Stack:** Rust 2024 workspace, GitHub Actions YAML, SvelteKit static frontends, shell deploy scripts, Markdown docs, ripgrep-based verification.

---

## Preconditions

- The approved spec is
  `docs/superpowers/specs/2026-04-02-remi-canonical-hostname-design.md`.
- `packages.conary.io` must remain live operationally as a compatibility alias
  outside the repo during this rollout.
- Archive docs and generated build outputs are out of scope for content edits.

## File Map

- Modify: `crates/conary-core/src/self_update.rs`
- Modify: `crates/conary-core/src/repository/sync.rs`
- Modify: `crates/conary-core/src/derivation/manifest.rs`
- Modify: `crates/conary-core/src/derivation/substituter.rs`
- Modify: `apps/conary/src/commands/system.rs`
- Modify: `apps/conary/src/commands/self_update.rs`
- Modify: `apps/conary/src/commands/bootstrap/mod.rs`
- Modify: `apps/conary-test/src/config/mod.rs`
- Modify: `apps/conary-test/src/engine/qemu.rs`
- Modify: `apps/conary-test/src/engine/runner.rs`
- Modify: `apps/conary-test/src/engine/variables.rs`
- Modify: `apps/conary/tests/integration/remi/config.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase2-group-f.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase4-group-e.toml`
- Modify: `.github/workflows/deploy-and-verify.yml`
- Modify: `README.md`
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/operations/infrastructure.md`
- Modify: `docs/conaryopedia-v2.md`
- Modify: `deploy/CLOUDFLARE.md`
- Modify: `deploy/deploy-sites.sh`
- Modify: `deploy/remi.toml.example`
- Modify: `site/README.md`
- Modify: `web/README.md`
- Modify: `site/src/routes/+layout.svelte`
- Modify: `site/src/routes/+page.svelte`
- Modify: `site/src/routes/install/+page.svelte`

## Chunk 1: Runtime Defaults And Tests

### Task 1: Replace canonical host defaults in runtime code

**Files:**
- Modify: `crates/conary-core/src/self_update.rs`
- Modify: `crates/conary-core/src/repository/sync.rs`
- Modify: `crates/conary-core/src/derivation/manifest.rs`
- Modify: `crates/conary-core/src/derivation/substituter.rs`
- Modify: `apps/conary/src/commands/system.rs`
- Modify: `apps/conary/src/commands/self_update.rs`
- Modify: `apps/conary/src/commands/bootstrap/mod.rs`
- Test: `apps/conary/src/commands/system.rs`
- Test: `crates/conary-core/src/self_update.rs`

- [ ] **Step 1: Capture the current runtime references**

Run:

```bash
rg -n 'packages\.conary\.io' \
  crates/conary-core/src/self_update.rs \
  crates/conary-core/src/repository/sync.rs \
  crates/conary-core/src/derivation/manifest.rs \
  crates/conary-core/src/derivation/substituter.rs \
  apps/conary/src/commands/system.rs \
  apps/conary/src/commands/self_update.rs \
  apps/conary/src/commands/bootstrap/mod.rs
```

Expected: the grep shows the old hostname embedded in default URLs, examples, or
assertions.

- [ ] **Step 2: Replace the canonical runtime defaults**

Edit each active public package-service URL so it uses
`https://remi.conary.io`, including:

- self-update channel defaults
- default Remi repository initialization
- bootstrap and repository-sync examples or defaults
- public seed URLs in derivation manifests
- comments that teach the old host as canonical

Leave any compatibility logic alone unless the old hostname is actively encoded
as the default.

- [ ] **Step 3: Update the colocated assertions**

Where tests or examples assert the exact default host, update them to
`remi.conary.io` so they match the new canonical default.

- [ ] **Step 4: Run focused runtime checks**

Run:

```bash
cargo test -p conary default_update_channel -- --nocapture
cargo test -p conary cmd_show_update_channel -- --nocapture
cargo test -p conary-core self_update -- --nocapture
```

Expected:

- tests covering default update-channel behavior pass
- no failing assertions still expect `packages.conary.io`

## Chunk 2: Test Harness And CI Verification

### Task 2: Update test-harness defaults and integration expectations

**Files:**
- Modify: `apps/conary-test/src/config/mod.rs`
- Modify: `apps/conary-test/src/engine/qemu.rs`
- Modify: `apps/conary-test/src/engine/runner.rs`
- Modify: `apps/conary-test/src/engine/variables.rs`
- Modify: `apps/conary/tests/integration/remi/config.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase2-group-f.toml`
- Modify: `apps/conary/tests/integration/remi/manifests/phase4-group-e.toml`
- Test: `apps/conary-test/src/config/mod.rs`
- Test: `apps/conary-test/src/engine/variables.rs`

- [ ] **Step 1: Capture the old harness defaults**

Run:

```bash
rg -n 'packages\.conary\.io' \
  apps/conary-test/src/config/mod.rs \
  apps/conary-test/src/engine/qemu.rs \
  apps/conary-test/src/engine/runner.rs \
  apps/conary-test/src/engine/variables.rs \
  apps/conary/tests/integration/remi/config.toml \
  apps/conary/tests/integration/remi/manifests/phase2-group-f.toml \
  apps/conary/tests/integration/remi/manifests/phase4-group-e.toml
```

Expected: the grep shows the old hostname in defaults, environment expansion
tests, and integration manifest expectations.

- [ ] **Step 2: Update harness defaults and manifest expectations**

Replace the old hostname with `remi.conary.io` in:

- default `[remi].endpoint` config
- QEMU artifact base URL
- environment-variable and runner assertions
- manifest text that validates the default update channel or repo-add output

- [ ] **Step 3: Align deploy verification**

Edit `.github/workflows/deploy-and-verify.yml` so the post-deploy checks use
`https://remi.conary.io` for:

- self-update latest version verification
- health endpoint verification

- [ ] **Step 4: Run focused green checks**

Run:

```bash
cargo test -p conary-test config -- --nocapture
cargo test -p conary-test variables -- --nocapture
cargo test -p conary-test runner -- --nocapture
```

Expected: the harness tests pass and no active test still encodes the old host
as the default.

## Chunk 3: Docs, Frontends, And Deploy Notes

### Task 3: Rewrite active docs and site links to the canonical host

**Files:**
- Modify: `README.md`
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/operations/infrastructure.md`
- Modify: `docs/conaryopedia-v2.md`
- Modify: `deploy/CLOUDFLARE.md`
- Modify: `deploy/deploy-sites.sh`
- Modify: `deploy/remi.toml.example`
- Modify: `site/README.md`
- Modify: `web/README.md`
- Modify: `site/src/routes/+layout.svelte`
- Modify: `site/src/routes/+page.svelte`
- Modify: `site/src/routes/install/+page.svelte`

- [ ] **Step 1: Sweep active docs and frontend copy**

Run:

```bash
rg -n --glob '!**/archive/**' --glob '!site/build/**' --glob '!web/build/**' \
  'packages\.conary\.io' \
  README.md docs site web deploy .github
```

Expected: the grep shows all active user-facing references that still teach the
old host.

- [ ] **Step 2: Rewrite canonical references**

Update all active public-host references to `remi.conary.io`, including:

- README links and examples
- infrastructure and integration docs
- Cloudflare and deploy notes
- `site/` package links and install examples
- `web/README.md` naming and deployment notes

If a compatibility note is still useful, phrase it explicitly as "legacy alias"
rather than as the primary hostname.

- [ ] **Step 3: Run frontend and text verification**

Run:

```bash
npm run build
```

from:

- `site/`
- `web/`

Then run:

```bash
git diff --check
rg -n --glob '!**/archive/**' --glob '!site/build/**' --glob '!web/build/**' \
  'packages\.conary\.io' \
  README.md docs site web deploy .github apps crates
```

Expected:

- both frontend builds pass
- `git diff --check` exits 0
- any remaining active `packages.conary.io` references are deliberate
  compatibility mentions only, or none remain

## Completion Notes

- Do not edit archive materials just to erase historical references.
- Do not change the current split deploy roots for `conary.io` and the package
  frontend in this slice.
- If a direct `:8082` admin URL remains in tracked docs, it should use
  `remi.conary.io` only when the doc is still meant to describe the preferred
  operator-facing hostname.
