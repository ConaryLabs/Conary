# Project Maintainability Phase 14 Remi Conversion Service Decomposition Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking. This is the Phase 14 child packet
> under
> `docs/superpowers/plans/2026-06-06-project-maintainability-agent-readiness-roadmap.md`.

**Goal:** Decompose the whole `apps/remi/src/server/conversion.rs` hotspot into
focused conversion child modules while preserving Remi conversion, publication,
benchmark, recipe-build, cache, CAS, and public API behavior.

**Architecture:** Keep `apps/remi/src/server/conversion.rs` as the stable module
hub and add child files under `apps/remi/src/server/conversion/`. The hub keeps
the public `ConversionService` constructor and re-exports the public conversion
result types; child modules own workflow orchestration, benchmark evidence,
package lookup/download refresh, metadata parsing, critical-package guards,
persistence/cache reconstruction, CAS storage, recipe fetching, and shared test
helpers. Do not rename `conversion.rs` to `conversion/mod.rs`, because Rust
supports child modules under a same-named directory and keeping the file avoids
unnecessary churn in `apps/remi/src/server/mod.rs`.

**Tech Stack:** Rust, Remi server modules, `conary_core` CCS conversion APIs,
SQLite repository/converted-package models, existing publication gates,
Tokio blocking boundaries, Cloudflare R2 write-through, cargo unit/integration
tests, docs-audit scripts.

---

## Status

Draft plan for local and external review.

## Candidate Choice

Phase 13 completed the update-module decomposition. The current hotspot report
puts `apps/remi/src/server/conversion.rs` at 2999 lines, making it the largest
service-owned Rust file and the second-largest Rust file in the workspace. This
phase intentionally targets the whole file rather than a thin first slice so a
single `/goal` can complete the full refactor with internal checkpoints.

Alternatives considered:

| Candidate | Trade-off | Decision |
|-----------|-----------|----------|
| Whole-file Remi conversion decomposition | Larger than prior phases, but all moved items live under one private server module and can be checked task-by-task | Choose for Phase 14 |
| Extract only package lookup and metadata | Lowest-risk Remi first slice, but leaves the public service workflow and test module hotspot mostly intact | Reject as too small for the new goal style |
| Convert `conversion.rs` into `conversion/mod.rs` | Common Rust directory-module style, but adds a rename and more path churn without behavior benefit | Reject; keep file hub plus child modules |
| Move publication policy into conversion children | Would reduce cross-module calls but mixes serving policy with conversion orchestration and risks behavior drift | Reject; keep `publication.rs` as the policy owner |

## Read First

- `AGENTS.md`
- `docs/llms/README.md`
- `docs/llms/subsystem-map.md`
- `docs/ARCHITECTURE.md`
- `docs/INTEGRATION-TESTING.md`
- `docs/modules/feature-ownership.md`
- `docs/modules/remi.md`
- `docs/modules/test-fixtures.md`
- `docs/conaryopedia-v2.md` Remi section 6
- `docs/superpowers/plans/2026-06-06-project-maintainability-agent-readiness-roadmap.md`
- `docs/superpowers/plans/2026-06-06-project-maintainability-phase13-update-module-completion-decomposition-plan.md`
- `apps/remi/src/server/mod.rs`
- `apps/remi/src/server/conversion.rs`
- `apps/remi/src/server/conversion_timing.rs`
- `apps/remi/src/server/publication.rs`
- `apps/remi/src/server/scriptlet_corpus.rs`
- `apps/remi/src/server/prewarm.rs`
- `apps/remi/src/server/handlers/packages.rs`
- `apps/remi/src/server/handlers/index.rs`
- `apps/remi/src/server/handlers/detail.rs`
- `apps/remi/src/server/handlers/sparse.rs`
- `apps/remi/src/server/jobs.rs`
- `apps/remi/src/bin/remi.rs`
- `apps/conary/tests/conversion_integration.rs`

## Current Repo-Grounded Inputs

| Signal | Current value | Phase 14 interpretation |
|--------|---------------|-------------------------|
| Hotspot rank | `apps/remi/src/server/conversion.rs` is 2999 lines, behind only `apps/conary/src/commands/ccs/install.rs` at 3118 lines | Remi conversion is the next service hotspot |
| Production/test split | `#[cfg(test)] mod tests` starts at line 1658, so roughly 1657 production lines and 1342 test lines live together | Move tests with their owning modules and extract shared test support |
| External public surface | `apps/remi/src/server/mod.rs` re-exports `ConversionBenchmarkEvidence`, `ConversionService`, and `ServerConversionResult` | Preserve these re-exports unchanged |
| Internal server callers | `publication.rs`, `index_gen.rs`, `jobs.rs`, `handlers/index.rs`, `handlers/packages.rs`, `prewarm.rs`, `server/mod.rs`, and `bin/remi.rs` depend on conversion types/service | Keep `crate::server::conversion::{ScriptletPackageMetadata, ServerConversionResult}` and `crate::server::ConversionService` routes working |
| Focused conversion tests | `cargo test -p remi --lib server::conversion::tests -- --list` finds 57 tests | Partition these tests into child-module test modules and one small hub constructor test |
| Broader conversion filter | `cargo test -p remi --lib conversion -- --list` finds 64 library tests | Use `cargo test -p remi --lib conversion` as the broad Remi conversion behavior gate after each major stage |
| Publication interaction gate | `cargo test -p remi publication -- --list` finds 6 tests | Use this whenever persistence/publication metadata moves |
| CLI benchmark gate | `cargo test -p remi --test cli_help conversion -- --list` finds 1 test | Preserve benchmark command output behavior |
| Conary integration gate | `cargo test -p conary --test conversion_integration golden_conversion -- --list` finds 4 tests | Run after final Remi conversion refactor because public conversion output affects CLI proof |
| Docs-audit baseline | 157 tracked doc-like files, 57 corrected rows | Lock-in should add one planning file and update counts to 158 total / 58 corrected |

Evidence commands used to shape this packet:

```bash
git status --short --branch
git rev-parse HEAD origin/main
wc -l apps/remi/src/server/conversion.rs
scripts/line-count-report.sh 30
find apps/remi/src/server -maxdepth 2 -type f | sort
rg -n "^(pub |pub\\(|struct |enum |impl |fn |async fn|#\\[cfg\\(test\\)]|\\s{4}(pub |async fn|fn ))" apps/remi/src/server/conversion.rs
rg -n "ConversionService|ServerConversionResult|ScriptletPackageMetadata|ConversionBenchmarkEvidence|conversion::" apps/remi/src crates/conary-core/src apps/conary/src apps/conary/tests docs -g '*.rs' -g '*.md'
cargo test -p remi --lib server::conversion::tests -- --list
cargo test -p remi --lib conversion -- --list
cargo test -p remi publication -- --list
cargo test -p remi --test cli_help conversion -- --list
cargo test -p conary --test conversion_integration golden_conversion -- --list
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
```

## Target Module Boundary

Create:

- `apps/remi/src/server/conversion/types.rs`
- `apps/remi/src/server/conversion/test_support.rs`
- `apps/remi/src/server/conversion/metadata.rs`
- `apps/remi/src/server/conversion/safety.rs`
- `apps/remi/src/server/conversion/lookup.rs`
- `apps/remi/src/server/conversion/storage.rs`
- `apps/remi/src/server/conversion/persistence.rs`
- `apps/remi/src/server/conversion/recipe.rs`
- `apps/remi/src/server/conversion/benchmark.rs`
- `apps/remi/src/server/conversion/workflow.rs`

Modify:

- `apps/remi/src/server/conversion.rs`
- `apps/remi/src/server/mod.rs` only if formatting/import cleanup becomes
  necessary; the expected public re-export line is unchanged.
- `docs/llms/subsystem-map.md`
- `docs/modules/feature-ownership.md`
- `docs/modules/remi.md`
- `docs/modules/test-fixtures.md`
- `docs/conaryopedia-v2.md`
- `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- `docs/superpowers/documentation-accuracy-audit-summary.md`

Keep `conversion.rs` as a hub with this final responsibility:

```rust
// apps/remi/src/server/conversion.rs
//! Package conversion service for the Remi server.

mod benchmark;
mod lookup;
mod metadata;
mod persistence;
mod recipe;
mod safety;
mod storage;
#[cfg(test)]
mod test_support;
mod types;
mod workflow;

use crate::server::R2Store;
use std::path::PathBuf;
use std::sync::Arc;

pub use types::{ConversionBenchmarkEvidence, ScriptletPackageMetadata, ServerConversionResult};

/// Conversion service for Remi.
#[derive(Clone)]
pub struct ConversionService {
    /// Path to chunk storage.
    chunk_dir: PathBuf,
    /// Path to cache/scratch directory.
    cache_dir: PathBuf,
    /// Database path.
    db_path: PathBuf,
    /// Optional R2 store for write-through.
    r2_store: Option<Arc<R2Store>>,
}

