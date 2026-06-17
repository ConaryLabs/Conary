# M3d Record-Mode Spike Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Locked for implementation after DeepSeek, Gemini, and local agentic review.

**Goal:** Build hidden `conary cook --record [SOURCE_DIR] -- <command>` as a scoped fanotify/inotify recording spike that produces a conservative draft recipe, redacted trace report, optional validation artifact, and recorded-draft publish-refusal proof without weakening M2 gates.

**Architecture:** `apps/conary/src/commands/record_mode/` owns CLI orchestration, private workspaces, watcher lifecycle, sandbox command running, redacted report writing, draft materialization, validation, and final output. Reusable pure DTOs and derivation helpers live under `crates/conary-core/src/recipe/recording/`; Linux watcher and runner code stays in the CLI because no non-CLI consumer exists. `apps/conary/src/commands/cook.rs` stays the cook owner and only routes record-mode requests plus exposes a narrow recorded-draft validation helper.

**Tech Stack:** Rust 2024, `anyhow`, `serde`, `serde_json`, `tempfile`, `walkdir`, `toml`, `libc` fanotify calls, new workspace `inotify` crate for recursive inotify, existing `nix` namespace/process helpers, existing `conary-core::container::{ContainerConfig, BindMount, Sandbox}`, existing `conary-core::diagnostics`, existing recipe parser/Kitchen, existing capability inference, and `cargo test`.

---

## Scope Locks

M3d includes:

- Hidden `conary cook --record [SOURCE_DIR] -- <command>` routing.
- Hidden flags: `--record-output`, `--record-backend`, `--record-validate`, `--keep-raw-trace`, `--record-unsafe-host`, and reserved fail-closed `--record-allow-network`.
- Default sandboxed command execution with network isolation.
- Exact source/work/install host roots watched and bind-mounted into the sandbox at `/conary/source`, `/conary/work`, and `/conary/destdir`.
- `DESTDIR`, `CONARY_DESTDIR`, `CONARY_WORKDIR`, and `SOURCE_DATE_EPOCH` exported to the recorded command.
- `/conary/source` is writable in M3d so build systems that patch or generate
  source files can still run. Source mutations must be recorded as
  `TraceOperation::SourceWrite` and surfaced in the trace report; later public
  UX can evaluate a read-only source default.
- Fanotify-first backend selection with recursive inotify support.
- Inotify-only fallback that declares incomplete read evidence.
- Event-loss handling for fanotify queue errors, inotify queue overflow, watch-limit exhaustion, and watcher thread failure.
- Public output layout under `--record-output`: `source/`, `recipe.toml`, `trace-report.json`, optional `trace-report.txt`, and optional `dist/`.
- Private raw trace fragments and private record workspaces with best-effort stale-workspace cleanup.
- Scope-relative redacted trace reports and operation records.
- Conservative draft recipe generation with `CONARY_DESTDIR` and concrete recording destdir normalization to `%(destdir)s`.
- Optional `--record-validate` normal cook using `origin_class_override = Some("recorded-draft")`.
- Publish-refusal regression proof for recorded-draft artifacts.
- Source and installed-tree symlinks are preserved as symlinks in snapshots and
  installed-file evidence. M3d does not follow symlinks into new trace roots.
- Source copying preserves fidelity by default. M3d does not silently skip
  `.git`, `target`, `node_modules`, or other large paths; large source copies
  are a known spike cost and a later public UX may add explicit exclude rules.

M3d excludes:

- A public stable record UX.
- Interactive shell recording.
- Host-root tracing.
- Network-enabled recording.
- Remi push, publish apply, or automatic keep/publish.
- DB migrations.
- Raw trace persistence unless `--keep-raw-trace` is explicitly set.
- Perfect dependency inference or malware scanning.

## File Structure

Create:

- `apps/conary/src/commands/record_mode/mod.rs`: module exports and `cmd_cook_record`.
- `apps/conary/src/commands/record_mode/types.rs`: CLI request DTOs, selected backend enum, operation state, command result, validation result, and final output DTOs.
- `apps/conary/src/commands/record_mode/workspace.rs`: private workspace creation, stale workspace cleanup, source copy, output directory materialization, raw trace retention/removal.
- `apps/conary/src/commands/record_mode/trace.rs`: `TraceBackend` trait, `TraceSession` trait, event classification bridge, backend selection, scope root validation, event-loss diagnostics.
- `apps/conary/src/commands/record_mode/inotify_backend.rs`: recursive inotify backend, dynamic directory registration, watch-budget checks, overflow handling, and tests.
- `apps/conary/src/commands/record_mode/fanotify_backend.rs`: raw libc fanotify probe/start/drain/finish code, capability checks, and fakeable syscall adapter tests.
- `apps/conary/src/commands/record_mode/runner.rs`: sandbox command runner, unsafe-host developer escape hatch, command environment, timeout/cancellation, and mount contract tests.
- `apps/conary/src/commands/record_mode/report.rs`: raw event aggregation, redaction, public report writing, human summary writing, operation-record output.
- `apps/conary/src/commands/record_mode/draft.rs`: CLI-facing draft recipe materialization around core pure helpers and output file writing.
- `apps/conary/src/commands/record_mode/validation.rs`: recorded-draft normal cook validation, artifact placement under `--record-output/dist`, and validation diagnostics.
- `apps/conary/tests/packaging_m3d.rs`: hidden CLI, inotify-only, sandbox, report, draft, validation, and publish-refusal integration tests.
- `crates/conary-core/src/recipe/recording/mod.rs`: pure recording module exports.
- `crates/conary-core/src/recipe/recording/report.rs`: serializable `RecordingReport`, path/event DTOs, backend limitations, redaction markers, and installed-file evidence.
- `crates/conary-core/src/recipe/recording/draft.rs`: draft recipe derivation inputs, command rendering, destdir normalization, source path safety, installed-file scan inputs, and tests.
- `crates/conary-core/src/recipe/recording/capabilities.rs`: advisory capability suggestion projection from installed-file evidence.

Modify:

- `Cargo.toml`: add workspace `inotify = "0.11"` dependency.
- `apps/conary/Cargo.toml`: add `inotify.workspace = true`.
- `apps/conary/src/cli/mod.rs`: add hidden cook record flags and parser tests only.
- `apps/conary/src/dispatch/root.rs`: route `Commands::Cook { record: true, .. }` to `cmd_cook_record`; preserve normal cook route.
- `apps/conary/src/command_risk.rs`: keep record mode classified as local state mutation.
- `apps/conary/src/commands/mod.rs`: register `record_mode`.
- `apps/conary/src/commands/cook.rs`: keep `cmd_cook` behavior unchanged; add narrow validation helper that accepts a `KitchenConfig` origin override.
- `apps/conary/src/commands/diagnostics.rs`: expose existing redaction helpers needed by record reports, or keep redaction inside core if no CLI-only data is needed.
- `crates/conary-core/src/diagnostics/mod.rs`: add `PackagingPhase::RecordMode`, record diagnostics, and record event kinds.
- `crates/conary-core/src/recipe/mod.rs`: export the new `recording` module.
- `crates/conary-core/src/container/mod.rs`: only add a tiny test helper or public config inspection method if runner tests cannot inspect the mount contract directly.
- `crates/conary-core/src/repository/static_repo/publish_gate.rs`: no behavior change expected; add regression test coverage for recorded-draft plus otherwise valid attestation if coverage is not already sufficient.
- `docs/modules/feature-ownership.md`: add record-mode start-here files after implementation lands.
- `docs/llms/subsystem-map.md`: route record-mode work to `commands/record_mode/` after implementation lands.
- `docs/superpowers/specs/2026-06-17-m3d-record-mode-spike-design.md`: mark implementation complete after implementation and verification pass.
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv`: regenerate after adding this plan and final doc edits.
- `docs/superpowers/documentation-accuracy-audit-ledger.tsv`: add this plan row and refresh the M3d spec row notes after implementation.

Maintainability boundaries:

- `apps/conary/src/cli/mod.rs` is over 1500 lines. This plan allows only hidden flag fields and parser/help tests there.
- `apps/conary/src/dispatch/root.rs` is over 1500 lines. This plan allows only the record-mode cook branch and routing tests there.
- `apps/conary/src/commands/cook.rs` is over 1500 lines. This plan allows only a small validation helper and route-preserving tests; record behavior lives in `record_mode/`.
- `crates/conary-core/src/recipe/kitchen/cook.rs` is over 1500 lines. M3d must not change Kitchen execution unless a test proves the validation helper cannot inject `origin_class_override` through existing config.
- `crates/conary-core/src/ccs/manifest.rs` is over 1500 lines. M3d must not
  modify it; recorded-draft provenance uses existing manifest provenance
  fields.
- Linux watcher implementation stays out of `crates/conary-core` because it is CLI-owned spike infrastructure.

Focused verification commands:

```bash
cargo test -p conary-core diagnostics
cargo test -p conary-core recipe::recording
cargo test -p conary-core repository::static_repo::publish_gate
cargo test -p conary --lib cli::tests
cargo test -p conary --lib command_risk::tests
cargo test -p conary --lib dispatch::root
cargo test -p conary --lib commands::record_mode
cargo test -p conary --lib commands::cook
cargo test -p conary --test packaging_m3a
cargo test -p conary --test packaging_m3d
cargo fmt --check
```

Merge gate:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Review lock mapping:

| Review concern | Locked plan owner |
| --- | --- |
| Public source snapshot lifecycle can point recipes at missing private data | Task 4 workspace/output lifecycle and Task 8 draft source tests |
| `CONARY_DESTDIR` does not exist in normal Kitchen validation | Task 8 destdir normalization tests and Task 9 normal cook validation |
| Trace backend must not spawn commands | Task 5 backend trait and Task 7 runner ownership |
| Sandbox watches must see exact bind-mounted inodes | Task 6 mount contract tests and Task 10 integration test |
| Failed tracing must not overclaim completeness | Task 5/6 event-loss tests and Task 11 final output diagnostics |
| Raw traces can leak secrets | Task 4 private workspace and Task 11 redaction/record tests |
| Recorded-draft publish gates must remain closed | Task 9 validation stamping and Task 11 publish-refusal tests |
| `--record-allow-network` is reserved, not minimum behavior | Task 1 fail-closed parser/routing test |
| Source symlinks and installed symlinks silently disappear | Task 3 snapshot symlink tests and Task 10 installed symlink evidence |
| Inotify can miss writes inside newly-created directories | Task 4 recursive watch tests plus Task 10 installed-file reconciliation limitation |
| Unsafe host mode can be missed in output | Task 10 stderr warning and report limitation |
| Sandbox environment can produce non-reproducible binaries | Task 6 `SOURCE_DATE_EPOCH` export |
| Writable source mounts can hide build-time mutations | Task 6 source-write contract and Task 10 trace/report reconciliation |

---

### Task 1: Hidden CLI Contract And Cook Routing Stub

**Files:**
- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/dispatch/root.rs`
- Modify: `apps/conary/src/command_risk.rs`
- Modify: `apps/conary/src/commands/mod.rs`
- Create: `apps/conary/src/commands/record_mode/mod.rs`
- Create: `apps/conary/src/commands/record_mode/types.rs`
- Test: `apps/conary/src/cli/mod.rs`
- Test: `apps/conary/src/dispatch/root.rs`
- Test: `apps/conary/src/command_risk.rs`

- [ ] **Step 1: Write failing CLI parser tests**

Add these tests to the existing `#[cfg(test)]` module in `apps/conary/src/cli/mod.rs`:

```rust
#[test]
fn cook_record_hidden_flags_parse_after_separator() {
    let cli = Cli::try_parse_from([
        "conary",
        "cook",
        "--record",
        "demo-source",
        "--record-output",
        "recorded/demo",
        "--record-backend",
        "inotify",
        "--record-validate",
        "--",
        "make",
        "install",
        "DESTDIR=$CONARY_DESTDIR",
    ])
    .unwrap();

    match cli.command {
        Some(Commands::Cook {
            target,
            record,
            record_output,
            record_backend,
            record_validate,
            keep_raw_trace,
            record_unsafe_host,
            record_allow_network,
            record_command,
            ..
        }) => {
            assert_eq!(target.as_deref(), Some("demo-source"));
            assert!(record);
            assert_eq!(record_output.as_deref(), Some("recorded/demo"));
            assert_eq!(record_backend.as_deref(), Some("inotify"));
            assert!(record_validate);
            assert!(!keep_raw_trace);
            assert!(!record_unsafe_host);
            assert!(!record_allow_network);
            assert_eq!(
                record_command,
                ["make", "install", "DESTDIR=$CONARY_DESTDIR"]
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            );
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn public_cook_help_hides_record_mode_flags() {
    let help = subcommand_help("cook");
    assert!(!help.contains("--record"));
    assert!(!help.contains("--record-output"));
    assert!(!help.contains("--keep-raw-trace"));
}
```

- [ ] **Step 2: Run the failing CLI parser tests**

Run:

```bash
cargo test -p conary --lib cook_record_hidden_flags_parse_after_separator public_cook_help_hides_record_mode_flags
```

Expected: fail because the cook command does not have record fields.

- [ ] **Step 3: Add hidden cook record fields**

In the `Commands::Cook` variant in `apps/conary/src/cli/mod.rs`, add these fields after `json`:

```rust
        /// Run hidden experimental record-mode recipe drafting
        #[arg(long)]
        #[arg(hide = true)]
        record: bool,

        /// Directory for record-mode public outputs
        #[arg(long)]
        #[arg(hide = true)]
        record_output: Option<String>,

        /// Trace backend for record mode: auto, fanotify, or inotify
        #[arg(long)]
        #[arg(hide = true)]
        record_backend: Option<String>,

        /// Validate the generated draft recipe with a normal cook
        #[arg(long)]
        #[arg(hide = true)]
        record_validate: bool,

        /// Keep private raw trace fragments for developer debugging
        #[arg(long)]
        #[arg(hide = true)]
        keep_raw_trace: bool,

        /// Run record command on the host without sandbox containment
        #[arg(long)]
        #[arg(hide = true)]
        record_unsafe_host: bool,

        /// Reserved hidden flag; M3d fails closed when this is set
        #[arg(long)]
        #[arg(hide = true)]
        record_allow_network: bool,

        /// Command to record, passed after `--`
        #[arg(last = true)]
        #[arg(hide = true)]
        record_command: Vec<String>,
```

Update every `Commands::Cook { ... }` pattern to bind the new fields or use `..`.

- [ ] **Step 4: Add the record-mode module stub**

Create `apps/conary/src/commands/record_mode/types.rs`:

```rust
// apps/conary/src/commands/record_mode/types.rs

use std::path::PathBuf;

use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestedRecordBackend {
    Auto,
    Fanotify,
    Inotify,
}

impl RequestedRecordBackend {
    pub(crate) fn parse(value: Option<&str>) -> Result<Self> {
        match value.unwrap_or("auto") {
            "auto" => Ok(Self::Auto),
            "fanotify" => Ok(Self::Fanotify),
            "inotify" => Ok(Self::Inotify),
            other => bail!("unsupported record backend `{other}`; expected auto, fanotify, or inotify"),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RecordCliRequest {
    pub(crate) source: PathBuf,
    pub(crate) output_dir: PathBuf,
    pub(crate) backend: RequestedRecordBackend,
    pub(crate) validate: bool,
    pub(crate) keep_raw_trace: bool,
    pub(crate) unsafe_host: bool,
    pub(crate) allow_network: bool,
    pub(crate) command: Vec<String>,
}
```

Create `apps/conary/src/commands/record_mode/mod.rs`:

```rust
// apps/conary/src/commands/record_mode/mod.rs

mod types;

use anyhow::{bail, Result};

pub(crate) use types::{RecordCliRequest, RequestedRecordBackend};

pub(crate) async fn cmd_cook_record(request: RecordCliRequest) -> Result<()> {
    validate_record_request(&request)?;
    bail!("record mode is not implemented yet")
}

fn validate_record_request(request: &RecordCliRequest) -> Result<()> {
    if request.command.is_empty() {
        bail!("record mode requires a command after `--`");
    }
    if request.allow_network {
        bail!("--record-allow-network is reserved for a later record-mode slice");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(command: Vec<String>) -> RecordCliRequest {
        RecordCliRequest {
            source: ".".into(),
            output_dir: "recorded/demo".into(),
            backend: RequestedRecordBackend::Auto,
            validate: false,
            keep_raw_trace: false,
            unsafe_host: false,
            allow_network: false,
            command,
        }
    }

    #[test]
    fn record_request_rejects_missing_command() {
        let error = validate_record_request(&request(Vec::new())).unwrap_err();
        assert!(error.to_string().contains("requires a command"));
    }

    #[test]
    fn record_request_rejects_reserved_network_flag() {
        let mut request = request(vec!["make".to_string()]);
        request.allow_network = true;
        let error = validate_record_request(&request).unwrap_err();
        assert!(error.to_string().contains("reserved"));
    }
}
```

Register it in `apps/conary/src/commands/mod.rs`:

```rust
pub(crate) mod record_mode;
pub(crate) use record_mode::cmd_cook_record;
```

- [ ] **Step 5: Route record mode before normal cook**

In `apps/conary/src/dispatch/root.rs`, replace the `Commands::Cook` match arm with a branch that constructs `RecordCliRequest` when `record` is true:

```rust
        Some(Commands::Cook {
            target,
            recipe,
            output,
            source_cache,
            jobs,
            keep_builddir,
            validate_only,
            fetch_only,
            explain,
            isolated,
            no_isolation,
            hermetic,
            json,
            record,
            record_output,
            record_backend,
            record_validate,
            keep_raw_trace,
            record_unsafe_host,
            record_allow_network,
            record_command,
        }) => {
            if record {
                let source = target
                    .as_deref()
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                let output_dir = record_output
                    .as_deref()
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| {
                        let name = source
                            .file_name()
                            .and_then(|value| value.to_str())
                            .filter(|value| !value.is_empty())
                            .unwrap_or("source");
                        std::path::PathBuf::from("recorded").join(name)
                    });
                return commands::cmd_cook_record(commands::record_mode::RecordCliRequest {
                    source,
                    output_dir,
                    backend: commands::record_mode::RequestedRecordBackend::parse(record_backend.as_deref())?,
                    validate: record_validate,
                    keep_raw_trace,
                    unsafe_host: record_unsafe_host,
                    allow_network: record_allow_network,
                    command: record_command,
                })
                .await;
            }

            commands::cmd_cook(
                target.as_deref(),
                recipe.as_deref(),
                &output,
                &source_cache,
                jobs,
                keep_builddir,
                validate_only,
                fetch_only,
                explain,
                isolated,
                no_isolation,
                hermetic,
                json,
            )
            .await
        }
```

Do not pass `recipe`, `output`, `source_cache`, `jobs`, `keep_builddir`, `validate_only`, `fetch_only`, `explain`, `isolated`, `no_isolation`, `hermetic`, or `json` into record mode in M3d. Record mode has its own hidden contract.

- [ ] **Step 6: Add route and risk tests**

Add a unit test in `apps/conary/src/command_risk.rs` if no cook risk test exists:

```rust
#[test]
fn cook_record_is_local_state_mutation_like_cook() {
    let command = Commands::Cook {
        target: Some(".".to_string()),
        recipe: None,
        output: "./dist".to_string(),
        source_cache: "/var/cache/conary/sources".to_string(),
        jobs: None,
        keep_builddir: false,
        validate_only: false,
        fetch_only: false,
        explain: false,
        isolated: false,
        no_isolation: false,
        hermetic: false,
        json: false,
        record: true,
        record_output: None,
        record_backend: None,
        record_validate: false,
        keep_raw_trace: false,
        record_unsafe_host: false,
        record_allow_network: false,
        record_command: vec!["make".to_string()],
    };
    let assessment = assess_command_risk(&command).expect("risk assessment");
    assert_eq!(assessment.summary, "conary cook");
}
```

Do not add a brittle dispatch unit seam for `Commands::Cook`. The route is
covered by the Task 11 integration tests that invoke `conary cook --record`
through the real binary.

- [ ] **Step 7: Run Task 1 verification**

Run:

```bash
cargo test -p conary --lib cook_record_hidden_flags_parse_after_separator public_cook_help_hides_record_mode_flags
cargo test -p conary --lib commands::record_mode
cargo test -p conary --lib command_risk::tests::cook_record_is_local_state_mutation_like_cook
```

Expected: all pass. `conary cook --record ...` still exits with "record mode is not implemented yet" until later tasks replace the stub.

- [ ] **Step 8: Commit Task 1**

```bash
git add apps/conary/src/cli/mod.rs apps/conary/src/dispatch/root.rs apps/conary/src/command_risk.rs apps/conary/src/commands/mod.rs apps/conary/src/commands/record_mode
git commit -m "feat(record): add hidden cook record route"
```

### Task 2: Core Recording DTOs, Scope Paths, And Diagnostics

**Files:**
- Modify: `crates/conary-core/src/diagnostics/mod.rs`
- Modify: `crates/conary-core/src/recipe/mod.rs`
- Create: `crates/conary-core/src/recipe/recording/mod.rs`
- Create: `crates/conary-core/src/recipe/recording/report.rs`
- Test: `crates/conary-core/src/diagnostics/mod.rs`
- Test: `crates/conary-core/src/recipe/recording/report.rs`

- [ ] **Step 1: Write failing diagnostics serialization test**

Add to `crates/conary-core/src/diagnostics/mod.rs` tests:

```rust
#[test]
fn record_mode_diagnostics_and_events_serialize_stably() {
    let diagnostic = PackagingDiagnostic::error(
        PackagingPhase::RecordMode,
        PackagingDiagnosticCode::RecordTraceFailed,
        "trace evidence is incomplete",
    );
    let event = PackagingEvent::diagnostic("record-1", 1, diagnostic);
    let value = serde_json::to_value(&event).unwrap();

    assert_eq!(value["phase"], "record-mode");
    assert_eq!(value["diagnostic"]["code"], "record-trace-failed");
    assert_eq!(value["kind"], "diagnostic-emitted");
}
```

- [ ] **Step 2: Add record diagnostics and event kinds**

Extend `PackagingPhase`:

```rust
    RecordMode,
```

Extend `PackagingDiagnosticCode`:

```rust
    RecordBackendUnavailable,
    RecordTraceFailed,
    RecordCommandFailed,
    RecordDraftGenerated,
    RecordValidationFailed,
    RecordRedactionFailed,
    RecordCleanupFailed,
```

Extend `PackagingEventKind`:

```rust
    RecordStarted,
    RecordBackendSelected,
    RecordCommandStarted,
    RecordCommandFinished,
    RecordTraceFinished,
    RecordDraftGenerated,
    RecordValidationStarted,
    RecordValidationFinished,
    RecordFinished,
```

- [ ] **Step 3: Write failing recording report tests**

Create `crates/conary-core/src/recipe/recording/report.rs` with tests first:

