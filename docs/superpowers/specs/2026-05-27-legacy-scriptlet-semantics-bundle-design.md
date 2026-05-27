---
last_updated: 2026-05-27
revision: 1
summary: Clean-room design for preserving and improving RPM, DEB, and Arch scriptlet behavior in converted CCS packages
---

# Legacy Scriptlet Semantics Bundle: Design Spec

**Date:** 2026-05-27
**Status:** Draft for implementation planning
**Goal:** Make Remi-converted CCS packages behavior-preserving by default,
behavior-improving when Conary can prove a safe declarative replacement, and
loudly non-convertible when neither is true.

---

## Purpose

Remi conversion is strategically central because Conary does not yet have a
large native CCS package corpus. That makes legacy package scriptlets a first
class product concern, not a side feature.

Converted RPM, DEB, and Arch packages must not lose the install, upgrade, or
remove behavior their native package managers rely on. At the same time, Conary
should not simply replay arbitrary root shell forever. CCS should preserve
native behavior exactly where needed, replace common scriptlet patterns with
idempotent declarative operations where proven, and record enough evidence for
operators to understand the decision.

## Problem

The current conversion pipeline extracts native scriptlets and then tries two
best-effort paths:

- capture some post-install behavior in a mocked sandbox;
- pattern-match remaining shell script bodies into CCS hooks.

That proved the concept, but it is not strong enough for a package conversion
pillar:

- only post-install scriptlets are captured today;
- RPM triggers and file triggers are not represented as first-class conversion
  semantics;
- DEB maintainer-script call modes and trigger behavior are not modeled beyond
  basic phase names;
- Arch `.INSTALL` context is reduced to extracted function bodies;
- regex detection is advisory at best and cannot be the source of truth for
  arbitrary shell;
- generated CCS manifests can contain hooks while losing the raw native
  scriptlet ABI needed for faithful replay;
- operators cannot clearly tell whether a converted package was fully
  declarativized, safely replayed, or partially understood.

## Decision

Introduce a versioned **Legacy Scriptlet Semantics Bundle** in converted CCS
packages.

The bundle records the original native scriptlets, the native ABI needed to run
them, the Conary-understood effects extracted from them, and the replay policy
selected during conversion. Conversion is successful only when every native
scriptlet slot has one of these outcomes:

- `replaced`: Conary has a complete declarative replacement and will not run the
  raw scriptlet.
- `legacy`: Conary will replay the original scriptlet under the native ABI in
  the protected scriptlet runner.
- `blocked`: Conary refuses publication or install under default policy because
  the scriptlet requires unsupported or unsafe behavior.
- `review`: Remi stores the conversion as a private artifact, but it is not
  eligible for public cache publication until curated.

Regex analysis may remain as a temporary signal source, but it is not the
authority. The authority is the bundle's explicit per-slot replay decision plus
the test evidence that produced it.

## Native ABI Model

Conary must model scriptlet slots by package format instead of flattening them
into generic pre/post names too early.

### RPM

RPM supports basic scriptlets, transaction scriptlets, triggers, and file
triggers. The model must preserve at least:

- slot name such as `%pre`, `%post`, `%preun`, `%postun`, `%pretrans`,
  `%posttrans`, trigger, and file-trigger variants;
- interpreter program and interpreter flags;
- script body after RPM macro expansion as present in the binary RPM;
- install/remove/upgrade argument conventions;
- trigger condition, trigger target package, trigger priority, and file-prefix
  matches where the parser exposes them;
- stdin contract for file-trigger paths;
- ordering relative to package payload mutation and transaction boundaries.

The first implementation may mark unsupported trigger classes as `blocked` or
`review`, but it must not silently discard them.

### DEB

Debian packages provide maintainer scripts as package metadata files. The model
must preserve:

- `preinst`, `postinst`, `prerm`, and `postrm`;
- shebang interpreter and arguments;
- native invocation modes such as `install`, `configure`, `upgrade`,
  `remove`, `purge`, `abort-*`, and old/new version arguments;
- `triggers` metadata from the control archive when present;
- whether scripts are expected to run noninteractively;
- ordering relative to unpack, configure, remove, purge, and trigger phases.

The first public Remi replay mode should cover normal install/configure/remove
paths. Purge and complex trigger behavior can fail closed until implemented.

### Arch

Arch packages may include an `.INSTALL` script with shell functions. The model
must preserve:

- the full `.INSTALL` file as source context;
- discovered functions: `pre_install`, `post_install`, `pre_upgrade`,
  `post_upgrade`, `pre_remove`, and `post_remove`;
- native arguments passed by pacman for package versions;
- the fact that functions run chrooted inside the install root;
- ordering relative to file extraction/removal.

Conary should generate a wrapper that sources the preserved `.INSTALL` file and
calls the matching function with native-compatible arguments, rather than
preserving only detached function bodies.

