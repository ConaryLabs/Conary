# Phase 3: Adversarial Testing Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add 82 integration tests across 8 groups (G-N) that verify Conary handles corrupted data, crashes, hostile inputs, real-world operations, and boot scenarios correctly.

**Architecture:** Extends the `conary-test` Rust engine with 3 new step types (`kill_after_log`, `mock_server`, `qemu_boot`), resource constraints on containers, and distro variable overrides. Tests are TOML manifests executed in Podman containers. Fixtures are CCS packages built by shell scripts and published to Remi.

**Tech Stack:** Rust (conary-test crate), bollard (container API), TOML manifests, Podman, QEMU (Group N only)

**Spec:** `docs/superpowers/specs/2026-03-11-phase3-adversarial-testing-design.md`

---

## Chunk 1: Engine Extensions

Engine changes to support Phase 3 test capabilities. All changes in the `conary-test` crate.

### Task 1: Add resource constraints to ContainerConfig and BollardBackend

**Files:**
- Modify: `conary-test/src/container/backend.rs` — add fields to `ContainerConfig`
- Modify: `conary-test/src/container/lifecycle.rs` — apply constraints in `create()`
- Modify: `conary-test/src/config/manifest.rs` — add `ResourceConstraints` to `TestDef`
- Modify: `conary-test/src/engine/runner.rs` — pass constraints to container creation

- [ ] **Step 1: Add ResourceConstraints struct to manifest.rs**

Add after the `Assertion` struct:

```rust
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResourceConstraints {
    #[serde(default)]
    pub tmpfs_size_mb: Option<u64>,
    #[serde(default)]
    pub memory_limit_mb: Option<u64>,
    #[serde(default)]
    pub network_isolated: Option<bool>,
}
```

Add to `TestDef`:
```rust
    #[serde(default)]
    pub resources: Option<ResourceConstraints>,
```

- [ ] **Step 2: Add resource fields to ContainerConfig**

In `backend.rs`, add to `ContainerConfig`:
```rust
    pub tmpfs: HashMap<String, String>,
    pub memory_limit: Option<i64>,
```

Update `Default` impl to include `tmpfs: HashMap::new()` and `memory_limit: None`.

- [ ] **Step 3: Apply constraints in BollardBackend::create()**

In `lifecycle.rs`, in the `create` method where `HostConfig` is built, add:
```rust
if !config.tmpfs.is_empty() {
    host_config.tmpfs = Some(config.tmpfs.clone());
}
if let Some(mem) = config.memory_limit {
    host_config.memory = Some(mem);
}
```

Also apply `network_mode`:
```rust
host_config.network_mode = Some(config.network_mode.clone());
```

- [ ] **Step 4: Run `cargo test -p conary-test` and `cargo clippy -p conary-test -- -D warnings`**

Expected: all existing tests pass, no new warnings.

- [ ] **Step 5: Commit**

```
feat(conary-test): add resource constraints to container configuration
```

### Task 2: Add distro_overrides variable substitution

**Files:**
- Modify: `conary-test/src/config/manifest.rs` — add `DistroOverrides` to `TestManifest`
- Modify: `conary-test/src/engine/runner.rs` — resolve `${var}` in step strings

- [ ] **Step 1: Add distro_overrides to manifest**

Add to `TestManifest`:
```rust
#[derive(Debug, Clone, Deserialize)]
pub struct TestManifest {
    pub suite: SuiteDef,
    pub test: Vec<TestDef>,
    #[serde(default)]
    pub distro_overrides: HashMap<String, HashMap<String, String>>,
}
```

- [ ] **Step 2: Load distro overrides into runner vars**

In `TestRunner::new()`, after existing vars setup, add:
```rust
// Load distro-specific overrides from manifest (populated per-run).
```

Add a method `pub fn load_manifest_vars(&mut self, manifest: &TestManifest)` that merges `manifest.distro_overrides[self.distro]` into `self.vars`.

- [ ] **Step 3: Call load_manifest_vars at start of run()**

In `TestRunner::run()`, at the top:
```rust
self.load_manifest_vars(manifest);
```

Note: `run()` needs `&mut self` instead of `&self`.

- [ ] **Step 4: Add variable substitution to step resolution**

