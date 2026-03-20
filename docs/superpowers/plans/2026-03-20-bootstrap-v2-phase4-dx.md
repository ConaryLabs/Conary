# Bootstrap v2 Phase 4: Developer Experience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add four developer experience features to the bootstrap pipeline: build log capture, partial builds (--up-to/--only/--cascade), shell-on-failure debugging, and recipe audit.

**Architecture:** Each feature is independent. Build log capture adds log file writing to the executor using Kitchen's existing `cook.log` accumulator. Partial builds add filtering to `Pipeline::execute()` using `PipelineConfig` fields. Shell-on-failure spawns an interactive shell inside the executor's error path. Recipe audit is a new module with static analysis and optional strace-based tracing.

**Tech Stack:** Rust 1.94, existing derivation module (pipeline.rs, executor.rs), Kitchen Cook API (cook.rs), clap CLI, regex crate (recipe audit)

**Spec:** `docs/superpowers/specs/2026-03-20-bootstrap-v2-phase4-developer-experience.md` (revision 3)

---

## File Structure

### Modified files

| File | Change |
|------|--------|
| `conary-core/src/recipe/kitchen/cook.rs` | Add `pub(crate) fn log(&self) -> &str` accessor |
| `conary-core/src/derivation/executor.rs` | Add `log_dir`, `keep_logs`, `shell_on_failure` config; write build logs; spawn debug shell on failure |
| `conary-core/src/derivation/pipeline.rs` | Add `up_to_stage`, `only_packages`, `cascade`, `log_dir`, `keep_logs`, `shell_on_failure` to `PipelineConfig`; add `UncachedDependency` error; add `BuildLogWritten` event; filter stages/packages in execute loop |
| `conary-core/src/derivation/stages.rs` | Add `Stage::from_str_name()` already exists — no change needed |
| `src/cli/bootstrap.rs` | Add `Run` variant with `--up-to`, `--only`, `--cascade`, `--keep-logs`, `--shell-on-failure` flags |
| `src/cli/mod.rs` | Add `RecipeAudit` variant to `Commands` enum |
| `src/commands/bootstrap/mod.rs` | Add `cmd_bootstrap_run()` handler |
| `src/commands/mod.rs` | Add `recipe_audit` module import and dispatch |

### New files

| File | Purpose |
|------|---------|
| `conary-core/src/recipe/audit.rs` | Static analysis + strace-based recipe audit |
| `src/commands/recipe_audit.rs` | CLI handler for `conary recipe-audit` |

---

## Task 1: Build Log Capture — Cook Accessor

**Files:**
- Modify: `conary-core/src/recipe/kitchen/cook.rs:33`

- [ ] **Step 1: Add the log accessor to Cook**

In `conary-core/src/recipe/kitchen/cook.rs`, after the `impl<'a> Cook<'a>` block opening (line 40), add:

```rust
/// Access the accumulated build log.
pub(crate) fn build_log(&self) -> &str {
    &self.log
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo build -p conary-core`
Expected: compiles cleanly

- [ ] **Step 3: Commit**

```
feat(recipe): expose Cook build log accessor

Add pub(crate) build_log() method on Cook so the derivation executor
can read accumulated build output for log file persistence.
```

---

## Task 2: Build Log Capture — Executor Log Writing

**Files:**
- Modify: `conary-core/src/derivation/executor.rs`

- [ ] **Step 1: Add ExecutorConfig struct and update DerivationExecutor**

In `executor.rs`, after the `ExecutorError` enum, add:

```rust
/// Configuration for the derivation executor.
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Directory for build log files. None disables logging.
    pub log_dir: Option<PathBuf>,
    /// Preserve logs for successful builds (otherwise deleted on success).
    pub keep_logs: bool,
    /// Spawn an interactive shell when a build fails.
    pub shell_on_failure: bool,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            log_dir: None,
            keep_logs: false,
            shell_on_failure: false,
        }
    }
}
```

Update `DerivationExecutor` to hold the config:

```rust
pub struct DerivationExecutor {
    cas: CasStore,
    cas_dir: PathBuf,
    config: ExecutorConfig,
}
```

Update `DerivationExecutor::new()` to accept `ExecutorConfig`:

```rust
pub fn new(cas: CasStore, cas_dir: PathBuf, config: ExecutorConfig) -> Self {
    Self { cas, cas_dir, config }
}
```

- [ ] **Step 2: Add log writing helper**

In `executor.rs`, add a private method:

```rust
impl DerivationExecutor {
    /// Write a build log file and return the path.
    fn write_build_log(
        &self,
        recipe: &Recipe,
        derivation_id: &DerivationId,
        build_env_hash: &str,
        cook_log: &str,
        result_status: &str,
        duration_secs: u64,
        output_hash: Option<&str>,
    ) -> Option<PathBuf> {
        let log_dir = self.config.log_dir.as_ref()?;

        if let Err(e) = std::fs::create_dir_all(log_dir) {
            tracing::warn!("failed to create log dir {}: {e}", log_dir.display());
            return None;
        }

        let log_path = log_dir.join(format!(
            "{}-{}.log",
            recipe.package.name,
            &derivation_id.as_str()[..16]
        ));

        let mut content = format!(
            "=== conary derivation build log ===\n\
             package: {}\n\
             version: {}\n\
             derivation_id: {}\n\
             build_env_hash: {}\n\
             timestamp: {}\n\
             ===================================\n\n",
            recipe.package.name,
            recipe.package.version,
            derivation_id,
            build_env_hash,
            chrono::Utc::now().to_rfc3339(),
        );

        content.push_str(cook_log);

        content.push_str(&format!(
            "\n=== result ===\n\
             status: {}\n\
             duration: {}s\n",
            result_status, duration_secs,
        ));

        if let Some(hash) = output_hash {
            content.push_str(&format!("output_hash: {}\n", hash));
        }

        if let Err(e) = std::fs::write(&log_path, &content) {
            tracing::warn!("failed to write build log: {e}");
            return None;
        }

        Some(log_path)
    }
}
```

- [ ] **Step 3: Integrate log writing into execute()**

In `execute()`, after the build phases (line 206), capture the cook log before any error mapping. Restructure the build phase section to capture the log on both success and failure:

Replace the four `cook.phase()` calls (lines 199-206) with:

```rust
let build_result = (|| -> Result<(), ExecutorError> {
    cook.prep()
        .map_err(|e| ExecutorError::Build(format!("prep: {e}")))?;
    cook.unpack()
        .map_err(|e| ExecutorError::Build(format!("unpack: {e}")))?;
    cook.patch()
        .map_err(|e| ExecutorError::Build(format!("patch: {e}")))?;
    cook.simmer()
        .map_err(|e| ExecutorError::Build(format!("simmer: {e}")))?;
    Ok(())
})();

let build_duration = start.elapsed().as_secs();
let cook_log = cook.build_log().to_owned();

if let Err(build_err) = build_result {
    // Write log on failure (always preserved)
    let log_path = self.write_build_log(
        recipe, &derivation_id, build_env_hash,
        &cook_log, "FAILED", build_duration, None,
    );
    if let Some(path) = &log_path {
        info!("build log: {}", path.display());
    }
    return Err(build_err);
}
```

After the success path (after recording in derivation index, around line 246), write and conditionally delete the log:

```rust
let log_path = self.write_build_log(
    recipe, &derivation_id, build_env_hash,
    &cook_log, "success", build_duration,
    Some(&pkg_output.manifest.output_hash),
);
if let Some(path) = &log_path {
    if !self.config.keep_logs {
        let _ = std::fs::remove_file(path);
    }
}
```

- [ ] **Step 4: Fix all callers of DerivationExecutor::new()**

Search for `DerivationExecutor::new(` in the codebase. Update each call to pass `ExecutorConfig::default()` as the third argument. Key locations:
- `conary-core/src/derivation/pipeline.rs` (Pipeline::new or where executor is created)
- Test files in `conary-core/src/derivation/executor.rs`

- [ ] **Step 5: Add test for log writing**

In `executor.rs` tests:

```rust
#[test]
fn build_log_written_on_failure() {
    let tmp = TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    let cas = test_cas(tmp.path());
    let conn = setup_db();

    let config = ExecutorConfig {
        log_dir: Some(log_dir.clone()),
        keep_logs: false,
        shell_on_failure: false,
    };

    let recipe = test_recipe("sed", "4.9");
    let sysroot = tmp.path().join("sysroot");
    std::fs::create_dir_all(&sysroot).unwrap();

    let executor = DerivationExecutor::new(cas, tmp.path().join("cas"), config);
    let result = executor.execute(
        &recipe, "env_hash", &BTreeMap::new(),
        "x86_64-unknown-linux-gnu", &sysroot, &conn,
    );

    assert!(result.is_err());

    // Log file should exist
    let logs: Vec<_> = std::fs::read_dir(&log_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(logs.len(), 1, "should have one log file");

    let content = std::fs::read_to_string(logs[0].path()).unwrap();
    assert!(content.contains("package: sed"));
    assert!(content.contains("status: FAILED"));
}
```

- [ ] **Step 6: Run tests and clippy**

Run: `cargo test -p conary-core derivation::executor -- --nocapture`
Run: `cargo clippy -p conary-core -- -D warnings`

- [ ] **Step 7: Commit**

```
feat(derivation): add build log capture to executor

Executor writes build logs to log_dir when configured. Uses Kitchen's
existing cook.log accumulator — no Kitchen API changes beyond a
pub(crate) accessor. Logs preserved on failure, deleted on success
unless keep_logs is set.
```

---

## Task 3: Build Log Capture — Pipeline + CLI Wiring

**Files:**
- Modify: `conary-core/src/derivation/pipeline.rs`
- Modify: `src/cli/bootstrap.rs`
- Modify: `src/commands/bootstrap/mod.rs`

- [ ] **Step 1: Add fields to PipelineConfig**

In `pipeline.rs`, add to `PipelineConfig` (after `jobs` field, line 47):

```rust
    /// Directory for build logs. None disables logging.
    pub log_dir: Option<PathBuf>,
    /// Preserve logs even for successful builds.
    pub keep_logs: bool,
    /// Spawn interactive shell on build failure.
    pub shell_on_failure: bool,
```

