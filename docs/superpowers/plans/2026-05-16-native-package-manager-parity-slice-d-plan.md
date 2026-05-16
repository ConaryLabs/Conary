# Native Package Manager Parity Slice D Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the `conary-test` native package-manager parity matrix proving Tier 0 and Tier 1 package-manager behavior across Fedora 44/RPM, Ubuntu 26.04/DEB, and Arch/package format.

**Architecture:** Treat this as a `/goal`-driven validation slice, not a product-feature grab bag. First make the harness honest enough to support release evidence, then add one named Phase 4 suite whose tests are explicit, non-skipped, and distro-parameterized. Product code changes are allowed only when the matrix exposes a real parity bug that earlier slices missed.

**Tech Stack:** Rust workspace, `conary-test` manifest runner, Podman-backed distro containers, TOML manifests, shell assertions inside containers, SQLite-backed Conary state, focused Rust tests for harness behavior.

---

## `/goal` Contract

Create the goal with this exact objective:

```text
Slice D: Add the conary-test native package-manager parity matrix. Create a named suite visible from `cargo run -p conary-test -- list` that proves Tier 0 and Tier 1 across Fedora 44/RPM, Ubuntu 26.04/DEB, and Arch package format with explicit step assertions, zero failed/skipped/cancelled result gates, forced deferred-follow-up tests, and exact native fixture metadata checks.
```

The goal is complete only when all of these are true:

- `cargo run -p conary-test -- list` shows `Phase 4: Native Package Manager Parity Matrix`.
- These commands pass and produce result summaries with `failed = 0`, `skipped = 0`, and `cancelled = 0`:

```bash
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro fedora44 --phase 4
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro ubuntu-26.04 --phase 4
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro arch --phase 4
```

- The matrix proves the same contract on every distro: repository sync/search, local native install, repository install, list/info/files/path ownership, pin blocking, unpin, update through `conary update`, remove, autoremove, history, `query whatprovides`, `query whatbreaks`, security-metadata honesty, and deferred follow-up reporting.
- Native fixture assertions include exact package name, version, architecture, version scheme, source distro or repository, file checksums, provider metadata, dependency metadata, config file metadata, and scriptlet metadata when exposed by the format.
- No release evidence relies on implicit setup behavior, implicit exit-code success, or a skipped test.
- The docs and audit ledger point at this plan and the new suite.
- Final verification passes: `cargo fmt --check`, `git diff --check`, focused Rust tests, `cargo run -p conary-test -- list`, matrix result gate checks for all three distros, audit ledger check, and `cargo clippy --workspace --all-targets -- -D warnings`.

When using `/goal`, do not mark the goal complete after the first distro passes. Update progress after each task, but call `update_goal(status = complete)` only after the full three-distro matrix and final verification pass on the branch intended for merge.

## `/goal` Operating Model

Run this plan as one active Codex goal with the main session acting as controller. The controller owns goal status, final evidence, merge/push/cleanup, and the decision to split or repair tasks when the matrix exposes a product bug. Worker agents own bounded implementation tasks and commits only.

Before implementation starts, confirm the active goal matches the objective above. If no goal exists, create it with the exact objective. If a different goal exists, stop and ask the user to clear or replace it instead of silently reusing the wrong goal.

Use this progress checklist as the child plan beneath the goal:

- [ ] Foundation: Tasks 1-3 are committed, reviewed, and locally verified.
- [ ] Manifest: Task 4 creates `phase4-native-pm-parity` and `cargo run -p conary-test -- list` shows the named suite.
- [ ] Fedora gate: Fedora matrix run passes and `scripts/check-conary-test-result-gate.sh` reports `ok`.
- [ ] Ubuntu gate: Ubuntu matrix run passes and the result gate reports `ok`.
- [ ] Arch gate: Arch matrix run passes and the result gate reports `ok`.
- [ ] Docs and audit: docs name the suite, record real evidence only after it exists, and the audit ledger check passes.
- [ ] Final verification: formatting, diff check, focused tests, list, three result gates, audit ledger, and clippy pass.
- [ ] Merge/push/cleanup: branch is fast-forwarded to `main`, pushed, post-merge verification runs from `main`, and local artifacts/processes are cleaned up.

