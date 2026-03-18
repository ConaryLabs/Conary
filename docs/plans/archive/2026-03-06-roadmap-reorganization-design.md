# Roadmap Reorganization Design

**Date:** 2026-03-06
**Status:** Approved

## Problem

The current ROADMAP.md is ~610 lines with ~95% of items marked [COMPLETE]. It reads as a historical changelog rather than a forward-looking plan. The project has extensive feature coverage (45 schema migrations, 1,800+ unit tests, 30+ modules across 4 crates) but no automated way to prove features work end-to-end on real systems. Several "incomplete" items in the roadmap are actually done (crate split, renameat2 superseded by composefs, canonical mapping implemented at v45).

## Goal

Replace the current ROADMAP.md with a CI-first, validation-driven, forward-looking roadmap that makes people look at Conary and think "why am I not using this?"

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Completed items | Delete from roadmap | Git history is the record; no separate changelog |
| Structure | 4 phases, validation-driven | CI infrastructure is the multiplier — everything else moves faster once it exists |
| Phase ordering | CI -> Validation -> DX -> Scale | Prove what works before adding features; DX pulls users in, scale grows the ecosystem |
| CI platform | Forgejo on Forge | Self-hosted, Podman-native, matches existing infrastructure |
| Version history table | Remove | Git log serves this purpose |
| Inspiration sources | Remove | Documented in git history |
| Not Planned section | Keep | Useful signal to contributors |

## Codebase Audit: Roadmap vs Reality

Items listed as incomplete that are actually done or superseded:

| Item | Actual Status |
|------|--------------|
| renameat2 RENAME_EXCHANGE | Superseded by composefs (switch.rs line 4) |
| Crate split | Done — 4-crate workspace (conary, conary-core, conary-erofs, conary-server) |
| Cross-distro canonical mapping | Implemented — v45 migration, canonical/ module |
| Schema says v44 | Actually v45 (canonical_packages, package_implementations tables) |

Items genuinely not implemented:

| Item | Status |
|------|--------|
| Shell integration (direnv-style) | No code found |
| Multi-version support (kernels) | No code found |
| Source components (:source troves) | No code found |
| Factory system (templates) | No code found |
| VFS component merging | Migration reference only |
| OS composition (full group model) | Collections exist, not full Foresight-style groups |
| Info/Capsule packages | No code found |
| P2P plugins (IPFS, BitTorrent) | No code found |
| Full repository server (VCS) | No code found |

## New ROADMAP.md Structure

### Phase 1: CI & Validation Infrastructure

The critical path. Everything else depends on this.

**Forgejo Setup (Forge server)**
- Install Forgejo on forge.conarylabs.com
- Mirror GitHub repo to Forgejo
- Set up Forgejo Actions runner (Podman-based)
- Configure push/PR triggers

**Test Container Images**
- Fedora 43 base image (current target)
- Ubuntu 24.04 base image
- Arch Linux base image
- Each image: Conary built from source, test fixtures, system packages

**CI Pipeline**
- `cargo build` + `cargo test` on every push (basic gate)
- `cargo clippy -- -D warnings` lint gate
- Integration test suite trigger (Podman containers on Forge)
- Remi server health check (packages.conary.io endpoints)

**Remi Health Monitoring**
- Endpoint smoke tests (sparse index, chunk fetch, stats, OCI)
- Conversion pipeline test (submit package, poll, verify chunks)
- Scheduled cron job (catch production regressions, not just on push)

### Phase 2: End-to-End Validation

Prove the features work on real systems. Each scenario becomes a CI job.

**Install Flow (per distro: Fedora, Ubuntu, Arch)**
- Adopt existing system packages into Conary DB
- Install a package from Remi (with dependencies)
- Remove a package (verify clean removal + orphan detection)
- Update a package (verify delta application)
- Rollback an operation (verify DB + filesystem revert)
- Pin/unpin a package, verify update skips it

**Generation Lifecycle**
- Build a generation from current state
- Switch to new generation
- Rollback to previous generation
- GC old generations
- System takeover flow (full adopt -> generation)

**Bootstrap Pipeline**
- Dry-run validation passes
- Stage 0 -> Stage 1 toolchain builds
- Base system builds with checkpointing
- Image generation produces bootable output
- (Stretch) Boot the image in QEMU and verify

**Remi Integration**
- Client fetches sparse index from packages.conary.io
- Chunk-level install (client has partial chunks, fetches missing)
- Federation peer discovery and chunk routing
- OCI distribution API serves valid manifests

**Recipe & Build**
- Cook a recipe from TOML, verify CCS output
- PKGBUILD converter produces valid recipe
- Hermetic build isolation works (network blocked)

### Phase 3: Developer Experience

The features that make people switch.

**Seamless Dev Environments**
- Shell integration — auto-activate on `cd` into project dirs (like direnv, but native)
- Multi-version packages — parallel-install kernels, toolchains, runtimes
- `conary use python@3.12` — version-qualified package selection

**Zero-Friction Install**
- First-run experience — `curl | sh` bootstrap on any Linux
- `conary adopt` just works on Fedora/Ubuntu/Arch with no manual steps
- Guided system takeover with rollback safety net

**Composable Systems (Foresight Linux revival)**
- Group packages — `group-desktop`, `group-server`, `group-dev`
- Nested groups with optional members
- `conary migrate group-desktop` — atomic system composition
- Published group definitions on Remi

### Phase 4: Infrastructure & Distribution

**P2P Chunk Distribution**
- IPFS fetcher plugin — check local node before CDN
- BitTorrent DHT for popular chunks
- Transport priority chain (P2P -> CDN -> Mirror)

**Source Repository**
- Source components — :source troves in repository
- Factory system — templates for common package types (library, daemon, CLI tool)
- `conary cook` from remote recipe URLs

### Not Planned

Carried forward from current roadmap:
- rBuilder Integration — proprietary appliance builder
- cvc Tool — replaced by standard git workflows
- Appliance Groups — specific to rPath's appliance model
- GNOME/KDE Package Templates — too specific, general templates sufficient

### Contributing

Priority areas (aligned with phases):
1. Forgejo CI setup and Podman test containers
2. End-to-end test scenarios for install/remove/update flows
3. Shell integration for dev environments
4. Group package system for OS composition
5. P2P chunk distribution plugins
