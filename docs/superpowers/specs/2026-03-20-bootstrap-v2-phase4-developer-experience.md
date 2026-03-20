---
last_updated: 2026-03-20
revision: 3
summary: Phase 4 developer experience features for bootstrap v2 — partial builds, build logs, shell-on-failure, recipe audit
---

# Bootstrap v2 Phase 4: Caching, Resume & Developer Experience

## Overview

Phase 4 makes the derivation pipeline usable day-to-day. Phases 1-3 built the
engine (derivation IDs, CAS capture, EROFS composition, staged pipeline
execution). Phase 4 adds the developer-facing features that make "change one
recipe, rebuild in minutes" a reality.

Resume support, profile show, and profile diff are already implemented. This
spec covers the four remaining features.

**Design date:** 2026-03-20

## Prerequisites

- Derivation engine core (Phase 1) — complete
- EROFS composition & layered builds (Phase 2) — complete
- Stage pipeline & profile generation (Phase 3) — complete

## Feature 1: Partial Builds (`--up-to` / `--only`)

### Problem

The pipeline always executes every stage and every package. Rebuilding a single
package after a recipe edit requires running the full pipeline, relying on cache
hits to skip completed work. This is wasteful — the pipeline must still walk
every stage, check every derivation, and compose every EROFS image.

### Design

`PipelineConfig` gains three fields:

```rust
pub struct PipelineConfig {
    // ... existing fields ...

    /// Stop after completing this stage (inclusive).
    pub up_to_stage: Option<Stage>,

    /// Only build these packages. All other packages use cache lookups.
    pub only_packages: Option<Vec<String>>,

    /// When combined with `only_packages`, also rebuild reverse dependents.
    pub cascade: bool,
}
```

**`--up-to <stage>`:** The pipeline iterates stages in order and stops after
the named stage completes. The composed EROFS from the final executed stage is
still produced. Stages after the cutoff are skipped entirely.

**`--only <package>`:** The pipeline walks the full stage/dependency order but
only builds the target package(s). All other packages use normal cache lookups
— they're needed for dependency ID computation. If a required dependency has
no cached derivation, the pipeline returns an error using a new
`PipelineError::UncachedDependency` variant:

```rust
/// A dependency required by `--only` target has no cached derivation.
#[error("package '{package}' depends on '{dependency}' which has no cached derivation — run a full build first or add '{dependency}' to --only")]
UncachedDependency {
    package: String,
    dependency: String,
},
```

**`--only` does NOT force-invalidate.** Targeted packages go through the
normal derivation ID computation and cache lookup. If the recipe hasn't
changed, the derivation ID is the same as before and the cache hit is
preserved. If the recipe or source changed, the derivation ID differs
naturally and a fresh build occurs. This avoids the cascading invalidation
problem where force-rebuilding a package would change its output hash and
invalidate all downstream dependents.

**`--only <package> --cascade`:** After identifying the target package(s), the
pipeline uses `RecipeGraph::transitive_dependents()` (which already exists in
`conary-core/src/recipe/graph.rs`) to find all reverse dependents. Those
packages are added to the build set. Execution order remains topological.

### CLI Structure

The existing `BootstrapCommands` enum in `src/cli/bootstrap.rs` uses
subcommands (`Init`, `Check`, `Resume`, `DryRun`, etc.) — not a flat command
with a positional manifest argument. The new flags go on a new `Run` variant:

```rust
/// Run the derivation pipeline from a system manifest
Run {
    /// Path to system manifest TOML
    manifest: String,

    /// Working directory for build artifacts
    #[arg(short, long, default_value = ".conary/bootstrap")]
    work_dir: String,

    /// Stop after completing this stage (requires Stage to derive clap::ValueEnum)
    #[arg(long, value_enum)]
    up_to: Option<Stage>,

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

Usage:

```
conary bootstrap run my-system.toml --up-to foundation
conary bootstrap run my-system.toml --only zlib
conary bootstrap run my-system.toml --only zlib --cascade
conary bootstrap run my-system.toml --only zlib,openssl --cascade
```

**`Stage` derive change:** `Stage` in `conary-core/src/derivation/stages.rs`
must derive `clap::ValueEnum` for the `--up-to` CLI argument. Since `clap` is
a dependency of the `conary` binary crate (not `conary-core`), this requires
adding `clap` as an optional dependency to `conary-core/Cargo.toml` behind a
`cli` feature, or using a string argument with manual parsing. The simpler
approach: accept `--up-to` as a `String` and parse it to `Stage` in the
command handler.

### Flag Interactions

**`--only` + `--up-to`:** If a target package is assigned to a stage beyond
the `--up-to` cutoff, the pipeline returns an error:

```
Error: package 'nginx' is in stage 'system' but --up-to stops at 'foundation'.
```

**`--cascade` + `--up-to`:** Reverse dependents beyond the stage cutoff are
excluded from the build set. The cascade only includes packages in stages up
to and including the cutoff. A warning is emitted for excluded dependents:

```
Warning: skipping reverse dependent 'nginx' (stage 'system') due to --up-to foundation
```

### Pipeline::execute() Changes

In the stage iteration loop:

1. Before entering a stage, check `up_to_stage`. If the previous stage matched
   the cutoff, break.
2. Before executing a package, check `only_packages`. If set and the package is
   not in the build set, attempt a cache lookup only. If cache miss, return
   `PipelineError::UncachedDependency`. If cache hit, record the manifest and
   continue without building.
3. For `--cascade`, compute the build set before entering the loop by calling
   `RecipeGraph::transitive_dependents()` for each target package and merging
   the results into the build set. If `up_to_stage` is also set, filter the
   build set to only include packages in stages up to the cutoff.

## Feature 2: Build Log Capture

### Problem

When a Kitchen build fails, the only output is the error message from the
failed phase. There's no persistent log of what happened during configure,
make, or install. Debugging requires re-running the build and watching stdout.

### Design

`DerivationExecutor` gains a `log_dir: Option<PathBuf>` field. When set, each
build writes a log file.

**Log file path:** `{log_dir}/{package_name}-{derivation_id[..16]}.log`

**Log format:**

The log file consists of a metadata header written by the executor, followed
by the Kitchen's accumulated build log (which already uses `=== phase ===`
headers for each sub-phase), followed by a result footer.

```
=== conary derivation build log ===
package: zlib
version: 1.3.2
derivation_id: a1b2c3d4e5f6...
build_env_hash: f7a8b9c0d1e2...
timestamp: 2026-03-20T14:30:00Z
===================================

=== setup ===
<stdout/stderr>

=== configure ===
<stdout/stderr>

=== make ===
<stdout/stderr>

=== install ===
<stdout/stderr>

=== result ===
status: success
duration: 12s
output_hash: d4e5f6a7b8c9...
```

### Kitchen Output Capture

The Kitchen's `Cook` struct already captures all stdout/stderr from build
phases into `cook.log: String` via `log_build_output()`. Each phase (setup,
configure, make, check, install, post_install) is labeled with
`=== phase ===` headers. The `CookResult` struct returns this log.

**Minimal Kitchen change needed.** `Cook.log` is `pub(super)` (visible only
within the `kitchen` module). The executor lives in `derivation::executor`
and cannot access it directly. Add a `pub(crate) fn log(&self) -> &str`
accessor on `Cook` (one-line change in `cook.rs`). The executor then calls
`cook.log()` after the build phases complete (success or failure) and writes
the content to the log file with a metadata header and result footer.

**Files affected:** `executor.rs` (log writing logic) and `cook.rs` (one-line
accessor).

If the process is attached to a tty and verbose mode is enabled, the executor
also forwards the log content to stderr for live feedback after each phase.

### Retention

- On `ExecutorError::Build` (build phase failure): logs always preserved
- On build success: logs deleted unless `keep_logs` is set
- Cache hits: no log produced (nothing was built)
- Non-build errors (`PipelineError::Compose`, `PipelineError::MissingRecipe`,
  etc.): no build log exists to retain — these are pipeline-level errors, not
  package build errors

### Pipeline Integration

```rust
pub struct PipelineConfig {
    // ... existing fields ...

    /// Directory for build logs. None disables logging.
    pub log_dir: Option<PathBuf>,

