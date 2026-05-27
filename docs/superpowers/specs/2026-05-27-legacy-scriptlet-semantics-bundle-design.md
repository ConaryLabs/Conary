---
last_updated: 2026-05-27
revision: 4
summary: Clean-room design for preserving and improving RPM, DEB, and Arch scriptlet behavior in Remi-converted CCS packages, including decision rubrics, target compatibility gates, adapter-based command growth, deferred trigger fields, and measured cold-path latency budgets
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

## Decision Rubrics

The converter must make per-entry decisions using explicit rubrics, not broad
"best effort" confidence.

### `replaced`

An entry may be marked `replaced` only when all of these are true:

- every lifecycle path relevant to the entry has been modeled from native ABI,
  static metadata, curated rules, or capture evidence;
- every observed or declared host-integration operation is handled by an
  adapter whose replacement status is `complete`;
- no unknown command, external script call, network use, package-manager
  recursion, privileged kernel/security-policy mutation, or uncertain
  filesystem mutation remains;
- control flow is either irrelevant to the replacement, fully modeled by the
  adapter, or covered by separate native-ABI lifecycle paths;
- the generated CCS hooks are idempotent and have fixture coverage proving the
  expected observable state.

If any operation is only partially understood, the entry is not `replaced`.
Partial effects can produce warnings, evidence, and future adapter work, but
they do not justify dropping the raw scriptlet.

### `legacy`

An entry may be marked `legacy` only when all of these are true:

- the native script body, interpreter, arguments, environment contract, stdin
  contract, and lifecycle ordering are preserved;
- the target compatibility policy allows legacy replay for the host;
- every external helper command is either present in the target, provided by a
  Conary shim, or explicitly allowed by sandbox policy;
- static and captured evidence show no network activity, package-manager
  recursion, unsupported setuid/setcap/sysctl/security-policy mutation, kernel
  or bootloader mutation, or unmodeled trigger dependency;
- the protected scriptlet runner can satisfy the required isolation policy.

If the entry is source-native or family-compatible but one of these checks
cannot be proven, the outcome is `review` or `blocked`, not optimistic replay.

### `review` versus `blocked`

Use `review` when the package may be safe but lacks enough automated evidence
or an adapter has not yet reached required coverage. Use `blocked` when the
entry belongs to a known unsafe or unsupported class under current policy.

Review artifacts are private Remi artifacts. Blocked artifacts are negative
conversion results with structured reasons. Neither is served as a ready public
CCS package under default policy.

### Double-application rule

V1 does not rewrite arbitrary shell to suppress individual commands. If a raw
entry is replayed, Conary must not also run CCS hooks generated from that same
entry unless the entry is explicitly marked with a future residual-replay mode
that proves command suppression in tests. Until that mode exists, an entry is
either fully replaced or fully replayed, never both.

## Remi Default Conversion Contract

Remi remains the default conversion authority for repository packages. A client
request for an RPM, DEB, or Arch package should return a complete CCS package
once conversion finishes, including payload, dependencies, provides, config-file
semantics, scriptlet semantics bundle, target compatibility result, conversion
evidence digest, and publication status.

Client-side conversion should be reserved for explicit local-file workflows,
developer diagnostics, or emergency compatibility modes. The normal repository
path is:

1. client asks Remi for a package;
2. Remi returns an existing complete CCS package, or starts/joins one conversion
   job;
3. Remi performs conversion, scriptlet classification, adapter extraction,
   bundle embedding, chunking, CAS storage, and DB publication;
4. client downloads a complete CCS package and never has to reinterpret the
   original native package format.

Partially converted packages must not be served as ready. If the scriptlet
bundle cannot be produced inside policy, Remi should publish a structured
`review-required` or `blocked` result instead of returning an incomplete CCS
artifact.

## Cold-Path Latency Budget

On-demand conversion has to be good enough for first-time use, but it does not
have to do every expensive validation step synchronously. The request path must
produce the complete CCS package and bundle; deeper corpus validation and native
versus CCS golden comparisons belong in pre-warm, CI, or curator workflows.