```rust
// conary-core/src/recipe/recording/report.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_path_rejects_private_prefix_leaks() {
        let source = ScopeRoot::new(TraceScope::Source, "/tmp/conary-record/source").unwrap();
        let scoped = source
            .scope_path(
                "/tmp/conary-record/source/src/main.rs",
                TraceOperation::SourceRead,
            )
            .unwrap();
        assert_eq!(scoped.scope, TraceScope::Source);
        assert_eq!(scoped.operation, TraceOperation::SourceRead);
        assert_eq!(scoped.path, "src/main.rs");

        let error = source
            .scope_path("/tmp/conary-record/other/secret", TraceOperation::SourceRead)
            .unwrap_err();
        assert!(error.to_string().contains("outside trace scope"));
    }

    #[test]
    fn report_serializes_backend_limitations_and_scope_relative_paths() {
        let report = RecordingReport {
            schema_version: 1,
            operation_id: "record-1".to_string(),
            backend: SelectedBackend::Inotify,
            scope_roots: vec![ScopeRootLabel::Source],
            command_summary: vec!["make".to_string(), "install".to_string()],
            command_exit: Some(0),
            observed_paths: vec![ObservedPath {
                scope: TraceScope::Install,
                operation: TraceOperation::InstallCreate,
                path: "usr/bin/demo".to_string(),
            }],
            installed_files: vec![InstalledFileEvidence {
                path: "usr/bin/demo".to_string(),
                file_type: "file".to_string(),
                executable: true,
                size: 12,
                link_target: None,
            }],
            inferred_build_steps: Vec::new(),
            inferred_install_steps: vec!["make install DESTDIR=%(destdir)s".to_string()],
            capability_suggestions: Vec::new(),
            ignored_events: vec![IgnoredEvent {
                reason: "out-of-scope".to_string(),
                count: 2,
            }],
            redactions: Vec::new(),
            limitations: vec![RecordingLimitation::IncompleteReadEvidence],
        };

        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"backend\":\"inotify\""));
        assert!(json.contains("\"incomplete-read-evidence\""));
        assert!(!json.contains("/tmp/conary-record"));
    }
}
```

- [ ] **Step 4: Implement recording report DTOs**

Replace the file body above the tests with:

```rust
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SelectedBackend {
    FanotifyInotify,
    Fanotify,
    Inotify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TraceScope {
    Source,
    Work,
    Install,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScopeRootLabel {
    Source,
    Work,
    Install,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TraceOperation {
    SourceRead,
    SourceWrite,
    WorkRead,
    WorkWrite,
    InstallCreate,
    InstallModify,
    InstallDelete,
    OutOfScope,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedPath {
    pub scope: TraceScope,
    pub operation: TraceOperation,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledFileEvidence {
    pub path: String,
    pub file_type: String,
    pub executable: bool,
    pub size: u64,
    pub link_target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySuggestion {
    pub capability: String,
    pub confidence: String,
    pub rationale: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IgnoredEvent {
    pub reason: String,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecordingLimitation {
    IncompleteReadEvidence,
    EventLoss,
    NetworkNotObserved,
    CommandFailed,
    ValidationSkipped,
    ValidationFailed,
    UnsafeHost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingReport {
    pub schema_version: u16,
    pub operation_id: String,
    pub backend: SelectedBackend,
    pub scope_roots: Vec<ScopeRootLabel>,
    pub command_summary: Vec<String>,
    pub command_exit: Option<i32>,
    pub observed_paths: Vec<ObservedPath>,
    pub installed_files: Vec<InstalledFileEvidence>,
    pub inferred_build_steps: Vec<String>,
    pub inferred_install_steps: Vec<String>,
    pub capability_suggestions: Vec<CapabilitySuggestion>,
    pub ignored_events: Vec<IgnoredEvent>,
    pub redactions: Vec<String>,
    pub limitations: Vec<RecordingLimitation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeRoot {
    pub scope: TraceScope,
    pub root: PathBuf,
}

impl ScopeRoot {
    pub fn new(scope: TraceScope, root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let root = root
            .canonicalize()
            .with_context(|| format!("failed to canonicalize trace root {}", root.display()))?;
        Ok(Self { scope, root })
    }

    pub fn scope_path(
        &self,
        path: impl AsRef<Path>,
        operation: TraceOperation,
    ) -> Result<ObservedPath> {
        let path = path.as_ref();
        let relative = path
            .strip_prefix(&self.root)
            .with_context(|| format!("path {} is outside trace scope", path.display()))?;
        if relative.components().any(|component| matches!(component, std::path::Component::ParentDir)) {
            bail!("path {} is outside trace scope", path.display());
        }
        Ok(ObservedPath {
            scope: self.scope,
            operation,
            path: relative.to_string_lossy().trim_start_matches('/').to_string(),
        })
    }
}
```

Create `crates/conary-core/src/recipe/recording/mod.rs`:

```rust
// conary-core/src/recipe/recording/mod.rs

pub mod report;

pub use report::{
    CapabilitySuggestion, IgnoredEvent, InstalledFileEvidence, ObservedPath, RecordingLimitation,
    RecordingReport, ScopeRoot, ScopeRootLabel, SelectedBackend, TraceOperation, TraceScope,
};
```

Update `crates/conary-core/src/recipe/mod.rs`:

```rust
pub mod recording;
```

- [ ] **Step 5: Run Task 2 verification**

Run:

```bash
cargo test -p conary-core diagnostics::tests::record_mode_diagnostics_and_events_serialize_stably
cargo test -p conary-core recipe::recording
```

Expected: all pass.

- [ ] **Step 6: Commit Task 2**

```bash
git add crates/conary-core/src/diagnostics/mod.rs crates/conary-core/src/recipe/mod.rs crates/conary-core/src/recipe/recording
git commit -m "feat(record): add recording report contract"
```

### Task 3: Workspace, Source Snapshot, And Raw Trace Lifecycle

**Files:**
- Modify: `apps/conary/src/commands/record_mode/mod.rs`
- Modify: `apps/conary/src/commands/record_mode/types.rs`
- Create: `apps/conary/src/commands/record_mode/workspace.rs`
- Test: `apps/conary/src/commands/record_mode/workspace.rs`

- [ ] **Step 1: Write failing workspace tests**

Create `apps/conary/src/commands/record_mode/workspace.rs`:

```rust
// apps/conary/src/commands/record_mode/workspace.rs

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn workspace_uses_private_permissions_and_public_source_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let output = temp.path().join("recorded/demo");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("main.c"), "int main(void){return 0;}\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("main.c", source.join("main-link.c")).unwrap();

        let workspace = RecordWorkspace::create(&source, &output, false).unwrap();
        let mode = std::fs::metadata(&workspace.private_root)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
        assert!(workspace.source_root.join("main.c").is_file());

        workspace.publish_source_snapshot().unwrap();
        assert!(output.join("source/main.c").is_file());
        #[cfg(unix)]
        assert_eq!(
            std::fs::read_link(output.join("source/main-link.c")).unwrap(),
            std::path::PathBuf::from("main.c")
        );
        assert!(!output.join("raw-trace").exists());
    }

    #[test]
    fn cleanup_removes_raw_trace_when_not_kept() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let output = temp.path().join("recorded/demo");
        std::fs::create_dir_all(&source).unwrap();

        let workspace = RecordWorkspace::create(&source, &output, false).unwrap();
        std::fs::write(workspace.raw_trace_dir.join("events.jsonl"), "secret").unwrap();
        let private_root = workspace.private_root.clone();
        workspace.cleanup().unwrap();

        assert!(!private_root.exists());
    }

    #[test]
    fn keep_raw_trace_preserves_private_trace_dir_only() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let output = temp.path().join("recorded/demo");
        std::fs::create_dir_all(&source).unwrap();

        let workspace = RecordWorkspace::create(&source, &output, true).unwrap();
        std::fs::write(workspace.raw_trace_dir.join("events.jsonl"), "secret").unwrap();
        let raw_trace_dir = workspace.raw_trace_dir.clone();
        workspace.cleanup().unwrap();

        assert!(raw_trace_dir.exists());
        assert!(!output.join("raw-trace").exists());
    }
}
```

- [ ] **Step 2: Implement workspace lifecycle**

Add this implementation above the tests:

```rust
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tempfile::Builder;
use walkdir::WalkDir;

#[derive(Debug)]
pub(crate) struct RecordWorkspace {
    pub(crate) private_root: PathBuf,
    pub(crate) source_root: PathBuf,
    pub(crate) work_root: PathBuf,
    pub(crate) install_root: PathBuf,
    pub(crate) raw_trace_dir: PathBuf,
    pub(crate) output_dir: PathBuf,
    keep_raw_trace: bool,
}

impl RecordWorkspace {
    pub(crate) fn create(source: &Path, output_dir: &Path, keep_raw_trace: bool) -> Result<Self> {
        let source = source
            .canonicalize()
            .with_context(|| format!("failed to canonicalize source {}", source.display()))?;
        let private_temp = Builder::new().prefix("conary-record-").tempdir()?;
        let private_root = private_temp.keep();
        fs::set_permissions(&private_root, fs::Permissions::from_mode(0o700))?;

        let workspace = Self {
            source_root: private_root.join("source"),
            work_root: private_root.join("work"),
            install_root: private_root.join("destdir"),
            raw_trace_dir: private_root.join("raw-trace"),
            output_dir: output_dir.to_path_buf(),
            private_root,
            keep_raw_trace,
        };
        fs::create_dir_all(&workspace.source_root)?;
        fs::create_dir_all(&workspace.work_root)?;
        fs::create_dir_all(&workspace.install_root)?;
        fs::create_dir_all(&workspace.raw_trace_dir)?;
        copy_tree(&source, &workspace.source_root)?;
        Ok(workspace)
    }

    pub(crate) fn publish_source_snapshot(&self) -> Result<()> {
        let public_source = self.output_dir.join("source");
        if public_source.exists() {
            fs::remove_dir_all(&public_source)?;
        }
        fs::create_dir_all(&self.output_dir)?;
        copy_tree(&self.source_root, &public_source)
    }

    pub(crate) fn cleanup(self) -> Result<()> {
        if self.keep_raw_trace {
            return Ok(());
        }
        if self.private_root.exists() {
            fs::remove_dir_all(&self.private_root)?;
        }
        Ok(())
    }
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    for entry in WalkDir::new(source).follow_links(false) {
        let entry = entry?;
        let relative = entry.path().strip_prefix(source)?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &target)?;
        } else if entry.file_type().is_symlink() {
            #[cfg(unix)]
            {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                let link_target = fs::read_link(entry.path())?;
                std::os::unix::fs::symlink(link_target, &target)?;
            }
            #[cfg(not(unix))]
            anyhow::bail!("record-mode source snapshots require Unix symlink support");
        }
    }
    Ok(())
}
```

Add to `apps/conary/src/commands/record_mode/mod.rs`:

```rust
mod workspace;
```

- [ ] **Step 3: Add best-effort stale workspace cleanup helper**

Extend `workspace.rs` with a helper and tests:

```rust
pub(crate) fn cleanup_stale_workspaces(parent: &Path) -> Result<usize> {
    let mut removed = 0;
    if !parent.is_dir() {
        return Ok(0);
    }
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if name.starts_with("conary-record-") && path.is_dir() {
            fs::remove_dir_all(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}
```

Add this test:

```rust
#[test]
fn stale_cleanup_only_removes_record_prefixes() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("conary-record-old")).unwrap();
    std::fs::create_dir_all(temp.path().join("unrelated")).unwrap();

    assert_eq!(cleanup_stale_workspaces(temp.path()).unwrap(), 1);
    assert!(!temp.path().join("conary-record-old").exists());
    assert!(temp.path().join("unrelated").exists());
}
```

- [ ] **Step 4: Run Task 3 verification**

Run:

```bash
cargo test -p conary --lib commands::record_mode::workspace
```

