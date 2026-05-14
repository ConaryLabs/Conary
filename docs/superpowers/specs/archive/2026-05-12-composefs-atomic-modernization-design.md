---
last_updated: 2026-05-14
revision: 3
summary: Historical design record for the completed composefs atomic switching modernization
---

# Composefs Atomic Modernization: Design Spec

**Date:** 2026-05-12
**Status:** Completed and archived on 2026-05-14 after implementation,
validation, merge, and push to `main` as `db938294`.
**Goal:** Make composefs atomic generations the only supported runtime contract
for Conary's package mutation, activation, recovery, export, and bootstrap
paths, removing legacy live-root and compatibility behavior that is no longer
needed before the limited public release.

---

> **Historical note:** Current active docs such as `docs/ARCHITECTURE.md`,
> `docs/conaryopedia-v2.md`, and
> `docs/operations/post-generation-export-follow-up-roadmap.md` are
> authoritative for the implemented next-boot selection model.

## Scope

This design covers the repo-wide modernization pass requested after the
composefs consistency audit. The limited public release has not happened yet,
and this repository is still under active development by a single primary user,
so the design intentionally favors one forward-looking contract over
compatibility layers.

The modernization covers:

- CCS, Remi, local `.ccs`, and converted legacy package installs
- install, remove, rollback, state restore, and generation takeover behavior
- `/conary` versus `/var/lib/conary` runtime data-root policy
- live generation switching versus boot-time activation
- dracut/initramfs boot activation
- recovery from interrupted transactions and damaged generation state
- raw, qcow2, OCI, and future image projections
- convergence and takeover defaults that currently preserve pre-generation
  compatibility behavior
- tests, docs, and validation gates that prove older pathways are gone

It excludes:

- implementing ISO export itself
- introducing portable signed generation bundles
- solving provider-specific image formats such as VMDK/OVF
- redesigning Remi's storage or federation model
- replacing CCS as the native package artifact format
- making conaryd execute package transactions in this slice

## Target Contract

Conary has one supported runtime mutation contract:

1. resolve package/source intent
2. verify trust and policy
3. store package content in CAS
4. commit the SQLite DB changeset
5. build a complete generation directory containing `root.erofs`, generation
   metadata, scoped CAS manifest, boot assets, and `.conary-artifact.json`
6. mount or stage that generation through composefs using the declared CAS
   basedir and verity policy
7. update the atomic active-generation pointer

Public commands must not use direct live-root file writes as their primary
mutation mechanism. They must not accept partial generation directories as
bootable runtime state. They must not silently downgrade from the artifact's
declared integrity requirements. They must not preserve stop-points whose only
purpose is to keep an older package-manager sidecar workflow alive.

Bootstrap may still construct mutable build sysroots internally, but the
runtime artifact it publishes must be the same complete generation artifact
contract consumed by activation, recovery, and export.

## Current Inconsistencies

### CCS Install Bypasses Composefs

`apps/conary/src/commands/ccs/install.rs` still describes itself as a minimal
future-transaction implementation, deploys files directly into `root`, then
updates the DB. Remi packages, local `.ccs` packages, and legacy packages
converted to CCS can reach this path through `install_converted_ccs()`.

Decision:

- remove direct filesystem deployment from the CCS install command path
- make CCS package install prepare a normal `PackageFormat` input and reuse
  the shared install transaction lifecycle
- preserve CCS signature, capability, dependency, and hook policy checks before
  the DB commit
- run post-install hooks/triggers after the generation is built and activated,
  using the same ordering model as legacy package install

### Runtime Data Root Is Split

The CLI default DB path and transaction config derive runtime state from
`/var/lib/conary`, while generation helpers and switching/export paths hard-code
`/conary`. This creates two possible generation trees and two possible
current-generation pointers.

Decision:

- define `ConaryRuntimeRoot` in core as the single authority for DB, objects,
  generations, mount state, `/etc` overlay state, GC roots, and active pointer
- keep `/conary` as the boot-visible runtime root for generation directories,
  CAS objects, mount state, and `current`
- keep `/var/lib/conary/conary.db` as the default SQLite DB location unless a
  later storage decision moves it deliberately
- remove ad hoc path helpers that infer generation paths from unrelated DB
  paths
- require commands that operate on installed generations to accept a runtime
  root explicitly or use the core default

This keeps boot artifacts stable at `/conary` while avoiding accidental
`/var/lib/conary/generations` generation trees.

### Live Switch Is Debug-Only

Release-facing generation switching selects a complete generation artifact for
the next boot. The remaining `switch_live()` path is debug/unsafe machinery: it
loads the same generation artifact contract, uses the same runtime root and
verity policy, and fails hard if `/etc` overlay setup fails.

Decision:

- make boot-time generation activation the supported public activation model
- remove live switching from release-facing commands or mark it explicitly
  debug/unsafe
