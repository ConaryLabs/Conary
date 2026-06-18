# M4b Native Authoring Build Lint And Local Test Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Locked for implementation after DeepSeek, Gemini, and local agentic review.

**Goal:** Implement the first native CCS v2 maintainer loop: initialize a minimal package, lint it, build a local-dev signed v2 `.ccs`, verify it, and dry-run-test installation without mutating the live host.

**Architecture:** Keep CCS command entrypoints thin and move reusable behavior into focused command/core modules. `apps/conary/src/commands/ccs/templates.rs`, `lint.rs`, `local_dev.rs`, and `test.rs` own CLI ergonomics and local state; `crates/conary-core/src/ccs/v2/authoring.rs` owns projection from parsed authoring data plus `BuildResult` into signed v2 authority. The implementation reuses the existing builder scan, v2 writer, verifier, and CCS dry-run install path rather than adding another scanner or installer.

**Tech Stack:** Rust 2024, Clap value enums, TOML manifest parsing, serde/serde_json diagnostics, existing `CcsBuilder`, existing v2 schema/validation/writer/reader, Ed25519 `SigningKeyPair`, temporary roots/databases with `tempfile`, existing static-repo publish gate tests, Cargo integration tests.

---

## Design Inputs

Read these before executing:

- `AGENTS.md`
- `docs/superpowers/specs/2026-06-18-m4b-native-authoring-build-lint-test-design.md`
- `docs/superpowers/specs/2026-06-17-m4-ccs-native-ecosystem-design.md`
- `docs/superpowers/specs/2026-06-17-m4a-ccs-v2-native-package-contract-design.md`
- `docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md`
- `docs/modules/ccs.md`
- `docs/modules/test-fixtures.md`
- `docs/llms/subsystem-map.md`
- `docs/modules/feature-ownership.md`
- `apps/conary/src/cli/ccs.rs`
- `apps/conary/src/dispatch/ccs.rs`
- `apps/conary/src/dispatch/root.rs`
- `apps/conary/src/command_risk.rs`
- `apps/conary/src/commands/ccs/init.rs`
- `apps/conary/src/commands/ccs/build.rs`
- `apps/conary/src/commands/ccs/inspect.rs`
- `apps/conary/src/commands/ccs/install/command.rs`
- `crates/conary-core/src/ccs/manifest.rs`
- `crates/conary-core/src/ccs/builder.rs`
- `crates/conary-core/src/ccs/builder/package_writer.rs`
- `crates/conary-core/src/ccs/signing.rs`
- `crates/conary-core/src/ccs/verify.rs`
- `crates/conary-core/src/ccs/v2/schema.rs`
- `crates/conary-core/src/ccs/v2/validation.rs`
- `crates/conary-core/src/ccs/v2/diagnostics.rs`
- `crates/conary-core/src/repository/static_repo/publish_gate.rs`

## Scope Locks

M4b includes:

- `conary ccs init --template minimal-file`.
- `ccs.toml` authoring fields for v2 package identity: `release` and `kind`.
- Maintainer-facing `conary ccs lint` with text and JSON output.
- `conary ccs build --format v2 --local-dev` for the first package path.
- Optional `conary ccs build --format v2 --key <private-key>` for explicit release-key signing.
- Local-dev key creation/loading under user-local Conary state.
- Local-dev trust for `ccs verify` and `ccs test` only; static publish and Remi release trust remain separate.
- V2 authority projection for ordinary file-owning package authoring.
- `conary ccs test --dry-run` using an isolated database/root and the existing CCS dry-run install path.
- Focused M4b integration tests plus M4a/M2 regression proof.

M4b excludes:

- Remi native publication, intake, staging, and promotion.
- Fedora 44, Ubuntu 26.04, or Arch target-profile facts.
- Service, tmpfiles, sysctl, alternatives, users, groups, and directory templates.
- `config-noreplace` as a required smoke path.
- Group and redirect authoring, unless tiny parser support falls out without widening the smoke path.
- Retiring all v1 build behavior.
- Letting unsigned v2 or local-dev signed packages pass release publish gates.

## Command Shape Decisions

- Keep existing `--target` as the ecosystem output selector: `ccs`, `deb`, `rpm`, `arch`, or `all`.
- Add `--format v1|v2` to `conary ccs build`; default remains `v1` for existing behavior.
- Require `--local-dev` or `--key <path>` when `--format v2` is selected.
- Treat `--local-dev` and `--key` as mutually exclusive, and reject both signing flags for `--format v1`.
- `conary ccs verify` with no `--policy` may load the user-local dev public key if it exists and must say so in output. This preserves the required smoke path while keeping release trust separate.
- `conary ccs test --dry-run` may use a supplied policy or generate a temporary policy from the local-dev public key; it must not trust arbitrary embedded signatures as release trust.

## File Map

Create:

- `apps/conary/src/commands/ccs/templates.rs` - minimal-file template generation and template tests.
- `apps/conary/src/commands/ccs/lint.rs` - CLI lint orchestration, text/JSON rendering, and authoring diagnostic buckets.
- `apps/conary/src/commands/ccs/local_dev.rs` - local-dev key paths, key generation/loading, and local-dev trust policy helpers.
- `apps/conary/src/commands/ccs/test.rs` - package-local dry-run test command using isolated root/database/trust policy.
- `crates/conary-core/src/ccs/v2/authoring.rs` - v2 package authority projection from `CcsManifest` and `BuildResult`.
- `apps/conary/tests/packaging_m4b.rs` - end-to-end CLI proof and negative regression tests.

Modify:

- `apps/conary/src/cli/ccs.rs` - add init template, build format/key/local-dev flags, `lint`, and `test`.
- `apps/conary/src/commands/ccs/mod.rs` - export new command modules.
- `apps/conary/src/commands/ccs/init.rs` - delegate template generation.
- `apps/conary/src/commands/ccs/build.rs` - route v2 builds through authoring projection and v2 writer.
- `apps/conary/src/commands/ccs/inspect.rs` - let verify load local-dev trust when no policy is supplied.
- `apps/conary/src/dispatch/ccs.rs` - route new command variants and new build/init args.
- `apps/conary/src/dispatch/root.rs` - route `lint`/`test` database selection and preflight behavior.
- `apps/conary/src/command_risk.rs` - classify init/build/test as local-state or dry-run-only isolated mutations, never active-host mutations.
- `crates/conary-core/src/ccs/manifest.rs` - parse `release` and `kind` as small v2 authoring fields.
- `crates/conary-core/src/ccs/v2/mod.rs` - export `authoring`.
- `crates/conary-core/src/ccs/v2/reader.rs` - keep debug TOML rejection aligned with new authoring fields.
- `crates/conary-core/src/ccs/builder/test_support.rs` - add focused fixtures if unit tests need BuildResult construction helpers.
- `crates/conary-core/src/repository/static_repo/publish_gate.rs` - add negative regression coverage for local-dev v2 output when needed.
- `docs/modules/ccs.md` - document the M4b loop after implementation passes.
- `docs/modules/test-fixtures.md` - document the M4b smoke fixture.
- `docs/llms/subsystem-map.md` and `docs/modules/feature-ownership.md` - update look-here-first routes when command ownership changes.
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv` and `docs/superpowers/documentation-accuracy-audit-ledger.tsv` - register the plan and touched docs.

Maintainability boundaries:

- Keep `apps/conary/src/commands/ccs/build.rs` as a thin orchestrator; do not bury v2 projection there.
- Keep `crates/conary-core/src/ccs/manifest.rs` to small serde fields only; no v2 validation engine there.
- Keep `crates/conary-core/src/ccs/v2/validation.rs` as the contract validator; lint may reclassify diagnostics for UX but must not fork validation rules.
- Do not add another install engine; `ccs test` calls existing CCS install with an isolated root/database.

## Checkpoints

- Checkpoint 1 after Task 2: template and parser tests pass.
- Checkpoint 2 after Task 4: authoring projection and lint tests pass.
- Checkpoint 3 after Task 5: v2 local-dev build and verify tests pass.
- Checkpoint 4 after Task 7: full M4b smoke test passes.
- Checkpoint 5 after Task 8: docs, fmt, clippy, and regression suites pass.

## Review Lock Mapping

| Design concern | Plan owner |
| --- | --- |
| `ccs.toml` remains the authoring filename but v2 identity is explicit | Task 1 |
| Template generation stays command-owned | Task 1 |
| `manifest.rs` remains a parser, not a v2 validator | Task 1 and Task 3 |
| V2 projection consumes existing builder scan output | Task 3 |
| Chunked builder output still yields whole v2 payloads | Task 3 |
| Profile-deferred lifecycle facts do not silently enter signed authority | Task 4 |
| Local-dev signing is visible and separate from release trust | Task 5 |
| `ccs verify` smoke path works with local-dev trust | Task 5 |
| `ccs test` reuses verify plus dry-run install in an isolated workspace | Task 6 |
| Command risk never classifies M4b as active-host mutation | Task 2 and Task 6 |
| Local-dev output fails static publish/Remi release trust | Task 7 |
| Docs-audit and coherency gates stay explicit | Task 8 |

---

### Task 1: Add V2 Authoring Fields And Minimal Template

**Files:**
- Modify: `crates/conary-core/src/ccs/manifest.rs`
- Create: `apps/conary/src/commands/ccs/templates.rs`
- Modify: `apps/conary/src/commands/ccs/init.rs`
- Modify: `apps/conary/src/commands/ccs/mod.rs`
- Modify: `apps/conary/src/cli/ccs.rs`
- Modify: `apps/conary/src/dispatch/ccs.rs`

- [ ] **Step 1: Add failing manifest parser test**

Add this test near existing `CcsManifest` tests in `crates/conary-core/src/ccs/manifest.rs`:

```rust
#[test]
fn parses_v2_authoring_identity_fields_without_guessing_release() {
    let manifest = CcsManifest::parse(
        r#"
[package]
name = "hello"
version = "0.1.0"
release = "1"
kind = "package"
description = "hello"
"#,
    )
    .unwrap();

    assert_eq!(manifest.package.release.as_deref(), Some("1"));
    assert_eq!(manifest.package.kind, Some(PackageKindTagV2::Package));

    let legacy = CcsManifest::parse(
        r#"
[package]
name = "legacy"
version = "1.0.0-1"
description = "legacy"
"#,
    )
    .unwrap();

    assert_eq!(legacy.package.release, None);
    assert_eq!(legacy.package.kind, None);
}
```

- [ ] **Step 2: Run parser test and verify it fails**

Run:

```bash
cargo test -p conary-core parses_v2_authoring_identity_fields_without_guessing_release
```

Expected: FAIL because `release` and `kind` do not exist on `Package`.

- [ ] **Step 3: Add small parser fields**

Before editing `crates/conary-core/src/ccs/manifest.rs`, name the ownership
boundary being preserved: this file remains the TOML parser/default owner, not a
v2 validation engine.

In `crates/conary-core/src/ccs/manifest.rs`, import the existing v2 kind tag and
extend `Package`:

```rust
use crate::ccs::v2::PackageKindTagV2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub description: String,

    #[serde(default)]
    pub release: Option<String>,

    #[serde(default)]
    pub kind: Option<PackageKindTagV2>,

    #[serde(default)]
    pub license: Option<String>,
