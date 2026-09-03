#!/usr/bin/env python3
# scripts/assemble-native-oracle-lanes.py

"""Validate and assemble one exact three-lane native-oracle evidence set."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import stat
import subprocess
import sys
from typing import Any


PROFILES = ("fedora-44", "ubuntu-26.04", "arch")
LANES = {
    "fedora-44": {
        "architecture": "x86_64",
        "image": "registry.fedoraproject.org/fedora@sha256:765b2260aa4b4eff379b9a6f983f15fcf41a6f9dda9b272b790e23e92fcbaafb",
        "package_binary": "conary-rpm-oracle",
        "resolution_binary": "conary-rpm-resolution-oracle",
        "package_implementation": {
            "ecosystem": "rpm",
            "name": "libsolv",
            "projection_schema": 1,
            "version": "0.7.36",
        },
        "resolution_implementation": {
            "ecosystem": "rpm",
            "name": "libsolv",
            "projection_schema": 5,
            "version": "0.7.36",
        },
    },
    "ubuntu-26.04": {
        "architecture": "amd64",
        "image": "docker.io/library/ubuntu:26.04@sha256:678c6550cc43645e08669028bc177f50be4e7c5b8cca677067b1914d4afc7a03",
        "package_binary": "conary-debian-oracle",
        "resolution_binary": "conary-debian-resolution-oracle",
        "package_implementation": {
            "ecosystem": "debian",
            "name": "apt-pkg",
            "projection_schema": 1,
            "version": "3.2.0",
        },
        "resolution_implementation": {
            "ecosystem": "debian",
            "name": "apt-pkg",
            "projection_schema": 3,
            "version": "3.2.0",
        },
    },
    "arch": {
        "architecture": "x86_64",
        "image": "docker.io/library/archlinux@sha256:fe6972d4dc1f660c0c10f4c41b2de8986bab89e7e2955378f8beadb8ebcd7433",
        "package_binary": "conary-alpm-oracle",
        "resolution_binary": "conary-alpm-resolution-oracle",
        "package_implementation": {
            "ecosystem": "alpm",
            "name": "libalpm",
            "projection_schema": 1,
        },
        "resolution_implementation": {
            "ecosystem": "alpm",
            "name": "libalpm",
            "projection_schema": 3,
        },
    },
}
MAX_JSON_BYTES = 16 * 1024 * 1024
NATIVE_ORACLE_LANE_EVIDENCE_SCHEMA = 4


def canonical_json(value: Any) -> bytes:
    return json.dumps(
        value, ensure_ascii=False, allow_nan=False, sort_keys=True, separators=(",", ":")
    ).encode()


def reject_duplicate_key(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise ValueError(f"duplicate JSON key {key!r}")
        value[key] = item
    return value


def plain_directory(path: Path, label: str) -> None:
    if not stat.S_ISDIR(path.lstat().st_mode):
        raise ValueError(f"{label} must be a directory, never a symlink")


def plain_file(path: Path, label: str, maximum: int | None = None) -> bytes:
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode):
        raise ValueError(f"{label} must be a regular file, never a symlink")
    if maximum is not None and metadata.st_size > maximum:
        raise ValueError(f"{label} exceeds {maximum} bytes")
    return path.read_bytes()


def load_canonical(path: Path, label: str) -> tuple[dict[str, Any], bytes]:
    data = plain_file(path, label, MAX_JSON_BYTES)
    value = json.loads(data, object_pairs_hook=reject_duplicate_key)
    if not isinstance(value, dict) or canonical_json(value) != data:
        raise ValueError(f"{label} is not a canonical JSON object")
    return value, data


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def require_sha256(value: Any, label: str) -> str:
    if not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{64}", value) is None:
        raise ValueError(f"{label} must be a lowercase SHA-256")
    return value


def require_commit(value: Any, label: str) -> str:
    if not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{40}", value) is None:
        raise ValueError(f"{label} must be a full lowercase commit SHA")
    return value


def require_positive(value: Any, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise ValueError(f"{label} must be a positive integer")
    return value


def require_keys(value: Any, expected: set[str], label: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != expected:
        raise ValueError(f"{label} has incomplete or unknown fields")
    return value


def git_is_ancestor(repository: Path, ancestor: str, descendant: str, label: str) -> None:
    result = subprocess.run(
        ["git", "merge-base", "--is-ancestor", ancestor, descendant],
        cwd=repository,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise ValueError(f"{label} ancestry is not proved")


def parse_lanes(raw_lanes: list[str]) -> dict[str, Path]:
    lanes: dict[str, Path] = {}
    for value in raw_lanes:
        profile, separator, raw_path = value.partition("=")
        if not separator or profile not in LANES or not raw_path or profile in lanes:
            raise ValueError("lane inputs must name each canonical profile exactly once")
        lanes[profile] = Path(raw_path).resolve()
    if tuple(profile for profile in PROFILES if profile in lanes) != PROFILES or len(lanes) != 3:
        raise ValueError("assembly requires exactly one Fedora, Ubuntu, and Arch lane")
    return lanes


def verify_oracle(
    root: Path,
    artifact_name: str,
    evidence: Any,
    profile: str,
    profile_revision: str,
    schema_version: int,
    expected_implementation: dict[str, Any],
    label: str,
) -> dict[str, Any]:
    evidence = require_keys(
        evidence,
        {"schema_version", "manifest_sha256", "artifact", "implementation"},
        f"{label} evidence",
    )
    plain_directory(root, f"{label} directory")
    if sorted(entry.name for entry in root.iterdir()) != sorted(
        [artifact_name, "manifest.json"]
    ):
        raise ValueError(f"{label} directory is incomplete or unexpected")
    manifest, manifest_bytes = load_canonical(root / "manifest.json", f"{label} manifest")
    artifact_path = root / artifact_name
    artifact_metadata = artifact_path.lstat()
    if not stat.S_ISREG(artifact_metadata.st_mode):
        raise ValueError(f"{label} artifact must be a regular file")
    artifact = require_keys(
        evidence["artifact"], {"name", "sha256", "size", "counts"}, f"{label} artifact"
    )
    if (
        evidence["schema_version"] != schema_version
        or manifest.get("schema_version") != schema_version
        or evidence["manifest_sha256"] != sha256(manifest_bytes)
        or artifact["name"] != artifact_name
        or artifact["size"] != artifact_metadata.st_size
        or artifact["sha256"] != sha256_file(artifact_path)
        or manifest.get("artifact", {}).get("size") != artifact["size"]
        or manifest.get("artifact", {}).get("sha256") != artifact["sha256"]
        or manifest.get("artifact", {}).get("counts") != artifact["counts"]
        or manifest.get("profile") != profile
        or manifest.get("profile_revision_sha256") != profile_revision
        or manifest.get("implementation") != evidence["implementation"]
    ):
        raise ValueError(f"{label} manifest or artifact binding drifted")
    implementation = evidence["implementation"]
    if profile == "arch":
        if (
            not isinstance(implementation, dict)
            or {key: implementation.get(key) for key in expected_implementation}
            != expected_implementation
            or not isinstance(implementation.get("version"), str)
            or not implementation["version"]
        ):
            raise ValueError(f"{label} implementation pin drifted")
    elif implementation != expected_implementation:
        raise ValueError(f"{label} implementation pin drifted")
    return evidence


def verify_binary(value: Any, expected_name: str, label: str) -> dict[str, str]:
    value = require_keys(value, {"name", "sha256"}, label)
    if value["name"] != expected_name:
        raise ValueError(f"{label} name drifted")
    require_sha256(value["sha256"], f"{label} digest")
    return value


def verify_resolution_walk_evidence(value: Any, label: str) -> dict[str, Any]:
    value = require_keys(
        value,
        {
            "schema_version",
            "workers",
            "worker_load_milliseconds",
            "memory_budget_bytes",
            "measured_worker_rss_bytes",
        },
        label,
    )
    if (
        value["schema_version"] != 1
        or not isinstance(value["workers"], int)
        or isinstance(value["workers"], bool)
        or value["workers"] < 1
        or not isinstance(value["worker_load_milliseconds"], list)
        or len(value["worker_load_milliseconds"]) != value["workers"]
        or any(
            not isinstance(item, int) or isinstance(item, bool) or item < 0
            for item in value["worker_load_milliseconds"]
        )
        or not isinstance(value["memory_budget_bytes"], int)
        or isinstance(value["memory_budget_bytes"], bool)
        or value["memory_budget_bytes"] < 1
        or not isinstance(value["measured_worker_rss_bytes"], int)
        or isinstance(value["measured_worker_rss_bytes"], bool)
        or value["measured_worker_rss_bytes"] < 1
    ):
        raise ValueError(f"{label} is malformed")
    return value


def verify_lane(
    root: Path,
    profile: str,
    arguments: argparse.Namespace,
    repository: Path,
) -> tuple[dict[str, Any], str]:
    plain_directory(root, f"{profile} lane")
    if sorted(entry.name for entry in root.iterdir()) != [
        "evidence.json",
        "package-oracle",
        "resolution-oracle",
    ]:
        raise ValueError(f"{profile} lane is not one strict oracle artifact")
    evidence, evidence_bytes = load_canonical(root / "evidence.json", f"{profile} evidence")
    expected_keys = {
        "schema_version",
        "artifact_type",
        "deployment_run_id",
        "export_run_id",
        "export_id",
        "transport_sha256",
        "deployed_commit",
        "producer_commit",
        "producer_binaries",
        "lane_image",
        "input_manifest_sha256",
        "profile",
        "profile_revision_sha256",
        "target_architecture",
        "package_oracle",
        "resolution_oracle",
        "resolution_implementation",
    }
    require_keys(evidence, expected_keys, f"{profile} evidence")
    lane = LANES[profile]
    producer_commit = require_commit(evidence["producer_commit"], f"{profile} producer commit")
    if (
        evidence["schema_version"] != NATIVE_ORACLE_LANE_EVIDENCE_SCHEMA
        or evidence["artifact_type"] != "native-oracle-lane"
        or evidence["deployment_run_id"] != arguments.deployment_run_id
        or evidence["export_run_id"] != arguments.export_run_id
        or evidence["export_id"] != arguments.export_id
        or evidence["transport_sha256"] != arguments.transport_sha256
        or evidence["deployed_commit"] != arguments.deployed_commit
        or evidence["profile"] != profile
        or evidence["target_architecture"] != lane["architecture"]
        or evidence["lane_image"] != lane["image"]
    ):
        raise ValueError(f"{profile} lane binding drifted")
    require_sha256(evidence["input_manifest_sha256"], f"{profile} input manifest")
    profile_revision = require_sha256(
        evidence["profile_revision_sha256"], f"{profile} profile revision"
    )
    binaries = require_keys(
        evidence["producer_binaries"], {"package", "resolution"}, f"{profile} binaries"
    )
    verify_binary(binaries["package"], lane["package_binary"], f"{profile} package binary")
    verify_binary(
        binaries["resolution"], lane["resolution_binary"], f"{profile} resolution binary"
    )
    verify_resolution_walk_evidence(
        evidence["resolution_implementation"], f"{profile} resolution implementation"
    )
    git_is_ancestor(
        repository,
        arguments.deployed_commit,
        producer_commit,
        f"{profile} deployed-to-producer",
    )
    git_is_ancestor(repository, producer_commit, arguments.main_ref, f"{profile} producer-to-main")
    package = verify_oracle(
        root / "package-oracle",
        "packages.jsonl",
        evidence["package_oracle"],
        profile,
        profile_revision,
        1,
        lane["package_implementation"],
        f"{profile} package oracle",
    )
    resolution = verify_oracle(
        root / "resolution-oracle",
        "roots.jsonl",
        evidence["resolution_oracle"],
        profile,
        profile_revision,
        3,
        lane["resolution_implementation"],
        f"{profile} resolution oracle",
    )
    resolution_manifest, _ = load_canonical(
        root / "resolution-oracle" / "manifest.json", f"{profile} resolution manifest"
    )
    if (
        resolution_manifest.get("package_oracle_manifest_sha256")
        != package["manifest_sha256"]
        or resolution_manifest.get("policy", {}).get("architecture")
        != lane["architecture"]
    ):
        raise ValueError(f"{profile} resolution-to-package binding drifted")
    if profile == "arch" and (
        package["implementation"]["version"] != resolution["implementation"]["version"]
    ):
        raise ValueError("arch implementation versions drifted")
    return evidence, sha256(evidence_bytes)


def load_artifact_metadata(path: Path) -> dict[str, dict[str, Any]]:
    metadata, _ = load_canonical(path, "GitHub artifact metadata")
    require_keys(metadata, {"schema_version", "artifacts"}, "GitHub artifact metadata")
    if metadata["schema_version"] != 1 or not isinstance(metadata["artifacts"], list):
        raise ValueError("GitHub artifact metadata schema is unsupported")
    by_profile: dict[str, dict[str, Any]] = {}
    expected_keys = {"profile", "artifact_id", "run_id", "name", "sha256"}
    for item in metadata["artifacts"]:
        item = require_keys(item, expected_keys, "GitHub artifact record")
        profile = item["profile"]
        if profile not in LANES or profile in by_profile:
            raise ValueError("GitHub artifact metadata profiles are incomplete or repeated")
        require_positive(item["artifact_id"], f"{profile} artifact id")
        require_positive(item["run_id"], f"{profile} artifact run id")
        require_sha256(item["sha256"], f"{profile} GitHub artifact digest")
        by_profile[profile] = item
    if tuple(profile for profile in PROFILES if profile in by_profile) != PROFILES or len(by_profile) != 3:
        raise ValueError("GitHub artifact metadata requires every canonical lane")
    return by_profile


def assemble(arguments: argparse.Namespace) -> dict[str, Any]:
    repository = arguments.repository.resolve()
    plain_directory(repository, "repository")
    require_commit(arguments.deployed_commit, "deployed commit")
    require_sha256(arguments.transport_sha256, "transport digest")
    require_positive(arguments.deployment_run_id, "deployment run id")
    require_positive(arguments.export_run_id, "export run id")
    if re.fullmatch(r"[a-z0-9][a-z0-9.-]*", arguments.export_id) is None:
        raise ValueError("export id is malformed")
    lanes = parse_lanes(arguments.lane)
    metadata = load_artifact_metadata(arguments.artifact_metadata.resolve())
    git_is_ancestor(repository, arguments.deployed_commit, arguments.main_ref, "deployed-to-main")

    assembled_lanes: list[dict[str, Any]] = []
    input_manifest_sha256: str | None = None
    for profile in PROFILES:
        evidence, evidence_sha256 = verify_lane(
            lanes[profile], profile, arguments, repository
        )
        if input_manifest_sha256 is None:
            input_manifest_sha256 = evidence["input_manifest_sha256"]
        elif evidence["input_manifest_sha256"] != input_manifest_sha256:
            raise ValueError("lane input manifest bindings disagree")
        artifact = metadata[profile]
        expected_name = (
            f"remi-native-oracle-lane-{profile}-{arguments.export_id}-"
            f"{evidence['producer_commit']}"
        )
        if artifact["name"] != expected_name:
            raise ValueError(f"{profile} GitHub artifact name disagrees with producer evidence")
        assembled_lanes.append(
            {
                "profile": profile,
                "profile_revision_sha256": evidence["profile_revision_sha256"],
                "target_architecture": evidence["target_architecture"],
                "lane_image": evidence["lane_image"],
                "producer_commit": evidence["producer_commit"],
                "producer_binaries": evidence["producer_binaries"],
                "lane_evidence_sha256": evidence_sha256,
                "package_oracle": evidence["package_oracle"],
                "resolution_oracle": evidence["resolution_oracle"],
                "github_artifact": {
                    "artifact_id": artifact["artifact_id"],
                    "run_id": artifact["run_id"],
                    "name": artifact["name"],
                    "sha256": artifact["sha256"],
                },
            }
        )
    return {
        "schema_version": 1,
        "artifact_type": "native-oracle-three-lane-set",
        "deployment_run_id": arguments.deployment_run_id,
        "export_run_id": arguments.export_run_id,
        "export_id": arguments.export_id,
        "transport_sha256": arguments.transport_sha256,
        "deployed_commit": arguments.deployed_commit,
        "input_manifest_sha256": input_manifest_sha256,
        "lanes": assembled_lanes,
    }


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", type=Path, required=True)
    parser.add_argument("--main-ref", default="origin/main")
    parser.add_argument("--deployed-commit", required=True)
    parser.add_argument("--deployment-run-id", type=int, required=True)
    parser.add_argument("--export-run-id", type=int, required=True)
    parser.add_argument("--export-id", required=True)
    parser.add_argument("--transport-sha256", required=True)
    parser.add_argument("--artifact-metadata", type=Path, required=True)
    parser.add_argument("--lane", action="append", default=[], required=True)
    parser.add_argument("--output", type=Path, required=True)
    return parser.parse_args()


def main() -> None:
    try:
        arguments = parse_arguments()
        evidence = assemble(arguments)
        output = arguments.output.resolve()
        output.parent.mkdir(parents=True, exist_ok=True)
        with output.open("xb") as destination:
            destination.write(canonical_json(evidence))
    except (json.JSONDecodeError, OSError, RuntimeError, TypeError, ValueError) as error:
        raise SystemExit(f"native-oracle lane assembly failed: {error}") from error
    json.dump(evidence, sys.stdout, sort_keys=True, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
