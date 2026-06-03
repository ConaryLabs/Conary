---
last_updated: 2026-05-27
revision: 1
summary: Goal 1 design for the passive Legacy Scriptlet Semantics Bundle schema, CCS manifest/archive round trips, and query-only operator surface
---

# Legacy Scriptlet Bundle Schema V1 And Passive Query Design

**Date:** 2026-05-27
**Status:** Draft for external model review
**Parent spec:** `docs/superpowers/specs/2026-05-27-legacy-scriptlet-semantics-bundle-design.md`
**Goal queue:** `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`

## Purpose

Goal 1 turns the clean-room legacy scriptlet semantics design into a concrete
passive data model. The work must let a CCS package carry complete scriptlet
decision metadata and let an operator inspect that metadata, while changing no
install, update, remove, replay, Remi publication, or conversion behavior.

This is the "metadata foundation" goal. If it succeeds, later goals can fill
the bundle from native ABI extraction, run adapters, gate publication, and
eventually replay legacy scriptlets without revisiting the manifest shape.

## Scope

Goal 1 includes:

- a `LegacyScriptletBundle` Rust data model in `conary-core`;
- TOML serialization and deserialization through `CcsManifest`;
- CCS archive read/write preservation through `MANIFEST.toml`;
- schema validation that catches malformed bundles early;
- passive `conary query scripts <pkg>` rendering for CCS bundles;
- stable JSON output for future tests and support bundles.

Goal 1 explicitly excludes:

- extracting native ABI entries from RPM, DEB, or Arch packages;
- generating bundles during Remi conversion;
- adapter registry decisions;
- publication gating;
- legacy replay;
- target compatibility enforcement;
- database migrations.

Even though Goal 1 does not add a database migration, later replay goals must
persist the complete bundle into local package state during install. Remove and
upgrade operations may occur after the original `.ccs` archive is gone, so the
replay engine cannot rely on archive lookup or the older raw scriptlet table
alone. Goals 6 and 7 must add local state storage that preserves target
compatibility, sandbox requirements, per-entry decisions, timeouts, and evidence
for installed troves.

## Current Code Constraints

`CcsManifest` is the TOML-facing manifest type in
`crates/conary-core/src/ccs/manifest.rs`. CCS packages include both compact CBOR
`MANIFEST` data and human-readable `MANIFEST.toml`. The CBOR `BinaryManifest`
does not carry every TOML field, and `archive_reader.rs` already overlays
TOML-only fields such as `scriptlets`, `policy`, `provenance`, `redirects`, and
`legacy` after decoding CBOR.

Goal 1 should use that existing pattern. The legacy scriptlet bundle is a
TOML-only manifest field in schema v1. `MANIFEST.toml` is already protected by
the binary manifest's TOML integrity hash when package verification is used, so
Goal 1 does not need to expand the CBOR manifest.

`conary query scripts <pkg>` already exists, but today it inspects native RPM,
DEB, or Arch package files. Goal 1 extends the same command to recognize CCS
packages with a bundle and render the passive metadata. Native package behavior
should remain intact.

Goal 1 must add CCS detection in the `cmd_scripts` path. CCS packages normally
use a `.ccs` extension and are tar archives with CCS manifests inside. Detection
should check `.ccs` extension first and parse through the CCS package reader. If
the extension is `.ccs` but parsing fails, propagate the CCS parse error instead
of silently falling back to native RPM/DEB/Arch detection.

## Design Decisions

### 1. Add A Dedicated CCS Module

Create `crates/conary-core/src/ccs/legacy_scriptlets.rs` and export it from
`crates/conary-core/src/ccs/mod.rs`.

The module owns:

- bundle structs;
- type-safe enums with unknown variants and validation helpers;
- TOML round-trip tests;
- small display/summary helpers used by the CLI.

This keeps the large schema out of `manifest.rs` and prevents scriptlet bundle
logic from bleeding into the existing transitional `ScriptletDeclarations`
capability model.

