---
last_updated: 2026-05-12
revision: 3
summary: Deferred architecture follow-ups after composefs atomic activation, bootstrap self-hosting, and generation-export milestones
---

# Bootstrap Follow-Up Investigations

## Purpose

This note records forward-looking cleanup and architecture opportunities that
became visible while fixing the truthful bootstrap self-hosting VM path.

These are intentionally **deferred** beyond the initial self-hosting VM and
generation-export milestones. They are not approval to expand scope
mid-debugging, and they should not distract from keeping the checked-in
bootstrap image, boot path, generation export, and validation flows green.

## Working Principle

The useful pattern is bigger than any single technology choice:

- one canonical artifact path instead of multiple semi-supported flows
- declarative assembly instead of imperative disk/image mutation
- content-addressed and verifiable runtime state
- signed metadata and attestable build outputs
- minimal privilege boundaries and rootless steps where practical

Conary already leans this way in its strongest areas. The follow-ups below mark
the places where the codebase still drifts back toward legacy assumptions or
duplicate paths.

## Deferred Investigation Areas

### 1. Keep Boot-Time Activation The Canonical Generation Contract

The repo's composefs/EROFS generation model is now the supported activation
contract. The atomic-modernization slice removed the old `root.erofs`-missing
boot fallback, keeps release-facing switching on next-boot activation, and
fails closed when requested verity cannot be honored.

Remaining work is maintenance and hardening:

Questions to revisit:

- can generation activation become strictly verity-backed once the kernel and
  image pipeline are aligned?
- should the developer-only live switch helper be removed entirely once boot
  activation has enough daily-use validation?

Relevant files:

- `packaging/dracut/90conary/conary-generator.sh`
- `apps/conary/src/commands/generation/switch.rs`
- `crates/conary-core/src/generation/mount.rs`
- `apps/conary/src/live_host_safety.rs`

### 2. Unify Disk And Export Artifacts Around One Canonical Generation Source

Generation-derived disk export is now implemented and the remaining follow-up
work is tracked by
[`docs/operations/post-generation-export-follow-up-roadmap.md`](post-generation-export-follow-up-roadmap.md).
The landed slices removed the old imperative generation-image path, moved
raw/qcow2 export onto the declarative `systemd-repart` direction shared with
bootstrap sysroot images, and proved self-contained installed runtime qcow2
export under QEMU.

Today:

- bootstrap sysroot raw/qcow2 uses `systemd-repart`
- generation raw/qcow2 export uses explicit generation artifacts, scoped CAS
  manifests, staged boot assets, and the shared repart backend
- OCI export also loads the same generation artifact contract and scopes CAS
  objects from the artifact manifest
- installed runtime generations are exportable when their root filesystem is
  fully CAS-backed, and partial roots fail closed before artifact publication
- ISO, VMDK, and other platform images remain future projections

Questions to revisit:

- should raw, qcow2, ISO, and later VMware all derive from the same generation
  artifact and partition contract?
- can OCI export, disk-image export, and generation metadata share one
  identity/signing model without collapsing their different media backends?

Relevant files:

- `crates/conary-core/src/bootstrap/image.rs`
- `apps/conary/src/commands/export.rs`
- `crates/conary-core/src/ccs/export/mod.rs`
- `crates/conary-core/src/generation/builder.rs`
- `crates/conary-core/src/generation/metadata.rs`

### 3. Finish The Sandbox Story So "Sandbox" Means No Host Mutation

Conary has strong isolation instincts, but the implementation is still uneven:

- live-root sandbox mode still bind-mounts host `/etc` and `/var`
- the container module explicitly calls out missing tmpfs overlay support
- bootstrap build config currently disables isolation and still allows a
  bootstrap-only checksum compatibility mode

Questions to revisit:

- can live-root sandboxing grow tmpfs/overlay-backed writable layers so package
  hooks no longer touch the host directly?
- can bootstrap builds move back toward rootless or minimally-privileged
  isolation once the self-host flow is stable?
