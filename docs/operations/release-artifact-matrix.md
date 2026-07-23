---
last_updated: 2026-07-23
revision: 12
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

The `v0.11.2` publication gate was reopened for `v0.11.3` after the supported `htop` path
exposed unreachable generic SONAME evidence; review also found an inexact or
ABI-unchecked critical-library fallback and discarded Arch capability
constraints. `v0.11.3` replaces that release with exact SONAME cache matches,
compatible ELF-class checks, and constraint-aware Arch capability proof. Its
publication, native-package onboarding, installed-binary self-update, service,
and site evidence are complete. Manual outreach remains unlaunched and is
now postponed: issue #41 exposed an in-root system-symlink false-positive, and
its support bundle exposed unusable unprivileged diagnostics against an
installed root-owned database. A replacement schedule requires those fixes to
ship with refreshed supported-path evidence, plus GitHub Support cached-history
dereferencing and fresh venue checks.

| Product | Artifact classes | Release workflow | Source commit | Binary download or package URL | Required evidence | Preview support | Known caveats | Source-build fallback |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `conary` | binary, `.ccs`, `.rpm`, `.deb`, `.pkg.tar.zst`, or source build | `.github/workflows/release-build.yml`, `scripts/release.sh conary`, `scripts/release-matrix.sh` | annotated tag object `a2a12791e695379e9313a210d2fd5eea2a39b352`, peeled commit `0fc31c33b42a84bb00c9c8d9bdfc574ebe960ae0` | https://github.com/ConaryLabs/Conary/releases/tag/v0.11.3 | published immutable at `2026-07-18T04:31:28Z`; final merge CI `29628990277` passed 11/11 jobs, release-build `29629361456` passed, and exact-tag deploy-and-verify `29630694438` passed for both sites; seven assets published; checksums: independent downloads matched all five `SHA256SUMS` payloads and all REST asset digests; the detached CCS signature verified with the official `v0.11.2` binary; an isolated schema-77 self-update from that binary to `v0.11.3` verified the signature and current-version result; released Arch and Fedora-form package paths installed, executed, and removed `htop` while exact SONAME/version/ELF evidence remained fail-closed; Remi health passed 10/10, six public routes returned HTTP 200, and the deployed API CCS matched the release hash, size, and signature; no SBOM/provenance sidecars are published or planned for this limited preview | published immutable baseline; broad outreach is postponed until the issue #41 path fix and installed-host support diagnostics ship with refreshed supported-path evidence, GitHub cached-history is dereferenced, and venue checks pass | early preview; `v0.11.3` can falsely reject a payload beneath a legitimate in-root system symlink such as Arch `/usr/lib64 -> lib`; package installs can fail while Remi scriptlet review/adapters mature; native PM remains authoritative for adopted packages; Arch live-package proof used an isolated `bwrap` target with host evidence exposed read-only, and Fedora-form proof used `minimal-boot-v4`, which lacks `rpm`/`dnf`, rather than a literal stock Fedora native-PM install; Remi first-use conversion can be slow; no SBOM/provenance sidecars | `cargo build -p conary` remains available for source rebuilds |
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
