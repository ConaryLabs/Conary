---
last_updated: 2026-07-26
revision: 18
summary: Track the post-hard-cut release suite and the proof required before its artifacts become preview-supported
---

# Release Artifact Matrix

This matrix is the limited-preview artifact contract. It distinguishes a
versioned release target from a published and independently verified release.
Artifact URLs, checksums, signatures, deployments, and runtime behavior become
authority only after their exact evidence is recorded here.

Remote Forge validation is paused until a KVM-capable runner replaces the old
VPS host. Local QEMU/KVM evidence may support a preview row only when it names
the absolute run date, distro, suite, and pass counts.

## Current Release Target

The issue #86 release suite targets `v0.13.0`, `remi-v0.8.0`,
`conaryd-v0.7.0`, and `conary-test-v0.9.0` from one reviewed release commit.
All four rows are **pending publication**. Their candidate URLs must not be
treated as downloads until the corresponding GitHub release exists and its
assets pass the evidence gate below.

The preceding release proof is available through Git history. It is not
current artifact authority after the owned manifests move to this suite.
External outreach remains postponed until the Conary and Remi rows are
published, deployed where applicable, and independently verified.

| Product | Artifact classes | Release workflow | Source authority | Candidate release URL | Required evidence | Preview support | Known caveats | Source-build fallback |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `conary` | binary, `.ccs`, `.rpm`, `.deb`, `.pkg.tar.zst` | `.github/workflows/release-build.yml`, `scripts/release.sh conary`, `scripts/release-matrix.sh` | pending reviewed commit and annotated `v0.13.0` tag | https://github.com/ConaryLabs/Conary/releases/tag/v0.13.0 | pending: terminal merge and release CI; exact tag/commit proof; complete asset inventory; independent checksums and GitHub digests; CCS signature verification; installed-binary identity and self-update; RPM/DEB/Arch package smoke; one exact cross-distro lifecycle loop; exact-tag site and Remi deployment proof; SBOM/provenance status | closed pending evidence | pre-alpha; use only on a disposable VM or non-critical host; no artifact is supported before this row records proof | `cargo build -p conary` |
| `remi` | binary and deploy bundle | `.github/workflows/release-build.yml`, `scripts/release.sh remi`, `scripts/release-matrix.sh` | pending reviewed commit and annotated `remi-v0.8.0` tag | https://github.com/ConaryLabs/Conary/releases/tag/remi-v0.8.0 | pending: terminal release CI; exact tag/commit proof; independent checksums and GitHub digests; installed-binary identity; deployment snapshot/retirement/repopulation result; pre/post full health; public-route and exact CCS proof; signature status; SBOM/provenance status | closed pending evidence | deployment intentionally retires the previous schema epoch and must prove rollback safety; no detached binary signature is expected unless publication says otherwise | `cargo build -p remi` |
| `conaryd` | binary and tarball | `.github/workflows/release-build.yml`, `scripts/release.sh conaryd`, `scripts/release-matrix.sh` | pending reviewed commit and annotated `conaryd-v0.7.0` tag | https://github.com/ConaryLabs/Conary/releases/tag/conaryd-v0.7.0 | pending: terminal release CI; exact tag/commit proof; asset inventory; independent checksums and GitHub digests; binary identity; signature status; SBOM/provenance status | release artifact only; no deployment | Forge staging remains paused; package jobs retain the CLI live-mutation acknowledgement boundary | `cargo build -p conaryd` |
| `conary-test` | binary and tarball | `.github/workflows/release-build.yml`, `scripts/release.sh conary-test`, `scripts/release-matrix.sh` | pending reviewed commit and annotated `conary-test-v0.9.0` tag | https://github.com/ConaryLabs/Conary/releases/tag/conary-test-v0.9.0 | pending: terminal release CI; exact tag/commit proof; asset inventory; independent checksums and GitHub digests; binary identity and suite inventory; signature status; SBOM/provenance status | release artifact only; no deployment | QEMU/KVM suites require a capable local host while remote validation is paused | `cargo build -p conary-test` |

Deploy-helper artifact publication uses CI-produced trust inputs as evidence:
`conary-remi-deploy deploy-conary` verifies staged `SHA256SUMS` before
installing release files, copies the verified checksum file into the installed
release directory, refuses symlinked trust inputs, and requires a sibling
`.ccs.sig` whenever a staged `.ccs` artifact is present. This does not make a
candidate release preview-supported without the recorded proof above.

## Evidence Command Block

Run these commands before publication and again where the proof depends on the
published tag:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p conary-test -- list
bash scripts/check-doc-truth.sh
bash scripts/check-release-matrix.sh
bash scripts/release-cargo-audit.sh
```

For each published release, also record:

- annotated tag object and peeled commit;
- publication time and complete asset names;
- workflow run IDs and terminal conclusions;
- independently downloaded asset SHA-256 values and matching GitHub digests;
- binary `--version` output from the downloaded artifact;
- signature, SBOM, and provenance status;
- deployment and live-behavior proof required by that product row.

## Support Loop

First-wave tester instructions link the support-bundle command,
`.github/ISSUE_TEMPLATE/pre_alpha_feedback.md`, this matrix, and the evidence
command block.

The support bundle is local-only. On an installed host, run `sudo -v` first;
the script uses cached authorization only for allowlisted database-backed
diagnostics and stops before writing a bundle if authorization is unavailable.
Review the result before attaching it. Do not include `/etc/conary/trust`,
private keys, SSH keys, host-local credential files, raw logs, environment
dumps, or live `conary.db` files unless a maintainer explicitly asks for a
separately reviewed follow-up.
