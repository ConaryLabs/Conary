# Legacy Scriptlet Bundle Schema V1 And Passive Query Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the passive Legacy Scriptlet Semantics Bundle schema, preserve it
through CCS manifest/archive round trips, and expose `conary query scripts`
rendering without changing package install behavior.

**Architecture:** Add a dedicated `legacy_scriptlets` CCS module with
versioned serde types and validation. Attach it to `CcsManifest` as a TOML-only
field. Preserve it through archive read/write by using the existing
`MANIFEST.toml` overlay path. Extend the existing query command so CCS package
files can render bundle summaries, verbose entry details, entry filters, and
JSON output.

**Tech Stack:** Rust, Serde, TOML manifest serialization, CCS archive reader and
writer, existing `conary` CLI query command, Cargo unit and CLI tests.

---

## `/goal` Objective

Use this exact objective when starting execution:

```text
/goal Implement Goal 1: add the type-safe legacy scriptlet semantics bundle data model, TOML/archive overlay round trips, strict cryptographic body-hash validation, and passive query rendering without changing install behavior. Add `.ccs` detection inside `conary query scripts`, keep `commands::cmd_scripts` re-exported, and map bundle validation errors into `ManifestError`. Stop when schema tests reject tampered scriptlet bodies, archive tests preserve passive bundle metadata, all CcsManifest literals compile, and `conary query scripts` renders CCS bundles in text and JSON.
```

## Design Spec

Read this design before editing code:

- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-bundle-schema-v1-passive-query-design.md`
- Parent context:
  `docs/superpowers/specs/2026-05-27-legacy-scriptlet-semantics-bundle-design.md`
- Goal queue context:
  `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`

## File Structure

Create:

- `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- `apps/conary/src/commands/query/scripts.rs`
- `apps/conary/tests/query_scripts.rs`

Modify:

- `crates/conary-core/src/ccs/mod.rs`
- `crates/conary-core/src/ccs/manifest.rs`
- `crates/conary-core/src/ccs/archive_reader.rs`
- `crates/conary-core/src/ccs/package.rs`
- `crates/conary-core/src/ccs/builder.rs`
- `crates/conary-core/src/ccs/convert/converter.rs`
- `apps/remi/src/server/conversion.rs`
- `apps/conary/src/commands/mod.rs`
- `apps/conary/src/commands/query/mod.rs`
- `apps/conary/src/cli/query.rs`
- `apps/conary/src/dispatch.rs`
- `docs/modules/ccs.md` or the closest existing CCS/query module doc

No functional change is expected in
`crates/conary-core/src/ccs/builder/package_writer.rs`; it already writes
`MANIFEST.toml`. Use it in tests if convenient.

## Safety Rules

- Do not make install, update, remove, adoption, or unadoption consume the
  bundle.
- Do not run preserved scriptlet bodies.
- Do not generate arbitrary `ScriptHook` values from legacy scriptlet entries.
- Do not add a database migration in this goal.
- TOML-only storage is the Goal 1 path. Archive preservation tests verify it
  works. If a future goal needs CBOR-level bundle access, expand
  `BinaryManifest` in that future goal.
- Do not change `CcsManifest::validate()` away from `Result<(), ManifestError>`;
  map bundle validation errors into the existing manifest error type.
- Do not add CCS detection to the global native `detect_package_format`
  function unless that broader registry intentionally grows CCS support. For
  Goal 1, add CCS detection inside the `cmd_scripts` path.
- Treat unknown action enum values as passive metadata only; later replay goals
  must fail closed before mutation.

## Preconditions

Goal 0 should already have produced Remi conversion timing and scriptlet corpus
evidence before Goal 1 is implemented. If Goal 1 is executed in isolation, keep
it strictly schema/query focused and avoid adding latency, publication, or
adapter coverage claims.

## Follow-Up Requirement For Replay Goals

Goal 1 remains migration-free, but Goals 6 and 7 must persist the complete
`LegacyScriptletBundle` into local installed-package state during install.
Remove and upgrade operations cannot assume the original `.ccs` archive is
still available, and the older raw scriptlet table is not enough to preserve
target compatibility, sandbox requirements, decisions, and timeouts. Treat this
as a replay-engine prerequisite before any live legacy replay is enabled.