Target budgets for normal preview packages:

- hot converted package: return metadata immediately from DB and serve chunks
  from local/R2 cache;
- cold small package: complete within about 5 seconds;
- cold medium package with scriptlets: complete within about 30 seconds;
- cold large package or dependency-heavy conversion: complete within the
  existing client polling window, currently 5 minutes;
- packages that cannot meet policy or budget should fail with `review-required`
  or `blocked`, not degrade to a partial artifact.

These are product SLOs, not claims about the current implementation. The
implementation plan must add conversion timing telemetry before public claims
use these numbers.

Before schema or replay implementation work begins, the implementation plan must
benchmark the current Remi cold path with representative packages and split the
timing by phase:

- upstream metadata lookup and package download;
- archive extraction and payload indexing;
- native metadata and scriptlet extraction;
- capture, when enabled;
- adapter dispatch and bundle generation;
- CDC chunking, CAS write, R2 write-through, and DB publication.

The benchmark corpus should include at least one small package, one common
scriptlet-bearing service package, one large payload package, and one
dependency-heavy desktop/library package from the preview lanes. Example
starting points are `jq`, `nginx`, `linux-firmware`, and a Qt base package, but
the exact package names should follow the supported Fedora 44, Ubuntu 26.04, and
Arch repositories available to the benchmark runner.

If measured timings miss the target budgets, the budgets must be revised or the
implementation plan must name concrete optimizations before public release notes
claim the numbers.

The scriptlet bundle should add little overhead for common packages:

- static native ABI extraction and adapter dispatch should be metadata-bound
  and normally sub-second;
- capture should run only for scriptlet entries that need observation evidence;
- capture should execute with per-entry timeouts and deterministic mocked
  helper commands;
- adapter results should be cached by `(source package checksum, conversion
  version, adapter registry version, target policy version)`;
- repeated requests for the same package must join the same in-flight job;
- dependency bursts should be queued and bounded by Remi's conversion
  concurrency controls.

The main latency risks are upstream package download size, archive extraction,
CDC chunking, R2 write-through, and scriptlets that require capture. The design
should optimize those before weakening scriptlet fidelity:

- pre-warm popular packages and common dependency closures;
- keep scriptlet extraction mostly metadata/adapter driven;
- avoid behavioral golden comparison in the request path;
- expose ETA and phase-specific progress when a conversion is queued;
- publish aggregate unknown-command and timing reports so the next adapter work
  is chosen by actual cold-path impact.

## Native ABI Model

Conary must model scriptlet slots by package format instead of flattening them
into generic pre/post names too early.

Every native ABI entry must also carry lifecycle-path coverage. For example,
an RPM `%post` entry may have first-install and upgrade call paths, while a DEB
`postinst` entry may have `configure`, `abort-configure`, and trigger-related
paths. Capture or static modeling must cover every path relevant to the
decision. Paths that cannot be modeled in the first implementation keep the
entry in `legacy`, `review`, or `blocked` depending on the replay safety
checklist.

### RPM

RPM supports basic scriptlets, transaction scriptlets, triggers, and file
triggers. The model must preserve at least:

- slot name such as `%pre`, `%post`, `%preun`, `%postun`, `%pretrans`,
  `%posttrans`, trigger, and file-trigger variants;
- interpreter program and interpreter flags;
- script body after RPM macro expansion as present in the binary RPM;
- pre-expansion macro source when available from source packages or curated
  rules, but never as a requirement for binary RPM fidelity;
- install/remove/upgrade argument conventions;
- trigger condition, trigger target package constraints including name,
  operator, and version, trigger priority, and file-matching glob patterns
  where the parser exposes them;
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

## Target Compatibility Model

Multi-format conversion does not mean every converted package is automatically
portable across every live root. A Fedora RPM converted to CCS is not safe on an
Arch host just because the payload is now in CCS format. Scriptlets and helper
tools often assume source-distro policy, source-distro paths, package-manager
state, SELinux/AppArmor defaults, trigger behavior, and service-management
conventions.

