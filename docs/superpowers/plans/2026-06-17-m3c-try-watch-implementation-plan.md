# M3c Try Watch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `conary try --watch` as a namespace-only package-authoring loop that cooks on meaningful source changes, refreshes the active try session, and preserves the last successful generation when a refresh fails.

**Architecture:** Watch orchestration lives in `apps/conary/src/commands/try_session/watch.rs`, source identity and debounce live in `watch_source.rs`, and try-session lifecycle internals stay in `session.rs` plus `namespace.rs`. Cook integration uses an in-process adapter so watch writes one operation record for the whole process and can force offline-cache-only refreshes for hermetic builds without adding hidden public CLI flags. Refresh commit is two-phase: stage a new package/DB/generation/namespace exposure, switch visible namespace state recoverably, then update the active session row through an active-only expected-generation compare-and-swap.

**Tech Stack:** Rust 2024, `anyhow`, `serde`, `serde_json`, `tokio`, `uuid`, `rusqlite`, existing `conary-core::diagnostics`, existing hermetic source identity and cook APIs, existing try-session namespace/test fixtures, `cargo test`.

---

## Scope Locks

M3c includes:

- `conary try --watch`, `conary try --watch .`, `conary try --watch --recipe path/to/recipe.toml`, and `conary try --watch --json`.
- Namespace-only watch sessions.
- Parser/dispatch refusal for `.ccs` artifacts, action words, `--activate`, `--allow-irreversible`, and trailing run commands when `--watch` is set.
- Polling-first source wakeups, with canonical source identity deciding whether a refresh is meaningful.
- Explicit `WatchSourceSet` handling for recipe files, local source roots, local additional source files, local patch files, and inferred source trees.
- In-process cook adapter with suppressed per-refresh operation records and offline-cache-only refresh policy for hermetic watch refreshes.
- Non-hermetic watch refreshes preserve the current cook source policy; M3c does not silently turn host iteration builds into hermetic builds.
- One active watch session row whose `work_dir` remains stable for startup, refresh, status, keep refusal, rollback, and cleanup.
- Watch-created session marker file under the stable try-session `work_dir`, written fail-closed before the session is externally keepable.
- `conary try keep` refusal for watch-created sessions without a schema migration.
- Active-only expected-generation compare-and-swap for refresh commits.
- Redacted NDJSON event streaming for `--json`.
- One bounded, redacted packaging operation record per watch process.
- Focused CLI integration coverage in `apps/conary/tests/packaging_m3c.rs`.

M3c excludes:

- Watch mode for prebuilt `.ccs` artifacts.
- `--watch --activate`, `--watch --allow-irreversible`, `--watch -- <command>`, and action-word watch management.
- Auto-keep, auto-publish, record mode, persistent watch sessions after process exit, remote build services, and MCP watch tools.
- DB schema migrations.
- A `notify`/file-watcher dependency in this slice.
- Reworking cook into a new package-building subsystem.

## File Structure

Create:

- `apps/conary/src/commands/try_session/watch.rs`: watch command options, loop state, event sequence builder, human/NDJSON rendering, cook/refresh orchestration, cancellation, and operation-record finalization.
- `apps/conary/src/commands/try_session/watch_source.rs`: `WatchSourceSet`, source identity hashing, debounce state, symlink-escape validation, polling test helpers, and source-set unit tests.
- `apps/conary/tests/packaging_m3c.rs`: end-to-end watch CLI tests with deterministic test-only exit controls.

Modify:

- `apps/conary/src/cli/mod.rs`: add `--watch`, `--recipe`, and `--json` to `try`; update parser/help tests.
- `apps/conary/src/dispatch/root.rs`: route watch before package/action routing and preserve normal try-management actions.
- `apps/conary/src/command_risk.rs`: classify watch as local state mutation before activated-try classification.
- `apps/conary/src/commands/try_session/mod.rs`: declare `watch` and `watch_source`, expose `cmd_try_watch`, extend `TryStartRequest`, add refresh request/outcome types.
- `apps/conary/src/commands/try_session/session.rs`: add watch marker startup support, keep refusal, and staged refresh orchestration.
- `apps/conary/src/commands/try_session/namespace.rs`: add staged exposure paths and recoverable stable namespace switch helpers.
- `apps/conary/src/commands/cook.rs`: add an in-process watch cook adapter and source-policy override; keep `cmd_cook` behavior unchanged.
- `apps/conary/src/commands/diagnostics.rs`: expose per-event redaction/NDJSON helpers and bounded watch-event retention.
- `apps/conary/src/commands/operation_records.rs`: no new storage format; use existing writer from watch finalization.
- `crates/conary-core/src/db/models/try_session.rs`: add active-only expected-generation replacement helper.
- `crates/conary-core/src/diagnostics/mod.rs`: add watch event kinds and diagnostic codes.
- `crates/conary-core/src/recipe/hermetic/source_identity.rs`: expose reusable symlink-escape validation for canonical local file lists.
- `crates/conary-core/src/recipe/kitchen/local_source.rs`: call the shared symlink validator to preserve current local-source behavior.
- `docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md`: mark M3c landed after implementation passes.
- `docs/modules/feature-ownership.md`: add watch-mode start-here files and proof commands after implementation passes.
- `docs/llms/subsystem-map.md`: route packaging watch work to `try_session/watch.rs` after implementation passes.

Maintainability boundaries:

- `apps/conary/src/cli/mod.rs` is over 1500 lines. This plan allows only try-flag fields and parser/help tests there.
- `apps/conary/src/dispatch/root.rs` is over 1500 lines. This plan allows only try-watch routing and dispatch tests there.
- `apps/conary/src/commands/cook.rs` is over 1500 lines. This plan allows one narrow in-process adapter and source-policy override; watch loop logic stays out.
- `apps/conary/src/commands/try_session/session.rs` owns try-session lifecycle. This plan adds marker and refresh lifecycle helpers there, while watch orchestration stays in `watch.rs`.
- `apps/conary/src/commands/try_session/namespace.rs` owns namespace exposure. This plan adds staged exposure and recoverable switching there, while DB row updates stay in `session.rs`.

Focused verification commands:

```bash
cargo test -p conary-core diagnostics
cargo test -p conary-core recipe::hermetic
cargo test -p conary-core recipe::kitchen::local_source
cargo test -p conary-core db::models::try_session
cargo test -p conary --lib cli::tests
cargo test -p conary --lib command_risk::tests
cargo test -p conary --lib dispatch::root
cargo test -p conary --lib commands::diagnostics::tests
cargo test -p conary --lib commands::try_session
cargo test -p conary --lib commands::cook
cargo test -p conary --test packaging_m1b
cargo test -p conary --test packaging_m3a
cargo test -p conary --test packaging_m3c
cargo fmt --check
```

Merge gate:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Review lock mapping:

| Review concern | Plan owner |
|----------------|------------|
| Refresh commit can destroy last-good namespace | Task 7 staged exposure and recoverable switch tests |
| Refresh updates orphaned or externally completed sessions | Task 5 active-only expected-generation CAS and Task 7 CAS miss handling |
| Child cook adapter would write per-refresh records and lacks offline controls | Task 4 in-process adapter with suppressed records and source-policy override |
| Watch marker is created after session becomes keepable | Task 6 marker creation inside `begin_try_session` before active row commit |
| Source identity misses recipe/patch/additional-source edits | Task 3 explicit `WatchSourceSet` |
| Watch records can grow unbounded or leak secrets | Task 2 event redaction and bounded retention, Task 8 record finalization |
| Active mount roots cannot be renamed or deleted as transient staging | Task 7 generational mount directories and `mount --move` switch contract |

---

### Task 1: CLI Parser, Dispatch, Risk, And Watch Stub

