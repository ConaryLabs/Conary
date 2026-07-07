# Conary CLI Output UX Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make what a person running `conary` sees quiet by default and visually consistent, and keep it that way with automated enforcement.

**Architecture:** Two independent pillars plus enforcement. Pillar A gives the CLI its own quiet-by-default `tracing` init (leaving the `remi` server untouched) with a global `--verbose`/`--quiet` control. Pillar B adds a single `ui` module that owns one status vocabulary, backed by `console` for TTY/`NO_COLOR`-aware color. A CI guardrail plus exact-output tests stop regressions.

**Tech Stack:** Rust (edition 2024), `clap` (workspace), `tracing-subscriber` (workspace), `console` 0.16 (new direct dep for `apps/conary`, already transitive via `indicatif`), `std::process::Command` integration tests using `CARGO_BIN_EXE_conary`.

## Global Constraints

- Scope is the `apps/conary` CLI only. Do **not** change `remi` or `conaryd` logging behavior.
- Output stays **ASCII-only**. No emoji, no Unicode symbol glyphs, no TUI.
- Color backend is **`console`** only. Do not add `anstyle`, `owo-colors`, or `colored`.
- Do **not** migrate the ~2,000-site general `println!` long tail. Migration is bounded to the guarded status vocabulary.
- The **guarded status vocabulary** is exactly this word-list: `[OK] [COMPLETE] [DONE] [VALID] [FAIL] [FAILED] [ERROR] [WARN] [WARNING] [INFO] [OFF] [MISSING] [PENDING]` (case-insensitive) plus any printed string beginning `Warning:`. The top-level `Error:` prose reporter in `app.rs` is **not** in this set and is left as-is.
- The `ui` `Status` enum is `{ Ok, Fail, Warn, Skip, Info, Off, Missing, Pending }`. Tags are lowercase, bracketed, no inner padding: `[ok] [fail] [warn] [skip] [info] [off] [missing] [pending]`. `row` right-pads the tag by **visible** width to the widest tag so columns align.
- Level precedence (highest wins): `RUST_LOG` env var → `-q`/`--verbose` flags → default `warn`.
- Global verbose flag is `--verbose` (long only, repeatable count). It has **no `-v` short** because `-v` is already `--version` on `install`/`update`/`list` and 11 other subcommands. `-q`/`--quiet` is free and keeps its short.
- `conary-bootstrap` is edition 2024, `rust-version = "1.96"`.
- Every `git commit` in this plan ends with the trailer line:
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`
- Work happens on branch `cli-output-ux` (already created; the spec commit is its first commit; rebased onto current `origin/main`).

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

In `apps/remi/src/bin/remi.rs:255`, change `conary_bootstrap::init_tracing();` to `conary_bootstrap::init_server_tracing();`.

- [ ] **Step 3: Verify the workspace still builds**

Run: `cargo build -p conary-bootstrap -p remi 2>&1 | tail -5`
Expected: `Finished` with no errors. (`apps/conary` still references the old name and is fixed in Task 3.)

- [ ] **Step 4: Commit**

```bash
git add crates/conary-bootstrap/src/lib.rs apps/remi/src/bin/remi.rs
git commit -m "refactor(bootstrap): split server and cli tracing init"
```

---

## Task 3: Wire global verbosity flags and quiet-by-default

Add `--verbose`/`--quiet` to the top-level CLI, reorder `app.rs` to parse before initializing tracing, and prove the log flood is gone by default.

**Files:**
- Modify: `apps/conary/src/cli/mod.rs` (add fields to `Cli`, after `allow_live_system_mutation`, before `#[command(subcommand)]`)
- Modify: `apps/conary/src/app.rs` (`run()`)
- Test: `apps/conary/tests/logging_verbosity.rs`

**Interfaces:**
- Consumes: `crate::logging::verbosity_directive` (Task 1), `conary_bootstrap::init_cli_tracing` (Task 2), `Cli.quiet: bool`, `Cli.verbose: u8`

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
Expected: FAIL (stderr still contains `INFO`, and/or the crate does not yet compile because `app.rs` references the renamed function). Either is red.

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

In `apps/conary/src/app.rs`, replace the body of `run()` with:

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

The single source of truth for output styling: pure `*_line` formatters (unit-testable, no I/O) plus thin printers. Color via `console`, auto-off when not a TTY or `NO_COLOR` is set. Eight-state `Status` enum; `row` column-aligns variable-width tags.

