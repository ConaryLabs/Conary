---
last_updated: 2026-04-22
revision: 1
summary: Design for replacing legacy generation image export with one canonical generation-artifact-to-image pipeline
---

# Generation Artifact Export Unification: Design Spec

**Date:** 2026-04-22
**Status:** Draft for user review (design approved in conversation)
**Goal:** Replace the legacy generation-derived image path with one truthful
generation-artifact export contract that emits `raw` and `qcow2` disk images
through a shared declarative image backend and reserves the same contract for
future `iso` output.

---

## Scope

This task covers the first implementation slice from
[`docs/operations/bootstrap-follow-up-investigations.md`](../../operations/bootstrap-follow-up-investigations.md):
unifying generation-derived image creation with the declarative
`systemd-repart` image contract.

It includes:

- treating an existing Conary generation directory as the canonical source of
  truth for bootable generation exports
- adding a generation-artifact loading and validation layer with an explicit
  root artifact manifest
- adding an explicit CAS hash manifest so export never guesses which objects
  belong to a generation
- staging explicit boot assets next to generated `root.erofs` artifacts
- teaching bootstrap-run output to produce the same boot-asset contract as
  installed runtime generations
- supporting `x86_64` bootable export first, while reserving explicit
  unsupported-architecture errors for `aarch64` and `riscv64`
- moving generation-derived disk export to `conary system generation export`
- removing `conary bootstrap image --from-generation`
- deleting the imperative `ImageBuilder::build_from_generation()` path instead
  of preserving it as a compatibility branch
- replacing generation-derived `sfdisk`, `mkfs.fat`, offset math, and manual
  raw-image writes with the shared declarative image backend
- supporting `raw` and `qcow2` generation export in this slice
- reserving `iso` on the same generation-artifact contract, even if the first
  implementation returns an explicit "not implemented yet" error for ISO
- creating a companion follow-up roadmap for the remaining deferred areas

It excludes:

- introducing a new portable generation bundle format in this slice
- moving OCI generation export onto the new interface in this slice
- changing CCS package export semantics
- removing the dracut legacy bind-mount fallback
- making live generation switching boot-only
- implementing full image signing, SLSA attestations, or in-toto layouts for
  bootable artifacts
- solving the broader sandbox/no-host-mutation story
- adding VMware, cloud-image, or provider metadata outputs

## Non-Goals

- keeping the old generation image path alive under a warning
- silently scraping live host `/boot` during export to make incomplete
  generation artifacts appear bootable
- interpreting arbitrary bootstrap work directories in export commands
- inventing a second "truth" artifact before the existing generation directory
  contract is clean
- making ISO output follow a different source model from raw/qcow2 output

---

## Repository Context

Bootstrap image creation already moved toward a declarative `systemd-repart`
contract:

- `crates/conary-core/src/bootstrap/image.rs` uses `systemd-repart` for
  bootstrap `raw` images and converts those raw images to `qcow2`
- `crates/conary-core/src/bootstrap/repart.rs` owns the current repart
  definition generator
- `conary bootstrap image` is still the sysroot-oriented bootstrap image
  command

Generation-derived image creation is still split and partly untruthful:

- `crates/conary-core/src/bootstrap/image.rs` still contains
  `ImageBuilder::build_from_generation()`
- that method hand-rolls a GPT layout with `sfdisk`, creates a FAT ESP with
  `mkfs.fat`, writes bytes into fixed offsets, and then optionally runs
  `qemu-img`
- it writes `root.erofs` into the root partition without building the runtime
  root layout that the boot path actually expects
- it warns that ESP kernel population is not implemented, which means the
  result is not truthfully bootable

Generation boot activation has a different shape:

- `packaging/dracut/90conary/conary-generator.sh` expects a root filesystem
  containing `/conary/generations/<N>/root.erofs`, `/conary/objects`, and
  `/conary/current`
- `crates/conary-core/src/generation/mount.rs` expects composefs to resolve CAS
  objects from a `basedir`
- `crates/conary-core/src/generation/metadata.rs` defines generation metadata,
  the `root.erofs` name, excluded runtime directories, and root-level
  usr-merge symlinks
