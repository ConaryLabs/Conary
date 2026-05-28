# Legacy Scriptlet Passive Remi Bundle Embedding Design

## Summary

Goal 4 is the first point where Remi conversion writes the legacy scriptlet
semantics work into converted CCS artifacts. Earlier goals created the passive
bundle schema, native ABI extraction, command evidence, blocked-class registry,
and corpus-backed bootstrap adapters. Goal 4 connects those pieces by building
a `LegacyScriptletBundle` during conversion, embedding it in
`CcsManifest.legacy_scriptlets`, persisting a compact scriptlet metadata summary
on Remi's `converted_packages` rows, and exposing that summary through Remi
package metadata APIs.

The work remains passive. Goal 4 does not make install/update/remove consume
the bundle, does not replay scriptlets, does not suppress generated CCS hooks,
and does not refuse to serve packages that are currently served by Remi.
`publication_status` is stored as informational conversion metadata in this
goal; Goal 5 makes publication outcomes authoritative.

## Source Context

Read these first when implementing:

- `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`
- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-semantics-bundle-design.md`
- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-bundle-schema-v1-passive-query-design.md`
- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-native-abi-extraction-design.md`
- `docs/superpowers/specs/2026-05-28-legacy-scriptlet-adapter-registry-blocked-classes-design.md`
- `docs/superpowers/specs/2026-05-28-legacy-scriptlet-bootstrap-adapters-design.md`
- `docs/modules/remi.md`
- `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- `crates/conary-core/src/ccs/convert/converter.rs`
- `crates/conary-core/src/ccs/convert/effects.rs`
- `crates/conary-core/src/packages/native_abi.rs`
- `crates/conary-core/src/db/models/converted.rs`
- `apps/remi/src/server/conversion.rs`
- `apps/remi/src/server/handlers/packages.rs`
- `apps/remi/src/server/handlers/index.rs`
- `apps/remi/src/server/index_gen.rs`

Relevant current code facts:

- `CcsManifest` already has `legacy_scriptlets:
  Option<LegacyScriptletBundle>`.
- `LegacyScriptletBundle::validate()` already enforces schema identity,
  decision-count consistency, duplicate-entry rejection, and strict
  `body_sha256` validation.
- `PackageMetadata` already carries both flattened `scriptlets` and
  byte-preserving `native_scriptlet_abi`.
- `ConversionResult` already carries `scriptlet_classification:
  ScriptletClassificationReport`.
- Goal 3b tests intentionally assert `manifest.legacy_scriptlets.is_none()`.
  Goal 4 changes those assertions for conversion paths that now embed bundles.

## Scope

Goal 4 includes:

- a focused bundle-building module that converts parser metadata plus
  `ScriptletClassificationReport` into `LegacyScriptletBundle`;
- deterministic evidence-digest generation for the bundle and Remi database
  summary;
- embedding the bundle into Remi-produced CCS manifests before the CCS package
  is written;
- adding passive scriptlet metadata fields to `ConversionResult`;
- adding passive scriptlet metadata fields to `converted_packages` with
  default values that preserve existing rows;
- storing those metadata fields when Remi records a conversion;
- returning scriptlet metadata in package manifest responses and repository
  metadata/index entries where converted-package rows are already read;
- tests proving converted packages carry bundles, archive round trips preserve
  them, and Remi exposes summary metadata.

All `LegacyConverter` outputs should get passive bundles after Goal 4. Remi
adds richer source context such as distro and, when available, release; local
conversion falls back to deterministic `unknown` source context where the local
caller has no repository metadata.

Goal 4 excludes:

- install/update/remove behavior changes;
- replay, sandbox execution, or target compatibility enforcement;
- Remi publication gating or refusal to serve ready artifacts;
- curation workflows and operator promotion;
- converting raw scriptlet replay into executable CCS hooks;
- sidecar bundle storage;
- database state for installed bundle execution or remove/upgrade replay;
- broad shell interpretation beyond the existing Goal 3a/3b evidence.

## Architecture

Add a conversion-local bridge under `crates/conary-core/src/ccs/convert/`:

```text
PackageMetadata + ExtractedFile list
        |
        v
classify_scriptlets(metadata, files)
        |
        v
ScriptletClassificationReport
        |
        v
bundle_builder::build_legacy_scriptlet_bundle(...)
        |
        v
CcsManifest.legacy_scriptlets + ConversionResult.scriptlet_metadata
        |
        v
Remi converted_packages metadata columns + package/index API fields
```

The bridge is intentionally one way. It projects passive evidence into the
bundle schema but does not teach the installer to act on that bundle.

### New Module

Create `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`.

Responsibilities:

- derive source-format, family, distro, architecture, and version-scheme
  metadata;
- build one bundle entry per byte-preserving native ABI entry when native ABI is
  present;
- fall back to one bundle entry per flattened `Scriptlet` when native ABI is not
  present;
