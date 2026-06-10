# Phase 20 CCS Scriptlet Bundle Decomposition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decompose `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs` from a 2,178-line hotspot into a stable public hub plus focused child modules for bundle types, entry construction, classification decisions, native metadata projection, digesting, summaries, and fixtures without changing conversion output or public API paths.

**Architecture:** Keep `scriptlet_bundle.rs` as the public `crate::ccs::convert::scriptlet_bundle` module and re-export point for the existing bundle API. Move implementation details into child modules under `crates/conary-core/src/ccs/convert/scriptlet_bundle/` using `pub(super)` only for cross-child helpers. Preserve the existing `crate::ccs::convert::{ScriptletBundleBuild, ScriptletBundleInput, ScriptletBundleSummary, ScriptletDecisionCountsSummary, build_legacy_scriptlet_bundle}` exports from `convert/mod.rs`.

**Tech Stack:** Rust 2024, `serde`, `serde_json`, `toml`, existing `conary-core` CCS conversion types, legacy scriptlet bundle model, native ABI metadata, canonical JSON hashing.

## Current Repo Facts To Preserve

- `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs` is 2,178 lines and is currently the largest Rust hotspot after Phase 19.
- `crates/conary-core/src/ccs/convert/mod.rs` currently declares `pub mod scriptlet_bundle;` and re-exports:

```rust
pub use scriptlet_bundle::{
    ScriptletBundleBuild, ScriptletBundleInput, ScriptletBundleSummary,
    ScriptletDecisionCountsSummary, build_legacy_scriptlet_bundle,
};
```

- `crates/conary-core/src/ccs/convert/converter.rs` imports:

```rust
use crate::ccs::convert::scriptlet_bundle::{
    ScriptletBundleInput, ScriptletBundleSummary, build_legacy_scriptlet_bundle,
};
```

- External workspace callers import `ScriptletBundleSummary` from `conary_core::ccs::convert` in Remi handlers, persistence, search, publication, prewarm, sparse handlers, and converted-package models. These paths must continue to resolve.
- Baseline direct test inventory:
  - `cargo test -p conary-core --lib ccs::convert::scriptlet_bundle -- --list` lists exactly 15 tests.
  - `cargo test -p conary-core --lib ccs::convert -- --list` lists exactly 138 tests.
- Baseline direct `scriptlet_bundle` tests:

```text
ccs::convert::scriptlet_bundle::tests::arch_alpm_hook_control_artifact_validates_with_placeholder_interpreter
ccs::convert::scriptlet_bundle::tests::blocked_classification_becomes_blocked_entry
ccs::convert::scriptlet_bundle::tests::digest_changes_when_classification_evidence_changes
ccs::convert::scriptlet_bundle::tests::flattened_scriptlet_with_complete_effect_builds_replaced_entry
ccs::convert::scriptlet_bundle::tests::format_metadata_boundaries_become_review_required_with_registry_reasons
ccs::convert::scriptlet_bundle::tests::format_specific_metadata_projects_into_bundle
ccs::convert::scriptlet_bundle::tests::native_abi_binary_body_is_base64_encoded_and_validates
ccs::convert::scriptlet_bundle::tests::native_deferred_and_unpreservable_support_drive_decisions
ccs::convert::scriptlet_bundle::tests::native_free_input_builds_zero_entry_bundle
ccs::convert::scriptlet_bundle::tests::review_classification_becomes_private_review_entry
ccs::convert::scriptlet_bundle::tests::scriptlet_bundle_summary_defaults_match_legacy_rows
ccs::convert::scriptlet_bundle::tests::scriptlet_bundle_summary_does_not_serialize_review_artifact_path
ccs::convert::scriptlet_bundle::tests::scriptlet_bundle_summary_from_bundle_is_public_api
ccs::convert::scriptlet_bundle::tests::tampered_body_after_build_fails_strict_bundle_validation
ccs::convert::scriptlet_bundle::tests::unknown_classification_becomes_source_native_legacy_replay_entry
```

- Baseline docs-audit inventory before locking this plan:
  - `LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l` returns `163`.
  - Ledger counts are `archived 73`, `corrected 63`, `retained-historical 14`, `verified-no-change 13`.
- After locking in this plan file, the docs-audit inventory must be `164` tracked doc-like files and the ledger must have `64` `corrected` rows.

## Desired End State

```text
crates/conary-core/src/ccs/convert/scriptlet_bundle.rs
crates/conary-core/src/ccs/convert/scriptlet_bundle/builder.rs
crates/conary-core/src/ccs/convert/scriptlet_bundle/classification.rs
crates/conary-core/src/ccs/convert/scriptlet_bundle/digest.rs
crates/conary-core/src/ccs/convert/scriptlet_bundle/entries.rs
crates/conary-core/src/ccs/convert/scriptlet_bundle/format_metadata.rs
crates/conary-core/src/ccs/convert/scriptlet_bundle/native_contracts.rs
crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs
crates/conary-core/src/ccs/convert/scriptlet_bundle/test_support.rs
crates/conary-core/src/ccs/convert/scriptlet_bundle/types.rs
```

Final `scriptlet_bundle.rs` should contain only:

- path comment and module docs,
- child module declarations,
- `#[cfg(test)] mod test_support;`,
- public re-exports for the existing public API,
- no bundle construction body,
- no digest helpers,
- no metadata projection helpers,
- no parent `#[cfg(test)] mod tests`.

Sketch:

```rust
// conary-core/src/ccs/convert/scriptlet_bundle.rs
//! Passive legacy scriptlet bundle construction for legacy package conversion.

mod builder;
mod classification;
mod digest;
mod entries;
mod format_metadata;
mod native_contracts;
mod summary;
#[cfg(test)]
mod test_support;
mod types;

pub use builder::build_legacy_scriptlet_bundle;
pub use types::{
    ScriptletBundleBuild, ScriptletBundleInput, ScriptletBundleSummary,
    ScriptletDecisionCountsSummary,
};
```

## Design Choice

Three decomposition paths were considered:

1. **Public hub with domain child modules.** This is the recommended path. It preserves the public module path while separating the real responsibilities: public DTOs, construction orchestration, entry decisions, native ABI projection, digest construction, summaries, and fixtures.
2. **One child module per package format.** This would isolate RPM, DEB, and Arch metadata, but most logic is shared across formats and the entry/digest tests would still need broad imports.
3. **Move only tests and digest helpers.** This lowers line count but leaves the hotspot responsible for entry construction, status aggregation, and native metadata projection, which is the complexity we need to make reviewable.

Use option 1.

## Visibility Contract

- `ScriptletBundleInput`, `ScriptletBundleBuild`, `ScriptletBundleSummary`, `ScriptletDecisionCountsSummary`, and `build_legacy_scriptlet_bundle` remain public through:
  - `crate::ccs::convert::scriptlet_bundle::*`
  - `crate::ccs::convert::*`
- `types.rs` defines the public structs. The module itself can stay private because `scriptlet_bundle.rs` re-exports its public types.
- `builder::build_legacy_scriptlet_bundle` must be `pub` because it is re-exported from `scriptlet_bundle.rs` and `convert/mod.rs`.
- `entries::build_entries` must be `pub(super)` because `builder.rs` calls it.
- `digest::evidence_digest` must be `pub(super)` because `builder.rs` calls it.
- `summary::{summary_from_bundle, decision_counts, aggregate_status}` must be `pub(super)` because `builder.rs` calls them and `summary.rs` owns `ScriptletBundleSummary::from_bundle`.
- `classification::{classification_entries_for, classify_entry}` must be `pub(super)` because `entries.rs` calls them.
- `classification::scriptlet_effect_from_evidence` stays private because only `classification.rs` calls it.
- `classification::EntryOutcome` must be `pub(super)` with `pub(super)` fields because `classify_entry` returns it to `entries.rs`.
- `format_metadata::project_format_metadata` must be `pub(super)` because `entries.rs` calls it.
- `native_contracts` conversion helpers used across child modules must be `pub(super)`:
  - `encoded_native_body`
  - `flat_transaction_order`
  - `native_invocation`
  - `native_lifecycle_paths`
  - `native_scriptlet_kind`
  - `native_stdin`
  - `native_transaction_order`
  - `native_transaction_position`
  - `non_empty_or_default`
  - `phase_from_native_lifecycle`
  - `phase_from_scriptlet_phase`
- `test_support.rs` stays behind `#[cfg(test)] mod test_support;`. Fixture helpers inside it should be `pub(super)` so sibling child test modules can import them through `super::super::test_support`.
- Rust privacy note: private items are visible only to their defining module and descendants. Sibling child modules under `scriptlet_bundle/` need explicit `pub(super)` helper exports when they call each other through the parent module.

## Test Redistribution

Move the 15 direct tests exactly once:

| Module | Count | Tests |
| --- | ---: | --- |
| `summary.rs` | 3 | `scriptlet_bundle_summary_defaults_match_legacy_rows`, `scriptlet_bundle_summary_does_not_serialize_review_artifact_path`, `scriptlet_bundle_summary_from_bundle_is_public_api` |
| `builder.rs` | 2 | `native_free_input_builds_zero_entry_bundle`, `tampered_body_after_build_fails_strict_bundle_validation` |
| `entries.rs` | 6 | `flattened_scriptlet_with_complete_effect_builds_replaced_entry`, `native_abi_binary_body_is_base64_encoded_and_validates`, `unknown_classification_becomes_source_native_legacy_replay_entry`, `review_classification_becomes_private_review_entry`, `blocked_classification_becomes_blocked_entry`, `native_deferred_and_unpreservable_support_drive_decisions` |
| `format_metadata.rs` | 3 | `format_metadata_boundaries_become_review_required_with_registry_reasons`, `format_specific_metadata_projects_into_bundle`, `arch_alpm_hook_control_artifact_validates_with_placeholder_interpreter` |
| `digest.rs` | 1 | `digest_changes_when_classification_evidence_changes` |

Final inventories:

- `cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::summary::tests -- --list` returns 3 tests.
- `cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::builder::tests -- --list` returns 2 tests.
- `cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::entries::tests -- --list` returns 6 tests.
- `cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::format_metadata::tests -- --list` returns 3 tests.
- `cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::digest::tests -- --list` returns 1 test.
- `cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::tests -- --list` returns 0 tests.
- `cargo test -p conary-core --lib ccs::convert::scriptlet_bundle -- --list` still returns 15 tests.

## Non-Goals

- Do not change bundle TOML, JSON summaries, canonical digest input, evidence digest prefixing, decision counts, publication policy, target compatibility, source format mapping, or validation behavior.
- Do not change `LegacyScriptletBundle`, `LegacyScriptletEntry`, native ABI metadata, adapter evidence, support-matrix rows, or blocked-class classification behavior.
- Do not change `convert/mod.rs` public re-export names.
- Do not add new conversion behavior, new scriptlet decisions, or new publication status values.
- Do not change Remi publication metadata or admin handler behavior.
- Do not alter golden conversion fixture semantics.

## Task 0: Lock In This Plan

**Files:**