Expected: all workspace tests pass.

- [ ] **Step 5: Commit Task 3**

```bash
git add apps/conary/src/commands/record_mode
git commit -m "feat(record): add private recording workspace"
```

### Task 4: Recursive Inotify Backend

**Files:**
- Modify: `Cargo.toml`
- Modify: `apps/conary/Cargo.toml`
- Modify: `apps/conary/src/commands/record_mode/mod.rs`
- Create: `apps/conary/src/commands/record_mode/trace.rs`
- Create: `apps/conary/src/commands/record_mode/inotify_backend.rs`
- Test: `apps/conary/src/commands/record_mode/trace.rs`
- Test: `apps/conary/src/commands/record_mode/inotify_backend.rs`

- [ ] **Step 1: Add the inotify dependency**

In root `Cargo.toml` workspace dependencies:

```toml
inotify = "0.11"
```

In `apps/conary/Cargo.toml` dependencies:

```toml
inotify.workspace = true
```

- [ ] **Step 2: Write failing trace trait tests**

Create `apps/conary/src/commands/record_mode/trace.rs`:

```rust
// apps/conary/src/commands/record_mode/trace.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_selection_falls_back_to_inotify_with_limitation() {
        let status = TraceBackendStatus::selected(
            conary_core::recipe::recording::SelectedBackend::Inotify,
            vec![TraceLimitation::IncompleteReadEvidence],
        );
        assert!(status.is_usable());
        assert_eq!(status.limitations, vec![TraceLimitation::IncompleteReadEvidence]);
    }
}
```

- [ ] **Step 3: Implement trace abstractions**

Add above the tests:

```rust
use std::path::PathBuf;

use anyhow::Result;
use conary_core::recipe::recording::{
    ObservedPath, RecordingLimitation, ScopeRoot, SelectedBackend,
};

use super::types::RequestedRecordBackend;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TraceLimitation {
    IncompleteReadEvidence,
    EventLoss,
}

impl TraceLimitation {
    pub(crate) fn to_report_limitation(&self) -> RecordingLimitation {
        match self {
            Self::IncompleteReadEvidence => RecordingLimitation::IncompleteReadEvidence,
            Self::EventLoss => RecordingLimitation::EventLoss,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TraceScope {
    pub(crate) source: ScopeRoot,
    pub(crate) work: ScopeRoot,
    pub(crate) install: ScopeRoot,
}

impl TraceScope {
    pub(crate) fn roots(&self) -> [&ScopeRoot; 3] {
        [&self.source, &self.work, &self.install]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TraceBackendStatus {
    pub(crate) backend: SelectedBackend,
    pub(crate) limitations: Vec<TraceLimitation>,
    pub(crate) unavailable_reason: Option<String>,
}

impl TraceBackendStatus {
    pub(crate) fn selected(backend: SelectedBackend, limitations: Vec<TraceLimitation>) -> Self {
        Self {
            backend,
            limitations,
            unavailable_reason: None,
        }
    }

    pub(crate) fn unavailable(backend: SelectedBackend, reason: impl Into<String>) -> Self {
        Self {
            backend,
            limitations: Vec::new(),
            unavailable_reason: Some(reason.into()),
        }
    }

    pub(crate) fn is_usable(&self) -> bool {
        self.unavailable_reason.is_none()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RawTraceEvent {
    pub(crate) path: PathBuf,
    pub(crate) observed: ObservedPath,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TraceDrain {
    pub(crate) events: Vec<RawTraceEvent>,
    pub(crate) ignored_events: u64,
    pub(crate) event_loss: bool,
}

pub(crate) trait TraceBackend {
    fn probe(
        &self,
        scope: &TraceScope,
        requested: RequestedRecordBackend,
    ) -> Result<TraceBackendStatus>;

    fn start(&self, scope: TraceScope) -> Result<Box<dyn TraceSession>>;
}

pub(crate) trait TraceSession {
    fn drain_events(&mut self) -> Result<TraceDrain>;
    fn finish(&mut self) -> Result<TraceDrain>;
}
```

Add to `record_mode/mod.rs`:

```rust
mod trace;
```

- [ ] **Step 4: Write failing inotify backend tests**

Create `apps/conary/src/commands/record_mode/inotify_backend.rs`:

```rust
// apps/conary/src/commands/record_mode/inotify_backend.rs

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::recipe::recording::{ScopeRoot, TraceScope as ReportScope};

    fn scope(temp: &tempfile::TempDir) -> super::super::trace::TraceScope {
        let source = temp.path().join("source");
        let work = temp.path().join("work");
        let install = temp.path().join("install");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&install).unwrap();
        super::super::trace::TraceScope {
            source: ScopeRoot::new(ReportScope::Source, &source).unwrap(),
            work: ScopeRoot::new(ReportScope::Work, &work).unwrap(),
            install: ScopeRoot::new(ReportScope::Install, &install).unwrap(),
        }
    }

    #[test]
    fn recursive_inotify_records_create_modify_delete_and_new_directory() {
        let temp = tempfile::tempdir().unwrap();
        let scope = scope(&temp);
        let backend = InotifyTraceBackend::new();
        let mut session = backend.start(scope).unwrap();

        let install_file = temp.path().join("install/usr/bin/demo");
        std::fs::create_dir_all(install_file.parent().unwrap()).unwrap();
        std::fs::write(&install_file, "one").unwrap();
        std::fs::write(&install_file, "two").unwrap();
        std::fs::remove_file(&install_file).unwrap();

        let drain = session.finish().unwrap();
        assert!(drain.events.iter().any(|event| event.observed.path == "usr/bin/demo"));
        assert!(drain.events.iter().any(|event| {
            event.observed.path == "usr/bin/demo"
                && event.observed.operation != conary_core::recipe::recording::TraceOperation::Unknown
        }));
        assert!(!drain.event_loss);
    }
}
```

- [ ] **Step 5: Implement recursive inotify backend**

Implement `InotifyTraceBackend` with:

```rust
pub(crate) struct InotifyTraceBackend;

impl InotifyTraceBackend {
    pub(crate) fn new() -> Self {
        Self
    }
}
```

Required behavior:

- `probe` always returns `SelectedBackend::Inotify` with `TraceLimitation::IncompleteReadEvidence`.
- `start` recursively adds watches for every directory under source, work, and install roots.
- `start` reads `/proc/sys/fs/inotify/max_user_watches`; if initial directory count is greater than the budget, return an error containing `max_user_watches`.
- `drain_events` maps create, modify, delete, and move events to `TraceOperation` based on the matched root and path.
- `drain_events` must call `ScopeRoot::scope_path(path, operation)` with a
  concrete operation. It must not emit `TraceOperation::Unknown` for known
  inotify masks.
- New directories get watches before returning from `drain_events`.
- `IN_Q_OVERFLOW` sets `TraceDrain.event_loss = true`.

Use these masks:

```rust
inotify::WatchMask::CREATE
    | inotify::WatchMask::MODIFY
    | inotify::WatchMask::DELETE
    | inotify::WatchMask::MOVED_FROM
    | inotify::WatchMask::MOVED_TO
    | inotify::WatchMask::ATTRIB
```

- [ ] **Step 6: Run Task 4 verification**

Run:

```bash
cargo test -p conary --lib commands::record_mode::trace
cargo test -p conary --lib commands::record_mode::inotify_backend
```

Expected: all pass without elevated privileges.

- [ ] **Step 7: Commit Task 4**

```bash
git add Cargo.toml apps/conary/Cargo.toml apps/conary/src/commands/record_mode
git commit -m "feat(record): add recursive inotify tracing"
```

### Task 5: Fanotify Probe And Auto Backend Selection

**Files:**
- Modify: `apps/conary/src/commands/record_mode/mod.rs`
- Modify: `apps/conary/src/commands/record_mode/trace.rs`
- Create: `apps/conary/src/commands/record_mode/fanotify_backend.rs`
- Test: `apps/conary/src/commands/record_mode/fanotify_backend.rs`
- Test: `apps/conary/src/commands/record_mode/trace.rs`

- [ ] **Step 1: Write failing fanotify probe tests**

Create `apps/conary/src/commands/record_mode/fanotify_backend.rs`:

```rust
// apps/conary/src/commands/record_mode/fanotify_backend.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_fanotify_fails_closed_without_capability() {
        let backend = FanotifyTraceBackend::with_probe(FanotifyProbe::PermissionDenied);
        let status = backend.probe_without_scope(RequestedRecordBackend::Fanotify).unwrap();
        assert!(!status.is_usable());
        assert!(
            status
                .unavailable_reason
                .as_deref()
                .unwrap()
                .contains("CAP_SYS_ADMIN")
        );
    }

    #[test]
    fn auto_can_report_fanotify_unavailable_for_fallback() {
        let backend = FanotifyTraceBackend::with_probe(FanotifyProbe::PermissionDenied);
        let status = backend.probe_without_scope(RequestedRecordBackend::Auto).unwrap();
        assert!(!status.is_usable());
        assert_eq!(status.backend, SelectedBackend::Fanotify);
    }
}
```

- [ ] **Step 2: Implement fakeable fanotify probe**

Add above tests:

```rust
use anyhow::Result;
use conary_core::recipe::recording::SelectedBackend;

use super::trace::{TraceBackendStatus, TraceLimitation};
use super::types::RequestedRecordBackend;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FanotifyProbe {
    Available,
    PermissionDenied,
    Unsupported,
}

pub(crate) struct FanotifyTraceBackend {
    probe_override: Option<FanotifyProbe>,
}

impl FanotifyTraceBackend {
    pub(crate) fn new() -> Self {
        Self { probe_override: None }
    }

    pub(crate) fn with_probe(probe: FanotifyProbe) -> Self {
        Self {
            probe_override: Some(probe),
        }
    }

    pub(crate) fn probe_without_scope(
        &self,
        _requested: RequestedRecordBackend,
    ) -> Result<TraceBackendStatus> {
        match self.probe_override.unwrap_or_else(probe_fanotify_support) {
            FanotifyProbe::Available => Ok(TraceBackendStatus::selected(
                SelectedBackend::Fanotify,
                Vec::new(),
            )),
            FanotifyProbe::PermissionDenied => Ok(TraceBackendStatus::unavailable(
                SelectedBackend::Fanotify,
                "fanotify requires CAP_SYS_ADMIN for scoped marks in this environment",
            )),
            FanotifyProbe::Unsupported => Ok(TraceBackendStatus::unavailable(
                SelectedBackend::Fanotify,
                "fanotify is not supported by this kernel",
            )),
        }
    }
}

fn probe_fanotify_support() -> FanotifyProbe {
    // SAFETY: fanotify_init is called with constant flags and returns either a
    // new file descriptor or -1. No pointer arguments are passed.
    let fd = unsafe {
        libc::fanotify_init(
            libc::FAN_CLASS_NOTIF | libc::FAN_CLOEXEC | libc::FAN_NONBLOCK,
            libc::O_RDONLY | libc::O_CLOEXEC,
        )
    };
    if fd >= 0 {
        // SAFETY: fd was returned by fanotify_init above and has not been
        // moved or closed yet.
        unsafe {
            libc::close(fd);
        }
        FanotifyProbe::Available
    } else {
        let error = std::io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::EPERM) | Some(libc::EACCES) => FanotifyProbe::PermissionDenied,
            _ => FanotifyProbe::Unsupported,
        }
    }
}
```

Then implement `TraceBackend` for `FanotifyTraceBackend`. `start` must call raw `libc::fanotify_init`, mark the three scoped roots with `libc::fanotify_mark`, and return a `FanotifyTraceSession`. It is acceptable for the first implementation to include tests through fakeable probe and trait-level event classification while gated live fanotify integration is opt-in through an ignored test.