- build a native-free zero-entry bundle when the classification report contains
  only the package-level native-free classification;
- copy adapter effect evidence into `legacy_scriptlets::ScriptletEffect`;
- aggregate unknown commands, blocked classes, decision counts, and
  unsupported class counts;
- compute the deterministic evidence digest;
- return both the full bundle and a compact summary used by Remi DB/API code.

Do not put this bridge in `legacy_scriptlets.rs`. The schema module should stay
format-neutral and validation-focused; conversion policy belongs in
`ccs::convert`.

Export the module from `crates/conary-core/src/ccs/convert/mod.rs`:

```rust
pub mod scriptlet_bundle;
pub use scriptlet_bundle::{
    build_legacy_scriptlet_bundle, ScriptletBundleBuild, ScriptletBundleInput,
    ScriptletBundleSummary, ScriptletDecisionCountsSummary,
};
```

## Bundle Construction

### Inputs

Use a small explicit input type:

```rust
pub struct ScriptletBundleInput<'a> {
    pub source_metadata: &'a PackageMetadata,
    pub final_metadata: &'a PackageMetadata,
    pub source_files: &'a [ExtractedFile],
    pub final_files: &'a [ExtractedFile],
    pub source_format: &'a str,
    pub source_distro: Option<&'a str>,
    pub source_release: Option<&'a str>,
    pub source_arch: Option<&'a str>,
    pub source_checksum: Option<&'a str>,
    pub classification: &'a ScriptletClassificationReport,
    pub conversion_tool: &'a str,
    pub conversion_tool_version: &'a str,
}
```

`source_metadata` and `source_files` are the original parser outputs and are the
authority for native ABI bodies and flattened fallback scriptlet bodies.
`final_metadata` and `final_files` are the post-capture/post-conversion values
used for payload evidence and manifest context. Do not build bundle entries from
`final_metadata`: the current converter may remove captured scriptlets from
that value before manifest creation.

The Remi path should pass the requested distro as `source_distro`. The local
converter path may pass `None`, but bundle construction must normalize it before
digesting:

1. use an explicit caller-provided distro when present;
2. otherwise use a deterministic parser-provided distro if metadata later gains
   one;
3. otherwise use the literal string `unknown`.

`source_release` follows the same rule and falls back to `unknown`.
`source_arch` should use the explicit caller-provided architecture first, then
`metadata.architecture.as_deref()`, then `unknown`.

The evidence digest may intentionally differ when Remi has authoritative distro
context and local conversion does not. It must not differ because one path used
`None` while another path serialized an omitted field differently.

Do not change the public `LegacyConverter::convert()` signature for Goal 4.
Thread source distro through `LegacyConverter` as optional conversion context,
for example:

```rust
pub struct LegacyConverter {
    options: ConversionOptions,
    analyzer: ScriptletAnalyzer,
    source_distro: Option<String>,
    source_release: Option<String>,
    conversion_tool: String,
}

impl LegacyConverter {
    pub fn with_source_distro(mut self, distro: impl Into<String>) -> Self {
        self.source_distro = Some(distro.into());
        self
    }

    pub fn with_conversion_tool(mut self, tool: impl Into<String>) -> Self {
        self.conversion_tool = tool.into();
        self
    }
}
```

`LegacyConverter::new(options)` should default that context to `None`, keeping
local CLI and tests unchanged. It should default `conversion_tool` to a local
converter name such as `conary`. Remi should set source distro from the known
distro name and set `conversion_tool = "remi"` when it constructs the
converter.

### Output

Return:

```rust
pub struct ScriptletBundleBuild {
    pub bundle: LegacyScriptletBundle,
    pub summary: ScriptletBundleSummary,
}
```

`conary_core::ccs::convert::ConversionResult` should add:

```rust
pub legacy_scriptlets: Option<LegacyScriptletBundle>,
pub scriptlet_metadata: ScriptletBundleSummary,
```

This is the core conversion result, not the Remi job result in
`apps/remi/src/server/jobs.rs`. Remi job/API result types may expose
`ScriptletPackageMetadata` summaries where useful, but they should not gain the
full `LegacyScriptletBundle`.

`build_result.manifest.legacy_scriptlets` and `legacy_scriptlets` must refer to
the same logical bundle. Tests should compare stable fields rather than pointer
identity.

Define the compact summary in `scriptlet_bundle.rs` so core conversion, Remi DB
storage, and Remi API serialization all use the same source of truth:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScriptletBundleSummary {
    pub scriptlet_fidelity: String,
    pub target_compatibility: String,
    pub publication_status: String,
    pub evidence_digest: Option<String>,
    pub curation_evidence_digest: Option<String>,
    pub decision_counts: ScriptletDecisionCountsSummary,
    pub blocked_reason_codes: Vec<String>,
    pub review_reason_codes: Vec<String>,
    pub unknown_commands: Vec<String>,
    pub blocked_classes: Vec<String>,
    #[serde(default, skip_serializing)]
    pub review_artifact_path: Option<String>,
}

