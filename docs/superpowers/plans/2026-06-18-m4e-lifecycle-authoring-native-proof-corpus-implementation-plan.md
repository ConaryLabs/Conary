# M4e Lifecycle Authoring And Native Proof Corpus Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Implemented in the M4e lifecycle authoring proof changeset after a
2026-07-03 current-checkout rebaseline. The original plan was locked after
DeepSeek, Gemini, and local agentic review; Task 0 is retained below as the
historical pre-implementation baseline that this changeset closes.

**Goal:** Implement lifecycle-aware native CCS v2 authoring, config/noreplace authority, target-profile validation, reader/debug-projection consistency, Remi lifecycle publication proof, and the M4 closeout corpus.

**Architecture:** Keep `apps/conary/src/commands/ccs/` responsible for CLI/template ergonomics and target-profile argument plumbing. Keep signed authority projection and structural/profile validation under `crates/conary-core/src/ccs/v2/`, with a small debug-projection consistency helper so v2 archive reads verify debug TOML without treating it as install truth. Keep supported target facts in `crates/conary-core/src/repository/supported_profiles/`, and make Remi release upload resolve route slugs to supported profiles before lifecycle validation.

**Tech Stack:** Rust 2024, clap, serde/toml, existing CCS v2 CBOR/signature reader, existing CCS builder/package writer, existing supported-profile catalog, Axum/Tokio Remi release upload, Cargo integration tests, docs-audit/coherency scripts.

---

## 2026-07-03 Pre-Implementation Refresh Baseline

This historical rebaseline captured the main-branch state before M4e was
implemented. A repo rebaseline on 2026-07-03 found:

- `apps/conary/tests/packaging_m4e.rs` does not exist, and
  `cargo test -p conary --test packaging_m4e` fails with no matching test
  target.
- `apps/conary/src/commands/ccs/init_template.rs` still has only
  `CcsInitTemplate::MinimalFile`.
- `apps/conary/src/cli/ccs.rs` still lacks `--target-profile` on `Build`,
  `Lint`, and `Test`.
- `crates/conary-core/src/ccs/v2/authoring.rs` still uses
  `AuthoringFindingBucket::ProfileDeferred` and blocks lifecycle declarations
  before v2 build.
- `crates/conary-core/src/repository/supported_profiles/catalog.toml` still
  contains M4d placeholder lifecycle entries (`example.service`,
  `example.conf`, `kernel.example`) and leaves users, groups, directories, and
  alternatives unsupported.
- `crates/conary-core/src/ccs/v2/debug_projection.rs` does not exist. Current
  debug TOML hash and drift checks live in the v2 reader tests.
- `apps/remi/src/server/native_publish/verify.rs` verifies the static publish
  gate but does not receive the release route/profile for lifecycle validation.

The prerequisite baselines passed on the same checkout:

```bash
cargo test -p conary --test packaging_m4a
cargo test -p conary --test packaging_m4b
cargo test -p conary --test packaging_m4c
cargo test -p conary --test packaging_m4d
cargo test -p conary-core ccs::v2
cargo test -p conary-core supported_profiles
cargo test -p remi release_upload_
```

M4e completion requires a real `packaging_m4e` corpus and the final gates listed
in Task 7.

## 2026-07-03 Implementation Proof

This changeset implements the planned `config-noreplace` and `service`
templates, target-profile-aware authoring diagnostics and CLI plumbing,
config/lifecycle v2 authority projection, structural reader validation, debug
TOML projection consistency, supported-profile proof allow-lists, Remi
route-derived lifecycle validation, M4e positive/negative proof corpus, and
Task 7 docs/coherency updates.

Focused proof already run during implementation:

```bash
cargo test -p conary-core ccs::v2
cargo test -p conary-core supported_profiles
cargo test -p conary commands::ccs::templates
cargo test -p conary commands::ccs::init
cargo test -p conary --test packaging_m4e
cargo test -p conary --test packaging_m4a --test packaging_m4b --test packaging_m4c --test packaging_m4d
cargo test -p remi native_publish
cargo test -p remi release_upload_
```

## Design Inputs

Read these before executing:

- `AGENTS.md`
- `docs/llms/README.md`
- `docs/superpowers/specs/2026-06-18-m4e-lifecycle-authoring-native-proof-corpus-design.md`
- `docs/superpowers/specs/2026-06-17-m4-ccs-native-ecosystem-design.md`
- `docs/superpowers/specs/2026-06-17-m4a-ccs-v2-native-package-contract-design.md`
- `docs/superpowers/specs/2026-06-18-m4b-native-authoring-build-lint-test-design.md`
- `docs/superpowers/specs/2026-06-18-m4c-remi-native-ccs-publication-design.md`
- `docs/superpowers/specs/2026-06-18-m4d-supported-distro-adapter-profiles-design.md`
- `docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md`
- `docs/modules/ccs.md`
- `docs/modules/remi.md`
- `docs/modules/test-fixtures.md`
- `docs/modules/feature-ownership.md`
- `docs/llms/subsystem-map.md`
- `apps/conary/src/cli/ccs.rs`
- `apps/conary/src/commands/ccs/`
- `crates/conary-core/src/ccs/manifest.rs`
- `crates/conary-core/src/ccs/builder.rs`
- `crates/conary-core/src/ccs/builder/test_support.rs`
- `crates/conary-core/src/ccs/v2/`
- `crates/conary-core/src/repository/supported_profiles/`
- `apps/remi/src/server/native_publish/verify.rs`
- `apps/remi/src/server/release_publish.rs`

## Scope Locks

M4e includes:

- `config-noreplace` and `service` `ccs init` templates.
- Buildable generated files for templates that need payload proof.
- Config/noreplace projection into both `FileAuthorityV2.config` and `PackageDataV2.config`.
- Exact manifest-hook projection into `LifecycleAuthorityV2` string vectors.
- `--target-profile <public-id>` support for lifecycle-bearing lint, build, and dry-run test flows.
- Config-only authoring without `--target-profile`.
- Replacement of `ProfileDeferred` lifecycle behavior with profile-aware diagnostics.
- Structural v2 authority validation that does not use no-profile lifecycle facts.
- Debug TOML hash verification plus signed-projection consistency checks.
- Supported-profile lifecycle allow-list updates for `conary-example` proof entries.
- Remi release upload lifecycle validation through route-derived supported profile facts.
- Focused positive and negative M4e proof corpus.
- Docs/coherency/audit updates and M4 closeout backlog preservation.

M4e excludes:

- Live service activation, user/group creation, tmpfiles application, sysctl writes, alternatives registration, or other live host mutation.
- Arbitrary native v2 script lifecycle authoring.
- New public target profiles beyond `fedora-44`, `ubuntu-26.04`, and `arch`.
- Runtime-loaded profile catalogs.
- Full dependency authoring and resolver integration for v2 dependencies.
- Production key-management UX.
- Broad lifecycle allow-list patterns.
- Profile-specific config policy semantics.

## File Map

Create:

- `crates/conary-core/src/ccs/v2/debug_projection.rs` - debug TOML to signed authority consistency checks.
- `apps/conary/src/commands/ccs/target_profile.rs` - CLI target-profile resolution and error wording.
- `apps/conary/tests/packaging_m4e.rs` - end-to-end M4e CLI proof corpus.

Modify:

