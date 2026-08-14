---
last_updated: 2026-08-13
revision: 22
summary: Define the synchronized suite release contract while preserving exact historical release and current Remi deployment evidence
---

# Release Artifact Matrix

This matrix is the limited-preview artifact contract. It distinguishes Cargo
package ownership, artifact construction, a prepared release target, immutable
publication, deployment, and independent runtime proof. Artifact URLs,
checksums, signatures, deployments, and behavior become authority only after
their exact evidence is recorded here.

Remote Forge validation and conary-test deployment are decommissioned. Local
QEMU/KVM evidence may support a preview row only when it names the absolute run
date, distro, suite, and pass counts.

## Current Release Suite

Issue [#428](https://github.com/ConaryLabs/Conary/issues/428) establishes the
current hard-cut topology: all eight Cargo packages inherit one root workspace
version; four artifact products are built from one reviewed suite commit; and
one annotated `vMAJOR.MINOR.PATCH` tag publishes one GitHub release. Conary and
Remi retain protected deployment lanes. conaryd and conary-test remain
build-only artifacts with `deploy_mode=none`. Suite metadata is a schema-v1
JSON resource; `dry_run` is a boolean rather than release authority encoded as
free-form text. Every member also inherits `publish = false`; the workspace has
no independent Cargo-registry publication track.
GitHub release immutability is enabled: publishing the fully populated draft
locks its tag and assets and creates the release attestation required by
independent closeout proof. The `Protect suite tags` ruleset separately permits
new `v*` tags while rejecting updates and deletions from the moment each tag is
created.

Version `0.15.0` is prepared by that issue but is not yet immutable release
authority. It becomes authority only after the preparation PR is reviewed and
merged, `v0.15.0` is created at that exact merge commit, all four products are
published together, deployment and released-package workflows finish, and the
independent evidence is recorded in a closeout PR. Historical product-prefixed
tags and releases remain immutable evidence for their own trees, but they are
not current release inputs.

The current production Remi is a merged exact-main candidate at
`c5b13097ef8818ab2df050afdf93d8343994cca9`, deployed by successful protected
run `31751375620`. It reports `remi 0.12.1` with binary SHA-256
`e6c6b826b1df6c12e33391dcbf5abc88e2719ab2b63c46750188d40675a80ef3`.
That candidate is deployment authority, not an independent release baseline.
The latest immutable historical Remi release is `remi-v0.12.1`; its lightweight
tag points directly to commit `ad8537f93ed94da417ecb4b53dc12c978d985bf9`.

Conary `v0.14.0` remains immutable evidence for its exact tree, but it is not
current tester authority. Broad external outreach remains separately postponed
at 0/10 qualifying completions.

| Artifact product | Artifact classes | Current construction authority | Suite deploy mode | Authority before the 0.15.0 closeout | Local build |
| --- | --- | --- | --- | --- | --- |
| `conary` | binary, `.ccs`, `.rpm`, `.deb`, `.pkg.tar.zst` | `.github/workflows/release-build.yml`, `scripts/release.sh suite`, `scripts/release-matrix.sh` | protected release assets, static sites, and released-package proof | immutable `v0.14.0` evidence at annotated tag object `c36c767c7169ff519a96dfdc7bedfa757211f334`, reviewed commit `fe23a604b64ea6f7cc87fce8298911e2245e027f`; not current tester authority | `cargo build -p conary` |
| `remi` | binary and tarball | `.github/workflows/release-build.yml`, `scripts/release.sh suite`, `scripts/release-matrix.sh` | protected Remi deployment and repopulation proof, serialized before Conary deployment | current deployed exact-main candidate `c5b13097ef8818ab2df050afdf93d8343994cca9`; latest immutable historical release [remi-v0.12.1](https://github.com/ConaryLabs/Conary/releases/tag/remi-v0.12.1) | `cargo build -p remi` |
| `conaryd` | binary and tarball | `.github/workflows/release-build.yml`, `scripts/release.sh suite`, `scripts/release-matrix.sh` | `none` | immutable historical `conaryd-v0.7.0` at tag object `381ee55cd051234dd78647ef5d7a8c3250c6b9df`, commit `a231276a900bbe8a8ccb6a0942f104cba2ab86b4`; no deployment | `cargo build -p conaryd` |
| `conary-test` | binary and tarball | `.github/workflows/release-build.yml`, `scripts/release.sh suite`, `scripts/release-matrix.sh` | `none` | immutable historical `conary-test-v0.9.0` at tag object `6c4a8cb4bb2c6e89f359a8ceb89ac40a5ce06890`, commit `a231276a900bbe8a8ccb6a0942f104cba2ab86b4`; no deployment | `cargo build -p conary-test` |

## Recorded Evidence

### Current exact-main Remi candidate

- Protected candidate-deployment run `31751375620` completed successfully on
  2026-08-13 at exact merged commit
  `c5b13097ef8818ab2df050afdf93d8343994cca9`.
- Independent host proof found the active `/usr/local/bin/remi` reporting
  `remi 0.12.1` with SHA-256
  `e6c6b826b1df6c12e33391dcbf5abc88e2719ab2b63c46750188d40675a80ef3`.
- `conary-remi-deploy inspect-remi --require-repopulated` passed with schema
  revision 37, 6/6 populated sources, four exact signing profiles, 110,220
  repository packages, and 1,645 conversions. Solus contributes 11,907 eopkg
  packages and one validated conversion.
- `scripts/remi-health.sh --full` independently passed 10/10, and public Solus
  package conversion for `0ad` returned HTTP 200.
- This evidence proves the current deployment. It does not create a tag,
  GitHub release, or separate Remi version authority.

### Remi 0.12.1

- Historical lightweight tag `remi-v0.12.1` points directly to release commit
  `ad8537f93ed94da417ecb4b53dc12c978d985bf9`. It remains exact evidence but
  does not satisfy the synchronized suite's annotated-tag contract.
- Release-build run `31327202489` passed and published the immutable release on
  2026-08-09. Independently downloaded assets matched their GitHub digests:
  `metadata.json`
  `ee3b00e16e4fd6a95db6ebe69a891555cfff9797a81d7e8c6f62133eb37530a8`,
  raw binary
  `88e6b76c1310e64080cb297c50aff44219a19584ab69fa1c6c468ba35f357401`,
  and tarball
  `f701b063c5a39a6f61d3deb7117b90fc85ad6f7163c9a2e410ab778cd96efa9a`.
- The raw and tar-extracted binaries are byte-identical and report
  `remi 0.12.1`. The release has no detached signature, `SHA256SUMS`, SBOM, or
  provenance sidecar.
- Protected deploy-and-verify run `31328183745` passed. The later exact-main
  candidate deployment recorded above supersedes its production state without
  changing this release's immutable evidence.

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

For each published suite release, also record:

- annotated tag object and peeled commit;
- publication time and complete asset names;
- workflow run IDs and terminal conclusions;
- independently downloaded asset SHA-256 values and matching GitHub digests;
- binary `--version` output from all four downloaded artifacts;
- signature, SBOM, and provenance status for each artifact product;
- serialized build-only routing for conaryd and conary-test;
- Conary and Remi deployment and live-behavior proof required by their rows.

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