## Task 1: Add Passive Bundle Types

**Files:**

- Create: `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- Modify: `crates/conary-core/src/ccs/mod.rs`

- [ ] **Step 1: Write failing serialization tests**

Create `crates/conary-core/src/ccs/legacy_scriptlets.rs` with the path comment
and an initial test module. Also add `pub mod legacy_scriptlets;` to
`crates/conary-core/src/ccs/mod.rs` now so Cargo can discover the test module.
Keep public type re-exports for Step 4 after the types exist. Add tests named:

- `legacy_scriptlet_bundle_round_trips_core_fields`
- `legacy_scriptlet_bundle_round_trips_reserved_metadata`
- `legacy_scriptlet_bundle_preserves_unknown_optional_fields`
- `legacy_scriptlet_bundle_retains_unknown_typed_enum_values`
- `legacy_scriptlet_bundle_accepts_zero_entry_native_free_package`

The first fixture should include:

- schema `conary.legacy-scriptlets.v1`;
- one source package identity;
- `target_compatibility = "source-native"`;
- `foreign_replay_policy = "deny"`;
- one `replaced` entry;
- one `legacy` entry;
- at least one effect with `replacement = "complete"`.

The reserved metadata fixture must include non-empty RPM trigger, DEB
maintainer/purge, Arch `.INSTALL`, and residual replay tables.

- [ ] **Step 2: Run the targeted test to confirm it fails**

Run:

```bash
cargo test -p conary-core legacy_scriptlets::tests::legacy_scriptlet_bundle_round_trips_core_fields
```

Expected: compile failure because the module and types are not implemented.

- [ ] **Step 3: Implement the data model**

Implement these public types or close equivalents:

```rust
// conary-core/src/ccs/legacy_scriptlets.rs
//! Passive Legacy Scriptlet Semantics Bundle metadata for CCS packages.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const LEGACY_SCRIPTLET_SCHEMA_V1: &str = "conary.legacy-scriptlets.v1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LegacyScriptletBundle {
    pub schema: String,
    pub schema_revision: u16,
    pub source_format: SourceFormat,
    pub source_family: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_distro: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_release: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_arch: Option<String>,
    pub source_package: String,
    pub source_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_checksum: Option<String>,
    pub version_scheme: VersionScheme,
    pub conversion_tool: String,
    pub conversion_tool_version: String,
    pub conversion_policy: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_registry_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_policy_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_digest: Option<String>,
    pub target_compatibility: TargetCompatibility,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_targets: Vec<String>,
    pub foreign_replay_policy: ForeignReplayPolicy,
    pub publication_policy: PublicationPolicy,
    pub publication_status: PublicationStatus,
    pub scriptlet_fidelity: ScriptletFidelity,
    pub decision_counts: DecisionCounts,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unsupported_class_counts: BTreeMap<String, u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<LegacyScriptletEntry>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}
```

Do not derive `Eq` on structs that contain `toml::Value` in an `extra` map;
`toml::Value` can contain floats and does not support `Eq`. Use `Eq` only for
enums or plain structs whose fields support it.

Do not use one generic string wrapper for all enum domains. Define distinct
type-safe enums for each category, each with an `Unknown(String)` variant so
passive query can retain future values without making unrelated domains
interchangeable:

- `SourceFormat`
- `VersionScheme`
- `TargetCompatibility`
- `ForeignReplayPolicy`
- `PublicationPolicy`
- `PublicationStatus`
- `ScriptletFidelity`
- `ScriptletDecision`
- `LifecyclePath`
- `EffectSource`
- `EffectConfidence`
- `EffectReplacement`

Serde's `#[serde(other)]` is not enough because it cannot retain the unknown
string. Implement custom `Serialize`/`Deserialize`, or add a small local macro
that generates distinct string enums with `Unknown(String)`. Add helper methods
such as `as_str()`, `is_known()`, `is_actionable_for_replay()`, and
`is_publication_eligible()` on the specific enum types that need them.

Concrete pattern:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetCompatibility {
    SourceNative,
    FamilyCompatible,
    ConaryPortable,
    ReviewRequired,
    Blocked,
    Unknown(String),
}