- `docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase20-ccs-scriptlet-bundle-decomposition-plan.md`
- `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- `docs/superpowers/documentation-accuracy-audit-summary.md`

**Steps:**

- [ ] Stage this plan file.
- [ ] Add a `corrected` ledger row for this plan file with exactly 9 tab-separated columns.
- [ ] The plan ledger row must use:

```text
origin_path = docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase20-ccs-scriptlet-bundle-decomposition-plan.md
path = docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase20-ccs-scriptlet-bundle-decomposition-plan.md
family = planning
audience = maintainer
status = verified
disposition = corrected
```

- [ ] Stage the ledger update after adding the row.
- [ ] Regenerate the tracked docs-audit inventory:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] Update `docs/superpowers/documentation-accuracy-audit-summary.md` so the latest maintainability planning note includes Phase 20 and the counts move to `164` tracked files / `64` corrected rows.
- [ ] Stage the inventory and summary updates.
- [ ] Use this evidence source set in the ledger row:

```text
crates/conary-core/src/ccs/convert/scriptlet_bundle.rs; crates/conary-core/src/ccs/convert/mod.rs; crates/conary-core/src/ccs/convert/converter.rs; crates/conary-core/src/ccs/legacy_scriptlets.rs; crates/conary-core/src/ccs/convert/effects.rs; crates/conary-core/src/packages/native_abi.rs; docs/modules/ccs.md; docs/modules/feature-ownership.md; docs/llms/subsystem-map.md
```

- [ ] Suggested ledger tags:

```text
maintainability; phase20; ccs-convert; scriptlet-bundle; hotspot-decomposition
```

- [ ] Suggested ledger note:

```text
Added the Phase 20 CCS scriptlet bundle decomposition plan for turning crates/conary-core/src/ccs/convert/scriptlet_bundle.rs into a public hub plus child modules for public bundle types, construction orchestration, entry decisions, native ABI metadata projection, evidence digesting, summaries, and fixtures without changing conversion output or public API paths.
```

- [ ] Run:

```bash
git diff --check
git diff --cached --check
bash -n scripts/docs-audit-inventory.sh scripts/check-doc-audit-ledger.sh
awk -F'\t' 'NF != 9 { print NR ":" NF ":" $0 }' docs/superpowers/documentation-accuracy-audit-ledger.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected after staging this plan:

```text
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
# 164 tracked doc-like files. This happens to equal the ledger row total after
# this plan is added, but inventory count and ledger row count are not a general
# invariant because archived ledger rows remain in the ledger while archived
# paths can be outside the active inventory.

ledger disposition count command
# archived 73
# corrected 64
# retained-historical 14
# verified-no-change 13
```

- [ ] Commit:

```bash
git add docs/superpowers/plans/archive/2026-06-09-project-maintainability-phase20-ccs-scriptlet-bundle-decomposition-plan.md \
    docs/superpowers/documentation-accuracy-audit-ledger.tsv \
    docs/superpowers/documentation-accuracy-audit-inventory.tsv \
    docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: plan ccs scriptlet bundle decomposition"
```

## Task 1: Extract Public Types And Summaries

**Files:**

- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`
- Create: `crates/conary-core/src/ccs/convert/scriptlet_bundle/types.rs`
- Create: `crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs`
- Create: `crates/conary-core/src/ccs/convert/scriptlet_bundle/test_support.rs`

**`types.rs` contents to move:**

- `ScriptletBundleInput`
- `ScriptletBundleBuild`
- `ScriptletBundleSummary`
- `impl Default for ScriptletBundleSummary`
- `ScriptletDecisionCountsSummary`

**`types.rs` import surface:**

```rust
// conary-core/src/ccs/convert/scriptlet_bundle/types.rs

use crate::ccs::convert::effects::ScriptletClassificationReport;
use crate::ccs::legacy_scriptlets::LegacyScriptletBundle;
use crate::packages::common::PackageMetadata;
use crate::packages::traits::ExtractedFile;
use serde::{Deserialize, Serialize};
```

**`summary.rs` contents to move:**

- `impl ScriptletBundleSummary { pub fn from_bundle(bundle: &LegacyScriptletBundle, evidence_digest: Option<String>) -> Self }`
- `summary_from_bundle`
- `sorted_entry_reason_codes`
- `decision_counts`
- `aggregate_status`

**`summary.rs` import surface:**

```rust
// conary-core/src/ccs/convert/scriptlet_bundle/summary.rs

use super::types::{ScriptletBundleSummary, ScriptletDecisionCountsSummary};
use crate::ccs::legacy_scriptlets::{
    DecisionCounts, LegacyScriptletBundle, LegacyScriptletEntry, PublicationPolicy,
    PublicationStatus, ScriptletDecision, ScriptletFidelity, TargetCompatibility,
};
use std::collections::BTreeSet;
```

**Visibility updates:**

- `summary_from_bundle` becomes `pub(super) fn summary_from_bundle`.
- `decision_counts` becomes `pub(super) fn decision_counts`.
- `aggregate_status` becomes `pub(super) fn aggregate_status`.
- `sorted_entry_reason_codes` stays private in `summary.rs`.

**Hub updates:**

```rust
mod summary;
#[cfg(test)]
mod test_support;
mod types;

pub use types::{
    ScriptletBundleBuild, ScriptletBundleInput, ScriptletBundleSummary,
    ScriptletDecisionCountsSummary,
};
```

While `build_legacy_scriptlet_bundle` still lives in `scriptlet_bundle.rs`, add private imports from the new children:

```rust
use summary::{aggregate_status, decision_counts, summary_from_bundle};
```

Do not add a private `use types::{...};` import next to the `pub use types::{...};`
re-export. Rust treats that as a duplicate definition in the parent type
namespace (`E0252`). The public re-exported type names are already available to
the parent module body.

**`test_support.rs` fixture contents to move:**

- `package_metadata`
- `complete_effect`
- `known_report_with_effect`
- `bundle_for_metadata`
- `native_entry_with_body`
- `rpm_trigger_entry`
- `deb_triggers_entry`
- `arch_install_entry`
- `arch_alpm_hook_entry`

**`test_support.rs` import surface:**

```rust
// conary-core/src/ccs/convert/scriptlet_bundle/test_support.rs