- can the bootstrap checksum contract converge on repo-owned strict `sha256`
  everywhere instead of keeping a bootstrap-only legacy mode?

Relevant files:

- `crates/conary-core/src/container/mod.rs`
- `crates/conary-core/src/recipe/kitchen/config.rs`
- `crates/conary-core/src/recipe/kitchen/archive.rs`
- `crates/conary-core/src/bootstrap/build_runner.rs`

### 4. Extend Trust And Provenance From Packages To Bootable System Artifacts

The package/provenance side of the repo is ahead of the boot artifact side:

- generation metadata already supports detached signatures
- package provenance already has SLSA/in-toto structures
- bootable images, seeds, and guest-profile outputs do not yet appear to share
  the same trust story end-to-end

Questions to revisit:

- should seeds, generation exports, raw/qcow2 images, and ISO artifacts all
  have explicit digest manifests and signatures?
- should the generation/image pipeline emit attestations the same way package
  provenance does?
- can operator tooling verify "this qcow2 came from this signed generation"
  without relying on informal logs and filenames?

Relevant files:

- `crates/conary-core/src/generation/metadata.rs`
- `crates/conary-core/src/provenance/slsa.rs`
- `apps/conary/src/commands/provenance.rs`
- `apps/conary/src/commands/bootstrap/mod.rs`

### 5. Keep CCS/CAS Canonical And Treat Compatibility Paths As Projections

One of Conary's strongest ideas is that the content-addressed store and native
CCS identity should be the center of the system. Some edges still orbit older
package-manager assumptions:

- Remi still has an explicit legacy package conversion service
- some export paths and operational docs still read like sidecar flows instead
  of projections from one canonical object model

Questions to revisit:

- can more of Remi's compatibility surface become "legacy in, canonical CCS/CAS
  out" with fewer special cases?
- can OCI, disk images, and boot artifacts all become thin projections from
  canonical CCS/CAS-backed state?
- are there places where we are re-encoding the same identity information in
  parallel instead of deriving it once?

Relevant files:

- `apps/remi/src/server/conversion.rs`
- `apps/remi/src/server/handlers/oci.rs`
- `crates/conary-core/src/ccs/export/oci.rs`
- `crates/conary-core/src/filesystem/cas.rs`

### 6. Make Self-Host Validation Inputs And Guest State Truthful By Default

The self-host VM tooling exposed two workflow traps during debugging:

- `vm-selfhost/inputs/conary-workspace.tar.gz` can drift behind the current
  worktree if we rebuild the image directly without restaging deterministic
  inputs
- `validate-selfhost-vm.sh` mutates the qcow2 it boots, so reruns can quietly
  stop being pristine unless we rebuild the image first

Questions to revisit:

- should there be a single command that always stages the workspace tarball
  and image together so validator inputs cannot drift?
- should the validator boot via a temporary overlay or QEMU snapshot mode so a
  rerun is automatically pristine?
- should validation fail closed when the staged workspace tarball no longer
  matches the current checkout instead of silently testing stale code?

Relevant files:

- `scripts/bootstrap-vm/build-selfhost-qcow2.sh`
- `scripts/bootstrap-vm/validate-selfhost-vm.sh`
- `scripts/bootstrap-vm/guest-validate.sh`

## Suggested Order After Initial Stabilization

The likely highest-leverage order is:

1. make self-host validation inputs and guest state truthful by default so
   bootstrap reruns are not silently stateful or stale
2. finish the live-root sandbox/tmpfs overlay work so live mutation paths are
   narrower and more honest
3. extend signing/attestation from packages and generation metadata to bootable
   system artifacts
4. simplify export and compatibility surfaces around one canonical CCS/CAS
   identity model
5. finish ISO/VMware projection work under the post-generation-export roadmap

## Scope Reminder

This document is still a parking lot for deliberate follow-up investigation.
Before widening any item, keep these proofs green:

- the checked-in self-hosting VM build and validation wrappers
- the Fedora 44 `phase3-group-o-generation-export` QEMU suite
- package/service tests for the subsystem being touched
