# M2a Builder Config, Project Publish, And Divergence Design

**Date:** 2026-06-14
**Status:** Approved design, pre-implementation planning
**Parent design:** `docs/superpowers/specs/2026-06-13-m2-publish-hardening-remi-design.md`
**Parent plan:** `docs/superpowers/plans/2026-06-14-m2a-hermetic-publish-foundation-implementation-plan.md`

## Purpose

This design finishes the remaining M2a hermetic publish foundation after the
initial `cook --isolated` hermetic path landed. The goal is to make hermetic
builder identity explicit from local machine policy, wire project-form
`conary publish <target>` through the same hermetic Kitchen path as isolated
cook, and add host-vs-hermetic divergence diagnostics without unlocking M2b
artifact-form publish.

The invariant remains:

> M2a can emit unsigned hermetic evidence, but it must not emit or accept a
> signed build attestation gate.

Artifact-form publish stays refused until M2b. Project-form publish may create
static repository output from a hermetic rebuild, but its messaging must say
that M2b release attestation gates are not present yet.

## Current Repo Facts

- `crates/conary-core/src/recipe/hermetic/plan.rs` already validates
  `BuilderEnvironmentIdentity` and rejects unconfigured or non-pristine
  identities before hermetic planning succeeds.
- `apps/conary/src/commands/cook.rs` currently creates `HermeticBuildInput`
  with the default unconfigured pristine identity, so `cook --isolated` routes
  through hermetic planning but cannot succeed without a configured builder
  identity.
- `apps/conary/src/commands/publish.rs` still uses `Kitchen::cook()` with
  `allow_network = true` and `pristine_mode = false`, so project-form publish
  remains an M1a sandboxed preview path.
- `apps/conary/src/commands/publish.rs` already refuses
  `conary publish <pkg.ccs> <target>` with the M2 attestation message. This
  behavior must remain unchanged in M2a.
- `KitchenConfig::pristine_mode` requires a local `sysroot` path containing the
  build toolchain; builder identity hashes alone are not enough for Kitchen to
  execute a pristine/no-host-mount build.
- `HermeticBuildPlan::from_recipe()` validates `recipe.all_build_deps()` against
  `locked_repository_dependencies`. Commands must either populate content
  identities for all build dependencies or fail closed before claiming hermetic
  evidence.
- `Kitchen::cook_hermetic()` intentionally prefetches with the caller's
  download policy, then applies `SourceDownloadPolicy::OfflineCacheOnly` only to
  the cloned build-phase config.
- `apps/conary/src/commands/cook.rs`,
  `crates/conary-core/src/recipe/hermetic/plan.rs`,
  `crates/conary-core/src/recipe/kitchen/mod.rs`,
  `crates/conary-core/src/recipe/kitchen/cook.rs`, and
  `crates/conary-core/src/recipe/kitchen/reproducibility_env.rs` are already
  large enough that new behavior should preserve or improve ownership
  boundaries instead of adding broad orchestration to those files.

## Scope

In scope:

- Local hermetic builder configuration for CLI commands.
- Shared builder identity loading for `cook --isolated` and project-form
  `publish <target>`.
- Local pristine sysroot path selection from builder config.
- Fail-closed build dependency lock population or refusal.
- Project-form publish through `Kitchen::cook_hermetic()`.
- Honest CLI messaging that distinguishes unsigned M2a hermetic evidence from
  M2b attestation gates.
- Diagnostic-only host-vs-hermetic divergence evidence.
- Focused tests and docs updates for the changed command behavior.

Out of scope:

- Signed `BuildAttestationEnvelope`.
- Artifact-form publish.
- Accepted signer policy.
- Remi push.
- Foreign package ingestion.
- Builder image discovery, inspection, or automatic sysroot hashing.
- Auto-installing or mutating build dependencies during hermetic publish.
- Deep file-level artifact diffing beyond stable content identity comparison.

## Builder Configuration

The CLI owns local builder configuration. Core owns validation and evidence
truth.

Add a small CLI helper, likely
`apps/conary/src/commands/hermetic_config.rs`, shared by `cook.rs` and
`publish.rs`. It parses local machine policy and returns a core
`BuilderEnvironmentIdentity` plus the local sysroot path needed by
`KitchenConfig`. It must not decide whether the build is truly hermetic;
`HermeticBuildPlan::from_recipe()` keeps that fail-closed gate.

