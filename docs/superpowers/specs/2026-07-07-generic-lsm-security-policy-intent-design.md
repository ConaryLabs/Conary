# Generic LSM Security Policy Intent Design

**Status:** Active design
**Date:** 2026-07-07
**Related work:** GitHub issue #35, Remi scriptlet evidence queue, `selinux-policy/v1`

## Goal

Conary should make package security-policy behavior portable across supported
distros by modeling Linux Security Module intent as data instead of replaying
distro-specific helper commands. A package from Fedora, Ubuntu, Arch, or a
native CCS source should install on any supported target even when the package's
source distro uses a different LSM. The active target provider should reconcile
the intent it understands, and incompatible or absent providers should leave
the intent dormant with visible state rather than blocking the package.

The first concrete source is SELinux scriptlet conversion. The long-term model
must also cover AppArmor because Ubuntu uses AppArmor as its default MAC system
and Arch ships AppArmor in its official package repositories. SELinux on Arch
is possible but not a normal first-class base setup, so Conary should support it
through explicit target facts rather than assuming Arch implies no SELinux.

## Principles

- Package source format chooses a parser, not whether the target can install.
- LSM helper commands are evidence sources, not install authority.
- No generic SELinux-to-AppArmor translation is allowed by default.
- Target providers decide what can be applied, deferred, or degraded.
- Absent providers are not failures unless the package declares policy as
  required for safe runtime.
- Public Remi serving requires fully modeled intent and deterministic provider
  behavior, never raw scriptlet replay.

## External Facts

- Ubuntu documents AppArmor as its default mandatory access control system
  instead of SELinux:
  <https://documentation.ubuntu.com/security/security-features/privilege-restriction/>
- Ubuntu's security feature overview lists AppArmor across supported releases:
  <https://documentation.ubuntu.com/security/security-features/security-features-overview/>
- Arch ships AppArmor in the official `extra` repository:
  <https://archlinux.org/packages/extra/x86_64/apparmor/>
- The SELinux userspace command surface includes a broad `semanage`, `semodule`,
  `setfiles`, `restorecon`, and `setsebool` family:
  <https://man7.org/linux/man-pages/dir_section_8.html>

## Current Repo Baseline

- `crates/conary-core/src/ccs/convert/selinux_adapters.rs` models a narrow set
  of SELinux commands as `selinux-policy/v1` optional policy effects.
- `PayloadHints` now records payload paths so adapters can prove package-scoped
  policy intent instead of live-root mutation.
- `SupportMatrix` has both a known `selinux-policy/v1` row and a blocked
  `selinux` fallback row for unsupported SELinux mutation.
- `docs/superpowers/specs/2026-07-03-kernel-initramfs-selinux-scriptlet-handling-design.md`
  documents the earlier boot/security evidence lane and the SELinux adapter
  follow-up.
- Remi stores and aggregates scriptlet evidence for review, but raw legacy
  replay remains non-public authority.

## Core Model

Introduce a generic security-policy intent concept that all LSM-specific
parsers feed:

```text
SecurityPolicyIntent {
  id
  source
  provider
  operation
  scope
  desired_state
  requirements
  fallback
  payload_evidence
  reconciliation
}
```

`provider` is one of `selinux`, `apparmor`, `tomoyo`, `smack`, `landlock`, or
`any`. Absence of an LSM is target state, not an intent provider; it belongs in
`LsmProviderFacts`. `any` is reserved for native provider-neutral intent whose
operation can be reconciled by more than one provider. `operation` captures
portable actions such as label refresh, file-context declaration, profile
install, profile reload, boolean set, policy module install, port rule
declaration, domain transition, or policy-store reconcile. `scope` must name the
package-owned path, service, binary, port, user, SELinux type, AppArmor profile,
or other object being affected.

`fallback` describes what happens when the target does not have a compatible
provider: `dormant`, `warning`, `degraded`, or `block-on-enforcing-target`.
The default for converted foreign LSM intent is `dormant` unless the parser can
prove the policy is required for safe runtime on the active target.

