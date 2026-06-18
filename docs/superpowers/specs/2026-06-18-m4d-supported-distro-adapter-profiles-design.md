# M4d Supported Distro Adapter Profiles Design

**Date:** 2026-06-18
**Status:** Locked design after DeepSeek, Gemini, Opus, and local agentic
review. Implementation plan:
`docs/superpowers/plans/2026-06-18-m4d-supported-distro-adapter-profiles-implementation-plan.md`.
**Parent umbrella:** `docs/superpowers/specs/2026-06-17-m4-ccs-native-ecosystem-design.md`
**Prerequisites:** M4a CCS v2 native package contract, M4b native authoring
workflow, and M4c Remi native CCS publication are implemented and merged.
**Scope:** M4d only: compile-time embedded supported-target profile data,
typed profile lookup APIs, profile-backed lifecycle validation, and fixture
proof for Fedora 44, Ubuntu 26.04, and Arch.

## Purpose

M4d answers the distro-adaptation gap in the M4 ecosystem. Conary already keeps
public user support intentionally narrow, but the facts that make a target
usable are still spread across Rust matches, CLI help, Remi route validation,
repository lookup patterns, source-selection policy, legacy replay target
normalization, and CCS v2 lifecycle placeholders.

M4d creates one supported-target profile surface for the three public targets:

- `fedora-44`
- `ubuntu-26.04`
- `arch`

The goal is not to add more distros. The goal is to make the current support
set data-owned, explicit, fixture-tested, and easy to audit. A future distro
addition should look like adding a reviewed profile plus fixtures and docs,
not rediscovering scattered hard-coded policy.

## Core Decision

M4d is a hard cutover to supported-target profiles, not a compatibility
migration. There is only one checked-in supported-target profile catalog after
the slice lands. Duplicate catalogs and stale helper paths should be deleted
or replaced in the same implementation slice that touches them.

The implementation should not keep a transitional parallel world where
`data/distros.toml`, `repository/distro.rs` constants, Remi's
`SUPPORTED_DISTROS`, and CLI examples all claim to be sources of truth.

The profile catalog owns:

- public supported IDs and display names;
- internal family slugs;
- Remi route slugs;
- package parser/backend format;
- dependency flavor and version scheme;
- public-profile replay target format/distro/release facts;
- repository matching hints used by Remi conversion and sync;
- lifecycle capability facts for CCS v2 target-profile validation;
- fixture expectations for positive and negative supported-target proof.

The profile catalog does not own non-public legacy replay normalization.
Public profile IDs map to replay targets through profile data. Generic family
pins such as `fedora`, unsupported derivative pins such as `debian-13`, and
other old normalization branches should be deleted and fail closed unless the
implementation plan proves a current fixture requires a short-lived private
compatibility shim. Any retained shim must live outside the public profile
catalog and must never appear in `conary distro list`, Remi help, route
validation, or supported-target docs.

Parser backends remain Rust code. RPM, DEB, and Arch package parsing,
repository metadata parsers, and genuinely family-specific behavior are code.
Profiles select and constrain those backends; they do not replace them.
`package_format = "rpm"` maps to the RPM parser, `package_format = "deb"` maps
to the DEB parser, and `package_format = "arch"` maps to the Arch parser. A
caller may still dispatch through a looked-up profile route slug, but the
selected profile is the source of truth for the parser family.

Profile string domains should be explicit to avoid DEB-family drift:

- `package_format`: `rpm`, `deb`, or `arch`;
- `dependency_flavor`: `rpm`, `deb`, or `arch`;
- `version_scheme`: `rpm`, `debian`, or `arch`;
- `replay_target.format`: `rpm`, `deb`, or `arch`.

## Current Repo Facts

The repo already has partial centralization, but it is not a complete adapter
surface:

- `data/distros.toml` lists `fedora-44`, `ubuntu-26.04`, and `arch`, but only
  with display, format, release, and EOL fields.