- `apps/conary/src/cli/ccs.rs` - add `--target-profile` to `Build`, `Lint`, and `Test`.
- `apps/conary/src/dispatch/ccs.rs` - bind and forward `--target-profile` through the Build, Lint, and Test dispatch arms.
- `apps/conary/src/commands/ccs/mod.rs` - expose new target-profile helper module.
- `apps/conary/src/commands/ccs/init_template.rs` - add `ConfigNoreplace` and `Service` variants.
- `apps/conary/src/commands/ccs/templates.rs` - generate M4e template manifests.
- `apps/conary/src/commands/ccs/init.rs` - write generated template files for buildable examples.
- `apps/conary/src/commands/ccs/lint.rs` - pass optional target profile into authoring lint.
- `apps/conary/src/commands/ccs/build.rs` - pass optional target profile into lint/projection and keep config-only builds ungated.
- `apps/conary/src/commands/ccs/test.rs` - accept optional target profile and validate lifecycle authority without live lifecycle execution.
- `crates/conary-core/src/ccs/builder/test_support.rs` - add fixture helpers for arbitrary file paths/config/lifecycle proof.
- `crates/conary-core/src/ccs/v2/mod.rs` - export debug-projection and structural validation helpers as needed.
- `crates/conary-core/src/ccs/v2/authoring.rs` - add config projection, lifecycle projection, profile-aware diagnostics, and target-profile-aware input.
- `crates/conary-core/src/ccs/v2/reader.rs` - call structural validation and debug-projection consistency instead of no-profile lifecycle rejection.
- `crates/conary-core/src/ccs/v2/validation.rs` - expose structural validation separately from profile validation.
- `crates/conary-core/src/repository/supported_profiles/catalog.toml` - replace placeholders with M4e proof entries.
- `crates/conary-core/src/repository/supported_profiles/tests.rs` - prove exact M4e lifecycle policy.
- `apps/remi/src/server/native_publish/types.rs` - add lifecycle-specific native publication error code.
- `apps/remi/src/server/native_publish/verify.rs` - validate lifecycle authority against route-derived profile.
- `apps/remi/src/server/release_publish.rs` - pass route slug into native verification.
- `docs/modules/ccs.md`, `docs/modules/remi.md`, `docs/modules/test-fixtures.md`, `docs/modules/feature-ownership.md`, and `docs/llms/subsystem-map.md` - update public usage and routing docs after behavior lands.
- `docs/superpowers/feature-coherency-ledger.tsv` and `docs/superpowers/feature-coherency-wave-scopes.tsv` - update rows for changed public authoring/Remi claims.
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv` and `docs/superpowers/documentation-accuracy-audit-ledger.tsv` - update plan/docs audit coverage.

Maintainability boundaries:

- `crates/conary-core/src/recipe/kitchen/cook.rs` is out of scope and should remain untouched.
- `crates/conary-core/src/ccs/manifest.rs` remains the TOML model/parser. Do not add projection or validation helper logic there.
- `crates/conary-core/src/ccs/manifest.rs` is already a large TOML model/parser file. M4e adds no new manifest fields; if implementation discovers a missing manifest field, stop and treat that as a blocking plan/design issue before editing `manifest.rs`.
- `ccs/v2/authoring.rs` owns manifest/build-result to signed authority projection and authoring diagnostics.
- If `ccs/v2/authoring.rs` grows past 800 lines after Tasks 1-3, split projection helpers into `ccs/v2/authoring/projection.rs` and diagnostics helpers into `ccs/v2/authoring/diagnostics.rs`, keeping `authoring.rs` as the public hub. Update `docs/llms/subsystem-map.md` if the look-here-first path changes.
- `ccs/v2/debug_projection.rs` owns debug TOML to signed authority consistency. It must not make debug TOML authoritative.
- `ccs/v2/reader.rs` owns archive decode/signature/hash orchestration, not lifecycle policy.
- `repository/supported_profiles/` owns target facts and lifecycle allow-list policy.
- `apps/remi/src/server/native_publish/verify.rs` owns release artifact verification, including route-derived profile validation; storage/persistence remains outside this change.

## Checkpoints

- Checkpoint 0 after Task 0: stale status is recorded and current baselines
  prove M4a-M4d remain green while M4e is still absent.
- Checkpoint 1 after Task 1: template and config projection tests pass.
- Checkpoint 2 after Task 2: target-profile CLI/lint/build tests pass.
- Checkpoint 3 after Task 3: lifecycle projection and supported-profile tests pass.
- Checkpoint 4 after Task 4: v2 reader/debug-projection tests pass.
- Checkpoint 5 after Task 5: Remi route-derived lifecycle validation tests pass.
- Checkpoint 6 after Task 6: M4e positive/negative CLI corpus passes.
- Checkpoint 7 after Task 7: docs, M2/M4 regression, clippy, fmt, and audit gates pass.

## Review Lock Mapping

| Design concern | Plan owner |
| --- | --- |
| Config/noreplace template and authority | Task 1 |
| Generated buildable template payloads | Task 1 |
| Config-only flow does not require target profile | Task 1 and Task 2 |
| Lifecycle target-profile CLI plumbing | Task 2 |
| `ProfileDeferred` replacement | Task 2 |
| Exact lifecycle projection strings | Task 3 |
| Placeholder profile entries replaced | Task 3 |
| Structural reader validation split | Task 4 |
| Debug TOML signed-projection consistency | Task 4 |
| Remi route-derived lifecycle validation | Task 5 |
| Positive minimal/config/service corpus | Task 6 |
| Negative lifecycle/target/trust corpus | Task 6 |
| M4 exit and post-M4 backlog docs | Task 7 |

---

### Task 0: Rebaseline The Current Checkout Before Implementation

**Files:**
- Read: `docs/superpowers/specs/2026-06-18-m4e-lifecycle-authoring-native-proof-corpus-design.md`
- Read: `docs/superpowers/plans/2026-06-18-m4e-lifecycle-authoring-native-proof-corpus-implementation-plan.md`
- Read: `apps/conary/src/commands/ccs/init_template.rs`
- Read: `apps/conary/src/cli/ccs.rs`
- Read: `crates/conary-core/src/ccs/v2/authoring.rs`
- Read: `crates/conary-core/src/repository/supported_profiles/catalog.toml`
- Read: `apps/remi/src/server/native_publish/verify.rs`
- Test: `cargo test -p conary --test packaging_m4a`
- Test: `cargo test -p conary --test packaging_m4b`
- Test: `cargo test -p conary --test packaging_m4c`
- Test: `cargo test -p conary --test packaging_m4d`
- Test: `cargo test -p conary-core ccs::v2`
- Test: `cargo test -p conary-core supported_profiles`
- Test: `cargo test -p remi release_upload_`

- [ ] **Step 1: Confirm M4e is absent before starting implementation**

Run:

```bash
test ! -f apps/conary/tests/packaging_m4e.rs
cargo test -p conary --test packaging_m4e
```

Expected: the first command exits successfully; the second command fails with
`no test target named packaging_m4e`.

- [ ] **Step 2: Confirm the current implementation still matches the
pre-M4e baseline**

Run:

```bash
rg -n "CcsInitTemplate::MinimalFile|enum CcsInitTemplate|target_profile|target-profile|ProfileDeferred|example.service|debug_projection|verify_native_artifact" \
  apps/conary/src/commands/ccs/init_template.rs \
  apps/conary/src/cli/ccs.rs \
  crates/conary-core/src/ccs/v2/authoring.rs \
  crates/conary-core/src/repository/supported_profiles/catalog.toml \
  apps/remi/src/server/native_publish/verify.rs \
  crates/conary-core/src/ccs/v2
```

Expected: the output shows `MinimalFile`, `ProfileDeferred`, M4d placeholder
catalog entries, and no real `target-profile` CLI plumbing or
`debug_projection.rs` module.

- [ ] **Step 3: Re-run prerequisite gates**

Run:

```bash
cargo test -p conary --test packaging_m4a
cargo test -p conary --test packaging_m4b
cargo test -p conary --test packaging_m4c
cargo test -p conary --test packaging_m4d
cargo test -p conary-core ccs::v2
cargo test -p conary-core supported_profiles
cargo test -p remi release_upload_
```

Expected: all commands pass. If one fails, stop and fix the prerequisite slice
before starting M4e.

- [ ] **Step 4: Commit any docs-only rebaseline changes**

Run:

```bash
git add docs/superpowers/specs/2026-06-18-m4e-lifecycle-authoring-native-proof-corpus-design.md \
  docs/superpowers/plans/2026-06-18-m4e-lifecycle-authoring-native-proof-corpus-implementation-plan.md \
  docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs(ccs): rebaseline m4e lifecycle plan"
```

Expected: one docs-only commit if the rebaseline text or ledger changed. If no
docs changed because this task was already completed, skip the commit.

### Task 1: Add Config/Noreplace Template And Config Authority Projection

**Files:**
- Modify: `apps/conary/src/commands/ccs/init_template.rs`
- Modify: `apps/conary/src/commands/ccs/templates.rs`
- Modify: `apps/conary/src/commands/ccs/init.rs`
- Modify: `crates/conary-core/src/ccs/builder/test_support.rs`
- Modify: `crates/conary-core/src/ccs/v2/authoring.rs`
- Test: `cargo test -p conary commands::ccs::templates`
- Test: `cargo test -p conary commands::ccs::init`
- Test: `cargo test -p conary-core ccs::v2::authoring`

- [ ] **Step 1: Write failing template unit tests**

Add tests to `apps/conary/src/commands/ccs/templates.rs`:

```rust
#[test]
fn config_noreplace_template_declares_config_authority() {
    let manifest = build_manifest(Some(CcsInitTemplate::ConfigNoreplace), "demo", "0.1.0")
        .expect("template manifest");

    assert_eq!(manifest.package.release.as_deref(), Some("1"));
    assert_eq!(manifest.package.kind, Some(PackageKindTagV2::Package));
    assert_eq!(
        manifest.config.files,
        vec!["/etc/conary-example/config.toml".to_string()]
    );
    assert!(manifest.config.noreplace);
    assert!(manifest.hooks.services.is_empty());
    assert!(manifest.hooks.systemd.is_empty());
}

#[test]
fn service_template_declares_lifecycle_without_scripts() {
    let manifest = build_manifest(Some(CcsInitTemplate::Service), "demo", "0.1.0")
        .expect("template manifest");

    assert_eq!(manifest.package.release.as_deref(), Some("1"));
    assert_eq!(manifest.package.kind, Some(PackageKindTagV2::Package));
    assert!(manifest.hooks.scriptlets.post_install.is_none());
    assert!(manifest.hooks.services.iter().any(|service| service.name == "conary-example.service"));
    assert!(manifest.hooks.users.iter().any(|user| user.name == "conary-example"));
    assert!(manifest.hooks.groups.iter().any(|group| group.name == "conary-example"));
    assert!(manifest.hooks.directories.iter().any(|dir| dir.path == "/var/lib/conary-example"));
    assert!(manifest.hooks.tmpfiles.iter().any(|entry| entry.path == "/var/lib/conary-example"));
}
```

- [ ] **Step 2: Write failing generated-file unit test**

Add a test to `apps/conary/src/commands/ccs/init.rs`:

```rust
#[tokio::test]
async fn config_template_writes_buildable_config_file() {
    let temp = tempfile::tempdir().unwrap();

    cmd_ccs_init(
        temp.path().to_str().unwrap(),
        Some("demo".to_string()),
        "0.1.0",
        false,
        Some(super::CcsInitTemplate::ConfigNoreplace),
    )
    .await
    .unwrap();

    assert!(temp.path().join("ccs.toml").exists());
    assert!(temp.path().join("etc/conary-example/config.toml").exists());
}