Every `unsafe` block in `fanotify_backend.rs` must have a `// SAFETY:` comment.
Add a unit test around the fakeable syscall adapter proving file descriptors
are closed when fanotify probing succeeds and a later mark/setup step fails.
Document kernel limitations in the backend diagnostic: fanotify read evidence
depends on kernel fanotify support for the selected marks, and the spike falls
back to inotify-only in `auto` when that support or permission is absent.

- [ ] **Step 3: Add auto backend selector**

In `trace.rs`, add:

```rust
pub(crate) fn select_backend(
    requested: RequestedRecordBackend,
    fanotify: &dyn TraceBackend,
    inotify: &dyn TraceBackend,
    scope: &TraceScope,
) -> Result<TraceBackendStatus> {
    match requested {
        RequestedRecordBackend::Fanotify => {
            let status = fanotify.probe(scope, requested)?;
            if status.is_usable() {
                Ok(status)
            } else {
                anyhow::bail!("{}", status.unavailable_reason.unwrap_or_else(|| "fanotify unavailable".to_string()))
            }
        }
        RequestedRecordBackend::Inotify => inotify.probe(scope, requested),
        RequestedRecordBackend::Auto => {
            let status = fanotify.probe(scope, requested)?;
            if status.is_usable() {
                return Ok(TraceBackendStatus::selected(
                    conary_core::recipe::recording::SelectedBackend::FanotifyInotify,
                    status.limitations,
                ));
            }
            inotify.probe(scope, RequestedRecordBackend::Inotify)
        }
    }
}
```

Add a unit test with fake `TraceBackend` values proving explicit fanotify fails and auto returns inotify with incomplete read evidence.

- [ ] **Step 4: Run Task 5 verification**

Run:

```bash
cargo test -p conary --lib commands::record_mode::fanotify_backend
cargo test -p conary --lib commands::record_mode::trace
```

Expected: all pass without requiring CAP_SYS_ADMIN.

- [ ] **Step 5: Commit Task 5**

```bash
git add apps/conary/src/commands/record_mode
git commit -m "feat(record): add fanotify backend selection"
```

### Task 6: Sandbox Command Runner And Mount Contract

**Files:**
- Modify: `apps/conary/src/commands/record_mode/mod.rs`
- Modify: `apps/conary/src/commands/record_mode/types.rs`
- Create: `apps/conary/src/commands/record_mode/runner.rs`
- Test: `apps/conary/src/commands/record_mode/runner.rs`

- [ ] **Step 1: Write failing runner contract tests**

Create `apps/conary/src/commands/record_mode/runner.rs`:

```rust
// apps/conary/src/commands/record_mode/runner.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_config_mounts_exact_record_roots_and_exports_destdir() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let work = temp.path().join("work");
        let install = temp.path().join("destdir");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&install).unwrap();

        let plan = sandbox_plan(&RecordCommandRequest {
            source_root: source.clone(),
            work_root: work.clone(),
            install_root: install.clone(),
            command: vec!["/bin/sh".to_string(), "-c".to_string(), "true".to_string()],
            unsafe_host: false,
        })
        .unwrap();

        assert!(plan.network_isolated);
        assert_eq!(plan.cwd, "/conary/source");
        assert_eq!(plan.env_value("DESTDIR"), Some("/conary/destdir"));
        assert_eq!(plan.env_value("CONARY_DESTDIR"), Some("/conary/destdir"));
        assert_eq!(plan.env_value("CONARY_WORKDIR"), Some("/conary/work"));
        assert!(plan.env_value("SOURCE_DATE_EPOCH").is_some());
        assert!(plan.has_mount(&source, "/conary/source", true));
        assert!(plan.has_mount(&work, "/conary/work", true));
        assert!(plan.has_mount(&install, "/conary/destdir", true));
    }

    #[test]
    fn unsafe_host_plan_is_explicit_and_still_scoped() {
        let temp = tempfile::tempdir().unwrap();
        let request = RecordCommandRequest {
            source_root: temp.path().join("source"),
            work_root: temp.path().join("work"),
            install_root: temp.path().join("destdir"),
            command: vec!["true".to_string()],
            unsafe_host: true,
        };
        std::fs::create_dir_all(&request.source_root).unwrap();
        std::fs::create_dir_all(&request.work_root).unwrap();
        std::fs::create_dir_all(&request.install_root).unwrap();

        let plan = sandbox_plan(&request).unwrap();
        assert!(plan.unsafe_host);
        assert_eq!(plan.env_value("DESTDIR"), Some(request.install_root.to_str().unwrap()));
        assert_eq!(
            plan.env_value("CONARY_DESTDIR"),
            Some(request.install_root.to_str().unwrap())
        );
        assert_eq!(
            plan.env_value("CONARY_WORKDIR"),
            Some(request.work_root.to_str().unwrap())
        );
        assert!(plan.env_value("SOURCE_DATE_EPOCH").is_some());
    }
}
```

- [ ] **Step 2: Implement command request and plan builder**

Add above tests:

```rust
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use conary_core::container::{BindMount, ContainerConfig, Sandbox};

#[derive(Debug, Clone)]
pub(crate) struct RecordCommandRequest {
    pub(crate) source_root: PathBuf,
    pub(crate) work_root: PathBuf,
    pub(crate) install_root: PathBuf,
    pub(crate) command: Vec<String>,
    pub(crate) unsafe_host: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct RecordSandboxPlan {
    pub(crate) unsafe_host: bool,
    pub(crate) cwd: String,
    pub(crate) network_isolated: bool,
    pub(crate) mounts: Vec<(PathBuf, String, bool)>,
    pub(crate) env: Vec<(String, String)>,
}

impl RecordSandboxPlan {
    fn env_value(&self, key: &str) -> Option<&str> {
        self.env
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.as_str())
    }

    fn has_mount(&self, source: &Path, target: &str, writable: bool) -> bool {
        self.mounts
            .iter()
            .any(|(candidate, mount_target, mount_writable)| {
                candidate == source && mount_target == target && *mount_writable == writable
            })
    }
}

pub(crate) fn sandbox_plan(request: &RecordCommandRequest) -> Result<RecordSandboxPlan> {
    let source_date_epoch = std::env::var("SOURCE_DATE_EPOCH").unwrap_or_else(|_| "0".to_string());

    if request.unsafe_host {
        let install = request.install_root.to_string_lossy().to_string();
        return Ok(RecordSandboxPlan {
            unsafe_host: true,
            cwd: request.source_root.to_string_lossy().to_string(),
            network_isolated: false,
            mounts: Vec::new(),
            env: vec![
                ("DESTDIR".to_string(), install.clone()),
                ("CONARY_DESTDIR".to_string(), install),
                (
                    "CONARY_WORKDIR".to_string(),
                    request.work_root.to_string_lossy().to_string(),
                ),
                ("SOURCE_DATE_EPOCH".to_string(), source_date_epoch),
            ],
        });
    }

    Ok(RecordSandboxPlan {
        unsafe_host: false,
        cwd: "/conary/source".to_string(),
        network_isolated: true,
        mounts: vec![
            (request.source_root.clone(), "/conary/source".to_string(), true),
            (request.work_root.clone(), "/conary/work".to_string(), true),
            (request.install_root.clone(), "/conary/destdir".to_string(), true),
        ],
        env: vec![
            ("DESTDIR".to_string(), "/conary/destdir".to_string()),
            ("CONARY_DESTDIR".to_string(), "/conary/destdir".to_string()),
            ("CONARY_WORKDIR".to_string(), "/conary/work".to_string()),
            ("SOURCE_DATE_EPOCH".to_string(), source_date_epoch),
        ],
    })
}
```

The `/conary/source` mount is writable by design for this spike. Trace backends
must classify source mutations as `TraceOperation::SourceWrite` and report them;
M3d does not hide the fact that a recorded build patched or generated source
files.

- [ ] **Step 3: Implement command execution**