Every converted package must therefore carry target compatibility metadata:

- `source_format`: `rpm`, `deb`, or `arch`;
- `source_family`: ecosystem family such as Fedora/RHEL, Debian/Ubuntu, or
  Arch/ALPM;
- `source_distro` and `source_release` when known;
- `version_scheme`;
- `target_compatibility`: `source-native`, `family-compatible`,
  `conary-portable`, `review-required`, or `blocked`;
- `allowed_targets`: optional distro/release identifiers when compatibility is
  narrower than the family;
- `foreign_replay_policy`: whether raw legacy replay may run on a non-native
  target. The default is `deny`.

Compatibility meanings:

- `source-native`: raw legacy replay is allowed only on matching source
  distro/release or an explicitly compatible target.
- `family-compatible`: raw legacy replay is allowed on a same-family target
  after helper-command and path preflight succeeds.
- `conary-portable`: scriptlet behavior is fully replaced by Conary-native IR
  and hooks, with no raw source-distro replay required.
- `review-required`: Remi may retain the artifact privately, but default
  install and public publication refuse it.
- `blocked`: unsupported or unsafe for conversion under current policy.

Cross-distro install of converted packages is allowed by default only for
`conary-portable` packages. `source-native` and `family-compatible` packages may
still be useful, but their replay is gated by the host's source-selection
policy:

- `strict`: reject foreign legacy replay before mutation.
- `guarded`: reject foreign legacy replay unless the package is explicitly
  marked compatible with the host target and helper preflight passes.
- `permissive`: require an explicit operator override for foreign legacy replay,
  and record that override in changeset metadata.

This keeps Conary's mixed-package story honest: Conary can understand many
ecosystems, but source-distro scriptlets do not get to mutate an arbitrary live
root by default.

### Distro Identifiers And Compatibility Matrix

Target identifiers use this logical form:

```text
<format>/<distro>/<release>/<arch>
```

Examples:

- `rpm/fedora/44/x86_64`
- `deb/ubuntu/26.04/x86_64`
- `arch/arch/rolling/x86_64`

Derivatives are not automatically compatible because they share a package
format. `family-compatible` must be granted by an explicit compatibility matrix
entry from `(source_format, source_distro, source_release, source_arch)` to
`(target_format, target_distro, target_release, target_arch)`.

Each matrix entry must define preflight checks, such as:

- required helper commands and accepted version ranges;
- expected service manager and initramfs/kernel integration capabilities;
- filesystem path conventions;
- security-policy assumptions such as SELinux or AppArmor;
- whether native trigger/debconf behavior is required;
- sandbox features required for replay.

Absent a matrix entry, same-format packages are treated as `source-native`, not
`family-compatible`.

## Bundle Shape

The CCS package should carry one structured bundle, either as a manifest section
or as a manifest-referenced sidecar. The bundle should be versioned separately
from the broader CCS manifest so the semantics model can evolve without
rewriting unrelated package metadata.

Recommended logical shape:

This is an abbreviated, non-normative sketch of the bundle shape. The exact v1
field names, enum values, reserved metadata fields, and validation rules are
defined in
`docs/superpowers/specs/2026-05-27-legacy-scriptlet-bundle-schema-v1-passive-query-design.md`.

```toml
[legacy_scriptlets]
schema = "conary.legacy-scriptlets.v1"
schema_revision = 1
source_format = "rpm"
source_family = "fedora"
source_distro = "fedora"
source_release = "44"
source_arch = "x86_64"
source_package = "nginx"
source_version = "1.28.0-1.fc44"
version_scheme = "rpm"
conversion_tool = "remi"
conversion_tool_version = "0.1.0"
conversion_policy = "safe-or-legacy"
target_compatibility = "source-native"
allowed_targets = ["rpm/fedora/44/x86_64"]
foreign_replay_policy = "deny"
publication_policy = "public-if-no-blocked"
publication_status = "private-review"
scriptlet_fidelity = "legacy-replay"

[legacy_scriptlets.decision_counts]
legacy = 1

[[legacy_scriptlets.entries]]
id = "rpm:%post"
native_slot = "%post"
phase = "post-install"
lifecycle_paths = ["install:first", "upgrade:new"]
interpreter = "/bin/sh"
interpreter_args = []
body_sha256 = "..."
body = "..."
decision = "legacy"
reason_code = "residual-shell-after-modeled-systemd-reload"
timeout_ms = 30000
transaction_order = { position = "after-payload" }

[[legacy_scriptlets.entries.effects]]
kind = "systemd-daemon-reload"
source = "capture-log"
confidence = "observed"
replacement = "complete"
adapter_id = "systemd-daemon-reload/v1"
adapter_digest = "sha256:..."
```