## Bundle Shape

The CCS package should carry one structured bundle, either as a manifest section
or as a manifest-referenced sidecar. The bundle should be versioned separately
from the broader CCS manifest so the semantics model can evolve without
rewriting unrelated package metadata.

Recommended logical shape:

```toml
[legacy_scriptlets]
schema = "conary.legacy-scriptlets.v1"
source_format = "rpm"
source_package = "nginx"
source_version = "1.28.0-1.fc44"
conversion_policy = "safe-or-legacy"
publication_policy = "public-if-no-blocked"

[[legacy_scriptlets.entries]]
id = "rpm:%post"
native_slot = "%post"
phase = "post-install"
interpreter = "/bin/sh"
interpreter_args = []
body_sha256 = "..."
body = "..."
decision = "legacy"
reason = "contains residual shell after modeled systemd reload"

[[legacy_scriptlets.entries.effects]]
kind = "systemd-daemon-reload"
source = "capture"
confidence = "observed"
replacement = "ccs-hook"
```

The final encoding can be optimized for binary CCS packages, but the logical
data must remain inspectable through `conary query scripts` or an equivalent
command.

## Effect IR

Conary should extract scriptlet effects into a typed intermediate
representation. This IR is not a shell parser. It is a list of observed or
declared host-integration effects with source evidence.

Initial effect kinds:

- user/group creation and modification;
- directory creation, ownership, and mode changes;
- systemd daemon reload, enable, disable, preset, restart, try-restart, reload;
- tmpfiles creation;
- alternatives registration and removal;
- dynamic linker cache updates;
- desktop database, MIME database, icon cache, GSettings schema, font cache,
  info-dir, and similar cache refreshes;
- sysusers declarations;
- D-Bus service or policy registration;
- sysctl settings;
- kernel/module/initramfs/grub interactions, initially blocked by default;
- package-manager recursion or network activity, blocked by default.

Each effect carries:

- source: native metadata, payload heuristic, capture log, wrapper observation,
  curated rule, or static signal;
- confidence: `declared`, `observed`, `inferred`, or `uncertain`;
- replacement status: `complete`, `partial`, `none`, or `blocked`;
- original evidence such as command, args, path, or matched native metadata.

Declarative CCS hooks are generated from `complete` effects only. Partial or
uncertain effects can add warnings but cannot justify dropping the raw script.

## Conversion Pipeline

The clean-room conversion pipeline should run in this order:

1. **Parse native package metadata.** Extract payload, dependencies, provides,
   config file semantics, and all scriptlet/trigger slots exposed by the
   format.
2. **Build native ABI entries.** Preserve script bodies, interpreters, flags,
   phase names, trigger metadata, call arguments, and format-specific ordering.
3. **Run static native metadata extraction.** Use package metadata and payload
   paths to identify declared services, tmpfiles, sysusers, alternatives,
   cache-trigger candidates, and known unsupported classes.
4. **Run controlled capture where useful.** Execute scriptlets in an isolated
   conversion root with mocked helper tools and no network. Capture helper
   invocations and filesystem writes. Capture is evidence, not authority.
5. **Apply curated conversion rules.** Allow Remi-maintained rules for common
   distro helper macros and package classes. Curated rules must include source,
   version scope, and tests.
6. **Choose per-entry decisions.** Select `replaced`, `legacy`, `blocked`, or
   `review` for every entry.
7. **Build CCS hooks.** Emit hooks only for effects with complete replacements.
8. **Embed the bundle.** Store the original scriptlets and decisions in the CCS
   package.
9. **Gate publication.** Public Remi cache publication requires no `blocked`
   entries and no unreviewed package classes outside the preview policy.

## Replay Engine

Install/update/remove must consume the bundle instead of treating converted CCS
packages as ordinary native CCS-only packages.

Default replay rules:

- `replaced` entries run only their CCS declarative hooks.
- `legacy` entries run the preserved native scriptlet through the protected
  scriptlet runner with native-compatible args, environment, stdin, and
  ordering.
- `blocked` entries fail before package file or DB mutation.
- `review` entries fail under public/default policy unless an explicit local
  override allows them.

Legacy replay should still be safer than native package managers:

- fail closed when sandbox setup cannot satisfy the selected policy;
- use a minimal environment;
- deny network by default;
- apply seccomp and namespace protections already used by protected scriptlets;
- record structured execution metadata on the changeset;
- distinguish setup/enforcement failures from script exit failures;
- preserve Conary's current warning-only handling only for post-commit script
  exits where rollback cannot be made truthful.

The replay engine must avoid double application. If a raw scriptlet is replayed,
Conary should not also run declarative replacements for effects inside that same
entry unless the entry explicitly declares those replacements as preconditions
and the raw script was rewritten or wrapped to skip the replaced effect.

