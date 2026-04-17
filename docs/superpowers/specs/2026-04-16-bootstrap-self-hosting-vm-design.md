---
last_updated: 2026-04-16
revision: 1
summary: QEMU-first self-hosting bootstrap design for end-to-end Conary VM testing
---

# Bootstrap Self-Hosting VM Validation: Design Spec

**Date:** 2026-04-16  
**Status:** Draft for user review (design approved in conversation)  
**Goal:** Finish the bootstrap pipeline far enough to produce a QEMU-validated,
self-hosting VM image that lets an operator test `conary` top to bottom inside
the guest, including package operations, recipe cooking, and rebuilding
`conary` itself.

---

## Scope

This task covers the first truthful self-hosting bootstrap target for Conary.

It includes:

- turning Phase 6 / Tier 2 into a real executable bootstrap stage
- using the existing `recipes/tier2/*.toml` set as the starting point for that
  stage
- producing a **Tier-2-complete** `qcow2` guest image as the primary test
  artifact
- adding a checked-in QEMU validation path that boots the guest and proves
  `conary` works inside it
- exercising real remote infrastructure from inside the guest for package
  operations, instead of relying only on a baked-in fake repository
- validating that the guest can cook packages and rebuild/install `conary`
  inside itself
- documenting a follow-up VMware conversion/import path after the QEMU artifact
  is working

It excludes:

- a polished VMware-native artifact in the first milestone
- cloud image work, virtualization-provider metadata, or generic cloud-init
  support
- turning the bootstrap image into a production operating system release
- solving every possible bootstrap reproducibility problem beyond what this
  self-hosting validation path needs
- making Tier 2 mandatory for every bootstrap use case; a minimal bootable base
  system is still a valid bootstrap output

## Non-Goals

- treating the Phase 5 base image as sufficient for top-to-bottom `conary`
  validation
- baking private credentials or operator-specific secrets into the VM artifact
- hard-coding one public repository forever as the only acceptable guest-side
  source of packages
- inventing a second package-installation path for Tier 2 that bypasses the
  bootstrap recipe machinery
- adding VMware packaging before the QEMU validation path is reliable

---

## Repository Context

The repo already has a six-phase bootstrap model:

- [docs/modules/bootstrap.md](../../modules/bootstrap.md) describes:
  - Phase 1: cross-tools
  - Phase 2: temp-tools
  - Phase 3: final system
  - Phase 4: system configuration
  - Phase 5: bootable image
  - Phase 6: Tier 2 (BLFS + Conary self-hosting)
- [crates/conary-core/src/bootstrap/stages.rs](../../crates/conary-core/src/bootstrap/stages.rs)
  still treats `Tier2` as optional today
- [apps/conary/src/commands/bootstrap/mod.rs](../../apps/conary/src/commands/bootstrap/mod.rs)
  already exposes `conary bootstrap image` and prints QEMU usage for `qcow2`
  output

The self-hosting gap is not conceptual; it is implementation detail and
validation truthfulness:

- [crates/conary-core/src/bootstrap/tier2.rs](../../crates/conary-core/src/bootstrap/tier2.rs)
  defines Tier 2 as BLFS + Conary self-hosting
- the intended Tier 2 package order already exists:
  - `linux-pam`
  - `openssh`
  - `make-ca`
  - `curl`
  - `sudo`
  - `nano`
  - `rust`
  - `conary`
- `Tier2Builder::build_all()` currently returns `NotImplemented`
- the bootstrap orchestrator explicitly skips marking Tier 2 complete when that
  happens

The recipe inventory is partly present but not yet trustworthy enough to claim a
self-hosting guest:

- `recipes/tier2/` already contains all eight recipe files
- several Tier 2 recipes still use placeholder checksums, for example:
  - `recipes/tier2/rust.toml`
  - `recipes/tier2/linux-pam.toml`
  - `recipes/tier2/sudo.toml`
- `recipes/tier2/conary.toml` assumes the bootstrap pipeline will copy the
  workspace into the build directory before invoking the recipe, but no
  end-to-end Tier 2 implementation currently enforces that contract
