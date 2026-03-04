# Conary

A modern package manager with atomic transactions, multi-format support, and declarative system state. Written in Rust, backed by SQLite.

Inspired by the [original Conary](https://en.wikipedia.org/wiki/Conary_(package_manager)) from rPath, which pioneered concepts like troves, changesets, flavors, and components that were ahead of their time. This project carries those ideas forward with a modern implementation.

---

## Why Conary

**Atomic operations.** Every install, remove, and update is a changeset -- an all-or-nothing transaction. If something fails, your system stays exactly as it was. Rollback is not an afterthought; it is core to how the system works.

**Format-agnostic.** RPM, DEB, Arch packages, and Conary's native CCS format are all first-class. One tool handles them all.

**Declarative state.** Define your system in TOML and let Conary compute the diff. Drift detection, state snapshots, and full rollback come built in.

**96K lines of Rust, 1,300+ tests, database schema v36.** This is not a prototype.

---

## Quick Start

```bash
# Build from source (requires Rust 1.92+)
git clone https://github.com/ConaryLabs/Conary.git
cd Conary
cargo build

# Initialize the database
conary system init

# Add a repository and sync
conary repo add fedora https://packages.conary.io/fedora/43
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
```

---

## Features

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

### Component Model

Packages are automatically split into components: `:runtime`, `:lib`, `:devel`, `:doc`, `:config`, `:debuginfo`. Install only what you need.

```bash
conary install nginx:runtime      # Binaries only
conary install openssl:devel      # Headers and libs for building
```

### Declarative System Model

Define desired system state in TOML. Conary computes what needs to change and applies it atomically.

```toml
# /etc/conary/system.toml
[model]
version = 1
install = ["nginx", "postgresql", "redis"]
exclude = ["sendmail"]

[pin]
openssl = "3.0.*"

[include]
models = ["group-base-server@corp:production"]
```

```bash
conary model diff     # What needs to change?
conary model apply    # Make it so
conary model check    # Drift detection (CI/CD friendly, uses exit codes)
```

### CCS Native Package Format

Conary's native format uses content-addressable chunked storage, CBOR manifests with Merkle tree verification, and Ed25519 signatures. Packages can be exported to OCI container images.

```bash
conary ccs build .                   # Build from ccs.toml
conary ccs sign package.ccs          # Ed25519 signatures
conary ccs verify package.ccs        # Verify integrity
conary ccs export package --format oci  # Export to container image
```

### Recipe System

Build packages from source using TOML recipe files in isolated, network-blocked build environments. Hermetic builds use PID, UTS, IPC, and network namespaces with dependency-hash cache invalidation.

```bash
conary cook nginx.recipe.toml                # Build from recipe
conary cook --hermetic nginx.recipe.toml     # Maximum isolation
conary cook --fetch-only nginx.recipe.toml   # Pre-fetch for offline build
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

### Dev Shells

Temporary environments without permanent installation -- similar to `nix shell`.

```bash
conary ccs shell python,nodejs   # Spawn a shell with packages available
conary ccs run gcc -- make       # One-shot command execution
```

### Collections

Group packages into named sets for bulk operations.

```bash
conary collection create web-stack --members nginx,postgresql,redis
conary install @web-stack
conary update-group web-stack    # Update all members atomically
```

### Labels and Federation

Route packages through label chains with delegation. Inspired by the original Conary's label system for tracking package provenance.

```bash
conary query label add local@devel:main
conary query label add fedora@f43:stable
conary query label delegate local@devel:main fedora@f43:stable
```

### Sandboxed Scriptlets

Package install scripts run in namespace isolation with resource limits. Dangerous scripts are detected automatically.

```bash
conary install pkg --sandbox=always   # Force sandboxing
conary install pkg --sandbox=never    # Trust the scripts
```

### Capability Enforcement

Packages declare their runtime capabilities. Landlock restricts filesystem access; seccomp-bpf restricts syscalls.

```bash
conary capability audit nginx         # Show declared capabilities
conary capability enforce nginx       # Apply restrictions
```

---

## How It Compares

| Capability | apt/dnf | pacman | Nix | Conary |
|---|---|---|---|---|
| Atomic transactions | No | No | Yes | Yes |
| Rollback to any state | No | No | Yes (generations) | Yes (snapshots) |
| Multi-format (RPM + DEB + Arch) | No | No | No | Yes |
| Component model (install :devel only) | No | Split packages | No | Automatic |
| Declarative system state | No | No | Yes (flake.nix) | Yes (system.toml) |
| Content-addressable storage | No | No | Yes | Yes |
| Hermetic builds | No | No | Yes | Yes |
| Dev shells | No | No | Yes | Yes |
| OCI container export | No | No | Yes | Yes |
| Capability enforcement (landlock/seccomp) | No | No | No | Yes |
| Scriptlet sandboxing | No | No | N/A | Yes |
| Single binary, no daemon required | Yes | Yes | No | Yes |
| Mature ecosystem | Yes | Yes | Yes | No (early) |
| Package count | 60K+ | 15K+ | 100K+ | Via conversion |

Conary is strongest where traditional package managers are weakest: atomic operations, cross-format support, and fine-grained component control. Nix shares several of Conary's design principles but uses a custom language (Nix expressions) where Conary uses TOML, and Nix does not handle RPM/DEB/Arch formats natively.

The honest gap: ecosystem maturity. apt and dnf have decades of packages and integration. Conary bridges this through format conversion (install .rpm/.deb/.pkg.tar.zst directly) and the Remi server (which converts upstream repos to CCS on the fly), but native CCS packages are still early.

---

## Architecture

Conary is structured around a few core concepts:

| Concept | Description |
|---------|-------------|
| **Trove** | The universal unit -- packages, components, and collections are all troves |
| **Changeset** | An atomic transition from one system state to another |
| **Flavor** | Build variations (architecture, feature flags): `[ssl, !debug, is: x86_64]` |
| **Label** | Package provenance: `repository@namespace:tag` |
| **CAS** | Content-addressable storage for all file data |

**Database-first design.** All state lives in SQLite. No config files for runtime state. Every operation is queryable, every state transition is recorded.

For a detailed architecture overview, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

---

## Remi Server

Conary includes an on-demand CCS conversion proxy called Remi. It converts legacy packages (RPM, DEB, Arch) to CCS format on the fly, serves chunks via content-addressable storage, and provides a sparse index for efficient client sync.

A public instance runs at **[packages.conary.io](https://packages.conary.io)**.

Features: Bloom filter acceleration, batch endpoints, pull-through caching, full-text search (Tantivy), TUF supply chain trust, and Prometheus metrics.

```bash
# Build with server support
cargo build --features server

# Run the server
conary server --bind 0.0.0.0:8080 --data-dir /var/lib/remi
```

---

## conaryd Daemon

A local daemon that provides a REST API for package operations over a Unix socket, with SSE event streaming for real-time progress. Integrates with systemd for socket activation and watchdog support.

```bash
# Build with daemon support
cargo build --features daemon

# Run the daemon
conary daemon
```

See the [API documentation](CLAUDE.md) for the full REST endpoint list.

---

## CAS Federation

Distributed chunk sharing across Conary nodes for bandwidth savings. Nodes discover peers via mDNS on the LAN and form a hierarchy (leaf -> cell hub -> region hub) for efficient chunk distribution.

```bash
conary federation status              # Overview
conary federation peers               # List peers
conary federation add-peer URL --tier cell_hub
conary federation scan                # mDNS LAN discovery
conary federation stats --days 7      # Bandwidth savings report
```

---

## Building

Requires Rust 1.92+ (edition 2024).

```bash
cargo build                          # Client only (default)
cargo build --features server        # With Remi server
cargo build --features daemon        # With daemon (includes server)
cargo test                           # Run all tests (~1,300 tests)
cargo clippy -- -D warnings          # Lint check
```

Release builds use LTO and single codegen unit for maximum optimization:

```bash
cargo build --release                # Optimized build
cargo build --profile fast-release   # Faster compile, still optimized
```

---

## Project Status

**Version 0.2.0** -- Core architecture is complete and tested. The codebase has 96,000+ lines of Rust across 326 source files, with 1,300+ tests passing. A production Remi server is running at packages.conary.io.

Areas of active development:
- Atomic filesystem updates (renameat2 RENAME_EXCHANGE)
- Multi-version package support
- Factory system for recipe templates
- Web interface for system state visualization

See [ROADMAP.md](ROADMAP.md) for the full feature status and planned work.

---

## Documentation

| Document | Description |
|----------|-------------|
| [ROADMAP.md](ROADMAP.md) | Feature status and planned work |
| [CHANGELOG.md](CHANGELOG.md) | Release history |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Development setup and contribution guidelines |
| [SECURITY.md](SECURITY.md) | Vulnerability reporting policy |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | System design overview |
| [docs/conaryopedia.md](docs/conaryopedia.md) | Original Conary concepts and terminology |
| [docs/specs/ccs-format-v1.md](docs/specs/ccs-format-v1.md) | CCS package format specification |
| [docs/SCRIPTLET_SECURITY.md](docs/SCRIPTLET_SECURITY.md) | Scriptlet sandboxing and isolation |

For CLI reference: `conary --help` or `man conary` (man pages are auto-generated during build).

---

## License

MIT