- [ ] **Step 2: Add BuildLogWritten event variant**

In `PipelineEvent` enum (after `PackageFailed`, line 89):

```rust
    /// A build log file was written (preserved on failure or keep_logs).
    BuildLogWritten {
        /// Package name.
        package: String,
        /// Path to the log file.
        path: PathBuf,
    },
```

- [ ] **Step 3: Wire PipelineConfig into executor creation**

In `Pipeline::new()` or wherever the executor is created, pass the log config through. The pipeline creates its executor in `Pipeline::new()` — update the executor config there:

```rust
impl Pipeline {
    pub fn new(config: PipelineConfig, executor: DerivationExecutor) -> Self {
        Self { config, executor }
    }
}
```

Since the executor is passed in from outside, the caller must construct it with the right `ExecutorConfig`. Update `Pipeline::execute()` to not need changes here — the executor already has the config. But update the calling code in `src/commands/bootstrap/mod.rs` to build the `ExecutorConfig` from the CLI flags.

- [ ] **Step 4: Fix all PipelineConfig construction sites**

Search for `PipelineConfig {` in the codebase. Add the new fields with defaults:

```rust
log_dir: None,
keep_logs: false,
shell_on_failure: false,
```

Key locations: `src/commands/bootstrap/mod.rs`, pipeline tests in `pipeline.rs`.

- [ ] **Step 5: Add `Run` variant to BootstrapCommands**

In `src/cli/bootstrap.rs`, add to the `BootstrapCommands` enum (after `Tier2`):

```rust
    /// Run the derivation pipeline from a system manifest
    Run {
        /// Path to system manifest TOML
        manifest: String,

        /// Working directory for build artifacts
        #[arg(short, long, default_value = ".conary/bootstrap")]
        work_dir: String,

        /// Stop after completing this stage (toolchain, foundation, system, customization)
        #[arg(long)]
        up_to: Option<String>,

        /// Only build these packages (comma-separated)
        #[arg(long, value_delimiter = ',')]
        only: Option<Vec<String>>,

        /// Also rebuild reverse dependents of --only targets
        #[arg(long, requires = "only")]
        cascade: bool,

        /// Preserve build logs for successful builds
        #[arg(long)]
        keep_logs: bool,

        /// Spawn interactive shell on build failure
        #[arg(long)]
        shell_on_failure: bool,

        /// Show verbose build output
        #[arg(short, long)]
        verbose: bool,
    },
```

- [ ] **Step 6: Add cmd_bootstrap_run stub**

In `src/commands/bootstrap/mod.rs`, add:

```rust
pub fn cmd_bootstrap_run(
    manifest: &str,
    work_dir: &str,
    up_to: Option<&str>,
    only: Option<&[String]>,
    cascade: bool,
    keep_logs: bool,
    shell_on_failure: bool,
    verbose: bool,
) -> Result<()> {
    info!("bootstrap run: manifest={manifest}, work_dir={work_dir}");

    if let Some(stage_name) = up_to {
        let _stage = Stage::from_str_name(stage_name)
            .map_err(|e| anyhow::anyhow!("invalid --up-to stage: {e}"))?;
    }

    // TODO: parse manifest, load seed, assign stages, create pipeline, execute
    println!("bootstrap run is not yet fully wired — pipeline integration pending");

    Ok(())
}
```

- [ ] **Step 7: Wire Run into CLI dispatch**

In `src/commands/mod.rs`, add to the `Bootstrap` match arm the `Run` variant dispatch.

- [ ] **Step 8: Verify compilation**

Run: `cargo build`
Run: `cargo clippy -- -D warnings`

- [ ] **Step 9: Commit**

```
feat(derivation): wire build log capture through pipeline and CLI

PipelineConfig gains log_dir, keep_logs, shell_on_failure fields.
PipelineEvent gains BuildLogWritten variant. Bootstrap CLI gains Run
subcommand with --keep-logs, --shell-on-failure, --up-to, --only,
--cascade flags.
```

---

## Task 4: Partial Builds — Pipeline Filtering

**Files:**
- Modify: `conary-core/src/derivation/pipeline.rs`

- [ ] **Step 1: Add partial build fields to PipelineConfig**

In `pipeline.rs`, add to `PipelineConfig` (after the `shell_on_failure` field added in Task 3):

```rust
    /// Stop after completing this stage (inclusive).
    pub up_to_stage: Option<Stage>,
    /// Only build these packages. All other packages use cache lookups.
    pub only_packages: Option<Vec<String>>,
    /// When combined with `only_packages`, also rebuild reverse dependents.
    pub cascade: bool,
```

Update ALL `PipelineConfig` construction sites (tests and commands) to add:

```rust
up_to_stage: None,
only_packages: None,
cascade: false,
```

- [ ] **Step 2: Add UncachedDependency error variant**

In `PipelineError` enum (after `Io` variant):

