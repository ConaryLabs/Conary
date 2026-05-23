# Local Bootstrap Smoke Proof Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a bounded `conary-test bootstrap smoke` path that an assistant can run after `bootstrap check` to execute or preview one local developer smoke proof loop with structured contract output.

**Architecture:** Keep this slice local to `conary-test` and the transport-neutral contract. The new smoke command reuses the existing `conary-test run --suite phase1-core --distro fedora44 --phase 1` CLI path through an injectable command runner, so it does not duplicate container/test execution logic and does not add live MCP resources, tools, prompts, or discovery behavior. `bootstrap check` remains read-only; `bootstrap smoke` is an explicit local validation action with medium risk because it may build images, start containers, and write result files.

**Tech Stack:** Rust 2024, `conary-agent-contract`, existing `conary-test` CLI/test engine, `serde_json`, `clap`, `cargo test`.

---

## Goal-Mode Prompt

Use this text for Codex `/goal`:

```text
Implement docs/superpowers/plans/2026-05-22-local-bootstrap-smoke-proof-loop.md task-by-task. Keep the slice local-bootstrap only: add `conary-test bootstrap smoke`, structured contract output, tests, and docs. Do NOT add live MCP resources, tools, prompts, or discovery behavior. Write tests first, verify expected failures before implementation, make one focused commit per task, update checkboxes, and stop only when final acceptance passes.
```

## Constraints

- Do not add new live MCP registrations.
- Do not require cloud credentials.
- Do not publish packages or fixtures.
- Do not duplicate the existing test runner; invoke the existing run path.
- Default smoke target is `suite=phase1-core`, `distro=fedora44`, `phase=1`.
- The smoke command must support `--dry-run` so CI and agents can preview the exact command without starting containers.
- The command must emit a contract-shaped JSON result under global `--json`.

## Task 1: Add Bootstrap Smoke Contract Helpers

**Files:**
- Modify: `apps/conary-test/src/bootstrap.rs`

- [ ] **Step 1: Write failing tests for smoke command construction**

Add these tests to `apps/conary-test/src/bootstrap.rs`:

```rust
#[test]
fn smoke_options_default_to_phase1_core_fedora44() {
    let options = BootstrapSmokeOptions::default();
    assert_eq!(options.suite, "phase1-core");
    assert_eq!(options.distro, "fedora44");
    assert_eq!(options.phase, 1);
    assert!(!options.force);
    assert!(!options.dry_run);
}

#[test]
fn smoke_command_invokes_existing_run_path() {
    let exe = Path::new("/tmp/conary-test");
    let command = build_smoke_command(exe, &BootstrapSmokeOptions::default());
    assert_eq!(command.program, exe);
    assert_eq!(
        command.args,
        vec![
            "run",
            "--suite",
            "phase1-core",
            "--distro",
            "fedora44",
            "--phase",
            "1",
        ]
    );
}
```

- [ ] **Step 2: Run tests and verify they fail**

```bash
cargo test -p conary-test bootstrap::tests::smoke_options_default_to_phase1_core_fedora44
cargo test -p conary-test bootstrap::tests::smoke_command_invokes_existing_run_path
```

Expected: FAIL because `BootstrapSmokeOptions` and `build_smoke_command` do not exist yet.

- [ ] **Step 3: Implement minimal smoke helper types**

Add near the top-level bootstrap types:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapSmokeOptions {
    pub suite: String,
    pub distro: String,
    pub phase: u32,
    pub dry_run: bool,
    pub force: bool,
}

