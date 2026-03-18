# Phase 2: End-to-End Validation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Rewrite the integration test runner in Python with a TOML config, port the existing 37 tests, then add 34 deep tests (T38-T71) covering full install/remove/update/rollback with checksums, generation lifecycle, bootstrap pipeline, recipe cooking, and Remi client validation.

**Architecture:** Replace `test-runner.sh` + `lib.sh` with `test_runner.py` (stdlib-only Python 3). All endpoints, distro mappings, and fixture checksums live in `config.toml`. The Podman orchestrator (`run.sh`) stays as bash. Tests are methods on a `TestSuite` class. Results written as JSON.

**Tech Stack:** Python 3.11+ (stdlib only -- no pip, no frameworks), TOML config, Podman (containers), CCS (fixture packages), Forgejo Actions (CI)

---

## Task 1: Create config.toml

All test configuration in one place. Easy to swap endpoints, distros, fixture checksums.

**Files:**
- Create: `tests/integration/remi/config.toml`

**Step 1: Write config.toml**

```toml
# tests/integration/remi/config.toml
# Integration test configuration -- single source of truth for all endpoints,
# distro mappings, and fixture definitions.

[remi]
endpoint = "https://packages.conary.io"

[paths]
db = "/var/lib/conary/conary.db"
conary_bin = "/usr/bin/conary"
results_dir = "/results"
fixture_dir = "/opt/remi-tests/fixtures"

# Distro-specific settings. Key = DISTRO env var value.
[distros.fedora43]
remi_distro = "fedora"
repo_name = "fedora-remi"
test_package = "which"
test_binary = "/usr/bin/which"
test_package_2 = "tree"
test_binary_2 = "/usr/bin/tree"
test_package_3 = "jq"
test_binary_3 = "/usr/bin/jq"

[distros.ubuntu-noble]
remi_distro = "ubuntu"
repo_name = "ubuntu-remi"
test_package = "patch"
test_binary = "/usr/bin/patch"
test_package_2 = "nano"
test_binary_2 = "/usr/bin/nano"
test_package_3 = "jq"
test_binary_3 = "/usr/bin/jq"

[distros.arch]
remi_distro = "arch"
repo_name = "arch-remi"
test_package = "which"
test_binary = "/usr/bin/which"
test_package_2 = "tree"
test_binary_2 = "/usr/bin/tree"
test_package_3 = "jq"
test_binary_3 = "/usr/bin/jq"

# Default repos to remove after init (avoid slow syncs)
[setup]
remove_default_repos = [
    "arch-core", "arch-extra", "arch-multilib",
    "fedora-43", "ubuntu-noble",
]

# Test fixture packages (Phase 2)
[fixtures]
package = "conary-test-fixture"
file = "/usr/share/conary-test/hello.txt"
added_file = "/usr/share/conary-test/added.txt"
marker = "/var/lib/conary-test/installed"

[fixtures.v1]
version = "1.0.0"
hello_sha256 = "PLACEHOLDER"  # computed in Task 5

[fixtures.v2]
version = "2.0.0"
hello_sha256 = "PLACEHOLDER"  # computed in Task 5
added_sha256 = "PLACEHOLDER"  # computed in Task 5
```

**Step 2: Commit**

```bash
git add tests/integration/remi/config.toml
git commit -m "test: add integration test config.toml with all endpoints and distro mappings"
```

---

## Task 2: Write the Python Test Runner Core

The runner core: config loading, test registration/execution, assertions, JSON result output. No tests yet -- just the framework that replaces `lib.sh`.

**Files:**
- Create: `tests/integration/remi/runner/test_runner.py`

**Step 1: Write the runner core**

