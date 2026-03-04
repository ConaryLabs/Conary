# Conary 0.1.0

*Released 2026-03-03*

The first tagged release of Conary, a modern package manager with atomic transactions, multi-format support, and declarative system state. Written in Rust, backed by SQLite.

This release represents 10 months of development: 274 commits, 100,000+ lines of Rust, 44 schema migrations, and 1,200+ passing tests. Every subsystem described below is implemented and tested.

---

## Highlights

- **Install RPM, DEB, and Arch packages with a single tool.** Conary parses metadata, dependencies, and scriptlets from all major Linux package formats -- no conversion step needed.
- **Atomic transactions with full rollback.** Every operation is an all-or-nothing changeset. If something fails, your system stays exactly as it was. Roll back to any previous state by snapshot number.
- **Declarative system management.** Define your desired system state in TOML and let Conary compute the diff. Drift detection with CI/CD-friendly exit codes included.
- **Content-addressable chunk storage with implicit delta updates.** FastCDC chunking means if you already have 48 of 50 chunks, you download only the 2 you're missing -- no delta pre-computation required.
- **Remi: on-demand CCS conversion server.** A public instance at packages.conary.io converts upstream RPM/DEB/Arch repositories to Conary's native CCS format on the fly, with full-text search and TUF supply chain trust.

---

## New Features

### Multi-Format Package Management

Install packages from any major Linux distribution format. Dependencies are resolved automatically using a SAT-based solver.

```bash
conary install ./package.rpm
conary install ./package.deb
conary install ./package.pkg.tar.zst
conary install nginx postgresql redis    # From repository
```

### CCS Native Package Format

Conary's native format uses CBOR manifests, Merkle tree verification, Ed25519 signatures, and content-defined chunking for cross-package deduplication.

```bash
conary ccs build .                          # Build from ccs.toml
conary ccs build --chunked .                # With FastCDC chunking
conary ccs sign package.ccs                 # Sign with Ed25519
conary ccs verify package.ccs               # Verify integrity
conary ccs export package --format oci      # Export as OCI container image
```

Build policies (DenyPaths, NormalizeTimestamps, StripBinaries, FixShebangs, CompressManpages) and `SOURCE_DATE_EPOCH` support enable reproducible builds.

### Declarative System Model

