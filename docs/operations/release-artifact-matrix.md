---
last_updated: 2026-07-29
revision: 20
summary: Track the v0.14.0 and Remi 0.9.0 release candidates without treating pending proof as authority
---

# Release Artifact Matrix

This matrix is the limited-preview artifact contract. It distinguishes a
versioned release target from a published and independently verified release.
Artifact URLs, checksums, signatures, deployments, and runtime behavior become
authority only after their exact evidence is recorded here.

Remote Forge validation is paused until a KVM-capable runner replaces the old
VPS host. Local QEMU/KVM evidence may support a preview row only when it names
the absolute run date, distro, suite, and pass counts.

## Current Release Suite

Issue #165 prepares Conary `v0.14.0` and `remi-v0.9.0`. Neither candidate is
current release or production authority yet. Their tag objects, reviewed
commits, workflow runs, assets, checksums, deployment results, and independent
behavior proof remain pending below. The previously verified release suite
continues to serve users until every applicable gate is complete.

The build-only conaryd and conary-test releases retain their existing immutable
lineage. Broad external outreach remains separately postponed at 0/10
qualifying completions.

| Product | Artifact classes | Release workflow | Source authority | Release URL | Required evidence | Preview support | Known caveats | Source-build fallback |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `conary` | binary, `.ccs`, `.rpm`, `.deb`, `.pkg.tar.zst` | `.github/workflows/release-build.yml`, `scripts/release.sh conary`, `scripts/release-matrix.sh` | candidate `v0.14.0`; annotated tag object and reviewed commit pending | https://github.com/ConaryLabs/Conary/releases/tag/v0.14.0 | publication, checksums, GitHub digests, detached signature, current binary/self-update, deployment, endpoint, site, released-package lifecycle, SBOM, and provenance proof all pending | not yet preview-supported | use only after this matrix records complete proof; supported scope will remain disposable, snapshotted, or explicitly non-critical x86_64 Fedora 44, Ubuntu 26.04 LTS, and Arch hosts | `cargo build -p conary` |
| `remi` | binary and deploy bundle | `.github/workflows/release-build.yml`, `scripts/release.sh remi`, `scripts/release-matrix.sh` | candidate `remi-v0.9.0`; annotated tag object and reviewed commit pending | https://github.com/ConaryLabs/Conary/releases/tag/remi-v0.9.0 | publication, checksums, installed identity, signature, protected deployment, current schema/source/signing state, conversions, full health, public converted CCS, SBOM, and provenance proof all pending | not yet production authority | the prior verified deployment remains authoritative until exact release and live proof complete | `cargo build -p remi` |
| `conaryd` | binary and tarball | `.github/workflows/release-build.yml`, `scripts/release.sh conaryd`, `scripts/release-matrix.sh` | annotated tag object `381ee55cd051234dd78647ef5d7a8c3250c6b9df`, commit `a231276a900bbe8a8ccb6a0942f104cba2ab86b4` | https://github.com/ConaryLabs/Conary/releases/tag/conaryd-v0.7.0 | exact publication, checksums, raw/tar identity, binary version, build-only routing, signature, SBOM, and provenance recorded below | release artifact only; no deployment | Forge staging remains paused; package jobs retain the CLI live-mutation acknowledgement boundary | `cargo build -p conaryd` |
| `conary-test` | binary and tarball | `.github/workflows/release-build.yml`, `scripts/release.sh conary-test`, `scripts/release-matrix.sh` | annotated tag object `6c4a8cb4bb2c6e89f359a8ceb89ac40a5ce06890`, commit `a231276a900bbe8a8ccb6a0942f104cba2ab86b4` | https://github.com/ConaryLabs/Conary/releases/tag/conary-test-v0.9.0 | exact publication, checksums, raw/tar identity, binary version, suite inventory, build-only routing, signature, SBOM, and provenance recorded below | release artifact only; no deployment | QEMU/KVM suites require a capable local host while remote validation is paused | `cargo build -p conary-test` |

## Recorded Evidence

### Conary 0.14.0 candidate

- Owned manifests, packaging metadata, the generated man page, site release
  data, tester guide, and changelog are prepared for `v0.14.0` on issue #165.
- The annotated tag object and peeled reviewed commit are pending.
- Exact-tag release-build and publication are pending. No asset inventory,
  checksum, GitHub digest, detached-signature, installed-binary, or forced
  self-update result is claimed.
- Protected deployment, public endpoint identity, exact-tag site content,
  branded HTTP 404, and released native-package lifecycle proof are pending.
- SBOM and provenance status will be recorded from the published asset
  inventory rather than inferred from the candidate tree.

Expected package asset names after successful publication are:

- `conary-0.14.0-1.fc44.x86_64.rpm`
- `conary_0.14.0-1_amd64.deb`
- `conary-0.14.0-1-x86_64.pkg.tar.zst`
- `conary-0.14.0.ccs`
- `conary-0.14.0.ccs.sig`
- `SHA256SUMS`
- `metadata.json`

### Remi 0.9.0 candidate

- The owned manifest and changelog are prepared for `remi-v0.9.0` on issue
  #165.
- The annotated tag object, peeled reviewed commit, exact-tag release-build,
  publication, three-asset inventory, checksums, binary identity, signature,
  SBOM, and provenance status are pending.
- Protected deployment and independent schema, revision, source population,
  signing-profile, conversion, full-health, and public converted-CCS proof are
  pending. The candidate must not replace current production authority before
  those checks pass.

### conaryd 0.7.0 and conary-test 0.9.0

- Release-build runs `30225403876` and `30225404886` passed at their shared
  immutable commit; deploy-routing runs `30226301835` and `30226177794`
  selected `no-deploy-required`, as the matrix requires.
- Independent downloads matched GitHub checksums, raw and tarred binaries were
  identical, and the binaries reported their owned versions. conary-test
  exposed the native cross-source lifecycle and complete owned suite
  inventory.
- Neither build-only product publishes a detached signature, SBOM, or
  provenance sidecar.

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
