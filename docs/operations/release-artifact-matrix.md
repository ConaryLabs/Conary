---
last_updated: 2026-07-27
revision: 19
summary: Record the verified post-hard-cut release suite and its limited-preview evidence
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

Issue #86 produced a split immutable lineage rather than pretending that four
products published at different times came from one commit:

- `v0.13.0` and the current `remi-v0.8.5` peel to
  `6f1429c362ac161f1ef817233e72ee9c9a031c11`;
- `conaryd-v0.7.0` and `conary-test-v0.9.0` peel to
  `a231276a900bbe8a8ccb6a0942f104cba2ab86b4`;
- published Remi 0.8.0 through 0.8.4 releases remain immutable recovery
  evidence, but none is current production authority.

Conary and Remi are deployed and independently identified below. Conary's
published native-package lifecycle matrix passed on all three supported hosts,
so the bounded pre-alpha preview is open. Broad outreach remains separately
postponed.

| Product | Artifact classes | Release workflow | Source authority | Release URL | Required evidence | Preview support | Known caveats | Source-build fallback |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `conary` | binary, `.ccs`, `.rpm`, `.deb`, `.pkg.tar.zst` | `.github/workflows/release-build.yml`, `scripts/release.sh conary`, `scripts/release-matrix.sh` | annotated tag object `f8298522fd7fe95a4994184ae20c34cf64096818`, commit `6f1429c362ac161f1ef817233e72ee9c9a031c11` | https://github.com/ConaryLabs/Conary/releases/tag/v0.13.0 | exact publication, checksums, GitHub digests, detached signature, current binary/self-update, deployment, endpoint, site, released-package lifecycle, SBOM, and provenance status recorded below | limited pre-alpha preview | disposable, snapshotted, or explicitly non-critical x86_64 Fedora 44, Ubuntu 26.04 LTS, and Arch hosts only; the intentional schema hard cut requires a fresh native-package install from 0.12 | `cargo build -p conary` |
| `remi` | binary and deploy bundle | `.github/workflows/release-build.yml`, `scripts/release.sh remi`, `scripts/release-matrix.sh` | annotated tag object `1d9b8588fe01453bab20f1a7956e4aa9d6263702`, commit `6f1429c362ac161f1ef817233e72ee9c9a031c11` | https://github.com/ConaryLabs/Conary/releases/tag/remi-v0.8.5 | exact publication, checksums, installed identity, signature, protected deployment, current schema/source/signing state, conversions, full health, public converted CCS, SBOM, and provenance status recorded below | live operator service | conversion defects are explicit engineering work in #98, #99, and #102 through #105; unknown `/v1` paths currently receive the SPA fallback instead of an API 404 and are tracked in #67 | `cargo build -p remi` |
| `conaryd` | binary and tarball | `.github/workflows/release-build.yml`, `scripts/release.sh conaryd`, `scripts/release-matrix.sh` | annotated tag object `381ee55cd051234dd78647ef5d7a8c3250c6b9df`, commit `a231276a900bbe8a8ccb6a0942f104cba2ab86b4` | https://github.com/ConaryLabs/Conary/releases/tag/conaryd-v0.7.0 | exact publication, three-asset inventory, matching independent/GitHub checksums, raw/tar identity, binary version, build-only routing, signature, SBOM, and provenance status recorded below | release artifact only; no deployment | Forge staging remains paused; package jobs retain the CLI live-mutation acknowledgement boundary | `cargo build -p conaryd` |
| `conary-test` | binary and tarball | `.github/workflows/release-build.yml`, `scripts/release.sh conary-test`, `scripts/release-matrix.sh` | annotated tag object `6c4a8cb4bb2c6e89f359a8ceb89ac40a5ce06890`, commit `a231276a900bbe8a8ccb6a0942f104cba2ab86b4` | https://github.com/ConaryLabs/Conary/releases/tag/conary-test-v0.9.0 | exact publication, three-asset inventory, matching independent/GitHub checksums, raw/tar identity, binary version, suite inventory, build-only routing, signature, SBOM, and provenance status recorded below | release artifact only; no deployment | QEMU/KVM suites require a capable local host while remote validation is paused | `cargo build -p conary-test` |

## Recorded Evidence

### Conary 0.13.0

- The release published at `2026-07-27T11:57:44Z`. Exact-tag
  release-build run `30261256730` passed the workspace gate and all four
  package builders.
- Independent downloads found exactly seven assets. Their SHA-256 values
  matched the GitHub REST digests:
  - `SHA256SUMS`:
    `ce67a7199f8adafec8d1a47a29d5b3ccd0c32068fff0804653251daa35d436c7`
  - `conary-0.13.0-1-x86_64.pkg.tar.zst`:
    `a2905964c1272ab0e7c963102429501fa4ab05a6b569b8b1d1f02b5be4e20997`
  - `conary-0.13.0-1.fc44.x86_64.rpm`:
    `f65adc8cad33e46c26d3aa6dcb0d47a89382857e5ece0f7fe6da7b787d4d76d8`
  - `conary-0.13.0.ccs`:
    `9f6bb7208aaf196a73a482d32aec918cb032857a002c85ecc40c51e4c44bd927`
  - `conary-0.13.0.ccs.sig`:
    `b47bde4d29394558dd0bd15554dc6e79f05776886f0c63807a0c3a644eb01a00`
  - `conary_0.13.0-1_amd64.deb`:
    `c67ea72ac8b8a3d061b6f0bf943fab1bb15258cfc7d6d3d0b0dea9bf2b472b9c`
  - `metadata.json`:
    `386f3ef51846d20186a6a42fda1dc14c8f0bda56db46ba57445974ec2ae26a90`
