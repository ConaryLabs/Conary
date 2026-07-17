# Conary

[![Merge validation](https://github.com/ConaryLabs/Conary/actions/workflows/merge-validation.yml/badge.svg?branch=main)](https://github.com/ConaryLabs/Conary/actions/workflows/merge-validation.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![v0.11.2](https://img.shields.io/badge/version-0.11.2-orange.svg)](CHANGELOG.md)

**Website:** [conary.io](https://conary.io) | **Packages:** [remi.conary.io](https://remi.conary.io) | **Discussions:** [GitHub Discussions](https://github.com/ConaryLabs/Conary/discussions)

Conary is an early Linux system manager written in Rust. It can install
native RPM, DEB, Arch, and CCS packages, record package transactions as
changesets, adopt packages that already exist on a Fedora, Ubuntu, or Arch
host, and build immutable system generations for rollback and image export.

The short-term goal is not to replace mature distro package managers. It is to
make a safer path for trying Conary on an existing Linux system: observe or
adopt native packages first, keep the native package manager authoritative
until the user explicitly crosses that boundary, and collect real failure data
from package installs that do not work yet.

Inspired by the [original Conary](https://en.wikipedia.org/wiki/Conary_(package_manager))
from rPath, but this is an independent project. It is not affiliated with,
endorsed by, or maintained by rPath, SAS, or the original Conary developers.

## Early Preview Warning

Conary is still early. Expect failures.

Use a VM or disposable host first. The current public preview is useful for
testing adoption, Remi package conversion, simple installs such as `htop`,
transaction history, and self-update. It is not ready to run a critical system
unattended.

The most important failure class right now is package scriptlets. Remi converts
packages from upstream Fedora, Ubuntu, and Arch repositories into CCS, but many
real packages run maintainer scripts for systemd, triggers, users/groups,
SELinux, alternatives, caches, and other host integration. Some of those paths
are native-free and install cleanly. Others are queued for scriptlet review,
blocked by conservative policy, or still expose bugs. That is expected during
the preview, and those failures are the data we need.

If you hit a failure, capture the command, distro, package name, Conary
version, and any refusal text. For first-wave testing, use the
[agent-assisted tester loop](docs/guides/agent-assisted-tester-loop.md) and
attach only a reviewed support bundle.

## Try It

Download the pinned preview release from
[v0.11.2](https://github.com/ConaryLabs/Conary/releases/tag/v0.11.2), verify
`SHA256SUMS`, and install the package for your test VM. Release artifact
expectations are tracked in
[docs/operations/release-artifact-matrix.md](docs/operations/release-artifact-matrix.md).

Then try the smallest package loop:

```bash
sudo conary repo list
sudo conary repo sync remi
sudo conary install htop --dry-run
sudo conary install htop --yes
sudo conary list htop --info
```

The RPM, DEB, and Arch packages initialize the root-owned system database with
the exact host profile during installation. Do not add a second Remi source.

To test reversible adoption without handing package ownership to Conary:

```bash
sudo conary system adopt --system --dry-run
sudo conary system adopt --system
sudo conary system adopt --status
sudo conary system unadopt --all --dry-run
sudo conary system unadopt --all --yes
```

Commands that change packages, files, generation state, or selected native
authority require command-local apply intent with `--yes`. Use `--dry-run`
first when the command supports it.

## What Works Today

- Package-manager preview on Fedora 44, Ubuntu 26.04 LTS, and Arch Linux.
- Native package adoption and non-destructive unadoption.
- Installing CCS packages and converted RPM/DEB/Arch packages when dependency
  and scriptlet policy allow it.
- Atomic package-state changesets, history, and rollback-oriented state
  tracking.
- Immutable EROFS/composefs generations on hosts with the needed kernel and
  tooling.
- Raw, qcow2, and x86_64 UEFI ISO generation export for validation workflows.
- Remi on-demand conversion, package search, sparse metadata, and public
  release/self-update serving.

## What Will Break

- Many scriptlet-heavy packages still need review or adapter work.
- Critical system packages and packages requiring privileged runtime
  capabilities may be refused before conversion.
- Security-only updates fail closed unless a repository declares trusted
  advisory metadata support.
- Native transaction-history import is not implemented.
- Non-x86_64 generation boot assets are still reserved.
- SBOM/provenance sidecars are not published for every preview artifact yet.

## Common Commands

```bash
# Install and inspect packages
sudo conary install nginx --dry-run
sudo conary install nginx --yes
sudo conary list
sudo conary list nginx --info
sudo conary list nginx --files
sudo conary query depends nginx
sudo conary query whatprovides 'soname(libssl.so.3)'

# Update Conary-owned packages
sudo conary update --dry-run
sudo conary update nginx --yes

# Build and select immutable generations
sudo conary system generation build --summary "After nginx setup" --yes
sudo conary system generation list
sudo conary system generation switch 1 --yes
sudo conary system generation rollback --yes

# Export a generation artifact
sudo conary system generation export --path /conary/generations/1 --format qcow2 --output gen1.qcow2
sudo conary system generation export --path /conary/generations/1 --format iso --output gen1.iso

# Self-update the CLI
sudo conary self-update --check
sudo conary self-update
```

The default `conary --help` shows the daily-driver commands. The full
packaging/platform surface is listed by `conary --help-advanced` and in
[docs/guides/advanced-commands.md](docs/guides/advanced-commands.md).

## Architecture At A Glance

Conary is a virtual Rust workspace:

- `apps/conary`: package-manager CLI.
- `crates/conary-core`: package metadata, repository sync, resolver,
  transactions, scriptlets, database, CAS, generation, CCS, model, and
  bootstrap logic.
- `apps/remi`: public/admin package conversion and serving service.
- `apps/conaryd`: local daemon with Unix-socket REST/SSE scaffolding.
- `apps/conary-test`: integration-test harness.
- `crates/conary-bootstrap`, `crates/conary-agent-contract`, and
  `crates/conary-mcp`: shared runtime and automation support crates.

For the maintained architecture map, see
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Build From Source

Conary requires Rust 1.96+ on Linux.

```bash
git clone https://github.com/ConaryLabs/Conary.git
cd Conary
cargo build -p conary
sudo ./target/debug/conary system init --profile fedora-44
```

Use `--profile ubuntu-26.04` or `--profile arch` on those supported hosts. For
an isolated non-root development database, pass both the exact profile and a
writable `--db-path`; subsequent commands must use the same path.

Useful verification commands:

```bash
cargo build -p conary
cargo build -p remi
cargo build -p conaryd
cargo build -p conary-test
cargo test -p conary
cargo test -p conary-core
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

## Remi

Remi is Conary's public on-demand conversion service at
[remi.conary.io](https://remi.conary.io). It converts supported Fedora,
Ubuntu, and Arch packages into CCS artifacts, serves public release metadata,
and records scriptlet evidence for packages that need review.

Remi public serving is intentionally conservative while the scriptlet adapter
surface matures. A package may fail even when the upstream package exists and
downloads normally. Those failures are preview feedback, not proof that the
package should be ignored.

See [docs/modules/remi.md](docs/modules/remi.md) and
[docs/guides/self-hosted-remi.md](docs/guides/self-hosted-remi.md) for service
details.

## Documentation

- [AGENTS.md](AGENTS.md): repo contract for coding agents.
- [CONTRIBUTING.md](CONTRIBUTING.md): development setup and contribution flow.
- [ROADMAP.md](ROADMAP.md): current priorities.
- [CHANGELOG.md](CHANGELOG.md): release history.
- [docs/guides/agent-assisted-tester-loop.md](docs/guides/agent-assisted-tester-loop.md): first-wave tester workflow.
- [docs/operations/release-artifact-matrix.md](docs/operations/release-artifact-matrix.md): release artifact expectations.
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): system architecture.
- [docs/INTEGRATION-TESTING.md](docs/INTEGRATION-TESTING.md): integration test harness and suites.
- [docs/SCRIPTLET_SECURITY.md](docs/SCRIPTLET_SECURITY.md): scriptlet sandboxing and policy.

## Community

- [GitHub Discussions](https://github.com/ConaryLabs/Conary/discussions)
- [CONTRIBUTING.md](CONTRIBUTING.md)

## License

[MIT](LICENSE)