use super::{ScriptletBundleBuild, ScriptletBundleInput, build_legacy_scriptlet_bundle};
use crate::ccs::convert::effects::{
    ScriptletClassification, ScriptletClassificationReport, ScriptletEffectEvidence,
};
use crate::ccs::legacy_scriptlets::{EffectConfidence, EffectReplacement, EffectSource};
use crate::packages::common::PackageMetadata;
use crate::packages::native_abi::*;
use crate::packages::traits::{ExtractedFile, ScriptletPhase};
use std::collections::BTreeMap;
use std::path::PathBuf;
```

**`test_support.rs` visibility:**

Every fixture helper listed above must be `pub(super)`.

**Move these tests to `summary.rs`:**

- `scriptlet_bundle_summary_defaults_match_legacy_rows`
- `scriptlet_bundle_summary_does_not_serialize_review_artifact_path`
- `scriptlet_bundle_summary_from_bundle_is_public_api`

**`summary.rs` test imports:**

```rust
#[cfg(test)]
mod tests {
    use super::super::test_support::{bundle_for_metadata, package_metadata};
    use super::super::{ScriptletBundleSummary, ScriptletDecisionCountsSummary};
    use crate::ccs::convert::effects::ScriptletClassificationReport;
}
```

The moved `scriptlet_bundle_summary_does_not_serialize_review_artifact_path` test uses `serde_json::to_string` through the fully-qualified `serde_json::to_string(&summary)` path, so no `use serde_json;` import is required.

- [ ] Create `types.rs`, `summary.rs`, and `test_support.rs` with path comments.
- [ ] Ensure the child module directory exists before creating child files:

```bash
mkdir -p crates/conary-core/src/ccs/convert/scriptlet_bundle
```

- [ ] Move the listed structs, impls, summary helpers, fixtures, and tests.
- [ ] Remove the moved code from the parent `scriptlet_bundle.rs`.
- [ ] Keep all unmoved tests in parent `scriptlet_bundle.rs` for now, importing fixtures from `test_support` with this temporary test import block:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super::test_support::{
        arch_alpm_hook_entry, arch_install_entry, bundle_for_metadata, complete_effect,
        deb_triggers_entry, known_report_with_effect, native_entry_with_body, package_metadata,
        rpm_trigger_entry,
    };
    use crate::ccs::convert::effects::{ScriptletClassification, ScriptletClassificationReport};
    use crate::ccs::legacy_scriptlets::{
        EffectReplacement, ForeignReplayPolicy, PublicationPolicy, PublicationStatus,
        ScriptletDecision, ScriptletFidelity, TargetCompatibility,
    };
    use crate::packages::native_abi::NativeScriptletSupport;
    use crate::packages::traits::{Scriptlet, ScriptletPhase};
}
```
- [ ] Run:

```bash
cargo fmt
cargo check -p conary-core
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::summary::tests -- --list
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::summary::tests
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle -- --list
```

Expected:

- summary test list returns 3 tests.
- direct scriptlet bundle inventory still returns 15 tests.

- [ ] Commit:

```bash
git add crates/conary-core/src/ccs/convert/scriptlet_bundle.rs \
    crates/conary-core/src/ccs/convert/scriptlet_bundle/types.rs \
    crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs \
    crates/conary-core/src/ccs/convert/scriptlet_bundle/test_support.rs
git commit -m "refactor(core): extract scriptlet bundle public types"
```

## Task 2: Extract Classification And Native Contract Mapping

**Files:**

- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`
- Create: `crates/conary-core/src/ccs/convert/scriptlet_bundle/classification.rs`
- Create: `crates/conary-core/src/ccs/convert/scriptlet_bundle/native_contracts.rs`

**`classification.rs` contents to move:**

- `EntryOutcome`
- `classify_entry`
- `classification_entries_for`
- `scriptlet_effect_from_evidence`

**`classification.rs` import surface:**

```rust
// conary-core/src/ccs/convert/scriptlet_bundle/classification.rs

use crate::ccs::convert::effects::{
    EntryClassification, ScriptletClassification, ScriptletClassificationReport,
    ScriptletEffectEvidence,
};
use crate::ccs::legacy_scriptlets::{EffectReplacement, ScriptletDecision, ScriptletEffect};
use crate::packages::native_abi::NativeScriptletSupport;
use std::collections::BTreeSet;
```

**Visibility updates:**

```rust
pub(super) struct EntryOutcome {
    pub(super) decision: ScriptletDecision,
    pub(super) reason_code: String,
    pub(super) effects: Vec<ScriptletEffect>,
    pub(super) unknown_commands: Vec<String>,
    pub(super) blocked_classes: Vec<String>,
}

pub(super) fn classify_entry
pub(super) fn classification_entries_for
```

Keep `scriptlet_effect_from_evidence` private because only `classify_entry` calls it inside `classification.rs`.

**`native_contracts.rs` contents to move:**

- `encoded_native_body`
- `native_invocation`
- `native_transaction_order`
- `flat_transaction_order`
- `phase_from_scriptlet_phase`
- `phase_from_native_lifecycle`
- `native_lifecycle_paths`
- `non_empty_or_default`
- `native_argument_contract`
- `native_argument_value`
- `native_stdin`
- `native_root`
- `native_transaction_position`
- `native_scriptlet_kind`

**`native_contracts.rs` import surface:**

```rust
// conary-core/src/ccs/convert/scriptlet_bundle/native_contracts.rs

use crate::ccs::legacy_scriptlets::{LifecyclePath, NativeInvocation, TransactionOrder};
use crate::packages::native_abi::{
    NativeArgumentContract, NativeArgumentValue, NativeInvocationContract, NativeLifecyclePath,
    NativeRootExpectation, NativeScriptletBody, NativeScriptletBodyEncoding,
    NativeScriptletEntry, NativeScriptletKind, NativeStdinContract, NativeTransactionOrder,
    NativeTransactionPosition,
};
use crate::packages::traits::ScriptletPhase;
use std::collections::BTreeMap;
```

**Visibility updates:**

Use `pub(super)` for these functions because later child modules call them:

```rust
pub(super) fn encoded_native_body
pub(super) fn native_invocation
pub(super) fn native_transaction_order
pub(super) fn flat_transaction_order
pub(super) fn phase_from_scriptlet_phase
pub(super) fn phase_from_native_lifecycle
pub(super) fn native_lifecycle_paths
pub(super) fn non_empty_or_default
pub(super) fn native_stdin
pub(super) fn native_transaction_position
pub(super) fn native_scriptlet_kind
```

Keep these private because only `native_contracts.rs` uses them:

```rust
fn native_argument_contract
fn native_argument_value
fn native_root
```

**Hub temporary imports while entry construction still lives in `scriptlet_bundle.rs`:**

```rust
use classification::{classification_entries_for, classify_entry};
use native_contracts::{
    encoded_native_body, flat_transaction_order, native_invocation, native_lifecycle_paths,
    native_scriptlet_kind, native_stdin, native_transaction_order, native_transaction_position,
    non_empty_or_default, phase_from_native_lifecycle, phase_from_scriptlet_phase,
};
```

- [ ] Create `classification.rs` and `native_contracts.rs` with path comments.
- [ ] Move the listed helpers.
- [ ] Remove only parent imports made unused by this task. Keep `ScriptletEffectEvidence` and `BTreeSet` in the parent until Task 5 extracts digest helpers. Keep `BTreeMap` in the parent until Task 6 extracts `build_legacy_scriptlet_bundle`.
- [ ] Run:

```bash
cargo fmt
cargo check -p conary-core
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle -- --list
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle
```

Expected direct inventory and test execution remain 15 tests.

- [ ] Commit:

```bash
git add crates/conary-core/src/ccs/convert/scriptlet_bundle.rs \
    crates/conary-core/src/ccs/convert/scriptlet_bundle/classification.rs \
    crates/conary-core/src/ccs/convert/scriptlet_bundle/native_contracts.rs
