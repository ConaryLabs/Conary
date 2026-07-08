# M4e Lifecycle Authoring And Native Proof Corpus Design

**Date:** 2026-06-18
**Status:** Implemented in the M4e lifecycle authoring proof changeset after a
2026-07-03 repo rebaseline. The original design was locked on 2026-06-18 after
DeepSeek, Gemini, and local agentic review; the refresh baseline below records
the pre-implementation state that this branch closes.
**Parent umbrella:** `docs/superpowers/specs/archive/2026-06-17-m4-ccs-native-ecosystem-design.md`
**Prerequisites:** M4a CCS v2 native package contract, M4b native authoring
workflow, M4c Remi native CCS publication, and M4d supported target profiles
are implemented and merged.
**Scope:** M4e only: lifecycle-aware native authoring, profile-backed v2
authority projection, representative proof corpus, M4 exit criteria, and the
post-M4 CCS authoring completion backlog.

## Purpose

M4e closes the M4 CCS-native ecosystem milestone. Earlier M4 slices made v2
authority real, gave maintainers a first minimal authoring loop, made Remi
publish native CCS packages, and centralized supported target facts. M4e turns
those pieces into a realistic maintainer workflow and a corpus that proves the
whole path together.

M4e is intentionally larger than the original proof-corpus-only closeout. It
adds the deferred lifecycle authoring behavior needed for real packages:
configuration files, systemd-style services, users, groups, directories,
tmpfiles, sysctl declarations, and alternatives. The lifecycle work remains
declarative, signed, and profile-validated. M4e does not activate services,
create users, write sysctl state, or mutate a live host.

## 2026-07-03 Pre-Implementation Refresh Baseline

This historical rebaseline captured the main-branch state before M4e was
implemented. A current-checkout rebaseline on 2026-07-03 found:

- `apps/conary/tests/packaging_m4e.rs` is absent, and
  `cargo test -p conary --test packaging_m4e` fails because no such test target
  exists.
- `apps/conary/src/commands/ccs/init_template.rs` still exposes only
  `MinimalFile`; `config-noreplace` and `service` templates are not present.
- `apps/conary/src/cli/ccs.rs` has no `--target-profile` option for
  `ccs build`, `ccs lint`, or `ccs test`.
- `crates/conary-core/src/ccs/v2/authoring.rs` still emits
  `ProfileDeferred` findings for lifecycle declarations.
- `crates/conary-core/src/repository/supported_profiles/catalog.toml` still
  carries M4d placeholder lifecycle entries such as `example.service`,
  `example.conf`, and `kernel.example`; user, group, directory, and alternatives
  lifecycle categories remain unsupported.
- `crates/conary-core/src/ccs/v2/debug_projection.rs` is not present. Debug
  TOML drift checks currently live in the v2 reader surface.
- Remi release upload validates supported route slugs and the static publish
  gate, but it does not yet validate lifecycle authority against a route-derived
  supported profile.

The same rebaseline confirmed the prerequisite slices were healthy:
`packaging_m4a`, `packaging_m4b`, `packaging_m4c`, `packaging_m4d`,
`conary-core ccs::v2`, `conary-core supported_profiles`, and
`remi release_upload_` all passed.

## 2026-07-03 Implementation Proof

The M4e changeset adds the `config-noreplace` and `service` templates,
target-profile-aware lifecycle authoring, config and lifecycle signed authority
projection, debug TOML projection checks, exact supported-profile lifecycle
allow-lists, Remi route-derived lifecycle validation, and
`apps/conary/tests/packaging_m4e.rs`.

Focused implementation proof:

```bash
cargo test -p conary-core ccs::v2
cargo test -p conary-core supported_profiles
cargo test -p conary --test packaging_m4e
cargo test -p conary --test packaging_m4a --test packaging_m4b --test packaging_m4c --test packaging_m4d
cargo test -p remi native_publish
cargo test -p remi release_upload_
```

## Core Decision

M4e is both the lifecycle-authoring slice and the proof-corpus slice.

The first M4b authoring path rejected lifecycle declarations as
profile-deferred. M4d then introduced profile-backed lifecycle validation.
M4e should convert that state from "deferred" to "validated": a
lifecycle-bearing native package may build when it selects an exact supported
target profile and every declared lifecycle entry is accepted by that profile.