impl ConversionService {
    pub fn new(
        chunk_dir: PathBuf,
        cache_dir: PathBuf,
        db_path: PathBuf,
        r2_store: Option<Arc<R2Store>>,
    ) -> Self {
        Self {
            chunk_dir,
            cache_dir,
            db_path,
            r2_store,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversion_service_new() {
        let service = ConversionService::new(
            PathBuf::from("/chunks"),
            PathBuf::from("/cache"),
            PathBuf::from("/db.sqlite"),
            None,
        );
        assert_eq!(service.chunk_dir, PathBuf::from("/chunks"));
        assert_eq!(service.cache_dir, PathBuf::from("/cache"));
        assert_eq!(service.db_path, PathBuf::from("/db.sqlite"));
        assert!(service.r2_store.is_none());
    }
}
```

Rust privacy note: keep `ConversionService` fields private in the parent
`conversion` module. Child modules under `conversion/` are descendants of that
module and can access the private fields from their inherent `impl
ConversionService` blocks. Do not widen the fields to `pub(crate)` or
`pub(super)`.

## Final File Responsibilities

| File | Owns | Does not own |
|------|------|--------------|
| `conversion.rs` | public `ConversionService` constructor, child module declarations, public conversion type re-exports | conversion workflow, parsing, lookup, persistence, recipe network validation |
| `conversion/types.rs` | public result/metadata/evidence DTOs and `ScriptletPackageMetadata` conversion | service methods or persistence |
| `conversion/test_support.rs` | shared test database/package/result builders and production-source scanning helper | production code |
| `conversion/metadata.rs` | safe CCS filenames, repository identity application, package parsing, package metadata building, repository-provide merge | database lookup, critical system package policy, CAS writes |
| `conversion/safety.rs` | critical package-name and runtime-capability refusal guards | source download or conversion |
| `conversion/lookup.rs` | repository package selection, distro-to-repo mapping, one-shot refresh after upstream 404, upstream-not-found detection | parsing, persistence, CAS writes |
| `conversion/storage.rs` | SHA-256 checksum and CAS/R2 blob storage | database persistence or publication decisions |
| `conversion/persistence.rs` | converted-package row insertion, cache-hit reconstruction, publication outcome wrapping, review artifact path persistence | package lookup, package parsing, CAS writes |
| `conversion/recipe.rs` | recipe URL fetch, SSRF host/IP validation, recipe build path | legacy package conversion workflow |
| `conversion/benchmark.rs` | benchmark package sampling, scan-only scriptlet corpus evidence, conversion benchmark wrapper | main conversion orchestration internals |
| `conversion/workflow.rs` | `convert_package_async`, cold/hot conversion workflow, timing, blocking-boundary orchestration, `ParsedConversion` | standalone benchmark sampling or recipe builds |

## Visibility Contract

Use `pub(super)` only for cross-child-module helpers:

- `metadata.rs`
  - `safe_ccs_filename`
  - `safe_ccs_filename_with_arch`
  - `apply_repository_identity`
  - `parse_package`
  - `merge_repository_provides`
- `safety.rs`
  - `ensure_package_name_not_critical`
  - `ensure_metadata_not_critical`
  - `ensure_repository_package_not_critical`
- `lookup.rs`
  - `PackageDownloadRefresh`
  - `find_package_for_conversion_async`
  - `find_package`
  - `download_package_with_refresh_async`
- `storage.rs`
  - `store_chunks_with_timing`
  - `calculate_checksum`
- `persistence.rs`
  - `PersistConversionInput`
  - `cached_conversion_result_async`
  - `persist_conversion_result`

Keep these private to their owner modules:

- `metadata.rs`: `build_metadata`, `should_skip_repository_provide`,
  `repository_provide_constraint`, `constraint_from_raw_provide`
- `safety.rs`: `metadata_provides_critical_runtime`,
  `repository_package_provides_critical_runtime`
- `lookup.rs`: `is_upstream_not_found`
- `storage.rs`: `store_chunks` stays `#[cfg(test)]`
- `persistence.rs`: `build_result_from_existing`,
  `outcome_from_converted_result`
- `recipe.rs`: `fetch_url`, `validate_host`, `validate_ip`
- `workflow.rs`: `record_cache_hit_skips`, `log_conversion_timing`,
  `ParsedConversion`, `parse_and_convert_package`

Rust privacy note: private items in a module are visible to that module and its
descendant modules. Child modules under `conversion/` can therefore access
private `ConversionService` fields defined in the parent hub, including
`r2_store`, without naming or importing `R2Store` directly. Tests nested under a
child module can also call private helpers owned by that child module, such as
`validate_host` or `build_metadata`. Sibling child modules still cannot call
each other's private helpers; keep those cross-child calls on the explicit
`pub(super)` list above.

## Non-Goals

- Do not change Remi public conversion behavior, publication gate behavior,
  job-state behavior, benchmark output, recipe SSRF validation, R2 write-through
  behavior, cache-hit/stale-cache behavior, critical-package refusal behavior,
  package parsing, repository-provide merging, or CAS layout.
- Do not change database schema, migrations, `ConvertedPackage` semantics,
  `RepositoryPackage` selection semantics, `PublicationDecision` semantics, or
  scriptlet publication rules.
- Do not change public server re-exports from `apps/remi/src/server/mod.rs`.
- Do not change CLI argument parsing in `apps/remi/src/bin/remi.rs`.
- Do not move or refactor `publication.rs`, `conversion_timing.rs`,
  `scriptlet_corpus.rs`, handlers, prewarm, jobs, or R2 in this phase.
- Do not convert `apps/remi/src/server/conversion.rs` into
  `apps/remi/src/server/conversion/mod.rs`.
- Do not introduce behavior-oriented refactors while moving code. Adjust only
  paths, imports, visibility, and test module locations.

## Risks And Checks

| Risk | Mitigation |
|------|------------|
| Public API route breakage | Keep `apps/remi/src/server/mod.rs` re-export unchanged and keep `conversion.rs` re-exporting the public types |
| Child-module privacy confusion | Keep service fields private in `conversion.rs`; use `pub(super)` only for helpers called by sibling child modules |
| Circular import between conversion and publication | Keep publication policy in `publication.rs`; `types.rs` may use the existing fully qualified `crate::server::publication::PublicationGateReport` field path |
| Tests stranded in old parent module | Move tests with owner code and create `test_support.rs` before moving the first test cluster |
| `Handle::block_on` guard missing new paths | Update the production-source scan test to include every new production child module |
| Pre-existing `Handle::block_on` guard coverage gap | Rewrite `production_source_without_comments` in Task 1 so it distinguishes `#[cfg(test)] mod tests` from earlier `#[cfg(test)]` items and keeps scanning production code after the test-only `store_chunks` helper |
| Benchmark scan-only path losing parser/lookup access | Move benchmark after metadata and lookup are stable and import `PackageDownloadRefresh` explicitly |
| Persistence path losing safe filename/checksum helpers | Mark metadata filename helpers and storage checksum helper `pub(super)` before moving persistence |
| Docs path drift | Update Remi module docs, feature ownership, fixture map, subsystem map, conaryopedia, and docs-audit ledger rows |
| Plan too large for one session | Use task-level commits and run focused Remi tests after each module extraction |

---

## Task 0: Register The Phase 14 Plan In Docs Audit

**Files:**
- Create:
  `docs/superpowers/plans/2026-06-06-project-maintainability-phase14-remi-conversion-service-decomposition-plan.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Confirm clean synced baseline**

Run:

```bash
git status --short --branch
git rev-parse HEAD origin/main
git rev-list --left-right --count HEAD...origin/main
```

Expected:

- branch is `main...origin/main`;
- no uncommitted changes other than this draft if lock-in edits are already in
  progress;
- `HEAD` and `origin/main` match;
- left/right count is `0	0`.

- [ ] **Step 2: Stage the new plan before regenerating docs inventory**

Run:

```bash
git add docs/superpowers/plans/2026-06-06-project-maintainability-phase14-remi-conversion-service-decomposition-plan.md
LC_ALL=C bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
```

Expected: tracked doc-like files grow from 157 to 158 because the inventory
script reads the staged index.

- [ ] **Step 3: Add the Phase 14 ledger row**

In `docs/superpowers/documentation-accuracy-audit-ledger.tsv`, locate the Phase
13 row by searching for:

```text
phase13-update-module-completion-decomposition-plan.md
```

Insert this new row immediately after it. The row uses literal tab characters:

```tsv
docs/superpowers/plans/2026-06-06-project-maintainability-phase14-remi-conversion-service-decomposition-plan.md	docs/superpowers/plans/2026-06-06-project-maintainability-phase14-remi-conversion-service-decomposition-plan.md	planning	maintainer	maintainability; phase14; remi; conversion-service; hotspot-decomposition	apps/remi/src/server/conversion.rs; apps/remi/src/server/conversion/types.rs; apps/remi/src/server/conversion/test_support.rs; apps/remi/src/server/conversion/metadata.rs; apps/remi/src/server/conversion/safety.rs; apps/remi/src/server/conversion/lookup.rs; apps/remi/src/server/conversion/storage.rs; apps/remi/src/server/conversion/persistence.rs; apps/remi/src/server/conversion/recipe.rs; apps/remi/src/server/conversion/benchmark.rs; apps/remi/src/server/conversion/workflow.rs; apps/remi/src/server/publication.rs; apps/remi/src/server/conversion_timing.rs; apps/remi/src/server/scriptlet_corpus.rs; apps/remi/src/server/prewarm.rs; apps/remi/src/server/handlers/packages.rs; apps/remi/src/server/handlers/index.rs; apps/remi/src/server/jobs.rs; apps/remi/src/bin/remi.rs; apps/conary/tests/conversion_integration.rs; docs/modules/remi.md; docs/modules/test-fixtures.md; docs/modules/feature-ownership.md; docs/llms/subsystem-map.md; docs/conaryopedia-v2.md	verified	corrected	Added Phase 14 plan for decomposing the full Remi conversion service hotspot into focused child modules while preserving public conversion APIs, publication gating, benchmark evidence, recipe SSRF safety, CAS/R2 storage, and conversion integration behavior.
```

- [ ] **Step 4: Update the audit summary and counts**

In `docs/superpowers/documentation-accuracy-audit-summary.md`, append this
paragraph to the existing `### 2026-06-06 Maintainability Planning` section:

```markdown
The Phase 14 Remi conversion service decomposition plan starts the next
whole-file maintainability pass by targeting
`apps/remi/src/server/conversion.rs`. It keeps `conversion.rs` as the public
module hub while planning child modules for conversion result types, shared
test support, package metadata parsing, critical-package safety guards,
repository lookup/download refresh, CAS/R2 storage, conversion persistence,
recipe URL/SSRF handling, benchmark evidence, and cold/hot conversion workflow
orchestration.
```

Then update the final counts from:

```markdown
- Total tracked doc-like files audited: 157
- `verified-no-change`: 13
- `corrected`: 57
- `archived`: 73
- `retained-historical`: 14
```

to:

```markdown
- Total tracked doc-like files audited: 158
- `verified-no-change`: 13
- `corrected`: 58
- `archived`: 73
- `retained-historical`: 14
```

Refresh the existing ledger row for
`docs/superpowers/documentation-accuracy-audit-summary.md` so:

- `tags` includes `phase14` and `remi-conversion-service`;
- `evidence_sources` includes
  `docs/superpowers/plans/2026-06-06-project-maintainability-phase14-remi-conversion-service-decomposition-plan.md`;
- `notes` mentions `Phase 14 Remi conversion service decomposition`.

- [ ] **Step 5: Verify docs-audit lock-in**

Stage the plan and refreshed audit files before checking the cached diff:

```bash
git add docs/superpowers/plans/2026-06-06-project-maintainability-phase14-remi-conversion-service-decomposition-plan.md \
  docs/superpowers/documentation-accuracy-audit-ledger.tsv \
  docs/superpowers/documentation-accuracy-audit-inventory.tsv \
  docs/superpowers/documentation-accuracy-audit-summary.md
```

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
awk -F'\t' 'NF != 9 { print NR ":" NF ":" $0 }' docs/superpowers/documentation-accuracy-audit-ledger.tsv
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --cached --check
```

Expected:

- inventory count is `158`;
- ledger distribution includes `corrected 58`;
- malformed-row check prints nothing;
- ledger checker passes;
- diff check passes.

- [ ] **Step 6: Commit plan lock-in**

Run:

```bash
git status --short
git commit -m "docs: plan remi conversion decomposition"
```

Expected: docs-only commit. Do not implement Rust code in this task.

---

## Task 1: Establish Conversion Hub, Public Types, And Test Support

**Files:**
- Modify: `apps/remi/src/server/conversion.rs`
- Create: `apps/remi/src/server/conversion/types.rs`
- Create: `apps/remi/src/server/conversion/test_support.rs`

- [ ] **Step 1: Create the child module directory**

Run:

```bash
mkdir -p apps/remi/src/server/conversion
```

Expected: the directory exists and contains no files until the next steps.

- [ ] **Step 2: Move public DTOs into `types.rs`**

Create `apps/remi/src/server/conversion/types.rs`:

```rust
// apps/remi/src/server/conversion/types.rs
//! Public DTOs emitted by Remi conversion workflows.

use crate::server::conversion_timing::ConversionTimingReport;
use conary_core::ccs::convert::{ScriptletBundleSummary, ScriptletDecisionCountsSummary};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Result of a server-side conversion.
#[derive(Debug)]
pub struct ServerConversionResult {
    pub name: String,
    pub version: String,
    pub distro: String,
    pub chunk_hashes: Vec<String>,
    pub total_size: u64,
    pub content_hash: String,
    pub ccs_path: PathBuf,
    pub cache_state: String,
    pub scriptlets: ScriptletPackageMetadata,
    pub publication: Option<crate::server::publication::PublicationGateReport>,
    pub timing: Option<ConversionTimingReport>,
}

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

#[derive(Debug, Serialize)]
pub struct ConversionBenchmarkEvidence {
    pub distro: String,
    pub package: String,
    pub version: Option<String>,
    pub scan_only: bool,
    pub cache_state: String,
    pub r2_configured: bool,
    pub timing: Option<ConversionTimingReport>,
    pub scriptlet_summary: Option<crate::server::scriptlet_corpus::ScriptletCorpusSummary>,
    pub converted: bool,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::jobs::JobStatus;
    use crate::server::publication::ServerConversionOutcome;
    use conary_core::ccs::convert::ScriptletBundleSummary;
    use std::time::Duration;

    #[test]
    fn server_conversion_result_can_carry_timing_report() {
        use crate::server::conversion_timing::{ConversionPhase, ConversionTimingReport};

        let mut timing = ConversionTimingReport::new("fedora", "nginx", None);
        timing.record(ConversionPhase::PackageLookup, Duration::from_millis(7));
        timing.finish(true);

        let result = ServerConversionResult {
            name: "nginx".to_string(),
            version: "1.28.0".to_string(),
            distro: "fedora".to_string(),
            chunk_hashes: vec![],
            total_size: 0,
            content_hash: "sha256:test".to_string(),
            ccs_path: PathBuf::from("/tmp/nginx.ccs"),
            cache_state: "cold".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&ScriptletBundleSummary::default()),
            publication: None,
            timing: Some(timing),
        };

        assert_eq!(result.timing.unwrap().phases[0].duration_ms, 7);
    }

    #[test]
    fn server_conversion_outcome_reports_terminal_state() {
        let result = ServerConversionResult {
            name: "pkg".to_string(),
            version: "1.0".to_string(),
            distro: "fedora".to_string(),
            chunk_hashes: Vec::new(),
            total_size: 0,
            content_hash: "sha256:test".to_string(),
            ccs_path: PathBuf::from("/tmp/pkg.ccs"),
            cache_state: "cold".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&ScriptletBundleSummary::default()),
            publication: None,
            timing: None,
        };

        assert!(matches!(
            ServerConversionOutcome::Ready(result).job_status(),
            JobStatus::Ready
        ));
    }