The final encoding can be optimized for binary CCS packages, but the logical
data must remain inspectable through `conary query scripts` or an equivalent
command.

### Bundle Schema v1 Field Requirements

The first schema must reserve fields for deferred scriptlet classes so Remi does
not need to rewrite stored CCS packages when triggers and purge paths become
supported.

Top-level required fields:

- schema ID and schema revision;
- source format, source family, source distro, source release, source arch, and
  version scheme;
- source package name, source version, source package checksum, and conversion
  tool version;
- adapter registry digest and target policy digest;
- target compatibility, allowed targets, foreign replay policy, publication
  policy, and scriptlet fidelity;
- aggregate counts by decision and unsupported class.

Per-entry required fields:

- stable entry ID, native slot, native phase, lifecycle paths, interpreter,
  interpreter args, body digest, preserved body, and body encoding;
- native invocation arguments for every modeled lifecycle path;
- transaction ordering marker such as before-payload, after-payload,
  pre-transaction, post-transaction, trigger, file-trigger, or remove/purge;
- timeout policy and sandbox/capability requirements;
- decision, reason code, human-readable reason, evidence digest, and source
  evidence references;
- effects with adapter IDs, adapter digests, replacement status, confidence,
  and original command evidence;
- unknown commands and blocked classes observed for that entry.

Deferred but reserved fields:

- RPM trigger condition, target package constraints including name, operator,
  and version, priority, file-matching glob patterns, stdin path contract, and
  trigger ordering;
- DEB maintainer-script invocation mode, old/new version arguments, control
  `triggers` content, debconf requirements, and purge/abort modes;
- Arch function name, old/new version arguments, and `.INSTALL` source digest;
- residual-replay metadata for a future mode that can prove command
  suppression without double application.

Schema evolution is additive within `v1`: unknown optional fields are preserved,
and unknown enum variants fail closed to `review` unless the running converter
explicitly supports them. Removing fields or changing the meaning of a required
field requires a new schema ID.

## Effect Adapter Registry

Conary needs a fast, repeatable way to add support for newly encountered
scriptlet helper commands. Support should not require rewriting the converter
each time a package uses another distro helper.

Add an effect adapter registry with two adapter classes:

- **Declarative adapters**: data-defined mappings for simple command forms.
  These are suitable for commands such as `ldconfig`,
  `gtk-update-icon-cache`, `glib-compile-schemas`, `update-mime-database`,
  `systemd-tmpfiles --create`, or straightforward `systemctl daemon-reload`.
- **Native Rust adapters**: code-defined parsers for commands whose semantics
  depend on arguments, filesystem state, distro policy, or multi-command flows,
  such as `systemctl preset`, `deb-systemd-helper`, `update-alternatives`,
  `dpkg-trigger`, `restorecon`, `semanage`, or package-manager recursion.

Each adapter must declare:

- command names and absolute paths it handles;
- supported source families and target compatibility implications;
- supported lifecycle phases;
- accepted argument shapes;
- emitted effect IR;
- whether the effect replacement is complete, partial, or blocked;
- sandbox/capability requirements;
- fixture tests and at least one conversion corpus example.

Adapter registry versions are content-addressed. Each entry records the adapter
IDs and adapter content hashes that influenced its effects and decision. A
global registry version may be used for coarse cache invalidation, but Remi
should be able to identify which entries need re-evaluation when one adapter
changes.