impl TargetCompatibility {
    pub fn as_str(&self) -> &str {
        match self {
            Self::SourceNative => "source-native",
            Self::FamilyCompatible => "family-compatible",
            Self::ConaryPortable => "conary-portable",
            Self::ReviewRequired => "review-required",
            Self::Blocked => "blocked",
            Self::Unknown(value) => value.as_str(),
        }
    }

    pub fn is_actionable_for_replay(&self) -> bool {
        !matches!(self, Self::Unknown(_) | Self::ReviewRequired | Self::Blocked)
    }
}
```

Use a local macro if that keeps the repeated serialize/deserialize boilerplate
small, but do not collapse these domains into one shared enum or newtype.

Implement entry/effect/reserved structs matching the design spec:

- `LegacyScriptletEntry`
- `NativeInvocation`
- `TransactionOrder`
- `ScriptletSandboxRequirements`
- `ScriptletEffect`
- `RpmTriggerMetadata`
- `RpmTriggerTargetConstraint`
- `DebMaintainerMetadata`
- `ArchInstallMetadata`
- `ResidualReplayMetadata`
- `DecisionCounts`

`DecisionCounts` must preserve forward-compatible unknown decision keys. Use a
shape like:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DecisionCounts {
    #[serde(default)]
    pub replaced: u32,
    #[serde(default)]
    pub legacy: u32,
    #[serde(default)]
    pub blocked: u32,
    #[serde(default)]
    pub review: u32,
    #[serde(flatten)]
    pub extra: BTreeMap<String, u32>,
}
```

Validation should include `extra` counts in the total entry count.

Prefer `#[serde(default, skip_serializing_if = "...")]` on optional fields so
minimal fixtures stay readable.

- [ ] **Step 4: Export the module**

Modify `crates/conary-core/src/ccs/mod.rs`:

```rust
pub mod legacy_scriptlets;
pub use legacy_scriptlets::{
    LegacyScriptletBundle, LegacyScriptletEntry, LEGACY_SCRIPTLET_SCHEMA_V1,
};
```

Export only the types needed outside `conary-core`; keep helper internals
module-local when possible.

- [ ] **Step 5: Verify Task 1**

Run:

```bash
cargo test -p conary-core legacy_scriptlets
```

Expected: serialization tests pass.

Commit checkpoint:

```bash
git add crates/conary-core/src/ccs/legacy_scriptlets.rs crates/conary-core/src/ccs/mod.rs
git commit -m "feat(ccs): add legacy scriptlet bundle schema"
```

## Task 2: Add Bundle Validation

**Files:**

- Modify: `crates/conary-core/src/ccs/legacy_scriptlets.rs`

- [ ] **Step 1: Write failing validation tests**

Add tests:

- `legacy_scriptlet_bundle_rejects_duplicate_entry_ids`
- `legacy_scriptlet_bundle_rejects_mismatched_decision_counts`
- `legacy_scriptlet_bundle_rejects_zero_timeout`
- `legacy_scriptlet_bundle_rejects_malformed_sha256_digest`
- `legacy_scriptlet_bundle_rejects_tampered_body_hash`
- `legacy_scriptlet_bundle_validates_base64_body_hash`
- `legacy_scriptlet_bundle_rejects_malformed_allowed_target`

- [ ] **Step 2: Run the targeted failing test**

Run:

```bash
cargo test -p conary-core legacy_scriptlets::tests::legacy_scriptlet_bundle_rejects_duplicate_entry_ids
```

Expected: failure because `validate()` does not exist or does not reject the
case yet.

- [ ] **Step 3: Implement validation**

Add:

```rust
impl LegacyScriptletBundle {
    pub fn validate(&self) -> anyhow::Result<()> {
        // schema, revision, required strings, unique IDs, digest shapes,
        // allowed target shape, entry validation, and decision-count checks.
    }
}

impl LegacyScriptletEntry {
    fn validate(&self) -> anyhow::Result<()> {
        // native slot, phase, interpreter, body digest, timeout, decision,
        // reason code, and nested reserved metadata checks.
    }
}
```

Validation rules:

- `schema` must equal `LEGACY_SCRIPTLET_SCHEMA_V1`;
- `schema_revision` must be greater than zero;
- required string fields must not be empty after trimming;
- `body_sha256`, `source_checksum`, `adapter_registry_digest`,
  `target_policy_digest`, `evidence_digest`, and entry/effect evidence digests
  must use `sha256:<64 lowercase-or-uppercase hex>` when present;
- `body_sha256` must cryptographically match the preserved body bytes. For
  `body_encoding = "base64"`, decode the body first and hash decoded bytes.
  For UTF-8 bodies, hash `body.as_bytes()`. Use the workspace `sha2` or
  `crate::hash` helpers already present in `conary-core`;
- `body_encoding` may be absent, `utf-8`, or `base64`. Reject unknown encodings
  because the preserved bytes cannot be validated safely;
- entry IDs must be unique;
- `timeout_ms` must be greater than zero;
- `decision_counts` must match entry decisions. Unknown decision keys are
  allowed for forward compatibility, but the sum of all counts, known and
  unknown, must equal `entries.len()`;
- `allowed_targets` must match `<format>/<distro>/<release>/<arch>` when
  provided.

Do not reject unknown optional fields. Do not require future trigger fields.

- [ ] **Step 4: Verify Task 2**

Run:

```bash
cargo test -p conary-core legacy_scriptlets
```

Commit checkpoint:

```bash
git add crates/conary-core/src/ccs/legacy_scriptlets.rs
git commit -m "feat(ccs): validate legacy scriptlet bundles"
```

## Task 3: Attach Bundle To `CcsManifest`

**Files:**

- Modify: `crates/conary-core/src/ccs/manifest.rs`
- Modify: `crates/conary-core/src/ccs/package.rs`
- Modify direct `CcsManifest` literals found in:
  - `crates/conary-core/src/ccs/builder.rs`
  - `crates/conary-core/src/ccs/convert/converter.rs`
  - `apps/remi/src/server/conversion.rs`

- [ ] **Step 1: Write failing manifest round-trip tests**

In `manifest.rs` tests, add:

- `manifest_toml_round_trips_legacy_scriptlet_bundle`
- `manifest_validation_rejects_invalid_legacy_scriptlet_bundle`

Use the fixture builder from `legacy_scriptlets.rs` if it is public under
`#[cfg(test)]`, or construct a small fixture in the test.

- [ ] **Step 2: Run the targeted failing test**

Run:

```bash
cargo test -p conary-core manifest_toml_round_trips_legacy_scriptlet_bundle
```

Expected: failure because `CcsManifest` has no `legacy_scriptlets` field.

- [ ] **Step 3: Add the manifest field and update struct literals**