```

Also update `CcsManifest::new_minimal` so `release` and `kind` default to `None`, not guessed values.

- [ ] **Step 4: Create template tests**

Create `apps/conary/src/commands/ccs/templates.rs`:

```rust
// apps/conary/src/commands/ccs/templates.rs

use anyhow::Result;
use clap::ValueEnum;
use conary_core::ccs::v2::PackageKindTagV2;
use conary_core::ccs::CcsManifest;

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum CcsInitTemplate {
    MinimalFile,
}

pub fn build_manifest(
    template: Option<CcsInitTemplate>,
    name: &str,
    version: &str,
) -> Result<CcsManifest> {
    match template {
        Some(CcsInitTemplate::MinimalFile) => minimal_file_manifest(name, version),
        None => Ok(CcsManifest::new_minimal(name, version)),
    }
}

fn minimal_file_manifest(name: &str, version: &str) -> Result<CcsManifest> {
    let mut manifest = CcsManifest::new_minimal(name, version);
    manifest.package.release = Some("1".to_string());
    manifest.package.kind = Some(PackageKindTagV2::Package);
    manifest.package.description = format!("{name} package");
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_file_template_writes_v2_identity_fields() {
        let manifest = build_manifest(Some(CcsInitTemplate::MinimalFile), "hello", "0.1.0")
            .expect("template manifest");

        assert_eq!(manifest.package.name, "hello");
        assert_eq!(manifest.package.version, "0.1.0");
        assert_eq!(manifest.package.release.as_deref(), Some("1"));
        assert_eq!(manifest.package.kind, Some(PackageKindTagV2::Package));
    }
}
```

- [ ] **Step 5: Wire init CLI and command**

In `apps/conary/src/cli/ccs.rs`, import `ValueEnum` and add the template enum if it is not re-exported from the command module. Then add this field to `CcsCommands::Init`:

```rust
        /// Authoring template to generate
        #[arg(long, value_enum)]
        template: Option<crate::commands::ccs::CcsInitTemplate>,
```

In `apps/conary/src/commands/ccs/mod.rs`, add:

```rust
mod templates;
pub use templates::CcsInitTemplate;
```

In `apps/conary/src/commands/ccs/init.rs`, update `cmd_ccs_init`:

```rust
pub async fn cmd_ccs_init(
    path: &str,
    name: Option<String>,
    version: &str,
    force: bool,
    template: Option<super::templates::CcsInitTemplate>,
) -> Result<()> {
```

Call `templates::build_manifest(template, &pkg_name, version)` before project metadata inference. The project-detection helper must accept that pre-built manifest as its base and overlay only detected name/version/description/license/homepage/repository fields; it must not call `CcsManifest::new_minimal` again in a way that clears `release` or `kind`.

In `apps/conary/src/dispatch/ccs.rs`, pass the `template` argument to `cmd_ccs_init`.

- [ ] **Step 6: Run template and parser tests**

Run:

```bash
cargo test -p conary-core parses_v2_authoring_identity_fields_without_guessing_release
cargo test -p conary --lib minimal_file_template_writes_v2_identity_fields
```

Expected: PASS.

- [ ] **Step 7: Commit Task 1**

```bash
git add crates/conary-core/src/ccs/manifest.rs apps/conary/src/commands/ccs/templates.rs apps/conary/src/commands/ccs/init.rs apps/conary/src/commands/ccs/mod.rs apps/conary/src/cli/ccs.rs apps/conary/src/dispatch/ccs.rs
git commit -m "feat(ccs): add minimal v2 authoring template"
```

### Task 2: Add CLI Surface And Command-Risk Classification

**Files:**
- Modify: `apps/conary/src/cli/ccs.rs`
- Modify: `apps/conary/src/dispatch/ccs.rs`
- Modify: `apps/conary/src/dispatch/root.rs`
- Modify: `apps/conary/src/command_risk.rs`

- [ ] **Step 1: Add failing command-risk tests**

In `apps/conary/src/command_risk.rs`, add tests next to existing CLI risk tests:

```rust
#[test]
fn m4b_ccs_authoring_commands_are_not_active_host_mutations() {
    let init = policy(&["conary", "ccs", "init", "--template", "minimal-file"]);
    assert_eq!(init.risk, CommandRisk::LocalStateMutation);

    let lint = policy(&["conary", "ccs", "lint"]);
    assert_eq!(lint.risk, CommandRisk::ReadOnly);

    let build = policy(&["conary", "ccs", "build", "--format", "v2", "--local-dev"]);
    assert_eq!(build.risk, CommandRisk::LocalStateMutation);

    let test = policy(&["conary", "ccs", "test", "pkg.ccs", "--dry-run"]);
    assert_eq!(test.risk, CommandRisk::LocalStateMutation);
}
```

- [ ] **Step 2: Run risk test and verify it fails**

Run:

```bash
cargo test -p conary --lib m4b_ccs_authoring_commands_are_not_active_host_mutations
```

Expected: FAIL because `lint`, `test`, build `--format`, and local-state classifications do not exist yet.

- [ ] **Step 3: Add CLI variants and build flags**

In `apps/conary/src/cli/ccs.rs`, add value enums:

```rust
#[derive(Clone, Copy, Debug, clap::ValueEnum, PartialEq, Eq)]
pub enum CcsBuildFormat {
    V1,
    V2,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum, PartialEq, Eq)]
pub enum CcsOutputFormat {
    Text,
    Json,
}
```

Add these fields to `Build`:

```rust
        /// CCS archive contract version
        #[arg(long, value_enum, default_value_t = CcsBuildFormat::V1)]
        format: CcsBuildFormat,

        /// Sign v2 output with the user-local development key
        #[arg(long, conflicts_with = "key")]
        local_dev: bool,

        /// Private signing key for v2 output
        #[arg(long)]
        key: Option<String>,
