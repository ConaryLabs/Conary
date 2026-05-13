---
last_updated: 2026-05-13
revision: 7
summary: Remaining follow-up roadmap after generation export, OCI loader, boot activation, installed-runtime validation, and composefs modernization work
---

# Post-Generation-Export Follow-Up Roadmap

## Purpose

This roadmap preserves the generation/image work that remains after four landed
slices:

1. generation artifact export unification
2. self-contained installed-runtime generation export
3. OCI export source loading on the generation artifact interface
4. composefs-only boot activation cleanup

The original parking-lot note remains
[`docs/operations/bootstrap-follow-up-investigations.md`](bootstrap-follow-up-investigations.md).
This document is the cleaned-up handoff list for the remaining image,
provenance, sandbox, and projection work.

Completed generation export unification:

- unify generation-derived raw/qcow2 export around the canonical generation
  directory contract
- remove the legacy imperative generation image path
- stage explicit boot assets next to generation artifacts
- reserve ISO on the same artifact contract
- validate the generation export path with the remote/QEMU suite

Completed self-contained installed-runtime export:

- migrate the active Fedora integration baseline to Fedora 44
- bulk-adopt installed packages into CAS with `conary system adopt --system --full`
- validate runtime generation inputs before publishing `.conary-artifact.json`
- preserve fail-closed behavior for metadata-only or partial installed roots
- boot a full CAS-backed installed runtime generation exported to qcow2 under UEFI

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

Historical operational validation:

- `cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora43 --phase 3`
- result on 2026-04-30: `TGE01` and `TGE02` passed, 2 passed / 0 failed
- that Fedora 43 run is now historical; Fedora 44 is the active baseline

Current active validation:

- `cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3`
- covered cases: `TGE01`, `TGE03`, `TGE04`, and `TGE02`
- when the source fixture is generation-builder-ready, `TGE04` proves
  installed-runtime qcow2 export boots under UEFI
- as of 2026-05-13 this suite requires a generation-builder-ready source
  image; `minimal-boot-v2` still lacks `cpio`/`dracut` and related export
  helpers, so the current suite fails after `TGE01` until that fixture is
  refreshed

Current composefs modernization validation:

- `cargo run -p conary-test -- run --suite phase3-composefs-modernization --distro fedora44 --phase 3`
- result on 2026-05-13: `TCM01` and `TCM02` passed, 2 passed / 0 failed / 0 skipped
- covered cases: OCI export rejects partial generation artifacts, generation
  switch validates artifacts before pointer updates, and rollback refuses to
  mutate without an active composefs generation; source-contract coverage also
  requires package-mutation apply and recovery to fail closed on `/etc` overlay
  failures

Everything below remains deferred follow-up work.

## Follow-Up Slices

### 1. Keep Installed Runtime Generations Self-Contained

The first follow-up landed. Installed runtime generation export now works when
the root filesystem is represented by Conary-owned CAS objects, and it fails
closed before artifact publication when the runtime root is partial or
metadata-only.

Remaining work is maintenance, not first implementation:

- refresh the QEMU source image used by Groups N and O so it already contains
  the runtime generation toolchain instead of relying on Conary to install
  helper tools on a partial live root
- keep `TGE01`, `TGE03`, and `TGE04` in the active Phase 3 rotation
- preserve usr-merge and package symlink handling for runtime generations
- keep missing-CAS and checksum/size mismatch failures before artifact
  publication
- avoid reintroducing live-host scraping into runtime export

### 2. Finish ISO Export On The Generation Artifact Contract

The landed slice reserves `iso` on the same source contract as raw/qcow2. A
focused follow-up should implement the ISO backend without changing the
generation artifact loader.

Likely work:

- decide whether the ISO is installer media, live media, or a bootable
  generation carrier
- build ISO staging from the same `GenerationArtifact` source object
- make boot configuration generation image-type-specific, not source-specific
- add QEMU boot validation for ISO output

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
provenance already has SLSA/in-toto structures. Bootable system artifacts need
the same level of traceability.

Likely work:

- emit digest manifests for raw, qcow2, ISO, OCI, and bundles
- build on the source-level digest binding added by the generation export
  unification slice without treating that binding as a full image signing
  story
- add provenance links from image artifacts back to generation metadata,
  source packages, and build records
- support operator verification such as "this qcow2 came from this signed
  generation"
- decide which trust roots apply to boot artifact verification

### 5. Make Self-Host Validation Inputs Pristine By Default

The self-host VM tooling can still become stale or stateful if validation
reuses a mutable qcow2 or an old staged workspace tarball.

Likely work:

- make build and validation share one input-staging command
- fail validation when the staged workspace tarball no longer matches the
  current checkout
- boot validation through a temporary overlay or QEMU snapshot mode
- make reruns pristine by default

### 6. Finish The Sandbox Story So Sandbox Means No Host Mutation

Live-root sandboxing still has uneven host mutation boundaries.

Likely work:

- add tmpfs or overlay-backed writable layers for live-root scriptlet
  execution
- prevent package hooks from mutating host `/etc` and `/var` directly
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

After self-contained installed-runtime export, the likely highest-leverage
order is:

1. finish ISO export on the same generation artifact contract
2. introduce signed portable generation bundles
3. extend trust and provenance to bootable artifacts
4. make self-host validation pristine by default
5. finish live-root sandbox/no-host-mutation work
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