Add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordCommandOutcome {
    pub(crate) exit_code: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

pub(crate) fn run_record_command(request: &RecordCommandRequest) -> Result<RecordCommandOutcome> {
    let plan = sandbox_plan(request)?;
    if plan.unsafe_host {
        let mut command = std::process::Command::new(&request.command[0]);
        command.args(&request.command[1..]);
        command.current_dir(&request.source_root);
        for (key, value) in &plan.env {
            command.env(key, value);
        }
        let output = command.output()?;
        return Ok(RecordCommandOutcome {
            exit_code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let mut config = ContainerConfig::default().for_untrusted();
    config.timeout = Duration::from_secs(3600);
    config.workdir = PathBuf::from(&plan.cwd);
    config.bind_mounts.clear();
    for (source, target, writable) in &plan.mounts {
        let mount = if *writable {
            BindMount::writable(source, target)
        } else {
            BindMount::readonly(source, target)
        };
        config.bind_mounts.push(mount);
    }
    let env = plan
        .env
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let command = render_command_for_shell(&request.command);
    let mut sandbox = Sandbox::new(config);
    let (exit_code, stdout, stderr) =
        sandbox.execute("/bin/sh", &command, &[], env.as_slice())?;
    Ok(RecordCommandOutcome {
        exit_code,
        stdout,
        stderr,
    })
}
```

Add the renderer in the same file:

```rust
fn render_command_for_shell(command: &[String]) -> String {
    command
        .iter()
        .map(|arg| shell_quote_for_execution(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Quote a command argument for the temporary `/bin/sh` execution wrapper.
///
/// `$` remains unquoted so `$CONARY_DESTDIR` can expand inside the recording
/// sandbox. Do not use this helper for generated recipe text.
fn shell_quote_for_execution(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '=' | '$'))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
```

Add a test proving `shell_quote_for_execution("$CONARY_DESTDIR/usr/bin")`
keeps the `$CONARY_DESTDIR` expansion intact.

- [ ] **Step 4: Run Task 6 verification**

Run:

```bash
cargo test -p conary --lib commands::record_mode::runner
```

Expected: plan tests pass. Live sandbox execution may require environment support and is covered by integration tests with skip logic.

- [ ] **Step 5: Commit Task 6**

```bash
git add apps/conary/src/commands/record_mode
git commit -m "feat(record): add sandbox command runner"
```

### Task 7: Report Aggregation, Redaction, And Operation Output

**Files:**
- Modify: `apps/conary/src/commands/record_mode/mod.rs`
- Create: `apps/conary/src/commands/record_mode/report.rs`
- Test: `apps/conary/src/commands/record_mode/report.rs`

- [ ] **Step 1: Write failing report tests**

Create `apps/conary/src/commands/record_mode/report.rs`:

```rust
// apps/conary/src/commands/record_mode/report.rs

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::recipe::recording::{
        ObservedPath, RecordingLimitation, SelectedBackend, TraceOperation, TraceScope,
    };

    #[test]
    fn report_writer_redacts_command_and_private_paths() {
        let temp = tempfile::tempdir().unwrap();
        let output_dir = temp.path().join("recorded/demo");
        let private_root = temp.path().join("conary-record-private");
        let report = build_recording_report(ReportInput {
            operation_id: "record-1".to_string(),
            backend: SelectedBackend::Inotify,
            command: vec![
                "curl".to_string(),
                "-H".to_string(),
                "Authorization: Bearer secret-token".to_string(),
            ],
            command_exit: Some(0),
            observed_paths: vec![ObservedPath {
                scope: TraceScope::Install,
                operation: TraceOperation::InstallCreate,
                path: "usr/bin/demo".to_string(),
            }],
            installed_files: Vec::new(),
            limitations: vec![RecordingLimitation::IncompleteReadEvidence],
            ignored_events: Vec::new(),
            private_prefixes: vec![private_root.clone()],
        })
        .unwrap();

        write_report_files(&output_dir, &report).unwrap();
        let text = std::fs::read_to_string(output_dir.join("trace-report.json")).unwrap();
        assert!(text.contains("Bearer [REDACTED]"));
        assert!(!text.contains("secret-token"));
        assert!(!text.contains(private_root.to_str().unwrap()));
    }
}
```

- [ ] **Step 2: Implement report builder and writer**

Add above tests:

```rust
use std::path::{Path, PathBuf};

use anyhow::Result;
use conary_core::diagnostics::redaction::redact_command;
use conary_core::recipe::recording::{
    IgnoredEvent, InstalledFileEvidence, RecordingLimitation, RecordingReport, ScopeRootLabel,
    SelectedBackend, ObservedPath,
};

pub(crate) struct ReportInput {
    pub(crate) operation_id: String,
    pub(crate) backend: SelectedBackend,
    pub(crate) command: Vec<String>,
    pub(crate) command_exit: Option<i32>,
    pub(crate) observed_paths: Vec<ObservedPath>,
    pub(crate) installed_files: Vec<InstalledFileEvidence>,
    pub(crate) limitations: Vec<RecordingLimitation>,
    pub(crate) ignored_events: Vec<IgnoredEvent>,
    pub(crate) private_prefixes: Vec<PathBuf>,
}

pub(crate) fn build_recording_report(input: ReportInput) -> Result<RecordingReport> {
    let command = redact_command(&input.command);
    Ok(RecordingReport {
        schema_version: 1,
        operation_id: input.operation_id,
        backend: input.backend,
        scope_roots: vec![ScopeRootLabel::Source, ScopeRootLabel::Work, ScopeRootLabel::Install],
        command_summary: command.value,
        command_exit: input.command_exit,
        observed_paths: input.observed_paths,
        installed_files: input.installed_files,
        inferred_build_steps: Vec::new(),
        inferred_install_steps: Vec::new(),
        capability_suggestions: Vec::new(),
        ignored_events: input.ignored_events,
        redactions: command
            .redactions
            .into_iter()
            .map(|marker| marker.reason)
            .collect(),
        limitations: input.limitations,
    })
}

pub(crate) fn write_report_files(output_dir: &Path, report: &RecordingReport) -> Result<()> {
    std::fs::create_dir_all(output_dir)?;
    let json = serde_json::to_string_pretty(report)?;
    std::fs::write(output_dir.join("trace-report.json"), json)?;
    std::fs::write(
        output_dir.join("trace-report.txt"),
        format!(
            "Recording backend: {:?}\nCommand exit: {:?}\nObserved paths: {}\n",
            report.backend,
            report.command_exit,
            report.observed_paths.len()
        ),
    )?;
    Ok(())
}
```

- [ ] **Step 3: Add PackagingCommandOutput finalizer**

Add:

```rust
use conary_core::diagnostics::{
    PACKAGING_JSON_SCHEMA_VERSION, PackagingCommandOutput, PackagingCommandStatus,
    PackagingDiagnostic, PackagingDiagnosticCode, PackagingEvent, PackagingEventKind,
    PackagingPhase,
};

pub(crate) fn record_command_output(
    operation_id: &str,
    success: bool,
    diagnostics: Vec<PackagingDiagnostic>,
    events: Vec<PackagingEvent>,
    summary: impl Into<String>,
) -> PackagingCommandOutput {
    PackagingCommandOutput {
        schema_version: PACKAGING_JSON_SCHEMA_VERSION,
        operation_id: operation_id.to_string(),
        command: "conary cook --record".to_string(),
        status: if success {
            PackagingCommandStatus::Succeeded
        } else {
            PackagingCommandStatus::Failed
        },
        diagnostics,
        events,
        artifacts: Vec::new(),
        summary: Some(summary.into()),
    }
}

pub(crate) fn record_event(
    operation_id: &str,
    sequence: u64,
    kind: PackagingEventKind,
    message: impl Into<String>,
) -> PackagingEvent {
    PackagingEvent {
        schema_version: PACKAGING_JSON_SCHEMA_VERSION,
        operation_id: operation_id.to_string(),
        sequence,
        phase: PackagingPhase::RecordMode,
        kind,
        message: Some(message.into()),
        diagnostic: None,
        artifact: None,
        progress: None,
    }
}

pub(crate) fn record_error(
    code: PackagingDiagnosticCode,
    message: impl Into<String>,
) -> PackagingDiagnostic {
    PackagingDiagnostic::error(PackagingPhase::RecordMode, code, message)
}
```

- [ ] **Step 4: Run Task 7 verification**

Run:

```bash
cargo test -p conary --lib commands::record_mode::report
```

Expected: all pass.

- [ ] **Step 5: Commit Task 7**

```bash
git add apps/conary/src/commands/record_mode
git commit -m "feat(record): write redacted trace reports"
```

### Task 8: Draft Recipe Derivation

**Files:**
- Create: `crates/conary-core/src/recipe/recording/draft.rs`
- Create: `crates/conary-core/src/recipe/recording/capabilities.rs`
- Modify: `crates/conary-core/src/recipe/recording/mod.rs`
- Create: `apps/conary/src/commands/record_mode/draft.rs`
- Modify: `apps/conary/src/commands/record_mode/mod.rs`
- Test: `crates/conary-core/src/recipe/recording/draft.rs`
- Test: `apps/conary/src/commands/record_mode/draft.rs`

- [ ] **Step 1: Write failing core draft tests**

Create `crates/conary-core/src/recipe/recording/draft.rs`:

```rust
// conary-core/src/recipe/recording/draft.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_renderer_preserves_arguments_and_redacts_destdir_forms() {
        let rendered = render_recorded_command(
            &[
                "make".to_string(),
                "install".to_string(),
                "DESTDIR=$CONARY_DESTDIR".to_string(),
                "/tmp/conary-record/demo/destdir/usr/bin/app".to_string(),
            ],
            "/tmp/conary-record/demo/destdir",
        );
        assert_eq!(
            rendered,
            "make install 'DESTDIR=%(destdir)s' %(destdir)s/usr/bin/app"
        );
    }

    #[test]
    fn recipe_quote_preserves_destdir_macro_without_shell_expansion() {
        assert_eq!(
            shell_quote_for_recipe("%(destdir)s/usr/bin/app"),
            "%(destdir)s/usr/bin/app"
        );
        assert_eq!(
            shell_quote_for_recipe("$CONARY_DESTDIR/usr/bin/app"),
            "'$CONARY_DESTDIR/usr/bin/app'"
        );
    }

    #[test]
    fn draft_recipe_uses_public_source_snapshot_and_install_step_when_files_exist() {
        let recipe = derive_draft_recipe(DraftRecipeInput {
            package_name: "demo".to_string(),
            package_version: "0.1.0-recorded".to_string(),
            command: vec!["make".to_string(), "install".to_string()],
            recording_destdir: "/tmp/private/destdir".to_string(),
            installed_files: vec!["usr/bin/demo".to_string()],
            network_likely: false,
        })
        .unwrap();

        assert!(recipe.contains("[source]"));
        assert!(recipe.contains("path = \"source\""));
        assert!(recipe.contains("install = \"make install\""));
        assert!(!recipe.contains("/tmp/private"));
    }
}
```

- [ ] **Step 2: Implement pure draft helpers**

Add above tests:

```rust
use anyhow::{bail, Result};

#[derive(Debug, Clone)]
pub struct DraftRecipeInput {
    pub package_name: String,
    pub package_version: String,
    pub command: Vec<String>,
    pub recording_destdir: String,
    pub installed_files: Vec<String>,
    pub network_likely: bool,
}

pub fn render_recorded_command(command: &[String], recording_destdir: &str) -> String {
    command
        .iter()
        .map(|arg| {
            let normalized = arg
                .replace("${CONARY_DESTDIR}", "%(destdir)s")
                .replace("$CONARY_DESTDIR", "%(destdir)s")
                .replace(recording_destdir, "%(destdir)s");
            shell_quote_for_recipe(&normalized)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn derive_draft_recipe(input: DraftRecipeInput) -> Result<String> {
    if input.command.is_empty() {
        bail!("draft recipe requires recorded command");
    }
    let rendered = render_recorded_command(&input.command, &input.recording_destdir);
    let step = if input.installed_files.is_empty() {
        format!("build = \"{rendered}\"")
    } else {
        format!("install = \"{rendered}\"")
    };
    let review_note = if input.network_likely {
        "# Review: network-like behavior was observed or could not be ruled out.\n"
    } else {
        ""
    };
    Ok(format!(
        r#"{review_note}[package]
name = "{name}"
version = "{version}"
release = "1"

[source]
path = "source"

[build]
{step}
"#,
        name = input.package_name,
        version = input.package_version,
    ))
}

/// Quote an argument for generated recipe text.
///
/// `%(destdir)s` syntax remains unquoted so normal Kitchen substitution can
/// replace it. Do not use this helper for live command execution.
fn shell_quote_for_recipe(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '%' | '(' | ')'))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
```

Update `crates/conary-core/src/recipe/recording/mod.rs`:

```rust
pub mod draft;
pub use draft::{derive_draft_recipe, render_recorded_command, DraftRecipeInput};
```

- [ ] **Step 3: Write failing CLI draft writer test**

Create `apps/conary/src/commands/record_mode/draft.rs`:

```rust
// apps/conary/src/commands/record_mode/draft.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materialize_draft_writes_recipe_under_output_dir() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("recorded/demo");
        std::fs::create_dir_all(output.join("source")).unwrap();

        let recipe_path = materialize_draft_recipe(DraftMaterialization {
            output_dir: output.clone(),
            package_name: "demo".to_string(),
            package_version: "0.1.0-recorded".to_string(),
            command: vec!["make".to_string(), "install".to_string()],
            recording_destdir: temp.path().join("destdir").to_string_lossy().to_string(),
            installed_files: vec!["usr/bin/demo".to_string()],
            network_likely: false,
        })
        .unwrap();

        assert_eq!(recipe_path, output.join("recipe.toml"));
        let text = std::fs::read_to_string(recipe_path).unwrap();
        assert!(text.contains("path = \"source\""));
    }
}
```

- [ ] **Step 4: Implement CLI draft materialization**

Add above tests:

```rust
use std::path::PathBuf;

use anyhow::Result;
use conary_core::recipe::recording::{derive_draft_recipe, DraftRecipeInput};

pub(crate) struct DraftMaterialization {
    pub(crate) output_dir: PathBuf,
    pub(crate) package_name: String,
    pub(crate) package_version: String,
    pub(crate) command: Vec<String>,
    pub(crate) recording_destdir: String,
    pub(crate) installed_files: Vec<String>,
    pub(crate) network_likely: bool,
}

pub(crate) fn materialize_draft_recipe(input: DraftMaterialization) -> Result<PathBuf> {
    std::fs::create_dir_all(&input.output_dir)?;
    let recipe = derive_draft_recipe(DraftRecipeInput {
        package_name: input.package_name,
        package_version: input.package_version,
        command: input.command,
        recording_destdir: input.recording_destdir,
        installed_files: input.installed_files,
        network_likely: input.network_likely,
    })?;
    let path = input.output_dir.join("recipe.toml");
    std::fs::write(&path, recipe)?;
    Ok(path)
}
```

- [ ] **Step 5: Add installed-file scan helper**

In `recording/draft.rs`, add a pure helper and tests:

```rust
pub fn installed_file_paths_from_evidence(
    files: &[crate::recipe::recording::InstalledFileEvidence],
) -> Vec<String> {
    let mut paths = files.iter().map(|file| file.path.clone()).collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}
```

Use this helper from CLI materialization when building `installed_files`.

- [ ] **Step 6: Run Task 8 verification**

Run:

```bash
cargo test -p conary-core recipe::recording::draft
cargo test -p conary --lib commands::record_mode::draft
```

Expected: all pass.

- [ ] **Step 7: Commit Task 8**

```bash
git add crates/conary-core/src/recipe/recording apps/conary/src/commands/record_mode
git commit -m "feat(record): derive draft recipe"
```

### Task 9: Recorded-Draft Validation Helper

**Files:**
- Modify: `apps/conary/src/commands/cook.rs`
- Create: `apps/conary/src/commands/record_mode/validation.rs`
- Modify: `apps/conary/src/commands/record_mode/mod.rs`
- Test: `apps/conary/src/commands/cook.rs`
- Test: `apps/conary/src/commands/record_mode/validation.rs`
- Test: `crates/conary-core/src/repository/static_repo/publish_gate.rs`

- [ ] **Step 1: Write failing cook helper test**

In `apps/conary/src/commands/cook.rs` tests, add:

```rust
#[test]
fn recorded_draft_validation_run_options_set_origin_override_and_isolation() {
    let options = CookRecordedDraftOptions {
        recipe: PathBuf::from("recorded/demo/recipe.toml"),
        output_dir: PathBuf::from("recorded/demo/dist"),
        source_cache: PathBuf::from("recorded/demo/sources"),
        operation_id: "record-1".to_string(),
    };
    let recipe = options.recipe.to_string_lossy().to_string();
    let output_dir = options.output_dir.to_string_lossy().to_string();
    let source_cache = options.source_cache.to_string_lossy().to_string();
    let run = recorded_draft_run_options(&options, &recipe, &output_dir, &source_cache);

    assert!(run.isolated);
    assert!(!run.no_isolation);
    assert_eq!(run.origin_class_override.as_deref(), Some("recorded-draft"));
}
```

- [ ] **Step 2: Add narrow cook validation helper**

In `apps/conary/src/commands/cook.rs`, extend the private `CookRunOptions`
with:

```rust
    origin_class_override: Option<String>,
```

Set the new field to `None` in ordinary `cmd_cook` and
`run_cook_for_try_watch` callers. When constructing `KitchenConfig`, use:

```rust
        origin_class_override: options
            .origin_class_override
            .clone()
            .or_else(|| resolved.origin_class_override.clone()),
```

Then add near `CookForTryWatchOptions`:

```rust
pub(crate) struct CookRecordedDraftOptions {
    pub(crate) recipe: PathBuf,
    pub(crate) output_dir: PathBuf,
    pub(crate) source_cache: PathBuf,
    pub(crate) operation_id: String,
}
```

Add a run-options helper that is used by the real validation path:

```rust
fn recorded_draft_run_options<'a>(
    options: &'a CookRecordedDraftOptions,
    recipe: &'a str,
    output_dir: &'a str,
    source_cache: &'a str,
) -> CookRunOptions<'a> {
    CookRunOptions {
        target: Some(recipe),
        recipe: None,
        output_dir,
        source_cache,
        jobs: None,
        keep_builddir: false,
        validate_only: false,
        fetch_only: false,
        explain: false,
        isolated: true,
        no_isolation: false,
        hermetic: false,
        json: true,
        operation_id: options.operation_id.clone(),
        source_download_policy_override: None,
        origin_class_override: Some("recorded-draft".to_string()),
    }
}
```

Add:

```rust
pub(crate) fn run_cook_for_recorded_draft(
    options: CookRecordedDraftOptions,
) -> Result<PackagingCommandOutput> {
    let mut sink = io::sink();
    let recipe = options.recipe.to_string_lossy().to_string();
    let output_dir = options.output_dir.to_string_lossy().to_string();
    let source_cache = options.source_cache.to_string_lossy().to_string();
    run_cook_operation(
        recorded_draft_run_options(&options, &recipe, &output_dir, &source_cache),
        &mut sink,
    )
}
```

Add a regression test proving ordinary cook paths still leave
`origin_class_override` as `None`.

- [ ] **Step 3: Write failing record validation tests**

Create `apps/conary/src/commands/record_mode/validation.rs`:

```rust
// apps/conary/src/commands/record_mode/validation.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_request_uses_dist_and_sources_under_output() {
        let output = std::path::PathBuf::from("recorded/demo");
        let request = validation_request(&output, "record-1");
        assert_eq!(request.recipe, std::path::PathBuf::from("recorded/demo/recipe.toml"));
        assert_eq!(request.output_dir, std::path::PathBuf::from("recorded/demo/dist"));
        assert_eq!(request.source_cache, std::path::PathBuf::from("recorded/demo/sources"));
    }
}
```

- [ ] **Step 4: Implement record validation wrapper**

Add above tests:

```rust
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::commands::cook::{run_cook_for_recorded_draft, CookRecordedDraftOptions};