The milestone closeout should therefore prove:

- a maintainer can author minimal, config, and service-style native packages;
- lifecycle declarations become signed v2 authority;
- supported target profiles accept only explicit proof-backed entries;
- Remi can publish and serve a representative lifecycle-bearing native package;
- negative fixtures fail clearly when authority, target profile, trust, or
  lifecycle support is missing.

## Scope

M4e includes:

- `config-noreplace` authoring template and projection into signed v2 config
  authority.
- Declarative lifecycle authoring for services, tmpfiles, users, groups,
  directories, sysctl, and alternatives.
- An explicit `--target-profile <public-id>` authoring validation path for
  lifecycle-bearing v2 build, lint, and dry-run test flows.
- A v2 reader/validation split so archive reads perform structural and trust
  checks without rejecting lifecycle authority solely because no caller target
  profile is available yet.
- Profile-backed lifecycle allow lists that replace the M4d placeholder
  `example.*` entries with proof-corpus entries.
- A representative native proof corpus with positive and negative package
  cases.
- Local Remi publish/fetch/install-dry-run proof for at least one
  lifecycle-bearing native package.
- Supported docs examples for config, service, Remi publication, and target
  profile validation.
- A tracked M4 exit checklist and post-M4 CCS authoring backlog.

M4e excludes:

- Live service activation, user/group creation, tmpfiles application, sysctl
  writes, alternatives registration, or other live host mutation.
- Arbitrary script lifecycle authoring as native v2 authority.
- New public distro targets beyond `fedora-44`, `ubuntu-26.04`, and `arch`.
- Runtime-loaded or user-editable supported target profiles.
- Full dependency authoring and resolver integration for v2 dependencies.
- Complete production key-management UX.
- Making every lifecycle category positive-supported if the corpus has no
  meaningful representative package for it.

## Ownership

CLI authoring and templates live in `apps/conary/src/commands/ccs/`:

- `init_template.rs` owns selectable template names.
- `templates.rs` owns generated `ccs.toml` templates.
- `init.rs` owns writing generated template files when a template needs a
  buildable example project, not just a manifest.
- `lint.rs`, `build.rs`, and `test.rs` own CLI diagnostics, target-profile
  arguments, and command flow.
- `local_dev.rs` remains the local-dev signing/trust helper.

Core v2 authoring lives in `crates/conary-core/src/ccs/v2/authoring.rs`.
It maps `CcsManifest` plus `BuildResult` into signed v2 package authority.
M4e should keep CLI text and template ergonomics outside the core projection
layer, while moving authority mapping and diagnostic classification into core
helpers.

The v2 contract and validation layer remains under
`crates/conary-core/src/ccs/v2/`. M4e may extend validation helpers, but it
must not move host I/O into v2 validation.

M4e must preserve the repo's large-file boundaries:

- `crates/conary-core/src/recipe/kitchen/cook.rs` is outside this slice and
  should remain untouched.
- `crates/conary-core/src/ccs/manifest.rs` remains the TOML model/parser. Do
  not add projection or validation helper logic there; keep those helpers under
  `ccs/v2/authoring.rs`, `ccs/v2/validation.rs`, or supported-profile modules.

Supported lifecycle facts live in
`crates/conary-core/src/repository/supported_profiles/`. M4e updates the
embedded catalog and lifecycle matcher so proof-corpus entries are accepted
only for exact public profiles.

Proof lives in focused tests:

- `apps/conary/tests/packaging_m4e.rs` for end-to-end CLI authoring corpus
  flows.
- Core `ccs::v2` tests for projection and validation details.
- Remi tests only where native publish/fetch/install behavior matters.

Docs and audit updates should touch `docs/modules/ccs.md`,
`docs/modules/test-fixtures.md`, `docs/modules/feature-ownership.md`,
`docs/modules/remi.md`, `docs/llms/subsystem-map.md`, and the docs/coherency
ledgers when implementation changes public claims or "look here first" paths.

## Authoring Workflow

The intended authoring flow is:

```bash
conary ccs init --template config-noreplace
conary ccs lint
conary ccs build --format v2 --local-dev
conary ccs test package.ccs --dry-run
```

and:

```bash
conary ccs init --template service
conary ccs lint --target-profile fedora-44
conary ccs build --format v2 --local-dev --target-profile fedora-44
conary ccs test package.ccs --dry-run --target-profile fedora-44
```