    #[test]
    fn test_server_conversion_result_debug() {
        let result = ServerConversionResult {
            name: "nginx".to_string(),
            version: "1.24.0".to_string(),
            distro: "fedora".to_string(),
            chunk_hashes: vec!["abc123".to_string()],
            total_size: 1024,
            content_hash: "sha256:deadbeef".to_string(),
            ccs_path: PathBuf::from("/data/nginx.ccs"),
            cache_state: "cold".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&ScriptletBundleSummary::default()),
            publication: None,
            timing: None,
        };
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("nginx"));
    }
}
```

- [ ] **Step 3: Add hub declarations and re-exports**

At the top of `conversion.rs`, replace the public DTO definitions with module
declarations and re-exports. Keep the `ConversionService` struct and `new`
constructor in `conversion.rs`.

For Task 1, add only the modules that exist at the end of this task and keep
the existing production imports that are still needed by unmoved methods. The
top of `conversion.rs` should include these new declarations near the module
header:

```rust
#[cfg(test)]
mod test_support;
mod types;
```

Then add the public type re-export after the existing `use` block:

```rust
pub use types::{ConversionBenchmarkEvidence, ScriptletPackageMetadata, ServerConversionResult};
```

Remove the original `ServerConversionResult`, `ScriptletPackageMetadata`,
`impl From<&ScriptletBundleSummary> for ScriptletPackageMetadata`, and
`ConversionBenchmarkEvidence` definitions from `conversion.rs`.

Do not collapse the parent imports to the final hub import list in this task.
Most conversion methods still live in `conversion.rs` until Tasks 2-9, so the
existing imports for `ConversionOptions`, `ConversionResult`, `LegacyConverter`,
`ScriptletBundleSummary`, database models, package parsers, `anyhow`, `TempDir`,
`Duration`, `Instant`, and tracing must remain until their owning module moves.
Each later task adds its own `mod ...;` declaration and removes only imports
made stale by that task. Task 9 verifies the final hub contains the complete
module list and only the final `R2Store`, `PathBuf`, and `Arc` imports.

- [ ] **Step 4: Create shared test support**

Create `apps/remi/src/server/conversion/test_support.rs` by moving these helpers
out of the parent test module:

- `create_test_db`
- `insert_repo`
- `insert_package`
- `production_source_without_comments`
- `make_conversion_result`
- `goal8a_scriptlet_summary`

Use this import surface:

```rust
// apps/remi/src/server/conversion/test_support.rs
//! Shared tests helpers for Remi conversion child modules.

use conary_core::ccs::convert::{ConversionResult, ScriptletBundleSummary};
use conary_core::db::models::{Repository, RepositoryPackage};
use conary_core::db::schema;
use std::fs;
use std::path::Path;
use tempfile::NamedTempFile;
```

Make each moved helper `pub(super)` so `conversion::*::tests` modules can import
them through `super::super::test_support::*`.

Rewrite `production_source_without_comments` while moving it. The current
helper stops at the first `#[cfg(test)]`, which becomes wrong once
`conversion.rs` has an early `#[cfg(test)] mod test_support;` declaration. The
helper must stop only at the real `#[cfg(test)] mod tests` boundary:

```rust
pub(super) fn production_source_without_comments(relative_path: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path);
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
    let mut stripped = String::new();
    let mut in_block_comment = false;
    let mut pending_test_cfg = false;

    for line in source.lines() {
        let trimmed = line.trim_start();
        if pending_test_cfg {
            if trimmed.starts_with("#[") {
                continue;
            }
            if trimmed.starts_with("mod tests") {
                break;
            }
            pending_test_cfg = false;
        }
        if trimmed.starts_with("#[cfg(test)]") {
            pending_test_cfg = true;
        }

        let mut chars = line.chars().peekable();
        while let Some(ch) = chars.next() {
            if in_block_comment {
                if ch == '*' && chars.peek() == Some(&'/') {
                    let _ = chars.next();
                    in_block_comment = false;
                }
                continue;
            }

            if ch == '/' && chars.peek() == Some(&'/') {
                break;
            }

            if ch == '/' && chars.peek() == Some(&'*') {
                let _ = chars.next();
                in_block_comment = true;
                continue;
            }

            stripped.push(ch);
        }

        stripped.push('\n');
    }

    stripped
}
```

- [ ] **Step 5: Keep the parent tests compiling during the transition**

If any tests remain in the parent `conversion.rs` test module after moving
shared helpers, add:

```rust
use super::test_support::*;
```

Remove helper-specific imports from the parent test module once they are no
longer needed directly:

```rust
use conary_core::db::schema;
use std::fs;
use std::path::Path;
use tempfile::NamedTempFile;
```

- [ ] **Step 6: Verify Task 1**

Run:

```bash
cargo fmt
cargo check -p remi
cargo clippy -p remi --lib -- -D warnings
cargo test -p remi --lib server::conversion::types::tests
cargo test -p remi --lib server::conversion::tests::test_conversion_service_new
cargo test -p remi --lib server::conversion::tests::remi_server_conversion_paths_do_not_block_on_async_work
cargo test -p remi scriptlet
cargo test -p remi --lib server::conversion::tests -- --list
cargo test -p remi --lib conversion
git diff --check
```

Expected:

- `cargo check` passes;
- the three type tests pass under `server::conversion::types::tests`;
- the constructor test still passes;
- the async-work guard still scans production Remi conversion paths after the
  new `test_support` module declaration;
- scriptlet metadata/publication-related filters still pass after moving public
  conversion DTOs;
- the remaining parent conversion test list is smaller by the three moved type
  tests;
- broad conversion test discovery still finds the adjacent conversion filters.

- [ ] **Step 7: Commit Task 1**

Run:

```bash
git add apps/remi/src/server/conversion.rs apps/remi/src/server/conversion/types.rs apps/remi/src/server/conversion/test_support.rs
git commit -m "refactor(remi): extract conversion result types"
```

---

## Task 2: Extract Package Metadata And Filename Helpers

**Files:**
- Create: `apps/remi/src/server/conversion/metadata.rs`
- Modify: `apps/remi/src/server/conversion.rs`

- [ ] **Step 1: Add the metadata module**

In `conversion.rs`, enable:

```rust
mod metadata;
```

- [ ] **Step 2: Create `metadata.rs` imports**

Create `apps/remi/src/server/conversion/metadata.rs`:

```rust
// apps/remi/src/server/conversion/metadata.rs
//! Package metadata extraction, safe CCS filenames, and native provide merging.

use super::ConversionService;
use anyhow::{Result, anyhow};
use conary_core::db::models::{RepositoryPackage, RepositoryProvide};
use conary_core::filesystem::path::sanitize_filename;
use conary_core::packages::arch::ArchPackage;
use conary_core::packages::common::PackageMetadata;
use conary_core::packages::deb::DebPackage;
use conary_core::packages::rpm::RpmPackage;
use conary_core::packages::traits::{Dependency, DependencyType, ExtractedFile, PackageFormat};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
```

- [ ] **Step 3: Move metadata-owned methods**

Move these `impl ConversionService` methods into `metadata.rs`:

- `safe_ccs_filename`
- `safe_ccs_filename_with_arch`
- `apply_repository_identity`
- `parse_package`
- `merge_repository_provides`
- `build_metadata`

Set visibility exactly:

```rust
impl ConversionService {
    pub(super) fn safe_ccs_filename(name: &str, version: &str) -> Result<String> { ... }

    pub(super) fn safe_ccs_filename_with_arch(
        name: &str,
        version: &str,
        architecture: Option<&str>,
    ) -> Result<String> { ... }

    pub(super) fn apply_repository_identity(
        metadata: &mut PackageMetadata,
        repo_pkg: &RepositoryPackage,
    ) { ... }

    pub(super) fn parse_package(
        &self,
        path: &Path,
        distro: &str,
    ) -> Result<(PackageMetadata, Vec<ExtractedFile>, &'static str)> { ... }

    pub(super) fn merge_repository_provides(
        conn: &rusqlite::Connection,
        repo_pkg: &RepositoryPackage,
        metadata: &mut PackageMetadata,
    ) -> Result<()> { ... }

    fn build_metadata<P: PackageFormat>(pkg: &P) -> PackageMetadata { ... }
}
```

- [ ] **Step 4: Move repository-provide helper functions**

Move the free helper functions into `metadata.rs` below the `impl` block:

- `should_skip_repository_provide`
- `repository_provide_constraint`
- `constraint_from_raw_provide`

Keep them private.

- [ ] **Step 5: Move metadata-owned tests**

Move these tests from the parent test module to `metadata.rs`:

- `test_safe_ccs_filename_normal`
- `test_safe_ccs_filename_complex_name`
- `test_safe_ccs_filename_with_architecture`
- `test_apply_repository_identity_preserves_epoch_and_architecture`
- `test_safe_ccs_filename_rejects_path_traversal_in_name`
- `test_safe_ccs_filename_rejects_path_traversal_in_version`
- `test_safe_ccs_filename_rejects_slash_in_name`
- `test_safe_ccs_filename_rejects_empty_name`
- `test_safe_ccs_filename_rejects_empty_version`
- `repository_native_provides_are_merged_into_conversion_metadata`

Use this test module import surface:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::test_support::{create_test_db, insert_repo};
    use conary_core::packages::common::PackageMetadata;
    use std::path::PathBuf;
}
```

The `repository_native_provides_are_merged_into_conversion_metadata` test also
needs `RepositoryPackage` and `RepositoryProvide`; import them from
`conary_core::db::models`.

- [ ] **Step 6: Clean parent imports conservatively**

After moving metadata-owned code, remove imports that are now used only by
`metadata.rs`:

```rust
use conary_core::filesystem::path::sanitize_filename;
use conary_core::packages::arch::ArchPackage;
use conary_core::packages::deb::DebPackage;
use conary_core::packages::rpm::RpmPackage;
use std::collections::HashSet;
```

Also split the parent trait import so `PackageFormat` leaves with the metadata
module while `Dependency` and `DependencyType` stay for the safety tests that
move in Task 3:

```diff
-use conary_core::packages::traits::{Dependency, DependencyType, PackageFormat};
+use conary_core::packages::traits::{Dependency, DependencyType};
```

Keep these imports in `conversion.rs` until later tasks move their remaining
parent users:

```rust
use conary_core::db::models::{ConvertedPackage, RepositoryPackage, RepositoryProvide};
use conary_core::packages::common::PackageMetadata;
use conary_core::packages::traits::{Dependency, DependencyType};
```

- [ ] **Step 7: Verify Task 2**

Run:

```bash
cargo fmt
cargo check -p remi
cargo test -p remi --lib server::conversion::metadata::tests
cargo test -p remi --lib server::conversion::tests -- --list
cargo test -p remi --lib conversion
git diff --check
```

Expected:

- metadata tests pass under `server::conversion::metadata::tests`;
- the parent conversion test list is smaller by the moved metadata tests;
- broad conversion discovery remains healthy.

- [ ] **Step 8: Commit Task 2**

Run:

```bash
git add apps/remi/src/server/conversion.rs apps/remi/src/server/conversion/metadata.rs
git commit -m "refactor(remi): extract conversion metadata helpers"
```

---

## Task 3: Extract Critical Package Safety Guards

**Files:**
- Create: `apps/remi/src/server/conversion/safety.rs`
- Modify: `apps/remi/src/server/conversion.rs`

- [ ] **Step 1: Add the safety module**

In `conversion.rs`, enable:

```rust
mod safety;
```

- [ ] **Step 2: Create `safety.rs` imports**

Create `apps/remi/src/server/conversion/safety.rs`:

```rust
// apps/remi/src/server/conversion/safety.rs
//! Critical package and runtime capability refusal guards.

use super::ConversionService;
use anyhow::Result;
use conary_core::db::models::{RepositoryPackage, RepositoryProvide};
use conary_core::packages::common::PackageMetadata;
```

- [ ] **Step 3: Move critical guard methods**

Move these methods into `safety.rs`:

- `ensure_package_name_not_critical`
- `metadata_provides_critical_runtime`
- `ensure_metadata_not_critical`
- `repository_package_provides_critical_runtime`
- `ensure_repository_package_not_critical`

Set visibility exactly:

```rust
impl ConversionService {
    pub(super) fn ensure_package_name_not_critical(package_name: &str) -> Result<()> { ... }

    fn metadata_provides_critical_runtime(metadata: &PackageMetadata) -> Option<&str> { ... }

