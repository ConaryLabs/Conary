---
last_updated: 2026-08-28
revision: 29
summary: Record immutable v0.16.1 historical evidence, build-once exact-main Remi candidates with bulk compiler reuse and attributable timings, and the unassigned external tester authority
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

Issue [#428](https://github.com/ConaryLabs/Conary/issues/428) established the
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

Version `0.16.1` is the current immutable release authority. Annotated tag
object `0c90d578fd3dd7b58e0c9f8a04f80228e5f65396` peels to reviewed merge commit
`0fb961bacc6360107506371b16b7f0345ba6f927`; all four products were published
together, routed through their declared deployment modes, and independently
verified. Historical product-prefixed tags and releases remain immutable
evidence for their own trees, but they are not current release inputs.

The v0.16.1 release-era deployment ran the exact tagged `remi 0.16.1` binary
whose release asset has SHA-256
`64452867a6b3dab69df6ffd6b2610379321247de3abb2be07a62b4089eb9959d`.
That historical deployment proof recorded schema revision 40, 6/6 populated
sources, 110,182 repository packages, 3,855 conversions, and all four signing
profiles. It is not a claim about the current production schema or active
public universe. Broad external outreach remains separately postponed at 0/10
qualifying completions behind the current gates in
`docs/roadmaps/launch-status.json`; release proof is not tester authority.

This suite adds two Conary product assets:
`conary-bootstrap-v1.manifest` and its detached `.sig`. The release workflow
constructs the manifest only after the exact RPM, DEB, and Arch packages exist;
it binds the suite tag/version and each supported host to one exact basename,
size, and SHA-256, then signs the manifest with the same Ed25519 release key
embedded for self-update authority. `site/static/install-conary-preview.sh`
verifies that signature before parsing any selection field and verifies the
selected artifact before a native package transaction. Exact-tag release-build
and released-artifact proof both completed the clean three-host bootstrap path.

Workspace version `0.16.1` and the published release are synchronized.
Protected tag `v0.16.0` remains reserved evidence for a failed
version-validation run and has no GitHub release; it was not moved or reused.

Between suite releases, `.github/workflows/build-remi-candidate.yml` constructs
one exact release-profile Remi artifact for every protected `main` commit. Its
schema-v2 manifest binds the source tree, lockfile, toolchain, build command,
flags, runner provenance, binary and deterministic bundle digests, bounded
local-bulk compiler-cache policy and statistics, and attributable compiler,
phase, and link timings. One compatible prior snapshot is restored in bulk,
all compilation is local, and the completed exact-head snapshot is saved once;
the cache remains an optimization rather than artifact authority. The candidate deployment lane accepts
only a successful `push` artifact for the requested SHA on this repository's
`main`, reopens and verifies the bundle, and enforces a 60-second
locate/download/verify budget. It never compiles Remi itself. These artifacts
are deployment candidates, not tags, releases, or substitutes for the
synchronized suite authority below.

| Artifact product | Artifact classes | Current construction authority | Suite deploy mode | Current immutable authority | Local build |
| --- | --- | --- | --- | --- | --- |
| `conary` | binary, `.ccs`, `.rpm`, `.deb`, `.pkg.tar.zst`, signed bootstrap manifest | `.github/workflows/release-build.yml`, `scripts/release.sh suite`, `scripts/release-matrix.sh` | protected release assets, static sites, and released-package proof | synchronized suite `v0.16.1`; detached signatures for the CCS artifact and bootstrap manifest | `cargo build -p conary` |
| `remi` | binary and tarball | `.github/workflows/release-build.yml` for suites; `.github/workflows/build-remi-candidate.yml` for build-once exact-main candidates; `scripts/release.sh suite`, `scripts/release-matrix.sh` | protected Remi deployment and repopulation proof, serialized before Conary deployment | synchronized suite `v0.16.1`; exact released binary proven at release closeout | `cargo build -p remi` |
| `conaryd` | binary and tarball | `.github/workflows/release-build.yml`, `scripts/release.sh suite`, `scripts/release-matrix.sh` | `none` | synchronized suite `v0.16.1`; build-only route | `cargo build -p conaryd` |
| `conary-test` | binary and tarball | `.github/workflows/release-build.yml`, `scripts/release.sh suite`, `scripts/release-matrix.sh` | `none` | synchronized suite `v0.16.1`; build-only route | `cargo build -p conary-test` |

## Recorded Evidence

### Conary 0.16.1 synchronized suite

- Preparation PR [#489](https://github.com/ConaryLabs/Conary/pull/489) passed
  its exact-head checks and merged as reviewed commit
  `0fb961bacc6360107506371b16b7f0345ba6f927`.
- Protected annotated tag `v0.16.1` has tag object
  `0c90d578fd3dd7b58e0c9f8a04f80228e5f65396` and peels to that merge commit.
  The protected failed `v0.16.0` tag remains unchanged and has no release.
- Exact-tag release-build run
  [32199379608](https://github.com/ConaryLabs/Conary/actions/runs/32199379608)
  passed all 14 jobs and published the immutable 15-asset release
  [v0.16.1](https://github.com/ConaryLabs/Conary/releases/tag/v0.16.1) at
  `2026-08-19T00:38:11Z`:
  - `SHA256SUMS`:
    `c93933178e4f87bfa1f58c6d6a9aa11b9ef644d5ae8fa8ba128a0c4167532464`
  - `metadata.json`:
    `b7d7c12de7b8608ef1114b2e2d2bd287934917741a00bff54e60ee63e085d141`
  - `conary-0.16.1.ccs`:
    `4da19ee11885456b7068e08836b2a8f72ffedd622b363c3afaa60326e1a1922e`
  - `conary-0.16.1.ccs.sig`:
    `e1f6875e415b7dcb65f87fbcb5b7e31a63fbfbc35f62e954bcbdf7449b5ad110`
  - `conary-bootstrap-v1.manifest`:
    `c8ae10059020b4cef531d0f73d50b806361dc4c7bfabd3e99551599d109cc276`
  - `conary-bootstrap-v1.manifest.sig`:
    `51b8552be96e423fe4e14881bda3d0a3d189516b266a17b60ab7a2d9deffa3b2`
  - `conary-0.16.1-1.fc44.x86_64.rpm`:
    `63d30c0b188bb871431c0eeff51a5333a55f4ab418468ca59c96b0e64824d62e`
  - `conary_0.16.1-1_amd64.deb`:
    `4c5fd64a30b7b1ddeada014d45bbee3369bcaffd845064aa764119e5ff0deb7e`
  - `conary-0.16.1-1-x86_64.pkg.tar.zst`:
    `e0e2aa2c556ef0d4765b34483dc6011c6450f2c7000be0c28eb8411306d87a76`
  - `remi-0.16.1-linux-x64`:
    `64452867a6b3dab69df6ffd6b2610379321247de3abb2be07a62b4089eb9959d`
  - `remi-0.16.1-linux-x64.tar.gz`:
    `335a8566286759959481c19cd41c2b12942c72085d43784686ac72b94421357a`
  - `conaryd-0.16.1-linux-x64`:
    `d875b6c78a64738769112e0b7553b37ffbf1887390fc58f673699f4350e101d3`
  - `conaryd-0.16.1-linux-x64.tar.gz`:
    `7e2cf36f53fb552c2ee33f3cf0df6e64632702e83c54bc96ba7404a658d8580c`
  - `conary-test-0.16.1-linux-x64`:
    `e78324f40f3c9bdfba360e7c946414690f55d821aed83104e526d7f25abccf05`
  - `conary-test-0.16.1-linux-x64.tar.gz`:
    `4f6a29d62f52ca9bd985ab24bf1763143b844bfe9f72a095d7b8fda50c1b6a97`
- A fresh independent download passed `sha256sum -c SHA256SUMS` for all 14
  non-checksum assets. `gh release verify v0.16.1` verified the immutable
  release attestation and every asset digest.
- Schema-v1 metadata names release `suite`, tag `v0.16.1`, version `0.16.1`,
  bundle `suite-bundle`, typed `dry_run=false`, and exactly four product routes.
  The signed bootstrap manifest binds Fedora 44, Ubuntu 26.04 LTS, and Arch
  x86_64 to their exact native package basename, size, and SHA-256.
- Protected deployment and proof run
  [32201994359](https://github.com/ConaryLabs/Conary/actions/runs/32201994359)
  passed exact routing, both build-only routes, Remi and Conary deployment,
  three native-package lifecycle jobs, and their aggregate release-artifact
  proof. Each supported host installed Conary 0.16.1 through the signed
  bootstrap protocol and exposed package-owned repository state.
- The same deployment recorded Remi schema revision 40, 6/6 populated sources,
  four exact signing profiles, 110,182 repository packages, and 3,855
  conversions; the public readiness endpoint reported `ready=true`.
- Release and deployment proof remain distinct from tester authority. W7/#110
  later passed, but this historical suite predates the signed-universe client;
  the external tester pin remains unassigned behind the current launch-status
  gates.

### Conary 0.15.0 synchronized suite

- Preparation PR [#429](https://github.com/ConaryLabs/Conary/pull/429) passed
  exact-head gate `31762562819` and exact-head rehearsal `31763203456` at
  `9361d2af4bdcc83b190de2fc6bf95234d92ac86c`, then merged as reviewed commit
  `642750878d5a59a9aa27976347cafc6f9dd86cfd`. Post-merge validation
  `31766458093` passed at that exact commit.
- Protected annotated tag `v0.15.0` has tag object
  `83ef2d8a264cb49c5deb9e79e2a84a20e6883dab` and peels to that merge commit.
  The commit is reachable from `main`; active ruleset `Protect suite tags`
  (`20825313`) rejects updates and deletions of `v*` tags with no bypass.
- Exact-tag release-build run
  [31766900566](https://github.com/ConaryLabs/Conary/actions/runs/31766900566)
  passed all 11 jobs. It published the immutable 13-asset release
  [v0.15.0](https://github.com/ConaryLabs/Conary/releases/tag/v0.15.0) at
  `2026-08-14T04:23:49Z`:
  - `SHA256SUMS`:
    `f160f65291b7d4f8a8e8357f6bf7783526fe17885165a09419f6c8652bf4024d`
  - `metadata.json`:
    `6c4615e8e9faa101674d5f168261f7b47621dad8f8f6cd5a474917b743f59033`
  - `conary-0.15.0.ccs`:
    `8c5348be89d2c92b094498443d23782c88ff5d4deed888290939b4d73d39cc8f`
  - `conary-0.15.0.ccs.sig`:
    `1730816e0cf92f219692f80e0575a4ef13f536d1bbe200ef45103a789a421c86`
  - `conary-0.15.0-1.fc44.x86_64.rpm`:
    `3297e0e1e625a3d0eb51c68f6bbe715443f2a69fb27216e61ab21529c76fe060`
  - `conary_0.15.0-1_amd64.deb`:
    `61f6c6691c0997f42abb2d2c6b37ed1a699a5cedca6721729d36be43a165cf94`
  - `conary-0.15.0-1-x86_64.pkg.tar.zst`:
    `ae28c6562e82dbb17bbdddf783b916c3ac37a02d5720fbb6101629cfa5e5078d`
  - `remi-0.15.0-linux-x64`:
    `5638e4715a7d6f6b2aa75105b337b77b49953ab6b04e84cf809daaa439563cc4`
  - `remi-0.15.0-linux-x64.tar.gz`:
    `d9fc6efde106e2e4d4f253eeeca929fa144b94737dfe22da9779122b3e99f0d0`
  - `conaryd-0.15.0-linux-x64`:
    `1ab85520d0c870bcd6e7f5c5df687b3487db2d268ab13324ab15e422aa34c770`
  - `conaryd-0.15.0-linux-x64.tar.gz`:
    `be1d6a23e094d7ca14d1bc5faee1899cab6b8105c740114833936f9f2dbb62dd`
  - `conary-test-0.15.0-linux-x64`:
    `7a135284ddc16901317c2ea1c66566cb6857f7bd67800d44c39d3f63235421a7`
  - `conary-test-0.15.0-linux-x64.tar.gz`:
    `3bc8fa88d80df3cbf03fbbe125dbcf5f924add3a9412d399842ac232cfe25ce1`
- Independent download proof passed `sha256sum -c SHA256SUMS` for all 12
  non-checksum assets and matched every one of the 13 local SHA-256 values to
  the release API digest. `gh release verify v0.15.0` and
  `gh release verify-asset` for every asset passed against GitHub's immutable
  release attestation.
- Schema-v1 metadata names release `suite`, tag `v0.15.0`, version `0.15.0`,
  bundle `suite-bundle`, typed `dry_run=false`, and exactly four product routes:
  protected Conary and Remi deployment plus `deploy_mode=none` for conaryd and
  conary-test. All 11 artifact patterns were present.
- Downloaded Remi, conaryd, and conary-test binaries report `0.15.0`; each raw
  binary is byte-identical to the copy in its tarball. The Arch package reports
  `conary 0.15.0-1` for `x86_64`, and the released-package workflow confirmed
  the installed Conary version for all three native package formats.
- Protected deployment run
  [31769739765](https://github.com/ConaryLabs/Conary/actions/runs/31769739765)
  passed exact metadata routing, build-only-route proof, Remi deployment,
  Conary release/static-site deployment, and terminal native-package lifecycle
  proof on Fedora 44, Ubuntu 26.04 LTS, and Arch at the tagged commit.
- Independent Remi proof passed `scripts/remi-health.sh --full` at 10/10 and
  `inspect-remi --require-repopulated` at schema revision 37, 6/6 populated
  sources, four exact signing profiles, 110,220 repository packages, and 1,798
  conversions. The installed `remi 0.15.0` binary hash exactly matches the
  release asset. The public self-update endpoint serves version `0.15.0` and
  the exact released CCS hash.
- The detached `.ccs.sig` is the product-owned signature. Native packages,
  executable bundles, and metadata have GitHub's release attestation and
  checksum coverage but no separate detached signature, SBOM, or additional
  provenance sidecar.

### Superseded exact-main Remi candidate

- Before the synchronized release, protected candidate-deployment run
  `31751375620` completed successfully on
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
- This evidence proves that superseded deployment. It did not create a tag,
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
- signed bootstrap manifest inventory and clean Fedora, Ubuntu, and Arch
  installer proof when the release carries the bootstrap protocol;
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