#[tokio::test]
async fn service_template_writes_buildable_service_files() {
    let temp = tempfile::tempdir().unwrap();

    cmd_ccs_init(
        temp.path().to_str().unwrap(),
        Some("demo".to_string()),
        "0.1.0",
        false,
        Some(super::CcsInitTemplate::Service),
    )
    .await
    .unwrap();

    assert!(temp.path().join("usr/bin/conary-example").exists());
    assert!(temp.path().join("usr/lib/systemd/system/conary-example.service").exists());
}
```

- [ ] **Step 3: Run template tests to verify they fail**

Run:

```bash
cargo test -p conary commands::ccs::templates
cargo test -p conary commands::ccs::init
```

Expected: FAIL because `CcsInitTemplate::ConfigNoreplace`, `CcsInitTemplate::Service`, and template file writing do not exist yet.

- [ ] **Step 4: Implement template variants and file generation**

Change `apps/conary/src/commands/ccs/init_template.rs`:

```rust
#[derive(Clone, Copy, Debug, clap::ValueEnum, PartialEq, Eq)]
pub enum CcsInitTemplate {
    MinimalFile,
    ConfigNoreplace,
    Service,
}
```

Add a generated-file helper in `apps/conary/src/commands/ccs/templates.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateFile {
    pub path: &'static str,
    pub contents: &'static str,
    pub executable: bool,
}

pub fn template_files(template: Option<CcsInitTemplate>) -> &'static [TemplateFile] {
    match template {
        Some(CcsInitTemplate::ConfigNoreplace) => &[TemplateFile {
            path: "etc/conary-example/config.toml",
            contents: "message = \"hello from conary\"\n",
            executable: false,
        }],
        Some(CcsInitTemplate::Service) => &[
            TemplateFile {
                path: "usr/bin/conary-example",
                contents: "#!/bin/sh\necho conary-example\n",
                executable: true,
            },
            TemplateFile {
                path: "usr/lib/systemd/system/conary-example.service",
                contents: "[Unit]\nDescription=Conary example service\n\n[Service]\nType=oneshot\nExecStart=/usr/bin/conary-example\n\n[Install]\nWantedBy=multi-user.target\n",
                executable: false,
            },
        ],
        _ => &[],
    }
}
```

In `cmd_ccs_init`, after writing `ccs.toml`, write `template_files(template)` into `dir`, creating parent directories and setting executable mode on Unix:

```rust
fn write_template_files(dir: &Path, template: Option<super::CcsInitTemplate>) -> Result<()> {
    for file in super::templates::template_files(template) {
        let path = dir.join(file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        std::fs::write(&path, file.contents)
            .with_context(|| format!("Failed to write {}", path.display()))?;
        #[cfg(unix)]
        if file.executable {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&path)?.permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions)?;
        }
    }
    Ok(())
}
```

- [ ] **Step 5: Write failing config projection tests**

Add a build-result helper to `crates/conary-core/src/ccs/builder/test_support.rs`:

```rust
pub(crate) fn single_file_build_result_at(
    name: &str,
    version: &str,
    path: &str,
    bytes: &[u8],
) -> BuildResult {
    let mut build = minimal_file_build_result(name, version, bytes);
    build.files[0].path = path.to_string();
    build.components.get_mut("runtime").unwrap().files[0].path = path.to_string();
    build
}
```

Add tests to `crates/conary-core/src/ccs/v2/authoring.rs`:

```rust
#[test]
fn projection_marks_noreplace_config_in_file_and_package_authority() {
    let mut build = test_support::single_file_build_result_at(
        "demo",
        "0.1.0",
        "/etc/conary-example/config.toml",
        b"message = \"hello\"\n",
    );
    build.manifest.package.release = Some("1".to_string());
    build.manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
    build.manifest.config.files = vec!["/etc/conary-example/config.toml".to_string()];
    build.manifest.config.noreplace = true;

    let projected = project_build_result_to_v2(V2AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: Some(build.manifest.to_toml().unwrap()),
    })
    .unwrap();

    let package = match &projected.authority.kind {
        PackageKindV2::Package(package) => package,
        other => panic!("expected package authority, got {other:?}"),
    };
    assert_eq!(package.files[0].config, Some(ConfigPolicyV2::NoReplace));
    assert_eq!(package.config[0].path, "/etc/conary-example/config.toml");
    assert_eq!(package.config[0].policy, ConfigPolicyV2::NoReplace);
}

#[test]
fn projection_marks_replace_config_when_noreplace_is_false() {
    let mut build = test_support::single_file_build_result_at(
        "demo",
        "0.1.0",
        "/etc/conary-example/config.toml",
        b"message = \"hello\"\n",
    );
    build.manifest.package.release = Some("1".to_string());
    build.manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
    build.manifest.config.files = vec!["/etc/conary-example/config.toml".to_string()];
    build.manifest.config.noreplace = false;

    let projected = project_build_result_to_v2(V2AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: Some(build.manifest.to_toml().unwrap()),
    })
    .unwrap();

    let package = match &projected.authority.kind {
        PackageKindV2::Package(package) => package,
        other => panic!("expected package authority, got {other:?}"),
    };
    assert_eq!(package.files[0].config, Some(ConfigPolicyV2::Replace));
    assert_eq!(package.config[0].policy, ConfigPolicyV2::Replace);
}

#[test]
fn projection_rejects_config_path_absent_from_payload() {
    let mut build = test_support::minimal_file_build_result("demo", "0.1.0", b"hello\n");
    build.manifest.package.release = Some("1".to_string());
    build.manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
    build.manifest.config.files = vec!["/etc/conary-example/config.toml".to_string()];

    let error = project_build_result_to_v2(V2AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: Some(build.manifest.to_toml().unwrap()),
    })
    .unwrap_err();

    assert!(error.to_string().contains("config path"));
}

#[test]
fn projection_rejects_relative_config_paths() {
    let mut build = test_support::single_file_build_result_at(
        "demo",
        "0.1.0",
        "etc/conary-example/config.toml",
        b"message = \"hello\"\n",
    );
    build.manifest.package.release = Some("1".to_string());
    build.manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
    build.manifest.config.files = vec!["etc/conary-example/config.toml".to_string()];

    let error = project_build_result_to_v2(V2AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: Some(build.manifest.to_toml().unwrap()),
    })
    .unwrap_err();

    assert!(error.to_string().contains("config path"));
    assert!(error.to_string().contains("absolute"));
}
```

- [ ] **Step 6: Run config projection tests to verify they fail**

Run:

```bash
cargo test -p conary-core ccs::v2::authoring
```

Expected: FAIL because config projection does not exist yet.

- [ ] **Step 7: Implement config projection**

In `crates/conary-core/src/ccs/v2/authoring.rs`, add a helper that validates config paths are absolute and present in `BuildResult.files`, then sets file-level and package-level policy:

```rust
fn config_policy_for_manifest(manifest: &crate::ccs::manifest::CcsManifest) -> ConfigPolicyV2 {
    if manifest.config.noreplace {
        ConfigPolicyV2::NoReplace
    } else {
        ConfigPolicyV2::Replace
    }
}
```

When mapping files, use a `BTreeMap<String, ConfigPolicyV2>` so every matching `FileAuthorityV2.config` is set. After file mapping, build `PackageDataV2.config` from the same map. Missing or relative config paths should return `anyhow::bail!("config path {path} must be absolute and present in build output")`.

- [ ] **Step 8: Run Task 1 tests to verify they pass**

Run:

```bash
cargo test -p conary commands::ccs::templates
cargo test -p conary commands::ccs::init
cargo test -p conary-core ccs::v2::authoring
```

Expected: PASS.

- [ ] **Step 9: Commit Task 1**

Run:

```bash
git add apps/conary/src/commands/ccs/init.rs apps/conary/src/commands/ccs/init_template.rs apps/conary/src/commands/ccs/templates.rs crates/conary-core/src/ccs/builder/test_support.rs crates/conary-core/src/ccs/v2/authoring.rs
git commit -m "feat(ccs): add config native authoring projection"
```

### Task 2: Add Target-Profile-Aware Authoring Diagnostics And CLI Flags

**Files:**
- Create: `apps/conary/src/commands/ccs/target_profile.rs`
- Modify: `apps/conary/src/commands/ccs/mod.rs`
- Modify: `apps/conary/src/cli/ccs.rs`
- Modify: `apps/conary/src/dispatch/ccs.rs`
- Modify: `apps/conary/src/commands/ccs/lint.rs`
- Modify: `apps/conary/src/commands/ccs/build.rs`
- Modify: `apps/conary/src/commands/ccs/test.rs`
- Modify: `crates/conary-core/src/ccs/v2/authoring.rs`
- Modify: `crates/conary-core/src/ccs/v2/validation.rs`
- Modify: `apps/conary/tests/packaging_m4b.rs`
- Test: `cargo test -p conary-core ccs::v2::authoring`
- Test: `cargo test -p conary --test packaging_m4e`
- Test: `cargo test -p conary --test packaging_m4b`

- [ ] **Step 1: Write failing authoring diagnostic tests**

Add tests to `crates/conary-core/src/ccs/v2/authoring.rs`:

```rust
use crate::ccs::v2::validation::{ProfileConstraintStatus, TargetProfileQuery};