Add to `CcsManifest`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub legacy_scriptlets: Option<LegacyScriptletBundle>,
```

Update constructors/defaults:

- `CcsManifest::new_minimal()`;
- the direct `CcsManifest` struct literal inside
  `convert_binary_to_ccs_manifest()` in `crates/conary-core/src/ccs/package.rs`
  with `legacy_scriptlets: None`;
- any direct test fixture literals that need the new field, including
  `crates/conary-core/src/ccs/builder.rs`,
  `crates/conary-core/src/ccs/convert/converter.rs`, and
  `apps/remi/src/server/conversion.rs`;
- imports in `manifest.rs`.

Before Task 3 verification, run:

```bash
rg -n "CcsManifest \\{" crates/conary-core apps -g '*.rs'
```

and update every direct struct literal so workspace clippy/tests do not fail
outside the focused package tests.

This `package.rs` update belongs in Task 3. Adding a field to `CcsManifest`
will otherwise break compilation immediately, even though archive overlay work
does not happen until Task 4.

- [ ] **Step 4: Wire validation**

In `CcsManifest::validate()`, call:

```rust
if let Some(bundle) = &self.legacy_scriptlets {
    bundle.validate()
        .map_err(|error| ManifestError::Invalid(format!(
            "legacy scriptlet bundle validation failed: {error}"
        )))?;
}
```

The exact variant may be a new `ManifestError::BundleValidation(String)` or the
existing `ManifestError::Invalid(String)`, but do not return `anyhow::Error`
from `CcsManifest::validate()`.

- [ ] **Step 5: Verify Task 3**

Run:

```bash
cargo test -p conary-core manifest_toml_round_trips_legacy_scriptlet_bundle
cargo test -p conary-core legacy_scriptlets
rg -n "CcsManifest \\{" crates/conary-core apps -g '*.rs'
```

Commit checkpoint:

```bash
git add crates/conary-core/src/ccs/manifest.rs crates/conary-core/src/ccs/legacy_scriptlets.rs
git add crates/conary-core/src/ccs/package.rs
git add crates/conary-core/src/ccs/builder.rs crates/conary-core/src/ccs/convert/converter.rs apps/remi/src/server/conversion.rs
git commit -m "feat(ccs): embed legacy scriptlet bundle in manifests"
```

## Task 4: Preserve Bundle Through CCS Archive Read/Write

**Files:**

- Modify: `crates/conary-core/src/ccs/archive_reader.rs`
- Modify: `crates/conary-core/src/ccs/package.rs`
- Use tests in either `archive_reader.rs` or existing CCS builder tests

- [ ] **Step 1: Write failing archive preservation test**

Add a test named:

- `archive_reader_preserves_legacy_scriptlet_bundle_from_toml_overlay`
- `builder_package_writer_preserves_legacy_scriptlet_bundle`

The overlay test should:

1. Build or assemble a CCS archive containing both `MANIFEST` and
   `MANIFEST.toml`.
2. Put `legacy_scriptlets` only in the TOML manifest path.
3. Read the archive through `read_ccs_archive` or the public reader helper.
4. Assert `contents.manifest.legacy_scriptlets.is_some()`.
5. Assert reserved metadata from the bundle still exists.

This test is intentionally about the mixed CBOR+TOML path, because CBOR does
not carry the bundle in Goal 1.

The builder test should create a minimal `CcsManifest` with
`legacy_scriptlets`, write a CCS package through the existing builder/package
writer path, read it back through `read_ccs_archive`, and assert the bundle
survived.

- [ ] **Step 2: Run the targeted failing test**

Run:

```bash
cargo test -p conary-core archive_reader_preserves_legacy_scriptlet_bundle_from_toml_overlay
```

Expected: failure because `archive_reader.rs` does not overlay
`legacy_scriptlets` from TOML yet.

- [ ] **Step 3: Overlay TOML-only field**

In `archive_reader.rs`, when both CBOR and TOML manifests are present, add:

```rust
merged.legacy_scriptlets = toml.legacy_scriptlets;
```

near the existing overlays for `scriptlets`, `policy`, `provenance`,
`redirects`, and `legacy`.

- [ ] **Step 4: Update binary conversion documentation**

In `package.rs`, update the comment above `convert_binary_to_ccs_manifest()` so
the list of TOML-only fields includes `legacy_scriptlets`.

- [ ] **Step 5: Verify Task 4**

Run:

```bash
cargo test -p conary-core archive_reader_preserves_legacy_scriptlet_bundle_from_toml_overlay
cargo test -p conary-core builder_package_writer_preserves_legacy_scriptlet_bundle
cargo test -p conary-core legacy_scriptlets
```

Commit checkpoint:

```bash
git add crates/conary-core/src/ccs/archive_reader.rs crates/conary-core/src/ccs/package.rs
git commit -m "feat(ccs): preserve legacy scriptlet bundles in archives"
```

## Task 5: Add Passive Query Rendering For CCS Bundles

**Files:**

- Create: `apps/conary/src/commands/query/scripts.rs`
- Modify: `apps/conary/src/commands/query/mod.rs`
- Modify: `apps/conary/src/commands/mod.rs`

- [ ] **Step 1: Write renderer unit tests**

Add tests in `apps/conary/src/commands/query/scripts.rs`:

- `script_query_summary_renders_bundle_counts`
- `script_query_verbose_renders_entry_details`
- `script_query_entry_filter_renders_one_entry`
- `script_query_json_omits_raw_bodies_by_default`
- `script_query_json_reports_no_bundle_without_entries`
- `script_query_json_reports_zero_entry_bundle`

Keep these as pure rendering tests where possible. They should operate on a
fixture `LegacyScriptletBundle` and avoid spawning the CLI.

Also add `mod scripts;` to `apps/conary/src/commands/query/mod.rs` before
running the targeted test so Cargo can discover the new module. Add public
re-exports after the types/functions exist in Step 3.

- [ ] **Step 2: Run the targeted failing test**

Run:

```bash
cargo test -p conary script_query_summary_renders_bundle_counts
```

Expected: compile failure because the module does not exist yet.

- [ ] **Step 3: Move native scriptlet query into the query module**

Move the current `cmd_scripts(package_path: &str)` implementation from
`apps/conary/src/commands/mod.rs` into
`apps/conary/src/commands/query/scripts.rs`.

Keep native RPM/DEB/Arch output behavior unchanged for default calls.

Add in `apps/conary/src/commands/query/mod.rs`:

```rust
mod scripts;
pub use scripts::{ScriptQueryOptions, cmd_scripts, cmd_scripts_with_options};
```

Remove the stale comment in `commands/mod.rs` that says `cmd_scripts` is defined
in that module, and include `cmd_scripts`/`ScriptQueryOptions` in the query
re-export list.

Keep the public path `commands::cmd_scripts` available from
`apps/conary/src/commands/mod.rs`. The dispatcher can continue calling that
path after Task 6 adds options as long as the query module re-export is present.

Move the native package scriptlet detection helpers and imports with the command
into `commands/query/scripts.rs`, or intentionally leave them in
`commands/mod.rs` and call them through the parent module. Preserve current
native RPM/DEB/Arch behavior and any tests that rely on the helper API.

- [ ] **Step 4: Add query options and CCS detection**

Define:

```rust
#[derive(Debug, Clone, Default)]
pub struct ScriptQueryOptions {
    pub verbose: bool,
    pub entry: Option<String>,
    pub json: bool,
}
```

Keep a compatibility wrapper so Task 5 compiles before CLI/dispatch flags are
added in Task 6:

```rust
pub async fn cmd_scripts(package_path: &str) -> Result<()> {
    cmd_scripts_with_options(package_path, ScriptQueryOptions::default()).await
}

