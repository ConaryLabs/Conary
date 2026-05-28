# Legacy Scriptlet Passive Remi Bundle Embedding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement Goal 4 by embedding passive legacy scriptlet bundles into converted CCS manifests and exposing Remi scriptlet fidelity metadata without changing install, update, remove, or publication enforcement behavior.

**Architecture:** Add a conversion-local `scriptlet_bundle` bridge that turns parser metadata, payload hints, and `ScriptletClassificationReport` into a validated `LegacyScriptletBundle` plus compact summary. Thread that bundle through `LegacyConverter`, persist summary fields on `converted_packages`, and expose a public scriptlet summary in package, metadata, and generated-index responses while stale conversion rows are filtered from all Remi serving/indexing paths.

**Tech Stack:** Rust, `serde`, `serde_json`, `toml`, `rusqlite`, SQLite migrations, Conary CCS TOML manifest schema, Remi Axum handlers, existing `conary_core::json::canonical_json`, and `crate::hash::sha256_prefixed`.

---

## Source Context

Read before implementation:

- `AGENTS.md`
- `docs/superpowers/specs/2026-05-28-legacy-scriptlet-passive-remi-bundle-embedding-design.md`
- `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`
- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-semantics-bundle-design.md`
- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-bundle-schema-v1-passive-query-design.md`
- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-native-abi-extraction-design.md`
- `docs/superpowers/specs/2026-05-28-legacy-scriptlet-adapter-registry-blocked-classes-design.md`
- `docs/superpowers/specs/2026-05-28-legacy-scriptlet-bootstrap-adapters-design.md`
- `crates/conary-core/src/ccs/legacy_scriptlets.rs`
- `crates/conary-core/src/ccs/convert/converter.rs`
- `crates/conary-core/src/ccs/convert/effects.rs`
- `crates/conary-core/src/packages/native_abi.rs`
- `crates/conary-core/src/db/models/converted.rs`
- `crates/conary-core/src/db/schema.rs`
- `crates/conary-core/src/db/migrations/v41_current.rs`
- `apps/remi/src/server/conversion.rs`
- `apps/remi/src/server/handlers/packages.rs`
- `apps/remi/src/server/handlers/detail.rs`
- `apps/remi/src/server/handlers/index.rs`
- `apps/remi/src/server/index_gen.rs`
- `apps/remi/src/server/handlers/oci.rs`
- `apps/remi/src/server/delta_manifests.rs`
- `docs/modules/remi.md`

## Scope Rules

- Do not change install, update, remove, hook execution, or scriptlet replay behavior.
- Do not gate Remi downloads or metadata by `publication_status` in Goal 4.
- Do not add sidecar bundle storage. The bundle lives only in `ccs.toml`.
- Do not emit `ScriptletDecision::Legacy`, `scriptlet_fidelity = "legacy-replay"`, or `publication_status = "local-only"`.
- Do not expose raw `review_artifact_path` values in public JSON.
- Keep `ConvertedPackage::new()` and `ConvertedPackage::new_server()` signatures stable.
- Prefer native ABI entries over flattened `Scriptlet` entries when both are present.

## File Structure

Create:

- `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`
  - Builds `LegacyScriptletBundle`.
  - Defines `ScriptletBundleInput`, `ScriptletBundleBuild`, `ScriptletBundleSummary`, and `ScriptletDecisionCountsSummary`.
  - Owns deterministic evidence digest construction.
  - Owns entry projection from native ABI and flattened fallback scriptlets.

Modify:

- `crates/conary-core/src/ccs/convert/mod.rs`
  - Export the new module and public bundle builder types.
- `crates/conary-core/src/ccs/convert/converter.rs`
  - Add converter context setters.
  - Embed the bundle before manifest TOML serialization.
  - Add `legacy_scriptlets` and `scriptlet_metadata` to core `ConversionResult`.
  - Update existing Goal 3b tests that asserted bundle absence.
- `crates/conary-core/src/db/schema.rs`
  - Bump `SCHEMA_VERSION` and route migration version 70.
- `crates/conary-core/src/db/migrations/v41_current.rs`
  - Add migration v70 for passive scriptlet metadata columns.
  - Extend migration tests to verify new columns.
- `crates/conary-core/src/db/models/converted.rs`
  - Bump `CONVERSION_VERSION`.
  - Add new fields, constructor defaults, `set_scriptlet_metadata`, `scriptlet_summary`, row mapping, insert SQL, and content-hash lookup helper.
- `apps/remi/src/server/conversion.rs`
  - Build `LegacyConverter` with Remi context.
  - Persist scriptlet metadata through `ConvertedPackage::set_scriptlet_metadata`.
  - Return scriptlet metadata on cold and hot `ServerConversionResult`.
  - Update stale timing text and direct test helper literals.
- `apps/remi/src/server/handlers/packages.rs`
  - Add public scriptlet summary to `PackageManifest`.
  - Parse summary from converted rows without leaking paths.
- `apps/remi/src/server/handlers/detail.rs`
  - Treat stale converted-package rows as unconverted in package detail and version summaries.
- `apps/remi/src/server/handlers/index.rs`
  - Expand converted-package queries.
  - Merge scriptlet metadata into repo-backed `PackageEntry.metadata["scriptlets"]`.
- `apps/remi/src/server/index_gen.rs`
  - Add scriptlet metadata to generated `VersionEntry` values.
- `apps/remi/src/server/handlers/oci.rs`
  - Replace manual `ConvertedPackage` literal with model helper.
  - Filter stale conversion rows from digest/tag manifest paths.
- `apps/remi/src/server/delta_manifests.rs`
  - Filter stale conversion rows before delta computation.
- `docs/modules/remi.md`
  - Document passive scriptlet metadata exposure and Goal 4's non-enforcement boundary.

Test:

- Existing unit tests in the touched modules.
- New tests colocated in `scriptlet_bundle.rs`, `converter.rs`, `converted.rs`, and Remi handler modules.

## Task 1: Core Bundle Summary Types And Exports

**Files:**

- Create: `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`
- Modify: `crates/conary-core/src/ccs/convert/mod.rs`

- [ ] **Step 1: Write failing summary/default/export tests**

Add this test module to the new file:

```rust
// conary-core/src/ccs/convert/scriptlet_bundle.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scriptlet_bundle_summary_defaults_match_legacy_rows() {
        let summary = ScriptletBundleSummary::default();

        assert_eq!(summary.scriptlet_fidelity, "unknown");
        assert_eq!(summary.target_compatibility, "unknown");
        assert_eq!(summary.publication_status, "public");
        assert_eq!(summary.evidence_digest, None);
        assert_eq!(summary.curation_evidence_digest, None);
        assert_eq!(summary.decision_counts, ScriptletDecisionCountsSummary::default());
        assert!(summary.blocked_reason_codes.is_empty());
        assert!(summary.review_reason_codes.is_empty());
        assert!(summary.unknown_commands.is_empty());
        assert!(summary.blocked_classes.is_empty());
        assert_eq!(summary.review_artifact_path, None);
    }

    #[test]
    fn scriptlet_bundle_summary_does_not_serialize_review_artifact_path() {
        let summary = ScriptletBundleSummary {
            review_artifact_path: Some("/tmp/private-review-secret".to_string()),
            ..ScriptletBundleSummary::default()
        };

        let json = serde_json::to_string(&summary).unwrap();

        assert!(!json.contains("review_artifact_path"));
        assert!(!json.contains("private-review-secret"));
    }
}
```

Add a compile-only public export use in `crates/conary-core/src/ccs/convert/converter.rs` tests:

```rust
#[test]
fn scriptlet_bundle_types_are_publicly_exported() {
    let summary = crate::ccs::convert::ScriptletBundleSummary::default();
    assert_eq!(summary.publication_status, "public");
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary-core scriptlet_bundle_summary
cargo test -p conary-core scriptlet_bundle_types_are_publicly_exported
```

Expected: fail because `scriptlet_bundle` and `ScriptletBundleSummary` do not exist yet.

- [ ] **Step 3: Add the new module skeleton and summary types**

Create `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`:

```rust
// conary-core/src/ccs/convert/scriptlet_bundle.rs
//! Passive legacy scriptlet bundle construction for legacy package conversion.

use crate::ccs::convert::effects::ScriptletClassificationReport;
use crate::ccs::legacy_scriptlets::LegacyScriptletBundle;
use crate::packages::common::PackageMetadata;
use crate::packages::traits::ExtractedFile;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, PartialEq)]
pub struct ScriptletBundleBuild {
    pub bundle: LegacyScriptletBundle,
    pub summary: ScriptletBundleSummary,
}

/// Internal conversion summary. Do not serialize directly in public API responses.
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

pub fn build_legacy_scriptlet_bundle(
    input: ScriptletBundleInput<'_>,
) -> anyhow::Result<ScriptletBundleBuild> {
    let _ = input;
    anyhow::bail!("legacy scriptlet bundle builder is not wired yet")
}
```

Modify `crates/conary-core/src/ccs/convert/mod.rs`:

```rust
pub mod scriptlet_bundle;
```

Place that module declaration after `pub mod payload_hints;`.

and add the exports:

```rust
pub use scriptlet_bundle::{
    ScriptletBundleBuild, ScriptletBundleInput, ScriptletBundleSummary,
    ScriptletDecisionCountsSummary, build_legacy_scriptlet_bundle,
};
```

Place the `pub use` block after the existing `pub use legacy_provenance::LegacyProvenance;`
line.

- [ ] **Step 4: Run the summary/export tests**

Run:

```bash
cargo test -p conary-core scriptlet_bundle_summary
cargo test -p conary-core scriptlet_bundle_types_are_publicly_exported
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/ccs/convert/scriptlet_bundle.rs crates/conary-core/src/ccs/convert/mod.rs crates/conary-core/src/ccs/convert/converter.rs
git commit -m "feat(scriptlets): add passive bundle summary types"
```

## Task 2: Bundle Builder Core And Deterministic Digest

**Files:**

- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`
- Test: `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`

- [ ] **Step 1: Write failing native-free bundle test**

Add:

```rust
#[test]
fn native_free_input_builds_zero_entry_bundle() {
    let metadata = package_metadata("native-free", "1.0");
    let files = Vec::new();
    let classification = ScriptletClassificationReport::default();

    let build = build_legacy_scriptlet_bundle(ScriptletBundleInput {
        source_metadata: &metadata,
        final_metadata: &metadata,
        source_files: &files,
        final_files: &files,
        source_format: "rpm",
        source_distro: Some("fedora"),
        source_release: Some("44"),
        source_arch: Some("x86_64"),
        source_checksum: Some("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        classification: &classification,
        conversion_tool: "remi",
        conversion_tool_version: "0.1.0",
    })
    .unwrap();

    assert!(build.bundle.entries.is_empty());
    assert_eq!(build.bundle.scriptlet_fidelity.as_str(), "native-free");
    assert_eq!(build.bundle.target_compatibility.as_str(), "conary-portable");
    assert_eq!(build.bundle.publication_status.as_str(), "public");
    assert_eq!(build.bundle.decision_counts.total(), 0);
    assert_eq!(build.summary.scriptlet_fidelity, "native-free");
    assert_eq!(build.summary.target_compatibility, "conary-portable");
    assert_eq!(build.summary.publication_status, "public");
    assert!(build.summary.evidence_digest.as_deref().unwrap().starts_with("sha256:"));
    build.bundle.validate().unwrap();
}
```

Add this helper in the test module:

```rust
fn package_metadata(name: &str, version: &str) -> PackageMetadata {
    PackageMetadata {
        package_path: std::path::PathBuf::from(format!("/tmp/{name}-{version}.rpm")),
        name: name.to_string(),
        version: version.to_string(),
        architecture: Some("x86_64".to_string()),
        description: Some("test package".to_string()),
        files: Vec::new(),
        dependencies: Vec::new(),
        provides: Vec::new(),
        scriptlets: Vec::new(),
        native_scriptlet_abi: Vec::new(),
        config_files: Vec::new(),
    }
}
```

- [ ] **Step 2: Run the failing native-free test**

Run:

```bash
cargo test -p conary-core native_free_input_builds_zero_entry_bundle
```

Expected: fail with the temporary builder error.

- [ ] **Step 3: Implement native-free bundle construction**

Implement these helpers in `scriptlet_bundle.rs`:

First extend the module imports:

```rust
use crate::ccs::convert::effects::{
    EntryClassification, ScriptletClassification, ScriptletEffectEvidence,
};
use crate::ccs::legacy_scriptlets::{
    DecisionCounts, EffectReplacement, ForeignReplayPolicy, LEGACY_SCRIPTLET_SCHEMA_V1,
    LifecyclePath, PublicationPolicy, PublicationStatus, ScriptletEffect, ScriptletFidelity,
    SourceFormat, TargetCompatibility, VersionScheme,
};
use crate::packages::native_abi::{
    NativeScriptletSupport, NativeStdinContract, NativeRootExpectation,
};
use crate::packages::traits::Scriptlet;
use std::collections::{BTreeMap, BTreeSet};
```

```rust
fn source_format(value: &str) -> anyhow::Result<SourceFormat> {
    match value {
        "rpm" => Ok(SourceFormat::Rpm),
        "deb" => Ok(SourceFormat::Deb),
        "arch" => Ok(SourceFormat::Arch),
        other => anyhow::bail!("unsupported scriptlet source format '{other}'"),
    }
}

fn source_family(format: SourceFormat) -> &'static str {
    match format {
        SourceFormat::Rpm => "rpm",
        SourceFormat::Deb => "deb",
        SourceFormat::Arch => "arch",
        SourceFormat::Unknown(_) => "unknown",
    }
}

fn version_scheme(format: SourceFormat) -> VersionScheme {
    match format {
        SourceFormat::Rpm => VersionScheme::Rpm,
        SourceFormat::Deb => VersionScheme::Deb,
        SourceFormat::Arch => VersionScheme::Arch,
        SourceFormat::Unknown(_) => VersionScheme::Semver,
    }
}

fn valid_prefixed_sha256(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}
```

Then replace the temporary builder with native-free support:

```rust
pub fn build_legacy_scriptlet_bundle(
    input: ScriptletBundleInput<'_>,
) -> anyhow::Result<ScriptletBundleBuild> {
    let format = source_format(input.source_format)?;
    let source_distro = input.source_distro.unwrap_or("unknown").to_string();
    let source_release = input.source_release.unwrap_or("unknown").to_string();
    let source_arch = input
        .source_arch
        .or(input.source_metadata.architecture.as_deref())
        .unwrap_or("unknown")
        .to_string();
    let source_checksum = input
        .source_checksum
        .filter(|checksum| valid_prefixed_sha256(checksum))
        .map(str::to_string);

    let mut bundle = LegacyScriptletBundle {
        schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
        schema_revision: 1,
        source_format: format.clone(),
        source_family: source_family(format.clone()).to_string(),
        source_distro: Some(source_distro),
        source_release: Some(source_release),
        source_arch: Some(source_arch),
        source_package: input.source_metadata.name.clone(),
        source_version: input.source_metadata.version.clone(),
        source_checksum,
        version_scheme: version_scheme(format),
        conversion_tool: input.conversion_tool.to_string(),
        conversion_tool_version: input.conversion_tool_version.to_string(),
        conversion_policy: "passive-scriptlet-bundle-goal4".to_string(),
        adapter_registry_digest: None,
        target_policy_digest: None,
        evidence_digest: None,
        target_compatibility: TargetCompatibility::ConaryPortable,
        allowed_targets: Vec::new(),
        foreign_replay_policy: ForeignReplayPolicy::Deny,
        publication_policy: PublicationPolicy::PublicIfNoBlocked,
        publication_status: PublicationStatus::Public,
        scriptlet_fidelity: ScriptletFidelity::NativeFree,
        decision_counts: DecisionCounts::default(),
        unsupported_class_counts: input.classification.unsupported_class_counts.clone(),
        entries: Vec::new(),
        extra: BTreeMap::new(),
    };

    let digest = evidence_digest(&bundle, &input)?;
    bundle.evidence_digest = Some(digest.clone());
    bundle.validate()?;

    Ok(ScriptletBundleBuild {
        summary: summary_from_bundle(&bundle, Some(digest)),
        bundle,
    })
}
```

Implement `summary_from_bundle` and `evidence_digest` with deterministic sorting:

```rust
fn summary_from_bundle(
    bundle: &LegacyScriptletBundle,
    evidence_digest: Option<String>,
) -> ScriptletBundleSummary {
    let blocked_reason_codes = sorted_entry_reason_codes(bundle, "blocked");
    let review_reason_codes = sorted_entry_reason_codes(bundle, "review");
    let unknown_commands = bundle
        .entries
        .iter()
        .flat_map(|entry| entry.unknown_commands.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let blocked_classes = bundle
        .entries
        .iter()
        .flat_map(|entry| entry.blocked_classes.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    ScriptletBundleSummary {
        scriptlet_fidelity: bundle.scriptlet_fidelity.as_str().to_string(),
        target_compatibility: bundle.target_compatibility.as_str().to_string(),
        publication_status: bundle.publication_status.as_str().to_string(),
        evidence_digest,
        curation_evidence_digest: None,
        decision_counts: ScriptletDecisionCountsSummary {
            replaced: bundle.decision_counts.replaced,
            legacy: bundle.decision_counts.legacy,
            blocked: bundle.decision_counts.blocked,
            review: bundle.decision_counts.review,
        },
        blocked_reason_codes,
        review_reason_codes,
        unknown_commands,
        blocked_classes,
        review_artifact_path: None,
    }
}

fn sorted_entry_reason_codes(bundle: &LegacyScriptletBundle, decision: &str) -> Vec<String> {
    bundle
        .entries
        .iter()
        .filter(|entry| entry.decision.as_str() == decision)
        .map(|entry| entry.reason_code.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn evidence_digest(
    bundle: &LegacyScriptletBundle,
    input: &ScriptletBundleInput<'_>,
) -> anyhow::Result<String> {
    let digest_doc = serde_json::json!({
        "schema": "conary-scriptlet-evidence-v1",
        "source_format": bundle.source_format.as_str(),
        "source_distro": bundle.source_distro.as_deref(),
        "source_release": bundle.source_release.as_deref(),
        "source_arch": bundle.source_arch.as_deref(),
        "source_package": &bundle.source_package,
        "source_version": &bundle.source_version,
        "source_checksum": bundle.source_checksum.as_deref(),
        "native_entries": sorted_native_digest_entries(input.source_metadata),
        "flat_entries": sorted_flat_digest_entries(input.source_metadata),
        "classification_counts": {
            "known": input.classification.known_count,
            "unknown": input.classification.unknown_count,
            "review": input.classification.review_count,
            "blocked": input.classification.blocked_count,
        },
        "classification_reasons": sorted_classification_reasons(input.classification),
        "classification_evidence": sorted_classification_evidence(input.classification),
        "entry_decisions": sorted_entry_decision_digest(bundle),
        "decision_counts": {
            "replaced": bundle.decision_counts.replaced,
            "legacy": bundle.decision_counts.legacy,
            "blocked": bundle.decision_counts.blocked,
            "review": bundle.decision_counts.review,
        },
        "scriptlet_fidelity": bundle.scriptlet_fidelity.as_str(),
        "target_compatibility": bundle.target_compatibility.as_str(),
        "publication_status": bundle.publication_status.as_str(),
    });
    let canonical = crate::json::canonical_json(&digest_doc)
        .map_err(|error| anyhow::anyhow!("failed to canonicalize scriptlet evidence: {error}"))?;
    let mut bytes = b"conary-scriptlet-evidence-v1\n".to_vec();
    bytes.extend_from_slice(&canonical);
    Ok(crate::hash::sha256_prefixed(&bytes))
}
```

Add the digest helper functions referenced above before `evidence_digest`:

```rust
fn sorted_native_digest_entries(metadata: &PackageMetadata) -> Vec<serde_json::Value> {
    let mut entries = metadata
        .native_scriptlet_abi
        .iter()
        .map(|entry| {
            serde_json::json!({
                "id": &entry.id,
                "slot": &entry.native_slot,
                "body_sha256": &entry.body.sha256,
                "support": native_support_digest(&entry.support),
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        left["id"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["id"].as_str().unwrap_or_default())
    });
    entries
}

fn sorted_flat_digest_entries(metadata: &PackageMetadata) -> Vec<serde_json::Value> {
    if !metadata.native_scriptlet_abi.is_empty() {
        return Vec::new();
    }
    metadata
        .scriptlets
        .iter()
        .enumerate()
        .map(|(index, scriptlet)| {
            serde_json::json!({
                "id": format!("scriptlet:{index}:{}", scriptlet.phase),
                "phase": scriptlet.phase.to_string(),
                "body_sha256": crate::hash::sha256_prefixed(scriptlet.content.as_bytes()),
            })
        })
        .collect()
}

fn sorted_classification_reasons(report: &ScriptletClassificationReport) -> Vec<String> {
    report
        .entries
        .iter()
        .filter_map(|entry| match &entry.classification {
            ScriptletClassification::Known { reason_code, .. }
            | ScriptletClassification::Unknown { reason_code, .. }
            | ScriptletClassification::Review { reason_code, .. }
            | ScriptletClassification::Blocked { reason_code, .. } => Some(reason_code.clone()),
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn sorted_classification_evidence(report: &ScriptletClassificationReport) -> Vec<serde_json::Value> {
    let mut values = report
        .entries
        .iter()
        .map(|entry| match &entry.classification {
            ScriptletClassification::Known { reason_code, effects } => serde_json::json!({
                "entry_id": &entry.entry_id,
                "outcome": "known",
                "reason_code": reason_code,
                "effects": sorted_effect_digest(effects),
            }),
            ScriptletClassification::Unknown { command, reason_code } => serde_json::json!({
                "entry_id": &entry.entry_id,
                "outcome": "unknown",
                "command": command,
                "reason_code": reason_code,
            }),
            ScriptletClassification::Review { class_id, reason_code } => serde_json::json!({
                "entry_id": &entry.entry_id,
                "outcome": "review",
                "class_id": class_id,
                "reason_code": reason_code,
            }),
            ScriptletClassification::Blocked { class_id, reason_code } => serde_json::json!({
                "entry_id": &entry.entry_id,
                "outcome": "blocked",
                "class_id": class_id,
                "reason_code": reason_code,
            }),
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        left["entry_id"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["entry_id"].as_str().unwrap_or_default())
            .then_with(|| {
                left["outcome"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["outcome"].as_str().unwrap_or_default())
            })
            .then_with(|| {
                left["reason_code"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["reason_code"].as_str().unwrap_or_default())
            })
    });
    values
}

fn sorted_effect_digest(effects: &[ScriptletEffectEvidence]) -> Vec<serde_json::Value> {
    let mut values = effects
        .iter()
        .map(|effect| {
            serde_json::json!({
                "kind": &effect.kind,
                "replacement": effect.replacement.as_str(),
                "adapter_id": effect.adapter_id.as_deref(),
                "adapter_digest": effect.adapter_digest.as_deref(),
                "reason_code": effect.reason_code.as_deref(),
                "command": effect.command.as_deref(),
            })
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        left["kind"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["kind"].as_str().unwrap_or_default())
            .then_with(|| {
                left["adapter_id"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["adapter_id"].as_str().unwrap_or_default())
            })
    });
    values
}

fn sorted_entry_decision_digest(bundle: &LegacyScriptletBundle) -> Vec<serde_json::Value> {
    let mut values = bundle
        .entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "id": &entry.id,
                "decision": entry.decision.as_str(),
                "reason_code": &entry.reason_code,
                "body_sha256": &entry.body_sha256,
                "unknown_commands": &entry.unknown_commands,
                "blocked_classes": &entry.blocked_classes,
            })
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        left["id"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["id"].as_str().unwrap_or_default())
    });
    values
}

fn native_support_digest(support: &NativeScriptletSupport) -> serde_json::Value {
    match support {
        NativeScriptletSupport::Parsed => serde_json::json!({"status": "parsed"}),
        NativeScriptletSupport::DeferredReview { reason_code } => {
            serde_json::json!({"status": "deferred-review", "reason_code": reason_code})
        }
        NativeScriptletSupport::Unpreservable { reason_code } => {
            serde_json::json!({"status": "unpreservable", "reason_code": reason_code})
        }
    }
}
```

- [ ] **Step 4: Run the native-free test**

Run:

```bash
cargo test -p conary-core native_free_input_builds_zero_entry_bundle
```

Expected: pass.

- [ ] **Step 5: Write failing flattened and native ABI entry tests**

Add tests:

```rust
#[test]
fn flattened_scriptlet_with_complete_effect_builds_replaced_entry() {
    let mut metadata = package_metadata("flat", "1.0");
    metadata.scriptlets.push(Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "/sbin/ldconfig\n".to_string(),
        flags: None,
    });
    let files = Vec::new();
    let mut classification = ScriptletClassificationReport::default();
    classification.push(
        "scriptlet:0:post-install",
        ScriptletClassification::Known {
            reason_code: "dynamic-linker-cache-complete".to_string(),
            effects: vec![complete_effect("dynamic-linker-cache", "ldconfig")],
        },
    );

    let build = bundle_for_metadata(&metadata, &files, &classification).unwrap();

    assert_eq!(build.bundle.entries.len(), 1);
    let entry = &build.bundle.entries[0];
    assert_eq!(entry.decision.as_str(), "replaced");
    assert_eq!(entry.reason_code, "dynamic-linker-cache-complete");
    assert_eq!(entry.effects.len(), 1);
    assert_eq!(entry.body, "/sbin/ldconfig\n");
    build.bundle.validate().unwrap();
}

#[test]
fn native_abi_binary_body_is_base64_encoded_and_validates() {
    let mut metadata = package_metadata("native-bin", "1.0");
    metadata.native_scriptlet_abi.push(native_entry_with_body(vec![0xff, 0x00, 0x01]));
    let files = Vec::new();
    let classification = ScriptletClassificationReport::default();

    let build = bundle_for_metadata(&metadata, &files, &classification).unwrap();
    let entry = &build.bundle.entries[0];

    assert_eq!(entry.body_encoding.as_deref(), Some("base64"));
    assert_eq!(entry.body_sha256, crate::hash::sha256_prefixed(&[0xff, 0x00, 0x01]));
    build.bundle.validate().unwrap();
}

#[test]
fn tampered_body_after_build_fails_strict_bundle_validation() {
    let mut metadata = package_metadata("tamper", "1.0");
    metadata.scriptlets.push(Scriptlet {
        phase: ScriptletPhase::PreInstall,
        interpreter: "/bin/sh".to_string(),
        content: "echo ok\n".to_string(),
        flags: None,
    });
    let files = Vec::new();
    let classification = ScriptletClassificationReport::default();
    let mut build = bundle_for_metadata(&metadata, &files, &classification).unwrap();

    build.bundle.entries[0].body.push_str("tampered\n");

    assert!(build.bundle.validate().is_err());
}
```

Add non-happy-path coverage in the same test module before implementation:

- `unknown_classification_becomes_review_entry`: a flattened scriptlet with
  `ScriptletClassification::Unknown` produces `decision = "review"` and carries
  the unknown command.
- `blocked_classification_becomes_blocked_entry`: a flattened scriptlet with
  `ScriptletClassification::Blocked` produces `decision = "blocked"` and carries
  the blocked class ID.
- `native_deferred_and_unpreservable_support_drive_decisions`: native
  `DeferredReview` maps to review and native `Unpreservable` maps to blocked
  even when no static command classification exists.
- `format_specific_metadata_projects_into_bundle`: RPM flags, DEB trigger/raw
  trigger content, debconf maintainer metadata, Arch `.INSTALL`, and ALPM hook
  metadata land in their first-class fields or `entry.extra`.
- `digest_changes_when_classification_evidence_changes`: changing adapter
  digest, effect replacement, unknown command, or blocked class changes the
  bundle `evidence_digest`.

Add helpers:

```rust
fn complete_effect(kind: &str, command: &str) -> ScriptletEffectEvidence {
    ScriptletEffectEvidence {
        kind: kind.to_string(),
        source: EffectSource::StaticSignal,
        confidence: EffectConfidence::Inferred,
        replacement: EffectReplacement::Complete,
        adapter_id: Some("test-adapter/v1".to_string()),
        adapter_digest: Some(crate::hash::sha256_prefixed(b"test-adapter/v1")),
        command: Some(command.to_string()),
        args: Vec::new(),
        path: None,
        reason_code: Some(format!("{kind}-complete")),
        extra: BTreeMap::new(),
    }
}

fn bundle_for_metadata(
    metadata: &PackageMetadata,
    files: &[ExtractedFile],
    classification: &ScriptletClassificationReport,
) -> anyhow::Result<ScriptletBundleBuild> {
    build_legacy_scriptlet_bundle(ScriptletBundleInput {
        source_metadata: metadata,
        final_metadata: metadata,
        source_files: files,
        final_files: files,
        source_format: "rpm",
        source_distro: Some("fedora"),
        source_release: Some("44"),
        source_arch: Some("x86_64"),
        source_checksum: None,
        classification,
        conversion_tool: "remi",
        conversion_tool_version: "0.1.0",
    })
}

fn native_entry_with_body(bytes: Vec<u8>) -> NativeScriptletEntry {
    NativeScriptletEntry {
        id: "rpm:%post".to_string(),
        format: NativeScriptletFormat::Rpm,
        kind: NativeScriptletKind::Executable,
        native_slot: "%post".to_string(),
        primary_lifecycle: NativeLifecyclePath::PostInstall,
        compatibility_phase: Some(ScriptletPhase::PostInstall),
        lifecycle_paths: vec![NativeLifecyclePath::PostInstall],
        interpreter: Some("/bin/sh".to_string()),
        interpreter_args: Vec::new(),
        body: NativeScriptletBody::from_bytes(bytes),
        invocation: NativeInvocationContract::none(),
        order: NativeTransactionOrder::new(NativeTransactionPosition::AfterPayload),
        support: NativeScriptletSupport::Parsed,
        metadata: NativeScriptletMetadata::Rpm(RpmNativeScriptletMetadata {
            slot: RpmScriptletSlot::Post,
            scriptlet_flags: None,
            trigger: None,
        }),
    }
}
```

- [ ] **Step 6: Run the failing entry tests**

Run:

```bash
cargo test -p conary-core flattened_scriptlet_with_complete_effect_builds_replaced_entry
cargo test -p conary-core native_abi_binary_body_is_base64_encoded_and_validates
cargo test -p conary-core tampered_body_after_build_fails_strict_bundle_validation
```

Expected: fail because entry construction is not implemented.

- [ ] **Step 7: Implement entry construction and decision grouping**

Implement:

- `group_classifications(report: &ScriptletClassificationReport) -> BTreeMap<String, Vec<&EntryClassification>>`
- `entry_decision(entry_id, native_support, grouped) -> EntryDecisionBuild`
- `legacy_effect_from_evidence(effect: &ScriptletEffectEvidence) -> ScriptletEffect`
- `entry_from_native(entry, grouped) -> LegacyScriptletEntry`
- `entry_from_flattened(index, scriptlet, grouped) -> LegacyScriptletEntry`
- `aggregate_bundle_status(bundle: &mut LegacyScriptletBundle)`