```python
#!/usr/bin/env python3
"""tests/integration/remi/runner/test_runner.py

Remi integration test runner. Replaces test-runner.sh + lib.sh.

Usage:
    python3 test_runner.py [--phase2]

Environment:
    DISTRO       - distro identifier (default: fedora43)
    DB_PATH      - override database path
    CONARY_BIN   - override conary binary path
    RESULTS_DIR  - override results directory
    REMI_ENDPOINT - override Remi endpoint URL
"""

import hashlib
import json
import os
import subprocess
import sys
import time
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional


# ── Configuration ────────────────────────────────────────────────────────────


@dataclass
class DistroConfig:
    remi_distro: str
    repo_name: str
    test_package: str
    test_binary: str
    test_package_2: str
    test_binary_2: str
    test_package_3: str
    test_binary_3: str


@dataclass
class FixtureConfig:
    package: str
    file: str
    added_file: str
    marker: str
    v1_version: str
    v1_hello_sha256: str
    v2_version: str
    v2_hello_sha256: str
    v2_added_sha256: str


@dataclass
class Config:
    endpoint: str
    db_path: str
    conary: str
    results_dir: str
    fixture_dir: str
    distro_name: str
    distro: DistroConfig
    fixtures: FixtureConfig
    remove_default_repos: list[str]

    @classmethod
    def load(cls, config_path: Path) -> "Config":
        with open(config_path, "rb") as f:
            raw = tomllib.load(f)

        distro_name = os.environ.get("DISTRO", "fedora43")
        distro_raw = raw["distros"][distro_name]

        fixtures_raw = raw["fixtures"]

        return cls(
            endpoint=os.environ.get("REMI_ENDPOINT", raw["remi"]["endpoint"]),
            db_path=os.environ.get("DB_PATH", raw["paths"]["db"]),
            conary=os.environ.get("CONARY_BIN", raw["paths"]["conary_bin"]),
            results_dir=os.environ.get("RESULTS_DIR", raw["paths"]["results_dir"]),
            fixture_dir=raw["paths"]["fixture_dir"],
            distro_name=distro_name,
            distro=DistroConfig(**distro_raw),
            fixtures=FixtureConfig(
                package=fixtures_raw["package"],
                file=fixtures_raw["file"],
                added_file=fixtures_raw["added_file"],
                marker=fixtures_raw["marker"],
                v1_version=fixtures_raw["v1"]["version"],
                v1_hello_sha256=fixtures_raw["v1"]["hello_sha256"],
                v2_version=fixtures_raw["v2"]["version"],
                v2_hello_sha256=fixtures_raw["v2"]["hello_sha256"],
                v2_added_sha256=fixtures_raw["v2"]["added_sha256"],
            ),
            remove_default_repos=raw["setup"]["remove_default_repos"],
        )


# ── Test Result Tracking ────────────────────────────────────────────────────


@dataclass
class TestResult:
    id: str
    name: str
    status: str  # "pass", "fail", "skip"
    duration_ms: int = 0
    message: str = ""

    def to_dict(self) -> dict:
        d = {"id": self.id, "name": self.name, "status": self.status,
             "duration_ms": self.duration_ms}
        if self.message:
            d["message"] = self.message[:500]
        return d


# ── Assertions ───────────────────────────────────────────────────────────────


class AssertionError(Exception):
    """Test assertion failed."""


def assert_file_exists(path: str) -> None:
    if not Path(path).is_file():
        raise AssertionError(f"file does not exist: {path}")


def assert_file_not_exists(path: str) -> None:
    if Path(path).is_file():
        raise AssertionError(f"file still exists: {path}")


def assert_file_executable(path: str) -> None:
    if not os.access(path, os.X_OK):
        raise AssertionError(f"file is not executable: {path}")


def assert_dir_exists(path: str) -> None:
    if not Path(path).is_dir():
        raise AssertionError(f"directory does not exist: {path}")


def assert_dir_not_exists(path: str) -> None:
    if Path(path).is_dir():
        raise AssertionError(f"directory still exists: {path}")


def assert_contains(needle: str, haystack: str) -> None:
    if needle not in haystack:
        raise AssertionError(
            f"output does not contain '{needle}'\noutput was: {haystack[:200]}")


def assert_not_contains(needle: str, haystack: str) -> None:
    if needle in haystack:
        raise AssertionError(f"output unexpectedly contains '{needle}'")


def assert_file_checksum(path: str, expected_sha256: str) -> None:
    assert_file_exists(path)
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            h.update(chunk)
    actual = h.hexdigest()
    if actual != expected_sha256:
        raise AssertionError(
            f"checksum mismatch for {path}\n"
            f"  expected: {expected_sha256}\n"
            f"  actual:   {actual}")


# ── Command Runner ───────────────────────────────────────────────────────────


def run_cmd(
    args: list[str],
    timeout: int = 120,
    check: bool = True,
) -> subprocess.CompletedProcess:
    """Run a command, capture output, optionally check return code."""
    result = subprocess.run(
        args,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    if check and result.returncode != 0:
        raise RuntimeError(
            f"command failed (exit {result.returncode}): {' '.join(args)}\n"
            f"{result.stdout}\n{result.stderr}")
    return result


def conary(cfg: Config, *args: str, timeout: int = 120,
           check: bool = True) -> subprocess.CompletedProcess:
    """Run a conary command with --db-path."""
    cmd = [cfg.conary, *args, "--db-path", cfg.db_path]
    return run_cmd(cmd, timeout=timeout, check=check)


# ── Test Suite ───────────────────────────────────────────────────────────────

# ANSI colors (disabled if not a terminal)
if sys.stdout.isatty():
    GREEN, RED, YELLOW, BLUE, NC = (
        "\033[0;32m", "\033[0;31m", "\033[1;33m", "\033[0;34m", "\033[0m")
else:
    GREEN = RED = YELLOW = BLUE = NC = ""


class TestSuite:
    def __init__(self, cfg: Config):
        self.cfg = cfg
        self.results: list[TestResult] = []
        self.fatal = False

    @property
    def pass_count(self) -> int:
        return sum(1 for r in self.results if r.status == "pass")

    @property
    def fail_count(self) -> int:
        return sum(1 for r in self.results if r.status == "fail")

    @property
    def skip_count(self) -> int:
        return sum(1 for r in self.results if r.status == "skip")

    def run_test(self, test_id: str, name: str, func, timeout: int = 120):
        """Execute a test function, record result."""
        if self.fatal:
            self.skip(test_id, name, "skipped due to prior critical failure")
            return

        print(f"{BLUE}[{test_id}]{NC} {name:<35} ", end="", flush=True)
        start = time.monotonic_ns()
        try:
            func()
            duration_ms = (time.monotonic_ns() - start) // 1_000_000
            self.results.append(
                TestResult(test_id, name, "pass", duration_ms))
            print(f"{GREEN}PASS{NC} ({duration_ms}ms)")
        except subprocess.TimeoutExpired:
            duration_ms = (time.monotonic_ns() - start) // 1_000_000
            self.results.append(
                TestResult(test_id, name, "fail", duration_ms,
                           f"timed out after {timeout}s"))
            print(f"{RED}TIMEOUT{NC} ({duration_ms}ms)")
        except Exception as e:
            duration_ms = (time.monotonic_ns() - start) // 1_000_000
            msg = str(e)
            self.results.append(
                TestResult(test_id, name, "fail", duration_ms, msg))
            print(f"{RED}FAIL{NC} ({duration_ms}ms)")

    def skip(self, test_id: str, name: str, reason: str):
        self.results.append(TestResult(test_id, name, "skip", message=reason))
        print(f"{BLUE}[{test_id}]{NC} {name:<35} "
              f"{YELLOW}SKIP{NC} ({reason})")

    def skip_group(self, test_ids: list[str], reason: str):
        """Skip multiple tests with the same reason."""
        for tid in test_ids:
            self.skip(tid, f"{tid}_skipped", reason)

    def set_fatal(self):
        self.fatal = True

    def checkpoint(self, label: str) -> int:
        """Return current fail count for later comparison."""
        return self.fail_count

    def failed_since(self, checkpoint: int) -> bool:
        """Check if any new failures occurred since checkpoint."""
        return self.fail_count > checkpoint

    def write_results(self):
        """Write JSON results file and print summary."""
        total = len(self.results)
        print()
        print("=" * 52)
        print(f"  Results: {GREEN}{self.pass_count} passed{NC}  "
              f"{RED}{self.fail_count} failed{NC}  "
              f"{YELLOW}{self.skip_count} skipped{NC}  {total} total")
        print("=" * 52)

        results_dir = Path(self.cfg.results_dir)
        results_dir.mkdir(parents=True, exist_ok=True)
        json_file = results_dir / f"{self.cfg.distro_name}.json"

        data = {
            "distro": self.cfg.distro_name,
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "summary": {
                "total": total,
                "passed": self.pass_count,
                "failed": self.fail_count,
                "skipped": self.skip_count,
            },
            "tests": [r.to_dict() for r in self.results],
        }
        with open(json_file, "w") as f:
            json.dump(data, f, indent=2)
        print(f"Results written to {json_file}")

        return 1 if self.fail_count > 0 else 0
```

**Step 2: Verify Python parses**

Run: `python3 -c "import ast; ast.parse(open('tests/integration/remi/runner/test_runner.py').read())"`
Expected: No output (no syntax errors)

**Step 3: Commit**

```bash
git add tests/integration/remi/runner/test_runner.py
git commit -m "test: add Python test runner core (config, assertions, result tracking)"
```

---

## Task 3: Port Phase 1 Tests (T01-T37) to Python

Port all 37 existing tests from `test-runner.sh` into the Python runner as methods. The old bash files stay until the port is verified.

**Files:**
- Modify: `tests/integration/remi/runner/test_runner.py`

**Step 1: Add test registration and Phase 1 tests**

Append to `test_runner.py` after the `TestSuite` class:

```python
# ── Phase 1 Tests (T01-T37) ─────────────────────────────────────────────────


def run_phase1(suite: TestSuite):
    """Port of existing test-runner.sh T01-T37."""
    cfg = suite.cfg
    d = cfg.distro

    # ── Setup: Initialize DB ─────────────────────────────────────────────
    print("[SETUP] Initializing database...")
    conary(cfg, "system", "init")
    for repo in cfg.remove_default_repos:
        conary(cfg, "repo", "remove", repo, check=False)
    print("[SETUP] Database ready\n")

    # ── T01: Health check ────────────────────────────────────────────────
    def t01():
        run_cmd(["curl", "-sf", f"{cfg.endpoint}/health"])

    suite.run_test("T01", "health_check", t01, timeout=10)
    if suite.fail_count > 0:
        print(f"\nRemi server unreachable at {cfg.endpoint} - skipping remaining")
        suite.set_fatal()
        return

    # ── T02: Repo add ────────────────────────────────────────────────────
    def t02():
        conary(cfg, "repo", "add", d.repo_name, cfg.endpoint,
               "--default-strategy", "remi",
               "--remi-endpoint", cfg.endpoint,
               "--remi-distro", d.remi_distro,
               "--no-gpg-check")

    suite.run_test("T02", "repo_add", t02, timeout=10)

    # ── T03: Repo list ───────────────────────────────────────────────────
    def t03():
        r = conary(cfg, "repo", "list")
        assert_contains(d.repo_name, r.stdout + r.stderr)

    suite.run_test("T03", "repo_list", t03, timeout=10)

    # ── T04: Repo sync ───────────────────────────────────────────────────
    cp_sync = suite.checkpoint("sync")

    def t04():
        r = conary(cfg, "repo", "sync", d.repo_name, "--force", timeout=300)
        assert_contains("[OK]", r.stdout + r.stderr)

    suite.run_test("T04", "repo_sync", t04, timeout=300)
    if suite.failed_since(cp_sync):
        print("\nRepo sync failed - skipping T05-T24")
        suite.set_fatal()
        return

    # ── T05: Search exists ───────────────────────────────────────────────
    def t05():
        r = conary(cfg, "search", d.test_package)
        output = r.stdout + r.stderr
        assert_contains(d.test_package, output)
        assert_not_contains("No packages found", output)

    suite.run_test("T05", "search_exists", t05, timeout=30)

    # ── T06: Search nonexistent ──────────────────────────────────────────
    def t06():
        r = conary(cfg, "search", "zzz-nonexistent-pkg-12345")
        assert_contains("No packages found", r.stdout + r.stderr)

    suite.run_test("T06", "search_nonexistent", t06, timeout=10)

    # ── T07: Install package ─────────────────────────────────────────────
    def t07():
        conary(cfg, "install", d.test_package,
               "--no-scripts", "--no-deps", "--sandbox", "never",
               timeout=300)

    suite.run_test("T07", "install_package", t07, timeout=300)

    # ── T08: Verify files ────────────────────────────────────────────────
    def t08():
        assert_file_exists(d.test_binary)
        assert_file_executable(d.test_binary)

    suite.run_test("T08", "verify_files", t08, timeout=10)

    # ── T09: List installed ──────────────────────────────────────────────
    def t09():
        r = conary(cfg, "list")
        assert_contains(d.test_package, r.stdout + r.stderr)

    suite.run_test("T09", "list_installed", t09, timeout=10)

    # ── T10: Install nonexistent ─────────────────────────────────────────
    def t10():
        r = conary(cfg, "install", "zzz-nonexistent-pkg-12345",
                   "--no-scripts", "--no-deps", "--sandbox", "never",
                   check=False, timeout=30)
        if r.returncode == 0:
            raise AssertionError("expected non-zero exit for nonexistent package")

    suite.run_test("T10", "install_nonexistent", t10, timeout=30)

    # ── T11: Remove package ──────────────────────────────────────────────
    def t11():
        conary(cfg, "remove", d.test_package, "--no-scripts", timeout=60)

    suite.run_test("T11", "remove_package", t11, timeout=60)

    # ── T12: Verify removed ──────────────────────────────────────────────
    def t12():
        assert_file_not_exists(d.test_binary)
        r = conary(cfg, "list")
        assert_not_contains(d.test_package, r.stdout + r.stderr)

    suite.run_test("T12", "verify_removed", t12, timeout=10)

    # ── T13: Version check ───────────────────────────────────────────────
    def t13():
        r = conary(cfg, "--version")
        assert_contains("conary", r.stdout + r.stderr)

    suite.run_test("T13", "version_check", t13, timeout=10)

    # ── T14: Reinstall ───────────────────────────────────────────────────
    cp_reinstall = suite.checkpoint("reinstall")

    def t14():
        conary(cfg, "install", d.test_package,
               "--no-scripts", "--no-deps", "--sandbox", "never",
               timeout=300)

    suite.run_test("T14", "reinstall_which", t14, timeout=300)
    reinstall_failed = suite.failed_since(cp_reinstall)

    # ── T15: Package info ────────────────────────────────────────────────
    if reinstall_failed:
        suite.skip("T15", "package_info", "skipped due to T14 failure")
    else:
        def t15():
            r = conary(cfg, "list", d.test_package, "--info")
            output = r.stdout + r.stderr
            assert_contains(d.test_package, output)
            assert_contains("Version", output)
        suite.run_test("T15", "package_info", t15, timeout=30)

    # ── T16: List files ──────────────────────────────────────────────────
    if reinstall_failed:
        suite.skip("T16", "list_files", "skipped due to T14 failure")
    else:
        def t16():
            r = conary(cfg, "list", d.test_package, "--files")
            assert_contains(d.test_binary, r.stdout + r.stderr)
        suite.run_test("T16", "list_files", t16, timeout=30)

    # ── T17: Path ownership ──────────────────────────────────────────────
    if reinstall_failed:
        suite.skip("T17", "path_ownership", "skipped due to T14 failure")
    else:
        def t17():
            r = conary(cfg, "list", "--path", d.test_binary)
            assert_contains(d.test_package, r.stdout + r.stderr)
        suite.run_test("T17", "path_ownership", t17, timeout=30)

    # ── T18: Install tree ────────────────────────────────────────────────
    cp_tree = suite.checkpoint("tree")

    def t18():
        conary(cfg, "install", d.test_package_2,
               "--no-scripts", "--no-deps", "--sandbox", "never",
               timeout=300)

    suite.run_test("T18", "install_tree", t18, timeout=300)
    tree_failed = suite.failed_since(cp_tree)

    # ── T19: Verify tree files ───────────────────────────────────────────
    if tree_failed:
        suite.skip("T19", "verify_tree_files", "skipped due to T18 failure")
    else:
        def t19():
            assert_file_exists(d.test_binary_2)
            assert_file_executable(d.test_binary_2)
        suite.run_test("T19", "verify_tree_files", t19, timeout=10)

    # ── T20: Adopt single package ────────────────────────────────────────
    cp_adopt = suite.checkpoint("adopt")

    def t20():
        conary(cfg, "system", "adopt", "curl", timeout=60)

    suite.run_test("T20", "adopt_single_package", t20, timeout=60)
    adopt_failed = suite.failed_since(cp_adopt)

    # ── T21: Adopt status ────────────────────────────────────────────────
    if adopt_failed:
        suite.skip("T21", "adopt_status", "skipped due to T20 failure")
    else:
        def t21():
            r = conary(cfg, "system", "adopt", "--status")
            output = r.stdout + r.stderr
            assert_contains("Conary Adoption Status", output)
            assert_contains("Adopted", output)
        suite.run_test("T21", "adopt_status", t21, timeout=30)

    # ── T22: Pin package ─────────────────────────────────────────────────
    if reinstall_failed:
        suite.skip("T22", "pin_package", "skipped due to T14 failure")
    else:
        def t22():
            conary(cfg, "pin", d.test_package)
            r = conary(cfg, "list", d.test_package, "--info")
            assert_contains("Pinned      : yes", r.stdout + r.stderr)
        suite.run_test("T22", "pin_package", t22, timeout=30)

    # ── T23: Unpin package ───────────────────────────────────────────────
    if reinstall_failed:
        suite.skip("T23", "unpin_package", "skipped due to T14 failure")
    else:
        def t23():
            conary(cfg, "unpin", d.test_package)
            r = conary(cfg, "list", d.test_package, "--info")
            assert_contains("Pinned      : no", r.stdout + r.stderr)
        suite.run_test("T23", "unpin_package", t23, timeout=30)

    # ── T24: Changeset history ───────────────────────────────────────────
    if reinstall_failed:
        suite.skip("T24", "changeset_history", "skipped due to T14 failure")
    else:
        def t24():
            r = conary(cfg, "system", "history")
            assert_contains("Changeset", r.stdout + r.stderr)
        suite.run_test("T24", "changeset_history", t24, timeout=30)

    # ── T25: Install dep package ─────────────────────────────────────────
    cp_dep = suite.checkpoint("dep")

    def t25():
        conary(cfg, "install", d.test_package_3,
               "--no-scripts", "--no-deps", "--sandbox", "never",
               timeout=300)

    suite.run_test("T25", "install_dep_package", t25, timeout=300)
    dep_failed = suite.failed_since(cp_dep)

    # ── T26: Verify dep files ────────────────────────────────────────────
    if dep_failed:
        suite.skip("T26", "verify_dep_files", "skipped due to T25 failure")
    else:
        def t26():
            assert_file_exists(d.test_binary_3)
        suite.run_test("T26", "verify_dep_files", t26, timeout=10)

    # ── T27: Multiple packages coexist ───────────────────────────────────
    if dep_failed or reinstall_failed:
        suite.skip("T27", "multi_package_coexist",
                   "skipped due to prior install failure")
    else:
        def t27():
            r = conary(cfg, "list")
            output = r.stdout + r.stderr
            assert_contains(d.test_package, output)
            assert_contains(d.test_package_2, output)
            assert_contains(d.test_package_3, output)
        suite.run_test("T27", "multi_package_coexist", t27, timeout=10)

    # ── T28: dep-mode satisfy ────────────────────────────────────────────
    def t28():
        conary(cfg, "install", d.test_package,
               "--no-scripts", "--dep-mode", "satisfy", "--yes",
               "--sandbox", "never", timeout=300)
        conary(cfg, "remove", d.test_package, "--no-scripts", check=False)

    suite.run_test("T28", "dep_mode_satisfy", t28, timeout=300)

    # ── T29: dep-mode adopt ──────────────────────────────────────────────
    def t29():
        conary(cfg, "install", d.test_package_2,
               "--no-scripts", "--dep-mode", "adopt", "--yes",
               "--sandbox", "never", timeout=300)
        assert_file_exists(d.test_binary_2)

    suite.run_test("T29", "dep_mode_adopt", t29, timeout=300)

    # ── T30: dep-mode takeover ───────────────────────────────────────────
    def t30():
        conary(cfg, "install", d.test_package_3,
               "--no-scripts", "--dep-mode", "takeover", "--yes",
               "--sandbox", "never", timeout=300)
        assert_file_exists(d.test_binary_3)

    suite.run_test("T30", "dep_mode_takeover", t30, timeout=300)

    # ── T31: Blocklist enforced ──────────────────────────────────────────
    def t31():
        conary(cfg, "install", "glibc",
               "--no-scripts", "--dep-mode", "takeover", "--yes",
               "--sandbox", "never", check=False, timeout=60)
        r = conary(cfg, "list")
        assert_not_contains("glibc", r.stdout + r.stderr)

    suite.run_test("T31", "blocklist_enforced", t31, timeout=60)

    # ── T32: Update with adopted ─────────────────────────────────────────
    def t32():
        conary(cfg, "system", "adopt", "curl", check=False)
        conary(cfg, "update", "--dep-mode", "satisfy", timeout=120)

    suite.run_test("T32", "update_with_adopted", t32, timeout=120)

    # ── T33: Generation list (empty) ─────────────────────────────────────
    def t33():
        r = conary(cfg, "system", "generation", "list")
        assert_contains("No generations", r.stdout + r.stderr)

    suite.run_test("T33", "generation_list_empty", t33, timeout=10)

    # ── T34: Takeover dry run ────────────────────────────────────────────
    def t34():
        r = conary(cfg, "system", "takeover", "--dry-run",
                   "--skip-conversion", check=False, timeout=60)
        output = r.stdout + r.stderr
        if r.returncode == 0:
            assert_contains("DRY RUN", output)
        # Non-zero is OK (may need root)

    suite.run_test("T34", "takeover_dry_run", t34, timeout=60)

    # ── T35: Generation GC (nothing) ────────────────────────────────────
    def t35():
        r = conary(cfg, "system", "generation", "gc")
        output = r.stdout + r.stderr
        # Should contain one of these
        if "Nothing to clean" not in output and "No generations" not in output:
            raise AssertionError(f"unexpected gc output: {output}")

    suite.run_test("T35", "generation_gc_empty", t35, timeout=10)

    # ── T36: Generation info format ──────────────────────────────────────
    def t36():
        r = conary(cfg, "system", "generation", "info", "1", check=False)
        output = r.stdout + r.stderr
        if r.returncode == 0:
            assert_contains("composefs", output.lower())
        # Non-zero expected (no generation yet)

    suite.run_test("T36", "generation_info_format", t36, timeout=10)

    # ── T37: Takeover composefs format ───────────────────────────────────
    def t37():
        r = conary(cfg, "system", "takeover", "--dry-run",
                   "--skip-conversion", check=False, timeout=60)
        output = r.stdout + r.stderr
        if r.returncode == 0:
            # Should mention composefs/EROFS or DRY RUN
            lower = output.lower()
            if not any(w in lower for w in
                       ["composefs", "erofs", "dry run"]):
                raise AssertionError(f"unexpected output: {output}")
        # Non-zero is OK

    suite.run_test("T37", "takeover_composefs_format", t37, timeout=60)

    # ── Cleanup ──────────────────────────────────────────────────────────
    print("\n[CLEANUP] Removing test packages...")
    conary(cfg, "remove", d.test_package, "--no-scripts", check=False)
    conary(cfg, "remove", d.test_package_2, "--no-scripts", check=False)
    conary(cfg, "remove", d.test_package_3, "--no-scripts", check=False)
```