- some Tier 2 test-access behavior is currently expressed inside the package
  recipes themselves, especially `openssh.toml`, rather than through a separate
  guest validation profile

The existing image builder is close to what this project needs:

- [crates/conary-core/src/bootstrap/image.rs](../../crates/conary-core/src/bootstrap/image.rs)
  already supports `raw`, `qcow2`, `iso`, and `erofs`
- [apps/conary/src/commands/bootstrap/mod.rs](../../apps/conary/src/commands/bootstrap/mod.rs)
  already treats `qcow2` as a first-class QEMU testing target
- there is no first-class VMware artifact or import flow yet

That means the right next step is not “invent VM support from scratch.” It is:

1. make Tier 2 real
2. produce a Tier-2-complete `qcow2`
3. prove that the guest is self-hosting under QEMU

---

## Decision

Use **QEMU-first self-hosting validation** as the first truthful VM-testing
target for bootstrap.

This means:

- Tier 2 becomes a real recipe-driven stage, not a stub
- the primary operator artifact is a `qcow2` image produced from a
  Tier-2-complete sysroot
- QEMU is the first-class acceptance environment
- guest validation uses existing remote infrastructure, but without baking
  private secrets into the image
- “top to bottom” means the guest can:
  - boot
  - reach the configured remote package infrastructure
  - perform real `conary` package operations
  - cook packages inside the VM
  - rebuild and use `conary` itself inside the VM
- VMware support is a documented follow-up conversion/import path after the
  `qcow2` artifact is trustworthy

Rejected alternatives:

- **Post-bootstrap provisioning only**
  - rejected because it would let the bootstrap pipeline keep claiming success
    before the guest is actually self-hosting
- **VMware-first**
  - rejected for the first milestone because it adds hypervisor-specific output
    complexity before the core self-hosting contract is proven
- **Bake in a fake local repo**
  - rejected because the operator explicitly wants real end-to-end testing
    against existing infrastructure, not only an isolated demo path

---

## Design

### 1. Tier 2 Becomes The Self-Hosting Closure

Tier 2 should be the stage that takes a bootable base system and makes it
capable of managing, building, and rebuilding Conary from inside itself.

For this design, a successful Tier 2 run means:

- all eight Tier 2 packages are installed into the sysroot in a defined order
- the resulting sysroot contains a usable Rust toolchain
- the resulting sysroot contains a usable `conary` binary
- the resulting sysroot contains the networking/auth/runtime packages needed to
  support in-guest testing
- the stage fails closed if the source or install contract is incomplete

Tier 2 should continue to use the existing bootstrap recipe model rather than a
special-case installer. The implementation should build on:

- `PackageBuildRunner`
- the existing `recipes/tier2/*.toml` files
- the current bootstrap work directory and source cache layout

Tier 2 should **not** be treated as “optional” for the self-hosting VM path.
It may remain optional for generic bootstrap users who only want a minimal base
OS, but any checked-in “build me a VM to test Conary” path must require Tier 2
success before claiming completion.

### 2. Tier 2 Needs A Tighter Source And Install Contract

The current Tier 2 recipe set mixes three different concerns:

- package installation into the sysroot
- bootstrap/test-access conveniences
- assumptions about the source tree for `conary`

The design should separate those concerns.

#### 2.1 Checksum policy

Tier 2 must fail closed on placeholder checksums by default.

That means:

- no Tier 2 recipe may quietly proceed with `VERIFY_BEFORE_BUILD` or similar
  placeholders during normal operation
- a development escape hatch such as `--skip-verify` may remain, but it must be
  visibly noisy and must not be the default success path for the self-hosting
  VM artifact

#### 2.2 `conary` source handoff

The `conary` Tier 2 recipe should not rely on undocumented magic.

The bootstrap pipeline must explicitly define how the source tree is handed into
the Tier 2 build:

- a filtered workspace snapshot or source bundle is created from the current
  checkout
- that bundle is staged into the Tier 2 build workspace in a deterministic
  location
- the `conary` recipe builds against that staged source, not against the live
  host checkout by accident

For this milestone, the build target is “the current local Conary source tree
under test,” not “a published release tarball.” The operator is trying to test
the in-repo code end to end.