After each committed task, update the child plan with the commit SHA, verification commands, and reviewer outcome. Do not use `update_goal(status = complete)` for progress updates; that call is reserved for the final verification point in Task 7.

When dispatching subagents, include these goal constraints in every prompt:

- The active goal is Slice D native package-manager parity matrix evidence.
- The worker has exactly one write-ownership slice and must not modify other slices unless explicitly redirected.
- The worker must commit its own completed task and report the commit SHA.
- The worker must run the task's verification commands and report exact pass/fail outcomes.
- The worker must not skip tests, weaken exact metadata assertions, or mark a distro as passing without the result gate.
- If the task exposes a real package-manager bug, the worker should report `BLOCKED` or `DONE_WITH_CONCERNS` with the smallest observed reproduction instead of papering over it in the manifest.

If a distro failure requires product code, keep the goal active and patch the smallest real root cause. Add focused unit or integration coverage for that bug, rerun the failed distro, then continue the same goal checklist. Do not split the goal unless the fix becomes unrelated to Tier 0/Tier 1 parity evidence.

## Subagent Boundaries

Use one worker per task when running this plan subagent-driven. Keep write ownership disjoint:

- Worker A owns `apps/conary-test/src/engine/runner.rs`, `apps/conary-test/src/report/json.rs`, and their Rust tests.
- Worker B owns `apps/conary/tests/fixtures/native/*` and any fixture metadata helper scripts.
- Worker C owns `apps/conary/tests/integration/remi/manifests/phase4-native-pm-parity.toml`.
- Worker D owns docs, result-gate script, and final verification plumbing.

Workers are not alone in the codebase. They must not revert edits made by other workers, and they must adjust to neighboring changes instead of overwriting them.

## Files

- Create: `docs/superpowers/plans/2026-05-16-native-package-manager-parity-slice-d-plan.md`
- Create: `apps/conary/tests/integration/remi/manifests/phase4-native-pm-parity.toml`
- Create: `scripts/check-conary-test-result-gate.sh`
- Modify: `apps/conary-test/src/engine/runner.rs`
- Modify: `apps/conary-test/src/report/json.rs`
- Modify: `apps/conary/tests/fixtures/native/build-native-fixtures.sh`
- Modify: `apps/conary/tests/integration/remi/config.toml` if shared distro fixture variables reduce repeated manifest overrides
- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/superpowers/specs/2026-05-14-native-package-manager-parity-matrix-design.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

---

## Task 1: Make Harness Evidence Honest

**Files:**

- Modify: `apps/conary-test/src/engine/runner.rs`
- Modify: `apps/conary-test/src/report/json.rs`
- Test: `apps/conary-test/src/engine/runner.rs`
- Test: `apps/conary-test/src/report/json.rs`

- [ ] **Step 1: Add a failing runner test proving suite setup executes before tests**

Add this test near the existing runner tests in `apps/conary-test/src/engine/runner.rs`:

```rust
#[tokio::test]
async fn suite_setup_executes_before_tests() {
    let backend = MockBackend::new(vec![
        ExecResult {
            exit_code: 0,
            stdout: "setup ok".to_string(),
            stderr: String::new(),
        },
        ExecResult {
            exit_code: 0,
            stdout: "test ok".to_string(),
            stderr: String::new(),
        },
    ]);

    let manifest = TestManifest {
        suite: SuiteDef {
            name: "setup-suite".to_string(),
            phase: 4,
            setup: vec![simple_step_run(
                "echo setup ok",
                Some(make_assertion(Some(0), Some("setup ok"))),
            )],
            mock_server: None,
            timeout: None,
        },
        test: vec![TestDef {
            id: "TSETUP".to_string(),
            name: "uses_setup".to_string(),
            description: "setup should run before tests".to_string(),
            timeout: 30,
            flaky: None,
            retries: None,
            retry_delay_ms: None,
            step: vec![simple_step_run(
                "echo test ok",
                Some(make_assertion(Some(0), Some("test ok"))),
            )],
            resources: None,
            depends_on: None,
            fatal: None,
            group: None,
            skip: None,
            requires: Vec::new(),
        }],
        distro_overrides: HashMap::new(),
    };

    let mut runner = TestRunner::new(test_config(), "fedora44".to_string());
    let suite = runner
        .run(&manifest, &backend, &"ctr-setup".to_string(), None)
        .await
        .unwrap();

    assert_eq!(suite.passed(), 1);
    assert_eq!(suite.failed(), 0);
}
```