    pub(super) fn ensure_metadata_not_critical(metadata: &PackageMetadata) -> Result<()> { ... }

    fn repository_package_provides_critical_runtime(
        conn: &rusqlite::Connection,
        repo_pkg: &RepositoryPackage,
    ) -> Result<Option<String>> { ... }

    pub(super) fn ensure_repository_package_not_critical(
        conn: &rusqlite::Connection,
        repo_pkg: &RepositoryPackage,
    ) -> Result<()> { ... }
}
```

- [ ] **Step 4: Move safety tests**

Move these tests into `safety.rs`:

- `test_critical_packages_blocked`
- `shared_critical_package_names_are_refused_by_conversion_guard`
- `metadata_provides_critical_runtime_capabilities_are_detected`
- `repository_provides_guard_blocks_cached_conversion_path`
- `test_normal_packages_not_blocked`

Use this test module import surface:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::test_support::{create_test_db, insert_package, insert_repo};
    use conary_core::db::models::RepositoryProvide;
    use conary_core::packages::common::PackageMetadata;
    use conary_core::packages::traits::{Dependency, DependencyType};
    use std::path::PathBuf;
}
```

- [ ] **Step 5: Verify Task 3**

Run:

```bash
cargo fmt
cargo check -p remi
cargo test -p remi --lib server::conversion::safety::tests
cargo test -p remi --lib conversion
git diff --check
```

Expected: safety tests pass in the new module and broad conversion discovery
still lists all adjacent conversion-related tests.

- [ ] **Step 6: Commit Task 3**

Run:

```bash
git add apps/remi/src/server/conversion.rs apps/remi/src/server/conversion/safety.rs
git commit -m "refactor(remi): extract conversion safety guards"
```

---

## Task 4: Extract Package Lookup And Download Refresh

**Files:**
- Create: `apps/remi/src/server/conversion/lookup.rs`
- Modify: `apps/remi/src/server/conversion.rs`

- [ ] **Step 1: Add the lookup module**

In `conversion.rs`, enable:

```rust
mod lookup;
```

- [ ] **Step 2: Create `lookup.rs` imports**

Create `apps/remi/src/server/conversion/lookup.rs`:

```rust
// apps/remi/src/server/conversion/lookup.rs
//! Repository package lookup and one-shot upstream refresh for conversion.

use super::ConversionService;
use anyhow::{Result, anyhow};
use conary_core::db::models::RepositoryPackage;
use conary_core::repository::download_package;
use std::path::{Path, PathBuf};
use tracing::info;
```

- [ ] **Step 3: Move lookup request type and methods**

Move `PackageDownloadRefresh<'a>` into `lookup.rs` and make it
`pub(super)` with `pub(super)` fields because `workflow.rs` and
`benchmark.rs` will construct it:

```rust
pub(super) struct PackageDownloadRefresh<'a> {
    pub(super) distro: &'a str,
    pub(super) package_name: &'a str,
    pub(super) version: Option<&'a str>,
    pub(super) architecture: Option<&'a str>,
    pub(super) repo_pkg: RepositoryPackage,
    pub(super) dest_dir: &'a Path,
}
```

Move these methods into `lookup.rs`:

- `find_package_for_conversion_async`
- `find_package`
- `download_package_with_refresh_async`
- `is_upstream_not_found`

Set visibility exactly:

```rust
impl ConversionService {
    pub(super) async fn find_package_for_conversion_async(...) -> Result<RepositoryPackage> { ... }

    pub(super) fn find_package(...) -> Result<RepositoryPackage> { ... }

    pub(super) async fn download_package_with_refresh_async(
        &self,
        request: PackageDownloadRefresh<'_>,
    ) -> Result<(RepositoryPackage, PathBuf)> { ... }

    fn is_upstream_not_found(err: &conary_core::Error) -> bool { ... }
}
```

Inside `find_package`, keep the existing local imports:

```rust
use conary_core::repository::dependency_model::RepositoryDependencyFlavor;
use conary_core::repository::distro::{flavor_from_distro_name, flavor_to_version_scheme};
use conary_core::repository::versioning::compare_repo_versions;
```

`find_package` is `pub(super)` because non-lookup conversion tests still call
it after this move, and later `safety.rs` and `persistence.rs` tests remain
sibling modules. This does not expose it outside `server::conversion`.

- [ ] **Step 4: Move lookup tests**

Move these tests into `lookup.rs`:

- `test_find_package_found`
- `test_find_package_with_specific_version`
- `test_find_package_with_specific_version_and_architecture`
- `test_find_package_not_found`
- `test_find_package_unknown_distro`
- `test_find_package_arch_distro`
- `test_find_package_ubuntu_distro`
- `test_find_package_debian_is_not_supported_distro`
- `test_find_package_maps_distro_to_repo_pattern`
- `test_detects_upstream_not_found_download_error`

Use this test module import surface:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::test_support::{create_test_db, insert_package, insert_repo};
    use std::path::PathBuf;
}
```

The architecture-specific test also needs `RepositoryPackage`; import it from
`conary_core::db::models`.

- [ ] **Step 5: Verify Task 4**

Run:

```bash
cargo fmt
cargo check -p remi
cargo test -p remi --lib server::conversion::lookup::tests
cargo test -p remi --lib test_detects_upstream_not_found_download_error
cargo test -p remi --lib conversion
git diff --check
```

Expected: lookup tests pass and the upstream-not-found filter maps to the moved
test.

- [ ] **Step 6: Commit Task 4**

Run:

```bash
git add apps/remi/src/server/conversion.rs apps/remi/src/server/conversion/lookup.rs
git commit -m "refactor(remi): extract conversion package lookup"
```

---

## Task 5: Extract CAS Storage And Checksum Helpers

**Files:**
- Create: `apps/remi/src/server/conversion/storage.rs`
- Modify: `apps/remi/src/server/conversion.rs`

- [ ] **Step 1: Add the storage module**

In `conversion.rs`, enable:

```rust
mod storage;
```

- [ ] **Step 2: Create `storage.rs` imports**

Create `apps/remi/src/server/conversion/storage.rs`:

```rust
// apps/remi/src/server/conversion/storage.rs
//! CAS and optional R2 write-through storage for converted blobs.

use super::ConversionService;
use anyhow::{Context, Result};
use conary_core::ccs::convert::ConversionResult;
use std::path::Path;
use std::time::{Duration, Instant};
use tracing::debug;
```

- [ ] **Step 3: Move storage type and methods**

Move `StoredChunks` into `storage.rs` and make it `pub(super)` with
`pub(super)` fields because `workflow.rs` reads timing values:

```rust
pub(super) struct StoredChunks {
    pub(super) chunk_hashes: Vec<String>,
    pub(super) cas_duration: Duration,
    pub(super) r2_duration: Option<Duration>,
}
```

Move these methods into `storage.rs`:

- `store_chunks`
- `store_chunks_with_timing`
- `calculate_checksum`

Set visibility exactly:

```rust
impl ConversionService {
    #[cfg(test)]
    async fn store_chunks(&self, result: &ConversionResult) -> Result<Vec<String>> { ... }

    pub(super) async fn store_chunks_with_timing(
        &self,
        result: &ConversionResult,
    ) -> Result<StoredChunks> { ... }

    pub(super) fn calculate_checksum(path: &Path) -> Result<String> { ... }
}
```

- [ ] **Step 4: Move storage tests**

Move these tests into `storage.rs`:

- `test_calculate_checksum_valid_file`
- `test_calculate_checksum_empty_file`
- `test_calculate_checksum_missing_file`
- `test_store_chunks_writes_files`
- `test_store_chunks_idempotent`
- `test_store_chunks_empty`

Use this test module import surface:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::test_support::make_conversion_result;
    use std::path::{Path, PathBuf};
}
```

- [ ] **Step 5: Verify Task 5**

Run:

```bash
cargo fmt
cargo check -p remi
cargo test -p remi --lib server::conversion::storage::tests
cargo test -p remi --lib test_store_chunks
cargo test -p remi --lib conversion
git diff --check
```

Expected: checksum and storage tests pass in `storage.rs`.

- [ ] **Step 6: Commit Task 5**

Run:

```bash
git add apps/remi/src/server/conversion.rs apps/remi/src/server/conversion/storage.rs
git commit -m "refactor(remi): extract conversion storage helpers"
```

---

## Task 6: Extract Persistence And Cache Reconstruction

**Files:**
- Create: `apps/remi/src/server/conversion/persistence.rs`
- Modify: `apps/remi/src/server/conversion.rs`

- [ ] **Step 1: Add the persistence module**

In `conversion.rs`, enable:

```rust
mod persistence;
```

- [ ] **Step 2: Create `persistence.rs` imports**

Create `apps/remi/src/server/conversion/persistence.rs`:

```rust
// apps/remi/src/server/conversion/persistence.rs
//! Converted-package persistence, cache-hit reconstruction, and publication outcomes.

use super::{ConversionService, ScriptletPackageMetadata, ServerConversionResult};
use anyhow::{Result, anyhow};
use conary_core::ccs::convert::ConversionResult;
use conary_core::db::models::{CONVERSION_VERSION, ConvertedPackage, RepositoryPackage};
use conary_core::packages::common::PackageMetadata;
use crate::server::publication::{
    PublicationDecision, PublicationRefusal, ReviewArtifactInput, ServerConversionOutcome,
    classify_converted_package, decision_refusal, write_review_artifact,
};
use std::path::PathBuf;
use tracing::info;
```