## Initial Storage And Compatibility

The first implementation should be additive. Generic `SecurityPolicyIntent`
records live beside existing legacy conversion metadata, and the current
`selinux-policy/v1` effects remain valid evidence for older bundles and review
tools. New converters should bridge those SELinux effects into generic intent,
but they should not remove the source command evidence or the provider-specific
effect until generic provider planning is proven end to end.

Bundles produced before this schema exists are still valid. They simply have no
generic LSM intent and continue through the existing scriptlet-evidence review
path.

## Provider Capabilities

Targets expose an `LsmProviderFacts` view through the same supported-profile
and runtime-target fact path used for lifecycle validation. The minimum fact set
is:

- active LSM providers and ordering
- provider mode such as disabled, complain, permissive, or enforcing
- policy store availability
- provider tooling availability
- supported operation families
- known policy modules, profiles, booleans, ports, labels, or abstractions
- whether reconciliation is allowed in this transaction context

Fedora-like targets normally advertise SELinux facts. Ubuntu-like targets
advertise AppArmor facts. Arch targets may advertise AppArmor, SELinux, neither,
or both depending on the actual installed system. Conary should infer defaults
from target profile IDs only as a starting hint; real install planning should
prefer detected target facts.

Targets can have multiple active LSMs. Planning must evaluate each intent
against the matching provider facts and must not collapse the system into a
single winner. Provider ordering is input to reconciliation when the kernel or
tooling makes ordering meaningful, but provider mismatch for one intent must not
hide a second provider that can reconcile a different intent.

## Reconciliation States

Each intent gets an explicit reconciliation state:

- `applied`: provider accepted the intent.
- `dormant`: provider is absent or incompatible; no action was needed.
- `pending`: provider exists but reconciliation is delayed until another phase.
- `degraded`: provider exists but only part of the intent can be honored.
- `blocked`: active enforcing provider requires the policy and reconciliation
  failed.
- `review`: parser understood enough to cluster evidence but not enough to
  apply safely.

Package install should not block solely because source and target providers
differ. Blocking is reserved for active provider failures that would leave an
enforcing target in a known unsafe or unusable state.

## SELinux Mapping

The existing `selinux-policy/v1` adapter becomes a parser that emits generic
LSM intent:

- `restorecon` becomes label refresh intent scoped to payload paths.
- `semanage fcontext` becomes file-context declaration intent.
- `setsebool -P` becomes boolean desired-state intent.
- `semodule -i` becomes policy-module install intent backed by payload files.

The next SELinux expansion should map the documented userspace family:
`semanage boolean`, `semanage port`, `semanage permissive`, `semanage login`,
`semanage user`, `semanage module`, `semodule` install/remove/enable/disable,
`setfiles`, and safe `restorecon` variants. Broad relabeling, destructive
policy removal, and unbacked policy-store mutation stay review or blocked until
they can be scoped precisely.

## AppArmor Mapping

AppArmor support should be first-class rather than an afterthought. Initial
intent categories are:

- profile install or update from package payload files
- profile reload through `apparmor_parser`
- complain/enforce mode declarations
- local override file preservation
- service/profile association for package-owned binaries

AppArmor intent applies only through an AppArmor provider. SELinux intent does
not automatically become AppArmor policy. Future curated translations may map
high-level intent for known packages, but those translations must be explicit,
versioned, and test-backed.

Before AppArmor packages are public-ready, the converter needs the same command
surface inventory that SELinux is getting now. Debian/Ubuntu and Arch package
helpers should be classified into typed profile install, reload, mode, override,
and service-association intent instead of being treated as opaque shell.

## Cross-Distro Behavior