In the runner, when resolving step commands, replace `${VAR}` patterns with values from `self.vars`. Add a helper:
```rust
fn substitute_vars(&self, input: &str) -> String {
    let mut result = input.to_string();
    for (key, value) in &self.vars {
        result = result.replace(&format!("${{{key}}}"), value);
    }
    result
}
```

Apply this to `run` and `conary` step commands before execution.

- [ ] **Step 5: Write test for variable substitution**

```rust
#[test]
fn test_substitute_vars() {
    let mut vars = HashMap::new();
    vars.insert("PKG".to_string(), "tree".to_string());
    // test that "${PKG}" becomes "tree"
}
```

- [ ] **Step 6: Run tests, commit**

```
feat(conary-test): add distro_overrides variable substitution in manifests
```

### Task 3: Add `kill_after_log` step type

This is the most complex engine extension. It starts a conary command, monitors its log output via bollard, and sends SIGKILL when a specific pattern appears.

**Files:**
- Modify: `conary-test/src/config/manifest.rs` — add `KillAfterLog` to `TestStep`
- Modify: `conary-test/src/engine/runner.rs` — implement kill-after-log execution
- Modify: `conary-test/src/container/backend.rs` — add `kill()` to trait
- Modify: `conary-test/src/container/lifecycle.rs` — implement `kill()` via bollard

- [ ] **Step 1: Add kill() to ContainerBackend trait**

In `backend.rs`:
```rust
    /// Send a signal to a container (e.g., SIGKILL).
    async fn kill(&self, id: &ContainerId, signal: &str) -> Result<()>;
```

- [ ] **Step 2: Implement kill() in BollardBackend**

In `lifecycle.rs`:
```rust
async fn kill(&self, id: &ContainerId, signal: &str) -> Result<()> {
    self.docker
        .kill_container(id, Some(bollard::container::KillContainerOptions {
            signal: signal.to_string(),
        }))
        .await
        .context("failed to kill container")?;
    Ok(())
}
```

- [ ] **Step 3: Add exec_streaming() to ContainerBackend trait**

Add a method that returns an exec ID and streams output:
```rust
    /// Start a command and return the exec ID for later monitoring/killing.
    async fn exec_detached(
        &self,
        id: &ContainerId,
        cmd: &[&str],
    ) -> Result<String>;

    /// Stream logs from an exec instance, returning lines as they arrive.
    async fn exec_logs(
        &self,
        exec_id: &str,
    ) -> Result<tokio::sync::mpsc::Receiver<String>>;
```

- [ ] **Step 4: Add KillAfterLog to manifest**

In `manifest.rs`, add to `TestStep`:
```rust
    #[serde(default)]
    pub kill_after_log: Option<KillAfterLog>,
```

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct KillAfterLog {
    pub conary: String,
    pub pattern: String,
    #[serde(default = "default_kill_timeout")]
    pub timeout_seconds: u64,
}

fn default_kill_timeout() -> u64 { 60 }
```

Add to `StepType`:
```rust
    KillAfterLog(KillAfterLog),
```

- [ ] **Step 5: Implement kill_after_log execution in runner**

In the runner's step execution match, add a branch that:
1. Starts the conary command via `exec_detached`
2. Monitors log output for the pattern
3. When pattern matches, sends SIGKILL via container kill or exec kill
4. Collects final stdout/stderr
5. Returns ExecResult with exit code (will be non-zero due to SIGKILL)

- [ ] **Step 6: Run tests, commit**

```
feat(conary-test): add kill_after_log step type for crash recovery tests
```

### Task 4: Add `mock_server` setup type

A lightweight mock HTTP server that runs inside the test container, configured via TOML.

**Files:**
- Create: `conary-test/src/engine/mock_server.rs` — mock server configuration and management
- Modify: `conary-test/src/config/manifest.rs` — add `MockServerConfig` to `SuiteDef.setup`
- Modify: `conary-test/src/engine/runner.rs` — start/stop mock server around tests
- Modify: `conary-test/src/engine/mod.rs` — add module

- [ ] **Step 1: Define MockServerConfig in manifest**

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct MockServerConfig {
    pub port: u16,
    pub routes: Vec<MockRoute>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MockRoute {
    pub path: String,
    pub status: u16,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub body_file: Option<String>,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub delay_ms: Option<u64>,
    #[serde(default)]
    pub truncate_at_bytes: Option<usize>,
}
```