- `crates/conary-core/src/repository/distro.rs` hard-codes
  `SUPPORTED_USER_DISTROS`, `SUPPORTED_USER_DISTRO_CATALOG`, internal family
  labels, flavor mapping, version-scheme mapping, and replay target
  normalization.
- `apps/remi/src/server/handlers/mod.rs` has a separate internal
  `SUPPORTED_DISTROS` list for route slugs: `arch`, `fedora`, and `ubuntu`.
- `apps/remi/src/server/conversion/lookup.rs` maps distro flavor to repository
  name patterns such as `fedora%`, `ubuntu%`, and `arch%`.
- `apps/remi/src/server/conversion/metadata.rs` maps route slugs to parser
  backends by matching `arch`, `fedora`, `ubuntu`, and `debian`.
- `crates/conary-core/src/repository/sync/remi.rs` infers version scheme from
  the Remi route distro string and defaults to RPM when it cannot infer one.
- `crates/conary-core/src/ccs/v2/validation.rs` already has the M4d-shaped
  `TargetProfileQuery` hook, but the default M4a implementation rejects
  lifecycle services, tmpfiles, and sysctl because real target facts do not
  exist yet.
- `apps/conary/src/commands/distro.rs` correctly lists only the supported
  public targets, but that behavior depends on the current hard-coded catalog.

These facts support the design direction: build one profile module and make
callers ask it for target facts instead of re-encoding string policy locally.

## Non-Goals

- Do not add public Debian, Linux Mint, Ubuntu noble, Fedora next, or generic
  derivative profiles.
- Do not make supported profiles user-editable or plugin-provided in M4d.
- Do not make DEB parser support imply Debian public support.
- Do not make every package parser or repository backend declarative.
- Do not move host I/O into core validation, planners, or profile lookups.
- Do not allow profiles to bypass CCS v2 package contract validation.
- Do not keep duplicate public-distro catalogs for compatibility with old
  call sites.
- Do not model package-manager execution authority boundaries beyond the
  dependency flavor, version scheme, parser, and repository-selection facts
  needed by current M4d callers.
- Do not solve the M4e proof corpus in this slice.

## Profile Catalog

The implementation should introduce a checked-in profile file embedded by
`conary-core`, such as:

```text
crates/conary-core/src/repository/supported_profiles/catalog.toml
```

This file should replace the thin `data/distros.toml` public-support role. The
implementation plan can either delete `data/distros.toml` or turn it into an
alias generated from the new catalog only if there is a concrete current
consumer that would otherwise require unrelated churn. The preferred end state
is one human-auditable profile file.

The catalog is not a runtime configuration file. `conary-core` should embed it
with `include_str!`, parse it through a typed schema, and expose profile facts
from memory. Missing or malformed embedded profile data is a test/startup
failure. Core validators and profile lookups must not read files from the
developer host or deployment filesystem.

Recommended profile shape:

```toml
[[profiles]]
id = "fedora-44"
display_name = "Fedora 44"
release = "44"
release_date = "2026-04-28"
eol = "2027-06-02"

[profiles.identity]
family_slug = "fedora"
remi_route_slug = "fedora"
package_format = "rpm"
dependency_flavor = "rpm"
version_scheme = "rpm"

[profiles.replay_target]
format = "rpm"
distro = "fedora"
release = "44"

[profiles.repository]
name_patterns = ["fedora%"]

[profiles.lifecycle]
service_manager = "systemd"
default_shell = "/bin/sh"
path_dirs = ["/usr/bin", "/bin"]

[profiles.lifecycle.services]
mode = "allow-list"
entries = ["example.service"]

[profiles.lifecycle.tmpfiles]
mode = "allow-list"
entries = ["example.conf"]

[profiles.lifecycle.sysctl]
mode = "allow-list"
keys = ["kernel.example"]

[profiles.lifecycle.users]
mode = "unsupported"

[profiles.lifecycle.groups]
mode = "unsupported"

[profiles.lifecycle.directories]
mode = "unsupported"

[profiles.lifecycle.alternatives]
mode = "unsupported"
```

The exact field names can be finalized in the implementation plan, but the
semantics should remain boring and explicit:

- no inheritance;
- no wildcard derivative support;
- no "Ubuntu family means Debian";
- no profile entry without matching fixture proof;
- no profile entry that expands public support beyond the three approved IDs;
- no category-level lifecycle boolean that accepts every entry by accident;
- no public-facing support-status taxonomy unless a later design adds one.

## Core API

The profile API should live in `conary-core`, preferably under a dedicated
module such as:

```text
crates/conary-core/src/repository/supported_profiles/
```

The old `repository/distro.rs` module should be deleted or reduced only if the
implementation plan proves a small re-export is cleaner than touching every
caller. The desired architecture is a profile-owned API, not a renamed copy of
the current hard-coded functions.

Recommended API capabilities:

- list public supported profiles in stable display order;
- look up a profile by public ID;
- look up a profile by internal family slug;
- look up route metadata and matching public profile IDs by Remi route slug;
- return dependency flavor and version scheme for a profile;
- map public ID plus architecture to a public-profile replay target;
- expose repository matching hints for Remi conversion lookup;
- expose parser/backend format for conversion metadata parsing;
- expose lifecycle capabilities through an extended concrete implementation of
  `TargetProfileQuery`;
- produce short supported-target lists for CLI and Remi help text.

The API should make the public/internal split explicit. Public IDs are what
users see and configure. Internal route slugs are how Remi and parser backends
address families. A route slug is not proof that a public target exists. In
the initial M4d catalog, every route slug maps to exactly one public profile,
but the API should not bake in a one-profile-per-route assumption for future
versioned families.

M4d should name and preserve the ownership boundary it changes in
`crates/conary-core/src/ccs/v2/validation.rs`: the existing M4a
`TargetProfileQuery` hook covers only services, tmpfiles, and sysctl. M4d
extends that query and `validate_authority_with_profile` to cover every
currently signed lifecycle vector in `LifecycleAuthorityV2`:
users, groups, directories, services, tmpfiles, sysctl, and alternatives.
The default no-profile implementation remains fail-closed for every category.

The profile engine belongs under `repository/supported_profiles`. Large files
and orchestrators such as `crates/conary-core/src/ccs/manifest.rs` and
`crates/conary-core/src/recipe/kitchen/cook.rs` should delegate to the profile
API instead of embedding new distro/profile policy.

## Caller Cutover

M4d should convert callers directly to the profile API and remove obsolete
local policy where it is touched:

- `conary distro list` uses the profile catalog for public IDs, display names,
  and configured-repository status.
- `conary distro set` validates the configured pin against profile public IDs
  and fails closed for unsupported IDs instead of storing arbitrary strings.
- install, update, resolver, source-selection, and effective-policy helpers
  use profile-derived dependency flavor and version scheme.
- legacy replay policy maps validated public distro pins to replay targets
  through profile data.
- Remi public and admin route validation uses profile route slugs instead of a
  local `SUPPORTED_DISTROS` array.
- Remi native publication validates release upload route slugs through the
  same profile API.
- Remi conversion lookup uses profile repository matching hints instead of
  local `fedora%` / `ubuntu%` / `arch%` matches.
- Remi package parsing chooses parser backend from the selected profile route
  slug.
- Remi sync and client-side repository code derive version scheme from the
  route/profile rather than defaulting unknown values to RPM.
- CCS v2 validation uses concrete target profile facts to validate lifecycle
  declarations through `TargetProfileQuery`.

Remi keeps the existing public route slugs `fedora`, `ubuntu`, and `arch`.
M4d does not design a route-ID transition. Route/profile agreement becomes a
fixture-backed invariant: every public Remi route slug must map to at least one
public profile, and no unsupported public ID can appear because a route slug
exists.

The Remi cutover must inventory every route with a `{distro}` path parameter,
not only the current call sites that already use `SUPPORTED_DISTROS`. Profile
route validation should happen before database queries, filesystem/cache path
creation, key-path access, or release-upload trust decisions. The route set to
cover includes:

- `/v1/{distro}/metadata` and `/v1/{distro}/metadata.sig`;
- `/v1/{distro}/packages/{name}`, `/download`, and `/delta`;
- sparse index routes `/v1/index/{distro}` and
  `/v1/index/{distro}/{name}`;
- detail routes `/v1/packages/{distro}/{name}`, `/versions`,
  `/dependencies`, and `/rdepends`;
- TUF routes `/v1/{distro}/tuf/timestamp.json`, `/snapshot.json`,
  `/targets.json`, `/root.json`, and `/{version}`;
- admin upload/review routes `/v1/admin/packages/{distro}`,
  `/v1/admin/releases/{distro}`, and
  `/v1/admin/packages/{distro}/{package}/scriptlet-review`;
- admin TUF refresh route `/v1/admin/tuf/{distro}/refresh-timestamp`.

Callers that currently parse arbitrary repository metadata may keep format
detection for parser capability. That detection must not add a supported public
target or make unsupported IDs appear in `conary distro list`, CLI help, Remi
help, or public docs.

## Lifecycle Validation

M4a intentionally left lifecycle validation behind `TargetProfileQuery`. M4d
fills that gap for supported targets.

The profile-backed validator should accept only lifecycle declarations that
match an explicit per-entry profile policy. Unsupported declarations produce
CCS v2 `LifecycleUnsupported` diagnostics with actionable field paths and fix
text.

Initial M4d lifecycle facts should cover at least:

- services;
- tmpfiles;
- sysusers/users/groups;
- sysctl;
- alternatives;
- directories;
- declared default shell/path facts.

Coverage means every category has an explicit profile policy and validation
path, not that every category is accepted for every target. If implementation
discovers that a lifecycle category is represented in v2 authority but has no
meaningful fixture-backed target behavior yet, the profile must mark that
category unsupported and tests must prove the diagnostic. M4d should not
silently accept lifecycle authority because all three initial targets happen to
be Linux systems.

The lifecycle policy shape should be per entry, not a single
`supports_services = true`-style category boolean. Exact allow lists, narrowly
reviewed patterns, or explicit `unsupported` modes are acceptable; broad
accept-any behavior is not. Declared shell and path facts are target facts for
future lifecycle execution or diagnostics. They are not validation enforcement
unless the implementation names a concrete M4d consumer and tests it.

Core validation remains host-I/O-free. Profiles carry declared target facts
and fixture evidence. They do not probe `/etc`, `systemctl`, package-manager
state, or the developer host.

## Failure Behavior

M4d fails closed:

- Unknown public IDs such as `debian`, `linux-mint`, `ubuntu-noble`, and
  `fedora-45` are unsupported unless they appear as explicit checked-in
  profiles in a future reviewed slice.
- Internal-only slugs such as `fedora` and `ubuntu` are route/backend
  identifiers, not public support IDs. `arch` is public only because it is an
  explicit profile ID; a route slug by itself is not proof of public support.
- DEB support remains available through the Ubuntu profile, but it does not
  imply public Debian support.
- Repository format detection may infer RPM, DEB, or Arch parser capability
  for a repository row, but that inference cannot broaden the supported public
  target catalog.
- Missing or malformed embedded profile data is a startup/test failure, not a
  runtime fallback to hard-coded defaults.
- Unknown version schemes or dependency flavors fail with target-profile
  diagnostics at profile lookup, Remi sync, and new route/profile derivation
  sites instead of defaulting to RPM.
- Existing resolver and automation reads of stored repository rows with
  missing `version_scheme` may keep their current RPM default until a separate
  data reconciliation slice changes persisted-row semantics.
- Remi unsupported route slugs return the existing unsupported-distro class of
  error, but the allowed route slugs come from profiles.
- Remi native release upload validation remains fail-closed: replacing a local
  route constant with the profile API must not weaken static publish trust,
  accepted-signer checks, or recorded-draft refusal.

## Testing Strategy

M4d tests should prove that the profile catalog is the source of truth.

Profile catalog tests:

- embedded catalog parsing succeeds without runtime file I/O;
- exactly three public IDs exist;
- display names, release facts, route slugs, dependency flavors, version
  schemes, parser formats, and replay target mappings match fixtures;
- unsupported IDs such as `debian`, `linux-mint`, `ubuntu-noble`, and
  `fedora-45` fail public lookup;
- internal slugs and route slugs do not appear as public IDs unless they are
  also explicit public IDs.

Caller cutover tests:

- `conary distro list` renders only profile-backed public targets;
- `conary distro set` rejects unsupported public IDs and internal-only route
  slugs;
- install/source-policy and update/source-selection use profile-derived flavor
  and version scheme;
- public-profile replay target mapping comes from profile data;
- old generic or unsupported legacy replay normalizations are either deleted
  with intentional test updates or quarantined in a non-public compatibility
  helper with leak-guard coverage;
- Remi route validation and native release upload validation use route slugs
  from profiles;
- Remi route/profile agreement proves `fedora`, `ubuntu`, and `arch` route
  slugs map to profile-backed support without exposing them as public IDs;
- unsupported route-slug tests cover at least sparse index, TUF metadata, and
  admin package upload paths so non-profile routes cannot query DB state,
  create cache/key paths, or reach release-upload trust checks;
- Remi conversion lookup uses profile repository hints;
- Remi sync preserves version scheme from profiles without unknown-default RPM
  behavior at route/profile derivation sites.

Lifecycle tests:

- profile-backed `TargetProfileQuery` covers users, groups, directories,
  services, tmpfiles, sysctl, and alternatives;
- supported lifecycle entries are accepted only by explicit allow-list or
  reviewed-pattern policy;
- unsupported lifecycle declarations fail with `LifecycleUnsupported` for each
  category;
- validation does not require live host files or commands.

Fixture tests:

- each supported target has a fixture row proving public ID, internal slug,
  route slug, parser/backend, replay target, lifecycle capabilities, and
  docs/help examples;
- negative fixtures prove derivative-like or future-looking IDs do not leak
  into public support.

## Documentation And Audit

The implementation plan should update docs where public target claims or "look
here first" routing changes:

- `docs/modules/source-selection.md`
- `docs/modules/remi.md`
- `docs/modules/ccs.md`
- `docs/modules/test-fixtures.md`
- `docs/modules/feature-ownership.md`
- `docs/llms/subsystem-map.md`
- `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- feature coherency ledger and wave-scope rows if public behavior claims move

The design doc itself should be audited as a planning document. The final
implementation docs should make the public/internal boundary hard to miss:
Fedora 44, Ubuntu 26.04, and Arch are supported public targets; `fedora` and
`ubuntu` route slugs are internal/public-route family identifiers; `arch` is
both a route slug and a public ID only because it is explicitly profiled; DEB
parser support is a format capability, not Debian public support.

## Verification Guidance

The implementation plan should include focused and broad proof:

```bash
cargo test -p conary-core supported_profile
cargo test -p conary-core ccs::v2
cargo test -p conary --lib commands::distro
cargo test -p conary --lib commands::install::source_policy
cargo test -p conary --test packaging_m2a
cargo test -p conary --lib commands::publish
cargo test -p conary-core repository::static_repo::publish_gate
cargo test -p remi route
cargo test -p remi conversion
cargo test -p remi release_upload_
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv
bash scripts/check-doc-truth.sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

The exact focused test names may change during implementation planning, but the
proof must cover core profile data, caller cutover, Remi route/native
publication behavior, M2 publish-gate regression behavior, CCS v2 lifecycle
validation, docs truth, and workspace linting.

## Plan Questions

- What are the final TOML field names and typed Rust structs for the embedded
  catalog?
- Which exact lifecycle entries or narrowly reviewed patterns are accepted in
  the first M4d pass for each target, and which remain explicit unsupported
  diagnostics?
- Should the old `repository/distro.rs` path be deleted outright, or kept as a
  short-lived module alias if that reduces implementation churn without
  preserving duplicate logic?