- [ ] **Step 2: Run the failing runner test**

Run:

```bash
cargo test -p conary-test suite_setup_executes_before_tests -- --nocapture
```

Expected before implementation: fail because the first mocked output is consumed by the test step instead of by suite setup.

- [ ] **Step 3: Execute `suite.setup` before manifest tests**

In `TestRunner::run_with_cancel`, after mock server startup and before creating or emitting the suite start event, execute every `manifest.suite.setup` step with the same executor and assertion path used by test steps. Use a helper so setup behavior is testable and does not duplicate `run_test_once`.

Add:

```rust
    async fn run_setup_steps(
        &self,
        manifest: &TestManifest,
        backend: &dyn ContainerBackend,
        container_id: &ContainerId,
    ) -> Result<()> {
        let ctx = ExecutionContext {
            conary_bin: &self.config.paths.conary_bin,
            db_path: &self.config.paths.db,
        };

        for step in &manifest.suite.setup {
            let action = StepAction::from_step(step, &self.vars)
                .ok_or_else(|| anyhow::anyhow!("suite setup step has no recognized type"))?;
            let timeout = Duration::from_secs(step.timeout.unwrap_or(300));
            let result = execute_step(&action, backend, container_id, &ctx, timeout).await?;

            if let Some(msg) = result.failure {
                bail!("suite setup failed: {msg}");
            }

            if let Some(ref assertion) = step.assert {
                let assertion = self.expand_assertion(assertion);
                evaluate_assertion(&assertion, result.exit_code, &result.stdout, &result.stderr)
                    .map_err(|err| anyhow::anyhow!("suite setup assertion failed: {err}"))?;
            }
        }

        Ok(())
    }
```

Then call:

```rust
self.run_setup_steps(manifest, backend, container_id).await?;
```

This makes existing setup blocks real. Steps without assertions keep their current permissive behavior for cleanup commands such as `repo remove ... || true`.

- [ ] **Step 4: Add cancelled count to JSON reports**

In `apps/conary-test/src/report/json.rs`, add `cancelled` to `Summary`:

```rust
#[derive(Serialize)]
struct Summary {
    total: usize,
    passed: usize,
    failed: usize,
    skipped: usize,
    cancelled: usize,
}
```

Populate it:

```rust
cancelled: suite.cancelled(),
```

Update `test_json_report_format`:

```rust
assert_eq!(parsed["summary"]["cancelled"], 0);
```

- [ ] **Step 5: Run harness tests**

Run:

```bash
cargo test -p conary-test suite_setup_executes_before_tests -- --nocapture
cargo test -p conary-test test_json_report_format -- --nocapture
```

Expected: both pass.

- [ ] **Step 6: Commit**

```bash
git add apps/conary-test/src/engine/runner.rs apps/conary-test/src/report/json.rs
git commit -m "test(harness): execute suite setup in conary-test"
```

---

## Task 2: Add A Result Gate For Release Evidence

**Files:**

- Create: `scripts/check-conary-test-result-gate.sh`

- [ ] **Step 1: Create the result-gate script**