**Files:**
- Create: `apps/conary/src/ui/mod.rs`
- Modify: `apps/conary/src/lib.rs` (add `pub mod ui;`)
- Modify: `apps/conary/Cargo.toml` (add `console` dependency)
- Test: inline `#[cfg(test)]` in `apps/conary/src/ui/mod.rs`

**Interfaces:**
- Produces: `pub enum Status { Ok, Fail, Warn, Skip, Info, Off, Missing, Pending }`
- Produces (formatters → `String`): `tag(Status)`, `row_line(Status, &[&str])`, `error_line(&str)`, `warn_line(&str)`, `note_line(&str)`, `status_line(&str, &str)`, `heading_line(&str)`, `field_line(&str, &str)`
- Produces (printers → `()`): `error`, `warn`, `note`, `status`, `row`, `heading`, `field`

- [ ] **Step 1: Add the `console` dependency**

In `apps/conary/Cargo.toml`, under `[dependencies]` (near `indicatif.workspace = true`), add:

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

/// Per-item indicator used by [`row`]/[`row_line`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    Fail,
    Warn,
    Skip,
    Info,
    Off,
    Missing,
    Pending,
}

impl Status {
    /// Lowercase word shown inside the brackets.
    fn label(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Fail => "fail",
            Status::Warn => "warn",
            Status::Skip => "skip",
            Status::Info => "info",
            Status::Off => "off",
            Status::Missing => "missing",
            Status::Pending => "pending",
        }
    }
}

/// Width of the widest rendered tag, `[missing]` / `[pending]` (9 columns).
/// Used by [`row_line`] to align the column after the tag.
const TAG_COLUMN: usize = 9;

/// Render a colored status tag, e.g. `[ok]`.
pub fn tag(status: Status) -> String {
    let inner = status.label();
    let styled = match status {
        Status::Ok => style(inner).green(),
        Status::Fail => style(inner).red(),
        Status::Warn => style(inner).yellow(),
        Status::Skip => style(inner).dim(),
        Status::Info => style(inner).cyan(),
        Status::Off => style(inner).dim(),
        Status::Missing => style(inner).red(),
        Status::Pending => style(inner).yellow(),
    };
    format!("[{styled}]")
}