- All five `SHA256SUMS` entries passed. Metadata names product `conary`,
  version `0.13.0`, tag `v0.13.0`, live `release_bundle` deployment, and
  `dry_run: false`. Extracted RPM and DEB binaries report `conary 0.13.0`;
  the Arch package declares `conary 0.13.0-1 x86_64`.
- The official 0.12 binary and embedded release key verified the detached
  signature over the CCS digest. A copied released 0.13.0 RPM binary then
  initialized an isolated database, reported itself current, forced a signed
  download, verified and extracted the CCS, replaced only its copied binary,
  and again reported `conary 0.13.0` and current.
- The 0.12 binary intentionally cannot parse the current CCS component
  manifest after the schema hard cut. This suite does not restore a legacy
  parser: install the 0.13 native package fresh instead of claiming an
  in-place 0.12-to-0.13 self-update.
- Deploy job `deploy-conary` in run `30263948968` passed. The server's
  `/conary/releases/latest` selects `0.13.0`; all seven deployed hashes match
  the release; the self-update endpoint reports version `0.13.0`, the exact
  CCS hash and size, and a signature. Both sites were built from the exact tag:
  conary.io and its install guide serve `v0.13.0`, while an unknown route
  returns the branded HTTP 404.
- Run `30263948968` completed successfully. Its immutable RPM, DEB, and Arch
  packages installed through the native package managers on Fedora 44, Ubuntu
  26.04 LTS, and Arch; each host passed the one owned Cartesian
  native-cross-source lifecycle suite, and the aggregate
  `release-artifact-proof` job passed.
- No SBOM or provenance sidecars are published. The CCS has both its embedded
  package authority and the separately verified release-digest signature.

### Remi 0.8.5

- Exact-tag release-build run `30259180128` passed, and the release published
  at `2026-07-27T11:09:12Z`.
- Independent downloads found exactly three assets whose SHA-256 values
  matched GitHub: `metadata.json`
  `e036548d3469a1ffd28d8687ffae1fb7b88662d7d284a7cbbe08015dc2f79bc8`,
  raw binary
  `ad949c026578bbadc8f45402d167b577f28fec5c3780839ac27c09d4718f008d`,
  and tarball
  `dfae67f91adc268009bd6f8e128ffd1f8103d82efe92d729da319e88bf3cc42a`.
  Raw and tarred binaries are identical and report `remi 0.8.5`.
- Protected deploy run `30260847616` passed. Independent inspection reports
  schema epoch `conary-current-v1`, revision 21, five configured and populated
  sources, 98,266 repository packages, exact signing profiles `arch`,
  `fedora-44`, and `ubuntu-26.04`, and 2,368 current conversions at the final
  release check.
- Full public health passes 10/10. A Fedora `curl` request returns a converted,
  native-free CCS artifact. No detached binary signature, SBOM, or provenance
  sidecar is published for this release.

### conaryd 0.7.0 and conary-test 0.9.0

- Release-build runs `30225403876` and `30225404886` passed at their shared
  immutable commit; deploy-routing runs `30226301835` and `30226177794`
  selected `no-deploy-required`, as the matrix requires.
- conaryd published at `2026-07-26T23:56:22Z`. Its metadata, raw binary, and
  tarball hashes are respectively
  `a9c902fad9dc9be09608a0d8e3e699fa7178d727822bc507d18bdc503b25d612`,
  `3e1559b13386ddd1effedc96a1f083c200e1b3bed2d062135b5e687143c259b3`,
  and `6bacb205a945150a6bfd4039370d54b5dc5a56599b63a4c51ac9e675fc5bf1f6`.
  Independent downloads match GitHub, raw and tarred binaries are identical,
  and the binary reports `conaryd 0.7.0`.
- conary-test published at `2026-07-26T23:52:48Z`. Its metadata, raw binary,
  and tarball hashes are respectively
  `ba29e17684039588340e58c4363db1c38f064781f58c030c326952c75c8480f9`,
  `1d8a4c7fa78c2e20955fd43c4e8c55ee263b768e8e743a19cbd657667feb5ecc`,
  and `e1476e24c6aa78f079d5374590f034bcd26860be0da98c4fd15ef31a7204f967`.
  Independent downloads match GitHub, raw and tarred binaries are identical,
  the binary reports `conary-test 0.9.0`, and `list` exposes the native
  cross-source lifecycle plus the complete owned suite inventory.
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