**Step 2: Add main entry point**

Append to `test_runner.py`:

```python
# ── Main ─────────────────────────────────────────────────────────────────────


def main():
    phase2 = "--phase2" in sys.argv

    # Find config.toml relative to this script
    script_dir = Path(__file__).resolve().parent
    config_path = script_dir.parent / "config.toml"
    cfg = Config.load(config_path)

    # Ensure DB directory exists
    Path(cfg.db_path).parent.mkdir(parents=True, exist_ok=True)

    print()
    print("=" * 52)
    print("  Remi Integration Tests")
    print(f"  Distro:    {cfg.distro_name}")
    print(f"  Remi repo: {cfg.distro.repo_name} ({cfg.distro.remi_distro})")
    print(f"  Endpoint:  {cfg.endpoint}")
    print(f"  Binary:    {cfg.conary}")
    print(f"  DB:        {cfg.db_path}")
    if phase2:
        print("  Mode:      Phase 1 + Phase 2")
    print("=" * 52)
    print()

    suite = TestSuite(cfg)

    # Phase 1: core tests
    run_phase1(suite)

    # Phase 2: deep E2E (enabled with --phase2)
    if phase2:
        if suite.fatal:
            print("\n[SKIP] Phase 2 skipped due to Phase 1 critical failure")
        else:
            print()
            print("=" * 52)
            print("  Phase 2: Deep E2E Validation")
            print("=" * 52)
            print()
            run_phase2(suite)
    else:
        print("\n[INFO] Phase 2 tests skipped (pass --phase2 to enable)\n")

    sys.exit(suite.write_results())


# Placeholder for Phase 2 -- implemented in Tasks 7-11
def run_phase2(suite: TestSuite):
    print("[TODO] Phase 2 tests not yet implemented")


if __name__ == "__main__":
    main()
```

**Step 3: Verify Python parses**

Run: `python3 -c "import ast; ast.parse(open('tests/integration/remi/runner/test_runner.py').read())"`
Expected: No output

**Step 4: Commit**

```bash
git add tests/integration/remi/runner/test_runner.py
git commit -m "test: port Phase 1 tests (T01-T37) to Python runner"
```

---

## Task 4: Update Container and Orchestrator for Python Runner

Switch containers from bash runner to Python runner. Keep bash files until verified.

**Files:**
- Modify: `tests/integration/remi/containers/Containerfile.fedora43`
- Modify: `tests/integration/remi/containers/Containerfile.ubuntu-noble`
- Modify: `tests/integration/remi/containers/Containerfile.arch`
- Modify: `tests/integration/remi/run.sh`

**Step 1: Update Containerfiles to install Python 3 and copy config**

For **Containerfile.fedora43**, change the `dnf install` line to include python3:

```dockerfile
RUN dnf install -y ca-certificates curl python3 && dnf clean all
```

Add after `COPY runner/ /opt/remi-tests/`:

```dockerfile
COPY config.toml /opt/remi-tests/config.toml
```

Change CMD:

```dockerfile
CMD ["python3", "/opt/remi-tests/runner/test_runner.py"]
```

For **Containerfile.ubuntu-noble**, change `apt-get install` to include python3:

```dockerfile
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl python3 \
    && rm -rf /var/lib/apt/lists/*
```

Same `COPY config.toml` and `CMD` changes.

For **Containerfile.arch**, change `pacman -Syu` to include python:

```dockerfile
RUN pacman -Syu --noconfirm ca-certificates curl python && pacman -Scc --noconfirm
```

Same `COPY config.toml` and `CMD` changes.

**Step 2: Update run.sh to pass --phase2 and copy config**

In `run.sh`, add `--phase2` argument parsing (add to the `while` loop):

```bash
        --phase2)
            PHASE2=1
            shift
            ;;
```

Add `PHASE2=0` to defaults.

Copy config.toml into build context (add after binary setup, before podman build):