Config resolution order:

1. `CONARY_HERMETIC_CONFIG`
2. `$XDG_CONFIG_HOME/conary/hermetic.toml`
3. `$HOME/.config/conary/hermetic.toml`

The config is local machine state. M2a must not discover repository-local
config by default.

Config format:

```toml
default_builder = "native-x86_64"

[builders.native-x86_64]
kind = "pristine"
sysroot_path = "/var/lib/conary/sysroots/fedora-44-x86_64"
sysroot_hash = "sha256:<64 hex>"
toolchain_hash = "sha256:<64 hex>"
description = "Fedora 44 pristine x86_64 builder"
```

Rules:

- Missing config fails closed for hermetic cook and project-form publish.
- Missing `default_builder` fails closed.
- Unknown default builder fails closed.
- `kind = "pristine"` is the only accepted M2a builder kind.
- `sysroot_path` is required, must resolve to an existing directory, and is
  passed to `KitchenConfig::sysroot`. It is local execution policy and is not
  copied into hermetic evidence.
- At least one of `sysroot_hash` or `toolchain_hash` is required.
- Hashes must be `sha256:` plus 64 hex characters.
- `description` is accepted for humans but is not copied into hermetic
  evidence.
- On Unix, a group-writable or world-writable config file fails closed because
  the file is trust policy, even though it is not secret.
- On Unix, the config file must be owned by the current effective user or root,
  and every existing parent directory from the resolved config file up to the
  config root must not be group- or world-writable.
- Loading and parse errors must include the path consulted. Core builder
  validation errors may pass through, but the CLI wrapper adds the resolved
  config path as context.
- `CONARY_HERMETIC_CONFIG` is independent of `CONARY_HERMETIC_CI`; the former
  selects config location, while the latter controls CI-mode local-tree policy.
- Tests and CI should pass a temp config with `CONARY_HERMETIC_CONFIG`.

This keeps command modules thin: commands load local policy, core validates the
identity and assembles evidence, and Kitchen executes the build.

## Dependency Lock Strategy

Hermetic commands must not rely on ambient host packages or mutable dependency
resolution. Before calling `Kitchen::cook_hermetic()`, `cook --isolated` and
project-form publish must inspect `recipe.all_build_deps()` and produce
`LockedRepositoryDependency` entries for every build dependency.

For M2a, the command behavior is:

- If the recipe has no build dependencies, an empty dependency lock is valid.
- If local repository metadata can resolve every build dependency to a package
  version, repository snapshot/version, architecture, and content identity, the
  command passes those entries through
  `HermeticBuildInput::with_locked_repository_dependencies(...)`.
- If any build dependency cannot be resolved with a content identity from local
  trusted metadata, the command fails closed before the build. It must not
  silently fall back to host packages, auto-install makedepends, or fetch
  mutable dependency metadata during the build.

The initial implementation plan may split the dependency-lock resolver into a
focused helper, but it must preserve the fail-closed rule above. The existing
core validation in `HermeticBuildPlan::from_recipe()` remains the final guard.

## Command Flow

### `conary cook --isolated`

`--isolated` remains the public hermetic cook path. The hidden `--hermetic`
compatibility flag should continue to use the same path.

Flow:

1. Resolve the explicit recipe or infer the source tree recipe.
2. Load the default builder identity from hermetic config.
3. Set `KitchenConfig::sysroot` from the selected builder's `sysroot_path`.
4. Resolve build dependency locks or fail closed.
5. Build `HermeticBuildInput` with the builder identity and dependency locks.
6. Prefetch sources before the build environment starts.
7. Run `Kitchen::cook_hermetic(..., detect_ci_mode())`.
8. Record hermetic evidence with no M2b build attestation.

If config loading fails, the command fails before cooking and prints an
actionable message naming the missing or invalid hermetic config.

Host `conary cook` remains the compatibility iteration path and should continue
to use host environment variables only for non-hermetic builds.

### `conary publish <target>`

Project-form publish should use the same builder config loader and the same
hermetic Kitchen entrypoint as `cook --isolated`.