git commit -m "refactor(core): extract scriptlet bundle classification helpers"
```

## Task 3: Extract Native Format Metadata Projection

**Files:**

- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`
- Create: `crates/conary-core/src/ccs/convert/scriptlet_bundle/format_metadata.rs`

**`format_metadata.rs` contents to move:**

- `project_format_metadata`
- `project_rpm_metadata`
- `project_deb_metadata`
- `project_arch_install_metadata`
- `rpm_trigger_family`
- `rpm_trigger_action`
- `deb_control_member`
- `deb_maintainer_mode`
- `deb_trigger_declaration_value`
- `deb_trigger_directive`
- `deb_trigger_await_mode`
- `arch_function_extraction_status`
- `arch_alpm_hook_value`
- `arch_alpm_hook_trigger_value`
- `arch_alpm_hook_action_value`
- `arch_alpm_hook_operation`
- `arch_alpm_hook_trigger_type`
- `toml_string_array`

**`format_metadata.rs` import surface:**

```rust
// conary-core/src/ccs/convert/scriptlet_bundle/format_metadata.rs

use super::native_contracts::{native_stdin, native_transaction_position};
use crate::ccs::legacy_scriptlets::{
    ArchInstallMetadata, DebMaintainerMetadata, RpmTriggerMetadata as BundleRpmTriggerMetadata,
    RpmTriggerTargetConstraint,
};
use crate::packages::native_abi::{
    ArchAlpmHookAction, ArchAlpmHookMetadata, ArchAlpmHookOperation, ArchAlpmHookTrigger,
    ArchAlpmHookTriggerType, ArchFunctionExtractionStatus, ArchInstallScriptletMetadata,
    ArchNativeScriptletMetadata, DebControlMember, DebMaintainerMode,
    DebNativeScriptletMetadata, DebTriggerAwaitMode, DebTriggerDeclaration, DebTriggerDirective,
    NativeInvocationContract, NativeScriptletBody, NativeScriptletEntry, NativeScriptletMetadata,
    NativeStdinContract, NativeTransactionOrder, RpmNativeScriptletMetadata, RpmTriggerAction,
    RpmTriggerFamily,
};
use std::collections::{BTreeMap, BTreeSet};
```

**Visibility updates:**

- `project_format_metadata` becomes `pub(super) fn project_format_metadata`.
- All other moved helpers stay private in `format_metadata.rs`.

After adding `ArchInstallScriptletMetadata` to the import list, shorten the
`project_arch_install_metadata` signature to:

```rust
fn project_arch_install_metadata(
    metadata: &ArchInstallScriptletMetadata,
    extra: &mut BTreeMap<String, toml::Value>,
) -> ArchInstallMetadata
```

**Hub temporary import while `build_native_entry` still lives in `scriptlet_bundle.rs`:**

```rust
use format_metadata::project_format_metadata;
```

**Move these tests to `format_metadata.rs`:**

- `format_metadata_boundaries_become_review_required_with_registry_reasons`
- `format_specific_metadata_projects_into_bundle`
- `arch_alpm_hook_control_artifact_validates_with_placeholder_interpreter`

**`format_metadata.rs` test imports:**

```rust
#[cfg(test)]
mod tests {
    use super::super::test_support::{
        arch_alpm_hook_entry, arch_install_entry, bundle_for_metadata, deb_triggers_entry,
        package_metadata, rpm_trigger_entry,
    };
    use crate::ccs::convert::effects::{ScriptletClassification, ScriptletClassificationReport};
    use crate::ccs::legacy_scriptlets::{
        PublicationStatus, ScriptletDecision, ScriptletFidelity, TargetCompatibility,
    };
}
```

The listed imports are used across the three moved tests; do not add `NativeScriptletSupport` to this block because the helper fixtures hide that type.

- [ ] Create `format_metadata.rs` with a path comment.
- [ ] Move the listed helpers and tests.
- [ ] Remove now-unused parent imports for native format metadata types that are no longer referenced by remaining parent functions.
- [ ] Run:

```bash
cargo fmt
cargo check -p conary-core
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::format_metadata::tests -- --list
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::format_metadata::tests
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle -- --list
```

Expected:

- format metadata test list returns 3 tests.
- direct scriptlet bundle inventory still returns 15 tests.

- [ ] Commit:

```bash
git add crates/conary-core/src/ccs/convert/scriptlet_bundle.rs \
    crates/conary-core/src/ccs/convert/scriptlet_bundle/format_metadata.rs
git commit -m "refactor(core): extract scriptlet bundle metadata projection"
```

## Task 4: Extract Entry Construction

**Files:**

- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`
- Create: `crates/conary-core/src/ccs/convert/scriptlet_bundle/entries.rs`

**`entries.rs` contents to move:**

- `build_entries`
- `build_flat_entry`
- `build_native_entry`

**`entries.rs` import surface:**

```rust
// conary-core/src/ccs/convert/scriptlet_bundle/entries.rs