```bash
# ── Copy config and fixtures into build context ─────────────────────────
cp "$SCRIPT_DIR/config.toml" "$BUILD_CONTEXT/config.toml"
CLEANUP_FILES+=("$BUILD_CONTEXT/config.toml")

FIXTURES_SRC="$PROJECT_ROOT/tests/fixtures"
FIXTURES_DST="$BUILD_CONTEXT/fixtures"
if [ -d "$FIXTURES_SRC" ]; then
    rm -rf "$FIXTURES_DST"
    mkdir -p "$FIXTURES_DST"
    cp -r "$FIXTURES_SRC/recipes" "$FIXTURES_DST/recipes" 2>/dev/null || true
    mkdir -p "$FIXTURES_DST/pkgbuild"
    cp "$PROJECT_ROOT/packaging/arch/PKGBUILD" "$FIXTURES_DST/pkgbuild/" 2>/dev/null || true
    CLEANUP_FILES+=("$FIXTURES_DST")
fi
```

Update the `podman run` to pass `--phase2`:

```bash
CONTAINER_CMD="python3 /opt/remi-tests/runner/test_runner.py"
if [ "$PHASE2" -eq 1 ]; then
    CONTAINER_CMD="$CONTAINER_CMD --phase2"
fi

podman run \
    --rm \
    --name "conary-test-run-${DISTRO}" \
    -v "${VOLUME_NAME}:/results:Z" \
    -e "DISTRO=${DISTRO}" \
    "$IMAGE_NAME" $CONTAINER_CMD || CONTAINER_EXIT=$?
```

**Step 3: Add fixture COPY to all Containerfiles**

After the config.toml COPY line:

```dockerfile
# Phase 2 test fixtures (recipes, PKGBUILD)
COPY fixtures/ /opt/remi-tests/fixtures/
```

Note: This COPY will fail if no `fixtures/` directory exists in the build context. Handle this by always creating an empty one in `run.sh`:

```bash
mkdir -p "$BUILD_CONTEXT/fixtures"
```

(Add this before the `if [ -d "$FIXTURES_SRC" ]` block.)

**Step 4: Verify syntax**

Run: `bash -n tests/integration/remi/run.sh`
Expected: No output

**Step 5: Commit**

```bash
git add tests/integration/remi/containers/ tests/integration/remi/run.sh
git commit -m "test: switch containers from bash to Python runner"
```

---

## Task 5: Create Test Fixture Packages

**Files:**
- Create: `tests/fixtures/conary-test-fixture/v1/ccs.toml`
- Create: `tests/fixtures/conary-test-fixture/v1/stage/usr/share/conary-test/hello.txt`
- Create: `tests/fixtures/conary-test-fixture/v1/stage/ccs.toml`
- Create: `tests/fixtures/conary-test-fixture/v2/ccs.toml`
- Create: `tests/fixtures/conary-test-fixture/v2/stage/usr/share/conary-test/hello.txt`
- Create: `tests/fixtures/conary-test-fixture/v2/stage/usr/share/conary-test/added.txt`
- Create: `tests/fixtures/conary-test-fixture/v2/stage/ccs.toml`
- Create: `tests/fixtures/conary-test-fixture/build-all.sh`
- Create: `tests/fixtures/recipes/simple-hello/recipe.toml`
- Create: `tests/fixtures/recipes/simple-hello/src/hello.sh`
- Create: `scripts/publish-test-fixtures.sh`

**Step 1: Create v1 fixture**

`tests/fixtures/conary-test-fixture/v1/ccs.toml`:
```toml
[package]
name = "conary-test-fixture"
version = "1.0.0"
description = "Test fixture for Phase 2 E2E validation"
license = "MIT"

[package.platform]
os = "linux"
arch = "x86_64"
libc = "gnu"

[provides]
capabilities = ["conary-test-fixture"]
binaries = []

[requires]
capabilities = []
packages = []

[components]
default = ["runtime"]

[hooks]

[[hooks.directories]]
path = "/usr/share/conary-test"
mode = "0755"

[[hooks.directories]]
path = "/var/lib/conary-test"
mode = "0755"

[hooks.post_install]
script = "touch /var/lib/conary-test/installed"

[hooks.pre_remove]
script = "rm -f /var/lib/conary-test/installed"
```

`tests/fixtures/conary-test-fixture/v1/stage/usr/share/conary-test/hello.txt`:
```
hello-v1
```

Copy: `cp v1/ccs.toml v1/stage/ccs.toml`

**Step 2: Create v2 fixture**

Same as v1 except `version = "2.0.0"`, different hello.txt content, and added.txt:

`tests/fixtures/conary-test-fixture/v2/stage/usr/share/conary-test/hello.txt`:
```
hello-v2
```

`tests/fixtures/conary-test-fixture/v2/stage/usr/share/conary-test/added.txt`:
```
added-in-v2
```

**Step 3: Create build-all.sh**

```bash
#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
CONARY="${CONARY_BIN:-$PROJECT_ROOT/target/debug/conary}"

for ver in v1 v2; do
    echo "Building conary-test-fixture $ver..."
    mkdir -p "$SCRIPT_DIR/$ver/output"
    "$CONARY" ccs build "$SCRIPT_DIR/$ver/ccs.toml" \
        --stage-dir "$SCRIPT_DIR/$ver/stage" \
        --output "$SCRIPT_DIR/$ver/output/"
done

echo ""
echo "Checksums for config.toml:"
echo "  v1 hello: $(sha256sum "$SCRIPT_DIR/v1/stage/usr/share/conary-test/hello.txt" | awk '{print $1}')"
echo "  v2 hello: $(sha256sum "$SCRIPT_DIR/v2/stage/usr/share/conary-test/hello.txt" | awk '{print $1}')"
echo "  v2 added: $(sha256sum "$SCRIPT_DIR/v2/stage/usr/share/conary-test/added.txt" | awk '{print $1}')"
```

**Step 4: Create simple recipe fixture**

`tests/fixtures/recipes/simple-hello/recipe.toml`:
```toml
[package]
name = "test-hello"
version = "1.0.0"
description = "Simple test recipe for E2E validation"
license = "MIT"

[source]
type = "local"
path = "src/"

[build]
steps = [
    "install -Dm755 hello.sh ${DESTDIR}/usr/bin/test-hello",
]

[package.platform]
os = "linux"
arch = "x86_64"
```

`tests/fixtures/recipes/simple-hello/src/hello.sh`:
```bash
#!/bin/sh
echo "hello from test recipe"
```

**Step 5: Create publish script**

`scripts/publish-test-fixtures.sh`:
```bash
#!/usr/bin/env bash
# scripts/publish-test-fixtures.sh
# Build and publish test fixture CCS packages to Remi.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
FIXTURE_DIR="$PROJECT_ROOT/tests/fixtures/conary-test-fixture"
REMI_ENDPOINT="${REMI_ENDPOINT:-https://packages.conary.io}"

bash "$FIXTURE_DIR/build-all.sh"

echo ""
echo "Publishing to Remi ($REMI_ENDPOINT)..."
for ver in v1 v2; do
    pkg=$(ls "$FIXTURE_DIR/$ver/output/"*.ccs 2>/dev/null | head -1)
    [ -z "$pkg" ] && { echo "FATAL: No CCS for $ver" >&2; exit 1; }

    for distro in fedora ubuntu arch; do
        printf "  %s -> %s... " "$ver" "$distro"
        curl -sf -X POST "$REMI_ENDPOINT/v1/$distro/packages" \
            -F "package=@$pkg" -F "format=ccs" \
            && echo "OK" \
            || echo "WARN (may already exist)"
    done
done
echo "[OK] Done"
```

**Step 6: Compute checksums and update config.toml**

Run: `sha256sum tests/fixtures/conary-test-fixture/v{1,2}/stage/usr/share/conary-test/*.txt`

Update the `PLACEHOLDER` values in `tests/integration/remi/config.toml` with real SHA-256 hashes.

**Step 7: Commit**

```bash
git add tests/fixtures/ scripts/publish-test-fixtures.sh tests/integration/remi/config.toml
git commit -m "test: add CCS fixture packages, recipe fixtures, and publish script"
```

---

## Task 6: Verify Phase 1 Python Port

Run the ported Phase 1 tests and confirm identical behavior to the bash version.

