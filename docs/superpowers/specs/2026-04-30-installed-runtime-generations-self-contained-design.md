---
last_updated: 2026-04-30
revision: 3
summary: Design for making installed runtime generation exports boot from CAS-backed package inputs after migrating the active Fedora baseline to Fedora 44
---

# Installed Runtime Generations Self-Contained: Design Spec

**Date:** 2026-04-30
**Status:** Draft for user review (design direction approved in conversation)
**Goal:** Make installed runtime generation export bootable from explicit
CAS-backed generation inputs while keeping metadata-only and partial
generations fail-closed.

---

## Scope

This task covers the first follow-up slice from
[`docs/operations/post-generation-export-follow-up-roadmap.md`](../../operations/post-generation-export-follow-up-roadmap.md):
make installed runtime generations self-contained enough that
`conary system generation export` can produce a bootable raw/qcow2 image from
an installed generation without scraping the live host root during export.

It includes:

- migrating the active Fedora integration-test and example baseline from Fedora
  43 to Fedora 44 before adding the new installed-runtime QEMU case
- defining the ownership boundary between metadata-only and CAS-backed runtime
  packages
- treating `AdoptedFull`, `Taken`, `Repository`, and `File` packages as
  eligible generation inputs
- keeping `AdoptedTrack` packages metadata-only and excluded from generation
  root composition
- failing closed when the resulting runtime root is incomplete
- verifying that `/sbin/init` resolves through root usr-merge symlinks and
  package symlinks to an executable CAS-backed file
- requiring included regular files to have valid CAS ownership, and included
  symlinks to have valid target identity, before an exportable runtime artifact
  is published
- making full system adoption/takeover the explicit bridge into
  self-contained installed generations
- adding installed-runtime QEMU validation that boots an exported generation
  produced from CAS-backed runtime package state

It excludes:

- export-time fallback reads from the live host root
- weakening the current fail-closed behavior for partial generations
- converting `AdoptedTrack` into implicit CAS-backed content during generation
  export
- importing the whole live host root as an anonymous rescue package in this
  slice
- changing ISO, OCI, portable bundle, or boot artifact signing behavior
- removing the dracut legacy bind-mount fallback
- adding database schema migrations unless implementation discovers an
  unavoidable need
- rewriting archived plans, reviews, or dated validation records merely because
  they mention Fedora 43 as historical context

## Non-Goals

- producing a "best effort" image that only boots on the original host
- treating package-manager metadata digests as CAS ownership without verifying
  or storing the corresponding object
- silently omitting unresolved files from packages that otherwise claim to be
  CAS-backed generation inputs
- making the export command repair incomplete generations
- broadening the sandbox/live-root mutation story beyond the generation
  build/adoption boundary

---

## Repository Context

The landed generation export slice established the artifact contract used by
raw/qcow2 export:

- generation disk export lives under `conary system generation export`
- raw/qcow2 export loads a validated `GenerationArtifact`
- export projects a runtime rootfs and ESP from generation-local manifests and
  boot assets
- export does not scrape `/boot`, `/conary`, or the live host root
- ISO is reserved and returns an explicit not-implemented error

The QEMU-validated implementation point for that slice was
`065cf795 fix(generation): stabilize artifact export validation`; later docs
commits recorded the validation outcome and follow-up roadmap.

The relevant code paths are:

- `crates/conary-core/src/generation/builder.rs`
  - builds installed runtime `root.erofs`
  - filters out `AdoptedTrack` file entries
  - validates that `/sbin/init` resolves to an executable generation entry
  - stages boot assets at generation-build time
  - writes `.conary-artifact.json`, `cas-manifest.json`, and
    `boot-assets/manifest.json`
  - also owns `rebuild_generation_image`, which must share the same runtime
    input classification and validation as new generation builds
- `crates/conary-core/src/generation/artifact.rs`
  - writes and validates the generation artifact manifests
  - verifies CAS object hashes and sizes before export
- `crates/conary-core/src/generation/export.rs`
  - projects validated generation artifacts into raw/qcow2 images
- `apps/conary/src/commands/adopt/system.rs`
  - adopts native package-manager packages as either `AdoptedTrack` or
    `AdoptedFull`
- `apps/conary/src/commands/generation/takeover.rs`
  - upgrades `AdoptedTrack` to CAS-backed state
  - promotes packages through `AdoptedFull` and `Taken`