struct AcceptsExampleLifecycle;

impl TargetProfileQuery for AcceptsExampleLifecycle {
    fn service_status(&self, service: &str) -> ProfileConstraintStatus {
        if service == "conary-example.service" { ProfileConstraintStatus::Accepted } else { ProfileConstraintStatus::Unsupported }
    }
    fn tmpfiles_status(&self, entry: &str) -> ProfileConstraintStatus {
        if entry == "/var/lib/conary-example" { ProfileConstraintStatus::Accepted } else { ProfileConstraintStatus::Unsupported }
    }
    fn sysctl_status(&self, _key: &str) -> ProfileConstraintStatus { ProfileConstraintStatus::Unsupported }
    fn user_status(&self, user: &str) -> ProfileConstraintStatus {
        if user == "conary-example" { ProfileConstraintStatus::Accepted } else { ProfileConstraintStatus::Unsupported }
    }
    fn group_status(&self, group: &str) -> ProfileConstraintStatus {
        if group == "conary-example" { ProfileConstraintStatus::Accepted } else { ProfileConstraintStatus::Unsupported }
    }
    fn directory_status(&self, directory: &str) -> ProfileConstraintStatus {
        if directory == "/var/lib/conary-example" { ProfileConstraintStatus::Accepted } else { ProfileConstraintStatus::Unsupported }
    }
    fn alternative_status(&self, _alternative: &str) -> ProfileConstraintStatus { ProfileConstraintStatus::Unsupported }
}

#[test]
fn lint_requires_target_profile_for_lifecycle_authoring() {
    let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal("demo", "0.1.0");
    manifest.package.release = Some("1".to_string());
    manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
    manifest.hooks.services.push(crate::ccs::manifest::Service {
        name: "conary-example.service".to_string(),
        action: crate::ccs::manifest::ServiceAction::Restart,
        reversible: None,
    });

    let findings = lint_manifest_for_v2_authoring(&manifest, None);

    assert!(findings.iter().any(|finding| {
        finding.bucket == AuthoringFindingBucket::Profile
            && finding.code == "m4e-target-profile-required"
            && finding.blocks_build
    }));
}

#[test]
fn lint_accepts_supported_lifecycle_with_target_profile() {
    let mut manifest = crate::ccs::manifest::CcsManifest::new_minimal("demo", "0.1.0");
    manifest.package.release = Some("1".to_string());
    manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
    manifest.hooks.services.push(crate::ccs::manifest::Service {
        name: "conary-example.service".to_string(),
        action: crate::ccs::manifest::ServiceAction::Restart,
        reversible: None,
    });

    let profile = V2AuthoringTargetProfile {
        public_id: "fedora-44",
        query: &AcceptsExampleLifecycle,
    };
    let findings = lint_manifest_for_v2_authoring(&manifest, Some(profile));

    assert!(findings.iter().all(|finding| finding.code != "m4e-target-profile-required"));
    assert!(findings.iter().all(|finding| !finding.blocks_build));
}
```

- [ ] **Step 2: Run authoring diagnostic tests to verify they fail**

Run:

```bash
cargo test -p conary-core ccs::v2::authoring
```

Expected: FAIL because `AuthoringFindingBucket::Profile`, `V2AuthoringTargetProfile`, and the new lint signature do not exist yet.

- [ ] **Step 3: Implement profile-aware authoring API**

In `crates/conary-core/src/ccs/v2/authoring.rs`, delete `AuthoringFindingBucket::ProfileDeferred`, add `AuthoringFindingBucket::Profile`, and add:

```rust
#[derive(Clone, Copy)]
pub struct V2AuthoringTargetProfile<'a> {
    pub public_id: &'a str,
    pub query: &'a dyn TargetProfileQuery,
}
```

Change:

```rust
pub fn lint_manifest_for_v2_authoring(
    manifest: &crate::ccs::manifest::CcsManifest,
    target_profile: Option<V2AuthoringTargetProfile<'_>>,
) -> Vec<AuthoringFinding>
```

Lifecycle declarations with no target profile should emit `m4e-target-profile-required` with `bucket: AuthoringFindingBucket::Profile`, `severity: Error`, and `blocks_build: true`. Delete the old `m4b-profile-deferred-lifecycle` diagnostic. Lifecycle declarations with a target profile should project lifecycle strings and call `validate_authority_with_profile` or equivalent per-entry checks so unsupported entries emit `m4e-lifecycle-unsupported`.

Keep dependency authoring blocked as `Profile` with the existing unresolved-dependencies behavior, because v2 dependency authoring remains out of scope. Rename the old dependency finding from `m4b-profile-deferred-dependencies` to `m4e-dependencies-unsupported` so no `ProfileDeferred` public diagnostic remains after Task 2.

Add `target_profile: Option<V2AuthoringTargetProfile<'a>>` to `V2AuthoringInput<'a>`. Since the field contains a trait object, either remove `#[derive(Debug)]` from `V2AuthoringInput` or add a custom `Debug` implementation that prints only the target profile public id.

In `crates/conary-core/src/ccs/v2/validation.rs`, make `validate_authority_with_profile` accept trait objects:

```rust
pub fn validate_authority_with_profile(
    authority: &AuthorityDocumentV2,
    profile: &(impl TargetProfileQuery + ?Sized),
) -> Result<(), V2ValidationError>
```

Add a compile-focused authoring unit test that constructs `V2AuthoringTargetProfile { query: profile as &dyn TargetProfileQuery, .. }` and passes it through both lint and projection.

- [ ] **Step 4: Write failing CLI tests for target-profile argument behavior**

Create `apps/conary/tests/packaging_m4e.rs` with this initial fixture and tests:

```rust
use std::process::{Command, Output};

#[test]
fn config_noreplace_template_builds_without_target_profile() {
    let fixture = M4eFixture::new("config-noreplace");
    let package = fixture.build_v2_local_dev(&[]);
    assert!(package.exists(), "expected package {}", package.display());
}

#[test]
fn service_template_requires_target_profile_for_v2_build() {
    let fixture = M4eFixture::new("service");
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

    assert_failure_contains(&output, &["target-profile"]);
}

#[test]
fn service_template_rejects_route_slug_as_target_profile() {
    let fixture = M4eFixture::new("service");
    let output = fixture
        .conary()
        .arg("ccs")
        .arg("lint")
        .arg(fixture.project_dir())
        .arg("--target-profile")
        .arg("fedora")
        .output()
        .expect("run conary ccs lint");

    assert_failure_contains(&output, &["fedora", "target profile"]);
}
```

Use the same helper pattern as `apps/conary/tests/packaging_m4b.rs`, with `M4eFixture::new(template)` running `conary ccs init --template <template> --name conary-example --version 0.1.0`. Its `build_v2_local_dev(&[&str])` should append extra args such as `["--target-profile", "fedora-44"]`.

- [ ] **Step 5: Run CLI tests to verify they fail**

Run:

```bash
cargo test -p conary --test packaging_m4e
```

Expected: FAIL because the test file is new and the CLI has no `--target-profile` option yet.

- [ ] **Step 6: Implement CLI target-profile plumbing**

Add `apps/conary/src/commands/ccs/target_profile.rs`:

```rust
use anyhow::{Context, Result};
use conary_core::repository::supported_profiles::SupportedProfile;

pub(crate) fn resolve_target_profile(
    id: Option<&str>,
) -> Result<Option<&'static SupportedProfile>> {
    let Some(id) = id else {
        return Ok(None);
    };
    conary_core::repository::supported_profiles::profile_by_public_id(id)
        .map(Some)
        .with_context(|| {
            format!(
                "unsupported target profile {id}; expected one of fedora-44, ubuntu-26.04, arch"
            )
        })
}
```

Update `apps/conary/src/commands/ccs/mod.rs` with `mod target_profile;`.

Update `apps/conary/src/cli/ccs.rs`:

```rust
/// Public supported target profile for lifecycle validation
#[arg(long)]
target_profile: Option<String>,
```

Add that field to `Build`, `Lint`, and `Test`, then thread it through dispatch into `CcsBuildOptions`, `cmd_ccs_lint`, and `cmd_ccs_test`.

Update `apps/conary/src/dispatch/ccs.rs` in this same step: bind `target_profile` in the `Build`, `Lint`, and `Test` match arms and forward it into the command options/functions. This file is the compile-lock path for the clap enum changes.

- [ ] **Step 7: Update build/lint/test command flows**

In `lint.rs`, import `super::target_profile`, resolve the optional profile, and call:

```rust
let profile = target_profile::resolve_target_profile(target_profile.as_deref())?;
let target_profile = profile.map(|profile| conary_core::ccs::v2::authoring::V2AuthoringTargetProfile {
    public_id: profile.id(),
    query: profile,
});
let findings = lint_manifest_for_v2_authoring(&manifest, target_profile);
```

