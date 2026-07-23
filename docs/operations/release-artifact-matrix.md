---
last_updated: 2026-07-23
revision: 13
summary: Stage the v0.12.0 candidate artifact and proof contract
---

# Release Artifact Matrix

This matrix is the limited-preview artifact contract. It keeps public tester
instructions honest by naming whether each product has published artifacts or
is still source-build-only, and by listing the evidence required before a row
can be treated as preview-supported.

Remote Forge validation is paused until a KVM-capable runner replaces the old
VPS host. Local QEMU/KVM evidence may support a preview row only when it names
the absolute run date, distro, suite, and pass counts.

## Current Preview State

The `v0.12.0` candidate carries the safe in-root system-symlink handling exposed
by issue #41 and the repaired installed-host support bundle. Trusted merge
validation for the remediation commit passed all 11 jobs in run `30041401268`.
The canonical tag, release artifacts, checksums, detached signature,
installed-binary self-update, supported Arch path, deployment, and live-service
evidence remain pending until the tag is pushed and the release pipeline
finishes. Manual outreach remains postponed; a replacement schedule also
requires GitHub Support cached-history dereferencing and fresh venue checks.

| Product | Artifact classes | Release workflow | Source commit | Binary download or package URL | Required evidence | Preview support | Known caveats | Source-build fallback |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `conary` | binary, `.ccs`, `.rpm`, `.deb`, `.pkg.tar.zst`, or source build | `.github/workflows/release-build.yml`, `scripts/release.sh conary`, `scripts/release-matrix.sh` | local annotated `v0.12.0` candidate; record the exact remote tag object and peeled commit after publication | https://github.com/ConaryLabs/Conary/releases/tag/v0.12.0 | remediation merge CI `30041401268` passed 11/11 jobs; checksums: pending; signature status: pending; SBOM/provenance status: not planned; release-build, immutable publication, all asset hashes and REST digests, installed-binary self-update, supported Arch-path install/remove proof, deploy-and-verify, Remi health, public routes, and deployed CCS identity are pending | candidate only; do not treat as preview-supported until every required evidence item passes; broad outreach remains postponed through the separate cached-history and venue gates | early preview; package installs can fail while Remi scriptlet review/adapters mature; native PM remains authoritative for adopted packages; supported Arch proof may use an isolated `bwrap` target with host evidence exposed read-only but must name that caveat; Fedora-form proof may use `minimal-boot-v4`, which lacks `rpm`/`dnf`, but is not a literal stock Fedora native-PM install; Remi first-use conversion can be slow; no SBOM/provenance sidecars are planned | `cargo build -p conary` remains available for source rebuilds |
| `remi` | binary, container/deploy bundle, or source build | `.github/workflows/release-build.yml`, `scripts/release.sh remi`, `scripts/release-matrix.sh` | tester post must pin an exact commit or release tag | source-build-only until service-operator preview artifacts are linked | checksums: pending for binaries; signature status: pending for binaries; SBOM/provenance status: pending for binaries; health check; admin-origin config review | service operator preview | production service operation remains maintainer-led; admin origin must stay explicit | `cargo build -p remi`; expected clean-VM build time must be measured before operator tester post |
| `conaryd` | binary, package artifacts, or source build | `.github/workflows/release-build.yml`, `scripts/release.sh conaryd`, `scripts/release-matrix.sh` | tester post must pin an exact commit or release tag | source-build-only until daemon preview artifacts are linked | checksums: pending for binaries; signature status: pending for binaries; SBOM/provenance status: pending for binaries; Unix-socket auth check; package-job queue smoke | local daemon preview | Forge staging deploy is paused; package jobs keep the CLI live-mutation acknowledgement boundary | `cargo build -p conaryd`; expected clean-VM build time must be measured before daemon tester post |
| `conary-test` | binary, package artifacts, or source build | `.github/workflows/release-build.yml`, `scripts/release.sh conary-test`, `scripts/release-matrix.sh` | tester post must pin an exact commit or release tag | source-build-only until validation-tooling artifacts are linked | checksums: pending for binaries; signature status: pending for binaries; SBOM/provenance status: pending for binaries; suite inventory parse; fixture manifest check | validation tooling | QEMU/KVM suites require a capable local host while remote validation is paused | `cargo build -p conary-test`; expected clean-VM build time must be measured before validation tester post |

Deploy-helper artifact publication uses CI-produced trust inputs as evidence:
`conary-remi-deploy deploy-conary` verifies the staged `SHA256SUMS` file before
installing release files, copies the verified checksum file into the installed
release directory, refuses symlinked trust inputs, and requires a sibling
`.ccs.sig` whenever a staged `.ccs` artifact is present. This does not by itself
make public binary downloads preview-supported; rows remain source-build-only
until concrete artifact URLs or paths are listed above.

## Evidence Command Block

Run these commands from the repository root before publishing a limited-preview
tester post or refreshing artifact status:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p conary-test -- list
bash scripts/check-doc-truth.sh
bash scripts/check-release-matrix.sh
bash scripts/release-cargo-audit.sh
```

For source-build-only rows, add a dated clean-VM build measurement before
linking public tester instructions. The matrix must not imply binaries,
checksums, signatures, SBOMs, or SLSA/provenance sidecars exist until their
URLs or paths are listed here.

## Support Loop

First-wave tester instructions should link all of these:

- Support bundle command: `bash scripts/conary-support-bundle.sh`
- Beta feedback template: `.github/ISSUE_TEMPLATE/beta_feedback.md`
- This release/source expectation matrix
- The evidence command block above

The support bundle is local-only. On an installed host, run `sudo -v` first; the
script uses cached authorization only for allowlisted database-backed
diagnostics and stops before writing a bundle if that authorization is
unavailable. Review the result before attaching it to an issue. Do not include
`/etc/conary/trust`, private keys, SSH keys, host-local credential files, raw
logs, environment dumps, or live `conary.db` files unless a maintainer
explicitly asks for a separately reviewed follow-up.