    /// Preserve logs even for successful builds.
    pub keep_logs: bool,
}
```

A `PipelineEvent::BuildLogWritten { package: String, path: PathBuf }` event is
emitted when a log file is preserved (either due to failure or `--keep-logs`).

### CLI

The `--keep-logs` flag goes on the `bootstrap run` variant (shown in Feature
1's CLI section).

Default log directory: `.conary/bootstrap/logs/` relative to the working
directory. Created automatically on first build.

## Feature 3: Shell on Failure (`--shell-on-failure`)

### Problem

When a build fails, the only option is to read the error, edit the recipe, and
re-run. There's no way to inspect the exact build environment where the failure
occurred — the sysroot, source tree, partially-built objects, and environment
variables are all torn down.

### Design

When `--shell-on-failure` is set and a Kitchen phase fails, the following
sequence occurs inside `DerivationExecutor::execute()` (same module as
`CleanupGuard`, since the guard is a private struct):

1. The build error is captured but not yet returned.
2. The `CleanupGuard` is temporarily disarmed (`guard.disarm()`) to prevent
   DESTDIR removal during the shell session.
3. Failure details and log file path are printed to stderr.
4. An interactive shell is spawned with:
   - Working directory: the source/build directory
   - Environment: same variables Kitchen set (`PATH`, `DESTDIR`, `PREFIX`,
     `MAKEFLAGS`, etc.)
   - The composefs sysroot remains mounted (the pipeline keeps
     `BuildEnvironment` mounted for the full stage duration)
5. The user debugs, then exits the shell.
6. After the shell process exits, the DESTDIR is explicitly cleaned up via
   `std::fs::remove_dir_all()` (since the guard was disarmed). The build
   error is returned.

**TTY guard:** If no tty is detected (checked via `std::io::stdin().is_terminal()`
from the `is-terminal` crate or `libc::isatty`), `--shell-on-failure` logs a
warning and proceeds without spawning a shell. This prevents hanging in
non-interactive environments.

**Shell selection:** Uses `$SHELL` if set, falls back to `/bin/bash`, then
`/bin/sh`.

**Implementation scope:** The shell spawning logic lives inside `executor.rs`
(same module as `CleanupGuard`). It does not enter the mount namespace of the
sandbox — it simply has the sysroot path available and the build artifacts
intact. Full namespace re-entry (chroot into the composefs mount) is a future
enhancement.

### CLI

The `--shell-on-failure` flag goes on the `bootstrap run` variant (shown in
Feature 1's CLI section).

Output on failure:

```
[FAILED] gcc-15.2.0 at simmer phase
  Error: make[3]: *** [Makefile:342: all] Error 2
  Build log: .conary/bootstrap/logs/gcc-f7a8b9c0.log
  Sysroot: /tmp/conary-work/sysroot
  DESTDIR: /tmp/conary-work/cas/build-f7a8b9c0d1e2f3a4

  Dropping into build environment. Exit shell to continue.
bash-5.2#
```

### PipelineConfig

```rust
pub struct PipelineConfig {
    // ... existing fields ...

    /// Spawn interactive shell on build failure.
    pub shell_on_failure: bool,
}
```

The flag flows from `PipelineConfig` through to `DerivationExecutor::execute()`.

## Feature 4: Recipe Audit

### Problem

The CAS-layered build model enforces that only declared dependencies exist in
the build environment. A recipe with missing `makedepends` will fail at build
time with cryptic "command not found" errors. There's no way to check a recipe
for missing dependencies before building.

### Design

Two analysis levels behind one command.

### Level 1: Static Analysis (default)

Parses the recipe's build script sections (configure, make, install, check)
and scans for known patterns:

**Tool detection:** Regex patterns for common build tools:
- `pkg-config`, `cmake`, `meson`, `ninja`, `scons`
- `python3`, `python`, `perl`, `m4`, `ruby`
- `autoconf`, `automake`, `libtool`, `autoreconf`
- `bison`, `flex`, `yacc`, `lex`
- `gettext`, `intltool`, `msgfmt`
- `install-info`, `makeinfo`, `texinfo`
- `nasm`, `yasm`
- `cargo`, `go`, `rustc`

**Library detection:**
- `-l<name>` in LDFLAGS, LIBS, or make/configure commands
- `pkg-config --cflags <name>` / `pkg-config --libs <name>`
- `find_package(<Name>)` / `find_library(<name>)` in cmake patterns

**Cross-reference:** Each detected tool/library is mapped to a package name
(using a built-in mapping table, e.g., `pkg-config` -> `pkgconf` or
`pkg-config`, `cmake` -> `cmake`). Declared `makedepends` and `requires` are
checked. Anything found but undeclared is reported.

**False positive handling:** Some tools are part of the base build environment
(e.g., `make`, `gcc`, `binutils`, `coreutils`, `bash`, `sed`, `grep`, `awk`,
`gzip`, `tar`). These are in a built-in allowlist and not reported.

### Level 2: Build-Time Tracing (`--trace`)

Runs the actual build in the derivation sandbox with `strace -f -e
trace=openat,execve` attached. Captures all file access.

**Post-build analysis:**
1. Filter accessed paths to those under the sysroot (ignore /proc, /dev, /tmp).
2. Map each accessed file to the package that owns it (from the composed
   sysroot's output manifests).
3. Report packages whose files were accessed but aren't in `makedepends` or
   `requires`.

**Caveats:**
- Requires a built sysroot (at least one full pipeline run must have completed).
- Uses the same `BuildEnvironment` and `DerivationExecutor` as a normal build.
- `strace` must be available in the host environment. `trace_audit()` checks
  for `strace` in `$PATH` at startup and returns `AuditError::StraceMissing`
  if not found.
- Slower than static analysis (runs the full build).
- `--all --trace` builds every recipe sequentially — this can take hours.
  The CLI prints a warning and estimated time before proceeding. Not
  recommended for routine use; prefer `--all` (static) for batch checks
  and `--trace` for individual recipes that fail static analysis.

### Output

```
$ conary recipe-audit recipes/system/zlib.toml

