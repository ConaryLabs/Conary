#!/usr/bin/env python3
# apps/conary/tests/fixtures/native/record-command-performance.py
"""Record one exact package-manager command as typed performance evidence."""

from __future__ import annotations

import argparse
import datetime
import errno
import hashlib
import json
import os
import platform
import re
import resource
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path


COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run one exact command and write typed performance evidence."
    )
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--implementation", required=True)
    parser.add_argument("--operation", required=True)
    parser.add_argument("--cache-state", required=True, choices=("cold", "warm"))
    parser.add_argument("--product-source-commit", required=True)
    parser.add_argument("--harness-source-commit", required=True)
    parser.add_argument("--implementation-version", required=True)
    parser.add_argument("--fixture-sha256", required=True)
    parser.add_argument("--environment-sha256", required=True)
    parser.add_argument("--sample", required=True, type=int)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    for option, value in (
        ("--product-source-commit", args.product_source_commit),
        ("--harness-source-commit", args.harness_source_commit),
    ):
        if not COMMIT_RE.fullmatch(value):
            parser.error(f"{option} must be a full lowercase Git commit")
    for option, value in (
        ("--fixture-sha256", args.fixture_sha256),
        ("--environment-sha256", args.environment_sha256),
    ):
        if not SHA256_RE.fullmatch(value):
            parser.error(f"{option} must be a lowercase SHA-256 digest")
    if args.sample < 1:
        parser.error("--sample must be positive")
    if not args.command:
        parser.error("an exact command is required after --")
    if args.command[0] == "--":
        args.command = args.command[1:]
    if not args.command:
        parser.error("an exact command is required after --")
    return args


def read_os_release() -> dict[str, str]:
    values: dict[str, str] = {}
    try:
        for raw_line in Path("/etc/os-release").read_text(encoding="utf-8").splitlines():
            line = raw_line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            key, value = line.split("=", 1)
            values[key] = value.strip().strip('"')
    except OSError:
        pass
    return values


def cpu_model() -> str | None:
    try:
        for line in Path("/proc/cpuinfo").read_text(encoding="utf-8").splitlines():
            if line.startswith("model name") and ":" in line:
                return line.split(":", 1)[1].strip()
    except OSError:
        pass
    return None


def memory_total_kib() -> int | None:
    try:
        for line in Path("/proc/meminfo").read_text(encoding="utf-8").splitlines():
            if line.startswith("MemTotal:"):
                return int(line.split()[1])
    except (OSError, ValueError, IndexError):
        pass
    return None


def read_first_line(path: str) -> str | None:
    try:
        return Path(path).read_text(encoding="utf-8").splitlines()[0]
    except (OSError, IndexError):
        return None


def available_logical_cpus() -> int | None:
    try:
        return len(os.sched_getaffinity(0))
    except AttributeError:
        return None


def additive_rusage(after: resource.struct_rusage, before: resource.struct_rusage) -> dict[str, int]:
    return {
        "user_cpu_ns": round((after.ru_utime - before.ru_utime) * 1_000_000_000),
        "system_cpu_ns": round((after.ru_stime - before.ru_stime) * 1_000_000_000),
        "minor_page_faults": after.ru_minflt - before.ru_minflt,
        "major_page_faults": after.ru_majflt - before.ru_majflt,
        "block_input_operations": after.ru_inblock - before.ru_inblock,
        "block_output_operations": after.ru_oublock - before.ru_oublock,
        "voluntary_context_switches": after.ru_nvcsw - before.ru_nvcsw,
        "involuntary_context_switches": after.ru_nivcsw - before.ru_nivcsw,
    }


def resolve_executable(command: str) -> Path:
    candidate = command if "/" in command else shutil.which(command)
    if candidate is None:
        raise FileNotFoundError(errno.ENOENT, os.strerror(errno.ENOENT), command)
    resolved = Path(candidate).resolve(strict=True)
    if not resolved.is_file():
        raise OSError(errno.EINVAL, "command executable is not a regular file", resolved)
    return resolved


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_new_json(path: Path, value: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()
    temporary_path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, prefix=f".{path.name}.", delete=False) as handle:
            temporary_path = Path(handle.name)
            handle.write(encoded)
            handle.flush()
            os.fsync(handle.fileno())
        os.link(temporary_path, path)
        directory_fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        if temporary_path is not None:
            temporary_path.unlink(missing_ok=True)


def main() -> int:
    args = parse_args()
    if os.path.lexists(args.output):
        raise FileExistsError(
            errno.EEXIST,
            os.strerror(errno.EEXIST),
            args.output,
        )
    executable = resolve_executable(args.command[0])
    executable_sha256 = sha256_file(executable)
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    started_ns = time.monotonic_ns()
    completed = subprocess.run(args.command, check=False)
    elapsed_ns = time.monotonic_ns() - started_ns
    after = resource.getrusage(resource.RUSAGE_CHILDREN)

    signal = -completed.returncode if completed.returncode < 0 else None
    exit_code = completed.returncode if completed.returncode >= 0 else None
    os_release = read_os_release()
    process = additive_rusage(after, before)
    process["wall_ns"] = elapsed_ns
    process["max_rss_kib"] = after.ru_maxrss
    record: dict[str, object] = {
        "schema_version": 1,
        "recorded_at": datetime.datetime.now(datetime.UTC).isoformat(),
        "identity": {
            "product_source_commit": args.product_source_commit,
            "harness_source_commit": args.harness_source_commit,
            "fixture_sha256": args.fixture_sha256,
            "environment_sha256": args.environment_sha256,
            "implementation": args.implementation,
            "implementation_version": args.implementation_version,
            "operation": args.operation,
            "cache_state": args.cache_state,
            "sample": args.sample,
        },
        "host": {
            "kernel_release": platform.release(),
            "machine": platform.machine(),
            "os_id": os_release.get("ID"),
            "os_version_id": os_release.get("VERSION_ID"),
            "logical_cpus": os.cpu_count(),
            "available_logical_cpus": available_logical_cpus(),
            "cpu_model": cpu_model(),
            "memory_total_kib": memory_total_kib(),
            "cgroup_v2": {
                "cpu_max": read_first_line("/sys/fs/cgroup/cpu.max"),
                "cpuset_cpus_effective": read_first_line(
                    "/sys/fs/cgroup/cpuset.cpus.effective"
                ),
                "memory_max": read_first_line("/sys/fs/cgroup/memory.max"),
            },
        },
        "command": {
            "argv": args.command,
            "executable_path": str(executable),
            "executable_sha256": executable_sha256,
        },
        "outcome": {"exit_code": exit_code, "signal": signal},
        "process": process,
    }
    write_new_json(args.output, record)
    if signal is not None:
        return 128 + signal
    return completed.returncode


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except FileExistsError as error:
        print(f"performance evidence already exists: {error.filename}", file=sys.stderr)
        raise SystemExit(73) from error
    except OSError as error:
        print(f"failed to record performance evidence: {error}", file=sys.stderr)
        raise SystemExit(74) from error
