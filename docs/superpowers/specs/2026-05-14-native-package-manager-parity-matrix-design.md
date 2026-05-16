---
last_updated: 2026-05-14
revision: 2
summary: Review-hardened design for Conary-owned package-manager parity against dnf, apt, and pacman expectations for the limited public preview
---

# Native Package Manager Parity Matrix: Design Spec

**Date:** 2026-05-14
**Status:** Review-patched design direction with Slice A implementation plan
**Goal:** Make Conary-owned package operations feel like a credible daily package-manager lane, not a step down from dnf, apt, or pacman, for Fedora 44, Ubuntu 26.04 LTS, and Arch Linux.

---

## Recommended Codex Goal Decomposition

Do not launch the full Tier 0 + Tier 1 matrix as a single implementation goal. The review pass found that no-generation live-host mutation is structural work, not a small CLI cleanup. Use these `/goal` slices in order:

```text
/goal Slice A: Implement the no-generation live-host package operation foundation for Conary-owned install and remove. Add the live-root transaction engine or equivalent safe path that writes/removes package files on mutable live hosts without a selected Conary generation; preserve the generation-aware path for active-generation hosts; introduce the shared PackageOperationOutcome contract and deferred-follow-up history metadata; prove with unit and CLI tests.

/goal Slice B: Implement Conary-owned update parity on top of Slice A. Ensure update works on no-generation live hosts, treats adopted packages as native-authoritative unless takeover is explicit, blocks critical takeover/remove/update cases, reports partial multi-package outcomes truthfully, and refuses or reports security-metadata-unavailable sources before mutation. Prove with unit, CLI, and distro tests.

/goal Slice C: Implement Tier 1 daily-driver command parity. Make search/list/info/files/path/pin/unpin/pinned/autoremove/system history/query whatprovides/query whatbreaks/repo list/repo sync share the same package selector, authority, pin, dependency, and ambiguity contracts as mutation commands. Prove with CLI and integration tests.

/goal Slice D: Add the conary-test native package-manager parity matrix. Create a named suite visible from `cargo run -p conary-test -- list` that proves Tier 0 and Tier 1 across Fedora 44/RPM, Ubuntu 26.04/DEB, and Arch package format with explicit step assertions, zero failed/skipped/cancelled result gates, forced deferred-follow-up tests, and exact native fixture metadata checks.
```

The slices are intentionally ordered. Slice B depends on Slice A because `update` delegates through install. Slice D depends on A, B, and C because the matrix must prove the final user-facing contract rather than merely compile.

Slice A plan: `docs/superpowers/plans/2026-05-14-native-package-manager-parity-slice-a-plan.md`.
Slice B plan: `docs/superpowers/plans/2026-05-15-native-package-manager-parity-slice-b-plan.md`.
Slice C plan: `docs/superpowers/plans/2026-05-15-native-package-manager-parity-slice-c-plan.md`.

## Product Promise

The limited public preview should let a user try Conary as the package manager for packages Conary owns without feeling an obvious downgrade from the native manager.

For Conary-owned packages, the ordinary package-manager loop must work:

1. Find a package.
2. Inspect it.
3. Install it.
4. List and query what was installed.
5. Pin or unpin it.
6. Update it.
7. Remove it.
8. Clean orphaned dependencies.
9. See history and dependency explanations.

Immutable generations remain one of Conary's advantages, but generation work must not make normal package operations look failed after package mutation has already succeeded. Generation-specific commands can stay generation-strict. Package-manager commands need a package-manager success boundary.

On hosts without a selected Conary generation, that boundary requires a real mutable live-root package path. Updating SQLite and CAS alone is not enough: users must see installed files on disk immediately, and remove/update must mutate those files through a recoverable live-root transaction path.

## Tier Matrix

### Tier 0: Preview Blockers

Tier 0 covers behavior that would make Conary feel unsafe, fake, or obviously worse than dnf, apt, or pacman during limited-preview package ownership.

