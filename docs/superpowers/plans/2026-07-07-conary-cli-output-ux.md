# Conary CLI Output UX Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make what a person running `conary` sees quiet by default and visually consistent, and keep it that way with automated enforcement.

**Architecture:** Two independent pillars plus enforcement. Pillar A gives the CLI its own quiet-by-default `tracing` init (leaving the `remi` server untouched) with a global `--verbose`/`--quiet` control. Pillar B adds a single `ui` module that owns one status vocabulary, backed by `console` for TTY/`NO_COLOR`-aware color. A CI guardrail plus exact-output tests stop regressions.

**Tech Stack:** Rust (edition 2024), `clap` (workspace), `tracing-subscriber` (workspace), `console` 0.16 (new direct dep for `apps/conary`, already transitive via `indicatif`), `std::process::Command` integration tests using `CARGO_BIN_EXE_conary`.

## Global Constraints

- Scope is the `apps/conary` CLI only. Do **not** change `remi` or `conaryd` logging behavior.
- Output stays **ASCII-only**. No emoji, no Unicode symbol glyphs, no TUI.
- Color backend is **`console`** only. Do not add `anstyle`, `owo-colors`, or `colored`.
- Do **not** migrate the ~2,000-site general `println!` long tail. Migration is bounded to the guarded status vocabulary plus the named daily-driver commands.
- The **guarded status vocabulary** is exactly these literals: bracket tags `[OK] [FAIL] [FAILED] [WARN] [WARNING] [ERROR] [COMPLETE] [VALID] [DONE]` (case-insensitive) and any printed string beginning `Warning:`. The top-level `Error:` prose reporter in `app.rs` is **not** in this set and is left as-is.
- Level precedence (highest wins): `RUST_LOG` env var → `-q`/`--verbose` flags → default `warn`.
- Global verbose flag is `--verbose` (long only, repeatable count). It has **no `-v` short** because `-v` is already `--version` on `install`/`update`/`list` and 11 other subcommands. `-q`/`--quiet` is free and keeps its short.
- `conary-bootstrap` is edition 2024, `rust-version = "1.96"`.
- Every `git commit` in this plan ends with the trailer line:
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`
- Work happens on branch `cli-output-ux` (already created; the spec commit is its first commit).

---

## Task 1: Verbosity directive helper

Pure function mapping `--quiet`/`--verbose` counts to a `tracing` filter directive. Isolated and fully unit-testable with no I/O.

**Files:**
- Create: `apps/conary/src/logging.rs`
- Modify: `apps/conary/src/lib.rs` (add `pub mod logging;`)
- Test: inline `#[cfg(test)]` in `apps/conary/src/logging.rs`

**Interfaces:**
- Produces: `pub fn verbosity_directive(quiet: bool, verbose: u8) -> &'static str`

- [ ] **Step 1: Write the failing test**

Create `apps/conary/src/logging.rs`:

```rust
//! CLI logging helpers.

/// Map the global `--quiet` / `--verbose` flags to a `tracing` EnvFilter
/// directive. `RUST_LOG`, when set, overrides this at init time.
pub fn verbosity_directive(quiet: bool, verbose: u8) -> &'static str {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_maps_to_error() {
        assert_eq!(verbosity_directive(true, 0), "error");
    }

    #[test]
    fn default_is_warn() {
        assert_eq!(verbosity_directive(false, 0), "warn");
    }

    #[test]
    fn verbose_counts_escalate() {
        assert_eq!(verbosity_directive(false, 1), "info");
        assert_eq!(verbosity_directive(false, 2), "debug");
        assert_eq!(verbosity_directive(false, 3), "trace");
        assert_eq!(verbosity_directive(false, 9), "trace");
    }
}
```

Add `pub mod logging;` to `apps/conary/src/lib.rs` after the existing `pub mod live_host_safety;` line.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p conary --lib logging:: 2>&1 | tail -20`
Expected: panics with `not yet implemented` (the `todo!()`).

- [ ] **Step 3: Write minimal implementation**

Replace the `todo!()` body:

```rust
pub fn verbosity_directive(quiet: bool, verbose: u8) -> &'static str {
    if quiet {
        return "error";
    }
    match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p conary --lib logging:: 2>&1 | tail -20`
Expected: `test result: ok. 3 passed`.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/logging.rs apps/conary/src/lib.rs
git commit -m "feat(cli): add verbosity_directive helper"
```