```rust
    /// A dependency required by --only target has no cached derivation.
    #[error("package '{package}' depends on '{dependency}' which has no cached derivation — run a full build first or add '{dependency}' to --only")]
    UncachedDependency {
        /// The package that needs the dependency.
        package: String,
        /// The dependency with no cache hit.
        dependency: String,
    },

    /// A --only target is in a stage beyond the --up-to cutoff.
    #[error("package '{package}' is in stage '{stage}' but --up-to stops at '{cutoff}'")]
    PackageBeyondCutoff {
        package: String,
        stage: String,
        cutoff: String,
    },
```

- [ ] **Step 3: Add build set computation helper**

After the `collect_dep_ids` function, add:

```rust
/// Compute the set of packages to build based on --only and --cascade flags.
///
/// If `only_packages` is None, returns None (meaning "build everything").
/// If `cascade` is true, includes transitive dependents via `RecipeGraph`.
/// If `up_to_stage` is set, filters out packages in stages beyond the cutoff.
fn compute_build_set(
    only_packages: Option<&[String]>,
    cascade: bool,
    up_to_stage: Option<Stage>,
    recipes: &HashMap<String, Recipe>,
    assignments: &[(Stage, Vec<String>)],
) -> Option<HashSet<String>> {
    let targets = only_packages?;
    let mut build_set: HashSet<String> = targets.iter().cloned().collect();

    if cascade {
        use crate::recipe::RecipeGraph;
        let mut graph = RecipeGraph::new();
        for recipe in recipes.values() {
            graph.add_from_recipe(recipe);
        }

        let mut expanded = HashSet::new();
        for target in &build_set {
            for dep in graph.transitive_dependents(target) {
                expanded.insert(dep);
            }
        }
        build_set.extend(expanded);
    }

    // Filter by up_to_stage if set
    if let Some(cutoff) = up_to_stage {
        let allowed_packages: HashSet<String> = assignments.iter()
            .filter(|(stage, _)| *stage <= cutoff)
            .flat_map(|(_, pkgs)| pkgs.iter().cloned())
            .collect();

        let excluded: Vec<String> = build_set.difference(&allowed_packages).cloned().collect();
        for pkg in &excluded {
            warn!("skipping reverse dependent '{pkg}' due to --up-to {cutoff}");
        }
        build_set.retain(|p| allowed_packages.contains(p));
    }

    Some(build_set)
}
```

- [ ] **Step 4: Update Pipeline::execute() with filtering logic**

In `Pipeline::execute()`, before the stage loop (around line 259):

```rust
// Validate --only targets against --up-to cutoff
if let (Some(only_pkgs), Some(cutoff)) = (&self.config.only_packages, &self.config.up_to_stage) {
    for pkg in only_pkgs {
        if let Some((stage, _)) = stages_ordered.iter().find(|(_, pkgs)| pkgs.contains(pkg)) {
            if stage > cutoff {
                return Err(PipelineError::PackageBeyondCutoff {
                    package: pkg.clone(),
                    stage: stage.to_string(),
                    cutoff: cutoff.to_string(),
                });
            }
        }
    }
}

let build_set = compute_build_set(
    self.config.only_packages.as_deref(),
    self.config.cascade,
    self.config.up_to_stage,
    recipes,
    &stages_ordered,
);
```

In the stage loop, add the `--up-to` check (after `on_event(StageStarted)`):

```rust
// Check --up-to cutoff
if let Some(cutoff) = &self.config.up_to_stage {
    if stages_ordered.iter()
        .position(|(s, _)| s == stage)
        .unwrap_or(0)
        > stages_ordered.iter()
            .position(|(s, _)| s == cutoff)
            .unwrap_or(usize::MAX)
    {
        break;
    }
}
```

In the package loop, add the `--only` filter (before `executor.execute()`):

```rust
// If --only is set and this package is not in the build set,
// only do a cache lookup.
if let Some(ref build_set) = build_set {
    if !build_set.contains(pkg_name.as_str()) {
        // Must be a cache hit
        let index = DerivationIndex::new(conn);
        // ... compute derivation_id, check index, if miss return UncachedDependency
        // if hit, load manifest, add to completed, continue
        continue;
    }
}
```

(The exact integration requires careful placement within the existing loop structure. The agent implementing this should read the full `execute()` method and integrate the checks at the appropriate points.)

- [ ] **Step 5: Add tests for partial builds**

In `pipeline.rs` tests:

```rust
#[test]
fn up_to_stage_stops_after_cutoff() {
    // Create assignments with packages in Toolchain, Foundation, System
    // Set up_to_stage = Some(Stage::Foundation)
    // Verify only Toolchain and Foundation packages appear in the result
}

#[test]
fn only_packages_filters_build_set() {
    // Verify compute_build_set with only=["zlib"] returns just zlib
}

#[test]
fn cascade_expands_build_set() {
    // Verify compute_build_set with cascade=true includes dependents
}

#[test]
fn only_beyond_cutoff_returns_error() {
    // Verify PackageBeyondCutoff error when --only target is past --up-to
}
```

- [ ] **Step 6: Run tests and clippy**

Run: `cargo test -p conary-core derivation::pipeline -- --nocapture`
Run: `cargo clippy -p conary-core -- -D warnings`