Add to `SuiteDef`:
```rust
    #[serde(default)]
    pub mock_server: Option<MockServerConfig>,
```

- [ ] **Step 2: Create mock_server.rs**

This module generates a Python script from the `MockServerConfig` and uploads it to the container. The script is a simple `http.server` handler that serves configured responses. Python is already available in all test container images.

```rust
pub fn generate_mock_script(config: &MockServerConfig) -> String {
    // Generate a Python 3 HTTP server script that serves the configured routes
    // with support for status codes, headers, delays, and truncation
}

pub async fn start_mock_server(
    backend: &dyn ContainerBackend,
    container_id: &ContainerId,
    config: &MockServerConfig,
) -> Result<()> {
    let script = generate_mock_script(config);
    backend.copy_to(container_id, "/tmp/mock_server.py", script.as_bytes()).await?;
    backend.exec_detached(container_id, &["python3", "/tmp/mock_server.py"]).await?;
    // Wait for server to be ready (poll port)
    Ok(())
}
```

- [ ] **Step 3: Start mock server in runner before test steps**

In `TestRunner::run()`, after container setup, check if `manifest.suite.mock_server` is `Some` and start the mock server.

- [ ] **Step 4: Write unit test for script generation**

Test that `generate_mock_script` produces valid Python with correct routes.

- [ ] **Step 5: Run tests, commit**

```
feat(conary-test): add mock_server setup for simulating repo failures
```

### Task 5: Add `flaky` and `retries` support to test manifests

**Files:**
- Modify: `conary-test/src/config/manifest.rs` — add `flaky` and `retries` to `TestDef`
- Modify: `conary-test/src/engine/runner.rs` — retry flaky tests with majority-pass

- [ ] **Step 1: Add fields to TestDef**

```rust
    #[serde(default)]
    pub flaky: Option<bool>,
    #[serde(default)]
    pub retries: Option<u32>,
```

- [ ] **Step 2: Implement retry logic in runner**

In the test execution loop, if a test has `flaky = true`, retry up to `retries` times (default 3). Mark as passed if majority of runs pass.

- [ ] **Step 3: Run tests, commit**

```
feat(conary-test): add flaky test retry support with majority-pass logic
```

### Task 6: Add `--phase 3` support to CLI

**Files:**
- Modify: `conary-test/src/cli.rs` — ensure phase 3 is accepted

- [ ] **Step 1: Verify phase filtering works for phase 3**

The CLI already accepts `--phase N`. Verify that `--phase 3` works by checking the manifest loading code filters correctly. If phase 3 manifests exist in the manifests directory, they should be picked up.

- [ ] **Step 2: Run `cargo run -p conary-test -- list` to verify phase 3 manifests show up (once they exist)**

- [ ] **Step 3: Commit if any changes needed**

```
feat(conary-test): verify phase 3 support in CLI
```

---

## Chunk 2: Fixture Infrastructure

Build scripts and fixture packages for adversarial testing.

### Task 7: Create fixture directory structure and build-all.sh

**Files:**
- Create: `tests/fixtures/adversarial/build-all.sh`
- Create: `tests/fixtures/adversarial/README.md`

- [ ] **Step 1: Create directory structure**

```bash
mkdir -p tests/fixtures/adversarial/{corrupted,malicious,deps,large}
```

- [ ] **Step 2: Write build-all.sh skeleton**

```bash
#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CONARY_BIN="${CONARY_BIN:-$(pwd)/target/debug/conary}"

echo "=== Building adversarial test fixtures ==="

# Build corrupted fixtures
echo "[1/4] Building corrupted fixtures..."
"$SCRIPT_DIR/build-corrupted.sh" "$CONARY_BIN"

# Build malicious fixtures
echo "[2/4] Building malicious fixtures..."
"$SCRIPT_DIR/build-malicious.sh" "$CONARY_BIN"

# Build dependency chain fixtures
echo "[3/4] Building dependency fixtures..."
"$SCRIPT_DIR/build-deps.sh" "$CONARY_BIN"

# Generate large fixtures (not checked in)
echo "[4/4] Generating large fixtures..."
"$SCRIPT_DIR/build-large.sh" "$CONARY_BIN"

echo "=== All fixtures built ==="
```

