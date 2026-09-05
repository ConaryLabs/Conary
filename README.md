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
or Arch, and an Arch package keeps ALPM semantics on Fedora or Ubuntu. Every
change is one atomic, recorded transaction over content-addressed storage,
with immutable system generations for rollback.

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

The native package runs `conary system init` on install, which creates the
root-owned database and seeds the default repositories, including Remi. Then
pick a source whose package format differs from the host and run the bounded
loop:

```bash
source=ubuntu-26.04  # on Fedora or Arch; use fedora-44 on Ubuntu
sudo conary install htop --from "$source" --dry-run
sudo conary install htop --from "$source" --yes
sudo conary list htop --info
sudo conary query depends htop
sudo conary update htop --dry-run
sudo conary remove htop --yes
```

A local RPM, DEB, or Arch artifact works on any supported host:

```bash
sudo conary install ./package.rpm --dry-run
sudo conary install ./package.deb --dry-run
sudo conary install ./package.pkg.tar.zst --dry-run
```

Preflight validates the package's declared capabilities against the target
before anything is mutated. An unsupported declaration fails there; there is
no capability-approval bypass.

To test reversible adoption of packages the host's native package manager
already owns:

```bash
sudo conary system adopt --system --dry-run
sudo conary system adopt --system
sudo conary system unadopt --all --yes
```

Commands that change packages, files, generation state, or native authority
require `--yes`. Use `--dry-run` first when the command supports it. The
default `conary --help` shows the daily-driver commands;
`conary --help-advanced` and
[docs/guides/advanced-commands.md](docs/guides/advanced-commands.md) list the
packaging and platform surface.

### What works today

- Source-independent installation, update, and removal of RPM, DEB, and Arch
  artifacts through typed native lifecycle, dependency, payload, and
  configuration contracts.
- Native package adoption and non-destructive unadoption.
- CCS packages and Remi-converted packages with Conary as package authority.
- Atomic changesets, transaction history, and rollback-oriented state
  tracking.
- Immutable EROFS/composefs generations on hosts with the needed kernel and
  tooling, with raw, qcow2, and x86_64 UEFI ISO export.
- Signed self-update from the published release channel.

### What will break

- A package that needs a target capability the host does not provide fails
  preflight before mutation.
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

An install runs through one pipeline regardless of source format:

1. Parse the RPM, DEB, or Arch artifact into a lossless source-authority
   record and convert it to a CCS lifecycle bundle.
2. Resolve dependencies with a SAT solver against typed provides, and match
   source requirements against the typed host capability inventory.
3. Preflight the exact lifecycle plan: stages, arguments, triggers, and
   payload boundaries in source-ABI order.
4. Commit the transaction: store content in the CAS, record the changeset in
   SQLite, build the generation artifact, and select it.
5. Execute scriptlets and triggers inside an isolated selected root.

The full contract is in
[docs/specs/foreign-package-lifecycle-contracts.md](docs/specs/foreign-package-lifecycle-contracts.md)
and the subsystem map in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

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
