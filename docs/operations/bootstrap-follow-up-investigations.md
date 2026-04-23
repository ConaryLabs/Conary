---
last_updated: 2026-04-22
revision: 1
summary: Deferred architecture follow-ups to revisit after the bootstrap self-hosting path is stable
---

# Bootstrap Follow-Up Investigations

## Purpose

This note records forward-looking cleanup and architecture opportunities that
became visible while fixing the truthful bootstrap self-hosting VM path.

These are intentionally **deferred** until the current bootstrap milestone is
stable. They are not approval to expand scope mid-debugging, and they should
not distract from getting the bootstrap image, boot path, and validation flow
green first.

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

### 1. Make Boot-Time Activation The Canonical Generation Contract

The repo's composefs/EROFS generation model is modern, but the edges still show
legacy escape hatches:

- `packaging/dracut/90conary/conary-generator.sh` still falls back to a legacy
  bind-mount path when `root.erofs` is absent
- `apps/conary/src/commands/generation/switch.rs` still performs direct live
  remount work on `/usr` and `/etc`
- live generation switching can downgrade from verity to non-verity retry

Questions to revisit:

- should boot-time composefs activation become the only fully-supported
  activation contract?
- should live generation switching be narrowed to a convenience/debug path
  rather than the primary truth path?
- can we remove the dracut legacy bind-mount fallback once generation images
  are always authoritative?
- can generation activation become strictly verity-backed once the kernel and
  image pipeline are aligned?

Relevant files:

- `packaging/dracut/90conary/conary-generator.sh`
- `apps/conary/src/commands/generation/switch.rs`
- `crates/conary-core/src/generation/mount.rs`
- `apps/conary/src/live_host_safety.rs`

### 2. Unify Disk And Export Artifacts Around One Canonical Generation Source

Generation-derived disk export is now tracked by
`docs/superpowers/specs/2026-04-22-generation-artifact-export-unification-design.md`
and
`docs/superpowers/plans/2026-04-22-generation-artifact-export-unification-plan.md`.
That slice removes the old imperative generation-image path and moves raw/qcow2
export onto the same declarative `systemd-repart` direction as bootstrap
sysroot images.

Today:

- bootstrap sysroot raw/qcow2 uses `systemd-repart`
- generation raw/qcow2 export is being unified around explicit generation
  artifacts, scoped CAS manifests, staged boot assets, and the shared repart
  backend
- CCS export treats OCI as real while `vmdk` and other platform images remain
  future formats

Questions to revisit:

- should raw, qcow2, ISO, and later VMware all derive from the same generation
  artifact and partition contract?
- can OCI export, disk-image export, and generation metadata share more of the
  same identity/signing model instead of behaving like separate products?

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

## Suggested Order After Bootstrap Stabilizes

If we revisit these after the current bootstrap work is green, the likely
highest-leverage order is:

1. unify generation-derived image creation with the new declarative image
   contract
2. remove the dracut legacy generation fallback once the canonical boot path is
   proven
3. make self-host validation inputs and guest state truthful by default so
   bootstrap reruns are not silently stateful or stale
4. finish the live-root sandbox/tmpfs overlay work so live mutation paths are
   narrower and more honest
5. extend signing/attestation from packages and generation metadata to bootable
   system artifacts
6. simplify export and compatibility surfaces around one canonical CCS/CAS
   identity model

## Scope Reminder

Until the bootstrap VM path is fully stable, this document is only a parking
lot for deliberate follow-up investigation. The active critical path remains:

- complete the truthful bootstrap image
- boot it successfully under validation
- clear the current runtime blocker
- verify the guest can exercise the intended self-host flow