Minimal and config-only packages without lifecycle authority may keep the
current simple path. Config paths and config policy are contract-validated
during projection and build. Any package with lifecycle authority must be
validated against an explicit public target profile before v2 build writes a
package.

The `--target-profile` value is a public supported profile ID. It accepts
`fedora-44`, `ubuntu-26.04`, or `arch`. It does not accept Remi route slugs
such as `fedora` or `ubuntu`, and it does not accept unsupported future-looking
IDs such as `debian`, `linux-mint`, `fedora-45`, or `ubuntu-noble`.

The `--target-profile` option does not exist yet in the M4d implementation.
M4e must add the CLI plumbing explicitly: `apps/conary/src/cli/ccs.rs`, build
options, lint command flow, build command flow, and dry-run test command flow
all need to accept and pass the selected public profile into core validation.

## Templates

`config-noreplace` and `service` are new M4e template variants, not existing
wiring. M4e must add `CcsInitTemplate` variants and matching template builders.
For generated examples that need a buildable package, `ccs init` should also
write the small source files beside `ccs.toml`; the existing `minimal-file`
template may remain manifest-only.

### `minimal-file`

The existing M4b template remains the smallest positive path. M4e should keep
it as a regression fixture and should not force target-profile selection when
the package has no config or lifecycle authority.

### `config-noreplace`

The config template creates:

- one file under `/etc/conary-example/config.toml`;
- an explicit `config` component mapping for that file;
- `[config] files = ["/etc/conary-example/config.toml"]`;
- `noreplace = true`;
- no lifecycle declarations.

Projection maps the config file into signed v2 authority in two places:

- the matching `FileAuthorityV2.config` is `Some(ConfigPolicyV2::NoReplace)`
  when `noreplace = true`;
- `PackageDataV2.config` contains a `ConfigAuthorityV2` entry for the path and
  the same policy.

If a manifest later opts into `[config] files` with `noreplace = false`, M4e
should either map that policy to `ConfigPolicyV2::Replace` with focused tests or
reject it clearly until a post-M4 config-conflict slice designs replacement
semantics. `ConfigPolicyV2::Merge` remains out of scope for M4e.

Build must fail if a manifest names a config path that is not present in the
build output or if the path is not absolute.

### `service`

The service template creates a tiny service-style package with:

- a regular payload file, such as `/usr/bin/conary-example`;
- a systemd service unit path, such as
  `/usr/lib/systemd/system/conary-example.service`;
- declarative service authority for `conary-example.service`;
- a system user `conary-example`;
- a system group `conary-example`;
- a state directory `/var/lib/conary-example`;
- a tmpfiles entry whose authority string is the state directory path
  `/var/lib/conary-example`.

The template should generate the tiny payload files needed for the example to
build, including the executable and unit file. It should not rely on the user
manually creating those files before the positive corpus can pass.

The template must not generate `post_install` or `pre_remove` script hooks.
It may include advisory comments explaining that M4e validates authority and
dry-run behavior only; it does not activate the service on the host.

### Optional Lifecycle Examples

M4e can add explicit sysctl and alternatives authoring examples only if the
implementation plan names a meaningful package reason and a positive corpus
fixture for each one. Otherwise, sysctl and alternatives remain supported as
manifest categories with negative proof: unsupported entries fail with
`LifecycleUnsupported` instead of silently passing.

## Projection And Validation

M4e must add projection from existing `CcsManifest` declarative hooks into
`LifecycleAuthorityV2` in `crates/conary-core/src/ccs/v2/authoring.rs`. Current
M4b projection leaves lifecycle authority empty, so this is new projection
work, not just CLI wiring.

- each `hooks.services` entry appends `Service.name` to
  `LifecycleAuthorityV2.services`;
- each `hooks.systemd` entry appends `SystemdHook.unit` to
  `LifecycleAuthorityV2.services`;
- each `hooks.tmpfiles` entry appends `TmpfilesHook.path` to
  `LifecycleAuthorityV2.tmpfiles`;
- each `hooks.sysctl` entry appends `SysctlHook.key` to
  `LifecycleAuthorityV2.sysctl`;
- each `hooks.users` entry appends `UserHook.name` to
  `LifecycleAuthorityV2.users`;
