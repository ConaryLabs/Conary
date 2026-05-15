---
last_updated: 2026-05-14
revision: 1
summary: Design for Conary's adoption-led, risk-free trial path with native package-manager authority and one-command unadoption
---

# Adopt Without Regret: Design Spec

**Date:** 2026-05-14
**Status:** Approved direction for implementation planning
**Goal:** Make Conary safe to try on Fedora 44, Ubuntu 26.04 LTS, and Arch by keeping native package managers authoritative during adoption mode and adding a one-command, non-destructive escape hatch.

---

## Suggested Codex Goal

Use this objective when launching the implementation with `/goal`:

```text
/goal Implement Conary Adopt Without Regret: adoption mode preserves dnf/apt/pacman authority for RPM, DEB, and Arch systems; `conary system unadopt` provides a tested one-command non-destructive escape hatch; update paths cannot silently take over adopted packages; roadmap/docs/tests prove the contract.
```

This is intentionally narrow enough for `/goal`: it has a clear target, a validation loop, and concrete stopping conditions.

## Product Promise

Conary's limited public preview should be adoption-led, not takeover-led:

> Adopt your existing system into Conary, get CAS-backed state and generation tooling, try Conary safely, and leave with one command if it is not for you.

The preview must not ask a normal Linux user to gamble their daily package-manager authority. In adoption mode, dnf, apt, and pacman remain the source of truth for packages they already own.

## Core Rules

### Native Authority In Adoption Mode

Packages with `InstallSource::AdoptedTrack` or `InstallSource::AdoptedFull` are still native-package-manager packages. Conary may track them, CAS-back their files, verify them, build generations from them, and detect drift, but it must not quietly replace them with Conary-owned files.

### Explicit Takeover Boundary

Conary-owned packages use `InstallSource::File`, `InstallSource::Repository`, or `InstallSource::Taken`. Moving an adopted package into this lane is takeover. Takeover must require an explicit command or explicit `--dep-mode takeover` acknowledgement and must not be part of the risk-free trial promise.

### One-Command Escape

`conary system unadopt --all` is the public escape hatch. It removes Conary tracking for adopted packages and disables adoption sync hooks by default. It leaves package files and native package-manager state intact.

### Non-Destructive By Default

`unadopt` must never delete package files by default. It may leave CAS objects behind for normal garbage collection. Any future destructive cleanup needs a separate explicit flag and must not be part of the first release slice.

### All Three Package Types

The contract must be proven for:

- RPM/dnf on Fedora 44
- DEB/apt on Ubuntu 26.04 LTS
- Arch packages/pacman on Arch Linux

No implementation slice is complete until it has unit tests for the ownership rules and integration coverage across those three distro/package-manager families.

## Command Design

### `conary system unadopt`

Supported forms:

```bash
conary system unadopt curl --dry-run
conary --allow-live-system-mutation system unadopt curl
conary system unadopt --all --dry-run
conary --allow-live-system-mutation system unadopt --all
```

Initial options:

- positional package names: unadopt only these adopted packages
- `--all`: unadopt every adopted package
- `--dry-run`: print the exact adopted packages that would be untracked
- `--keep-hooks`: leave native PM sync hooks installed when using `--all`

Argument rules:

- require at least one package name or `--all`
- reject package names combined with `--all`
- require `--allow-live-system-mutation` unless `--dry-run` is set

Behavior:

1. Open the Conary database.
2. Select matching packages whose source is `AdoptedTrack` or `AdoptedFull`.
3. Report packages that are already Conary-owned and leave them untouched.
4. In dry-run mode, print the planned adopted-package removals and hook action.
5. In apply mode, delete adopted troves from Conary tracking. Existing cascade behavior removes file, dependency, and provide rows.
6. Record a changeset and state snapshot for the tracking removal.
7. For `--all`, remove native PM sync hooks unless `--keep-hooks` is set.
8. Print a summary that says native package files were not deleted.

Exit behavior:

- success when at least one adopted package was unadopted or the dry-run plan is valid
- non-zero when a named package does not exist, is not adopted, or the arguments are invalid
- `--all` may succeed while reporting Conary-owned packages that were intentionally skipped

## Update Boundary

The risk-free adoption lane requires update behavior to stay native-authoritative.

For adopted packages:

- `--dep-mode satisfy`: leave updates to the native PM and print the native command.
- `--dep-mode adopt`: do not write package files. Either delegate to the native PM and refresh adoption state, or explicitly report that native update delegation is not implemented yet.
- `--dep-mode takeover`: allow Conary ownership only after the explicit takeover mode is visible to the user.

The first implementation plan may choose the smaller safe step: prevent silent takeover and make adopted-package updates explicit. Native PM delegation can then be the next slice, but the command must not imply that Conary updated adopted packages when it only refreshed metadata.

## Documentation Shape

The roadmap and README should distinguish four lanes:

- **Adopt:** native PM owns packages; Conary tracks/CAS-backs/verifies/builds generations.
- **Unadopt:** Conary removes tracking and hooks; native PM remains intact.
- **Conary-owned install:** Conary owns packages it installed directly.
- **Takeover:** Conary replaces native authority for selected packages; explicit and not risk-free.

Avoid saying "replace dnf/apt/pacman" in limited-preview copy. Prefer "adopt an existing system without giving up native package-manager authority."

## Testing Contract

### Unit Tests

Add Rust tests proving:

- adopted sources are unadoptable
- Conary-owned sources are not unadopted
- dry-run does not mutate the database
- `--all` removes all adopted troves and leaves Conary-owned troves
- file, dependency, and provide rows for unadopted troves disappear through cascade
- update classification for adopted packages does not select Conary file writes except under explicit takeover
- native PM command guidance exists for RPM, DEB, and Arch

### CLI Help Tests

Add clap/parser tests proving:

- `system unadopt --all` parses
- `system unadopt curl` parses
- `system unadopt` without package names or `--all` fails
- `system unadopt --all curl` fails
- `--dry-run` is available

### Integration Tests

Extend or add conary-test coverage so all three supported distros run the same contract:

```bash
cargo run -p conary-test -- run --suite phase1-advanced --distro fedora44 --phase 1
cargo run -p conary-test -- run --suite phase1-advanced --distro ubuntu-26.04 --phase 1
cargo run -p conary-test -- run --suite phase1-advanced --distro arch --phase 1
```

The integration suite must prove:

- a native package can be adopted
- `system unadopt <package> --dry-run` reports the tracking removal
- `system unadopt <package>` removes Conary tracking
- the native package binary still exists and runs after unadoption
- adoption status no longer counts the package

## Acceptance Criteria

- `conary system unadopt` exists and is discoverable from help.
- `conary system unadopt --all` is the one-command escape hatch for adopted packages.
- Unadoption never deletes package files by default.
- Adopted-package update behavior cannot silently take over native-owned packages.
- RPM, DEB, and Arch paths have real tests, not just doc promises.
- Roadmap/README copy presents adoption as the preview path and takeover as explicit follow-up/opt-in.

## Non-Goals

- Do not implement full native PM update delegation unless the implementation plan explicitly chooses that sub-slice.
- Do not implement destructive cleanup of package files.
- Do not make conaryd package execution part of this slice.
- Do not change generation export, ISO export, or bootstrap scope.
- Do not hide takeover; make it explicit and separate.

## Open Follow-Ups

- Native PM update delegation after adoption: `conary update` runs dnf/apt/pacman safely, then `system adopt --refresh --full`.
- `conary system takeover undo` for packages that already crossed into Conary ownership.
- A richer "trial status" summary that reports adopted, Conary-owned, and takeover packages in one screen.
