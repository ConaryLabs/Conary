# Conary Roadmap

Conary is a next-generation Linux package manager with content-addressed storage, atomic generations, and chunk-level distribution. This roadmap tracks what we're building next.

For the full feature set already implemented, see git history and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

---

## Phase 1: CI & Validation Infrastructure [COMPLETE]

Everything else depends on this.

### Forgejo Setup (Forge server)

- [x] Install Forgejo on forge.conarylabs.com
- [x] Mirror GitHub repo to Forgejo
- [x] Set up Forgejo Actions runner (host-based, linux-native label)
- [x] Configure push/PR triggers

### Test Container Images

- [x] Fedora 43 base image (current target)
- [x] Ubuntu 24.04 base image
- [x] Arch Linux base image
- [x] Each image: Conary built from source, test fixtures, system packages

### CI Pipeline

- [x] `cargo build` + `cargo test` on every push (basic gate)
- [x] `cargo clippy -- -D warnings` lint gate
- [x] Integration test suite trigger (Podman containers on Forge)
- [x] Remi server health check (packages.conary.io endpoints)

### Remi Health Monitoring

- [x] Endpoint smoke tests (sparse index, chunk fetch, stats, OCI)
- [x] Conversion pipeline test (submit package, poll, verify chunks)
- [x] Scheduled cron job (catch production regressions, not just on push)

---

## Phase 2: End-to-End Validation [COMPLETE]

278 integration tests across Fedora 43, Ubuntu Noble, and Arch Linux.

### Install Flow (per distro: Fedora, Ubuntu, Arch)

- [x] Adopt existing system packages into Conary DB (T20-T21)
- [x] Install a package from Remi with dependencies (T38-T41)
- [x] Remove a package, verify clean removal + orphan detection (T42, T50)
- [x] Update a package, verify delta application + checksums (T44-T46)
- [x] Rollback an operation, verify DB + filesystem + checksums (T47-T48)
- [x] Pin/unpin a package, verify update skips it (T49)

### Generation Lifecycle

- [x] Build a generation from current state (T51)
- [x] Switch to new generation (T54)
- [x] Rollback to previous generation (T55)
- [x] GC old generations (T56)
- [x] System takeover flow -- full adopt -> generation (T57)

### Bootstrap Pipeline

- [x] Dry-run validation passes (T58)
- [x] Stage 0 -> Stage 1 toolchain builds (T59-T61)
- [x] Base system builds with checkpointing (31 packages built, qcow2 image generation working)
- [x] Image generation produces bootable output (qcow2 generation working)
- [ ] (Stretch) Boot the image in QEMU and verify (QEMU tests in orchestrator)

### Remi Integration

- [x] Client fetches sparse index from packages.conary.io (T67)
- [x] Chunk-level install -- client has partial chunks, fetches missing (T68)
- [ ] Federation peer discovery and chunk routing
- [x] OCI distribution API serves valid manifests (T69-T70)

### Remi Admin API

- [x] External admin API on :8082 with bearer token auth (P0)
- [x] Token CRUD, CI proxy, SSE event stream (P0)
- [x] OpenAPI 3.1 spec, MCP endpoint with 24 tools (P0)
- [x] Repository management endpoints (P1)
- [x] Federation peer management endpoints (P1)
- [x] Per-IP rate limiting via governor (P2)
- [x] Audit logging with query/purge endpoints (P2)

### Test Infrastructure

- [x] Python test runner retired, all tests use Rust engine (conary-test)
- [x] Remi test data API for persistent result storage
- [x] 23 conary-test MCP tools (test ops, deployment, image management)
- [x] 24 remi-admin MCP tools (admin, CI, test data, canonical, chunk GC)
- [x] Phase 4 feature validation tests (T160-T249, 118 tests covering all CLI commands)
- [x] Capability policy engine (three-tier: allowed/prompt/denied)

### Recipe & Build

- [x] Cook a recipe from TOML, verify CCS output (T62-T63)
- [x] PKGBUILD converter produces valid recipe (T64-T65)
- [x] Hermetic build isolation works -- network blocked (T66)

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

1. Federation peer discovery and chunk routing
2. Shell integration for dev environments
3. Group package system for OS composition
4. P2P chunk distribution plugins
5. QEMU boot verification for bootstrap images

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and [CLAUDE.md](CLAUDE.md) for coding conventions.