---

## Task 2: Split tracing init in conary-bootstrap

Give the CLI a compact, level-controllable init while keeping the server's behavior byte-for-byte. Rename the existing function so intent is explicit and update the `remi` caller.

**Files:**
- Modify: `crates/conary-bootstrap/src/lib.rs:5-13`
- Modify: `apps/remi/src/bin/remi.rs:255`

**Interfaces:**
- Produces: `pub fn init_server_tracing()` (identical to today's `init_tracing`)
- Produces: `pub fn init_cli_tracing(default_directive: &str)` (compact: no timestamp, no target)

- [ ] **Step 1: Replace the tracing functions**

In `crates/conary-bootstrap/src/lib.rs`, replace the existing `init_tracing` (lines 5-13) with:

```rust
/// Server-style tracing: full format (timestamp + target), default level
/// `info`, honoring `RUST_LOG`. Used by long-running daemons where verbose
/// stderr logging is captured by journald.
pub fn init_server_tracing() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
}

/// CLI-style tracing: compact format (no timestamp, no target) so that only
/// genuinely user-relevant logs reach the terminal. `default_directive` is the
/// fallback filter (e.g. "warn"); `RUST_LOG`, when set, overrides it.
pub fn init_cli_tracing(default_directive: &str) {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .without_time()
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_directive)),
        )
        .init();
}
```

- [ ] **Step 2: Update the remi caller**

In `apps/remi/src/bin/remi.rs:255`, change:

```rust
    conary_bootstrap::init_tracing();
```

to:

```rust
    conary_bootstrap::init_server_tracing();
```

- [ ] **Step 3: Verify the workspace still builds**

Run: `cargo build -p conary-bootstrap -p remi 2>&1 | tail -5`
Expected: `Finished` with no errors. (`apps/conary` still references the old name and is fixed in Task 3; build it there.)

- [ ] **Step 4: Commit**

```bash
git add crates/conary-bootstrap/src/lib.rs apps/remi/src/bin/remi.rs
git commit -m "refactor(bootstrap): split server and cli tracing init"
```

---

## Task 3: Wire global verbosity flags and quiet-by-default

Add `--verbose`/`--quiet` to the top-level CLI, reorder `app.rs` to parse before initializing tracing, and prove the log flood is gone by default.

**Files:**
- Modify: `apps/conary/src/cli/mod.rs:139-155` (add fields to `Cli`)
- Modify: `apps/conary/src/app.rs:9-19` (`run()`)
- Test: `apps/conary/tests/logging_verbosity.rs`

**Interfaces:**
- Consumes: `crate::logging::verbosity_directive` (Task 1), `conary_bootstrap::init_cli_tracing` (Task 2)
- Consumes: `Cli.quiet: bool`, `Cli.verbose: u8`

- [ ] **Step 1: Write the failing integration test**

Create `apps/conary/tests/logging_verbosity.rs`:

```rust
mod common;

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(args)
        .env_remove("RUST_LOG")
        .output()
        .expect("failed to run conary")
}

#[test]
fn list_is_quiet_by_default() {
    let (_tmp, db) = common::setup_command_test_db();
    let out = run(&["list", "--db-path", &db]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("INFO"),
        "default stderr should carry no INFO logs, got:\n{stderr}"
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "No packages found.\n");
}

#[test]
fn verbose_restores_info_logs() {
    let (_tmp, db) = common::setup_command_test_db();
    let out = run(&["--verbose", "list", "--db-path", &db]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("INFO"),
        "--verbose should surface INFO logs, got:\n{stderr}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p conary --test logging_verbosity 2>&1 | tail -25`
Expected: FAIL. `list_is_quiet_by_default` fails because stderr still contains `INFO` (current default), and the crate may fail to compile until Task 3's code changes land — either failure counts as red.

- [ ] **Step 3: Add the CLI flags**

In `apps/conary/src/cli/mod.rs`, inside `pub struct Cli { ... }` (after the `allow_live_system_mutation` field, before `#[command(subcommand)]`), add:

```rust
    /// Increase log verbosity (repeat for more: info, debug, trace)
    #[arg(long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Silence all logs except errors
    #[arg(short = 'q', long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,
```

- [ ] **Step 4: Reorder app.rs to parse before init**

In `apps/conary/src/app.rs`, replace the body of `run()` (lines 9-19) with:

```rust
pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    conary_bootstrap::init_cli_tracing(crate::logging::verbosity_directive(
        cli.quiet,
        cli.verbose,
    ));

    if cli.help_advanced {
        print!("{}", crate::cli::render_advanced_help());
        return Ok(());
    }
    conary_core::scriptlet::set_seccomp_warn_override(cli.seccomp_warn);

    dispatch::dispatch(cli).await
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p conary --test logging_verbosity 2>&1 | tail -25`
Expected: both tests PASS.

- [ ] **Step 6: Sanity-check the help still renders**

Run: `cargo run -p conary -- --help 2>&1 | grep -E 'verbose|quiet'`
Expected: shows `--verbose` and `-q, --quiet` under Options.

- [ ] **Step 7: Commit**

```bash
git add apps/conary/src/cli/mod.rs apps/conary/src/app.rs apps/conary/tests/logging_verbosity.rs
git commit -m "feat(cli): quiet-by-default logging with --verbose/--quiet"
```

---

## Task 4: The `ui` module

The single source of truth for output styling: pure `*_line` formatters (unit-testable, no I/O) plus thin printers. Color via `console`, auto-off when not a TTY or `NO_COLOR` is set.

**Files:**
- Create: `apps/conary/src/ui/mod.rs`
- Modify: `apps/conary/src/lib.rs` (add `pub mod ui;`)
- Modify: `apps/conary/Cargo.toml` (add `console` dependency)
- Test: inline `#[cfg(test)]` in `apps/conary/src/ui/mod.rs`

**Interfaces:**
- Produces (formatters returning `String`): `tag(Status) -> String`, `row_line(Status, &[&str]) -> String`, `error_line(&str) -> String`, `warn_line(&str) -> String`, `note_line(&str) -> String`, `status_line(&str, &str) -> String`, `heading_line(&str) -> String`, `field_line(&str, &str) -> String`
- Produces (printers): `error`, `warn`, `note`, `status`, `row`, `heading`, `field` (same args, return `()`)
- Produces: `pub enum Status { Ok, Fail, Warn, Skip }`

- [ ] **Step 1: Add the `console` dependency**

In `apps/conary/Cargo.toml`, under `[dependencies]` (near the `indicatif.workspace = true` line), add:

```toml
console = "0.16"
```

- [ ] **Step 2: Write the module with failing tests**

Create `apps/conary/src/ui/mod.rs`:

```rust
//! Single source of truth for user-facing CLI output styling.
//!
//! Every user-facing status tag, message prefix, and detail line goes through
//! this module so the CLI speaks one visual vocabulary. Color is applied via
//! `console`, which disables ANSI automatically when the stream is not a TTY or
//! `NO_COLOR` is set.

use console::style;

/// Per-item outcome used by [`row`]/[`row_line`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    Fail,
    Warn,
    Skip,
}

impl Status {
    /// Fixed-width (4 char) inner label, so every tag renders as width 6.
    fn inner(self) -> &'static str {
        match self {
            Status::Ok => " ok ",
            Status::Fail => "fail",
            Status::Warn => "warn",
            Status::Skip => "skip",
        }
    }
}

/// Render a fixed-width status tag, e.g. `[ ok ]`.
pub fn tag(status: Status) -> String {
    let inner = status.inner();
    let styled = match status {
        Status::Ok => style(inner).green(),
        Status::Fail => style(inner).red(),
        Status::Warn => style(inner).yellow(),
        Status::Skip => style(inner).dim(),
    };
    format!("[{styled}]")
}

/// Render a per-item row: `[ ok ]  cell1  cell2`.
pub fn row_line(status: Status, cells: &[&str]) -> String {
    format!("{}  {}", tag(status), cells.join("  "))
}

pub fn error_line(msg: &str) -> String {
    format!("{}: {msg}", style("error").red().bold())
}

pub fn warn_line(msg: &str) -> String {
    format!("{}: {msg}", style("warning").yellow().bold())
}

pub fn note_line(msg: &str) -> String {
    format!("{}: {msg}", style("note").cyan().bold())
}

pub fn status_line(verb: &str, msg: &str) -> String {
    format!("{} {msg}", style(verb).green().bold())
}

pub fn heading_line(text: &str) -> String {
    style(text).bold().to_string()
}

pub fn field_line(label: &str, value: &str) -> String {
    format!("  {}: {value}", style(label).bold())
}

// --- Printers: thin side-effecting wrappers ---

/// `error: {msg}` to stderr.
pub fn error(msg: &str) {
    eprintln!("{}", error_line(msg));
}

/// `warning: {msg}` to stderr.
pub fn warn(msg: &str) {
    eprintln!("{}", warn_line(msg));
}

/// `note: {msg}` to stderr.
pub fn note(msg: &str) {
    eprintln!("{}", note_line(msg));
}

/// Green verb line to stdout, e.g. `Installing nginx 1.27.2`.
pub fn status(verb: &str, msg: &str) {
    println!("{}", status_line(verb, msg));
}

/// Status row to stdout.
pub fn row(status: Status, cells: &[&str]) {
    println!("{}", row_line(status, cells));
}

/// Bold heading to stdout.
pub fn heading(text: &str) {
    println!("{}", heading_line(text));
}

/// Bold-label detail line to stdout.
pub fn field(label: &str, value: &str) {
    println!("{}", field_line(label, value));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Force color off so assertions compare plain text regardless of the
    /// environment the test runs in.
    fn plain() {
        console::set_colors_enabled(false);
    }

    #[test]
    fn tags_are_uniform_width() {
        plain();
        assert_eq!(tag(Status::Ok), "[ ok ]");
        assert_eq!(tag(Status::Fail), "[fail]");
        assert_eq!(tag(Status::Warn), "[warn]");
        assert_eq!(tag(Status::Skip), "[skip]");
    }

    #[test]
    fn row_joins_cells_after_tag() {
        plain();
        assert_eq!(
            row_line(Status::Ok, &["nginx", "1.27.2"]),
            "[ ok ]  nginx  1.27.2"
        );
    }

    #[test]
    fn message_prefixes_are_lowercase() {
        plain();
        assert_eq!(error_line("boom"), "error: boom");
        assert_eq!(warn_line("stale"), "warning: stale");
        assert_eq!(note_line("hint"), "note: hint");
        assert_eq!(status_line("Installing", "nginx"), "Installing nginx");
        assert_eq!(field_line("Arch", "x86_64"), "  Arch: x86_64");
        assert_eq!(heading_line("Installed packages:"), "Installed packages:");
    }
}
```

Add `pub mod ui;` to `apps/conary/src/lib.rs` after `pub mod logging;`.

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p conary --lib ui:: 2>&1 | tail -20`
Expected: `test result: ok. 3 passed`.

- [ ] **Step 4: Commit**

```bash
git add apps/conary/Cargo.toml apps/conary/src/ui/mod.rs apps/conary/src/lib.rs
git commit -m "feat(cli): add ui module for unified output vocabulary"
```

---

## Task 5: Exact-output tests for daily-driver commands

Lock the user-visible output of `list` and `search` so the later sweep and polish are provably safe. Uses inline expected strings (a lightweight snapshot) with `NO_COLOR=1` for determinism — no new test dependency.

**Files:**
- Test: `apps/conary/tests/cli_output_snapshots.rs`

**Interfaces:**
- Consumes: `common::create_test_db()` → `(TempDir, String, rusqlite::Connection)`, `common::setup_command_test_db()` → `(TempDir, String)`

- [ ] **Step 1: Write the tests capturing current output**

Create `apps/conary/tests/cli_output_snapshots.rs`:

```rust
mod common;

use conary_core::db::models::{Trove, TroveType};
use std::process::Command;

fn stdout_of(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(args)
        .env("NO_COLOR", "1")
        .env_remove("RUST_LOG")
        .output()
        .expect("failed to run conary");
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn list_empty() {
    let (_tmp, db) = common::setup_command_test_db();
    assert_eq!(stdout_of(&["list", "--db-path", &db]), "No packages found.\n");
}

#[test]
fn search_no_match() {
    let (_tmp, db) = common::setup_command_test_db();
    assert_eq!(
        stdout_of(&["search", "nonesuch", "--db-path", &db]),
        "No packages found matching 'nonesuch'\n"
    );
}

#[test]
fn list_one_package() {
    let (_tmp, db, conn) = common::create_test_db();
    let mut trove = Trove::new("nginx".to_string(), "1.27.2".to_string(), TroveType::Package);
    trove.architecture = Some("x86_64".to_string());
    trove.insert(&conn).unwrap();

    assert_eq!(
        stdout_of(&["list", "--db-path", &db]),
        "Installed packages:\n  nginx 1.27.2 (Package) [x86_64]\n\nTotal: 1 package(s)\n"
    );
}
```

- [ ] **Step 2: Run the tests to verify they pass against current output**

Run: `cargo test -p conary --test cli_output_snapshots 2>&1 | tail -20`
Expected: all three PASS. (If `Trove::new` needs a different signature, mirror the constructor used in `apps/conary/tests/cli_daily_ux.rs` — `Trove::new_with_source` — and drop the `source` argument to `InstallSource::default()` if required. Adjust only the constructor call, not the expected strings.)

- [ ] **Step 3: Commit**

```bash
git add apps/conary/tests/cli_output_snapshots.rs
git commit -m "test(cli): lock daily-driver output for list and search"
```

---

## Task 6: Migrate status-tag sites and add the guardrail

Convert every guarded-vocabulary site (89 bracket-tag occurrences across 24 files + 9 `Warning:` prints across 6 files, plus `progress.rs` phase strings) to the `ui` module, and add a test that fails if any survive outside `ui/`. The guard is the completeness gate: when it is green, the vocabulary is centralized.

**Files:**
- Create: `apps/conary/tests/output_vocabulary_guard.rs`
- Modify: every `apps/conary/src/**/*.rs` file the guard reports (drive from the test output)
- Modify: `apps/conary/src/commands/progress.rs:111-112,318-319,451-452` (phase strings `[done]`/`[FAILED: …]`)

**Interfaces:**
- Consumes: `crate::ui` (Task 4) — `ui::row`, `ui::warn`, `ui::error`, `ui::note`, `ui::Status`

**Conversion recipe** (apply per reported site):

| Current literal | Replace with |
|---|---|
| `println!("[OK] {x}")` / `[COMPLETE]` / `[VALID]` / `[DONE]` status line | `ui::row(ui::Status::Ok, &[/* columns */])` — or `ui::status("Done", x)` for a one-off verb line |
| `println!("[FAIL] {x}")` / `[FAILED]` / `[ERROR]` row | `ui::row(ui::Status::Fail, &[/* columns */])` |
| `eprintln!("[WARN] {x}")` / `[WARNING]` | `ui::warn(x)` |
| `println!("Warning: {x}")` | `ui::warn(x)` |
| a "skipped"/"already" row | `ui::row(ui::Status::Skip, &[/* columns */])` |

Keep the column content identical to today; only the tag/prefix changes. Comments that literally contain a guarded tag (e.g. `// prints [OK]`) must be reworded — the guard scans whole lines.

- [ ] **Step 1: Write the guardrail test**

Create `apps/conary/tests/output_vocabulary_guard.rs`:

```rust
//! Fails if any guarded status-vocabulary literal is emitted outside the ui
//! module. Keeps CLI output converging on one vocabulary (see the CLI output
//! UX spec). The forbidden set here must match the spec's "replaces" table.

use std::fs;
use std::path::Path;

const FORBIDDEN_TAGS: &[&str] = &[
    "[OK]", "[FAIL]", "[FAILED]", "[WARN]", "[WARNING]", "[ERROR]", "[COMPLETE]", "[VALID]",
    "[DONE]",
];

fn scan(dir: &Path, violations: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            // The ui module defines the canonical vocabulary; skip it.
            if path.file_name().is_some_and(|n| n == "ui") {
                continue;
            }
            scan(&path, violations);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = fs::read_to_string(&path).unwrap();
            for (i, line) in text.lines().enumerate() {
                let upper = line.to_uppercase();
                if FORBIDDEN_TAGS.iter().any(|t| upper.contains(t)) || line.contains("\"Warning:")
                {
                    violations.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
                }
            }
        }
    }
}

#[test]
fn no_raw_status_vocabulary_outside_ui() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    scan(&src, &mut violations);
    assert!(
        violations.is_empty(),
        "raw status vocabulary must route through the ui module ({} sites):\n{}",
        violations.len(),
        violations.join("\n")
    );
}
```

- [ ] **Step 2: Run the guard to enumerate violations (expected red)**

Run: `cargo test -p conary --test output_vocabulary_guard 2>&1 | tail -60`
Expected: FAIL, printing the list of sites to fix. This list is your worklist.

- [ ] **Step 3: Convert every reported site using the recipe**

Work through the printed list file by file, applying the conversion recipe above. Include `apps/conary/src/commands/progress.rs`: change the phase strings so `format!("{} [done]", package)` becomes `format!("{} done", package)` (the `[done]` tag is the guarded literal) and `format!("{} [FAILED: {}]", package, err)` becomes `format!("{package} failed: {err}")`. Import `crate::ui` where needed.

- [ ] **Step 4: Re-run the guard until green**

Run: `cargo test -p conary --test output_vocabulary_guard 2>&1 | tail -20`
Expected: PASS (`test result: ok`). Repeat Step 3 for any remaining hits.

- [ ] **Step 5: Confirm nothing else regressed**

Run: `cargo test -p conary 2>&1 | tail -30`
Expected: the full suite passes, including `cli_output_snapshots` (list/search have no guarded tags, so their goldens are unchanged) and `cli_daily_ux`.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/tests/output_vocabulary_guard.rs apps/conary/src
git commit -m "refactor(cli): route status vocabulary through ui module + guardrail"
```

---

## Task 7: Polish the `list` command output

`list` is the flagship read command. Route its heading through `ui::heading` and remove the `{:?}` Debug-format leak on the trove type (`(Package)` → `(package)`), then update the locked golden.

**Files:**
- Modify: `apps/conary/src/commands/query/package.rs:78-92` (`print_installed_packages`)
- Modify: `apps/conary/tests/cli_output_snapshots.rs` (`list_one_package` expected string)

**Interfaces:**
- Consumes: `crate::ui::heading` (Task 4)

- [ ] **Step 1: Update the expected golden first (TDD red)**

In `apps/conary/tests/cli_output_snapshots.rs`, change the `list_one_package` expected string from:

```rust
        "Installed packages:\n  nginx 1.27.2 (Package) [x86_64]\n\nTotal: 1 package(s)\n"
```

to:

```rust
        "Installed packages:\n  nginx 1.27.2 (package) [x86_64]\n\nTotal: 1 package(s)\n"
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p conary --test cli_output_snapshots list_one_package 2>&1 | tail -20`
Expected: FAIL — output still shows `(Package)`.

- [ ] **Step 3: Update the renderer**

In `apps/conary/src/commands/query/package.rs`, replace `print_installed_packages`:

```rust
fn print_installed_packages(troves: &[conary_core::db::models::Trove]) {
    crate::ui::heading("Installed packages:");
    for trove in troves {
        let kind = format!("{:?}", trove.trove_type).to_lowercase();
        print!("  {} {} ({})", trove.name, trove.version, kind);
        if let Some(arch) = &trove.architecture {
            print!(" [{}]", arch);
        }
        println!();
    }
    println!("\nTotal: {} package(s)", troves.len());
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p conary --test cli_output_snapshots 2>&1 | tail -20`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/src/commands/query/package.rs apps/conary/tests/cli_output_snapshots.rs
git commit -m "polish(cli): list uses ui heading and drops debug-format type"
```

---

## Task 8: Document the vocabulary and verbosity in AGENTS.md

Give contributors one canonical reference so the guardrail's rules are discoverable.

**Files:**
- Modify: `AGENTS.md` (add a "CLI output conventions" section)

- [ ] **Step 1: Find the insertion point**

Run: `grep -nE '^#{1,3} ' AGENTS.md | tail -30`
Expected: a list of section headers. Pick the end of the most relevant existing section (e.g. conventions/standards); if none is obvious, append at the end of the file.

- [ ] **Step 2: Add the conventions section**

Append this section to `AGENTS.md`:

```markdown
## CLI output conventions (apps/conary)

All user-facing status output goes through `apps/conary/src/ui/`. Do not print
raw status tags. The `output_vocabulary_guard` test enforces this.

| State | Message form | Row tag | Color |
|---|---|---|---|
| success | `ui::status(verb, msg)` (green verb) | `ui::row(Status::Ok, …)` → `[ ok ]` | green |
| failure | `ui::error(msg)` → `error: …` | `ui::row(Status::Fail, …)` → `[fail]` | red |
| warning | `ui::warn(msg)` → `warning: …` | `ui::row(Status::Warn, …)` → `[warn]` | yellow |
| skipped | — | `ui::row(Status::Skip, …)` → `[skip]` | dim |
| note | `ui::note(msg)` → `note: …` | — | cyan |

Tags are lowercase and ASCII-only; color is applied by `console` and disabled
automatically off-TTY or under `NO_COLOR`.

Logging: the CLI defaults to `warn` and is quiet. `--verbose` (repeatable) raises
to info/debug/trace; `-q`/`--quiet` drops to errors only; `RUST_LOG` overrides
both. Internal `tracing` logs must not be relied on as primary user output.
```

- [ ] **Step 3: Commit**

```bash
git add AGENTS.md
git commit -m "docs(cli): document output vocabulary and verbosity conventions"
```

---

## Self-Review

**Spec coverage:**

- Pillar A quiet logging + `init_server_tracing`/`init_cli_tracing` split → Tasks 1–3.
- `RUST_LOG` > flags > `warn` precedence → Task 1 (directive) + Task 2 (`try_from_default_env` fallback) + Task 3 (wiring), asserted in `logging_verbosity.rs`.
- `remi` unchanged → Task 2 keeps `init_server_tracing` byte-for-byte and only renames the call site.
- Global `--verbose`/`-q` with the `-v` collision documented → Task 3 + Global Constraints.
- Pillar B `ui` module + hybrid vocabulary + `console` backend + TTY/`NO_COLOR` → Task 4.
- Bounded migration (status-tag sites + daily commands) + `progress.rs` alignment → Task 6; `list` polish → Task 7; long tail explicitly out of scope → Global Constraints.
- Enforcement: guardrail → Task 6; color-stripped exact-output tests for daily commands → Task 5.
- Vocabulary documented in `AGENTS.md` → Task 8.
- Command tiering already shipped → not a task (Global Constraints / spec framing).

**Placeholder scan:** No `TBD`/`TODO` in delivered code. The only `todo!()` is the intentional TDD-red stub in Task 1 Step 1, replaced in Step 3. The Task 6 sweep is driven by the guard's concrete output plus an explicit recipe and named files rather than a per-site diff, because the guard is the machine-checkable completeness gate.

**Type consistency:** `verbosity_directive(bool, u8) -> &'static str` is defined in Task 1 and consumed with the same signature in Task 3. `init_cli_tracing(&str)` is defined in Task 2 and called with a `&'static str` in Task 3. `ui::Status`, `ui::row`, `ui::warn`, `ui::error`, `ui::heading` are defined in Task 4 and consumed in Tasks 6–7 with matching signatures. `console = "0.16"` (Task 4) matches the resolved transitive version.