- `crates/conary-core/src/db/models/trove.rs`
  - defines `InstallSource::{File, Repository, AdoptedTrack, AdoptedFull,
    Taken}`
- `crates/conary-core/src/db/models/file_entry.rs`
  - stores file paths, hashes, modes, owner/group names, and symlink targets

The current fail-closed behavior is correct but incomplete as a product
milestone. An installed runtime generation fails if the filtered CAS-backed root
does not contain an executable `/sbin/init`. That prevents false bootable
exports, but it means the positive installed-generation export path is not yet
QEMU-validated.

The current weak point is the bridge from native package-manager state into
truthful CAS-backed runtime package state. Full adoption and takeover already
try to hardlink or copy files into CAS, but the generation builder should have
an explicit input validation layer that refuses to publish an exportable
artifact if eligible packages contain unresolved regular files or symlinks.

---

## Prerequisite: Fedora 44 Baseline Migration

Before implementing the self-contained installed-runtime generation path, move
the active Fedora baseline from Fedora 43 to Fedora 44. Fedora Linux 44 became
generally available on 2026-04-28, and the new installed-runtime QEMU validation
should not be introduced on an already-stale Fedora fixture.

The migration should update active code, test fixtures, CI defaults, and living
documentation as a dedicated preparatory step:

- rename the `conary-test` Fedora distro key from `fedora43` to `fedora44`
- replace `Containerfile.fedora43` with a Fedora 44 fixture based on
  `registry.fedoraproject.org/fedora:44`
- update the `ENV DISTRO` value and any other inline Fedora distro references
  inside the Containerfile, not only the `FROM` image tag
- update the existing `test_global_config_with_fedora()` and
  `test_app_state()` helpers in `apps/conary-test/src/lib.rs` so they create
  Fedora 44 fixture data, and rename them only if the implementation chooses
  versioned helper names
- update the inline `[distros.fedora43]` test configuration and assertions in
  `apps/conary-test/src/config/mod.rs`
- update remaining `apps/conary-test/src/` fixture and assertion strings that
  treat `fedora43` as the current configured distro, including server, handler,
  engine, and runner tests
- update hardcoded `Containerfile.fedora43` path references in
  `apps/conary-test/src/container/image.rs`,
  `apps/conary-test/src/container/lifecycle.rs`, and active operator docs such
  as `deploy/FORGE.md`
- update active integration manifests, distro overrides, cleanup repo names,
  and Remi/test fixture variables that refer to the Fedora 43 baseline
- update active GitHub workflow defaults, scheduled validation matrices, and
  single-distro job arguments
- update Remi test-database fixture data in `apps/remi/src/server/test_db.rs`
- update the RPM build container at `packaging/rpm/Containerfile.build` and its
  associated script comments in `packaging/rpm/build.sh`
- add a `fedora-44` entry to `data/distros.toml` with verified Fedora 44
  release and EOL dates; decide explicitly whether `fedora-43` remains in the
  catalog as a supported previous Fedora release, and if retained ensure it is
  not the default active Fedora baseline
- update active product examples and default distro entries from `fedora-43` /
  `Fedora 43` to `fedora-44` / `Fedora 44`
- update living docs, guides, README snippets, and site copy so they point users
  at Fedora 44
- update unit tests whose Fedora 43 strings are example fixture data rather than
  historical assertions

Historical records should stay factual. Archived plans, archived reviews, and
dated validation notes may continue to mention Fedora 43 when they describe
work that actually ran on Fedora 43. If an active document keeps a Fedora 43
reference for historical context, it should label that context explicitly rather
than presenting Fedora 43 as the current baseline.

Some source tests use `fedora-43` as semantic fixture data for source-selection,
replatform, policy, and identity behavior rather than as the current Fedora
baseline. Do not mechanically rewrite those unless the test itself is explicitly
about the current Fedora fixture. Initial skip-list candidates are:

- `crates/conary-core/src/repository/selector.rs`
- `crates/conary-core/src/db/models/trove.rs`
- `crates/conary-core/src/model/diff.rs`
- `crates/conary-core/src/model/replatform.rs`
- `crates/conary-core/src/packages/mod.rs`
- `crates/conary-core/src/repository/effective_policy.rs`

In user-facing docs, use Fedora 44 as the default Fedora example. If an example
needs an older Fedora release to illustrate multi-version repository behavior,
label it explicitly as a previous release rather than leaving it as the apparent
default.