pub async fn cmd_scripts_with_options(
    package_path: &str,
    options: ScriptQueryOptions,
) -> Result<()>
```

Detection order:

1. If the path ends with `.ccs`, parse it as CCS first:
   `let pkg = <CcsPackage as PackageFormat>::parse(path)?;`. Import
   `conary_core::ccs::CcsPackage` and the `PackageFormat` trait, or use
   `read_ccs_archive` directly if that keeps the module cleaner.
2. For `.ccs` paths, if parsing fails, return a CCS parse error. Do not fall
   back to native RPM/DEB/Arch detection.
3. For `.ccs` paths with a bundle, render the CCS bundle.
4. For `.ccs` paths without a bundle, return an error if `--entry` was
   requested. Otherwise print the no-bundle message and exit 0.
5. For non-`.ccs` paths, try the existing native RPM/DEB/Arch detection first
   and keep current native scriptlet output behavior.
6. If native detection fails for a non-`.ccs` path, optionally try CCS parsing
   as a fallback for extensionless local fixtures. If that also fails, return
   the original native detection error.

A bundle with `entries = []` is valid. Render it as "No legacy scriptlet
entries. This package does not require native scriptlet replay." and exit 0
unless `--entry` requested a missing ID.

Goal 1 option behavior:

- `--verbose`, `--entry`, and `--json` are CCS bundle modes.
- Native RPM/DEB/Arch default output remains unchanged.
- `--verbose` on native packages may keep the current detailed native output.
- `--entry` and `--json` on native packages should return a clear
  "CCS legacy scriptlet bundles only" error unless native structured output is
  designed in a later goal.

- [ ] **Step 5: Add JSON report types**

Add serializable report structs local to `scripts.rs`, such as:

```rust
#[derive(Debug, Serialize)]
struct ScriptQueryReport<'a> {
    package: PackageQueryIdentity<'a>,
    bundle_present: bool,
    bundle: Option<BundleQuerySummary<'a>>,
    entries: Vec<EntryQuerySummary<'a>>,
    warnings: Vec<String>,
}
```

JSON output should include body digests, decisions, reason codes, effects, and
reserved metadata summaries. CCS bundle query output should not include full
raw script bodies in either text or JSON in Goal 1. Existing native
RPM/DEB/Arch scriptlet inspection keeps its current behavior.

For a CCS package with no bundle, JSON output should be:
`bundle_present: false`, `bundle: null`, `entries: []`, and success unless
`--entry` requested a missing entry. For a zero-entry bundle, JSON should be:
`bundle_present: true` with `entries: []`.

- [ ] **Step 6: Verify Task 5**

Run:

```bash
cargo test -p conary query_scripts
```

Commit checkpoint:

```bash
git add apps/conary/src/commands/query/scripts.rs apps/conary/src/commands/query/mod.rs apps/conary/src/commands/mod.rs
git commit -m "feat(query): render legacy scriptlet bundles"
```

## Task 6: Wire CLI Flags And Dispatch

**Files:**

- Modify: `apps/conary/src/cli/query.rs`
- Modify: `apps/conary/src/dispatch.rs`
- Check: `apps/conary/src/command_risk.rs`

- [ ] **Step 1: Write failing CLI tests**

In `apps/conary/tests/query_scripts.rs`, add tests that invoke the CLI parser or
CLI binary for:

- `query_scripts_accepts_verbose_flag`
- `query_scripts_accepts_entry_filter`
- `query_scripts_accepts_json_flag`

If integration helpers make constructing a CCS archive too heavy, keep one
parser-level test in the CLI module and reserve full file execution for Task 7.

- [ ] **Step 2: Run the targeted failing test**

Run:

```bash
cargo test -p conary query_scripts_accepts_json_flag
```

Expected: failure because the CLI enum does not accept the flag yet.

- [ ] **Step 3: Add CLI flags**

Change the enum variant in `apps/conary/src/cli/query.rs` to:

```rust
Scripts {
    /// Path to the package file to inspect
    package_path: String,
    /// Show full bundle entry details
    #[arg(long)]
    verbose: bool,
    /// Show only one bundle entry by ID
    #[arg(long)]
    entry: Option<String>,
    /// Emit machine-readable JSON
    #[arg(long)]
    json: bool,
},
```

- [ ] **Step 4: Update dispatch**

Change the dispatch arm to build `ScriptQueryOptions`:

```rust
cli::QueryCommands::Scripts {
    package_path,
    verbose,
    entry,
    json,
} => {
    commands::cmd_scripts_with_options(
        &package_path,
        commands::ScriptQueryOptions { verbose, entry, json },
    )
    .await
}
```

Check `apps/conary/src/command_risk.rs`. If it already matches
`QueryCommands::Scripts { .. }`, no risk change is needed and the command
should remain read-only.

- [ ] **Step 5: Verify Task 6**

Run:

```bash
cargo test -p conary query_scripts_accepts_json_flag
cargo test -p conary query_scripts
```

Commit checkpoint:

```bash
git add apps/conary/src/cli/query.rs apps/conary/src/dispatch.rs apps/conary/src/command_risk.rs apps/conary/tests/query_scripts.rs
git commit -m "feat(cli): add script bundle query flags"
```

## Task 7: Add End-To-End Query Tests

**Files:**

- Modify: `apps/conary/tests/query_scripts.rs`
- Reuse helpers from existing CLI tests where available

- [ ] **Step 1: Add CCS fixture generation**

Build a minimal CCS package fixture with a `legacy_scriptlets` bundle. Prefer
using existing CCS builder/package writer helpers over static binary fixtures.
The fixture should include:

- one `replaced` entry;
- one `legacy` entry;
- one reserved metadata table;
- no raw body in text or JSON assertions for CCS bundle output.

- [ ] **Step 2: Add CLI behavior tests**

Add tests:

- `query_scripts_ccs_bundle_prints_summary`
- `query_scripts_ccs_bundle_verbose_prints_effects`
- `query_scripts_ccs_bundle_entry_filter_prints_single_entry`
- `query_scripts_ccs_bundle_missing_entry_exits_with_error`
- `query_scripts_ccs_bundle_json_is_stable`
- `query_scripts_ccs_without_bundle_exits_successfully`
- `query_scripts_ccs_without_bundle_json_reports_absent_bundle`
- `query_scripts_ccs_zero_entry_bundle_json_reports_empty_entries`
- `query_scripts_native_json_reports_ccs_bundle_only`
- `query_scripts_native_entry_filter_reports_ccs_bundle_only`

Keep assertions focused on stable strings and JSON keys. Do not assert full
pretty text formatting.

- [ ] **Step 3: Run targeted tests**

Run:

```bash
cargo test -p conary query_scripts_ccs_bundle
```

- [ ] **Step 4: Verify no behavior path changed**

Run a narrow native-package regression if existing fixtures are available. If
the repository does not have RPM/DEB/Arch package fixtures, leave a test comment
explaining that pure renderer tests preserve native behavior and no package
fixture exists in-tree. Also manually verify the default native path still uses
the current `detect_package_format` flow when a local RPM, DEB, or Arch fixture
is available.

- [ ] **Step 5: Commit Task 7**

```bash
git add apps/conary/tests/query_scripts.rs
git commit -m "test(query): cover legacy scriptlet bundle output"
```

## Task 8: Document The Passive Surface

**Files:**

- Modify: `docs/modules/ccs.md` or the closest existing CCS/query module doc
- Optionally modify: `docs/modules/remi.md` if it mentions future bundle work

- [ ] **Step 1: Document the bundle as passive metadata**

Add a short section that says:

- converted CCS packages may carry `[legacy_scriptlets]`;
- the bundle is queryable metadata in this goal;
- it does not enable replay yet;
- raw foreign scriptlet replay remains denied until later safety gates land.

- [ ] **Step 2: Document query examples**

Include:

```bash
conary query scripts ./nginx.ccs
conary query scripts ./nginx.ccs --verbose
conary query scripts ./nginx.ccs --entry rpm:%post
conary query scripts ./nginx.ccs --json
```

- [ ] **Step 3: Commit docs**

```bash
git add docs/modules/ccs.md docs/modules/remi.md
git commit -m "docs(ccs): describe passive scriptlet bundles"
```

## Task 9: Final Verification

- [ ] **Step 1: Run target tests**

```bash
cargo test -p conary-core legacy_scriptlets
cargo test -p conary-core manifest_toml_round_trips_legacy_scriptlet_bundle
cargo test -p conary-core archive_reader_preserves_legacy_scriptlet_bundle_from_toml_overlay
cargo test -p conary-core builder_package_writer_preserves_legacy_scriptlet_bundle
cargo test -p conary query_scripts
```

- [ ] **Step 2: Run workspace gates**

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

- [ ] **Step 3: Inspect final diff**

```bash
git status --short
git log --oneline --decorate -5
```

Confirm:

- no install/update/remove path reads `legacy_scriptlets`;
- no Remi conversion path emits bundles yet;
- query text and JSON omit raw script bodies for CCS bundles by default;
- archive preservation uses TOML overlay, not a new CBOR field;
- commits are small and conventional.

## Expected Commit Sequence

1. `feat(ccs): add legacy scriptlet bundle schema`
2. `feat(ccs): validate legacy scriptlet bundles`
3. `feat(ccs): embed legacy scriptlet bundle in manifests`
4. `feat(ccs): preserve legacy scriptlet bundles in archives`
5. `feat(query): render legacy scriptlet bundles`
6. `feat(cli): add script bundle query flags`
7. `test(query): cover legacy scriptlet bundle output`
8. `docs(ccs): describe passive scriptlet bundles`

If a task is tiny, adjacent commits may be combined, but keep schema,
archive-preservation, and CLI behavior independently reviewable.

## Review Questions For DeepSeek And Gemini

1. Which future goal, if any, should consider CBOR-level bundle access, given
   that Goal 1 intentionally keeps the bundle TOML-only?
2. Do distinct type-safe enums with `Unknown(String)` variants preserve enough
   forward compatibility without making future replay safety ambiguous?
3. Are the validation rules strict enough for passive metadata while avoiding
   premature rejection of deferred trigger/purge classes?
4. Should `conary query scripts` remain file-path-only for Goal 1, or should it
   also support installed package lookup by DB name immediately?
5. Should a future debug UX add an explicit `--include-body` flag, or should raw
   CCS bundle bodies stay out of query output long-term?