use super::classification::{classification_entries_for, classify_entry};
use super::format_metadata::project_format_metadata;
use super::native_contracts::{
    encoded_native_body, flat_transaction_order, native_invocation, native_lifecycle_paths,
    native_scriptlet_kind, native_transaction_order, non_empty_or_default,
    phase_from_native_lifecycle, phase_from_scriptlet_phase,
};
use super::types::ScriptletBundleInput;
use crate::ccs::convert::effects::ScriptletClassificationReport;
use crate::ccs::legacy_scriptlets::{LegacyScriptletEntry, NativeInvocation};
use crate::packages::native_abi::{NativeScriptletEntry, NativeScriptletSupport};
use crate::packages::traits::{Scriptlet, ScriptletPhase};
use std::collections::BTreeMap;
```

**Visibility updates:**

- `build_entries` becomes `pub(super) fn build_entries`.
- `build_flat_entry` and `build_native_entry` stay private in `entries.rs`.

**Parent temporary import while `build_legacy_scriptlet_bundle` still lives in `scriptlet_bundle.rs`:**

```rust
use entries::build_entries;
```

**Move these tests to `entries.rs`:**

- `flattened_scriptlet_with_complete_effect_builds_replaced_entry`
- `native_abi_binary_body_is_base64_encoded_and_validates`
- `unknown_classification_becomes_source_native_legacy_replay_entry`
- `review_classification_becomes_private_review_entry`
- `blocked_classification_becomes_blocked_entry`
- `native_deferred_and_unpreservable_support_drive_decisions`

**`entries.rs` test imports:**

```rust
#[cfg(test)]
mod tests {
    use super::super::test_support::{
        bundle_for_metadata, complete_effect, native_entry_with_body, package_metadata,
    };
    use crate::ccs::convert::effects::{ScriptletClassification, ScriptletClassificationReport};
    use crate::ccs::legacy_scriptlets::{
        ForeignReplayPolicy, PublicationPolicy, PublicationStatus, ScriptletDecision,
        ScriptletFidelity, TargetCompatibility,
    };
    use crate::packages::native_abi::NativeScriptletSupport;
    use crate::packages::traits::{Scriptlet, ScriptletPhase};
}
```

- [ ] Create `entries.rs` with a path comment.
- [ ] Move the listed entry construction helpers and tests.
- [ ] Remove now-unused parent imports for `Scriptlet`, `ScriptletPhase`, `LegacyScriptletEntry`, `NativeInvocation`, `NativeScriptletEntry`, and `BTreeMap` if they are no longer referenced by parent code.
- [ ] Run:

```bash
cargo fmt
cargo check -p conary-core
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::entries::tests -- --list
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::entries::tests
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle -- --list
```

Expected:

- entries test list returns 6 tests.
- direct scriptlet bundle inventory still returns 15 tests.

- [ ] Commit:

```bash
git add crates/conary-core/src/ccs/convert/scriptlet_bundle.rs \
    crates/conary-core/src/ccs/convert/scriptlet_bundle/entries.rs
git commit -m "refactor(core): extract scriptlet bundle entry construction"
```

## Task 5: Extract Evidence Digest Construction

**Files:**

- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`
- Create: `crates/conary-core/src/ccs/convert/scriptlet_bundle/digest.rs`

**`digest.rs` contents to move:**

- `evidence_digest`
- `sorted_native_digest_entries`
- `sorted_flat_digest_entries`
- `sorted_classification_reasons`
- `sorted_classification_evidence`
- `sorted_effect_digest`
- `sorted_entry_decision_digest`
- `native_support_digest`

**`digest.rs` import surface:**

```rust
// conary-core/src/ccs/convert/scriptlet_bundle/digest.rs

use super::types::ScriptletBundleInput;
use crate::ccs::convert::effects::{
    ScriptletClassification, ScriptletClassificationReport, ScriptletEffectEvidence,
};
use crate::ccs::legacy_scriptlets::LegacyScriptletBundle;
use crate::packages::common::PackageMetadata;
use crate::packages::native_abi::NativeScriptletSupport;
use std::collections::BTreeSet;
```

`serde_json::json!`, `serde_json::Value`, `crate::json::canonical_json`, `crate::hash::sha256_prefixed`, and `anyhow::anyhow!` are used through fully-qualified paths in the moved code. Do not add unused imports for those symbols.

**Visibility updates:**

- `evidence_digest` becomes `pub(super) fn evidence_digest`.
- All sorted digest helpers stay private in `digest.rs`.

**Parent temporary import while `build_legacy_scriptlet_bundle` still lives in `scriptlet_bundle.rs`:**

```rust
use digest::evidence_digest;
```

**Move this test to `digest.rs`:**

- `digest_changes_when_classification_evidence_changes`

**`digest.rs` test imports:**

```rust
#[cfg(test)]
mod tests {
    use super::super::test_support::{
        bundle_for_metadata, complete_effect, known_report_with_effect, package_metadata,
    };
    use crate::ccs::convert::effects::{ScriptletClassification, ScriptletClassificationReport};
    use crate::ccs::legacy_scriptlets::EffectReplacement;
    use crate::packages::traits::{Scriptlet, ScriptletPhase};
}
```

- [ ] Create `digest.rs` with a path comment.
- [ ] Move the listed digest helpers and test.
- [ ] Remove now-unused parent imports for `serde_json`, classification digest helpers, and `BTreeSet` if the parent no longer references them.
- [ ] Run:

```bash
cargo fmt
cargo check -p conary-core
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::digest::tests -- --list
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::digest::tests
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle -- --list
```

Expected:

- digest test list returns 1 test.
- direct scriptlet bundle inventory still returns 15 tests.

- [ ] Commit:

```bash
git add crates/conary-core/src/ccs/convert/scriptlet_bundle.rs \
    crates/conary-core/src/ccs/convert/scriptlet_bundle/digest.rs
git commit -m "refactor(core): extract scriptlet bundle evidence digest"
```

## Task 6: Extract Bundle Builder And Reduce Hub

**Files:**

- Modify: `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`
- Create: `crates/conary-core/src/ccs/convert/scriptlet_bundle/builder.rs`
- Modify: `crates/conary-core/src/ccs/convert/converter.rs`

**`builder.rs` contents to move:**

- `build_legacy_scriptlet_bundle`
- `source_format`
- `source_family`
- `version_scheme`
- `valid_prefixed_sha256`

**`builder.rs` import surface:**

