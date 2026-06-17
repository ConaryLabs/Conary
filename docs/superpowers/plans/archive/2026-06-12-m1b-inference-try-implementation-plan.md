# M1b Inference And Try Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the M1b package-authoring loop: build-system inference, `conary new` recipe materialization, inference-mode `conary cook`, tarball/git target routing, `--explain`, and the first safe `conary try` state machine.

**Architecture:** Add one shared inference engine under `conary-core` and route `new` and `cook` through it so materialized recipes and invisible inference cannot diverge. Keep CLI modules thin. Add `try_sessions` as durable core state, but keep `try` orchestration in the CLI command layer because it coordinates package install, inactive generation creation, namespace execution, and keep/rollback user flow.

**Tech Stack:** Rust 2024, clap, serde/toml/serde_json, rusqlite, tempfile, existing Kitchen/CCS/generation primitives, existing static repository M1a code, and system `git` only for M1b git target fetches.

---

## Rollout Gate

Use the parent-spec hidden-surface gate for M1b. During Tasks 4-13, the new `conary new`, `conary try`, and `cook --explain` surfaces parse in tests but are hidden from normal help output with clap `hide` attributes. Task 14 is the graduation gate: keep the surface hidden until Task 12 integration is green, then unhide and rerun the targeted integration, workspace, docs-audit, and coherency gates before the final commit. Do not add an `unstable-packaging` cargo feature unless the hidden-surface gate proves unworkable.

Before Task 1 starts, lock in this plan as a tracked, docs-audit-registered document and add the first-package tutorial skeleton with the final command sequence. The tutorial may contain "coming in this slice" notes until the matching tests exist, but the command list is written first and kept aligned with the plan. This pre-execution lock-in prevents mid-plan commits from tracking unregistered docs.

## Scope

In scope:

- Core build-system inference for Cargo, CMake, Meson, Autotools, npm, Python, and Go source trees.
- `conary new <name>`, bare `conary new`, and `conary new --from <path|archive|git>` recipe materialization, with archive/git sources made durable beside the generated recipe.
- Inference-mode `conary cook [TARGET]`, including directory, source tarball, and git target routing.
- `--explain` for `new` and `cook`, backed by a structured `InferenceTrace`.
- M1b provenance: `origin_class = inferred-source` for inferred builds, existing `native-built` for explicit recipes, and source identity for archive/git inferred builds.
- `conary try <pkg.ccs>`, `conary try status`, `conary try rollback`, and `conary try keep`, with at most one active session.
- M1b-safe hook policy: namespace and activated sessions fail closed for script hooks, legacy scriptlet bundles, and service start/stop hooks; declarative hooks are allowed only when executed against the try root/generation root.
- The "first package in 5 minutes" tutorial path: create a tiny source tree, infer/materialize/cook, try it, keep or rollback.

Out of scope:

- M2 foreign package ingestion (`.rpm`, `.deb`, `.pkg.tar.zst`) through `cook`.
- M2 artifact-form `publish <pkg.ccs> <target>`, hermetic builds, attestations, and publish lint gates.
- M3 `--record`, `--json`, `try --watch`, and MCP packaging tools.
- Full cross-build inference. M1b may preserve explicit recipe `[cross]` behavior but does not infer cross targets.

## Current Repo Facts

- `conary cook` exists and is recipe-driven. It currently rejects bare source inference with an M1b message in `apps/conary/src/commands/cook.rs`.
- `conary publish` exists in project form and uses the M1a static repository path in `apps/conary/src/commands/publish.rs`.
- `repo add --replace`, `repo reset-trust`, static fail-closed sync, and static package key persistence are already implemented in the M1a code path.
- Static repository sync routes on `default_strategy = "static"` in `crates/conary-core/src/repository/sync.rs`; M1b should not change that trust boundary.
- `crates/conary-core/src/recipe/format.rs` already supports local source paths and build command fields that inference can populate.
- Kitchen supports `recipe_source_base_dir`, local sources, host/sandboxed hardening levels, and provenance capture; M1b should reuse that instead of inventing a parallel builder.
- `crates/conary-core/src/ccs/manifest.rs` has `ManifestProvenance { origin_class, hardening_level }`; it has `Hooks` and `ScriptHook` but no reversible hook metadata yet.
- Generation code already supports inactive generation builds through `GenerationActivation::Inactive`, and CLI generation switch/rollback already know how to publish and mark active generations.
- No `try_sessions` table exists; current schema version is 72.
- `apps/conary/src/commands/install/transaction.rs` supports `defer_generation` for generation-aware installs, but it still mutates the selected DB. `try` must account for that explicitly.
- `apps/conary/src/cli/mod.rs` has no top-level `New` or `Try` command variants yet.
- `apps/conary/src/command_risk.rs` classifies command risk for safety prompts; `try keep` and activated try paths need explicit classification.

## File Map

Create:

- `crates/conary-core/src/recipe/inference/mod.rs` - module hub and public inference API.
- `crates/conary-core/src/recipe/inference/types.rs` - `InferenceTrace`, detector decisions, options, and result structs.
- `crates/conary-core/src/recipe/inference/detectors.rs` - build-system detector dispatch and ambiguity handling.
- `crates/conary-core/src/recipe/inference/materialize.rs` - recipe TOML serialization and scaffold helpers.
- `crates/conary-core/src/recipe/inference/targets.rs` - directory/archive/git source target resolution for inference and cook.
- `crates/conary-core/src/db/models/try_session.rs` - durable `try_sessions` model.
- `apps/conary/src/commands/new.rs` - `conary new` command implementation.
- `apps/conary/src/commands/try_session.rs` - `conary try` state machine and user-facing flow.
- `apps/conary/tests/packaging_m1b.rs` - CLI integration coverage for `new`, inference `cook`, tarball/git routing, and `try`.

Modify:

- `crates/conary-core/src/recipe/mod.rs` - export inference API.
- `crates/conary-core/src/recipe/format.rs` - add only small helper methods if serialization needs them.
- `crates/conary-core/src/recipe/kitchen/cook.rs`, `recipe/kitchen/config.rs`, and `recipe/kitchen/provenance_capture.rs` - preserve explicit-recipe provenance and stamp inferred builds as `inferred-source` with source identity.
- `crates/conary-core/src/recipe/kitchen/archive.rs` and `recipe/kitchen/mod.rs` - expose bounded archive download/extraction helpers for M1b target routing and resolve local relative archive paths against the recipe directory when configured.
- `crates/conary-core/src/ccs/manifest.rs` - add hook reversibility metadata or helper classification needed by `try`.
- `crates/conary-core/src/db/schema.rs`, `db/migrations/v41_current.rs`, `db/models/mod.rs` - schema v73 `try_sessions`.
- `apps/conary/src/cli/mod.rs` - add `New`, `Try`, `--explain`, and tests.
- `apps/conary/src/dispatch.rs` and `apps/conary/src/dispatch/root.rs` - run try-session orphan preflight before the existing risk gate, then route new command variants.
- `apps/conary/src/commands/cook.rs` - route bare source/archive/git targets through inference.
- `apps/conary/src/commands/install/ccs_transaction.rs` and related install helpers only as needed to expose a safe package-to-inactive-generation path for `try`; do not change normal install semantics.
- `apps/conary/src/commands/composefs_ops.rs` - add focused helper for try generation creation/promotion if the existing publication helpers are too tied to normal package mutations.
- `apps/conary/src/dispatch/root.rs` - route `new`/`try`.
- `apps/conary/src/command_risk.rs` - classify `new`, `cook --explain`, `try`, `try keep`, and activated try forms.
- `docs/llms/subsystem-map.md`, `docs/modules/feature-ownership.md`, `docs/ARCHITECTURE.md` - update look-here-first routing after implementation.
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv`, `docs/superpowers/documentation-accuracy-audit-ledger.tsv` - register this plan during lock-in after the file is staged.

## Checkpoints

- Checkpoint 1 after Task 4: `conary new --from . --explain` materializes and tests deterministic recipes, with no `cook` routing changes.
- Checkpoint 2 after Task 7: inference `cook`, archive/git routing, and provenance pass targeted tests.
- Checkpoint 3 after Task 11: `try` state machine, safe hook policy, and keep/rollback pass targeted tests.
- Checkpoint 4 after Task 14: tutorial, docs routing, docs-audit gates, clippy, fmt, and full targeted verification pass.

---

### Task 1: Add Core Inference Types And Trace

**Files:**
- Create: `crates/conary-core/src/recipe/inference/mod.rs`
- Create: `crates/conary-core/src/recipe/inference/types.rs`
- Modify: `crates/conary-core/src/recipe/mod.rs`

- [ ] **Step 1: Write trace serialization and display tests**

Add tests in `types.rs` covering:

```rust
#[test]
fn inference_trace_serializes_decisions() {
    let mut trace = InferenceTrace::new();
    trace.record_detector("cargo", 100, "Cargo.toml", "found [package] name/version");
    trace.record_decision("build-system", "cargo", "highest-confidence detector");

    let json = serde_json::to_string(&trace).unwrap();
    assert!(json.contains("\"detector\":\"cargo\""));
    assert!(json.contains("\"confidence\":100"));

    let rendered = trace.render_human();
    assert!(rendered.contains("cargo"));
    assert!(rendered.contains("Cargo.toml"));
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary-core recipe::inference::types
```

Expected: compile failure because the inference module does not exist.

- [ ] **Step 3: Implement the public inference data model**

Use this API shape:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildSystem {
    Cargo,
    CMake,
    Meson,
    Autotools,
    Npm,
    Python,
    Go,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InferenceOptions {
    pub source_root: std::path::PathBuf,
    pub package_name_override: Option<String>,
    pub version_override: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InferenceTrace {
    pub events: Vec<InferenceEvent>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InferenceEvent {
    Detector {
        detector: String,
        confidence: u8,
        evidence: String,
        detail: String,
    },
    Decision {
        field: String,
        value: String,
        reason: String,
    },
    Warning {
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct InferenceResult {
    pub build_system: BuildSystem,
    pub recipe: crate::recipe::format::Recipe,
    pub trace: InferenceTrace,
    pub source_root: std::path::PathBuf,
}
```

Implement `InferenceTrace::new`, `record_detector`, `record_decision`, `warn`, and `render_human`.

- [ ] **Step 4: Re-run the tests**

Run:

```bash
cargo test -p conary-core recipe::inference::types
```

Expected: tests pass.

- [ ] **Step 5: Commit checkpoint**

Commit:

```bash
git add crates/conary-core/src/recipe
git commit -m "feat(packaging): add inference trace model"
```

---

### Task 2: Implement Build-System Detectors

**Files:**
- Create: `crates/conary-core/src/recipe/inference/detectors.rs`
- Modify: `crates/conary-core/src/recipe/inference/mod.rs`
- Modify: `crates/conary-core/src/recipe/inference/types.rs`

- [ ] **Step 1: Write detector tests for every M1b build system**

Add tests that create temp source trees and assert the selected build system and generated recipe fields:

```rust
#[test]
fn cargo_detector_uses_package_metadata() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        r#"[package]
name = "hello-conary"
version = "0.2.0"
description = "hello from cargo"
license = "MIT"
"#,
    )
    .unwrap();

    let result = infer_recipe_from_path(dir.path(), InferenceOptions::for_source_root(dir.path())).unwrap();
    assert_eq!(result.build_system, BuildSystem::Cargo);
    assert_eq!(result.recipe.package.name, "hello-conary");
    assert_eq!(result.recipe.package.version, "0.2.0");
    assert!(result.recipe.build.make.as_deref().unwrap().contains("cargo build"));
}
```

Also cover:

- CMake from `CMakeLists.txt`.
- Meson from `meson.build`.
- Autotools from `configure.ac`, `configure.in`, or executable `configure`.
- npm from `package.json`.
- Python from `pyproject.toml`, `setup.cfg`, or `setup.py`.
- Go from `go.mod`.
- Ambiguous same-confidence trees fail and name both detectors.
- A tree with no known markers fails with a message that suggests `conary new --from .` only after adding a supported build marker or writing `recipe.toml`.
- npm inference emits a warning that registry/network access may be required and offline/reproducible npm builds are M2 work.
- Python inference emits a warning that build backends may resolve dependencies or build isolation over the network and offline/reproducible Python builds are M2 work.
- Go inference emits a warning that module downloads may be required unless `vendor/` is present and offline/reproducible Go builds are M2 work.

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary-core recipe::inference::detectors
```

Expected: tests fail because detectors are not implemented.

- [ ] **Step 3: Implement detector priority and recipe generation**

Implement `infer_recipe_from_path(source_root, options)` with this behavior:

- Canonicalize `source_root` and require it to be a directory.
- Run all detectors and record every match in `InferenceTrace`.
- If one detector has the highest confidence, select it.
- If more than one detector shares the highest confidence, return an ambiguity error naming the tied detectors and their evidence.
- Source-tree inference recipes default to `[source] path = "."`; target materialization may override the source section when `new --from <archive|git>` must make temporary sources durable.
- Generated recipes use release `1` when the recipe format supports it; otherwise preserve the current default release behavior.
- Generated recipes set `summary` from source metadata when available; otherwise use `<name> inferred from <build-system>`.
- Build commands:
  - Cargo: `cargo build --release --locked` when `Cargo.lock` exists, otherwise `cargo build --release`; install the binary named by package name to `%(destdir)s/usr/bin/<name>`.
  - CMake: configure with `cmake -S . -B build -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr`, build with `cmake --build build`, install with `DESTDIR=%(destdir)s cmake --install build`.
  - Meson: configure with `meson setup build --prefix=/usr --buildtype=release`, build with `meson compile -C build`, install with `DESTDIR=%(destdir)s meson install -C build`.
  - Autotools: run `autoreconf -fi` only when no executable `configure` exists, configure with `./configure --prefix=/usr`, build with `make`, install with `DESTDIR=%(destdir)s make install`.
  - npm: run `npm ci --omit=dev` when `package-lock.json` exists, otherwise `npm install --omit=dev`; install the source tree under `%(destdir)s/usr/lib/conary/node/<name>`. Emit an `InferenceTrace::Warning` saying npm dependency resolution may use the network in M1b and offline/reproducible builds are M2 work. If `package.json` has a string `bin` entry, add a separate warning that automatic wrapper generation is not in M1b.
  - Python: run `python -m pip install --root %(destdir)s --prefix /usr --no-deps .`. Emit an `InferenceTrace::Warning` saying Python build backends may still use network/build isolation in M1b and offline/reproducible builds are M2 work.
  - Go: run `go build -trimpath -o <name> .`, or `go build -mod=vendor -trimpath -o <name> .` when `vendor/` exists; install the binary to `%(destdir)s/usr/bin/<name>`. Emit an `InferenceTrace::Warning` saying Go module resolution may use the network when `vendor/` is absent and offline/reproducible builds are M2 work.

M1b inference assumes the existing host-build default for network-dependent npm, Go, and Python ecosystems. `conary cook --isolated` may fail for inferred projects that need registry/module downloads unless dependencies are already vendored, cached, or otherwise available inside the sandbox; offline/reproducible dependency closure is M2 work and must be called out in `--explain`.

- [ ] **Step 4: Keep the detector implementation deterministic**

Sort evidence and emitted trace events by detector order. Do not use filesystem iteration order for decisions.

- [ ] **Step 5: Re-run detector tests**

Run:

```bash
cargo test -p conary-core recipe::inference::detectors
```

Expected: tests pass.

- [ ] **Step 6: Commit checkpoint**

Commit:

```bash
git add crates/conary-core/src/recipe/inference crates/conary-core/src/recipe/mod.rs
git commit -m "feat(packaging): infer recipes from source trees"
```

---

### Task 3: Add Recipe Materialization

**Files:**
- Create: `crates/conary-core/src/recipe/inference/materialize.rs`
- Modify: `crates/conary-core/src/recipe/inference/mod.rs`
- Modify: `crates/conary-core/src/recipe/format.rs` only if helper methods are necessary

- [ ] **Step 1: Write materialization tests**

Add tests covering:

- Materializing an inferred recipe to `recipe.toml`.
- Refusing to overwrite an existing `recipe.toml` without `force`.
- `force` overwrites deterministically.
- `conary new <name>` scaffold recipe parses with `parse_recipe`.
- Serialized TOML is byte-stable for the same inference result.

Use `toml::from_str::<Recipe>(&rendered)` and `validate_recipe(&recipe)` in tests.

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary-core recipe::inference::materialize
```

Expected: tests fail because materialization helpers are not implemented.

- [ ] **Step 3: Implement materialization helpers**

Use this API shape:

```rust
#[derive(Debug, Clone)]
pub struct MaterializeOptions {
    pub output_path: std::path::PathBuf,
    pub force: bool,
    pub source_override: Option<SourceSection>,
}

pub fn render_recipe_toml(recipe: &Recipe) -> Result<String>;
pub fn write_recipe_toml(recipe: &Recipe, options: &MaterializeOptions) -> Result<()>;
pub fn scaffold_named_recipe(name: &str) -> Result<Recipe>;
```

When `source_override` is present, materialization writes that source section instead of the inference result's default `[source] path = "."`. This is required for archive/git `new --from` targets whose inference source root is temporary.

`scaffold_named_recipe` must produce a valid local-source recipe:

```toml
[package]
name = "NAME"
version = "0.1.0"
summary = "NAME"
license = "MIT"

[source]
path = "."

[build]
install = "mkdir -p %(destdir)s/usr/share/%(name)s && cp -a . %(destdir)s/usr/share/%(name)s"
```

Reject empty scaffold names and names containing path separators, `.` or `..`. Do not cite recipe validation as the syntax authority; today it only rejects empty package names.

- [ ] **Step 4: Re-run materialization tests**

Run:

```bash
cargo test -p conary-core recipe::inference::materialize
```

Expected: tests pass.

- [ ] **Step 5: Commit checkpoint**

Commit:

```bash
git add crates/conary-core/src/recipe
git commit -m "feat(packaging): materialize inferred recipes"
```

---

### Task 4: Add `conary new`

**Files:**
- Create: `apps/conary/src/commands/new.rs`
- Modify: `apps/conary/src/commands/mod.rs`
- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/dispatch/root.rs`
- Modify: `apps/conary/src/command_risk.rs`

- [ ] **Step 1: Write CLI parse and command tests**

Add tests in `cli/mod.rs`:

```rust
#[test]
fn new_from_current_dir_parses_with_explain() {
    let cli = Cli::try_parse_from(["conary", "new", "--from", ".", "--explain"]).unwrap();
    match cli.command {
        Some(Commands::New { from, explain, .. }) => {
            assert_eq!(from.as_deref(), Some("."));
            assert!(explain);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}
```

Add command tests in `commands/new.rs` covering:

- `cmd_new(None, Some("."), output, force=false, explain=true)` writes an inferred recipe and prints trace.
- Bare `cmd_new(None, None, output, force=false, explain=false)` uses current directory as `--from .` when a supported build marker is present.
- `cmd_new(Some("demo"), None, output, force=false, explain=false)` scaffolds `demo/recipe.toml`.
- Existing output without `--force` fails.

Place these tests inside `#[cfg(test)] mod tests` in `apps/conary/src/commands/new.rs`; use the `commands::new::tests` cargo filter below.

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary --lib cli::tests::new_from_current_dir_parses_with_explain
cargo test -p conary --lib commands::new::tests
```

Expected: compile failure because the command does not exist.

- [ ] **Step 3: Add CLI shape**

Add:

```rust
#[command(hide = true)] // removed in Task 14 after gates pass
New {
    /// Package project name for scaffold mode
    name: Option<String>,

    /// Infer a recipe from an existing source tree
    #[arg(long = "from")]
    from: Option<String>,

    /// Output directory for scaffold mode, or recipe path for --from mode
    #[arg(short, long)]
    output: Option<String>,

    /// Overwrite an existing recipe.toml
    #[arg(long)]
    force: bool,

    /// Print inference decisions
    #[arg(long)]
    explain: bool,
}
```

Rules:

- `conary new <name>` creates `<name>/recipe.toml`.
- `conary new --from .` writes `./recipe.toml`.
- Bare `conary new` behaves like `conary new --from .`.
- `--output` for scaffold mode is a directory; `--output` for `--from` mode is a recipe path, defaulting to `<source>/recipe.toml` for local directories.
- `name` and `--from` conflict at runtime with a clear message.
- The command is hidden from help until Task 14's gate-removal step; tests may still parse it directly.
- Archive and git `new --from` targets are added in Task 5 with the shared target resolver; Task 4 only implements local directories.

- [ ] **Step 4: Implement command routing**

`commands/new.rs` should call only core materialization/target helpers and perform user-facing printing. Keep it free of build logic.

Print:

- `Created recipe: <path>`
- When `--explain` is present, `Inference trace:` followed by `InferenceTrace::render_human()`.

- [ ] **Step 5: Classify command risk**

`conary new` writes local files only. Classify it as local-state mutation in `command_risk.rs`; `new --explain` alone still writes unless paired with a future dry-run flag, so do not classify it as read-only.

- [ ] **Step 6: Re-run tests**

Run:

```bash
cargo test -p conary --lib cli::tests::new_from_current_dir_parses_with_explain
cargo test -p conary --lib commands::new::tests
```

Expected: tests pass.

- [ ] **Step 7: Commit checkpoint**

Commit:

```bash
git add apps/conary/src/cli apps/conary/src/commands apps/conary/src/dispatch apps/conary/src/command_risk.rs
git commit -m "feat(packaging): add recipe materialization command"
```

---

### Task 5: Add Source Target Routing For Inference

**Files:**
- Create: `crates/conary-core/src/recipe/inference/targets.rs`
- Modify: `crates/conary-core/src/recipe/inference/mod.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/archive.rs` if helper visibility is needed
- Modify: `apps/conary/src/commands/new.rs`
- Modify: `apps/conary/src/cli/mod.rs`

- [ ] **Step 1: Write target routing tests**

Cover:

- Directory target returns that directory.
- `recipe.toml` target is classified as explicit recipe, not inference.
- Local `.tar`, `.tar.gz`, and `.tgz` archives extract to a temp source root and reject traversal entries.
- HTTP/HTTPS archive URLs use existing bounded download behavior.
- Git URLs clone with `git clone --depth 1` into a temp source root.
- `conary new --from <archive>` materializes `./recipe.toml` from the extracted source.
- `conary new --from <git-url-or-path>` materializes `./recipe.toml` from the cloned source.
- Unsupported extensions fail with a message that names supported target forms.

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary-core recipe::inference::targets
```

Expected: tests fail because target routing is not implemented.

- [ ] **Step 3: Implement target routing API**

Use this API shape:

```rust
pub enum CookTarget {
    RecipeFile(std::path::PathBuf),
    SourceTree(ResolvedSourceTree),
}

pub struct ResolvedSourceTree {
    pub root: std::path::PathBuf,
    pub temporary: Option<tempfile::TempDir>,
    pub original: String,
    pub kind: SourceTargetKind,
    pub provenance: SourceTargetProvenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceTargetKind {
    Directory,
    Archive,
    Git,
}

pub struct SourceTargetProvenance {
    pub original: String,
    pub kind: SourceTargetKind,
    pub archive_checksum: Option<String>,
    pub git_commit: Option<String>,
}

pub fn resolve_cook_target(target: Option<&str>, explicit_recipe: Option<&str>) -> Result<CookTarget>;
pub fn resolve_new_from_target(target: &str) -> Result<ResolvedSourceTree>;
```

Rules:

- Explicit `--recipe` always returns `RecipeFile`.
- No target checks `./recipe.toml` first, then falls back to current-directory inference.
- Directory with `recipe.toml` returns `RecipeFile`.
- Directory without `recipe.toml` returns `SourceTree`.
- Archive extraction must normalize every archive entry and reject absolute paths, `..`, and symlink entries that escape the extraction root.
- Git support shells out to `git` and requires a clear error if `git` is not installed.
- Archive targets compute `sha256:<hex>` for the archive bytes before extraction.
- Git targets record the resolved `HEAD` commit after clone.
- M1b git URLs are `https://`, `http://`, `ssh://`, `git@host:path`, or local paths ending in `.git`. Bare non-git paths stay directory/archive only.

- [ ] **Step 4: Wire archive/git targets into `conary new --from`**

Update `apps/conary/src/commands/new.rs` to call `resolve_new_from_target` for every `--from` value. Update CLI help to say "source tree, archive, or git URL" only in this task.

Rules:

- Local directory output defaults to `<source>/recipe.toml`.
- Archive output defaults to `./recipe.toml`, copies or downloads the archive to a stable path under `./sources/`, and materializes a remote-source recipe with `archive = "sources/<filename>"` and the computed `sha256:<hex>` checksum. HTTP/HTTPS archive `new --from` may preserve the original URL in `archive` only when the same computed checksum is written.
- Git output defaults to `./recipe.toml`, clones or copies the checkout to a stable source tree under `./sources/<safe-name>/`, and materializes `[source] path = "sources/<safe-name>"`. M1b does not invent a git recipe source format.
- Because current Kitchen local archive fetches treat bare paths as process-cwd relative, the implementation must add support for resolving relative archive paths against `recipe_source_base_dir` or the recipe file directory before `new --from <archive>` is considered complete. The `new_from_local_tarball_then_cook_recipe_builds_same_package` test must fail without this fix.
- `--output` remains a recipe file path for `--from`.
- `--explain` prints the same trace format regardless of source target kind.

- [ ] **Step 5: Re-run routing tests**

Run:

```bash
cargo test -p conary-core recipe::inference::targets
cargo test -p conary --lib commands::new::tests
```

Expected: tests pass.

- [ ] **Step 6: Commit checkpoint**

Commit:

```bash
git add crates/conary-core/src/recipe apps/conary/src/commands/new.rs apps/conary/src/cli/mod.rs
git commit -m "feat(packaging): resolve inference targets"
```

---

### Task 6: Route `conary cook` Through Inference

**Files:**
- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/dispatch/root.rs`
- Modify: `apps/conary/src/commands/cook.rs`
- Modify: `apps/conary/src/command_risk.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/config.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/cook.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/provenance_capture.rs`

- [ ] **Step 1: Write cook CLI and command tests**

Add CLI test:

```rust
#[test]
fn cook_accepts_explain() {
    let cli = Cli::try_parse_from(["conary", "cook", ".", "--explain"]).unwrap();
    match cli.command {
        Some(Commands::Cook { explain, .. }) => assert!(explain),
        other => panic!("unexpected command: {other:?}"),
    }
}
```

Add command tests:

- A directory with `recipe.toml` still uses explicit recipe mode and stamps `origin_class = native-built`.
- A directory with `Cargo.toml` and no `recipe.toml` infers a recipe and stamps `origin_class = inferred-source`.
- Archive and git inferred builds stamp source identity: archive original plus checksum, or git original plus resolved commit.
- Command risk classifies `conary cook` as local-state mutation, including `--validate-only`; M1b does not attempt target-sensitive read-only classification because archive/git resolution may fetch, clone, extract, or create temp/source-cache state.
- `--recipe` wins over a target with build markers.
- `--validate-only --explain` prints trace and validates the inferred recipe without building.
  This is an intentional behavior expansion: `--validate-only` on a bare source tree triggers inference and answers "is this source tree buildable by M1b inference?" instead of requiring an existing `recipe.toml`.

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary --lib cli::tests::cook_accepts_explain
cargo test -p conary --lib commands::cook
```

Expected: tests fail because `--explain` and inference routing are not wired.

- [ ] **Step 3: Add hidden `--explain` to cook CLI and command signature**

Add `explain: bool` to `Commands::Cook`, hide the flag from help until Task 14's gate-removal step, and dispatch it to `cmd_cook`.

- [ ] **Step 4: Refactor cook resolution without growing `cook.rs` into a second inference engine**

Replace `resolve_recipe_path` with a small wrapper around `conary_core::recipe::inference::resolve_cook_target`.

Behavior:

- Explicit recipe path uses existing parse/validate/build flow.
- Inferred source tree calls `infer_recipe_from_path`.
- `config.recipe_source_base_dir` is the source tree root for inferred builds.
- For explicit recipes generated by `new --from <archive>`, relative local archive paths in `[source] archive = "sources/<file>"` resolve against the recipe file directory, not the process current directory. Preserve existing HTTP/HTTPS behavior.
- Inferred builds print `Inference trace:` only with `--explain`.
- For `fetch-only`, inferred local source trees should report that no remote source fetch is required.

- [ ] **Step 5: Stamp inferred provenance**

Use a `KitchenConfig` override, not a parallel cook method:

```rust
// crates/conary-core/src/recipe/kitchen/config.rs
pub struct KitchenConfig {
    pub recipe_source_base_dir: Option<PathBuf>,
    pub origin_class_override: Option<String>,
    pub source_provenance_override: Option<SourceTargetProvenance>,
    // existing fields...
}
```

Default it to `None`. Add a matching field to `ProvenanceCapture`:

```rust
pub struct ProvenanceCapture {
    pub origin_class: Option<String>,
    pub source_provenance: Option<SourceTargetProvenance>,
    // existing fields...
}
```

In both `Cook::new` and `Cook::new_with_dest`, after `ProvenanceCapture::new()`, set:

```rust
provenance.origin_class = kitchen.config.origin_class_override.clone();
provenance.source_provenance = kitchen.config.source_provenance_override.clone();
```

In `provenance_capture.rs`, replace the hardcoded `origin_class: Some("native-built".to_string())` with:

```rust
origin_class: Some(
    self.origin_class
        .clone()
        .unwrap_or_else(|| "native-built".to_string()),
),
```

Explicit recipe mode remains `native-built`; inferred mode is `inferred-source`. Hardening remains `host` or `sandboxed`.
When `source_provenance` is present, `prep` must not overwrite it with the local-source fallback `local:.`; instead `to_manifest_provenance` maps archive provenance to `upstream_url` and `upstream_hash`, maps git provenance to `upstream_url` and `git_commit`, and leaves ordinary local-directory inference as the existing sanitized `local:<path>` marker.

- [ ] **Step 6: Re-run cook tests**

Run:

```bash
cargo test -p conary --lib cli::tests::cook_accepts_explain
cargo test -p conary --lib commands::cook
cargo test -p conary-core recipe::inference
```

Expected: tests pass.

- [ ] **Step 7: Commit checkpoint**

Commit:

```bash
git add apps/conary/src/cli apps/conary/src/commands/cook.rs apps/conary/src/dispatch/root.rs apps/conary/src/command_risk.rs crates/conary-core/src/recipe
git commit -m "feat(packaging): cook inferred source trees"
```

---

### Task 7: Add Archive And Git Cook Integration Tests

**Files:**
- Create: `apps/conary/tests/packaging_m1b.rs`
- Modify: `apps/conary/src/commands/cook.rs` only if integration tests expose routing gaps
- Modify: `crates/conary-core/src/recipe/inference/targets.rs` only if integration tests expose target gaps
- Modify: `crates/conary-core/src/recipe/kitchen/mod.rs` and `archive.rs` if integration tests expose relative local archive resolution gaps

- [ ] **Step 1: Write end-to-end target tests**

In `apps/conary/tests/packaging_m1b.rs`, add:

- `cook_local_cargo_tree_from_inference_builds_ccs`
- `cook_local_tarball_from_inference_builds_ccs`
- `new_from_local_tree_then_cook_recipe_builds_same_package`
- `new_from_local_tarball_materializes_recipe`
- `new_from_local_tarball_then_cook_recipe_builds_same_package`
- `new_from_git_target_materializes_persistent_source_then_cook_builds`

Use a tiny Cargo binary project because `cargo` is guaranteed in the Rust workspace test environment:

```toml
[package]
name = "hello-m1b"
version = "0.1.0"
edition = "2021"
```

```rust
fn main() {
    println!("hello m1b");
}
```

The generated Cargo recipe must omit `--locked` when `Cargo.lock` is absent so the fixture builds without a prior lockfile.
For `new --from <archive|git>`, tests must delete/drop the resolver's temporary extraction or clone before cooking the materialized recipe. The cook must still succeed from the stable `sources/` path or archive+checksum written beside `recipe.toml`.

- [ ] **Step 2: Run the failing integration tests**

Run:

```bash
cargo test -p conary --test packaging_m1b cook_local_cargo_tree_from_inference_builds_ccs
cargo test -p conary --test packaging_m1b cook_local_tarball_from_inference_builds_ccs
cargo test -p conary --test packaging_m1b new_from_local_tree_then_cook_recipe_builds_same_package
cargo test -p conary --test packaging_m1b new_from_local_tarball_materializes_recipe
cargo test -p conary --test packaging_m1b new_from_local_tarball_then_cook_recipe_builds_same_package
cargo test -p conary --test packaging_m1b new_from_git_target_materializes_persistent_source_then_cook_builds
```

Expected: tests may fail until CLI output paths and archive routing are fully integrated.

- [ ] **Step 3: Fix only integration gaps**

Do not add new inference behavior in this task. Fix plumbing bugs such as:

- Output directory creation.
- Temporary source root lifetime.
- Recipe source base directory for extracted archives.
- CLI success output parsing in tests.

- [ ] **Step 4: Re-run integration tests**

Run:

```bash
cargo test -p conary --test packaging_m1b cook_local_cargo_tree_from_inference_builds_ccs
cargo test -p conary --test packaging_m1b cook_local_tarball_from_inference_builds_ccs
cargo test -p conary --test packaging_m1b new_from_local_tree_then_cook_recipe_builds_same_package
cargo test -p conary --test packaging_m1b new_from_local_tarball_materializes_recipe
cargo test -p conary --test packaging_m1b new_from_local_tarball_then_cook_recipe_builds_same_package
cargo test -p conary --test packaging_m1b new_from_git_target_materializes_persistent_source_then_cook_builds
```

Expected: tests pass.

- [ ] **Step 5: Commit checkpoint**

Commit:

```bash
git add apps/conary/tests/packaging_m1b.rs apps/conary/src/commands/cook.rs crates/conary-core/src/recipe/inference crates/conary-core/src/recipe/kitchen
git commit -m "test(packaging): cover inferred cook targets"
```

---

### Task 8: Add `try_sessions` Schema And Model

**Files:**
- Create: `crates/conary-core/src/db/models/try_session.rs`
- Modify: `crates/conary-core/src/db/models/mod.rs`
- Modify: `crates/conary-core/src/db/schema.rs`
- Modify: `crates/conary-core/src/db/migrations/v41_current.rs`

- [ ] **Step 1: Write model and migration tests**

Add tests covering:

- Migration reaches schema version 73.
- Creating an active try session succeeds.
- Creating a second active try session fails.
- Marking a session `rolled_back` allows a later active session.
- `find_active_or_orphaned` returns only `active` or `orphaned` sessions.
- `set_launcher` records the launcher process id and boot id.

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary-core db::models::try_session
cargo test -p conary-core db::schema::tests
```

Expected: tests fail because schema v73 and the model do not exist.

- [ ] **Step 3: Add schema v73**

Set `SCHEMA_VERSION` to 73 and add:

```sql
CREATE TABLE try_sessions (
    id TEXT PRIMARY KEY,
    package_path TEXT NOT NULL,
    package_name TEXT,
    package_version TEXT,
    previous_generation_id INTEGER,
    try_generation_id INTEGER,
    launcher_pid INTEGER,
    launcher_boot_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'orphaned', 'kept', 'rolled_back')),
    mode TEXT NOT NULL CHECK (mode IN ('namespace', 'activated')),
    open_slot INTEGER NOT NULL DEFAULT 1 CHECK (open_slot = 1),
    work_dir TEXT NOT NULL,
    last_error TEXT,
    started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    completed_at TEXT
);

CREATE UNIQUE INDEX idx_try_sessions_single_open
    ON try_sessions(open_slot)
    WHERE status IN ('active', 'orphaned');
```

Use RFC3339-style SQLite defaults as in schema v72, not bare `CURRENT_TIMESTAMP`.
Update `apply_migration` with `73 => migrations::migrate_v73(conn)`.

- [ ] **Step 4: Implement `TrySession` model**

Use a style matching `GenerationPublication` and `RepositoryPackageKey`:

```rust
pub enum TrySessionStatus { Active, Orphaned, Kept, RolledBack }
pub enum TrySessionMode { Namespace, Activated }

pub struct TrySession {
    pub id: String,
    pub package_path: String,
    pub package_name: Option<String>,
    pub package_version: Option<String>,
    pub previous_generation_id: Option<i64>,
    pub try_generation_id: Option<i64>,
    pub launcher_pid: Option<i64>,
    pub launcher_boot_id: Option<String>,
    pub status: TrySessionStatus,
    pub mode: TrySessionMode,
    pub work_dir: String,
    pub last_error: Option<String>,
    pub started_at: Option<String>,
    pub updated_at: Option<String>,
    pub completed_at: Option<String>,
}
```

Implement `create_active`, `find_active_or_orphaned`, `find_by_id`, `set_try_generation`, `set_launcher`, `mark_orphaned`, `mark_kept`, `mark_rolled_back`, and `mark_failed_orphaned`.

- [ ] **Step 5: Re-run model tests**

Run:

```bash
cargo test -p conary-core db::models::try_session
cargo test -p conary-core db::schema::tests
```

Expected: tests pass.

- [ ] **Step 6: Commit checkpoint**

Commit:

```bash
git add crates/conary-core/src/db
git commit -m "feat(packaging): persist try sessions"
```

---

### Task 9: Define Try Hook Safety Policy

**Files:**
- Modify: `crates/conary-core/src/ccs/manifest.rs`
- Create or modify: `apps/conary/src/commands/try_session.rs`
- Modify: `apps/conary/src/commands/mod.rs`

- [ ] **Step 1: Write hook policy tests**

Cover:

- Package with no hooks is allowed.
- Package with declarative hooks is allowed only when the planned executor root is the try root or generation root.
- Package with `ScriptHook { post_install }` is rejected by default.
- Package with `ScriptHook { pre_remove }` is rejected by default.
- Package with a legacy scriptlet bundle is rejected by default.
- Package with non-empty `hooks.services` is rejected in M1b; start/stop/restart service lifecycle is not generation-scoped.
- `--allow-irreversible` does not permit script hooks, legacy scriptlet bundles, or service hooks in M1b. Namespace errors say scripts cannot run against the host root; activated errors say the host-root lifecycle helper is M2 work.
- Manifest serialization accepts an omitted `reversible` field and applies M1b defaults by hook type.

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary-core ccs::manifest
cargo test -p conary --lib commands::try_session
```

Expected: tests fail until policy helpers exist.

- [ ] **Step 3: Add manifest-side reversibility metadata and classification helpers**

Add optional wire metadata on every hook struct that can produce lifecycle effects:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub reversible: Option<bool>,
```

Apply it to declarative hook structs, `Service`, and `ScriptHook`. Omitted values use these defaults:

- Declarative hook types default to reversible only when the executor root is not `/`.
- `Service` defaults to irreversible and unsupported in M1b.
- `ScriptHook` defaults to irreversible.
- Legacy scriptlet bundles default to irreversible.

Add helper methods:

```rust
pub enum HookExecutionRoot {
    TryRoot,
    GenerationRoot,
    HostRoot,
}

impl Hooks {
    pub fn has_script_hooks(&self) -> bool;
    pub fn has_service_hooks(&self) -> bool;
    pub fn has_declarative_hooks(&self) -> bool;
    pub fn has_irreversible_hooks_for_try_root(&self, execution_root: HookExecutionRoot) -> bool;
}
```

Do not relax normal `install` scriptlet behavior in this task.

- [ ] **Step 4: Add try policy helper**

In `commands/try_session.rs`, implement:

```rust
enum TryExecutionRoot {
    NamespaceRoot,
    GenerationRoot,
    HostRoot,
}

fn validate_try_package_policy(
    package: &conary_core::ccs::CcsPackage,
    execution_root: TryExecutionRoot,
    allow_irreversible: bool,
    activated: bool,
) -> Result<()>;
```

This function must fail closed for unknown or unclassified hooks.
It must treat `HookExecutionRoot::HostRoot` as irreversible for every hook type except packages with no hooks.
It must reject `hooks.services`, script hooks, and legacy scriptlet bundles for both namespace and activated M1b sessions, even when `allow_irreversible` is true.
Map `TryExecutionRoot::NamespaceRoot` to `HookExecutionRoot::TryRoot`, `GenerationRoot` to `HookExecutionRoot::GenerationRoot`, and `HostRoot` to `HookExecutionRoot::HostRoot`.

- [ ] **Step 5: Re-run policy tests**

Run:

```bash
cargo test -p conary-core ccs::manifest
cargo test -p conary --lib commands::try_session
```

Expected: tests pass.

- [ ] **Step 6: Commit checkpoint**

Commit:

```bash
git add crates/conary-core/src/ccs/manifest.rs apps/conary/src/commands/try_session.rs apps/conary/src/commands/mod.rs
git commit -m "security(packaging): gate unsafe try hooks"
```

---

### Task 10: Add Try Generation Builder

**Files:**
- Modify: `apps/conary/src/commands/try_session.rs`
- Modify: `apps/conary/src/commands/install/ccs_transaction.rs` only if a small public wrapper is needed
- Modify: `apps/conary/src/commands/install/transaction.rs` to thread a try-only `TransactionConfig` override
- Modify: `apps/conary/src/commands/composefs_ops.rs` only if a focused try helper is needed

**Maintainability boundary:** `apps/conary/src/commands/install/*` owns normal package installation. `try_session.rs` owns the try state machine and may call small install/generation helpers, but it must not move normal install policy or scriptlet behavior into the try module.

- [ ] **Step 1: Write try builder tests**

Use temp DB/runtime roots and `CONARY_TEST_SKIP_GENERATION_MOUNT=1` where needed. Cover:

- Starting try with no active session creates an active session row.
- Starting try with an active session errors and names the active session.
- Try generation build leaves current generation symlink unchanged.
- Try generation build writes CAS objects and generation images under the live runtime root, not under `<work-dir>`.
- Try install uses an explicit transaction config override; a regression test must fail if the install path derives `objects_dir` or `generations_dir` from `<work-dir>/conary.db`.
- Activated try publishes the try generation, records the previous generation, and marks the session mode `activated`.
- Try package install passes a try root, not `/`, to lifecycle/hook execution.
- Namespace/default try code has a regression test proving it never constructs `HookExecutor` with `/`.
- Namespace declarative hook execution writes only into the mounted try generation root or that generation's live `etc-state` upperdir; hook effects must not be stored only in throwaway `<work-dir>` scratch space.
- Activated declarative hook execution uses the same promotable generation root or `etc-state` upperdir as namespace mode before publishing the generation current.
- Namespace mode rejects script hooks and legacy scriptlet bundles even when `--allow-irreversible` is supplied.
- Activated mode rejects script hooks, legacy scriptlet bundles, and `hooks.services` in M1b, even when `--allow-irreversible` is supplied.
- Activated rollback can still read the package manifest after the original input path is deleted because the package artifact is copied into the try work dir and that copy path is persisted in the session row.
- Namespace keep checkpoints the session DB, removes stale live WAL/SHM sidecars, and reopens the promoted DB without replaying old live WAL content.
- Namespace keep holds the live runtime `objects_dir/conary.lock` across checkpoint, quarantine/rename, DB reopen, generation publication, and session marking.
- Rollback marks the session `rolled_back` and removes the work dir.
- Keep publishes the try generation and marks the session `kept`.

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary --lib commands::try_session
```

Expected: tests fail because try orchestration is not implemented.

- [ ] **Step 3: Implement safe default try flow**

Default `conary try <pkg.ccs>` uses namespace mode:

1. Open the selected DB and run migrations.
2. Detect an existing active or orphaned try session and fail with the active session id.
3. Create a work dir under the runtime root, for example `<runtime-root>/try/<session-id>`, create `<work-dir>/root` for scratch-root validation, and copy the input `.ccs` to `<work-dir>/package.ccs`.
4. Create the active session row in the live DB with `package_path = <work-dir>/package.ccs`, then copy the live DB to `<work-dir>/conary.db` with `VACUUM main INTO ?1`, matching the existing DB backup pattern in `crates/conary-core/src/db/backup.rs`.
5. Build a try transaction configuration manually. Start from the live runtime root selected by the user's DB path, then set `db_path = <work-dir>/conary.db` while keeping `root`, `objects_dir`, `generations_dir`, `etc_state_dir`, and `mount_point` from the live runtime root. Thread this config through the install stack with a try-only `TransactionConfig` override on `CcsTransactionInstallOptions`/`TransactionContext`; `execute_install_transaction` must use the override instead of reconstructing `TransactionConfig::from_paths(ctx.root, ctx.db_path)` when present. Normal install callers continue to use `TransactionConfig::from_paths` unchanged.
6. Install the package into the copied DB with generation publication deferred and `CcsTransactionInstallOptions { root: <work-dir>/root, no_scripts: true, ... }`. `no_scripts = true` is mandatory for namespace mode so the transaction cannot construct `HookExecutor::new("/")` or run legacy scriptlet replay against the host.
7. Build an inactive generation from the copied DB while writing CAS objects and generation images into the live runtime root's `objects/` and `generations/` directories.
8. Mount or otherwise expose the inactive try generation as the namespace root for the launcher, using the same live runtime generation root and `etc-state/<try_generation_id>` upperdir that would be promoted by `try keep`.
9. Apply allowed declarative hooks explicitly with `HookExecutor::new(<namespace-root>)` after policy validation and before launching the shell/command. Hook writes must land in the mounted try generation root or the generation's live `etc-state` upperdir, so a later namespace `try keep` preserves them. Do not apply hooks only to `<work-dir>/root`; that directory is scratch space for install-time path validation. If a writable/promotable generation root cannot be exposed, fail closed for hook-bearing packages in namespace mode. Script hooks and legacy scriptlet bundles remain rejected in namespace mode.
10. Record `try_generation_id` in both the live DB and the copied DB.
11. Launch a shell, or the command after `--`, inside the try generation's root namespace.
12. Record the launcher pid and current boot id in both DBs before waiting on the shell/command.
13. When the shell/command exits normally, leave the session active but clear or update liveness so a later preflight can distinguish a completed namespace process from an actually still-running one; print the exact commands.

The copied-DB design is intentional: rollback can discard the copy and generated artifacts without mutating the live DB.

Activated `conary try <pkg.ccs> --activate` uses the same copied package artifact, package-policy validation, no-scripts package install, inactive generation build, and generation-root declarative hook application, then:

1. Prints the host-global activation risk before mutation.
2. Records `previous_generation_id`.
3. Publishes `try_generation_id` as the current generation through the existing generation switch helpers.
4. Marks the session mode `activated`.
5. Records the current boot id and the launcher pid when a command is supplied.
6. Runs the requested command on the host, or prints `try keep` / `try rollback` instructions when no command is supplied.

Activated mode is not the default and must not be reached as a fallback from namespace mode unless the user explicitly passed `--activate`.
M1b does not have a safe post-activation host-root lifecycle helper. Therefore `--allow-irreversible` in activated mode is accepted by the CLI but still rejects script hooks, legacy scriptlet bundles, and service hooks with an M2 message. Namespace mode also rejects them, even with `--allow-irreversible`.

- [ ] **Step 4: Implement namespace launcher with a testable fallback**

Prefer `bubblewrap` when available. If `bubblewrap` is missing:

- In interactive real-root usage, fail with an error that says `conary try --activate` is the M1b fallback for host-global testing and names the risk.
- In tests, allow a hidden environment-controlled dry launcher (`CONARY_TEST_TRY_LAUNCHER=echo`) so command routing can be verified without a real namespace.

Do not silently use `chroot` or host execution as the default.

- [ ] **Step 5: Implement keep/rollback**

`try rollback`:

- Finds active/orphaned session.
- For namespace mode, deletes the try work dir, removes the unkept try generation artifacts when they are not current, and marks `rolled_back`.
- For activated mode, loads the manifest from the copied package artifact in the work dir, confirms M1b-rejected service/script/legacy hooks are absent, then switches to `previous_generation_id` and marks `rolled_back`. M1b stores no service teardown plan because packages that request service start/stop/restart are rejected before activation.

`try keep`:

- Finds active or orphaned session.
- For namespace mode, acquires the live runtime transaction lock at `objects_dir/conary.lock` using the same fs2 file-lock semantics as `TransactionEngine::begin`.
- Opens the session copy, runs `PRAGMA wal_checkpoint(TRUNCATE)`, and closes the session copy connection before promotion.
- Deletes the session copy's `conary.db-wal` and `conary.db-shm` after the checkpoint if SQLite leaves empty sidecar files behind.
- Verifies the session copy opens cleanly after checkpointing.
- Creates a live DB backup with the existing `conary_core::db::backup::create_checkpoint` helper; do not hand-roll backup file naming.
- Closes all Conary-owned live DB handles before replacing files.
- Uses the same safe sidecar/quarantine discipline as `crates/conary-core/src/db/backup.rs` recovery code: rename/quarantine the old live `conary.db`, `conary.db-wal`, and `conary.db-shm` after the checkpoint backup succeeds, then remove any stale live sidecars before reopening.
- Atomically renames the clean session `conary.db` into the live DB path. Do not copy the session `-wal` or `-shm`; after checkpoint/truncate, they must be absent or empty and removed before live reopen.
- Reopens the promoted DB and runs migration/version verification.
- Verifies any namespace-mode declarative hook effects were applied to the promotable generation root or `etc-state/<try_generation_id>` upperdir; if the session used an ephemeral hook root, keep fails and instructs the user to rollback.
- Publishes `try_generation_id` as current through existing generation switch helpers.
- Marks the corresponding system state active.
- Marks session `kept`.
- Hold `objects_dir/conary.lock` until after the promoted DB has reopened, the generation has been published, system state has been marked active, and the session has been marked `kept`.
- If any promotion step fails after the backup is created, restore the live DB backup before returning the error.

Activated keep:

- Finds active or orphaned activated session.
- Acquires the normal package mutation lock.
- Verifies the current generation is still `try_generation_id`.
- Marks the session `kept`; no DB replacement or generation re-publish is needed because activated mode already made the try generation current.

- [ ] **Step 6: Re-run try builder tests**

Run:

```bash
cargo test -p conary --lib commands::try_session
```

Expected: tests pass.

- [ ] **Step 7: Commit checkpoint**

Commit:

```bash
git add apps/conary/src/commands/try_session.rs apps/conary/src/commands/install apps/conary/src/commands/composefs_ops.rs
git commit -m "feat(packaging): build throwaway try generations"
```

---

### Task 11: Add `conary try` CLI And Dispatch

**Files:**
- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/dispatch.rs`
- Modify: `apps/conary/src/dispatch/root.rs`
- Modify: `apps/conary/src/commands/mod.rs`
- Modify: `apps/conary/src/command_risk.rs`
- Modify: `apps/conary/src/commands/try_session.rs`

- [ ] **Step 1: Write CLI tests**

Add tests for:

- `conary try pkg.ccs`
- `conary try pkg.ccs -- /usr/bin/hello`
- `conary try pkg.ccs --activate`
- `conary try pkg.ccs --allow-irreversible --activate`
- `conary try status`
- `conary try rollback`
- `conary try keep`
- `conary try --watch` is rejected or absent because watch is M3.
- A non-try command sees an active session and runs orphan preflight before ordinary execution.

- [ ] **Step 2: Run the failing CLI tests**

Run:

```bash
cargo test -p conary --lib cli::tests::try_package_parses
cargo test -p conary --lib cli::tests::try_action_words_parse
```

Expected: tests fail because the CLI shape is absent.

- [ ] **Step 3: Add CLI shape**

Use one target positional so the visible form stays exactly `conary try <pkg.ccs>`:

```rust
#[command(hide = true)] // removed in Task 14 after gates pass
Try {
    /// Package artifact, or one of: status, rollback, keep
    target: Option<String>,

    /// Activate globally instead of the default namespace try
    #[arg(long)]
    activate: bool,

    /// Allow packages with irreversible hooks in activated mode
    #[arg(long)]
    allow_irreversible: bool,

    /// Command to run inside the try session
    #[arg(last = true)]
    run: Vec<String>,

    #[command(flatten)]
    db: DbArgs,
}
```

Dispatch reserves bare `status`, `rollback`, and `keep` as actions when no `--activate`, `--allow-irreversible`, or trailing `-- <cmd>` is present. A package file literally named `status`, `rollback`, or `keep` must be passed as `./status`, `./rollback`, or `./keep`. Do not expose `watch`.

- [ ] **Step 4: Wire dispatch**

Route:

- `try <pkg.ccs>` to `cmd_try_package`.
- `try status` to `cmd_try_status`.
- `try rollback` to `cmd_try_rollback`.
- `try keep` to `cmd_try_keep`.

- [ ] **Step 5: Add try-session orphan preflight**

In `apps/conary/src/dispatch.rs`, run the try-session preflight before `command_risk::enforce_cli_policy(...)`, then run the existing risk gate and root dispatch. The preflight may call `command_risk::classify_cli` for read-only vs mutating decisions, but it must not depend on `enforce_cli_policy` having accepted the command; otherwise orphaned activated non-interactive rollback can be blocked before recovery runs.

Before dispatching ordinary commands, call a helper that:

- Skips `try status`, `try rollback`, and `try keep`; those commands manage the active session directly.
- Opens the command's selected DB when the command has `DbArgs`, otherwise uses the default DB path.
- Treats "database does not exist" as no active session.
- Reads the current boot id from `/proc/sys/kernel/random/boot_id`, with `CONARY_TEST_BOOT_ID` as the test override.
- Classifies the requested command through `command_risk.rs` before deciding whether a live active session blocks it. Read-only and dry-run-only commands may proceed while a live active try session exists; DB-mutating, local-state-mutating, active-host, and always-live commands fail closed with `try status`, `try rollback`, and `try keep` instructions.
- For namespace sessions:
  - If `launcher_boot_id` matches the current boot id and `launcher_pid` is still alive under `/proc/<pid>`, the session is active, not orphaned. Do not mutate the session row. Allow read-only/dry-run-only commands to proceed; block mutating commands with an error that says another try session is active and names `try status`, `try rollback`, and `try keep`.
  - If the boot id changed or the launcher process is dead/missing, mark the session `orphaned`, then return the rollback/keep action text. This behavior is the same in interactive and non-interactive contexts; non-interactive never proceeds with the requested command.
- For activated sessions:
  - If the current boot id matches `launcher_boot_id` and the current generation equals `try_generation_id`, the activated session is active, not orphaned. Do not mutate the session row and do not auto-rollback. Allow read-only/dry-run-only commands to proceed; block mutating commands with `try keep` / `try rollback` instructions.
  - If the boot id changed or the current generation diverged from `try_generation_id`, mark the session `orphaned`.
  - For an orphaned activated session in interactive mode, return `try keep` / `try rollback` instructions and do not run the requested command.
  - For an orphaned activated session in non-interactive mode (`stdin` is not a TTY or `CONARY_NON_INTERACTIVE=1`), attempt automatic rollback to `previous_generation_id`; if rollback fails, return the rollback error and do not run the requested command.

Add tests for live namespace read-only allowed, live namespace mutating blocked, orphaned namespace blocked, live activated read-only allowed, live activated mutating blocked, orphaned activated interactive, and orphaned activated non-interactive preflight using a temp DB. Do not require every command variant to gain bespoke tests; one ordinary DB-bearing command and one command that uses the default DB are enough to prove the helper is wired.

- [ ] **Step 6: Classify command risk**

Classify:

- `try <pkg.ccs>` namespace mode as local-state mutation, because it writes DB/work dirs and generated artifacts.
- `try --activate` and `try keep` as active host mutation.
- `try rollback` as active host mutation only when the active session is activated; static classification can conservatively choose active host mutation.
- `try status` as read-only.

- [ ] **Step 7: Re-run CLI and dispatch tests**

Run:

```bash
cargo test -p conary --lib cli::tests::try_package_parses
cargo test -p conary --lib cli::tests::try_action_words_parse
cargo test -p conary --lib commands::try_session
cargo test -p conary --lib dispatch::root
```

Expected: tests pass.

- [ ] **Step 8: Commit checkpoint**

Commit:

```bash
git add apps/conary/src/cli apps/conary/src/dispatch.rs apps/conary/src/dispatch apps/conary/src/commands apps/conary/src/command_risk.rs
git commit -m "feat(packaging): add try command surface"
```

---

### Task 12: Add Try Integration Tests

**Files:**
- Modify: `apps/conary/tests/packaging_m1b.rs`
- Modify: `apps/conary/src/commands/try_session.rs` only if integration tests expose command gaps

- [ ] **Step 1: Add end-to-end try tests**

Use the package built by the tiny Cargo fixture from Task 7. Set test-only environment:

- `CONARY_TEST_SKIP_GENERATION_MOUNT=1`
- `CONARY_TEST_TRY_LAUNCHER=echo`

Cover:

- `conary try <pkg.ccs> -- /usr/bin/hello-m1b` creates an active session and leaves current generation unchanged.
- A second `conary try <pkg.ccs>` fails and names the active session.
- `conary try status` prints package, session id, status, and generation id.
- `conary try rollback` marks the session rolled back.
- `conary try keep` promotes the generation and marks the session kept.

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary --test packaging_m1b try_package_creates_session
cargo test -p conary --test packaging_m1b try_rollback_clears_session
```

Expected: tests may fail until test roots, DB paths, and launcher environment are plumbed.

- [ ] **Step 3: Fix only integration gaps**

Keep fixes localized to `try_session.rs` unless a genuine lower-level helper bug is exposed.

- [ ] **Step 4: Re-run try integration tests**

Run:

```bash
cargo test -p conary --test packaging_m1b try_package_creates_session
cargo test -p conary --test packaging_m1b try_rollback_clears_session
```

Expected: tests pass.

- [ ] **Step 5: Commit checkpoint**

Commit:

```bash
git add apps/conary/tests/packaging_m1b.rs apps/conary/src/commands/try_session.rs
git commit -m "test(packaging): cover try sessions"
```

---

### Task 13: Update Documentation Routing And Tutorial

**Files:**
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/ARCHITECTURE.md`
- Create or modify: `docs/guides/first-package.md` if no current tutorial exists

- [ ] **Step 1: Locate coherency-ledger rows and current public packaging claims**

Run:

```bash
awk -F '\t' 'NR == 1 || $2 ~ /(cook|new|try|explain|recipe\.toml|packaging)/ || $3 ~ /(cook|new|try|explain|recipe\.toml|packaging)/ || $7 ~ /(conary new|conary cook|conary try|--explain|recipe\.toml|packaging)/ || $8 ~ /(conary new|conary cook|conary try|--explain|recipe\.toml|packaging)/ { print NR ":" $0 }' docs/superpowers/feature-coherency-ledger.tsv
rg -n "conary (new|cook|try)|--explain|first package|packaging|static repo|recipe.toml|M1b|inference" docs apps/conary/src crates/conary-core/src
```

Expected: identify every public claim and feature-coherency row touched by M1b. The ledger scan intentionally checks `surface`, `source`, `claim`, and `actual_or_gap` fields only; do not treat evidence-only path matches as touched rows.

Update every touched feature-coherency row before committing docs, or record in the commit notes that no feature-coherency rows were touched. Then run:

```bash
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv
```

- [ ] **Step 2: Update look-here-first docs**

Update assistant routing so future workers find:

- Core inference: `crates/conary-core/src/recipe/inference/`.
- CLI surfaces: `apps/conary/src/commands/new.rs`, `cook.rs`, `try_session.rs`.
- Try state: `crates/conary-core/src/db/models/try_session.rs`.
- Tests: `apps/conary/tests/packaging_m1b.rs`.

- [ ] **Step 3: Add the first-package tutorial**

The tutorial must fit the parent spec exit gate:

1. Create a tiny Cargo project.
2. Run `conary new --from . --explain`.
3. Inspect `recipe.toml`.
4. Run `conary cook . --output ./dist --source-cache ./cache`.
5. Run `conary try ./dist/<artifact>.ccs -- /usr/bin/<name>`.
6. Run `conary try rollback` or `conary try keep`.

Every command in the tutorial must be backed by an integration or unit test from this plan.

- [ ] **Step 4: Run docs grep for stale M1a-only wording**

Run:

```bash
rg -n "M1a requires|M1b feature|bare source inference|conary try.*M1b|conary new.*M1b|--explain.*M1b" docs apps/conary/src crates/conary-core/src
```

Expected: remaining M1b mentions are historical specs/plans or accurate milestone notes.

- [ ] **Step 5: Commit checkpoint**

Commit:

```bash
git add docs/llms/subsystem-map.md docs/modules/feature-ownership.md docs/ARCHITECTURE.md docs/guides/first-package.md docs/superpowers/feature-coherency-ledger.tsv apps/conary/src crates/conary-core/src
git commit -m "docs(packaging): document inference and try workflow"
```

---

### Task 14: Run Exit Gates And Reconcile Docs Registration

**Files:**
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Stage any new or changed docs before regenerating docs-audit inventory**

The docs inventory script reads tracked files. The plan should already be tracked and registered from the pre-execution lock-in. Stage any new tutorial or docs files before regenerating inventory:

```bash
git add docs/superpowers/plans/2026-06-12-m1b-inference-try-implementation-plan.md docs/guides/first-package.md docs/llms/subsystem-map.md docs/modules/feature-ownership.md docs/ARCHITECTURE.md
```

- [ ] **Step 2: Regenerate or reconcile docs-audit inventory**

Run:

```bash
scripts/docs-audit-inventory.sh > /tmp/conary-docs-inventory.tsv
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv /tmp/conary-docs-inventory.tsv
```

Expected: inventory deltas are limited to this M1b plan if it was not already registered at lock-in and any new tutorial doc created in Task 13. Apply those deltas to `docs/superpowers/documentation-accuracy-audit-inventory.tsv`.

- [ ] **Step 3: Add ledger row**

Add or verify rows for this plan and any new tutorial doc in `docs/superpowers/documentation-accuracy-audit-ledger.tsv` with:

- Family matching the generated inventory row.
- Status `verified`.
- Disposition `corrected`.
- Related files covering parent spec, M1a plan, core inference files, CLI command files, try session model, M1b tests, and tutorial files where applicable.
- Notes saying the plan is an implementation plan and must be rechecked after execution; tutorial notes must cite the tests that execute its commands.

- [ ] **Step 4: Remove hidden rollout gate**

The Task 12 integration tests should already be passing before this step. Remove the temporary clap hiding from `conary new`, `conary try`, and `cook --explain`. Add tests that `--help` now exposes the M1b surfaces and still omits `try --watch`, `--record`, `--json`, foreign-package cook, artifact-form publish, hermetic, and attestation surfaces.

- [ ] **Step 5: Run targeted test gates**

Run:

```bash
cargo test -p conary-core recipe::inference
cargo test -p conary-core db::models::try_session
cargo test -p conary --lib commands::new::tests
cargo test -p conary --lib commands::cook
cargo test -p conary --lib commands::try_session
cargo test -p conary --test packaging_m1b
```

Expected: tests pass.

- [ ] **Step 6: Run workspace and docs gates**

Run:

```bash
cargo run -p conary-test -- list
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv <(bash scripts/docs-audit-inventory.sh)
scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
scripts/check-doc-truth.sh
bash scripts/test-doc-truth.sh
bash scripts/test-coherency-ledger.sh
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv
bash scripts/test-support-bundle.sh
```

Expected: all commands pass.

- [ ] **Step 7: Self-review against parent spec**

Check:

- `conary new --from .` landed before inference `cook`.
- `cook` supports directory, archive, and git targets.
- `--explain` is visible only now and is backed by `InferenceTrace`.
- `try` implements status, rollback, keep, one active session, durable session rows, and safe default namespace semantics.
- `try --watch`, `--record`, `--json`, foreign package cook, artifact-form publish, hermetic, and attestations remain absent or explicitly M2/M3-gated.
- Tutorial commands match executable behavior.
- Help output exposes M1b surfaces only after the hidden rollout gate is removed.
- If arch-bearing artifact filenames remain a pre-existing gap, the feature-coherency ledger records that gap and the tutorial uses the printed artifact path rather than asserting a stale filename shape.
- No stale M1a error text remains in active user-facing paths.

- [ ] **Step 8: Commit final gate changes**

Commit:

```bash
git add apps/conary/src/cli docs/guides/first-package.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "feat(packaging): graduate m1b authoring loop"
```

---

## Review Packet

Ask reviewers to check these areas first:

- The `try` copy-DB/namespace/keep design: whether it is executable with the current install and generation helpers, and whether any normal install behavior is accidentally weakened.
- Whether `conary try keep` promotion by copied-DB replacement is transactionally safe enough, including backup/restore behavior and SQLite sidecar handling.
- Detector command choices for npm, Python, and Go, especially whether generated recipes are honest about M1b network use and M2 offline/reproducible-build deferral.
- Archive and git target rules for path safety and SSRF boundaries.
- Clap shape for `conary try <pkg.ccs>` plus `try status|rollback|keep`.
- Cargo test commands in this plan; every command should be runnable as written.
- Docs-audit registration sequence; the plan file must be staged before inventory regeneration.

## Execution Notes

- Preserve the M1a static repository trust boundary. M1b inference must not parse untrusted repository metadata or bypass TUF/CCS package verification.
- Keep large-file boundaries explicit. If adding substantial behavior to a Rust file over 1000 lines, state the ownership boundary in the commit or task note before editing.
- Avoid broad compatibility layers for unused retired behavior. This package manager is still pre-user; choose the simpler correct path when old-key or old-package compatibility would add complexity without protecting real users.
- Do not add hidden future flags except the temporary M1b rollout gate and the test-only launcher escape hatch explicitly named in this plan.
- Keep commits small enough that review can bisect from inference to `cook` to `try`.
- M1b preflight serializes Conary CLI entry points only. It does not attempt to serialize conaryd or non-CLI direct DB writers during an open try session; that broader coordination belongs with later daemon/runtime hardening.
- M1b `try keep` holds the live runtime `objects_dir/conary.lock` while promoting the copied DB, but it cannot serialize non-Conary direct DB writers. That is acceptable for the current pre-production/single-user package-manager stage; broader daemon/runtime coordination is M2+ hardening.
- M1b proves the tutorial through cargo-level CLI integration tests. Cross-distro `conary-test` execution of the full cook-to-try-to-publish flow is deferred until the M2/M3 packaging integration suite; M1b still runs `cargo run -p conary-test -- list` to keep manifest parsing healthy.