In `build.rs`, store `pub target_profile: Option<String>` in `CcsBuildOptions`, resolve it before lint, pass the same optional `V2AuthoringTargetProfile` into lint and `V2AuthoringInput`. Update all existing `V2AuthoringInput` call sites, including Task 1 tests, to set `target_profile: None` when lifecycle is absent.

Update every `lint_manifest_for_v2_authoring` call site, including existing unit tests in `crates/conary-core/src/ccs/v2/authoring.rs`, to pass `None` when no target profile is under test.

In `test.rs`, add `target_profile: Option<String>`. For M4e, keep the install dry-run path non-mutating, but inspect the package authority before `cmd_ccs_install_with_replay_options`; return the same target-profile-required diagnostic when lifecycle authority is non-empty and no target profile was supplied, and call `validate_authority_with_profile` when one was supplied.

- [ ] **Step 8: Update the M4b lifecycle-deferred regression in the same checkpoint**

In `apps/conary/tests/packaging_m4b.rs`, replace `lifecycle_authoring_is_profile_deferred_and_blocks_v2_build` with a test that points to M4e semantics:

```rust
#[test]
fn lifecycle_authoring_now_requires_explicit_target_profile() {
    let fixture = MinimalPackageFixture::new();
    let manifest_path = fixture.project_dir().join("ccs.toml");
    let text = std::fs::read_to_string(&manifest_path).unwrap().replace(
        "services = []",
        r#"services = [{ name = "hello.service", action = "restart" }]"#,
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

    assert_failure_contains(&output, &["target-profile"]);
}
```

Do this in Task 2, not Task 6, so the `ProfileDeferred` removal and its existing regression update compile in the same checkpoint.

Also update `dependency_authoring_is_profile_deferred_and_blocks_v2_build` in this same file. Rename it around unsupported M4e dependency authoring and assert `dependencies` plus `m4e-dependencies-unsupported`, or intentionally preserve the old human-readable wording while removing `M4b` from the assertion. The test must not keep expecting `M4b` after the diagnostic code rename.

- [ ] **Step 9: Run Task 2 tests to verify they pass**

Run:

```bash
cargo test -p conary-core ccs::v2::authoring
cargo test -p conary --test packaging_m4e
cargo test -p conary --test packaging_m4b
```

Expected: PASS for Task 2 tests, including the migrated M4b lifecycle-authoring regression.

- [ ] **Step 10: Commit Task 2**

Run:

```bash
git add apps/conary/src/cli/ccs.rs apps/conary/src/dispatch/ccs.rs apps/conary/src/commands/ccs/mod.rs apps/conary/src/commands/ccs/target_profile.rs apps/conary/src/commands/ccs/lint.rs apps/conary/src/commands/ccs/build.rs apps/conary/src/commands/ccs/test.rs crates/conary-core/src/ccs/v2/authoring.rs crates/conary-core/src/ccs/v2/validation.rs apps/conary/tests/packaging_m4e.rs apps/conary/tests/packaging_m4b.rs
git commit -m "feat(ccs): validate lifecycle authoring by target profile"
```

### Task 3: Project Lifecycle Authority And Replace Profile Placeholder Entries

**Files:**
- Modify: `crates/conary-core/src/ccs/v2/authoring.rs`
- Modify: `crates/conary-core/src/repository/supported_profiles/catalog.toml`
- Modify: `crates/conary-core/src/repository/supported_profiles/tests.rs`
- Test: `cargo test -p conary-core ccs::v2::authoring`
- Test: `cargo test -p conary-core supported_profiles`

- [ ] **Step 1: Write failing lifecycle projection tests**

Add tests to `crates/conary-core/src/ccs/v2/authoring.rs`:

```rust
#[test]
fn projection_maps_declarative_lifecycle_hooks_to_signed_authority() {
    let mut build = test_support::single_file_build_result_at(
        "conary-example",
        "0.1.0",
        "/usr/bin/conary-example",
        b"#!/bin/sh\necho conary-example\n",
    );
    build.manifest.package.release = Some("1".to_string());
    build.manifest.package.kind = Some(crate::ccs::v2::PackageKindTagV2::Package);
    build.manifest.hooks.services.push(crate::ccs::manifest::Service {
        name: "conary-example.service".to_string(),
        action: crate::ccs::manifest::ServiceAction::Restart,
        reversible: None,
    });
    build.manifest.hooks.tmpfiles.push(crate::ccs::manifest::TmpfilesHook {
        entry_type: "d".to_string(),
        path: "/var/lib/conary-example".to_string(),
        mode: "0755".to_string(),
        owner: "conary-example".to_string(),
        group: "conary-example".to_string(),
        reversible: None,
    });
    build.manifest.hooks.users.push(crate::ccs::manifest::UserHook {
        name: "conary-example".to_string(),
        system: true,
        home: None,
        shell: Some("/bin/sh".to_string()),
        group: Some("conary-example".to_string()),
        reversible: None,
    });
    build.manifest.hooks.groups.push(crate::ccs::manifest::GroupHook {
        name: "conary-example".to_string(),
        system: true,
        reversible: None,
    });
    build.manifest.hooks.directories.push(crate::ccs::manifest::DirectoryHook {
        path: "/var/lib/conary-example".to_string(),
        mode: "0755".to_string(),
        owner: "conary-example".to_string(),
        group: "conary-example".to_string(),
        cleanup: None,
        reversible: None,
    });

    let profile = crate::repository::supported_profiles::profile_by_public_id("fedora-44")
        .expect("fedora profile");
    let projected = project_build_result_to_v2(V2AuthoringInput {
        build: &build,
        local_dev: true,
        debug_toml: Some(build.manifest.to_toml().unwrap()),
        target_profile: Some(V2AuthoringTargetProfile {
            public_id: profile.id(),
            query: profile,
        }),
    })
    .unwrap();

    assert_eq!(projected.authority.lifecycle.services, vec!["conary-example.service"]);
    assert_eq!(projected.authority.lifecycle.tmpfiles, vec!["/var/lib/conary-example"]);
    assert_eq!(projected.authority.lifecycle.users, vec!["conary-example"]);
    assert_eq!(projected.authority.lifecycle.groups, vec!["conary-example"]);
    assert_eq!(projected.authority.lifecycle.directories, vec!["/var/lib/conary-example"]);
}
```

- [ ] **Step 2: Write failing supported-profile catalog tests**

Add tests to `crates/conary-core/src/repository/supported_profiles/tests.rs`:

```rust
#[test]
fn m4e_profiles_accept_exact_proof_corpus_lifecycle_entries() {
    for id in ["fedora-44", "ubuntu-26.04", "arch"] {
        let profile = profile_by_public_id(id).expect(id);
        assert_eq!(profile.service_status("conary-example.service"), ProfileConstraintStatus::Accepted);
        assert_eq!(profile.tmpfiles_status("/var/lib/conary-example"), ProfileConstraintStatus::Accepted);
        assert_eq!(profile.user_status("conary-example"), ProfileConstraintStatus::Accepted);
        assert_eq!(profile.group_status("conary-example"), ProfileConstraintStatus::Accepted);
        assert_eq!(profile.directory_status("/var/lib/conary-example"), ProfileConstraintStatus::Accepted);
    }
}

#[test]
fn m4e_profiles_reject_old_placeholder_lifecycle_entries() {
    for id in ["fedora-44", "ubuntu-26.04", "arch"] {
        let profile = profile_by_public_id(id).expect(id);
        assert_eq!(profile.service_status("example.service"), ProfileConstraintStatus::Unsupported);
        assert_eq!(profile.tmpfiles_status("example.conf"), ProfileConstraintStatus::Unsupported);
    }
}
```

- [ ] **Step 3: Run lifecycle/profile tests to verify they fail**

Run:

```bash
cargo test -p conary-core ccs::v2::authoring
cargo test -p conary-core supported_profiles
```

Expected: FAIL because lifecycle projection is empty and profile catalog still contains M4d placeholders.

- [ ] **Step 4: Implement lifecycle projection**

In `authoring.rs`, build `LifecycleAuthorityV2` from manifest hooks:

```rust
fn project_lifecycle(manifest: &crate::ccs::manifest::CcsManifest) -> LifecycleAuthorityV2 {
    LifecycleAuthorityV2 {
        services: manifest
            .hooks
            .services
            .iter()
            .map(|service| service.name.clone())
            .chain(manifest.hooks.systemd.iter().map(|service| service.unit.clone()))
            .collect(),
        tmpfiles: manifest.hooks.tmpfiles.iter().map(|entry| entry.path.clone()).collect(),
        sysctl: manifest.hooks.sysctl.iter().map(|entry| entry.key.clone()).collect(),
        users: manifest.hooks.users.iter().map(|entry| entry.name.clone()).collect(),
        groups: manifest.hooks.groups.iter().map(|entry| entry.name.clone()).collect(),
        directories: manifest.hooks.directories.iter().map(|entry| entry.path.clone()).collect(),
        alternatives: manifest.hooks.alternatives.iter().map(|entry| entry.name.clone()).collect(),
    }
}
```

