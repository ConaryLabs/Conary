# Conary Roadmap

Conary is a next-generation Linux package manager with content-addressed storage, atomic generations, and chunk-level distribution. This roadmap tracks what we're building next.

For the full feature set already implemented, see git history and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

---

## Phase 1: CI & Validation Infrastructure

Everything else depends on this. We have 1,800+ unit tests and 45 schema migrations, but no automated way to prove features work end-to-end on real systems.

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