- [ ] **Step 7: Commit**

```
feat(derivation): add partial build support to pipeline

Pipeline::execute() respects --up-to (stage cutoff), --only (package
filter), and --cascade (reverse dependent expansion) via PipelineConfig.
Uses RecipeGraph::transitive_dependents() for cascade. Non-targeted
packages must have cached derivations or UncachedDependency is returned.
```

---

## Task 5: Shell on Failure

**Files:**
- Modify: `conary-core/src/derivation/executor.rs`

- [ ] **Step 1: Add shell spawning helper**

In `executor.rs`, add a private function (after `CleanupGuard`):

```rust
/// Spawn an interactive debug shell in the build environment.
///
/// Only spawns if stdin is a tty. Returns when the user exits the shell.
fn spawn_debug_shell(destdir: &Path, sysroot: &Path, recipe: &Recipe) {
    use std::io::IsTerminal;

    if !std::io::stdin().is_terminal() {
        tracing::warn!("--shell-on-failure: no tty detected, skipping shell");
        return;
    }

    let shell = std::env::var("SHELL")
        .unwrap_or_else(|_| {
            if std::path::Path::new("/bin/bash").exists() {
                "/bin/bash".to_owned()
            } else {
                "/bin/sh".to_owned()
            }
        });

    eprintln!("\n  Dropping into build environment. Exit shell to continue.\n");

    let status = std::process::Command::new(&shell)
        .current_dir(destdir)
        .env("DESTDIR", destdir)
        .env("SYSROOT", sysroot)
        .env("PACKAGE", &recipe.package.name)
        .env("VERSION", &recipe.package.version)
        .status();

    match status {
        Ok(s) => info!("debug shell exited with {s}"),
        Err(e) => tracing::warn!("failed to spawn debug shell: {e}"),
    }
}
```

- [ ] **Step 2: Integrate into execute() error path**

In `execute()`, in the build failure block (where `build_result` is `Err`), before returning the error, add:

```rust
if self.config.shell_on_failure {
    // Disarm guard to keep DESTDIR alive during shell session
    destdir_guard.disarm();

    eprintln!("[FAILED] {}-{} at build phase", recipe.package.name, recipe.package.version);
    if let Some(path) = &log_path {
        eprintln!("  Build log: {}", path.display());
    }
    eprintln!("  Sysroot: {}", sysroot.display());
    eprintln!("  DESTDIR: {}", destdir.display());

    spawn_debug_shell(&destdir, sysroot, recipe);

    // Clean up DESTDIR after shell exits (guard was disarmed)
    let _ = std::fs::remove_dir_all(&destdir);
}
```

- [ ] **Step 3: Add test for tty guard**

```rust
#[test]
fn shell_on_failure_does_not_hang_without_tty() {
    // In CI/test, stdin is not a tty.
    // Verify that with shell_on_failure=true, execute() still returns
    // the build error without blocking.
    let tmp = TempDir::new().unwrap();
    let cas = test_cas(tmp.path());
    let conn = setup_db();

    let config = ExecutorConfig {
        log_dir: None,
        keep_logs: false,
        shell_on_failure: true, // enabled, but no tty
    };

    let recipe = test_recipe("sed", "4.9");
    let sysroot = tmp.path().join("sysroot");
    std::fs::create_dir_all(&sysroot).unwrap();

    let executor = DerivationExecutor::new(cas, tmp.path().join("cas"), config);
    let result = executor.execute(
        &recipe, "env_hash", &BTreeMap::new(),
        "x86_64-unknown-linux-gnu", &sysroot, &conn,
    );

    // Should fail with Build error, not hang
    assert!(matches!(result, Err(ExecutorError::Build(_))));
}
```

- [ ] **Step 4: Run tests and clippy**

Run: `cargo test -p conary-core derivation::executor -- --nocapture`
Run: `cargo clippy -p conary-core -- -D warnings`

- [ ] **Step 5: Commit**

```
feat(derivation): add --shell-on-failure debug support

On build failure with shell_on_failure enabled, the executor disarms
the DESTDIR cleanup guard, prints failure details with log path, and
spawns an interactive shell. Skipped when no tty is attached.
```

---

## Task 6: Recipe Audit — Static Analysis

**Files:**
- Create: `conary-core/src/recipe/audit.rs`
- Modify: `conary-core/src/recipe/mod.rs`

- [ ] **Step 1: Create audit module with types**

Create `conary-core/src/recipe/audit.rs`:

```rust
// conary-core/src/recipe/audit.rs

//! Recipe dependency audit -- static analysis and build-time tracing.
//!
//! Detects tools and libraries used in recipe build scripts that are not
//! declared in `makedepends` or `requires`.

use crate::recipe::Recipe;

/// Errors during recipe auditing.
#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    /// The recipe has no build section or build scripts.
    #[error("recipe has no build scripts to audit")]
    NoBuildScripts,

    /// strace is not available for --trace mode.
    #[error("strace not found in PATH — required for --trace")]
    StraceMissing,

    /// The build failed during tracing.
    #[error("build failed during trace: {0}")]
    BuildFailed(String),

    /// No sysroot available for tracing.
    #[error("no built sysroot available — run a full pipeline build first")]
    NoSysroot,

    /// Recipe parsing failed.
    #[error("recipe parse error: {0}")]
    RecipeParse(String),
}

/// Classification of an audit finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindingKind {
    /// Tool/library used but not in makedepends or requires.
    Missing,
    /// Tool/library declared and verified as used.
    Verified,
    /// Tool/library in the base build environment allowlist.
    Ignored,
}

/// A single audit finding.
#[derive(Debug, Clone)]
pub struct AuditFinding {
    /// The tool or library name detected.
    pub tool: String,
    /// The package that provides this tool (from mapping table).
    pub package: String,
    /// Where in the recipe it was detected.
    pub context: String,
    /// Classification.
    pub kind: FindingKind,
}

/// Result of auditing a recipe.
#[derive(Debug)]
pub struct AuditReport {
    /// Package name from the recipe.
    pub package_name: String,
    /// Package version from the recipe.
    pub package_version: String,
    /// All findings.
    pub findings: Vec<AuditFinding>,
}

impl AuditReport {
    /// Count findings of a specific kind.
    #[must_use]
    pub fn count(&self, kind: FindingKind) -> usize {
        self.findings.iter().filter(|f| f.kind == kind).count()
    }
}
```

- [ ] **Step 2: Add tool-to-package mapping and base allowlist**

```rust
/// Map of tool binary names to the package that provides them.
const TOOL_PACKAGE_MAP: &[(&str, &str)] = &[
    ("pkg-config", "pkg-config"),
    ("cmake", "cmake"),
    ("meson", "meson"),
    ("ninja", "ninja"),
    ("scons", "scons"),
    ("python3", "python"),
    ("python", "python"),
    ("perl", "perl"),
    ("m4", "m4"),
    ("ruby", "ruby"),
    ("autoconf", "autoconf"),
    ("automake", "automake"),
    ("libtool", "libtool"),
    ("autoreconf", "autoconf"),
    ("bison", "bison"),
    ("flex", "flex"),
    ("yacc", "bison"),
    ("lex", "flex"),
    ("gettext", "gettext"),
    ("intltool-update", "intltool"),
    ("msgfmt", "gettext"),
    ("makeinfo", "texinfo"),
    ("install-info", "texinfo"),
    ("nasm", "nasm"),
    ("yasm", "yasm"),
    ("cargo", "rust"),
    ("rustc", "rust"),
    ("go", "go"),
];

/// Tools in the base build environment that don't need declaring.
const BASE_TOOLS: &[&str] = &[
    "make", "gcc", "g++", "cc", "c++", "ld", "ar", "as", "nm", "ranlib",
    "strip", "objdump", "objcopy", "readelf", "strings",
    "bash", "sh", "env", "test", "true", "false",
    "cat", "cp", "mv", "rm", "mkdir", "rmdir", "ln", "ls", "chmod",
    "chown", "touch", "head", "tail", "sort", "uniq", "wc", "tr",
    "cut", "paste", "comm", "diff", "find", "xargs",
    "sed", "awk", "grep", "egrep", "fgrep",
    "tar", "gzip", "gunzip", "bzip2", "xz", "zstd",
    "install", "dirname", "basename", "realpath", "readlink",
    "echo", "printf", "expr", "tee",
];
```

- [ ] **Step 3: Implement static_audit**

```rust
/// Run static analysis on a recipe's build scripts.
///
/// Scans configure, make, install, and check sections for tool and library
/// references, cross-references against declared makedepends and requires.
pub fn static_audit(recipe: &Recipe) -> Result<AuditReport, AuditError> {
    let build = &recipe.build;

    // Collect all build script text
    let mut scripts = String::new();
    if let Some(ref s) = build.configure { scripts.push_str(s); scripts.push('\n'); }
    if let Some(ref s) = build.setup { scripts.push_str(s); scripts.push('\n'); }
    if let Some(ref s) = build.make { scripts.push_str(s); scripts.push('\n'); }
    if let Some(ref s) = build.install { scripts.push_str(s); scripts.push('\n'); }
    if let Some(ref s) = build.check { scripts.push_str(s); scripts.push('\n'); }

    if scripts.trim().is_empty() {
        return Err(AuditError::NoBuildScripts);
    }

    let declared: std::collections::HashSet<&str> = build.requires.iter()
        .chain(build.makedepends.iter())
        .map(|s| s.as_str())
        .collect();

    let mut findings = Vec::new();

    // Scan for tool invocations
    for &(tool, package) in TOOL_PACKAGE_MAP {
        if scripts.contains(tool) {
            let kind = if BASE_TOOLS.contains(&tool) {
                FindingKind::Ignored
            } else if declared.contains(package) {
                FindingKind::Verified
            } else {
                FindingKind::Missing
            };

            findings.push(AuditFinding {
                tool: tool.to_owned(),
                package: package.to_owned(),
                context: format!("found in build scripts"),
                kind,
            });
        }
    }

    // Scan for -l<lib> flags
    for word in scripts.split_whitespace() {
        if let Some(lib) = word.strip_prefix("-l") {
            if !lib.is_empty() && lib.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
                let kind = if declared.iter().any(|d| d.contains(lib)) {
                    FindingKind::Verified
                } else {
                    FindingKind::Missing
                };

                findings.push(AuditFinding {
                    tool: format!("-l{lib}"),
                    package: lib.to_owned(),
                    context: "linker flag".to_owned(),
                    kind,
                });
            }
        }
    }

    // Deduplicate by (tool, kind)
    findings.dedup_by(|a, b| a.tool == b.tool && a.kind == b.kind);

    Ok(AuditReport {
        package_name: recipe.package.name.clone(),
        package_version: recipe.package.version.clone(),
        findings,
    })
}
```