- if a debug live-switch path remains, it must fail hard on `/etc` overlay
  failure and share the same runtime root and verity policy as boot activation
- rollback should update the active generation pointer and boot entry rather
  than pretending a live root can be made fully coherent after arbitrary
  package changes

### Dracut Allows Legacy Generation Directories

`packaging/dracut/90conary/conary-generator.sh` falls back to bind-mounting
`usr` and `etc` from a generation directory when `root.erofs` is absent.

Decision:

- missing `root.erofs` is a hard boot activation failure
- generation directories without `.conary-artifact.json` and valid metadata are
  not bootable runtime state
- verity fallback behavior must follow generation metadata and feature probing,
  not a blind retry that hides an integrity mismatch

### OCI Export Has A Parallel Generation Loader

Raw and qcow2 export consume `GenerationArtifact`; top-level OCI export
independently resolves `/conary/current`, metadata, `root.erofs`, and CAS
objects.

Decision:

- move OCI generation source loading onto `GenerationArtifact`
- keep OCI media-layout emission separate from disk-image emission
- decide during implementation whether the public command remains `conary
  export` or moves under `conary system generation export --format oci`
- all image projections must derive identity labels and source digests from the
  same generation artifact fields

### Recovery Promotes Generation Artifacts

Transaction recovery promotes only valid generation artifacts. The old
magic-number promotion helper is gone; recovery now validates artifact metadata,
content hashes, CAS scope, and verity policy before mounting or scanning a
generation.

Decision:

- recovery must prefer a valid active pointer plus generation artifact metadata
- DB rebuild recovery must publish or repair a complete generation artifact
  before activation
- generation scanning must skip partial or unverifiable generation directories
- verity-enabled generations must recover with the same verity requirements
  used by normal activation

### No-Generation Fallbacks Mutate The Live Root

Remove and rollback paths still remove or restore files directly when no active
generation exists.

Decision:

- release-facing remove and rollback require an initialized active generation
- the error should explain how to initialize/build a generation instead of
  silently mutating the host root
- any remaining direct live-root restoration helper must be test-only or
  bootstrap-internal, with names that make that boundary obvious

### Takeover And Convergence Preserve Compatibility Stop-Points

`ConvergenceIntent::TrackOnly`, dependency `Satisfy`, and takeover phases that
stop at CAS/Owned preserve older sidecar states as normal user-facing choices.

Decision:

- limited-preview defaults should move toward generation-backed ownership
- `generation` should be the normal public takeover outcome
- lower takeover phases may remain as internal/debug checkpoints, but release
  docs and prompts should not present them as the primary path
- dependency defaults should prefer CAS-backed or full-ownership behavior where
  the system model asks Conary to own runtime state

### Build And Bootstrap Have Transitional EROFS Paths

Derivation/build environments and bootstrap still use mutable sysroots where
that is appropriate for construction, and some build environments can fall back
to plain EROFS seed mounts.

Decision:

- mutable sysroots are acceptable only as construction inputs
- published bootstrap output must be a complete generation artifact
- plain EROFS seed handling must either be documented as construction-only or
  replaced by a CAS-backed artifact once seeds are represented by the runtime
  contract

## Architecture

### Core Runtime Root

Add a small core-owned runtime-root abstraction under `crates/conary-core` that
returns canonical paths for:

- SQLite DB
- CAS objects
- generations
- active generation pointer
- composefs mount state
- `/etc` overlay upper/work state
- GC roots
- boot asset staging, when applicable

Command code should stop constructing those paths from string literals. Tests
should be able to inject a temporary runtime root without changing production
defaults.

### Package Mutation Flow

The existing shared install path already has the right shape:

- parse package into `PackageFormat`
- extract/classify selected files
- run pre-install checks/scriptlets
- call `install_inner()` for CAS and DB writes
- call `composefs_ops::rebuild_and_mount()`
- run post-install scriptlets/triggers

CCS install should become a thin wrapper around that flow. CCS-specific work is
front-loaded into preparation:

- verify signatures unless explicitly allowed for local conversion
- evaluate capability policy
- resolve dependencies using the existing policy-aware logic
- translate selected CCS components into the shared `ComponentSelection`
- expose declarative CCS hooks as scriptlets or a dedicated post-generation
  hook phase with the same transaction status semantics

No CCS command should write package files directly to the root filesystem.

### Activation Flow

Activation has two supported forms:

- boot activation from `/conary/current` and a complete generation artifact
- debug activation used only by tests or explicit developer commands

The public operational story is boot activation. Debug activation, if kept,
must be named as such and must fail closed on mount, overlay, or verity errors.

### Recovery Flow

Recovery should use the same evidence as normal activation:

1. load the active generation pointer
2. load and validate the generation artifact
3. mount with the artifact's CAS basedir and verity policy
4. if artifact validation fails, rebuild from DB into a fresh complete
   generation artifact