- runtime boot entries in `apps/conary/src/commands/generation/boot.rs` assume
  real kernel and initramfs assets exist outside the composefs generation

Bootstrap-run output already creates a generation-shaped artifact, but it is
not yet self-contained enough for truthful disk export:

- `conary bootstrap image --format erofs` emits `objects/`,
  `generations/1/root.erofs`, `generations/1/.conary-gen.json`, and
  `db.sqlite3`
- `conary bootstrap run` writes operation-scoped output with
  `output/generations/1/root.erofs` and a `current` symlink
- neither path currently gives the generation directory an explicit
  boot-assets manifest that disk export can validate

The current top-level OCI generation export is out of scope for this slice:

- `apps/conary/src/commands/export.rs` packages a generation's EROFS image and
  scoped CAS objects into an OCI image layout
- it remains unchanged for now, but it is a later convergence target once the
  generation-artifact contract is established

---

## Decision

Use the existing generation directory as the canonical on-disk source of truth
for this slice, and design the new code so that a future portable bundle can be
introduced without another rewrite.

The generation directory contract for exportable artifacts becomes:

```text
<generation-dir>/
  .conary-artifact.json
  root.erofs
  .conary-gen.json
  .conary-gen.sig              # optional existing metadata signature
  cas-manifest.json
  boot-assets/
    manifest.json
    vmlinuz
    initramfs.img
    EFI/
      BOOT/
        BOOTX64.EFI            # arch-specific filename for x86_64
```

For this slice, `x86_64` is the only required bootable export architecture.
The manifest schema reserves `aarch64` and `riscv64`, but the exporter must
fail closed with an explicit unsupported-architecture message until those boot
asset staging paths are implemented and tested.

Generation export consumes this one artifact shape whether it came from:

- an installed runtime generation under `/conary/generations/<N>`
- bootstrap EROFS output under `generations/1`
- bootstrap-run operation output under `output/generations/1`

Rejected alternatives:

- **Bundle-first.** Rejected for this slice because it would add a new artifact
  identity before the existing generation directory contract is clean.
- **Keep old and new paths side by side.** Rejected because the project is
  early, the maintainer is the active user, and keeping a known-false legacy
  path would create future confusion.
- **Only replace `sfdisk` with repart in place.** Rejected because it would
  leave generation export structurally owned by bootstrap image code and would
  not fix the missing boot/runtime rootfs contract.
- **Only support installed generations.** Rejected because bootstrap-run output
  is one of the main consumers of this work and should become self-contained in
  this slice.

---

## Design

### 1. Generation Directory Remains The Source Contract

Add a generation-artifact layer that can load an exportable generation from
either an installed generation number or an explicit generation directory path.

The loader validates:

- the generation directory exists
- the generation is not pending
- `.conary-artifact.json` exists and parses
- `root.erofs` exists and passes the existing EROFS structural validation
- `.conary-gen.json` can be read and, when policy requires it, verified
- `cas-manifest.json` exists, parses, and lists SHA-256 object hashes
- `boot-assets/manifest.json` exists and parses
- every file declared in the boot-assets manifest exists under
  `boot-assets/`
- all artifact and boot-asset paths are normalized relative paths, are not
  absolute, and do not escape their declared roots through `..` traversal
- the declared CAS locator resolves to an object store
- every hash in `cas-manifest.json` exists in that object store
- the artifact manifest architecture matches the boot-assets manifest
  architecture
- the generation's declared architecture can be mapped to bootloader and
  partition conventions

The loader should expose a small, focused source object rather than making
export backends read arbitrary paths themselves. Conceptually:

```rust
pub struct GenerationArtifact {
    pub generation: i64,
    pub generation_dir: PathBuf,
    pub artifact_manifest: GenerationArtifactManifest,
    pub metadata: GenerationMetadata,
    pub erofs_path: PathBuf,
    pub cas_dir: PathBuf,
    pub cas_hashes: Vec<String>,
    pub boot_assets: BootAssets,
}
```

This is an internal interface, not a new public bundle format.