Adapters consume structured command invocations, not arbitrary regex matches
against whole scripts. Conversion capture should record command, argv, cwd,
environment deltas, stdin summary, scriptlet entry ID, and source phase. Static
analysis may produce possible invocations, but only captured invocations,
native metadata, payload metadata, or curated rules can justify a `complete`
replacement.

Unknown commands are first-class output:

- same-target packages may fall back to `legacy` replay when the native ABI and
  sandbox policy are complete;
- foreign-target packages default to `review` or `blocked`;
- Remi records unknown commands in conversion evidence so maintainers can rank
  the next adapters by real corpus frequency;
- adding a new adapter requires a fixture, a golden conversion expectation, and
  a regression entry in the command-support matrix.

This creates a quality ratchet: every new helper command Conary learns becomes
structured, tested conversion knowledge rather than another one-off heuristic.

Before the public preview advertises broad Remi conversion, the registry must
have a measured bootstrap set for the preview corpus. The first corpus scan
should cover Fedora 44 base/update packages in scope, Ubuntu 26.04 main packages
in scope, and Arch core/extra packages in scope. At minimum, the implementation
plan must report:

- the top helper commands and command forms by occurrence;
- the percentage of packages with no scriptlets;
- the percentage eligible for `replaced`, `legacy`, `review`, and `blocked`;
- the top blocked classes by package count and by user-visible importance;
- the adapter set required to cover the top 25 helper command names, or every
  helper command that accounts for at least 1% of observed scriptlet command
  occurrences, whichever is broader.

If the blocked/review rate is too high for the advertised preview lane, Remi
should expose a curated converted-package lane first instead of implying that
the full native repository is ready.

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
- SELinux/AppArmor labeling or policy changes, initially blocked unless a
  target-compatible adapter exists;
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
Current `hooks.post_install` and `hooks.pre_remove` script hooks remain valid
for native CCS packages and existing transitional artifacts, but Remi-converted
legacy package scriptlets must be represented by the bundle. Arbitrary
`ScriptHook` fields must not be used as the lossy preservation path for native
RPM, DEB, or Arch scriptlet entries.

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
5. **Run effect adapters.** Feed captured invocations, native metadata, payload
   metadata, and curated static signals into the adapter registry to produce
   effect IR and target-compatibility implications.
6. **Apply curated conversion rules.** Allow Remi-maintained rules for common
   distro helper macros and package classes. Curated rules must include source,
   version scope, and tests.
7. **Choose per-entry decisions.** Select `replaced`, `legacy`, `blocked`, or
   `review` for every entry.
8. **Assign package target compatibility.** Derive the package-level
   compatibility from all entry decisions, adapter results, and unsupported
   source-distro assumptions.
9. **Build CCS hooks.** Emit hooks only for effects with complete replacements.
10. **Embed the bundle.** Store the original scriptlets and decisions in the CCS
   package.
11. **Gate publication.** Public Remi cache publication requires no `blocked`
   entries, no unreviewed package classes outside the preview policy, and a
   target compatibility result that matches the advertised repository lane.

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

Before running any `legacy` entry, the replay engine must perform target
compatibility preflight:

- compare bundle source metadata with the host target and effective
  source-selection policy;
- verify required helper commands exist or have Conary-provided shims;
- verify required paths and service manager capabilities are target-compatible;
- reject foreign replay unless the bundle and policy explicitly allow it;
- record the compatibility decision and any operator override in the changeset.

Install must persist the complete Legacy Scriptlet Semantics Bundle into local
package state, because remove and upgrade operations may run after the original
`.ccs` archive is no longer present. The replay engine must retrieve the stored
bundle during remove and upgrade instead of falling back to the older raw
scriptlet table alone. That stored bundle is what preserves target
compatibility, sandbox requirements, per-entry decisions, timeouts, and evidence
needed to apply the same safety boundary after install.

