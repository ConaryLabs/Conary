# Conary

[![CI](https://github.com/ConaryLabs/Conary/actions/workflows/ci.yml/badge.svg)](https://github.com/ConaryLabs/Conary/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![v0.7.0](https://img.shields.io/badge/version-0.7.0-orange.svg)](CHANGELOG.md)

**Website:** [conary.io](https://conary.io) | **Packages:** [packages.conary.io](https://packages.conary.io) | **Discussions:** [GitHub Discussions](https://github.com/ConaryLabs/Conary/discussions)

A cross-distribution Linux system manager with immutable generations, atomic transactions, content-addressable storage, and a declarative system model. Conary installs native RPM/DEB/Arch packages, builds and installs CCS packages, and layers a declarative system workflow on top.

Inspired by the [original Conary](https://en.wikipedia.org/wiki/Conary_(package_manager)) from rPath, which pioneered concepts like troves, changesets, flavors, and components that were ahead of their time. This project carries those ideas forward with a modern implementation.

---

## Why Conary

**Immutable system generations.** Build read-only EROFS images of your entire system and mount them via composefs. Switch between complete system states live, without rebooting. Every generation is a self-contained snapshot -- rollback means switching a mount, not undoing thousands of file operations.

```bash
conary system generation build --summary "After nginx setup"
conary system generation list
conary system generation switch 2
conary system generation rollback
```

**Atomic operations.** Every install, remove, and update is a changeset -- an all-or-nothing transaction. If something fails, your system stays exactly as it was. Rollback is not an afterthought; it is core to how the system works. Generations extend this further: the entire system state is atomic.

```bash
conary install nginx postgresql redis
conary system state list
conary system state rollback 5
```

**Format-agnostic.** RPM, DEB, Arch packages, and Conary's native CCS format are all first-class. One tool handles them all.

```bash
conary install ./package.rpm
conary install ./package.deb
conary install ./package.pkg.tar.zst
```

**Declarative state.** Define your system in TOML and let Conary compute the diff. Drift detection, state snapshots, and full rollback come built in.

```bash
conary model diff     # What needs to change?
conary model apply    # Make it so
conary model check    # Drift detection (CI/CD friendly, uses exit codes)
```

**Cross-distro package access on day one.** Remi, the on-demand conversion proxy at [packages.conary.io](https://packages.conary.io), transparently converts upstream RPM/DEB/Arch packages into CCS format. No upstream changes are required to start using Conary against the supported upstream repositories.

```bash
conary repo add remi https://packages.conary.io
conary repo sync
conary install nginx
```

**Current focus: hardening and developer experience.** The core install, rollback, generation, bootstrap, and server paths are in place; the project is now spending more time on verification, operational polish, and documentation than on first-pass scaffolding.

---

## How It Compares

| Capability | apt/dnf | pacman | Nix | Conary |
|---|---|---|---|---|
| Immutable generations | No | No | Yes (generations) | Yes (EROFS + composefs) |
| Atomic transactions | No | No | Yes | Yes |
| Rollback to any state | No | No | Yes (generations) | Yes (snapshots + generations) |
| System takeover | No | No | No | Yes (partial) |
| Bootstrap from scratch | No | No | Yes | Yes (partial) |
| Multi-format (RPM + DEB + Arch) | No | No | No | Yes |
| Derived packages | No | No | Yes (overlays) | Yes |
| Component model (install :devel only) | No | Split packages | No | Automatic |
| Declarative system state | No | No | Yes (flake.nix) | Yes (system.toml) |
| Content-addressable storage | No | No | Yes | Yes |
| Hermetic builds | No | No | Yes | Yes (experimental) |
| Dev shells | No | No | Yes | Yes |
| OCI container export | No | No | Yes | Yes (experimental) |
| Capability enforcement (landlock/seccomp) | No | No | No | Yes |
| Scriptlet sandboxing | No | No | N/A | Yes |
| Single binary, no daemon required | Yes | Yes | No | Yes |
| Mature ecosystem | Yes | Yes | Yes | No (early) |
| Package count | 60K+ | 15K+ | 100K+ | Via conversion |

Conary is strongest where traditional package managers are weakest: atomic operations, cross-format support, immutable system images, and fine-grained component control. Nix shares several of Conary's design principles but uses a custom language (Nix expressions) where Conary uses TOML, and Nix does not handle RPM/DEB/Arch formats natively.

The honest gap: ecosystem maturity. apt and dnf have decades of packages and integration. Conary bridges this through format conversion (install .rpm/.deb/.pkg.tar.zst directly) and the Remi server (which converts upstream repos to CCS on the fly), but native CCS packages are still early. Immutable generations are a recent addition and are under active development.

---

## Quick Start

```bash
# Build from source (requires Rust 1.94+, Linux only)
git clone https://github.com/ConaryLabs/Conary.git
cd Conary
cargo build

# Initialize the database
conary system init

# Add the Remi package server (Fedora, Arch, Ubuntu, Debian)
conary repo add remi https://packages.conary.io
conary repo sync

# Install a package
conary install nginx --dry-run   # Preview changes
conary install nginx             # Apply atomically

# Query your system
conary list                      # All installed packages
conary query depends nginx       # Show dependencies
conary query whatprovides libc.so.6

# Adopt packages already on the system
conary system adopt --system     # Track everything installed by RPM/APT

# Build a generation from current system state
conary system generation build --summary "Initial setup"
conary system generation list
conary system generation switch 1
```

---

## Features

### System Generations

Build immutable EROFS images of your entire system and mount them via composefs. Each generation is a complete, read-only system snapshot. Switch between generations live without rebooting -- the active generation is swapped atomically. Old generations can be garbage collected to reclaim space.

Requires Linux 6.2+ with composefs support.

```bash
conary system generation build --summary "Post-update"
conary system generation list        # Show all generations
conary system generation switch 3    # Switch to generation 3
conary system generation rollback    # Revert to previous generation
conary system generation gc --keep 3 # Keep only the 3 most recent
conary system generation info 2      # Detailed info about generation 2
```

### System Takeover

Convert an existing Linux installation into a Conary-managed system. The stable adoption path today is `conary system adopt --system --full`, which bulk-imports packages into Conary with CAS backing. The progressive `system takeover` pipeline now supports `--up-to cas|owned|generation`; the `generation` level builds a bootable generation and boot entry, then stops ready to activate instead of switching live automatically.

```bash
conary system adopt --system --full  # Bulk adoption with CAS backing
conary system takeover --dry-run     # Preview the takeover plan
conary system takeover --up-to cas   # Adopt + CAS-back packages (PM untouched)
conary system takeover --up-to owned # Remove non-blocked packages from the system PM
conary system takeover --up-to generation --yes
conary system generation switch 1    # Activate the prepared generation explicitly
```

### Atomic Transactions

Every operation produces a changeset. It applies completely or not at all. The full history is retained for rollback.

```bash
conary install nginx postgresql redis
conary system state list          # See all system snapshots
conary system state diff 5 8      # Compare two snapshots
conary system state rollback 5    # Revert to snapshot 5
```

### Multi-Format Support

Install packages from any major Linux format. Conary parses metadata, dependencies, and scriptlets from all of them.

```bash
conary install ./package.rpm
conary install ./package.deb
conary install ./package.pkg.tar.zst
```

### Declarative System Model

Define desired system state in TOML. Conary computes what needs to change and applies it atomically.

```toml
# /etc/conary/system.toml
[packages]
include = ["nginx", "postgresql", "redis"]
exclude = ["sendmail"]

[packages.pinned]
openssl = "3.0.*"

[[collections]]
name = "base-server"
url = "https://packages.conary.io/collections/base-server.toml"
```

```bash
conary model diff     # What needs to change?
conary model apply    # Make it so
conary model check    # Drift detection (CI/CD friendly, uses exit codes)
```

### Content-Addressable Storage

Files are stored by SHA-256 hash with automatic deduplication. Content-defined chunking (FastCDC) enables cross-package deduplication and implicit delta updates -- if the client has 48 of 50 chunks, it downloads only 2.

```bash
conary system verify nginx      # Integrity check against CAS
conary system restore nginx     # Restore files from CAS
conary system gc                # Garbage collect unreferenced objects
```

### Dependency Resolution

SAT-based resolver (via [resolvo](https://github.com/prefix-dev/resolvo)) with typed dependency kinds: package, soname, python, perl, ruby, java, pkgconfig, cmake, binary, and more.

```bash
conary query depends nginx          # Forward dependencies
conary query rdepends openssl       # Reverse dependencies
conary query whatprovides libc.so.6 # Capability lookup
conary deptree nginx                # Full dependency tree
```

### Component Model

Packages are automatically split into components: `:runtime`, `:lib`, `:devel`, `:doc`, `:config`, `:debuginfo`. Install only what you need.

```bash
conary install nginx:runtime      # Binaries only
conary install openssl:devel      # Headers and libs for building
```

### Bootstrap System

Build a complete Conary-managed Linux system from scratch. The current public command surface is `cross-tools`, `temp-tools`, `system`, `config`, `image`, and optional `tier2`, with `bootstrap run` available for manifest-driven derivation pipelines. Completed manifest-driven runs now persist operation-scoped artifacts under `<work_dir>/operations/<op-id>/`, and the comparison commands operate on those completed run workdirs. Targets x86_64, aarch64, and riscv64.

```bash
conary bootstrap init --target x86_64
conary bootstrap check              # Verify prerequisites
conary bootstrap dry-run            # Validate pipeline without building
conary bootstrap cross-tools        # Cross-compilation toolchain
conary bootstrap temp-tools         # Temporary/self-hosted tools
conary bootstrap system             # Core system packages
conary bootstrap config             # System configuration
conary bootstrap tier2              # BLFS + Conary self-hosting (optional)
conary bootstrap image --format raw # Bootable disk image (systemd-repart)
conary bootstrap status             # Progress report
conary bootstrap resume             # Resume from last checkpoint
conary bootstrap system --skip-verify   # Skip checksum enforcement
conary bootstrap run conaryos.toml --seed ./seed    # Manifest-driven derivation pipeline
conary bootstrap verify-convergence --run-a ./bootstrap-a --run-b ./bootstrap-b
conary bootstrap diff-seeds ./seed-a ./seed-b
```

### Derived Packages

Create custom variants of existing packages with patches and file overrides, without rebuilding from source. Derived packages track their parent and can be flagged as stale when the parent is updated.

```bash
conary derive create my-nginx --from nginx
conary derive patch my-nginx fix-config.patch
conary derive override my-nginx /etc/nginx/nginx.conf --source ./custom.conf
conary derive build my-nginx
conary derive stale              # List derived packages needing rebuild
```

<details>
<summary><strong>CCS Native Package Format</strong></summary>

Conary's native format uses content-addressable chunked storage, CBOR manifests with Merkle tree verification, and Ed25519 signatures. Packages can be exported to OCI container images.

```bash
conary ccs build .                   # Build from ccs.toml
conary ccs install package.ccs       # Install a CCS package
conary ccs install package.ccs --reinstall    # Reinstall same version
conary ccs sign package.ccs          # Ed25519 signatures
conary ccs verify package.ccs        # Verify integrity
conary ccs export package --format oci  # Export to container image
```

</details>

<details>
<summary><strong>Recipe System and Hermetic Builds</strong></summary>

Build packages from source using TOML recipe files in isolated, network-blocked build environments. Hermetic builds use PID, UTS, IPC, and network namespaces with dependency-hash cache invalidation.

```bash
conary cook nginx.recipe.toml                # Build from recipe
conary cook --hermetic nginx.recipe.toml     # Maximum isolation
conary cook --fetch-only nginx.recipe.toml   # Pre-fetch for offline build
```

</details>

<details>
<summary><strong>Dev Shells</strong></summary>

Temporary environments without permanent installation -- similar to `nix shell`.

```bash
conary ccs shell python,nodejs   # Spawn a shell with packages available
conary ccs run gcc -- make       # One-shot command execution
```

</details>

<details>
<summary><strong>Collections</strong></summary>

Group packages into named sets for bulk operations.

```bash
conary collection create web-stack --members nginx,postgresql,redis
conary install @web-stack
conary collection show web-stack
conary collection add web-stack haproxy
```

</details>

<details>
<summary><strong>Labels and Federation</strong></summary>

Route packages through label chains with delegation. Inspired by the original Conary's label system for tracking package provenance.

```bash
conary query label add local@devel:main
conary query label add fedora@f43:stable
conary query label delegate local@devel:main fedora@f43:stable
```

</details>

<details>
<summary><strong>Sandboxed Scriptlets</strong></summary>

Package install scripts run in namespace isolation with resource limits. Dangerous scripts are detected automatically.

```bash
conary install pkg --sandbox=always   # Force sandboxing
conary install pkg --sandbox=never    # Trust the scripts
```

</details>

<details>
<summary><strong>Capability Enforcement</strong></summary>

Packages declare their runtime capabilities. Landlock restricts filesystem access; seccomp-bpf restricts syscalls.

```bash
conary capability list                # Packages with/without declarations
conary capability show nginx          # Show declared restrictions
conary capability run nginx -- /usr/sbin/nginx -t
```

#### Install-Time Policy

CCS packages declaring capabilities are evaluated against a three-tier policy:

| Tier | Behavior | Default capabilities |
|------|----------|---------------------|
| **Allowed** | Install proceeds silently | `cap-dac-read-search`, `cap-chown` |
| **Prompt** | Requires `--allow-capabilities` | `cap-net-raw`, `cap-sys-ptrace` |
| **Denied** | Always rejected | `cap-sys-admin`, `cap-sys-rawio` |

```bash
conary ccs install package.ccs --allow-capabilities    # Approve prompted caps
conary ccs install package.ccs --capability-policy /path/to/policy.toml
```

Custom policy file (`/etc/conary/capability-policy.toml`):
```toml
[capabilities]
allowed = ["cap-dac-read-search", "cap-chown"]
prompt = ["cap-net-raw", "cap-net-bind-service"]
denied = ["cap-sys-admin"]
default_tier = "prompt"
```

</details>

<details>
<summary><strong>Configuration Management</strong></summary>

Track, diff, backup, and restore configuration files across package updates. Conary records which config files belong to which packages and detects local modifications. Backup and restore operations are tied to the database so you can see the full history of changes.

```bash
conary config list                   # Show modified config files
conary config list nginx --all       # All config files for a package
conary config diff /etc/nginx/nginx.conf  # Diff against package version
conary config backup /etc/nginx/nginx.conf
conary config restore /etc/nginx/nginx.conf
conary config check                  # Check all config file status
```

</details>

<details>
<summary><strong>Package Provenance and SBOM</strong></summary>

Full supply chain metadata for every package. Provenance tracks where a package came from, how it was built, and what it contains. SBOM generation produces standard formats for auditing. Includes SLSA attestation support for verifying build integrity.

```bash
conary provenance show nginx         # Origin, build info, signatures
conary provenance verify nginx       # Verify signatures and attestations
conary provenance diff nginx openssl # Compare provenance between packages
conary system sbom nginx --format spdx  # Generate SBOM
```

</details>

<details>
<summary><strong>Trigger System</strong></summary>

Automatic post-transaction actions with DAG-based ordering. 10+ built-in triggers handle common system maintenance: ldconfig (shared library cache), depmod (kernel modules), fc-cache (fonts), update-mime-database, update-desktop-database, gtk-update-icon-cache, glib-compile-schemas, systemd-related reloads, and more. Triggers fire based on file path patterns and can be enabled, disabled, or extended with custom triggers.

```bash
conary system trigger list           # Show all triggers
conary system trigger show ldconfig  # Trigger details
conary system trigger enable NAME    # Enable a trigger
conary system trigger disable NAME   # Disable a trigger
```

</details>

---

## Architecture

Conary is a system manager structured around a few core concepts:

| Concept | Description |
|---------|-------------|
| **Trove** | The universal unit -- packages, components, and collections are all troves |
| **Changeset** | An atomic transition from one system state to another |
| **Generation** | An immutable EROFS image of a complete system state |
| **Flavor** | Build variations (architecture, feature flags): `[ssl, !debug, is: x86_64]` |
| **Label** | Package provenance: `repository@namespace:tag` |
| **CAS** | Content-addressable storage for all file data |

**Database-first design.** All state lives in SQLite. No config files for runtime state. Every operation is queryable, every state transition is recorded.

For a detailed architecture overview, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

---

## Remi Server

Conary includes an on-demand CCS conversion proxy called Remi. It converts legacy packages (RPM, DEB, Arch) to CCS format on the fly, serves chunks via content-addressable storage, and provides a sparse index for efficient client sync without requiring upstream package authors to republish in CCS first.

A public instance runs at **[packages.conary.io](https://packages.conary.io)**.

Features: Bloom filter acceleration, batch endpoints, pull-through caching, full-text search (Tantivy), repository metadata verification, and Prometheus metrics.

- **Admin API** on `:8082` with bearer token auth -- token management, CI proxy, test data persistence, and MCP endpoints for infrastructure automation

```bash
# Build with server support
cargo build --features server

# Run the server
conary remi --bind 0.0.0.0:8080
```

---

## conaryd Daemon

A local daemon that provides a REST API for package operations over a Unix socket, with SSE event streaming for real-time progress. Integrates with systemd for socket activation and watchdog support.

```bash
# Build with server + daemon support
cargo build --features server

# Run the daemon
conary daemon
```

See the [Conaryopedia](docs/conaryopedia-v2.md) for the full REST endpoint list.

---

## CAS Federation

Distributed chunk sharing across Conary nodes for bandwidth savings. Federation supports hierarchical peer routing, optional mDNS discovery for trusted LANs, tier allowlists, and pinned TLS identities for HTTPS peers.

```bash
conary federation status              # Overview
conary federation peers               # List peers
conary federation add-peer URL --tier cell_hub --tls-fingerprint SHA256HEX
conary federation scan                # mDNS discovery (requires allowlist or authenticated transport)
conary federation stats --days 7      # Bandwidth savings report
```

---

## Test Infrastructure

The `conary-test` Rust engine runs container-backed integration suites against real distros and a live Remi deployment:

```bash
cargo run -p conary-test -- run --suite phase1-core --distro fedora43 --phase 1
cargo run -p conary-test -- health    # Service health check
cargo run -p conary-test -- logs T42  # Retrieve test logs
```

- TOML manifest-based tests with per-step logging
- MCP-backed test and deployment operations for automation
- Results streamed to Remi for persistent storage

---

## Building

Requires Rust 1.94+ (edition 2024). The project is a Cargo workspace with 4 crates: `conary` (CLI), `conary-core` (library), `conary-server` (Remi + conaryd), and `conary-test` (test infrastructure).

```bash
cargo build                          # Client only (default)
cargo build --features server        # With Remi server + conaryd daemon
cargo test                           # Run all tests
cargo test --features server         # Full workspace verification, including server paths
cargo clippy -- -D warnings          # Lint check
cargo clippy --features server -- -D warnings
```

Release builds use LTO and single codegen unit for maximum optimization:

```bash
cargo build --release                # Optimized build
cargo build --profile fast-release   # Faster compile, still optimized
```

---

## Project Status

**Version 0.7.0** -- The project has a working end-to-end stack: multi-format installs, atomic changesets, immutable generations, takeover/bootstrap flows, Remi conversion and serving, federation, and capability-restricted runtime execution. Recent work has focused on tightening trust defaults, transaction atomicity, daemon/server auth, scriptlet isolation, and integrity verification across retrieval and generation paths.

See [ROADMAP.md](ROADMAP.md) for what we're building next.

---

## What's Next

The next milestone is the current **developer-experience and validation** push -- see [ROADMAP.md](ROADMAP.md) for the full plan. Near-term priorities:

- Shell integration and smoother day-to-day developer workflows
- Bootstrap and takeover validation on real systems
- Better operational docs, release hygiene, and contributor onboarding
- Continued hardening around trust, federation, and rollback behavior

---

## Documentation

| Document | Description |
|----------|-------------|
| [ROADMAP.md](ROADMAP.md) | Forward-looking development roadmap |
| [CHANGELOG.md](CHANGELOG.md) | Release history |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Development setup and contribution guidelines |
| [SECURITY.md](SECURITY.md) | Vulnerability reporting policy |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | System design overview |
| [docs/conaryopedia-v2.md](docs/conaryopedia-v2.md) | Comprehensive technical guide |
| [docs/specs/ccs-format-v1.md](docs/specs/ccs-format-v1.md) | CCS package format specification |
| [docs/SCRIPTLET_SECURITY.md](docs/SCRIPTLET_SECURITY.md) | Scriptlet sandboxing and isolation |

For CLI reference: `conary --help` or `man conary` (man pages are auto-generated during build).

---

## Community

- **[GitHub Discussions](https://github.com/ConaryLabs/Conary/discussions)** -- Questions, ideas, and general conversation
- **[Good First Issues](https://github.com/ConaryLabs/Conary/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22)** -- Starter tasks for new contributors
- **[CONTRIBUTING.md](CONTRIBUTING.md)** -- Development setup and guidelines

---

## License

MIT