| Source package intent | Target provider | Expected result |
| --- | --- | --- |
| Fedora SELinux intent | Fedora SELinux | Apply if target facts support the exact operation. |
| Fedora SELinux intent | Ubuntu AppArmor | Store dormant SELinux intent; install continues. |
| Fedora SELinux intent | Arch without LSM facts | Store dormant SELinux intent; install continues. |
| Fedora SELinux intent | Arch with SELinux | Apply if Arch SELinux facts prove compatible tooling and policy store. |
| Ubuntu AppArmor intent | Fedora SELinux | Store dormant AppArmor intent unless a curated translation exists. |
| Native CCS generic intent | Any provider | Provider reconciles matching operations; incompatible provider stores dormant. |

This is the core portability guarantee: every supported package type can
express policy intent, and every supported target can install without pretending
to have the source distro's LSM.

## Public Serving

Public Remi serving can accept packages with LSM intent when:

- every LSM command was converted into typed intent;
- unsupported or incompatible provider behavior has an explicit non-blocking
  fallback;
- active-provider application is deterministic for supported target facts; and
- no raw helper execution is required.

Packages that require a specific provider on the target are still public-ready
only for targets that advertise that provider. For example, a package may be
public-ready for `fedora-44` with SELinux but private-review for `ubuntu-26.04`
if it declares SELinux policy as required rather than dormant.

## Review Queue And Data Collection

The scriptlet evidence queue remains the canonical intake for unreviewed system
mutation. Generic LSM conversion should attach the normalized intent, source
helper command, payload proof, target-provider facts, planned reconciliation
state, and public/private decision to each queued record. That lets testers keep
installing packages on managed systems while maintainers review the exact
adapter gap that prevented public promotion.

Queued LSM records should become rarer as adapters mature. The success metric is
not zero scriptlets, but that common package-manager helper calls are typed
automatically and only genuinely new or unsafe mutations require human review.

## Error Handling

Conversion-time errors should distinguish:

- unknown helper command;
- known provider command with unsupported operation;
- operation parsed but not payload-backed;
- provider mismatch with dormant fallback;
- provider present but missing tooling;
- provider enforcing and reconciliation failed.

For local managed installs, only the last class should normally block. Unknown
or unsupported commands still create review records and non-public conversion
state, but they should not stop a tester from gathering data unless the package
declares the policy required, target facts prove the active enforcing provider
would be left unsafe, or the administrator selected strict enforcement. For
public Remi serving, unknown commands, unsupported operations, and unbacked
policy-store mutation remain private-review until they are modeled.

## Testing Strategy

The proof corpus should include the same package intent across multiple targets:

- Fedora SELinux package on Fedora: applied or pending SELinux intent.
- Fedora SELinux package on Ubuntu: dormant SELinux intent, public-safe when no
  required policy is declared.
- Fedora SELinux package on Arch without SELinux: dormant SELinux intent.
- Fedora SELinux package on Arch with SELinux facts: applied SELinux intent.
- Ubuntu AppArmor package on Ubuntu and Arch with AppArmor: applied AppArmor
  intent.
- Ubuntu AppArmor package on Fedora: dormant AppArmor intent.
- Mixed SELinux/AppArmor package: provider-specific intents reconcile
  independently.

Unit tests should cover intent parsing and provider planning. Integration tests
should verify conversion metadata, Remi public/private outcomes, and local
install planning without executing host policy tools.

Queue-focused tests should prove that an unreviewed LSM helper can continue a
local managed install as review data, while the same package remains non-public
until the helper maps to typed intent or an explicit safe fallback.

## Non-Goals

- No automatic generic SELinux-to-AppArmor policy translation.
- No raw execution of LSM helper tools during conversion.
- No claim that AppArmor and SELinux have equivalent semantics.
- No public serving for packages whose active enforcing provider would be left
  unreconciled.
- No host policy-store mutation in Remi conversion workers.
- No broad rewrite of unrelated bootloader, initramfs, or kernel-module handling
  in this design.

## Implementation Boundary

The first implementation plan should land the generic intent schema as legacy
conversion metadata only. Provider planning should follow after the metadata
shape is stable. CCS v2 native authority should wait until reconciliation
states are proven by conversion, Remi, and local-install tests.