### 2. Embed The Bundle As `CcsManifest.legacy_scriptlets`

Add:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub legacy_scriptlets: Option<LegacyScriptletBundle>,
```

to `CcsManifest`.

Do not reuse:

- `scriptlets: ScriptletDeclarations`, which declares package-required
  scriptlet capabilities;
- `hooks.post_install` or `hooks.pre_remove`, which are native CCS or
  transitional arbitrary script hooks;
- `legacy`, which is existing conversion/provenance metadata, not scriptlet ABI.

Converted legacy package scriptlets must eventually be represented by
`legacy_scriptlets`. Goal 1 only creates the container.

### 3. Preserve The Bundle Through Archive Paths

`package_writer.rs` already writes `MANIFEST.toml`, so the bundle is preserved
on write once it is part of `CcsManifest`.

`archive_reader.rs` must overlay `toml.legacy_scriptlets` onto the
CBOR-converted manifest when both `MANIFEST` and `MANIFEST.toml` are present.
This mirrors the existing overlays for `scriptlets`, `policy`, `provenance`,
`redirects`, and `legacy`.

The binary manifest conversion note in `package.rs` should list
`legacy_scriptlets` among TOML-only fields.

### 4. Keep Query Passive And File-Based

For Goal 1, `conary query scripts <pkg>` treats `<pkg>` as a package file path.
When the path is a CCS package:

- if the manifest has `legacy_scriptlets`, render the bundle;
- if the manifest has no bundle, print a concise "no legacy scriptlet bundle"
  message and exit successfully unless `--entry` requested a specific bundle
  entry;
- do not resolve installed packages from the state DB.

When the path is RPM, DEB, or Arch, preserve the current native scriptlet output
unless new flags require structured rendering.

### 5. Use Type-Safe Enums With Unknown Variants

Goal 1 should preserve unknown optional TOML fields where practical by using
`#[serde(flatten)] extra: BTreeMap<String, toml::Value>` on the bundle, entries,
effects, and reserved metadata structs.

Known safety-critical values such as `decision`, `target_compatibility`,
`foreign_replay_policy`, and `publication_policy` should parse into distinct
type-safe enums, each with an `Unknown(String)` variant. A generic string
wrapper would preserve bytes, but it would let developers accidentally compare
unrelated domains such as publication status and target compatibility. Unknown
values are valid for passive query, but helper methods must treat them as
non-actionable. Later install/replay goals will fail closed on non-actionable
values.

## Bundle Schema V1

The TOML section is named `[legacy_scriptlets]`. The schema identifier is:

```toml
schema = "conary.legacy-scriptlets.v1"
schema_revision = 1
```