### 2. Boot Assets Become Explicit Generation Data

Add a root artifact manifest under each exportable generation:

```json
{
  "version": 1,
  "generation": 1,
  "architecture": "x86_64",
  "metadata": ".conary-gen.json",
  "erofs": "root.erofs",
  "cas_base": "../../objects",
  "cas_manifest": "cas-manifest.json",
  "boot_assets": "boot-assets/manifest.json"
}
```

All fields above are required in version 1. `cas_base`, `cas_manifest`, and
`boot_assets` are relative to the generation directory and must not escape
through path traversal. `cas_base` points at the object store; `cas_manifest`
is the authoritative scoped hash list for export. `db.sqlite3` is not part of
the exportable generation artifact contract. Runtime or bootstrap builders may
use a database to produce `cas-manifest.json`, but exporters must consume the
manifest instead of querying a database or copying an entire CAS store.

Add a versioned boot-assets manifest under each exportable generation:

```json
{
  "version": 1,
  "generation": 1,
  "architecture": "x86_64",
  "kernel_version": "6.19.8-conary",
  "kernel": "vmlinuz",
  "initramfs": "initramfs.img",
  "efi_bootloader": "EFI/BOOT/BOOTX64.EFI",
  "created_at": "2026-04-22T00:00:00Z"
}
```

All fields above are required for version 1.
`kernel`, `initramfs`, and `efi_bootloader` are paths relative to
`boot-assets/`. They must be normalized, non-absolute, and non-traversing; the
loader rejects any path that contains `..` or resolves outside the
`boot-assets/` subtree.

Supported architecture matrix for this slice:

| Architecture | EFI removable loader | Export behavior |
|--------------|----------------------|-----------------|
| `x86_64` | `EFI/BOOT/BOOTX64.EFI` | implemented |
| `aarch64` | `EFI/BOOT/BOOTAA64.EFI` | reserved; fail closed until implemented |
| `riscv64` | `EFI/BOOT/BOOTRISCV64.EFI` | reserved; fail closed until implemented |

The manifest schema is allowed to carry reserved architectures, but raw/qcow2
export must not pretend those architectures work until their boot asset staging
and QEMU validation are implemented.

Runtime generation builds should stage boot assets at generation-build time,
not export time. That may read the live system's `/boot` while building the
generation, because the generation build is the point where a host-specific
runtime snapshot is being created. Export must not later paper over missing
assets by scraping live `/boot`.

Bootstrap EROFS generation creation and bootstrap-run output should stage boot
assets from the bootstrap sysroot as part of producing `generations/1`. This is
what makes external `--path` export truthful without requiring an operator to
pass a separate boot root.

Any generation without the required boot-assets manifest fails export with an
explicit remediation message, for example:

```text
Generation 7 is missing .conary-artifact.json or boot-assets/manifest.json.
Rebuild the generation with a Conary version that stages generation export
metadata before exporting disk images.
```

### 3. Export Projects A Runtime Rootfs, Not EROFS-As-Root

The old generation export path wrote `root.erofs` directly into a root
partition. The new path must instead build a minimal runtime rootfs staging
tree that can activate the composefs generation at boot.

The projected rootfs should contain:

- `/conary/generations/<N>/root.erofs`
- `/conary/generations/<N>/.conary-gen.json`
- `/conary/generations/<N>/.conary-gen.sig` when present
- `/conary/generations/<N>/.conary-artifact.json`
- `/conary/generations/<N>/cas-manifest.json`
- `/conary/generations/<N>/boot-assets/`
- `/conary/objects/` scoped to objects referenced by the generation
- `/conary/current -> generations/<N>`
- `/conary/etc-state/`
- `/usr`, `/etc`, `/boot`, `/var`, `/tmp`, `/run`, `/home`, `/root`, `/srv`,
  `/opt`, `/proc`, `/sys`, `/dev`, `/mnt`, and `/media` mountpoints or runtime
  directories as appropriate
- root-level usr-merge symlinks defined by `ROOT_SYMLINKS`, such as
  `/bin -> usr/bin`