Use these concrete rules:

```rust
match classifications_for_entry {
    any Blocked => ScriptletDecision::Blocked,
    native support Unpreservable => ScriptletDecision::Blocked,
    any Review or Unknown => ScriptletDecision::Review,
    native support DeferredReview => ScriptletDecision::Review,
    at least one Known classification, at least one effect, and every effect
        replacement is Complete => ScriptletDecision::Replaced,
    _ => ScriptletDecision::Review,
}
```

When native ABI is present:

```rust
let entries = input
    .source_metadata
    .native_scriptlet_abi
    .iter()
    .map(|entry| entry_from_native(entry, &grouped))
    .collect::<anyhow::Result<Vec<_>>>()?;
```

When native ABI is absent:

```rust
let entries = input
    .source_metadata
    .scriptlets
    .iter()
    .enumerate()
    .map(|(index, scriptlet)| entry_from_flattened(index, scriptlet, &grouped))
    .collect::<anyhow::Result<Vec<_>>>()?;
```

Use `base64::Engine` for binary bodies. If the repo already imports `base64`, use:

```rust
use base64::Engine as _;
let body = base64::engine::general_purpose::STANDARD.encode(&entry.body.bytes);
```

Entry defaults:

```rust
timeout_ms: 30_000,
sandbox: None,
capabilities: Vec::new(),
source_evidence_refs: Vec::new(),
residual_replay: None,
extra: BTreeMap::new(),
```

For flattened entries, generate IDs with the same convention as classification:

```rust
let id = format!("scriptlet:{index}:{}", scriptlet.phase);
```

For native entries, use `entry.id.clone()`.

For native control artifacts with no executable interpreter, use a stable
placeholder instead of leaving `LegacyScriptletEntry.interpreter` empty:

```rust
let interpreter = entry
    .interpreter
    .clone()
    .unwrap_or_else(|| "package-manager-control-artifact".to_string());
```

This applies especially to ALPM hook entries. Add a test named
`arch_alpm_hook_control_artifact_validates_with_placeholder_interpreter` that
builds a `NativeScriptletKind::ControlArtifact` ALPM hook entry with
`interpreter: None`, verifies the placeholder interpreter, verifies
`extra["arch_alpm_hook"]` exists, and calls `bundle.validate().unwrap()`.

Do not compute the final evidence digest until entries, decisions, aggregate
status, and decision counts are final. The builder should:

1. build entries with a temporary empty evidence digest;
2. call `aggregate_bundle_status(&mut bundle)`;
3. compute `let digest = evidence_digest(&bundle, &input)?;`;
4. set `bundle.evidence_digest = Some(digest.clone())`;
5. set each entry's `evidence_digest` to that digest unless the entry already
   has a more specific digest;
6. validate the final bundle before returning.

- [ ] **Step 8: Add format-specific metadata projections**

Implement first-class projections:

- RPM trigger data to `LegacyScriptletEntry.rpm_trigger`.
- RPM scriptlet flags to `entry.extra["rpm_scriptlet_flags"]`.
- DEB control member, triggers content, trigger names, raw lines, and modes to `deb_maintainer` plus `entry.extra`.
- Arch `.INSTALL` metadata to `arch_install`.
- Arch ALPM hook metadata to `entry.extra["arch_alpm_hook"]`.
- `NativeScriptletKind` to `entry.extra["native_scriptlet_kind"]`.
- Native invocation contract to `native_invocation`.

Use helper functions with total mappings, for example:

```rust
fn native_stdin(value: NativeStdinContract) -> &'static str {
    match value {
        NativeStdinContract::None => "none",
        NativeStdinContract::Debconf => "debconf",
        NativeStdinContract::Paths => "paths",
        NativeStdinContract::Unknown => "unknown",
    }
}
```

For nonrepresentable values, use `toml::Value::String`, `toml::Value::Array`, and `toml::Value::Table` under `entry.extra`.

- [ ] **Step 9: Run all scriptlet bundle tests**

Run:

```bash
cargo test -p conary-core scriptlet_bundle
```

Expected: pass.

- [ ] **Step 10: Commit**

```bash
git add crates/conary-core/src/ccs/convert/scriptlet_bundle.rs
git commit -m "feat(scriptlets): build passive legacy bundles"
```

## Task 3: LegacyConverter Embeds Bundles

**Files:**

- Modify: `crates/conary-core/src/ccs/convert/converter.rs`
- Test: `crates/conary-core/src/ccs/convert/converter.rs`
- Modify test helper in: `apps/remi/src/server/conversion.rs`

- [ ] **Step 1: Write failing converter integration tests**

Update the existing Goal 3b tests that currently assert `manifest.legacy_scriptlets.is_none()` to assert passive presence. Add these focused tests:

```rust
#[test]
fn conversion_result_embeds_legacy_scriptlet_bundle() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "/sbin/ldconfig\n".to_string(),
        flags: None,
    }];

    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert(
            &metadata,
            &make_test_files(),
            "rpm",
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        )
        .unwrap();

    let bundle = result.build_result.manifest.legacy_scriptlets.as_ref().unwrap();
    assert_eq!(bundle.source_package, metadata.name);
    assert_eq!(
        result.legacy_scriptlets.as_ref().unwrap().evidence_digest.as_deref(),
        bundle.evidence_digest.as_deref()
    );
    assert_eq!(
        result.scriptlet_metadata.evidence_digest.as_deref(),
        bundle.evidence_digest.as_deref()
    );
    bundle.validate().unwrap();
}

#[test]
fn remi_converter_context_flows_into_bundle_metadata() {
    let temp_dir = tempfile::tempdir().unwrap();
    let metadata = make_test_metadata();
    let converter = passive_test_converter(temp_dir.path())
    .with_source_distro("fedora")
    .with_source_release("44")
    .with_conversion_tool("remi");

    let result = converter
        .convert(&metadata, &make_test_files(), "rpm", "not-a-real-prefixed-sha")
        .unwrap();

    let bundle = result.build_result.manifest.legacy_scriptlets.as_ref().unwrap();
    assert_eq!(bundle.source_distro.as_deref(), Some("fedora"));
    assert_eq!(bundle.source_release.as_deref(), Some("44"));
    assert_eq!(bundle.conversion_tool, "remi");
    assert_eq!(bundle.source_checksum, None);
}
```

- [ ] **Step 2: Run failing converter tests**

Run:

```bash
cargo test -p conary-core conversion_result_embeds_legacy_scriptlet_bundle
cargo test -p conary-core remi_converter_context_flows_into_bundle_metadata
```

Expected: fail because `ConversionResult` lacks bundle fields and `LegacyConverter` lacks context setters.

- [ ] **Step 3: Add converter context fields and setters**

Modify `LegacyConverter`:

```rust
pub struct LegacyConverter {
    options: ConversionOptions,
    analyzer: ScriptletAnalyzer,
    source_distro: Option<String>,
    source_release: Option<String>,
    conversion_tool: String,
}
```

Modify `new`:

```rust
Self {
    options,
    analyzer: ScriptletAnalyzer::new(),
    source_distro: None,
    source_release: None,
    conversion_tool: "conary".to_string(),
}
```

Add setters:

```rust
pub fn with_source_distro(mut self, distro: impl Into<String>) -> Self {
    self.source_distro = Some(distro.into());
    self
}

pub fn with_source_release(mut self, release: impl Into<String>) -> Self {
    self.source_release = Some(release.into());
    self
}

pub fn with_conversion_tool(mut self, tool: impl Into<String>) -> Self {
    self.conversion_tool = tool.into();
    self
}
```

- [ ] **Step 4: Add bundle fields to core ConversionResult**

Add imports:

```rust
use crate::ccs::convert::{
    ScriptletBundleInput, ScriptletBundleSummary, build_legacy_scriptlet_bundle,
};
use crate::ccs::legacy_scriptlets::LegacyScriptletBundle;
```

Add fields:

```rust
pub legacy_scriptlets: Option<LegacyScriptletBundle>,
pub scriptlet_metadata: ScriptletBundleSummary,
```

Update all direct `conary_core::ccs::convert::ConversionResult` struct
literals. The primary production literal is the `Ok(ConversionResult { ... })`
return site in `crates/conary-core/src/ccs/convert/converter.rs`; populate the
new fields from the `scriptlet_bundle` value built in Step 5:

```rust
Ok(ConversionResult {
    build_result,
    package_path: Some(package_path),
    fidelity,
    original_format: format.to_string(),
    original_checksum: checksum.to_string(),
    detected_hooks,
    inferred_capabilities,
    inference_error,
    legacy_provenance,
    scriptlet_classification,
    legacy_scriptlets: Some(scriptlet_bundle.bundle),
    scriptlet_metadata: scriptlet_bundle.summary,
})
```

The known direct Remi test helper site is
`apps/remi/src/server/conversion.rs::make_conversion_result`; initialize it
with:

```rust
legacy_scriptlets: None,
scriptlet_metadata: ScriptletBundleSummary::default(),
```

- [ ] **Step 5: Embed the bundle before TOML serialization**

In `LegacyConverter::convert()`, insert after capabilities are attached and before `toml::to_string_pretty(&manifest)`:

```rust
let scriptlet_bundle = build_legacy_scriptlet_bundle(ScriptletBundleInput {
    source_metadata: metadata,
    final_metadata: &final_metadata,
    source_files: files,
    final_files: &final_files,
    source_format: format,
    source_distro: self.source_distro.as_deref(),
    source_release: self.source_release.as_deref(),
    source_arch: metadata.architecture.as_deref(),
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
```

At the return site, add:

```rust
legacy_scriptlets: Some(scriptlet_bundle.bundle),
scriptlet_metadata: scriptlet_bundle.summary,
```

Do not leave a second unstated `ConversionResult` literal for a later compile
pass. The production return site and the Remi test helper literal should be
updated in the same task.

- [ ] **Step 6: Run converter tests**

Run:

```bash
cargo test -p conary-core conversion_result_embeds_legacy_scriptlet_bundle
cargo test -p conary-core remi_converter_context_flows_into_bundle_metadata
cargo test -p conary-core conversion_result_carries_scriptlet_classification_report
cargo test -p conary-core parsed_native_abi_body_uses_adapter_classification_when_flattened_scriptlets_are_empty
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/conary-core/src/ccs/convert/converter.rs apps/remi/src/server/conversion.rs
git commit -m "feat(scriptlets): embed passive bundles during conversion"
```