### Top-Level Fields

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `schema` | string | yes | Must equal `conary.legacy-scriptlets.v1` for v1. |
| `schema_revision` | integer | yes | Starts at `1`; additive changes can increase this within v1. |
| `source_format` | enum string | yes | `rpm`, `deb`, `arch`, or retained unknown string. |
| `source_family` | string | yes | Ecosystem family such as `fedora-rhel`, `debian-ubuntu`, or `arch-alpm`. |
| `source_distro` | string | no | Distro identifier when known, such as `fedora` or `ubuntu`. |
| `source_release` | string | no | Source release when known, such as `44` or `26.04`. |
| `source_arch` | string | no | Source architecture when known, such as `x86_64`. |
| `source_package` | string | yes | Native source package name. |
| `source_version` | string | yes | Native source package version string. |
| `source_checksum` | string | no | Digest of the source package, preferably `sha256:<hex>`. |
| `version_scheme` | enum string | yes | `rpm`, `deb`, `arch`, `semver`, or unknown retained string. |
| `conversion_tool` | string | yes | Tool name that produced the bundle, normally `remi`. |
| `conversion_tool_version` | string | yes | Conversion tool version or git revision. |
| `conversion_policy` | string | yes | Policy identifier used during conversion. |
| `adapter_registry_digest` | string | no | Digest of adapter registry inputs when adapters exist. |
| `target_policy_digest` | string | no | Digest of target policy inputs when compatibility gates exist. |
| `evidence_digest` | string | no | Digest covering bundle evidence artifacts. |
| `target_compatibility` | enum string | yes | `source-native`, `family-compatible`, `conary-portable`, `review-required`, or `blocked`. |
| `allowed_targets` | array of strings | no | Explicit target IDs such as `rpm/fedora/44/x86_64`. |
| `foreign_replay_policy` | enum string | yes | Default must be `deny`. |
| `publication_policy` | enum string | yes | Public cache policy chosen by conversion. |
| `publication_status` | enum string | yes | `public`, `private-review`, `blocked`, or `local-only`. |
| `scriptlet_fidelity` | enum string | yes | Aggregate result such as `native-free`, `fully-replaced`, `legacy-replay`, `mixed`, `review-required`, or `blocked`. |
| `decision_counts` | table | yes | Counts for `replaced`, `legacy`, `blocked`, `review`, and any unknown future decision keys. |
| `unsupported_class_counts` | table | no | Count by blocked-class ID. |
| `entries` | array of tables | no, default empty | One entry per native scriptlet slot, including blocked/deferred slots. Empty means the package is native-free and has no legacy scriptlet entries. |

### Entry Fields

Each entry appears under `[[legacy_scriptlets.entries]]`.

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `id` | string | yes | Stable bundle-local ID, for example `rpm:%post` or `deb:postinst:configure`. |
| `native_slot` | string | yes | Native slot name such as `%post`, `postinst`, or `.INSTALL:post_install`. |
| `phase` | enum string | yes | Normalized Conary phase. |
| `lifecycle_paths` | array of strings | yes | Native call paths modeled by the entry. |
| `interpreter` | string | yes | Interpreter path or logical interpreter. |
| `interpreter_args` | array of strings | no | Native interpreter flags. |
| `body_sha256` | string | yes | Digest of preserved body bytes. |
| `body` | string | yes | Preserved script body or wrapper source context. |
| `body_encoding` | string | no | Defaults to `utf-8`; allows `base64` for non-UTF-8 bodies. |
| `native_invocation` | table | yes | Native arguments, environment notes, stdin contract, and chroot/root behavior. |
| `transaction_order` | table | yes | Ordering relative to payload mutation and transaction boundaries. |
| `timeout_ms` | integer | yes | Replay timeout selected by conversion policy. |
| `sandbox` | table | no | Required sandbox features and policy hints. |
| `capabilities` | array of strings | no | Required host integration capabilities. |
| `decision` | enum string | yes | `replaced`, `legacy`, `blocked`, or `review`. |
| `reason_code` | string | yes | Stable machine-readable reason ID. |
| `human_reason` | string | no | Operator-facing explanation. |
| `evidence_digest` | string | no | Entry evidence digest. |
| `source_evidence_refs` | array of strings | no | References into conversion evidence artifacts. |
| `effects` | array of tables | no | Effect IR produced by metadata, capture, or adapters. |
| `unknown_commands` | array of strings | no | Static or captured commands that adapters did not understand. |
| `blocked_classes` | array of strings | no | Blocked class IDs such as `network`, `pam`, or `initramfs`. |
| `rpm_trigger` | table | no | Reserved RPM trigger/file-trigger metadata. |
| `deb_maintainer` | table | no | Reserved DEB invocation, trigger, purge, and abort metadata. |
| `arch_install` | table | no | Reserved Arch `.INSTALL` function metadata. |
| `residual_replay` | table | no | Reserved metadata for future partial-replay supersession. |

### Effect Fields

