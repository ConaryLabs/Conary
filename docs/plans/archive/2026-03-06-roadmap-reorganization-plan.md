# Roadmap Reorganization Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the changelog-style ROADMAP.md with a forward-looking, CI-first 4-phase roadmap and update all cross-references.

**Architecture:** Pure documentation change. Rewrite ROADMAP.md from scratch (design doc has the full content). Update references in CLAUDE.md, README.md, and docs/ARCHITECTURE.md to match new roadmap description.

**Tech Stack:** Markdown only. No code changes.

---

### Task 1: Rewrite ROADMAP.md

**Files:**
- Modify: `ROADMAP.md` (complete rewrite)

**Step 1: Read the current ROADMAP.md and the design doc**

Read both files to have full context:
- `ROADMAP.md` (current, ~610 lines)
- `docs/plans/2026-03-06-roadmap-reorganization-design.md` (approved design)

**Step 2: Write the new ROADMAP.md**

Replace the entire file with this content. The new file should be ~150 lines, structured as:

```markdown
# Conary Roadmap

Conary is a next-generation Linux package manager with content-addressed storage, atomic generations, and chunk-level distribution. This roadmap tracks what we're building next.

For the full feature set already implemented, see git history and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

---

## Phase 1: CI & Validation Infrastructure

Everything else depends on this. We have 1,800+ unit tests and 45 schema migrations, but no automated way to prove features work end-to-end on real systems.

### Forgejo Setup (Forge server)

- [ ] Install Forgejo on forge.conarylabs.com
- [ ] Mirror GitHub repo to Forgejo
- [ ] Set up Forgejo Actions runner (Podman-based)
- [ ] Configure push/PR triggers

### Test Container Images

- [ ] Fedora 43 base image (current target)
- [ ] Ubuntu 24.04 base image
- [ ] Arch Linux base image
- [ ] Each image: Conary built from source, test fixtures, system packages

### CI Pipeline

- [ ] `cargo build` + `cargo test` on every push (basic gate)
- [ ] `cargo clippy -- -D warnings` lint gate
- [ ] Integration test suite trigger (Podman containers on Forge)
- [ ] Remi server health check (packages.conary.io endpoints)

### Remi Health Monitoring

- [ ] Endpoint smoke tests (sparse index, chunk fetch, stats, OCI)
- [ ] Conversion pipeline test (submit package, poll, verify chunks)
- [ ] Scheduled cron job (catch production regressions, not just on push)

---

## Phase 2: End-to-End Validation

Prove the features work on real systems. Each scenario becomes a CI job.

### Install Flow (per distro: Fedora, Ubuntu, Arch)

- [ ] Adopt existing system packages into Conary DB
- [ ] Install a package from Remi (with dependencies)
- [ ] Remove a package (verify clean removal + orphan detection)
- [ ] Update a package (verify delta application)
- [ ] Rollback an operation (verify DB + filesystem revert)
- [ ] Pin/unpin a package, verify update skips it

### Generation Lifecycle

- [ ] Build a generation from current state
- [ ] Switch to new generation
- [ ] Rollback to previous generation
- [ ] GC old generations
- [ ] System takeover flow (full adopt -> generation)

### Bootstrap Pipeline

- [ ] Dry-run validation passes
- [ ] Stage 0 -> Stage 1 toolchain builds
- [ ] Base system builds with checkpointing
- [ ] Image generation produces bootable output
- [ ] (Stretch) Boot the image in QEMU and verify

### Remi Integration

- [ ] Client fetches sparse index from packages.conary.io
- [ ] Chunk-level install (client has partial chunks, fetches missing)
- [ ] Federation peer discovery and chunk routing
- [ ] OCI distribution API serves valid manifests

### Recipe & Build

- [ ] Cook a recipe from TOML, verify CCS output
- [ ] PKGBUILD converter produces valid recipe
- [ ] Hermetic build isolation works (network blocked)

---

## Phase 3: Developer Experience

The features that make people switch.

### Seamless Dev Environments

- [ ] Shell integration -- auto-activate on `cd` into project dirs (like direnv, but native)
- [ ] Multi-version packages -- parallel-install kernels, toolchains, runtimes
- [ ] `conary use python@3.12` -- version-qualified package selection

### Zero-Friction Install

- [ ] First-run experience -- `curl | sh` bootstrap on any Linux
- [ ] `conary adopt` just works on Fedora/Ubuntu/Arch with no manual steps
- [ ] Guided system takeover with rollback safety net

### Composable Systems (Foresight Linux revival)

- [ ] Group packages -- `group-desktop`, `group-server`, `group-dev`
- [ ] Nested groups with optional members
- [ ] `conary migrate group-desktop` -- atomic system composition
- [ ] Published group definitions on Remi

---

## Phase 4: Infrastructure & Distribution

### P2P Chunk Distribution

- [ ] IPFS fetcher plugin -- check local node before CDN
- [ ] BitTorrent DHT for popular chunks
- [ ] Transport priority chain (P2P -> CDN -> Mirror)

### Source Repository

- [ ] Source components -- :source troves in repository
- [ ] Factory system -- templates for common package types (library, daemon, CLI tool)
- [ ] `conary cook` from remote recipe URLs

---

## Not Planned

These features from original Conary are not planned for implementation:

- **rBuilder Integration** -- Proprietary appliance builder
- **cvc Tool** -- Replaced by standard git workflows
- **Appliance Groups** -- Specific to rPath's appliance model
- **GNOME/KDE Package Templates** -- Too specific, general templates sufficient

---

## Contributing

Priority areas (aligned with phases):

1. Forgejo CI setup and Podman test containers
2. End-to-end test scenarios for install/remove/update flows
3. Shell integration for dev environments
4. Group package system for OS composition
5. P2P chunk distribution plugins

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and [CLAUDE.md](CLAUDE.md) for coding conventions.
```

