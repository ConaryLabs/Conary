# Scriptlet Public Authority Roadmap Design

**Status:** Approved umbrella design
**Date:** 2026-07-08
**Related specs:**
`docs/superpowers/specs/2026-07-03-kernel-initramfs-selinux-scriptlet-handling-design.md`,
`docs/superpowers/specs/2026-07-08-remi-non-public-test-serving-design.md`,
and the archived legacy scriptlet adapter/publication designs under
`docs/superpowers/specs/archive/`

## Goal

Conary should keep converted legacy scriptlets fail-closed by default while
growing a precise, auditable path from legacy host mutation to native CCS
authority. The public Remi gate must remain stricter than local review or test
workflows: public serving is allowed only when a converted package is
native-free, or every scriptlet effect is fully replaced by adapter evidence
that also passes an explicit public policy for the target.

This umbrella is the parent spec for follow-on implementation plans. It covers
security-sensitive public-ready policy, generation-aware file capability
preservation, Remi non-public test serving, docs/schema truth, and the
maintainability work needed to keep future adapter slices reviewable.

## Baseline

The current conversion model has the right high-level shape:

- blocked and review classes are detected before adapter evidence can promote
  anything;
- only a small set of blocked classes can be overridden by complete adapter
  replacement;
- converted bundles become public only when they are native-free or fully
  replaced;
- public Remi package, detail, index, sparse, OCI, and chunk surfaces consult
  the public-ready predicate;
- Remi non-public test serving is admin-scoped, default-off, and does not
  rewrite publication status.

The remaining correctness gap is policy precision. Some adapters are
syntactically narrow but still security-broad. The clearest case is
`file-capability/v1`: `setcap cap_*=+ep <payload-executable>` is payload-gated
and projects into manifest authority, but its capability acceptance is the
manifest's known Linux capability table, not a public-risk allowlist. That table
contains capabilities such as `cap_sys_admin`, `cap_sys_module`,
`cap_sys_rawio`, `cap_sys_boot`, `cap_bpf`, and `cap_net_admin`, which should
not become public-ready merely because the command shape is narrow.

Sysctl has the same pattern in a milder form. `sysctl/v1` accepts a single
validated `sysctl -w <key>=<value>` and rejects denied keys, but denylist-shaped
validation is not the same as positive public target policy.

## Safety Invariants

- Raw legacy scriptlet replay is never public-serving authority.
- Public-ready conversion requires complete replacement plus positive public
  policy for the exact authority being projected.
- Syntactic narrowness alone is not enough for public-ready status when the
  operation grants kernel, LSM, boot, PAM, privilege, network, or package
  manager authority.
- Review-required, blocked, malformed, stale, and local-only rows do not appear
  in public Remi listings, search, sparse indexes, OCI tags, TUF targets, or
  public chunk routes.
- Non-public test serving remains default-off, admin-only, sanitized, and
  separate from publication authority.
- Generation-aware installs must fail closed for file capabilities until
  `security.capability` xattrs are preserved through image construction,
  activation, rollback, and verification.
- Target-profile facts are the public policy boundary for distro-dependent
  authority. Defaults must answer unsupported.
- Broad refactors are allowed only when tied to a touched product slice,
  failing gate, or factual drift.

## Current Repo Facts

- `crates/conary-core/src/ccs/convert/adapters.rs` owns adapter registry,
  dispatch, common helpers, built-in adapter tests, and the blocked-class
  override rule.
- `crates/conary-core/src/ccs/convert/blocked_classes.rs` owns unsafe command
  family defaults such as network, package-manager recursion, PAM, kernel
  module, initramfs, bootloader, setuid/setcap, sysctl, SELinux, and AppArmor.
- `crates/conary-core/src/ccs/convert/support_matrix.rs` records adapter and
  blocked-class rows plus fixture evidence.
- `crates/conary-core/src/ccs/convert/converter.rs` projects complete adapter
  evidence into native manifest authority such as `hooks.sysctl`,
  `policy.allow_setuid_paths`, and `[[file_capabilities]]`.
- `crates/conary-core/src/ccs/manifest.rs` validates native manifest
  `FileCapability` entries against the known Linux capability table.
- `apps/conary/src/commands/install/transaction.rs` rejects generation-aware
  file capability installs until xattr propagation exists.
- `apps/remi/src/server/publication.rs` classifies converted rows as public,
  review-required, or blocked from scriptlet summary metadata.
- `apps/remi/src/server/handlers/admin/non_public_test_serving.rs` serves valid
  non-public rows through admin test routes while refusing public-ready,
  stale, malformed, missing, and ambiguous rows.
- `crates/conary-core/src/db/models/converted.rs` validates scriptlet summary
  shape for publication and currently requires `boot_security_intents`.
  Publication reports also include `security_policy_intents`, so the shape gate
  should require that field as well.
- `docs/modules/ccs.md`, `docs/modules/remi.md`, `docs/SCRIPTLET_SECURITY.md`,
  and `docs/modules/test-fixtures.md` describe the current public gate and
  fixture ownership surfaces.

