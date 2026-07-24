---
last_updated: 2026-07-24
revision: 16
summary: Record the published v0.12.0 and remi-v0.7.1 artifact proof contracts
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

Immutable `v0.12.0` carries the safe in-root system-symlink handling exposed by
issue #41 and the repaired installed-host support bundle. Remediation merge
validation `30041401268` and exact release-commit validation `30042990554`
each passed all 11 jobs. Release-build `30043930486`, exact-tag
deploy-and-verify `30047027525`, independent artifact/signature checks,
installed-binary self-update, supported Arch-path proof, and live-service
checks all passed. Manual outreach remains postponed; a replacement schedule
still requires GitHub Support cached-history dereferencing and fresh venue
checks.

Canonical `remi-v0.7.1` is the current maintainer-operated service release.
Release-commit validation `30073358549`, exact-tag release-build
`30074239104`, production deploy-and-verify `30076168995`, independent asset
downloads, installed-binary identity, and pre/post-reconciliation live-service
checks all passed. The AppArmor normalization-v2 inventory contained zero
active or historical production clusters, so dry run, apply, and repeat apply
completed as verified no-ops while preserving both queue fingerprints and a
representative public-sanitized packet.

| Product | Artifact classes | Release workflow | Source commit | Binary download or package URL | Required evidence | Preview support | Known caveats | Source-build fallback |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `conary` | binary, `.ccs`, `.rpm`, `.deb`, `.pkg.tar.zst`, or source build | `.github/workflows/release-build.yml`, `scripts/release.sh conary`, `scripts/release-matrix.sh` | annotated tag object `8411169b40d8523ee716518cb3dc3e51acddb019` peels to commit `eb256b19b4f04ca1d03b6af39a2819d746d3a22a` | https://github.com/ConaryLabs/Conary/releases/tag/v0.12.0 | published immutable at `2026-07-23T21:39:20Z`; remediation merge CI `30041401268` and release-commit CI `30042990554` passed 11/11 jobs, release-build `30043930486` passed, and exact-tag deploy-and-verify `30047027525` passed; seven assets published; checksums: all five `SHA256SUMS` payloads and all seven REST digests matched independent downloads; signature status: the official preceding binary verified the detached CCS signature; an isolated schema-77 self-update reached `v0.12.0` and then reported current; the exact Arch package binary initialized profile `arch`, synchronized 15,462 Arch-only rows, installed/executed/removed `htop 3.5.2-1`, accepted `/usr/lib64 -> lib` while normalizing the stored path, and rejected an out-of-root symlink without writing through it; support-bundle self-tests and an isolated integrity/profile bundle passed without including the database; Remi health passed 10/10, six public routes returned HTTP 200, and the deployed CCS matched release hash `c973fb654b67da0619d6837b34e2f5f78bbea90dfd9fb8de19b6edf9cbe9582a`, size `16183371`, and signature; SBOM/provenance status: not published or planned | published limited-preview baseline; broad outreach remains postponed through the separate cached-history and venue gates | early preview; package installs can fail while Remi scriptlet review/adapters mature; native PM remains authoritative for adopted packages; the Arch proof used the exact released package binary in an isolated writable root with live-host pacman evidence read-only, not native `pacman -U` or a pristine VM; this host had no installed root-owned Conary database, so the successful cached-sudo bundle path remains regression-tested rather than live-proven here; Fedora-form proof from the preceding release remains a conaryOS `minimal-boot-v4` baseline, not literal stock Fedora native-PM onboarding; Remi first-use conversion can be slow; no SBOM/provenance sidecars | `cargo build -p conary` remains available for source rebuilds |
| `remi` | binary, deploy bundle, or source build | `.github/workflows/release-build.yml`, `scripts/release.sh remi`, `scripts/release-matrix.sh` | annotated tag object `d21eef4349b5b694c7ee66039524e5925bba0f41` peels to commit `301985230834112c8e08ad016921374b43dd8d2e` | https://github.com/ConaryLabs/Conary/releases/tag/remi-v0.7.1 | published immutable at `2026-07-24T07:39:02Z`; release-commit CI `30073358549` passed 11/11 jobs, exact-tag release-build `30074239104` passed, and production deploy-and-verify `30076168995` passed its real `deploy-remi` lane; checksums: all three REST digests matched independent downloads: `metadata.json` SHA-256 `9bc6f722c368acceacdb1b3fd4a024a9317e00ea3597c5f4155260f6bb83f310`, standalone binary `28be831a930aa0a43c1612e080fc4ee6e65d519dd55ba5b68128d1bc01bd358c`, and tarball `eabd6f404c15b6c2a86af9278e0ba7c06ff415dfdfffe657b6a520cbff3c254c`; the standalone, unpacked, and installed binaries matched and reported `remi 0.7.1`; full public health passed 10/10 before deploy, after deploy, and after reconciliation; production contained zero active or historical AppArmor clusters, the bounded dry run preserved the 932-cluster / 3,988-attempt active fingerprint and 1,020-cluster / 5,996-attempt historical fingerprint, explicit apply and repeat apply each scanned zero with zero remaining, and a representative public-sanitized packet retained its canonical JSON fingerprint while exposing empty incoming/outgoing reconciliation links; deterministic fixtures cover identity, rekey, split, merge, conflicting-state preservation, lineage, and private-output behavior for non-empty populations; signature status: no detached binary signature is published; SBOM/provenance status: not published | published maintainer-operated service binary; service operator preview remains maintainer-led | admin origin must stay explicit; no detached signature or SBOM/provenance sidecars; expected clean-VM build time and wider operator guidance remain required before an operator tester post | `cargo build -p remi` remains available for source rebuilds |
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