- [ ] **Step 3: Commit**

```
chore(test): create adversarial fixture directory structure
```

### Task 8: Build corrupted fixture packages

**Files:**
- Create: `tests/fixtures/adversarial/build-corrupted.sh`
- Create: `tests/fixtures/adversarial/corrupted/bad-checksum/ccs.toml`
- Create: `tests/fixtures/adversarial/corrupted/bad-checksum/stage/usr/bin/hello`
- Create: `tests/fixtures/adversarial/corrupted/truncated/ccs.toml`
- Create: `tests/fixtures/adversarial/corrupted/size-lie/ccs.toml`

- [ ] **Step 1: Create valid base fixture, then corrupt it**

The build script:
1. Builds a valid CCS package using `conary ccs build`
2. Copies it and modifies the SHA-256 in the manifest (bad-checksum)
3. Copies it and truncates a chunk file at 50% (truncated)
4. Creates a package with inflated size in metadata (size-lie)

- [ ] **Step 2: Write ccs.toml for each fixture**

Each fixture has a minimal `ccs.toml` with name, version, and a small file tree.

- [ ] **Step 3: Write build-corrupted.sh**

Script that builds valid CCS then creates corrupted variants via `sed` and `truncate`.

- [ ] **Step 4: Test that `conary ccs build` works on the base fixture**

- [ ] **Step 5: Commit**

```
test(fixtures): add corrupted CCS fixture packages
```

### Task 9: Build malicious fixture packages

**Files:**
- Create: `tests/fixtures/adversarial/build-malicious.sh`
- Create: `tests/fixtures/adversarial/malicious/path-traversal/ccs.toml`
- Create: `tests/fixtures/adversarial/malicious/symlink-attack/ccs.toml`
- Create: `tests/fixtures/adversarial/malicious/setuid/ccs.toml`
- Create: `tests/fixtures/adversarial/malicious/hostile-scriptlet/ccs.toml`

- [ ] **Step 1: Write malicious fixture definitions**

Each fixture has a `ccs.toml` and staged files that exercise a specific attack:
- `path-traversal`: file staged at `stage/../../etc/shadow`
- `symlink-attack`: symlink to `/etc/passwd` + file that would overwrite
- `setuid`: binary with setuid bit (`chmod 4755`)
- `hostile-scriptlet`: post-install script that attempts `curl` and writes to `/etc`

- [ ] **Step 2: Write build-malicious.sh**

- [ ] **Step 3: Commit**

```
test(fixtures): add malicious CCS fixture packages for security tests
```

### Task 10: Build dependency chain fixtures

**Files:**
- Create: `tests/fixtures/adversarial/build-deps.sh`
- Create: `tests/fixtures/adversarial/deps/` (10-15 ccs.toml files)

- [ ] **Step 1: Design dependency graph**

Create 12 packages:
- `dep-base-v1`, `dep-base-v2` — base package with two versions
- `dep-liba-v1`, `dep-liba-v2` — library with version constraint
- `dep-libb-v1` — depends on dep-liba >= 1
- `dep-app-v1` — depends on dep-liba >= 1, < 2 and dep-libb
- `dep-conflict-v1` — conflicts with dep-app
- `dep-circular-a-v1` — depends on dep-circular-b
- `dep-circular-b-v1` — depends on dep-circular-a
- `dep-virtual-provider-v1` — provides "mail-transport-agent"
- `dep-virtual-consumer-v1` — depends on "mail-transport-agent"
- `dep-or-a-v1`, `dep-or-b-v1` — alternatives for OR dependency
- `dep-unresolvable-v1` — depends on "nonexistent-package"

- [ ] **Step 2: Write ccs.toml for each package**

Each has appropriate `[dependencies]`, `[conflicts]`, `[provides]` sections.

- [ ] **Step 3: Write build-deps.sh**

Builds all 12+ packages and generates a SHA256SUMS file.

- [ ] **Step 4: Commit**