**Step 3: Verify the new file**

Read back `ROADMAP.md` and confirm:
- No [COMPLETE] markers anywhere
- No version history table
- No inspiration sources section
- 4 clear phases
- Not Planned section preserved
- Contributing section updated

**Step 4: Commit**

```bash
git add ROADMAP.md
git commit -m "docs: Rewrite ROADMAP.md as forward-looking 4-phase plan

Remove all completed items (git history is the record).
Organize around: CI infrastructure -> validation -> DX -> scale.
Design: docs/plans/2026-03-06-roadmap-reorganization-design.md"
```

---

### Task 2: Update CLAUDE.md reference

**Files:**
- Modify: `CLAUDE.md:34`

**Step 1: Read CLAUDE.md**

Read the file to confirm current line content.

**Step 2: Update the ROADMAP.md reference**

Line 34 currently says:
```
Database schema is currently **v45** (40+ tables across 45 migrations). See ROADMAP.md for version history.
```

Change to:
```
Database schema is currently **v45** (40+ tables across 45 migrations). See ROADMAP.md for what's next.
```

The schema version (v45) is already correct. Only the description of what ROADMAP.md contains needs updating.

**Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: Update CLAUDE.md roadmap reference"
```

---

### Task 3: Update README.md references

**Files:**
- Modify: `README.md:484,486,490-496,504`

**Step 1: Read README.md lines 480-510**

Read the relevant section.

**Step 2: Update Project Status section**

Line 484, change:
```
**Version 0.1.0** -- Core architecture is complete and tested. The codebase has 100,000+ lines of Rust with 1,800+ tests passing (schema v44). System generations (EROFS + composefs), system takeover, and the bootstrap pipeline are implemented. A production Remi server is running at packages.conary.io.
```

To (fix schema version v44 -> v45):
```
**Version 0.1.0** -- Core architecture is complete and tested. The codebase has 100,000+ lines of Rust with 1,800+ tests passing (schema v45). System generations (EROFS + composefs), system takeover, and the bootstrap pipeline are implemented. A production Remi server is running at packages.conary.io.
```

**Step 3: Update "What's Next" section**

Lines 490-496, replace the old list:
```markdown
## What's Next

- CI & validation infrastructure (Forgejo, Podman test matrix)
- End-to-end testing across Fedora, Ubuntu, Arch
- Shell integration (direnv-style dev environments)
- Composable systems (group packages, OS composition)
- P2P chunk distribution plugins
```

**Step 4: Update Documentation table**

Line 504, change description:
```
| [ROADMAP.md](ROADMAP.md) | Feature status and planned work |
```

To:
```
| [ROADMAP.md](ROADMAP.md) | Forward-looking development roadmap |
```

**Step 5: Commit**

```bash
git add README.md
git commit -m "docs: Update README.md roadmap references, fix schema v44->v45"
```

---

### Task 4: Update docs/ARCHITECTURE.md reference

**Files:**
- Modify: `docs/ARCHITECTURE.md:487`

**Step 1: Read docs/ARCHITECTURE.md line 487**

Confirm current content.

**Step 2: Update the reference**

Change:
```
- [ROADMAP.md](/ROADMAP.md) - Feature status and version history
```

To:
```
- [ROADMAP.md](/ROADMAP.md) - Forward-looking development roadmap
```

**Step 3: Commit**

```bash
git add docs/ARCHITECTURE.md
git commit -m "docs: Update ARCHITECTURE.md roadmap reference"
```

---

### Task 5: Final verification

**Step 1: Search for stale roadmap references**

```bash
grep -rn "version history\|feature status\|schema v44" --include="*.md" .
```

Verify no stale references remain (ignore docs/plans/ design docs which are historical records).

**Step 2: Verify ROADMAP.md is clean**

```bash
grep -c "COMPLETE" ROADMAP.md
```

Expected: 0 matches.

```bash
wc -l ROADMAP.md
```

Expected: ~150 lines (down from ~610).

**Step 3: Run cargo build to ensure no breakage**

```bash
cargo build
```

Expected: success (docs-only change, but verify nothing was accidentally touched).