pub(crate) fn validation_request(
    output_dir: &Path,
    operation_id: &str,
) -> CookRecordedDraftOptions {
    CookRecordedDraftOptions {
        recipe: output_dir.join("recipe.toml"),
        output_dir: output_dir.join("dist"),
        source_cache: output_dir.join("sources"),
        operation_id: operation_id.to_string(),
    }
}

pub(crate) fn validate_recorded_draft(
    output_dir: &Path,
    operation_id: &str,
) -> Result<conary_core::diagnostics::PackagingCommandOutput> {
    let request = validation_request(output_dir, operation_id);
    run_cook_for_recorded_draft(request)
}
```

- [ ] **Step 5: Verify publish-gate regression coverage**

In `crates/conary-core/src/repository/static_repo/publish_gate.rs`, verify
there is existing attested-path coverage for recorded-draft artifacts. The
current tree has a table case named like `recorded-draft artifacts are not
publishable`; keep that coverage instead of adding a duplicate. If the test is
missing or no longer checks an otherwise-valid attestation payload, add or
extend one that builds a lint context whose payload has:

```rust
payload.origin_class = "recorded-draft".to_string();
```

Assert:

```rust
assert_eq!(
    report.failures[0].code,
    PublishGateFailureCode::RecordedDraftArtifact
);
```

If there is already exact coverage for recorded-draft plus otherwise valid attestation, reference that test in the final task notes and do not duplicate it.

Generated recorded-draft validation artifacts may fail artifact-form publish
earlier with `MissingAttestation`. That CLI path is covered in Task 11; this
unit coverage must continue proving the specific `RecordedDraftArtifact` gate
for an otherwise-valid attested payload.

- [ ] **Step 6: Run Task 9 verification**

Run:

```bash
cargo test -p conary --lib commands::cook
cargo test -p conary --lib commands::record_mode::validation
cargo test -p conary-core repository::static_repo::publish_gate
```

Expected: all pass, and ordinary cook provenance tests still show no origin override.

- [ ] **Step 7: Commit Task 9**

```bash
git add apps/conary/src/commands/cook.rs apps/conary/src/commands/record_mode crates/conary-core/src/repository/static_repo/publish_gate.rs
git commit -m "feat(record): validate recorded drafts"
```

### Task 10: End-To-End Record Orchestration

**Files:**
- Modify: `apps/conary/src/commands/record_mode/mod.rs`
- Modify: `apps/conary/src/commands/record_mode/types.rs`
- Modify: `apps/conary/src/commands/record_mode/workspace.rs`
- Modify: `apps/conary/src/commands/record_mode/trace.rs`
- Modify: `apps/conary/src/commands/record_mode/inotify_backend.rs`
- Modify: `apps/conary/src/commands/record_mode/fanotify_backend.rs`
- Modify: `apps/conary/src/commands/record_mode/runner.rs`
- Modify: `apps/conary/src/commands/record_mode/report.rs`
- Modify: `apps/conary/src/commands/record_mode/draft.rs`
- Modify: `apps/conary/src/commands/record_mode/validation.rs`
- Test: `apps/conary/src/commands/record_mode/mod.rs`

- [ ] **Step 1: Write failing orchestration unit tests**

In `apps/conary/src/commands/record_mode/mod.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_id_uses_record_prefix() {
        let id = new_record_operation_id();
        assert!(id.starts_with("record-"));
    }

    #[test]
    fn default_output_dir_uses_source_directory_name() {
        assert_eq!(
            default_record_output_dir(std::path::Path::new("/tmp/demo")),
            std::path::PathBuf::from("recorded/demo")
        );
        assert_eq!(
            default_record_output_dir(std::path::Path::new(".")),
            std::path::PathBuf::from("recorded/source")
        );
    }
}
```

- [ ] **Step 2: Replace the stub with orchestration**

`cmd_cook_record` must:

1. Validate request.
2. Create `operation_id = new_record_operation_id()`.
3. Clean stale workspaces in the temp parent.
4. Create `RecordWorkspace`.
5. Build `TraceScope` from workspace source/work/install roots.
6. Select backend.
7. Start trace session.
8. Run command through `run_record_command`.
9. Drain and finish trace events.
10. Publish `source/` snapshot.
11. Scan installed files from `workspace.install_root`.
12. Build and write redacted report.
13. Materialize `recipe.toml`.
14. Run validation only when `request.validate` is true.
15. Write operation record through `commands::diagnostics::write_packaging_record_if_possible`.
16. Print concise human output or return JSON if a later task wires JSON into record mode.
17. Cleanup workspace.

When `request.unsafe_host` is true, print a loud warning to stderr before the
command starts:

```text
WARNING: executing record command directly on the host without sandboxing.
```

Also add `RecordingLimitation::UnsafeHost` to the report limitations. This
marker must survive even if the host command succeeds.

Use this helper:

```rust
fn new_record_operation_id() -> String {
    crate::commands::operation_records::new_operation_id("record")
}
```

Use this helper:

```rust
pub(crate) fn default_record_output_dir(source: &std::path::Path) -> std::path::PathBuf {
    let name = source
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("source");
    std::path::PathBuf::from("recorded").join(name)
}
```

- [ ] **Step 3: Add installed-file scanner**

In `report.rs` or `draft.rs`, add:

```rust
pub(crate) fn installed_file_evidence(
    install_root: &std::path::Path,
) -> anyhow::Result<Vec<conary_core::recipe::recording::InstalledFileEvidence>> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(install_root).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_dir() {
            continue;
        }
        let relative = entry.path().strip_prefix(install_root)?;
        if entry.file_type().is_symlink() {
            let link_target = std::fs::read_link(entry.path())?;
            files.push(conary_core::recipe::recording::InstalledFileEvidence {
                path: relative.to_string_lossy().to_string(),
                file_type: "symlink".to_string(),
                executable: false,
                size: 0,
                link_target: Some(link_target.to_string_lossy().to_string()),
            });
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let metadata = entry.metadata()?;
        files.push(conary_core::recipe::recording::InstalledFileEvidence {
            path: relative.to_string_lossy().to_string(),
            file_type: "file".to_string(),
            executable: executable_bit(&metadata),
            size: metadata.len(),
            link_target: None,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn executable_bit(metadata: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}
```

Use a cfg-gated executable-bit helper as above instead of a module-level
`std::os::unix::fs::PermissionsExt` import.

After scanning installed files, reconcile the scan with traced install writes:

- Build a set of installed evidence paths for `file_type == "file"` and
  `file_type == "symlink"`.
- Build a set of observed install paths where `scope == TraceScope::Install`
  and `operation` is `InstallCreate` or `InstallModify`.
- If any installed path has no matching observed write, add
  `RecordingLimitation::EventLoss` and an ignored-event/report diagnostic with
  reason `installed-scan-reconciled-missing-watch-event` and the missing count.

This makes the known recursive-inotify dynamic-directory race non-destructive:
the final installed scan still drives the draft recipe, while the report admits
that watch evidence was incomplete.

- [ ] **Step 4: Wire backend modules**

In `record_mode/mod.rs`:

```rust
mod draft;
mod fanotify_backend;
mod inotify_backend;
mod report;
mod runner;
mod trace;
mod validation;
mod workspace;
```

Use `FanotifyTraceBackend::new()` and `InotifyTraceBackend::new()` for backend selection. When auto selects `FanotifyInotify`, start fanotify for read evidence and inotify for mutation evidence if both are available; if the implementation keeps one combined session wrapper, name it `CombinedTraceSession`.

- [ ] **Step 5: Run Task 10 verification**

Run:

```bash
cargo test -p conary --lib commands::record_mode
```

Expected: unit tests pass. Some live sandbox paths may be skipped by integration test helpers if user namespaces are unavailable.

- [ ] **Step 6: Commit Task 10**

```bash
git add apps/conary/src/commands/record_mode
git commit -m "feat(record): orchestrate cook record mode"
```

### Task 11: Integration Coverage

**Files:**
- Create: `apps/conary/tests/packaging_m3d.rs`
- Test: `apps/conary/tests/packaging_m3d.rs`

- [ ] **Step 1: Create integration fixture helpers**

Create `apps/conary/tests/packaging_m3d.rs`:

```rust
mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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

fn write_record_source(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("payload.txt"), "hello record\n").unwrap();
    std::fs::write(
        root.join("install.sh"),
        r#"#!/bin/sh
set -eu
mkdir -p "$CONARY_DESTDIR/usr/share/record-demo"
cp payload.txt "$CONARY_DESTDIR/usr/share/record-demo/payload.txt"
"#,
    )
    .unwrap();
}
```

- [ ] **Step 2: Add hidden help and missing command tests**

Add:

```rust
#[test]
fn cook_record_is_hidden_and_requires_command() {
    let help = Command::new(env!("CARGO_BIN_EXE_conary"))
        .args(["cook", "--help"])
        .output()
        .expect("cook help");
    assert_success(&help);
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(!help_text.contains("--record"));

    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    write_record_source(&source);
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("cook")
        .arg("--record")
        .arg(&source)
        .output()
        .expect("cook record without command");
    assert_failure(&output);
    assert!(output_text(&output).contains("requires a command"));
}
```

- [ ] **Step 3: Add inotify-only draft/report test**

Add:

```rust
#[test]
fn cook_record_inotify_generates_source_recipe_and_redacted_report() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let recorded = temp.path().join("recorded/demo");
    write_record_source(&source);

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("cook")
        .arg("--record")
        .arg(&source)
        .args(["--record-backend", "inotify"])
        .arg("--record-output")
        .arg(&recorded)
        .arg("--")
        .arg("/bin/sh")
        .arg("install.sh")
        .output()
        .expect("cook record");

    assert_success(&output);
    assert!(recorded.join("source/payload.txt").is_file());
    let recipe = std::fs::read_to_string(recorded.join("recipe.toml")).unwrap();
    assert!(recipe.contains("path = \"source\""));
    assert!(recipe.contains("%(destdir)s"));
    assert!(!recipe.contains(temp.path().to_str().unwrap()));

    let report = std::fs::read_to_string(recorded.join("trace-report.json")).unwrap();
    assert!(report.contains("\"backend\""));
    assert!(report.contains("incomplete-read-evidence"));
    assert!(report.contains("usr/share/record-demo/payload.txt"));
    assert!(!report.contains(temp.path().to_str().unwrap()));
}
```

- [ ] **Step 4: Add reserved network flag test**

Add:

```rust
#[test]
fn cook_record_allow_network_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    write_record_source(&source);
    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("cook")
        .arg("--record")
        .arg("--record-allow-network")
        .arg(&source)
        .arg("--")
        .arg("/bin/true")
        .output()
        .expect("cook record allow network");
    assert_failure(&output);
    assert!(output_text(&output).contains("reserved"));
}
```

- [ ] **Step 5: Add validation and publish-refusal test**

Add:

```rust
#[test]
fn cook_record_validate_stamps_recorded_draft_artifact() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let recorded = temp.path().join("recorded/demo");
    write_record_source(&source);

    let output = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("cook")
        .arg("--record")
        .arg(&source)
        .args(["--record-backend", "inotify"])
        .arg("--record-output")
        .arg(&recorded)
        .arg("--record-validate")
        .arg("--")
        .arg("/bin/sh")
        .arg("install.sh")
        .output()
        .expect("cook record validate");

    assert_success(&output);
    let artifact = std::fs::read_dir(recorded.join("dist"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().and_then(|value| value.to_str()) == Some("ccs"))
        .expect("recorded validation artifact");
    let package = conary_core::ccs::CcsPackage::parse(&artifact.to_string_lossy()).unwrap();
    assert_eq!(
        package.manifest().provenance.as_ref().unwrap().origin_class,
        "recorded-draft"
    );

    let publish_repo = temp.path().join("repo");
    let publish_keys = temp.path().join("publish-keys");
    let publish_state = temp.path().join("publish-state.toml");
    let publish = Command::new(env!("CARGO_BIN_EXE_conary"))
        .arg("publish")
        .arg(&artifact)
        .arg(&publish_repo)
        .arg("--key-dir")
        .arg(&publish_keys)
        .arg("--state-file")
        .arg(&publish_state)
        .arg("--json")
        .output()
        .expect("publish recorded draft artifact");
    assert_failure(&publish);
    let value: serde_json::Value = serde_json::from_slice(&publish.stdout).expect("valid json");
    let failure_code = value["diagnostics"][0]["evidence"][0]["metadata"]
        ["publish_lint_report"]["failures"][0]["code"]
        .as_str()
        .expect("publish gate failure code");
    assert!(
        matches!(failure_code, "missing-attestation" | "recorded-draft-artifact"),
        "{}",
        output_text(&publish)
    );
    assert!(!publish_repo.exists(), "publish gate failure must not create repo");
}
```

The expected first failure for a generated validation artifact is usually
`missing-attestation`; the focused Task 9 unit coverage proves the more specific
`recorded-draft-artifact` failure for otherwise-valid attested payloads.

- [ ] **Step 6: Run Task 11 verification**

Run:

```bash
cargo test -p conary --test packaging_m3d
```

Expected: integration tests pass in the normal local test environment. If user namespaces or container sandboxing are unavailable, tests that require sandbox execution must skip with a clear stdout/stderr marker and the unit tests from Task 6 must still prove the mount contract.

- [ ] **Step 7: Commit Task 11**

```bash
git add apps/conary/tests/packaging_m3d.rs
git commit -m "test(record): cover cook record workflow"
```

### Task 12: Docs, Audit Ledger, And Final Verification

**Files:**
- Modify: `docs/superpowers/specs/2026-06-17-m3d-record-mode-spike-design.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Update M3d status after implementation passes**

