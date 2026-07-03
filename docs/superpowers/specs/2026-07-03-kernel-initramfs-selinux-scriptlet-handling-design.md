# Kernel Initramfs SELinux Scriptlet Handling Design

**Status:** Active design
**Date:** 2026-07-03
**Related issue:** GitHub issue #35, especially Fedora kernel-package conversion and Remi publication refusal behavior

## Baseline

This design is rebased on `v0.9.2` after the M4e lifecycle-authoring work landed. The scriptlet classification and Remi publication files this design depends on are unchanged by that pull: `blocked_classes.rs`, `support_matrix.rs`, `legacy_replay.rs`, `publication.rs`, and `docs/SCRIPTLET_SECURITY.md` still preserve the same fail-closed policy. The CCS v2 side did change in a way that strengthens this design: lifecycle authority is projected into `LifecycleAuthorityV2`, lifecycle-bearing authoring requires an exact target profile, `TargetProfileQuery` in `crates/conary-core/src/ccs/v2/validation.rs` is the fail-closed profile-fact boundary, and Remi release upload already validates native lifecycle authority against the route-derived supported profile.

The current issue #35 diagnostic slice is separate from this design. The branch-level Remi/client changes make terminal `review-required` and `blocked` jobs actionable and improve minimal-root HTTP-client diagnostics. This design handles the next trust question: how Conary should eventually model kernel, initramfs, and SELinux scriptlet behavior instead of allowing raw replay.

## Goal

Conary should classify boot/security-critical legacy scriptlet intent, preserve enough evidence for review and future adapter work, and eventually allow a narrow set of native-backed kernel/initramfs/SELinux operations without weakening the current public Remi fail-closed default.

The short-term goal is not to make Fedora kernel packages public-ready. It is to make refusals precise, evidence-rich, and stable enough that maintainers can see what native work is missing. Public serving remains limited to native-free packages or packages whose scriptlet effects are fully replaced by trusted adapter evidence.

## Safety Invariants

- Raw legacy scriptlet replay is not public-serving authority.
- `--no-scripts` must not bypass boot or security policy for operations that require scriptlet effects.
- Remi public serving stays stricter than local experimental workflows.
- Boot and SELinux actions fail closed unless a target profile explicitly supports the exact operation.
- Text-pattern detection is advisory until converted into typed adapter evidence.
- Bootloader mutation remains out of scope for public-ready conversion in this design.
- Minimal roots and chroots must not accidentally run host boot or SELinux tools.

## Current Repo Facts

- `crates/conary-core/src/ccs/convert/blocked_classes.rs` blocks `kernel-module`, `initramfs`, `bootloader`, and `selinux` classes.
- `crates/conary-core/src/ccs/convert/support_matrix.rs` projects those blocked classes into the support matrix with blocked fixture rows.
- `crates/conary-core/src/ccs/convert/adapters.rs` owns conversion-time adapter classification. It is already over 1500 lines, so feature work should preserve the existing classification boundary and avoid unrelated decomposition.
- `crates/conary-core/src/ccs/convert/scriptlet_bundle/` builds passive legacy bundles, summary fields, and publication metadata from classification reports.
- `apps/remi/src/server/publication.rs` turns scriptlet summary fields into public, review-required, or blocked publication decisions.
- `crates/conary-core/src/ccs/v2/validation.rs` defines `TargetProfileQuery`, whose default implementation rejects all profile facts.
- `crates/conary-core/src/repository/supported_profiles/` owns public profile facts and currently allow-lists services, tmpfiles, sysctl, users, groups, directories, and alternatives.
- `apps/remi/src/server/native_publish/verify.rs` already maps a Remi route slug to a supported profile and validates native CCS v2 lifecycle authority before public release publication.

## Intent Classes

The design treats kernel, initramfs, and SELinux scriptlets as distinct intent classes even when they appear in the same package.

### Kernel Module Intents

Classify, but keep blocked until native authority exists:

- `depmod` cache regeneration
- `weak-modules` registration or removal
- `kernel-install` add/remove
- `dkms` build/install/remove
- `modprobe` live module loading
- direct writes under `/lib/modules`, `/usr/lib/modules`, or `/boot`

Candidate future lane:

- `depmod` can become adapter-backed if it is target-rooted, generation-scoped, and tied to the package's module payload plus a target kernel ABI fact.
- `weak-modules` may become private-review first because it depends on distro-specific ABI compatibility policy.
- `dkms` and `modprobe` remain blocked for public Remi because they build or mutate live kernel state.

### Initramfs Intents

Classify, but keep blocked until native authority exists:

- `dracut`, including `--kver`, `--regenerate-all`, `--force`, and host-only modes
- `mkinitcpio`
- `update-initramfs`
- direct writes to initramfs files under `/boot`

Candidate future lane:

- A generation-scoped initramfs refresh can become adapter-backed only when the target profile declares the initramfs tool, boot artifact layout, kernel ABI source, and a no-live-host execution contract.
- Broad host rebuilds such as `dracut --regenerate-all` stay private-review or blocked unless reduced to exact generation artifacts.

### SELinux Intents

Classify, but keep blocked until native authority exists:

- `restorecon` and `fixfiles`
- `semanage fcontext`, `semanage boolean`, `semanage port`, and related policy-store edits
- `setsebool`, especially persistent booleans
- `semodule -i`, `semodule -r`, and package-shipped policy modules

Candidate future lane:

- Static policy module installation can become private-review first, then public-ready only with profile-backed policy-store facts and signed module evidence.
- A narrow allow-list of SELinux booleans can reuse the supported-profile allow-list pattern from users/groups/directories.
- Label reconciliation can become adapter-backed only when the target path set is payload-backed and the profile declares SELinux mode and policy-store semantics.

## Lane Policy

| Lane | Meaning | Examples |
| --- | --- | --- |
| Public-ready | Fully native-free or fully replaced by adapter evidence and accepted by the target profile. | Future exact `depmod` for package-shipped modules under a known kernel ABI. |
| Private-review | Evidence is structured, but the operation has host, distro, or ordering ambiguity. | `weak-modules`, exact SELinux policy module install, exact initramfs refresh without enough profile proof. |
| Blocked | Unsafe for public Remi and unsafe to bypass with raw replay. | `dkms`, `modprobe`, bootloader mutation, broad initramfs regeneration, persistent SELinux policy mutation without allow-list evidence. |

## Architecture

### 1. Conversion Evidence

The first implementation slice should enrich the passive legacy bundle rather than add new CCS v2 authority fields. `AdapterRegistry::classify_invocation_with_context()` already has the `CommandInvocation` when it assigns a blocked class. It should attach sanitized command evidence to blocked/review classifications for boot/security classes, then project that evidence into `LegacyScriptletEntry` and `ScriptletBundleSummary`.

"Sanitized" means the evidence may carry package-authored command names, normalized argument shapes, phase, lifecycle paths, and stable evidence-source labels, but it must not carry raw environment values. Kernel-version-like argument values should be normalized to `<kver>` and `/boot/...` arguments should be reduced to `<boot>/...` before the evidence is surfaced in Remi refusal reports or client diagnostics. The evidence remains diagnostic metadata, not install authority.

This preserves the current publication decision while making the refusal explain what was seen: for example `initramfs` with command `dracut`, `kernel-module` with `depmod`, or `selinux` with `restorecon`.

### 2. Remi Publication

Remi should surface boot/security intent evidence in refusal reports and review artifacts. It should not promote any of these classes to public readiness in the MVP. Public serving remains driven by the existing `publication_status == "public"` rule and support-matrix adapter evidence.

### 3. Target Profile Facts

Later phases should extend `TargetProfileQuery` instead of creating a parallel profile mechanism. The default no-facts implementation must answer `Unsupported` for every new fact.

Required profile facts:

- kernel ABI identity and module directory layout
- initramfs tool family and supported modes
- boot artifact layout and generation ownership
- bootloader ownership assumptions
- SELinux availability, mode, policy store, and module/boolean allow-lists
- whether the target is a minimal root, chroot, or full bootable generation

### 4. Native Authority And Adapters

Later CCS v2 work should add typed native authority only after evidence and profile facts are stable. Candidate authority categories are:

- kernel module cache refresh
- generation initramfs refresh
- SELinux policy module install
- SELinux boolean allow-list reconciliation
- SELinux label reconciliation for payload-backed paths

Adapters must prove complete replacement before `SupportOutcome::Known` and public-ready status are allowed. Partial replacement remains review-only.

### 5. Local Install And Bootstrap

Local install should continue to refuse raw replay for blocked boot/security classes unless a future experimental flag is deliberately introduced with a clear non-public scope. Minimal roots and chroots should prefer explanatory diagnostics: missing `/etc/resolv.conf` or trust roots are HTTP setup problems, while missing `dracut`, `semodule`, `restorecon`, kernel ABI facts, or SELinux policy stores are native handling blockers.

## MVP

The first implementation slice should:

1. Carry sanitized command evidence for `kernel-module`, `initramfs`, `bootloader`, and `selinux` blocked classes.
2. Add missing command coverage for obvious boot/security tools such as `kernel-install`, SELinux module tools such as `semodule`, and label tools such as `fixfiles`.
3. Project boot/security intent evidence into legacy bundle summaries.
4. Include that evidence in Remi publication refusal reports and client diagnostics.
5. Keep all boot/security classes blocked for public Remi.
6. Add regression tests proving no `--no-scripts`, raw replay, or malformed summary path can turn these packages public-ready.

This MVP helps issue #35 by making the Fedora kernel refusal specific and actionable without pretending Conary can safely serve kernel packages publicly yet.

## Later Phases

1. Add fail-closed boot/security profile facts to `TargetProfileQuery` and `supported_profiles`.
2. Add private-review native adapter evidence for exact `depmod`, exact initramfs refresh, and SELinux policy-module detection.
3. Add CCS v2 native authority fields only after the adapter evidence shape is stable.
4. Extend Remi release validation to reject unsupported boot/security authority using the existing route-derived profile hook.
5. Promote only proof-corpus-backed adapter cases to `SupportOutcome::Known`.

Promoting a currently blocked command to adapter-backed status requires changing the blocked-class registry or dispatch order. Today `classify_invocation_with_context()` checks blocked classes before adapters, so adding a new adapter alone cannot make a `depmod`, `dracut`, `restorecon`, or similar blocked command public-ready.

## Required Tests

- Blocked-class tests for `depmod`, `kernel-install`, `dracut`, `mkinitcpio`, `update-initramfs`, `restorecon`, `fixfiles`, `semanage`, `setsebool`, and `semodule`.
- Bundle tests proving boot/security intent evidence is present in entries and summaries.
- Support-matrix tests proving boot/security rows remain blocked until an adapter row is added with golden fixture evidence.
- Remi publication tests proving refusal reports include blocked classes and sanitized intent evidence.
- Client tests proving Remi blocked/review-required jobs become terminal actionable errors.
- CCS v2 validation tests proving new boot/security profile facts default to unsupported in later phases.
- Release-upload tests proving unsupported native boot/security authority is rejected before public state in later phases.

## Non-Goals

- No public serving of kernel packages because they merely converted successfully.
- No raw scriptlet replay bypass for boot/security classes.
- No bootloader mutation model in the MVP.
- No DKMS build execution in Remi.
- No SELinux host policy-store mutation from conversion-time scriptlets.
- No AppArmor adapter or AppArmor-specific evidence promotion in this MVP. Existing AppArmor scriptlet classes remain blocked and can get their own design if issue-driven evidence says they need the same detailed reporting path.
- No broad file split of `adapters.rs` as part of the MVP.
