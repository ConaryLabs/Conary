#!/usr/bin/env python3
"""Verify paths and content through Conary's selected-generation authority."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import sys
from typing import Any, NoReturn


def fail(message: str) -> NoReturn:
    raise SystemExit(f"selected-generation assertion failed: {message}")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def relative_artifact_path(generation_dir: Path, value: Any, field: str) -> Path:
    if not isinstance(value, str) or not value:
        fail(f"artifact field {field!r} is not a non-empty path")
    relative = Path(value)
    if relative.is_absolute() or ".." in relative.parts:
        fail(f"artifact field {field!r} escapes the generation directory: {value!r}")
    return generation_dir / relative


def cas_base_from_artifact(root: Path, generation_dir: Path, value: Any) -> Path:
    if not isinstance(value, str) or not value:
        fail("artifact field 'cas_base' is not a non-empty path")
    relative = Path(value)
    if relative.is_absolute():
        fail(f"artifact field 'cas_base' is absolute: {value!r}")
    resolved = (generation_dir / relative).resolve()
    expected = (root / "objects").resolve()
    if resolved != expected:
        fail(
            f"artifact field 'cas_base' resolves to {resolved}, "
            f"expected runtime CAS {expected}"
        )
    return resolved


def load_hashed_json(
    generation_dir: Path,
    artifact: dict[str, Any],
    path_field: str,
    digest_field: str,
) -> tuple[Path, dict[str, Any]]:
    path = relative_artifact_path(generation_dir, artifact.get(path_field), path_field)
    expected = artifact.get(digest_field)
    if not isinstance(expected, str) or len(expected) != 64:
        fail(f"artifact field {digest_field!r} is not a SHA-256 digest")
    actual = sha256(path)
    if actual != expected:
        fail(f"{path_field} digest mismatch: expected {expected}, got {actual}")
    with path.open("r", encoding="utf-8") as stream:
        value = json.load(stream)
    if not isinstance(value, dict):
        fail(f"{path_field} does not contain a JSON object")
    return path, value


def selected_generation(root: Path) -> tuple[int, Path]:
    root = root.resolve()
    current = root / "current"
    if not current.is_symlink():
        fail(f"{current} is not a selected-generation symlink")
    target = (root / os.readlink(current)).resolve()
    generations = (root / "generations").resolve()
    if target.parent != generations:
        fail(f"{current} targets {target}, outside {generations}")
    try:
        generation = int(target.name)
    except ValueError:
        fail(f"{current} target is not a numeric generation: {target.name!r}")
    if not target.is_dir():
        fail(f"selected generation directory does not exist: {target}")
    return generation, target


def load_entries(root: Path) -> tuple[int, dict[str, dict[str, Any]], Path]:
    generation, generation_dir = selected_generation(root)
    artifact_path = generation_dir / ".conary-artifact.json"
    with artifact_path.open("r", encoding="utf-8") as stream:
        artifact = json.load(stream)
    if not isinstance(artifact, dict):
        fail(f"{artifact_path} does not contain a JSON object")
    if artifact.get("version") != 2:
        fail(f"unsupported artifact version: {artifact.get('version')!r}")
    if artifact.get("generation") != generation:
        fail(
            f"artifact generation {artifact.get('generation')!r} "
            f"does not match selected generation {generation}"
        )

    _, generation_root = load_hashed_json(
        generation_dir,
        artifact,
        "generation_root_manifest",
        "generation_root_manifest_sha256",
    )
    _, mutable_state = load_hashed_json(
        generation_dir,
        artifact,
        "mutable_state_manifest",
        "mutable_state_manifest_sha256",
    )
    _, cas_manifest = load_hashed_json(
        generation_dir,
        artifact,
        "cas_manifest",
        "cas_manifest_sha256",
    )
    if generation_root.get("version") != 1 or mutable_state.get("version") != 1:
        fail("selected root uses an unsupported manifest version")
    if cas_manifest.get("generation") != generation:
        fail("CAS manifest generation does not match selected generation")

    cas_base = cas_base_from_artifact(root.resolve(), generation_dir, artifact.get("cas_base"))
    entries: dict[str, dict[str, Any]] = {}
    for owner, manifest in (
        ("generation-root", generation_root),
        ("mutable-state", mutable_state),
    ):
        manifest_entries = manifest.get("entries")
        if not isinstance(manifest_entries, list):
            fail(f"{owner} entries are not an array")
        for entry in manifest_entries:
            if not isinstance(entry, dict) or not isinstance(entry.get("path"), str):
                fail(f"{owner} contains an invalid path entry")
            path = entry["path"]
            if path in entries:
                fail(f"path {path!r} is duplicated across selected-root manifests")
            entries[path] = entry

    declared_objects: dict[str, int] = {}
    objects = cas_manifest.get("objects")
    if not isinstance(objects, list):
        fail("CAS manifest objects are not an array")
    for item in objects:
        if not isinstance(item, dict):
            fail("CAS manifest contains a non-object entry")
        digest = item.get("sha256")
        size = item.get("size")
        if (
            not isinstance(digest, str)
            or len(digest) != 64
            or not isinstance(size, int)
            or size < 0
        ):
            fail("CAS manifest contains invalid content authority")
        if digest in declared_objects and declared_objects[digest] != size:
            fail(f"CAS object {digest} has conflicting declared sizes")
        declared_objects[digest] = size

    for path, entry in entries.items():
        content = entry.get("content")
        if content is None:
            continue
        if not isinstance(content, dict):
            fail(f"path {path!r} has invalid content authority")
        digest = content.get("sha256")
        size = content.get("size")
        if digest not in declared_objects or declared_objects[digest] != size:
            fail(f"path {path!r} is absent from the exact CAS manifest")
        object_path = cas_base / digest[:2] / digest[2:]
        if not object_path.is_file():
            fail(f"path {path!r} CAS object is missing: {object_path}")
        if object_path.stat().st_size != size:
            fail(f"path {path!r} CAS object size does not match its authority")
        if sha256(object_path) != digest:
            fail(f"path {path!r} CAS object digest does not match its authority")

    return generation, entries, cas_base


def split_expectation(value: str, option: str) -> tuple[str, str]:
    try:
        path, expected = value.split("=", 1)
    except ValueError:
        fail(f"{option} expects PATH=VALUE, got {value!r}")
    if not path.startswith("/") or not expected:
        fail(f"{option} expects an absolute PATH and non-empty VALUE")
    return path, expected


def entry_content(
    entries: dict[str, dict[str, Any]], cas_base: Path, path: str
) -> tuple[dict[str, Any], bytes]:
    entry = entries.get(path)
    if entry is None:
        fail(f"selected generation does not contain {path}")
    content = entry.get("content")
    if not isinstance(content, dict) or not isinstance(content.get("sha256"), str):
        fail(f"selected-generation path {path} has no regular-file content authority")
    digest = content["sha256"]
    return entry, (cas_base / digest[:2] / digest[2:]).read_bytes()


def parse_colon_records(content: bytes, path: str, field_count: int) -> list[list[str]]:
    try:
        lines = content.decode("utf-8").splitlines()
    except UnicodeDecodeError as error:
        fail(f"{path} is not UTF-8 account data: {error}")
    records: list[list[str]] = []
    for line_number, line in enumerate(lines, start=1):
        if not line:
            continue
        fields = line.split(":")
        if len(fields) != field_count:
            fail(
                f"{path}:{line_number} has {len(fields)} fields; "
                f"expected exact {field_count}-field account grammar"
            )
        records.append(fields)
    return records


def unique_named_record(records: list[list[str]], name: str, path: str) -> list[str]:
    matches = [record for record in records if record[0] == name]
    if len(matches) != 1:
        fail(f"{path} contains {len(matches)} records named {name!r}; expected exactly one")
    return matches[0]


def validate_decimal_id(value: str, field: str, path: str) -> int:
    if not value.isascii() or not value.isdecimal():
        fail(f"{path} field {field} is not an unsigned decimal ID: {value!r}")
    return int(value)


def assert_selected_user(
    entries: dict[str, dict[str, Any]],
    cas_base: Path,
    expectation: str,
) -> None:
    try:
        name, contract = expectation.split("=", 1)
    except ValueError:
        fail(f"--expect-user expects NAME=HOME,SHELL,GROUP, got {expectation!r}")
    if not name or name.startswith("/") or any(character in name for character in ":,"):
        fail(f"--expect-user has an invalid account name: {name!r}")
    fields = contract.split(",")
    if len(fields) != 3 or any(not field for field in fields):
        fail("--expect-user expects NAME=HOME,SHELL,GROUP")
    expected_home, expected_shell, expected_group = fields

    _, passwd_content = entry_content(entries, cas_base, "/etc/passwd")
    _, group_content = entry_content(entries, cas_base, "/etc/group")
    passwd = unique_named_record(
        parse_colon_records(passwd_content, "/etc/passwd", 7),
        name,
        "/etc/passwd",
    )
    group = unique_named_record(
        parse_colon_records(group_content, "/etc/group", 4),
        expected_group,
        "/etc/group",
    )
    uid = validate_decimal_id(passwd[2], "uid", "/etc/passwd")
    gid = validate_decimal_id(passwd[3], "gid", "/etc/passwd")
    group_gid = validate_decimal_id(group[2], "gid", "/etc/group")
    if uid == 0 or gid == 0:
        fail(f"selected user {name!r} unexpectedly has a root UID or GID")
    if gid != group_gid:
        fail(
            f"selected user {name!r} GID {gid} does not match "
            f"group {expected_group!r} GID {group_gid}"
        )
    if passwd[5] != expected_home or passwd[6] != expected_shell:
        fail(
            f"selected user {name!r} has home/shell {passwd[5]!r}/{passwd[6]!r}; "
            f"expected {expected_home!r}/{expected_shell!r}"
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--present", action="append", default=[])
    parser.add_argument("--absent", action="append", default=[])
    parser.add_argument("--expect-sha256", action="append", default=[], metavar="PATH=SHA256")
    parser.add_argument("--contains-line", action="append", default=[], metavar="PATH=LINE")
    parser.add_argument(
        "--expect-user",
        action="append",
        default=[],
        metavar="NAME=HOME,SHELL,GROUP",
    )
    args = parser.parse_args()

    generation, entries, cas_base = load_entries(args.root)

    for path in args.present:
        if path not in entries:
            fail(f"selected generation does not contain {path}")
    for path in args.absent:
        if path in entries:
            fail(f"selected generation unexpectedly contains {path}")
    for value in args.expect_sha256:
        path, expected = split_expectation(value, "--expect-sha256")
        entry, content = entry_content(entries, cas_base, path)
        authority = entry["content"]["sha256"]
        if authority != expected:
            fail(f"{path} authority is {authority}, expected {expected}")
        if hashlib.sha256(content).hexdigest() != expected:
            fail(f"{path} bytes do not match expected digest {expected}")
    for value in args.contains_line:
        path, expected = split_expectation(value, "--contains-line")
        _, content = entry_content(entries, cas_base, path)
        lines = content.decode("utf-8").splitlines()
        if expected not in lines:
            fail(f"{path} does not contain exact line {expected!r}")
    for expectation in args.expect_user:
        assert_selected_user(entries, cas_base, expectation)

    print(
        f"selected generation {generation}: "
        f"{len(entries)} paths and "
        f"{len(args.present) + len(args.expect_sha256) + len(args.expect_user)} "
        "expectations verified"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