- Conary-owned `install`, `remove`, and `update` must work on normal mutable live hosts without requiring a selected Conary generation.
- No-generation live-host support must include required filesystem mutation, not only DB/CAS bookkeeping or skipped generation rebuilds.
- A package operation that completed its required DB/filesystem mutation must not exit as failed solely because optional state snapshot or generation follow-up failed.
- Unsupported or dangerous cases must refuse before mutation.
- Adopted packages must remain native package-manager authoritative unless explicit takeover is requested.
- Critical system packages and critical runtime capabilities remain protected.
- Security-only update behavior must be truthful per source. If the source cannot answer security metadata, Conary must say so instead of pretending all security updates are applied or absent.
- Pinned packages, dependency breakage, ambiguous variants, missing repo metadata, missing live-mutation acknowledgement, and unsupported source transitions must fail before mutation.

### Tier 1: Daily Driver Basics

Tier 1 covers the everyday command loop around package mutation.

- `search <term>`
- `list [pattern]`
- `list --info`
- `list --files`
- `list --path <path>`
- `pin <package>`
- `unpin <package>`
- pinned package listing
- `autoremove --dry-run`
- `autoremove`
- `system history`
- `query whatprovides`
- `query whatbreaks`
- repository `list` and `sync` behavior where it affects search/install/update

Tier 1 is not a promise to match every output column or flag from native managers. It is a promise that users can answer the same ordinary questions without dropping back to dnf, apt, or pacman for Conary-owned packages.

### Tier 2: Native PM Depth

Tier 2 is roadmap scope, not this implementation slice.

- Weak dependencies, recommends, suggests, and optional dependency policy parity
- Distro-specific package holds beyond Conary pinning
- Repository priorities and pinning beyond current source-policy behavior
- Rich cache cleaning and download-only workflows
- Deep security advisory metadata, CVE display, and severity parity
- Native PM transaction-history import/export
- Every native manager flag and formatting convention

### Tier 3: Explicitly Deferred

Tier 3 remains out of scope for this package-manager parity implementation.

- conaryd package install/remove/update execution
- Active-generation handoff back to native package-manager authority
- ISO generation export
- Portable signed generation bundles
- Non-x86_64 boot asset parity
- Full replacement of dnf, apt, or pacman repository ecosystems

## Package Operation Contract

Introduce a shared package-operation outcome contract used by `install`, `remove`, `update`, and `autoremove`.

The contract separates required package work from optional follow-up work.

### Execution Paths

The implementation must support two explicit execution paths:

1. **Generation-aware path:** Used when a selected Conary generation is active or when the command is generation-specific. This path keeps the existing DB/CAS to EROFS generation publication model. It must not fall back to unsafe direct writes into an immutable selected root.
2. **Mutable live-root path:** Used for Conary-owned package-manager operations on a normal host without a selected Conary generation. This is new structural work. It must write, replace, or remove files from the live root through a recoverable transaction path instead of only updating SQLite/CAS and skipping generation publication.

`defer_generation` is a useful part of this design, but it is not sufficient by itself. Install/update still need direct live-root file materialization when no generation is selected, and remove still needs direct live-root file deletion.

### Required Phase

The required phase includes:

- authority classification
- package and dependency resolution
- preflight checks
- DB/filesystem mutation
- required pre/post scriptlets
- changeset and file-history recording
- rollback or refusal when required work cannot be completed

If required work fails, the command exits non-zero.

For the mutable live-root path, the required phase must include a live-root transaction plan:

- preflight root/path safety, ownership conflicts, pins, dependency breakage, critical package and runtime capability checks, scriptlet policy, and live-mutation acknowledgement
- backup or CAS capture for files that may be overwritten or removed
- direct filesystem materialization/removal for regular files, symlinks, and directories with path traversal and symlink escape protections
- DB and file-history updates that are consistent with the filesystem result
- crash-recovery or rollback behavior for pending live-root operations before any later package mutation begins

The implementation plan must choose and document the live-root ordering. A safe default is:

1. Create a pending changeset and live-root rollback journal before filesystem mutation.
2. Apply filesystem changes with enough backup data to undo them.
3. Commit DB/file-history changes only after required filesystem changes succeed.
4. If DB commit fails after filesystem changes, restore from the live-root rollback journal.
5. On the next package command, recover or roll back any pending live-root journal before acquiring a new mutation plan.

The generation-aware path may continue to treat DB commit as the point of no return because the generation image is re-derived from DB state. The mutable live-root path cannot assume that property.

### Optional Follow-Up Phase

The optional phase includes:

- state snapshot capture
- generation rebuild or publication
- generation metadata refresh
- non-critical reporting/bookkeeping that is not required for package mutation

If optional follow-up fails after required work succeeded, the command exits success, emits a warning, and records the deferred follow-up where history can show it.

The first implementation should avoid a schema migration if possible by recording deferred warnings in existing changeset metadata while keeping the changeset status `Applied`. Add a schema change only if the implementation plan proves existing metadata cannot support clear history and tests.

Because `changesets.metadata` already stores rollback snapshots, deferred follow-up must use a versioned JSON envelope rather than ad hoc strings. The envelope must preserve legacy rollback parsing and support both rollback payloads and deferred warnings, for example:

```json
{
  "schema": "conary.changeset.metadata.v1",
  "rollback_snapshot": null,
  "removed_troves": [],
  "deferred_follow_up": [
    {
      "kind": "generation_rebuild",
      "status": "failed",
      "message": "root is not self-contained",
      "retry_command": "conary --allow-live-system-mutation system generation build --summary \"Retry deferred package follow-up\""
    }
  ]
}
```

`system history` must render deferred follow-up distinctly from a clean `Applied` changeset. Rollback parsing must continue to accept legacy raw `TroveSnapshot` and `RevertMetadata` payloads.

## Operation Outcomes

Every Tier 0 mutation command should end in one of these states:

### Applied

Required package operation completed. Optional follow-up either completed or was not needed. Exit code is success.

### AppliedWithDeferredFollowUp

Required package operation completed. Optional state/generation follow-up failed or was skipped. Exit code is success. Output names the deferred work and tells the user how to retry or repair it.

Examples:

- "Installed nginx. Generation rebuild was deferred: root is not self-contained. Run `conary --allow-live-system-mutation system generation build --summary \"Retry deferred package follow-up\"` after resolving the listed files."
- "Removed fixture. State snapshot failed after removal; package DB and files reflect the removal. Run `conary system state create \"Removed fixture\"` to capture a manual state."

This is an operation outcome, not necessarily a new `ChangesetStatus`. The expected first implementation is `ChangesetStatus::Applied` plus versioned metadata that `system history` can display as deferred follow-up.

### RefusedBeforeMutation

Conary detected a blocker before changing DB or files. Exit code is failure.

Examples:

- missing `--allow-live-system-mutation`
- pinned package
- dependency breakage
- critical package or runtime capability
- adopted package without explicit takeover
- ambiguous package variant
- missing repository metadata
- unsupported security metadata for `update --security`
- generation-specific request without generation readiness

### RolledBack

Required mutation started and failed. Conary restored DB/files/history as far as possible. Exit code is failure, and history marks or describes the rollback.

For multi-package operations such as `conary update`, per-package outcomes must be reported explicitly. If any required package mutation fails after another package succeeds, the command must not summarize the whole run as a clean success. The command may exit non-zero with a partial-results summary, but optional follow-up failures alone must not cause non-zero exit after required package mutations succeeded.

## Authority Classification

Every mutation begins by classifying the package authority.

### Conary-Owned

`InstallSource::Repository`, `InstallSource::File`, and `InstallSource::Taken` are Conary-owned. These are in scope for normal package-manager parity.

On a host without a selected Conary generation, Conary-owned package operations may mutate the live root directly through the mutable live-root transaction path after the existing live-mutation acknowledgement.

On a host with a selected Conary generation, operations should use the existing generation-aware path. They must not fall back to unsafe direct writes into an immutable selected root. If the generation-aware path cannot proceed, refuse before mutation with guidance.

### Adopted

`adopted-track` and `adopted-full` remain native package-manager authoritative. Tier 0 preserves the Adopt Without Regret boundary:

- normal update does not silently take over adopted packages
- `--dep-mode takeover` is required for takeover-shaped behavior
- critical adopted packages remain blocked from takeover
- output names the native manager command where native authority remains required
- `conary remove <adopted-package>` must not look like a native uninstall. It should refuse before mutation with native package-manager guidance and `conary system unadopt <package>` / `conary system unadopt --all` guidance for tracking removal, unless the user supplied an explicitly destructive adopted purge option.

### Critical Or Blocklisted