Each effect appears under an entry's `effects` array.

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `kind` | string | yes | Effect kind such as `ldconfig`, `systemd-daemon-reload`, `tmpfiles`, or `unknown`. |
| `source` | enum string | yes | `native-metadata`, `payload-heuristic`, `capture-log`, `wrapper-observation`, `curated-rule`, or `static-signal`. |
| `confidence` | enum string | yes | `declared`, `observed`, `inferred`, or `uncertain`. |
| `replacement` | enum string | yes | `complete`, `partial`, `none`, or `blocked`. |
| `adapter_id` | string | no | Adapter that emitted the effect. |
| `adapter_digest` | string | no | Digest of adapter implementation or rule set. |
| `command` | string | no | Original helper command when applicable. |
| `args` | array of strings | no | Original helper arguments when applicable. |
| `path` | string | no | Original path or payload path when applicable. |
| `reason_code` | string | no | Effect-level reason ID. |

### Reserved Metadata

Goal 1 must include reserved structs and round-trip tests even before later
goals populate them.

RPM reserved metadata:

- trigger kind;
- trigger condition;
- trigger target package constraints, including package name, operator, and
  version;
- trigger priority;
- file-matching glob patterns;
- stdin contract;
- transaction ordering marker.

DEB reserved metadata:

- maintainer-script invocation mode;
- old version;
- new version;
- trigger control-file content;
- trigger names;
- purge path marker;
- abort path marker;
- noninteractive expectation.

Arch reserved metadata:

- original `.INSTALL` digest;
- called function;
- old version argument;
- new version argument;
- wrapper source digest.

Residual replay reserved metadata:

- superseded effect kinds;
- wrapper strategy;
- suppression markers;
- residual body digest.

## Query Contract

Add flags to `conary query scripts <pkg>`:

```text
conary query scripts <pkg>
conary query scripts <pkg> --verbose
conary query scripts <pkg> --entry <entry-id>
conary query scripts <pkg> --json
```

Default CCS bundle output should be concise:

```text
Package: nginx 1.28.0
Legacy scriptlet bundle: conary.legacy-scriptlets.v1
Source: rpm fedora 44 x86_64
Compatibility: source-native
Foreign replay: deny
Fidelity: mixed
Entries: 4 replaced, 1 legacy, 0 blocked, 0 review

rpm:%post        legacy    post-install    reason=protected-replay-required
rpm:%preun       replaced  pre-remove      reason=systemd-hook-complete
```

`--verbose` adds interpreter, timeout, lifecycle paths, blocked classes,
unknown commands, effects, adapter IDs, evidence digests, and body digests. For
CCS bundles, raw preserved scriptlet bodies are not printed by default in either
text or JSON output. A future `--include-body` flag can add full-body output if
operator debugging needs it. Existing native RPM/DEB/Arch inspection behavior
is not changed by this CCS bundle policy.

In Goal 1, `--verbose`, `--entry`, and `--json` are CCS bundle features.
Default native RPM/DEB/Arch output remains unchanged. `--verbose` on native
packages may behave as the current detailed native output; `--entry` and
`--json` on native packages should fail with a clear message that those modes
are only defined for CCS legacy scriptlet bundles.

`--entry <entry-id>` filters to one entry and exits with a non-zero error if the
entry is not present.

When a bundle has zero entries, `conary query scripts` should print a concise
"No legacy scriptlet entries. This package does not require native scriptlet
replay." message and exit 0. `--entry` against a zero-entry bundle should still
return a non-zero not-found error.

`--json` emits a stable report type rather than ad hoc printed text. The report
should include:

- package identity;
- whether a bundle was present;
- bundle summary;
- entries after optional filtering;
- warnings for unknown non-actionable enum values;
- no raw host logs or environment details.

For a CCS package with no bundle, `--json` should emit
`bundle_present: false`, `bundle: null`, `entries: []`, and exit 0 unless
`--entry` requested a missing bundle entry. For a zero-entry bundle, `--json`
should emit `bundle_present: true` with an empty `entries` array.

