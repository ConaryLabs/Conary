---
last_updated: 2026-05-20
revision: 11
summary: Follow-up roadmap after ISO export and output provenance landed
---

# Post-Generation-Export Follow-Up Roadmap

## Purpose

This roadmap preserves the generation/image work that remains after five landed
slices:

1. generation artifact export unification
2. self-contained installed-runtime generation export
3. OCI export source loading on the generation artifact interface
4. composefs-only boot activation cleanup
5. x86_64 ISO generation-carrier export with output provenance sidecars

The original parking-lot note remains
[`docs/operations/bootstrap-follow-up-investigations.md`](bootstrap-follow-up-investigations.md).
This document is the cleaned-up handoff list for the remaining image,
provenance, sandbox, and projection work.

Completed generation export unification:

- unify generation-derived raw/qcow2 export around the canonical generation
  directory contract
- remove the legacy imperative generation image path
- stage explicit boot assets next to generation artifacts
- keep ISO on the same artifact contract as raw/qcow2
- validate the generation export path with the remote/QEMU suite

Completed self-contained installed-runtime export:

- migrate the active Fedora integration baseline to Fedora 44
- bulk-adopt installed packages into CAS with `conary system adopt --system --full`
- validate runtime generation inputs before publishing `.conary-artifact.json`
- preserve fail-closed behavior for metadata-only or partial installed roots
- boot a full CAS-backed installed runtime generation exported to qcow2 under
  UEFI. The 2026-05-16 checkpoint exposed a `TGE04` regression where the
  exported image booted its kernel but had no working init; the 2026-05-19
  refresh fixed that path by generating a Conary-aware initramfs and restored
  the installed-runtime boot proof.

Completed OCI export source loading:

- load OCI generation export sources through the shared generation artifact
  interface
- keep OCI media-layout code separate from disk-image projection code
- derive OCI and disk-image identity labels from the same generation metadata

Completed composefs-only boot activation cleanup:

- require `root.erofs` for boot activation
- fail closed when an installed generation is incomplete
- validate generation artifacts before switch, rollback, recovery, and export
- fail closed instead of publishing an active generation when `/etc` overlay
  setup fails
- keep live generation switching out of the supported release path
- publish package-mutation results by selecting `/conary/current` for the next
  boot instead of live-mounting the newly built generation
- keep ordinary transaction recovery selected-generation-only; explicit boot
  selection recovery remains the path that scans, promotes, and remounts

Historical operational validation:

- superseded 2026-04-30 baseline: `TGE01` and `TGE02` passed, 2 passed / 0 failed
- Fedora 44 is the active baseline

Current active validation:

- `cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3`
- covered cases: `TGE01`, `TGE03`, `TGE04`, and `TGE02`
- when the source fixture is generation-builder-ready, `TGE04` is the intended
  proof that installed-runtime qcow2 export boots under UEFI
- the active manifests use the generation-builder-ready `minimal-boot-v3`
  source image; they no longer install `cpio`, `dracut`, `qemu-img`,
  `dosfstools`, or related helper libraries through Conary before the runtime
  is generation-owned
- historical local Group O evidence includes the 2026-05-09 pass of `TGE01`,
  `TGE02`, `TGE03`, and `TGE04` with 0 failures and 0 skipped results
- current Group O evidence from 2026-05-19 passed `TGE01`, `TGE03`, `TGE04`,
  and `TGE02` with 4 passed / 0 failed / 0 skipped / 0 cancelled. `TGE04` now
  boots the installed-runtime qcow2 under UEFI, reaches SSH, and emits the
  `installed-runtime-generation-export-booted` marker.
- the 2026-05-21 local wrapper refresh kept composefs modernization,
  Group N, and Group O green, and the focused Group P run passed ISO export,
  provenance, copy-back, readonly-carrier boot, and writable `/etc` overlay
  proof
- `cargo run -p conary-test -- run --suite phase3-group-p-iso-export --distro fedora44 --phase 3`
- covered case: `TISO01`, which exports a bootstrap-run generation to ISO,
  copies the ISO plus `.conary-provenance.json` sidecar to the host, and boots
  the ISO under UEFI using the QEMU `image_format = "iso"` contract
- the Group P manifest is present and listed by `cargo run -p conary-test -- list`;
  the focused 2026-05-21 local KVM Group P run passed with 1 passed / 0 failed /
  0 skipped / 0 cancelled

Current composefs modernization validation:

- `cargo run -p conary-test -- run --suite phase3-composefs-modernization --distro fedora44 --phase 3`
- result on 2026-05-13: `TCM01` and `TCM02` passed, 2 passed / 0 failed / 0 skipped
- covered cases: OCI export rejects partial generation artifacts, generation
  switch validates artifacts before pointer updates, and rollback refuses to
  mutate without an active composefs generation; source-contract coverage also
  requires package-mutation apply and recovery to fail closed on `/etc` overlay
  failures
- `cargo run -p conary-test -- run --suite phase3-group-n-qemu --distro fedora44 --phase 3`
- result on 2026-05-14: `T150`, `T151`, `T153`, `T154`, and `T156` passed,
  5 passed / 0 failed / 0 skipped against `minimal-boot-v3`
- `T154` covers bootloader deployment after full CAS-backed live-root adoption
  and versioned critical runtime dependency satisfaction through
  `conary-live-root` identity provides for `glibc`/`libc6`

Everything below is deferred follow-up, remaining evidence work, or maintenance.

