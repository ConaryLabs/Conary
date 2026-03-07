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


def run_cmd(
    args: list[str],
    *,
    timeout: int = 60,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    """Run a command, capturing combined stdout+stderr."""
    result = subprocess.run(
        args,
        capture_output=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        timeout=timeout,
        check=check,
    )
    return result


def conary(
    cfg: Config,
    *args: str,
    timeout: int = 120,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    """Run the conary binary with --db-path prepended."""
    cmd = [cfg.conary_bin, "--db-path", cfg.db_path, *args]
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
    """Phase 1 tests -- core Remi integration."""
    print("[TODO] Phase 1 tests not yet implemented")


def run_phase2(suite: TestSuite) -> None:
    """Phase 2 tests -- fixture-based E2E validation."""
    print("[TODO] Phase 2 tests not yet implemented")


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