- [ ] **Step 3: Move persistence input type**

Move `PersistConversionInput` into `persistence.rs` and make it `pub(super)`
with `pub(super)` fields:

```rust
pub(super) struct PersistConversionInput {
    pub(super) distro: String,
    pub(super) metadata: PackageMetadata,
    pub(super) format: &'static str,
    pub(super) original_checksum: String,
    pub(super) conversion_result: ConversionResult,
    pub(super) repo_pkg: RepositoryPackage,
    pub(super) chunk_hashes: Vec<String>,
}
```

- [ ] **Step 4: Move persistence methods**

Move these methods into `persistence.rs`:

- `cached_conversion_result_async`
- `persist_conversion_result`
- `build_result_from_existing`
- `outcome_from_converted_result`

Set visibility exactly:

```rust
impl ConversionService {
    pub(super) async fn cached_conversion_result_async(...) -> Result<Option<ServerConversionOutcome>> { ... }

    pub(super) fn persist_conversion_result(
        &self,
        input: PersistConversionInput,
    ) -> Result<ServerConversionOutcome> { ... }

    fn build_result_from_existing(...) -> Result<ServerConversionOutcome> { ... }

    fn outcome_from_converted_result(
        converted: &ConvertedPackage,
        mut result: ServerConversionResult,
    ) -> ServerConversionOutcome { ... }
}
```

Inside `persist_conversion_result`, replace the existing fully qualified
conversion version path:

```rust
conversion_version: conary_core::db::models::CONVERSION_VERSION,
```

with:

```rust
conversion_version: CONVERSION_VERSION,
```

- [ ] **Step 5: Move persistence tests**

Move these tests into `persistence.rs`:

- `test_build_result_from_existing_with_server_fields`
- `test_build_result_from_existing_without_chunk_hashes`
- `persisted_goal8a_golden_outcomes_respect_publication_gate`
- `persisted_conversion_records_scriptlet_metadata`

Use this test module import surface:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::test_support::{
        create_test_db, goal8a_scriptlet_summary, insert_package, insert_repo,
        make_conversion_result,
    };
    use conary_core::ccs::convert::{ScriptletBundleSummary, ScriptletDecisionCountsSummary};
    use conary_core::db::models::{ConvertedPackage, RepositoryPackage};
    use conary_core::packages::common::PackageMetadata;
    use std::path::PathBuf;
}
```

- [ ] **Step 6: Verify Task 6**

Run:

```bash
cargo fmt
cargo check -p remi
cargo clippy -p remi --lib -- -D warnings
cargo test -p remi --lib server::conversion::persistence::tests
cargo test -p remi publication
cargo test -p remi scriptlet
cargo test -p remi review_artifact
cargo test -p remi public_ready
cargo test -p remi --lib conversion
git diff --check
```

Expected:

- persistence tests pass in the new module;
- publication tests still pass because publication outcome behavior and private
  review-path redaction are unchanged.
- scriptlet, review artifact, public-ready, and broad conversion filters still
  pass after the persistence and cache-reconstruction move.

- [ ] **Step 7: Commit Task 6**

Run:

```bash
git add apps/remi/src/server/conversion.rs apps/remi/src/server/conversion/persistence.rs
git commit -m "refactor(remi): extract conversion persistence"
```

---

## Task 7: Extract Recipe Build And SSRF Validation

**Files:**
- Create: `apps/remi/src/server/conversion/recipe.rs`
- Modify: `apps/remi/src/server/conversion.rs`

- [ ] **Step 1: Add the recipe module**

In `conversion.rs`, enable:

```rust
mod recipe;
```

- [ ] **Step 2: Create `recipe.rs` imports**

Create `apps/remi/src/server/conversion/recipe.rs`:

```rust
// apps/remi/src/server/conversion/recipe.rs
//! Recipe URL fetching, SSRF validation, and server-side recipe builds.

use super::{ConversionService, ScriptletPackageMetadata, ServerConversionResult};
use anyhow::{Context, Result, anyhow};
use conary_core::ccs::convert::ScriptletBundleSummary;
use tempfile::TempDir;
use tracing::info;
```

- [ ] **Step 3: Move recipe methods**

Move these methods into `recipe.rs`:

- `build_from_recipe`
- `fetch_url`
- `validate_host`
- `validate_ip`

Keep `build_from_recipe` public:

```rust
impl ConversionService {
    pub async fn build_from_recipe(&self, recipe_url: &str) -> Result<ServerConversionResult> { ... }

    async fn fetch_url(url: &str) -> Result<String> { ... }

    fn validate_host(host: &str) -> Result<()> { ... }

    fn validate_ip(ip: &std::net::IpAddr) -> Result<()> { ... }
}
```

- [ ] **Step 4: Move recipe SSRF tests**

Move these tests into `recipe.rs`:

- `test_validate_host_allows_public`
- `test_validate_host_blocks_localhost`
- `test_validate_host_blocks_cloud_metadata`
- `test_validate_host_blocks_internal_domains`
- `test_validate_ip_allows_public_ipv4`
- `test_validate_ip_blocks_loopback`
- `test_validate_ip_blocks_private_10`
- `test_validate_ip_blocks_private_172`
- `test_validate_ip_blocks_private_192_168`
- `test_validate_ip_blocks_link_local`
- `test_validate_ip_blocks_broadcast`
- `test_validate_ip_blocks_unspecified`
- `test_validate_ip_allows_public_ipv6`
- `test_validate_ip_blocks_ipv6_loopback`
- `test_validate_ip_blocks_ipv6_unspecified`
- `test_validate_ip_blocks_ipv6_ula`
- `test_validate_ip_blocks_ipv6_link_local`

Add this no-network public-wrapper test to prove `build_from_recipe` reaches the
SSRF guard before any fetch:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn build_from_recipe_rejects_localhost_url_before_fetch() {
        let temp = tempfile::TempDir::new().unwrap();
        let service = ConversionService::new(
            temp.path().join("chunks"),
            temp.path().join("cache"),
            temp.path().join("remi.db"),
            None,
        );

        let err = service
            .build_from_recipe("https://localhost/recipe.conary")
            .await
            .expect_err("localhost recipe URL should be rejected before fetch")
            .to_string();

        assert!(err.contains("Localhost URLs are not allowed"));
    }
}
```

- [ ] **Step 5: Verify Task 7**

Run:

```bash
cargo fmt
cargo check -p remi
cargo test -p remi --lib server::conversion::recipe::tests
cargo test -p remi --lib build_from_recipe_rejects_localhost_url_before_fetch
cargo test -p remi --lib test_validate_host
cargo test -p remi --lib test_validate_ip
cargo test -p remi --lib conversion
git diff --check
```

Expected:

- all host/IP validation tests pass under `recipe.rs`;
- the new `build_from_recipe` wrapper test proves localhost URLs are rejected
  before network fetch;
- the broad conversion behavior filter still passes after moving recipe code.

- [ ] **Step 6: Commit Task 7**

Run:

```bash
git add apps/remi/src/server/conversion.rs apps/remi/src/server/conversion/recipe.rs
git commit -m "refactor(remi): extract conversion recipe build"
```

---

## Task 8: Extract Benchmark Evidence Methods

**Files:**
- Create: `apps/remi/src/server/conversion/benchmark.rs`
- Modify: `apps/remi/src/server/conversion.rs`

- [ ] **Step 1: Add the benchmark module**

In `conversion.rs`, enable:

```rust
mod benchmark;
```

- [ ] **Step 2: Create `benchmark.rs` imports**

Create `apps/remi/src/server/conversion/benchmark.rs`:

```rust
// apps/remi/src/server/conversion/benchmark.rs
//! Conversion benchmark and scan-only scriptlet corpus evidence.

use super::lookup::PackageDownloadRefresh;
use super::{ConversionBenchmarkEvidence, ConversionService};
use anyhow::{Context, Result, anyhow};
use tempfile::TempDir;
```

- [ ] **Step 3: Move benchmark methods**

Move these public methods into `benchmark.rs`:

- `benchmark_package_sample`
- `scan_package_scriptlets`
- `benchmark_package_conversion`

Keep all three public:

```rust
impl ConversionService {
    pub async fn benchmark_package_sample(...) -> Result<Vec<String>> { ... }

    pub async fn scan_package_scriptlets(...) -> Result<ConversionBenchmarkEvidence> { ... }

    pub async fn benchmark_package_conversion(...) -> Result<ConversionBenchmarkEvidence> { ... }
}
```

Inside `scan_package_scriptlets`, import and construct
`PackageDownloadRefresh` from `super::lookup`.

- [ ] **Step 4: Add no-network benchmark tests**

Add this test module to `benchmark.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::test_support::{create_test_db, insert_package, insert_repo};
    use std::path::PathBuf;

    #[tokio::test]
    async fn benchmark_package_sample_returns_largest_repository_packages_for_distro() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "small", "1.0", 10);
        insert_package(&conn, repo_id, "large", "1.0", 200);
        insert_package(&conn, repo_id, "medium", "1.0", 100);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let names = service.benchmark_package_sample("fedora", 2).await.unwrap();
        assert_eq!(names, vec!["large".to_string(), "medium".to_string()]);
    }

    #[tokio::test]
    async fn benchmark_package_conversion_returns_error_evidence_without_network() {
        let (temp_file, _conn) = create_test_db();
        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let evidence = service
            .benchmark_package_conversion("fedora", "missing-package", None, None)
            .await
            .unwrap();

        assert!(!evidence.converted);
        assert_eq!(evidence.cache_state, "error");
        assert!(evidence.error.unwrap().contains("not found"));
    }
}
```