Set `authority.lifecycle` to this value, and if it is non-empty and a target profile is present, call `validate_authority_with_profile`.

- [ ] **Step 5: Replace profile catalog placeholders**

In every profile in `crates/conary-core/src/repository/supported_profiles/catalog.toml`, replace:

```toml
[profiles.lifecycle.services]
mode = "allow-list"
entries = ["example.service"]

[profiles.lifecycle.tmpfiles]
mode = "allow-list"
entries = ["example.conf"]
```

with:

```toml
[profiles.lifecycle.services]
mode = "allow-list"
entries = ["conary-example.service"]

[profiles.lifecycle.tmpfiles]
mode = "allow-list"
entries = ["/var/lib/conary-example"]
```

Change users/groups/directories from unsupported to exact allow lists:

```toml
[profiles.lifecycle.users]
mode = "allow-list"
entries = ["conary-example"]

[profiles.lifecycle.groups]
mode = "allow-list"
entries = ["conary-example"]

[profiles.lifecycle.directories]
mode = "allow-list"
entries = ["/var/lib/conary-example"]
```

Keep sysctl and alternatives negative-only unless a separate reviewed positive fixture is added in this same task.

- [ ] **Step 6: Run Task 3 tests to verify they pass**

Run:

```bash
cargo test -p conary-core ccs::v2::authoring
cargo test -p conary-core supported_profiles
```

Expected: PASS.

- [ ] **Step 7: Commit Task 3**

Run:

```bash
git add crates/conary-core/src/ccs/v2/authoring.rs crates/conary-core/src/repository/supported_profiles/catalog.toml crates/conary-core/src/repository/supported_profiles/tests.rs
git commit -m "feat(ccs): project lifecycle authority for profiles"
```

### Task 4: Split V2 Reader Structural Validation And Debug Projection Consistency

**Files:**
- Create: `crates/conary-core/src/ccs/v2/debug_projection.rs`
- Modify: `crates/conary-core/src/ccs/v2/mod.rs`
- Modify: `crates/conary-core/src/ccs/v2/validation.rs`
- Modify: `crates/conary-core/src/ccs/v2/reader.rs`
- Test: `cargo test -p conary-core ccs::v2::reader`
- Test: `cargo test -p conary-core ccs::v2::debug_projection`
- Test: `cargo test -p conary-core ccs::v2::validation`

- [ ] **Step 1: Write failing reader tests**

Add tests to `crates/conary-core/src/ccs/v2/reader.rs`:

```rust
#[test]
fn reader_accepts_lifecycle_authority_without_no_profile_rejection() {
    let mut authority = crate::ccs::v2::schema::AuthorityDocumentV2::package_for_tests("service");
    authority.lifecycle.services = vec!["conary-example.service".to_string()];
    let raw = authority.to_cbor().unwrap();
    let key = SigningKeyPair::generate();
    let signature = key.sign(&raw);
    let policy = TrustPolicy::strict(vec![signature.public_key.clone()]);

    read_authority_document(
        &raw,
        Some(&serde_json::to_string(&signature).unwrap()),
        None,
        None,
        None,
        &policy,
    )
    .unwrap();
}

#[test]
fn reader_accepts_debug_toml_config_when_signed_projection_matches() {
    let toml = r#"
[package]
name = "demo"
version = "0.1.0"
release = "1"
kind = "package"

[config]
files = ["/etc/conary-example/config.toml"]
noreplace = true
"#;
    let mut authority = crate::ccs::v2::schema::AuthorityDocumentV2::package_for_tests("demo");
    authority.debug_toml_sha256 = Some(crate::hash::sha256(toml.as_bytes()));
    let package = match &mut authority.kind {
        crate::ccs::v2::schema::PackageKindV2::Package(package) => package,
        _ => panic!("expected package authority"),
    };
    package.files[0].path = "/etc/conary-example/config.toml".to_string();
    package.files[0].config = Some(crate::ccs::v2::schema::ConfigPolicyV2::NoReplace);
    package.config = vec![crate::ccs::v2::schema::ConfigAuthorityV2 {
        path: "/etc/conary-example/config.toml".to_string(),
        policy: crate::ccs::v2::schema::ConfigPolicyV2::NoReplace,
    }];
    let raw = authority.to_cbor().unwrap();
    let key = SigningKeyPair::generate();
    let signature = key.sign(&raw);
    let policy = TrustPolicy::strict(vec![signature.public_key.clone()]);

    read_authority_document(
        &raw,
        Some(&serde_json::to_string(&signature).unwrap()),
        Some(toml.as_bytes()),
        None,
        None,
        &policy,
    )
    .unwrap();
}

#[test]
fn reader_accepts_debug_toml_lifecycle_when_signed_projection_matches() {
    let toml = r#"
[package]
name = "demo"
version = "0.1.0"
release = "1"
kind = "package"

[[hooks.services]]
name = "conary-example.service"
action = "restart"
"#;
    let mut authority = crate::ccs::v2::schema::AuthorityDocumentV2::package_for_tests("demo");
    authority.debug_toml_sha256 = Some(crate::hash::sha256(toml.as_bytes()));
    authority.lifecycle.services = vec!["conary-example.service".to_string()];
    let raw = authority.to_cbor().unwrap();
    let key = SigningKeyPair::generate();
    let signature = key.sign(&raw);
    let policy = TrustPolicy::strict(vec![signature.public_key.clone()]);

    read_authority_document(
        &raw,
        Some(&serde_json::to_string(&signature).unwrap()),
        Some(toml.as_bytes()),
        None,
        None,
        &policy,
    )
    .unwrap();
}
```

- [ ] **Step 2: Write failing debug projection rejection test**

Create `crates/conary-core/src/ccs/v2/debug_projection.rs` with tests:

```rust
#[cfg(test)]
mod tests {
    use crate::ccs::v2::schema::*;

    #[test]
    fn rejects_debug_config_missing_from_signed_authority() {
        let toml = r#"
[package]
name = "demo"
version = "0.1.0"
release = "1"
kind = "package"

[config]
files = ["/etc/conary-example/config.toml"]
noreplace = true
"#;
        let authority = AuthorityDocumentV2::package_for_tests("demo");
        let manifest = crate::ccs::manifest::CcsManifest::parse(toml).unwrap();

        let error = super::validate_debug_toml_projection(&authority, &manifest).unwrap_err();
        assert!(error.to_string().contains("debug TOML"));
        assert!(error.to_string().contains("/etc/conary-example/config.toml"));
    }

    #[test]
    fn rejects_debug_lifecycle_mismatch_with_signed_authority() {
        let toml = r#"
[package]
name = "demo"
version = "0.1.0"
release = "1"
kind = "package"

[[hooks.services]]
name = "other.service"
action = "restart"
"#;
        let mut authority = AuthorityDocumentV2::package_for_tests("demo");
        authority.lifecycle.services = vec!["conary-example.service".to_string()];
        let manifest = crate::ccs::manifest::CcsManifest::parse(toml).unwrap();

        let error = super::validate_debug_toml_projection(&authority, &manifest).unwrap_err();
        assert!(error.to_string().contains("debug TOML"));
        assert!(error.to_string().contains("conary-example.service"));
        assert!(error.to_string().contains("other.service"));
    }

    #[test]
    fn rejects_signed_lifecycle_missing_from_debug_toml() {
        let toml = r#"
[package]
name = "demo"
version = "0.1.0"
release = "1"
kind = "package"
"#;
        let mut authority = AuthorityDocumentV2::package_for_tests("demo");
        authority.lifecycle.services = vec!["conary-example.service".to_string()];
        let manifest = crate::ccs::manifest::CcsManifest::parse(toml).unwrap();

        let error = super::validate_debug_toml_projection(&authority, &manifest).unwrap_err();
        assert!(error.to_string().contains("debug TOML"));
        assert!(error.to_string().contains("conary-example.service"));
    }

    #[test]
    fn still_rejects_unsupported_debug_toml_install_authority() {
        let toml = r#"
[package]
name = "demo"
version = "0.1.0"
release = "1"
kind = "package"

[[dependencies.packages]]
name = "openssl"
version = ">=3.0"
"#;
        let authority = AuthorityDocumentV2::package_for_tests("demo");
        let manifest = crate::ccs::manifest::CcsManifest::parse(toml).unwrap();

        let error = super::reject_unsupported_debug_toml_install_authority(&manifest).unwrap_err();
        assert!(error.to_string().contains("debug TOML"));
        assert!(error.to_string().contains("dependencies"));
        super::validate_debug_toml_projection(&authority, &manifest).unwrap();
    }
}
```

- [ ] **Step 3: Run reader tests to verify they fail**

Run:

```bash
cargo test -p conary-core ccs::v2::reader
cargo test -p conary-core ccs::v2::debug_projection
cargo test -p conary-core ccs::v2::validation
```

Expected: FAIL because `debug_projection` does not exist and reader still calls no-profile validation plus blanket debug TOML rejection.

- [ ] **Step 4: Expose structural validation**

In `validation.rs`, rename the private common helper to a public structural helper:

```rust
pub fn validate_authority_structure(
    authority: &AuthorityDocumentV2,
) -> Result<(), V2ValidationError> {
    validate_authority_common(authority)
}
```