## Publication Policy

Remi should expose conversion quality as package metadata:

- `scriptlet_fidelity`: `native-free`, `fully-replaced`, `legacy-replay`,
  `review-required`, or `blocked`;
- counts by decision;
- unsupported effect kinds;
- whether triggers are present;
- whether install, update, remove, and purge/remove-equivalent paths are covered;
- link or digest for conversion evidence.

Default public publication should allow:

- packages without scriptlets;
- packages whose scriptlets are fully replaced by declarative hooks;
- packages requiring legacy replay when sandbox preflight and native ABI coverage
  are complete for the relevant lifecycle paths.

Default public publication should reject or quarantine:

- kernel, bootloader, initramfs, PAM, SELinux policy, package-manager recursion,
  or network-using scriptlets until specific support exists;
- packages with unmodeled RPM triggers/file triggers;
- DEB packages whose normal install/configure/remove paths require unsupported
  trigger or debconf behavior;
- Arch packages whose `.INSTALL` cannot be wrapped faithfully.

## CLI And Operator UX

Operators need an inspectable answer, not a hidden conversion decision.

Required UX:

- `conary query scripts <pkg>` shows native entries, decisions, interpreters,
  source format, and effect summaries.
- `conary install --dry-run <pkg>` reports whether the package uses declarative
  hooks, legacy replay, or is blocked before mutation.
- Remi package metadata includes scriptlet fidelity and publication status.
- Changeset history records scriptlet decision and execution outcomes.
- Unsupported packages explain the exact blocked entry and the next safe action,
  such as using the native package manager, running inside a VM, or waiting for
  a curated conversion rule.

## Testing Strategy

The design needs behavioral tests, not just parser tests.

Required test layers:

- unit tests for RPM, DEB, and Arch native ABI extraction;
- unit tests for effect IR serialization and manifest round trips;
- fixture conversion tests for each scriptlet decision;
- sandbox tests proving legacy replay receives native-compatible arguments and
  cannot mutate outside allowed roots;
- golden behavior tests that install a native fixture and converted CCS fixture
  into isolated roots, then compare observable state such as files, symlinks,
  users/groups, systemd enablement markers, alternatives, cache refresh markers,
  and changeset warnings;
- Remi publication-gate tests for public, review, and blocked conversions;
- docs-truth tests for public claims around scriptlet fidelity.

The first golden corpus should include simple packages for:

- no scriptlets;
- user/group creation;
- systemd enable and daemon reload;
- tmpfiles/cache refresh;
- alternatives registration;
- residual unknown shell requiring legacy replay;
- blocked package-manager recursion;
- RPM trigger or file-trigger quarantine;
- DEB trigger quarantine;
- Arch `.INSTALL` wrapper replay.

## Rollout

This should land as a clean-room replacement in slices:

1. Define the bundle data model, parser extraction structs, and query rendering
   without changing installation behavior.
2. Embed bundle sidecars in Remi-generated CCS packages while keeping current
   hook behavior behind a compatibility path.
3. Add replay-engine support for no-scriptlet, fully-replaced, and legacy-replay
   normal install paths.
4. Add publication gating and Remi metadata exposure.
5. Expand update/remove/trigger coverage and block unsupported classes by
   default.
6. Remove the old regex analyzer as an authority after the new tests cover the
   preview conversion corpus.

During rollout, existing converted packages without a bundle must be treated as
legacy/untrusted conversion artifacts. They can be installed only under an
explicit compatibility policy, or regenerated through Remi to receive a bundle.

## Non-Goals

This spec does not require:

- a complete shell parser;
- complete RPM trigger parity in the first implementation slice;
- running unsupported package classes on daily-driver hosts;
- making all Fedora, Ubuntu, and Arch packages publicly publishable at once;
- replacing native package managers for adopted packages.

## Success Criteria

The work is complete for preview when:

- every Remi-converted CCS package carries a legacy scriptlet semantics bundle;
- no scriptlet body from RPM, DEB, or Arch is dropped silently;
- public Remi metadata reports scriptlet fidelity and publication status;
- install dry-run exposes the replay decision before mutation;
- supported fixture packages show equivalent observable behavior between native
  install and converted CCS install;
- unsupported fixture packages fail before mutation with specific reasons;
- regex-based analysis is no longer the authoritative conversion mechanism.

## References

- RPM scriptlets and triggers: <https://rpm.org/docs/latest/manual/scriptlet_expansion>
- RPM spec runtime scriptlets: <https://rpm.org/docs/4.19.x/manual/spec.html>
- Debian Policy maintainer scripts: <https://www.debian.org/doc/debian-policy/ch-maintainerscripts.html>
- Arch install scriptlets: <https://man.archlinux.org/man/alpm-install-scriptlet.5.en>
