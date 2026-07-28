# Conary

[![Merge validation](https://github.com/ConaryLabs/Conary/actions/workflows/merge-validation.yml/badge.svg?branch=main)](https://github.com/ConaryLabs/Conary/actions/workflows/merge-validation.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![v0.13.0](https://img.shields.io/badge/version-0.13.0-orange.svg)](CHANGELOG.md)

**Website:** [conary.io](https://conary.io) | **Packages:** [remi.conary.io](https://remi.conary.io) | **Discussions:** [GitHub Discussions](https://github.com/ConaryLabs/Conary/discussions)

Conary is a cross-distro package manager for Linux, written in Rust. It
installs RPM, DEB, Arch, and native CCS packages on Fedora, Ubuntu, and Arch
hosts: an RPM keeps exact RPM lifecycle and dependency semantics on Ubuntu or
Arch, a DEB keeps exact Debian semantics on Fedora or Arch, and an Arch
package keeps exact ALPM semantics on Fedora or Ubuntu — without invoking
dnf, apt, or pacman.

Every change runs through one source-independent atomic transaction model,
recorded as changesets over content-addressed storage, with immutable system
generations for rollback and image export.
The source package format defines the package ABI; Conary owns install,
update, remove, and rollback; the target supplies an explicitly inventoried
set of typed host capabilities.

The primary adoption path is cross-distro package installation. For packages
already owned by dnf, apt, or pacman on an existing system, adoption remains
available as a reversible migration bridge; those native package managers are
not runtime authority for normal Conary-owned package operations.

Inspired by the [original Conary](https://en.wikipedia.org/wiki/Conary_(package_manager))
from rPath, but this is an independent project. It is not affiliated with,
endorsed by, or maintained by rPath, SAS, or the original Conary developers.

## Early Preview Warning

Conary is still early. Expect failures.

Use a VM or disposable host first. The current public preview is useful for
testing cross-distro package installation, Remi conversion, exact transaction
history, removal, and self-update. Adoption is the secondary migration lane for
an existing system. Conary is not ready to run a critical system unattended.

The highest-risk failure class is source-package lifecycle execution. RPM,
Debian, and ALPM expose finite documented transaction ABIs plus package-authored
programs. Conary preserves those native slots, arguments, ordering, triggers,
configuration semantics, and payload visibility, then executes them against
typed target capabilities without invoking the source package manager. A
missing model or capability is an engineering defect or an exact target
preflight error—not a reason to guess semantics or route a package into an
indefinite manual-review queue.

If you hit a failure, capture the command, distro, package name, Conary
version, source package format, and exact error. For first-wave testing, use the
[agent-assisted tester loop](docs/guides/agent-assisted-tester-loop.md) and
attach only a reviewed support bundle.

## Try It

Download the pinned preview release from
[v0.13.0](https://github.com/ConaryLabs/Conary/releases/tag/v0.13.0), verify
the package against its published `SHA256SUMS`, and install it only on a VM or
non-critical host. Release artifact evidence is tracked in
[docs/operations/release-artifact-matrix.md](docs/operations/release-artifact-matrix.md).

Then choose a source whose package format differs from the host and run the
complete bounded loop:

```bash
source=ubuntu-26.04  # Fedora/Arch hosts; use fedora-44 on Ubuntu
sudo conary repo list
sudo conary repo sync remi
sudo conary install htop --from "$source" --dry-run
sudo conary install htop --from "$source" --yes
sudo conary list htop --info
sudo conary query depends htop
sudo conary update htop --dry-run
sudo conary remove htop --yes
```

You can also pass a local RPM, DEB, or Arch artifact on any supported target:

```bash
sudo conary install ./package.rpm --dry-run
sudo conary install ./package.deb --dry-run
sudo conary install ./package.pkg.tar.zst --dry-run
```

Conary validates exact package-declared capabilities against the selected
target during preflight and applies the executor-owned enforcement contract
automatically. An unsupported declaration fails before mutation; there is no
blanket capability-approval bypass.

The RPM, DEB, and Arch packages initialize the root-owned system database and
all built-in RPM, DEB, and Arch source feeds during installation. The host
distribution does not select which package ecosystems Conary may resolve.

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
- Source-independent installation of RPM, DEB, and Arch artifacts through
  typed native lifecycle, dependency, payload, and configuration contracts.
- Native package adoption and non-destructive unadoption.
- Installing CCS packages and converted RPM/DEB/Arch packages with Conary as
  package authority.
- Atomic package-state changesets, history, and rollback-oriented state
  tracking.
- Immutable EROFS/composefs generations on hosts with the needed kernel and
  tooling.
- Raw, qcow2, and x86_64 UEFI ISO generation export for validation workflows.
- Remi on-demand conversion, package search, sparse metadata, and public
  release/self-update serving.

## What Will Break

- A package that needs a target capability the host does not provide fails
  exact preflight before mutation.
- Source lifecycle forms outside the implemented RPM, Debian, or ALPM ABI are
  bugs to model and test; Conary does not invent behavior from command text.
- Security-only updates fail closed unless a repository declares trusted
  advisory metadata support.
- Native transaction-history import is not implemented.
- Non-x86_64 generation boot assets are still reserved.
- The 0.13 schema hard cut is not readable by the 0.12 CCS self-update parser;
  install the 0.13 native package fresh instead of attempting that in-place
  update.
- SBOM/provenance sidecars are not published for the current preview suite.

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

## Scale And Verification

Conary is a single-maintainer project, so the verification layer carries the
weight that a team's review would otherwise carry.

| | |
| --- | --- |
| First-party Rust | 447,093 lines across 1,225 files, 8-crate workspace, edition 2024, Rust 1.96+ |
| Unit tests | 5,499 `#[test]` and `#[tokio::test]` functions |
| Integration tests | 324 tests in 29 suites across 4 phases |
| Test targets | Real Fedora 44, Ubuntu 26.04 LTS, and Arch VMs driven by `apps/conary-test` |
| CI | 7 GitHub Actions workflows: merge validation, PR gate, release build, release-artifact proof, deploy-and-verify, site deploy, scheduled ops |
| Lint gate | `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` |

Two of those deserve specific mention:

- **Cross-distro proof runs on real hosts, not mocks.** The integration harness
  boots Fedora, Ubuntu, and Arch VMs and exercises the same RPM, DEB, Arch, and
  CCS pipeline on each, because the claim being tested is that a package keeps
  its source semantics on a host whose native format differs. A mocked target
  cannot prove that.
- **Documentation claims are gated by CI.** `scripts/check-doc-truth.sh` fails
  the build when this README, the roadmap, the module docs, or the public
  websites claim something the code does not do. Release tags, install-command
  forms, and preview boundaries are all checked against the source tree. The
  cost of that is real: changing a public claim means changing the code or the
  check, not the prose.

## Build From Source

Conary requires Rust 1.96+ on Linux.

```bash
git clone https://github.com/ConaryLabs/Conary.git
cd Conary
cargo build -p conary
sudo ./target/debug/conary system init
```

For an isolated non-root development database, pass a writable `--db-path`;
subsequent commands must use the same path.

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
and validates the exact source-format lifecycle contract carried by each
converted artifact. There is no operator-review lane between conversion and
serving.

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