The replay engine must avoid double application. If a raw scriptlet is replayed,
Conary should not also run declarative replacements for effects inside that same
entry unless the entry explicitly declares those replacements as preconditions
and the raw script was rewritten or wrapped to skip the replaced effect.

## Blocked-Class Registry

Remi must maintain a blocked-class registry alongside the adapter registry. The
registry turns deferrals into trackable policy instead of implicit gaps.

Each class entry records:

- class ID and description;
- default outcome: `replaced`, `legacy`, `review`, or `blocked`;
- affected package formats and preview distros;
- unblock criteria;
- required adapters, replay capabilities, or target compatibility matrix
  entries;
- fixture and golden-corpus requirements.

Initial registry expectations:

| Class | Initial outcome | Unblock criteria |
| --- | --- | --- |
| `ldconfig` | `replaced` candidate | Adapter emits idempotent dynamic-linker-cache effect or proves no-op under generation model. |
| `icon-cache`, `font-cache`, `desktop-db`, `mime-db`, `gsettings` | `replaced` candidate | Declarative cache-refresh adapters with fixture coverage. |
| `systemd-enable`, `systemd-preset`, `daemon-reload` | `replaced` or `legacy` | Native helper semantics modeled per source family and target preflight. |
| `tmpfiles`, `sysusers` | `replaced` candidate | Payload metadata extraction plus hook support and ordering tests. |
| `alternatives` | `replaced` candidate | Adapter maps native registration/removal to CCS alternatives hooks. |
| `dbus-policy` | `review` | D-Bus policy/service registration adapter and target compatibility checks. |
| `pam` | `blocked` | Explicit PAM policy model and daily-driver safety review. |
| `selinux`, `apparmor` | `blocked` | Target-compatible labeling/policy adapter and rollback story. |
| `kernel-module`, `initramfs`, `bootloader` | `blocked` | Kernel/initramfs/boot artifact plan and isolated golden tests. |
| `package-manager-recursion` | `blocked` | No default unblock path; requires explicit product decision. |
| `network` | `blocked` | No default unblock path; requires explicit product decision. |
| `rpm-trigger`, `rpm-file-trigger` | `review` or `blocked` | Native trigger ABI extraction, ordering model, and replay/golden tests. |
| `deb-trigger`, `debconf`, `purge`, `abort-*` | `review` or `blocked` | DEB trigger/debconf mode model and lifecycle-path tests. |

## Publication Policy

Remi should expose conversion quality as package metadata:

- `scriptlet_fidelity`: `native-free`, `fully-replaced`, `legacy-replay`,
  `review-required`, or `blocked`;
- `target_compatibility`: `source-native`, `family-compatible`,
  `conary-portable`, `review-required`, or `blocked`;
- source format, family, distro, release, and version scheme;
- counts by decision;
- unsupported effect kinds;
- unknown helper commands with frequency and phase;
- whether triggers are present;
- whether install, update, remove, and purge/remove-equivalent paths are covered;
- link or digest for conversion evidence.

Default public publication should allow:

- packages without scriptlets;
- packages whose scriptlets are fully replaced by declarative hooks;
- packages requiring legacy replay when sandbox preflight and native ABI coverage
  are complete for the relevant lifecycle paths and the repository lane matches
  the package target compatibility.

Default public publication should reject or quarantine:

- kernel, bootloader, initramfs, PAM, SELinux policy, package-manager recursion,
  or network-using scriptlets until specific support exists;
- packages with unmodeled RPM triggers/file triggers;
- DEB packages whose normal install/configure/remove paths require unsupported
  trigger or debconf behavior;
- Arch packages whose `.INSTALL` cannot be wrapped faithfully.
- source-native packages requested for a cross-distro portable lane.

## Review And Curation Workflow

The `review` outcome needs an explicit promotion path.

1. Remi stores the artifact privately with the bundle, source package digest,
   evidence digest, unknown commands, blocked-class hits, and timing profile.
2. A curator runs `remi conversion inspect` or equivalent tooling to view native
   entries, adapter evidence, target compatibility, and golden-test gaps.