For site copy, update the source under `site/src/`. Do not hand-edit generated
output under `site/build/` or `site/.svelte-kit/`; regenerate those artifacts
through the site build if tracked output needs to change.

Do not keep dual Fedora fixtures just for inertia. Add temporary `fedora43` and
`fedora44` support only if the Fedora 44 fixture exposes a concrete blocker
that must be debugged without losing the previous regression lane. A concrete
blocker is a defect in the Fedora 44 base image, kernel, or package set that
prevents the QEMU fixture from booting or completing integration tests. Conary
bugs exposed by Fedora 44 package changes are not blockers; fix them on the new
baseline instead of holding the fixture back.

---

## Decision

Use **strict package-level CAS completeness** for installed runtime generation
inputs.

The generation builder will treat install sources as follows:

| Install source | Generation role |
| --- | --- |
| `AdoptedTrack` | Metadata-only. Excluded from root composition and never repaired by export. |
| `AdoptedFull` | CAS-backed runtime input, but only if its included regular files resolve to valid CAS objects and symlinks validate against their stored targets. |
| `Taken` | Runtime input with the same validation as `AdoptedFull`. |
| `Repository` | Runtime input with the same validation as `AdoptedFull`. |
| `File` | Runtime input with the same validation as `AdoptedFull`. |

Full adoption or takeover is the explicit bridge from host package-manager
state into self-contained installed generations. If host filesystem import is
needed, it happens during adoption, CAS upgrade, or takeover. Export remains a
pure projection from an already-valid generation artifact.

Rejected alternatives:

- **Boot-critical closure only.** Rejected because it would prove only that
  `/sbin/init` can start, not that the generation is self-contained. Missing
  libraries, configuration, or service binaries would become runtime surprises.
- **Whole-root synthetic import package.** Deferred because it can be useful as
  an explicit rescue/import command later, but it weakens package provenance and
  bypasses the ownership ladder if used as the default path.
- **Export-time live-root fallback.** Rejected because it would recreate the
  false behavior the generation artifact export slice removed.

---

## Design

### 1. Runtime Input Classification

Add a focused runtime-generation input classification step before EROFS image
construction.

The classifier reads the installed troves and file entries from the database
and produces:

- included file entries from CAS-backed install sources
- included symlink entries from CAS-backed install sources
- metadata-only troves excluded because they are `AdoptedTrack`
- validation failures for CAS-backed troves whose file entries cannot be
  represented truthfully

The current `AdoptedTrack` filter remains, but it becomes an explicit policy
decision rather than an incidental part of file collection. A generation with
only metadata-only packages may still fail later because the root has no init,
and that is desirable.

The classifier should not write CAS objects. CAS writes belong to install,
full adoption, CAS upgrade, takeover, or an explicit future import command.

The classifier must use its own eligibility function.
`InstallSource::is_conary_owned()` has different semantics: it means
package-manager ownership and intentionally excludes `AdoptedFull`. Reusing it
for generation-input eligibility would drop the main bridge this design depends
on.

If the database schema cannot expose package symlink targets, the generation
build must return an error. An empty symlink set produced by a missing
`symlink_target` column is not a valid input for exportable runtime generation
root composition.

### 2. CAS Completeness Validation

For every included CAS-backed package, pre-build validation must classify each
included path by file type from the file mode bits stored on `FileEntry` and
reject entries the immutable EROFS generation root cannot represent truthfully.
Classification precedence is:

- a non-empty `symlink_target` means symlink regardless of mode bits
- an entry with symlink mode bits and no `symlink_target` is invalid
- an entry with directory mode bits and no `symlink_target` is a directory
- an entry with regular-file mode bits, or with no `S_IFMT` type bits set, is
  a regular file; the no-type-bits fallback preserves older tests and fixtures
  that store bare permission bits such as `0o755`
- any other file type inside the immutable generation root is
  non-representable and must fail clearly

The classifier must not infer directory status from the hash value alone. A
non-hex or placeholder hash on an entry classified as a regular file remains a
regular-file validation failure.

For regular files, validation must ensure:

- each regular file has a valid 64-character SHA-256 digest
- the digest must resolve under the generation CAS contract path
- the CAS object must exist
- the CAS object size must match the file entry size
- hashing the CAS object must reproduce the declared digest