- [ ] **Step 4: Register module in mod.rs**

In `conary-core/src/recipe/mod.rs`, add:

```rust
pub mod audit;
```

- [ ] **Step 5: Add tests**

At the bottom of `audit.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn test_recipe_with_scripts(configure: &str, makedepends: &[&str]) -> Recipe {
        let deps = makedepends.iter()
            .map(|d| format!("\"{}\"", d))
            .collect::<Vec<_>>()
            .join(", ");
        let toml_str = format!(
            r#"
[package]
name = "test"
version = "1.0"

[source]
archive = "https://example.com/test-1.0.tar.gz"
checksum = "sha256:abc123"

[build]
makedepends = [{deps}]
configure = "{configure}"
make = "make"
install = "make install"
"#
        );
        toml::from_str(&toml_str).expect("test recipe must parse")
    }

    #[test]
    fn detects_missing_pkg_config() {
        let recipe = test_recipe_with_scripts("pkg-config --cflags foo", &[]);
        let report = static_audit(&recipe).unwrap();
        assert!(report.count(FindingKind::Missing) >= 1);
        assert!(report.findings.iter().any(|f| f.tool == "pkg-config" && f.kind == FindingKind::Missing));
    }

    #[test]
    fn verified_when_declared() {
        let recipe = test_recipe_with_scripts("pkg-config --cflags foo", &["pkg-config"]);
        let report = static_audit(&recipe).unwrap();
        assert!(report.findings.iter().any(|f| f.tool == "pkg-config" && f.kind == FindingKind::Verified));
        assert_eq!(report.count(FindingKind::Missing), 0);
    }

    #[test]
    fn ignores_base_tools() {
        let recipe = test_recipe_with_scripts("sed -i 's/foo/bar/' file && grep bar file", &[]);
        let report = static_audit(&recipe).unwrap();
        // sed and grep are base tools, should not be flagged as missing
        assert_eq!(report.count(FindingKind::Missing), 0);
    }

    #[test]
    fn detects_linker_flags() {
        let recipe = test_recipe_with_scripts("./configure LIBS='-lssl -lcrypto'", &[]);
        let report = static_audit(&recipe).unwrap();
        assert!(report.findings.iter().any(|f| f.tool == "-lssl" && f.kind == FindingKind::Missing));
    }

    #[test]
    fn empty_build_scripts_returns_error() {
        let toml_str = r#"
[package]
name = "empty"
version = "1.0"

[source]
archive = "https://example.com/empty.tar.gz"
checksum = "sha256:abc123"

[build]
"#;
        let recipe: Recipe = toml::from_str(toml_str).expect("parse");
        let result = static_audit(&recipe);
        assert!(matches!(result, Err(AuditError::NoBuildScripts)));
    }
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p conary-core recipe::audit -- --nocapture`
Run: `cargo clippy -p conary-core -- -D warnings`

- [ ] **Step 7: Commit**

```
feat(recipe): add static dependency audit for recipes

Scans build scripts for tool invocations and linker flags, cross-
references against declared makedepends/requires. Reports missing,
verified, and ignored (base tool) findings. Foundation for conary
recipe-audit command.
```

---

## Task 7: Recipe Audit — CLI Command

**Files:**
- Create: `src/commands/recipe_audit.rs`
- Modify: `src/cli/mod.rs`
- Modify: `src/commands/mod.rs`

- [ ] **Step 1: Add RecipeAudit variant to Commands enum**

In `src/cli/mod.rs`, add to the `Commands` enum:

```rust
    /// Audit a recipe for missing build dependencies
    #[command(name = "recipe-audit")]
    RecipeAudit {
        /// Path to recipe file
        recipe: Option<String>,

        /// Audit all recipes in the recipes/ directory
        #[arg(long)]
        all: bool,

        /// Run build-time tracing (slower, more thorough)
        #[arg(long)]
        trace: bool,
    },
```

- [ ] **Step 2: Create command handler**

Create `src/commands/recipe_audit.rs`:

```rust
// src/commands/recipe_audit.rs

//! Implementation of `conary recipe-audit` command.

use std::path::Path;

use anyhow::Result;
use conary_core::recipe::audit::{static_audit, AuditReport, FindingKind};

/// Run recipe audit on a single recipe file.
pub fn cmd_recipe_audit(recipe_path: Option<&str>, all: bool, trace: bool) -> Result<()> {
    if trace {
        println!("--trace mode is not yet implemented. Running static analysis only.");
    }

    if all {
        return audit_all_recipes();
    }

    let path = recipe_path
        .ok_or_else(|| anyhow::anyhow!("provide a recipe path or use --all"))?;

    let recipe = conary_core::recipe::parse_recipe_file(Path::new(path))
        .map_err(|e| anyhow::anyhow!("failed to parse recipe: {e}"))?;

    match static_audit(&recipe) {
        Ok(report) => print_report(&report),
        Err(e) => eprintln!("  audit error: {e}"),
    }

    Ok(())
}

fn audit_all_recipes() -> Result<()> {
    let recipes_dir = Path::new("recipes");
    if !recipes_dir.exists() {
        anyhow::bail!("recipes/ directory not found in current directory");
    }

    let mut total = 0;
    let mut total_missing = 0;

    for entry in walkdir::WalkDir::new(recipes_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
    {
        let path = entry.path();
        let recipe = match conary_core::recipe::parse_recipe_file(path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[WARN] skipping {}: {e}", path.display());
                continue;
            }
        };

        match static_audit(&recipe) {
            Ok(report) => {
                let missing = report.count(FindingKind::Missing);
                if missing > 0 {
                    println!("\n{} ({}):", report.package_name, path.display());
                    for f in &report.findings {
                        if f.kind == FindingKind::Missing {
                            println!("  [WARN] '{}' used but not in makedepends", f.tool);
                        }
                    }
                    total_missing += missing;
                }
                total += 1;
            }
            Err(_) => continue,
        }
    }

    println!("\nAudited {total} recipes. {total_missing} potential missing dependencies found.");
    Ok(())
}

fn print_report(report: &AuditReport) {
    println!("\nStatic analysis of {}-{}:", report.package_name, report.package_version);

    for finding in &report.findings {
        match finding.kind {
            FindingKind::Missing => {
                println!("  [WARN] '{}' used in {} but not in makedepends", finding.tool, finding.context);
            }
            FindingKind::Verified => {
                println!("  [OK]   '{}' declared and used", finding.tool);
            }
            FindingKind::Ignored => {} // don't print base tools
        }
    }

    let missing = report.count(FindingKind::Missing);
    let verified = report.count(FindingKind::Verified);

    println!("\n  {} verified, {} potential missing dependencies found.", verified, missing);
    if missing > 0 {
        println!("  Run with --trace for build-time verification.");
    }
}
```

- [ ] **Step 3: Wire into commands/mod.rs**

Add module declaration and import, then add to the command dispatch match:

```rust
Commands::RecipeAudit { recipe, all, trace } => {
    cmd_recipe_audit(recipe.as_deref(), all, trace)
}
```

- [ ] **Step 4: Check if walkdir is a dependency**

Run: `grep walkdir Cargo.toml` — if not present, add `walkdir = "2"` to `[dependencies]` in the root `Cargo.toml`. Alternatively, use `std::fs::read_dir` recursively to avoid a new dependency.

- [ ] **Step 5: Verify compilation and test CLI help**

Run: `cargo build`
Run: `cargo run -- recipe-audit --help`
Expected: shows recipe-audit usage with --all and --trace flags

- [ ] **Step 6: Commit**

```
feat: add conary recipe-audit command

Static analysis of recipe build scripts for undeclared makedepends.
Supports single recipe and --all (batch) modes. --trace (build-time
strace tracing) is stubbed for future implementation.
```

---

## Task 8: Final Integration Test

**Files:**
- Modify: `conary-core/src/derivation/executor.rs` (verify all tests pass)

- [ ] **Step 1: Run full test suite**

Run: `cargo test`
Expected: all tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: no warnings

- [ ] **Step 3: Run format check**

Run: `cargo fmt --check`
Expected: no formatting issues

- [ ] **Step 4: Verify CLI commands**

Run: `cargo run -- bootstrap run --help`
Run: `cargo run -- recipe-audit --help`
Expected: both show correct usage

- [ ] **Step 5: Final commit (if any fixups needed)**

```
chore: fix clippy warnings and formatting from Phase 4 implementation
```

---

## Summary

| Task | What | Key Risk |
|------|------|----------|
| 1 | Cook log accessor | Trivial one-liner |
| 2 | Executor log writing | Core feature — log format, retention, error path handling |
| 3 | Pipeline + CLI wiring | Touches PipelineConfig, BootstrapCommands — many callers to update |
| 4 | Partial builds | Most complex — filtering logic in Pipeline::execute() loop |
| 5 | Shell on failure | Self-contained — tty detection, shell spawning, guard manipulation |
| 6 | Recipe audit module | New module — static analysis patterns, mapping tables |
| 7 | Recipe audit CLI | Straightforward CLI wiring + output formatting |
| 8 | Integration test | Verify everything works together |

Tasks 1-3 build log capture end-to-end. Task 4 adds partial builds. Task 5 adds shell debugging. Tasks 6-7 add recipe audit. Task 8 verifies the whole thing. Dependencies: 2 depends on 1, 3 depends on 2, 5 depends on 2 (uses log_path). Tasks 4, 6, 7 are independent of each other.
