---
last_updated: 2026-05-14
revision: 1
summary: Design for Conary-owned package-manager parity against dnf, apt, and pacman expectations for the limited public preview
---

# Native Package Manager Parity Matrix: Design Spec

**Date:** 2026-05-14
**Status:** Approved design direction for user review before implementation planning
**Goal:** Make Conary-owned package operations feel like a credible daily package-manager lane, not a step down from dnf, apt, or pacman, for Fedora 44, Ubuntu 26.04 LTS, and Arch Linux.

---

## Suggested Codex Goal

Use this objective when launching the implementation with `/goal`:

```text
/goal Implement Conary Native Package Manager Parity Matrix Tier 0 and Tier 1: Conary-owned install/remove/update work on normal live Fedora 44, Ubuntu 26.04, and Arch hosts without requiring a selected Conary generation; package mutation success is not reported as failure when optional state/generation follow-up fails; search/list/info/files/path/pin/autoremove/history/whatprovides/whatbreaks provide the daily package-manager loop; unsupported cases fail before mutation with clear guidance; real unit, CLI, and conary-test distro evidence proves the contract.
```

The goal is intentionally broader than the Adopt Without Regret slice, but it is still bounded. Tier 0 and Tier 1 are implementation scope. Tier 2 and Tier 3 form the roadmap matrix and stop the implementation plan from absorbing every native package-manager feature.

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

## Tier Matrix

### Tier 0: Preview Blockers

Tier 0 covers behavior that would make Conary feel unsafe, fake, or obviously worse than dnf, apt, or pacman during limited-preview package ownership.

- Conary-owned `install`, `remove`, and `update` must work on normal mutable live hosts without requiring a selected Conary generation.
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
- `whatprovides`
- `whatbreaks`
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

### Optional Follow-Up Phase

The optional phase includes:

- state snapshot capture
- generation rebuild or publication
- generation metadata refresh
- non-critical reporting/bookkeeping that is not required for package mutation

If optional follow-up fails after required work succeeded, the command exits success, emits a warning, and records the deferred follow-up where history can show it.

The first implementation should avoid a schema migration if possible by recording deferred warnings in existing changeset metadata while keeping the changeset status `Applied`. Add a schema change only if the implementation plan proves existing metadata cannot support clear history and tests.

## Operation Outcomes

Every Tier 0 mutation command should end in one of these states:

### Applied

Required package operation completed. Optional follow-up either completed or was not needed. Exit code is success.

### AppliedWithDeferredFollowUp

Required package operation completed. Optional state/generation follow-up failed or was skipped. Exit code is success. Output names the deferred work and tells the user how to retry or repair it.

Examples:

- "Installed nginx. Generation rebuild was deferred: root is not self-contained. Run `conary system generation build --summary ...` after resolving the listed files."
- "Removed fixture. State snapshot failed after removal; package DB and files reflect the removal. Run `conary system state create --summary ...` to capture a manual state."

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

## Authority Classification

Every mutation begins by classifying the package authority.

### Conary-Owned

`InstallSource::Repository`, `InstallSource::File`, and `InstallSource::Taken` are Conary-owned. These are in scope for normal package-manager parity.

On a host without a selected Conary generation, Conary-owned package operations may mutate the live root directly under the existing live-mutation acknowledgement and transaction/rollback rules.

On a host with a selected Conary generation, operations should use the existing generation-aware path. They must not fall back to unsafe direct writes into an immutable selected root. If the generation-aware path cannot proceed, refuse before mutation with guidance.

### Adopted

`adopted-track` and `adopted-full` remain native package-manager authoritative. Tier 0 preserves the Adopt Without Regret boundary:

- normal update does not silently take over adopted packages
- `--dep-mode takeover` is required for takeover-shaped behavior
- critical adopted packages remain blocked from takeover
- output names the native manager command where native authority remains required

### Critical Or Blocklisted

Critical package names and critical runtime capabilities remain protected before mutation. This slice must not weaken the blocklist to make parity look better.

### Unknown Or Ambiguous

Unknown packages, missing repository metadata, and multiple installed variants must fail before mutation with next-step guidance. For pin/unpin and remove, if multiple installed variants match a package name, Conary should list variants and require a selector rather than acting on an arbitrary row. If selector support is not implemented in the same slice, the command must say so clearly.

## Command Matrix

### Install

Native expectation: `dnf install`, `apt install`, and `pacman -S` either install the package or refuse before changing the system.

Conary Tier 0 contract:

- `conary install <pkg>` installs a Conary-owned package from configured repository/Remi metadata when resolution succeeds.
- `conary install ./pkg.rpm`, `./pkg.deb`, and `./pkg.pkg.tar.zst` install local package files.
- Missing dependencies are handled according to `--dep-mode` and model convergence intent.
- Critical dependencies are treated as live runtime/system-provided only when the existing policy says that is safe.
- `--force` cannot silently replace an adopted package; takeover remains explicit.
- Post-commit state/generation follow-up failure does not turn a successful install into a failed command.

Proof:

- unit tests for authority classification, post-commit deferred follow-up, and force/adopted boundaries
- local artifact CLI tests for RPM, DEB, and Arch package files
- conary-test distro runs for Fedora 44, Ubuntu 26.04, and Arch

### Remove

Native expectation: `dnf remove`, `apt remove`, and `pacman -R` can remove packages they own from a normal live host.

Current code signal: `cmd_remove` currently refuses non-adopted removal when no active composefs generation is selected, and `autoremove` inherits this by delegating to `cmd_remove`.

Conary Tier 0 contract:

- `conary remove <pkg>` removes Conary-owned package DB state and package files on normal mutable live hosts without requiring an active generation.
- Dependency breakage, pins, critical packages, missing live mutation acknowledgement, and ambiguous variants fail before mutation.
- Active-generation hosts remain generation-aware and fail before mutation if safe generation handling is not available.
- Adopted-package remove without `--purge-files` remains tracking-only and points to native PM for full uninstall.
- Adopted-package destructive purge remains explicit and must obey the same safety rules as other live mutations.
- Post-commit state/generation follow-up failure becomes `AppliedWithDeferredFollowUp`.

Proof:

- replace the current "remove requires active generation" test with coverage for both paths: mutable live-host direct removal succeeds, active-generation unsafe fallback refuses
- direct live-root removal tests for files, symlinks, directories, already-missing files, rollback, and history
- distro integration proof for all three package formats

### Update

Native expectation: `dnf upgrade`, `apt upgrade`, and `pacman -Syu` update packages they own and report when no update is available.

Conary Tier 0 contract:

- `conary update` and `conary update <pkg>` update Conary-owned packages when a newer eligible candidate exists.
- Source-policy and latest-mode source switches remain explicit and confirmed unless `--yes` is supplied.
- Adopted packages are skipped with native-authority guidance unless explicit takeover is requested.
- Critical adopted packages remain blocked even under takeover.
- `update --security` only applies sources with known security metadata. If a source lacks security metadata, Conary must report that limitation before mutation for affected packages.
- "All packages are up to date" is only printed when Conary actually evaluated the relevant package set and no updates were skipped for authority or metadata reasons.
- Post-commit state/generation follow-up failure becomes `AppliedWithDeferredFollowUp`.

Proof:

- unit tests for candidate selection, adopted skip messaging, source switch confirmation, and security metadata unavailable behavior
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

### What Provides And What Breaks

Native expectation: users can answer capability/file provider questions and understand why removal/update is blocked.

Conary Tier 1 contract:

- `conary whatprovides <capability>` consults installed and repository metadata where available.
- `conary whatbreaks <package>` explains installed reverse dependency breakage before remove.
- Output uses the same dependency model as install/remove/update preflight, so diagnostics match behavior.

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
4. **Commit required package operation:** DB/filesystem mutation, scriptlets, changeset, file history, install source, version scheme, repository/native identity, and reason.
5. **Run optional follow-up:** state snapshot and generation rebuild/publish if appropriate.
6. **Report:** changed packages, skipped packages, deferred follow-up, and next commands.

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
- install post-commit follow-up failure exits success with warning
- remove without active generation succeeds for Conary-owned packages on mutable live roots
- active-generation remove does not fall back to unsafe direct live-root writes
- autoremove delegates through the same package-operation contract
- adopted package updates stay native-authoritative unless takeover is explicit
- critical adopted packages remain blocked from takeover
- security metadata unavailable behavior for `update --security`
- pin/remove/update preflight consistency
- ambiguous variant handling for remove, pin, unpin, list info/files, and path ownership
- blocklist and dependency-break refusal before mutation

### CLI Integration Tests

Add or extend CLI-level tests proving:

- local RPM/DEB/Arch install then list/info/files/path queries
- Conary-owned remove without active generation
- update from fixture v1 to v2
- pin blocks update/remove and unpin restores them
- autoremove dry-run and apply behavior
- history shows applied and deferred-follow-up status
- whatprovides and whatbreaks diagnostics match actual preflight decisions
- repo list/sync/search guidance when metadata is absent

### conary-test Distro Matrix

Create a focused Tier 0 + Tier 1 suite or extend existing manifests so the following are proved for:

- Fedora 44 / RPM
- Ubuntu 26.04 LTS / DEB
- Arch / pacman package format

Required distro proof:

- repository search and sync are usable
- local package install works
- repository/Remi package install works where the fixture is available
- list/info/files/path ownership answer after install
- pin blocks update/remove before mutation
- unpin allows update/remove
- update moves a Conary-owned fixture from v1 to v2
- remove deletes Conary-owned package files and DB rows without requiring selected generation
- autoremove preview and apply behave consistently
- history records applied operations and deferred follow-up warnings
- whatprovides and whatbreaks explain dependency decisions
- optional generation/state follow-up can be forced to fail after mutation, and the command still exits success with deferred follow-up output

Use fresh commands such as:

```bash
cargo run -p conary-test -- run --suite <tier0-tier1-suite> --distro fedora44 --phase 1
cargo run -p conary-test -- run --suite <tier0-tier1-suite> --distro ubuntu-26.04 --phase 1
cargo run -p conary-test -- run --suite <tier0-tier1-suite> --distro arch --phase 1
```

The implementation plan may choose the exact suite name and phase grouping, but the distro matrix must be visible from `cargo run -p conary-test -- list`.

## Acceptance Criteria

- Tier 0 and Tier 1 command matrix rows are either implemented and tested or explicitly fail before mutation with preview guidance.
- Conary-owned `install`, `remove`, and `update` work on normal mutable live hosts without requiring a selected Conary generation.
- Successful required package mutation is never reported as command failure solely because optional state/generation follow-up failed.
- `autoremove` works for Conary-owned orphan packages through the same remove contract.
- Adopted packages remain native-authoritative unless takeover is explicit.
- Critical package and critical runtime capability protections remain in place.
- `update --security` is honest about metadata support for each supported package source.
- Everyday commands around the mutation path answer search, list, info, file ownership, pinning, history, provider, and breakage questions.
- RPM, DEB, and Arch paths have real tests.
- Documentation names Tier 2 and Tier 3 follow-up work instead of implying full dnf/apt/pacman replacement parity.

## Non-Goals

- Do not implement every native package-manager flag.
- Do not implement conaryd package execution.
- Do not implement active-generation handoff back to native package-manager authority.
- Do not weaken live mutation acknowledgement.
- Do not weaken critical package or runtime capability blocklists.
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