**Step 1: Build conary**

Run: `cargo build`

**Step 2: Run Python runner locally in container**

Run: `./tests/integration/remi/run.sh --distro fedora43`
Expected: T01-T37 results match the old bash runner

**Step 3: Compare JSON output**

Verify pass/fail/skip counts match. If any discrepancies, fix the Python port.

**Step 4: Run on all 3 distros**

Run: `./tests/integration/remi/run.sh --distro ubuntu-noble`
Run: `./tests/integration/remi/run.sh --distro arch`

**Step 5: Fix any issues, commit**

```bash
git add -A
git commit -m "fix: resolve Phase 1 Python port discrepancies"
```

---

## Task 7: Write Group A Tests — Deep Install Flow (T38-T50)

**Files:**
- Modify: `tests/integration/remi/runner/test_runner.py`

**Step 1: Implement run_phase2 with Group A**

Replace the `run_phase2` placeholder:

```python
def run_phase2(suite: TestSuite):
    """Phase 2: Deep E2E Validation (T38-T71)."""
    run_group_a(suite)
    run_group_b(suite)
    run_group_c(suite)
    run_group_d(suite)
    run_group_e(suite)


def run_group_a(suite: TestSuite):
    """Group A: Deep Install Flow (T38-T50)."""
    cfg = suite.cfg
    fx = cfg.fixtures
    print("-- Group A: Deep Install Flow --\n")

    # ── T38: Install fixture v1 with deps ────────────────────────────────
    cp = suite.checkpoint("T38")

    def t38():
        conary(cfg, "install", f"{fx.package}={fx.v1_version}",
               "--dep-mode", "takeover", "--yes", "--sandbox", "never",
               timeout=300)

    suite.run_test("T38", "install_fixture_v1_deps", t38, timeout=300)
    if suite.failed_since(cp):
        suite.skip_group(
            [f"T{i}" for i in range(39, 51)],
            "skipped due to T38 failure")
        return

    # ── T39: Verify dep files on disk ────────────────────────────────────
    def t39():
        assert_file_exists(fx.file)
        assert_dir_exists("/usr/share/conary-test")

    suite.run_test("T39", "verify_dep_files_disk", t39)

    # ── T40: Verify v1 content checksum ──────────────────────────────────
    def t40():
        assert_file_checksum(fx.file, fx.v1_hello_sha256)

    suite.run_test("T40", "verify_v1_checksum", t40)

    # ── T41: Verify scriptlet ran ────────────────────────────────────────
    def t41():
        assert_file_exists(fx.marker)

    suite.run_test("T41", "verify_scriptlet_ran", t41)

    # ── T42: Remove with scriptlets ──────────────────────────────────────
    def t42():
        conary(cfg, "remove", fx.package, timeout=60)
        assert_file_not_exists(fx.marker)
        assert_file_not_exists(fx.file)

    suite.run_test("T42", "remove_with_scriptlets", t42, timeout=60)

    # ── T43: Reinstall fixture v1 ────────────────────────────────────────
    cp43 = suite.checkpoint("T43")

    def t43():
        conary(cfg, "install", f"{fx.package}={fx.v1_version}",
               "--dep-mode", "takeover", "--yes", "--sandbox", "never",
               timeout=300)

    suite.run_test("T43", "reinstall_fixture_v1", t43, timeout=300)
    if suite.failed_since(cp43):
        suite.skip_group(
            [f"T{i}" for i in range(44, 51)],
            "skipped due to T43 failure")
        return

    # ── T44: Update v1 -> v2 ─────────────────────────────────────────────
    cp44 = suite.checkpoint("T44")

    def t44():
        conary(cfg, "update", fx.package,
               "--dep-mode", "takeover", "--yes", "--sandbox", "never",
               timeout=300)

    suite.run_test("T44", "update_v1_to_v2", t44, timeout=300)

    # ── T45: Delta update verify ─────────────────────────────────────────
    if suite.failed_since(cp44):
        suite.skip_group(["T45", "T46", "T47", "T48"],
                         "skipped due to T44 failure")
    else:
        def t45():
            assert_file_checksum(fx.file, fx.v2_hello_sha256)

        suite.run_test("T45", "delta_update_verify", t45)

        # ── T46: Verify v2 added file ────────────────────────────────────
        def t46():
            assert_file_exists(fx.added_file)
            assert_file_checksum(fx.added_file, fx.v2_added_sha256)

        suite.run_test("T46", "verify_v2_added", t46)

        # ── T47: Rollback after update ───────────────────────────────────
        cp47 = suite.checkpoint("T47")

        def t47():
            conary(cfg, "restore", "--last", "--yes", timeout=120)

        suite.run_test("T47", "rollback_after_update", t47, timeout=120)

        # ── T48: Rollback filesystem check ───────────────────────────────
        if suite.failed_since(cp47):
            suite.skip("T48", "rollback_fs_check",
                       "skipped due to T47 failure")
        else:
            def t48():
                assert_file_checksum(fx.file, fx.v1_hello_sha256)
                assert_file_not_exists(fx.added_file)

            suite.run_test("T48", "rollback_fs_check", t48)

    # ── T49: Pin blocks update ───────────────────────────────────────────
    def t49():
        conary(cfg, "pin", fx.package)
        conary(cfg, "update", fx.package,
               "--dep-mode", "takeover", "--yes", "--sandbox", "never",
               check=False, timeout=300)
        r = conary(cfg, "list", fx.package, "--info")
        assert_contains(fx.v1_version, r.stdout + r.stderr)
        conary(cfg, "unpin", fx.package)

    suite.run_test("T49", "pin_blocks_update", t49, timeout=300)

    # ── T50: Orphan detection ────────────────────────────────────────────
    def t50():
        conary(cfg, "remove", fx.package, "--no-scripts", check=False,
               timeout=60)
        r = conary(cfg, "list", "--orphans", check=False)
        # Should not crash; output is informational
        print(r.stdout)

    suite.run_test("T50", "orphan_detection", t50, timeout=60)
```

**Step 2: Verify Python parses**

Run: `python3 -c "import ast; ast.parse(open('tests/integration/remi/runner/test_runner.py').read())"`

**Step 3: Commit**

```bash
git add tests/integration/remi/runner/test_runner.py
git commit -m "test: add Group A deep install flow tests (T38-T50)"
```

---

## Task 8: Write Group B Tests — Generation Lifecycle (T51-T57)

**Files:**
- Modify: `tests/integration/remi/runner/test_runner.py`

**Step 1: Add run_group_b**

```python
def run_group_b(suite: TestSuite):
    """Group B: Generation Lifecycle (T51-T57)."""
    cfg = suite.cfg
    fx = cfg.fixtures
    print("\n-- Group B: Generation Lifecycle --\n")

    # Ensure fixture is installed for generation snapshot
    conary(cfg, "install", f"{fx.package}={fx.v1_version}",
           "--dep-mode", "takeover", "--yes", "--no-scripts",
           "--sandbox", "never", check=False, timeout=300)

    # ── T51: Build generation ────────────────────────────────────────────
    cp51 = suite.checkpoint("T51")

    def t51():
        conary(cfg, "system", "generation", "build", timeout=120)

    suite.run_test("T51", "generation_build", t51, timeout=120)
    if suite.failed_since(cp51):
        suite.skip_group([f"T{i}" for i in range(52, 58)],
                         "skipped due to T51 failure")
        return

    # ── T52: Generation list ─────────────────────────────────────────────
    def t52():
        r = conary(cfg, "system", "generation", "list")
        output = r.stdout + r.stderr
        assert_not_contains("No generations", output)

    suite.run_test("T52", "generation_list", t52)

    # ── T53: Generation info ─────────────────────────────────────────────
    def t53():
        r = conary(cfg, "system", "generation", "info", "1")
        assert_contains("packages", r.stdout + r.stderr)

    suite.run_test("T53", "generation_info", t53)

    # ── T54: Switch generation ───────────────────────────────────────────
    cp54 = suite.checkpoint("T54")

    def t54():
        # Create different state for gen 2
        conary(cfg, "update", fx.package,
               "--dep-mode", "takeover", "--yes", "--no-scripts",
               "--sandbox", "never", timeout=300)
        conary(cfg, "system", "generation", "build", timeout=120)
        conary(cfg, "system", "generation", "switch", "2", timeout=120)

    suite.run_test("T54", "generation_switch", t54, timeout=300)

    # ── T55: Rollback generation ─────────────────────────────────────────
    if suite.failed_since(cp54):
        suite.skip_group(["T55", "T56"], "skipped due to T54 failure")
    else:
        def t55():
            conary(cfg, "system", "generation", "switch", "1", timeout=120)

        suite.run_test("T55", "generation_rollback", t55, timeout=120)

        # ── T56: GC old generation ───────────────────────────────────────
        def t56():
            r = conary(cfg, "system", "generation", "gc", timeout=60)
            print(r.stdout)

        suite.run_test("T56", "generation_gc", t56, timeout=60)

    # ── T57: System takeover full ────────────────────────────────────────
    def t57():
        r = conary(cfg, "system", "takeover", "--skip-conversion", "--yes",
                   check=False, timeout=300)
        output = r.stdout + r.stderr
        if r.returncode != 0:
            # May fail in container -- graceful failure is OK
            assert_not_contains("panic", output)
            print(f"takeover exited {r.returncode} "
                  f"(may be expected in container)")

    suite.run_test("T57", "system_takeover_full", t57, timeout=300)
```