```
test(fixtures): add dependency chain fixture packages
```

### Task 11: Extend publish-test-fixtures.sh for adversarial fixtures

**Files:**
- Modify: `scripts/publish-test-fixtures.sh`

- [ ] **Step 1: Add adversarial fixture publishing**

Add a section that uploads `tests/fixtures/adversarial/corrupted/*.ccs`, `malicious/*.ccs`, and `deps/*.ccs` to `https://packages.conary.io/test-fixtures/adversarial/`.

- [ ] **Step 2: Commit**

```
chore(scripts): extend publish-test-fixtures.sh for adversarial fixtures
```

---

## Chunk 3: Group M — Real-World Operations (T138-T149)

The highest-value group: does the product actually work with real packages?

### Task 12: Write Group M manifest

**Files:**
- Create: `tests/integration/remi/manifests/phase3-group-m.toml`

- [ ] **Step 1: Write the TOML manifest**

```toml
[suite]
name = "Phase 3 Group M: Real-World Operations"
phase = 3

[distro_overrides.fedora43]
small_package = "tree"
dep_heavy_package = "vim-enhanced"
distro_repo = "fedora-43"

[distro_overrides.ubuntu-noble]
small_package = "tree"
dep_heavy_package = "vim"
distro_repo = "ubuntu-noble"

[distro_overrides.arch]
small_package = "tree"
dep_heavy_package = "vim"
distro_repo = "arch-extra"

[[test]]
id = "T138"
name = "install_real_ccs_package"
description = "Sync Remi, install real CCS package, verify files deployed"
timeout = 120
group = "M"

[[test.step]]
conary = "repo-sync remi --db-path ${DB_PATH}"

[[test.step]]
conary = "install conary-test-fixture --db-path ${DB_PATH}"
[test.step.assert]
exit_code = 0

[[test.step]]
conary = "query --db-path ${DB_PATH}"
[test.step.assert]
stdout_contains = "conary-test-fixture"

# T139-T149 follow same pattern...
```

- [ ] **Step 2: Add all 12 tests (T138-T149)**

Each test uses `${small_package}`, `${dep_heavy_package}`, `${distro_repo}` variables that resolve per-distro.

- [ ] **Step 3: Validate manifest parses**

```bash
cargo run -p conary-test -- list
```

- [ ] **Step 4: Commit**

```
test(integration): add Phase 3 Group M real-world operations manifest
```

---

## Chunk 4: Groups G & H — Data Integrity & Error Recovery (T77-T96)

### Task 13: Write Group G manifest (Data Integrity)

**Files:**
- Create: `tests/integration/remi/manifests/phase3-group-g.toml`

- [ ] **Step 1: Write T77-T86 tests**

Each test installs a corrupted fixture and asserts failure with a specific error message. Example:

```toml
[[test]]
id = "T77"
name = "bad_checksum_rejection"
description = "Install CCS with bad SHA-256 — expect rejection"
timeout = 30
group = "G"

[[test.step]]
conary = "ccs install /fixtures/adversarial/bad-checksum.ccs --db-path ${DB_PATH}"
[test.step.assert]
exit_code_not = 0
stderr_contains = "checksum"
```

- [ ] **Step 2: Commit**

```
test(integration): add Phase 3 Group G data integrity manifest
```

### Task 14: Write Group H manifest (Error Recovery)

**Files:**
- Create: `tests/integration/remi/manifests/phase3-group-h.toml`

- [ ] **Step 1: Write T87-T96 tests**

Tests use `kill_after_log`, resource constraints, and concurrent execution. Example:

```toml
[[test]]
id = "T87"
name = "sigkill_mid_install_recovery"
description = "SIGKILL mid-install, verify DB and filesystem consistent"
timeout = 60
group = "H"
flaky = true
retries = 3

[[test.step]]
[test.step.kill_after_log]
conary = "ccs install /fixtures/adversarial/large-package.ccs --db-path ${DB_PATH}"
pattern = "Deploying files"
timeout_seconds = 30

# After kill, verify consistency
[[test.step]]
run = "sqlite3 ${DB_PATH} 'PRAGMA integrity_check'"
[test.step.assert]
stdout_contains = "ok"

[[test]]
id = "T88"
name = "disk_full_rollback"
description = "Disk-full during install — clean rollback"
timeout = 60
group = "H"

[test.resources]
tmpfs_size_mb = 50
```