## Design Principle

Every public-ready adapter has two contracts:

1. **Replacement contract:** the legacy effect is fully represented by native
   CCS or portable policy metadata, with no raw replay required.
2. **Public policy contract:** the represented authority is safe for the
   target profile without private review.

Existing adapters already focus on the replacement contract. This roadmap adds
the public policy contract where security-sensitive authority needs more than
command-shape parsing.

## Workstream A: File Capability Policy Precision

### Decision

Split file capability validation into at least two concepts:

- **Known Linux capability:** valid manifest syntax and install-time name
  recognition.
- **Public-ready file capability:** capability names allowed to pass public
  conversion policy without private review.

The first public-ready allowlist should be intentionally tiny. Start with
`cap_net_bind_service` because it is common, narrow, and already used by the
fixture corpus. High-risk capabilities remain private-review until a target
profile explicitly authorizes them.

High-risk examples that must not be public-ready by default:

- `cap_sys_admin`
- `cap_sys_module`
- `cap_sys_rawio`
- `cap_sys_boot`
- `cap_sys_ptrace`
- `cap_bpf`
- `cap_net_admin`
- `cap_setpcap`
- `cap_setfcap`

### Behavior

- `setcap cap_net_bind_service=+ep /usr/bin/demo` may remain fully replaced and
  public-ready when the executable is payload-backed.
- `setcap cap_sys_admin=+ep /usr/bin/demo` remains recognized evidence, but the
  bundle must be private-review or blocked for public Remi until target policy
  explicitly allows it.
- Unknown capability names remain rejected or blocked as they are today.
- Inheritable, process, ambient, removal, setpriv, setgid, broad chmod, and
  non-payload privilege mutations stay blocked/private.

### First Plan

The first child plan should add tests before implementation:

- adapter classification for an allowed public capability stays replaced;
- high-risk known capabilities do not produce public-ready publication status;
- conversion still projects allowed public capabilities into
  `[[file_capabilities]]`;
- support-matrix and golden fixture evidence distinguish public-ready from
  private-review capability evidence.

## Workstream B: Generation-Aware File Capability Propagation

### Decision

Do not make generation-aware installs accept `file_capabilities` until the
generation builder preserves `security.capability` through the whole lifecycle.
The current fail-closed check is correct and remains in force.

### Requirements

A future xattr propagation plan must cover:

- storage of `security.capability` in the generation image input model;
- preservation through EROFS or the selected generation carrier;
- activation proof that the selected generation exposes the xattr on the target
  executable;
- rollback proof that the prior generation's capability state is restored;
- verification or inspection output that reports expected file capability
  authority;
- failure behavior when the host or image format cannot preserve the xattr.

### Interaction Boundary

This work crosses CCS install, transaction commit, generation builder, image
artifact, and rollback behavior. It needs the generation interaction gate, not
only CCS conversion tests.

## Workstream C: Sysctl Target-Profile Public Policy

### Decision

Keep `sysctl/v1` syntactic parsing narrow, but add positive public policy before
declaring converted sysctl scriptlets public-ready. The public policy should
live in target-profile facts rather than in a free-floating denylist.

### Requirements

- `TargetProfileQuery` gains a public sysctl policy query or equivalent
  profile-backed allowlist.
- Defaults answer unsupported.
- Existing denied-key validation remains as a defensive floor.
- Public-ready conversion requires both parse success and target-profile
  approval for the exact key, and possibly value constraints if needed.
- Non-public evidence can still be served through the admin test lane when it
  is valid but not public-ready.

## Workstream D: LSM Policy Semantics

### Decision

Keep the current SELinux and AppArmor narrow lanes, but do not broaden public
promotion without explicit provider semantics.

Current public-ready lanes:

- supported SELinux helper evidence represented as optional generic
  `SecurityPolicyIntent`;
- payload-backed AppArmor profile reload through
  `apparmor_parser -r|--replace /etc/apparmor.d/<profile>`.

Future public-ready expansion requires:

- target provider facts for SELinux and AppArmor availability, mode, and policy
  store behavior;
- profile or module content validation where applicable;
- explicit semantics for enforce, complain, disable, status, broad reloads, and
  directory reloads;
- absent-provider behavior that is safe and operator-visible;
- review artifacts that show provider-specific evidence without exposing
  private host paths.

Unsupported AppArmor mode changes, disable/status helpers, broad reloads, and
unbacked profile paths remain blocked/private until modeled.

## Workstream E: PAM, Kernel, Initramfs, And Bootloader Authority

### Decision

Keep these classes blocked for public Remi until native authority is explicit
and generation-aware.

### Native Authority Criteria

PAM needs:

- an explicit PAM adapter;
- operator-visible policy impact;
- target-profile facts for PAM stack layout and supported service files;
- rollback and review behavior.

Kernel and initramfs need:

- target kernel ABI facts;
- module directory and boot artifact layout facts;
- no-live-host execution contracts;
- generation-scoped artifact construction;
- rollback semantics;
- release validation before public Remi state is written.

