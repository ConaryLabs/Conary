---
last_updated: 2026-07-18
revision: 10
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

The `v0.11.2` publication gate was reopened for `v0.11.3` after the supported
`htop` path exposed unreachable generic SONAME evidence; review also found an
inexact or ABI-unchecked critical-library fallback and discarded Arch
capability constraints. The replacement release is a candidate only until the
exact tag, workflow runs, hashes, native-package onboarding, and
installed-binary self-update evidence below are complete.

| Product | Artifact classes | Release workflow | Source commit | Binary download or package URL | Required evidence | Preview support | Known caveats | Source-build fallback |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `conary` | binary, `.ccs`, `.rpm`, `.deb`, `.pkg.tar.zst`, or source build | `.github/workflows/release-build.yml`, `scripts/release.sh conary`, `scripts/release-matrix.sh` | candidate `v0.11.3`; exact annotated tag and commit pending | https://github.com/ConaryLabs/Conary/releases/tag/v0.11.3 (pending publication) | candidate repair requires exact SONAME cache matches, compatible ELF class, and constraint-aware Arch capability proof on the supported `htop` path; pending release evidence: exact tag/commit, release-build and deploy-and-verify run IDs, seven published assets, independent matches for all five `SHA256SUMS` payloads, detached CCS signature verification, profile-correct released native-package initialization and Remi synchronization, real update from the preceding official binary to `v0.11.3`, full Remi health, and checked public-site deployment; the prior release's checksum, signature, native-onboarding, self-update, service, and site evidence remains a superseded baseline only; no SBOM/provenance sidecars are planned for this limited preview and their absence remains explicit | candidate only; not published or preview-verified; outreach remains gated by completed release evidence, GitHub Support cached-history dereferencing, and venue checks | early preview; package installs can fail while Remi scriptlet review/adapters mature; native PM remains authoritative for adopted packages; Remi first-use conversion can be slow; no SBOM/provenance sidecars | `cargo build -p conary` remains available for source rebuilds |
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