Create `scripts/check-conary-test-result-gate.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 <result.json> [<result.json> ...]" >&2
  exit 64
fi

for result in "$@"; do
  if [[ ! -f "$result" ]]; then
    echo "missing result file: $result" >&2
    exit 1
  fi

  python3 - "$result" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
data = json.loads(path.read_text())
summary = data.get("summary", {})
results = data.get("results", [])

failed = int(summary.get("failed", 0))
skipped = int(summary.get("skipped", 0))
cancelled = int(summary.get("cancelled", 0))

if "cancelled" not in summary:
    cancelled = sum(1 for item in results if item.get("status") == "cancelled")

bad = []
if failed:
    bad.append(f"failed={failed}")
if skipped:
    bad.append(f"skipped={skipped}")
if cancelled:
    bad.append(f"cancelled={cancelled}")

if bad:
    print(f"{path}: release gate failed: {', '.join(bad)}", file=sys.stderr)
    for item in results:
        if item.get("status") in {"failed", "skipped", "cancelled"}:
            print(
                f"  {item.get('id', '<unknown>')} {item.get('name', '<unknown>')}: "
                f"{item.get('status')} {item.get('message', '')}",
                file=sys.stderr,
            )
    sys.exit(1)

print(f"{path}: ok")
PY
done
```

- [ ] **Step 2: Verify the script catches failures, skips, and cancellations**

Run:

```bash
tmpdir="$(mktemp -d)"
printf '%s\n' '{"summary":{"failed":0,"skipped":0,"cancelled":0},"results":[]}' > "$tmpdir/pass.json"
printf '%s\n' '{"summary":{"failed":1,"skipped":0,"cancelled":0},"results":[{"id":"T1","name":"bad","status":"failed","message":"boom"}]}' > "$tmpdir/fail.json"
bash scripts/check-conary-test-result-gate.sh "$tmpdir/pass.json"
! bash scripts/check-conary-test-result-gate.sh "$tmpdir/fail.json"
rm -rf "$tmpdir"
```

Expected: the passing file prints `ok`; the failing file exits non-zero and prints `release gate failed: failed=1`.

- [ ] **Step 3: Commit**

```bash
git add scripts/check-conary-test-result-gate.sh
git commit -m "test(harness): add conary-test result gate"
```

---

## Task 3: Tighten Native Fixture Metadata

**Files:**

- Modify: `apps/conary/tests/fixtures/native/build-native-fixtures.sh`

- [ ] **Step 1: Make fixture builds emit metadata**

After the generated package file check in `build-native-fixtures.sh`, write a small env file beside the artifact so later manifest steps can reuse the exact path and checksum:

```bash
artifact="$(find "${output_dir}" -maxdepth 1 -type f -name "*${expected_suffix}" | sort | head -1)"
checksum="$(sha256sum "${artifact}" | awk '{print $1}')"
cat > "${output_dir}/native-fixture.env" <<EOF
NATIVE_PKG_FILE=${artifact}
NATIVE_PKG_SHA256=${checksum}
NATIVE_TARGET=${target}
EOF
```

Do not remove the existing suffix check; the env file depends on it.

- [ ] **Step 2: Verify all three fixture targets build locally**

Run:

```bash
tmpdir="$(mktemp -d)"
CONARY_BIN=target/debug/conary bash apps/conary/tests/fixtures/native/build-native-fixtures.sh rpm "$tmpdir/rpm" apps/conary/tests/fixtures/phase4-runtime-fixture
CONARY_BIN=target/debug/conary bash apps/conary/tests/fixtures/native/build-native-fixtures.sh deb "$tmpdir/deb" apps/conary/tests/fixtures/phase4-runtime-fixture
CONARY_BIN=target/debug/conary bash apps/conary/tests/fixtures/native/build-native-fixtures.sh arch "$tmpdir/arch" apps/conary/tests/fixtures/phase4-runtime-fixture
test -s "$tmpdir/rpm/native-fixture.env"
test -s "$tmpdir/deb/native-fixture.env"
test -s "$tmpdir/arch/native-fixture.env"
rm -rf "$tmpdir"
```

Expected: every target creates one native package and a `native-fixture.env` file.

- [ ] **Step 3: Commit**

```bash
git add apps/conary/tests/fixtures/native/build-native-fixtures.sh
git commit -m "test(fixtures): record native fixture metadata"
```

---

## Task 4: Add The Three-Distro Matrix Manifest

**Files:**

- Create: `apps/conary/tests/integration/remi/manifests/phase4-native-pm-parity.toml`

- [ ] **Step 1: Create the manifest header and distro overrides**

Create `apps/conary/tests/integration/remi/manifests/phase4-native-pm-parity.toml` with:

```toml
# tests/integration/remi/manifests/phase4-native-pm-parity.toml
#
# Phase 4 native package-manager parity matrix.
# This suite is the Slice D release evidence for Conary-owned package flows.

[suite]
name = "Phase 4: Native Package Manager Parity Matrix"
phase = 4
timeout = 3600

[[suite.setup]]
conary = "system init"

[suite.setup.assert]
exit_code = 0

[[suite.setup]]
run = "${CONARY_BIN} repo remove ${REPO_NAME} --db-path ${DB_PATH} >/dev/null 2>&1 || true"

[[suite.setup]]
conary = "repo add ${REPO_NAME} ${REMI_ENDPOINT} --default-strategy remi --remi-endpoint ${REMI_ENDPOINT} --remi-distro ${REMI_DISTRO} --no-gpg-check"

[suite.setup.assert]
exit_code = 0

[distro_overrides.fedora44]
native_target = "rpm"
native_glob = "*.rpm"
native_arch = "x86_64"
native_scheme = "rpm"
native_distro = "fedora"
native_fixture_version = "1.0.0-1"
repo_install_pkg = "tree"
repo_install_path = "/usr/bin/tree"

[distro_overrides."ubuntu-26.04"]
native_target = "deb"
native_glob = "*.deb"
native_arch = "amd64"
native_scheme = "debian"
native_distro = "ubuntu"
native_fixture_version = "1.0.0"
repo_install_pkg = "nano"
repo_install_path = "/usr/bin/nano"

[distro_overrides.arch]
native_target = "arch"
native_glob = "*.pkg.tar.zst"
native_arch = "x86_64"
native_scheme = "arch"
native_distro = "arch"
native_fixture_version = "1.0.0-1"
repo_install_pkg = "tree"
repo_install_path = "/usr/bin/tree"
```

- [ ] **Step 2: Add repository sync and search tests**

Add tests:

```toml
[[test]]
id = "TNPM01"
name = "repo_sync_and_search"
description = "Repository metadata sync and search are usable"
timeout = 300
fatal = true

[[test.step]]
conary = "repo sync ${REPO_NAME} --force"

[test.step.assert]
exit_code = 0
stdout_contains = "[OK]"

[[test.step]]
conary = "search ${repo_install_pkg}"

[test.step.assert]
exit_code = 0
stdout_contains = "${repo_install_pkg}"
stdout_not_contains = "No packages found"
```

- [ ] **Step 3: Add local native fixture build/install/metadata tests**

Add tests that:

- build the native fixture for `${native_target}`
- source `/tmp/native-pm-parity/native-fixture.env`
- install the generated local package file through `conary install`
- assert the binary, config file, file checksum, DB metadata, provides, requirements, and package info

Use these commands inside the manifest:

```toml
[[test]]
id = "TNPM02"
name = "local_native_fixture_build"
description = "Build the local native fixture in the host distro format"
timeout = 180
fatal = true
depends_on = ["TNPM01"]

[[test.step]]
run = "rm -rf /tmp/native-pm-parity && mkdir -p /tmp/native-pm-parity && CONARY_BIN=${CONARY_BIN} /opt/remi-tests/fixtures/native/build-native-fixtures.sh ${native_target} /tmp/native-pm-parity /opt/remi-tests/fixtures/phase4-runtime-fixture && . /tmp/native-pm-parity/native-fixture.env && test -f \"$NATIVE_PKG_FILE\" && test \"$NATIVE_TARGET\" = \"${native_target}\""

[test.step.assert]
exit_code = 0

[[test]]
id = "TNPM03"
name = "local_native_install"
description = "Install a local native package on a no-generation live root"
timeout = 180
fatal = true
depends_on = ["TNPM02"]

[[test.step]]
run = ". /tmp/native-pm-parity/native-fixture.env && ${CONARY_BIN} install --allow-live-system-mutation \"$NATIVE_PKG_FILE\" --db-path ${DB_PATH} --no-deps --yes --sandbox never"

[test.step.assert]
exit_code = 0
stdout_contains_any = ["Successfully installed", "Installed", "phase4-runtime-fixture"]

[[test.step]]
file_exists = "/usr/bin/phase4-runtime-fixture"

[[test.step]]
file_exists = "/etc/phase4-runtime-fixture/app.conf"

[[test.step]]
run = "test \"$(sha256sum /etc/phase4-runtime-fixture/app.conf | awk '{print $1}')\" = \"1da0b50cb027387347265437a11956c1433788d045e4c63b379a1e0740882e7c\""

[test.step.assert]
exit_code = 0

[[test.step]]
run = "test \"$(sha256sum /usr/bin/phase4-runtime-fixture | awk '{print $1}')\" = \"517631de24336343a6aaf1a8f704d326299c14c619fe8c8d75d17824d074bd7f\""

[test.step.assert]
exit_code = 0

[[test]]
id = "TNPM04"
name = "local_native_metadata"
description = "Installed native fixture has exact metadata"
timeout = 60
depends_on = ["TNPM03"]

[[test.step]]
run = "sqlite3 ${DB_PATH} \"SELECT name || '|' || version || '|' || COALESCE(architecture, '') || '|' || COALESCE(version_scheme, '') || '|' || COALESCE(install_source, '') || '|' || COALESCE(install_reason, '') FROM troves WHERE name = 'phase4-runtime-fixture'\""

[test.step.assert]
exit_code = 0
stdout_contains = "phase4-runtime-fixture|${native_fixture_version}|${native_arch}|${native_scheme}|file|explicit"

[[test.step]]
conary = "list phase4-runtime-fixture --info"

[test.step.assert]
exit_code = 0
stdout_contains_all = ["Name        : phase4-runtime-fixture", "Version     : ${native_fixture_version}", "Authority   : conary-owned"]

[[test.step]]
conary = "list phase4-runtime-fixture --files"

[test.step.assert]
exit_code = 0
stdout_contains_all = ["/usr/bin/phase4-runtime-fixture", "/etc/phase4-runtime-fixture/app.conf"]

[[test.step]]
conary = "list --path /usr/bin/phase4-runtime-fixture"

[test.step.assert]
exit_code = 0
stdout_contains = "phase4-runtime-fixture ${native_fixture_version} provides /usr/bin/phase4-runtime-fixture"
```

If the exact checksum differs after the fixture is regenerated, update the checksum in the manifest and explain why in the commit. Do not weaken the assertion to "file exists only".

- [ ] **Step 4: Add pin, unpin, remove, history, and query tests**

Add tests proving:

- `conary pin phase4-runtime-fixture` succeeds
- pinned remove fails before mutation
- the binary and DB row still exist after the failed remove
- `conary unpin phase4-runtime-fixture` succeeds
- `conary query whatprovides phase4-runtime-fixture` reports the fixture provider
- `conary query whatbreaks phase4-runtime-fixture` reports no breakage or the same breakage preflight that remove reports
- unpinned remove succeeds and deletes the file and DB row
- `conary system history` shows applied install/remove entries

Each command must have an explicit `[test.step.assert]` with `exit_code` or `exit_code_not`.

- [ ] **Step 5: Add repository install and repository metadata tests**

Add tests proving:

- `conary install ${repo_install_pkg} --repo ${REPO_NAME} --no-deps --yes --sandbox never --allow-live-system-mutation` succeeds
- `${repo_install_path}` exists afterward
- `troves.installed_from_repository_id` points at `${REPO_NAME}`
- `troves.version_scheme` and `troves.source_distro` match the distro override
- `conary list`, `conary list --info`, `conary list --files`, and `conary list --path` can answer ordinary daily-driver questions for the repository package

- [ ] **Step 6: Add update, security-honesty, autoremove, and deferred-follow-up tests**

Add tests proving:

- update v1 to v2 happens through `conary update`, not direct reinstall. If Remi does not already publish a v2 repository fixture for all three distros, seed one in SQLite and serve the v2 artifact from a background local HTTP server inside the container.
- `conary update --security ${repo_install_pkg}` refuses before mutation when `${REPO_NAME}` has unknown or unsupported advisory metadata.
- an orphaned Conary-owned package is listed by `conary autoremove --dry-run` and removed by `conary autoremove`.
- the forced deferred-follow-up path exits 0, prints deferred work, and is rendered by `conary system history`.

