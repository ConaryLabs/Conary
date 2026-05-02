---
last_updated: 2026-05-01
revision: 4
summary: Remaining follow-up roadmap after generation export unification and self-contained installed-runtime export validation
---

# Post-Generation-Export Follow-Up Roadmap

## Purpose

This roadmap preserves the generation/image work that remains after two landed
slices:

1. generation artifact export unification
2. self-contained installed-runtime generation export

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

Historical operational validation:

- `cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora43 --phase 3`
- result on 2026-04-30: `TGE01` and `TGE02` passed, 2 passed / 0 failed
- that Fedora 43 run is now historical; Fedora 44 is the active baseline

Current active validation:

- `cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3`
- covered cases: `TGE01`, `TGE03`, `TGE04`, and `TGE02`
- `TGE04` proves installed-runtime qcow2 export boots under UEFI

Everything below remains deferred follow-up work.

## Follow-Up Slices

### 1. Keep Installed Runtime Generations Self-Contained

The first follow-up landed. Installed runtime generation export now works when
the root filesystem is represented by Conary-owned CAS objects, and it fails
closed before artifact publication when the runtime root is partial or
metadata-only.

Remaining work is maintenance, not first implementation:

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

### 3. Move OCI Generation Export Onto The Same Artifact Interface

`conary export` currently packages a generation into an OCI image layout through
its own path. Once disk export consumes `GenerationArtifact`, OCI should use
that same loader instead of independently resolving generation paths, metadata,
and CAS.

Likely work:

- move shared generation source loading into `conary-core`
- keep OCI media-layout code separate from disk-image code
- preserve current top-level `conary export` behavior or intentionally migrate
  it under `conary system generation export --format oci`
- ensure OCI and disk-image exports derive identity labels from the same
  generation metadata

### 4. Introduce Signed Portable Generation Bundles

After the generation directory contract is clean, we can decide whether to
promote the internal artifact interface into a portable bundle format.

Likely work:

- define a bundle layout containing `root.erofs`, scoped CAS objects,
  generation metadata, boot assets, and an artifact manifest
- sign the bundle manifest
- make raw/qcow2/iso/oci export consume either a local generation directory or
  a bundle without changing backend logic
- make bundles suitable for Remi publication or artifact archival

### 5. Extend Trust And Provenance To Bootable Artifacts

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

### 6. Make Boot-Time Activation The Only Supported Generation Contract

The dracut path still contains a legacy bind-mount fallback for generation
directories that lack `root.erofs`. Once the generation and export contracts
are composefs-native and bootable, that fallback should be removed. This is the
continuation of the current slice's explicit non-goal to leave the fallback in
place while generation artifact export is being unified.

Likely work:

- remove the legacy bind-mount fallback from
  `packaging/dracut/90conary/conary-generator.sh`
- make missing `root.erofs` a hard boot activation failure
- decide whether live generation switching is a debug/convenience path rather
  than the main activation story
- make verity-backed activation strict once the kernel and image pipeline are
  aligned

### 7. Make Self-Host Validation Inputs Pristine By Default

The self-host VM tooling can still become stale or stateful if validation
reuses a mutable qcow2 or an old staged workspace tarball.

Likely work:

- make build and validation share one input-staging command
- fail validation when the staged workspace tarball no longer matches the
  current checkout
- boot validation through a temporary overlay or QEMU snapshot mode
- make reruns pristine by default

### 8. Finish The Sandbox Story So Sandbox Means No Host Mutation

Live-root sandboxing still has uneven host mutation boundaries.

Likely work:

- add tmpfs or overlay-backed writable layers for live-root scriptlet
  execution
- prevent package hooks from mutating host `/etc` and `/var` directly
- revisit bootstrap's currently relaxed isolation assumptions after
  self-hosting remains stable
- converge bootstrap source verification on strict repo-owned `sha256`
  checksums everywhere

### 9. Treat CCS/CAS Compatibility Surfaces As Projections

Conary's strongest model is native CCS/CAS identity, but some edges still read
like legacy package-manager sidecars.

Likely work:

- simplify Remi conversion around "legacy in, CCS/CAS out"
- make OCI, disk images, and boot artifacts projections from one canonical
  object model
- remove duplicate identity encoding where metadata can be derived once
- audit docs for wording that implies sidecar flows are primary products

### 10. VMware And Other Image Projections

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
2. move OCI generation export onto the same artifact loader
3. introduce signed portable generation bundles
4. extend trust and provenance to bootable artifacts
5. remove the dracut legacy bind-mount fallback
6. make self-host validation pristine by default
7. finish live-root sandbox/no-host-mutation work
8. simplify CCS/CAS compatibility projections
9. add VMware and other provider-specific image projections

## Scope Guard

Do not widen these follow-ups without keeping the existing QEMU proof green.
The completed slices established:

- one generation artifact contract
- no legacy generation image path
- truthful raw/qcow2 export
- ISO reserved on the same contract
- self-contained installed-runtime qcow2 export and boot validation
- fail-closed handling for partial runtime roots and missing CAS objects