```

Add variants:

```rust
    /// Lint a CCS authoring manifest
    Lint {
        /// Path to ccs.toml or directory containing it
        #[arg(default_value = ".")]
        path: String,

        /// Output format
        #[arg(long, value_enum, default_value_t = CcsOutputFormat::Text)]
        format: CcsOutputFormat,
    },

    /// Verify and dry-run-test a CCS package in an isolated workspace
    Test {
        /// Path to .ccs package file
        package: String,

        /// Run the package-local install proof without creating live state
        #[arg(long)]
        dry_run: bool,

        /// Trust policy file
        #[arg(long)]
        policy: Option<String>,

        /// Keep the isolated test workspace after completion
        #[arg(long)]
        keep_workspace: bool,
    },
```

- [ ] **Step 4: Update dispatch and root routing**

Update `apps/conary/src/dispatch/ccs.rs` to pass new build fields and route `Lint`/`Test` to command functions that Task 4 and Task 6 will add:

```rust
        cli::CcsCommands::Lint { path, format } => {
            commands::ccs::cmd_ccs_lint(&path, format).await
        }

        cli::CcsCommands::Test {
            package,
            dry_run,
            policy,
            keep_workspace,
        } => commands::ccs::cmd_ccs_test(&package, dry_run, policy, keep_workspace).await,
```

Update `apps/conary/src/dispatch/root.rs` so `Lint` uses `DEFAULT_DB_PATH` and `Test` does not use the live database path. Add the new variants to the try-session preflight false branch.

- [ ] **Step 5: Update command risk**

In `classify_ccs`, classify commands explicitly:

```rust
        cli::CcsCommands::Init { .. } => local_state("conary ccs init"),
        cli::CcsCommands::Build { .. } => local_state("conary ccs build"),
        cli::CcsCommands::Lint { .. } => read_only("conary ccs lint"),
        cli::CcsCommands::Test { .. } => local_state("conary ccs test"),
```

Leave `Inspect`, `Verify`, `Sign`, `Keygen`, `Export`, `Shell`, and `Run` behavior unchanged unless tests expose an existing mismatch that must be handled separately.

- [ ] **Step 6: Add temporary stubs so CLI compiles**

Add temporary command stubs in `apps/conary/src/commands/ccs/mod.rs` only if later task modules are not in place yet:

```rust
pub async fn cmd_ccs_lint(
    _path: &str,
    _format: crate::cli::CcsOutputFormat,
) -> anyhow::Result<()> {
    anyhow::bail!("conary ccs lint is wired but not implemented")
}

pub async fn cmd_ccs_test(
    _package: &str,
    _dry_run: bool,
    _policy: Option<String>,
    _keep_workspace: bool,
) -> anyhow::Result<()> {
    anyhow::bail!("conary ccs test is wired but not implemented")
}
```

Remove these stubs when Task 4 and Task 6 add real modules.

- [ ] **Step 7: Run risk test**

Run:

```bash
cargo test -p conary --lib m4b_ccs_authoring_commands_are_not_active_host_mutations
```

Expected: PASS.

- [ ] **Step 8: Commit Task 2**

```bash
git add apps/conary/src/cli/ccs.rs apps/conary/src/dispatch/ccs.rs apps/conary/src/dispatch/root.rs apps/conary/src/command_risk.rs apps/conary/src/commands/ccs/mod.rs
git commit -m "feat(ccs): wire m4b command surface"
```

### Task 3: Implement V2 Authoring Projection

**Files:**
- Create: `crates/conary-core/src/ccs/v2/authoring.rs`
- Modify: `crates/conary-core/src/ccs/v2/mod.rs`
- Modify: `crates/conary-core/src/ccs/builder/test_support.rs`

- [ ] **Step 1: Write projection tests**

Create `crates/conary-core/src/ccs/v2/authoring.rs` with tests first:

```rust
// conary-core/src/ccs/v2/authoring.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::builder::test_support;

    #[test]
    fn projection_requires_release_and_kind_for_v2_package_authoring() {
        let build = test_support::minimal_file_build_result("hello", "0.1.0", b"hello\n");

        let error = project_build_result_to_v2(V2AuthoringInput {
            build: &build,
            local_dev: true,
            debug_toml: None,
        })
        .unwrap_err();

        assert!(
            error.to_string().contains("release"),
            "expected release diagnostic, got {error}"
        );
    }

    #[test]
    fn projection_builds_complete_local_dev_package_authority() {
        let mut build = test_support::minimal_file_build_result("hello", "0.1.0", b"hello\n");
        build.manifest.package.release = Some("1".to_string());
        build.manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);

        let projected = project_build_result_to_v2(V2AuthoringInput {
            build: &build,
            local_dev: true,
            debug_toml: Some(build.manifest.to_toml().unwrap()),
        })
        .unwrap();

        assert_eq!(projected.authority.identity.name, "hello");
        assert_eq!(projected.authority.identity.release, "1");
        assert_eq!(
            projected.authority.provenance.hardening_level.as_deref(),
            Some("host")
        );
        assert!(projected.authority.components["runtime"].default);
        assert!(projected.payloads_by_path.contains_key("/hello"));
    }

    #[test]
    fn projection_keeps_host_hardening_for_release_key_signing_path() {
        let mut build = test_support::minimal_file_build_result("hello", "0.1.0", b"hello\n");
        build.manifest.package.release = Some("1".to_string());
        build.manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);

        let projected = project_build_result_to_v2(V2AuthoringInput {
            build: &build,
            local_dev: false,
            debug_toml: Some(build.manifest.to_toml().unwrap()),
        })
        .unwrap();

        assert_eq!(
            projected.authority.provenance.hardening_level.as_deref(),
            Some("host")
        );
    }
}
```

- [ ] **Step 2: Run projection tests and verify they fail**

Run:

```bash
cargo test -p conary-core ccs::v2::authoring
```

Expected: FAIL because `authoring.rs` is not exported and the test fixture does not exist.

- [ ] **Step 3: Add BuildResult test fixture**

In `crates/conary-core/src/ccs/builder/test_support.rs`, add a focused helper:

```rust
pub(crate) fn minimal_file_build_result(name: &str, version: &str, bytes: &[u8]) -> BuildResult {
    use super::{ComponentData, FileEntry, FileType};
    use std::collections::HashMap;

    let manifest = crate::ccs::manifest::CcsManifest::new_minimal(name, version);
    let hash = crate::hash::sha256(bytes);
    let entry = FileEntry {
        path: format!("/{name}"),
        hash: hash.clone(),
        size: bytes.len() as u64,
        mode: 0o755,
        component: "runtime".to_string(),
        file_type: FileType::Regular,
        target: None,
        chunks: None,
    };
    BuildResult {
        manifest,
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: vec![entry.clone()],
                hash: hash.clone(),
                size: bytes.len() as u64,
            },
        )]),
        files: vec![entry],
        blobs: HashMap::from([(hash, bytes.to_vec())]),
        total_size: bytes.len() as u64,
        chunked: false,
        chunk_stats: None,
    }
}
```

- [ ] **Step 4: Implement projection types and payload reconstruction**

Implement `authoring.rs`:

```rust
use super::schema::*;
use crate::ccs::builder::{BuildResult, FileType};
use crate::ccs::v2::PackageKindTagV2;
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;

pub struct V2AuthoringInput<'a> {
    pub build: &'a BuildResult,
    pub local_dev: bool,
    pub debug_toml: Option<String>,
}

pub struct ProjectedV2Package {
    pub authority: AuthorityDocumentV2,
    pub payloads_by_path: BTreeMap<String, Vec<u8>>,
    pub debug_toml: Option<String>,
}