Use a fatal test for any local server setup so downstream update tests cannot silently skip.

- [ ] **Step 7: Run manifest inventory**

Run:

```bash
cargo run -p conary-test -- list
```

Expected: output includes `Phase 4: Native Package Manager Parity Matrix`.

- [ ] **Step 8: Commit**

```bash
git add apps/conary/tests/integration/remi/manifests/phase4-native-pm-parity.toml apps/conary/tests/integration/remi/config.toml
git commit -m "test(matrix): add native package manager parity suite"
```

---

## Task 5: Run And Stabilize The Matrix

**Files:**

- Modify product, harness, manifest, or fixture files only for failures exposed by the matrix.

- [ ] **Step 1: Run Fedora first**

Run:

```bash
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro fedora44 --phase 4
bash scripts/check-conary-test-result-gate.sh apps/conary/tests/integration/remi/results/fedora44-phase4.json
```

Expected: matrix passes or exposes a concrete bug. If it exposes a bug, fix the smallest product or test issue and rerun Fedora before moving on.

- [ ] **Step 2: Run Ubuntu**

Run:

```bash
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro ubuntu-26.04 --phase 4
bash scripts/check-conary-test-result-gate.sh apps/conary/tests/integration/remi/results/ubuntu-26.04-phase4.json
```

Expected: matrix passes with zero failed/skipped/cancelled results.

- [ ] **Step 3: Run Arch**

Run:

```bash
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro arch --phase 4
bash scripts/check-conary-test-result-gate.sh apps/conary/tests/integration/remi/results/arch-phase4.json
```

Expected: matrix passes with zero failed/skipped/cancelled results.

- [ ] **Step 4: Commit fixes in small slices**

For each bug fix, commit the narrowest coherent change:

```bash
git add <changed-files>
git commit -m "fix(package): <short imperative summary>"
```

Do not batch unrelated Fedora, Ubuntu, and Arch failures into one commit unless the root cause is the same function or manifest assumption.

---

## Task 6: Update Docs And Audit Metadata

**Files:**

- Modify: `docs/INTEGRATION-TESTING.md`
- Modify: `docs/superpowers/specs/2026-05-14-native-package-manager-parity-matrix-design.md`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-summary.md`

- [ ] **Step 1: Add Slice D plan link to the parity spec**

In `docs/superpowers/specs/2026-05-14-native-package-manager-parity-matrix-design.md`, add:

```markdown
Slice D plan: `docs/superpowers/plans/2026-05-16-native-package-manager-parity-slice-d-plan.md`.
```

- [ ] **Step 2: Update integration testing docs**

In `docs/INTEGRATION-TESTING.md`, add a focused section for the new suite:

```markdown
Focused Slice D native package-manager parity proof:

- `cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro fedora44 --phase 4`
- `cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro ubuntu-26.04 --phase 4`
- `cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro arch --phase 4`

Each run must pass `scripts/check-conary-test-result-gate.sh`, which requires
zero failed, skipped, and cancelled results before the matrix can count as
limited-preview release evidence.
```

After the runs pass, record the actual result counts and date. Do not record expected future evidence as if it already passed.

- [ ] **Step 3: Add this plan to the audit inventory**

Add this row after the Slice C plan:

```tsv
docs/superpowers/plans/2026-05-16-native-package-manager-parity-slice-d-plan.md	planning	maintainer
```

- [ ] **Step 4: Add this plan to the audit ledger**

Add this row after the Slice C plan:

```tsv
docs/superpowers/plans/2026-05-16-native-package-manager-parity-slice-d-plan.md	docs/superpowers/plans/2026-05-16-native-package-manager-parity-slice-d-plan.md	planning	maintainer	native-package-manager-parity; implementation-plan; conary-test-matrix; public-preview	docs/superpowers/specs/2026-05-14-native-package-manager-parity-matrix-design.md; docs/INTEGRATION-TESTING.md; apps/conary-test/src/engine/runner.rs; apps/conary/tests/integration/remi/manifests/phase4-native-pm-parity.toml	verified	corrected	Added the active Slice D implementation plan for the three-distro native package-manager parity matrix and explicit release-evidence gates.
```

- [ ] **Step 5: Update audit summary counts**

In `docs/superpowers/documentation-accuracy-audit-summary.md`, update:

```markdown
- Total tracked doc-like files audited: 80
- `corrected`: 34
```

Leave the other counts unchanged unless implementation edits correct additional docs.

- [ ] **Step 6: Verify docs**

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --check
```