Flow:

1. Resolve and parse the project recipe.
2. Validate the recipe.
3. Load the default builder identity from hermetic config.
4. Set `KitchenConfig::sysroot` from the selected builder's `sysroot_path`.
5. Resolve build dependency locks or fail closed.
6. Build `HermeticBuildInput::explicit_recipe(...)` with the builder identity
   and dependency locks.
7. Run `Kitchen::cook_hermetic(..., detect_ci_mode())`.
8. Publish the resulting CCS through the existing static repo publisher.
9. Print that M2a hermetic evidence was recorded and that M2b attestation gates
   are not present.

Suggested output:

```text
M2a static publish records hermetic build evidence, but release attestation gates arrive in M2b.
Cooking <name> <version> for static publish (hermetic, pristine/no-host-mount, network disabled)...
```

`publish_kitchen_config()` should move to hermetic defaults:

- `use_isolation = true`
- `allow_network = false`
- `pristine_mode = true`
- `sysroot` set from selected builder config
- `recipe_source_base_dir` derived from the recipe path

The initial config passed into `cook_hermetic()` must retain
`source_download_policy = AllowDownloads` so the internal prefetch phase can
populate the source cache. The hermetic plan applies
`SourceDownloadPolicy::OfflineCacheOnly`, reproducibility controls, and evidence
only to the build-phase config after prefetch completes.

### `conary publish <pkg.ccs> <target>`

Artifact-form publish remains refused in M2a with the existing exact message:

```text
artifact-form publish requires M2 attestation support; run project-form publish from a recipe project
```

M2a must not serialize `attested` as a hardening level, must not embed
`build_attestation`, and must not create an alternate artifact-form bypass for
locally built hermetic packages.

## Divergence Diagnostics

Divergence is diagnostic evidence in M2a, not a publish gate.

Core additions under `crates/conary-core/src/recipe/hermetic/`:

- `HostBuildRecord`
- `DivergenceReport`
- `DivergenceStatus`
- `divergence.rs` for comparison logic and tests

Suggested statuses:

- `no-host-record`
- `matches-host`
- `differs-from-host`

Host cook should record a small local host build record after a successful
non-hermetic build when provenance contains a stable content identity. The
write is command-owned: after `cmd_cook_with_output` receives a successful
non-hermetic `CookResult`, it extracts `merkle_root` and `dna_hash` from
`CookResult.provenance` and calls a command-owned state helper. Core does not
write host-build-record files. If either stable identity field is absent, host
cook still succeeds and no record is written. The record should include:

- a stable input key derived from recipe/source identity
- package name, version, and release
- output content identity, preferably manifest `merkle_root` plus `dna_hash`
- package path for diagnostics only
- build timestamp

Local storage is command-owned state, not repository state. Use:

1. `CONARY_HERMETIC_STATE_DIR` when set
2. `$XDG_STATE_HOME/conary/hermetic`
3. `$HOME/.local/state/conary/hermetic`

Hermetic cook and publish should attempt to load the most recent matching host
record before the hermetic build. Missing or unreadable local state should
produce `no-host-record` with a diagnostic, not fail the hermetic build.

The comparison target should be the stable content identity Kitchen already
computes during plating, such as manifest `merkle_root` and `dna_hash`, rather
than the final `.ccs` hash. The final package hash includes provenance and would
be affected by the divergence report itself.

`HermeticBuildEvidence` should carry a `divergence: DivergenceReport` field.
The initial plan may start with `no-host-record` or a pending comparison, but
final divergence status is not known until Kitchen plating computes the
hermetic output identity. The expected host record should be passed into the
build-phase Kitchen config, and plating should call the core divergence helper
after `merkle_root` and `dna_hash` are available but before the CCS package is
written. This avoids comparing the final `.ccs` hash and avoids making
divergence depend on evidence that itself contains the divergence result.

The output content identity used for divergence must stay independent of
`hermetic_evidence`, `hardening_level`, and the divergence report. If that
invariant changes, divergence must move outside the identity it compares.

If the hermetic output differs from the host record, the command should print a
concise warning and embed `differs-from-host` in hermetic evidence. Project-form
publish still proceeds in M2a as long as the hermetic build itself passes.

