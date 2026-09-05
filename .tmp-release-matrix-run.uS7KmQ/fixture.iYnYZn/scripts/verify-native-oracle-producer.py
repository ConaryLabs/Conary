#!/usr/bin/env python3
# scripts/verify-native-oracle-producer.py

"""Verify the shared deployed-to-producer-to-main source predicate."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import subprocess
import sys


def require_commit(value: str, label: str) -> str:
    if re.fullmatch(r"[0-9a-f]{40}", value) is None:
        raise ValueError(f"{label} must be one full lowercase 40-hex SHA")
    return value


def git(repository: Path, arguments: list[str], label: str) -> None:
    result = subprocess.run(
        ["git", *arguments],
        cwd=repository,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        detail = result.stderr.strip()
        raise ValueError(f"{label} failed{': ' + detail if detail else ''}")


def verify(arguments: argparse.Namespace) -> dict[str, str]:
    repository = arguments.repository.resolve()
    if not repository.is_dir():
        raise ValueError("repository must be a directory")
    deployed_commit = require_commit(arguments.deployed_commit, "deployed commit")
    producer_commit = require_commit(arguments.producer_commit, "producer commit")
    git(repository, ["fetch", "--no-tags", "origin", "main"], "fetch origin main")
    git(
        repository,
        ["merge-base", "--is-ancestor", "HEAD", "origin/main"],
        "operator-to-origin/main ancestry",
    )
    git(
        repository,
        ["merge-base", "--is-ancestor", deployed_commit, deployed_commit],
        "deployed-to-producer ancestry",
    )
    git(
        repository,
        ["merge-base", "--is-ancestor", producer_commit, "origin/main"],
        "producer-to-origin/main ancestry",
    )
    status_result = subprocess.run(
        ["git", "status", "--porcelain"],
        cwd=repository,
        capture_output=True,
        check=False,
    )
    if status_result.returncode != 0:
        raise ValueError("operator tree status failed")
    if status_result.stdout:
        raise ValueError("operator tree must be clean")
    return {
        "deployed_commit": deployed_commit,
        "producer_commit": producer_commit,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", type=Path, required=True)
    parser.add_argument("--deployed-commit", required=True)
    parser.add_argument("--producer-commit", required=True)
    try:
        evidence = verify(parser.parse_args())
    except (OSError, ValueError) as error:
        raise SystemExit(f"native-oracle producer verification failed: {error}") from error
    json.dump(evidence, sys.stdout, sort_keys=True, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