Bootloader mutation needs:

- bootloader ownership assumptions;
- artifact validation;
- rollback and recovery model;
- target-profile policy before any public-ready conversion.

Until those exist, the non-public lane may help maintainers inspect converted
artifacts, but it does not publish them.

## Workstream F: Network And Package-Manager Recursion

### Decision

Live fetch and nested package-manager calls remain blocked for public-ready
conversion. Future support must model dependency intent or curated offline
artifacts, not run foreign package managers during conversion or install.

Future work can classify evidence for:

- dependency extraction;
- offline artifact requirements;
- repository metadata hints;
- maintainer review clusters.

That evidence remains advisory until native dependency or artifact authority is
available.

## Workstream G: Remi Non-Public Test Serving

### Decision

Preserve the current separation between public serving and admin test serving.
The test lane may serve valid blocked or review-required artifacts to maintainers
when explicitly enabled. It must never become a public gate bypass.

### Follow-On Alignment

The next Remi alignment slice should decide whether `local-only` is meant to be
test-servable:

- If yes, docs should say the lane serves valid non-public rows, including
  local-only.
- If no, the handler should reject local-only rows with a specific response.

Either choice is acceptable only if public package, chunk, OCI, search, sparse,
index, and detail routes keep filtering through public-ready status.

## Workstream H: Schema And Docs Truth

### Required Corrections

- Require `security_policy_intents` in the publication summary shape gate when
  non-default scriptlet metadata is present.
- Keep `boot_security_intents` and `security_policy_intents` both sanitized in
  public refusal and non-public test responses.
- Refresh CCS/Remi docs when file-capability or sysctl public policy changes.
- Keep `docs/modules/ccs.md` front matter dates aligned when its public claims
  change.
- Keep `docs/modules/test-fixtures.md` aligned with new fixture families.
- Update the documentation accuracy ledger and inventory for every new active
  spec or plan.

### Verification Floor

Doc and metadata slices use:

```bash
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

## Workstream I: Enabling Refactors

### Decision

Refactor only where a product or policy slice forces the boundary. The goal is
not to split files mechanically; it is to make the next adapter and publication
changes easy to review.

### Candidate Boundaries

- `adapters.rs`: split registry/dispatch, common parsing helpers, privilege
  adapters, service/cache adapters, and adapter-family tests.
- `converter.rs`: move manifest projections for sysctl, setuid,
  file-capabilities, and security policy into a `manifest_projection` module;
  keep build/archive/provenance orchestration separate from classification
  projection.
- `publication.rs`: split decisions/report construction, review artifact
  persistence, and chunk/public reachability helpers.
- `handlers/admin/non_public_test_serving.rs`: split lookup, response DTOs,
  streaming, route handlers, and tests if future behavior makes the current
  module hard to hold in one review.
- `converted.rs`: move scriptlet-publication summary validation and chunk
  reachability into a publication-focused model helper module if schema policy
  grows further.

### Refactor Rules

Each refactor child plan must state:

- which behavior moves;
- which module owns it afterward;
- whether persisted state or public API shape changes;
- the focused test that proves behavior stayed the same or changed
  intentionally;
- which docs or subsystem maps need routing updates.

## Cross-Slice Testing Strategy

Use focused tests for each child plan, then widen when a slice crosses Remi,
generation, or install behavior.

Common focused proof:

```bash
cargo test -p conary-core golden_fixtures
cargo test -p conary-core support_matrix
cargo test -p conary --test conversion_integration golden_conversion
cargo test -p remi publication
```

Generation file capability propagation needs:

```bash
cargo test -p conary-core generation::builder
cargo test -p conary-core generation::export
```

Remi serving behavior changes need:

```bash
cargo test -p remi
```

Full closeout for broad policy changes should include:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

## First Slice Recommendation

Start with file capability public policy hardening because it reduces public
security exposure without requiring the generation xattr machinery yet.

The first plan should:

1. Add a public-ready file capability allowlist, initially
   `cap_net_bind_service`.
2. Keep the manifest's known Linux capability table for syntax and install-time
   validity.
3. Route high-risk known capabilities to private-review or blocked publication
   status.
4. Update golden fixtures and support-matrix expectations.
5. Tighten publication summary shape validation for `security_policy_intents`.
6. Update CCS/Remi/scriptlet security docs and audit metadata.

The second slice should handle generation-aware `security.capability` xattr
propagation. It should start only after the public policy boundary is explicit.

## Non-Goals

- No broad public gate bypass for blocked or review-required rows.
- No live package-manager recursion or live network fetch support.
- No generation-aware file capability acceptance until xattr propagation is
  proven.
- No AppArmor mode, disable, status, or broad reload promotion in the first
  file-capability slice.
- No PAM, kernel, initramfs, or bootloader promotion without separate native
  authority designs and target-profile facts.
- No unrelated meta-layer cleanup. Ledger, ownership-card, gate, and tooling
  changes happen only when this roadmap's slices touch them or when verification
  fails.
