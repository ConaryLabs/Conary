---
last_updated: 2026-07-31
revision: 21
summary: Record published Conary 0.14.0 and deployed Remi 0.9.5 while keeping release proof separate from current tester authority
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

Conary `v0.14.0` is an immutable published limited-preview release with exact
asset, deployment, and released-package proof. It is not current
tester-authoritative: supported-host generation bring-up after the tag exposed
product defects whose fixes remain unreleased on issue #137 and PR #151. The
release remains valid evidence for the exact tree it contains; later source
proof does not retroactively change that tree.

Remi `0.9.5` is the current immutable release and production authority. Its
exact release, protected deployment, independent asset identity, and live
health proof are recorded below.

The build-only conaryd and conary-test releases retain their existing immutable
lineage. Broad external outreach remains separately postponed at 0/10
qualifying completions.

| Product | Artifact classes | Release workflow | Source authority | Release URL | Required evidence | Preview support | Known caveats | Source-build fallback |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `conary` | binary, `.ccs`, `.rpm`, `.deb`, `.pkg.tar.zst` | `.github/workflows/release-build.yml`, `scripts/release.sh conary`, `scripts/release-matrix.sh` | annotated tag object `c36c767c7169ff519a96dfdc7bedfa757211f334`, reviewed commit `fe23a604b64ea6f7cc87fce8298911e2245e027f` | https://github.com/ConaryLabs/Conary/releases/tag/v0.14.0 | immutable publication, checksums/GitHub digests, detached CCS signature, deployment, self-update endpoint, three-distro released-package lifecycle, and Fedora sparse-sync proof complete; no SBOM or provenance sidecar published | published limited-preview artifact; not current tester-authoritative after unreleased supported-host fixes | use only on disposable, snapshotted, or explicitly non-critical x86_64 Fedora 44, Ubuntu 26.04 LTS, and Arch hosts; current source fixes are not part of this artifact | `cargo build -p conary` |
| `remi` | binary and deploy bundle | `.github/workflows/release-build.yml`, `scripts/release.sh remi`, `scripts/release-matrix.sh` | annotated tag object `f2bf17f0086a7f8ea4be3e032336551c4e6089c1`, reviewed commit `101dba655257f1ff3d1bee689d9c5ac8b2b68cbd` | https://github.com/ConaryLabs/Conary/releases/tag/remi-v0.9.5 | immutable publication, checksums/GitHub digests, raw/tar identity, installed identity, protected deployment, schema/source/signing state, conversions, full health, and public serving proof complete; no detached signature, SBOM, or provenance sidecar published | current production authority | distribution and a wider stranger-operated path remain limited | `cargo build -p remi` |
| `conaryd` | binary and tarball | `.github/workflows/release-build.yml`, `scripts/release.sh conaryd`, `scripts/release-matrix.sh` | annotated tag object `381ee55cd051234dd78647ef5d7a8c3250c6b9df`, commit `a231276a900bbe8a8ccb6a0942f104cba2ab86b4` | https://github.com/ConaryLabs/Conary/releases/tag/conaryd-v0.7.0 | exact publication, checksums, raw/tar identity, binary version, build-only routing, signature, SBOM, and provenance recorded below | release artifact only; no deployment | Forge staging remains paused; package jobs retain the CLI live-mutation acknowledgement boundary | `cargo build -p conaryd` |
| `conary-test` | binary and tarball | `.github/workflows/release-build.yml`, `scripts/release.sh conary-test`, `scripts/release-matrix.sh` | annotated tag object `6c4a8cb4bb2c6e89f359a8ceb89ac40a5ce06890`, commit `a231276a900bbe8a8ccb6a0942f104cba2ab86b4` | https://github.com/ConaryLabs/Conary/releases/tag/conary-test-v0.9.0 | exact publication, checksums, raw/tar identity, binary version, suite inventory, build-only routing, signature, SBOM, and provenance recorded below | release artifact only; no deployment | QEMU/KVM suites require a capable local host while remote validation is paused | `cargo build -p conary-test` |

## Recorded Evidence

### Conary 0.14.0

- Annotated tag object `c36c767c7169ff519a96dfdc7bedfa757211f334`
  peels to reviewed commit
  `fe23a604b64ea6f7cc87fce8298911e2245e027f`.
- Exact-tag release-build run `30409720307` passed and published the immutable
  release on 2026-07-29. Independent downloads matched `SHA256SUMS` and every
  GitHub digest:
  - `conary-0.14.0-1.fc44.x86_64.rpm`:
    `646d89bd8e6de86e8ec983d7cdba942ccfcd45cecc32a3e3efb69576a62dc09a`
  - `conary_0.14.0-1_amd64.deb`:
    `6cb55e510dfe2578af530ca89054458cc0b61e33870af09245d1f1e6069eae8d`
  - `conary-0.14.0-1-x86_64.pkg.tar.zst`:
    `bab913f539325122ba86fbcc8cdb62dc396657709c55fac78927ae4bbc9d88ec`
  - `conary-0.14.0.ccs`:
    `f38fa623880b383fd9d2b49a0b003d2885a826f78d51c874ef216363cc61b3fd`
  - `conary-0.14.0.ccs.sig`:
    `9b26ddc8496f23005f5c6c3906c956816d215a73d1d789d6e2062a0763d7975d`
  - `SHA256SUMS`:
    `5e7aef32d1dcc31fa5a3779b6cf283704cefa56c4e3e6ab5b842c233e0168c7a`
  - `metadata.json`:
    `dfd5e8bb1add27867ca8a631223ce4d7f5f5c649d6b7207373ddc0ddb665418c`
- Metadata identifies product `conary`, tag `v0.14.0`, version `0.14.0`, and
  `release_bundle` deployment. No SBOM or provenance sidecar is published.
- Protected deploy-and-verify run `30412130145` passed exact release-bundle
  deployment, self-update endpoint and static-site checks, published artifact
  proof, and installed-package lifecycle proof on Fedora 44, Ubuntu 26.04, and
  Arch.
- The released RPM reports `conary 0.14.0`. In a 2,048 MiB Fedora 44 KVM
  guest it synchronized 76,354 live Fedora packages in 194,288 ms, and the
  guest database contained exactly 76,354 repository-package rows.
- The supported-host generation fixes developed after this tag are not in
  `v0.14.0`. Their qcow2/ISO proof belongs to issue #137 and PR #151 and does
  not make this older artifact current tester authority.

### Remi 0.9.5

- Annotated tag object `f2bf17f0086a7f8ea4be3e032336551c4e6089c1`
  peels to reviewed commit
  `101dba655257f1ff3d1bee689d9c5ac8b2b68cbd`.
- Exact-tag release-build run `30583793501` passed and published the immutable
  release on 2026-07-30. Independent downloads matched every GitHub digest:
  `metadata.json`
  `4ee564321ab8826679e880f8a386e7da6bb94589cad35c6374e71a6538f3bd80`,
  raw binary
  `8333105420ca30de79a8f312758699a950e7e0f280e80cbebf20302a942cbe14`,
  and tarball
  `9144164b9e61e666cc3a801eb08c8ca2d02de2efa09902f3a9e98e2aacf5b40b`.
- The raw and tar-extracted binaries are byte-identical and report
  `remi 0.9.5`. This release publishes no detached signature, SBOM, or
  provenance sidecar.
- Protected deploy-and-verify run `30585462182` passed at the exact release
  commit. Independent public proof on 2026-07-31 passed full health 11/11;
  readiness reports `ready=true` at schema revision 23. Public stats reported
  83,885 packages, 3,494 downloads, three distributions, and 1,783 converted
  packages.

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
