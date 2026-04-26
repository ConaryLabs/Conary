---
last_updated: 2026-04-25
revision: 2
summary: Follow-up roadmap for bootstrap and generation architecture work after the generation export unification slice
---

# Post-Generation-Export Follow-Up Roadmap

## Purpose

This roadmap preserves the work we did **not** tackle in the first
generation-export unification slice.

The original parking-lot note remains
[`docs/operations/bootstrap-follow-up-investigations.md`](bootstrap-follow-up-investigations.md).
This document is the cleaned-up handoff list to use now that the current slice
has landed.

Completed first slice:

- unify generation-derived raw/qcow2 export around the canonical generation
  directory contract
- remove the legacy imperative generation image path
- stage explicit boot assets next to generation artifacts
- reserve ISO on the same artifact contract

Operational validation still to run before we call the slice fully proven:

- run the `Generation Artifact Export QEMU` suite against the remote/QEMU
  environment and record the result
- keep the existing fail-closed behavior if the runtime-generation path is not
  yet self-contained

Everything below remains deferred follow-up work.

## Follow-Up Slices

### 1. Make Installed Runtime Generations Self-Contained

The landed slice fails closed when an installed runtime generation has
boot assets but its root filesystem is not represented fully in Conary CAS. A
follow-up should make installed-generation export bootable without scraping the
live host root.

Likely work:

- define how a running ConaryOS base is adopted or imported into Conary-owned
  CAS identity
- ensure `/sbin/init` resolves through usr-merge and package symlinks to a
  CAS-backed executable in runtime generations
- make installed-generation QEMU export validation boot a truly self-contained
  runtime generation
- keep the fail-closed behavior for partial or metadata-only generations

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

After the current generation-export unification slice lands, the likely
highest-leverage order is:

1. make installed runtime generations self-contained
2. finish ISO export on the same generation artifact contract
3. move OCI generation export onto the same artifact loader
4. introduce signed portable generation bundles
5. extend trust and provenance to bootable artifacts
6. remove the dracut legacy bind-mount fallback
7. make self-host validation pristine by default
8. finish live-root sandbox/no-host-mutation work
9. simplify CCS/CAS compatibility projections
10. add VMware and other provider-specific image projections

## Scope Guard

Do not start these follow-ups until the remaining QEMU-suite validation from
the generation-export unification slice has either passed or produced a
narrowly scoped blocker. The completed slice established:

- one generation artifact contract
- no legacy generation image path
- truthful raw/qcow2 export
- ISO reserved on the same contract