Critical package names and critical runtime capabilities remain protected before mutation. This slice must not weaken the blocklist to make parity look better. The same protection applies to install/takeover, update, remove, adopted purge, and autoremove.

### Unknown Or Ambiguous

Unknown packages, missing repository metadata, and multiple installed variants must fail before mutation with next-step guidance. Pin, unpin, remove, update, list info/files, and path ownership must share one selector contract instead of using different "first match" behavior per command. The implementation plan should define concrete selector syntax, at minimum package name plus version and architecture, before mutating ambiguous rows. If selector support is not implemented in the same slice, the command must say so clearly.

## Command Matrix

### Install

Native expectation: `dnf install`, `apt install`, and `pacman -S` either install the package or refuse before changing the system.

Conary Tier 0 contract:

- `conary install <pkg>` installs a Conary-owned package from configured repository/Remi metadata when resolution succeeds.
- `conary install ./pkg.rpm`, `./pkg.deb`, and `./pkg.pkg.tar.zst` install local package files.
- On no-generation live hosts, install must materialize package files directly into the live root through the mutable live-root transaction path. Recording DB/CAS state and setting `defer_generation` is not enough.
- On generation-aware hosts, install uses the existing DB/CAS/generation publication path and refuses before mutation if the generation path is not safe.
- Missing dependencies are handled according to `--dep-mode` and model convergence intent.
- Critical dependencies are treated as live runtime/system-provided only when the existing policy says that is safe.
- `--force` cannot silently replace an adopted package; takeover remains explicit.
- Post-commit state/generation follow-up failure does not turn a successful install into a failed command.

Proof:

- unit tests for authority classification, post-commit deferred follow-up, and force/adopted boundaries
- live-root transaction tests proving no-generation install writes files, symlinks, permissions, and DB/file-history consistently
- local artifact CLI tests for RPM, DEB, and Arch package files
- conary-test distro runs for Fedora 44, Ubuntu 26.04, and Arch

### Remove

Native expectation: `dnf remove`, `apt remove`, and `pacman -R` can remove packages they own from a normal live host.

Current code signal: `cmd_remove` currently refuses non-adopted removal when no active composefs generation is selected, and `autoremove` inherits this by delegating to `cmd_remove`.

Conary Tier 0 contract:

- `conary remove <pkg>` removes Conary-owned package DB state and package files on normal mutable live hosts without requiring an active generation.
- The code path must split explicitly: active-generation hosts use the existing generation-aware removal path; no-generation hosts use the mutable live-root transaction path and must not call `rebuild_and_mount` as the required filesystem mutation.
- Dependency breakage, pins, critical packages, missing live mutation acknowledgement, and ambiguous variants fail before mutation.
- Active-generation hosts remain generation-aware and fail before mutation if safe generation handling is not available.
- Adopted-package remove without an explicit unadopt/purge intent refuses before mutation and points to native PM for full uninstall or `conary system unadopt` for tracking removal.
- Adopted-package destructive purge remains explicit and must obey the same safety rules as other live mutations, including critical package and runtime capability protection.
- Post-commit state/generation follow-up failure becomes `AppliedWithDeferredFollowUp`.

Proof:

- replace the current "remove requires active generation" test with coverage for both paths: mutable live-host direct removal succeeds, active-generation unsafe fallback refuses
- direct live-root removal tests for files, symlinks, directories, already-missing files, rollback/recovery, and history
- tests proving `conary remove <adopted>` is not a silent tracking-only uninstall unless the command explicitly asks for unadoption/purge behavior
- distro integration proof for all three package formats

### Update

Native expectation: `dnf upgrade`, `apt upgrade`, and `pacman -Syu` update packages they own and report when no update is available.

Conary Tier 0 contract:

- `conary update` and `conary update <pkg>` update Conary-owned packages when a newer eligible candidate exists.
- Update inherits the install path. No-generation update cannot work until install can materialize updated package files through the mutable live-root transaction path.
- Single-package update must have one clear operation outcome. Multi-package update must report per-package outcomes and must not mark required package failures as a clean success.
- Source-policy and latest-mode source switches remain explicit and confirmed unless `--yes` is supplied.
- Adopted packages are skipped with native-authority guidance unless explicit takeover is requested.
- Critical adopted packages remain blocked even under takeover.
- `update --security` only applies sources with known security metadata. Repository/source metadata must record whether security-advisory queries are supported, unsupported, or unknown.
- For `update --security`, candidate selection must distinguish: update is known security update; update exists but source security metadata is unsupported or unknown; no security update exists from a security-capable source.
- Default preview behavior is fail-before-mutation for a requested package set that includes any Conary-owned source with unsupported or unknown security metadata. Future partial-apply behavior can be added behind an explicit flag, but the default must not imply complete security coverage when Conary cannot know.
- "All packages are up to date" is only printed when Conary actually evaluated the relevant package set and no updates were skipped for authority or metadata reasons.
- Post-commit state/generation follow-up failure becomes `AppliedWithDeferredFollowUp`.

Proof:

- unit tests for candidate selection, adopted skip messaging, critical protection, source switch confirmation, partial multi-package outcomes, and security metadata unavailable behavior
- integration tests with fixture v1/v2 packages across RPM, DEB, and Arch metadata

### Search

Native expectation: users can discover packages and understand when repository metadata is missing.

Conary Tier 1 contract:

- `conary search <term>` returns repository results from configured synced repositories.
- If no repositories are configured or synced, output points to `conary repo list` and `conary repo sync`.
- Results should show enough source identity to understand cross-distro/package-format candidates.

### List, Info, Files, And Path Ownership

Native expectation: users can see installed packages, inspect a package, list package files, and ask who owns a path.

Conary Tier 1 contract:

- `conary list [pattern]` shows installed packages and makes ownership/source visible enough to distinguish Conary-owned from adopted.
- `conary list --info <pkg>` shows version, architecture, source, repository/native authority, reason, and pin state.
- `conary list --files <pkg>` lists package files.
- `conary list --path <path>` identifies the owning package or says no Conary-tracked package owns it.
- Ambiguous package variants are listed with selector guidance.

### Pin And Unpin

Native expectation: users can prevent a package from updating or being removed, then undo that hold.

Conary Tier 1 contract:

- `conary pin <pkg>` pins the intended installed package before updates/removes can affect it.
- `conary unpin <pkg>` clears the pin.
- pinned package listing shows all pinned variants.
- Ambiguous variants are not silently pinned or unpinned. Either add selectors or refuse with the variants listed.
- Pin, remove, and update must share the same understanding of pin state.

### Autoremove

Native expectation: users can preview and remove orphaned dependencies.

Conary Tier 1 contract:

- `conary autoremove --dry-run` previews orphaned Conary-owned packages.
- `conary autoremove` removes Conary-owned orphans through the same package-operation contract as `remove`.
- Adopted packages are not destructively autoremoved.
- Failures are per-package and summarized without hiding partial results.
- No active generation is required for normal mutable live-host Conary-owned orphan cleanup.

### History

Native expectation: users can see what package operations happened.

Conary Tier 1 contract:

- `conary system history` shows install/remove/update/autoremove changesets.
- Applied-with-deferred-follow-up warnings are visible.
- Rollbacks and refused-before-mutation errors are distinguishable from applied package operations.

### Query What Provides And What Breaks

Native expectation: users can answer capability/file provider questions and understand why removal/update is blocked.

Conary Tier 1 contract:

- `conary query whatprovides <capability>` consults installed and repository metadata where available.
- `conary query whatbreaks <package>` explains installed reverse dependency breakage before remove.
- Output uses the same dependency model as install/remove/update preflight, so diagnostics match behavior.
- Capability resolution uses exact package names or declared provider metadata from installed rows,
  Remi repository sync, and AppStream/Repology-backed normalized data. It must not invent
  package matches from soname stems, package-name lookalikes, case folding, or cross-distro
  string variations.

Adding top-level aliases for `whatprovides` and `whatbreaks` is a separate CLI ergonomics decision. The implementation plan should not test top-level aliases unless it also adds them.

### Repository List And Sync

Native expectation: package search/install/update depends on configured repository metadata, and users can refresh it.

Conary Tier 1 contract:

- `conary repo list` shows enabled/disabled repositories and enough distro/source identity to explain candidate selection.
- `conary repo sync` refreshes metadata or fails with source-specific errors.
- install/update/search errors that depend on repository metadata should name the missing or stale setup step.

## Data Flow