- [ ] **Step 5: Verify Task 8**

Run:

```bash
cargo fmt
cargo check -p remi
cargo test -p remi --lib server::conversion::benchmark::tests
cargo test -p remi --lib benchmark_package
cargo test -p remi --test cli_help conversion
cargo test -p remi --lib conversion
git diff --check
```

Expected:

- benchmark module tests pass without network access;
- benchmark CLI help test passes;
- the broad conversion behavior filter still passes.

- [ ] **Step 6: Commit Task 8**

Run:

```bash
git add apps/remi/src/server/conversion.rs apps/remi/src/server/conversion/benchmark.rs
git commit -m "refactor(remi): extract conversion benchmark paths"
```

---

## Task 9: Extract Cold/Hot Conversion Workflow

**Files:**
- Create: `apps/remi/src/server/conversion/workflow.rs`
- Modify: `apps/remi/src/server/conversion.rs`

- [ ] **Step 1: Add the workflow module**

In `conversion.rs`, enable:

```rust
mod workflow;
```

- [ ] **Step 2: Create `workflow.rs` imports**

Create `apps/remi/src/server/conversion/workflow.rs`:

```rust
// apps/remi/src/server/conversion/workflow.rs
//! Cold/hot package conversion workflow orchestration.

use super::lookup::PackageDownloadRefresh;
use super::persistence::PersistConversionInput;
use super::ConversionService;
use crate::server::conversion_timing::{
    ConversionPhase, ConversionPhaseTiming, ConversionSkippedPhase, ConversionTimingReport,
};
use crate::server::publication::ServerConversionOutcome;
use anyhow::{Context, Result, anyhow};
use conary_core::ccs::convert::{ConversionOptions, ConversionResult, LegacyConverter};
use conary_core::db::models::RepositoryPackage;
use conary_core::packages::common::PackageMetadata;
use std::path::PathBuf;
use std::time::Instant;
use tempfile::TempDir;
use tracing::info;
```

- [ ] **Step 3: Move workflow-owned type**

Move `ParsedConversion` into `workflow.rs` and keep it private:

```rust
struct ParsedConversion {
    metadata: PackageMetadata,
    format: &'static str,
    original_checksum: String,
    conversion_result: ConversionResult,
    repo_pkg: RepositoryPackage,
    phase_timings: Vec<ConversionPhaseTiming>,
    skipped_phases: Vec<ConversionSkippedPhase>,
}
```

- [ ] **Step 4: Move workflow methods**

Move these methods into `workflow.rs`:

- `convert_package_async`
- `convert_package_async_inner`
- `record_cache_hit_skips`
- `log_conversion_timing`
- `parse_and_convert_package`

Keep only `convert_package_async` public:

```rust
impl ConversionService {
    pub async fn convert_package_async(...) -> Result<ServerConversionOutcome> { ... }

    async fn convert_package_async_inner(...) -> Result<ServerConversionOutcome> { ... }

    fn record_cache_hit_skips(timing: &mut ConversionTimingReport) { ... }

    fn log_conversion_timing(timing: &ConversionTimingReport) { ... }

    fn parse_and_convert_package(...) -> Result<ParsedConversion> { ... }
}
```

- [ ] **Step 5: Move and expand the async-work guard test**

Move `remi_server_conversion_paths_do_not_block_on_async_work` into
`workflow.rs`.

Update the scanned production paths to include every new production conversion
child module:

```rust
for relative_path in [
    "src/server/admin_service.rs",
    "src/server/conversion.rs",
    "src/server/conversion/benchmark.rs",
    "src/server/conversion/lookup.rs",
    "src/server/conversion/metadata.rs",
    "src/server/conversion/persistence.rs",
    "src/server/conversion/recipe.rs",
    "src/server/conversion/safety.rs",
    "src/server/conversion/storage.rs",
    "src/server/conversion/types.rs",
    "src/server/conversion/workflow.rs",
    "src/server/handlers/packages.rs",
    "src/server/prewarm.rs",
] {
    let source = production_source_without_comments(relative_path);
    assert!(
        !source.contains(".block_on("),
        "{relative_path} must not call Handle::block_on in production Remi server paths"
    );
}
```

Use this test module import surface:

```rust
#[cfg(test)]
mod tests {
    use super::super::test_support::production_source_without_comments;
}
```

Before moving the guard, confirm `production_source_without_comments` uses the
Task 1 `#[cfg(test)] mod tests` boundary logic. It must not stop at the earlier
`#[cfg(test)] mod test_support;` declaration in the hub.

- [ ] **Step 6: Shrink `conversion.rs` to the final hub**

After moving workflow, remove all stale production imports from `conversion.rs`
except:

```rust
use crate::server::R2Store;
use std::path::PathBuf;
use std::sync::Arc;
```

Confirm `conversion.rs` contains only:

- module declarations;
- public type re-export;
- `ConversionService`;
- `ConversionService::new`;
- the constructor test.

- [ ] **Step 7: Verify Task 9**

Run:

```bash
cargo fmt
cargo check -p remi
cargo clippy -p remi --lib -- -D warnings
cargo test -p remi --lib server::conversion::workflow::tests
cargo test -p remi --lib server::conversion::tests::test_conversion_service_new
cargo test -p remi --lib server::conversion::tests -- --list
cargo test -p remi --lib conversion
rg -n '^(pub |pub\(|fn |async fn|struct |enum |impl |mod |pub use |#\[cfg\(test\)\])' apps/remi/src/server/conversion.rs
rg -n 'use crate::server::conversion' apps/remi/src/server -g '*.rs'
scripts/line-count-report.sh 30
git diff --check
```

Expected:

- workflow test passes;
- parent `server::conversion::tests -- --list` now lists only
  `test_conversion_service_new`;
- broad conversion behavior remains healthy;
- the final hub item audit lists only module declarations, public re-exports,
  `ConversionService`, `ConversionService::new`, and the constructor test;
- internal callers still import through `crate::server::conversion` and resolve
  via the hub re-exports;
- `conversion.rs` drops sharply in the hotspot report.

- [ ] **Step 8: Commit Task 9**

Run:

```bash
git add apps/remi/src/server/conversion.rs apps/remi/src/server/conversion/workflow.rs
git commit -m "refactor(remi): extract conversion workflow"
```

---

## Task 10: Update Remi Docs And Final Verification

**Files:**
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/modules/remi.md`
- Modify: `docs/modules/test-fixtures.md`
- Modify: `docs/conaryopedia-v2.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Update subsystem map Remi pointers**

In `docs/llms/subsystem-map.md`, expand the Remi look-here-first bullet from:

```markdown
- Remi admin, publication, artifact fixture, and MCP flows:
  `apps/remi/src/server/admin_service.rs`,
  `apps/remi/src/server/publication.rs`,
  `apps/remi/src/server/mcp.rs`,
  `apps/remi/src/server/handlers/artifacts.rs`,
  `apps/remi/src/server/handlers/admin/`, and
  `docs/modules/test-fixtures.md`
```

to include the conversion hub and the highest-signal child modules:

```markdown
- Remi admin, conversion, publication, artifact fixture, and MCP flows:
  `apps/remi/src/server/admin_service.rs`,
  `apps/remi/src/server/conversion.rs`,
  `apps/remi/src/server/conversion/workflow.rs`,
  `apps/remi/src/server/conversion/persistence.rs`,
  `apps/remi/src/server/conversion/lookup.rs`,
  `apps/remi/src/server/conversion/metadata.rs`,
  `apps/remi/src/server/publication.rs`,
  `apps/remi/src/server/mcp.rs`,
  `apps/remi/src/server/handlers/artifacts.rs`,
  `apps/remi/src/server/handlers/admin/`, and
  `docs/modules/test-fixtures.md`
```

- [ ] **Step 2: Update feature ownership Remi card**

In `docs/modules/feature-ownership.md`, replace this exact Remi card segment:

```markdown
`apps/remi/src/server/publication.rs`;
`apps/remi/src/server/conversion.rs`; `apps/remi/src/server/index_gen.rs`;
```

with:

```markdown
`apps/remi/src/server/publication.rs`;
`apps/remi/src/server/conversion.rs`;
`apps/remi/src/server/conversion/types.rs`;
`apps/remi/src/server/conversion/workflow.rs`;
`apps/remi/src/server/conversion/persistence.rs`;
`apps/remi/src/server/conversion/lookup.rs`;
`apps/remi/src/server/conversion/metadata.rs`;
`apps/remi/src/server/conversion/safety.rs`;
`apps/remi/src/server/conversion/storage.rs`;
`apps/remi/src/server/conversion/recipe.rs`;
`apps/remi/src/server/conversion/benchmark.rs`;
`apps/remi/src/server/index_gen.rs`;
```

Keep `prewarm.rs`, handlers, and docs in the same card.

- [ ] **Step 3: Update Remi module guide**

In `docs/modules/remi.md`, add a section before `## Conversion Benchmark
Evidence`:

```markdown
## Conversion Service Ownership

The conversion service now keeps `apps/remi/src/server/conversion.rs` as the
stable public hub for `ConversionService` and conversion result DTO re-exports.
Implementation ownership lives in child modules:

- `conversion/workflow.rs`: cold/hot package conversion orchestration and
  timing.
- `conversion/types.rs`: public conversion result DTOs, scriptlet package
  metadata projection, and conversion benchmark evidence records.
- `conversion/benchmark.rs`: benchmark sampling, scan-only scriptlet evidence,
  and benchmark conversion wrappers.
- `conversion/lookup.rs`: repository package selection, supported distro
  mapping, upstream download, and one-shot metadata refresh after upstream
  404s.
- `conversion/metadata.rs`: safe CCS filenames, package parsing, metadata
  construction, repository identity application, and repository-provide merging.
- `conversion/safety.rs`: critical package and runtime capability refusal
  guards.
- `conversion/storage.rs`: local CAS writes, optional R2 write-through, and
  checksum helpers.
- `conversion/persistence.rs`: converted-package rows, cache-hit
  reconstruction, review artifact persistence, and publication outcome
  wrapping.
- `conversion/recipe.rs`: recipe URL fetch, DNS/IP validation, SSRF refusal,
  and server-side recipe builds.
- `conversion/test_support.rs`: conversion-owned test DB, repository package,
  conversion result, and scriptlet summary builders shared by child-module
  tests.

For conversion behavior changes, start with the owner module and run the
focused module tests plus `cargo test -p remi --lib conversion`. For public
listing, review artifact, or scriptlet-publication behavior changes, also run
`cargo test -p remi publication`.
```

- [ ] **Step 4: Update fixture map conversion sources**

In `docs/modules/test-fixtures.md`, update the
`remi-scriptlet-publication-gate` fixture sources from:

```markdown
  `apps/remi/src/server/conversion.rs`; `apps/remi/src/server/index_gen.rs`;
```

to:

```markdown
  `apps/remi/src/server/conversion.rs`;
  `apps/remi/src/server/conversion/test_support.rs`;
  `apps/remi/src/server/conversion/persistence.rs`;
  `apps/remi/src/server/conversion/workflow.rs`;
  `apps/remi/src/server/index_gen.rs`;
```

- [ ] **Step 5: Update conaryopedia Remi architecture tree**

In `docs/conaryopedia-v2.md`, replace the conversion tree line:

```text
  conversion.rs       ConversionService: download -> parse -> CCS -> store
```

with:

```text
  conversion.rs       ConversionService public hub
  conversion/         workflow, lookup, metadata, safety, storage, persistence, recipe, benchmark
```

Then replace:

```markdown
The `ConversionService` (`apps/remi/src/server/conversion.rs`) orchestrates this pipeline:
```

with:

```markdown
The `ConversionService` public hub lives at `apps/remi/src/server/conversion.rs`.
The pipeline orchestration lives in `apps/remi/src/server/conversion/workflow.rs`
with supporting lookup, metadata, safety, storage, persistence, recipe, and
benchmark child modules under `apps/remi/src/server/conversion/`:
```

Also replace the filename-sanitization sentence:

```markdown
Filename sanitization prevents path traversal -- `safe_ccs_filename()` passes both the package name and version through `sanitize_filename()` before constructing the output path.
```

with:

```markdown
Filename sanitization lives in
`apps/remi/src/server/conversion/metadata.rs`; `safe_ccs_filename()` passes
both the package name and version through `sanitize_filename()` before
constructing the output path.
```

- [ ] **Step 6: Refresh ledger rows for touched docs**

Update existing ledger rows for these active docs so their evidence sources and
notes mention the Phase 14 Remi conversion child modules:

- `docs/llms/subsystem-map.md`
- `docs/modules/feature-ownership.md`
- `docs/modules/remi.md`
- `docs/modules/test-fixtures.md`
- `docs/conaryopedia-v2.md`

Required evidence additions:

```text
apps/remi/src/server/conversion.rs; apps/remi/src/server/conversion/types.rs; apps/remi/src/server/conversion/test_support.rs; apps/remi/src/server/conversion/workflow.rs; apps/remi/src/server/conversion/persistence.rs; apps/remi/src/server/conversion/lookup.rs; apps/remi/src/server/conversion/metadata.rs; apps/remi/src/server/conversion/safety.rs; apps/remi/src/server/conversion/storage.rs; apps/remi/src/server/conversion/recipe.rs; apps/remi/src/server/conversion/benchmark.rs
```

Required note phrase for each updated row:

```text
Phase 14 Remi conversion service child-module ownership
```

- [ ] **Step 7: Final verification**

Run:

```bash
cargo fmt --check
cargo check -p remi
cargo test -p remi --lib --no-run
cargo test -p remi --lib server::conversion::types::tests
cargo test -p remi --lib server::conversion::metadata::tests
cargo test -p remi --lib server::conversion::safety::tests
cargo test -p remi --lib server::conversion::lookup::tests
cargo test -p remi --lib server::conversion::storage::tests
cargo test -p remi --lib server::conversion::persistence::tests
cargo test -p remi --lib server::conversion::recipe::tests
cargo test -p remi --lib server::conversion::workflow::tests
cargo test -p remi --lib server::conversion::tests
cargo test -p remi publication
cargo test -p remi scriptlet
cargo test -p remi review_artifact
cargo test -p remi public_ready
cargo test -p remi --test cli_help conversion
cargo test -p remi --lib conversion
cargo test -p remi conversion
cargo test -p remi
cargo test -p conary --test conversion_integration golden_conversion
cargo clippy -p remi --all-targets -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
LC_ALL=C bash scripts/docs-audit-inventory.sh | tail -n +2 | wc -l
awk -F'\t' 'NR>1 {counts[$8]++} END {for (k in counts) print k, counts[k]}' docs/superpowers/documentation-accuracy-audit-ledger.tsv | sort
awk -F'\t' 'NF != 9 { print NR ":" NF ":" $0 }' docs/superpowers/documentation-accuracy-audit-ledger.tsv
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
scripts/maintainability-drift-report.sh
scripts/line-count-report.sh 30
rg -n "apps/remi/src/server/conversion/(types|workflow|persistence|lookup|metadata|safety|storage|recipe|benchmark|test_support)\\.rs" \
  docs/llms/subsystem-map.md docs/modules/feature-ownership.md docs/modules/remi.md docs/modules/test-fixtures.md docs/conaryopedia-v2.md
if rg -n 'conversion\.rs\s+ConversionService: download -> parse -> CCS -> store|The `ConversionService` \(`apps/remi/src/server/conversion.rs`\) orchestrates this pipeline' docs/conaryopedia-v2.md; then
  exit 1
fi
git diff --check
```

Expected:

- all Remi and conversion integration tests pass;
- `cargo clippy -p remi --all-targets -- -D warnings` passes;
- workspace clippy passes or any unrelated pre-existing warning is documented
  with exact output before stopping;
- docs-audit inventory remains `158`;
- ledger distribution remains `corrected 58`, `archived 73`,
  `retained-historical 14`, `verified-no-change 13`;
- malformed-row check prints nothing;
- ledger checker passes;
- drift report runs successfully;
- line-count report shows `apps/remi/src/server/conversion.rs` is no longer a
  large hotspot;
- path grep shows updated docs route readers to the conversion hub and child
  modules.

- [ ] **Step 8: Commit Task 10**

Run:

```bash
git add docs/llms/subsystem-map.md \
  docs/modules/feature-ownership.md \
  docs/modules/remi.md \
  docs/modules/test-fixtures.md \
  docs/conaryopedia-v2.md \
  docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs: route remi conversion modules"
```

---

## Final Self-Review Checklist

- [ ] `apps/remi/src/server/mod.rs` still re-exports
  `ConversionBenchmarkEvidence`, `ConversionService`, and
  `ServerConversionResult`.
- [ ] `crate::server::conversion::ScriptletPackageMetadata` still resolves for
  server sibling modules.
- [ ] `ConversionService::convert_package_async`,
  `benchmark_package_sample`, `scan_package_scriptlets`,
  `benchmark_package_conversion`, and `build_from_recipe` signatures are
  unchanged.
- [ ] `ConversionService` fields remain private in `conversion.rs`.
- [ ] Only cross-child helpers are `pub(super)`; no moved helper is
  `pub(crate)` unless it was public before the refactor.
- [ ] `conversion.rs` has no large production workflow left after Task 9.
- [ ] Parent `server::conversion::tests` has only the constructor test.
- [ ] The no-`Handle::block_on` guard scans all new conversion production child
  modules.
- [ ] Publication tests pass after persistence moves.
- [ ] The docs-audit count moves from 157/57 to 158/58 during Task 0 lock-in
  and remains 158/58 through implementation.
- [ ] Active Remi docs route readers to the new child module owners.
- [ ] No unresolved planning markers or stale module paths are introduced in the
  plan or docs changes.

## Rollback Plan

Each task commits independently. If a later task fails in a way that cannot be
fixed within the task:

1. Stop before starting the next task.
2. Keep already-passing earlier task commits.
3. Use `git status --short` and `git diff` to identify only the current task's
   changes.
4. Revert the current task commit if it exists with `git revert <commit>`.
5. If the current task has not been committed, remove only the files created by
   that task and restore only the moved code for that task from the previous
   commit using non-destructive file edits.
6. Re-run the last passing verification command set before handing the branch
   back for review.

Do not use `git reset --hard` or `git checkout --` in this shared worktree.