- each `hooks.groups` entry appends `GroupHook.name` to
  `LifecycleAuthorityV2.groups`;
- each `hooks.directories` entry appends `DirectoryHook.path` to
  `LifecycleAuthorityV2.directories`;
- each `hooks.alternatives` entry appends `AlternativeHook.name` to
  `LifecycleAuthorityV2.alternatives`.

The first implementation may use exact string references because
`LifecycleAuthorityV2` currently stores vectors of strings. The design does
not require a schema expansion to structured lifecycle entries before M4e.
If implementation discovers exact strings are too ambiguous for a positive
fixture, the plan must either add a focused v2 schema extension with tests or
keep that category negative-only.

Exact string projection does not claim full action semantics for service
enable/start, tmpfiles type, sysctl value, or alternatives priority. Those
fields stay in the debug TOML and become execution semantics only in a later
live lifecycle slice. M4e signs and validates the accepted authority references.

Lint should classify findings into stable buckets:

- **Contract:** missing v2 authority such as release, kind, invalid config
  paths, or config paths not present in build output.
- **Profile:** lifecycle declarations that require a target profile or are
  unsupported by the selected target profile.
- **Publication readiness:** local-dev or host-hardened artifacts that cannot
  pass release publication.
- **Style:** non-blocking suggestions.

Build should fail before writing a v2 package when blocking findings exist.
Local dry-run test should validate signed v2 authority and target-profile
constraints, but must not perform live lifecycle execution.

M4e must replace the current `ProfileDeferred` behavior instead of leaving it
orphaned. Lifecycle declarations that need a profile and declarations
unsupported by the selected profile should use the `Profile` diagnostic bucket
or an intentionally renamed equivalent with tests proving the old deferred
state no longer blocks supported lifecycle packages. Config-only errors remain
contract diagnostics unless a later profile-policy design adds profile-specific
config semantics.

Archive reads need a separate validation path. `read_authority_document` should
decode the v2 authority, verify the exact signed bytes, verify debug TOML and
attestation hashes, and run common structural validation. It must not reject a
lifecycle-bearing package solely through no-profile lifecycle facts. Callers
that know a target profile, such as `ccs build`, `ccs lint`, `ccs test`, Remi
native publication, and future install flows, then run
`validate_authority_with_profile` using the selected or route-derived profile.

M4e must also replace the current debug-TOML install-affecting-field rejection.
`MANIFEST.toml` remains debug material, but M4e config and lifecycle templates
will intentionally include `[config]` and `[hooks]` there. Archive reads should
verify the debug TOML hash, keep signed CBOR authority canonical, and reject
only when an M4e-owned debug config or lifecycle declaration is not represented
by the signed authority projection. Reader tests should cover config and
lifecycle debug TOML that reads structurally, tampered TOML hash rejection, and
missing signed projection rejection.

`ccs test --dry-run` for lifecycle-bearing packages should continue to prove an
isolated non-mutating path. It may keep the current no-sandbox dry-run shape for
declarative-only lifecycle authority if tests prove no lifecycle executor path
runs. If implementation needs any lifecycle execution path for validation,
switch that flow to `SandboxMode::Always` and keep it isolated.

## Supported Profile Policy

M4e replaces M4d placeholder lifecycle entries with proof-corpus entries in
`crates/conary-core/src/repository/supported_profiles/catalog.toml`. The M4d
placeholders are not durable support claims.

The positive allow list should be exact and small:

- service: `conary-example.service`;
- tmpfiles path: `/var/lib/conary-example`;
- user: `conary-example`;
- group: `conary-example`;
- directory: `/var/lib/conary-example`;
- sysctl: only a named harmless key if a positive sysctl fixture exists;
- alternative: only a named alternative if a positive alternative fixture
  exists.

All three public profiles may share the same first corpus entries when the
entries are profile-appropriate. Fedora 44, Ubuntu 26.04, and Arch all use
systemd in the current profile facts, so the first service corpus can be
shared. If implementation finds a profile-specific difference, the catalog
should represent it explicitly instead of widening the allow list.

M4e should not replace allow lists with broad patterns such as "all `.service`
units" or "all `/var/lib/*` directories." Broader lifecycle policy can be a
post-M4 task after the proof corpus demonstrates why it is safe.

Profile catalog changes are category-specific:

- services and tmpfiles replace their M4d placeholder entries with the
  proof-corpus service unit and tmpfiles path;
- users, groups, and directories change from `unsupported` to `allow-list` only
  for the exact proof-corpus entries;
- sysctl and alternatives remain `unsupported` unless the implementation plan
  adds meaningful positive fixtures, in which case their placeholders are
  replaced with exact proof-corpus facts.

Failure behavior:

- missing `--target-profile` for lifecycle-bearing v2 build/test fails before
  package write or lifecycle dry-run;
- unsupported public profile IDs fail with target-profile diagnostics;
- Remi route slugs remain internal route slugs, not public authoring target
  profile IDs;
- unsupported lifecycle entries fail with `LifecycleUnsupported`;
- local-dev signing never bypasses profile validation;
- M4d's DEB parser support through Ubuntu does not imply Debian public target
  support.

## Proof Corpus

The corpus should be small and representative, not a package zoo.

Positive packages:

- **minimal-file:** proves the M4b happy path still works.
- **config-noreplace:** proves config template generation, config component
  assignment, v2 `NoReplace` authority, lint, build, verify, and dry-run test.
- **service package:** proves service/user/group/directory/tmpfiles authoring,
  target-profile validation, v2 projection, local dry-run test, and Remi
  publish/fetch/install-dry-run.
- **Remi-published native package:** may reuse the service package so M4c is
  proven with realistic v2 authority rather than only a hand-built toy
  artifact.
- **library/devel split:** include only if implementation can keep it modest.
  Otherwise record it in the M4 completion backlog rather than expanding the
  first M4e plan.

Negative fixtures:

- missing v2 release or kind;
- config path listed in `[config]` but absent from the payload;
- lifecycle-bearing build without `--target-profile`;
- unsupported target profile IDs: `debian`, `linux-mint`, `fedora-45`, and
  `ubuntu-noble`;
- Remi route slug used as public authoring target profile, such as `fedora`;
- unsupported lifecycle entry for every category that lacks positive support;
- lifecycle authority that validates for no supported profile;
- lifecycle authority that can be decoded and signature-checked structurally but
  fails explicit target-profile validation;
- debug TOML config or lifecycle declarations that are not represented by the
  signed v2 authority projection;
- local-dev artifact rejected by static publish and Remi release publish;
- Remi unsupported route validation still rejects unknown route slugs before
  storage, DB, key, or trust work.

Proof style:

- Prefer generated temporary projects in `apps/conary/tests/packaging_m4e.rs`
  for CLI flows.
- Use core unit tests for projection and validation details.
- Use Remi tests only where publication/fetch/install behavior matters.
- Keep live host mutation out of proof. M4e local tests stay dry-run and
  isolated.

## Remi Integration

M4e should prove that a representative lifecycle-bearing native package can be
published to local Remi, fetched, and installed in dry-run mode without
conversion semantics.

The Remi proof must preserve M4c invariants:

- no synthetic `converted_packages` row for native publication;
- native publication rows remain the source of truth for native artifacts;
- release upload reuses the shared M2/M4 publish gate;
- local-dev artifacts are refused for release publication;
- unsupported route slugs fail before storage, DB, key-path, or trust work.

Remi must not use no-profile v2 archive reads as a hidden lifecycle rejection
gate. Native publication should first perform structural/trust verification,
then apply the route-derived supported profile facts for lifecycle-bearing v2
authority.

M4e does not add new Remi route families or public distro routes.

## Documentation

M4e should graduate design-only examples into supported usage docs:

- creating a minimal native package;
- creating a config/noreplace native package;
- creating a service-style native package;
- validating against a supported target profile;
- publishing a native package to local Remi;
- reading the M4 closeout proof corpus.

Docs must keep the support boundary visible:

- public authoring target profiles are `fedora-44`, `ubuntu-26.04`, and
  `arch`;
- Remi route slugs `fedora`, `ubuntu`, and `arch` are route/backend slugs;
- `arch` is both a route slug and public ID only because it is explicitly
  profiled;
- DEB format support through Ubuntu does not mean Debian is a supported public
  target.

Docs and ledgers to update during implementation:

- `docs/modules/ccs.md`;
- `docs/modules/remi.md`;
- `docs/modules/test-fixtures.md`;
- `docs/modules/feature-ownership.md`;
- `docs/llms/subsystem-map.md`;
- `docs/superpowers/documentation-accuracy-audit-ledger.tsv`;
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv`;
- `docs/superpowers/feature-coherency-ledger.tsv`;
- `docs/superpowers/feature-coherency-wave-scopes.tsv`.

## M4 Exit Criteria

M4e closes M4 when all of these are true:

- CCS v2 package authority is the native package contract for new native
  packages.
- Maintainers can author, lint, build, verify, and dry-run-test minimal,
  config/noreplace, and service-style native packages.
- Lifecycle authority is declarative, signed, and profile-validated.
- Remi can publish, fetch, and install-dry-run a representative native package
  without conversion semantics.
- Supported target facts are centralized in profiles and proven by fixtures.
- Docs show the supported native authoring loop without implying broader distro
  support.
- M4a, M4b, M4c, and M4d focused gates still pass after the M4e corpus lands.
- M2 publish-gate regressions still pass.
- The post-M4 CCS authoring backlog is written down and scoped honestly.

## Post-M4 CCS Authoring Completion Backlog

The following items are not M4e failures. They are the backlog from "M4 native
ecosystem foundation is real and proven" to "CCS authoring is production
complete in every corner."

- Live lifecycle execution and rollback semantics.
- Real service activation policy and host-mutation UX.
- Config merge/runtime conflict behavior beyond signed authority.
- Richer dependency authoring and resolver integration for v2 dependencies.
- Library/devel split if it does not fit in the first M4e implementation plan.
- Positive alternatives authoring if kept negative-only in M4e.
- Positive sysctl authoring if kept negative-only in M4e.
- Key and trust management UX beyond local-dev and explicit key paths.
- Release-grade hermetic/native authoring, not only host-hardened local builds.
- Broader VM/integration proof across supported targets.
- Remaining v1 compatibility cleanup once fixtures no longer need it.
- Runtime profile policy evolution beyond exact proof-corpus allow lists.

The M4e implementation plan should preserve this backlog in docs so the next
umbrella can choose a concrete first post-M4 slice instead of rediscovering the
remaining work.

## Verification Guidance

The implementation plan should include these focused gates, adjusted as files
move during implementation:

```bash
cargo test -p conary-core ccs::v2
cargo test -p conary-core supported_profiles
cargo test -p conary --test packaging_m4a
cargo test -p conary --test packaging_m4b
cargo test -p conary --test packaging_m4c
cargo test -p conary --test packaging_m4d
cargo test -p conary --test packaging_m4e
cargo test -p remi release_upload_
cargo test -p remi route
```

Because M4e touches native package trust, Remi release behavior, and public
authoring claims, the plan must also include M2 publish-gate regressions:

```bash
cargo test -p conary --test packaging_m2a
cargo test -p conary --lib commands::publish
cargo test -p conary-core repository::static_repo::publish_gate
```

Docs and broad gates:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv
bash scripts/check-doc-truth.sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

## Implementation Slice Guidance

The implementation plan should split M4e into reviewable tasks:

1. Add config/noreplace template and v2 config authority projection.
2. Add target-profile-aware v2 authoring diagnostics and CLI flags.
3. Map declarative manifest lifecycle hooks into signed v2 lifecycle authority.
4. Split structural v2 archive validation from profile-specific lifecycle
   validation and replace debug-TOML install-affecting-field rejection with
   signed-projection consistency checks.
5. Replace placeholder profile lifecycle entries with proof-corpus entries and
   change users/groups/directories from `unsupported` to exact allow lists.
6. Add positive CLI corpus tests for minimal, config, and service packages.
7. Add negative lifecycle/target/trust fixtures from the Proof Corpus section.
8. Change Remi native release verification to resolve the route slug to a
   supported profile after route validation, run profile-specific lifecycle
   validation after structural/trust verification, and extend Remi proof with a
   lifecycle-bearing native package plus unsupported-route and
   unsupported-lifecycle negatives.
9. Update docs, audit ledgers, coherency rows, M4 exit criteria, and post-M4
   backlog.
10. Run external and local agentic implementation reviews before locking the
   plan and launching the `/goal`.

Do not combine M4e with live lifecycle execution. If execution semantics become
necessary during implementation, stop and write a post-M4 design instead of
stretching this slice.