Digest-shape validation must use the same parser as the EROFS builder,
`hex_to_digest` from `crates/conary-core/src/generation/builder/erofs.rs`, or a
shared helper derived from it. The validator and builder must not drift into
different definitions of a valid digest.

For symlinks:

- symlink entries must have a non-empty `symlink_target`
- `sha256_hash` must equal `CasStore::compute_symlink_hash(symlink_target)`,
  which is the SHA-256 digest of the raw target-path bytes
- symlinks are represented inline in EROFS metadata and do not require a CAS
  object in the object store
- symlink entries must not be included in `CasObjectRef` lists for
  `cas-manifest.json`
- the generation builder currently has no dependency on `CasStore`; adding one
  for `compute_symlink_hash` is a new cross-module dependency, so the
  implementation should verify that it introduces no circular dependency or
  feature-flag surprise

For directories and special files:

- directories are structural and do not require CAS content
- directory entries bypass digest-shape and CAS-object validation, and must not
  be passed to the EROFS builder as regular `FileEntryRef` entries; parent
  directories are synthesized by the builder when it inserts regular files and
  symlinks
- device nodes, FIFOs, and sockets in CAS-backed packages are not representable
  in the current EROFS generation builder and must be rejected if they are
  inside the immutable generation root
- silent skipping of non-representable included entries is not acceptable

For excluded paths:

- the authoritative exclusion policy is `EXCLUDED_DIRS` in
  `crates/conary-core/src/generation/metadata.rs`
- as of this spec, that excludes `var`, `tmp`, `run`, `home`, `root`, `srv`,
  `opt`, `proc`, `sys`, `dev`, `mnt`, and `media`
- `/etc`, `/usr`, and `/boot` are not excluded by that policy; package-owned
  content under those paths must therefore pass generation input validation
  unless a future design changes the exclusion set explicitly

The validation error should report package names and representative paths, with
a remediation message that points at `conary system adopt --system --full` or
`conary system takeover --up-to cas` as appropriate. The implementation can cap
path samples to keep output readable.

Validation has two timing layers:

- before EROFS construction, fail fast on file type, regular-file digest
  shape, symlink target, and symlink-hash errors
- before artifact manifest publication, verify regular-file CAS objects exist,
  sizes match, and contents re-hash correctly

For this slice, keep `write_generation_artifact` / `verify_cas_objects` as the
single CAS integrity gate for regular-file object existence, size, and content
rehashing. The pre-build validator should not re-hash every CAS object; it
should only prove that included entries are representable and have valid digest
shape. Moving CAS object verification earlier is deferred until profiling shows
the EROFS build cost dominates CAS re-hash cost for typical installed systems.
Pending generation cleanup should continue to remove incomplete directories.

### 3. Init Entrypoint Closure

Keep the existing `/sbin/init` contract and make it part of the same
self-contained-root validation:

- root-level usr-merge symlinks from `ROOT_SYMLINKS` are always considered
  present
- `ROOT_SYMLINKS` are static generation-root symlinks, not database entries,
  and have no file-entry hash to validate
- package symlinks from included CAS-backed packages are considered
- `/sbin/init` must resolve through those symlinks within the virtual
  generation root
- the final resolved path must be an included regular file
- that file must be executable and CAS-backed

This preserves the current fail-closed behavior while making the positive path
explicit. Common Fedora/systemd layouts such as `/sbin/init -> ../lib/systemd/systemd`
through usr-merge should pass when the relevant package has been fully adopted
or taken over.

### 4. Adoption And Takeover Boundary

`AdoptedFull` means more than "metadata with better hashes" for generation
purposes: it is a promise that package content has been written into Conary CAS.

The design does not require a new schema state for this slice. Instead, the
runtime-generation classifier verifies the promise mechanically from existing
file-entry and CAS data.

The later implementation plan should inspect and tighten these paths:

- `conary system adopt --system --full`
- `conary system takeover --up-to cas`
- `conary system takeover --up-to generation`
- track-to-CAS upgrade helpers used by takeover

For adoption-time edge cases:

- unreadable files in a full adoption must not be recorded as `AdoptedFull`;
  the preferred behavior is to fail that package adoption with the unreadable
  path while leaving successfully adopted packages intact under the existing
  best-effort bulk adoption model
- package-manager digests with non-SHA-256 algorithms must not be treated as
  generation CAS hashes