#### 2.3 Sysroot-safe installation semantics

Tier 2 recipes must be valid for “install into a future guest filesystem,” not
for “mutate the live host.”

That means the design should explicitly avoid relying on host-global side
effects from inside the package recipes. In particular:

- enabling systemd units should be expressed as sysroot file/link creation, not
  as reliance on a live systemd instance
- guest users/groups needed by packages should be created through a controlled
  sysroot mutation path, not by assuming the host account database is the right
  place to mutate
- package recipes should not bake permanent operator/test credentials into the
  image

Where package install steps truly need a runtime/testing overlay rather than a
generic package install action, that work belongs in the guest validation
profile, not in the package recipe itself.

### 3. Guest Validation Profile Is Separate From Tier 2 Package Installation

The self-hosting VM needs two layers:

1. the Tier 2 package set
2. a guest validation profile that makes the image reachable and testable

Those are not the same thing.

The guest validation profile should be a dedicated, checked-in layer that
prepares the Tier-2-complete sysroot for VM testing. It should own:

- enabling and configuring SSH for guest access
- generating or triggering generation of SSH host keys without embedding
  long-lived private material in the repo
- installing an ephemeral or operator-provided public key for test access
- any guest-only “ready for validation” unit/service hooks
- bootstrap-time repository/trust configuration needed for the selected remote
  infrastructure

This design intentionally keeps that profile out of the package recipes
themselves, so the recipes stay about package installation and the validation
overlay stays about “how do we get into and test this VM.”

The validation profile should be explicitly marked as a **test image profile**,
not a production security posture.

### 4. The Final Artifact Is A Tier-2-Complete `qcow2`

The operator-facing deliverable for this project is not merely “a base system
that once produced a bootable image.” It is a `qcow2` built from the sysroot
after Tier 2 and the guest validation profile have been applied.

The design allows the existing Phase 5 image builder to stay the imaging
mechanism, but the user-facing flow must guarantee that:

- the image is emitted from the Tier-2-complete sysroot
- the operator does not accidentally validate a stale pre-Tier-2 image

The checked-in path for this should be a single-entry build/validation flow.
This design prefers a checked-in wrapper script or equivalent orchestrated entry
point over adding broad new bootstrap CLI surface immediately, because the core
risk is Tier 2 correctness and guest validation, not CLI taxonomy.

That single-entry path must make the ordering explicit:

1. complete base bootstrap stages
2. run Tier 2
3. apply the guest validation profile
4. emit the `qcow2`
5. boot and validate it under QEMU

### 5. QEMU Validation Is The Acceptance Test

The repo should gain a checked-in QEMU validation path that proves the finished
artifact is actually self-hosting.

The validation path should:

- boot the produced `qcow2`
- wait until guest access is available
- run guest-side checks over SSH or another explicit access channel
- collect logs and return a clear pass/fail result

The guest-side checks should prove the “top to bottom” contract:

1. the guest boots and is reachable
2. the guest can access the chosen remote infrastructure
3. the guest can configure repository/trust state needed for package operations
4. `conary` can perform representative install/update/remove/query operations
5. the guest can cook at least one representative package/recipe
6. the guest can rebuild `conary` itself from source
7. the rebuilt `conary` binary runs successfully inside the guest

The validation output should be a checked, operator-readable artifact rather
than a requirement to scroll through unstructured console output.

### 6. Real Remote Infrastructure Is An Input, Not A Hidden Assumption

The operator explicitly wants the guest to exercise real infrastructure rather
than only a baked-in local repo.

That means the validation design must make those inputs explicit:

- which repository/remi endpoint(s) are used
- what trust bootstrap material is required
- what part of that input is checked into the repo and what part is supplied at
  validation time

This design should not assume that “whatever happens to be public today” is a
safe implicit contract.

Instead, the checked-in validation flow should define a small, explicit input
surface such as:

- repository URL / Remi endpoint
- optional TUF root metadata path or equivalent trust bootstrap material
- optional guest-side public key for SSH access

