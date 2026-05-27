# Conary

[![PR Gate](https://github.com/ConaryLabs/Conary/actions/workflows/pr-gate.yml/badge.svg)](https://github.com/ConaryLabs/Conary/actions/workflows/pr-gate.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![v0.8.0](https://img.shields.io/badge/version-0.8.0-orange.svg)](CHANGELOG.md)

**Website:** [conary.io](https://conary.io) | **Packages:** [remi.conary.io](https://remi.conary.io) | **Discussions:** [GitHub Discussions](https://github.com/ConaryLabs/Conary/discussions)

A cross-distribution Linux system manager with immutable generations, atomic transactions, content-addressable storage, and a declarative system model. Conary installs native RPM/DEB/Arch packages, builds and installs CCS packages, and layers a declarative system workflow on top.

Inspired by the [original Conary](https://en.wikipedia.org/wiki/Conary_(package_manager)) from rPath, which pioneered concepts like troves, changesets, flavors, and components that were ahead of their time. This project carries those ideas forward with a modern implementation.

**Release status:** Conary is being prepared as an adoption-led limited public preview for Fedora 44, Ubuntu 26.04 LTS, and Arch Linux. The package-manager preview surface is the CLI install/remove/update path, native-package adoption/unadoption, selected-generation native-authority handoff, Remi conversion, and local validation for those flows. Immutable generations and raw/qcow2 generation export remain core capabilities, and the refreshed 2026-05-21 Group O QEMU run passed installed-runtime and bootstrap-run generation export boot proof. The codebase now also has x86_64 UEFI ISO generation-carrier export with output provenance sidecars, and the focused 2026-05-21 Group P QEMU run passed ISO export, host copy-back, provenance, readonly-carrier boot, and writable `/etc` overlay proof. The public ask is still intentionally package-manager focused: in adoption mode, dnf, apt, and pacman remain authoritative for packages they already own; `conary --allow-live-system-mutation system unadopt --all` is the non-destructive escape hatch on hosts without a selected Conary generation, and `conary --allow-live-system-mutation system native-handoff --yes` is the staged handoff path after a Conary generation has been selected. Conary-owned updates work without requiring a selected generation, adopted packages are skipped unless takeover is explicit, and `update --security` refuses before mutation when a requested Conary-owned source cannot prove advisory metadata support. Repositories marked `--security-advisories supported` can now drive security-only updates from trusted JSON advisory metadata with persisted advisory ID, CVE, severity, fixed-version, and source-trust data. Takeover remains explicit, native transaction-history import remains out of scope, and non-x86_64 generation boot assets remain reserved.

Release artifact and provenance expectations for the limited preview are tracked in [docs/operations/release-artifact-matrix.md](docs/operations/release-artifact-matrix.md).

---

## Why Conary

**Immutable system generations.** Build read-only EROFS images of your entire system and mount them via composefs. Select complete system states for the next boot or export them as bootable raw/qcow2 artifacts or x86_64 UEFI ISO generation carriers. Exportable runtime generations are self-contained CAS-backed snapshots -- rollback means switching a generation, not undoing thousands of file operations.

```bash
conary --allow-live-system-mutation system generation build --summary "After nginx setup"
conary system generation list
conary --allow-live-system-mutation system generation switch 2
conary --allow-live-system-mutation system generation rollback
```

**Atomic package state and generation selection.** Install, remove, and update operations commit package DB/file state as changesets, and generation rollback switches complete system states. Legacy RPM/DEB/Arch post-scriptlets can still fail after package files are installed or removed. Until the scriptlet trust plan lands structured degradation metadata, treat those as warning-only post-scriptlet side effects rather than part of the same rollback boundary.

```bash
conary --allow-live-system-mutation install nginx postgresql redis
conary system state list
conary --allow-live-system-mutation system state revert 5
```

**Format-agnostic.** RPM, DEB, Arch packages, and Conary's native CCS format are all first-class. One tool handles them all.

```bash
conary --allow-live-system-mutation install ./package.rpm
conary --allow-live-system-mutation install ./package.deb
conary --allow-live-system-mutation install ./package.pkg.tar.zst
```

**Declarative state.** Define your system in TOML and let Conary compute the diff. Drift detection, state snapshots, and full rollback come built in.

```bash
conary model diff     # What needs to change?
conary --allow-live-system-mutation model apply    # Make it so
conary model check    # Drift detection (CI/CD friendly, uses exit codes)
```

**Cross-distro package access on day one.** Remi, the on-demand conversion proxy at [remi.conary.io](https://remi.conary.io), transparently converts upstream RPM/DEB/Arch packages into CCS format. No upstream changes are required to start using Conary against the supported upstream repositories.

```bash
conary repo add remi https://remi.conary.io
conary repo sync
conary --allow-live-system-mutation install nginx
```

**Current focus: limited public preview readiness.** The core install, rollback, generation, bootstrap, and server paths are in place. The preview path is now adopt-first: users can let Conary observe existing RPM/DEB/Arch packages while the native package manager remains the authority, optionally CAS-back them with full adoption, then unadopt without deleting package files if they decide not to continue. Remote Forge validation is paused pending a new KVM-capable runner; QEMU release evidence should come from `scripts/local-qemu-validation.sh` on a local machine with `/dev/kvm`.

For first-wave feedback, collect an allowlist-only local support bundle with `bash scripts/conary-support-bundle.sh target/conary-support-bundle`, review it, and attach only the reviewed output to the beta feedback or bug template.

---

## How It Compares

| Capability | apt/dnf | pacman | Nix | Conary |
|---|---|---|---|---|
| Immutable generations | No | No | Yes (generations) | Yes (EROFS + composefs) |
| Package-state transaction boundary | No | No | Yes | Yes |
| Bootable generation rollback | No | No | Yes | Yes |
| Native distro adoption/unadoption | No | No | No | Yes |
| Rollback to any state | No | No | Yes (generations) | Yes (snapshots + generations) |
| Explicit system takeover | No | No | No | Yes (partial) |
| Bootstrap from scratch | No | No | Yes | Yes (partial) |
| Multi-format (RPM + DEB + Arch) | No | No | No | Yes |
| Derived packages | No | No | Yes (overlays) | Yes |
| Component model (install :devel only) | No | Split packages | No | Automatic |
| Declarative system state | No | No | Yes (flake.nix) | Yes (system.toml) |
| Content-addressable storage | No | No | Yes | Yes |
| Hermetic builds | No | No | Yes | Partial (experimental) |
| Dev shells | No | No | Yes | Yes |
| CCS package OCI export | No | No | Yes | Yes (experimental) |
| Capability enforcement (landlock/seccomp) | No | No | No | Yes |
| Scriptlet sandboxing | No | No | N/A | Yes |
| Single binary, no daemon required | Yes | Yes | No | Yes |
| Mature ecosystem | Yes | Yes | Yes | No (early) |
| Package count | 60K+ | 15K+ | 100K+ | Via conversion |

If you already run NixOS and like it, Conary is probably not trying to pull you away. Conary's near-term bet is different: keep Fedora, Ubuntu, or Arch as the base system, let Conary adopt and CAS-back what is already installed, and move into Conary-owned generations only when the user explicitly chooses that authority boundary. The trade-off is maturity and package count; Nix wins there today. Conary wins only if the migration path is safer and easier to try.

The honest gap: ecosystem maturity. apt and dnf have decades of packages and integration. Conary bridges this through format conversion (install .rpm/.deb/.pkg.tar.zst directly) and the Remi server (which converts upstream repos to CCS on the fly), but native CCS packages are still early. Immutable generations and raw/qcow2/ISO export are working on x86_64 generation artifacts, OCI export uses the same generation artifact source, and signed portable bundles remain active follow-up work.

---

## Quick Start

### Five-Minute Preview

Use this path on a VM or non-critical host first. Release binaries are not
linked for this preview tag yet, so the commands below assume the developer
build path in the next subsection and use `./target/debug/conary`.

```bash
./target/debug/conary system init
./target/debug/conary repo add remi https://remi.conary.io
./target/debug/conary repo sync
./target/debug/conary system adopt --system --dry-run
./target/debug/conary system adopt --status
```

The first install or dry-run that needs a package not already converted by Remi
may spend extra time converting upstream RPM/DEB/Arch metadata into CCS. That
cold-start latency is expected during the limited preview; reruns should be
faster once the conversion cache is warm.

When you are ready to test the reversible adoption apply path on that host:

```bash
./target/debug/conary --allow-live-system-mutation system adopt --system
./target/debug/conary system adopt --status
./target/debug/conary system unadopt --all --dry-run
./target/debug/conary --allow-live-system-mutation system unadopt --all
```

`--allow-live-system-mutation` is intentionally long: it marks the exact point
where the preview moves from inspection into changing the active host. Before
selecting a Conary generation, `conary --allow-live-system-mutation system
unadopt --all` removes Conary tracking without deleting native package files.

### Developer Build

```bash
# Build from source (requires Rust 1.94+, Linux only)
git clone https://github.com/ConaryLabs/Conary.git
cd Conary
cargo build -p conary

# Use the freshly built CLI directly from target/
./target/debug/conary system init

# Add the Remi package server (Fedora 44, Ubuntu 26.04 LTS, Arch)
./target/debug/conary repo add remi https://remi.conary.io
./target/debug/conary repo sync
```

Commands that mutate the active host require the explicit `--allow-live-system-mutation` acknowledgement; dry-run commands remain the safest first pass.

```bash
# Install a package
./target/debug/conary install nginx --dry-run   # Preview changes
./target/debug/conary --allow-live-system-mutation install nginx             # Apply atomically

# Query your system
./target/debug/conary list                      # All installed packages
./target/debug/conary list nginx --info         # Installed package identity
./target/debug/conary list nginx --files        # Installed files
./target/debug/conary list --path /usr/sbin/nginx
./target/debug/conary query depends nginx       # Show dependencies
./target/debug/conary query whatprovides 'soname(libc.so.6)'
./target/debug/conary query whatbreaks openssl
# Add --version and/or --arch when multiple installed variants match.

# Adopt packages already on the system; dnf/apt/pacman remain authoritative
./target/debug/conary system adopt --system --dry-run
./target/debug/conary --allow-live-system-mutation system adopt --system # Track native packages

# Optional escape hatch before a Conary generation is selected
# ./target/debug/conary system unadopt --all --dry-run
# ./target/debug/conary --allow-live-system-mutation system unadopt --all

# Update only Conary-owned packages by default; adopted packages stay native-PM owned
./target/debug/conary update --dry-run
./target/debug/conary --allow-live-system-mutation update nginx --yes

# Security-only updates are fail-closed unless the requested source publishes advisories
./target/debug/conary --allow-live-system-mutation update --security --dry-run

# Build a generation from current system state
./target/debug/conary --allow-live-system-mutation system generation build --summary "Initial setup"
./target/debug/conary system generation list
./target/debug/conary --allow-live-system-mutation system generation switch 1
```

---

## Features

### System Generations

Build immutable EROFS images of your entire system and mount them via composefs. Each generation is a complete, read-only system snapshot. Select generations for next boot, or export a complete generation artifact to raw/qcow2/ISO for QEMU and image validation. Old generations can be garbage collected to reclaim space.

Requires Linux 6.2+ with composefs support.

```bash
conary --allow-live-system-mutation system generation build --summary "Post-update"
conary system generation list        # Show all generations
conary --allow-live-system-mutation system generation switch 3    # Select generation 3 for next boot
conary --allow-live-system-mutation system generation rollback    # Select previous generation for next boot
conary --allow-live-system-mutation system generation gc --keep 3 # Keep only the 3 most recent
conary system generation info 2      # Detailed info about generation 2
conary system generation export --path /conary/generations/3 --format qcow2 --output gen3.qcow2
conary system generation export --path /conary/generations/3 --format iso --output gen3.iso
```

When a package mutation commits but generation publication fails, Conary exits
successfully for the package transaction and records pending publication debt.
Run `conary system generation pending` to inspect it and
`conary --allow-live-system-mutation system generation publish` to retry
publication of the current DB state.

### Adoption And Explicit Takeover

Adopt an existing Linux installation without giving up native package-manager authority. The fastest preview path today is metadata-only system adoption, `conary --allow-live-system-mutation system adopt --system`, which records native packages in Conary while dnf, apt, or pacman remains authoritative for those packages. Use `--full` when you want CAS backing for adopted package files and have budgeted the extra first-run time and disk growth. `conary --allow-live-system-mutation system unadopt --all` removes Conary tracking without deleting native package files on hosts without a selected Conary generation. If a Conary generation is already selected, use the staged native-authority handoff flow: run `conary system native-handoff --dry-run`, then `conary --allow-live-system-mutation system native-handoff --yes`; if the operation is interrupted after its record is written, rerun `conary --allow-live-system-mutation system native-handoff --recover --yes`.

Updates follow the same authority boundary. Conary-owned packages can be installed, removed, and updated on a normal mutable host without first selecting a Conary generation. Adopted packages are not silently replaced by Conary during `update`; Conary reports that the native package manager remains authoritative unless you explicitly choose `--dep-mode takeover`. Critical adopted packages remain blocked even under takeover.

Security-only updates are deliberately conservative. A repository added without advisory support metadata is treated as `unknown`, so `conary update --security` refuses before changing requested Conary-owned packages from that source. Only mark a repository `--security-advisories supported` when its synced metadata publishes a trusted advisory source. The supported JSON repository path records advisory ID, CVEs, severity, fixed version, and source trust, and the update preview prints those details before applying the trusted fix.

Takeover is a separate, explicit step. The `system takeover` release path builds a bootable generation and boot entry, then stops ready to activate instead of switching live automatically. The lower `cas` and `owned` stop-points still exist as internal/debug checkpoints, not normal preview workflows.

```bash
# Risk-free adoption lane
conary --allow-live-system-mutation system adopt --system --full  # Bulk adoption with CAS backing
conary system unadopt --all --dry-run
conary system native-handoff --dry-run
conary --allow-live-system-mutation system unadopt --all
conary --allow-live-system-mutation system native-handoff --yes

# Explicit takeover lane
conary system takeover --dry-run     # Preview the takeover plan
conary --allow-live-system-mutation system takeover --up-to generation --yes
conary --allow-live-system-mutation system generation switch 1    # Select the prepared generation for next boot
```

### Atomic Transactions

Every operation produces a changeset. It applies completely or not at all. The full history is retained for rollback.

```bash
conary --allow-live-system-mutation install nginx postgresql redis
conary system state list          # See all system snapshots
conary system state diff 5 8      # Compare two snapshots
conary --allow-live-system-mutation system state revert 5      # Revert to snapshot 5
```

### Multi-Format Support

Install packages from any major Linux format. Conary parses metadata, dependencies, and scriptlets from all of them.

```bash
conary --allow-live-system-mutation install ./package.rpm
conary --allow-live-system-mutation install ./package.deb
conary --allow-live-system-mutation install ./package.pkg.tar.zst
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

[system]
selection_mode = "latest"
allowed_distros = ["fedora-44", "ubuntu-26.04", "arch"]

[system.pin]
distro = "fedora-44"
strength = "guarded"
```

```bash
conary model diff     # What needs to change?
conary --allow-live-system-mutation model apply    # Make it so
conary model check    # Drift detection (CI/CD friendly, uses exit codes)
conary distro info
conary distro selection-mode latest
```

### Content-Addressable Storage

Files are stored by SHA-256 hash with automatic deduplication. Content-defined chunking (FastCDC) enables cross-package deduplication and implicit delta updates -- if the client has 48 of 50 chunks, it downloads only 2.

```bash
conary system verify nginx      # Integrity check against CAS
conary --allow-live-system-mutation system restore nginx     # Restore files from CAS
conary system gc                # Garbage collect unreferenced objects
```

### Dependency Resolution

SAT-based resolver (via [resolvo](https://github.com/prefix-dev/resolvo)) with typed dependency kinds: package, soname, python, perl, ruby, java, pkgconfig, cmake, binary, and more.

```bash
conary query depends nginx          # Forward dependencies
conary query rdepends openssl       # Reverse dependencies
conary query whatprovides 'soname(libssl.so.3)' # Typed capability lookup
conary query whatbreaks openssl     # Removal preflight explanation
conary query deptree nginx          # Full dependency tree
conary pin nginx                    # Hold an installed package
conary list --pinned                # Show held packages
conary unpin nginx                  # Release the hold
conary autoremove --dry-run         # Preview orphan cleanup
```

### Component Model

Packages are automatically split into components: `:runtime`, `:lib`, `:devel`, `:doc`, `:config`, `:debuginfo`. Install only what you need.

```bash
conary --allow-live-system-mutation install nginx:runtime      # Binaries only
conary --allow-live-system-mutation install openssl:devel      # Headers and libs for building
```

### Bootstrap System

Build a complete Conary-managed Linux system from scratch. The current public command surface is `cross-tools`, `temp-tools`, `system`, `config`, `image`, and optional `tier2`, with `bootstrap run` available for manifest-driven derivation pipelines. Completed manifest-driven runs persist operation-scoped artifacts under `<work_dir>/operations/<op-id>/`, and the comparison commands operate on those completed run workdirs. The checked-in self-hosting VM wrapper builds and validates an x86_64 Tier-2 qcow2 under QEMU. Targets are x86_64, aarch64, and riscv64, though the first self-hosting VM path is x86_64-first.

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
scripts/bootstrap-vm/build-selfhost-qcow2.sh --work-dir /tmp/conary-selfhost-vm --image-size 32G
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
conary --allow-live-system-mutation ccs install package.ccs       # Install a CCS package
conary --allow-live-system-mutation ccs install package.ccs --reinstall    # Reinstall same version
conary ccs sign package.ccs          # Ed25519 signatures
conary ccs verify package.ccs        # Verify integrity
conary ccs export package.ccs --output ./package.oci  # Export to container image
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
conary ccs shell python nodejs   # Spawn a shell with packages available
conary ccs run gcc -- make       # One-shot command execution
```

</details>

<details>
<summary><strong>Collections</strong></summary>

Group packages into named sets for bulk operations.

```bash
conary collection create web-stack --members nginx,postgresql,redis
conary --allow-live-system-mutation install @web-stack
conary collection show web-stack
conary collection add web-stack haproxy
```

</details>

<details>
<summary><strong>Labels and Federation</strong></summary>

Route packages through label chains with delegation. Inspired by the original Conary's label system for tracking package provenance.

```bash
conary query label add local@devel:main
conary query label add fedora@f44:stable
conary query label delegate local@devel:main fedora@f44:stable
```

</details>

<details>
<summary><strong>Sandboxed Scriptlets</strong></summary>

Package install scripts run in namespace isolation with resource limits. Dangerous scripts are detected automatically.

```bash
conary --allow-live-system-mutation install pkg --sandbox=always   # Force sandboxing
conary --allow-live-system-mutation install pkg --sandbox=never    # Trust the scripts
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
conary --allow-live-system-mutation ccs install package.ccs --allow-capabilities    # Approve prompted caps
conary --allow-live-system-mutation ccs install package.ccs --capability-policy /path/to/policy.toml
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
conary system sbom nginx --format cyclonedx        # Generate runtime SBOM
conary provenance export nginx --format spdx       # Export provenance SBOM
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

**Database-first design.** All state lives in SQLite. No config files for runtime state. Every operation is queryable, every state transition is recorded. Conary writes SQLite-native checkpoint backups for first-wave adoption/unadoption paths and stores generation-bound DB backups next to published generation artifacts. Use `conary system db-backup recover --latest --dry-run`, `conary system generation verify-db-backup --current`, and `conary system generation recover-db --generation <n> --dry-run` to verify recovery metadata before applying it.

SQLite-native backups recover Conary manager visibility for packages and generations represented by the backed-up DB. They do not recover missing package payloads, private keys, remote repository history, or native package-manager transaction history. Applying a generation-bound DB backup requires an explicit live-host acknowledgement and confirmation: `conary --allow-live-system-mutation system generation recover-db --generation <n> --yes`.

For a detailed architecture overview, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

---

## Remi Server

Conary includes an on-demand CCS conversion proxy called Remi. It converts legacy packages (RPM, DEB, Arch) to CCS format on the fly, serves chunks via content-addressable storage, and provides a sparse index for efficient client sync without requiring upstream package authors to republish in CCS first.

A public instance runs at **[remi.conary.io](https://remi.conary.io)**.

Features: Bloom filter acceleration, batch endpoints, pull-through caching, full-text search (Tantivy), repository metadata verification, and Prometheus metrics.

- **Authenticated MCP endpoint** at [`https://remi.conary.io/mcp`](https://remi.conary.io/mcp) for production automation
- **Admin origin listener** on `:8082` for bearer-authenticated REST operations behind the reverse proxy

```bash
# Build the owning service crate
cargo build -p remi

# Run the server
cargo run -p remi -- --bind 0.0.0.0:8080
```

---

## conaryd Daemon

A local daemon with Unix-socket REST scaffolding, a persistent job queue, SSE event streaming, read/query routes, package install/remove/update execution, and enhance-job support. Package mutation jobs reuse the CLI command contracts and keep the same explicit live-host mutation acknowledgement boundary.

```bash
# Build the daemon crate
cargo build -p conaryd

# Run the daemon
cargo run -p conaryd -- --foreground
```

See [docs/modules/conaryd.md](docs/modules/conaryd.md) for the maintained daemon endpoint list.

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
cargo run -p conary-test -- bootstrap check --json  # Local prerequisite/smoke-readiness inspection
cargo run -p conary-test -- run --suite phase1-core --distro fedora44 --phase 1
cargo run -p conary-test -- health    # Service health check
cargo run -p conary-test -- logs T42  # Retrieve test logs
```

- TOML manifest-based tests with per-step logging
- Structured test and deployment operations exposed through HTTP/MCP transports
- Results streamed to Remi for persistent storage

---

## Building

Requires Rust 1.94+ (edition 2024). The project root is a virtual Cargo
workspace with eight members: `apps/conary` (CLI), `apps/remi` (Remi),
`apps/conaryd` (daemon), `apps/conary-test` (test infrastructure),
`crates/conary-bootstrap` (shared binary bootstrap helpers),
`crates/conary-core` (shared library), `crates/conary-agent-contract`
(transport-neutral agent operation contract), and `crates/conary-mcp` (shared
MCP adapter helpers).

```bash
cargo build -p conary                              # CLI
cargo build -p remi && cargo build -p conaryd      # Service binaries
cargo test -p conary                               # CLI tests
cargo test -p remi && cargo test -p conaryd        # Service tests
cargo build -p conary-test                         # Test harness
cargo clippy --workspace --all-targets -- -D warnings
```

Release builds use LTO and single codegen unit for maximum optimization:

```bash
cargo build --release                # Optimized build
cargo build --profile fast-release   # Faster compile, still optimized
```

---

## Project Status

**Version 0.8.0** -- The project has a working end-to-end stack: multi-format installs, atomic changesets, adoption/unadoption, selected-generation native-authority handoff, immutable generations, explicit takeover/bootstrap flows, Remi conversion and serving, federation, conaryd package execution, and capability-restricted runtime execution. The current release-readiness pass is narrowing the public preview to an adoption-led Fedora 44, Ubuntu 26.04 LTS, and Arch Linux package-manager slice; keeping the installed-runtime and bootstrap-run generation-export QEMU gates green; adding x86_64 ISO generation-carrier export and provenance sidecars; reducing the `tough`/Sigstore advisory path while carrying the dated `rsa` waiver; and documenting remaining gaps such as portable bundle signing, native transaction-history import, and non-x86_64 generation boot assets.

See [ROADMAP.md](ROADMAP.md) for what we're building next.

---

## What's Next

The next milestone is the current **adoption-led preview and validation** push -- see [ROADMAP.md](ROADMAP.md) for the full plan. Near-term priorities:

- Keep Fedora 44, Ubuntu 26.04 LTS, and Arch adoption/unadoption proof in regular rotation
- Keep QEMU generation-export validation green in regular rotation, and unblock the ISO Group P gate with a refreshed source fixture
- Keep the selected-generation native-authority handoff suite green across Fedora 44, Ubuntu 26.04 LTS, and Arch
- Introduce signed portable generation bundles and boot-artifact provenance
- Keep self-host VM validation freshness checks green and finish snapshot/overlay rerun hygiene
- Keep the Goal 7 daily-driver UX matrix, shell completion rendering checks, and operator diagnostics current
- Continue hardening trust, federation, release, and rollback behavior

---

## Documentation

For the repo-level documentation system:

- Human contributors should start with [CONTRIBUTING.md](CONTRIBUTING.md).
- Coding assistants should start with [AGENTS.md](AGENTS.md) and
  [docs/llms/README.md](docs/llms/README.md), then follow the linked canonical
  docs.

| Document | Description |
|----------|-------------|
| [AGENTS.md](AGENTS.md) | Canonical repo-wide contract for coding agents |
| [ROADMAP.md](ROADMAP.md) | Forward-looking development roadmap |
| [CHANGELOG.md](CHANGELOG.md) | Release history |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Development setup and contribution guidelines |
| [SECURITY.md](SECURITY.md) | Vulnerability reporting policy |
| [docs/llms/README.md](docs/llms/README.md) | Vendor-neutral assistant map into the canonical docs |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | System design overview |
| [docs/INTEGRATION-TESTING.md](docs/INTEGRATION-TESTING.md) | Integration-test suites, phases, and runtime expectations |
| [docs/modules/bootstrap.md](docs/modules/bootstrap.md) | Bootstrap pipeline and command-surface reference |
| [docs/operations/bootstrap-selfhosting-vm.md](docs/operations/bootstrap-selfhosting-vm.md) | Truthful operator flow for the current self-hosting VM path |
| [docs/operations/daily-driver-ux-matrix.md](docs/operations/daily-driver-ux-matrix.md) | Daily-driver CLI diagnostics, unsupported-case routes, and shell completion checks |
| [docs/operations/post-generation-export-follow-up-roadmap.md](docs/operations/post-generation-export-follow-up-roadmap.md) | Remaining generation export, image projection, and provenance follow-ups |
| [docs/operations/bootstrap-follow-up-investigations.md](docs/operations/bootstrap-follow-up-investigations.md) | Deferred bootstrap architecture follow-ups to revisit later |
| [docs/operations/infrastructure.md](docs/operations/infrastructure.md) | MCP, deploy, and host workflow notes |
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