- special files outside the immutable generation model should be excluded only
  by explicit generation exclusion policy, not by silent hash parse failures

### 5. Artifact Publication And Export

The artifact writer stays behind the self-contained-root gate. New generation
builds and `rebuild_generation_image` must use the same gate, either by sharing
one helper or by making the duplication mechanically obvious in tests.

1. collect and classify runtime inputs
2. validate file type, regular-file digest shape, symlink targets, and symlink
   hashes for included package entries
3. validate `/sbin/init` closure
4. build `root.erofs`
5. stage boot assets from the generation build environment
6. verify regular-file CAS objects and write `cas-manifest.json` from the
   validated CAS object set
7. write `.conary-artifact.json`
8. write `.conary-gen.json`
9. clear the pending marker

Boot assets are staged from the host boot directory at generation-build time
and are not currently CAS-backed. This is an intentional exception to the
no-export-time-live-root-scraping rule: boot assets are captured while creating
the host-specific runtime snapshot, then export consumes only the staged
generation-local copies. Missing kernel, initramfs, or EFI bootloader assets
must fail the generation build before artifact publication; the QEMU fixture
must provide a boot environment that satisfies those inputs.

Export remains unchanged in principle. It should continue to fail if a
generation lacks artifact manifests or references missing CAS objects. Export
must not gain any fallback path that reads files from the current host root.

### 6. Installed Runtime QEMU Validation

Keep the existing Phase 3 Group O fail-closed case for a metadata-only runtime
generation. Add a positive installed-runtime case that proves:

- a fresh guest initializes Conary
- packages are promoted to CAS-backed state through the explicit bridge
- a generation build publishes `.conary-artifact.json`,
  `cas-manifest.json`, and `boot-assets/manifest.json`
- `conary system generation export --format qcow2` succeeds for the installed
  generation
- the exported qcow2 boots under QEMU
- the booted guest reports the expected generation and artifact files

The Fedora 44 baseline migration is a prerequisite for this installed-runtime
case. The existing bootstrap-run generation export test previously booted a
composefs-based generation under the Fedora 43 QEMU fixture, proving the kernel
baseline can support the generation export boot path; after the migration, the
bootstrap-run and installed-runtime cases should preserve that proof on the
current Fedora 44 baseline rather than introducing a separate kernel
assumption.

The QEMU test should avoid relying on export-time host scraping. Any package
content needed by the runtime root must already be CAS-backed before generation
build.

---

## Error Handling

The user-facing failure mode should be direct and repairable:

```text
exportable runtime generation is not self-contained: package systemd has
unresolved CAS-backed file /usr/lib/systemd/systemd. Run conary system adopt
--system --full for bulk adoption, conary system adopt <pkg> --full for a
single package, or conary system takeover --up-to cas before building this
generation.
```

For multiple failures, report a concise summary:

```text
exportable runtime generation is not self-contained: 12 CAS-backed file
entries are unresolved across 3 package(s); first unresolved path:
/usr/lib64/libc.so.6
```

Keep current hard failures for:

- missing executable `/sbin/init`
- invalid SHA-256 digests in included regular file entries
- missing CAS objects for included file entries
- CAS size or digest mismatches
- missing symlink targets for included symlink entries
- symlink hash mismatches against `CasStore::compute_symlink_hash`
- directory entries with placeholder or empty hashes are classified as
  directories and excluded from EROFS input; parent directories are synthesized
  by the builder from file and symlink paths
- non-representable device nodes, FIFOs, or sockets inside the immutable
  generation root
- missing boot assets
- missing artifact manifests during export

Warnings are acceptable for excluded `AdoptedTrack` packages before the final
self-contained-root check, but publishing an exportable artifact is not.

---

## Testing Strategy

Unit coverage should focus on pure classification and validation:

- `AdoptedTrack` packages are excluded from runtime root inputs
- `AdoptedFull`, `Taken`, `Repository`, and `File` packages are included
- invalid placeholder hashes on regular files in CAS-backed packages fail
  validation
- missing CAS objects fail validation
- CAS size and digest mismatches fail validation
- a CAS-backed package with mixed valid and invalid file entries reports the
  package name and first unresolved path, and does not publish an artifact
- directory entries bypass digest-shape and CAS-object validation instead of
  being rejected for placeholder hashes
