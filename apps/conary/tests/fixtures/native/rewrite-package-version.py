#!/usr/bin/env python3
# tests/fixtures/native/rewrite-package-version.py
"""Rewrite exactly package.version while preserving the pinned TOML fixture."""

from __future__ import annotations

import copy
import os
from pathlib import Path
import sys
import tempfile
import tomllib


def fail(message: str) -> None:
    raise SystemExit(message)


if len(sys.argv) != 4:
    fail(f"Usage: {sys.argv[0]} <ccs.toml> <expected-version> <new-version>")

manifest_path = Path(sys.argv[1])
expected_version = sys.argv[2]
new_version = sys.argv[3]
manifest = manifest_path.read_text(encoding="utf-8")

try:
    before = tomllib.loads(manifest)
except tomllib.TOMLDecodeError as error:
    fail(f"daily-driver manifest is invalid TOML: {error}")

package = before.get("package")
if not isinstance(package, dict) or package.get("version") != expected_version:
    fail(f"package.version must equal expected version {expected_version}")

lines = manifest.splitlines(keepends=True)
in_package = False
replacement_count = 0
old_assignment = f'version = "{expected_version}"'
new_assignment = f'version = "{new_version}"'
for index, line in enumerate(lines):
    stripped = line.strip()
    if stripped.startswith("[") and stripped.endswith("]"):
        in_package = stripped == "[package]"
        continue
    if in_package and stripped == old_assignment:
        lines[index] = line.replace(old_assignment, new_assignment, 1)
        replacement_count += 1

if replacement_count != 1:
    fail("[package] must contain one exact version assignment")

rewritten = "".join(lines)
try:
    after = tomllib.loads(rewritten)
except tomllib.TOMLDecodeError as error:
    fail(f"rewritten daily-driver manifest is invalid TOML: {error}")

expected = copy.deepcopy(before)
expected["package"]["version"] = new_version
if after != expected:
    fail("package version rewrite changed fields outside package.version")

mode = manifest_path.stat().st_mode & 0o777
temporary_path: Path | None = None
try:
    with tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        dir=manifest_path.parent,
        prefix=f".{manifest_path.name}.",
        delete=False,
    ) as stream:
        temporary_path = Path(stream.name)
        stream.write(rewritten)
        stream.flush()
        os.fsync(stream.fileno())
    os.chmod(temporary_path, mode)
    os.replace(temporary_path, manifest_path)
except BaseException:
    if temporary_path is not None:
        temporary_path.unlink(missing_ok=True)
    raise