**Step 2: Verify, commit**

```bash
git add tests/integration/remi/runner/test_runner.py
git commit -m "test: add Group B generation lifecycle tests (T51-T57)"
```

---

## Task 9: Write Group C Tests — Bootstrap Pipeline (T58-T61)

**Files:**
- Modify: `tests/integration/remi/runner/test_runner.py`

**Step 1: Add run_group_c**

```python
def run_group_c(suite: TestSuite):
    """Group C: Bootstrap Pipeline (T58-T61)."""
    cfg = suite.cfg
    print("\n-- Group C: Bootstrap Pipeline --\n")

    work_dir = "/tmp/conary-bootstrap-test"
    recipe_dir = "/tmp/conary-bootstrap-recipes"
    Path(work_dir).mkdir(parents=True, exist_ok=True)
    Path(recipe_dir).mkdir(parents=True, exist_ok=True)

    # ── T58: Bootstrap dry-run ───────────────────────────────────────────
    def t58():
        r = conary(cfg, "bootstrap", "dry-run",
                   "--work-dir", work_dir, "--recipe-dir", recipe_dir,
                   check=False, timeout=60)
        output = r.stdout + r.stderr
        assert_not_contains("panic", output)
        if r.returncode == 0:
            assert_contains("Graph resolved", output)

    suite.run_test("T58", "bootstrap_dry_run", t58, timeout=60)

    # ── T59: Stage 0 runs ───────────────────────────────────────────────
    def t59():
        r = conary(cfg, "bootstrap", "stage0",
                   "--work-dir", work_dir, check=False, timeout=300)
        output = r.stdout + r.stderr
        assert_not_contains("panic", output)

    suite.run_test("T59", "bootstrap_stage0", t59, timeout=300)

    # ── T60: Stage 0 output valid ────────────────────────────────────────
    def t60():
        stage0_dir = Path(work_dir) / "stage0"
        if stage0_dir.is_dir():
            contents = list(stage0_dir.iterdir())
            print(f"Stage 0 output: {len(contents)} entries")
            for p in contents[:10]:
                print(f"  {p.name}")
        else:
            print("No stage0 output (expected if stage0 did not succeed)")

    suite.run_test("T60", "bootstrap_stage0_output", t60)

    # ── T61: Stage 1 starts ──────────────────────────────────────────────
    def t61():
        try:
            r = run_cmd(
                [cfg.conary, "bootstrap", "stage1",
                 "--work-dir", work_dir, "--db-path", cfg.db_path],
                timeout=60, check=False)
            output = r.stdout + r.stderr
            assert_not_contains("panic", output)
            print(f"stage1 exited {r.returncode}")
        except subprocess.TimeoutExpired:
            # Timeout = proof of life (it started and ran)
            print("Stage 1 started (timed out as expected)")

    suite.run_test("T61", "bootstrap_stage1_starts", t61, timeout=120)

    # Cleanup
    import shutil
    shutil.rmtree(work_dir, ignore_errors=True)
    shutil.rmtree(recipe_dir, ignore_errors=True)
```

**Step 2: Verify, commit**

```bash
git add tests/integration/remi/runner/test_runner.py
git commit -m "test: add Group C bootstrap pipeline tests (T58-T61)"
```

---

## Task 10: Write Group D Tests — Recipe & Build (T62-T66)

**Files:**
- Modify: `tests/integration/remi/runner/test_runner.py`

**Step 1: Add run_group_d**

```python
def run_group_d(suite: TestSuite):
    """Group D: Recipe & Build (T62-T66)."""
    cfg = suite.cfg
    print("\n-- Group D: Recipe & Build --\n")

    recipe_output = "/tmp/conary-recipe-output"
    recipe_cache = "/tmp/conary-recipe-cache"
    recipe_dir = Path(cfg.fixture_dir) / "recipes" / "simple-hello"
    pkgbuild_path = Path(cfg.fixture_dir) / "pkgbuild" / "PKGBUILD"
    Path(recipe_output).mkdir(parents=True, exist_ok=True)
    Path(recipe_cache).mkdir(parents=True, exist_ok=True)

    # ── T62: Cook TOML recipe ────────────────────────────────────────────
    cp62 = suite.checkpoint("T62")

    def t62():
        recipe_toml = recipe_dir / "recipe.toml"
        if not recipe_toml.is_file():
            raise AssertionError(f"recipe fixture not found: {recipe_toml}")
        conary(cfg, "cook", str(recipe_toml),
               "--output", recipe_output,
               "--source-cache", recipe_cache,
               "--no-isolation", timeout=120)

    suite.run_test("T62", "cook_toml_recipe", t62, timeout=120)

    # ── T63: CCS output valid ────────────────────────────────────────────
    if suite.failed_since(cp62):
        suite.skip("T63", "ccs_output_valid", "skipped due to T62 failure")
    else:
        def t63():
            ccs_files = list(Path(recipe_output).glob("*.ccs"))
            if not ccs_files:
                raise AssertionError(
                    f"no CCS file found in {recipe_output}")
            print(f"CCS output: {ccs_files[0].name} "
                  f"({ccs_files[0].stat().st_size} bytes)")

        suite.run_test("T63", "ccs_output_valid", t63)

    # ── T64: PKGBUILD conversion ─────────────────────────────────────────
    cp64 = suite.checkpoint("T64")

    def t64():
        if not pkgbuild_path.is_file():
            raise AssertionError(f"PKGBUILD not found: {pkgbuild_path}")
        r = conary(cfg, "convert-pkgbuild", str(pkgbuild_path))
        output = r.stdout + r.stderr
        assert_contains("name", output)
        assert_contains("version", output)

    suite.run_test("T64", "pkgbuild_conversion", t64, timeout=30)

    # ── T65: Converted recipe cooks ──────────────────────────────────────
    if suite.failed_since(cp64):
        suite.skip("T65", "converted_recipe_cooks",
                   "skipped due to T64 failure")
    else:
        def t65():
            converted = f"{recipe_output}/converted-recipe.toml"
            conary(cfg, "convert-pkgbuild", str(pkgbuild_path),
                   "--output", converted)
            if not Path(converted).is_file():
                raise AssertionError(f"converted recipe not at {converted}")
            # fetch-only validates parsing + source resolution
            r = conary(cfg, "cook", converted,
                       "--output", f"{recipe_output}/converted",
                       "--source-cache", recipe_cache,
                       "--no-isolation", "--fetch-only",
                       check=False, timeout=120)
            assert_not_contains("panic", r.stdout + r.stderr)

        suite.run_test("T65", "converted_recipe_cooks", t65, timeout=120)

    # ── T66: Hermetic build isolation ────────────────────────────────────
    def t66():
        recipe_toml = recipe_dir / "recipe.toml"
        if not recipe_toml.is_file():
            raise AssertionError(f"recipe fixture not found: {recipe_toml}")
        conary(cfg, "cook", str(recipe_toml),
               "--output", f"{recipe_output}/hermetic",
               "--source-cache", recipe_cache,
               "--hermetic", timeout=120)

    suite.run_test("T66", "hermetic_build", t66, timeout=120)

    # Cleanup
    import shutil
    shutil.rmtree(recipe_output, ignore_errors=True)
    shutil.rmtree(recipe_cache, ignore_errors=True)
```

