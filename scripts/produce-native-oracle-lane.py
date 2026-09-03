#!/usr/bin/env python3
# scripts/produce-native-oracle-lane.py

"""Produce one exact native package-fact and resolution oracle lane."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
from typing import Any


PUBLIC_PROFILES = ("fedora-44", "ubuntu-26.04", "arch")
LANES = {
    "fedora-44": {
        "ecosystem": "rpm",
        "implementation": "libsolv",
        "roles": ("rpm_primary", "rpm_filelists"),
        "flags": ("--primary", "--filelists"),
    },
    "ubuntu-26.04": {
        "ecosystem": "debian",
        "implementation": "apt-pkg",
        "roles": ("debian_packages",),
        "flags": ("--packages",),
    },
    "arch": {
        "ecosystem": "alpm",
        "implementation": "libalpm",
        "roles": ("arch_database",),
        "flags": ("--database",),
    },
}
SHA256_LENGTH = 64
MAX_MANIFEST_BYTES = 16 * 1024 * 1024
NATIVE_PACKAGE_ORACLE_SCHEMA = 1
NATIVE_RESOLUTION_ORACLE_SCHEMA = 2


def canonical_json(value: Any) -> bytes:
    return json.dumps(
        value, ensure_ascii=False, allow_nan=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")


def reject_duplicate_key(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise ValueError(f"duplicate JSON key {key!r}")
        value[key] = item
    return value


def plain_file(path: Path, label: str, maximum: int | None = None) -> bytes:
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode):
        raise ValueError(f"{label} must be a regular file, never a symlink")
    if maximum is not None and metadata.st_size > maximum:
        raise ValueError(f"{label} exceeds {maximum} bytes")
    return path.read_bytes()


def plain_directory(path: Path, label: str) -> None:
    metadata = path.lstat()
    if not stat.S_ISDIR(metadata.st_mode):
        raise ValueError(f"{label} must be a directory, never a symlink")


def load_canonical(path: Path, label: str) -> tuple[Any, bytes]:
    data = plain_file(path, label, MAX_MANIFEST_BYTES)
    value = json.loads(data, object_pairs_hook=reject_duplicate_key)
    if canonical_json(value) != data:
        raise ValueError(f"{label} is not canonical JSON")
    return value, data


def require_keys(value: Any, expected: set[str], label: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != expected:
        raise ValueError(f"{label} has incomplete or unknown fields")
    return value


def require_sha256(value: Any, label: str) -> str:
    if (
        not isinstance(value, str)
        or len(value) != SHA256_LENGTH
        or any(character not in "0123456789abcdef" for character in value)
    ):
        raise ValueError(f"{label} must be a lowercase SHA-256 digest")
    return value


def require_commit(value: Any, label: str) -> str:
    if (
        not isinstance(value, str)
        or len(value) != 40
        or any(character not in "0123456789abcdef" for character in value)
    ):
        raise ValueError(f"{label} must be a full lowercase commit digest")
    return value


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def validate_input(root: Path, selected_profile: str) -> tuple[dict[str, Any], dict[str, Any], str]:
    plain_directory(root, "native-oracle input root")
    names = sorted(entry.name for entry in root.iterdir())
    if names != ["manifest.json", "objects"]:
        raise ValueError("native-oracle input root entries are incomplete or unexpected")
    objects_root = root / "objects"
    plain_directory(objects_root, "native-oracle input object root")
    manifest, manifest_bytes = load_canonical(root / "manifest.json", "native-oracle input manifest")
    manifest = require_keys(manifest, {"schema_version", "profiles", "objects"}, "native-oracle input manifest")
    if manifest["schema_version"] != 1:
        raise ValueError("native-oracle input schema must be 1")
    profiles = manifest["profiles"]
    if (
        not isinstance(profiles, list)
        or any(not isinstance(profile, dict) for profile in profiles)
        or any(not isinstance(profile.get("revision"), dict) for profile in profiles)
        or [profile["revision"].get("profile") for profile in profiles] != list(PUBLIC_PROFILES)
    ):
        raise ValueError("native-oracle input requires canonical Fedora, Ubuntu, and Arch order")

    inventory: list[tuple[str, int]] = []
    raw_objects = manifest["objects"]
    if not isinstance(raw_objects, list):
        raise ValueError("native-oracle object inventory must be an array")
    for ordinal, raw_object in enumerate(raw_objects):
        item = require_keys(raw_object, {"sha256", "size"}, f"object {ordinal}")
        digest = require_sha256(item["sha256"], f"object {ordinal} digest")
        size = item["size"]
        if not isinstance(size, int) or isinstance(size, bool) or size < 0:
            raise ValueError(f"object {ordinal} has invalid size")
        inventory.append((digest, size))
    if inventory != sorted(set(inventory)) or not inventory:
        raise ValueError("native-oracle object inventory is empty, repeated, or reordered")
    if sorted(entry.name for entry in objects_root.iterdir()) != [digest for digest, _ in inventory]:
        raise ValueError("native-oracle object directory disagrees with the manifest")
    for digest, size in inventory:
        path = objects_root / digest
        data = plain_file(path, f"native-oracle object {digest}")
        if len(data) != size or sha256(data) != digest:
            raise ValueError(f"native-oracle object {digest} changed size or digest")

    profile = next(profile for profile in profiles if profile["revision"]["profile"] == selected_profile)
    profile = require_keys(profile, {"profile_revision_sha256", "revision", "sources"}, f"{selected_profile} input")
    revision = profile["revision"]
    if not isinstance(revision, dict):
        raise ValueError(f"{selected_profile} revision must be an object")
    if revision.get("schema_version") != 3:
        raise ValueError(f"{selected_profile} profile revision schema must be 3")
    target_architecture = revision.get("target_architecture")
    if (
        not isinstance(target_architecture, str)
        or re.fullmatch(r"[a-z0-9][a-z0-9_.-]*", target_architecture) is None
    ):
        raise ValueError(f"{selected_profile} target architecture is invalid")
    observed_revision = require_sha256(profile["profile_revision_sha256"], f"{selected_profile} revision")
    if sha256(canonical_json(revision)) != observed_revision:
        raise ValueError(f"{selected_profile} revision digest drifted")
    members = revision.get("members")
    sources = profile["sources"]
    if not isinstance(members, list) or not isinstance(sources, list) or len(members) != len(sources) or not members:
        raise ValueError(f"{selected_profile} source membership is incomplete")
    object_digests = {digest for digest, _ in inventory}
    expected_roles = LANES[selected_profile]["roles"]
    for ordinal, (member, source) in enumerate(zip(members, sources, strict=True)):
        if not isinstance(member, dict) or not isinstance(source, dict):
            raise ValueError(f"{selected_profile} source membership is malformed")
        if member.get("ordinal") != ordinal:
            raise ValueError(f"{selected_profile} member order changed at ordinal {ordinal}")
        source_digest = require_sha256(member.get("source_snapshot_sha256"), f"{selected_profile} source {ordinal}")
        if sha256(canonical_json(source)) != source_digest:
            raise ValueError(f"{selected_profile} source {ordinal} digest drifted")
        if source.get("source_profile") != selected_profile:
            raise ValueError(f"{selected_profile} source {ordinal} profile drifted")
        authenticated = source.get("authenticated_objects")
        if (
            not isinstance(authenticated, list)
            or any(not isinstance(item, dict) for item in authenticated)
            or tuple(item.get("role") for item in authenticated) != expected_roles
        ):
            raise ValueError(f"{selected_profile} source {ordinal} authenticated roles changed")
        for item in authenticated:
            digest = require_sha256(item.get("sha256"), f"{selected_profile} source object")
            if digest not in object_digests:
                raise ValueError(f"{selected_profile} source object {digest} is absent from inventory")
    return manifest, profile, sha256(manifest_bytes)


def invoke(command: list[str], label: str) -> None:
    result = subprocess.run(command, check=False)
    if result.returncode != 0:
        raise RuntimeError(f"{label} failed with exit status {result.returncode}")


def oracle_evidence(
    directory: Path,
    artifact_name: str,
    profile: dict[str, Any],
    lane: dict[str, Any],
    label: str,
    required_schema: int,
    package_manifest_sha256: str | None = None,
) -> dict[str, Any]:
    plain_directory(directory, f"{label} directory")
    if sorted(entry.name for entry in directory.iterdir()) != ["manifest.json", artifact_name]:
        raise ValueError(f"{label} entries are incomplete or unexpected")
    manifest, manifest_bytes = load_canonical(directory / "manifest.json", f"{label} manifest")
    artifact = plain_file(directory / artifact_name, f"{label} artifact")
    if manifest.get("schema_version") != required_schema:
        raise ValueError(f"{label} schema must be {required_schema}")
    if manifest.get("profile") != profile["revision"]["profile"] or manifest.get("profile_revision_sha256") != profile["profile_revision_sha256"]:
        raise ValueError(f"{label} does not bind the exact profile revision")
    if manifest.get("members") != profile["revision"].get("members"):
        raise ValueError(f"{label} member binding drifted")
    implementation = manifest.get("implementation")
    if not isinstance(implementation, dict) or implementation.get("ecosystem") != lane["ecosystem"] or implementation.get("name") != lane["implementation"]:
        raise ValueError(f"{label} native implementation drifted")
    if not isinstance(implementation.get("version"), str) or not implementation["version"]:
        raise ValueError(f"{label} native implementation version is empty")
    bound_artifact = manifest.get("artifact")
    if not isinstance(bound_artifact, dict) or bound_artifact.get("size") != len(artifact) or bound_artifact.get("sha256") != sha256(artifact):
        raise ValueError(f"{label} artifact binding drifted")
    if package_manifest_sha256 is not None:
        if manifest.get("package_oracle_manifest_sha256") != package_manifest_sha256:
            raise ValueError("native resolution does not bind the exact package oracle")
        policy = manifest.get("policy")
        if (
            not isinstance(policy, dict)
            or policy.get("architecture") != profile["revision"]["target_architecture"]
        ):
            raise ValueError("native resolution target architecture drifted")
    return {
        "schema_version": manifest["schema_version"],
        "manifest_sha256": sha256(manifest_bytes),
        "artifact": {
            "name": artifact_name,
            "sha256": sha256(artifact),
            "size": len(artifact),
            "counts": bound_artifact.get("counts"),
        },
        "implementation": implementation,
    }


def produce(arguments: argparse.Namespace) -> dict[str, Any]:
    input_root = arguments.input_root.resolve()
    output_root = arguments.output_root.resolve()
    lane = LANES[arguments.profile]
    manifest, profile, input_manifest_sha256 = validate_input(input_root, arguments.profile)
    target_architecture = profile["revision"]["target_architecture"]
    if arguments.architecture != target_architecture:
        raise ValueError(
            f"{arguments.profile} architecture must match profile authority {target_architecture}"
        )
    if output_root.exists():
        raise ValueError("native-oracle lane output already exists")
    output_root.parent.mkdir(parents=True, exist_ok=True)
    output_root.mkdir(mode=0o700)
    package_output = output_root / "package-oracle"
    resolution_output = output_root / "resolution-oracle"
    resolution_implementation_path = output_root / "resolution-implementation.json"

    with tempfile.TemporaryDirectory(prefix="native-oracle-lane-", dir=output_root.parent) as temporary:
        staging = Path(temporary)
        profile_manifest = staging / "profile.json"
        profile_manifest.write_bytes(canonical_json(profile["revision"]))
        source_paths: list[Path] = []
        for ordinal, source in enumerate(profile["sources"]):
            path = staging / f"source-{ordinal}.json"
            path.write_bytes(canonical_json(source))
            source_paths.append(path)

        package_command = [str(arguments.package_producer), "--profile-manifest", str(profile_manifest)]
        resolution_command = [str(arguments.resolution_producer), "--profile-manifest", str(profile_manifest)]
        roles = lane["roles"]
        flags = lane["flags"]
        for source_path, source in zip(source_paths, profile["sources"], strict=True):
            package_command.extend(("--source-snapshot", str(source_path)))
            resolution_command.extend(("--source-snapshot", str(source_path)))
            objects_by_role = {item["role"]: input_root / "objects" / item["sha256"] for item in source["authenticated_objects"]}
            for role, flag in zip(roles, flags, strict=True):
                package_command.extend((flag, str(objects_by_role[role])))
                resolution_command.extend((flag, str(objects_by_role[role])))
        package_command.extend(("--output", str(package_output)))
        invoke(package_command, "native package-fact producer")
        resolution_command.extend((
            "--package-oracle", str(package_output),
            "--architecture", arguments.architecture,
            "--output", str(resolution_output),
            "--implementation-evidence", str(resolution_implementation_path),
        ))
        invoke(resolution_command, "native resolution producer")

    package = oracle_evidence(
        package_output,
        "packages.jsonl",
        profile,
        lane,
        "native package oracle",
        NATIVE_PACKAGE_ORACLE_SCHEMA,
    )
    resolution = oracle_evidence(
        resolution_output,
        "roots.jsonl",
        profile,
        lane,
        "native resolution oracle",
        NATIVE_RESOLUTION_ORACLE_SCHEMA,
        package["manifest_sha256"],
    )
    resolution_implementation, _ = load_canonical(
        resolution_implementation_path, "resolution implementation evidence"
    )
    resolution_implementation = require_keys(
        resolution_implementation,
        {
            "schema_version",
            "workers",
            "worker_load_milliseconds",
            "memory_budget_bytes",
            "measured_worker_rss_bytes",
        },
        "resolution implementation evidence",
    )
    if (
        resolution_implementation["schema_version"] != 1
        or not isinstance(resolution_implementation["workers"], int)
        or resolution_implementation["workers"] < 1
        or not isinstance(resolution_implementation["worker_load_milliseconds"], list)
        or len(resolution_implementation["worker_load_milliseconds"])
        != resolution_implementation["workers"]
    ):
        raise ValueError("resolution implementation evidence is malformed")
    evidence = {
        "schema_version": 2,
        "export_id": arguments.export_id,
        "deployed_commit": arguments.deployed_commit,
        "input_manifest_sha256": input_manifest_sha256,
        "profile": arguments.profile,
        "profile_revision_sha256": profile["profile_revision_sha256"],
        "target_architecture": target_architecture,
        "package_oracle": package,
        "resolution_oracle": resolution,
        "resolution_implementation": resolution_implementation,
    }
    evidence_bytes = canonical_json(evidence)
    (output_root / "evidence.json").write_bytes(evidence_bytes)
    return evidence


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input-root", type=Path, required=True)
    parser.add_argument("--profile", choices=PUBLIC_PROFILES, required=True)
    parser.add_argument("--architecture", required=True)
    parser.add_argument("--package-producer", type=Path, required=True)
    parser.add_argument("--resolution-producer", type=Path, required=True)
    parser.add_argument("--output-root", type=Path, required=True)
    parser.add_argument("--export-id", required=True)
    parser.add_argument("--deployed-commit", required=True)
    arguments = parser.parse_args()
    require_commit(arguments.deployed_commit, "deployed commit")
    if re.fullmatch(r"[a-z0-9][a-z0-9.-]*", arguments.export_id) is None:
        raise ValueError("export ID must be a lowercase public identity")
    return arguments


def main() -> None:
    try:
        evidence = produce(parse_arguments())
    except (KeyError, OSError, TypeError, ValueError, RuntimeError, json.JSONDecodeError) as error:
        raise SystemExit(f"native-oracle lane production failed: {error}") from error
    json.dump(evidence, sys.stdout, sort_keys=True, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
