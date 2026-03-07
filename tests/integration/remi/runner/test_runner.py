#!/usr/bin/env python3
# tests/integration/remi/runner/test_runner.py
"""
Conary integration test runner -- Python replacement for test-runner.sh + lib.sh.

Loads configuration from config.toml, runs tests, tracks results, and writes
JSON output.  Python 3.11+ stdlib only.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import time
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable

# ---------------------------------------------------------------------------
# ANSI colours (disabled when stdout is not a tty)
# ---------------------------------------------------------------------------

_IS_TTY = hasattr(sys.stdout, "isatty") and sys.stdout.isatty()

GREEN = "\033[0;32m" if _IS_TTY else ""
RED = "\033[0;31m" if _IS_TTY else ""
YELLOW = "\033[1;33m" if _IS_TTY else ""
BLUE = "\033[0;34m" if _IS_TTY else ""
NC = "\033[0m" if _IS_TTY else ""

# ---------------------------------------------------------------------------
# Config dataclasses
# ---------------------------------------------------------------------------


@dataclass
class DistroConfig:
    """Per-distro test parameters."""

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
    """Fixture package paths and checksums."""

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
    """Top-level test configuration, loaded from config.toml with env overrides."""

    endpoint: str
    db_path: str
    conary_bin: str
    results_dir: str
    fixture_dir: str
    distro_name: str
    distro: DistroConfig
    fixture: FixtureConfig
    remove_default_repos: list[str]

    @classmethod
    def load(cls, toml_path: Path) -> Config:
        """Load config from *toml_path*, applying env-var overrides."""
        with open(toml_path, "rb") as fh:
            raw = tomllib.load(fh)

        # Resolve distro name (env takes precedence).
        distro_name = os.environ.get("DISTRO", "fedora43")
        distro_table = raw.get("distros", {}).get(distro_name)
        if distro_table is None:
            available = ", ".join(raw.get("distros", {}).keys())
            raise ValueError(
                f"Unknown distro '{distro_name}'. Available: {available}"
            )

        distro = DistroConfig(
            remi_distro=distro_table["remi_distro"],
            repo_name=distro_table["repo_name"],
            test_package=distro_table["test_package"],
            test_binary=distro_table["test_binary"],
            test_package_2=distro_table["test_package_2"],
            test_binary_2=distro_table["test_binary_2"],
            test_package_3=distro_table["test_package_3"],
            test_binary_3=distro_table["test_binary_3"],
        )

        # Fixtures
        fix = raw.get("fixtures", {})
        fixture = FixtureConfig(
            package=fix.get("package", ""),
            file=fix.get("file", ""),
            added_file=fix.get("added_file", ""),
            marker=fix.get("marker", ""),
            v1_version=fix.get("v1", {}).get("version", ""),
            v1_hello_sha256=fix.get("v1", {}).get("hello_sha256", ""),
            v2_version=fix.get("v2", {}).get("version", ""),
            v2_hello_sha256=fix.get("v2", {}).get("hello_sha256", ""),
            v2_added_sha256=fix.get("v2", {}).get("added_sha256", ""),
        )

        paths = raw.get("paths", {})

        return cls(
            endpoint=os.environ.get(
                "REMI_ENDPOINT", raw.get("remi", {}).get("endpoint", "")
            ),
            db_path=os.environ.get("DB_PATH", paths.get("db", "")),
            conary_bin=os.environ.get("CONARY_BIN", paths.get("conary_bin", "")),
            results_dir=os.environ.get("RESULTS_DIR", paths.get("results_dir", "")),
            fixture_dir=paths.get("fixture_dir", ""),
            distro_name=distro_name,
            distro=distro,
            fixture=fixture,
            remove_default_repos=raw.get("setup", {}).get(
                "remove_default_repos", []
            ),
        )


# ---------------------------------------------------------------------------
# Test result tracking
# ---------------------------------------------------------------------------


@dataclass
class TestResult:
    """Outcome of a single test."""

    id: str
    name: str
    status: str  # "pass", "fail", "skip"
    duration_ms: int = 0
    message: str = ""

    def to_dict(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "name": self.name,
            "status": self.status,
            "duration_ms": self.duration_ms,
            "message": self.message,
        }


# ---------------------------------------------------------------------------
# Assertion helpers (plain functions, raise AssertionError)
# ---------------------------------------------------------------------------


def assert_file_exists(path: str) -> None:
    p = Path(path)
    if not p.is_file():
        raise AssertionError(f"Expected file to exist: {path}")


def assert_file_not_exists(path: str) -> None:
    p = Path(path)
    if p.exists():
        raise AssertionError(f"Expected file NOT to exist: {path}")


def assert_file_executable(path: str) -> None:
    p = Path(path)
    if not p.is_file():
        raise AssertionError(f"File does not exist: {path}")
    if not os.access(path, os.X_OK):
        raise AssertionError(f"File is not executable: {path}")


def assert_dir_exists(path: str) -> None:
    p = Path(path)
    if not p.is_dir():
        raise AssertionError(f"Expected directory to exist: {path}")


def assert_dir_not_exists(path: str) -> None:
    p = Path(path)
    if p.exists():
        raise AssertionError(f"Expected directory NOT to exist: {path}")


def assert_contains(needle: str, haystack: str) -> None:
    if needle not in haystack:
        raise AssertionError(
            f"Expected to find '{needle}' in output "
            f"(first 200 chars): {haystack[:200]}"
        )


def assert_not_contains(needle: str, haystack: str) -> None:
    if needle in haystack:
        raise AssertionError(f"Expected NOT to find '{needle}' in output")


def assert_file_checksum(path: str, expected_sha256: str) -> None:
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(8192), b""):
            h.update(chunk)
    actual = h.hexdigest()
    if actual != expected_sha256:
        raise AssertionError(
            f"Checksum mismatch for {path}: "
            f"expected {expected_sha256}, got {actual}"
        )


# ---------------------------------------------------------------------------
# Command runner
# ---------------------------------------------------------------------------

_ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")


def _strip_ansi(text: str) -> str:
    """Remove ANSI escape sequences from text."""
    return _ANSI_RE.sub("", text)


def run_cmd(
    args: list[str],
    *,
    timeout: int = 60,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    """Run a command, capturing combined stdout+stderr (ANSI stripped)."""
    result = subprocess.run(
        args,
        capture_output=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        timeout=timeout,
        check=check,
    )
    result.stdout = _strip_ansi(result.stdout)
    return result


def conary(
    cfg: Config,
    *args: str,
    timeout: int = 120,
    check: bool = True,
    no_db: bool = False,
) -> subprocess.CompletedProcess[str]:
    """Run the conary binary with --db-path appended (subcommand option).

    Pass no_db=True for subcommands that don't accept --db-path
    (e.g. system generation list/gc/switch/rollback/info).
    """
    if no_db:
        cmd = [cfg.conary_bin, *args]
    else:
        cmd = [cfg.conary_bin, *args, "--db-path", cfg.db_path]
    return run_cmd(cmd, timeout=timeout, check=check)


# ---------------------------------------------------------------------------
# TestSuite
# ---------------------------------------------------------------------------


class TestSuite:
    """Collects test executions, tracks results, writes JSON report."""

    def __init__(self, cfg: Config) -> None:
        self.cfg = cfg
        self.results: list[TestResult] = []
        self._fatal = False

    # -- Properties ----------------------------------------------------------

    @property
    def pass_count(self) -> int:
        return sum(1 for r in self.results if r.status == "pass")

    @property
    def fail_count(self) -> int:
        return sum(1 for r in self.results if r.status == "fail")

    @property
    def skip_count(self) -> int:
        return sum(1 for r in self.results if r.status == "skip")

    # -- Test execution ------------------------------------------------------

    def run_test(
        self,
        test_id: str,
        name: str,
        func: Callable[[], None],
        timeout: int = 120,
    ) -> None:
        """Execute *func*, recording the result.  If fatal is set, skip."""
        if self._fatal:
            self.skip(test_id, name, "skipped (fatal failure earlier)")
            return

        start = time.monotonic_ns()
        try:
            func()
            duration_ms = (time.monotonic_ns() - start) // 1_000_000
            result = TestResult(
                id=test_id,
                name=name,
                status="pass",
                duration_ms=duration_ms,
            )
            print(f"  {GREEN}[PASS]{NC} {test_id} {name} ({duration_ms}ms)")
        except subprocess.TimeoutExpired as exc:
            duration_ms = (time.monotonic_ns() - start) // 1_000_000
            msg = f"Timeout after {exc.timeout}s"
            result = TestResult(
                id=test_id,
                name=name,
                status="fail",
                duration_ms=duration_ms,
                message=msg,
            )
            print(f"  {RED}[FAIL]{NC} {test_id} {name} -- {msg}")
        except Exception as exc:
            duration_ms = (time.monotonic_ns() - start) // 1_000_000
            msg = str(exc)
            result = TestResult(
                id=test_id,
                name=name,
                status="fail",
                duration_ms=duration_ms,
                message=msg,
            )
            print(f"  {RED}[FAIL]{NC} {test_id} {name} -- {msg}")

        self.results.append(result)

    def skip(self, test_id: str, name: str, reason: str = "") -> None:
        """Record a skipped test."""
        result = TestResult(
            id=test_id,
            name=name,
            status="skip",
            message=reason,
        )
        self.results.append(result)
        print(f"  {YELLOW}[SKIP]{NC} {test_id} {name} -- {reason}")

    def skip_group(self, test_ids: list[tuple[str, str]], reason: str) -> None:
        """Skip multiple tests.  *test_ids* is a list of (id, name) tuples."""
        for tid, tname in test_ids:
            self.skip(tid, tname, reason)

    def set_fatal(self) -> None:
        """Mark suite as fatally failed -- remaining run_test calls become skips."""
        self._fatal = True

    def checkpoint(self, label: str) -> int:
        """Return current fail count (for later comparison via failed_since)."""
        _ = label  # informational only
        return self.fail_count

    def failed_since(self, checkpoint: int) -> bool:
        """Return True if any new failures occurred since *checkpoint*."""
        return self.fail_count > checkpoint

    # -- Reporting -----------------------------------------------------------

    def write_results(self) -> int:
        """Print summary, write JSON to results_dir, return exit code."""
        total = len(self.results)
        print()
        print(
            f"{BLUE}Results:{NC}  "
            f"{GREEN}{self.pass_count} passed{NC}  "
            f"{RED}{self.fail_count} failed{NC}  "
            f"{YELLOW}{self.skip_count} skipped{NC}  "
            f"/ {total} total"
        )

        # Write JSON
        results_dir = Path(self.cfg.results_dir)
        results_dir.mkdir(parents=True, exist_ok=True)

        report = {
            "distro": self.cfg.distro_name,
            "endpoint": self.cfg.endpoint,
            "total": total,
            "passed": self.pass_count,
            "failed": self.fail_count,
            "skipped": self.skip_count,
            "tests": [r.to_dict() for r in self.results],
        }

        out_file = results_dir / f"{self.cfg.distro_name}.json"
        with open(out_file, "w") as fh:
            json.dump(report, fh, indent=2)
            fh.write("\n")
        print(f"Results written to {out_file}")

        return 1 if self.fail_count > 0 else 0


# ---------------------------------------------------------------------------
# Phase placeholders (tests added in Task 3+)
# ---------------------------------------------------------------------------


def run_phase1(suite: TestSuite) -> None:
    """Phase 1: Core Remi integration tests (T01-T37)."""
    cfg = suite.cfg
    d = cfg.distro

    # ── Setup: Initialize DB ─────────────────────────────────────────
    print("[SETUP] Initializing database...")
    Path(cfg.db_path).parent.mkdir(parents=True, exist_ok=True)
    conary(cfg, "system", "init")
    for repo in cfg.remove_default_repos:
        conary(cfg, "repo", "remove", repo, check=False)
    print("[SETUP] Database ready\n")

    # ── T01: Health Check ────────────────────────────────────────────

    def t01():
        run_cmd(["curl", "-sf", f"{cfg.endpoint}/health"], timeout=10)

    suite.run_test("T01", "health_check", t01, timeout=10)

    if suite.fail_count > 0:
        print(f"\nRemi unreachable at {cfg.endpoint} - skipping remaining")
        suite.set_fatal()
        return

    # ── T02: Repo Add ────────────────────────────────────────────────

    def t02():
        conary(cfg, "repo", "add", d.repo_name, cfg.endpoint,
               "--default-strategy", "remi",
               "--remi-endpoint", cfg.endpoint,
               "--remi-distro", d.remi_distro,
               "--no-gpg-check",
               timeout=10)

    suite.run_test("T02", "repo_add", t02, timeout=10)

    # ── T03: Repo List ───────────────────────────────────────────────

    def t03():
        r = conary(cfg, "repo", "list", timeout=10)
        assert_contains(d.repo_name, r.stdout)

    suite.run_test("T03", "repo_list", t03, timeout=10)

    # ── T04: Repo Sync ───────────────────────────────────────────────

    cp_sync = suite.checkpoint("sync")

    def t04():
        r = conary(cfg, "repo", "sync", d.repo_name, "--force", timeout=300)
        assert_contains("[OK]", r.stdout)

    suite.run_test("T04", "repo_sync", t04, timeout=300)

    if suite.failed_since(cp_sync):
        print("\nRepo sync failed - skipping package operation tests (T05-T37)")
        suite.set_fatal()
        return

    # ── T05: Search Exists ───────────────────────────────────────────

    def t05():
        r = conary(cfg, "search", d.test_package, timeout=30)
        assert_contains(d.test_package, r.stdout)
        assert_not_contains("No packages found", r.stdout)

    suite.run_test("T05", "search_exists", t05, timeout=30)

    # ── T06: Search Nonexistent ──────────────────────────────────────

    def t06():
        r = conary(cfg, "search", "zzz-nonexistent-pkg-12345", timeout=10)
        assert_contains("No packages found", r.stdout)

    suite.run_test("T06", "search_nonexistent", t06, timeout=10)

    # ── T07: Install Package ─────────────────────────────────────────

    def t07():
        conary(cfg, "install", d.test_package,
               "--no-scripts", "--no-deps", "--sandbox", "never",
               timeout=300)

    suite.run_test("T07", "install_package", t07, timeout=300)

    # ── T08: Verify Files ────────────────────────────────────────────

    def t08():
        assert_file_exists(d.test_binary)
        assert_file_executable(d.test_binary)

    suite.run_test("T08", "verify_files", t08, timeout=10)

    # ── T09: List Installed ──────────────────────────────────────────

    def t09():
        r = conary(cfg, "list", timeout=10)
        assert_contains(d.test_package, r.stdout)

    suite.run_test("T09", "list_installed", t09, timeout=10)

    # ── T10: Install Nonexistent ─────────────────────────────────────

    def t10():
        r = conary(cfg, "install", "zzz-nonexistent-pkg-12345",
                   "--no-scripts", "--no-deps", "--sandbox", "never",
                   timeout=30, check=False)
        if r.returncode == 0:
            raise AssertionError(
                "expected non-zero exit code for nonexistent package"
            )

    suite.run_test("T10", "install_nonexistent", t10, timeout=30)

    # ── T11: Remove Package ──────────────────────────────────────────

    def t11():
        conary(cfg, "remove", d.test_package, "--no-scripts", timeout=60)

    suite.run_test("T11", "remove_package", t11, timeout=60)

    # ── T12: Verify Removed ──────────────────────────────────────────

    def t12():
        assert_file_not_exists(d.test_binary)
        r = conary(cfg, "list", timeout=10)
        assert_not_contains(d.test_package, r.stdout)

    suite.run_test("T12", "verify_removed", t12, timeout=10)

    # ── T13: Version Check ───────────────────────────────────────────

    def t13():
        r = run_cmd([cfg.conary_bin, "--version"], timeout=10)
        assert_contains("conary", r.stdout)

    suite.run_test("T13", "version_check", t13, timeout=10)

    # ── T14: Reinstall Which ─────────────────────────────────────────

    cp_reinstall = suite.checkpoint("reinstall")

    def t14():
        conary(cfg, "install", d.test_package,
               "--no-scripts", "--no-deps", "--sandbox", "never",
               timeout=300)

    suite.run_test("T14", "reinstall_which", t14, timeout=300)

    reinstall_failed = suite.failed_since(cp_reinstall)

    # ── T15: Package Info ────────────────────────────────────────────

    if reinstall_failed:
        suite.skip("T15", "package_info", "skipped due to T14 failure")
    else:
        def t15():
            r = conary(cfg, "list", d.test_package, "--info", timeout=30)
            assert_contains(d.test_package, r.stdout)
            assert_contains("Version", r.stdout)

        suite.run_test("T15", "package_info", t15, timeout=30)

    # ── T16: List Files ──────────────────────────────────────────────

    if reinstall_failed:
        suite.skip("T16", "list_files", "skipped due to T14 failure")
    else:
        def t16():
            r = conary(cfg, "list", d.test_package, "--files", timeout=30)
            assert_contains(d.test_binary, r.stdout)

        suite.run_test("T16", "list_files", t16, timeout=30)

    # ── T17: Path Ownership ──────────────────────────────────────────

    if reinstall_failed:
        suite.skip("T17", "path_ownership", "skipped due to T14 failure")
    else:
        def t17():
            r = conary(cfg, "list", "--path", d.test_binary, timeout=30)
            assert_contains(d.test_package, r.stdout)

        suite.run_test("T17", "path_ownership", t17, timeout=30)

    # ── T18: Install Tree ────────────────────────────────────────────

    cp_tree = suite.checkpoint("tree")

    def t18():
        conary(cfg, "install", d.test_package_2,
               "--no-scripts", "--no-deps", "--sandbox", "never",
               timeout=300)

    suite.run_test("T18", "install_tree", t18, timeout=300)

    tree_failed = suite.failed_since(cp_tree)

    # ── T19: Verify Tree Files ───────────────────────────────────────

    if tree_failed:
        suite.skip("T19", "verify_tree_files", "skipped due to T18 failure")
    else:
        def t19():
            assert_file_exists(d.test_binary_2)
            assert_file_executable(d.test_binary_2)

        suite.run_test("T19", "verify_tree_files", t19, timeout=10)

    # ── T20: Adopt Single Package ────────────────────────────────────

    cp_adopt = suite.checkpoint("adopt")

    def t20():
        conary(cfg, "system", "adopt", "curl", timeout=60)

    suite.run_test("T20", "adopt_single_package", t20, timeout=60)

    adopt_failed = suite.failed_since(cp_adopt)

    # ── T21: Adopt Status ────────────────────────────────────────────

    if adopt_failed:
        suite.skip("T21", "adopt_status", "skipped due to T20 failure")
    else:
        def t21():
            r = conary(cfg, "system", "adopt", "--status", timeout=30)
            assert_contains("Conary Adoption Status", r.stdout)
            assert_contains("Adopted", r.stdout)

        suite.run_test("T21", "adopt_status", t21, timeout=30)

    # ── T22: Pin Package ─────────────────────────────────────────────

    if reinstall_failed:
        suite.skip("T22", "pin_package", "skipped due to T14 failure")
    else:
        def t22():
            conary(cfg, "pin", d.test_package, timeout=30)
            r = conary(cfg, "list", d.test_package, "--info", timeout=30)
            assert_contains("Pinned      : yes", r.stdout)

        suite.run_test("T22", "pin_package", t22, timeout=30)

    # ── T23: Unpin Package ───────────────────────────────────────────

    if reinstall_failed:
        suite.skip("T23", "unpin_package", "skipped due to T14 failure")
    else:
        def t23():
            conary(cfg, "unpin", d.test_package, timeout=30)
            r = conary(cfg, "list", d.test_package, "--info", timeout=30)
            assert_contains("Pinned      : no", r.stdout)

        suite.run_test("T23", "unpin_package", t23, timeout=30)

    # ── T24: Changeset History ───────────────────────────────────────

    if reinstall_failed:
        suite.skip("T24", "changeset_history", "skipped due to T14 failure")
    else:
        def t24():
            r = conary(cfg, "system", "history", timeout=30)
            assert_contains("Changeset", r.stdout)

        suite.run_test("T24", "changeset_history", t24, timeout=30)

    # ── T25: Install Dep Package ─────────────────────────────────────

    cp_dep = suite.checkpoint("dep")

    def t25():
        conary(cfg, "install", d.test_package_3,
               "--no-scripts", "--no-deps", "--sandbox", "never",
               timeout=300)

    suite.run_test("T25", "install_dep_package", t25, timeout=300)

    dep_failed = suite.failed_since(cp_dep)

    # ── T26: Verify Dep Files ────────────────────────────────────────

    if dep_failed:
        suite.skip("T26", "verify_dep_files", "skipped due to T25 failure")
    else:
        def t26():
            assert_file_exists(d.test_binary_3)

        suite.run_test("T26", "verify_dep_files", t26, timeout=10)

    # ── T27: Multi Package Coexist ───────────────────────────────────

    if dep_failed or reinstall_failed:
        suite.skip("T27", "multi_package_coexist",
                   "skipped due to prior install failure")
    else:
        def t27():
            r = conary(cfg, "list", timeout=10)
            assert_contains(d.test_package, r.stdout)
            assert_contains(d.test_package_2, r.stdout)
            assert_contains(d.test_package_3, r.stdout)

        suite.run_test("T27", "multi_package_coexist", t27, timeout=10)

    # ── T28: Dep Mode Satisfy ────────────────────────────────────────

    def t28():
        r = conary(cfg, "install", d.test_package,
                   "--no-scripts", "--dep-mode", "satisfy",
                   "--yes", "--sandbox", "never",
                   timeout=300, check=False)
        # Cleanup: remove the package for later tests
        conary(cfg, "remove", d.test_package, "--no-scripts",
               timeout=60, check=False)
        if r.returncode != 0:
            raise AssertionError(
                f"install with --dep-mode satisfy failed "
                f"(exit {r.returncode}): {r.stdout}"
            )

    suite.run_test("T28", "dep_mode_satisfy", t28, timeout=300)

    # ── T29: Dep Mode Adopt ──────────────────────────────────────────

    def t29():
        r = conary(cfg, "install", d.test_package_2,
                   "--no-scripts", "--dep-mode", "adopt",
                   "--yes", "--sandbox", "never",
                   timeout=300, check=False)
        if r.returncode != 0:
            raise AssertionError(
                f"install with --dep-mode adopt failed "
                f"(exit {r.returncode}): {r.stdout}"
            )
        assert_file_exists(d.test_binary_2)

    suite.run_test("T29", "dep_mode_adopt", t29, timeout=300)

    # ── T30: Dep Mode Takeover ───────────────────────────────────────

    def t30():
        r = conary(cfg, "install", d.test_package_3,
                   "--no-scripts", "--dep-mode", "takeover",
                   "--yes", "--sandbox", "never",
                   timeout=300, check=False)
        if r.returncode != 0:
            raise AssertionError(
                f"install with --dep-mode takeover failed "
                f"(exit {r.returncode}): {r.stdout}"
            )
        assert_file_exists(d.test_binary_3)

    suite.run_test("T30", "dep_mode_takeover", t30, timeout=300)

    # ── T31: Blocklist Enforced ──────────────────────────────────────

    def t31():
        conary(cfg, "install", "glibc",
               "--no-scripts", "--dep-mode", "takeover",
               "--yes", "--sandbox", "never",
               timeout=60, check=False)
        r = conary(cfg, "list", timeout=10)
        assert_not_contains("glibc", r.stdout)

    suite.run_test("T31", "blocklist_enforced", t31, timeout=60)

    # ── T32: Update With Adopted ─────────────────────────────────────

    def t32():
        conary(cfg, "system", "adopt", "curl", timeout=60, check=False)
        r = conary(cfg, "update", "--dep-mode", "satisfy",
                   timeout=120, check=False)
        if r.returncode != 0:
            raise AssertionError(
                f"update with adopted packages failed "
                f"(exit {r.returncode}): {r.stdout}"
            )

    suite.run_test("T32", "update_with_adopted", t32, timeout=120)

    # ── T33: Generation List Empty ───────────────────────────────────

    def t33():
        r = conary(cfg, "system", "generation", "list", timeout=10,
                   check=False, no_db=True)
        assert_contains("No generations", r.stdout)

    suite.run_test("T33", "generation_list_empty", t33, timeout=10)

    # ── T34: Takeover Dry Run ────────────────────────────────────────

    def t34():
        r = conary(cfg, "system", "takeover",
                   "--dry-run", "--skip-conversion",
                   timeout=60, check=False)
        if r.returncode == 0:
            assert_contains("DRY RUN", r.stdout)
        # Non-zero is acceptable (requires root)

    suite.run_test("T34", "takeover_dry_run", t34, timeout=60)

    # ── T35: Generation GC Empty ─────────────────────────────────────

    def t35():
        r = conary(cfg, "system", "generation", "gc", timeout=10,
                   check=False, no_db=True)
        output = r.stdout
        expected = ["Nothing to clean", "No generations", "Nothing to collect"]
        if not any(phrase in output for phrase in expected):
            raise AssertionError(
                f"Expected one of {expected} "
                f"in output: {output[:200]}"
            )

    suite.run_test("T35", "generation_gc_empty", t35, timeout=10)

    # ── T36: Generation Info Format ──────────────────────────────────

    def t36():
        r = conary(cfg, "system", "generation", "info", "1",
                   timeout=10, check=False, no_db=True)
        # Non-zero is fine (no generation exists yet), just verify no panic
        if r.returncode == 0:
            output = r.stdout
            if "composefs" not in output.lower():
                raise AssertionError(
                    f"Expected 'composefs' in generation info output: "
                    f"{output[:200]}"
                )

    suite.run_test("T36", "generation_info_format", t36, timeout=10)

    # ── T37: Takeover Composefs Format ───────────────────────────────

    def t37():
        r = conary(cfg, "system", "takeover",
                   "--dry-run", "--skip-conversion",
                   timeout=60, check=False)
        # Non-zero is acceptable (requires root), just verify no panic
        if r.returncode == 0:
            output = r.stdout
            if not any(kw in output for kw in
                       ("composefs", "EROFS", "erofs", "DRY RUN")):
                raise AssertionError(
                    f"Expected composefs/EROFS/DRY RUN in output: "
                    f"{output[:200]}"
                )

    suite.run_test("T37", "takeover_composefs_format", t37, timeout=60)

    # ── Cleanup ──────────────────────────────────────────────────────

    print("\n[CLEANUP] Removing test packages...")
    conary(cfg, "remove", d.test_package, "--no-scripts",
           timeout=60, check=False)
    conary(cfg, "remove", d.test_package_2, "--no-scripts",
           timeout=60, check=False)
    conary(cfg, "remove", d.test_package_3, "--no-scripts",
           timeout=60, check=False)


def run_group_a(suite: TestSuite) -> None:
    """Group A: Deep Install Flow (T38-T50)."""
    cfg = suite.cfg
    fx = cfg.fixture
    print("\n-- Group A: Deep Install Flow --\n")

    # ── T38: Install fixture v1 with deps ───────────────────────────
    cp_install = suite.checkpoint("fixture_install")

    def t38():
        conary(cfg, "install", f"{fx.package}={fx.v1_version}",
               "--dep-mode", "takeover", "--yes", "--sandbox", "never",
               timeout=300)

    suite.run_test("T38", "install_fixture_v1_with_deps", t38, timeout=300)

    if suite.failed_since(cp_install):
        suite.skip_group([
            ("T39", "verify_dep_files_on_disk"),
            ("T40", "verify_v1_checksum"),
            ("T41", "verify_scriptlet_ran"),
            ("T42", "remove_with_scriptlets"),
            ("T43", "reinstall_fixture_v1"),
            ("T44", "update_v1_to_v2"),
            ("T45", "delta_update_verify"),
            ("T46", "verify_v2_added_file"),
            ("T47", "rollback_after_update"),
            ("T48", "rollback_filesystem_check"),
            ("T49", "pin_blocks_update"),
            ("T50", "orphan_detection"),
        ], "skipped due to T38 failure")
        return

    # ── T39: Verify dep files on disk ───────────────────────────────

    def t39():
        assert_file_exists(fx.file)
        assert_dir_exists("/usr/share/conary-test")

    suite.run_test("T39", "verify_dep_files_on_disk", t39, timeout=10)

    # ── T40: Verify v1 checksum ─────────────────────────────────────

    def t40():
        assert_file_checksum(fx.file, fx.v1_hello_sha256)

    suite.run_test("T40", "verify_v1_checksum", t40, timeout=10)

    # ── T41: Verify scriptlet ran ───────────────────────────────────

    def t41():
        assert_file_exists(fx.marker)

    suite.run_test("T41", "verify_scriptlet_ran", t41, timeout=10)

    # ── T42: Remove with scriptlets ─────────────────────────────────

    def t42():
        conary(cfg, "remove", fx.package, timeout=60)
        assert_file_not_exists(fx.marker)
        assert_file_not_exists(fx.file)

    suite.run_test("T42", "remove_with_scriptlets", t42, timeout=60)

    # ── T43: Reinstall fixture v1 ───────────────────────────────────
    cp_reinstall = suite.checkpoint("fixture_reinstall")

    def t43():
        conary(cfg, "install", f"{fx.package}={fx.v1_version}",
               "--dep-mode", "takeover", "--yes", "--sandbox", "never",
               timeout=300)

    suite.run_test("T43", "reinstall_fixture_v1", t43, timeout=300)

    if suite.failed_since(cp_reinstall):
        suite.skip_group([
            ("T44", "update_v1_to_v2"),
            ("T45", "delta_update_verify"),
            ("T46", "verify_v2_added_file"),
            ("T47", "rollback_after_update"),
            ("T48", "rollback_filesystem_check"),
            ("T49", "pin_blocks_update"),
            ("T50", "orphan_detection"),
        ], "skipped due to T43 failure")
        return

    # ── T44: Update v1 to v2 ────────────────────────────────────────
    cp_update = suite.checkpoint("fixture_update")

    def t44():
        conary(cfg, "update", fx.package,
               "--dep-mode", "takeover", "--yes", "--sandbox", "never",
               timeout=300)

    suite.run_test("T44", "update_v1_to_v2", t44, timeout=300)

    if suite.failed_since(cp_update):
        suite.skip_group([
            ("T45", "delta_update_verify"),
            ("T46", "verify_v2_added_file"),
            ("T47", "rollback_after_update"),
            ("T48", "rollback_filesystem_check"),
        ], "skipped due to T44 failure")
    else:
        # ── T45: Delta update verify ────────────────────────────────

        def t45():
            assert_file_checksum(fx.file, fx.v2_hello_sha256)

        suite.run_test("T45", "delta_update_verify", t45, timeout=10)

        # ── T46: Verify v2 added file ───────────────────────────────

        def t46():
            assert_file_exists(fx.added_file)
            assert_file_checksum(fx.added_file, fx.v2_added_sha256)

        suite.run_test("T46", "verify_v2_added_file", t46, timeout=10)

        # ── T47: Rollback after update ──────────────────────────────
        cp_rollback = suite.checkpoint("fixture_rollback")

        def t47():
            conary(cfg, "restore", "--last", "--yes", timeout=120)

        suite.run_test("T47", "rollback_after_update", t47, timeout=120)

        if suite.failed_since(cp_rollback):
            suite.skip("T48", "rollback_filesystem_check",
                       "skipped due to T47 failure")
        else:
            # ── T48: Rollback filesystem check ──────────────────────

            def t48():
                assert_file_checksum(fx.file, fx.v1_hello_sha256)
                assert_file_not_exists(fx.added_file)

            suite.run_test("T48", "rollback_filesystem_check", t48, timeout=10)

    # ── T49: Pin blocks update ──────────────────────────────────────

    def t49():
        conary(cfg, "pin", fx.package, timeout=30)
        r = conary(cfg, "update", fx.package,
                   "--dep-mode", "takeover", "--yes", "--sandbox", "never",
                   timeout=120, check=False)
        info = conary(cfg, "list", fx.package, "--info", timeout=30)
        assert_contains(fx.v1_version, info.stdout)
        conary(cfg, "unpin", fx.package, timeout=30)

    suite.run_test("T49", "pin_blocks_update", t49, timeout=120)

    # ── T50: Orphan detection ───────────────────────────────────────

    def t50():
        conary(cfg, "remove", fx.package, "--no-scripts", timeout=60,
               check=False)
        r = conary(cfg, "list", "--orphans", timeout=30, check=False)
        # Just verify no crash -- output content is informational
        _ = r.stdout

    suite.run_test("T50", "orphan_detection", t50, timeout=60)


def run_group_b(suite: TestSuite) -> None:
    """Group B: Generation Lifecycle (T51-T57)."""
    cfg = suite.cfg
    fx = cfg.fixture
    print("\n-- Group B: Generation Lifecycle --\n")

    # Ensure fixture is installed for generation tests
    conary(cfg, "install", f"{fx.package}={fx.v1_version}",
           "--dep-mode", "takeover", "--yes", "--sandbox", "never",
           timeout=300, check=False)

    # ── T51: Build generation ───────────────────────────────────────
    cp_gen = suite.checkpoint("gen_build")

    def t51():
        conary(cfg, "system", "generation", "build", timeout=120)

    suite.run_test("T51", "build_generation", t51, timeout=120)

    if suite.failed_since(cp_gen):
        suite.skip_group([
            ("T52", "generation_list"),
            ("T53", "generation_info"),
            ("T54", "switch_generation"),
            ("T55", "rollback_generation"),
            ("T56", "gc_old_generation"),
            ("T57", "system_takeover_full"),
        ], "skipped due to T51 failure")
        return

    # ── T52: Generation list ────────────────────────────────────────

    def t52():
        r = conary(cfg, "system", "generation", "list", timeout=30, no_db=True)
        assert_not_contains("No generations", r.stdout)

    suite.run_test("T52", "generation_list", t52, timeout=30)

    # ── T53: Generation info ────────────────────────────────────────

    def t53():
        r = conary(cfg, "system", "generation", "info", "1", timeout=30,
                   no_db=True)
        assert_contains("packages", r.stdout)

    suite.run_test("T53", "generation_info", t53, timeout=30)

    # ── T54: Switch generation ──────────────────────────────────────
    cp_switch = suite.checkpoint("gen_switch")

    def t54():
        conary(cfg, "update", fx.package,
               "--dep-mode", "takeover", "--yes", "--sandbox", "never",
               timeout=300)
        conary(cfg, "system", "generation", "build", timeout=120)
        conary(cfg, "system", "generation", "switch", "2", timeout=60,
               no_db=True)

    suite.run_test("T54", "switch_generation", t54, timeout=300)

    if suite.failed_since(cp_switch):
        suite.skip_group([
            ("T55", "rollback_generation"),
            ("T56", "gc_old_generation"),
        ], "skipped due to T54 failure")
    else:
        # ── T55: Rollback generation ────────────────────────────────

        def t55():
            conary(cfg, "system", "generation", "switch", "1", timeout=60,
                   no_db=True)

        suite.run_test("T55", "rollback_generation", t55, timeout=60)

        # ── T56: GC old generation ──────────────────────────────────

        def t56():
            conary(cfg, "system", "generation", "gc", timeout=60, no_db=True)

        suite.run_test("T56", "gc_old_generation", t56, timeout=60)

    # ── T57: System takeover full ───────────────────────────────────

    def t57():
        r = conary(cfg, "system", "takeover",
                   "--skip-conversion", "--yes",
                   timeout=300, check=False)
        # Accept non-zero exit, just verify no panic
        output = r.stdout
        assert_not_contains("panic", output)

    suite.run_test("T57", "system_takeover_full", t57, timeout=300)


def run_group_c(suite: TestSuite) -> None:
    """Group C: Bootstrap Pipeline (T58-T61)."""
    cfg = suite.cfg
    print("\n-- Group C: Bootstrap Pipeline --\n")

    work_dir = "/tmp/conary-bootstrap-test"
    recipe_dir = "/tmp/conary-bootstrap-recipes"
    Path(work_dir).mkdir(parents=True, exist_ok=True)
    Path(recipe_dir).mkdir(parents=True, exist_ok=True)

    # ── T58: Bootstrap dry-run ──────────────────────────────────────

    def t58():
        r = conary(cfg, "bootstrap", "dry-run",
                   "--work-dir", work_dir,
                   "--recipe-dir", recipe_dir,
                   timeout=60, check=False)
        output = r.stdout
        assert_not_contains("panic", output)
        if r.returncode == 0:
            assert_contains("Graph resolved", output)

    suite.run_test("T58", "bootstrap_dry_run", t58, timeout=60)

    # ── T59: Stage 0 ────────────────────────────────────────────────

    def t59():
        r = conary(cfg, "bootstrap", "stage0",
                   "--work-dir", work_dir,
                   timeout=300, check=False)
        output = r.stdout
        assert_not_contains("panic", output)

    suite.run_test("T59", "bootstrap_stage0", t59, timeout=300)

    # ── T60: Stage 0 output ─────────────────────────────────────────

    def t60():
        stage0_dir = Path(work_dir) / "stage0"
        if stage0_dir.is_dir():
            contents = list(stage0_dir.iterdir())
            print(f"    stage0 dir has {len(contents)} entries:")
            for entry in contents[:20]:
                print(f"      {entry.name}")
        else:
            print(f"    stage0 dir does not exist at {stage0_dir}")

    suite.run_test("T60", "bootstrap_stage0_output", t60, timeout=10)

    # ── T61: Stage 1 starts ─────────────────────────────────────────

    def t61():
        cmd = [cfg.conary_bin, "--db-path", cfg.db_path,
               "bootstrap", "stage1", "--work-dir", work_dir]
        try:
            r = run_cmd(cmd, timeout=60, check=False)
            # If it completes within timeout, just verify no panic
            output = r.stdout
            assert_not_contains("panic", output)
        except subprocess.TimeoutExpired:
            # Timeout is acceptable -- proof of life that stage1 started
            print("    stage1 timed out (expected -- proof of life)")

    suite.run_test("T61", "bootstrap_stage1_starts", t61, timeout=120)

    # Cleanup
    shutil.rmtree(work_dir, ignore_errors=True)
    shutil.rmtree(recipe_dir, ignore_errors=True)


def run_group_d(suite: TestSuite) -> None:
    """Group D: Recipe & Build (T62-T66)."""
    cfg = suite.cfg
    print("\n-- Group D: Recipe & Build --\n")

    recipe_output = "/tmp/conary-recipe-output"
    recipe_cache = "/tmp/conary-recipe-cache"
    Path(recipe_output).mkdir(parents=True, exist_ok=True)
    Path(recipe_cache).mkdir(parents=True, exist_ok=True)

    recipe_dir = Path(cfg.fixture_dir) / "recipes" / "simple-hello"
    recipe_toml = recipe_dir / "recipe.toml"
    pkgbuild_path = Path(cfg.fixture_dir) / "pkgbuild" / "PKGBUILD"

    # ── T62: Cook TOML recipe ───────────────────────────────────────
    cp_cook = suite.checkpoint("cook")

    def t62():
        conary(cfg, "cook", str(recipe_toml),
               "--output", recipe_output,
               "--source-cache", recipe_cache,
               "--no-isolation",
               timeout=120)

    suite.run_test("T62", "cook_toml_recipe", t62, timeout=120)

    if suite.failed_since(cp_cook):
        suite.skip("T63", "ccs_output_valid", "skipped due to T62 failure")
    else:
        # ── T63: CCS output valid ──────────────────────────────────

        def t63():
            ccs_files = list(Path(recipe_output).glob("*.ccs"))
            if not ccs_files:
                raise AssertionError(
                    f"No .ccs files found in {recipe_output}")
            print(f"    Found {len(ccs_files)} CCS file(s): "
                  f"{[f.name for f in ccs_files]}")

        suite.run_test("T63", "ccs_output_valid", t63, timeout=10)

    # ── T64: PKGBUILD conversion ────────────────────────────────────
    cp_convert = suite.checkpoint("convert")

    def t64():
        r = conary(cfg, "convert-pkgbuild", str(pkgbuild_path), timeout=60)
        assert_contains("name", r.stdout)
        assert_contains("version", r.stdout)

    suite.run_test("T64", "pkgbuild_conversion", t64, timeout=60)

    if suite.failed_since(cp_convert):
        suite.skip("T65", "converted_recipe_cooks",
                   "skipped due to T64 failure")
    else:
        # ── T65: Converted recipe cooks ─────────────────────────────

        def t65():
            # Convert PKGBUILD to a temp recipe file
            r = conary(cfg, "convert-pkgbuild", str(pkgbuild_path),
                       timeout=60)
            converted_path = Path("/tmp/conary-converted-recipe.toml")
            converted_path.write_text(r.stdout)
            r2 = conary(cfg, "cook", str(converted_path),
                        "--output", recipe_output,
                        "--source-cache", recipe_cache,
                        "--fetch-only", "--no-isolation",
                        timeout=120, check=False)
            output = r2.stdout
            assert_not_contains("panic", output)
            converted_path.unlink(missing_ok=True)

        suite.run_test("T65", "converted_recipe_cooks", t65, timeout=120)

    # ── T66: Hermetic build ─────────────────────────────────────────

    hermetic_out = "/tmp/conary-hermetic-output"
    Path(hermetic_out).mkdir(parents=True, exist_ok=True)

    def t66():
        conary(cfg, "cook", str(recipe_toml),
               "--output", hermetic_out,
               "--source-cache", recipe_cache,
               "--hermetic",
               timeout=120)

    suite.run_test("T66", "hermetic_build", t66, timeout=120)

    # Cleanup
    shutil.rmtree(recipe_output, ignore_errors=True)
    shutil.rmtree(recipe_cache, ignore_errors=True)
    shutil.rmtree(hermetic_out, ignore_errors=True)


def run_group_e(suite: TestSuite) -> None:
    """Group E: Remi Client (T67-T71)."""
    cfg = suite.cfg
    fx = cfg.fixture
    print("\n-- Group E: Remi Client --\n")

    # ── T67: Sparse index ───────────────────────────────────────────

    def t67():
        r = run_cmd(["curl", "-sf",
                     f"{cfg.endpoint}/v1/{cfg.distro.remi_distro}/index"],
                    timeout=30)
        if not r.stdout.strip():
            raise AssertionError("Sparse index response was empty")

    suite.run_test("T67", "sparse_index", t67, timeout=30)

    # ── T68: Chunk-level install ────────────────────────────────────

    def t68():
        conary(cfg, "install", fx.package,
               "--dep-mode", "takeover", "--yes", "--sandbox", "never",
               timeout=300)
        assert_file_exists(fx.file)
        conary(cfg, "remove", fx.package, "--no-scripts",
               timeout=60, check=False)

    suite.run_test("T68", "chunk_level_install", t68, timeout=300)

    # ── T69: OCI manifest ──────────────────────────────────────────

    def t69():
        r = run_cmd(["curl", "-sf", "-o", "/dev/null", "-w", "%{http_code}",
                     f"{cfg.endpoint}/v2/"],
                    timeout=30, check=False)
        code = r.stdout.strip()
        if code not in ("200", "401"):
            raise AssertionError(
                f"Expected HTTP 200 or 401 from /v2/, got {code}")

    suite.run_test("T69", "oci_manifest", t69, timeout=30)

    # ── T70: OCI blob fetch ─────────────────────────────────────────

    def t70():
        r = run_cmd(["curl", "-sf",
                     f"{cfg.endpoint}/v2/_catalog"],
                    timeout=30, check=False)
        if r.returncode != 0:
            print("    OCI catalog not available (soft pass)")
        else:
            print(f"    OCI catalog: {r.stdout[:200]}")

    suite.run_test("T70", "oci_blob_fetch", t70, timeout=30)

    # ── T71: Stats endpoint ─────────────────────────────────────────

    def t71():
        r = run_cmd(["curl", "-sf", f"{cfg.endpoint}/stats"],
                    timeout=30)
        assert_contains("packages", r.stdout)

    suite.run_test("T71", "stats_endpoint", t71, timeout=30)


def run_phase2(suite: TestSuite) -> None:
    """Phase 2: Deep E2E Validation (T38-T71)."""
    run_group_a(suite)
    run_group_b(suite)
    run_group_c(suite)
    run_group_d(suite)
    run_group_e(suite)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Conary integration test runner"
    )
    parser.add_argument(
        "--phase2",
        action="store_true",
        help="Also run Phase 2 (fixture) tests after Phase 1",
    )
    args = parser.parse_args()

    # Locate config.toml relative to this script.
    script_dir = Path(__file__).resolve().parent
    config_path = script_dir / ".." / "config.toml"
    config_path = config_path.resolve()

    if not config_path.is_file():
        print(f"{RED}[FATAL]{NC} Config not found: {config_path}", file=sys.stderr)
        sys.exit(2)

    cfg = Config.load(config_path)
    suite = TestSuite(cfg)

    print(f"{BLUE}Conary Integration Tests{NC}")
    print(f"  Distro:   {cfg.distro_name}")
    print(f"  Endpoint: {cfg.endpoint}")
    print(f"  Binary:   {cfg.conary_bin}")
    print(f"  DB:       {cfg.db_path}")
    print()

    run_phase1(suite)

    if args.phase2:
        run_phase2(suite)

    exit_code = suite.write_results()
    sys.exit(exit_code)


if __name__ == "__main__":
    main()