5. if DB rebuild fails, scan only complete generation artifacts in descending
   order

Magic-number-only checks may remain as a low-level diagnostic helper, but not
as sufficient recovery evidence.

### Export Flow

All generation image exports consume `GenerationArtifact`:

- raw
- qcow2
- OCI
- future ISO
- future portable bundle

Backends stay format-specific. Source loading, metadata validation, CAS
scoping, boot asset validation, and identity labels are shared.

## Error Handling

The modernization should replace compatibility behavior with explicit failures:

- no active generation: fail with the command needed to initialize/build one
- missing `root.erofs`: fail boot activation and recovery
- missing artifact manifest: fail export, activation, and recovery
- CAS object missing or hash mismatch: fail before publishing or activating
- requested verity cannot be honored: fail when metadata declares verity ready
- live debug switch cannot mount `/etc`: fail and leave the previous generation
  active
- unsupported image format: fail with a reserved-format error on the same
  artifact contract

Warnings are acceptable for post-commit non-critical hooks, but not for
generation integrity, activation completeness, or runtime root selection.

## Testing And Validation

Each implementation slice needs a red-green test or manifest gate that proves
the removed behavior is gone.

Required coverage:

- CCS install does not write selected package files into the live root before
  generation rebuild
- Remi/local `.ccs`/converted package installs call the shared transaction
  path
- generation helpers and commands use the same runtime root
- missing `root.erofs` in dracut fails instead of bind-mounting `usr`/`etc`
- recovery rejects partial generation directories and honors verity metadata
- OCI export loads `GenerationArtifact`
- remove/rollback fail when no active generation exists instead of mutating the
  live root
- takeover docs/defaults lead to generation-backed ownership for preview
- bootstrap export still produces a self-contained generation artifact

Fast verification after focused changes:

```bash
cargo fmt --check
cargo test -p conary-core
cargo test -p conary
cargo run -p conary-test -- list
cargo clippy --workspace --all-targets -- -D warnings
```

Release-gate verification after the implementation plan lands:

```bash
cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3
```

Additional focused manifests should be added when an old fallback is removed
and no existing manifest exercises that failure mode.

## Rollout Order

### Phase 1: CCS And Install Unification

Unify CCS, Remi, local `.ccs`, and converted package installs onto the shared
install transaction and composefs rebuild path. This removes the largest
release-facing duplicate mutation mechanism first.

### Phase 2: Runtime Root Canonicalization

Introduce the core runtime-root abstraction, update generation helpers and CLI
commands to use it, and remove hard-coded `/conary` versus DB-derived
generation path drift.

### Phase 3: Strict Boot Activation

Remove dracut's legacy bind-mount fallback, make missing `root.erofs` fatal,
and decide the final public status of live generation switching.

### Phase 4: Recovery And Rollback Tightening

Move recovery to artifact/metadata validation and remove no-generation
live-root remove/rollback fallbacks from release-facing commands.

### Phase 5: Export Unification

Move OCI generation export to `GenerationArtifact` and keep raw/qcow2 behavior
green.

### Phase 6: Defaults, Takeover, And Docs

Align convergence/takeover defaults and docs with generation-backed ownership
as the preview path. Update assistant-facing and user-facing docs after the
code behavior is real.

### Phase 7: Full Validation

Run fast workspace gates, then the relevant local/QEMU validation suites. Keep
the existing generation export evidence green while adding checks for removed
fallbacks.

## Acceptance Criteria

The modernization is complete when:

- every public package install path, including CCS and Remi, reaches the shared
  CAS/DB/generation apply lifecycle
- generation commands, recovery, activation, and export agree on one runtime
  root policy
- dracut does not boot partial generation directories
- release-facing remove and rollback do not directly mutate a live root because
  no generation exists
- recovery never promotes a generation using only EROFS magic validation
- OCI export shares the generation artifact loader with raw/qcow2 export
- release-facing takeover/default docs present generation-backed ownership as
  the path forward
- focused regression tests prove the removed fallback paths fail closed
- `cargo fmt --check`, focused package tests, `cargo run -p conary-test -- list`,
  `cargo clippy --workspace --all-targets -- -D warnings`, and the active
  generation export QEMU gate pass from the final integrated branch

## Implementation Planning Decisions

These are narrow implementation choices, not open questions about the target
contract:

- Phase 6 resolves the preview system model default to
  `ConvergenceIntent::CasBacked`; `FullOwnership` remains available as an
  explicit stronger ownership intent.

Remaining choices for later slices:

- whether CCS declarative hooks become shared scriptlets or a separate
  post-generation hook phase
- whether live generation switch is deleted from public CLI or retained behind
  an explicit debug/unsafe command
- whether OCI stays at `conary export` or moves under
  `conary system generation export --format oci`

The implementation plan should resolve each remaining choice before code
changes begin.