pub fn project_build_result_to_v2(input: V2AuthoringInput<'_>) -> Result<ProjectedV2Package> {
    let package = &input.build.manifest.package;
    let release = package
        .release
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .context("v2 package authoring requires package.release")?;
    let kind = package.kind.context("v2 package authoring requires package.kind")?;
    if kind != PackageKindTagV2::Package {
        bail!("M4b only supports package authoring for v2 build");
    }

    let payloads_by_path = payloads_by_path(input.build)?;
    let files = input
        .build
        .files
        .iter()
        .map(|file| FileAuthorityV2 {
            path: file.path.clone(),
            sha256: file.hash.clone(),
            size: file.size,
            file_type: match file.file_type {
                FileType::Regular => FileTypeV2::Regular,
                FileType::Symlink => FileTypeV2::Symlink,
                FileType::Directory => FileTypeV2::Directory,
            },
            mode: file.mode,
            owner: "root".to_string(),
            group: "root".to_string(),
            component: file.component.clone(),
            symlink_target: file.target.clone(),
            config: None,
            conflict: ConflictPolicyV2::Error,
        })
        .collect::<Vec<_>>();

    let default_component = select_default_component(input.build)?;
    let components = input
        .build
        .components
        .iter()
        .map(|(name, component)| {
            (
                name.clone(),
                ComponentAuthorityV2 {
                    name: name.clone(),
                    default: name == &default_component,
                    file_count: component.files.len() as u32,
                    total_size: component.size,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();

    let build_input_identity = crate::hash::sha256(
        format!("{}:{}:{}", package.name, package.version, release).as_bytes(),
    );
    let evidence_hash = crate::hash::sha256(
        serde_json::json!({
            "mode": if input.local_dev { "local-dev" } else { "signed" },
            "package": package.name,
            "version": package.version,
            "release": release,
            "file_count": files.len(),
        })
        .to_string()
        .as_bytes(),
    );
    // M4b uses the existing host file scan for both local-dev and explicit-key signing.
    // Do not claim hermetic hardening until a later slice routes through a hermetic builder.

    let authority = AuthorityDocumentV2 {
        format_version: FORMAT_VERSION_V2,
        identity: PackageIdentityV2 {
            name: package.name.clone(),
            version: package.version.clone(),
            release: release.to_string(),
            architecture: package.platform.as_ref().and_then(|platform| platform.arch.clone()),
            platform: package.platform.as_ref().map(|platform| platform.os.clone()),
            kind: PackageKindTagV2::Package,
        },
        kind: PackageKindV2::Package(PackageDataV2 {
            files,
            config: Vec::new(),
            policy: PackagePolicyV2::default(),
        }),
        provides: Vec::new(),
        requires: Vec::new(),
        components,
        lifecycle: LifecycleAuthorityV2::default(),
        provenance: ProvenanceAuthorityV2 {
            origin_class: Some("native-built".to_string()),
            hardening_level: Some("host".to_string()),
            build_input_identity: Some(build_input_identity),
            hermetic_evidence_hash: Some(evidence_hash),
            foreign_conversion_boundary_hash: None,
        },
        debug_toml_sha256: input
            .debug_toml
            .as_ref()
            .map(|toml| crate::hash::sha256(toml.as_bytes())),
    };

    super::validation::validate_authority(&authority).map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(ProjectedV2Package {
        authority,
        payloads_by_path,
        debug_toml: input.debug_toml,
    })
}

fn select_default_component(build: &BuildResult) -> Result<String> {
    let manifest_defaults = build
        .manifest
        .components
        .default
        .iter()
        .filter(|name| build.components.contains_key(name.as_str()))
        .collect::<Vec<_>>();

    if let Some(name) = manifest_defaults.first() {
        return Ok((*name).clone());
    }
    if build.components.len() == 1 {
        return Ok(build.components.keys().next().expect("one component").clone());
    }
    bail!("v2 package authoring requires one default component present in build output");
}

fn payloads_by_path(build: &BuildResult) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut payloads = BTreeMap::new();
    for file in &build.files {
        if file.file_type != FileType::Regular {
            continue;
        }
        let bytes = if let Some(chunks) = &file.chunks {
            let mut bytes = Vec::new();
            for chunk_hash in chunks {
                bytes.extend(
                    build
                        .blobs
                        .get(chunk_hash)
                        .with_context(|| format!("missing chunk {chunk_hash} for {}", file.path))?,
                );
            }
            bytes
        } else {
            build
                .blobs
                .get(&file.hash)
                .with_context(|| format!("missing payload blob for {}", file.path))?
                .clone()
        };
        if crate::hash::sha256(&bytes) != file.hash {
            bail!("payload bytes for {} do not match builder hash", file.path);
        }
        payloads.insert(file.path.clone(), bytes);
    }
    Ok(payloads)
}
```

- [ ] **Step 5: Export the authoring module**

In `crates/conary-core/src/ccs/v2/mod.rs`, add:

```rust
pub mod authoring;
pub use authoring::{ProjectedV2Package, V2AuthoringInput, project_build_result_to_v2};
```

- [ ] **Step 6: Run projection tests**

Run:

```bash
cargo test -p conary-core ccs::v2::authoring
```

Expected: PASS.

- [ ] **Step 7: Commit Task 3**

```bash
git add crates/conary-core/src/ccs/v2/authoring.rs crates/conary-core/src/ccs/v2/mod.rs crates/conary-core/src/ccs/builder/test_support.rs
git commit -m "feat(ccs): project authoring manifests to v2 authority"
```

### Task 4: Implement Maintainer-Facing Lint

**Files:**
- Create: `apps/conary/src/commands/ccs/lint.rs`
- Modify: `apps/conary/src/commands/ccs/mod.rs`
- Modify: `crates/conary-core/src/ccs/v2/authoring.rs`
- Modify: `crates/conary-core/src/ccs/v2/reader.rs`

- [ ] **Step 1: Add lint diagnostic tests**

In `crates/conary-core/src/ccs/v2/authoring.rs`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthoringFindingBucket {
    Contract,
    PublicationReadiness,
    ProfileDeferred,
    Style,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthoringFindingSeverity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AuthoringFinding {
    pub code: &'static str,
    pub bucket: AuthoringFindingBucket,
    pub severity: AuthoringFindingSeverity,
    pub field: Option<&'static str>,
    pub message: String,
    pub suggestion: &'static str,
    pub blocks_build: bool,
    pub blocks_local_test: bool,
    pub blocks_publish: bool,
}
```

Add tests:

```rust
#[test]
fn lint_manifest_reports_missing_release_and_kind() {
    let manifest = crate::ccs::manifest::CcsManifest::new_minimal("hello", "0.1.0");
    let findings = lint_manifest_for_v2_authoring(&manifest);

    assert!(findings.iter().any(|f| f.field == Some("package.release")));
    assert!(findings.iter().any(|f| f.field == Some("package.kind")));
    assert!(findings.iter().all(|f| f.blocks_build));
}

#[test]
fn lint_manifest_marks_lifecycle_as_profile_deferred() {
    let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal("hello", "0.1.0");
    manifest.package.release = Some("1".to_string());
    manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
    manifest.hooks.services.push("hello.service".to_string());

    let findings = lint_manifest_for_v2_authoring(&manifest);
    assert!(findings.iter().any(|f| {
        f.bucket == AuthoringFindingBucket::ProfileDeferred && f.blocks_build
    }));
}

#[test]
fn lint_manifest_blocks_unresolved_dependencies_for_m4b() {
    let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal("hello", "0.1.0");
    manifest.package.release = Some("1".to_string());
    manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
    manifest.requires.packages.push(crate::ccs::manifest::PackageDep {
        name: "openssl".to_string(),
        version: Some(">=3.0".to_string()),
    });

    let findings = lint_manifest_for_v2_authoring(&manifest);
    assert!(findings.iter().any(|f| {
        f.code == "m4b-profile-deferred-dependencies" && f.blocks_build
    }));
}
```

- [ ] **Step 2: Implement manifest lint helper**

Add `lint_manifest_for_v2_authoring`:

```rust
pub fn lint_manifest_for_v2_authoring(
    manifest: &crate::ccs::manifest::CcsManifest,
) -> Vec<AuthoringFinding> {
    let mut findings = Vec::new();
    if manifest.package.release.as_deref().is_none_or(str::is_empty) {
        findings.push(AuthoringFinding {
            code: "m4b-missing-release",
            bucket: AuthoringFindingBucket::Contract,
            severity: AuthoringFindingSeverity::Error,
            field: Some("package.release"),
            message: "v2 package authoring requires package.release".to_string(),
            suggestion: "add release = \"1\" under [package]",
            blocks_build: true,
            blocks_local_test: true,
            blocks_publish: true,
        });
    }
    if manifest.package.kind.is_none() {
        findings.push(AuthoringFinding {
            code: "m4b-missing-kind",
            bucket: AuthoringFindingBucket::Contract,
            severity: AuthoringFindingSeverity::Error,
            field: Some("package.kind"),
            message: "v2 package authoring requires package.kind".to_string(),
            suggestion: "add kind = \"package\" under [package]",
            blocks_build: true,
            blocks_local_test: true,
            blocks_publish: true,
        });
    }
    if manifest.hooks.has_script_hooks()
        || manifest.hooks.has_service_hooks()
        || manifest.hooks.has_declarative_hooks()
    {
        findings.push(AuthoringFinding {
            code: "m4b-profile-deferred-lifecycle",
            bucket: AuthoringFindingBucket::ProfileDeferred,
            severity: AuthoringFindingSeverity::Warning,
            field: Some("hooks"),
            message: "lifecycle declarations need M4d target-profile facts before v2 build".to_string(),
            suggestion: "remove lifecycle declarations for the M4b minimal-file path",
            blocks_build: true,
            blocks_local_test: true,
            blocks_publish: true,
        });
    }
    if !manifest.requires.packages.is_empty() || !manifest.requires.capabilities.is_empty() {
        findings.push(AuthoringFinding {
            code: "m4b-profile-deferred-dependencies",
            bucket: AuthoringFindingBucket::ProfileDeferred,
            severity: AuthoringFindingSeverity::Warning,
            field: Some("requires"),
            message: "dependencies need database/profile support before v2 build".to_string(),
            suggestion: "remove [requires] entries for the M4b minimal-file path",
            blocks_build: true,
            blocks_local_test: true,
            blocks_publish: true,
        });
    }
    // PublicationReadiness and Style buckets are part of the stable diagnostic shape,
    // but M4b's first implementation only emits concrete contract/profile-deferred findings.
    findings
}
```

If field names differ in current manifest types, use the closest existing lifecycle/config fields and keep the test names unchanged.

- [ ] **Step 3: Keep v2 debug TOML rejection aligned**

In `crates/conary-core/src/ccs/v2/reader.rs`, extend
`reject_install_authority_toml` so it rejects service hooks too:

```rust
        || toml_manifest.hooks.has_service_hooks()
```

Add a focused reader test using current TOML syntax:

```rust
#[test]
fn v2_debug_toml_with_service_hooks_is_rejected() {
    let toml = r#"
[package]
name = "hello"
version = "0.1.0"

[[hooks.services]]
name = "hello.service"
action = "restart"
"#;

    let error = reject_install_authority_toml(Some(toml.as_bytes())).unwrap_err();
    assert!(error.to_string().contains("install-affecting"));
}
```

- [ ] **Step 4: Create CLI lint module**

Create `apps/conary/src/commands/ccs/lint.rs`:

```rust
// apps/conary/src/commands/ccs/lint.rs

use anyhow::{Context, Result};
use conary_core::ccs::v2::authoring::lint_manifest_for_v2_authoring;
use conary_core::ccs::CcsManifest;
use std::path::Path;

pub async fn cmd_ccs_lint(path: &str, format: crate::cli::CcsOutputFormat) -> Result<()> {
    let manifest_path = manifest_path(path)?;
    let manifest = CcsManifest::from_file(&manifest_path).context("Failed to parse ccs.toml")?;
    let findings = lint_manifest_for_v2_authoring(&manifest);

    match format {
        crate::cli::CcsOutputFormat::Text => print_text(&findings),
        crate::cli::CcsOutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&findings)?);
        }
    }

    if findings.iter().any(|finding| finding.severity == conary_core::ccs::v2::authoring::AuthoringFindingSeverity::Error) {
        anyhow::bail!("ccs lint found blocking errors");
    }
    Ok(())
}

fn manifest_path(path: &str) -> Result<std::path::PathBuf> {
    let path = Path::new(path);
    if path.is_file() {
        Ok(path.to_path_buf())
    } else if path.is_dir() {
        Ok(path.join("ccs.toml"))
    } else {
        anyhow::bail!("Cannot find ccs.toml at {}", path.display())
    }
}

fn print_text(findings: &[conary_core::ccs::v2::authoring::AuthoringFinding]) {
    if findings.is_empty() {
        println!("ccs lint passed");
        return;
    }
    for finding in findings {
        println!(
            "{} {:?}: {}",
            finding.code, finding.severity, finding.message
        );
        println!("  fix: {}", finding.suggestion);
    }
}
```

In `apps/conary/src/commands/ccs/mod.rs`, replace any temporary lint stub with:

```rust
mod lint;
pub use lint::cmd_ccs_lint;
```

- [ ] **Step 5: Run lint tests**

Run:

```bash
cargo test -p conary-core ccs::v2::authoring::tests::lint_manifest_reports_missing_release_and_kind
cargo test -p conary-core ccs::v2::authoring::tests::lint_manifest_marks_lifecycle_as_profile_deferred
cargo test -p conary-core ccs::v2::authoring::tests::lint_manifest_blocks_unresolved_dependencies_for_m4b
cargo test -p conary-core v2_debug_toml_with_service_hooks_is_rejected
cargo test -p conary --lib commands::ccs
```

Expected: PASS.

- [ ] **Step 6: Commit Task 4**

```bash
git add crates/conary-core/src/ccs/v2/authoring.rs crates/conary-core/src/ccs/v2/reader.rs apps/conary/src/commands/ccs/lint.rs apps/conary/src/commands/ccs/mod.rs
git commit -m "feat(ccs): add v2 authoring lint"
```

### Task 5: Build Signed V2 Packages With Local-Dev Trust

**Files:**
- Create: `apps/conary/src/commands/ccs/local_dev.rs`
- Modify: `apps/conary/src/commands/ccs/build.rs`
- Modify: `apps/conary/src/commands/ccs/inspect.rs`
- Modify: `apps/conary/src/commands/ccs/mod.rs`
- Modify: `apps/conary/src/dispatch/ccs.rs`
- Modify: `apps/conary/tests/packaging_m4b.rs`

- [ ] **Step 1: Add local-dev helper tests**

Create `apps/conary/src/commands/ccs/local_dev.rs` with tests:

```rust
// apps/conary/src/commands/ccs/local_dev.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_dev_policy_trusts_generated_public_key() {
        let temp = tempfile::tempdir().unwrap();
        let key = conary_core::ccs::signing::SigningKeyPair::generate()
            .with_key_id("local-dev");
        let policy_path = temp.path().join("policy.toml");

        write_local_dev_policy(&policy_path, &key).unwrap();
        let policy = conary_core::ccs::verify::TrustPolicy::from_file(&policy_path).unwrap();

        assert_eq!(policy.trusted_keys, vec![key.public_key_base64()]);
        assert!(!policy.allow_unsigned);
    }
}
```

- [ ] **Step 2: Implement local-dev key helpers**

Implement:

```rust
use anyhow::{Context, Result};
use conary_core::ccs::signing::SigningKeyPair;
use std::path::{Path, PathBuf};

pub struct LocalDevKeyPaths {
    pub private: PathBuf,
    pub public: PathBuf,
}

pub fn local_dev_key_paths() -> Result<LocalDevKeyPaths> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .context("HOME or XDG_DATA_HOME is required for local-dev CCS keys")?
        .join("conary")
        .join("ccs")
        .join("local-dev");
    Ok(LocalDevKeyPaths {
        private: base.join("local-dev-key.private.toml"),
        public: base.join("local-dev-key.public.toml"),
    })
}

pub fn load_or_create_local_dev_key() -> Result<SigningKeyPair> {
    let paths = local_dev_key_paths()?;
    if paths.private.exists() {
        return SigningKeyPair::load_from_file(&paths.private)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("load local-dev key {}", paths.private.display()));
    }
    let key = SigningKeyPair::generate().with_key_id("local-dev");
    key.save_to_files(&paths.private, &paths.public)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("write local-dev key {}", paths.private.display()))?;
    Ok(key)
}

pub fn write_local_dev_policy(path: &Path, key: &SigningKeyPair) -> Result<()> {
    std::fs::write(
        path,
        format!(
            "trusted_keys = [\"{}\"]\nallow_unsigned = false\nrequire_timestamp = false\n",
            key.public_key_base64()
        ),
    )
    .with_context(|| format!("write local-dev trust policy {}", path.display()))
}

pub fn local_dev_trust_policy() -> Result<Option<conary_core::ccs::verify::TrustPolicy>> {
    let paths = local_dev_key_paths()?;
    if !paths.public.exists() {
        return Ok(None);
    }
    #[derive(serde::Deserialize)]
    struct PublicKeyFile {
        key: String,
    }

    let public_text = std::fs::read_to_string(&paths.public)
        .with_context(|| format!("read local-dev public key {}", paths.public.display()))?;
    let public_key: PublicKeyFile = toml::from_str(&public_text)
        .with_context(|| format!("parse local-dev public key {}", paths.public.display()))?;
    Ok(Some(conary_core::ccs::verify::TrustPolicy::strict(vec![
        public_key.key,
    ])))
}
```

`ccs verify` should not load private key material just to construct local-dev
trust.

In `apps/conary/src/commands/ccs/mod.rs`, add:

```rust
mod local_dev;
```

- [ ] **Step 3: Add failing build tests**

In `apps/conary/tests/packaging_m4b.rs`, add a first integration test:

```rust
use std::process::{Command, Output};

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected command to succeed\n{}",
        output_text(output)
    );
}

fn assert_stdout_contains(output: &Output, needle: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(needle),
        "expected stdout to contain {needle:?}\n{}",
        output_text(output)
    );
}

fn assert_failure_contains(output: &Output, needles: &[&str]) {
    assert!(
        !output.status.success(),
        "expected command to fail\n{}",
        output_text(output)
    );
    let combined = output_text(output);
    for needle in needles {
        assert!(
            combined.contains(needle),
            "expected output to contain {needle:?}\n{combined}"
        );
    }
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn ccs_build_v2_requires_key_or_local_dev() {
    let fixture = MinimalPackageFixture::new();

    let output = fixture
        .conary()
        .arg("ccs")
        .arg("build")
        .arg(fixture.project_dir())
        .arg("--format")
        .arg("v2")
        .arg("--output")
        .arg(fixture.output_dir())
        .output()
        .expect("run conary ccs build");

    assert_failure_contains(&output, &["--local-dev", "--key"]);
}

#[test]
fn ccs_build_v2_accepts_explicit_release_key_and_policy_verify() {
    let fixture = MinimalPackageFixture::new();
    let key_base = fixture.work.path().join("release-key");
    let private_key = key_base.with_extension("private");
    let public_key = key_base.with_extension("public");
    let policy_path = fixture.work.path().join("release-policy.toml");

    let keygen = fixture
        .conary()
        .arg("ccs")
        .arg("keygen")
        .arg("--output")
        .arg(&key_base)
        .arg("--key-id")
        .arg("release")
        .output()
        .expect("run conary ccs keygen");
    assert_success(&keygen);
    write_trust_policy_from_public_key(&public_key, &policy_path);

    let build = fixture
        .conary()
        .arg("ccs")
        .arg("build")
        .arg(fixture.project_dir())
        .arg("--format")
        .arg("v2")
        .arg("--key")
        .arg(&private_key)
        .arg("--output")
        .arg(fixture.output_dir())
        .output()
        .expect("run conary ccs build");
    assert_success(&build);

    let package = fixture.output_dir().join("hello-0.1.0-1.ccs");
    let verify = fixture
        .conary()
        .arg("ccs")
        .arg("verify")
        .arg(&package)
        .arg("--policy")
        .arg(&policy_path)
        .output()
        .expect("run conary ccs verify");
    assert_success(&verify);
}
```

Add the fixture helper in the same file:

```rust
struct MinimalPackageFixture {
    work: tempfile::TempDir,
    project: std::path::PathBuf,
    output: std::path::PathBuf,
    home: std::path::PathBuf,
    xdg_data: std::path::PathBuf,
    xdg_config: std::path::PathBuf,
}

impl MinimalPackageFixture {
    fn new() -> Self {
        let work = tempfile::tempdir().unwrap();
        let project = work.path().join("project");
        let output = work.path().join("out");
        let home = work.path().join("home");
        let xdg_data = work.path().join("xdg-data");
        let xdg_config = work.path().join("xdg-config");
        std::fs::create_dir_all(project.join("bin")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&xdg_data).unwrap();
        std::fs::create_dir_all(&xdg_config).unwrap();
        std::fs::write(project.join("bin/hello"), "#!/bin/sh\necho hello\n").unwrap();
        let fixture = Self {
            work,
            project,
            output,
            home,
            xdg_data,
            xdg_config,
        };
        let init = fixture
            .conary()
            .arg("ccs")
            .arg("init")
            .arg(&fixture.project)
            .arg("--template")
            .arg("minimal-file")
            .arg("--name")
            .arg("hello")
            .arg("--version")
            .arg("0.1.0")
            .output()
            .expect("run conary ccs init");
        assert_success(&init);
        fixture
    }

    fn project_dir(&self) -> &std::path::Path {
        &self.project
    }

    fn output_dir(&self) -> &std::path::Path {
        &self.output
    }

    fn conary(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_conary"));
        command
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", &self.xdg_data)
            .env("XDG_CONFIG_HOME", &self.xdg_config);
        command
    }
}
```

All M4b integration tests must use `fixture.conary()` rather than raw
`Command::new(env!("CARGO_BIN_EXE_conary"))` so local-dev signing and verify
state never reads or writes the developer's real `HOME`/`XDG_DATA_HOME`.

Add the policy helper in the same file:

```rust
fn write_trust_policy_from_public_key(public_key_path: &std::path::Path, policy_path: &std::path::Path) {
    #[derive(serde::Deserialize)]
    struct PublicKeyFile {
        key: String,
    }

    let key_text = std::fs::read_to_string(public_key_path).unwrap();
    let key: PublicKeyFile = toml::from_str(&key_text).unwrap();
    std::fs::write(
        policy_path,
        format!(
            "trusted_keys = [\"{}\"]\nallow_unsigned = false\nrequire_timestamp = false\n",
            key.key
        ),
    )
    .unwrap();
}
```

- [ ] **Step 4: Route v2 build**

Update `cmd_ccs_build` signature to accept `format`, `key`, and `local_dev`. In `dispatch/ccs.rs`, pass these fields.

Inside `cmd_ccs_build`, enforce:

```rust
if local_dev && key.is_some() {
    anyhow::bail!("--local-dev and --key are mutually exclusive signing options");
}
if format == crate::cli::CcsBuildFormat::V1 && (key.is_some() || local_dev) {
    anyhow::bail!("--key and --local-dev are only supported when building with --format v2");
}
if format == crate::cli::CcsBuildFormat::V2 && target != "ccs" {
    anyhow::bail!("--format v2 only supports --target ccs in M4b");
}
if format == crate::cli::CcsBuildFormat::V2 && key.is_none() && !local_dev {
    anyhow::bail!("ccs build --format v2 requires --key <private-key> or --local-dev");
}
if format == crate::cli::CcsBuildFormat::V2 {
    let findings = conary_core::ccs::v2::authoring::lint_manifest_for_v2_authoring(&manifest);
    if findings.iter().any(|finding| finding.blocks_build) {
        for finding in &findings {
            if finding.blocks_build {
                eprintln!("{}: {}", finding.code, finding.message);
                eprintln!("  fix: {}", finding.suggestion);
            }
        }
        anyhow::bail!("ccs build --format v2 blocked by M4b authoring lint");
    }
}
```

After `CcsBuilder::build()`, route v2:

```rust
let output_path = if format == crate::cli::CcsBuildFormat::V2 {
    let release = manifest
        .package
        .release
        .as_deref()
        .context("v2 output naming requires package.release")?;
    output_dir.join(format!(
        "{}-{}-{}.ccs",
        manifest.package.name, manifest.package.version, release
    ))
} else {
    output_path
};
let debug_toml = manifest.to_toml().context("serialize debug ccs.toml")?;
let projected = conary_core::ccs::v2::project_build_result_to_v2(
    conary_core::ccs::v2::V2AuthoringInput {
        build: result,
        local_dev,
        debug_toml: Some(debug_toml),
    },
)
.context("project v2 package authority")?;
let signing_key = if local_dev {
    super::local_dev::load_or_create_local_dev_key()?
} else {
    let key_path = key.as_deref().context("missing --key for v2 release signing")?;
    conary_core::ccs::signing::SigningKeyPair::load_from_file(std::path::Path::new(key_path))
        .map_err(anyhow::Error::from)?
};
builder::write_v2_ccs_package(
    &projected.authority,
    &projected.payloads_by_path,
    &output_path,
    &signing_key,
    projected.debug_toml.as_deref(),
    None,
    None,
)
.context("Failed to write CCS v2 package")?;
if local_dev {
    println!("  Signed with local-dev CCS key; release publish will reject this artifact.");
}
```

Keep the function as a thin orchestrator. If the positional parameter list feels
fragile during implementation, introduce a small command-owned `CcsBuildOptions`
struct instead of adding more positional arguments.

Add a focused assertion that the legacy v1 path still writes `hello-0.1.0.ccs`,
while the v2 path writes `hello-0.1.0-1.ccs`.

- [ ] **Step 5: Let verify use local-dev trust when policy is omitted**

In `apps/conary/src/commands/ccs/inspect.rs`, update `cmd_ccs_verify` policy selection:

```rust
    let policy = if let Some(policy_file) = policy_path {
        TrustPolicy::from_file(Path::new(&policy_file)).context("Failed to load trust policy")?
    } else if allow_unsigned {
        TrustPolicy::permissive()
    } else if let Some(local_policy) = super::local_dev::local_dev_trust_policy()? {
        println!("Using local-dev CCS trust policy for verification.");
        local_policy
    } else {
        TrustPolicy::default()
    };
```

This local-dev fallback must not be used by static publish gate code.

- [ ] **Step 6: Run build and verify tests**

Run:

```bash
cargo test -p conary --test packaging_m4b ccs_build_v2_requires_key_or_local_dev
cargo test -p conary --test packaging_m4b ccs_build_v2_accepts_explicit_release_key_and_policy_verify
cargo test -p conary --lib local_dev_policy_trusts_generated_public_key
```

Expected: PASS.

- [ ] **Step 7: Commit Task 5**

```bash
git add apps/conary/src/commands/ccs/local_dev.rs apps/conary/src/commands/ccs/build.rs apps/conary/src/commands/ccs/inspect.rs apps/conary/src/commands/ccs/mod.rs apps/conary/src/dispatch/ccs.rs apps/conary/tests/packaging_m4b.rs
git commit -m "feat(ccs): build local-dev signed v2 packages"
```

### Task 6: Implement Isolated `ccs test --dry-run`

**Files:**
- Create: `apps/conary/src/commands/ccs/test.rs`
- Modify: `apps/conary/src/commands/ccs/mod.rs`
- Modify: `apps/conary/tests/packaging_m4b.rs`

- [ ] **Step 1: Add failing test command integration test**

In `apps/conary/tests/packaging_m4b.rs`, add:

```rust
#[test]
fn local_dev_v2_package_passes_verify_and_dry_run_test() {
    let fixture = MinimalPackageFixture::new();
    let package = fixture.build_v2_local_dev();

    let verify = fixture
        .conary()
        .arg("ccs")
        .arg("verify")
        .arg(&package)
        .output()
        .expect("run conary ccs verify");
    assert_success(&verify);
    assert_stdout_contains(&verify, "local-dev");

    let test = fixture
        .conary()
        .arg("ccs")
        .arg("test")
        .arg(&package)
        .arg("--dry-run")
        .output()
        .expect("run conary ccs test");
    assert_success(&test);
    assert_stdout_contains(&test, "dry-run");
    assert_stdout_contains(&test, "isolated");
}

#[test]
fn ccs_test_requires_dry_run_for_m4b() {
    let fixture = MinimalPackageFixture::new();
    let package = fixture.build_v2_local_dev();

    let test = fixture
        .conary()
        .arg("ccs")
        .arg("test")
        .arg(&package)
        .output()
        .expect("run conary ccs test");

    assert_failure_contains(&test, &["dry-run"]);
}
```

Add fixture method:

```rust
fn build_v2_local_dev(&self) -> std::path::PathBuf {
    let output = self
        .conary()
        .arg("ccs")
        .arg("build")
        .arg(&self.project)
        .arg("--format")
        .arg("v2")
        .arg("--local-dev")
        .arg("--output")
        .arg(&self.output)
        .output()
        .expect("run conary ccs build");
    assert_success(&output);
    self.output.join("hello-0.1.0-1.ccs")
}
```

- [ ] **Step 2: Implement test command**

Create `apps/conary/src/commands/ccs/test.rs`:

```rust
// apps/conary/src/commands/ccs/test.rs

use anyhow::{Context, Result};
use std::path::Path;

pub async fn cmd_ccs_test(
    package: &str,
    dry_run: bool,
    policy: Option<String>,
    keep_workspace: bool,
) -> Result<()> {
    if !dry_run {
        anyhow::bail!("M4b supports only conary ccs test --dry-run");
    }
    let package_path = Path::new(package);
    if !package_path.exists() {
        anyhow::bail!("Package not found: {package}");
    }

    let workspace = tempfile::tempdir().context("create isolated CCS test workspace")?;
    let root = workspace.path().join("root");
    let db_path = workspace.path().join("conary.db");
    let policy_path = workspace.path().join("trust-policy.toml");
    std::fs::create_dir_all(&root)?;
    conary_core::db::init(&db_path).context("initialize isolated test database")?;

    let policy = if let Some(policy) = policy {
        policy
    } else {
        let key = super::local_dev::load_or_create_local_dev_key()?;
        super::local_dev::write_local_dev_policy(&policy_path, &key)?;
        policy_path.to_string_lossy().into_owned()
    };

    println!("Testing CCS package in isolated dry-run workspace:");
    println!("  root: {}", root.display());
    println!("  db: {}", db_path.display());

    // SandboxMode::None is acceptable in M4b because minimal-file authoring
    // emits no script hooks and ccs test forces dry-run against an isolated
    // root/database. Future lifecycle/template slices must reevaluate this and
    // prefer SandboxMode::Always before any script execution is admitted.
    super::cmd_ccs_install_with_replay_options(
        package,
        &db_path.to_string_lossy(),
        &root.to_string_lossy(),
        true,
        false,
        Some(policy),
        None,
        crate::commands::SandboxMode::None,
        false,
        true,
        false,
        false,
        None,
        crate::commands::LegacyReplayOptions::default(),
    )
    .await?;

    if keep_workspace {
        let kept = workspace.keep();
        println!("Kept isolated CCS test workspace: {}", kept.display());
    }
    Ok(())
}
```

Do not make `ccs test` execute scripts in M4b. Add a test assertion that the
isolated dry-run root does not contain package side-effect files after the test
command returns; if the implementation exposes `--keep-workspace`, inspect that
kept root in the integration test.

In `apps/conary/src/commands/ccs/mod.rs`, add:

```rust
mod test;
pub use test::cmd_ccs_test;
```

Remove any temporary `cmd_ccs_test` stub from Task 2.

- [ ] **Step 3: Run test command proof**

Run:

```bash
cargo test -p conary --test packaging_m4b local_dev_v2_package_passes_verify_and_dry_run_test
cargo test -p conary --test packaging_m4b ccs_test_requires_dry_run_for_m4b
```

Expected: PASS.

- [ ] **Step 4: Commit Task 6**

```bash
git add apps/conary/src/commands/ccs/test.rs apps/conary/src/commands/ccs/mod.rs apps/conary/tests/packaging_m4b.rs
git commit -m "feat(ccs): dry-run test local v2 packages"
```

### Task 7: Complete M4b Integration And Publish-Gate Regressions

**Files:**
- Modify: `apps/conary/tests/packaging_m4b.rs`
- Modify: `crates/conary-core/src/repository/static_repo/publish_gate.rs`

- [ ] **Step 1: Add full smoke path test**

In `apps/conary/tests/packaging_m4b.rs`, add:

```rust
#[test]
fn m4b_minimal_file_smoke_path_creates_lints_builds_verifies_and_tests_v2_package() {
    let fixture = MinimalPackageFixture::new();

    let lint = fixture
        .conary()
        .arg("ccs")
        .arg("lint")
        .arg(fixture.project_dir())
        .output()
        .expect("run conary ccs lint");
    assert_success(&lint);

    let package = fixture.build_v2_local_dev();
    assert!(package.exists(), "expected v2 package {}", package.display());

    let verify = fixture
        .conary()
        .arg("ccs")
        .arg("verify")
        .arg(&package)
        .output()
        .expect("run conary ccs verify");
    assert_success(&verify);

    let test = fixture
        .conary()
        .arg("ccs")
        .arg("test")
        .arg(&package)
        .arg("--dry-run")
        .output()
        .expect("run conary ccs test");
    assert_success(&test);
}
```

- [ ] **Step 2: Add profile-deferred and dependency policy tests**

Add tests proving unsupported lifecycle fields block build and external dependencies remain checked:

```rust
#[test]
fn lifecycle_authoring_is_profile_deferred_and_blocks_v2_build() {
    let fixture = MinimalPackageFixture::new();
    let manifest_path = fixture.project_dir().join("ccs.toml");
    let mut text = std::fs::read_to_string(&manifest_path).unwrap();
    text.push_str(
        r#"

[[hooks.services]]
name = "hello.service"
action = "restart"
"#,
    );
    std::fs::write(&manifest_path, text).unwrap();

    let output = fixture
        .conary()
        .arg("ccs")
        .arg("build")
        .arg(fixture.project_dir())
        .arg("--format")
        .arg("v2")
        .arg("--local-dev")
        .arg("--output")
        .arg(fixture.output_dir())
        .output()
        .expect("run conary ccs build");

    assert_failure_contains(&output, &["profile", "M4d"]);
}

#[test]
fn dependency_authoring_is_profile_deferred_and_blocks_v2_build() {
    let fixture = MinimalPackageFixture::new();
    let manifest_path = fixture.project_dir().join("ccs.toml");
    let mut text = std::fs::read_to_string(&manifest_path).unwrap();
    text.push_str(
        r#"

[requires]
packages = [{ name = "openssl", version = ">=3.0" }]
"#,
    );
    std::fs::write(&manifest_path, text).unwrap();

    let output = fixture
        .conary()
        .arg("ccs")
        .arg("build")
        .arg(fixture.project_dir())
        .arg("--format")
        .arg("v2")
        .arg("--local-dev")
        .arg("--output")
        .arg(fixture.output_dir())
        .output()
        .expect("run conary ccs build");

    assert_failure_contains(&output, &["dependencies", "M4b"]);
}
```

If current manifest hook TOML uses a different shape, use the existing shape accepted by `CcsManifest` and keep the assertion text focused on profile-deferred output.

- [ ] **Step 3: Add publish-gate local-dev rejection regression**

Before editing `crates/conary-core/src/repository/static_repo/publish_gate.rs`,
name the ownership boundary being preserved: this file owns static publish
eligibility checks only; M4b must not move local-dev trust into release publish
trust.

In `crates/conary-core/src/repository/static_repo/publish_gate.rs`, add a test
that mutates a v2 fixture to `hardening_level = "host"` and has no build
attestation:

```rust
#[test]
fn artifact_gate_rejects_local_dev_v2_package() {
    let signer = crate::ccs::signing::SigningKeyPair::generate().with_key_id("local-dev");
    let temp = tempfile::tempdir().unwrap();
    let package_path = temp.path().join("local-dev-v2.ccs");
    let mut authority = crate::ccs::v2::test_support::package_authority_with_one_file("local-dev");
    authority.provenance.hardening_level = Some("host".to_string());
    let payloads = crate::ccs::v2::test_support::one_file_payloads_for_tests();
    crate::ccs::builder::write_v2_ccs_package(
        &authority,
        &payloads,
        &package_path,
        &signer,
        None,
        None,
        None,
    )
    .unwrap();

    let report = verify_static_artifact_publish_eligibility(
        &package_path,
        &AcceptedStaticSignerSet::from_initial_key("local-dev", signer.public_key_base64()),
        "m2-static-publish-policy-v1",
    )
    .unwrap();

    assert!(!report.is_passed());
    assert!(report.failures.iter().any(|failure| {
        matches!(failure.code, PublishGateFailureCode::MissingAttestation)
    }));
}
```

This test intentionally asserts the current early-return behavior: a package
without a build attestation is rejected before `NonHermeticHardeningLevel` can
be evaluated. The authoring projection unit tests prove M4b local-dev and
explicit-key output both carry `hardening_level = "host"`.

- [ ] **Step 4: Run M4b and publish-gate tests**

Run:

```bash
cargo test -p conary --test packaging_m4b
cargo test -p conary-core repository::static_repo::publish_gate::tests::artifact_gate_rejects_local_dev_v2_package
```

Expected: PASS.

- [ ] **Step 5: Commit Task 7**

```bash
git add apps/conary/tests/packaging_m4b.rs crates/conary-core/src/repository/static_repo/publish_gate.rs
git commit -m "test(ccs): prove m4b local authoring loop"
```

### Task 8: Documentation, Audit Metadata, And Final Verification

**Files:**
- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/test-fixtures.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Update CCS docs with the first M4b loop**

In `docs/modules/ccs.md`, add a concise maintainer loop section:

````markdown
### Native CCS v2 Local Authoring Loop

The first supported native authoring loop is:

```text
conary ccs init --template minimal-file
conary ccs lint
conary ccs build --format v2 --local-dev
conary ccs verify
conary ccs test --dry-run
```

`--local-dev` signs with a user-local development key for iteration. Local-dev
artifacts can verify and dry-run-test locally, but static publish and Remi
release paths still require accepted release trust and build attestation.
````

Adjust heading level to match the surrounding document.

- [ ] **Step 2: Update fixture and ownership docs**

In `docs/modules/test-fixtures.md`, record `apps/conary/tests/packaging_m4b.rs` as the M4b smoke proof.

In `docs/llms/subsystem-map.md` and `docs/modules/feature-ownership.md`, route native CCS v2 authoring work to:

```text
apps/conary/src/commands/ccs/{templates.rs,lint.rs,build.rs,test.rs,local_dev.rs}
crates/conary-core/src/ccs/v2/authoring.rs
```

- [ ] **Step 3: Update docs-audit metadata**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh > /tmp/conary-docs-inventory.tsv
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv /tmp/conary-docs-inventory.tsv
```

If the diff only contains expected new/changed tracked docs, replace the inventory file with the generated output.

Add or update ledger rows for the M4b plan and touched docs. The M4b plan row should use:

```text
docs/superpowers/plans/2026-06-18-m4b-native-authoring-build-lint-test-implementation-plan.md	docs/superpowers/plans/2026-06-18-m4b-native-authoring-build-lint-test-implementation-plan.md	planning	maintainer	ccs-native; m4b; authoring-workflow; implementation-plan; ccs-v2; lint; local-test	docs/superpowers/specs/2026-06-18-m4b-native-authoring-build-lint-test-design.md; docs/superpowers/specs/2026-06-17-m4-ccs-native-ecosystem-design.md; apps/conary/src/cli/ccs.rs; apps/conary/src/commands/ccs/init.rs; apps/conary/src/commands/ccs/build.rs; apps/conary/src/commands/ccs/install/command.rs; crates/conary-core/src/ccs/v2/authoring.rs; crates/conary-core/src/ccs/builder/package_writer.rs	verified	corrected	Locked the M4b implementation plan after DeepSeek, Gemini, and local agentic review; covers minimal-file template generation, v2 authoring projection, component-default selection, lint diagnostics, service-hook and dependency blocking, local-dev and explicit-key signing, host hardening for all M4b builds, v2 release filenames, signing flag guardrails, local-dev verification, isolated local-dev test state, isolated dry-run package test, command-risk classification, M4b integration proof, external and local review patch points, and docs/coherency gates.
```

- [ ] **Step 4: Run final verification**

Run:

```bash
cargo fmt --check
cargo test -p conary --test packaging_m4a
cargo test -p conary --test packaging_m4b
cargo test -p conary-core ccs::v2
cargo test -p conary --lib commands::ccs
cargo test -p conary --test packaging_m2a
cargo test -p conary --lib commands::publish
cargo test -p conary-core repository::static_repo::publish_gate
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-doc-truth.sh
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

Expected: all commands PASS.

- [ ] **Step 5: Commit Task 8**

```bash
git add docs/modules/ccs.md docs/modules/test-fixtures.md docs/llms/subsystem-map.md docs/modules/feature-ownership.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs(ccs): document m4b local authoring loop"
```

### Task 9: Local Agentic Review And Closeout

**Files:**
- Review only: full implementation diff against this plan and the M4b design.

- [ ] **Step 1: Run local agentic review**

Dispatch a local reviewer with this prompt:

```text
Review the M4b implementation against:
- docs/superpowers/specs/2026-06-18-m4b-native-authoring-build-lint-test-design.md
- docs/superpowers/plans/2026-06-18-m4b-native-authoring-build-lint-test-implementation-plan.md

Focus on: v2 authority projection from BuildResult, local-dev signing trust separation, ccs verify local-dev fallback, ccs test isolated root/database behavior, command-risk classification, static publish rejection of local-dev output, and docs-audit/coherency updates.

Return blocking findings first. If no blockers, say READY.
```

- [ ] **Step 2: Patch any accepted review findings**

For each accepted finding, add the failing test first, patch the implementation, and rerun the narrow test that proves the finding is fixed.

- [ ] **Step 3: Rerun final verification**

Run the full command batch from Task 8 Step 4 again.

Expected: all commands PASS.

- [ ] **Step 4: Commit review fixes if needed**

If review fixes changed files:

```bash
git add .
git commit -m "fix(ccs): address m4b review findings"
```

- [ ] **Step 5: Report implementation completion**

Report:

- final commit range;
- local review verdict;
- verification commands and results;
- any intentionally deferred work, especially config-noreplace, profile-backed lifecycle templates, Remi publication, and target-profile facts.