This staging tree is generated from Conary generation invariants. It is not a
copy of the host root.

The rootfs projection copies exactly the hashes listed in
`cas-manifest.json`. If that manifest is missing, invalid, or references
objects absent from `cas_base`, the exporter fails closed rather than silently
copying an arbitrary host-wide CAS store.

### 4. Shared Declarative Image Backend

Move the reusable partition/image backend out of bootstrap-specific ownership.

The final module layout can be adjusted during implementation, but the intended
responsibilities are:

- generation artifact loader and boot-assets manifest handling under
  `crates/conary-core/src/generation/`
- reusable repart definition and disk-image materialization under a shared
  image/layout module, not under `bootstrap` only
- bootstrap sysroot image creation remains a caller of the shared repart
  backend
- generation disk export becomes another caller of the same backend

The shared backend should consume a declarative image plan such as:

```rust
pub struct DiskImagePlan {
    pub architecture: TargetArch,
    pub esp_source: PathBuf,
    pub root_source: PathBuf,
    pub output: PathBuf,
    pub size: ImageSize,
}
```

The plan is intentionally about staged inputs, not about generation internals.
That keeps `systemd-repart` as an implementation backend instead of the top
level architecture.

### 5. CLI Shape

Generation-derived export moves under generation management:

```bash
conary system generation export 7 --format qcow2 --output gen7.qcow2
conary system generation export --path ./output/generations/1 --format raw --output gen1.raw
conary system generation export --format qcow2 --output current.qcow2
```

CLI behavior:

- positional generation number selects an installed generation
- omitting both generation number and `--path` exports the current installed
  generation
- `--path` selects an explicit generation directory and conflicts with the
  positional number
- `--format raw|qcow2|iso`
- `--output <path>` is required
- `--size <size>` is optional for raw/qcow2 and should request a size larger
  than the computed minimum
- `iso` is accepted by the CLI because the contract is reserved, but it may
  return an explicit "ISO export is reserved on the generation artifact
  contract but not implemented yet" error in this slice

Remove:

```bash
conary bootstrap image --from-generation ...
```

The bootstrap image command should be responsible only for sysroot-derived
bootstrap image creation and bootstrap EROFS generation output.

The existing top-level `conary export` OCI command remains unchanged in this
slice. A later task can move or wrap it once OCI export consumes the same
generation-artifact interface.

### 6. Size Selection

Raw and qcow2 export should compute a minimum truthful disk size from:

- fixed GPT overhead
- fixed ESP size
- projected runtime rootfs size
- scoped CAS object size
- a small safety margin

If the user provides `--size`, it must be at least that computed minimum. If it
is smaller, export fails with a message that includes both the requested size
and the computed minimum.

This is intentionally different from the old fixed default behavior. The
exported image should be big enough for its actual payload by construction.

### 7. Failure Behavior

The exporter fails closed when:

- generation metadata is unreadable or untrusted under the active verification
  policy
- the generation is pending
- `.conary-artifact.json` is missing or invalid
- `root.erofs` is missing or invalid
- `cas-manifest.json` is missing, invalid, or references absent objects
- `boot-assets/manifest.json` is missing
- any boot asset declared by the manifest is missing
- the declared CAS path cannot be resolved
- the requested architecture is not implemented in this slice
- required host tools are unavailable (`systemd-repart` for raw, plus
  `qemu-img` for qcow2)
- the requested disk size is too small
- `iso` is requested before the ISO backend is implemented

The exporter must not fall back to:

- live host `/boot`
- live host `/conary`
- the deleted `ImageBuilder::build_from_generation()` implementation
- direct `sfdisk` or offset-writing generation image logic

### 8. Companion Follow-Up Roadmap

This slice should create a new companion doc:

```text
docs/operations/post-generation-export-follow-up-roadmap.md
```

That doc should preserve the remaining follow-up areas after this first slice
is scoped out, rather than mutating the original bootstrap parking-lot note
into a moving target.

---

## Testing Strategy

Unit tests:

- generation artifact loader accepts complete artifacts
- generation artifact loader rejects pending generations
- generation artifact loader rejects missing metadata
- generation artifact loader rejects missing artifact manifests
- generation artifact loader rejects missing CAS manifests
- generation artifact loader rejects CAS manifests that reference absent objects
- generation artifact loader rejects missing boot-assets manifest
- generation artifact loader rejects missing declared boot assets
- generation artifact loader rejects reserved unsupported architectures
- artifact manifest round-trips through JSON
- CAS manifest round-trips through JSON
- boot-assets manifest round-trips through JSON
- rootfs projection creates `/conary/current`
- rootfs projection stages generation metadata and `root.erofs`
- rootfs projection creates `etc-state` and runtime mountpoints
- rootfs projection creates usr-merge symlinks from `ROOT_SYMLINKS`
- size computation rejects undersized images
- repart definitions use staged ESP and rootfs sources

CLI tests:

- `conary bootstrap image --from-generation` no longer parses
- `conary system generation export --format qcow2 --output out.qcow2` parses
- `conary system generation export 7 --format raw --output out.raw` parses
- `conary system generation export --path gen --format raw --output out.raw`
  parses
- `--path` conflicts with a positional generation number
- `iso` parses but reports explicit unimplemented behavior if the backend is
  not implemented in this slice

Regression tests:

- no generation export code path invokes `sfdisk`
- no generation export code path invokes `mkfs.fat` directly
- `ImageBuilder::build_from_generation()` is removed
- bootstrap-run output contains `generations/1/.conary-artifact.json`,
  `generations/1/cas-manifest.json`, and
  `generations/1/boot-assets/manifest.json`

Verification commands for the implementation:

```bash
cargo test -p conary --bin conary cli::bootstrap::tests::cli_rejects_bootstrap_image_from_generation
cargo test -p conary --bin conary cli::generation::tests
cargo test -p conary-core generation::artifact
cargo test -p conary-core generation::export
cargo test -p conary-core bootstrap::image
cargo run -p conary-test -- list
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

The exact test names may change during implementation, but the coverage above
is required.

---

## Risks And Mitigations

- **Risk:** Runtime generations do not currently have enough boot-asset data to
  export truthfully.
  **Mitigation:** stage boot assets during generation build and fail export for
  older generations.

- **Risk:** Bootstrap-run output points at a generation but not at the needed
  CAS store.
  **Mitigation:** make `.conary-artifact.json` declare `cas_base`, make
  `cas-manifest.json` declare the scoped object hashes, and validate both
  before export.

- **Risk:** The shared repart backend becomes too generic.
  **Mitigation:** keep it focused on staged ESP/rootfs inputs and leave
  generation-specific logic in the generation exporter.

- **Risk:** ISO output pulls the interface toward live-image assumptions too
  early.
  **Mitigation:** reserve `iso` in the CLI and source contract, but allow an
  explicit unimplemented error until the ISO backend gets its own focused
  slice.

- **Risk:** OCI export remains visibly separate after this slice.
  **Mitigation:** document OCI convergence as a follow-up roadmap item and do
  not force it into this disk-image cleanup.

---

## Acceptance Criteria

- `conary bootstrap image --from-generation` is gone.
- `conary system generation export` is the only disk-image export surface for
  generation-derived artifacts.
- `ImageBuilder::build_from_generation()` and its imperative image-writing code
  are removed.
- generation export uses the shared declarative repart backend for raw output.
- qcow2 export converts the generated raw artifact with `qemu-img`.
- generation export builds a runtime rootfs staging tree with `/conary`
  generation state instead of writing `root.erofs` as the root partition.
- installed generations and bootstrap-run generations both stage boot assets.
- installed generations and bootstrap-run generations both stage
  `.conary-artifact.json` and `cas-manifest.json`.
- external `--path` export works for a complete bootstrap-run generation
  artifact.
- unsupported architectures fail closed instead of attempting to guess boot
  asset paths.
- incomplete generations fail closed with actionable messages.
- `iso` is represented in the same generation-artifact contract, even if the
  first implementation returns an explicit not-implemented error.
- the companion follow-up roadmap exists and excludes this slice from the
  remaining backlog.