impl Default for ScriptletBundleSummary {
    fn default() -> Self {
        Self {
            scriptlet_fidelity: "unknown".to_string(),
            target_compatibility: "unknown".to_string(),
            publication_status: "public".to_string(),
            evidence_digest: None,
            curation_evidence_digest: None,
            decision_counts: ScriptletDecisionCountsSummary::default(),
            blocked_reason_codes: Vec::new(),
            review_reason_codes: Vec::new(),
            unknown_commands: Vec::new(),
            blocked_classes: Vec::new(),
            review_artifact_path: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ScriptletDecisionCountsSummary {
    pub replaced: u32,
    pub legacy: u32,
    pub blocked: u32,
    pub review: u32,
}
```

`review_artifact_path` is storage metadata only. It must use
`#[serde(default, skip_serializing)]` on the core summary type, and public Remi
endpoints must map it to a boolean such as `review_artifact_available`. Do not
serialize raw local filesystem paths in public package or index responses.

### Native-Free Packages

If `metadata.scriptlets` and `metadata.native_scriptlet_abi` are both empty,
build a valid zero-entry bundle:

- `entries = []`;
- `decision_counts = DecisionCounts::default()`;
- `scriptlet_fidelity = native-free`;
- `target_compatibility = conary-portable`;
- `publication_policy = public-if-no-blocked`;
- `publication_status = public`;
- `foreign_replay_policy = deny`;
- `blocked_reason_codes = []` and `review_reason_codes = []` in the summary.

This keeps Remi metadata uniform: every newly converted package has scriptlet
metadata even when there are no scriptlets.

### Entry Source Preference

Prefer `metadata.native_scriptlet_abi` over flattened `metadata.scriptlets`.
Native ABI entries are byte-preserving and carry format-specific metadata.

If native ABI entries are present, the bundle must not also create entries from
the flattened compatibility projection. Flattened `scriptlets` may still feed
existing hook detection and command classification, but bundle entry bodies come
from native ABI to avoid duplicate entries and lossy body preservation.

If the classification report contains flattened fallback entry IDs while native
ABI entries are present, those orphan classifications should be included in the
evidence digest and aggregate summary only when they add a reason code, unknown
command, or blocked class not already present on a native entry. They must not
create extra bundle entries and must not downgrade a native entry decision
unless the entry ID matches that native entry.

If native ABI entries are absent, create fallback entries from flattened
`scriptlets`. These entries are always `review` unless every classification for
that entry is known and every effect is `EffectReplacement::Complete`.

### Body Preservation

Native ABI body handling:

- `NativeScriptletBodyEncoding::Utf8` maps to `body_encoding = None` or
  `Some("utf-8")`, with `body` set to the UTF-8 text;
- `NativeScriptletBodyEncoding::Binary` maps to `body_encoding =
  Some("base64")`, with `body` set to base64-encoded original bytes;
- `body_sha256` must reuse `NativeScriptletBody.sha256`;
- after building the bundle, call `LegacyScriptletBundle::validate()` so
  tampered or mismatched bodies fail conversion tests immediately.

Flattened fallback body handling:

- encode `Scriptlet.content` as UTF-8;
- compute `body_sha256` with `crate::hash::sha256_prefixed`;
- preserve `Scriptlet.interpreter` and split `Scriptlet.flags` with whitespace
  into `interpreter_args` when present.

### Lifecycle And Order Mapping

Map native lifecycle values into bundle lifecycle strings and primary
`LifecyclePath` values.

Direct mappings:

| Native path | Bundle phase |
| --- | --- |
| `PreInstall` | `pre-install` |
| `PostInstall` | `post-install` |
| `PreUpgrade` | `pre-upgrade` |
| `PostUpgrade` | `post-upgrade` |
| `PreRemove` | `pre-remove` |
| `PostRemove` | `post-remove` |
| `PreTransaction` | `pre-transaction` |
| `PostTransaction` | `post-transaction` |
| `Trigger` | `trigger` |
| `FileTrigger` | `file-trigger` |

Review-only or approximated mappings:

| Native path | Bundle phase | Required decision |
| --- | --- | --- |
| `TransactionFileTrigger` | `file-trigger` | `review` unless blocked by support status |
| `PreUntransaction` | `pre-transaction` | `review` |
| `PostUntransaction` | `post-transaction` | `review` |
| `Verify` | `trigger` | `review` |
| `Config` | `post-install` | `review` |
| `Purge` | `post-remove` | `review` |
| `Abort` | `post-remove` | `review` |

The approximation is only for passive display/query compatibility. It must not
be interpreted as replay authority in Goal 4.

`NativeTransactionPosition` maps to `TransactionOrder.position` strings:

| Native position | Bundle string |
| --- | --- |
| `BeforePayload` | `before-payload` |
| `AfterPayload` | `after-payload` |
| `BeforeTransaction` | `before-transaction` |
| `AfterTransaction` | `after-transaction` |
| `Untransaction` | `untransaction` |
| `Verification` | `verification` |
| `Trigger` | `trigger` |
| `ControlArtifact` | `control-artifact` |

### Format-Specific Metadata

For RPM native metadata:

- copy trigger family/conditions/file globs into
  `LegacyScriptletEntry.rpm_trigger` when present, using a lossy projection
  table where native trigger metadata is richer than the v1 bundle fields;
- preserve scriptlet flags under `entry.extra["rpm_scriptlet_flags"]` until the
  bundle schema gets a first-class field;
- mark `%verify`, `%preuntrans`, `%postuntrans`, triggers, file triggers, and
  transaction file triggers as `review` unless an explicit blocked support
  status already made them blocked.

For DEB native metadata:

- copy `control_member`, trigger declarations, trigger names, await/noawait
  facts, and raw triggers content into `LegacyScriptletEntry.deb_maintainer`
  where the current v1 reserved fields allow it;
- store any invocation modes that do not fit existing reserved fields in
  `entry.extra["deb_maintainer_modes"]`;
- preserve each `DebTriggerDeclaration.raw_line` either through full
  `triggers_content` or, when individual declarations are projected, in
  `entry.extra["deb_trigger_raw_lines"]`;
- `config`, `triggers`, `purge`, and `abort-*` paths remain `review` in Goal 4.

For Arch native metadata:

- `.INSTALL` entries keep the full `.INSTALL` body from native ABI, not a
  detached function body;
- copy native `install_source_sha256` into
  `LegacyScriptletEntry.arch_install.install_digest`; copy called function and
  function-body extraction status into `arch_install` and `entry.extra` as
  needed;
- ALPM hooks remain `review` and keep the full hook file content in the body.
  ALPM hook details should be stored under `entry.extra["arch_alpm_hook"]`,
  not forced into `arch_install`.

Lossy or extra-field projections:

| Native fact | Bundle field |
| --- | --- |
| `NativeInvocationContract.args` | `native_invocation.args` as stable strings such as `1:new-version:required` |
| `NativeInvocationContract.environment` | `native_invocation.environment` as `NAME=value` or `NAME` |
| `NativeStdinContract` | `native_invocation.stdin` string (`none`, `debconf`, `paths`, `unknown`) |
| `NativeRootExpectation` | `native_invocation.chroot` string (`package-manager-default`, `install-root`, `host-root`, `unknown`) |
| `NativeScriptletKind` | `entry.extra["native_scriptlet_kind"]` |
| RPM native trigger family/conditions | best-effort `rpm_trigger.kind`, `target_constraints`, `file_globs`, plus raw details in `entry.extra["rpm_trigger_native"]` |
| DEB `control_member`, directives, await/noawait, raw lines | first-class `deb_maintainer` fields where possible, otherwise `entry.extra["deb_native"]` |
| Arch ALPM hook metadata | `entry.extra["arch_alpm_hook"]` |

## Decision Mapping

Bundle decisions are per native entry, while
`ScriptletClassificationReport.entries` are per evidence item. Group report
entries by `entry_id` before deciding.

Classification precedence for a bundle entry:

1. `NativeScriptletSupport::Unpreservable` or any `Blocked` classification makes
   the entry `blocked`.
2. `NativeScriptletSupport::DeferredReview`, any `Review` classification, or
   any `Unknown` classification makes the entry `review` unless already
   blocked.
3. If all classifications for the entry are `Known` and every known effect is
   `EffectReplacement::Complete`, mark the entry `replaced`.
4. Otherwise mark the entry `review`.

Goal 4 must not emit `ScriptletDecision::Legacy`. This intentionally narrows
the parent spec's generic decision workflow for Goal 4; `legacy` remains a
schema value for Goal 6+ replay decisions. Legacy replay is an install-time
capability and target-compatibility decision owned by later goals.
Preserved-but-unhandled bodies stay `review`.

`decision_counts.legacy` will be `0` in every Goal 4 bundle. The field is
required by schema v1 and should still serialize normally; nonzero legacy counts
are reserved for Goal 6+.

Reason-code selection:

| Decision | Reason code |
| --- | --- |
| `replaced` | the most specific complete helper reason when there is one, otherwise `scriptlet-entry-fully-replaced` |
| `review` due to unknown commands | `unknown-command` |
| `review` due to parser support | parser-provided reason code |
| `review` due to known review class | class reason code |
| `review` fallback | `scriptlet-review-required` |
| `blocked` due to blocked class | class reason code |
| `blocked` due to parser support | parser-provided reason code |
| `blocked` fallback | `scriptlet-blocked` |

`unknown_commands` should contain sorted unique command names from
`Unknown` classifications for that entry. `blocked_classes` should contain
sorted unique class IDs from `Blocked` classifications and any review classes
whose default outcome is blocked in the registry.

## Aggregate Metadata

Compute aggregate fields from the final entries:

| Condition | `scriptlet_fidelity` | `target_compatibility` | `publication_status` |
| --- | --- | --- | --- |
| no entries | `native-free` | `conary-portable` | `public` |
| every entry `replaced` | `fully-replaced` | `conary-portable` | `public` |
| any entry `blocked` | `blocked` | `blocked` | `blocked` |
| any entry `review` | `review-required` | `review-required` | `private-review` |
| mixed future decisions without blocked/review | `mixed` | `review-required` | `private-review` |

`scriptlet_fidelity = legacy-replay` is reserved for Goal 6+. Goal 4's bundle
builder must not produce it even though the schema enum already contains the
variant.

`publication_policy` should be:

- `public-if-no-blocked` for native-free and fully replaced packages;
- `blocked` when any entry is blocked;
- `private-review` for review-required and mixed packages.

`publication_policy = local-only` and `publication_status = local-only` are
reserved for future local-file workflows. Goal 4 must not emit either value.

`foreign_replay_policy` is always `deny` in Goal 4.

`allowed_targets` should be empty for `conary-portable` packages. For
`review-required` and `blocked` packages, include a best-effort source-native
target only when distro, release, and architecture are known. Empty
`allowed_targets` is acceptable and must not be treated as an allow-all list.

## Evidence Digest

`evidence_digest` must be deterministic and stable for identical conversion
inputs. Use the existing `conary_core::json::canonical_json` helper to
serialize a small internal digest document, then hash:

```text
crate::hash::sha256_prefixed(
    b"conary-scriptlet-evidence-v1\n" + canonical_json_bytes
)
```

The result must be a prefixed `sha256:<64 hex>` string because bundle
validation applies the same digest validation used by other CCS metadata.

The digest document should include:

- source format, source distro, source package, source version, normalized
  source checksum, and source architecture;
- sorted native entry IDs, native slots, body hashes, support statuses, and
  parser reason codes;
- flattened fallback entry IDs and body hashes when native ABI is absent;
- classification report counts;
- sorted classification reason codes, unknown commands, blocked/review class
  IDs, adapter IDs, adapter digests, effect kinds, and effect replacement
  values;
- final decision counts, `scriptlet_fidelity`, `target_compatibility`, and
  `publication_status`.

Do not include absolute temporary paths, output package paths, wall-clock
timestamps, or chunk hashes. Those make the digest environment-dependent.
`conary_core::json::canonical_json` sorts object keys but preserves array order,
so every vector in the digest document must be sorted and deduplicated before
serialization. Prefer `BTreeMap` and `BTreeSet` while assembling the digest
document.

`LegacyScriptletBundle.source_checksum` is optional and validates as a prefixed
SHA-256 value. Set it only when the caller provides a real
`sha256:<64 hex>` digest. If a test or local caller passes a placeholder such
as `sha256:test`, omit the bundle field but still normalize the digest document
deterministically.

Use the same digest for:

- `LegacyScriptletBundle.evidence_digest`;
- each entry's `evidence_digest` when entry-local digesting is not implemented
  yet;
- `ConversionResult.scriptlet_metadata.evidence_digest`;
- `converted_packages.evidence_digest`.

A future goal may split bundle, entry, and curation digests. Goal 4 keeps one
conversion evidence digest to avoid pretending curation exists.

## Remi Database Model

Add passive scriptlet metadata columns to `converted_packages` in a new schema
version after the current version:

| Column | Type/default | Notes |
| --- | --- | --- |
| `scriptlet_fidelity` | `TEXT NOT NULL DEFAULT 'unknown'` | Existing rows predate bundle metadata. |
| `target_compatibility` | `TEXT NOT NULL DEFAULT 'unknown'` | Existing rows are not retroactively trusted. |
| `publication_status` | `TEXT NOT NULL DEFAULT 'public'` | Preserves current serving behavior until Goal 5. |
| `evidence_digest` | `TEXT` | Conversion evidence digest. |
| `curation_evidence_digest` | `TEXT` | Reserved for Goal 5 curation/promotion evidence. |
| `blocked_reason_codes_json` | `TEXT NOT NULL DEFAULT '[]'` | Sorted unique blocked reason codes. |
| `scriptlet_summary_json` | `TEXT NOT NULL DEFAULT '{}'` | Serialized internal `ScriptletBundleSummary` with private path skipped. |
| `review_artifact_path` | `TEXT` | Reserved private review artifact path. Goal 4 may leave it `NULL`. |

`scriptlet_summary_json = '{}'` is valid because `ScriptletBundleSummary`
deserializes with `#[serde(default)]` and a custom `Default` implementation
matching the SQL defaults.

The queue listed the scalar columns and `review_artifact_path`, and explicitly
named `blocked_reason_codes_json`; keep that denormalized field available for
queries and future gates. Store the bulkier list/count data
(`decision_counts`, review reason codes, unknown commands, and blocked classes)
inside `scriptlet_summary_json` to avoid five parallel JSON-text columns that
are always consumed together by APIs. `blocked_reason_codes_json` and the
blocked reason list inside `scriptlet_summary_json` must be generated from the
same `ScriptletBundleSummary`; tests should assert they stay in sync.

`ConvertedPackage` should gain matching fields and default them in both
`new()` and `new_server()` so existing tests and non-Remi conversion paths keep
working. `from_row`, `COLUMNS`, `insert`, and identity lookups must include the
new fields.

Keep the existing `ConvertedPackage::new()` and
`ConvertedPackage::new_server()` signatures. There are many call sites across
Remi, CLI conversion, and tests; adding scriptlet parameters would create
mechanical churn for callers whose correct value is the default. Constructors
should initialize the new fields to the SQL defaults. Production conversion
code that has a real `ScriptletBundleSummary` should assign fields after
construction or use a focused setter such as:

```rust
impl ConvertedPackage {
    pub fn set_scriptlet_metadata(
        &mut self,
        summary: &ScriptletBundleSummary,
    ) -> serde_json::Result<()> {
        self.scriptlet_fidelity = summary.scriptlet_fidelity.clone();
        self.target_compatibility = summary.target_compatibility.clone();
        self.publication_status = summary.publication_status.clone();
        self.evidence_digest = summary.evidence_digest.clone();
        self.curation_evidence_digest = summary.curation_evidence_digest.clone();
        self.blocked_reason_codes_json = serde_json::to_string(
            &summary.blocked_reason_codes,
        )?;
        self.scriptlet_summary_json = serde_json::to_string(summary)?;
        self.review_artifact_path = summary.review_artifact_path.clone();
        Ok(())
    }
}
```

SQLite `ALTER TABLE ADD COLUMN` appends the new columns. Append them to
`ConvertedPackage::COLUMNS` and extend `from_row` with new `row.get(N)` calls at
the end so positional indices for existing fields stay unchanged.

`CONVERSION_VERSION` should be bumped because Remi artifacts produced before
Goal 4 lack embedded `legacy_scriptlets` and scriptlet DB metadata. The bump
lets Remi reconvert instead of serving stale Goal 3b artifacts as if they had
Goal 4 metadata.

That version bump is only effective where Remi checks it. Every Remi query over
`converted_packages` that serves, advertises, indexes, or computes from
converted artifacts must either filter `conversion_version >=
CONVERSION_VERSION` or reject rows where `needs_reconversion()` is true. This
includes package metadata/download paths, `/v1/:distro/metadata`, generated
indexes, sparse metadata, OCI manifest/tag/catalog/digest lookups, and delta
manifest queries.

Avoid duplicating `ConvertedPackage` row mapping. The OCI digest lookup
currently constructs a manual `ConvertedPackage { ... }` literal; Goal 4 should
replace that path with a model helper, such as
`ConvertedPackage::find_by_content_hash_identity(...)`, or expose a shared row
mapper that uses `ConvertedPackage::COLUMNS`. New scriptlet fields should be
centralized in the model instead of copied into handler literals.

`LegacyScriptletBundle.unsupported_class_counts` remains bundle-only in Goal 4.
The compact `ScriptletBundleSummary` carries flattened `blocked_classes` for
API display, but it does not duplicate the full count map.

## Remi API Model

Add a serializable summary type in Remi or re-export a core summary type:

```rust
pub struct ScriptletPackageMetadata {
    pub scriptlet_fidelity: String,
    pub target_compatibility: String,
    pub publication_status: String,
    pub evidence_digest: Option<String>,
    pub curation_evidence_digest: Option<String>,
    pub decision_counts: ScriptletDecisionCountsSummary,
    pub blocked_reason_codes: Vec<String>,
    pub review_reason_codes: Vec<String>,
    pub unknown_commands: Vec<String>,
    pub blocked_classes: Vec<String>,
    pub review_artifact_available: bool,
}
```

Expose this under a `scriptlets` field on:

- `apps/remi/src/server/handlers/packages.rs::PackageManifest`;
- converted `PackageEntry.metadata` in `/v1/:distro/metadata`;
- `apps/remi/src/server/index_gen.rs::VersionEntry`.

Existing clients that ignore unknown JSON fields remain compatible. Goal 4
should not remove or rename any existing field.

`ScriptletBundleSummary` derives `Serialize` for DB JSON storage only. It must
not be embedded directly in public API response types. Add a doc comment on the
struct that says: "Internal summary type. Do not serialize directly in public
API responses."

When adding scriptlet metadata to `PackageEntry.metadata`, merge it into any
existing metadata object under `metadata["scriptlets"]`. Do not replace native
metadata such as provides or repository-derived facts.

The current `/v1/:distro/metadata` flow marks repo-backed entries converted via
a set and separately appends converted-only entries. Goal 4 needs a richer
converted-row structure keyed by `(name, version, architecture)`, or a keyed
metadata map plus converted-only entries, so repo-backed packages can merge
scriptlet metadata from their converted rows while preserving repository
metadata.

`apps/remi/src/server/handlers/index.rs::build_converted_packages` must expand
its current query. It currently selects only package identity and format fields:

```sql
SELECT package_name, package_version, package_architecture, original_format
FROM converted_packages
```

Goal 4 must select the scriptlet metadata columns needed to build the
`metadata["scriptlets"]` object, including `scriptlet_fidelity`,
`target_compatibility`, `publication_status`, `evidence_digest`,
`curation_evidence_digest`, `blocked_reason_codes_json`,
`scriptlet_summary_json`, and `review_artifact_path`. Without that query
expansion, `/v1/:distro/metadata` cannot expose the new API field.

Public package and index responses may expose whether a private review artifact
exists, but they must not expose `review_artifact_path`. Goal 5 or an admin-only
handler can introduce authenticated review artifact retrieval. Tests should seed
a path such as `/tmp/private-review-secret` and assert package manifest,
`/v1/:distro/metadata`, and generated index JSON do not contain it.

For rows with default `'unknown'` scriptlet metadata, include the `scriptlets`
field only when a package is known converted and the row exists. The summary may
contain `scriptlet_fidelity = "unknown"` for stale rows until reconversion.

Download endpoints such as `converted_ccs_path_for_download` must keep their
current behavior in Goal 4. They may read the new metadata for logging later,
but they must not refuse paths based on `publication_status` yet.

## CCS Manifest Embedding

`LegacyConverter::convert()` should:

1. classify scriptlets as it does today;
2. build the manifest with the final metadata/files/hooks;
3. build the legacy scriptlet bundle using the final metadata/files and the
   classification report;
4. assign `manifest.legacy_scriptlets = Some(bundle.clone())`;
5. validate the bundle through manifest validation/build;
6. write the CCS package normally.

The bundle remains TOML-only inside `ccs.toml`. Goal 4 should not introduce a
sidecar file. The archive overlay and query code already know how to preserve
and render TOML-side `legacy_scriptlets`. This intentionally chooses the
manifest-embedding path and supersedes the goal queue's broader allowance for a
referenced sidecar.

The insertion point in the current converter flow is after capabilities are
attached to the manifest and before TOML serialization:

```rust
let scriptlet_classification = classify_scriptlets(metadata, files);
// capture/analyzer/inference work...
let mut manifest = self.build_manifest(&final_metadata, &final_files, &detected_hooks)?;
manifest.capabilities = inferred_capabilities
    .as_ref()
    .map(InferredCapabilities::to_declaration);

let scriptlet_bundle = build_legacy_scriptlet_bundle(ScriptletBundleInput {
    source_metadata: metadata,
    final_metadata: &final_metadata,
    source_files: files,
    final_files: &final_files,
    source_format: format,
    source_distro: self.source_distro.as_deref(),
    source_release: self.source_release.as_deref(),
    source_arch: final_metadata.architecture.as_deref(),
    source_checksum: Some(checksum),
    classification: &scriptlet_classification,
    conversion_tool: self.conversion_tool.as_str(),
    conversion_tool_version: env!("CARGO_PKG_VERSION"),
})
.map_err(|error| ConversionError::ManifestError(error.to_string()))?;
scriptlet_bundle
    .bundle
    .validate()
    .map_err(|error| ConversionError::ManifestError(error.to_string()))?;
manifest.legacy_scriptlets = Some(scriptlet_bundle.bundle.clone());

let manifest_toml = toml::to_string_pretty(&manifest)?;
```

`ConversionResult.legacy_scriptlets` and
`ConversionResult.scriptlet_metadata` should be populated from
`scriptlet_bundle` at the final return site.

## Passive Publication Semantics

`publication_status` in Goal 4 is a report, not a gate:

- `public` means the conversion evidence is compatible with future public
  publication policy.
- `private-review` means the converted artifact needs operator review before
  a future publication gate should treat it as public-ready.
- `blocked` means the converted artifact carries a structured negative
  scriptlet result.
- `local-only` remains reserved for future local-file workflows.

Remi must still behave as it does before Goal 4 when a client asks for a
converted package. Goal 5 changes serving behavior so review/blocked packages
are not advertised or served as ready public artifacts.

This deliberate split keeps the migration observable before it becomes
enforcing.

## Error Handling

Bundle-building failures should fail the conversion before writing a new CCS
artifact or inserting a converted-package row. Examples:

- invalid source format mapping;
- missing required bundle fields;
- duplicate entry IDs;
- malformed base64 body encoding;
- strict body-hash mismatch;
- non-serializable digest document;
- database metadata JSON serialization failure.

Remi should surface those failures through the existing conversion job failure
path. Goal 4 does not introduce partial DB rows for failed bundle generation.

When existing rows have malformed JSON in a new summary field, API builders
should log a warning and hydrate scalar fields from the dedicated columns
(`scriptlet_fidelity`, `target_compatibility`, `publication_status`,
`evidence_digest`, `curation_evidence_digest`, and
`blocked_reason_codes_json`). Default only list/count fields that exist solely
inside `scriptlet_summary_json`; do not turn a scalar `blocked` row into an
`unknown`/`public` response.

## Testing Strategy

Core converter and bundle tests:

- native-free conversion embeds a zero-entry bundle and summary metadata;
- a payload-backed complete-helper fixture embeds a `fully-replaced` bundle with
  `replaced` entries and complete effects copied from adapter evidence;
- review fixtures for DEB private helpers and parser-deferred native entries
  embed `review` entries with unknown/review reason codes;
- blocked fixtures embed `blocked` entries with blocked reason codes and class
  IDs;
- non-UTF-8 native ABI bodies are base64 encoded and still validate;
- tampering with an embedded body after construction fails
  `LegacyScriptletBundle::validate()`;
- archive round trip preserves the embedded bundle;
- previous Goal 3b scope-guard tests are updated to assert passive bundle
  presence rather than absence, while still asserting existing detected hooks
  are preserved.
- direct `conary_core::ccs::convert::ConversionResult` struct literals are
  updated wherever they exist, especially Remi test helpers.

Database tests:

- migration adds the new columns with defaults for existing rows;
- `ConvertedPackage::new()` and `new_server()` default summary fields
  correctly;
- insert/find/list round trip all new fields;
- `needs_reconversion()` returns true for pre-Goal-4 conversion versions.

Remi tests:

- cold conversion stores scriptlet metadata in `converted_packages`;
- hot conversion result returns metadata from the existing row;
- `apps/remi/src/server/conversion.rs::make_conversion_result` initializes the
  new `ConversionResult` fields with `legacy_scriptlets: None` and
  `scriptlet_metadata: ScriptletBundleSummary::default()`;
- `GET /v1/:distro/packages/:name` includes `scriptlets` metadata for converted
  packages;
- `/v1/:distro/metadata` includes scriptlet metadata for converted package
  entries without hiding review/blocked rows;
- generated repository index includes scriptlet metadata on converted
  `VersionEntry` values;
- download path behavior remains unchanged for `private-review` and `blocked`
  metadata until Goal 5.
- malformed `scriptlet_summary_json` or reason-code JSON on an existing row logs
  a warning and returns default scriptlet metadata instead of failing the API
  request;
- stale conversion timing text in
  `apps/remi/src/server/conversion.rs` no longer says the adapter registry is
  unimplemented.

Additional focused tests:

- `ScriptletBundleSummary::default()` reports `scriptlet_fidelity = "unknown"`,
  `target_compatibility = "unknown"`, and `publication_status = "public"`;
- `ConvertedPackage` insert/find/list round trips non-default scriptlet
  metadata, proving `COLUMNS` and `from_row` positional indices include every
  new column;
- a pre-Goal-4 converted-package row with `conversion_version = 3` returns true
  from `needs_reconversion()` after the version bump;
- an RPM native ABI entry with scriptlet flags embeds those flags in
  `entry.extra["rpm_scriptlet_flags"]` and preserves them through TOML
  round trip;
- conversion fails before writing a CCS package when bundle validation fails.

Verification commands:

```bash
cargo test -p conary-core legacy_scriptlets
cargo test -p conary-core conversion_integration
cargo test -p conary-core converted_package
cargo test -p conary
cargo test -p remi conversion
cargo test -p remi packages
cargo test -p remi index
cargo test -p remi oci
cargo test -p remi sparse
cargo test -p remi delta_manifests
cargo test -p remi routes
cargo test -p remi
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

## Review Constraints Incorporated

This design intentionally incorporates the review constraints from the
DeepSeek, Gemini, and internal agent passes:

- passive metadata first, enforcement later;
- no public serialization of review artifact paths;
- no `legacy` decisions or `legacy-replay` fidelity until replay exists;
- native ABI entries are preferred over flattened compatibility projections;
- malformed summary JSON hydrates from scalar columns before defaulting lists;
- constructor signatures stay stable and scriptlet metadata is set separately;
- version bumps must be backed by stale-row filtering on every Remi serving and
  indexing path.

## Review Questions

Ask reviewers to focus on:

1. Is the passive/enforcing split clear enough, especially around
   `publication_status`?
2. Is it correct to emit no `legacy` decisions in Goal 4 and keep all
   preserved-but-unhandled entries in `review`?
3. Are the added Remi DB summary columns justified, or should unknown/review
   metadata be stored only in a generic JSON blob?
4. Is bumping `CONVERSION_VERSION` the right way to avoid stale converted CCS
   artifacts without a separate Remi cache migration?
5. Does the native ABI to bundle mapping preserve enough RPM, DEB, and Arch
   format-specific metadata for later replay/review goals?
6. Are package metadata and repository index API additions sufficient without
   changing download/publication behavior?