- [ ] **Step 2: Commit**

```
test(integration): add Phase 3 Group H error recovery manifest
```

---

## Chunk 5: Groups I & J — Security & Dependencies (T97-T117)

### Task 15: Write Group I manifest (Security Boundaries)

**Files:**
- Create: `tests/integration/remi/manifests/phase3-group-i.toml`

- [ ] **Step 1: Write T97-T106 tests**

Each test installs a malicious fixture and asserts the attack is blocked. Example:

```toml
[[test]]
id = "T97"
name = "path_traversal_blocked"
description = "Package with ../../etc/shadow — blocked"
timeout = 30
group = "I"

[[test.step]]
conary = "ccs install /fixtures/adversarial/path-traversal.ccs --db-path ${DB_PATH} --allow-unsigned"
[test.step.assert]
exit_code_not = 0
stderr_contains = "path traversal"

[[test.step]]
file_not_exists = "/etc/shadow.bak"
```

- [ ] **Step 2: Commit**

```
test(integration): add Phase 3 Group I security boundaries manifest
```

### Task 16: Write Group J manifest (Dependency Edge Cases)

**Files:**
- Create: `tests/integration/remi/manifests/phase3-group-j.toml`

- [ ] **Step 1: Write T107-T117 tests**

Uses the dependency chain fixtures. Example:

```toml
[[test]]
id = "T107"
name = "circular_dependency_detected"
description = "Circular dep A→B→A — detection and clear error"
timeout = 30
group = "J"

[[test.step]]
conary = "install dep-circular-a --db-path ${DB_PATH}"
[test.step.assert]
exit_code_not = 0
stderr_contains = "circular"
```

- [ ] **Step 2: Commit**

```
test(integration): add Phase 3 Group J dependency edge cases manifest
```

---

## Chunk 6: Groups K & L — Server Resilience & Lifecycle (T118-T137)

### Task 17: Write Group K manifest (Server Resilience)

**Files:**
- Create: `tests/integration/remi/manifests/phase3-group-k.toml`

- [ ] **Step 1: Write T118-T127 tests**

Tests use the mock_server setup. Example:

```toml
[suite]
name = "Phase 3 Group K: Server Resilience"
phase = 3

[suite.mock_server]
port = 8888

[[suite.mock_server.routes]]
path = "/v1/metadata/test"
status = 200
body = '{"version": 0, "packages": []}'

[[suite.mock_server.routes]]
path = "/v1/packages/test.ccs"
status = 429
headers = { "Retry-After" = "1" }

[[test]]
id = "T122"
name = "rate_limiting_retry"
description = "HTTP 429 — backoff and retry"
timeout = 30
group = "K"

[[test.step]]
conary = "install test-pkg --endpoint http://localhost:8888 --db-path ${DB_PATH}"
[test.step.assert]
stderr_contains = "rate limit"
```

- [ ] **Step 2: Commit**

```
test(integration): add Phase 3 Group K server resilience manifest
```

### Task 18: Write Group L manifest (Lifecycle Robustness)

**Files:**
- Create: `tests/integration/remi/manifests/phase3-group-l.toml`

- [ ] **Step 1: Write T128-T137 tests**

Self-update, generation, and bootstrap tests.

- [ ] **Step 2: Commit**

```
test(integration): add Phase 3 Group L lifecycle robustness manifest
```

---

## Chunk 7: Group N — Kernel & Boot (T150-T159)

### Task 19: Write Group N container tests manifest (T150-T155)

**Files:**
- Create: `tests/integration/remi/manifests/phase3-group-n-container.toml`

- [ ] **Step 1: Write T150-T155 tests**

Kernel file deployment verification. Uses distro_overrides for kernel package names.

- [ ] **Step 2: Commit**

```
test(integration): add Phase 3 Group N kernel container tests manifest
```

### Task 20: Add qemu_boot step type to engine