## Task 4: ConvertedPackage Schema, Model Fields, And Version Bumps

**Files:**

- Modify: `crates/conary-core/src/db/schema.rs`
- Modify: `crates/conary-core/src/db/migrations/v41_current.rs`
- Modify: `crates/conary-core/src/db/models/converted.rs`
- Modify: `apps/remi/src/server/handlers/oci.rs`

- [ ] **Step 1: Write failing model and migration tests**

In `converted.rs` tests, add:

```rust
#[test]
fn converted_package_defaults_scriptlet_metadata() {
    let converted = ConvertedPackage::new(
        "rpm".to_string(),
        "sha256:source".to_string(),
        "high".to_string(),
    );

    assert_eq!(converted.scriptlet_fidelity, "unknown");
    assert_eq!(converted.target_compatibility, "unknown");
    assert_eq!(converted.publication_status, "public");
    assert_eq!(converted.blocked_reason_codes_json, "[]");
    assert_eq!(converted.scriptlet_summary_json, "{}");
    assert_eq!(converted.review_artifact_path, None);
}

#[test]
fn converted_package_round_trips_scriptlet_metadata() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::migrate(&conn).unwrap();
    let mut converted = ConvertedPackage::new_server(
        "fedora".to_string(),
        "gtk3".to_string(),
        "3.24.0-1.fc44".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        "high".to_string(),
        &["sha256:chunk".to_string()],
        42,
        "sha256:content".to_string(),
        "/tmp/gtk3.ccs".to_string(),
    );
    let summary = ScriptletBundleSummary {
        scriptlet_fidelity: "review-required".to_string(),
        target_compatibility: "review-required".to_string(),
        publication_status: "private-review".to_string(),
        evidence_digest: Some(crate::hash::sha256_prefixed(b"evidence")),
        blocked_reason_codes: vec!["blocked-class-network".to_string()],
        review_reason_codes: vec!["review-class-debconf".to_string()],
        unknown_commands: vec!["custom-helper".to_string()],
        blocked_classes: vec!["network".to_string()],
        ..ScriptletBundleSummary::default()
    };
    converted.set_scriptlet_metadata(&summary).unwrap();
    converted.insert(&conn).unwrap();

    let found = ConvertedPackage::find_by_package_identity_with_arch(
        &conn,
        "fedora",
        "gtk3",
        Some("3.24.0-1.fc44"),
        None,
    )
    .unwrap()
    .unwrap();

    assert_eq!(found.scriptlet_fidelity, "review-required");
    assert_eq!(found.target_compatibility, "review-required");
    assert_eq!(found.publication_status, "private-review");
    assert_eq!(found.blocked_reason_codes_json, "[\"blocked-class-network\"]");
    assert!(found.scriptlet_summary_json.contains("custom-helper"));
}

#[test]
fn scriptlet_summary_recovers_from_malformed_json_with_scalar_fields() {
    let mut converted = ConvertedPackage::new(
        "rpm".to_string(),
        "sha256:source".to_string(),
        "high".to_string(),
    );
    converted.scriptlet_fidelity = "blocked".to_string();
    converted.target_compatibility = "blocked".to_string();
    converted.publication_status = "blocked".to_string();
    converted.evidence_digest = Some(crate::hash::sha256_prefixed(b"fallback-evidence"));
    converted.blocked_reason_codes_json = "[\"blocked-class-network\"]".to_string();
    converted.scriptlet_summary_json = "{not valid json".to_string();

    let summary = converted.scriptlet_summary();

    assert_eq!(summary.scriptlet_fidelity, "blocked");
    assert_eq!(summary.target_compatibility, "blocked");
    assert_eq!(summary.publication_status, "blocked");
    assert_eq!(
        summary.evidence_digest,
        Some(crate::hash::sha256_prefixed(b"fallback-evidence"))
    );
    assert_eq!(summary.blocked_reason_codes, vec!["blocked-class-network"]);
    assert!(summary.review_reason_codes.is_empty());
    assert!(summary.unknown_commands.is_empty());
}
```

In `schema.rs` tests, add:

```rust
#[test]
fn migration_adds_scriptlet_metadata_columns_to_converted_packages() {
    let (_temp, conn) = create_test_db_at_version(69);
    conn.execute(
        "INSERT INTO converted_packages (original_format, original_checksum, conversion_version, conversion_fidelity, enhancement_version, enhancement_status)
         VALUES ('rpm', 'sha256:old', 3, 'high', 0, 'pending')",
        [],
    )
    .unwrap();

    migrate(&conn).unwrap();

    let row = conn
        .query_row(
            "SELECT scriptlet_fidelity, target_compatibility, publication_status, blocked_reason_codes_json, scriptlet_summary_json
             FROM converted_packages
             WHERE original_checksum = 'sha256:old'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .unwrap();

    assert_eq!(row.0, "unknown");
    assert_eq!(row.1, "unknown");
    assert_eq!(row.2, "public");
    assert_eq!(row.3, "[]");
    assert_eq!(row.4, "{}");
}
```

- [ ] **Step 2: Run failing DB tests**

Run:

```bash
cargo test -p conary-core converted_package_defaults_scriptlet_metadata
cargo test -p conary-core converted_package_round_trips_scriptlet_metadata
cargo test -p conary-core scriptlet_summary_recovers_from_malformed_json_with_scalar_fields
cargo test -p conary-core migration_adds_scriptlet_metadata_columns_to_converted_packages
```

Expected: fail because fields and migration v70 do not exist.

- [ ] **Step 3: Add migration v70**

Set `SCHEMA_VERSION` to `70` in `schema.rs`, add `70 => migrations::migrate_v70(conn),`, and add to `v41_current.rs`:

```rust
/// Version 70: Passive legacy scriptlet bundle metadata for converted packages
pub fn migrate_v70(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 70");

    conn.execute_batch(
        "
        ALTER TABLE converted_packages ADD COLUMN scriptlet_fidelity TEXT NOT NULL DEFAULT 'unknown';
        ALTER TABLE converted_packages ADD COLUMN target_compatibility TEXT NOT NULL DEFAULT 'unknown';
        ALTER TABLE converted_packages ADD COLUMN publication_status TEXT NOT NULL DEFAULT 'public';
        ALTER TABLE converted_packages ADD COLUMN evidence_digest TEXT;
        ALTER TABLE converted_packages ADD COLUMN curation_evidence_digest TEXT;
        ALTER TABLE converted_packages ADD COLUMN blocked_reason_codes_json TEXT NOT NULL DEFAULT '[]';
        ALTER TABLE converted_packages ADD COLUMN scriptlet_summary_json TEXT NOT NULL DEFAULT '{}';
        ALTER TABLE converted_packages ADD COLUMN review_artifact_path TEXT;

        CREATE INDEX idx_converted_packages_scriptlet_fidelity
            ON converted_packages(scriptlet_fidelity);
        CREATE INDEX idx_converted_packages_publication_status
            ON converted_packages(publication_status);
        ",
    )?;

    info!("Schema version 70 applied successfully (passive scriptlet metadata)");
    Ok(())
}
```

- [ ] **Step 4: Extend ConvertedPackage fields without changing constructors**

Import the summary type in `converted.rs`:

```rust
use crate::ccs::convert::ScriptletBundleSummary;
```

Set `CONVERSION_VERSION` to `4` and update the version comment:

```rust
/// v4 invalidates Remi artifacts produced before passive legacy scriptlet
/// bundles and scriptlet metadata were embedded in converted CCS manifests.
pub const CONVERSION_VERSION: i32 = 4;
```

Add fields:

```rust
pub scriptlet_fidelity: String,
pub target_compatibility: String,
pub publication_status: String,
pub evidence_digest: Option<String>,
pub curation_evidence_digest: Option<String>,
pub blocked_reason_codes_json: String,
pub scriptlet_summary_json: String,
pub review_artifact_path: Option<String>,
```

In both constructors, initialize:

```rust
scriptlet_fidelity: "unknown".to_string(),
target_compatibility: "unknown".to_string(),
publication_status: "public".to_string(),
evidence_digest: None,
curation_evidence_digest: None,
blocked_reason_codes_json: "[]".to_string(),
scriptlet_summary_json: "{}".to_string(),
review_artifact_path: None,
```

Append the new fields to `COLUMNS`, extend `from_row` at indices `22..=29`, and append the fields to `insert` SQL.

- [ ] **Step 5: Add model helpers**

Add:

```rust
pub fn set_scriptlet_metadata(
    &mut self,
    summary: &ScriptletBundleSummary,
) -> serde_json::Result<()> {
    self.scriptlet_fidelity = summary.scriptlet_fidelity.clone();
    self.target_compatibility = summary.target_compatibility.clone();
    self.publication_status = summary.publication_status.clone();
    self.evidence_digest = summary.evidence_digest.clone();
    self.curation_evidence_digest = summary.curation_evidence_digest.clone();
    self.blocked_reason_codes_json = serde_json::to_string(&summary.blocked_reason_codes)?;
    self.scriptlet_summary_json = serde_json::to_string(summary)?;
    self.review_artifact_path = summary.review_artifact_path.clone();
    Ok(())
}

pub fn scriptlet_summary(&self) -> ScriptletBundleSummary {
    match serde_json::from_str::<ScriptletBundleSummary>(&self.scriptlet_summary_json) {
        Ok(mut summary) => {
            summary.scriptlet_fidelity = self.scriptlet_fidelity.clone();
            summary.target_compatibility = self.target_compatibility.clone();
            summary.publication_status = self.publication_status.clone();
            summary.evidence_digest = self.evidence_digest.clone();
            summary.curation_evidence_digest = self.curation_evidence_digest.clone();
            summary.review_artifact_path = self.review_artifact_path.clone();
            summary
        }
        Err(error) => {
            tracing::warn!(
                "failed to parse converted package scriptlet summary JSON: {}",
                error
            );
            let mut summary = ScriptletBundleSummary {
                scriptlet_fidelity: self.scriptlet_fidelity.clone(),
                target_compatibility: self.target_compatibility.clone(),
                publication_status: self.publication_status.clone(),
                evidence_digest: self.evidence_digest.clone(),
                curation_evidence_digest: self.curation_evidence_digest.clone(),
                ..ScriptletBundleSummary::default()
            };
            summary.blocked_reason_codes =
                serde_json::from_str(&self.blocked_reason_codes_json).unwrap_or_default();
            summary
        }
    }
}
```