```rust
// conary-core/src/ccs/convert/scriptlet_bundle/builder.rs

use super::digest::evidence_digest;
use super::entries::build_entries;
use super::summary::{aggregate_status, decision_counts, summary_from_bundle};
use super::types::{ScriptletBundleBuild, ScriptletBundleInput};
use crate::ccs::legacy_scriptlets::{
    ForeignReplayPolicy, LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle, SourceFormat,
    VersionScheme,
};
use std::collections::BTreeMap;
```

**Visibility updates:**

- `build_legacy_scriptlet_bundle` remains `pub fn build_legacy_scriptlet_bundle(input: ScriptletBundleInput<'_>) -> anyhow::Result<ScriptletBundleBuild>`.
- `source_format`, `source_family`, `version_scheme`, and `valid_prefixed_sha256` stay private in `builder.rs`.

**Move these tests to `builder.rs`:**

- `native_free_input_builds_zero_entry_bundle`
- `tampered_body_after_build_fails_strict_bundle_validation`

**`builder.rs` test imports:**

```rust
#[cfg(test)]
mod tests {
    use super::super::test_support::{bundle_for_metadata, package_metadata};
    use super::super::ScriptletBundleInput;
    use crate::ccs::convert::effects::ScriptletClassificationReport;
    use crate::packages::traits::{Scriptlet, ScriptletPhase};
}
```

The moved `native_free_input_builds_zero_entry_bundle` test calls `build_legacy_scriptlet_bundle` directly through `super::build_legacy_scriptlet_bundle` because the function now lives in `builder.rs`.

**Final hub import and module surface:**

After this task, `scriptlet_bundle.rs` should be:

```rust
// conary-core/src/ccs/convert/scriptlet_bundle.rs
//! Passive legacy scriptlet bundle construction for legacy package conversion.

mod builder;
mod classification;
mod digest;
mod entries;
mod format_metadata;
mod native_contracts;
mod summary;
#[cfg(test)]
mod test_support;
mod types;

pub use builder::build_legacy_scriptlet_bundle;
pub use types::{
    ScriptletBundleBuild, ScriptletBundleInput, ScriptletBundleSummary,
    ScriptletDecisionCountsSummary,
};
```

- [ ] Create `builder.rs` with a path comment.
- [ ] Move the listed builder code and tests.
- [ ] Remove the parent `#[cfg(test)] mod tests` block.
- [ ] Remove all parent private `use` imports; the hub should need none beyond module declarations and public re-exports.
- [ ] In `crates/conary-core/src/ccs/convert/converter.rs`, replace the existing `scriptlet_bundle_types_are_publicly_exported` test with this fuller public API proof:

```rust
#[test]
fn scriptlet_bundle_types_are_publicly_exported() {
    let summary = crate::ccs::convert::ScriptletBundleSummary::default();
    assert_eq!(summary.publication_status, "public");
    let nested_summary = crate::ccs::convert::scriptlet_bundle::ScriptletBundleSummary::default();
    assert_eq!(nested_summary.publication_status, "public");

    let counts = crate::ccs::convert::ScriptletDecisionCountsSummary::default();
    let nested_counts =
        crate::ccs::convert::scriptlet_bundle::ScriptletDecisionCountsSummary::default();
    assert_eq!(counts, nested_counts);

    assert!(
        std::any::type_name::<crate::ccs::convert::ScriptletBundleInput<'static>>()
            .contains("ScriptletBundleInput")
    );
    assert!(
        std::any::type_name::<crate::ccs::convert::scriptlet_bundle::ScriptletBundleInput<'static>>()
            .contains("ScriptletBundleInput")
    );
    assert!(
        std::any::type_name::<crate::ccs::convert::ScriptletBundleBuild>()
            .contains("ScriptletBundleBuild")
    );
    assert!(
        std::any::type_name::<crate::ccs::convert::scriptlet_bundle::ScriptletBundleBuild>()
            .contains("ScriptletBundleBuild")
    );

    let _root_builder: for<'a> fn(
        crate::ccs::convert::ScriptletBundleInput<'a>,
    ) -> anyhow::Result<crate::ccs::convert::ScriptletBundleBuild> =
        crate::ccs::convert::build_legacy_scriptlet_bundle;
    let _module_builder: for<'a> fn(
        crate::ccs::convert::scriptlet_bundle::ScriptletBundleInput<'a>,
    ) -> anyhow::Result<crate::ccs::convert::scriptlet_bundle::ScriptletBundleBuild> =
        crate::ccs::convert::scriptlet_bundle::build_legacy_scriptlet_bundle;
}
```

- [ ] Run boundary checks:

```bash
rg -n "^\s*use " crates/conary-core/src/ccs/convert/scriptlet_bundle.rs
rg -n "^\s*(pub\s+)?(async\s+)?fn " crates/conary-core/src/ccs/convert/scriptlet_bundle.rs
rg -n "^#\[cfg\(test\)\]\s*$|mod tests" crates/conary-core/src/ccs/convert/scriptlet_bundle.rs
```

Expected:

- first command has no output,
- second command has no output,
- third command shows only `#[cfg(test)]` for `test_support` if `rg` matches the attribute, and no `mod tests`.

- [ ] Run:

```bash
cargo fmt
cargo check -p conary-core
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::builder::tests -- --list
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::builder::tests
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::tests -- --list
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle -- --list
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle
cargo test -p conary-core --lib ccs::convert -- --list
```

Expected:

- builder test list returns 2 tests.
- parent `ccs::convert::scriptlet_bundle::tests` inventory returns 0 tests.
- direct `ccs::convert::scriptlet_bundle` inventory returns 15 tests.
- broader `ccs::convert` inventory remains 138 tests.

- [ ] Commit:

```bash
git add crates/conary-core/src/ccs/convert/scriptlet_bundle.rs \
    crates/conary-core/src/ccs/convert/scriptlet_bundle/builder.rs \
    crates/conary-core/src/ccs/convert/converter.rs
git commit -m "refactor(core): extract scriptlet bundle builder"
```

## Task 7: Update Documentation Ownership And Audit Summary

**Files:**

- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

**Docs updates:**

- [ ] In `docs/modules/ccs.md`, add a short note under "Legacy Scriptlet Bundles And Replay" that passive conversion bundle construction is owned by `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs` and its child modules:

```markdown
Passive conversion bundle construction lives under
`crates/conary-core/src/ccs/convert/scriptlet_bundle.rs` and
`crates/conary-core/src/ccs/convert/scriptlet_bundle/`. The hub preserves the
public conversion API while child modules own public DTOs, entry decisions,
native ABI metadata projection, evidence digests, summaries, and fixtures.
```

- [ ] In `docs/modules/feature-ownership.md`, update the CCS ownership "Start here" list to include:

```markdown
`crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`;
`crates/conary-core/src/ccs/convert/scriptlet_bundle/`;
```

- [ ] In `docs/llms/subsystem-map.md`, update the CCS routing bullet to include:

```markdown
`crates/conary-core/src/ccs/convert/scriptlet_bundle.rs`,
`crates/conary-core/src/ccs/convert/scriptlet_bundle/`,
```

**Docs-audit updates:**

- [ ] Update the ledger row for this plan if its note needs to mention that implementation completed. Keep exactly 9 tab-separated fields.
- [ ] Update the existing ledger rows for `docs/modules/ccs.md`, `docs/modules/feature-ownership.md`, and `docs/llms/subsystem-map.md` if their evidence sources, tags, or notes need to mention the new scriptlet-bundle child modules. Do not add new rows for these existing files; this task should keep the inventory count at `164` and corrected count at `64`.
- [ ] Regenerate the inventory:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] Update the summary's latest maintainability planning section to mention Phase 20 completion and keep the counts at `164` tracked files / `64` corrected rows.
- [ ] Run:

```bash
git diff --check
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
awk -F'\t' 'NF != 9 { print NR ":" NF ":" $0 }' docs/superpowers/documentation-accuracy-audit-ledger.tsv
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected:

```text
inventory count: 164
ledger counts:
archived 73
corrected 64
retained-historical 14
verified-no-change 13
```

- [ ] Commit:

```bash
git add docs/modules/ccs.md docs/modules/feature-ownership.md docs/llms/subsystem-map.md \
    docs/superpowers/documentation-accuracy-audit-ledger.tsv \
    docs/superpowers/documentation-accuracy-audit-inventory.tsv \
    docs/superpowers/documentation-accuracy-audit-summary.md
git commit -m "docs: update ccs scriptlet bundle ownership"
```

## Task 8: Final Verification

**Files:**

- All files touched by Tasks 0-7.

**Steps:**

- [ ] Run formatting and compilation:

```bash
cargo fmt --check
cargo check -p conary-core
cargo check -p conary
cargo check -p remi
```

- [ ] Run focused test inventories:

```bash
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::builder::tests -- --list
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::entries::tests -- --list
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::format_metadata::tests -- --list
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::digest::tests -- --list
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::summary::tests -- --list
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle::tests -- --list
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle -- --list
cargo test -p conary-core --lib ccs::convert -- --list
```

Expected counts:

```text
builder: 2
entries: 6
format_metadata: 3
digest: 1
summary: 3
parent tests: 0
scriptlet_bundle total: 15
ccs::convert total: 138
```

- [ ] Run focused tests:

```bash
cargo test -p conary-core --lib ccs::convert::scriptlet_bundle
cargo test -p conary-core --lib ccs::convert::converter::tests::conversion_result_embeds_legacy_scriptlet_bundle
cargo test -p conary-core --lib ccs::convert::converter::tests::converted_ccs_archive_round_trip_preserves_legacy_scriptlet_bundle
cargo test -p conary-core --lib ccs::convert::converter::tests::remi_converter_context_flows_into_bundle_metadata
cargo test -p conary-core --lib ccs::convert::converter::tests::scriptlet_bundle_types_are_publicly_exported
cargo test -p conary --test conversion_integration golden_conversion
cargo test -p conary --test query_scripts query_scripts_ccs_bundle
```

- [ ] Run owning package suites:

```bash
cargo test -p conary-core ccs::convert
cargo test -p conary --test conversion_integration
cargo test -p remi publication
```

- [ ] Run the workspace library test sweep:

```bash
cargo test --workspace --lib
```

- [ ] Run lint gates:

```bash
cargo clippy -p conary-core --all-targets -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] Run docs and drift gates:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/maintainability-drift-report.sh
scripts/line-count-report.sh 30
git diff --check
git status --short --branch
```

- [ ] Confirm `crates/conary-core/src/ccs/convert/scriptlet_bundle.rs` is no longer a hotspot and that the largest new child module is below the old parent size:

```bash
wc -l crates/conary-core/src/ccs/convert/scriptlet_bundle.rs \
    crates/conary-core/src/ccs/convert/scriptlet_bundle/*.rs
```

- [ ] If all verification passes, push and confirm sync:

```bash
git push
git status --short --branch
git rev-parse HEAD origin/main
```

Expected final state:

- `git status --short --branch` shows clean `main` tracking `origin/main` with no ahead or behind marker.
- `git rev-parse HEAD origin/main` prints the same SHA twice.

## Review Checklist For Agentic Review

Ask reviewers to verify:

- `ScriptletBundleSummary::from_bundle` remains public and reachable through both public paths.
- `build_legacy_scriptlet_bundle` remains public and produces byte-for-byte equivalent bundle fields and digest inputs.
- `convert/mod.rs` public re-export surface is unchanged.
- `converter.rs` imports still resolve without path changes.
- `ScriptletBundleInput` lifetime and public fields are unchanged.
- `ScriptletBundleSummary` serialization still skips `review_artifact_path`.
- `EntryOutcome` fields are visible to `entries.rs` after moving classification logic.
- `native_contracts` functions used by both `entries.rs` and `format_metadata.rs` are `pub(super)`.
- `project_format_metadata` is `pub(super)` and all format-specific projection helpers remain private.
- Digest helpers keep fully-qualified `serde_json::json!`, `serde_json::Value`, `crate::json::canonical_json`, and `crate::hash::sha256_prefixed` references or add only necessary imports.
- All 15 tests are assigned exactly once and the parent `scriptlet_bundle::tests` module is gone.
- `test_support.rs` fixture helpers are `pub(super)` and behind `#[cfg(test)]`.
- Docs-audit count math moves from `163/63` to `164/64` after plan lock-in and remains there after implementation.