**Files:**
- Create: `conary-test/src/engine/qemu.rs` — QEMU VM lifecycle
- Modify: `conary-test/src/config/manifest.rs` — add `QemuBoot` to `TestStep`
- Modify: `conary-test/src/engine/runner.rs` — execute qemu_boot steps
- Modify: `conary-test/src/engine/mod.rs` — add module

- [ ] **Step 1: Define QemuBoot config in manifest**

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct QemuBoot {
    pub image: String,
    #[serde(default = "default_qemu_memory")]
    pub memory_mb: u32,
    #[serde(default = "default_qemu_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_ssh_port")]
    pub ssh_port: u16,
    pub commands: Vec<String>,
    #[serde(default)]
    pub expect_output: Vec<String>,
}
```

- [ ] **Step 2: Implement QEMU lifecycle in qemu.rs**

Functions to:
1. Download/cache boot image from Remi
2. Launch QEMU with KVM (or TCG fallback)
3. Wait for SSH availability
4. Execute commands over SSH
5. Collect output
6. Shutdown VM

- [ ] **Step 3: Wire into runner**

- [ ] **Step 4: Commit**

```
feat(conary-test): add qemu_boot step type for boot verification tests
```

### Task 21: Write Group N QEMU tests manifest (T156-T159)

**Files:**
- Create: `tests/integration/remi/manifests/phase3-group-n-qemu.toml`

- [ ] **Step 1: Write T156-T159 tests**

- [ ] **Step 2: Commit**

```
test(integration): add Phase 3 Group N QEMU boot tests manifest
```

### Task 22: Create build-boot-image.sh

**Files:**
- Create: `tests/fixtures/adversarial/build-boot-image.sh`

- [ ] **Step 1: Write script that builds minimal bootable qcow2**

Uses bootstrap stage output to create a small system image with kernel, init, and conary binary.

- [ ] **Step 2: Commit**

```
chore(test): add QEMU boot image build script
```

---

## Chunk 8: CI Integration

### Task 23: Update e2e.yaml for Phase 3

**Files:**
- Modify: `.forgejo/workflows/e2e.yaml`

- [ ] **Step 1: Add `--phase 3` to the E2E workflow**

Add Phase 3 as a separate job or extend existing phase runs:
```yaml
- name: Phase 3 (Adversarial)
  run: cargo run -p conary-test -- run --distro ${{ matrix.distro }} --phase 3
```

- [ ] **Step 2: Add QEMU tests as separate optional job**

```yaml
qemu-tests:
  runs-on: linux-native
  if: github.event_name == 'schedule' || github.event_name == 'workflow_dispatch'
  steps:
    - name: QEMU boot tests
      run: cargo run -p conary-test -- run --distro fedora43 --phase 3 --group N-qemu
```

- [ ] **Step 3: Commit**

```
ci: add Phase 3 adversarial tests to daily E2E workflow
```

### Task 24: Update publish-test-fixtures.sh and run initial publish

**Files:**
- Already modified in Task 11

- [ ] **Step 1: Build all fixtures locally**

```bash
./tests/fixtures/adversarial/build-all.sh
```

- [ ] **Step 2: Publish to Remi**

```bash
./scripts/publish-test-fixtures.sh
```

- [ ] **Step 3: Verify fixtures are accessible**

```bash
curl -s https://packages.conary.io/test-fixtures/adversarial/ | head
```

- [ ] **Step 4: Commit any fixture changes**

---

## Execution Notes

**Parallelism opportunities:**
- Tasks 1-6 (engine extensions) are sequential — each builds on the previous
- Tasks 7-11 (fixtures) can run in parallel with engine work after Task 1
- Tasks 12-18 (manifest writing) can run in parallel once engine and fixtures are done
- Tasks 19-22 (QEMU) are independent and can be deferred
- Task 23 (CI) depends on everything else

**Testing strategy:**
- After each chunk, run `cargo test -p conary-test` and `cargo clippy -p conary-test -- -D warnings`
- After Chunk 3+, run Phase 3 tests on Forge: `cargo run -p conary-test -- run --distro fedora43 --phase 3`
- QEMU tests require manual verification on Forge (KVM access needed)

**Total estimated tasks: 24**
**Total estimated new files: ~20 (manifests, fixtures, engine modules)**
**Total estimated modified files: ~10 (engine, CI, scripts)**