Define your system in TOML and apply it atomically. Remote collections can be fetched from Remi servers with Ed25519 signature verification.

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
conary model diff       # What needs to change?
conary model apply      # Make it so
conary model check      # Drift detection (exit codes for CI/CD)
conary model snapshot   # Capture current state as a model
```

### System State Snapshots and Rollback

Every install and remove operation creates a numbered snapshot. Compare any two snapshots and roll back to any previous state.

```bash
conary system state list             # List all snapshots
conary system state diff 5 8         # What changed between 5 and 8?
conary system state restore 5        # Revert to snapshot 5
conary system state prune --keep 10  # Clean up old snapshots
```

### Component Model

Packages are automatically split into components. Install only what you need.

```bash
conary install nginx:runtime       # Binaries only
conary install openssl:devel       # Headers and libs for building
conary install bash:doc            # Just the docs
```

### Dev Shells and One-Shot Execution

Temporary environments without permanent installation, similar to `nix shell`.

```bash
conary ccs shell python,nodejs      # Spawn a shell with packages available
conary ccs run gcc -- make          # Execute a command without installing
```

### Remi Server (On-Demand CCS Conversion Proxy)

Converts upstream RPM/DEB/Arch packages to CCS format when requested. Build with `--features server`.

- Async conversion with 202 Accepted pattern and job polling
- Bloom filter acceleration for chunk existence checks
- Full-text package search powered by Tantivy
- Pull-through caching from upstream repositories
- Cloudflare R2 write-through for chunk storage
- Sparse index for efficient client metadata sync
- TUF supply chain trust with root, timestamp, snapshot, and targets roles
- Prometheus metrics at `/v1/admin/metrics/prometheus`
- Web package index at packages.conary.io

```bash
cargo build --features server
conary server --bind 0.0.0.0:8080 --data-dir /var/lib/remi
```

### conaryd Daemon

Local REST API for package operations over Unix socket with SSE event streaming. Build with `--features daemon`.

- SO_PEERCRED authentication with permission checking
- Persistent job queue in SQLite (survives daemon restart)
- Priority-based operation queue
- Systemd socket activation and watchdog integration
- CLI auto-forwarding: commands detect and forward to daemon when available

```bash
cargo build --features daemon
conary daemon
```

### CAS Federation

Distributed chunk sharing across Conary nodes for bandwidth savings on LANs and multi-site deployments.

```bash
conary federation status                       # Overview
conary federation peers                        # List peers
conary federation add-peer URL --tier cell_hub # Add a hub
conary federation scan                         # mDNS LAN discovery
conary federation stats --days 7               # Bandwidth savings
```

### Recipe System and Hermetic Builds

Build packages from source using TOML recipe files. Hermetic builds use Linux namespaces (PID, UTS, IPC, mount, network) with a two-phase approach: fetch (network allowed), then build (network blocked).

```bash
conary cook nginx.recipe.toml                  # Build from recipe
conary cook --hermetic nginx.recipe.toml       # Maximum isolation
conary cook --fetch-only nginx.recipe.toml     # Pre-fetch for offline build
```

### Capability Enforcement

Packages declare their runtime capabilities. Enforcement uses Landlock for filesystem restrictions and seccomp-bpf for syscall filtering.

```bash
conary capability audit nginx       # Show declared capabilities
conary capability enforce nginx     # Apply restrictions
```

### Package Provenance (DNA) and SBOM

Full provenance chain from source to deployment: origin, build environment, signatures, and content hashes. Sigstore integration for signing and verification.

```bash
conary query sbom nginx     # CycloneDX 1.5 SBOM export
```

### Additional Features

- **Collections** -- Group packages for bulk operations (`conary collection create web-stack --members nginx,postgresql,redis`)
- **Labels and Federation** -- Route packages through label chains with delegation and cycle detection
- **Trigger system** -- 10 built-in triggers (ldconfig, systemd-reload, fc-cache, etc.) with DAG-ordered execution
- **Configuration management** -- Track, diff, backup, and restore config files with `noreplace` support
- **Security-only updates** -- `conary update --security` to apply only security patches
- **Package pinning** -- Prevent specific packages from being updated or removed
- **System adoption** -- Scan and track packages already installed by RPM/APT
- **Sandboxed scriptlets** -- Package install scripts run in namespace isolation with resource limits
- **Shell completions** -- Bash, Zsh, Fish, PowerShell
- **Dry run mode** -- Preview any destructive operation before executing

---

## Improvements

- **Dependency resolution** is now SAT-based (via resolvo), replacing the hand-rolled graph solver. Handles complex version conflicts and cross-distro typed dependencies (soname, python, perl, pkgconfig, cmake, and more).
- **Repository sync is 40x faster** via batch inserts and transactions.
- **Unified package parser** provides a single interface for RPM, DEB, and Arch formats.
- **Unified decompression** handles Gzip, Xz, and Zstd with automatic format detection via magic bytes.
- **1,500+ lines of dead code removed** across two simplification passes.
- **Transaction engine** uses UUID-based crash recovery correlation for safer interrupted operations.

---

## Bug Fixes

- Fixed TOCTOU race condition in the file deployer: CAS inode reference is now held during hardlink, and copy reads from an open fd.
- Fixed double-wait bug in scriptlet execution that caused ECHILD errors when running package scripts.
- Fixed stale in-flight entries in the download manager that caused hangs when retrying failed downloads.
- Fixed arithmetic overflow in exponential backoff calculation on high retry counts.
- Multi-package dependency installs are now truly atomic (installed as a unit, not individually).
- Fixed RPM version normalization for correct epoch:version-release comparison.
- Fixed file conflict detection for upgrades of adopted packages.
- Fixed FK constraint errors when removing adopted packages.
- Fixed directory removal on package uninstall.
- Fixed symlink handling in adoption and file restore.
- Fixed DEB scriptlet arguments to conform to Debian Policy.
- Fixed Arch upgrade scriptlet function selection (pre_upgrade vs post_upgrade).
- Fixed excluded package duplicate removal in model apply.

---

## Security

- **TOCTOU hardening** in the file deployer prevents race conditions during file deployment.
- **Auth gate middleware** on the daemon rejects unauthenticated POST/PUT/DELETE requests on v1 routes.
- **Seccomp-bpf enforcement** for scriptlet execution with a dedicated profile (~90 allowed syscalls).
- **Double-wait fix** eliminates a process management bug that could leave zombie processes or cause unexpected errors.
- **Sandboxed scriptlets** run in namespace isolation (mount, PID, IPC, UTS) with resource limits by default.
- **Landlock filesystem restrictions** limit package runtime access to declared paths only.
- **TUF supply chain trust** verifies repository metadata integrity with threshold signatures.
- **Ed25519 signatures** for packages, collections, and federation manifests.
- **Token-bucket rate limiting** and IP ban lists on the Remi server.
- **CORS restrictions** separate public and admin endpoints.

---

## Breaking Changes

This is the first tagged release. There are no breaking changes from a previous stable version.

If you have been running development builds, note that the database schema is now at v44. Migrations run automatically on startup -- no manual steps required. However, downgrading to an older development build after upgrading is not supported.

---

## Migration Guide

**From development builds:**

1. Back up your database before upgrading: `cp /var/lib/conary/conary.db /var/lib/conary/conary.db.bak`
2. Build and install the new version. Schema migrations (up to v44) will run automatically on first use.
3. `conary update` with no arguments now updates all installed packages (previously required explicit package names). Adjust any scripts that relied on the old behavior.

**From no prior installation:**

No migration needed. Run `conary system init` to create a fresh database.

---

## Installation

Requires **Rust 1.92+** (edition 2024).

```bash
# Build from source
git clone https://github.com/ConaryLabs/Conary.git
cd Conary
cargo build

# With server support
cargo build --features server

# With daemon support
cargo build --features daemon

# Initialize and start using
conary system init
conary repo add fedora https://packages.conary.io/fedora/43
conary repo sync
conary install nginx
```

For release builds with LTO:

```bash
cargo build --release
```

---

## What's Next

- **Atomic filesystem updates** using `renameat2(RENAME_EXCHANGE)` for instant, zero-downtime directory swaps
- **Shell integration** for automatic environment activation when entering project directories (direnv-style)
- **P2P chunk distribution** plugins (IPFS, BitTorrent DHT) as optional transport backends
- **Multi-version package support** for keeping multiple versions of pinned packages (e.g., kernel)
- **VFS component merging** for more efficient installation of component subsets
- **Full repository server** with version control and branch management beyond Remi's conversion proxy

---

## Acknowledgments

Conary carries forward ideas from the [original Conary](https://en.wikipedia.org/wiki/Conary_(package_manager)) by rPath (troves, changesets, flavors, components, labels), with additional inspiration from Aeryn OS / Serpent OS (atomic updates, triggers, state snapshots), Nix (dev shells, lockfiles, hermetic builds), and TUF (supply chain trust).

---

**Full changelog:** [CHANGELOG.md](CHANGELOG.md)
**Documentation:** [docs/](docs/) | [Conaryopedia](docs/conaryopedia-v2.md) | `conary --help`
**License:** MIT