**Files:**
- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/dispatch/root.rs`
- Modify: `apps/conary/src/command_risk.rs`
- Modify: `apps/conary/src/commands/try_session/mod.rs`
- Create: `apps/conary/src/commands/try_session/watch.rs`
- Test: `apps/conary/src/cli/mod.rs`
- Test: `apps/conary/src/dispatch/root.rs`
- Test: `apps/conary/src/command_risk.rs`

- [ ] **Step 1: Write failing CLI parser tests**

In `apps/conary/src/cli/mod.rs`, replace the M1b watch refusal assertion inside `try_package_parses` with a dedicated parser test:

```rust
#[test]
fn try_watch_parses_project_recipe_and_json_forms() {
    let default_target = Cli::try_parse_from(["conary", "try", "--watch"]).unwrap();
    match default_target.command {
        Some(Commands::Try {
            target,
            watch,
            recipe,
            json,
            activate,
            allow_irreversible,
            run,
            ..
        }) => {
            assert_eq!(target, None);
            assert!(watch);
            assert_eq!(recipe, None);
            assert!(!json);
            assert!(!activate);
            assert!(!allow_irreversible);
            assert!(run.is_empty());
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let recipe = Cli::try_parse_from([
        "conary",
        "try",
        "--watch",
        ".",
        "--recipe",
        "packaging/recipe.toml",
        "--json",
    ])
    .unwrap();
    match recipe.command {
        Some(Commands::Try {
            target,
            watch,
            recipe,
            json,
            ..
        }) => {
            assert_eq!(target.as_deref(), Some("."));
            assert!(watch);
            assert_eq!(recipe.as_deref(), Some("packaging/recipe.toml"));
            assert!(json);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}
```

Update `publish_help_exposes_attested_artifact_form` so try help now expects landed watch flags and still rejects record mode:

```rust
let try_help = subcommand_help("try");
assert!(try_help.contains("--watch"));
assert!(try_help.contains("--recipe"));
assert!(try_help.contains("--json"));
assert!(!try_help.contains("--record"));
```

- [ ] **Step 2: Run the failing CLI parser test**

Run:

```bash
cargo test -p conary --lib try_watch_parses_project_recipe_and_json_forms
```

Expected: fail because the `Try` command does not yet have `watch`, `recipe`, or `json` fields.

- [ ] **Step 3: Add try watch flags to the CLI**

In the `Commands::Try` variant in `apps/conary/src/cli/mod.rs`, add these fields before `run`:

```rust
        /// Watch a recipe project or inferable source tree and refresh a namespace try session
        #[arg(long)]
        watch: bool,

        /// Recipe file to use for watch mode
        #[arg(long)]
        recipe: Option<String>,

        /// Stream watch events as newline-delimited JSON
        #[arg(long)]
        json: bool,
```

Update every `Commands::Try { ... }` pattern in tests and code to bind `watch`, `recipe`, and `json` or use `..`.

- [ ] **Step 4: Write failing dispatch tests for watch routing and conflicts**

In `apps/conary/src/dispatch/root.rs`, add tests inside the existing test module:

```rust
#[test]
fn try_dispatch_watch_defaults_to_current_dir() {
    match super::try_dispatch_action(None, false, false, &[], true, None, false).unwrap() {
        super::TryDispatchAction::Watch(watch) => {
            assert_eq!(watch.target, ".");
            assert_eq!(watch.recipe, None);
            assert!(!watch.json);
        }
        other => panic!("unexpected try dispatch action: {other:?}"),
    }
}

#[test]
fn try_dispatch_watch_rejects_artifacts_actions_activation_and_run_commands() {
    for (target, activate, allow_irreversible, run, message) in [
        (Some("pkg.ccs".to_string()), false, false, vec![], "does not accept prebuilt .ccs artifacts"),
        (Some("status".to_string()), false, false, vec![], "cannot be combined with try action"),
        (Some("rollback".to_string()), false, false, vec![], "cannot be combined with try action"),
        (Some("keep".to_string()), false, false, vec![], "cannot be combined with try action"),
        (None, true, false, vec![], "cannot be combined with --activate"),
        (None, false, true, vec![], "cannot be combined with --allow-irreversible"),
        (None, false, false, vec!["/bin/true".to_string()], "cannot run a command"),
    ] {
        let err = super::try_dispatch_action(
            target,
            activate,
            allow_irreversible,
            &run,
            true,
            None,
            false,
        )
        .expect_err("watch conflict should fail");
        assert!(err.to_string().contains(message), "{err:#}");
    }
}
```

- [ ] **Step 5: Add watch dispatch and a compiling command stub**

In `apps/conary/src/dispatch/root.rs`, change the action enum and dispatch helper shape:

```rust
struct TryWatchDispatch {
    target: String,
    recipe: Option<String>,
    json: bool,
}

enum TryDispatchAction {
    Package(String),
    Watch(TryWatchDispatch),
    Status,
    Rollback,
    Keep,
}

fn try_dispatch_action(
    target: Option<String>,
    activate: bool,
    allow_irreversible: bool,
    run: &[String],
    watch: bool,
    recipe: Option<String>,
    json: bool,
) -> Result<TryDispatchAction> {
    if watch {
        if activate {
            bail!("conary try --watch cannot be combined with --activate");
        }
        if allow_irreversible {
            bail!("conary try --watch cannot be combined with --allow-irreversible");
        }
        if !run.is_empty() {
            bail!("conary try --watch cannot run a command");
        }
        let target = target.unwrap_or_else(|| ".".to_string());
        if is_reserved_try_action(&target) {
            bail!("conary try --watch cannot be combined with try action '{target}'");
        }
        if target.ends_with(".ccs") {
            bail!("conary try --watch does not accept prebuilt .ccs artifacts");
        }
        return Ok(TryDispatchAction::Watch(TryWatchDispatch {
            target,
            recipe,
            json,
        }));
    }

    match target {
        Some(target)
            if is_reserved_try_action(&target)
                && !activate
                && !allow_irreversible
                && run.is_empty() =>
        {
            Ok(match target.as_str() {
                "status" => TryDispatchAction::Status,
                "rollback" => TryDispatchAction::Rollback,
                "keep" => TryDispatchAction::Keep,
                _ => unreachable!("reserved try action checked above"),
            })
        }
        Some(target) => Ok(TryDispatchAction::Package(target)),
        None => bail!("conary try requires a package artifact or one of: status, rollback, keep"),
    }
}
```

Update the `Commands::Try` route to call `commands::cmd_try_watch` for `TryDispatchAction::Watch`.

Create `apps/conary/src/commands/try_session/watch.rs`:

```rust
// apps/conary/src/commands/try_session/watch.rs
//! Watch-mode try-session orchestration.

use anyhow::{Result, bail};

pub(super) struct TryWatchOptions<'a> {
    pub(super) db_path: &'a str,
    pub(super) target: &'a str,
    pub(super) recipe: Option<&'a str>,
    pub(super) json: bool,
}

pub(super) async fn cmd_try_watch(_options: TryWatchOptions<'_>) -> Result<()> {
    bail!("conary try --watch is not wired yet")
}
```

In `apps/conary/src/commands/try_session/mod.rs`, add `mod watch;` and expose:

```rust
pub(crate) async fn cmd_try_watch(
    db_path: &str,
    target: &str,
    recipe: Option<&str>,
    json: bool,
) -> Result<()> {
    watch::cmd_try_watch(watch::TryWatchOptions {
        db_path,
        target,
        recipe,
        json,
    })
    .await
}
```

- [ ] **Step 6: Write and pass command-risk tests**

In `apps/conary/src/command_risk.rs`, extend `classify_try_commands_by_session_risk`:

```rust
let watch = policy(&["conary", "try", "--watch"]);
assert_eq!(watch.command_label.as_ref(), "conary try --watch");
assert_eq!(watch.risk, CommandRisk::LocalStateMutation);
assert!(!watch.requires_ack());

let invalid_activated_watch = policy(&["conary", "try", "--watch", "--activate"]);
assert_eq!(invalid_activated_watch.risk, CommandRisk::LocalStateMutation);
assert!(!invalid_activated_watch.requires_ack());
```

Update `classify_try` to receive `watch: bool` and return `local_state("conary try --watch")` before the activated branch when `watch` is true.

Run:

```bash
cargo test -p conary --lib cli::tests
cargo test -p conary --lib command_risk::tests::classify_try_commands_by_session_risk
cargo test -p conary --lib dispatch::root::tests::try_dispatch_watch
```

Expected: pass.

- [ ] **Step 7: Commit CLI and routing foundation**

```bash
git add apps/conary/src/cli/mod.rs apps/conary/src/dispatch/root.rs apps/conary/src/command_risk.rs apps/conary/src/commands/try_session/mod.rs apps/conary/src/commands/try_session/watch.rs
git commit -m "feat(try): add watch CLI routing foundation"
```

### Task 2: Watch Diagnostics, Event Redaction, And Bounded Records

**Files:**
- Modify: `crates/conary-core/src/diagnostics/mod.rs`
- Modify: `apps/conary/src/commands/diagnostics.rs`
- Test: `crates/conary-core/src/diagnostics/mod.rs`
- Test: `apps/conary/src/commands/diagnostics.rs`

- [ ] **Step 1: Write failing serialization tests for watch events and diagnostics**

In `crates/conary-core/src/diagnostics/mod.rs`, add:

```rust
#[test]
fn watch_event_kinds_and_diagnostics_serialize_as_kebab_case() {
    let event = PackagingEvent {
        schema_version: PACKAGING_JSON_SCHEMA_VERSION,
        operation_id: "watch-1".to_string(),
        sequence: 1,
        phase: PackagingPhase::TrySession,
        kind: PackagingEventKind::WatchRefreshSucceeded,
        message: Some("refreshed try generation 42".to_string()),
        diagnostic: None,
        artifact: None,
        progress: None,
    };
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["kind"], "watch-refresh-succeeded");

    let diagnostic = PackagingDiagnostic::error(
        PackagingPhase::TrySession,
        PackagingDiagnosticCode::WatchTryRefreshFailed,
        "refresh failed",
    );
    let value = serde_json::to_value(&diagnostic).unwrap();
    assert_eq!(value["code"], "watch-try-refresh-failed");
}
```

- [ ] **Step 2: Add additive watch event kinds and diagnostic codes**

Extend `PackagingDiagnosticCode` with:

```rust
    WatchCookFailed,
    WatchTryRefreshFailed,
    WatchCleanupFailed,
    WatchSourceIdentityFailed,
    TryWatchUnsupported,
```

Extend `PackagingEventKind` with:

```rust
    WatchStarted,
    WatchDebounced,
    WatchRefreshStarted,
    WatchRefreshSkipped,
    WatchRefreshSucceeded,
    WatchRefreshFailed,
    WatchCancelled,
```

Run:

```bash
cargo test -p conary-core diagnostics::tests::watch_event_kinds_and_diagnostics_serialize_as_kebab_case
```

Expected: pass.

- [ ] **Step 3: Write failing per-event redaction and retention tests**

In `apps/conary/src/commands/diagnostics.rs`, add:

```rust
#[test]
fn packaging_event_ndjson_redacts_diagnostic_before_serializing() {
    let diagnostic = PackagingDiagnostic::error(
        PackagingPhase::Build,
        PackagingDiagnosticCode::WatchCookFailed,
        "failed with API_TOKEN=secret",
    )
    .with_evidence(DiagnosticEvidence::log(
        "build log",
        "Authorization: Bearer abc.def",
    ));
    let event = PackagingEvent::diagnostic("watch-1", 3, diagnostic);

    let line = render_packaging_event_ndjson(&event).unwrap();

    assert!(line.ends_with('\n'));
    assert!(!line.contains("API_TOKEN=secret"), "{line}");
    assert!(!line.contains("abc.def"), "{line}");
    assert!(line.contains("\"schema_version\""), "{line}");
    assert!(line.contains("\"redactions\""), "{line}");
}

#[test]
fn bounded_watch_events_retains_newest_events_and_records_trim_count() {
    let events = (1..=505)
        .map(|sequence| PackagingEvent {
            schema_version: PACKAGING_JSON_SCHEMA_VERSION,
            operation_id: "watch-1".to_string(),
            sequence,
            phase: PackagingPhase::TrySession,
            kind: PackagingEventKind::WatchDebounced,
            message: Some(format!("event {sequence}")),
            diagnostic: None,
            artifact: None,
            progress: None,
        })
        .collect::<Vec<_>>();

    let retained = bounded_watch_events("watch-1", &events, 500);

    assert_eq!(retained.len(), 500);
    assert_eq!(retained[0].sequence, 6);
    assert_eq!(retained[0].kind, PackagingEventKind::WatchRefreshSkipped);
    assert_eq!(
        retained[0].message.as_deref(),
        Some("5 older watch events were omitted from this operation record")
    );
    assert_eq!(retained.last().unwrap().sequence, 505);
}
```

- [ ] **Step 4: Expose event redaction, NDJSON rendering, and bounded retention**

In `apps/conary/src/commands/diagnostics.rs`, make `redact_diagnostic` and `redact_artifact` usable by event redaction inside the module, then add:

```rust
pub(crate) fn redacted_packaging_event(event: &PackagingEvent) -> PackagingEvent {
    let mut event = event.clone();
    if let Some(message) = &mut event.message {
        let redacted = redact_text(message);
        *message = redacted.value;
    }
    if let Some(diagnostic) = &mut event.diagnostic {
        redact_diagnostic(diagnostic);
    }
    if let Some(artifact) = &mut event.artifact {
        redact_artifact(artifact);
    }
    event
}

pub(crate) fn render_packaging_event_ndjson(event: &PackagingEvent) -> Result<String> {
    let mut rendered = serde_json::to_string(&redacted_packaging_event(event))?;
    rendered.push('\n');
    Ok(rendered)
}

pub(crate) fn bounded_watch_events(
    operation_id: &str,
    events: &[PackagingEvent],
    limit: usize,
) -> Vec<PackagingEvent> {
    if events.len() <= limit {
        return events.to_vec();
    }
    let omitted = events.len() - limit + 1;
    let mut retained = Vec::with_capacity(limit);
    // This synthetic event is only persisted in the final operation record. It
    // is not emitted on the live NDJSON stream, so sequence reuse cannot confuse
    // stream consumers.
    retained.push(PackagingEvent {
        schema_version: PACKAGING_JSON_SCHEMA_VERSION,
        operation_id: operation_id.to_string(),
        sequence: events[omitted].sequence,
        phase: PackagingPhase::OperationRecord,
        kind: PackagingEventKind::WatchRefreshSkipped,
        message: Some(format!(
            "{omitted} older watch events were omitted from this operation record"
        )),
        diagnostic: None,
        artifact: None,
        progress: None,
    });
    retained.extend(events[omitted + 1..].iter().cloned());
    retained
}
```

Run:

```bash
cargo test -p conary --lib commands::diagnostics::tests::packaging_event_ndjson_redacts_diagnostic_before_serializing
cargo test -p conary --lib commands::diagnostics::tests::bounded_watch_events_retains_newest_events_and_records_trim_count
```

Expected: pass.

- [ ] **Step 5: Commit diagnostics foundation**

```bash
git add crates/conary-core/src/diagnostics/mod.rs apps/conary/src/commands/diagnostics.rs
git commit -m "feat(diagnostics): add try watch events"
```

### Task 3: Watch Source Set, Canonical Identity, And Debounce

**Files:**
- Create: `apps/conary/src/commands/try_session/watch_source.rs`
- Modify: `apps/conary/src/commands/try_session/mod.rs`
- Modify: `crates/conary-core/src/recipe/hermetic/source_identity.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/local_source.rs`
- Test: `apps/conary/src/commands/try_session/watch_source.rs`
- Test: `crates/conary-core/src/recipe/hermetic/source_identity.rs`
- Test: `crates/conary-core/src/recipe/kitchen/local_source.rs`

- [ ] **Step 1: Add shared symlink-escape validator tests**

In `crates/conary-core/src/recipe/hermetic/source_identity.rs`, add a Unix test:

```rust
#[cfg(unix)]
#[test]
fn validate_canonical_file_list_rejects_symlink_escape() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "secret\n").unwrap();
    std::os::unix::fs::symlink("../outside/secret.txt", source.join("escape.txt")).unwrap();

    let files = canonical_local_file_list(&source, CiMode::Off).unwrap();
    let err = validate_canonical_local_file_list(&source, &files).unwrap_err();

    assert!(
        err.to_string()
            .contains("Local source symlink must stay within the source directory"),
        "{err}"
    );
}
```

- [ ] **Step 2: Implement shared symlink validation and reuse it in local-source materialization**

Add this public helper in `crates/conary-core/src/recipe/hermetic/source_identity.rs`:

```rust
use std::path::{Component, Path, PathBuf};