Every Tier 0 mutation command should follow this sequence:

1. **Classify authority:** Conary-owned, adopted, critical/blocklisted, unknown, or ambiguous.
2. **Resolve operation:** package artifact, repository candidate, update candidate, removal target, or orphan set.
3. **Preflight:** locks, pins, dependency breakage, scriptlet policy, permissions, live mutation acknowledgement, source policy, and security metadata.
4. **Plan execution path:** generation-aware publication or mutable live-root transaction.
5. **Commit required package operation:** filesystem mutation, DB mutation, scriptlets, changeset, file history, install source, version scheme, repository/native identity, and reason in the ordering selected for that execution path.
6. **Run optional follow-up:** state snapshot and generation rebuild/publish if appropriate.
7. **Report:** changed packages, skipped packages, deferred follow-up, and next commands.

The exit code reflects required package mutation. Optional follow-up can add warnings and history metadata; it cannot retroactively make a successful package mutation look failed.

## Error Handling And Messaging

Error messages must answer three questions:

- Did Conary change anything?
- Who owns this package right now: Conary or the native package manager?
- What command should the user run next?

Required patterns:

- No generic "operation failed" after package success.
- No generic "all packages are up to date" when adopted packages were skipped or security metadata was unavailable.
- No silent no-op for commands that look like they mutate.
- No hidden takeover.
- No arbitrary choice when multiple installed variants match.

## Testing Contract

### Unit And Contract Tests

Add focused Rust tests for:

- package-operation outcome classification
- deferred follow-up after successful package mutation
- versioned changeset metadata envelope containing rollback snapshots and deferred follow-up without breaking legacy rollback parsing
- `system history` rendering deferred follow-up separately from clean `Applied`
- install post-commit follow-up failure exits success with warning
- no-generation install materializes files on a mutable live root for Conary-owned packages
- no-generation remove succeeds for Conary-owned packages on mutable live roots
- active-generation remove does not fall back to unsafe direct live-root writes
- live-root rollback/recovery for partial file install/remove failure
- autoremove delegates through the same package-operation contract
- adopted package updates stay native-authoritative unless takeover is explicit
- critical adopted packages remain blocked from takeover
- critical Conary-owned packages and runtime capabilities are blocked from destructive remove/purge/autoremove
- security metadata unavailable behavior for `update --security`
- pin/remove/update preflight consistency
- ambiguous variant handling for remove, pin, unpin, list info/files, and path ownership
- blocklist and dependency-break refusal before mutation

### CLI Integration Tests

Add or extend CLI-level tests proving:

- local RPM/DEB/Arch install on a no-generation live root, then list/info/files/path queries and real file existence/checksum
- Conary-owned remove without active generation
- update from fixture v1 to v2 through `conary update`, not by direct installing v2
- pin blocks update/remove and unpin restores them
- autoremove dry-run and apply behavior
- history shows applied and deferred-follow-up status
- `query whatprovides` and `query whatbreaks` diagnostics match actual preflight decisions
- repo list/sync/search guidance when metadata is absent
- adopted remove refuses with native PM / unadopt guidance unless explicit unadopt or purge intent is supplied
- `update --security` refuses before mutation or reports skipped sources when security metadata is unsupported or unknown

### conary-test Distro Matrix

Create a focused Tier 0 + Tier 1 suite named `phase4-native-pm-parity` or intentionally rename/extend an existing Phase 4 suite with that visible purpose. The suite must appear in `cargo run -p conary-test -- list` and the implementation plan must run it for:

- Fedora 44 / RPM
- Ubuntu 26.04 LTS / DEB
- Arch / pacman package format

Use concrete commands:

```bash
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro fedora44 --phase 4
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro ubuntu-26.04 --phase 4
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro arch --phase 4
```

Every proof command that depends on process success or failure must include an explicit `exit_code` assertion. Do not rely on implicit manifest behavior. The final release evidence for each distro must show:

- `failed = 0`
- `skipped = 0`
- `cancelled = 0`

Do not rely on `suite.setup` for required setup unless the harness is changed to execute it. Encode required setup as fatal tests or add harness support in the same implementation slice.

Required distro proof:

- repository search and sync are usable
- local package install works
- repository/Remi package install works where the fixture is available
- list/info/files/path ownership answer after install
- pin blocks update/remove before mutation
- unpin allows update/remove
- update moves a Conary-owned fixture from v1 to v2 through `conary update`
- remove deletes Conary-owned package files and DB rows without requiring selected generation
- autoremove preview and apply behave consistently
- history records applied operations and deferred follow-up warnings
- `query whatprovides` and `query whatbreaks` explain dependency decisions
- optional generation/state follow-up can be forced to fail after mutation, and the command still exits success with deferred follow-up output
- native fixture assertions include exact package name, version, architecture, version scheme, source distro/repository, file checksums, provides, dependencies, config-file behavior, and scriptlet behavior where that format exposes it
- pin/update/remove tests prove failed mutations leave package version, files, DB rows, and history unchanged

Expect meaningful test volume. Existing integration coverage can be reused, but this matrix likely needs 60-90 new manifest or CLI proof steps once every format, mutation command, query command, and refusal path is counted.

## Acceptance Criteria

- Tier 0 mutation rows and Tier 1 daily-driver rows work for Conary-owned packages on the supported preview distros. "Fail before mutation" is acceptable only for explicitly named unsafe or unsupported cases such as adopted native-authority operations, critical packages, unsupported security metadata, ambiguous selectors, dependency breakage, missing acknowledgement, or missing repository metadata.
- Conary-owned `install`, `remove`, and `update` work on normal mutable live hosts without requiring a selected Conary generation.
- No-generation install/update materialize package files on the live root; no-generation remove deletes package files from the live root; DB-only bookkeeping does not satisfy package-manager parity.
- Active-generation hosts continue to use generation-aware paths and never fall back to unsafe direct writes into an immutable selected root.
- Successful required package mutation is never reported as command failure solely because optional state/generation follow-up failed.
- Deferred follow-up is represented in a versioned changeset metadata envelope and rendered by `system history`.
- `autoremove` works for Conary-owned orphan packages through the same remove contract.
- Adopted packages remain native-authoritative unless takeover is explicit.
- Critical package and critical runtime capability protections remain in place.
- `update --security` is honest about metadata support for each supported package source and refuses before mutation by default when the requested package set includes unknown or unsupported security metadata.
- Everyday commands around the mutation path answer search, list, info, file ownership, pinning, history, provider, and breakage questions through existing CLI paths such as `conary query whatprovides` and `conary query whatbreaks`.
- RPM, DEB, and Arch paths have real tests.
- The named conary-test parity suite is visible from `cargo run -p conary-test -- list` and produces zero failed, skipped, and cancelled tests for Fedora 44, Ubuntu 26.04, and Arch evidence runs.
- Documentation names Tier 2 and Tier 3 follow-up work instead of implying full dnf/apt/pacman replacement parity.

## Non-Goals

- Do not implement every native package-manager flag.
- Do not implement conaryd package execution.
- Do not implement active-generation handoff back to native package-manager authority.
- Do not weaken live mutation acknowledgement.
- Do not weaken critical package or runtime capability blocklists.
- Do not claim package-manager parity for no-generation hosts using DB/CAS-only mutation that leaves package files absent or stale on the live root.
- Do not require ISO export, OCI registry polish, or signed bundles.
- Do not broaden the limited preview beyond Fedora 44, Ubuntu 26.04 LTS, and Arch Linux.
- Do not solve every weak-dependency/recommends/source-priority edge case in this slice.

## Open Follow-Ups

- Tier 2 weak dependency and recommends policy.
- Native security advisory/CVE display parity.
- Cache clean/download-only workflows.
- Rich repository priority and hold behavior.
- conaryd shared package executor.
- Active-generation handoff from Conary ownership back to native package-manager authority.
- A public matrix page comparing Conary command support with dnf, apt, and pacman.
- Product decision: whether to add top-level aliases for `query whatprovides` and `query whatbreaks`.
- Product decision: whether future `update --security` should support an explicit partial-apply mode for mixed security-metadata sources.
- Product decision: how much native config-file removal behavior to emulate (`.rpmsave`, `.dpkg-old`, pacman equivalents) in the preview path.
- Product decision: whether cross-distro local file installs, such as installing a DEB on Fedora, are allowed by default, require acknowledgement, or refuse before mutation.