Expected: both pass.

- [ ] **Step 7: Commit**

```bash
git add docs/INTEGRATION-TESTING.md docs/superpowers/specs/2026-05-14-native-package-manager-parity-matrix-design.md docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-summary.md docs/superpowers/plans/2026-05-16-native-package-manager-parity-slice-d-plan.md
git commit -m "docs: plan native package manager parity matrix"
```

---

## Task 7: Final Verification, Merge, Push, And Cleanup

**Files:**

- No new files unless verification exposes a bug.

- [ ] **Step 1: Run final verification**

Run:

```bash
cargo fmt --check
git diff --check
cargo test -p conary-test suite_setup_executes_before_tests -- --nocapture
cargo test -p conary-test test_json_report_format -- --nocapture
cargo run -p conary-test -- list
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro fedora44 --phase 4
bash scripts/check-conary-test-result-gate.sh apps/conary/tests/integration/remi/results/fedora44-phase4.json
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro ubuntu-26.04 --phase 4
bash scripts/check-conary-test-result-gate.sh apps/conary/tests/integration/remi/results/ubuntu-26.04-phase4.json
cargo run -p conary-test -- run --suite phase4-native-pm-parity --distro arch --phase 4
bash scripts/check-conary-test-result-gate.sh apps/conary/tests/integration/remi/results/arch-phase4.json
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all commands pass. The three matrix result gates print `ok`.

- [ ] **Step 2: Mark the `/goal` complete**

Only after Step 1 passes, call:

```text
update_goal(status = complete)
```

Report the goal tool's final token usage in the user-facing wrap-up.

- [ ] **Step 3: Merge and push**

Run:

```bash
git status --short --branch
git checkout main
git pull --ff-only origin main
git merge --ff-only slice-d-native-pm-matrix
git push origin main
```

Expected: `main` fast-forwards and push succeeds.

- [ ] **Step 4: Post-merge verification from `main`**

Run at minimum:

```bash
cargo fmt --check
git diff --check origin/main..HEAD
cargo run -p conary-test -- list
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
cargo clippy --workspace --all-targets -- -D warnings
```

For final release evidence, keep the three per-distro matrix result files from the pre-merge run unless product code changed during merge conflict resolution. If product code changed, rerun the matrix from `main`.

- [ ] **Step 5: Cleanup**

Run:

```bash
git branch -d slice-d-native-pm-matrix
git worktree list
pgrep -af 'qemu|conary-test' || true
find /tmp -maxdepth 1 \( -name 'conary-*' -o -name 'tmp.*' \) -print
```

Remove only temporary artifacts created by this slice. Do not remove retained release evidence under `target/local-validation` unless the user asks.

---

## Plan Self-Review

- The plan is explicitly tailored to `/goal`: it defines the goal objective, completion contract, subagent ownership, and the point where `update_goal(status = complete)` is allowed.
- Slice D is kept as a validation matrix slice. Harness honesty and result-gate fixes are included because the matrix cannot serve as release evidence without them.
- The plan does not rely on `suite.setup` magically working; Task 1 makes it real and tests it.
- The matrix requires explicit exit-code assertions and a zero failed/skipped/cancelled gate for every distro.
- The plan preserves the previous product decision that `whatprovides` and `whatbreaks` remain under `conary query`.
- Dependabot `tough` advisories are not mixed into this implementation slice. They are blocked by `sigstore 0.13.0` pinning `tough = "^0.21"` while the fixed `tough` is `0.22.0`; handle that as a separate security waiver/fork/dependency-removal decision.
