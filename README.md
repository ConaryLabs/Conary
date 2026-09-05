# Conary

[![Merge validation](https://github.com/FieldmouseWorks/Conary/actions/workflows/merge-validation.yml/badge.svg?branch=main)](https://github.com/FieldmouseWorks/Conary/actions/workflows/merge-validation.yml)
[![Client license: MIT OR Apache-2.0](https://img.shields.io/badge/Client%20license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![Remi license: AGPL-3.0-or-later](https://img.shields.io/badge/Remi%20license-AGPL--3.0--or--later-blue.svg)](apps/remi/LICENSE)
[![Latest release](https://img.shields.io/github/v/release/FieldmouseWorks/Conary?label=release)](https://github.com/FieldmouseWorks/Conary/releases/latest)

**Website:** [conary.io](https://conary.io) | **Packages:** [remi.conary.io](https://remi.conary.io) | **Discussions:** [GitHub Discussions](https://github.com/FieldmouseWorks/Conary/discussions)

Conary is a cross-distro package manager for Linux, written in Rust. It
installs RPM, DEB, Arch, and native CCS packages on Fedora, Ubuntu, and Arch
hosts without invoking dnf, apt, or pacman: an RPM keeps RPM lifecycle and
dependency semantics on Ubuntu or Arch, a DEB keeps Debian semantics on Fedora
or Arch, and an Arch package keeps ALPM semantics on Fedora or Ubuntu. Each
changeset is one atomic, recorded transaction over content-addressed storage,
with immutable system generations for rollback. An install that first pulls
in missing dependencies is not yet one unit: the dependency batch commits as
its own changeset before the requested package is applied
([#917](https://github.com/FieldmouseWorks/Conary/issues/917)).

It is for people who want packages from another distribution's repositories
on the host they already run, without a container or a rebuild, and who are
willing to test pre-alpha software on a machine they can throw away.

Conary is a [Fieldmouse Works](https://github.com/FieldmouseWorks) project.
It is inspired by the
[original Conary](https://en.wikipedia.org/wiki/Conary_(package_manager))
from rPath, but it is an independent project. It is not affiliated with,
endorsed by, or maintained by rPath, SAS, or the original Conary developers.

## Status

Conary is still early. Expect failures.

Conary Preview is a rollback-first package bridge for installing proven RPM,
DEB, and Arch packages across Fedora, Ubuntu, and Arch. It is not a
full-system replacement for apt, dnf, or pacman, and it is not ready to run a
critical system unattended. Use a VM or disposable host first.

| Channel | Current state | Authority |
| --- | --- | --- |
| Development head | Root [`Cargo.toml`](Cargo.toml) `[workspace.package]` version | Repository source authority |
| Latest published, artifact-verified release | [Latest immutable GitHub release](https://github.com/FieldmouseWorks/Conary/releases/latest) | [Release artifact matrix](docs/operations/release-artifact-matrix.md) |
| Current external tester pin | **None** | [Launch status](docs/roadmaps/launch-status.json) |

The latest release is checksummed, attested, signed where the matrix says so,
and proven on all three supported hosts. No release is assigned as external
tester authority yet: the signed public package universe, the daily-driver
floor, a synchronized public release, and performance proof are still open.
The machine-readable [launch status](docs/roadmaps/launch-status.json) owns
those gates; the [agent-assisted tester loop](docs/guides/agent-assisted-tester-loop.md)
stays paused until it names a pinned release.

If you hit a failure, capture the command, distro, package name, Conary
version, source package format, and exact error, then open an issue. Attach
only a reviewed support bundle; never a live database or trust directory.

## Try It

Supported hosts are Fedora 44, Ubuntu 26.04 LTS, and Arch Linux on x86_64.

The signed bootstrap installer downloads the latest release manifest, verifies
its Ed25519 signature against the embedded release key, selects the native
package for your host, verifies that package, and prints the plan. Nothing is
installed without `--apply --yes`. Do not pipe it into a shell.

```bash
curl --proto '=https' --tlsv1.2 -fLO https://conary.io/install-conary-preview.sh
less install-conary-preview.sh
bash ./install-conary-preview.sh
bash ./install-conary-preview.sh --apply --yes
```

### Runnable today

The native package runs `conary system init`, which records host capabilities
and Remi feed definitions but no installed-package providers, and
`apps/conary/src/commands/install/dep_resolution.rs` resolves dependencies
only from Conary's persisted provider graph or repository metadata. So adopt
the host's installed packages first, with `--full` so their files are
CAS-backed and present in the selected root; they become the dependency
source and the lifecycle environment for a local artifact:

```bash
sudo conary system adopt --system --full --dry-run
sudo conary system adopt --system --full
sudo conary install ./package.rpm --dry-run   # .deb and .pkg.tar.zst work the same way
sudo conary install ./package.rpm --yes
sudo conary list <name> --info
sudo conary remove <name> --yes
```

The artifact's dependencies must be satisfied by the adopted packages; with
no active public universe there is nothing else to resolve them from. Once a
generation is selected, unadoption goes through
`conary system native-handoff`, not `conary system unadopt`.

### Paused: the Remi-backed tester loop

The bounded cross-distro loop that installs a repository package from another
distribution's feed is owned by the
[agent-assisted tester loop](docs/guides/agent-assisted-tester-loop.md) and is
paused. It is not runnable on a fresh install today: no complete
Fedora/Ubuntu/Arch universe is active, so Remi refuses package reads until
[#598](https://github.com/FieldmouseWorks/Conary/issues/598) activates one
(see the Remi row of
[docs/roadmaps/development-roadmap.md](docs/roadmaps/development-roadmap.md)),
and [launch status](docs/roadmaps/launch-status.json) has not pinned a tester
release. Once both change, the loop is:

```bash
sudo conary repo sync  # every enabled Remi feed; they are named remi-<profile>
source=ubuntu-26.04  # on Fedora or Arch; use fedora-44 on Ubuntu
sudo conary install htop --from "$source" --dry-run
sudo conary install htop --from "$source" --yes
sudo conary list htop --info
sudo conary query depends htop
sudo conary update htop --dry-run
sudo conary remove htop --yes
```

Commands that change packages, files, generation state, or native authority
require `--yes` unless run with `--dry-run`. The command-risk policy in
`apps/conary/src/command_risk.rs` authorizes these state-changing commands
without it: `conary self-update` (the `--check` and verify forms are
read-only), `conary try keep`, `conary try rollback`, and
`conary try --activate`, which carry apply intent themselves;
`conary system adopt` in its `--system`, package, and `--refresh` forms,
which change Conary's tracking records rather than host files; the root-only
`conary system adopt --refresh --quiet --from-sync-hook` native
package-manager hook; the hidden boot-time `conary system generation activate`
continuation, authorized by the selected generation artifact and kernel
command line; and the local-state class (`conary repo`, `conary pin`,
`conary unpin`, `conary system init`, and similar), which is not gated even
where a member writes host files, as `conary config restore` does. Use
`--dry-run` first when the command supports it. The
default `conary --help` shows the daily-driver commands;
`conary --help-advanced` and
[docs/guides/advanced-commands.md](docs/guides/advanced-commands.md) list the
packaging and platform surface.

### What works today

- Source-independent installation, update, and removal of RPM, DEB, and Arch
  artifacts through typed native lifecycle, dependency, payload, and
  configuration contracts.
- Native package adoption and non-destructive unadoption.
- CCS packages, and Remi-converted packages from a
  [self-hosted Remi](docs/guides/self-hosted-remi.md) or, once #598 activates
  it, the public universe, with Conary as package authority.
- Atomic per-changeset transactions, transaction history, and
  rollback-oriented state tracking. A complete install with missing
  dependencies is two changesets, not one
  ([#917](https://github.com/FieldmouseWorks/Conary/issues/917)).
- Immutable EROFS/composefs generations on hosts with the needed kernel and
  tooling, with raw, qcow2, and x86_64 UEFI ISO export.
- Signed self-update from the published release channel.

### What will break

- A package that needs a target capability the host does not provide fails
  preflight before it mutates anything; dependencies installed ahead of it
  stay installed ([#917](https://github.com/FieldmouseWorks/Conary/issues/917)).
- Source lifecycle forms outside the implemented RPM, Debian, or ALPM ABI are
  bugs to model and test; Conary does not guess behavior from script text.
- Remi's first complete signed public universe is still blocked on
  [#598](https://github.com/FieldmouseWorks/Conary/issues/598), so a package
  may fail to convert even when the upstream package exists.
- Generation boot and export proof is x86_64 only.
- Releases publish checksums, attestations, and detached CCS and bootstrap
  signatures, but no SBOM or provenance sidecars.
- conaryd and federation are outside the reliable preview path.

## How It Works

The source package format defines the package ABI; Conary owns install,
update, remove, and rollback; the target supplies an explicitly inventoried
set of typed host capabilities.
The primary adoption path is cross-distro package installation. For packages
already owned by dnf, apt, or pacman, adoption remains available as a
reversible migration bridge; those native package managers are not runtime
authority for Conary-owned operations.

A native RPM, DEB, or Arch install applies in this order:

1. Download the artifact and parse it into a lossless source-authority record
   with its typed lifecycle bundle
   (`apps/conary/src/commands/install/command.rs`).
2. Resolve dependencies with a SAT solver against typed provides. Missing
   dependencies are installed first as their own changeset, which commits
   before the requested package is touched and stays committed if a later
   step fails (`apps/conary/src/commands/install/dependencies.rs`,
   [#917](https://github.com/FieldmouseWorks/Conary/issues/917)).
3. Take the runtime lock, plan conflicts and replacements, extract the
   payload, prepare an isolated selected root, and preflight the typed
   lifecycle plan against that root and the host capability inventory. A
   failure here stops before the requested package mutates anything
   (`command.rs`).
4. Inside that root and one SQLite transaction, run the lifecycle scriptlets,
   payload, config decisions, and triggers, then bind the selected-root
   snapshot. Any failure rolls back the transaction and discards the root
   (`apps/conary/src/commands/install/batch/execution.rs`).
5. Commit SQLite, then publish and select the recorded generation. A
   publication failure after commit leaves typed debt for deterministic
   retry.

`--dry-run` plans dependencies and relations without the lock, prints the
plan, and returns before payload extraction, root preparation, and lifecycle
preflight, so a scriptlet or capability failure can still surface on the real
run.

The rollback boundary is documented under "Composefs-native transactions" in
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). The lifecycle contract itself is
[docs/specs/foreign-package-lifecycle-contracts.md](docs/specs/foreign-package-lifecycle-contracts.md).

Conary is a virtual Rust workspace:

- `apps/conary`: package-manager CLI.
- `crates/conary-core`: parsers, repositories, resolver, transactions,
  scriptlets, database, CAS, generations, and CCS. Internal crate, not a
  stable external API.
- `apps/remi`: package conversion and serving service.
- `apps/conaryd`: local daemon with Unix-socket REST/SSE routes.
- `apps/conary-test`: integration harness that runs the same lifecycle in
  Podman containers per supported distro, plus QEMU suites for generation boot
  and export.
- `crates/conary-bootstrap`, `crates/conary-agent-contract`, and
  `crates/conary-mcp`: shared runtime and agent-integration crates.

## Remi

Remi is Conary's conversion and package-serving service at
[remi.conary.io](https://remi.conary.io). It authenticates Fedora, Ubuntu, and
Arch repositories with pinned trust roots, converts their packages into CCS
artifacts, and serves them as a signed catalog. Its first complete immutable
public universe is the current #598 gate; a liveness response alone is not
package-serving readiness. See [docs/modules/remi.md](docs/modules/remi.md)
and [docs/guides/self-hosted-remi.md](docs/guides/self-hosted-remi.md).

## Build From Source

Conary requires Rust 1.98.0+ on Linux.

```bash
git clone https://github.com/FieldmouseWorks/Conary.git
cd Conary
cargo build -p conary
sudo ./target/debug/conary system init
```

For an isolated non-root development database, pass a writable `--db-path`;
subsequent commands must use the same path.

Repository gates:

```bash
cargo test -p conary
cargo test -p conary-core
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
bash scripts/check-doc-truth.sh
```

`scripts/check-doc-truth.sh` runs in CI and fails when this README, the
roadmap, the module docs, or the public site claim something the source tree
does not do.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) for the issue, branch, and pull-request
flow, and [AGENTS.md](AGENTS.md) if you work with a coding agent; both are
welcome. Security reports go through
[private advisories](SECURITY.md), not public issues.

- [ROADMAP.md](ROADMAP.md): direction and current milestone.
- [CHANGELOG.md](CHANGELOG.md): release history.
- [docs/INTEGRATION-TESTING.md](docs/INTEGRATION-TESTING.md): integration harness and suites.
- [docs/SCRIPTLET_SECURITY.md](docs/SCRIPTLET_SECURITY.md): scriptlet sandboxing and policy.
- [GitHub Discussions](https://github.com/FieldmouseWorks/Conary/discussions): questions and ideas.

## License

The Conary client, daemon, test harness, and every library crate are licensed under either the [MIT License](LICENSE-MIT) or the [Apache License, Version 2.0](LICENSE-APACHE), at your option. The Remi server (`apps/remi`) is licensed under the [GNU Affero General Public License, version 3 or later](apps/remi/LICENSE). Releases published before this split were MIT only.