Keep `validate_authority` as a compatibility wrapper around `validate_authority_with_profile(authority, &M4aNoProfileFacts)` only for callers that intentionally want no-profile lifecycle rejection.

- [ ] **Step 5: Implement debug projection helper**

In `debug_projection.rs`, add:

```rust
pub fn validate_debug_toml_projection(
    authority: &AuthorityDocumentV2,
    manifest: &crate::ccs::manifest::CcsManifest,
) -> anyhow::Result<()> {
    validate_config_projection(authority, manifest)?;
    validate_lifecycle_projection(authority, manifest)?;
    Ok(())
}
```

Split the current blanket debug TOML install-authority rejection into two checks:

- Projection-matched fields are allowed in debug TOML only when they exactly match signed CBOR authority. For M4e this includes `[config] files` and `noreplace`, plus declarative lifecycle references from services, systemd units, tmpfiles paths, sysctl keys, users, groups, directories, and alternatives.
- Unsupported install-authority fields remain rejected from debug TOML. Continue rejecting debug TOML dependencies, scriptlets, legacy scriptlets, component overrides, and any future install-affecting TOML fields until their signed v2 projection is designed.

`validate_config_projection` should verify every `manifest.config.files` path appears in `PackageDataV2.config` and in the matching `FileAuthorityV2.config`, and that the debug TOML `noreplace` setting matches the signed `ConfigPolicyV2`. `validate_lifecycle_projection` should compare signed lifecycle vectors and debug TOML lifecycle references as sets for each M4e-owned lifecycle category. A mismatch in either direction should fail with a diagnostic that includes `debug TOML` and the mismatched entry names. These checks only validate consistency; debug TOML must never populate or override signed authority.

Export the new module from `crates/conary-core/src/ccs/v2/mod.rs` before wiring reader tests. Replace the existing blanket service-hook debug TOML rejection coverage with projection-matched acceptance plus mismatch rejection, while keeping unsupported fields covered by `reject_unsupported_debug_toml_install_authority`.

- [ ] **Step 6: Update reader orchestration**

In `reader.rs`, replace:

```rust
validate_authority(&authority).map_err(|error| anyhow::anyhow!("{error}"))?;
```

with:

```rust
super::validation::validate_authority_structure(&authority)
    .map_err(|error| anyhow::anyhow!("{error}"))?;
```

Replace `reject_install_authority_toml(toml_raw)?;` with a helper that parses debug TOML, rejects still-unsupported debug install fields, and calls `debug_projection::validate_debug_toml_projection(&authority, &toml_manifest)`.

- [ ] **Step 7: Run Task 4 tests to verify they pass**

Run:

```bash
cargo test -p conary-core ccs::v2::reader
cargo test -p conary-core ccs::v2::debug_projection
cargo test -p conary-core ccs::v2::validation
```

Expected: PASS.

- [ ] **Step 8: Commit Task 4**

Run:

```bash
git add crates/conary-core/src/ccs/v2/debug_projection.rs crates/conary-core/src/ccs/v2/mod.rs crates/conary-core/src/ccs/v2/validation.rs crates/conary-core/src/ccs/v2/reader.rs
git commit -m "fix(ccs): validate v2 debug projection structurally"
```

### Task 5: Add Remi Route-Derived Lifecycle Validation

**Files:**
- Modify: `apps/remi/src/server/native_publish/types.rs`
- Modify: `apps/remi/src/server/native_publish/verify.rs`
- Modify: `apps/remi/src/server/release_publish.rs`
- Test: `cargo test -p remi native_publish::verify`
- Test: `cargo test -p remi release_upload_`

- [ ] **Step 1: Write failing native publish verification tests**

Add tests to `apps/remi/src/server/native_publish/verify.rs`:

```rust
#[test]
fn release_route_resolves_to_supported_profile() {
    let profile = release_profile_for_route("fedora").expect("fedora route profile");
    assert_eq!(profile.id(), "fedora-44");
}

#[test]
fn release_route_rejects_unknown_profile_before_artifact_verification() {
    let error = release_profile_for_route("debian").unwrap_err();
    assert_eq!(error.code, NativePublishErrorCode::UnsupportedDistro);
}
```

Add the route validation negative in Task 6 by extending the existing `release_publish.rs` test fixture so the same signed/attested package path can be generated with supported and unsupported lifecycle authority.

- [ ] **Step 2: Run Remi tests to verify they fail**

Run:

```bash
cargo test -p remi native_publish::verify
```

Expected: FAIL because `release_profile_for_route` does not exist.

- [ ] **Step 3: Add lifecycle-specific native publication error code**

In `apps/remi/src/server/native_publish/types.rs`, add:

```rust
LifecycleUnsupported,
```

and map it in `NativePublishErrorCode::as_str`:

```rust
Self::LifecycleUnsupported => "LIFECYCLE_UNSUPPORTED",
```

Keep existing static publish-gate failures mapped through `PUBLISH_GATE_FAILED`; this new code is only for route-derived profile lifecycle validation after the package has already passed the shared static publish gate.

Keep unknown release-route uploads in the existing route/admin boundary: they should continue to fail before native artifact verification with the route-layer unknown-distribution response. `LifecycleUnsupported` is only for supported Remi routes whose verified v2 authority declares lifecycle entries not accepted by that route's supported profile.

- [ ] **Step 4: Implement release route profile resolution**

In `apps/remi/src/server/native_publish/verify.rs`, add:

```rust
pub(crate) fn release_profile_for_route(
    distro: &str,
) -> Result<&'static conary_core::repository::supported_profiles::SupportedProfile, NativePublishError> {
    let route = conary_core::repository::supported_profiles::route_by_slug(distro).ok_or_else(|| {
        NativePublishError::unprocessable(
            NativePublishErrorCode::UnsupportedDistro,
            format!("unsupported release distro {distro}"),
        )
    })?;
    let public_id = route.public_profile_ids().first().ok_or_else(|| {
        NativePublishError::unprocessable(
            NativePublishErrorCode::UnsupportedDistro,
            format!("release distro {distro} has no public target profile"),
        )
    })?;
    conary_core::repository::supported_profiles::profile_by_public_id(public_id).ok_or_else(|| {
        NativePublishError::unprocessable(
            NativePublishErrorCode::UnsupportedDistro,
            format!("release distro {distro} maps to missing public target profile {public_id}"),
        )
    })
}
```

- [ ] **Step 5: Thread profile into artifact verification**

Change `verify_native_artifact` to accept `route_slug: &str`, call `release_profile_for_route(route_slug)?`, and after shared static publish-gate verification, run:

```rust
conary_core::ccs::v2::validate_authority_with_profile(authority, profile).map_err(|error| {
    NativePublishError::unprocessable(
        NativePublishErrorCode::LifecycleUnsupported,
        format!("native release lifecycle validation failed: {error}"),
    )
})?;
```

Update `release_publish.rs` so the `distro` route slug is cloned into the `spawn_blocking` closure and passed to `verify_native_artifact`.

- [ ] **Step 6: Run Remi tests to verify they pass**

Run:

```bash
cargo test -p remi native_publish::verify
cargo test -p remi release_upload_
```

Expected: PASS.

- [ ] **Step 7: Commit Task 5**

Run:

```bash
git add apps/remi/src/server/native_publish/types.rs apps/remi/src/server/native_publish/verify.rs apps/remi/src/server/release_publish.rs
git commit -m "feat(remi): validate native lifecycle by release profile"
```

### Task 6: Add The M4e Positive And Negative Proof Corpus

**Files:**
- Modify: `apps/conary/tests/packaging_m4e.rs`
- Modify: `apps/remi/src/server/release_publish.rs`
- Test: `cargo test -p conary --test packaging_m4e`
- Test: `cargo test -p conary --test packaging_m4b`
- Test: `cargo test -p conary --test packaging_m4a`
- Test: `cargo test -p conary --test packaging_m4c`
- Test: `cargo test -p conary --test packaging_m4d`

- [ ] **Step 1: Add positive config and service CLI proof**

Extend `apps/conary/tests/packaging_m4e.rs`:

```rust
#[test]
fn config_noreplace_template_lints_builds_verifies_and_tests_without_target_profile() {
    let fixture = M4eFixture::new("config-noreplace");

    let lint = fixture
        .conary()
        .arg("ccs")
        .arg("lint")
        .arg(fixture.project_dir())
        .output()
        .expect("run conary ccs lint");
    assert_success(&lint);

    let package = fixture.build_v2_local_dev(&[]);
    assert!(package.exists());

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

#[test]
fn service_template_lints_builds_verifies_and_tests_with_supported_profile() {
    let fixture = M4eFixture::new("service");

    let lint = fixture
        .conary()
        .arg("ccs")
        .arg("lint")
        .arg(fixture.project_dir())
        .arg("--target-profile")
        .arg("fedora-44")
        .output()
        .expect("run conary ccs lint");
    assert_success(&lint);

    let package = fixture.build_v2_local_dev(&["--target-profile", "fedora-44"]);
    assert!(package.exists());

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
        .arg("--target-profile")
        .arg("fedora-44")
        .output()
        .expect("run conary ccs test");
    assert_success(&test);
}
```

- [ ] **Step 2: Add target-profile negative proof**

Add tests:

```rust
#[test]
fn service_template_rejects_unsupported_target_profile_ids() {
    for target in ["debian", "linux-mint", "fedora-45", "ubuntu-noble"] {
        let fixture = M4eFixture::new("service");
        let output = fixture
            .conary()
            .arg("ccs")
            .arg("build")
            .arg(fixture.project_dir())
            .arg("--format")
            .arg("v2")
            .arg("--local-dev")
            .arg("--target-profile")
            .arg(target)
            .arg("--output")
            .arg(fixture.output_dir())
            .output()
            .expect("run conary ccs build");

        assert_failure_contains(&output, &[target, "target profile"]);
    }
}

#[test]
fn unsupported_lifecycle_entry_fails_with_profile_diagnostic() {
    let fixture = M4eFixture::new("service");
    let manifest_path = fixture.project_dir().join("ccs.toml");
    let text = std::fs::read_to_string(&manifest_path)
        .unwrap()
        .replace("conary-example.service", "other.service");
    std::fs::write(&manifest_path, text).unwrap();

    let output = fixture
        .conary()
        .arg("ccs")
        .arg("build")
        .arg(fixture.project_dir())
        .arg("--format")
        .arg("v2")
        .arg("--local-dev")
        .arg("--target-profile")
        .arg("fedora-44")
        .arg("--output")
        .arg(fixture.output_dir())
        .output()
        .expect("run conary ccs build");

    assert_failure_contains(&output, &["lifecycle-unsupported", "other.service"]);
}
```

- [ ] **Step 3: Run focused CLI corpus tests**

Run:

```bash
cargo test -p conary --test packaging_m4e
cargo test -p conary --test packaging_m4b
cargo test -p conary --test packaging_m4a
cargo test -p conary --test packaging_m4c
cargo test -p conary --test packaging_m4d
```

Expected: PASS.

- [ ] **Step 4: Add Remi lifecycle and local-dev publication proof**

Add focused M4e tests in `apps/remi/src/server/release_publish.rs` that publish a service-authority package to local Remi route `fedora`, verify the upload succeeds for profile-supported lifecycle authority, then publish an otherwise valid package with lifecycle service `other.service` and verify release upload fails with lifecycle validation before storage/DB commit.

Do not model unsupported route slugs as `LifecycleUnsupported`: unknown route uploads are already rejected by the route layer with the existing unknown-distribution response before the native publish verifier runs.

Extend the private `attested_release_artifact_with_release` helper in `apps/remi/src/server/release_publish.rs` by adding an `attested_release_artifact_with_lifecycle` variant that accepts `LifecycleAuthorityV2`. Use `LifecycleAuthorityV2 { services: vec!["conary-example.service".to_string()], ..Default::default() }` for the positive fixture and `LifecycleAuthorityV2 { services: vec!["other.service".to_string()], ..Default::default() }` for the negative fixture.

Add a route-level local-dev refusal test in the same file:

```rust
#[tokio::test]
async fn release_upload_rejects_local_dev_artifact_before_public_state() {
    let signer = SigningKeyPair::generate().with_key_id("publisher");
    let local_dev = SigningKeyPair::generate().with_key_id("local-dev");
    let artifact = local_dev_release_artifact(&local_dev, "hello", "1.0.0");
    let fixture = ReleaseFixture::new(vec![trusted_signer(&signer)]);

    let response = fixture.upload_release(artifact.bytes).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response_text(response).await;
    assert_json_code(&body, "PUBLISH_GATE_FAILED");
    assert!(body.contains("missing build attestation") || body.contains("local-dev"));
    assert_no_public_state(&fixture, "hello", &artifact.content_hash);
}
```

Implement `local_dev_release_artifact` by writing a v2 package signed with the local-dev key and no accepted build-attestation envelope, so the Remi upload route proves the shared static publish gate rejects it before storage or DB commit.

- [ ] **Step 5: Run Remi proof tests**

Run:

```bash
cargo test -p remi release_upload_
cargo test -p remi native_publish
```

Expected: PASS.

- [ ] **Step 6: Commit Task 6**

Run:

```bash
git add apps/conary/tests/packaging_m4e.rs apps/remi/src/server/release_publish.rs
git commit -m "test(ccs): add m4e lifecycle proof corpus"
```

### Task 7: Update Docs, Coherency Rows, And Final Verification Gates

**Files:**
- Modify: `docs/modules/ccs.md`
- Modify: `docs/modules/remi.md`
- Modify: `docs/modules/test-fixtures.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/superpowers/feature-coherency-ledger.tsv`
- Modify: `docs/superpowers/feature-coherency-wave-scopes.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Update CCS docs**

In `docs/modules/ccs.md`, document:

```text
conary ccs init --template config-noreplace
conary ccs build --format v2 --local-dev

conary ccs init --template service
conary ccs build --format v2 --local-dev --target-profile fedora-44
conary ccs test package.ccs --dry-run --target-profile fedora-44
```

State that config-only packages are contract-validated without target profiles, while lifecycle-bearing packages require `fedora-44`, `ubuntu-26.04`, or `arch`.

- [ ] **Step 2: Update Remi docs**

In `docs/modules/remi.md`, document that native release upload validates route slugs before storage and applies route-derived supported profile facts to lifecycle-bearing v2 authority after structural/trust verification.

- [ ] **Step 3: Update fixture and ownership docs**

In `docs/modules/test-fixtures.md`, add the M4e corpus:

- `minimal-file`
- `config-noreplace`
- `service`
- target-profile negative cases
- debug TOML projection negative case
- Remi lifecycle-bearing native publication proof

In `docs/modules/feature-ownership.md` and `docs/llms/subsystem-map.md`, point lifecycle authoring and debug projection readers to:

- `apps/conary/src/commands/ccs/`
- `crates/conary-core/src/ccs/v2/authoring.rs`
- `crates/conary-core/src/ccs/v2/debug_projection.rs`
- `crates/conary-core/src/repository/supported_profiles/`
- `apps/remi/src/server/native_publish/verify.rs`

- [ ] **Step 4: Update coherency rows**

Search:

```bash
rg -n "ccs init|ccs build|target-profile|native release|Remi native|supported profile|M4e" docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv docs/modules docs/llms
```

For every public claim changed by M4e, either update the existing row or add a row with the implementation evidence path and focused test command.

Name these likely rows explicitly during the pass, updating them if their claims change: `ROUTE-REMI-NATIVE-001`, `ROUTE-REMI-M4D-001`, and `OPS-CCS-V2-M4D-001`.

- [ ] **Step 5: Refresh docs audit inventory and ledger**

Run:

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Add or update ledger rows for:

- `docs/superpowers/plans/2026-06-18-m4e-lifecycle-authoring-native-proof-corpus-implementation-plan.md`
- `docs/modules/ccs.md`
- `docs/modules/remi.md`
- `docs/modules/test-fixtures.md`
- `docs/modules/feature-ownership.md`
- `docs/llms/subsystem-map.md`

- [ ] **Step 6: Run final M4e and regression gates**

Run:

```bash
cargo test -p conary-core ccs::v2
cargo test -p conary-core supported_profiles
cargo test -p conary --test packaging_m4a
cargo test -p conary --test packaging_m4b
cargo test -p conary --test packaging_m4c
cargo test -p conary --test packaging_m4d
cargo test -p conary --test packaging_m4e
cargo test -p conary --test packaging_m2a
cargo test -p conary --lib commands::publish
cargo test -p conary-core repository::static_repo::publish_gate
cargo test -p remi release_upload_
cargo test -p remi native_publish
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv
bash scripts/check-doc-truth.sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

Expected: PASS.

- [ ] **Step 7: Commit Task 7**

Run:

```bash
git add docs/modules/ccs.md docs/modules/remi.md docs/modules/test-fixtures.md docs/modules/feature-ownership.md docs/llms/subsystem-map.md docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs(ccs): document m4e lifecycle authoring proof"
```

## Plan Review And Lock-In

Before launching implementation:

1. Run DeepSeek and Gemini with:

```bash
scripts/agentic-plan-review.sh docs/superpowers/plans/2026-06-18-m4e-lifecycle-authoring-native-proof-corpus-implementation-plan.md --review-kind plan --context docs/superpowers/specs/2026-06-18-m4e-lifecycle-authoring-native-proof-corpus-design.md --context docs/superpowers/specs/2026-06-18-m4b-native-authoring-build-lint-test-design.md --context docs/superpowers/specs/2026-06-18-m4c-remi-native-ccs-publication-design.md --context docs/superpowers/specs/2026-06-18-m4d-supported-distro-adapter-profiles-design.md
```

2. Patch verified external findings.
3. Run local agentic review.
4. Patch verified local findings.
5. Update this plan status to locked.
6. Rerun docs/audit checks:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-doc-truth.sh
git diff --check
```

## Self-Review Notes

- Spec coverage: Tasks 1-7 cover config template/projection, lifecycle template/projection, target-profile validation, reader/debug-projection split, supported profile allow-list changes, Remi route-derived validation, proof corpus, M4 exit docs, and post-M4 backlog preservation.
- Type consistency: Plan names `V2AuthoringTargetProfile`, `target_profile`, `validate_authority_structure`, `validate_debug_toml_projection`, and `release_profile_for_route` consistently across tasks.
- Placeholder scan: no unresolved implementation placeholders are intentionally left in this plan.