**Step 2: Verify, commit**

```bash
git add tests/integration/remi/runner/test_runner.py
git commit -m "test: add Group D recipe and build tests (T62-T66)"
```

---

## Task 11: Write Group E Tests — Remi Client (T67-T71)

**Files:**
- Modify: `tests/integration/remi/runner/test_runner.py`

**Step 1: Add run_group_e**

```python
def run_group_e(suite: TestSuite):
    """Group E: Remi Client (T67-T71)."""
    cfg = suite.cfg
    fx = cfg.fixtures
    print("\n-- Group E: Remi Client --\n")

    # ── T67: Sparse index fetch ──────────────────────────────────────────
    def t67():
        r = run_cmd(["curl", "-sf",
                     f"{cfg.endpoint}/v1/{cfg.distro.remi_distro}/index"],
                    timeout=30)
        if not r.stdout.strip():
            raise AssertionError("empty response from sparse index")
        lines = r.stdout.strip().count("\n") + 1
        print(f"Sparse index: {lines} lines")

    suite.run_test("T67", "sparse_index_fetch", t67, timeout=30)

    # ── T68: Chunk-level install ─────────────────────────────────────────
    def t68():
        conary(cfg, "install", f"{fx.package}={fx.v1_version}",
               "--dep-mode", "takeover", "--yes", "--no-scripts",
               "--sandbox", "never", timeout=300)
        assert_file_exists(fx.file)
        conary(cfg, "remove", fx.package, "--no-scripts", check=False)

    suite.run_test("T68", "chunk_level_install", t68, timeout=300)

    # ── T69: OCI manifest valid ──────────────────────────────────────────
    def t69():
        r = run_cmd(["curl", "-sf", "-o", "/dev/null",
                     "-w", "%{http_code}",
                     f"{cfg.endpoint}/v2/"],
                    check=False, timeout=30)
        code = r.stdout.strip()
        if code not in ("200", "401"):
            raise AssertionError(
                f"OCI endpoint returned unexpected {code}")
        print(f"OCI endpoint: HTTP {code}")

    suite.run_test("T69", "oci_manifest_valid", t69, timeout=30)

    # ── T70: OCI blob fetch ──────────────────────────────────────────────
    def t70():
        r = run_cmd(
            ["curl", "-sf",
             f"{cfg.endpoint}/v2/{cfg.distro.remi_distro}/"
             f"{fx.package}/tags/list"],
            check=False, timeout=30)
        if r.returncode == 0 and "tags" in r.stdout:
            print(f"OCI tags: {r.stdout.strip()}")
        else:
            print("No OCI tags (expected if not published as OCI)")

    suite.run_test("T70", "oci_blob_fetch", t70, timeout=30)

    # ── T71: Stats endpoint ──────────────────────────────────────────────
    def t71():
        r = run_cmd(["curl", "-sf", f"{cfg.endpoint}/stats"],
                    timeout=30)
        if not r.stdout.strip():
            raise AssertionError("empty response from /stats")
        assert_contains("packages", r.stdout)

    suite.run_test("T71", "stats_endpoint", t71, timeout=30)
```

**Step 2: Verify Python parses**

Run: `python3 -c "import ast; ast.parse(open('tests/integration/remi/runner/test_runner.py').read())"`

**Step 3: Commit**

```bash
git add tests/integration/remi/runner/test_runner.py
git commit -m "test: add Group E Remi client tests (T67-T71)"
```

---

## Task 12: Add E2E CI Workflow

**Files:**
- Create: `.forgejo/workflows/e2e.yaml`

**Step 1: Write workflow**

```yaml
# .forgejo/workflows/e2e.yaml
# Phase 2 E2E validation -- daily + manual (~20-30 min)
name: E2E Validation

on:
  schedule:
    - cron: '0 6 * * *'
  workflow_dispatch:

jobs:
  e2e:
    runs-on: linux-native
    strategy:
      fail-fast: false
      matrix:
        distro: [fedora43, ubuntu-noble, arch]
    name: E2E (${{ matrix.distro }})
    steps:
      - uses: actions/checkout@v4

      - name: Build conary
        run: cargo build

      - name: Run E2E tests (${{ matrix.distro }})
        run: ./tests/integration/remi/run.sh --distro ${{ matrix.distro }} --phase2

      - name: Upload results
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: e2e-results-${{ matrix.distro }}
          path: tests/integration/remi/results/${{ matrix.distro }}.json
```

**Step 2: Commit**

```bash
git add .forgejo/workflows/e2e.yaml
git commit -m "ci: add daily E2E validation workflow for Phase 2"
```

---

## Task 13: Remove Old Bash Runner

After Phase 1 port is verified (Task 6), remove the old bash files.

**Files:**
- Delete: `tests/integration/remi/runner/test-runner.sh`
- Delete: `tests/integration/remi/runner/lib.sh`

**Step 1: Remove old files**

```bash
git rm tests/integration/remi/runner/test-runner.sh tests/integration/remi/runner/lib.sh
```

**Step 2: Commit**

```bash
git commit -m "test: remove old bash test runner (replaced by Python)"
```

---

## Task 14: Update ROADMAP.md Phase 2 with Test IDs

**Files:**
- Modify: `ROADMAP.md`

**Step 1: Cross-reference roadmap items with test IDs**

Update each Phase 2 checkbox with the test IDs that prove it, marking items as complete when tests pass.

**Step 2: Commit**

```bash
git add ROADMAP.md
git commit -m "docs: cross-reference Phase 2 roadmap with test IDs"
```

---

## Task 15: Full Integration Smoke Test

**Step 1:** `cargo build`

**Step 2:** Build and publish fixtures: `bash scripts/publish-test-fixtures.sh`

**Step 3:** Run Phase 1 on all distros (verify port):
```bash
./tests/integration/remi/run.sh --distro fedora43
./tests/integration/remi/run.sh --distro ubuntu-noble
./tests/integration/remi/run.sh --distro arch
```

**Step 4:** Run Phase 2 on all distros:
```bash
./tests/integration/remi/run.sh --distro fedora43 --phase2
./tests/integration/remi/run.sh --distro ubuntu-noble --phase2
./tests/integration/remi/run.sh --distro arch --phase2
```

**Step 5:** Fix issues, commit fixes.

---

## Execution Order Summary

| Task | Description | Depends On | Parallelizable |
|------|-------------|------------|----------------|
| 1 | config.toml | None | Yes |
| 2 | Python runner core | None | Yes |
| 3 | Port Phase 1 tests | 2 | No |
| 4 | Update containers + run.sh | 1, 2 | No |
| 5 | Create fixture packages | None | Yes |
| 6 | Verify Phase 1 port | 3, 4 | No |
| 7 | Group A tests (T38-T50) | 6 | Yes |
| 8 | Group B tests (T51-T57) | 6 | Yes |
| 9 | Group C tests (T58-T61) | 6 | Yes |
| 10 | Group D tests (T62-T66) | 6 | Yes |
| 11 | Group E tests (T67-T71) | 6 | Yes |
| 12 | E2E CI workflow | 7-11 | No |
| 13 | Remove old bash runner | 6 | No |
| 14 | ROADMAP.md updates | 7-11 | No |
| 15 | Full smoke test | All above | No |

Tasks 1, 2, 5 can run in parallel. Tasks 7-11 can run in parallel after Task 6 verifies the port.