/// Render a per-item row with the tag column padded so cells align:
/// `[ok]       nginx  1.27.2`.
pub fn row_line(status: Status, cells: &[&str]) -> String {
    // Pad by the *visible* width (`[label]`), never the styled string, so ANSI
    // codes do not throw off alignment.
    let visible = status.label().len() + 2;
    let pad = TAG_COLUMN.saturating_sub(visible);
    format!("{}{}  {}", tag(status), " ".repeat(pad), cells.join("  "))
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
    fn tags_are_lowercase_bracketed_words() {
        plain();
        assert_eq!(tag(Status::Ok), "[ok]");
        assert_eq!(tag(Status::Fail), "[fail]");
        assert_eq!(tag(Status::Warn), "[warn]");
        assert_eq!(tag(Status::Skip), "[skip]");
        assert_eq!(tag(Status::Info), "[info]");
        assert_eq!(tag(Status::Off), "[off]");
        assert_eq!(tag(Status::Missing), "[missing]");
        assert_eq!(tag(Status::Pending), "[pending]");
    }

    #[test]
    fn rows_align_regardless_of_tag_width() {
        plain();
        let short = row_line(Status::Ok, &["alpha"]);
        let long = row_line(Status::Missing, &["beta"]);
        // The cell starts at the same column in both rows.
        assert_eq!(short.find("alpha"), long.find("beta"));
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
git commit -m "feat(cli): add ui module with 8-state status vocabulary"
```

---

## Task 5: Exact-output tests for daily-driver commands

Lock the user-visible output of `list` and `search` so the later sweep is provably safe. Inline expected strings (a lightweight snapshot) with `NO_COLOR=1` for determinism — no new test dependency.

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
Expected: all three PASS. (If `Trove::new` has a different arity in your tree, mirror the constructor used in `apps/conary/tests/cli_daily_ux.rs`. Adjust only the constructor call, never the expected strings — those are the lock.)

- [ ] **Step 3: Commit**

```bash
git add apps/conary/tests/cli_output_snapshots.rs
git commit -m "test(cli): lock daily-driver output for list and search"
```

---

## Task 6: Migrate warnings + add the guardrail scaffold

First migration family: warnings are unambiguously single-state, so they are the safe place to introduce the guard. Add the guard with only the warning literals, convert every warning site, and end green.

**Files:**
- Create: `apps/conary/tests/output_vocabulary_guard.rs`
- Modify: the ~33 files/lines emitting `Warning:` / `[WARN]` / `[WARNING]` / `[warning]` (drive from the guard output)

**Interfaces:**
- Consumes: `crate::ui::warn` (Task 4)

**Conversion recipe:** `println!("Warning: {x}")`, `eprintln!("[WARN] {x}")`, `[WARNING]`, `[warning]` → `crate::ui::warn(&x)` (drop the tag/prefix; `ui::warn` supplies `warning:`). Reword any comment that literally contains a listed word.

- [ ] **Step 1: Write the guardrail test (warning literals only)**

Create `apps/conary/tests/output_vocabulary_guard.rs`:

```rust
//! Fails if a guarded status-vocabulary literal is emitted outside the ui
//! module. This is a line-level lint: it matches the exact listed words only.
//! The list grows as migration families land (warnings, then errors, then
//! success/info/state), so every migration step ends with the guard green.

use std::fs;
use std::path::Path;

/// Guarded literals. Extended by later migration tasks.
const FORBIDDEN: &[&str] = &["[WARN]", "[WARNING]"];

fn scan(dir: &Path, violations: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "ui") {
                continue; // the ui module defines the canonical vocabulary
            }
            scan(&path, violations);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = fs::read_to_string(&path).unwrap();
            for (i, line) in text.lines().enumerate() {
                let upper = line.to_uppercase();
                let hit = FORBIDDEN.iter().any(|t| upper.contains(t))
                    || line.contains("\"Warning:");
                if hit {
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

- [ ] **Step 2: Run the guard to enumerate warning sites (expected red)**

Run: `cargo test -p conary --test output_vocabulary_guard 2>&1 | tail -60`
Expected: FAIL, printing the warning sites. This is your worklist.

- [ ] **Step 3: Convert every reported site with the recipe**

Work the printed list file by file, replacing each with `crate::ui::warn(...)`. Import `crate::ui` where needed.

- [ ] **Step 4: Re-run the guard until green**

Run: `cargo test -p conary --test output_vocabulary_guard 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Confirm nothing else regressed**

Run: `cargo test -p conary 2>&1 | tail -30`
Expected: full suite passes, including `cli_output_snapshots` and `cli_daily_ux`.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/tests/output_vocabulary_guard.rs apps/conary/src
git commit -m "refactor(cli): route warnings through ui::warn + guard"
```

---

## Task 7: Migrate errors (extend the guard)

**Files:**
- Modify: `apps/conary/tests/output_vocabulary_guard.rs` (extend `FORBIDDEN`)
- Modify: the ~18 files/lines emitting `[FAIL]` / `[FAILED]` / `[ERROR]`

**Interfaces:**
- Consumes: `crate::ui::error`, `crate::ui::row`, `crate::ui::Status` (Task 4)

**Conversion recipe:** a one-off error message → `crate::ui::error(&x)` (→ `error: x`). A per-item failure row in a list → `crate::ui::row(ui::Status::Fail, &[/* same columns */])`. An enum arm that returns a bare tag string (e.g. `DerivedStatus::Error => "[ERROR]"`) → return `ui::tag(ui::Status::Fail)`.

- [ ] **Step 1: Extend the guard (TDD red)**

In `apps/conary/tests/output_vocabulary_guard.rs`, change `FORBIDDEN` to:

```rust
const FORBIDDEN: &[&str] = &["[WARN]", "[WARNING]", "[FAIL]", "[FAILED]", "[ERROR]"];
```

- [ ] **Step 2: Run the guard to enumerate error sites (expected red)**

Run: `cargo test -p conary --test output_vocabulary_guard 2>&1 | tail -60`
Expected: FAIL, listing the `[FAIL]`/`[FAILED]`/`[ERROR]` sites.

- [ ] **Step 3: Convert every reported site with the recipe**

- [ ] **Step 4: Re-run guard + full suite until green**

Run: `cargo test -p conary --test output_vocabulary_guard 2>&1 | tail -20`
Then: `cargo test -p conary 2>&1 | tail -30`
Expected: both green.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/tests/output_vocabulary_guard.rs apps/conary/src
git commit -m "refactor(cli): route errors through ui + extend guard"
```

---

## Task 8: Migrate success / info / state (extend the guard, align progress.rs)

The largest and most entangled family: success tags plus the multi-state indicators (`[OK]` paired with `[OFF]`/`[MISSING]`, `[COMPLETE]` paired with `[PENDING]`). This is where the 8-state `Status` enum earns its keep — convert *both* sides of each indicator so no raw tag is left next to a styled one.

**Files:**
- Modify: `apps/conary/tests/output_vocabulary_guard.rs` (extend `FORBIDDEN`)
- Modify: the ~54 success/info/state sites, notably:
  - `apps/conary/src/commands/federation.rs:198` — `if peer.enabled { "[OK]" } else { "[OFF]" }`
  - `apps/conary/src/commands/bootstrap/setup.rs:51` — `if present { "[OK]" } else { "[MISSING]" }`
  - `apps/conary/src/commands/bootstrap/setup.rs:108` — `if complete { "[COMPLETE]" } else { "[PENDING]" }`
  - `apps/conary/src/commands/derived.rs:36` — enum arm returning `"[ERROR]"`-style tags (convert the whole set of arms)
  - `apps/conary/src/commands/progress.rs` — phase strings (see recipe)

**Interfaces:**
- Consumes: `crate::ui::{row, tag, status, note, Status}` (Task 4)

**Conversion recipe:**

| Legacy | Replace with |
|---|---|
| `[OK]` / `[COMPLETE]` / `[DONE]` / `[VALID]` success row | `ui::row(ui::Status::Ok, &[…])` |
| `[OK]`/`[OFF]` indicator | `ui::tag(if cond { ui::Status::Ok } else { ui::Status::Off })` |
| `[OK]`/`[MISSING]` indicator | `ui::tag(if cond { ui::Status::Ok } else { ui::Status::Missing })` |
| `[COMPLETE]`/`[PENDING]` indicator | `ui::tag(if cond { ui::Status::Ok } else { ui::Status::Pending })` |
| `[INFO]` line | `ui::note(&x)` (message) or `ui::row(ui::Status::Info, &[…])` (row) |
| one-off success verb (e.g. "Cooked: x") | `ui::status("Cooked", &x)` |

For `progress.rs`: `format!("{} [done]", package)` → `format!("{package} done")`; `format!("{} [FAILED: {}]", package, err)` → `format!("{package} failed: {err}")`. Note the guard does **not** catch `[FAILED: …]` (colon before `]`) — convert it by hand while you are in the file; the `[done]` form *is* guard-caught via `[DONE]`.

- [ ] **Step 1: Extend the guard to the full word-list (TDD red)**

In `apps/conary/tests/output_vocabulary_guard.rs`, change `FORBIDDEN` to:

```rust
const FORBIDDEN: &[&str] = &[
    "[OK]", "[COMPLETE]", "[DONE]", "[VALID]", "[FAIL]", "[FAILED]", "[ERROR]", "[WARN]",
    "[WARNING]", "[INFO]", "[OFF]", "[MISSING]", "[PENDING]",
];
```

- [ ] **Step 2: Run the guard to enumerate remaining sites (expected red)**

Run: `cargo test -p conary --test output_vocabulary_guard 2>&1 | tail -80`
Expected: FAIL, listing the success/info/state sites (including the entangled indicators above).

- [ ] **Step 3: Convert every reported site with the recipe**

Convert both branches of each paired indicator so no raw tag remains beside a styled one. Include `progress.rs` per the recipe.

- [ ] **Step 4: Re-run guard + full suite until green**

Run: `cargo test -p conary --test output_vocabulary_guard 2>&1 | tail -20`
Then: `cargo test -p conary 2>&1 | tail -30`
Expected: both green.

- [ ] **Step 5: Commit**

```bash
git add apps/conary/tests/output_vocabulary_guard.rs apps/conary/src
git commit -m "refactor(cli): route success/info/state tags through ui + full guard"
```

---

## Task 9: De-Debug the `list` type column

`list` is the flagship read command. Its type column uses `{:?}` (Debug) — replace it with the type's own `as_str()`. `TroveType` is a strum `AsRefStr`, so `as_str()` returns `"Package"`: output is unchanged, but the fragile Debug-in-output is gone. Route the heading through `ui::heading`.

**Files:**
- Modify: `apps/conary/src/commands/query/package.rs` (`print_installed_packages`)

**Interfaces:**
- Consumes: `crate::ui::heading` (Task 4); `TroveType::as_str()` (existing)

- [ ] **Step 1: Update the renderer**

In `apps/conary/src/commands/query/package.rs`, replace `print_installed_packages`:

```rust
fn print_installed_packages(troves: &[conary_core::db::models::Trove]) {
    crate::ui::heading("Installed packages:");
    for trove in troves {
        print!(
            "  {} {} ({})",
            trove.name,
            trove.version,
            trove.trove_type.as_str()
        );
        if let Some(arch) = &trove.architecture {
            print!(" [{}]", arch);
        }
        println!();
    }
    println!("\nTotal: {} package(s)", troves.len());
}
```

- [ ] **Step 2: Run the locked snapshot to prove output is unchanged**

Run: `cargo test -p conary --test cli_output_snapshots 2>&1 | tail -20`
Expected: all PASS — `as_str()` yields `"Package"` and `ui::heading` under `NO_COLOR` yields the bare text, so the golden `(Package)` / `Installed packages:` still match.

- [ ] **Step 3: Commit**

```bash
git add apps/conary/src/commands/query/package.rs
git commit -m "polish(cli): list heading via ui, drop debug-format type column"
```

---

## Task 10: Document the vocabulary and verbosity in AGENTS.md

Give contributors one canonical reference so the guardrail's rules are discoverable.

**Files:**
- Modify: `AGENTS.md`

- [ ] **Step 1: Find the insertion point**

Run: `grep -nE '^#{1,3} ' AGENTS.md | tail -30`
Expected: section headers. Pick the end of the most relevant conventions section; if none is obvious, append at the end.

- [ ] **Step 2: Add the conventions section**

Append to `AGENTS.md`:

```markdown
## CLI output conventions (apps/conary)

All user-facing status output goes through `apps/conary/src/ui/`. Do not print
raw status tags. The `output_vocabulary_guard` test enforces this.

`Status { Ok, Fail, Warn, Skip, Info, Off, Missing, Pending }` — tags render
lowercase and bracketed (`[ok]`, `[fail]`, `[warn]`, `[skip]`, `[info]`,
`[off]`, `[missing]`, `[pending]`); `ui::row` aligns the column after the tag.

| Use | Call |
|---|---|
| warning message | `ui::warn(msg)` → `warning: …` |
| error message | `ui::error(msg)` → `error: …` |
| note / info message | `ui::note(msg)` → `note: …` |
| success verb line | `ui::status(verb, msg)` (green verb) |
| per-item row | `ui::row(Status::_, &[cells])` |
| section heading | `ui::heading(text)` |
| key/value line | `ui::field(label, value)` |

Tags are ASCII-only; color is applied by `console` and disabled automatically
off-TTY or under `NO_COLOR`.

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
- Pillar B `ui` module, 8-state `Status`, column-aligned tags, `console` backend → Task 4.
- Bounded migration of the full guarded vocabulary, by family → Tasks 6 (warnings), 7 (errors), 8 (success/info/state + `progress.rs`); entangled indicators explicitly enumerated in Task 8.
- Enforcement: guardrail introduced in Task 6 and grown in Tasks 7–8 (each ends green); color-stripped exact-output tests for `list`/`search` → Task 5.
- `list` Debug-format cleanup → Task 9. Vocabulary documented in `AGENTS.md` → Task 10.
- Command tiering already shipped → not a task (Global Constraints / spec framing).

**Placeholder scan:** No `TBD`/`TODO` in delivered code. The only `todo!()` is the intentional TDD-red stub in Task 1. The Tasks 6–8 sweeps are driven by the guard's concrete output plus an explicit recipe and named files, because the guard is the machine-checkable completeness gate; the entangled sites are named individually in Task 8.

**Type consistency:** `verbosity_directive(bool, u8) -> &'static str` (Task 1) consumed identically in Task 3. `init_cli_tracing(&str)` (Task 2) called with `&'static str` in Task 3. `Status` (8 variants), `tag`, `row`, `warn`, `error`, `note`, `status`, `heading` defined in Task 4 and consumed with matching signatures in Tasks 6–10. `TroveType::as_str()` (Task 9) is the existing strum `AsRefStr` method. The guard's `FORBIDDEN` list only ever grows and is a superset at each step (warnings ⊂ +errors ⊂ +success/info/state). `console = "0.16"` (Task 4) matches the resolved transitive version 0.16.3.