Host build records have no integrity protection in M2a. Because divergence is
diagnostic-only, corrupted or tampered local records cannot make an artifact
publishable. If M2b promotes divergence to a publish lint gate, the host-record
format must gain a content checksum or signature before that promotion lands.

Host build record pruning is deferred. M2a may write records indefinitely; a
max-count or max-age policy is later cleanup work.

M2b may later promote divergence to a publish lint gate for artifact-form
publish, but this design does not do that.

## Error Handling

Hermetic config errors are build-start blockers:

- no config file found
- config file cannot be parsed
- default builder is missing or unknown
- builder kind is not `pristine`
- no accepted hash is present
- hash format is invalid
- Unix policy file is group/world-writable
- Unix policy file is owned by another non-root user
- Unix parent directory security checks fail
- configured `sysroot_path` is missing or is not a directory
- build dependencies cannot be locked with content identities

Divergence record errors are diagnostics only unless the implementation cannot
write required package output. A missing host record must never degrade the
hermetic hardening claim; it only records that no comparison was available.

Artifact-form publish refusal stays an exact, early error before local key or
repository side effects.

## Maintainability Boundaries

- `apps/conary/src/commands/hermetic_config.rs` owns config path resolution,
  TOML parsing, permission checks, sysroot path validation, and conversion to
  the command-local builder selection plus `BuilderEnvironmentIdentity`.
- A small command-owned state helper owns host build record storage path
  resolution and filesystem writes. Core owns the record and comparison data
  types.
- `crates/conary-core/src/recipe/hermetic/divergence.rs` owns divergence
  comparison logic. Large Kitchen files should only call this helper.
- `apps/conary/src/commands/cook.rs` and
  `apps/conary/src/commands/publish.rs` remain orchestration layers.
- `crates/conary-core/src/recipe/hermetic/` owns evidence semantics and
  comparison status.
- `crates/conary-core/src/recipe/kitchen/` owns build execution and content
  identity capture. If divergence comparison requires non-trivial Kitchen
  changes, keep them near provenance capture rather than broadening
  `kitchen/cook.rs` command flow.

## Testing

Focused unit tests:

- config path precedence
- valid config returns a pristine `BuilderEnvironmentIdentity`
- valid config returns a sysroot path for `KitchenConfig::sysroot`
- missing `default_builder`
- unknown default builder
- invalid hash prefix or length
- unsupported `kind`
- missing or non-directory `sysroot_path`
- Unix group/world-writable policy rejection
- Unix ownership and parent-directory policy rejection
- artifact-form publish exact rejection message
- `publish_kitchen_config()` hermetic defaults while preserving
  `SourceDownloadPolicy::AllowDownloads` for prefetch
- dependency-lock handling for no dependencies, all dependencies locked, and
  missing content identity
- divergence status for no record, matching record, and differing record
- divergence comparison edge cases: both identity fields present and matching,
  both present and differing, only `merkle_root`, only `dna_hash`, and both
  absent

Focused CLI/integration tests:

- `cook --isolated` succeeds with temp `CONARY_HERMETIC_CONFIG`
- `cook --isolated` fails clearly without hermetic config
- `cook --isolated` fails clearly when build dependencies cannot be locked
- project-form publish records hermetic evidence without `build_attestation`
- artifact-form publish still requires M2b attestation
- host cook followed by hermetic cook records a divergence status when feasible

Suggested focused verification:

```bash
cargo test -p conary --lib commands::cook
cargo test -p conary --lib commands::publish
cargo test -p conary --test packaging_m2a
```

Suggested broader verification before lock-in:

```bash
cargo test -p conary
cargo test -p conary-core recipe::hermetic
cargo test -p conary-core recipe::kitchen
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

## Documentation Updates

When implementation lands, update only docs whose public truth changes:

- `docs/superpowers/specs/2026-06-13-m2-publish-hardening-remi-design.md`
- `docs/superpowers/plans/2026-06-14-m2a-hermetic-publish-foundation-implementation-plan.md`
- `docs/modules/recipe.md`
- `docs/modules/feature-ownership.md`
- `docs/llms/subsystem-map.md` if the "look here first" route changes
- docs audit inventory or ledger rows required by touched public claims