## Follow-Up Slices

### 1. Keep Installed Runtime Generations Self-Contained

The first follow-up landed, the historical 2026-05-09 gate proved the
installed-runtime export path, and the 2026-05-19 refresh restored the
installed-runtime positive boot proof after the 2026-05-16 `TGE04` regression.
Generation export is green for the current x86_64 raw/qcow2 preview evidence,
but it should stay in rotation because it is still supporting evidence rather
than the headline public-preview ask.

Remaining work:

- keep the `minimal-boot-v3` QEMU source image generation-builder-ready for
  Groups N and O, and keep Group P helper provisioning covered while the source
  fixture remains minimal
- keep `TGE01`, `TGE03`, and `TGE04` in the active Phase 3 rotation
- preserve usr-merge and package symlink handling for runtime generations
- keep missing-CAS and checksum/size mismatch failures before artifact
  publication
- avoid reintroducing live-host scraping into runtime export

### 2. Keep ISO Export On The Generation Artifact Contract

The x86_64 ISO backend now uses the same source contract as raw/qcow2. It loads
`GenerationArtifact`, projects the runtime tree, builds a UEFI bootable
generation carrier, and emits an output provenance sidecar. The ISO boot entry
uses `root=LABEL=CONARY_ISO rootfstype=iso9660 ro conary.carrier=readonly`, and
the initramfs places the writable `/etc` overlay upper/work under
`/sysroot/run/conary/etc-state` for read-only carriers.

Remaining work:

- keep the focused Group P ISO QEMU evidence in the release-candidate rotation
- keep ISO framed as a bootable generation carrier, not installer media
- keep non-x86_64 boot assets reserved until real boot assets and validation
  land

### 3. Introduce Signed Portable Generation Bundles

After the generation directory contract is clean, we can decide whether to
promote the internal artifact interface into a portable bundle format.

Likely work:

- define a bundle layout containing `root.erofs`, scoped CAS objects,
  generation metadata, boot assets, and an artifact manifest
- sign the bundle manifest
- make raw/qcow2/iso/oci export consume either a local generation directory or
  a bundle without changing backend logic
- make bundles suitable for Remi publication or artifact archival

### 4. Extend Trust And Provenance To Bootable Artifacts

Generation metadata already supports detached signatures, and package
provenance already has SLSA/in-toto structures. Bootable system artifacts now
emit output provenance sidecars for raw, qcow2, and ISO outputs, including the
source artifact digest and output digest. They still need signed verification
and operator-facing trust policy.

Remaining work:

- build on the source-level digest binding added by the generation export
  unification slice without treating that binding as a full image signing
  story
- add provenance links from image artifacts back to generation metadata,
  source packages, and build records
- support operator verification such as "this qcow2 came from this signed
  generation"
- decide which trust roots apply to boot artifact verification

### 5. Make Self-Host Validation Inputs Pristine By Default

The validation wrapper now fails before QEMU when the staged workspace tarball
checksum sidecar is invalid, the tarball does not match the sidecar, or the
sidecar digest does not match a freshly generated deterministic tarball from the
current checkout. The remaining hygiene risk is mutable guest image state across
reruns.

Remaining work:

- make build and validation share one input-staging command
- boot validation through a temporary overlay or QEMU snapshot mode
- make reruns pristine by default

### 6. Finish The Sandbox Story So Sandbox Means No Host Mutation

Protected live-root scriptlet sandboxing now gives `/etc` and `/var` private
writable layers and fails before execution when those protection guarantees
cannot be set up.

Likely remaining work:

- extend the same no-host-mutation contract to any package-hook paths that do
  not already route through protected scriptlet execution
- revisit bootstrap's currently relaxed isolation assumptions after
  self-hosting remains stable
- converge bootstrap source verification on strict repo-owned `sha256`
  checksums everywhere

### 7. Treat CCS/CAS Compatibility Surfaces As Projections

Conary's strongest model is native CCS/CAS identity, but some edges still read
like legacy package-manager sidecars.

Likely work:

- simplify Remi conversion around "legacy in, CCS/CAS out"
- make OCI, disk images, and boot artifacts projections from one canonical
  object model
- remove duplicate identity encoding where metadata can be derived once
- audit docs for wording that implies sidecar flows are primary products

### 8. VMware And Other Image Projections

VMware remains follow-up work after raw/qcow2 export is truthful.

Likely work:

- add VMDK conversion from the canonical raw artifact
- decide whether OVF packaging is in scope
- document import expectations and validation limits
- keep provider-specific metadata out of the core generation contract unless
  the provider genuinely requires it

## Suggested Order

After ISO export and output provenance, the likely highest-leverage order is:

1. introduce signed portable generation bundles
2. extend sidecar provenance into signed boot-artifact verification
3. keep Group P ISO QEMU evidence green in the local release-candidate rotation
4. finish self-host snapshot/overlay rerun isolation
5. extend live-root sandbox/no-host-mutation work to remaining hook surfaces
6. simplify CCS/CAS compatibility projections
7. add VMware and other provider-specific image projections

## Scope Guard

Do not widen these follow-ups without keeping the existing QEMU proof green.
The completed slices established:

- one generation artifact contract
- no legacy generation image path
- truthful raw/qcow2 export
- ISO reserved on the same contract
- self-contained installed-runtime qcow2 export and boot validation
- fail-closed handling for partial runtime roots and missing CAS objects
