#!/usr/bin/env python3
# scripts/native_oracle_common.py

"""Shared validation primitives for native-oracle operator scripts."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import stat
from typing import Any


DEFAULT_MAX_JSON_BYTES = 16 * 1024 * 1024
SHA256 = re.compile(r"[0-9a-f]{64}")
COMMIT = re.compile(r"[0-9a-f]{40}")


def canonical_json(value: Any) -> bytes:
    return json.dumps(
        value,
        ensure_ascii=False,
        allow_nan=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")


def reject_duplicate_key(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise ValueError(f"manifest repeats key {key!r} (repeats JSON key)")
        value[key] = item
    return value


def plain_file(
    path: Path, label: str, maximum: int | None = None
) -> os.stat_result:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise ValueError(f"cannot inspect {label}: {error}") from error
    if not stat.S_ISREG(metadata.st_mode) or path.is_symlink():
        raise ValueError(f"{label} must be a regular file, never a symlink")
    if maximum is not None and (metadata.st_size <= 0 or metadata.st_size > maximum):
        raise ValueError(f"{label} size is outside its bounded contract")
    return metadata


def plain_directory(path: Path, label: str) -> None:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise ValueError(f"cannot inspect {label}: {error}") from error
    if not stat.S_ISDIR(metadata.st_mode) or path.is_symlink():
        raise ValueError(f"{label} must be a directory, never a symlink")


def load_canonical(
    path: Path, label: str, maximum: int = DEFAULT_MAX_JSON_BYTES
) -> tuple[dict[str, Any], bytes]:
    metadata = plain_file(path, label, maximum)
    data = path.read_bytes()
    if len(data) != metadata.st_size:
        raise ValueError(f"{label} changed while being read")
    value = json.loads(data, object_pairs_hook=reject_duplicate_key)
    if not isinstance(value, dict) or canonical_json(value) != data:
        raise ValueError(f"{label} is not a canonical JSON object")
    return value, data


def require_sha256(value: Any, label: str) -> str:
    if not isinstance(value, str) or SHA256.fullmatch(value) is None:
        raise ValueError(f"{label} must be a lowercase SHA-256 digest")
    return value


def require_commit(
    value: Any,
    label: str,
    requirement: str = "a full lowercase commit digest",
) -> str:
    if not isinstance(value, str) or COMMIT.fullmatch(value) is None:
        raise ValueError(f"{label} must be {requirement}")
    return value


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()