Change the M3d spec status line:

```markdown
**Status:** Landed in M3d; implementation complete on `main`
```

- [ ] **Step 2: Add feature ownership routing**

In `docs/modules/feature-ownership.md`, update the packaging-toolchain card with:

```markdown
- Record-mode spike: start in `apps/conary/src/commands/record_mode/`, keep
  `apps/conary/src/commands/cook.rs` as a thin router/validator helper, and
  put reusable DTO/draft helpers under `crates/conary-core/src/recipe/recording/`.
- Focused proof: `cargo test -p conary --lib commands::record_mode`,
  `cargo test -p conary-core recipe::recording`, and
  `cargo test -p conary --test packaging_m3d`.
```

- [ ] **Step 3: Add assistant subsystem routing**

In `docs/llms/subsystem-map.md`, add record-mode routing near other packaging entries:

```markdown
- `conary cook --record` / record-mode spike: start in
  `apps/conary/src/commands/record_mode/`; use
  `crates/conary-core/src/recipe/recording/` for pure report/draft helpers and
  keep `apps/conary/src/commands/cook.rs` to routing plus recorded-draft
  validation helper work.
```

- [ ] **Step 4: Refresh docs audit inventory**

Run:

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

- [ ] **Step 5: Add or refresh docs audit ledger rows**

Ensure `docs/superpowers/documentation-accuracy-audit-ledger.tsv` has a retained row for:

```text
docs/superpowers/plans/2026-06-17-m3d-record-mode-spike-implementation-plan.md
```

Use claim clusters:

```text
packaging-toolchain; m3d; record-mode; implementation-plan; fanotify; inotify; recorded-draft
```

Use evidence sources:

```text
docs/superpowers/specs/2026-06-17-m3d-record-mode-spike-design.md; apps/conary/src/commands/record_mode/; apps/conary/src/commands/cook.rs; crates/conary-core/src/recipe/recording/; crates/conary-core/src/diagnostics/mod.rs; crates/conary-core/src/repository/static_repo/publish_gate.rs; apps/conary/tests/packaging_m3d.rs
```

Use notes:

```text
Added implementation plan for hidden cook --record record-mode spike, covering CLI routing, scoped fanotify/inotify tracing, sandboxed command execution, private source/work/install lifecycle, redacted reports, conservative draft recipe derivation, recorded-draft validation, publish-refusal proof, and docs-audit closure.
```

Refresh the M3d design row notes to mention implementation has landed once Task 12 runs after implementation.

- [ ] **Step 6: Run final verification**

Run:

```bash
cargo test -p conary-core diagnostics
cargo test -p conary-core recipe::recording
cargo test -p conary-core repository::static_repo::publish_gate
cargo test -p conary --lib cli::tests
cargo test -p conary --lib command_risk::tests
cargo test -p conary --lib dispatch::root
cargo test -p conary --lib commands::record_mode
cargo test -p conary --lib commands::cook
cargo test -p conary --test packaging_m3a
cargo test -p conary --test packaging_m3d
cargo fmt --check
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --check
```

Expected: all pass.

- [ ] **Step 7: Run merge gate**

Run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: passes with zero warnings.

- [ ] **Step 8: Commit Task 12**

```bash
git add docs/superpowers/specs/2026-06-17-m3d-record-mode-spike-design.md docs/modules/feature-ownership.md docs/llms/subsystem-map.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs(record): close m3d implementation"
```

---

## Self-Review

Spec coverage:

- Hidden CLI and reserved network flag: Task 1 and Task 11.
- Private workspace, public source snapshot, and cleanup: Task 3 and Task 10.
- Fanotify/inotify backend contract: Task 4 and Task 5.
- Backend lifecycle separated from command spawning: Task 4/5 traits and Task 6 runner.
- Sandbox mount/inode visibility and `DESTDIR`/`CONARY_DESTDIR`: Task 6 and Task 11.
- Redacted report and operation output: Task 7 and Task 10.
- Draft recipe derivation and destdir normalization: Task 8.
- Recorded-draft validation and publish refusal: Task 9 and Task 11.
- Docs audit closure: Task 12.

Type consistency:

- CLI request type is `RecordCliRequest`.
- Backend request type is `RequestedRecordBackend`.
- Core report type is `RecordingReport`.
- Trace traits are `TraceBackend` and `TraceSession`.
- Sandbox request type is `RecordCommandRequest`.
- Validation uses `CookRecordedDraftOptions`.

Implementation choice:

- M3d keeps `--record-allow-network` parsed but fail-closed. This preserves the reserved contract without widening the spike.
- M3d adds `CONARY_DESTDIR` only to the recorded demonstration command, not to normal Kitchen validation. Generated recipes normalize to `%(destdir)s`, so current normal cook behavior remains valid.
- Fanotify live behavior is gated by environment capability; inotify and fakeable probe tests carry normal CI.