## Validation Rules

`CcsManifest::validate()` should call `LegacyScriptletBundle::validate()` when
the bundle is present.

Minimum v1 validation:

- schema equals `conary.legacy-scriptlets.v1`;
- schema revision is non-zero;
- source format, source package, source version, conversion tool, conversion
  policy, target compatibility, foreign replay policy, publication status, and
  scriptlet fidelity are present;
- entry IDs are unique;
- every entry has a native slot, phase, interpreter, body digest, preserved
  body, transaction order, timeout, decision, and reason code;
- `body_sha256` must use the `sha256:<64 hex>` shape and must match the
  SHA-256 of the preserved body bytes. For `body_encoding = "base64"`, validate
  the digest against the decoded bytes. For UTF-8 bodies, validate against
  `body.as_bytes()`;
- `body_encoding` may be absent, `utf-8`, or `base64`. Unknown encodings are
  rejected because Conary cannot safely validate the preserved bytes;
- other digest fields use the `sha256:<64 hex>` shape when present;
- decision counts match entry decisions. Unknown decision keys are allowed for
  forward compatibility, but the sum of all counts, known and unknown, must
  equal the total entry count;
- `timeout_ms` is greater than zero;
- `allowed_targets` use `<format>/<distro>/<release>/<arch>` when present.

Validation should not require future trigger or adapter fields to be populated.
It should preserve unknown optional fields and report unknown action enum values
as warnings for query, not as install approval.

Bundle validation errors must be mapped into the existing `ManifestError` type
when called from `CcsManifest::validate()`. Goal 1 must not change
`CcsManifest::validate()` away from `Result<(), ManifestError>`.

## Safety Properties

Goal 1 must keep these properties true:

- no install, update, remove, adoption, or unadoption path reads the bundle for
  behavior;
- no legacy scriptlet body is executed;
- no bundle decision can make a package eligible for publication;
- no target compatibility claim can permit foreign replay;
- no arbitrary `ScriptHook` field is generated from native scriptlets.

The CCS bundle query surface should not display raw preserved scriptlet bodies
by default. Text and JSON output should include body digests and metadata, not
full bodies, until an explicit `--include-body` flag is designed.

## Test Requirements

`conary-core` tests:

- TOML round trip preserves top-level bundle fields;
- TOML round trip preserves entries, effects, decision counts, and digests;
- reserved RPM trigger metadata round trips;
- reserved DEB purge/trigger metadata round trips;
- reserved Arch `.INSTALL` metadata round trips;
- unknown optional fields are preserved;
- malformed digest, tampered body hash, duplicate entry ID, zero timeout, and
  mismatched decision counts fail validation;
- archive reader preserves `legacy_scriptlets` when both CBOR `MANIFEST` and
  `MANIFEST.toml` are present.

`conary` tests:

- `query scripts` preserves existing native RPM/DEB/Arch behavior;
- `query scripts` renders a CCS bundle summary;
- `--verbose` shows entry detail without changing behavior;
- `--entry` filters one entry and errors when absent;
- `--json` emits stable JSON for bundle-present and bundle-absent packages.

## Review Questions

1. Which future goal, if any, should consider adding CBOR-level bundle access,
   given that Goal 1 intentionally uses TOML-only storage?
2. Are the required top-level fields sufficient for later Remi publication
   gates, or should private review artifact pointers be reserved now?
3. Should a future debug UX add an explicit `--include-body` flag, or should raw
   CCS bundle bodies stay out of query output long-term?
4. Are unknown safety-critical enum values better represented as typed
   `Unknown(String)` variants with non-actionable helpers, or should manifest
   parsing reject them?
5. Does the `allowed_targets` identifier format
   `<format>/<distro>/<release>/<arch>` cover preview targets cleanly enough for
   Fedora, Ubuntu, and Arch?