- mode-bit and `symlink_target` disagreements follow the classification
  precedence rules: `symlink_target` wins, symlink mode without a target fails,
  and bare permission-only modes are treated as regular files
- symlinks with valid targets pass and contribute to init resolution
- symlink hash mismatches fail validation without requiring symlink CAS objects
- `/sbin/init` resolves through usr-merge and package symlinks to an executable
  CAS-backed file
- non-executable init targets fail
- excluded runtime paths from `EXCLUDED_DIRS` do not force CAS completeness
- device nodes, FIFOs, and sockets inside included paths fail clearly
- new generation builds and `rebuild_generation_image` enforce the same
  validation gate

`rebuild_generation_image` is `pub(crate)`, so parity coverage can live inside
`crates/conary-core/src/generation/builder.rs`'s existing `#[cfg(test)]` module
or use a test-only visibility lift if needed.

Package bridge coverage should exercise adoption/takeover helpers with fake
package-manager metadata where possible:

- full adoption stores regular files with SHA-256 CAS identity and symlinks
  with SHA-256 target identity
- track-to-CAS upgrade updates file hashes only after CAS storage succeeds
- failed file queries or unreadable regular files do not promote packages to
  `AdoptedFull`

Integration coverage should extend
`apps/conary/tests/integration/remi/manifests/phase3-group-o-generation-export.toml`:

- first migrate the active Fedora integration fixture and suite defaults to
  `fedora44`
- preserve the existing installed-generation fail-closed test
- include at least one negative case for the new validation, preferably a
  generation that fails before artifact publication or export because an
  included regular file's CAS object is missing or corrupt
- add a positive installed runtime export and boot test
- keep the bootstrap-run export boot test green

Before merge, run:

```bash
cargo fmt --check
cargo test -p conary-core generation
cargo test -p conary
cargo test -p conary-test config::manifest engine::variables engine::qemu
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3
```

During the Fedora baseline migration, also run a stale-reference sweep over the
active tree for `fedora43`, `fedora-43`, `Fedora 43`, and
`conary-fedora43`. Remaining matches should be either removed, updated to
Fedora 44, or explicitly justified as historical context or semantic fixture
data. The sweep should verify that no client scripts, Remi server
configuration, GitHub Actions jobs, or operator snippets still send `fedora43`
as a distro key to `conary-test serve` or `conary-test run`. Exclude generated
site output from hand edits, but regenerate it if the build artifacts are
tracked and need to reflect source changes.

Before treating the Fedora 44 baseline as ready, build or otherwise preflight
the Fedora 44 container fixture so the implementation catches missing base
images, package-name changes, or dnf install failures before the QEMU suite.
The exact focused test names can be narrowed in the implementation plan, but
the QEMU suite is the acceptance gate for claiming installed runtime generation
export is bootable.

---

## Acceptance Criteria

- Active Fedora integration-test, CI, fixture, product-example, guide, and site
  references use Fedora 44 as the current baseline.
- The Fedora 44 container fixture builds, selects `DISTRO=fedora44`, and is the
  default active Fedora fixture for `conary-test` runs.
- Remaining Fedora 43 references are historical, archived, semantic test data,
  retained previous-release catalog entries, or explicitly justified
  compatibility notes.
- `AdoptedTrack` and other partial metadata-only generations still fail closed.
- a runtime generation whose installed packages are all `AdoptedFull`, `Taken`,
  `Repository`, or `File` with valid CAS objects produces a bootable qcow2
  export under QEMU.
- `AdoptedFull`, `Taken`, `Repository`, and `File` packages are the only
  eligible runtime generation inputs.
- file type, regular-file digest shape, symlink target, and symlink-hash
  validation happens before `root.erofs` is built.
- regular-file CAS object existence, size, and digest validation happens before
  artifact manifests are published.
- `/sbin/init` resolves through usr-merge and package symlinks to a
  CAS-backed executable.
- Export never reads the live host root to repair missing runtime files.
- Installed-generation QEMU validation boots a qcow2 exported from a truly
  self-contained runtime generation.
- The existing bootstrap-run generation export validation remains green.

---

## Follow-Ups

This slice should leave these topics for later roadmap items:

- ISO export on the same artifact contract
- OCI export through `GenerationArtifact`
- signed portable generation bundles
- whole-root synthetic import/rescue packages
- boot artifact provenance and attestations
- removing the dracut legacy bind-mount fallback
- broader sandbox/no-host-mutation work