3. The curator may add a scoped curated rule, add or improve an adapter, narrow
   allowed targets, or mark the artifact blocked.
4. Promotion to public cache requires rerunning conversion with the updated
   adapter/rule set and producing a new evidence digest.
5. Curated decisions must be scoped by source package checksum, package name
   and version range, source distro/release, or a documented helper-command
   pattern. Unscoped one-off approvals are not valid.
6. Every promotion produces an audit entry that can be linked from Remi package
   metadata.

Human review is acceptable for early preview, but the workflow must feed back
into adapters, curated rules, or blocked-class policy so the same package class
does not require repeated manual judgment.

## CLI And Operator UX

Operators need an inspectable answer, not a hidden conversion decision.

Required UX:

- `conary query scripts <pkg>` defaults to a concise summary: package
  fidelity, target compatibility, counts by decision, blocked/review reasons,
  and whether raw legacy replay is needed.
- `conary query scripts <pkg> --verbose` shows native entries, decisions,
  interpreters, lifecycle paths, source format, effect summaries, adapter IDs,
  unknown commands, and timeout/sandbox requirements.
- `conary query scripts <pkg> --entry <id>` shows one entry in detail.
- `conary query scripts <pkg> --json` emits stable machine-readable output for
  support bundles, Remi dashboards, and corpus analysis.
- `conary install --dry-run <pkg>` reports whether the package uses declarative
  hooks, legacy replay, or is blocked before mutation, and whether the replay is
  native, family-compatible, or foreign.
- Remi package metadata includes scriptlet fidelity and publication status.
- Remi conversion reports include top unknown commands so new adapter work can
  be prioritized from actual package corpus data.
- Changeset history records scriptlet decision and execution outcomes.
- Unsupported packages explain the exact blocked entry and the next safe action,
  such as using the native package manager, running inside a VM, or waiting for
  a curated conversion rule.

## Testing Strategy

The design needs behavioral tests, not just parser tests.

Required test layers:

- unit tests for RPM, DEB, and Arch native ABI extraction;
- unit tests for effect IR serialization and manifest round trips;
- schema compatibility tests proving reserved trigger/purge fields round-trip
  even before enforcement exists;
- adapter tests for every supported helper command and every blocked helper
  command class;
- fixture conversion tests for each scriptlet decision;
- latency benchmark tests or benchmark harness output for the current Remi
  cold-path phases before product SLOs are published;
- sandbox tests proving legacy replay receives native-compatible arguments and
  cannot mutate outside allowed roots;
- target compatibility tests proving Fedora-native legacy replay is rejected on
  Arch-like targets under `strict` and `guarded` policy unless explicitly
  marked compatible;
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
- foreign legacy replay rejection;
- RPM trigger or file-trigger quarantine;
- DEB trigger quarantine;
- Arch `.INSTALL` wrapper replay.

## Rollout

This should land as a clean-room replacement in slices:

0. Benchmark the current Remi cold path and scan the preview corpus for
   scriptlet/helper-command frequency.
1. Define the bundle data model, reserved schema fields, parser extraction
   structs, command evidence format, and query rendering without changing
   installation behavior.
2. Define the adapter registry, blocked-class registry, bootstrap support
   matrix, and first adapters.
3. Embed bundle sidecars in Remi-generated CCS packages as passive metadata
   while keeping current install behavior behind a compatibility path.
4. Add publication metadata and review/blocked results without enforcing them on
   install.
5. Add replay-engine support for no-scriptlet, fully-replaced, and legacy-replay
   normal install paths behind a feature gate.
6. Add target compatibility preflight and foreign replay refusal.
7. Enable publication gating for the curated preview lane.
8. Expand update/remove/trigger coverage and block unsupported classes by
   default.
9. Remove the old regex analyzer as an authority after the adapter registry
   reaches parity for common cases covered by the preview corpus.

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
- every converted package has a target compatibility result, and foreign legacy
  replay is denied by default;
- new helper-command support can be added through the adapter registry with
  fixtures, golden expectations, and support-matrix coverage;
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
