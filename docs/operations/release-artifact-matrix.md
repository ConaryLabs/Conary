---
last_updated: 2026-07-17
revision: 6
summary: Track limited-preview artifact, provenance, and source-build expectations
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

| Product | Artifact classes | Release workflow | Source commit | Binary download or package URL | Required evidence | Preview support | Known caveats | Source-build fallback |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `conary` | binary, `.ccs`, `.rpm`, `.deb`, `.pkg.tar.zst`, or source build | `.github/workflows/release-build.yml`, `scripts/release.sh conary`, `scripts/release-matrix.sh` | `v0.11.1` | https://github.com/ConaryLabs/Conary/releases/tag/v0.11.1 | checksums and CCS signature: release-build publication pending; the immutable `v0.11.0` tag produced no artifacts because release-build `29538737592` failed on Rust 1.97 Clippy, and `v0.11.1` carries only that compatibility fix; SBOM/provenance decision: not published for `v0.11.1`, retained as an explicit limited-preview caveat; pre-tag evidence: compatible Remi commit `c001f8d69b9e8ef34fba39139576a9809800a9a6`, exact deployed binary SHA-256 `c955a24ff6b90f98ba5f20b37e6a67b79bdde199ec0dcbfac0ce78b001d0f485`, public health 10/10, conversion-version-6 prewarm state 11 public and 1 private-review, successful rollback/redeploy rehearsal, and Fedora 44 clean-host `htop 3.4.1` install/run/remove; installed-binary self-update proof pending | tagged release candidate | early preview; artifact workflow and installed-binary self-update must pass before tester launch; package installs can fail while Remi scriptlet review/adapters mature; native PM remains authoritative for adopted packages; Remi first-use conversion can be slow; no SBOM/provenance sidecars | `cargo build -p conary` remains available for source rebuilds |
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

The support bundle is local-only. Review it before attaching it to an issue.
Do not include `/etc/conary/trust`, private keys, SSH keys, host-local
credential files, raw logs, environment dumps, or live `conary.db` files unless
a maintainer explicitly asks for a separately reviewed follow-up.