Add a helper to replace OCI manual row construction:

```rust
pub fn find_by_content_hash_identity(
    conn: &Connection,
    distro: &str,
    package: &str,
    content_hash: &str,
) -> Result<Option<Self>> {
    let normalized_hash = content_hash
        .strip_prefix("sha256:")
        .unwrap_or(content_hash);
    let prefixed_hash = format!("sha256:{normalized_hash}");
    let sql = format!(
        "SELECT {} FROM converted_packages \
         WHERE distro = ?1 AND package_name = ?2 \
         AND (content_hash = ?3 OR content_hash = ?4) \
         ORDER BY converted_at DESC LIMIT 1",
        Self::COLUMNS
    );
    Ok(conn
        .query_row(
            &sql,
            params![distro, package, normalized_hash, prefixed_hash],
            Self::from_row,
        )
        .optional()?)
}
```

Use that helper in `apps/remi/src/server/handlers/oci.rs` in this same task,
before running any Remi tests. Otherwise adding fields to `ConvertedPackage`
will break the existing manual `ConvertedPackage { ... }` digest-path literal.
Replace the digest lookup branch with:

```rust
let converted = if let Some(ver) = version {
    ConvertedPackage::find_by_package_identity(&conn, distro, package, Some(ver))?
} else {
    ConvertedPackage::find_by_content_hash_identity(&conn, distro, package, reference)?
};

let converted = converted.and_then(|converted| {
    if converted.needs_reconversion() {
        None
    } else {
        Some(converted)
    }
});
```

Remove the entire digest-path `conn.query_row(..., |row| ConvertedPackage { ... })`
closure. The model helper normalizes OCI digest references by accepting both
prefixed and unprefixed `sha256` content hashes.

- [ ] **Step 6: Run DB/model tests**

Run:

```bash
cargo test -p conary-core converted_package
cargo test -p conary-core migration_adds_scriptlet_metadata_columns_to_converted_packages
cargo test -p remi oci
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/conary-core/src/db/schema.rs crates/conary-core/src/db/migrations/v41_current.rs crates/conary-core/src/db/models/converted.rs apps/remi/src/server/handlers/oci.rs
git commit -m "feat(scriptlets): store passive metadata on converted packages"
```

## Task 5: Remi Conversion Persistence And Hot Cache Metadata

**Files:**

- Modify: `apps/remi/src/server/conversion.rs`

- [ ] **Step 1: Write failing Remi conversion tests**

Add tests that exercise cold persistence and hot readback:

```rust
#[test]
fn persisted_conversion_records_scriptlet_metadata() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("remi.db");
    conary_core::db::init(&db_path).unwrap();
    let chunk_dir = temp.path().join("chunks");
    let cache_dir = temp.path().join("cache");
    let output_ccs = temp.path().join("out/test.ccs");
    std::fs::create_dir_all(output_ccs.parent().unwrap()).unwrap();
    std::fs::write(&output_ccs, b"ccs payload").unwrap();
    let service = ConversionService::new(chunk_dir, cache_dir, db_path.clone(), None);
    let metadata = PackageMetadata::new(
        PathBuf::from("/tmp/test.rpm"),
        "test".to_string(),
        "1.0".to_string(),
    );
    let mut result = make_conversion_result(Default::default());
    result.package_path = Some(output_ccs);
    let mut repo_pkg = RepositoryPackage::new(
        1,
        "test".to_string(),
        "1.0".to_string(),
        "sha256:repo".to_string(),
        11,
        "https://example.invalid/test.rpm".to_string(),
    );
    repo_pkg.architecture = Some("x86_64".to_string());
    let input = PersistConversionInput {
        distro: "fedora".to_string(),
        metadata,
        format: "rpm",
        original_checksum: "sha256:source".to_string(),
        conversion_result: result,
        repo_pkg,
        chunk_hashes: vec!["sha256:chunk".to_string()],
    };

    let server_result = service.persist_conversion_result(input).unwrap();

    assert_eq!(server_result.scriptlets.scriptlet_fidelity, "unknown");
    let conn = conary_core::db::open(&db_path).unwrap();
    let converted = ConvertedPackage::find_by_package_identity_with_arch(
        &conn,
        "fedora",
        "test",
        Some("1.0"),
        Some("x86_64"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(converted.scriptlet_fidelity, "unknown");
}
```

`persist_conversion_result` is private but callable from the same file's test
module. Update `make_conversion_result` so this test can construct direct core
conversion results with the new `legacy_scriptlets` and `scriptlet_metadata`
fields.

- [ ] **Step 2: Run failing Remi conversion tests**

Run:

```bash
cargo test -p remi persisted_conversion_records_scriptlet_metadata
```

Expected: fail because `ServerConversionResult` has no scriptlet metadata and persistence does not call `set_scriptlet_metadata`.

- [ ] **Step 3: Add public scriptlet metadata type and server result field**

In `apps/remi/src/server/conversion.rs`, import `serde::{Deserialize,
Serialize}` if the file does not already have both traits in scope, plus:

```rust
use conary_core::ccs::convert::{
    ScriptletBundleSummary, ScriptletDecisionCountsSummary,
};
```

Then add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

Add:

```rust
impl From<&ScriptletBundleSummary> for ScriptletPackageMetadata {
    fn from(summary: &ScriptletBundleSummary) -> Self {
        Self {
            scriptlet_fidelity: summary.scriptlet_fidelity.clone(),
            target_compatibility: summary.target_compatibility.clone(),
            publication_status: summary.publication_status.clone(),
            evidence_digest: summary.evidence_digest.clone(),
            curation_evidence_digest: summary.curation_evidence_digest.clone(),
            decision_counts: summary.decision_counts,
            blocked_reason_codes: summary.blocked_reason_codes.clone(),
            review_reason_codes: summary.review_reason_codes.clone(),
            unknown_commands: summary.unknown_commands.clone(),
            blocked_classes: summary.blocked_classes.clone(),
            review_artifact_available: summary.review_artifact_path.is_some(),
        }
    }
}
```

Add `pub scriptlets: ScriptletPackageMetadata` to `ServerConversionResult`.

Update every direct `ServerConversionResult { ... }` literal in
`apps/remi/src/server/conversion.rs`. The known unit-test literals are in:

- `server_conversion_result_can_carry_timing_report`
- `test_server_conversion_result_debug`

Each should add:

```rust
scriptlets: ScriptletPackageMetadata::from(&ScriptletBundleSummary::default()),
```

- [ ] **Step 4: Set Remi converter context**

Replace:

```rust
let converter = LegacyConverter::new(options);
```

with:

```rust
let converter = LegacyConverter::new(options)
    .with_source_distro(distro)
    .with_conversion_tool("remi");
```

Do not derive source release from `RepositoryPackage::version_scheme`. Goal 4
should leave release unset in Remi until repository metadata has an actual
release channel; the bundle builder will normalize it to `unknown`.

- [ ] **Step 5: Persist and return scriptlet metadata**

In `persist_conversion_result`, after detected hooks:

```rust
converted.detected_hooks = Some(serde_json::to_string(&conversion_result.detected_hooks)?);
converted.set_scriptlet_metadata(&conversion_result.scriptlet_metadata)?;
```

Return:

```rust
scriptlets: ScriptletPackageMetadata::from(&conversion_result.scriptlet_metadata),
```

In `build_result_from_existing`, parse:

```rust
let scriptlet_summary = existing.scriptlet_summary();
```

and return:

```rust
scriptlets: ScriptletPackageMetadata::from(&scriptlet_summary),
```

In recipe/build paths that do not use legacy conversion, use:

```rust
scriptlets: ScriptletPackageMetadata::from(&ScriptletBundleSummary::default()),
```

Update the stale `ConversionPhase::AdapterDispatch` skip reason that currently
says the adapter registry is not implemented to:

```rust
"adapter dispatch timing is included in legacy converter timing"
```

- [ ] **Step 6: Run Remi conversion tests**

Run:

```bash
cargo test -p remi conversion
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add apps/remi/src/server/conversion.rs
git commit -m "feat(scriptlets): persist remi scriptlet metadata"
```

## Task 6: Public Package And Metadata API Exposure

**Files:**

- Modify: `apps/remi/src/server/handlers/packages.rs`
- Modify: `apps/remi/src/server/handlers/index.rs`
- Modify: `apps/remi/src/server/index_gen.rs`

- [ ] **Step 1: Write failing package manifest leak-prevention test**

In `packages.rs` tests, seed a converted row:

```rust
#[test]
fn package_manifest_includes_scriptlets_without_private_path() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("remi.db");
    conary_core::db::init(&db_path).unwrap();
    let ccs_path = temp.path().join("cache/packages/pkg-1.0-x86_64.ccs");
    std::fs::create_dir_all(ccs_path.parent().unwrap()).unwrap();
    std::fs::write(&ccs_path, b"ccs").unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    let mut converted = ConvertedPackage::new_server(
        "fedora".to_string(),
        "pkg".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        "high".to_string(),
        &["sha256:chunk".to_string()],
        3,
        "sha256:content".to_string(),
        ccs_path.to_string_lossy().to_string(),
    );
    converted.package_architecture = Some("x86_64".to_string());
    let summary = ScriptletBundleSummary {
        scriptlet_fidelity: "review-required".to_string(),
        target_compatibility: "review-required".to_string(),
        publication_status: "private-review".to_string(),
        review_reason_codes: vec!["review-class-debconf".to_string()],
        review_artifact_path: Some("/tmp/private-review-secret".to_string()),
        ..ScriptletBundleSummary::default()
    };
    converted.set_scriptlet_metadata(&summary).unwrap();
    converted.insert(&conn).unwrap();

    let manifest = check_converted(&db_path, "fedora", "pkg", Some("1.0"), Some("x86_64"))
        .unwrap()
        .unwrap();
    let json = serde_json::to_string(&manifest).unwrap();

    assert_eq!(manifest.scriptlets.scriptlet_fidelity, "review-required");
    assert!(manifest.scriptlets.review_artifact_available);
    assert!(!json.contains("review_artifact_path"));
    assert!(!json.contains("private-review-secret"));
}
```

- [ ] **Step 2: Run failing package test**

Run:

```bash
cargo test -p remi package_manifest_includes_scriptlets_without_private_path
```

Expected: fail because `PackageManifest` has no `scriptlets` field.

- [ ] **Step 3: Add package manifest scriptlet field**

Import `ScriptletPackageMetadata` and `ScriptletBundleSummary`. Add:

```rust
pub scriptlets: ScriptletPackageMetadata,
```

to `PackageManifest`.

In `check_converted`, add:

```rust
let scriptlet_summary = converted.scriptlet_summary();
```

and set:

```rust
scriptlets: ScriptletPackageMetadata::from(&scriptlet_summary),
```

- [ ] **Step 4: Write failing `/metadata` merge tests**

Add tests in `index.rs` that create one repo-backed converted row and one converted-only row. Assert:

```rust
let repo_backed = metadata
    .packages
    .iter()
    .find(|pkg| pkg.name == "repo-backed")
    .unwrap();
assert!(repo_backed.converted);
assert_eq!(
    repo_backed
        .metadata
        .as_ref()
        .unwrap()
        .get("scriptlets")
        .unwrap()
        .get("scriptlet_fidelity")
        .unwrap(),
    "fully-replaced"
);
assert!(
    !serde_json::to_string(repo_backed)
        .unwrap()
        .contains("private-review-secret")
);
```

- [ ] **Step 5: Implement richer converted metadata rows in index handler**

Introduce a small row type and a loader. This keeps the current caller logic
clear: one source of converted rows feeds the converted key set, repo-backed
metadata merge, and converted-only entries.

```rust
use std::collections::{HashMap, HashSet};

type PackageKey = (String, String, Option<String>);

struct ConvertedMetadataRow {
    name: String,
    version: String,
    architecture: Option<String>,
    original_format: String,
    scriptlets: ScriptletPackageMetadata,
}
```

Add:

```rust
fn load_converted_metadata_rows(
    conn: &Connection,
    distro: &str,
) -> Result<Vec<ConvertedMetadataRow>, anyhow::Error> {
    let mut stmt = conn.prepare(
        "SELECT package_name, package_version, package_architecture, original_format,
                scriptlet_fidelity, target_compatibility, publication_status,
                evidence_digest, curation_evidence_digest,
                blocked_reason_codes_json, scriptlet_summary_json, review_artifact_path
         FROM converted_packages
         WHERE distro = ?1
           AND package_name IS NOT NULL
           AND package_version IS NOT NULL
           AND conversion_version >= ?2",
    )?;
    let mut rows = stmt.query(rusqlite::params![distro, CONVERSION_VERSION])?;
    let mut converted = Vec::new();
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        let version: String = row.get(1)?;
        let architecture: Option<String> = row.get(2)?;
        let original_format: String = row.get(3)?;
        if architecture.is_none() && original_format != "ccs" {
            continue;
        }
        let scriptlet_summary = scriptlet_summary_from_selected_columns(row)?;
        converted.push(ConvertedMetadataRow {
            name,
            version,
            architecture,
            original_format,
            scriptlets: ScriptletPackageMetadata::from(&scriptlet_summary),
        });
    }
    Ok(converted)
}
```

Add the selected-column summary helper in `index.rs`:

```rust
fn scriptlet_summary_from_selected_columns(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ScriptletBundleSummary> {
    let scriptlet_fidelity: String = row.get(4)?;
    let target_compatibility: String = row.get(5)?;
    let publication_status: String = row.get(6)?;
    let evidence_digest: Option<String> = row.get(7)?;
    let curation_evidence_digest: Option<String> = row.get(8)?;
    let blocked_reason_codes_json: String = row.get(9)?;
    let scriptlet_summary_json: String = row.get(10)?;
    let review_artifact_path: Option<String> = row.get(11)?;

    let mut summary = serde_json::from_str::<ScriptletBundleSummary>(&scriptlet_summary_json)
        .unwrap_or_else(|error| {
            tracing::warn!(
                "failed to parse converted package scriptlet summary JSON: {}",
                error
            );
            let mut fallback = ScriptletBundleSummary::default();
            fallback.blocked_reason_codes =
                serde_json::from_str(&blocked_reason_codes_json).unwrap_or_default();
            fallback
        });
    summary.scriptlet_fidelity = scriptlet_fidelity;
    summary.target_compatibility = target_compatibility;
    summary.publication_status = publication_status;
    summary.evidence_digest = evidence_digest;
    summary.curation_evidence_digest = curation_evidence_digest;
    summary.review_artifact_path = review_artifact_path;
    Ok(summary)
}
```

This mirrors `ConvertedPackage::scriptlet_summary()` fallback behavior while
avoiding public JSON mapping duplication in the handler.

Add a helper for existing repository metadata. It must preserve `metadata: None`
for unconverted packages that do not already have metadata:

```rust
fn metadata_object_from_json(
    metadata: Option<&str>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    metadata
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
}
```

In `build_metadata`, replace the current `converted_packages` vector with:

```rust
let converted_rows = load_converted_metadata_rows(&conn, distro)?;
let converted_set: HashSet<PackageKey> = converted_rows
    .iter()
    .map(|row| package_key(&row.name, &row.version, row.architecture.as_deref()))
    .collect();
let converted_by_key: HashMap<PackageKey, &ConvertedMetadataRow> = converted_rows
    .iter()
    .map(|row| {
        (
            package_key(&row.name, &row.version, row.architecture.as_deref()),
            row,
        )
    })
    .collect();
```

When mapping repo packages, prefer an explicit loop or a `Result<Vec<_>>`
collection so the `serde_json::to_value` error can be propagated cleanly:

```rust
let mut packages = Vec::new();
for pkg in repo_packages {
    let key = package_key(&pkg.name, &pkg.version, pkg.architecture.as_deref());
    let converted = converted_by_key.get(&key);
    let mut metadata = metadata_object_from_json(pkg.metadata.as_deref());

    if let Some(converted) = converted {
        let object = metadata.get_or_insert_with(serde_json::Map::new);
        object.insert(
            "scriptlets".to_string(),
            serde_json::to_value(&converted.scriptlets)?,
        );
    }

    packages.push(PackageEntry {
        converted: converted.is_some(),
        metadata: metadata.map(serde_json::Value::Object),
        // preserve the current package fields here...
    });
}
```

For converted-only packages, iterate `converted_rows` and create `PackageEntry`
for keys not already present:

```rust
metadata: Some(serde_json::json!({
    "scriptlets": converted.scriptlets.clone()
})),
```

Keep repository packages that are not converted and have no native metadata as
`metadata: None`; do not emit an empty object just because the scriptlet feature
exists.

- [ ] **Step 6: Add generated index scriptlet metadata**

Add to `VersionEntry`:

```rust
#[serde(skip_serializing_if = "Option::is_none")]
pub scriptlets: Option<ScriptletPackageMetadata>,
```

When `converted_info` exists:

```rust
scriptlets: Some(ScriptletPackageMetadata::from(&conv.scriptlet_summary())),
```

For pending entries:

```rust
scriptlets: None,
```

Add an index test that serializes an index seeded with `review_artifact_path = "/tmp/private-review-secret"` and asserts the JSON contains `"scriptlets"` but not the private path.

Use this assertion shape:

```rust
let json = serde_json::to_string(&index).unwrap();
assert!(json.contains("\"scriptlets\""));
assert!(!json.contains("review_artifact_path"));
assert!(!json.contains("private-review-secret"));
```

- [ ] **Step 7: Run Remi API/index tests**

Run:

```bash
cargo test -p remi packages
cargo test -p remi index
```

Expected: pass.

- [ ] **Step 8: Commit**

```bash
git add apps/remi/src/server/handlers/packages.rs apps/remi/src/server/handlers/index.rs apps/remi/src/server/index_gen.rs
git commit -m "feat(scriptlets): expose passive remi metadata"
```

## Task 7: Stale Row Filtering And OCI/Delta Hardening

**Files:**

- Modify: `apps/remi/src/server/handlers/oci.rs`
- Modify: `apps/remi/src/server/handlers/detail.rs`
- Modify: `apps/remi/src/server/delta_manifests.rs`
- Modify: `crates/conary-core/src/db/models/converted.rs` if additional helpers are needed

- [ ] **Step 1: Write failing OCI stale-row test**

In `oci.rs` tests, create a converted row with `conversion_version = CONVERSION_VERSION - 1` and assert manifest lookup returns not found:

```rust
#[test]
fn oci_manifest_ignores_stale_converted_rows() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("remi.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut converted = ConvertedPackage::new_server(
        "fedora".to_string(),
        "pkg".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        "high".to_string(),
        &["sha256:chunk".to_string()],
        3,
        "sha256:content".to_string(),
        temp.path().join("pkg.ccs").to_string_lossy().to_string(),
    );
    converted.conversion_version = CONVERSION_VERSION - 1;
    converted.package_architecture = Some("x86_64".to_string());
    converted.insert(&conn).unwrap();

    let chunk_cache = test_chunk_cache(temp.path());
    let result = build_manifest(&db_path, "fedora", "pkg", "1.0", &chunk_cache).unwrap();

    assert!(result.is_none());
}
```

- [ ] **Step 2: Run failing OCI test**

Run:

```bash
cargo test -p remi oci_manifest_ignores_stale_converted_rows
```

Expected: fail if the stale row is accepted.

- [ ] **Step 3: Verify OCI digest lookup uses model helper and filter stale rows**

The manual `ConvertedPackage { ... }` digest-path literal is removed in Task 4
because the model field expansion otherwise breaks Remi compilation. In this
task, verify the digest lookup still has this shape:

```rust
let converted = if let Some(ver) = version {
    ConvertedPackage::find_by_package_identity(&conn, distro, package, Some(ver))?
} else {
    ConvertedPackage::find_by_content_hash_identity(&conn, distro, package, reference)?
};

let converted = converted.and_then(|converted| {
    if converted.needs_reconversion() {
        None
    } else {
        Some(converted)
    }
});
```

There must be no remaining digest-path
`conn.query_row(..., |row| ConvertedPackage { ... })` closure. The model helper
normalizes OCI digest references by comparing both prefixed and unprefixed
`sha256` content hashes.

For list/tag/catalog queries that read `converted_packages` directly, add SQL:

```sql
AND conversion_version >= ?N
```

and bind `CONVERSION_VERSION`.

- [ ] **Step 4: Add package detail stale-row filtering tests**

In `detail.rs`, add tests proving stale rows do not make package detail report
converted status:

```rust
#[test]
fn package_detail_ignores_stale_converted_rows() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("remi.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    seed_repository_package(&conn, "fedora", "pkg", "1.0", Some("x86_64"));
    let mut converted = ConvertedPackage::new_server(
        "fedora".to_string(),
        "pkg".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        "high".to_string(),
        &[],
        3,
        "sha256:content".to_string(),
        temp.path().join("pkg.ccs").to_string_lossy().to_string(),
    );
    converted.conversion_version = CONVERSION_VERSION - 1;
    converted.package_architecture = Some("x86_64".to_string());
    converted.insert(&conn).unwrap();

    let detail = query_package_detail(&db_path, "fedora", "pkg")
        .unwrap()
        .unwrap();

    assert!(!detail.converted);
    assert!(detail.versions.iter().all(|version| !version.converted));
}

fn seed_repository_package(
    conn: &rusqlite::Connection,
    distro: &str,
    name: &str,
    version: &str,
    architecture: Option<&str>,
) {
    conn.execute(
        "INSERT INTO repositories (name, url, enabled)
         VALUES (?1, ?2, 1)",
        rusqlite::params![distro, format!("https://example.invalid/{distro}")],
    )
    .unwrap();
    let repository_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO repository_packages
         (repository_id, name, version, architecture, description, checksum, size, download_url, dependencies)
         VALUES (?1, ?2, ?3, ?4, 'test package', 'sha256:repo', 3, 'https://example.invalid/pkg.rpm', '[]')",
        rusqlite::params![repository_id, name, version, architecture],
    )
    .unwrap();
}
```

Add a companion overview stats assertion or focused test proving a stale
converted row does not increase `OverviewStats.total_converted`.

- [ ] **Step 5: Implement package detail stale-row filters**

In `query_package_detail`, change the converted count query from:

```sql
SELECT COUNT(*) FROM converted_packages
WHERE distro = ?1 AND package_name = ?2
```

to:

```sql
SELECT COUNT(*) FROM converted_packages
WHERE distro = ?1 AND package_name = ?2 AND conversion_version >= ?3
```

and bind `CONVERSION_VERSION`.

Import `CONVERSION_VERSION` from `conary_core::db::models::converted`.

In `query_versions_internal`, extend the `LEFT JOIN converted_packages cp` join:

```sql
LEFT JOIN converted_packages cp
    ON cp.package_name = rp.name
       AND cp.distro = ?{distro_idx}
       AND cp.package_version = rp.version
       AND cp.package_architecture IS rp.architecture
       AND cp.conversion_version >= ?{conversion_version_idx}
```

Add `conversion_version_idx = repo_ids.len() + 2`, push
`CONVERSION_VERSION` before the package-name parameter, and move the
package-name parameter to the next index. Add a mismatched-architecture test so
a current `x86_64` converted row does not mark an `aarch64` repository version
as converted.

In `query_overview`, add the same current-row filter to the total converted
count:

```sql
SELECT COUNT(*) FROM converted_packages
WHERE distro IS NOT NULL AND conversion_version >= ?1
```

- [ ] **Step 6: Add delta stale-row filtering tests**

In `delta_manifests.rs`, add:

- a test that seeds one stale row and one current row, then asserts the stale
  row is excluded from the delta candidate set. If the module exposes only
  higher-level functions, seed rows and assert the resulting manifest does not
  contain the stale package name.
- a cached-delta test that seeds a `delta_manifests` row for `from_version` and
  `to_version`, then marks either the source or target converted package row as
  stale. `get_delta` must return `None` for that cache hit so the public
  handler cannot serve a stale cached delta before recomputation.

- [ ] **Step 7: Implement delta stale-row filters**

Update these concrete queries:

- `get_version_chunks`: add `AND conversion_version >= ?4` to the `WHERE`
  clause and bind `CONVERSION_VERSION`.
- `compute_deltas_for_package`: add `AND conversion_version >= ?3` to the
  `SELECT DISTINCT package_version FROM converted_packages` query and bind
  `CONVERSION_VERSION`.

Any additional query over `converted_packages` used to compute deltas must include:

```sql
conversion_version >= ?N
```

or call `needs_reconversion()` before returning or computing with the row.

Cached deltas need their own current-row guard because
`apps/remi/src/server/handlers/packages.rs::get_delta` returns the cached
`delta_manifests` row before recomputing. Add a small helper such as:

```rust
use conary_core::db::models::converted::CONVERSION_VERSION;

fn versions_have_current_conversions(
    conn: &Connection,
    distro: &str,
    package_name: &str,
    from_version: &str,
    to_version: &str,
) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT package_version)
         FROM converted_packages
         WHERE distro = ?1
           AND package_name = ?2
           AND package_version IN (?3, ?4)
           AND conversion_version >= ?5",
        params![distro, package_name, from_version, to_version, CONVERSION_VERSION],
        |row| row.get(0),
    )?;
    Ok(count == 2)
}
```

Call this helper inside `get_delta` before returning a cached row. If it returns
false, ignore the cache entry and return `Ok(None)` so the handler takes the
compute path or reports no delta from current conversions.

- [ ] **Step 8: Run stale-row tests**

Run:

```bash
cargo test -p remi oci
cargo test -p remi detail
cargo test -p remi delta_manifests
cargo test -p remi sparse
cargo test -p remi routes
```

Expected: pass.

- [ ] **Step 9: Commit**

```bash
git add apps/remi/src/server/handlers/oci.rs apps/remi/src/server/handlers/detail.rs apps/remi/src/server/delta_manifests.rs crates/conary-core/src/db/models/converted.rs
git commit -m "fix(remi): ignore stale converted scriptlet rows"
```

## Task 8: Archive Round Trip, CCS Query Guard, And Full Verification

**Files:**

- Modify tests in `crates/conary-core/src/ccs/convert/converter.rs`
- Modify tests in `apps/conary/tests/` only if current CLI snapshots require updated expected JSON/text
- Modify: `docs/modules/remi.md`
- No production install/update/remove files

- [ ] **Step 1: Add archive round-trip test**

Add a test that converts a package, reads the resulting `.ccs`, and proves `legacy_scriptlets` survived:

```rust
#[test]
fn converted_ccs_archive_round_trip_preserves_legacy_scriptlet_bundle() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.scriptlets = vec![Scriptlet {
        phase: ScriptletPhase::PostInstall,
        interpreter: "/bin/sh".to_string(),
        content: "/sbin/ldconfig\n".to_string(),
        flags: None,
    }];
    let converter = passive_test_converter(temp_dir.path());
    let result = converter
        .convert(
            &metadata,
            &make_test_files(),
            "rpm",
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        )
        .unwrap();
    let package_path = result.package_path.as_ref().unwrap();

    let file = std::fs::File::open(package_path).unwrap();
    let archive = crate::ccs::archive_reader::read_ccs_archive(file).unwrap();
    let manifest = archive.manifest;

    assert!(manifest.legacy_scriptlets.is_some());
    manifest.legacy_scriptlets.as_ref().unwrap().validate().unwrap();
}
```

- [ ] **Step 2: Run archive and query tests**

Run:

```bash
cargo test -p conary-core converted_ccs_archive_round_trip_preserves_legacy_scriptlet_bundle
cargo test -p conary query_scripts
```

Expected: pass after any expected-output updates. Native RPM/DEB/Arch query script defaults must remain unchanged.

- [ ] **Step 3: Run targeted test suite**

Run:

```bash
cargo test -p conary-core legacy_scriptlets
cargo test -p conary-core scriptlet_bundle
cargo test -p conary-core conversion_integration
cargo test -p conary-core converted_package
cargo test -p conary-core
cargo test -p conary
cargo test -p remi conversion
cargo test -p remi packages
cargo test -p remi index
cargo test -p remi oci
cargo test -p remi detail
cargo test -p remi sparse
cargo test -p remi delta_manifests
cargo test -p remi routes
cargo test -p remi
```

Expected: all pass.

- [ ] **Step 4: Update Remi module documentation**

Update `docs/modules/remi.md` with:

- `converted_packages` stores passive scriptlet fidelity/publication metadata
  for Goal 4 conversions;
- public package, metadata, and generated-index responses expose a sanitized
  `scriptlets` object;
- `review_artifact_path` remains private and is represented publicly only as
  `review_artifact_available`;
- Goal 4 does not gate downloads or publication by `publication_status`.

- [ ] **Step 5: Run workspace quality gates**

Run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

Expected: all pass.

- [ ] **Step 6: Scope audit**

Run:

```bash
git diff --stat origin/main..HEAD
git diff --name-only origin/main..HEAD
git diff origin/main..HEAD -- crates/conary-core/src/ccs/convert apps/remi docs/modules/remi.md | \
    rg -n "publication_status|legacy_scriptlets|ScriptletDecision::Legacy|legacy-replay|local-only|review_artifact_path"
```

Confirm:

- no install/update/remove execution path consumes `legacy_scriptlets`;
- no Remi download/publication endpoint rejects packages because of `publication_status`;
- no Goal 4 code emits `ScriptletDecision::Legacy`;
- no Goal 4 code emits `scriptlet_fidelity = "legacy-replay"`;
- no public JSON serialization includes `review_artifact_path`.
- schema enum definitions, validation tests, and pre-existing fixtures are
  allowed to mention reserved values when they are not emitted by Goal 4 code.

- [ ] **Step 7: Commit final test or snapshot updates**

```bash
git add crates apps docs/modules/remi.md
git commit -m "test(scriptlets): verify passive bundle metadata flow"
```

If no files changed in this task, record that in the implementation notes and do not create an empty commit.

## Final Verification Before Merge

Run:

```bash
git status --short --branch
cargo test -p conary-core legacy_scriptlets
cargo test -p conary-core scriptlet_bundle
cargo test -p conary-core conversion_integration
cargo test -p conary-core converted_package
cargo test -p conary-core
cargo test -p conary
cargo test -p remi conversion
cargo test -p remi packages
cargo test -p remi index
cargo test -p remi oci
cargo test -p remi detail
cargo test -p remi sparse
cargo test -p remi delta_manifests
cargo test -p remi routes
cargo test -p remi
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

Expected final state:

- converted CCS manifests contain `legacy_scriptlets`;
- bundle validation rejects tampered bodies;
- native-free packages get zero-entry bundles;
- Remi DB rows store passive scriptlet metadata;
- Remi package, metadata, and generated index responses expose public scriptlet summary metadata;
- stale pre-Goal-4 conversion rows are not served or indexed as current;
- publication/download behavior is otherwise unchanged;
- working tree is clean after commit.

## Implementation Handoff

Recommended execution mode: **Subagent-Driven**.

Use a fresh feature worktree before code changes, then implement one task at a time. Review the diff and run the task-specific tests between tasks. After Task 8 and full verification pass, merge, push, and remove the feature worktree.