impl Default for BootstrapSmokeOptions {
    fn default() -> Self {
        Self {
            suite: "phase1-core".to_string(),
            distro: "fedora44".to_string(),
            phase: 1,
            dry_run: false,
            force: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapSmokeCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
}

pub fn build_smoke_command(exe: &Path, options: &BootstrapSmokeOptions) -> BootstrapSmokeCommand {
    BootstrapSmokeCommand {
        program: exe.to_path_buf(),
        args: vec![
            "run".to_string(),
            "--suite".to_string(),
            options.suite.clone(),
            "--distro".to_string(),
            options.distro.clone(),
            "--phase".to_string(),
            options.phase.to_string(),
        ],
    }
}
```

- [ ] **Step 4: Run tests and commit**

```bash
cargo test -p conary-test bootstrap::tests::smoke_options_default_to_phase1_core_fedora44
cargo test -p conary-test bootstrap::tests::smoke_command_invokes_existing_run_path
git add apps/conary-test/src/bootstrap.rs docs/superpowers/plans/2026-05-22-local-bootstrap-smoke-proof-loop.md
git commit -m "feat(test): define bootstrap smoke command"
```

## Task 2: Add Dry-Run And Readiness Gating

**Files:**
- Modify: `apps/conary-test/src/bootstrap.rs`

- [ ] **Step 1: Write failing tests for dry-run and not-ready output**

Add:

```rust
#[test]
fn smoke_dry_run_returns_planned_command_without_execution() {
    let mut options = BootstrapSmokeOptions::default();
    options.dry_run = true;
    let inspect = ready_bootstrap_report();
    let report = smoke_with_runner(&inspect, &options, |_command| {
        panic!("dry-run must not execute the smoke command")
    });

    assert_eq!(report.envelope.status, OperationStatus::Planned);
    assert_eq!(report.envelope.risk, RiskLevel::Medium);
    assert_eq!(report.data["dry_run"], true);
    assert_eq!(report.data["command"]["args"][0], "run");
}

#[test]
fn smoke_refuses_when_bootstrap_check_is_not_ready() {
    let mut inspect = ready_bootstrap_report();
    inspect.data["default_smoke_candidate"]["ready"] = serde_json::json!(false);
    let report = smoke_with_runner(&inspect, &BootstrapSmokeOptions::default(), |_command| {
        panic!("not-ready smoke must not execute")
    });

    assert_eq!(report.envelope.status, OperationStatus::Unavailable);
    assert_eq!(report.data["executed"], false);
    assert!(
        report
            .envelope
            .warnings
            .iter()
            .any(|warning| warning.contains("bootstrap check is not ready"))
    );
}
```

Add a test helper:

```rust
fn ready_bootstrap_report() -> InspectResult {
    let root = tempdir().unwrap();
    let manifests = root.path().join("manifests");
    std::fs::create_dir_all(&manifests).unwrap();
    write_valid_manifest(&manifests.join("phase1-core.toml"));
    let config = root.path().join("config.toml");
    write_valid_config(&config);
    inspect_with_paths_and_probe(root.path(), &manifests, &config, ready_probe())
}
```

If returning `InspectResult` from a tempdir helper runs into lifetime cleanup problems, keep the `TempDir` in a helper struct:

```rust
struct ReadyBootstrapFixture {
    _root: tempfile::TempDir,
    report: InspectResult,
}
```

- [ ] **Step 2: Run tests and verify they fail**

```bash
cargo test -p conary-test bootstrap::tests::smoke_dry_run_returns_planned_command_without_execution
cargo test -p conary-test bootstrap::tests::smoke_refuses_when_bootstrap_check_is_not_ready
```

Expected: FAIL because `smoke_with_runner` does not exist.

- [ ] **Step 3: Implement dry-run and readiness gating**

Add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmokeCommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn smoke_with_runner(
    inspect: &InspectResult,
    options: &BootstrapSmokeOptions,
    mut runner: impl FnMut(&BootstrapSmokeCommand) -> SmokeCommandOutput,
) -> conary_agent_contract::VerifyResult {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("conary-test"));
    let command = build_smoke_command(&exe, options);
    let mut envelope = OperationEnvelope::new(
        "conary-test.bootstrap.smoke",
        OperationStatus::Planned,
        RiskLevel::Medium,
        "Local Conary developer bootstrap smoke proof loop",
    );
    envelope.subject = Some(local_bootstrap_status());

    let command_json = serde_json::json!({
        "program": command.program.display().to_string(),
        "args": command.args.clone(),
    });

    let ready = inspect
        .data
        .pointer("/default_smoke_candidate/ready")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    if options.dry_run {
        return conary_agent_contract::VerifyResult::new(envelope).with_data(serde_json::json!({
            "dry_run": true,
            "executed": false,
            "command": command_json,
        }));
    }

    if !ready && !options.force {
        envelope.status = OperationStatus::Unavailable;
        envelope
            .warnings
            .push("bootstrap check is not ready; rerun bootstrap check or use --force".to_string());
        return conary_agent_contract::VerifyResult::new(envelope).with_data(serde_json::json!({
            "dry_run": false,
            "executed": false,
            "command": command_json,
        }));
    }

    let output = runner(&command);
    let status = if output.exit_code == 0 {
        OperationStatus::Ok
    } else {
        OperationStatus::Failed
    };
    envelope.status = status;
    envelope.evidence.push(EvidenceItem {
        kind: EvidenceKind::Command,
        summary: format!("bootstrap smoke exited {}", output.exit_code),
        uri: None,
        path: None,
        id: Some("bootstrap-smoke".to_string()),
        command: Some(
            std::iter::once(command.program.display().to_string())
                .chain(command.args.iter().cloned())
                .collect(),
        ),
        exit_code: Some(output.exit_code),
        metadata: Default::default(),
    });

    conary_agent_contract::VerifyResult::new(envelope).with_data(serde_json::json!({
        "dry_run": false,
        "executed": true,
        "command": command_json,
        "exit_code": output.exit_code,
        "stdout": output.stdout,
        "stderr": output.stderr,
    }))
}
```

- [ ] **Step 4: Run tests and commit**

```bash
cargo test -p conary-test bootstrap::tests::smoke_dry_run_returns_planned_command_without_execution
cargo test -p conary-test bootstrap::tests::smoke_refuses_when_bootstrap_check_is_not_ready
git add apps/conary-test/src/bootstrap.rs docs/superpowers/plans/2026-05-22-local-bootstrap-smoke-proof-loop.md
git commit -m "feat(test): gate bootstrap smoke execution"
```

## Task 3: Execute Smoke Through The Current Binary

**Files:**
- Modify: `apps/conary-test/src/bootstrap.rs`

- [ ] **Step 1: Write failing command-runner tests**

Add:

```rust
#[test]
fn smoke_success_records_command_evidence() {
    let fixture = ready_bootstrap_fixture();
    let report = smoke_with_runner(&fixture.report, &BootstrapSmokeOptions::default(), |_command| {
        SmokeCommandOutput {
            exit_code: 0,
            stdout: r#"{"suite":"phase1-core","status":"passed"}"#.to_string(),
            stderr: String::new(),
        }
    });

    assert_eq!(report.envelope.status, OperationStatus::Ok);
    assert_eq!(report.envelope.evidence[0].kind, EvidenceKind::Command);
    assert_eq!(report.data["executed"], true);
    assert_eq!(report.data["exit_code"], 0);
}

#[test]
fn smoke_failure_records_failed_status_and_stderr() {
    let fixture = ready_bootstrap_fixture();
    let report = smoke_with_runner(&fixture.report, &BootstrapSmokeOptions::default(), |_command| {
        SmokeCommandOutput {
            exit_code: 2,
            stdout: String::new(),
            stderr: "container runtime unavailable".to_string(),
        }
    });

    assert_eq!(report.envelope.status, OperationStatus::Failed);
    assert_eq!(report.data["stderr"], "container runtime unavailable");
}
```

- [ ] **Step 2: Run tests and verify they fail if helpers are incomplete**

```bash
cargo test -p conary-test bootstrap::tests::smoke_success_records_command_evidence
cargo test -p conary-test bootstrap::tests::smoke_failure_records_failed_status_and_stderr
```

Expected: FAIL until fixture/helper names match the implementation.

- [ ] **Step 3: Add real execution wrapper**

Add:

```rust
pub fn run_smoke(options: &BootstrapSmokeOptions) -> conary_agent_contract::VerifyResult {
    let inspect = inspect_default();
    smoke_with_runner(&inspect, options, |command| {
        let output = Command::new(&command.program).args(&command.args).output();
        match output {
            Ok(output) => SmokeCommandOutput {
                exit_code: output.status.code().unwrap_or(1),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            },
            Err(error) => SmokeCommandOutput {
                exit_code: 127,
                stdout: String::new(),
                stderr: error.to_string(),
            },
        }
    })
}
```

- [ ] **Step 4: Run tests and commit**

```bash
cargo test -p conary-test bootstrap
git add apps/conary-test/src/bootstrap.rs docs/superpowers/plans/2026-05-22-local-bootstrap-smoke-proof-loop.md
git commit -m "feat(test): execute bootstrap smoke command"
```

## Task 4: Wire `conary-test bootstrap smoke`

**Files:**
- Modify: `apps/conary-test/src/cli.rs`
- Modify: `apps/conary-test/src/bootstrap.rs` only if CLI needs a small display helper

- [ ] **Step 1: Write failing CLI tests**

Add tests near the existing CLI tests in `apps/conary-test/src/cli.rs`:

```rust
#[test]
fn cli_accepts_bootstrap_smoke_dry_run() {
    Cli::try_parse_from([
        "conary-test",
        "--json",
        "bootstrap",
        "smoke",
        "--dry-run",
    ])
    .expect("bootstrap smoke dry-run should parse");
}

#[test]
fn cli_accepts_bootstrap_smoke_overrides() {
    Cli::try_parse_from([
        "conary-test",
        "bootstrap",
        "smoke",
        "--suite",
        "phase1-core",
        "--distro",
        "fedora44",
        "--phase",
        "1",
        "--force",
    ])
    .expect("bootstrap smoke overrides should parse");
}
```

- [ ] **Step 2: Run tests and verify they fail**

```bash
cargo test -p conary-test cli_accepts_bootstrap_smoke
```

Expected: FAIL because the subcommand does not exist.

- [ ] **Step 3: Add CLI subcommand**

Extend `BootstrapCommands`:

```rust
enum BootstrapCommands {
    /// Check local prerequisites and emit structured bootstrap status
    Check,

    /// Run or preview the default local developer smoke proof loop
    Smoke {
        #[arg(long, default_value = "phase1-core")]
        suite: String,
        #[arg(long, default_value = "fedora44")]
        distro: String,
        #[arg(long, default_value = "1")]
        phase: u32,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        force: bool,
    },
}
```

Add dispatch:

```rust
Commands::Bootstrap {
    command:
        BootstrapCommands::Smoke {
            suite,
            distro,
            phase,
            dry_run,
            force,
        },
} => {
    let report = conary_test::bootstrap::run_smoke(&conary_test::bootstrap::BootstrapSmokeOptions {
        suite,
        distro,
        phase,
        dry_run,
        force,
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{}", report.envelope.summary);
        println!("status: {:?}", report.envelope.status);
        for warning in &report.envelope.warnings {
            println!("warning: {warning}");
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run CLI tests and dry-run command**

```bash
cargo test -p conary-test cli_accepts_bootstrap_smoke
cargo run -p conary-test -- bootstrap smoke --dry-run --json
```

Expected: tests pass, command emits JSON with `operation = "conary-test.bootstrap.smoke"`, `status = "planned"`, and `data.command.args` containing `run --suite phase1-core --distro fedora44 --phase 1`.

- [ ] **Step 5: Commit**

```bash
git add apps/conary-test/src/cli.rs apps/conary-test/src/bootstrap.rs docs/superpowers/plans/2026-05-22-local-bootstrap-smoke-proof-loop.md
git commit -m "feat(test): add bootstrap smoke cli"
```

## Task 5: Update Docs And Assistant Map

**Files:**
- Modify: `apps/conary-test/README.md`
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/llms/README.md` if assistant routing needs the new smoke command

- [ ] **Step 1: Update docs**

Document:

```bash
cargo run -p conary-test -- bootstrap check --json
cargo run -p conary-test -- bootstrap smoke --dry-run --json
cargo run -p conary-test -- bootstrap smoke --json
```

Text must say `bootstrap smoke` may build images, start containers, and write result files. It is not package publishing and does not require cloud credentials.

- [ ] **Step 2: Run docs/stale scans**

```bash
rg -n "bootstrap check|bootstrap smoke|smoke-readiness" apps/conary-test/README.md docs/INTEGRATION-TESTING.md docs/llms/README.md
rg -n "publish fixtures|cloud credentials|live MCP resources|live MCP prompts" apps/conary-test/README.md docs/INTEGRATION-TESTING.md docs/llms/README.md
```

Expected: command references are current; no doc implies `bootstrap smoke` publishes fixtures or adds live MCP surface area.

- [ ] **Step 3: Commit**

```bash
git add apps/conary-test/README.md docs/INTEGRATION-TESTING.md docs/llms/README.md docs/superpowers/plans/2026-05-22-local-bootstrap-smoke-proof-loop.md
git commit -m "docs(test): document bootstrap smoke proof loop"
```

## Task 6: Final Verification

**Files:**
- All touched files.

- [ ] **Step 1: Run formatting**

```bash
cargo fmt --check
```

- [ ] **Step 2: Run focused tests**

```bash
cargo test -p conary-test bootstrap
cargo test -p conary-test cli_accepts_bootstrap_smoke
cargo run -p conary-test -- bootstrap check --json
cargo run -p conary-test -- bootstrap smoke --dry-run --json
cargo run -p conary-test -- list
```

- [ ] **Step 3: Run lint**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 4: Run final status checks**

```bash
rg -n "conary-test bootstrap smoke|bootstrap smoke --dry-run|conary-local://bootstrap/status" apps/conary-test/README.md docs/INTEGRATION-TESTING.md docs/llms/README.md apps/conary-test/src
git diff --check
git status --short
git log --oneline -8
```

- [ ] **Step 5: Request review**

Use `/review` or dispatch a code-review subagent with this scope:

```text
Review the local bootstrap smoke proof loop. Focus on whether `bootstrap smoke` is bounded, contract-shaped, does not publish packages or require cloud credentials, does not add live MCP surface area, correctly gates on bootstrap readiness unless forced, and reuses the existing test runner without duplicating execution logic.
```

Address Critical and Important findings before marking the goal complete.

## Final Acceptance Criteria

- `conary-test bootstrap smoke --dry-run --json` emits a contract-shaped planned result and does not start containers.
- `conary-test bootstrap smoke --json` gates on `bootstrap check` readiness unless `--force` is set.
- Smoke execution reuses the existing `conary-test run` path.
- The command reports command evidence, exit code, stdout, and stderr in structured JSON.
- Docs explain check vs smoke, smoke side effects, and the no-publishing/no-cloud-credentials boundary.
- No live MCP resource/tool/prompt/discovery behavior is added.
- Final verification commands pass.