Static analysis of zlib-1.3.2:
  [WARN] 'pkg-config' used in configure but not in makedepends
  [OK]   1 declared makedepend verified

  1 potential missing dependency found.
  Run with --trace for build-time verification.

$ conary recipe-audit recipes/system/zlib.toml --trace

Build-time trace of zlib-1.3.2:
  Building in sandbox with strace... done (12s)

  Accessed packages not in makedepends:
    [WARN] pkg-config (accessed /usr/bin/pkg-config)

  Declared makedepends verified:
    [OK]   glibc (accessed 14 files)

  1 undeclared build dependency found.
  Suggested fix: add 'pkg-config' to makedepends in recipes/system/zlib.toml
```

### CLI Structure

The existing CLI has `Cook` and `ConvertPkgbuild` as top-level variants in the
main `Commands` enum — there is no `Recipe` subcommand group. Rather than
restructure the CLI, recipe audit is added as a new top-level variant:

```rust
/// Audit a recipe for missing dependencies
#[command(name = "recipe-audit")]
RecipeAudit {
    /// Path to recipe file, or --all for all recipes
    recipe: Option<String>,

    /// Audit all recipes in the recipes/ directory
    #[arg(long)]
    all: bool,

    /// Run build-time tracing (slower, more thorough)
    #[arg(long)]
    trace: bool,
},
```

Usage:

```
conary recipe-audit recipes/system/zlib.toml
conary recipe-audit recipes/system/zlib.toml --trace
conary recipe-audit --all
conary recipe-audit --all --trace
```

**Recipe discovery for `--all`:** Recursively scans the `recipes/` directory
(relative to the current working directory) for `*.toml` files. Each file is
parsed as a `Recipe` and audited. Files that fail to parse are reported as
warnings and skipped.

### Module Structure

New module: `conary-core/src/recipe/audit.rs`

- `pub fn static_audit(recipe: &Recipe) -> Result<AuditReport, AuditError>`
  — pattern-based analysis (can fail if recipe has no build section)
- `pub fn trace_audit(recipe: &Recipe, ...) -> Result<AuditReport,
  AuditError>` — build + strace (can fail: strace missing, build failure,
  sysroot not built)
- `AuditReport` struct with findings, each typed as `Missing`, `Verified`, or
  `Ignored` (in allowlist)
- `AuditError` enum: `StraceMissing`, `BuildFailed(String)`,
  `NoSysroot`, `RecipeParse(String)`
- Tool-to-package mapping table as a const array

CLI implementation in `src/commands/recipe_audit.rs`.

## Summary

| Feature | Complexity | Core Files Modified | Core Files Created | CLI Files Modified |
|---------|-----------|--------------------|--------------------|-------------------|
| Partial builds | Medium | `pipeline.rs` | — | `cli/bootstrap.rs`, `commands/bootstrap/` |
| Build log capture | Medium | `executor.rs`, `pipeline.rs`, `cook.rs` (accessor) | — | `cli/bootstrap.rs` |
| Shell on failure | Small | `executor.rs` | — | `cli/bootstrap.rs` |
| Recipe audit | Large | — | `recipe/audit.rs` | `cli/mod.rs`, `commands/recipe_audit.rs` |

All four features are independent. Partial builds and build log capture modify
`PipelineConfig` but in non-overlapping ways. Shell-on-failure touches the
executor's error path within `executor.rs` (where `CleanupGuard` lives).
Recipe audit is entirely new code.

### Recommended Build Order

1. **Build log capture** — small, enables all subsequent debugging. Uses
   existing `cook.log` — only `executor.rs` changes.
2. **Partial builds** — highest impact feature, uses logs for feedback.
3. **Shell-on-failure** — builds on log capture (shows log path in failure
   message). Contained within `executor.rs`.
4. **Recipe audit** — independent, largest scope, can ship separately.