pub fn validate_canonical_local_file_list(
    root: &Path,
    files: &[CanonicalLocalFile],
) -> Result<()> {
    let root = canonical_source_root(root)?;
    for file in files {
        if file
            .relative_path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
        {
            return Err(Error::ConfigError(format!(
                "Local source entry must stay within the source directory: {}",
                file.relative_path.display()
            )));
        }
        let Some(target) = &file.symlink_target else {
            continue;
        };
        let link_parent = file
            .relative_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let resolved = if target.is_absolute() {
            target.clone()
        } else {
            root.join(link_parent).join(target)
        };
        let normalized = normalize_path_without_require_existing(&resolved);
        if !normalized.starts_with(&root) {
            return Err(Error::ConfigError(format!(
                "Local source symlink must stay within the source directory: {} -> {}",
                file.relative_path.display(),
                target.display()
            )));
        }
        if let Ok(canonical_target) = std::fs::canonicalize(&normalized)
            && !canonical_target.starts_with(&root)
        {
            return Err(Error::ConfigError(format!(
                "Local source symlink must stay within the source directory: {} -> {}",
                file.relative_path.display(),
                target.display()
            )));
        }
    }
    Ok(())
}

fn normalize_path_without_require_existing(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            component @ Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}
```

In `crates/conary-core/src/recipe/kitchen/local_source.rs`, call `validate_canonical_local_file_list(source, files)?;` at the start of `materialize_local_source_from_file_list`. Keep the existing materialization symlink tests passing.

Run:

```bash
cargo test -p conary-core recipe::hermetic::source_identity::tests::validate_canonical_file_list_rejects_symlink_escape
cargo test -p conary-core recipe::kitchen::local_source
```

Expected: pass.

- [ ] **Step 3: Write failing watch-source tests**

Create `apps/conary/src/commands/try_session/watch_source.rs` with tests first:

```rust
// apps/conary/src/commands/try_session/watch_source.rs
//! Source identity and debounce for try watch mode.

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn write_recipe(root: &std::path::Path) {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("patches")).unwrap();
        std::fs::write(root.join("src/main.txt"), "hello\n").unwrap();
        std::fs::write(root.join("patches/fix.patch"), "diff --git a/a b/a\n").unwrap();
        std::fs::write(
            root.join("recipe.toml"),
            r#"
[package]
name = "watch-demo"
version = "1.0.0"

[source]
path = "src"

[build]
install = "mkdir -p %(destdir)s/usr/share/watch-demo && cp main.txt %(destdir)s/usr/share/watch-demo/main.txt"

[patches]
files = [{ file = "patches/fix.patch", strip = 1 }]
"#,
        )
        .unwrap();
    }

    #[test]
    fn explicit_recipe_source_set_includes_recipe_local_source_and_patch() {
        let temp = tempfile::tempdir().unwrap();
        write_recipe(temp.path());

        let set = resolve_watch_source_set(
            Some(temp.path().join("recipe.toml").to_str().unwrap()),
            None,
        )
        .unwrap();

        assert_eq!(set.mode, WatchSourceMode::ExplicitRecipe);
        assert_eq!(set.recipe_path.as_deref(), Some(&temp.path().join("recipe.toml").canonicalize().unwrap()));
        assert!(set.local_roots.iter().any(|root| root.ends_with("src")));
        assert!(set.local_files.iter().any(|path| path.ends_with("patches/fix.patch")));
    }

    #[test]
    fn identity_changes_for_recipe_source_and_patch_edits() {
        let temp = tempfile::tempdir().unwrap();
        write_recipe(temp.path());
        let recipe = temp.path().join("recipe.toml");
        let set = resolve_watch_source_set(Some(recipe.to_str().unwrap()), None).unwrap();
        let first = compute_watch_identity(&set).unwrap();

        std::fs::write(temp.path().join("patches/fix.patch"), "changed patch\n").unwrap();
        let patch_changed = compute_watch_identity(&set).unwrap();
        assert_ne!(first.digest, patch_changed.digest);

        std::fs::write(recipe, std::fs::read_to_string(temp.path().join("recipe.toml")).unwrap() + "\n# change\n").unwrap();
        let recipe_changed = compute_watch_identity(&set).unwrap();
        assert_ne!(patch_changed.digest, recipe_changed.digest);
    }

    #[cfg(unix)]
    #[test]
    fn watch_source_set_rejects_symlink_escape_in_patch_file() {
        let temp = tempfile::tempdir().unwrap();
        write_recipe(temp.path());
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("escape.patch"), "secret\n").unwrap();
        std::fs::remove_file(temp.path().join("patches/fix.patch")).unwrap();
        std::os::unix::fs::symlink("../outside/escape.patch", temp.path().join("patches/fix.patch")).unwrap();

        let err = resolve_watch_source_set(
            Some(temp.path().join("recipe.toml").to_str().unwrap()),
            None,
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("must stay within the recipe directory"),
            "{err:#}"
        );
    }

    #[test]
    fn identity_changes_when_recipe_points_to_different_local_source_root() {
        let temp = tempfile::tempdir().unwrap();
        write_recipe(temp.path());
        std::fs::create_dir_all(temp.path().join("src2")).unwrap();
        std::fs::write(temp.path().join("src2/main.txt"), "hello two\n").unwrap();
        let recipe = temp.path().join("recipe.toml");
        let first_set = resolve_watch_source_set(Some(recipe.to_str().unwrap()), None).unwrap();
        let first = compute_watch_identity(&first_set).unwrap();

        let edited = std::fs::read_to_string(&recipe)
            .unwrap()
            .replace("path = \"src\"", "path = \"src2\"");
        std::fs::write(&recipe, edited).unwrap();
        let second_set = resolve_watch_source_set(Some(recipe.to_str().unwrap()), None).unwrap();
        let second = compute_watch_identity(&second_set).unwrap();

        assert_ne!(first.digest, second.digest);
    }

    #[test]
    fn debounce_coalesces_rapid_changes() {
        let start = Instant::now();
        let mut debounce = DebounceState::new(Duration::from_millis(750));
        assert_eq!(debounce.record_wakeup(start), None);
        assert_eq!(
            debounce.record_wakeup(start + Duration::from_millis(100)),
            None
        );
        assert_eq!(
            debounce.ready_at(),
            Some(start + Duration::from_millis(850))
        );
        assert!(debounce.take_ready(start + Duration::from_millis(849)).is_none());
        assert!(debounce.take_ready(start + Duration::from_millis(850)).is_some());
    }
}
```

- [ ] **Step 4: Implement `watch_source.rs`**

Add the module in `apps/conary/src/commands/try_session/mod.rs`:

```rust
mod watch_source;
```

Implement the core types and functions in `watch_source.rs`:

```rust
use std::fs::File;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use conary_core::hash::{HashAlgorithm, Hasher};
use conary_core::recipe::format::is_remote_url;
use conary_core::recipe::hermetic::source_identity::{
    CiMode, canonical_local_file_list, detect_ci_mode, validate_canonical_local_file_list,
};
use conary_core::recipe::inference::{
    CookTarget, InferenceOptions, infer_recipe_from_path, resolve_cook_target,
};
use conary_core::recipe::{Recipe, parse_recipe_file};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WatchSourceMode {
    ExplicitRecipe,
    InferredSourceTree,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WatchSourceSet {
    pub(super) mode: WatchSourceMode,
    pub(super) recipe_path: Option<PathBuf>,
    pub(super) local_roots: Vec<PathBuf>,
    pub(super) local_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WatchIdentity {
    pub(super) digest: String,
    pub(super) file_count: usize,
}

pub(super) fn resolve_watch_source_set(
    target: Option<&str>,
    recipe: Option<&str>,
) -> Result<WatchSourceSet> {
    match resolve_cook_target(target, recipe).map_err(|error| anyhow::anyhow!(error))? {
        CookTarget::RecipeFile(recipe_path) => {
            let parsed = parse_recipe_file(&recipe_path)
                .with_context(|| format!("failed to parse recipe {}", recipe_path.display()))?;
            watch_source_set_for_recipe(recipe_path, &parsed)
        }
        CookTarget::SourceTree(source_tree) => {
            if source_tree.kind != conary_core::recipe::inference::SourceTargetKind::Directory {
                bail!("conary try --watch only supports local source directories and recipe projects");
            }
            let _ = infer_recipe_from_path(
                &source_tree.root,
                InferenceOptions::for_source_root(source_tree.root.clone()),
            )
            .with_context(|| {
                format!(
                    "failed to infer recipe from watched source tree {}",
                    source_tree.root.display()
                )
            })?;
            Ok(WatchSourceSet {
                mode: WatchSourceMode::InferredSourceTree,
                recipe_path: None,
                local_roots: vec![source_tree.root],
                local_files: Vec::new(),
            })
        }
    }
}

fn watch_source_set_for_recipe(recipe_path: PathBuf, recipe: &Recipe) -> Result<WatchSourceSet> {
    let recipe_path = recipe_path.canonicalize()?;
    let recipe_dir = recipe_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut local_roots = Vec::new();
    let mut local_files = vec![recipe_path.clone()];

    if let Some(local) = recipe.local_source() {
        let source_root = local
            .resolve_against(&recipe_dir)
            .map_err(|error| anyhow::anyhow!(error))?
            .canonicalize()
            .with_context(|| "failed to canonicalize watched local source root")?;
        if !source_root.starts_with(&recipe_dir.canonicalize()?) {
            bail!("watched local source root must stay within the recipe directory");
        }
        local_roots.push(source_root);
    }

    if let Some(remote) = recipe.remote_source() {
        for additional in &remote.additional {
            let url = recipe.substitute(&additional.url, "");
            if !is_remote_url(&url) {
                local_files.push(resolve_local_recipe_file(&recipe_dir, &url)?);
            }
        }
    }

    if let Some(patches) = &recipe.patches {
        for patch in &patches.files {
            let patch_file = recipe.substitute(&patch.file, "");
            if !is_remote_url(&patch_file) {
                local_files.push(resolve_local_recipe_file(&recipe_dir, &patch_file)?);
            }
        }
    }

    local_files.sort();
    local_files.dedup();
    local_roots.sort();
    local_roots.dedup();

    Ok(WatchSourceSet {
        mode: WatchSourceMode::ExplicitRecipe,
        recipe_path: Some(recipe_path),
        local_roots,
        local_files,
    })
}

fn resolve_local_recipe_file(recipe_dir: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    if path.as_os_str().is_empty() || path.is_absolute() {
        bail!("watched local recipe file must be relative to the recipe directory: {relative}");
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
    {
        bail!("watched local recipe file must stay within the recipe directory: {relative}");
    }
    let canonical_dir = recipe_dir.canonicalize()?;
    let canonical_file = canonical_dir.join(path).canonicalize()?;
    if !canonical_file.starts_with(&canonical_dir) {
        bail!("watched local recipe file must stay within the recipe directory: {relative}");
    }
    Ok(canonical_file)
}

pub(super) fn compute_watch_identity(source_set: &WatchSourceSet) -> Result<WatchIdentity> {
    let ci_mode = detect_ci_mode();
    let mut hasher = Hasher::new(HashAlgorithm::Sha256);
    let mut file_count = 0usize;

    hasher.update(format!("{:?}\0", source_set.mode).as_bytes());
    if let Some(recipe_path) = &source_set.recipe_path {
        hasher.update(recipe_path.to_string_lossy().as_bytes());
        hasher.update(b"\0");
    }
    for root in &source_set.local_roots {
        hasher.update(root.to_string_lossy().as_bytes());
        hasher.update(b"\0");
    }
    for file in &source_set.local_files {
        hasher.update(file.to_string_lossy().as_bytes());
        hasher.update(b"\0");
    }

    for file in &source_set.local_files {
        let mut reader = File::open(file)
            .with_context(|| format!("failed to open watched file {}", file.display()))?;
        let file_hash = conary_core::hash::sha256_reader_hex(&mut reader)
            .with_context(|| format!("failed to hash watched file {}", file.display()))?;
        hasher.update(file.to_string_lossy().as_bytes());
        hasher.update(format!("sha256:{file_hash}").as_bytes());
        file_count += 1;
    }

    for root in &source_set.local_roots {
        let files = canonical_local_file_list(root, ci_mode).map_err(|error| anyhow::anyhow!(error))?;
        validate_canonical_local_file_list(root, &files).map_err(|error| anyhow::anyhow!(error))?;
        for file in files {
            hasher.update(root.to_string_lossy().as_bytes());
            hasher.update(file.relative_path.to_string_lossy().as_bytes());
            hasher.update(file.hash.as_bytes());
            if let Some(target) = file.symlink_target {
                hasher.update(target.to_string_lossy().as_bytes());
            }
            file_count += 1;
        }
    }

    let hash = hasher.finalize();
    Ok(WatchIdentity {
        digest: format!("sha256:{}", hash.value),
        file_count,
    })
}

#[derive(Debug, Clone)]
pub(super) struct DebounceState {
    delay: Duration,
    ready_at: Option<Instant>,
}

impl DebounceState {
    pub(super) fn new(delay: Duration) -> Self {
        Self {
            delay,
            ready_at: None,
        }
    }

    pub(super) fn record_wakeup(&mut self, now: Instant) -> Option<Instant> {
        self.ready_at = Some(now + self.delay);
        None
    }

    pub(super) fn ready_at(&self) -> Option<Instant> {
        self.ready_at
    }

    pub(super) fn take_ready(&mut self, now: Instant) -> Option<()> {
        if self.ready_at.is_some_and(|ready| now >= ready) {
            self.ready_at = None;
            Some(())
        } else {
            None
        }
    }
}
```

- [ ] **Step 5: Run source and debounce tests**

Run:

```bash
cargo test -p conary-core recipe::hermetic::source_identity::tests::validate_canonical_file_list_rejects_symlink_escape
cargo test -p conary-core recipe::kitchen::local_source
cargo test -p conary --lib commands::try_session::watch_source
```

Expected: pass.

- [ ] **Step 6: Commit watch source identity**

```bash
git add apps/conary/src/commands/try_session/mod.rs apps/conary/src/commands/try_session/watch_source.rs crates/conary-core/src/recipe/hermetic/source_identity.rs crates/conary-core/src/recipe/kitchen/local_source.rs
git commit -m "feat(try): add watch source identity"
```

### Task 4: In-Process Cook Adapter For Watch

**Files:**
- Modify: `apps/conary/src/commands/cook.rs`
- Test: `apps/conary/src/commands/cook.rs`

- [ ] **Step 1: Write failing cook-adapter tests**

In `apps/conary/src/commands/cook.rs`, add tests:

```rust
#[test]
fn cooked_artifact_path_extracts_single_ccs_artifact() {
    let mut output = PackagingCommandOutput::succeeded("watch-1", "conary cook");
    output.artifacts.push(PackagingArtifact {
        path: "/tmp/demo.ccs".to_string(),
        kind: Some("ccs".to_string()),
    });

    assert_eq!(
        cooked_artifact_path(&output).unwrap(),
        PathBuf::from("/tmp/demo.ccs")
    );
}

#[test]
fn watch_refresh_cook_options_force_offline_policy_for_hermetic_refresh() {
    let options = CookRunOptions {
        target: Some("."),
        recipe: None,
        output_dir: "dist",
        source_cache: "sources",
        jobs: None,
        keep_builddir: false,
        validate_only: false,
        fetch_only: false,
        explain: false,
        isolated: true,
        no_isolation: false,
        hermetic: false,
        json: true,
        operation_id: "watch-1".to_string(),
        source_download_policy_override: Some(SourceDownloadPolicy::OfflineCacheOnly),
    };

    assert_eq!(
        options.source_download_policy_override,
        Some(SourceDownloadPolicy::OfflineCacheOnly)
    );
}

#[test]
fn watch_refresh_preserves_source_policy_for_non_hermetic_refresh() {
    let options = CookForTryWatchOptions {
        target: Some("."),
        recipe: None,
        output_dir: "dist",
        source_cache: "sources",
        jobs: None,
        keep_builddir: false,
        isolated: false,
        no_isolation: false,
        hermetic: false,
        source_policy: WatchCookSourcePolicy::Refresh,
        operation_id: "watch-1".to_string(),
    };

    assert_eq!(watch_source_download_policy_override(&options), None);
}
```

- [ ] **Step 2: Add source-policy override to `CookRunOptions`**

Import `SourceDownloadPolicy` from `conary_core::recipe`, then extend `CookRunOptions`:

```rust
    source_download_policy_override: Option<SourceDownloadPolicy>,
```

Every existing CLI construction of `CookRunOptions` must set:

```rust
        source_download_policy_override: None,
```

After `KitchenConfig` is built and before `Kitchen::new`, add:

```rust
    if let Some(policy) = options.source_download_policy_override {
        config.source_download_policy = policy;
    }
```

- [ ] **Step 3: Add the watch cook adapter**

Add these public(crate) types and helpers near `CookRunOptions`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WatchCookSourcePolicy {
    Initial,
    Refresh,
}

pub(crate) struct CookForTryWatchOptions<'a> {
    pub(crate) target: Option<&'a str>,
    pub(crate) recipe: Option<&'a str>,
    pub(crate) output_dir: &'a str,
    pub(crate) source_cache: &'a str,
    pub(crate) jobs: Option<u32>,
    pub(crate) keep_builddir: bool,
    pub(crate) isolated: bool,
    pub(crate) no_isolation: bool,
    pub(crate) hermetic: bool,
    pub(crate) source_policy: WatchCookSourcePolicy,
    pub(crate) operation_id: String,
}

pub(crate) fn run_cook_for_try_watch(
    options: CookForTryWatchOptions<'_>,
) -> Result<PackagingCommandOutput> {
    let source_download_policy_override = watch_source_download_policy_override(&options);
    let mut sink = io::sink();
    run_cook_operation(
        CookRunOptions {
            target: options.target,
            recipe: options.recipe,
            output_dir: options.output_dir,
            source_cache: options.source_cache,
            jobs: options.jobs,
            keep_builddir: options.keep_builddir,
            validate_only: false,
            fetch_only: false,
            explain: false,
            isolated: options.isolated,
            no_isolation: options.no_isolation,
            hermetic: options.hermetic,
            json: true,
            operation_id: options.operation_id,
            source_download_policy_override,
        },
        &mut sink,
    )
}

fn watch_source_download_policy_override(
    options: &CookForTryWatchOptions<'_>,
) -> Option<SourceDownloadPolicy> {
    let hermetic_requested = options.hermetic || options.isolated;
    if hermetic_requested && options.source_policy == WatchCookSourcePolicy::Refresh {
        Some(SourceDownloadPolicy::OfflineCacheOnly)
    } else {
        None
    }
}

pub(crate) fn cooked_artifact_path(output: &PackagingCommandOutput) -> Result<PathBuf> {
    let artifacts = output
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind.as_deref() == Some("ccs"))
        .collect::<Vec<_>>();
    match artifacts.as_slice() {
        [artifact] => Ok(PathBuf::from(&artifact.path)),
        [] => anyhow::bail!("watch cook completed without a CCS artifact"),
        _ => anyhow::bail!("watch cook produced multiple CCS artifacts"),
    }
}
```

This adapter must not call `write_packaging_record_if_possible`.
It must not force `OfflineCacheOnly` for non-hermetic refreshes; that would
change the host-iteration build mode beyond the reviewed M3c design.

- [ ] **Step 4: Run cook tests**

Run:

```bash
cargo test -p conary --lib commands::cook::tests::cooked_artifact_path_extracts_single_ccs_artifact
cargo test -p conary --lib commands::cook::tests::watch_refresh_cook_options_force_offline_policy_for_hermetic_refresh
cargo test -p conary --lib commands::cook
```

Expected: pass.

- [ ] **Step 5: Commit cook adapter**

```bash
git add apps/conary/src/commands/cook.rs
git commit -m "feat(cook): add try watch adapter"
```

### Task 5: Active-Only Try-Generation Compare-And-Swap

**Files:**
- Modify: `crates/conary-core/src/db/models/try_session.rs`
- Test: `crates/conary-core/src/db/models/try_session.rs`

- [ ] **Step 1: Write failing model tests**

Add tests:

```rust
#[test]
fn replace_active_try_generation_updates_only_matching_active_generation() {
    let (_temp, conn) = create_test_db();
    let session = create_namespace_session(&conn, "try-a");
    session.set_try_generation(&conn, 41).unwrap();

    let replaced = session
        .replace_active_try_generation(&conn, 41, "/tmp/new.ccs", 42)
        .unwrap();

    assert!(replaced);
    let stored = TrySession::find_by_id(&conn, "try-a").unwrap().unwrap();
    assert_eq!(stored.package_path, "/tmp/new.ccs");
    assert_eq!(stored.try_generation_id, Some(42));
    assert_eq!(stored.status, TrySessionStatus::Active);
}

#[test]
fn replace_active_try_generation_refuses_stale_or_non_active_rows() {
    let (_temp, conn) = create_test_db();
    let session = create_namespace_session(&conn, "try-a");
    session.set_try_generation(&conn, 41).unwrap();

    assert!(
        !session
            .replace_active_try_generation(&conn, 40, "/tmp/new.ccs", 42)
            .unwrap()
    );
    let stored = TrySession::find_by_id(&conn, "try-a").unwrap().unwrap();
    assert_eq!(stored.try_generation_id, Some(41));
    assert_eq!(stored.package_path, "/tmp/try-a.ccs");

    session.mark_orphaned(&conn).unwrap();
    assert!(
        !session
            .replace_active_try_generation(&conn, 41, "/tmp/new.ccs", 42)
            .unwrap()
    );
    let stored = TrySession::find_by_id(&conn, "try-a").unwrap().unwrap();
    assert_eq!(stored.status, TrySessionStatus::Orphaned);
    assert_eq!(stored.try_generation_id, Some(41));
}
```

- [ ] **Step 2: Implement active-only CAS helper**

Add this method to `impl TrySession`:

```rust
pub fn replace_active_try_generation(
    &self,
    conn: &Connection,
    expected_try_generation_id: i64,
    package_path: &str,
    next_try_generation_id: i64,
) -> Result<bool> {
    let rows = conn.execute(
        "UPDATE try_sessions
         SET package_path = ?1,
             try_generation_id = ?2,
             updated_at = strftime('%s','now')
         WHERE id = ?3
           AND status = 'active'
           AND try_generation_id = ?4",
        params![
            package_path,
            next_try_generation_id,
            self.id,
            expected_try_generation_id
        ],
    )?;
    Ok(rows == 1)
}
```

Do not change `set_try_generation`; rollback and existing lifecycle cleanup still use the open-session helper.

- [ ] **Step 3: Run model tests**

Run:

```bash
cargo test -p conary-core db::models::try_session::tests::replace_active_try_generation
```

Expected: pass.

- [ ] **Step 4: Commit model CAS**

```bash
git add crates/conary-core/src/db/models/try_session.rs
git commit -m "feat(try): add active generation replacement"
```

### Task 6: Watch Marker Startup And Keep Refusal

**Files:**
- Modify: `apps/conary/src/commands/try_session/mod.rs`
- Modify: `apps/conary/src/commands/try_session/session.rs`
- Test: `apps/conary/src/commands/try_session/session.rs`

- [ ] **Step 1: Write failing marker and keep-refusal tests**

In `apps/conary/src/commands/try_session/session.rs`, add tests near keep/rollback coverage:

```rust
#[test]
fn namespace_watch_start_writes_marker_before_session_is_keepable() -> anyhow::Result<()> {
    let fixture = super::test_support::TryRuntimeFixture::new();
    let package = fixture.write_package(
        "watch-demo",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.0"),
    );

    let outcome = begin_try_session(TryStartRequest {
        db_path: &fixture.db_path_string,
        package_path: &package,
        activate: false,
        allow_irreversible: false,
        command: None,
        watch_marker: Some(TryWatchMarkerRequest {
            operation_id: "watch-1",
        }),
    })?;

    let marker = outcome.work_dir.join(".conary-try-watch-session.json");
    let marker_text = std::fs::read_to_string(&marker)?;
    assert!(marker_text.contains("\"operation_id\":\"watch-1\""), "{marker_text}");

    let err = keep_active_try_session(&fixture.db_path_string).unwrap_err();
    assert!(err.to_string().contains("watch-created try session"), "{err:#}");
    Ok(())
}

#[test]
fn watch_marker_write_failure_does_not_leave_active_session() {
    let fixture = super::test_support::TryRuntimeFixture::new();
    let package = fixture.write_package(
        "watch-demo",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.0"),
    );

    let _guard = EnvVarGuard::set("CONARY_TEST_TRY_WATCH_MARKER_FAIL", "1");
    let err = begin_try_session(TryStartRequest {
        db_path: &fixture.db_path_string,
        package_path: &package,
        activate: false,
        allow_irreversible: false,
        command: None,
        watch_marker: Some(TryWatchMarkerRequest {
            operation_id: "watch-1",
        }),
    })
    .unwrap_err();

    assert!(err.to_string().contains("failed to write try watch marker"), "{err:#}");
    let conn = fixture.open();
    assert!(TrySession::find_active_or_orphaned(&conn).unwrap().is_none());
}
```

- [ ] **Step 2: Extend try-start request types**

In `apps/conary/src/commands/try_session/mod.rs`, add:

```rust
#[derive(Debug, Clone, Copy)]
pub(crate) struct TryWatchMarkerRequest<'a> {
    pub(crate) operation_id: &'a str,
}
```

Extend `TryStartRequest`:

```rust
    pub watch_marker: Option<TryWatchMarkerRequest<'a>>,
```

Update all existing `TryStartRequest` construction sites to set `watch_marker: None`.

- [ ] **Step 3: Write the marker before creating the active row**

In `apps/conary/src/commands/try_session/session.rs`, add:

```rust
const TRY_WATCH_MARKER_FILE: &str = ".conary-try-watch-session.json";

#[derive(serde::Serialize)]
struct TryWatchMarker<'a> {
    schema_version: u16,
    operation_id: &'a str,
}

fn write_try_watch_marker(work_dir: &Path, marker: TryWatchMarkerRequest<'_>) -> Result<()> {
    #[cfg(test)]
    if std::env::var_os("CONARY_TEST_TRY_WATCH_MARKER_FAIL").is_some() {
        anyhow::bail!("failed to write try watch marker: forced test failure");
    }

    let path = work_dir.join(TRY_WATCH_MARKER_FILE);
    let payload = TryWatchMarker {
        schema_version: 1,
        operation_id: marker.operation_id,
    };
    let json = serde_json::to_vec(&payload)?;
    std::fs::write(&path, json)
        .with_context(|| format!("failed to write try watch marker {}", path.display()))?;
    Ok(())
}

fn is_watch_created_try_session(session: &TrySession) -> bool {
    Path::new(&session.work_dir)
        .join(TRY_WATCH_MARKER_FILE)
        .is_file()
}
```

In `begin_try_session`, after package parsing and policy validation but before `TrySession::create_active`, call:

```rust
    if let Some(marker) = request.watch_marker {
        write_try_watch_marker(&work_dir, marker)?;
    }
```

If any failure after `create_active` requires cleanup, preserve existing rollback/orphan behavior; marker failures happen before the row exists.

- [ ] **Step 4: Refuse keep for marked watch sessions**

At the start of `keep_active_try_session_inner`, after loading the session and before mode-specific promotion:

```rust
    if is_watch_created_try_session(&session) {
        bail!(
            "cannot keep watch-created try session {}; stop watch or run `conary try rollback`",
            session.id
        );
    }
```

Run:

```bash
cargo test -p conary --lib commands::try_session::session::tests::namespace_watch_start_writes_marker_before_session_is_keepable
cargo test -p conary --lib commands::try_session::session::tests::watch_marker_write_failure_does_not_leave_active_session
cargo test -p conary --lib commands::try_session
```

Expected: pass.

- [ ] **Step 5: Commit marker behavior**

```bash
git add apps/conary/src/commands/try_session/mod.rs apps/conary/src/commands/try_session/session.rs
git commit -m "feat(try): mark watch sessions"
```

### Task 7: Staged Refresh API And Recoverable Namespace Switch

**Files:**
- Modify: `apps/conary/src/commands/try_session/mod.rs`
- Modify: `apps/conary/src/commands/try_session/session.rs`
- Modify: `apps/conary/src/commands/try_session/namespace.rs`
- Test: `apps/conary/src/commands/try_session/session.rs`
- Test: `apps/conary/src/commands/try_session/namespace.rs`

- [ ] **Step 1: Add refresh request/outcome types**

In `apps/conary/src/commands/try_session/mod.rs`, add:

```rust
#[derive(Debug, Clone, Copy)]
pub(crate) struct TryRefreshRequest<'a> {
    pub(crate) db_path: &'a str,
    pub(crate) session_id: &'a str,
    pub(crate) expected_try_generation_id: i64,
    pub(crate) package_path: &'a Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TryRefreshOutcome {
    pub(crate) previous_generation_id: i64,
    pub(crate) try_generation_id: i64,
    pub(crate) namespace_root: PathBuf,
    pub(crate) copied_package_path: PathBuf,
}
```

Expose:

```rust
pub(crate) use session::refresh_try_session;
```

- [ ] **Step 2: Write failing refresh tests**

In `apps/conary/src/commands/try_session/session.rs`, add:

```rust
#[test]
fn refresh_try_session_updates_generation_after_staging_succeeds() -> anyhow::Result<()> {
    let fixture = super::test_support::TryRuntimeFixture::new();
    let first = fixture.write_package(
        "watch-demo-a",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.0"),
    );
    let second = fixture.write_package(
        "watch-demo-b",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.1"),
    );
    let started = begin_try_session(TryStartRequest {
        db_path: &fixture.db_path_string,
        package_path: &first,
        activate: false,
        allow_irreversible: false,
        command: None,
        watch_marker: Some(TryWatchMarkerRequest {
            operation_id: "watch-1",
        }),
    })?;

    let refreshed = refresh_try_session(TryRefreshRequest {
        db_path: &fixture.db_path_string,
        session_id: &started.session_id,
        expected_try_generation_id: started.try_generation_id,
        package_path: &second,
    })?;

    assert_eq!(refreshed.previous_generation_id, started.try_generation_id);
    assert!(refreshed.try_generation_id > started.try_generation_id);
    let conn = fixture.open();
    let session = TrySession::find_by_id(&conn, &started.session_id)?.unwrap();
    assert_eq!(session.try_generation_id, Some(refreshed.try_generation_id));
    assert_eq!(Path::new(&session.work_dir), started.work_dir);
    Ok(())
}

#[test]
fn refresh_try_session_cas_miss_preserves_previous_generation() -> anyhow::Result<()> {
    let fixture = super::test_support::TryRuntimeFixture::new();
    let first = fixture.write_package(
        "watch-demo-a",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.0"),
    );
    let second = fixture.write_package(
        "watch-demo-b",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.1"),
    );
    let started = begin_try_session(TryStartRequest {
        db_path: &fixture.db_path_string,
        package_path: &first,
        activate: false,
        allow_irreversible: false,
        command: None,
        watch_marker: Some(TryWatchMarkerRequest {
            operation_id: "watch-1",
        }),
    })?;
    {
        let conn = fixture.open();
        let session = TrySession::find_by_id(&conn, &started.session_id)?.unwrap();
        session.mark_orphaned(&conn)?;
    }

    let err = refresh_try_session(TryRefreshRequest {
        db_path: &fixture.db_path_string,
        session_id: &started.session_id,
        expected_try_generation_id: started.try_generation_id,
        package_path: &second,
    })
    .unwrap_err();

    assert!(err.to_string().contains("changed outside the watcher"), "{err:#}");
    let conn = fixture.open();
    let session = TrySession::find_by_id(&conn, &started.session_id)?.unwrap();
    assert_eq!(session.try_generation_id, Some(started.try_generation_id));
    assert_eq!(session.status, conary_core::db::models::TrySessionStatus::Orphaned);
    Ok(())
}

#[test]
fn refresh_try_session_cleans_staging_after_generation_build_failure() -> anyhow::Result<()> {
    let fixture = super::test_support::TryRuntimeFixture::new();
    let first = fixture.write_package(
        "watch-demo-a",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.0"),
    );
    let second = fixture.write_package(
        "watch-demo-b",
        conary_core::ccs::manifest::CcsManifest::new_minimal("watch-demo", "1.0.1"),
    );
    let started = begin_try_session(TryStartRequest {
        db_path: &fixture.db_path_string,
        package_path: &first,
        activate: false,
        allow_irreversible: false,
        command: None,
        watch_marker: Some(TryWatchMarkerRequest {
            operation_id: "watch-1",
        }),
    })?;
    let _guard = EnvVarGuard::set(
        "CONARY_TEST_FAIL_GENERATION_REBUILD",
        "forced watch refresh generation failure",
    );

    let err = refresh_try_session(TryRefreshRequest {
        db_path: &fixture.db_path_string,
        session_id: &started.session_id,
        expected_try_generation_id: started.try_generation_id,
        package_path: &second,
    })
    .unwrap_err();

    assert!(
        err.to_string().contains("forced watch refresh generation failure"),
        "{err:#}"
    );
    let conn = fixture.open();
    let session = TrySession::find_by_id(&conn, &started.session_id)?.unwrap();
    assert_eq!(session.try_generation_id, Some(started.try_generation_id));
    assert!(
        std::fs::read_dir(&started.work_dir)?
            .filter_map(|entry| entry.ok())
            .all(|entry| !entry.file_name().to_string_lossy().starts_with("refresh-")),
        "failed refresh staging directory should be cleaned"
    );
    Ok(())
}
```

- [ ] **Step 3: Add staged namespace path helpers and mount-move tests**

In `apps/conary/src/commands/try_session/namespace.rs`, add test-only failure support and a recoverable switch helper. The stable path must remain `work_dir/namespace-root`. Real mounted paths must not live under a transient directory that will be deleted after commit. Use generational directories under the stable `work_dir`:

```text
try/<session-id>/
  namespace-root/              # stable visible mount
  namespace-root.next/         # staged visible mount before switch
  namespace-root.previous/     # temporary previous visible mount during switch
  generation-root-42/          # active composefs lowerdir for generation 42
  namespace-work-42/           # active overlay workdir for generation 42
  generation-root-43/          # staged composefs lowerdir for generation 43
  namespace-work-43/           # staged overlay workdir for generation 43
  refresh-43/                  # transient package/db/install staging only
```

The real switch must use `mount --move` for active mount points. `std::fs::rename` is allowed only in `CONARY_TEST_SKIP_GENERATION_MOUNT` test mode where namespace roots are symlink materializations rather than Linux mounts.

Add tests:

```rust
#[test]
fn switch_stable_namespace_root_restores_previous_on_forced_failure() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let stable = temp.path().join("namespace-root");
    let previous = temp.path().join("previous-root");
    let staged = temp.path().join("namespace-root.next");
    std::fs::create_dir_all(&previous)?;
    std::fs::create_dir_all(staged.parent().unwrap())?;
    std::fs::create_dir_all(&staged)?;
    std::fs::write(previous.join("marker"), "old")?;
    std::fs::write(staged.join("marker"), "new")?;
    recreate_path_symlink(&previous, &stable)?;

    let _mount_guard = EnvVarGuard::set("CONARY_TEST_SKIP_GENERATION_MOUNT", "1");
    let _fail_guard = EnvVarGuard::set(
        "CONARY_TEST_TRY_REFRESH_FAIL_NAMESPACE_SWITCH",
        "1",
    );
    let exposure = StagedNamespaceExposure {
        generation_id: 2,
        next_namespace_root: staged,
        stable_namespace_root: stable.clone(),
        previous_namespace_root: temp.path().join("namespace-root.previous"),
        generation_root: temp.path().join("generation-root-2"),
        namespace_workdir: temp.path().join("namespace-work-2"),
    };
    let err = switch_stable_namespace_root(exposure).unwrap_err();

    assert!(err.to_string().contains("failed to switch stable try namespace"), "{err:#}");
    assert_eq!(std::fs::read_link(&stable)?, previous);
    Ok(())
}
```

Add helpers:

```rust
pub(super) fn refresh_staging_dir(work_dir: &Path, next_generation_id: i64) -> PathBuf {
    work_dir.join(format!("refresh-{next_generation_id}"))
}

#[derive(Debug, Clone)]
pub(super) struct StagedNamespaceExposure {
    pub(super) generation_id: i64,
    pub(super) next_namespace_root: PathBuf,
    pub(super) stable_namespace_root: PathBuf,
    pub(super) previous_namespace_root: PathBuf,
    pub(super) generation_root: PathBuf,
    pub(super) namespace_workdir: PathBuf,
}

pub(super) fn expose_staged_try_namespace_root(
    runtime_root: &ConaryRuntimeRoot,
    work_dir: &Path,
    copied_conn: &Connection,
    try_generation_id: i64,
    hook_upperdir: &Path,
) -> Result<StagedNamespaceExposure> {
    let next_namespace_root = work_dir.join("namespace-root.next");
    let generation_root = work_dir.join(format!("generation-root-{try_generation_id}"));
    let namespace_workdir = work_dir.join(format!("namespace-work-{try_generation_id}"));

    if std::env::var_os("CONARY_TEST_SKIP_GENERATION_MOUNT").is_some() {
        materialize_test_try_namespace_root(copied_conn, runtime_root, hook_upperdir)?;
        recreate_path_symlink(hook_upperdir, &next_namespace_root)?;
    } else {
        expose_try_namespace_root_at_paths(
            runtime_root,
            copied_conn,
            try_generation_id,
            hook_upperdir,
            &generation_root,
            &namespace_workdir,
            &next_namespace_root,
        )?;
    }

    Ok(StagedNamespaceExposure {
        generation_id: try_generation_id,
        next_namespace_root,
        stable_namespace_root: work_dir.join("namespace-root"),
        previous_namespace_root: work_dir.join("namespace-root.previous"),
        generation_root,
        namespace_workdir,
    })
}

pub(super) struct NamespaceSwitch {
    exposure: StagedNamespaceExposure,
}

impl NamespaceSwitch {
    pub(super) fn commit(self) -> Result<()> {
        if std::env::var_os("CONARY_TEST_SKIP_GENERATION_MOUNT").is_some() {
            remove_path_if_exists(&self.exposure.previous_namespace_root)?;
            return Ok(());
        }
        unmount_try_path_if_mounted(&self.exposure.previous_namespace_root)?;
        remove_path_if_exists(&self.exposure.previous_namespace_root)?;
        Ok(())
    }

    pub(super) fn restore(self) -> Result<()> {
        if std::env::var_os("CONARY_TEST_SKIP_GENERATION_MOUNT").is_some() {
            remove_path_if_exists(&self.exposure.stable_namespace_root)?;
            if self.exposure.previous_namespace_root.exists() {
                std::fs::rename(
                    &self.exposure.previous_namespace_root,
                    &self.exposure.stable_namespace_root,
                )?;
            }
            return Ok(());
        }
        run_mount_move(
            &self.exposure.stable_namespace_root,
            &self.exposure.next_namespace_root,
        )?;
        run_mount_move(
            &self.exposure.previous_namespace_root,
            &self.exposure.stable_namespace_root,
        )
    }
}

pub(super) fn switch_stable_namespace_root(
    exposure: StagedNamespaceExposure,
) -> Result<NamespaceSwitch> {
    if std::env::var_os("CONARY_TEST_SKIP_GENERATION_MOUNT").is_some() {
        remove_path_if_exists(&exposure.previous_namespace_root)?;
        if exposure.stable_namespace_root.exists() {
            std::fs::rename(&exposure.stable_namespace_root, &exposure.previous_namespace_root)?;
        }
        #[cfg(test)]
        if std::env::var_os("CONARY_TEST_TRY_REFRESH_FAIL_NAMESPACE_SWITCH").is_some() {
            if exposure.previous_namespace_root.exists() {
                let _ = std::fs::rename(
                    &exposure.previous_namespace_root,
                    &exposure.stable_namespace_root,
                );
            }
            anyhow::bail!("failed to switch stable try namespace: forced test failure");
        }
        std::fs::rename(&exposure.next_namespace_root, &exposure.stable_namespace_root)?;
        return Ok(NamespaceSwitch { exposure });
    }

    std::fs::create_dir_all(&exposure.previous_namespace_root)?;
    run_mount_move(&exposure.stable_namespace_root, &exposure.previous_namespace_root)?;
    if let Err(error) = run_mount_move(&exposure.next_namespace_root, &exposure.stable_namespace_root) {
        let _ = run_mount_move(&exposure.previous_namespace_root, &exposure.stable_namespace_root);
        return Err(error.context("failed to switch stable try namespace and restored previous namespace root"));
    }
    Ok(NamespaceSwitch { exposure })
}

pub(super) fn teardown_staged_namespace_exposure(exposure: &StagedNamespaceExposure) -> Result<()> {
    if std::env::var_os("CONARY_TEST_SKIP_GENERATION_MOUNT").is_none() {
        unmount_try_path_if_mounted(&exposure.next_namespace_root)?;
        unmount_try_path_if_mounted(&exposure.generation_root)?;
    }
    remove_path_if_exists(&exposure.next_namespace_root)?;
    remove_path_if_exists(&exposure.namespace_workdir)?;
    remove_path_if_exists(&exposure.generation_root)?;
    Ok(())
}

fn run_mount_move(from: &Path, to: &Path) -> Result<()> {
    let status = std::process::Command::new("mount")
        .arg("--move")
        .arg(from)
        .arg(to)
        .status()
        .context("failed to execute mount --move for try namespace switch")?;
    if !status.success() {
        bail!(
            "failed to move try namespace mount from {} to {}",
            from.display(),
            to.display()
        );
    }
    Ok(())
}
```

`expose_try_namespace_root_at_paths` should be a small refactor of the existing `expose_try_namespace_root` body that accepts explicit lower, work, and namespace paths. The existing startup path should keep calling `expose_try_namespace_root` so normal try behavior stays stable.

- [ ] **Step 4: Implement `refresh_try_session` in session ownership**

In `apps/conary/src/commands/try_session/session.rs`, implement a refresh path that mirrors `begin_try_session` but writes into a refresh staging directory:

```rust
pub(crate) fn refresh_try_session(request: TryRefreshRequest<'_>) -> Result<TryRefreshOutcome> {
    let mut namespace_switch_started = false;
    let mut refresh_dir: Option<PathBuf> = None;
    let mut staged_namespace_cleanup: Option<namespace::StagedNamespaceExposure> = None;
    let result = (|| -> Result<TryRefreshOutcome> {
    let live_conn = conary_core::db::open(request.db_path)
        .with_context(|| format!("failed to open Conary DB {}", request.db_path))?;
    let session = TrySession::find_by_id(&live_conn, request.session_id)?
        .ok_or_else(|| anyhow::anyhow!("try watch session {} missing", request.session_id))?;
    if session.status != conary_core::db::models::TrySessionStatus::Active {
        bail!("try watch session {} changed outside the watcher", request.session_id);
    }
    if session.try_generation_id != Some(request.expected_try_generation_id) {
        bail!("try watch session {} changed outside the watcher", request.session_id);
    }
    if session.mode != TrySessionMode::Namespace {
        bail!("try watch refresh requires a namespace try session");
    }

    let runtime_root = ConaryRuntimeRoot::from_db_path(PathBuf::from(request.db_path));
    let work_dir = PathBuf::from(&session.work_dir);
    let staging_dir = namespace::refresh_staging_dir(&work_dir, request.expected_try_generation_id + 1);
    refresh_dir = Some(staging_dir.clone());
    let install_root = staging_dir.join("root");
    let copied_package_path = staging_dir.join("package.ccs");
    let copied_db_path = staging_dir.join("conary.db");
    std::fs::create_dir_all(&install_root)?;
    std::fs::copy(request.package_path, &copied_package_path)?;

    let copied_package_path_string = copied_package_path.to_string_lossy().into_owned();
    let package = <CcsPackage as PackageFormat>::parse(&copied_package_path_string)
        .map_err(|error| anyhow::anyhow!(error))?;
    validate_try_package_policy(&package, TryExecutionRoot::Namespace, false, false)?;

    vacuum_db_into(&live_conn, &copied_db_path)?;
    let mut copied_conn = conary_core::db::open(&copied_db_path)?;
    let install_plan = build_try_install_plan(
        &runtime_root,
        &staging_dir,
        copied_db_path.clone(),
        TrySessionMode::Namespace,
    );
    install_try_package(&mut copied_conn, &package, &install_plan)?;

    let summary = format!("Try {}-{}", package.name(), package.version());
    let built = crate::commands::composefs_ops::build_inactive_generation_for_runtime(
        &copied_conn,
        &runtime_root,
        &summary,
        None,
    )?;
    let hook_upperdir = promotable_try_hook_root(&runtime_root, built.generation_number)?;
    let staged_namespace = namespace::expose_staged_try_namespace_root(
        &runtime_root,
        &work_dir,
        &copied_conn,
        built.generation_number,
        &hook_upperdir,
    )?;
    staged_namespace_cleanup = Some(staged_namespace.clone());
    apply_declarative_try_hooks(package.manifest(), &staged_namespace.next_namespace_root)?;

    let stable_package_path = work_dir.join("package.ccs");
    let stable_db_path = work_dir.join("conary.db");
    let file_switch = switch_stable_try_files(
        &stable_package_path,
        &stable_db_path,
        &copied_package_path,
        &copied_db_path,
    )?;

    let stable_namespace_root = work_dir.join("namespace-root");
    let namespace_switch = namespace::switch_stable_namespace_root(staged_namespace)?;
    namespace_switch_started = true;
    staged_namespace_cleanup = None;
    let stable_package_path_string = stable_package_path.to_string_lossy().into_owned();
    let replaced = session.replace_active_try_generation(
        &live_conn,
        request.expected_try_generation_id,
        &stable_package_path_string,
        built.generation_number,
    )?;
    if !replaced {
        let _ = namespace_switch.restore();
        let _ = file_switch.restore();
        bail!("try watch session {} changed outside the watcher", request.session_id);
    }

    namespace_switch.commit()?;
    file_switch.commit()?;
    remove_dir_if_exists(staging_dir)?;

    Ok(TryRefreshOutcome {
        previous_generation_id: request.expected_try_generation_id,
        try_generation_id: built.generation_number,
        namespace_root: stable_namespace_root,
        copied_package_path: stable_package_path,
    })
    })();

    if result.is_err()
        && !namespace_switch_started
    {
        if let Some(staged_namespace) = &staged_namespace_cleanup {
            let _ = namespace::teardown_staged_namespace_exposure(staged_namespace);
        }
        if let Some(staging_dir) = refresh_dir {
            remove_dir_if_exists(staging_dir)?;
        }
    }
    result
}
```

Add `switch_stable_try_files` next to the refresh helper:

```rust
use super::util::{remove_dir_if_exists, remove_path_if_exists};

struct StableTryFileSwitch {
    package_path: PathBuf,
    package_backup: PathBuf,
    db_path: PathBuf,
    db_backup: PathBuf,
}

impl StableTryFileSwitch {
    fn commit(self) -> Result<()> {
        remove_path_if_exists(&self.package_backup)?;
        remove_path_if_exists(&self.db_backup)?;
        Ok(())
    }

    fn restore(self) -> Result<()> {
        remove_path_if_exists(&self.package_path)?;
        remove_path_if_exists(&self.db_path)?;
        if self.package_backup.exists() {
            std::fs::rename(&self.package_backup, &self.package_path)?;
        }
        if self.db_backup.exists() {
            std::fs::rename(&self.db_backup, &self.db_path)?;
        }
        Ok(())
    }
}

fn switch_stable_try_files(
    stable_package_path: &Path,
    stable_db_path: &Path,
    staged_package_path: &Path,
    staged_db_path: &Path,
) -> Result<StableTryFileSwitch> {
    let switch_id = uuid::Uuid::new_v4();
    let package_tmp = stable_package_path.with_extension(format!("{switch_id}.ccs.next"));
    let db_tmp = stable_db_path.with_extension(format!("{switch_id}.db.next"));
    let package_backup = stable_package_path.with_extension(format!("{switch_id}.ccs.previous"));
    let db_backup = stable_db_path.with_extension(format!("{switch_id}.db.previous"));

    std::fs::copy(staged_package_path, &package_tmp)?;
    std::fs::copy(staged_db_path, &db_tmp)?;
    if stable_package_path.exists() {
        std::fs::rename(stable_package_path, &package_backup)?;
    }
    if stable_db_path.exists() {
        std::fs::rename(stable_db_path, &db_backup)?;
    }
    std::fs::rename(&package_tmp, stable_package_path)?;
    std::fs::rename(&db_tmp, stable_db_path)?;

    Ok(StableTryFileSwitch {
        package_path: stable_package_path.to_path_buf(),
        package_backup,
        db_path: stable_db_path.to_path_buf(),
        db_backup,
    })
}
```

Keep the expected-generation CAS as the only session row update for the refresh.

- [ ] **Step 5: Run refresh and namespace tests**

Run:

```bash
cargo test -p conary --lib commands::try_session::namespace::tests::switch_stable_namespace_root_restores_previous_on_forced_failure
cargo test -p conary --lib commands::try_session::session::tests::refresh_try_session_updates_generation_after_staging_succeeds
cargo test -p conary --lib commands::try_session::session::tests::refresh_try_session_cas_miss_preserves_previous_generation
cargo test -p conary --lib commands::try_session::session::tests::refresh_try_session_cleans_staging_after_generation_build_failure
cargo test -p conary --lib commands::try_session
```

Expected: pass. If mount-point switching requires a different implementation from the symlink test path, keep both test-mode and real-mode behavior behind the same helper names.

- [ ] **Step 6: Commit staged refresh API**

```bash
git add apps/conary/src/commands/try_session/mod.rs apps/conary/src/commands/try_session/session.rs apps/conary/src/commands/try_session/namespace.rs
git commit -m "feat(try): add staged watch refresh"
```

### Task 8: Watch Loop, Rendering, Cancellation, And Operation Record

**Files:**
- Modify: `apps/conary/src/commands/try_session/watch.rs`
- Modify: `apps/conary/src/commands/try_session/mod.rs`
- Test: `apps/conary/src/commands/try_session/watch.rs`

- [ ] **Step 1: Write failing watch-loop unit tests**

In `apps/conary/src/commands/try_session/watch.rs`, add tests:

```rust
#[test]
fn watch_event_builder_assigns_monotonic_sequences() {
    let mut events = WatchEvents::new("watch-1");
    let first = events.push(
        PackagingPhase::TrySession,
        PackagingEventKind::WatchStarted,
        "Watching .",
    );
    let second = events.push(
        PackagingPhase::Build,
        PackagingEventKind::WatchRefreshStarted,
        "cooking",
    );

    assert_eq!(first.sequence, 1);
    assert_eq!(second.sequence, 2);
    assert_eq!(events.all().len(), 2);
}

#[test]
fn watch_record_output_is_bounded_and_redacted() {
    let mut events = WatchEvents::new("watch-1");
    for index in 0..505 {
        events.push(
            PackagingPhase::Build,
            PackagingEventKind::WatchRefreshFailed,
            format!("API_TOKEN=secret event {index}"),
        );
    }

    let output = events.into_command_output(PackagingCommandStatus::Failed, "done");
    assert_eq!(output.events.len(), 500);
    let rendered = serde_json::to_string(&crate::commands::diagnostics::redacted_packaging_output(&output)).unwrap();
    assert!(!rendered.contains("API_TOKEN=secret"), "{rendered}");
    assert!(rendered.contains("older watch events were omitted"), "{rendered}");
}

#[test]
fn watch_state_does_not_retry_same_failed_identity_without_new_changes() {
    let first = WatchIdentity {
        digest: "sha256:first".to_string(),
        file_count: 1,
    };
    let mut state = WatchRefreshState::new(first.clone(), 41);

    assert!(state.should_attempt(&first));
    state.record_attempt(first.clone());
    state.record_failure();

    assert!(
        !state.should_attempt(&first),
        "same failed source snapshot should not rebuild again until files change"
    );
    let changed = WatchIdentity {
        digest: "sha256:changed".to_string(),
        file_count: 1,
    };
    assert!(state.should_attempt(&changed));
}
```

- [ ] **Step 2: Implement event builder and command-output finalization**

In `watch.rs`, replace the stub with:

```rust
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use conary_core::diagnostics::{
    PACKAGING_JSON_SCHEMA_VERSION, PackagingCommandOutput, PackagingCommandStatus,
    PackagingDiagnostic, PackagingDiagnosticCode, PackagingEvent, PackagingEventKind,
    PackagingPhase,
};

use super::watch_source::{
    DebounceState, WatchIdentity, compute_watch_identity, resolve_watch_source_set,
};
use super::{TryRefreshRequest, TryStartRequest, TryWatchMarkerRequest};

const WATCH_EVENT_RECORD_LIMIT: usize = 500;
const DEFAULT_DEBOUNCE_MS: u64 = 750;
const DEFAULT_POLL_MS: u64 = 500;

struct WatchEvents {
    operation_id: String,
    next_sequence: u64,
    events: Vec<PackagingEvent>,
    diagnostics: Vec<PackagingDiagnostic>,
}

impl WatchEvents {
    fn new(operation_id: impl Into<String>) -> Self {
        Self {
            operation_id: operation_id.into(),
            next_sequence: 1,
            events: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn push(
        &mut self,
        phase: PackagingPhase,
        kind: PackagingEventKind,
        message: impl Into<String>,
    ) -> PackagingEvent {
        let event = PackagingEvent {
            schema_version: PACKAGING_JSON_SCHEMA_VERSION,
            operation_id: self.operation_id.clone(),
            sequence: self.next_sequence,
            phase,
            kind,
            message: Some(message.into()),
            diagnostic: None,
            artifact: None,
            progress: None,
        };
        self.next_sequence += 1;
        self.events.push(event.clone());
        event
    }

    fn diagnostic(&mut self, diagnostic: PackagingDiagnostic) -> PackagingEvent {
        let event = PackagingEvent::diagnostic(
            self.operation_id.clone(),
            self.next_sequence,
            diagnostic.clone(),
        );
        self.next_sequence += 1;
        self.diagnostics.push(diagnostic);
        self.events.push(event.clone());
        event
    }

    fn all(&self) -> &[PackagingEvent] {
        &self.events
    }

    fn into_command_output(
        self,
        status: PackagingCommandStatus,
        summary: impl Into<String>,
    ) -> PackagingCommandOutput {
        let events = crate::commands::diagnostics::bounded_watch_events(
            &self.operation_id,
            &self.events,
            WATCH_EVENT_RECORD_LIMIT,
        );
        PackagingCommandOutput {
            schema_version: PACKAGING_JSON_SCHEMA_VERSION,
            operation_id: self.operation_id,
            command: "conary try --watch".to_string(),
            status,
            diagnostics: self.diagnostics,
            events,
            artifacts: Vec::new(),
            summary: Some(summary.into()),
        }
    }
}

struct WatchRefreshState {
    last_successful_identity: WatchIdentity,
    last_attempted_identity: Option<WatchIdentity>,
    last_good_generation_id: i64,
}

impl WatchRefreshState {
    fn new(initial_identity: WatchIdentity, last_good_generation_id: i64) -> Self {
        Self {
            last_successful_identity: initial_identity,
            last_attempted_identity: None,
            last_good_generation_id,
        }
    }

    fn should_attempt(&self, current: &WatchIdentity) -> bool {
        self.last_attempted_identity.as_ref() != Some(current)
    }

    fn record_attempt(&mut self, identity: WatchIdentity) {
        self.last_attempted_identity = Some(identity);
    }

    fn record_success(&mut self, identity: WatchIdentity, generation_id: i64) {
        self.last_successful_identity = identity.clone();
        self.last_attempted_identity = Some(identity);
        self.last_good_generation_id = generation_id;
    }

    fn record_failure(&mut self) {}
}
```

- [ ] **Step 3: Implement `cmd_try_watch_with_output` with deterministic test controls**

Add a testable entrypoint:

```rust
struct WatchLoopConfig {
    poll_interval: Duration,
    debounce: Duration,
    max_refreshes: Option<usize>,
    ready_file: Option<PathBuf>,
    failure_file: Option<PathBuf>,
}

impl WatchLoopConfig {
    fn from_env() -> Self {
        let max_refreshes = std::env::var("CONARY_TEST_TRY_WATCH_EXIT_AFTER_REFRESHES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok());
        Self {
            poll_interval: Duration::from_millis(DEFAULT_POLL_MS),
            debounce: Duration::from_millis(DEFAULT_DEBOUNCE_MS),
            max_refreshes,
            ready_file: std::env::var_os("CONARY_TEST_TRY_WATCH_READY_FILE").map(PathBuf::from),
            failure_file: std::env::var_os("CONARY_TEST_TRY_WATCH_FAILURE_FILE").map(PathBuf::from),
        }
    }
}

pub(super) async fn cmd_try_watch(options: TryWatchOptions<'_>) -> Result<()> {
    let mut output = io::stdout();
    cmd_try_watch_with_output(options, WatchLoopConfig::from_env(), &mut output).await
}
```

Implement the loop so it:

- creates one `operation_id` with `operation_records::new_operation_id("try-watch")`
- cooks into a watch-owned output directory under the Conary runtime root, for example `runtime_root.root()/try/watch-cook/{operation_id}/refresh-N`
- uses `CONARY_TRY_WATCH_SOURCE_CACHE` when set, otherwise the existing cook source-cache default `/var/cache/conary/sources`
- resolves a fresh `WatchSourceSet` before every identity comparison so recipe edits that change local source roots, patch paths, or additional-source paths are reflected before deciding whether to cook
- computes initial identity before cooking
- runs `run_cook_for_try_watch` with `WatchCookSourcePolicy::Initial`
- starts a namespace try session with `watch_marker: Some(TryWatchMarkerRequest { operation_id: &operation_id })`
- writes `CONARY_TEST_TRY_WATCH_READY_FILE` after the initial try session is active when the env var is set
- polls identity on `poll_interval`
- uses `DebounceState` before rebuild
- tracks both `last_successful_identity` and `last_attempted_identity`
- skips rebuild when the current identity equals `last_attempted_identity`; this prevents repeated cook failures for the same broken source snapshot from hot-looping
- runs refresh cook with `WatchCookSourcePolicy::Refresh`
- updates `last_attempted_identity` before running cook for both success and failure paths
- recomputes identity after cook and skips stale artifacts
- calls `refresh_try_session` only after cook succeeds and identity is still current
- updates `last_successful_identity` and `last_good_generation_id` only after refresh commit succeeds
- keeps `last_good_generation_id` unchanged on cook, source identity, validation, staging, namespace, or hook failure
- writes `CONARY_TEST_TRY_WATCH_FAILURE_FILE` after a non-destructive refresh failure when the env var is set
- stops on CAS miss, cleanup failure, source root removal, cancellation, or configured `max_refreshes`
- on normal cancellation or configured `max_refreshes`, calls `rollback_active_try_session`
- writes one final redacted operation record with `write_packaging_record_if_possible`

Use this rendering helper for every emitted event:

```rust
fn write_event(
    event: &PackagingEvent,
    json: bool,
    output: &mut impl Write,
) -> Result<()> {
    if json {
        output.write_all(crate::commands::diagnostics::render_packaging_event_ndjson(event)?.as_bytes())?;
    } else if let Some(message) = &event.message {
        writeln!(output, "{message}")?;
    }
    Ok(())
}
```

For cancellation, use `tokio::signal::ctrl_c()` in the async loop. Unit tests can use `CONARY_TEST_TRY_WATCH_EXIT_AFTER_REFRESHES` rather than sending signals.

- [ ] **Step 4: Replace the Task 1 stub**

Remove the `bail!("conary try --watch is not wired yet")` stub. `cmd_try_watch` must call `cmd_try_watch_with_output`.

Run:

```bash
cargo test -p conary --lib commands::try_session::watch
cargo test -p conary --lib commands::try_session
```

Expected: pass.

- [ ] **Step 5: Commit watch loop**

```bash
git add apps/conary/src/commands/try_session/watch.rs apps/conary/src/commands/try_session/mod.rs
git commit -m "feat(try): run watch refresh loop"
```

### Task 9: End-To-End M3c Integration Tests

**Files:**
- Create: `apps/conary/tests/packaging_m3c.rs`
- Test: `apps/conary/tests/packaging_m3c.rs`

- [ ] **Step 1: Create deterministic watch integration fixture**

Create `apps/conary/tests/packaging_m3c.rs`:

```rust
// apps/conary/tests/packaging_m3c.rs

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use conary_core::db::models::{TrySession, TrySessionStatus};
use conary_core::runtime_root::ConaryRuntimeRoot;

struct WatchFixture {
    _work: tempfile::TempDir,
    source: PathBuf,
    db_path: String,
    _db_temp: tempfile::TempDir,
}

impl WatchFixture {
    fn new() -> Self {
        let work = tempfile::tempdir().unwrap();
        let root = work.path().to_path_buf();
        let source = root.join("source");
        write_watch_project(&source, "hello-one");
        let (db_temp, db_path) = common::setup_command_test_db();
        let runtime_root = ConaryRuntimeRoot::from_db_path(PathBuf::from(&db_path));
        fs::create_dir_all(runtime_root.generation_path(7)).unwrap();
        conary_core::generation::mount::update_current_symlink(runtime_root.root(), 7).unwrap();
        Self {
            _work: work,
            source,
            db_path,
            _db_temp: db_temp,
        }
    }
}

fn write_watch_project(source: &Path, message: &str) {
    fs::create_dir_all(source.join("src")).unwrap();
    fs::write(
        source.join("Cargo.toml"),
        r#"[package]
name = "watch-demo"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(
        source.join("src/main.rs"),
        format!("fn main() {{ println!(\"{message}\"); }}\n"),
    )
    .unwrap();
}

fn base_watch_command(fixture: &WatchFixture) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_conary"));
    command
        .env("CONARY_TEST_SKIP_GENERATION_MOUNT", "1")
        .env("CONARY_TEST_TRY_LAUNCHER", "echo")
        .args(["try", "--watch"])
        .arg(&fixture.source)
        .args(["--db-path", &fixture.db_path]);
    command
}

fn watch_until_test_exit(fixture: &WatchFixture, json: bool) -> Output {
    let mut command = base_watch_command(fixture);
    command.env("CONARY_TEST_TRY_WATCH_EXIT_AFTER_REFRESHES", "1");
    if json {
        command.arg("--json");
    }
    command.output().expect("failed to run conary try --watch")
}

fn spawn_watch(
    fixture: &WatchFixture,
    ready_file: &Path,
    failure_file: Option<&Path>,
    extra_env: &[(&str, &str)],
) -> Child {
    let mut command = base_watch_command(fixture);
    command
        .env("CONARY_TEST_TRY_WATCH_READY_FILE", ready_file)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = failure_file {
        command.env("CONARY_TEST_TRY_WATCH_FAILURE_FILE", path);
    }
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command.spawn().expect("failed to spawn conary try --watch")
}

fn wait_for_file(path: &Path) {
    for _ in 0..100 {
        if path.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("timed out waiting for {}", path.display());
}

fn rollback(fixture: &WatchFixture) {
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .env("CONARY_TEST_SKIP_GENERATION_MOUNT", "1")
        .args(["try", "rollback", "--db-path", &fixture.db_path])
        .output()
        .expect("failed to run conary try rollback");
    assert_success(&output);
}

fn active_try_session(db_path: &str) -> Option<TrySession> {
    let conn = conary_core::db::open(db_path).unwrap();
    TrySession::find_active_or_orphaned(&conn).unwrap()
}

fn stdout_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", output_text(output));
}

fn assert_failure(output: &Output) {
    assert!(!output.status.success(), "{}", output_text(output));
}
```

- [ ] **Step 2: Add startup, keep-refusal, and JSON tests**

Add:

```rust
#[test]
fn try_watch_startup_creates_active_namespace_session_and_refuses_keep() {
    let fixture = WatchFixture::new();
    let ready = fixture.source.join(".watch-ready");

    let mut child = spawn_watch(&fixture, &ready, None, &[]);
    wait_for_file(&ready);

    let session = active_try_session(&fixture.db_path).expect("active watch session");
    assert_eq!(session.status, TrySessionStatus::Active);
    assert!(Path::new(&session.work_dir)
        .join(".conary-try-watch-session.json")
        .is_file());

    let keep = Command::new(env!("CARGO_BIN_EXE_conary"))
        .env("CONARY_TEST_SKIP_GENERATION_MOUNT", "1")
        .args(["try", "keep", "--db-path", &fixture.db_path])
        .output()
        .expect("failed to run conary try keep");
    assert_failure(&keep);
    assert!(output_text(&keep).contains("watch-created try session"));

    child.kill().expect("kill watch process");
    let _ = child.wait();
    rollback(&fixture);
}

#[test]
fn try_watch_json_outputs_ndjson_without_human_stdout() {
    let fixture = WatchFixture::new();

    let output = watch_until_test_exit(&fixture, true);
    assert_success(&output);
    let stdout = stdout_text(&output);

    assert!(!stdout.contains("Watching .\n"), "{stdout}");
    for line in stdout.lines() {
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert!(value["operation_id"].as_str().unwrap().starts_with("try-watch-"));
        assert!(value["sequence"].as_u64().unwrap() >= 1);
    }
}

#[test]
fn try_watch_refuses_dirty_tracked_tree_in_ci() {
    let fixture = WatchFixture::new();
    Command::new("git")
        .args(["init"])
        .current_dir(&fixture.source)
        .status()
        .expect("git init")
        .success()
        .then_some(())
        .expect("git init should succeed");
    Command::new("git")
        .args(["config", "user.email", "conary@example.invalid"])
        .current_dir(&fixture.source)
        .status()
        .expect("git config email");
    Command::new("git")
        .args(["config", "user.name", "Conary Test"])
        .current_dir(&fixture.source)
        .status()
        .expect("git config name");
    Command::new("git")
        .args(["add", "."])
        .current_dir(&fixture.source)
        .status()
        .expect("git add");
    Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(&fixture.source)
        .status()
        .expect("git commit");
    fs::write(
        fixture.source.join("src/main.rs"),
        "fn main() { println!(\"dirty\"); }\n",
    )
    .unwrap();

    let output = base_watch_command(&fixture)
        .env("CI", "1")
        .output()
        .expect("failed to run conary try --watch");

    assert_failure(&output);
    assert!(
        output_text(&output).contains("dirty local source"),
        "{}",
        output_text(&output)
    );
    assert!(active_try_session(&fixture.db_path).is_none());
}
```

- [ ] **Step 3: Add last-good failure recovery test**

Add:

```rust
#[test]
fn try_watch_failed_refresh_keeps_last_successful_generation() {
    let fixture = WatchFixture::new();
    let ready = fixture.source.join(".watch-ready");
    let failure = fixture.source.join(".watch-failure");

    let mut child = spawn_watch(&fixture, &ready, Some(&failure), &[]);
    wait_for_file(&ready);
    let session = active_try_session(&fixture.db_path).expect("active watch session");
    let first_generation = session.try_generation_id.expect("initial generation");

    fs::write(
        fixture.source.join("src/main.rs"),
        "fn main() { this is not rust }\n",
    )
    .unwrap();
    wait_for_file(&failure);

    let session = active_try_session(&fixture.db_path).expect("active watch session");
    assert_eq!(session.try_generation_id, Some(first_generation));

    child.kill().expect("kill watch process");
    let _ = child.wait();
    rollback(&fixture);
}
```

- [ ] **Step 4: Add stale-cook and rollback cleanup tests**

Add a spawned-process test for stale cook output:

```rust
#[test]
fn try_watch_discards_cook_when_source_changes_during_build() {
    let fixture = WatchFixture::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_conary"))
        .env("CONARY_TEST_SKIP_GENERATION_MOUNT", "1")
        .env("CONARY_TEST_TRY_WATCH_EXIT_AFTER_REFRESHES", "2")
        .env("CONARY_TEST_TRY_WATCH_PAUSE_DURING_COOK", "1")
        .args(["try", "--watch"])
        .arg(&fixture.source)
        .args(["--db-path", &fixture.db_path])
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to spawn watch");

    std::thread::sleep(std::time::Duration::from_millis(500));
    fs::write(
        fixture.source.join("src/main.rs"),
        "fn main() { println!(\"changed during cook\"); }\n",
    )
    .unwrap();

    let output = child.wait_with_output().expect("watch output");
    assert_success(&output);
    assert!(stdout_text(&output).contains("source changed during cook"));
}

#[test]
fn try_rollback_after_failed_watch_refresh_cleans_session() {
    let fixture = WatchFixture::new();
    let ready = fixture.source.join(".watch-ready");
    let failure = fixture.source.join(".watch-failure");
    let mut child = spawn_watch(&fixture, &ready, Some(&failure), &[]);
    wait_for_file(&ready);

    fs::write(
        fixture.source.join("src/main.rs"),
        "fn main() { this is not rust }\n",
    )
    .unwrap();
    wait_for_file(&failure);
    child.kill().expect("kill watch process");
    let _ = child.wait();

    rollback(&fixture);
    assert!(active_try_session(&fixture.db_path).is_none());
}
```

Keep `CONARY_TEST_TRY_WATCH_PAUSE_DURING_COOK` as a test-only branch in `watch.rs` around the cook adapter call.

- [ ] **Step 5: Run M3c and neighboring integration tests**

Run:

```bash
cargo test -p conary --test packaging_m3c
cargo test -p conary --test packaging_m1b
cargo test -p conary --test packaging_m3a
```

Expected: pass.

- [ ] **Step 6: Commit integration tests**

```bash
git add apps/conary/tests/packaging_m3c.rs apps/conary/src/commands/try_session/watch.rs
git commit -m "test(try): cover watch mode integration"
```

### Task 10: Final Docs, Full Proof, And Cleanup

**Files:**
- Modify: `docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/llms/subsystem-map.md`
- Test: full focused verification listed below

- [ ] **Step 1: Update M3 parent design status**

In `docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md`, change the status line to:

```markdown
**Status:** M3a, M3b, M3c0, and M3c landed; M3d record-mode spike is next
```

Update the M3c milestone row to say:

```markdown
| M3c | Watch mode | Landed: namespace-only source watch, cook-on-change, last-good try refresh preservation, redacted NDJSON events |
```

- [ ] **Step 2: Update feature ownership and subsystem map**

In `docs/modules/feature-ownership.md`, under Packaging/Try Sessions, add `apps/conary/src/commands/try_session/watch.rs` and `watch_source.rs` to `Start here`, and add this focused proof:

```markdown
`cargo test -p conary --lib commands::try_session`;
`cargo test -p conary --test packaging_m3c`.
```

In `docs/llms/subsystem-map.md`, update the packaging/try-session pointer so watch-mode questions route to:

```markdown
`apps/conary/src/commands/try_session/watch.rs` and
`apps/conary/src/commands/try_session/watch_source.rs`
```

- [ ] **Step 3: Run the focused M3c proof**

Run:

```bash
cargo test -p conary-core diagnostics
cargo test -p conary-core recipe::hermetic
cargo test -p conary-core recipe::kitchen::local_source
cargo test -p conary-core db::models::try_session
cargo test -p conary --lib cli::tests
cargo test -p conary --lib command_risk::tests
cargo test -p conary --lib dispatch::root
cargo test -p conary --lib commands::diagnostics::tests
cargo test -p conary --lib commands::try_session
cargo test -p conary --lib commands::cook
cargo test -p conary --test packaging_m1b
cargo test -p conary --test packaging_m3a
cargo test -p conary --test packaging_m3c
cargo fmt --check
```

Expected: all pass.

- [ ] **Step 4: Run the merge gate**

Run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: pass.

- [ ] **Step 5: Commit final docs**

```bash
git add docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md docs/modules/feature-ownership.md docs/llms/subsystem-map.md
git commit -m "docs: mark try watch mode landed"
```

- [ ] **Step 6: Final status check**

Run:

```bash
git status --short
git log --oneline -10
```

Expected: clean worktree, with the M3c implementation commits visible at the top.

## Self-Review Notes

Spec coverage:

- CLI contract is covered by Task 1 and Task 9.
- Watch lifecycle is covered by Tasks 3, 4, 6, 7, 8, and 9.
- Last-good staged refresh semantics are covered by Tasks 5, 7, and 9.
- Source watching and explicit `WatchSourceSet` are covered by Task 3.
- Cook/source policy is covered by Task 4.
- Events, redaction, JSON streaming, and operation records are covered by Tasks 2, 8, and 9.
- Keep refusal, marker fail-closed startup, cancellation, external lifecycle changes, and cleanup behavior are covered by Tasks 6, 7, 8, and 9.
- Docs updates are deferred to Task 10 after behavior passes.

Execution notes:

- Start execution in an isolated worktree if the current workspace is dirty.
- Use one commit per task.
- If a task uncovers a smaller required helper, keep it in the owning module named by that task and add a focused test before using it.
- Do not add a file-watcher dependency in M3c; polling plus canonical identity is the committed approach for this slice.