If those inputs are missing or invalid, guest validation must fail with a
specific infrastructure/configuration error, not with a vague Tier 2 success
claim followed by hand-wavy caveats.

### 7. VMware Is A Follow-Up Packaging Layer

Once the QEMU path is truthful and repeatable, the next step is to document and
optionally automate a VMware import story.

That follow-up may use:

- `qemu-img convert` into `vmdk`
- a small OVF wrapper
- a documented VMware import procedure

But none of that should block the first self-hosting milestone.

The first milestone is successful when the QEMU-validated `qcow2` is real and
useful. VMware support should reuse that proven artifact rather than forcing a
new artifact format into the critical path prematurely.

---

## Failure Model

This design should fail clearly rather than claim partial success.

Failure conditions include:

- any Tier 2 recipe required for self-hosting still has placeholder checksums
  and the operator did not explicitly choose a development override
- the `conary` source handoff into Tier 2 is ambiguous or broken
- the post-Tier-2 image artifact does not actually contain the Tier 2 closure
- the guest boots but cannot reach the configured remote infrastructure
- package operations work but recipe cooking or `conary` self-rebuild fails
- the image only boots before Tier 2 but regresses after Tier 2 is applied

The implementation should report which class of failure occurred:

- Tier 2 build failure
- image emission failure
- guest access/profile failure
- remote infrastructure/trust failure
- in-guest `conary` functional failure
- in-guest self-hosting rebuild failure

---

## Verification Strategy

Verification should happen at three levels.

### 1. Local structural checks

- Tier 2 recipe inventory loads cleanly
- no required Tier 2 recipe uses placeholder checksums in the default path
- source handoff for the `conary` recipe is validated before the build starts
- guest validation input parsing and script generation are tested

### 2. Bootstrap-stage checks

- `Tier2Builder` executes packages in the declared order
- the Tier 2 stage writes enough state to prove the sysroot is self-hosting
  capable
- the wrapper/orchestrated flow refuses to emit the “test this in a VM” image
  unless Tier 2 completed successfully

### 3. Guest acceptance checks

- boot the produced `qcow2` under QEMU
- establish guest access
- run the checked guest-side validation script
- store the result and enough logs to debug failures without rerunning blindly

For this design, the QEMU guest validation is the authoritative acceptance
signal. Unit tests and dry-runs are useful, but they do not by themselves prove
that the image is self-hosting.

---

## Risks And Tradeoffs

- **Long runtime and heavy host requirements**
  - a full self-hosting build plus guest validation will be expensive; this is
    expected and should be treated as a deliberate validation path, not a cheap
    CI smoke test
- **Remote infrastructure drift**
  - using real infrastructure improves truthfulness but increases external
    variability; that is why the validation inputs must be explicit and logged
- **Recipe/install semantics mismatch**
  - the current Tier 2 recipes include bootstrap-friendly behavior that is not
    cleanly separated from package installation; this design addresses that by
    moving guest-access concerns into a validation profile
- **Stage model tension**
  - the current six-phase language describes Phase 5 image generation before
    Phase 6 Tier 2; this design preserves that historical model but requires a
    post-Tier-2 image artifact for self-hosting validation
- **VMware delay**
  - VMware users wait slightly longer for a first-class artifact, but the
    resulting conversion/import path is built on a proven `qcow2` instead of on
    an unvalidated parallel artifact

---

## Success Criteria

This project is complete when all of the following are true:

1. `conary bootstrap tier2` is a real executable stage rather than a stub.
2. The default self-hosting path does not rely on placeholder checksums for
   required Tier 2 recipes.
3. The pipeline has a checked-in, single-entry path that builds a
   Tier-2-complete `qcow2` and validates it under QEMU.
4. The produced guest can reach the configured real remote infrastructure using
   explicit, documented trust/configuration inputs.
5. Inside the guest, `conary` can perform representative query/install/update/
   remove operations.
6. Inside the guest, at least one representative recipe can be cooked
   successfully.
7. Inside the guest, `conary` can be rebuilt from source and the rebuilt binary
   runs successfully.
8. The operator can use the resulting `qcow2` as the foundation for later
   VMware conversion/import work without needing a separate bootstrap design.
