#!/usr/bin/env python3
# scripts/remi-resolution-survey-transport.py

"""Build authenticated survey inputs and verify sanitized survey outputs."""

from __future__ import annotations

import argparse
import hashlib
import json
import mmap
import os
from pathlib import Path, PurePosixPath
import re
import stat
import sys
import tarfile
import tempfile
from typing import Any, Iterator, NoReturn


PUBLIC_PROFILES = ("fedora-44", "ubuntu-26.04", "arch")
INPUT_MANIFEST_SCHEMA = 2
INPUT_EVIDENCE_SCHEMA = 2
OUTPUT_MANIFEST_SCHEMA = 3
OUTPUT_EVIDENCE_SCHEMA = 3
PROFILE_ARCHITECTURES = {
    "fedora-44": "x86_64",
    "ubuntu-26.04": "amd64",
    "arch": "x86_64",
}
PROFILE_ECOSYSTEMS = {
    "fedora-44": "rpm",
    "ubuntu-26.04": "debian",
    "arch": "alpm",
}
PROFILE_PRODUCER_BINARIES = {
    "fedora-44": ("conary-rpm-oracle", "conary-rpm-resolution-oracle"),
    "ubuntu-26.04": ("conary-debian-oracle", "conary-debian-resolution-oracle"),
    "arch": ("conary-alpm-oracle", "conary-alpm-resolution-oracle"),
}
SHA256 = re.compile(r"^[0-9a-f]{64}$")
COMMIT = re.compile(r"^[0-9a-f]{40}$")
IDENTITY = re.compile(r"^[a-z0-9][a-z0-9._-]{0,127}$")
RUN_ID = re.compile(r"^[1-9][0-9]*$")
MAX_MANIFEST_BYTES = 1024 * 1024
MAX_SURVEY_DOCUMENTS = len(PUBLIC_PROFILES) * 4
SURVEY_RECORD_LIMIT = 5_000
SURVEY_EVIDENCE_BYTE_LIMIT = 32 * 1024 * 1024
ORACLE_TRANSPORT_TAR_FORMAT = tarfile.USTAR_FORMAT
U32_MAX = 2**32 - 1
U64_MAX = 2**64 - 1
NATIVE_ECOSYSTEMS = {"rpm", "debian", "alpm"}
NATIVE_ERROR_VARIANTS = (
    "database", "io", "io_error", "init_error", "schema_rebuild_required", "missing_id",
    "version_parse", "version_comparison", "hash_error", "config_error", "database_not_found",
    "download_error", "repository_response_body", "durable_chunk_unavailable", "http_status",
    "conflict_error", "profile_architecture_mismatch", "unknown_architecture_token",
    "unsupported_native_host_target", "ambiguous_package_selection", "checksum_mismatch",
    "parse_error", "budget", "catalog_scratch_capacity", "delta_error",
    "gpg_verification_failed", "scriptlet_execution", "trigger_error", "already_exists",
    "invalid_path", "path_traversal", "not_found", "recovery_failed", "timeout_error",
    "resolution_error", "not_implemented", "json", "capability", "federation", "cancelled",
    "internal_error", "trust_error", "pool_overflow",
)
CONARY_ERROR_REASONS = (
    "exact_root_projection_failed", "architecture_admission_failed", "solver_failed",
    "resolved_closure_projection_failed", "resolved_closure_omitted_root",
    "unresolved_projection_failed",
)
OUTCOME_KINDS = ("resolved", "unresolved", "not_installable")
MISMATCH_KINDS = (
    "resolution_outcome", "dependency_closure", "unresolved_dependencies",
    "not_installable_reason",
)


class ValidationError(ValueError):
    """An input or output differs from the reviewed transport contract."""


class SchemaRebuildRequired(ValidationError):
    """A recognized retired envelope is non-authority and must be rebuilt."""

    def __init__(self, envelope: str, found: int, current: int):
        self.evidence = {
            "status": "obsolete",
            "reason": "schema_rebuild_required",
            "envelope": envelope,
            "found_schema": found,
            "current_schema": current,
            "message": f"obsolete {envelope} schema {found}; rebuild as schema {current}",
        }
        super().__init__(self.evidence["message"])


def require_envelope_schema(value: Any, current: int, label: str) -> None:
    if not isinstance(value, dict):
        fail(f"{label} must be an object")
    version = exact_u32(value.get("schema_version"), f"{label} schema_version")
    if 0 < version < current:
        raise SchemaRebuildRequired(label, version, current)
    if version != current:
        fail(f"{label} schema {version} is unsupported; expected {current}")


def fail(message: str) -> NoReturn:
    raise ValidationError(message)


def canonical_json(value: Any) -> bytes:
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    ).encode()


def reject_duplicate_key(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            fail(f"JSON repeats key {key!r}")
        value[key] = item
    return value


def decode_json(data: bytes, label: str) -> Any:
    try:
        return json.loads(
            data,
            object_pairs_hook=reject_duplicate_key,
            parse_constant=lambda value: fail(
                f"{label} contains non-finite JSON number {value!r}"
            ),
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        fail(f"{label} is not valid JSON: {error}")


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def hash_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def require_sha256(value: Any, label: str) -> str:
    if not isinstance(value, str) or SHA256.fullmatch(value) is None:
        fail(f"{label} must be one lowercase SHA-256")
    return value


def require_commit(value: Any, label: str) -> str:
    if not isinstance(value, str) or COMMIT.fullmatch(value) is None:
        fail(f"{label} must be one lowercase commit SHA")
    return value


def require_identity(value: Any, label: str) -> str:
    if not isinstance(value, str) or IDENTITY.fullmatch(value) is None:
        fail(f"{label} is not a canonical public identity")
    return value


def require_run_id(value: str, label: str) -> int:
    if RUN_ID.fullmatch(value) is None:
        fail(f"{label} must be a positive decimal GitHub run id")
    return int(value)


def exact_object(value: Any, keys: set[str], label: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != keys:
        fail(f"{label} fields differ from the exact schema")
    return value


def exact_nonnegative_int(value: Any, label: str) -> int:
    if (
        not isinstance(value, int)
        or isinstance(value, bool)
        or value < 0
        or value > U64_MAX
    ):
        fail(f"{label} must be one unsigned 64-bit integer")
    return value


def exact_positive_int(value: Any, label: str) -> int:
    value = exact_nonnegative_int(value, label)
    if value == 0:
        fail(f"{label} must be positive")
    return value


def exact_u32(value: Any, label: str, *, positive: bool = False) -> int:
    value = exact_nonnegative_int(value, label)
    if value > U32_MAX or (positive and value == 0):
        fail(f"{label} must be one{' positive' if positive else ''} unsigned 32-bit integer")
    return value


def require_rust_identity(value: Any, label: str) -> str:
    if (
        not isinstance(value, str)
        or not value
        or len(value.encode()) > 255
        or value.strip() != value
        or any(ord(character) < 0x20 or ord(character) > 0x7E for character in value)
    ):
        fail(f"{label} must be 1 to 255 printable ASCII bytes without surrounding whitespace")
    return value


def plain_file(path: Path, label: str, maximum: int | None = None) -> os.stat_result:
    try:
        metadata = path.lstat()
    except OSError as error:
        fail(f"cannot inspect {label}: {error}")
    if not stat.S_ISREG(metadata.st_mode) or path.is_symlink():
        fail(f"{label} must be a plain file")
    if maximum is not None and (metadata.st_size <= 0 or metadata.st_size > maximum):
        fail(f"{label} size is outside its bounded contract")
    return metadata


def plain_directory(path: Path, label: str) -> None:
    try:
        metadata = path.lstat()
    except OSError as error:
        fail(f"cannot inspect {label}: {error}")
    if not stat.S_ISDIR(metadata.st_mode) or path.is_symlink():
        fail(f"{label} must be a plain directory")


def load_json(path: Path, label: str, *, canonical: bool = False) -> tuple[Any, bytes]:
    metadata = plain_file(path, label, MAX_MANIFEST_BYTES)
    data = path.read_bytes()
    if len(data) != metadata.st_size:
        fail(f"{label} changed while being read")
    value = decode_json(data, label)
    if canonical and canonical_json(value) != data:
        fail(f"{label} is not canonical JSON")
    return value, data


def write_new(path: Path, data: bytes) -> None:
    if path.exists() or path.is_symlink():
        fail(f"output already exists: {path}")
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
    except BaseException:
        try:
            os.close(descriptor)
        except OSError:
            pass
        raise


def validate_run(
    value: Any,
    run_id: int,
    repository: str,
    workflow: str,
    label: str,
) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{label} metadata must be an object")
    if (
        value.get("id") != run_id
        or value.get("event") != "workflow_dispatch"
        or value.get("status") != "completed"
        or value.get("conclusion") != "success"
        or value.get("head_branch") != "main"
        or not isinstance(value.get("head_repository"), dict)
        or value["head_repository"].get("full_name") != repository
        or value.get("path") != workflow
        or COMMIT.fullmatch(value.get("head_sha", "")) is None
    ):
        fail(f"{label} is not one exact successful protected-main workflow run")
    return value


def unexpired_artifact_names(value: Any, label: str) -> list[str]:
    if not isinstance(value, dict) or not isinstance(value.get("artifacts"), list):
        fail(f"{label} artifact response is malformed")
    names = [
        item.get("name")
        for item in value["artifacts"]
        if isinstance(item, dict) and item.get("expired") is False
    ]
    if any(not isinstance(name, str) for name in names) or len(names) != len(set(names)):
        fail(f"{label} artifacts contain invalid or duplicate names")
    return names


def parse_lane(value: str) -> tuple[str, Path]:
    profile, separator, raw_path = value.partition("=")
    if not separator or profile not in PUBLIC_PROFILES or not raw_path:
        fail("--lane must use PROFILE=DIRECTORY for one canonical public profile")
    return profile, Path(raw_path)


def validate_artifact_binding(
    manifest: dict[str, Any],
    artifact_path: Path,
    evidence: dict[str, Any],
    evidence_key: str,
    artifact_name: str,
    label: str,
) -> dict[str, Any]:
    bound = evidence.get(evidence_key)
    if not isinstance(bound, dict):
        fail(f"{label} evidence binding is missing")
    manifest_bytes = canonical_json(manifest)
    manifest_sha256 = sha256_bytes(manifest_bytes)
    metadata = plain_file(artifact_path, f"{label} artifact")
    artifact_sha256 = hash_file(artifact_path)
    if (
        bound.get("schema_version") != manifest.get("schema_version")
        or bound.get("manifest_sha256") != manifest_sha256
        or not isinstance(bound.get("artifact"), dict)
        or bound["artifact"].get("name") != artifact_name
        or bound["artifact"].get("sha256") != artifact_sha256
        or bound["artifact"].get("size") != metadata.st_size
        or bound["artifact"].get("counts") != manifest.get("artifact", {}).get("counts")
        or manifest.get("artifact", {}).get("sha256") != artifact_sha256
        or manifest.get("artifact", {}).get("size") != metadata.st_size
    ):
        fail(f"{label} evidence, manifest, and artifact bindings disagree")
    return {
        "manifest_sha256": manifest_sha256,
        "artifact": {
            "name": artifact_name,
            "sha256": artifact_sha256,
            "size": metadata.st_size,
        },
    }


def validate_resolution_implementation(value: Any, label: str) -> dict[str, Any]:
    evidence = exact_object(
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
    if exact_u32(evidence["schema_version"], f"{label} schema_version") != 1:
        fail(f"{label} schema_version is unsupported")
    workers = exact_positive_int(evidence["workers"], f"{label} workers")
    loads = evidence["worker_load_milliseconds"]
    if not isinstance(loads, list) or len(loads) != workers:
        fail(f"{label} worker load count differs from workers")
    for index, load in enumerate(loads):
        exact_nonnegative_int(load, f"{label} worker load {index}")
    exact_positive_int(evidence["memory_budget_bytes"], f"{label} memory budget")
    exact_positive_int(
        evidence["measured_worker_rss_bytes"], f"{label} measured worker RSS"
    )
    return evidence


def validate_lane(
    root: Path,
    profile: str,
    assembly_lane: dict[str, Any],
    export_id: str,
    deployed_commit: str,
    input_manifest_sha256: str,
    candidate_sha256: str,
    deployment_run_id: int,
    export_run_id: int,
    transport_sha256: str,
) -> tuple[dict[str, Any], list[tuple[str, Path]]]:
    plain_directory(root, f"{profile} lane")
    if sorted(path.name for path in root.iterdir()) != [
        "evidence.json",
        "package-oracle",
        "resolution-oracle",
    ]:
        fail(f"{profile} lane has missing or unexpected entries")
    evidence_value, evidence_bytes = load_json(
        root / "evidence.json", f"{profile} lane evidence", canonical=True
    )
    evidence = exact_object(
        evidence_value,
        {
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
        },
        f"{profile} lane evidence",
    )
    architecture = PROFILE_ARCHITECTURES[profile]
    producer_commit = require_commit(evidence.get("producer_commit"), f"{profile} producer commit")
    binaries = exact_object(
        evidence.get("producer_binaries"), {"package", "resolution"}, f"{profile} producers"
    )
    package_binary, resolution_binary = PROFILE_PRODUCER_BINARIES[profile]
    for kind, expected_name in (("package", package_binary), ("resolution", resolution_binary)):
        binary = exact_object(
            binaries.get(kind), {"name", "sha256"}, f"{profile} {kind} producer"
        )
        if binary["name"] != expected_name:
            fail(f"{profile} {kind} producer name drifted")
        require_sha256(binary["sha256"], f"{profile} {kind} producer digest")
    validate_resolution_implementation(
        evidence["resolution_implementation"], f"{profile} resolution implementation"
    )
    if (
        exact_u32(evidence["schema_version"], f"{profile} lane schema_version") != 5
        or evidence["artifact_type"] != "native-oracle-lane"
        or evidence["deployment_run_id"] != deployment_run_id
        or evidence["export_run_id"] != export_run_id
        or evidence["export_id"] != export_id
        or evidence["transport_sha256"] != transport_sha256
        or evidence["deployed_commit"] != deployed_commit
        or evidence["input_manifest_sha256"] != input_manifest_sha256
        or evidence["profile"] != profile
        or evidence["profile_revision_sha256"] != candidate_sha256
        or evidence["target_architecture"] != architecture
    ):
        fail(f"{profile} lane differs from its export and deployment authority")

    expected_assembly_keys = {
        "profile",
        "profile_revision_sha256",
        "target_architecture",
        "lane_image",
        "producer_commit",
        "producer_binaries",
        "lane_evidence_sha256",
        "package_oracle",
        "resolution_oracle",
        "github_artifact",
    }
    assembly_lane = exact_object(
        assembly_lane, expected_assembly_keys, f"{profile} assembled lane"
    )
    github_artifact = exact_object(
        assembly_lane["github_artifact"],
        {"artifact_id", "run_id", "name", "sha256"},
        f"{profile} assembled GitHub artifact",
    )
    exact_positive_int(github_artifact["artifact_id"], f"{profile} artifact id")
    exact_positive_int(github_artifact["run_id"], f"{profile} artifact run id")
    require_sha256(github_artifact["sha256"], f"{profile} artifact digest")
    if github_artifact["name"] != f"remi-native-oracle-lane-{profile}-{export_id}-{producer_commit}":
        fail(f"{profile} assembled artifact name drifted")
    if (
        assembly_lane["profile"] != profile
        or assembly_lane["profile_revision_sha256"] != candidate_sha256
        or assembly_lane["target_architecture"] != architecture
        or assembly_lane["lane_image"] != evidence["lane_image"]
        or assembly_lane["producer_commit"] != producer_commit
        or assembly_lane["producer_binaries"] != binaries
        or assembly_lane["lane_evidence_sha256"] != sha256_bytes(evidence_bytes)
        or assembly_lane["package_oracle"] != evidence["package_oracle"]
        or assembly_lane["resolution_oracle"] != evidence["resolution_oracle"]
    ):
        fail(f"{profile} lane differs from the authenticated three-lane assembly")

    package_root = root / "package-oracle"
    resolution_root = root / "resolution-oracle"
    for directory, entries, label in (
        (package_root, ["manifest.json", "packages.jsonl"], "package oracle"),
        (resolution_root, ["manifest.json", "roots.jsonl"], "resolution oracle"),
    ):
        plain_directory(directory, f"{profile} {label}")
        if sorted(path.name for path in directory.iterdir()) != entries:
            fail(f"{profile} {label} has missing or unexpected entries")

    package_value, _ = load_json(
        package_root / "manifest.json", f"{profile} package manifest", canonical=True
    )
    resolution_value, _ = load_json(
        resolution_root / "manifest.json", f"{profile} resolution manifest", canonical=True
    )
    if not isinstance(package_value, dict) or not isinstance(resolution_value, dict):
        fail(f"{profile} oracle manifest must be an object")
    require_envelope_schema(resolution_value, 3, "native resolution bundle")
    if (
        exact_u32(package_value.get("schema_version"), f"{profile} package schema_version") != 1
        or package_value.get("profile") != profile
        or package_value.get("profile_revision_sha256") != candidate_sha256
        or exact_u32(
            resolution_value.get("schema_version"), f"{profile} resolution schema_version"
        )
        != 3
        or resolution_value.get("profile") != profile
        or resolution_value.get("profile_revision_sha256") != candidate_sha256
        or resolution_value.get("policy", {}).get("architecture") != architecture
    ):
        fail(f"{profile} oracle manifest identity drifted")
    package = validate_artifact_binding(
        package_value,
        package_root / "packages.jsonl",
        evidence,
        "package_oracle",
        "packages.jsonl",
        f"{profile} package oracle",
    )
    resolution = validate_artifact_binding(
        resolution_value,
        resolution_root / "roots.jsonl",
        evidence,
        "resolution_oracle",
        "roots.jsonl",
        f"{profile} resolution oracle",
    )
    if resolution_value.get("package_oracle_manifest_sha256") != package["manifest_sha256"]:
        fail(f"{profile} resolution oracle is not bound to its package oracle")
    resolution["package_oracle_manifest_sha256"] = package["manifest_sha256"]

    files = [
        (f"{profile}/package-oracle/manifest.json", package_root / "manifest.json"),
        (f"{profile}/package-oracle/packages.jsonl", package_root / "packages.jsonl"),
        (f"{profile}/native-resolution/manifest.json", resolution_root / "manifest.json"),
        (f"{profile}/native-resolution/roots.jsonl", resolution_root / "roots.jsonl"),
    ]
    return (
        {
            "profile": profile,
            "profile_revision_sha256": candidate_sha256,
            "target_architecture": architecture,
            "input_manifest_sha256": input_manifest_sha256,
            "package_oracle": package,
            "native_resolution": resolution,
        },
        files,
    )


def tar_add_plain(archive: tarfile.TarFile, source: Path, name: str) -> None:
    metadata = plain_file(source, f"transport source {name}")
    info = tarfile.TarInfo(name)
    info.size = metadata.st_size
    info.mode = 0o400
    info.uid = 0
    info.gid = 0
    info.uname = "root"
    info.gname = "root"
    info.mtime = 0
    with source.open("rb") as stream:
        archive.addfile(info, stream)


def validate_export_operator(
    export_root: Path,
    export_run: dict[str, Any],
    export_run_id: int,
    export_id: str,
) -> dict[str, Any]:
    value, data = load_json(
        export_root / "native-oracle-export-operator-v1.json",
        "native-oracle export operator attestation",
        canonical=True,
    )
    attestation = exact_object(
        value,
        {
            "schema_version",
            "export_id",
            "workflow_commit_sha",
            "workflow_run_id",
            "workflow_run_attempt",
            "ssh_host_key_contract",
        },
        "native-oracle export operator attestation",
    )
    run_attempt = export_run.get("run_attempt")
    if (
        exact_u32(attestation["schema_version"], "export operator schema_version") != 1
        or attestation["export_id"] != export_id
        or attestation["workflow_commit_sha"] != export_run["head_sha"]
        or attestation["workflow_run_id"] != export_run_id
        or not isinstance(run_attempt, int)
        or isinstance(run_attempt, bool)
        or run_attempt <= 0
        or attestation["workflow_run_attempt"] != run_attempt
        or attestation["ssh_host_key_contract"] != "protected-pinned-known-hosts-v1"
    ):
        fail("export run lacks its exact pinned SSH operator attestation")
    return {
        "schema_version": 1,
        "workflow_commit_sha": attestation["workflow_commit_sha"],
        "workflow_run_id": export_run_id,
        "workflow_run_attempt": run_attempt,
        "attestation_sha256": sha256_bytes(data),
    }


def build_input(args: argparse.Namespace) -> None:
    survey_id = require_identity(args.survey_id, "survey id")
    oracle_id = require_run_id(args.oracle_run_id, "oracle run id")
    workflow_commit = require_commit(args.workflow_commit, "survey workflow commit")
    repository = args.repository
    if not isinstance(repository, str) or re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository) is None:
        fail("repository must be one explicit owner/name")

    oracle_run, _ = load_json(args.oracle_run, "oracle run metadata")
    validate_run(
        oracle_run,
        oracle_id,
        repository,
        ".github/workflows/produce-remi-native-oracles.yml",
        "oracle run",
    )
    oracle_attempt = exact_positive_int(
        oracle_run.get("run_attempt"), "oracle run attempt"
    )
    if oracle_run["head_sha"] != workflow_commit:
        fail("oracle run does not use the survey's exact current-main operator")
    oracle_artifacts, _ = load_json(args.oracle_artifacts, "oracle artifact metadata")
    oracle_names = unexpired_artifact_names(oracle_artifacts, "oracle run")
    assembly_value, assembly_bytes = load_json(
        args.assembly_evidence, "native-oracle three-lane assembly", canonical=True
    )
    assembly = exact_object(
        assembly_value,
        {
            "schema_version",
            "artifact_type",
            "deployment_run_id",
            "export_run_id",
            "export_id",
            "transport_sha256",
            "deployed_commit",
            "input_manifest_sha256",
            "lanes",
        },
        "native-oracle three-lane assembly",
    )
    export_id = require_identity(assembly.get("export_id"), "assembled export identity")
    export_id_run = exact_positive_int(assembly.get("export_run_id"), "assembled export run")
    deployment_id = exact_positive_int(
        assembly.get("deployment_run_id"), "assembled deployment run"
    )
    assembled_deployed_commit = require_commit(
        assembly.get("deployed_commit"), "assembled deployed commit"
    )
    assembled_transport_sha256 = require_sha256(
        assembly.get("transport_sha256"), "assembled export transport"
    )
    assembled_input_manifest_sha256 = require_sha256(
        assembly.get("input_manifest_sha256"), "assembled input manifest"
    )
    assembled_lanes = assembly.get("lanes")
    if (
        exact_u32(assembly["schema_version"], "oracle assembly schema_version") != 2
        or assembly["artifact_type"] != "native-oracle-three-lane-set"
        or not isinstance(assembled_lanes, list)
        or len(assembled_lanes) != len(PUBLIC_PROFILES)
        or [item.get("profile") for item in assembled_lanes if isinstance(item, dict)]
        != list(PUBLIC_PROFILES)
    ):
        fail("oracle run does not provide one canonical assembled three-lane set")
    assembly_artifact_name = f"remi-native-oracle-set-{export_id}-{oracle_id}"
    if oracle_names.count(assembly_artifact_name) != 1:
        fail("oracle run must retain its exact assembled three-lane artifact")
    if export_id_run != require_run_id(args.export_run_id, "export run id"):
        fail("requested export run differs from the oracle assembly binding")

    export_run, _ = load_json(args.export_run, "export run metadata")
    validate_run(
        export_run,
        export_id_run,
        repository,
        ".github/workflows/export-remi-native-oracle-inputs.yml",
        "export run",
    )
    export_artifacts, _ = load_json(args.export_artifacts, "export artifact metadata")
    export_names = unexpired_artifact_names(export_artifacts, "export run")
    export_pattern = re.compile(
        rf"^remi-native-oracle-input-([1-9][0-9]*)-{export_id_run}$"
    )
    export_matches = [match for name in export_names if (match := export_pattern.fullmatch(name))]
    if len(export_matches) != 1 or len(export_names) != 1:
        fail("export run must retain exactly one exact native-oracle handoff")
    export_deployment_id = int(export_matches[0].group(1))
    if export_deployment_id != deployment_id:
        fail("oracle assembly deployment differs from the export artifact binding")
    if deployment_id != require_run_id(args.deployment_run_id, "deployment run id"):
        fail("requested deployment run differs from the export artifact binding")

    deployment_run, _ = load_json(args.deployment_run, "deployment run metadata")
    validate_run(
        deployment_run,
        deployment_id,
        repository,
        ".github/workflows/deploy-remi-candidate.yml",
        "deployment run",
    )

    export_root = args.export_root
    plain_directory(export_root, "export artifact")
    verification_value, _ = load_json(
        export_root / "native-oracle-input-verification.json",
        "export verification",
        canonical=True,
    )
    verification = exact_object(
        verification_value,
        {"schema_version", "export_id", "transport", "manifest", "profiles", "counts"},
        "export verification",
    )
    inspection, _ = load_json(
        export_root / "remi-deployment-inspection.json", "deployment inspection"
    )
    if not isinstance(inspection, dict) or not isinstance(inspection.get("deployment"), dict):
        fail("deployment inspection is malformed")
    deployment = inspection["deployment"]
    deployed_commit = require_commit(deployment.get("commit_sha"), "deployed commit")
    binary_sha256 = require_sha256(deployment.get("binary_sha256"), "deployed binary")
    if (
        exact_u32(
            inspection.get("deployment_evidence_schema_version"),
            "deployment evidence schema_version",
        )
        != 3
        or deployment.get("completion_mode") != "private-candidates"
        or deployment.get("outcome") != "complete"
        or deployment.get("failure_phase") is not None
        or exact_u32(
            verification.get("schema_version"), "export verification schema_version"
        )
        != 1
    ):
        fail("export evidence does not prove a complete private-candidate deployment")
    verified_export_id = require_identity(verification.get("export_id"), "export identity")
    if verified_export_id != export_id:
        fail("oracle assembly export identity differs from export verification")
    export_operator = validate_export_operator(
        export_root, export_run, export_id_run, export_id
    )
    input_manifest_sha256 = require_sha256(
        verification.get("manifest", {}).get("sha256"), "export input manifest"
    )
    export_transport_sha256 = require_sha256(
        verification.get("transport", {}).get("sha256"), "export transport"
    )
    if (
        deployed_commit != assembled_deployed_commit
        or input_manifest_sha256 != assembled_input_manifest_sha256
        or export_transport_sha256 != assembled_transport_sha256
    ):
        fail("oracle assembly differs from authenticated export or deployment evidence")
    candidates = inspection.get("candidates")
    verified_profiles = verification.get("profiles")
    if (
        not isinstance(candidates, list)
        or [item.get("profile") for item in candidates if isinstance(item, dict)]
        != list(PUBLIC_PROFILES)
        or not isinstance(verified_profiles, list)
        or [item.get("profile") for item in verified_profiles if isinstance(item, dict)]
        != list(PUBLIC_PROFILES)
    ):
        fail("export evidence does not retain canonical public-profile order")
    candidate_digests: dict[str, str] = {}
    for candidate, verified in zip(candidates, verified_profiles, strict=True):
        profile = candidate["profile"]
        digest = require_sha256(
            candidate.get("profile_revision_sha256"), f"{profile} deployed candidate"
        )
        if verified.get("profile_revision_sha256") != digest:
            fail(f"{profile} export verification differs from deployment candidate")
        candidate_digests[profile] = digest

    lane_arguments = dict(parse_lane(value) for value in args.lane)
    if tuple(lane_arguments) != PUBLIC_PROFILES or len(args.lane) != 3:
        fail("lanes must be supplied once in canonical Fedora, Ubuntu, Arch order")
    profiles: list[dict[str, Any]] = []
    files: list[tuple[str, Path]] = []
    assembly_by_profile = {item["profile"]: item for item in assembled_lanes}
    for profile in PUBLIC_PROFILES:
        profile_binding, profile_files = validate_lane(
            lane_arguments[profile],
            profile,
            assembly_by_profile[profile],
            export_id,
            deployed_commit,
            input_manifest_sha256,
            candidate_digests[profile],
            deployment_id,
            export_id_run,
            export_transport_sha256,
        )
        profiles.append(profile_binding)
        files.extend(profile_files)

    file_inventory = []
    for name, path in files:
        metadata = plain_file(path, f"oracle transport member {name}")
        file_inventory.append(
            {"path": name, "sha256": hash_file(path), "size": metadata.st_size}
        )
    manifest = {
        "schema_version": INPUT_MANIFEST_SCHEMA,
        "survey_id": survey_id,
        "export_id": export_id,
        "workflow_runs": {
            "oracle": oracle_id,
            "export": export_id_run,
            "deployment": deployment_id,
        },
        "deployment": {
            "commit_sha": deployed_commit,
            "binary_sha256": binary_sha256,
        },
        "profiles": profiles,
        "files": file_inventory,
    }
    manifest_bytes = canonical_json(manifest)
    if args.output.exists() or args.output.is_symlink():
        fail("oracle transport output already exists")
    temporary = args.output.with_name(f".{args.output.name}.next-{os.getpid()}")
    if temporary.exists() or temporary.is_symlink():
        fail("oracle transport temporary output already exists")
    manifest_path = args.output.with_name(f".{args.output.name}.manifest-{os.getpid()}")
    try:
        write_new(manifest_path, manifest_bytes)
        with tarfile.open(
            temporary, mode="w", format=ORACLE_TRANSPORT_TAR_FORMAT
        ) as archive:
            tar_add_plain(archive, manifest_path, "manifest.json")
            for name, path in files:
                tar_add_plain(archive, path, name)
                if args.consume_lane_files:
                    path.unlink()
        os.chmod(temporary, 0o600)
        os.replace(temporary, args.output)
    finally:
        temporary.unlink(missing_ok=True)
        manifest_path.unlink(missing_ok=True)

    evidence = {
        "schema_version": INPUT_EVIDENCE_SCHEMA,
        "survey_id": survey_id,
        "export_id": export_id,
        "workflow_runs": manifest["workflow_runs"],
        "oracle_operator": {
            "workflow_commit_sha": workflow_commit,
            "workflow_run_id": oracle_id,
            "workflow_run_attempt": oracle_attempt,
        },
        "oracle_assembly": {"sha256": sha256_bytes(assembly_bytes)},
        "export_operator": export_operator,
        "deployment": manifest["deployment"],
        "profiles": [
            {
                "profile": item["profile"],
                "profile_revision_sha256": item["profile_revision_sha256"],
                "target_architecture": item["target_architecture"],
                "package_oracle_manifest_sha256": item["package_oracle"]["manifest_sha256"],
                "native_resolution_manifest_sha256": item["native_resolution"]["manifest_sha256"],
            }
            for item in profiles
        ],
        "manifest_sha256": sha256_bytes(manifest_bytes),
        "transport": {
            "sha256": hash_file(args.output),
            "size": plain_file(args.output, "oracle transport").st_size,
        },
    }
    write_new(args.evidence, canonical_json(evidence))
    print(canonical_json(evidence).decode())


def safe_member_name(name: str) -> str:
    path = PurePosixPath(name)
    if (
        not name
        or name.startswith("/")
        or name.endswith("/")
        or path.as_posix() != name
        or any(part in {"", ".", ".."} for part in path.parts)
    ):
        fail(f"survey transport contains unsafe member {name!r}")
    return name


def read_tar_member(archive: tarfile.TarFile, member: tarfile.TarInfo, maximum: int) -> bytes:
    if not member.isreg() or member.size <= 0 or member.size > maximum:
        fail(f"survey transport member {member.name!r} is not a bounded plain file")
    stream = archive.extractfile(member)
    if stream is None:
        fail(f"survey transport member {member.name!r} cannot be read")
    data = stream.read(member.size + 1)
    if len(data) != member.size:
        fail(f"survey transport member {member.name!r} changed size")
    return data


def copy_tar_member(
    archive: tarfile.TarFile,
    member: tarfile.TarInfo,
    destination: Path,
    expected_size: int,
    expected_sha256: str,
) -> None:
    if not member.isreg() or member.size <= 0 or member.size != expected_size:
        fail(f"survey transport member {member.name!r} is not the declared plain file")
    stream = archive.extractfile(member)
    if stream is None:
        fail(f"survey transport member {member.name!r} cannot be read")
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = os.open(destination, flags, 0o600)
    digest = hashlib.sha256()
    copied = 0
    try:
        with os.fdopen(descriptor, "wb") as output:
            while chunk := stream.read(1024 * 1024):
                copied += len(chunk)
                if copied > expected_size:
                    fail(f"survey transport member {member.name!r} exceeded its declared size")
                digest.update(chunk)
                output.write(chunk)
    except BaseException:
        destination.unlink(missing_ok=True)
        raise
    if copied != expected_size:
        destination.unlink(missing_ok=True)
        fail(f"survey transport member {member.name!r} changed size")
    if digest.hexdigest() != expected_sha256:
        destination.unlink(missing_ok=True)
        fail(f"survey file {member.name} changed digest")


def copy_declared_survey_member(
    transport: Path,
    name: str,
    destination: Path,
    expected_size: int,
    expected_sha256: str,
) -> None:
    try:
        archive = tarfile.open(transport, mode="r:")
    except (OSError, tarfile.TarError) as error:
        fail(f"survey transport cannot be reopened: {error}")
    with archive:
        try:
            member = archive.getmember(name)
        except KeyError:
            fail(f"survey file {name} disappeared while reopening its transport")
        copy_tar_member(
            archive, member, destination, expected_size, expected_sha256
        )


def scan_json_string_end(data: mmap.mmap, start: int, label: str) -> int:
    if start >= len(data) or data[start] != ord('"'):
        fail(f"{label} expected a JSON string")
    position = start + 1
    hexadecimal = b"0123456789abcdefABCDEF"
    while position < len(data):
        byte = data[position]
        if byte == ord('"'):
            return position + 1
        if byte < 0x20:
            fail(f"{label} contains a control byte in a JSON string")
        if byte == ord("\\"):
            position += 1
            if position >= len(data):
                fail(f"{label} has an unterminated JSON escape")
            escape = data[position]
            if escape == ord("u"):
                end = position + 5
                if end > len(data) or any(
                    value not in hexadecimal for value in data[position + 1 : end]
                ):
                    fail(f"{label} has an invalid Unicode escape")
                position = end
                continue
            if escape not in b'"\\/bfnrt':
                fail(f"{label} has an invalid JSON escape")
        position += 1
    fail(f"{label} has an unterminated JSON string")


def scan_json_value_end(data: mmap.mmap, start: int, label: str) -> int:
    if start >= len(data):
        fail(f"{label} has a missing JSON value")
    first = data[start]
    if first == ord('"'):
        return scan_json_string_end(data, start, label)
    if first in (ord("{"), ord("[")):
        stack = [ord("}") if first == ord("{") else ord("]")]
        position = start + 1
        while position < len(data):
            byte = data[position]
            if byte in b" \t\r\n":
                fail(f"{label} is not canonical JSON")
            if byte == ord('"'):
                position = scan_json_string_end(data, position, label)
                continue
            if byte in (ord("{"), ord("[")):
                stack.append(ord("}") if byte == ord("{") else ord("]"))
            elif byte in (ord("}"), ord("]")):
                if byte != stack[-1]:
                    fail(f"{label} has mismatched JSON delimiters")
                stack.pop()
                if not stack:
                    return position + 1
            position += 1
        fail(f"{label} has an unterminated JSON value")
    position = start
    while position < len(data) and data[position] not in b",]}":
        if data[position] in b" \t\r\n":
            fail(f"{label} is not canonical JSON")
        position += 1
    if position == start:
        fail(f"{label} has a missing JSON value")
    return position


def decode_canonical_fragment(data: mmap.mmap, start: int, end: int, label: str) -> Any:
    fragment = data[start:end]
    value = decode_json(fragment, label)
    if canonical_json(value) != fragment:
        fail(f"{label} is not canonical JSON")
    return value


class StreamingJsonArray:
    """Canonical JSON array decoded one element at a time from a read-only mapping."""

    def __init__(self, data: mmap.mmap, start: int, end: int, label: str) -> None:
        self.data = data
        self.start = start
        self.end = end
        self.label = label
        self._length: int | None = None

    def spans(self) -> Iterator[tuple[int, int, int]]:
        position = self.start + 1
        index = 0
        if position >= self.end or self.data[position] == ord("]"):
            if position + 1 != self.end:
                fail(f"{self.label} is not one canonical JSON array")
            return
        while position < self.end:
            item_end = scan_json_value_end(
                self.data, position, f"{self.label} element {index}"
            )
            yield position, item_end, index
            index += 1
            if item_end >= self.end:
                fail(f"{self.label} has an unterminated JSON array")
            delimiter = self.data[item_end]
            if delimiter == ord("]"):
                if item_end + 1 != self.end:
                    fail(f"{self.label} has trailing JSON data")
                return
            if delimiter != ord(",") or item_end + 1 >= self.end:
                fail(f"{self.label} has an invalid JSON array delimiter")
            position = item_end + 1
        fail(f"{self.label} has an unterminated JSON array")

    def __iter__(self) -> Iterator[Any]:
        for start, end, index in self.spans():
            yield decode_canonical_fragment(
                self.data, start, end, f"{self.label} element {index}"
            )

    def objects(self, stream_spec: dict[str, Any]) -> Iterator["StreamingJsonObject"]:
        for start, end, index in self.spans():
            yield StreamingJsonObject(
                self.data,
                start,
                end,
                f"{self.label} element {index}",
                stream_spec,
            )

    def __len__(self) -> int:
        if self._length is None:
            self._length = sum(1 for _ in self.spans())
        return self._length


class StreamingJsonObject:
    """Canonical JSON object with selected nested values left as mapped views."""

    def __init__(
        self,
        data: mmap.mmap,
        start: int,
        end: int,
        label: str,
        stream_spec: dict[str, Any],
    ) -> None:
        self.data = data
        self.start = start
        self.end = end
        self.label = label
        self.spans: dict[str, tuple[int, int]] = {}
        self.value = self.parse(stream_spec)

    def parse(self, stream_spec: dict[str, Any]) -> dict[str, Any]:
        if self.start >= self.end or self.data[self.start] != ord("{"):
            fail(f"{self.label} must be one canonical JSON object")
        position = self.start + 1
        result: dict[str, Any] = {}
        previous_key: str | None = None
        if position < self.end and self.data[position] == ord("}"):
            if position + 1 != self.end:
                fail(f"{self.label} has trailing JSON data")
            return result
        while position < self.end:
            key_end = scan_json_string_end(self.data, position, self.label)
            key = decode_canonical_fragment(
                self.data, position, key_end, f"{self.label} key"
            )
            if not isinstance(key, str) or (
                previous_key is not None and key <= previous_key
            ):
                fail(f"{self.label} keys are repeated or noncanonical")
            previous_key = key
            if key_end >= self.end or self.data[key_end] != ord(":"):
                fail(f"{self.label} has a malformed object member")
            value_start = key_end + 1
            value_end = scan_json_value_end(
                self.data, value_start, f"{self.label}.{key}"
            )
            self.spans[key] = (value_start, value_end)
            specification = stream_spec.get(key)
            if specification == "array":
                if self.data[value_start] != ord("["):
                    fail(f"{self.label}.{key} must be an array")
                result[key] = StreamingJsonArray(
                    self.data, value_start, value_end, f"{self.label}.{key}"
                )
            elif isinstance(specification, dict):
                result[key] = StreamingJsonObject(
                    self.data,
                    value_start,
                    value_end,
                    f"{self.label}.{key}",
                    specification,
                ).value
            elif specification == "skip":
                result[key] = None
            else:
                result[key] = decode_canonical_fragment(
                    self.data, value_start, value_end, f"{self.label}.{key}"
                )
            if value_end >= self.end:
                fail(f"{self.label} has an unterminated JSON object")
            delimiter = self.data[value_end]
            if delimiter == ord("}"):
                if value_end + 1 != self.end:
                    fail(f"{self.label} has trailing JSON data")
                return result
            if delimiter != ord(",") or value_end + 1 >= self.end:
                fail(f"{self.label} has an invalid JSON object delimiter")
            position = value_end + 1
        fail(f"{self.label} has an unterminated JSON object")


class StreamingJsonLines:
    """Canonical JSON-lines records over one authenticated mapped extent."""

    def __init__(
        self, data: mmap.mmap, start: int, size: int, label: str
    ) -> None:
        self.data = data
        self.start = start
        self.end = start + size
        self.label = label
        if start < 0 or size <= 0 or self.end > len(data):
            fail(f"{label} lies outside its authenticated archive")

    def objects(self, stream_spec: dict[str, Any]) -> Iterator[StreamingJsonObject]:
        position = self.start
        index = 0
        while position < self.end:
            newline = self.data.find(b"\n", position, self.end)
            if newline < 0 or newline == position:
                fail(f"{self.label} has an empty or unterminated row")
            if scan_json_value_end(
                self.data, position, f"{self.label} row {index}"
            ) != newline:
                fail(f"{self.label} row {index} is not one canonical JSON value")
            yield StreamingJsonObject(
                self.data,
                position,
                newline,
                f"{self.label} row {index}",
                stream_spec,
            )
            position = newline + 1
            index += 1


class StreamingJsonDocument:
    """A canonical top-level object with selected arrays represented as streams."""

    def __init__(self, path: Path, label: str, streamed_keys: set[str]) -> None:
        self.stream = path.open("rb")
        self.mapping: mmap.mmap | None = None
        try:
            if os.fstat(self.stream.fileno()).st_size == 0:
                fail(f"{label} is empty")
            self.mapping = mmap.mmap(self.stream.fileno(), 0, access=mmap.ACCESS_READ)
            self.value = StreamingJsonObject(
                self.mapping,
                0,
                len(self.mapping),
                label,
                {key: "array" for key in streamed_keys},
            ).value
        except BaseException:
            self.close()
            raise

    def close(self) -> None:
        if self.mapping is not None:
            self.mapping.close()
            self.mapping = None
        self.stream.close()

    def __enter__(self) -> dict[str, Any]:
        return self.value

    def __exit__(self, *_: Any) -> None:
        self.close()


def require_optional_string(value: Any, label: str) -> str | None:
    if value is not None and not isinstance(value, str):
        fail(f"{label} must be a string or null")
    return value


def validate_implementation(value: Any, profile: str, label: str) -> None:
    implementation = exact_object(
        value, {"ecosystem", "name", "version", "projection_schema"}, label
    )
    if implementation["ecosystem"] not in NATIVE_ECOSYSTEMS:
        fail(f"{label}.ecosystem is unsupported")
    require_rust_identity(implementation["name"], f"{label}.name")
    require_rust_identity(implementation["version"], f"{label}.version")
    exact_u32(implementation["projection_schema"], f"{label}.projection_schema", positive=True)
    if (
        implementation["ecosystem"] != PROFILE_ECOSYSTEMS[profile]
        or implementation["name"] != "conary-sat"
        or implementation["projection_schema"] != 3
    ):
        fail(f"{label} differs from the fixed Conary candidate producer")


def validate_policy(value: Any, architecture: str, label: str) -> None:
    policy = exact_object(
        value,
        {
            "architecture", "architecture_admission", "installed_state", "roots",
            "positive_requirements", "provider_selection",
        },
        label,
    )
    require_rust_identity(policy["architecture"], f"{label}.architecture")
    if policy != {
        "architecture": architecture,
        "architecture_admission": "native_only",
        "installed_state": "empty",
        "roots": "every_exact_package",
        "positive_requirements": "required_only",
        "provider_selection": "native_precedence",
    }:
        fail(f"{label} differs from the fixed native-resolution policy")


NATIVE_OUTCOME_STREAM_SPEC = {
    "closure_package_keys_sha256": "array",
    "dependencies": "array",
}
CANDIDATE_ROOT_STREAM_SPEC = {"outcome": NATIVE_OUTCOME_STREAM_SPEC}
CANDIDATE_FAILURE_COVERAGE_STREAM_SPEC = {
    "error_kind": "skip",
    "error_message": "skip",
    "native_explanation": "skip",
}
COMPARISON_MISMATCH_STREAM_SPEC = {
    "oracle": {"outcome": NATIVE_OUTCOME_STREAM_SPEC},
    "candidate": {"outcome": NATIVE_OUTCOME_STREAM_SPEC},
}


def stream_objects(
    value: list[Any] | StreamingJsonArray,
    stream_spec: dict[str, Any],
) -> Iterator[dict[str, Any]]:
    if isinstance(value, StreamingJsonArray):
        for item in value.objects(stream_spec):
            yield item.value
    else:
        yield from value


def validate_native_outcome(value: Any, root_sha256: str, label: str) -> str:
    if not isinstance(value, dict):
        fail(f"{label} must be a typed native-resolution outcome")
    status = value.get("status")
    if status == "resolved":
        outcome = exact_object(value, {"status", "closure_package_keys_sha256"}, label)
        closure = outcome["closure_package_keys_sha256"]
        if not isinstance(closure, (list, StreamingJsonArray)) or len(closure) == 0:
            fail(f"{label} resolved closure must be nonempty")
        previous = ""
        contains_root = False
        for index, digest_value in enumerate(closure):
            digest_value = require_sha256(digest_value, f"{label} closure {index}")
            if digest_value <= previous:
                fail(f"{label} resolved closure is noncanonical or omits its root")
            previous = digest_value
            contains_root = contains_root or digest_value == root_sha256
        if not contains_root:
            fail(f"{label} resolved closure is noncanonical or omits its root")
    elif status == "unresolved":
        outcome = exact_object(value, {"status", "dependencies"}, label)
        dependencies = outcome["dependencies"]
        if not isinstance(dependencies, (list, StreamingJsonArray)) or len(dependencies) == 0:
            fail(f"{label} unresolved dependencies must be nonempty")
        previous_key: tuple[str, str] | None = None
        for index, item in enumerate(dependencies):
            dependency = exact_object(
                item,
                {"requiring_package_key_sha256", "requirement_group_sha256"},
                f"{label} dependency {index}",
            )
            key = (
                require_sha256(
                    dependency["requiring_package_key_sha256"],
                    f"{label} dependency {index} package",
                ),
                require_sha256(
                    dependency["requirement_group_sha256"],
                    f"{label} dependency {index} requirement",
                ),
            )
            if previous_key is not None and key <= previous_key:
                fail(f"{label} unresolved dependencies are noncanonical")
            previous_key = key
    elif status == "not_installable":
        outcome = exact_object(value, {"status", "reason"}, label)
        if outcome["reason"] not in ("architecture_excluded", "conflicting_closure"):
            fail(f"{label} not-installable reason is unsupported")
    else:
        fail(f"{label} has an unsupported outcome status")
    return status


def update_native_outcome_digest(digest: Any, outcome: dict[str, Any]) -> None:
    status = outcome["status"]
    if status == "resolved":
        digest.update(b'{"closure_package_keys_sha256":[')
        for index, value in enumerate(outcome["closure_package_keys_sha256"]):
            if index:
                digest.update(b",")
            digest.update(canonical_json(value))
        digest.update(b'],"status":"resolved"}')
    elif status == "unresolved":
        digest.update(b'{"dependencies":[')
        for index, value in enumerate(outcome["dependencies"]):
            if index:
                digest.update(b",")
            digest.update(canonical_json(value))
        digest.update(b'],"status":"unresolved"}')
    else:
        digest.update(b'{"reason":')
        digest.update(canonical_json(outcome["reason"]))
        digest.update(b',"status":"not_installable"}')


def native_outcome_sha256(outcome: dict[str, Any]) -> str:
    digest = hashlib.sha256()
    update_native_outcome_digest(digest, outcome)
    return digest.hexdigest()


class SizedSha256:
    def __init__(self) -> None:
        self.digest = hashlib.sha256()
        self.size = 0

    def update(self, value: bytes) -> None:
        self.digest.update(value)
        self.size += len(value)
        if self.size > U64_MAX:
            fail("reconstructed candidate artifact exceeds u64")

    def hexdigest(self) -> str:
        return self.digest.hexdigest()


def validate_error_kind(value: Any, label: str) -> tuple[int, int]:
    kind = exact_object(value, {"error_variant", "reason"}, label)
    try:
        variant = NATIVE_ERROR_VARIANTS.index(kind["error_variant"])
        reason = CONARY_ERROR_REASONS.index(kind["reason"])
    except (ValueError, TypeError):
        fail(f"{label} contains an unsupported error variant or reason")
    return variant, reason


def validate_solvable(value: Any, label: str) -> None:
    solvable = exact_object(
        value,
        {
            "package_key_sha256", "name", "version", "release", "architecture",
            "repository_name", "source_profile",
        },
        label,
    )
    digest_value = solvable["package_key_sha256"]
    if digest_value is not None:
        require_sha256(digest_value, f"{label}.package_key_sha256")
    for key in ("name", "version", "repository_name"):
        if not isinstance(solvable[key], str):
            fail(f"{label}.{key} must be a string")
    for key in ("release", "architecture", "source_profile"):
        require_optional_string(solvable[key], f"{label}.{key}")


def validate_version_set(value: Any, label: str) -> None:
    version_set = exact_object(value, {"name", "constraint"}, label)
    if not all(isinstance(version_set[key], str) for key in version_set):
        fail(f"{label} fields must be strings")


def validate_explanation(value: Any, label: str) -> tuple[bool, int]:
    if not isinstance(value, dict):
        fail(f"{label} must be a typed native explanation")
    source = value.get("source")
    if source == "withheld":
        explanation = exact_object(value, {"source", "reason"}, label)
        if explanation["reason"] not in {
            "evidence_budget_exhausted", "conflict_graph_unavailable"
        }:
            fail(f"{label} has an unsupported withholding reason")
        return explanation["reason"] == "evidence_budget_exhausted", len(canonical_json(value))
    if source != "resolvo_conflict_graph":
        fail(f"{label} has an unsupported explanation source")
    explanation = exact_object(
        value, {"source", "unresolved_edges", "conflict_edges", "excluded_nodes"}, label
    )
    for collection in ("unresolved_edges", "conflict_edges", "excluded_nodes"):
        if not isinstance(explanation[collection], list):
            fail(f"{label}.{collection} must be an array")
    for index, item in enumerate(explanation["unresolved_edges"]):
        edge = exact_object(
            item, {"requiring", "requirement", "version_sets"}, f"{label} unresolved edge {index}"
        )
        validate_solvable(edge["requiring"], f"{label} unresolved edge {index} requiring")
        if not isinstance(edge["requirement"], str) or not isinstance(edge["version_sets"], list):
            fail(f"{label} unresolved edge {index} fields are malformed")
        for set_index, version_set in enumerate(edge["version_sets"]):
            validate_version_set(version_set, f"{label} unresolved edge {index} set {set_index}")
    for index, item in enumerate(explanation["conflict_edges"]):
        edge = exact_object(item, {"from", "to", "conflict"}, f"{label} conflict edge {index}")
        validate_solvable(edge["from"], f"{label} conflict edge {index} from")
        validate_solvable(edge["to"], f"{label} conflict edge {index} to")
        conflict = edge["conflict"]
        if not isinstance(conflict, dict):
            fail(f"{label} conflict edge {index} kind is malformed")
        if conflict.get("kind") in {"locked", "forbid_multiple_instances"}:
            exact_object(conflict, {"kind"}, f"{label} conflict edge {index} kind")
        elif conflict.get("kind") == "constrains":
            exact_object(conflict, {"kind", "version_set"}, f"{label} conflict edge {index} kind")
            validate_version_set(conflict["version_set"], f"{label} conflict edge {index} set")
        else:
            fail(f"{label} conflict edge {index} kind is unsupported")
    for index, item in enumerate(explanation["excluded_nodes"]):
        node = exact_object(item, {"solvable", "reason", "message"}, f"{label} excluded node {index}")
        validate_solvable(node["solvable"], f"{label} excluded node {index} solvable")
        if node["reason"] != "missing_dependency_authority" or not isinstance(node["message"], str):
            fail(f"{label} excluded node {index} fields are malformed")
    return False, len(canonical_json(value))


def validate_candidate_survey(value: Any, profile: dict[str, Any], name: str) -> None:
    survey = exact_object(
        value,
        {
            "schema_version", "profile", "profile_revision_sha256",
            "package_oracle_manifest_sha256", "implementation", "policy",
            "target_architecture", "counts", "outcomes", "failure_record_limit",
            "total_failures", "retained_failures", "truncated", "evidence_byte_limit",
            "retained_evidence_bytes", "retained_explanations", "withheld_explanations",
            "truncated_evidence", "failures",
        },
        name,
    )
    architecture = profile["target_architecture"]
    if (
        exact_u32(survey["schema_version"], f"{name}.schema_version") != 2
        or survey["profile"] != profile["profile"]
        or survey["profile_revision_sha256"] != profile["profile_revision_sha256"]
        or survey["package_oracle_manifest_sha256"]
        != profile["package_oracle_manifest_sha256"]
        or survey["target_architecture"] != architecture
    ):
        fail(f"{name} identity or oracle binding drifted")
    require_rust_identity(survey["profile"], f"{name}.profile")
    require_sha256(survey["profile_revision_sha256"], f"{name}.profile_revision_sha256")
    require_sha256(
        survey["package_oracle_manifest_sha256"], f"{name}.package_oracle_manifest_sha256"
    )
    require_rust_identity(survey["target_architecture"], f"{name}.target_architecture")
    validate_implementation(
        survey["implementation"], survey["profile"], f"{name}.implementation"
    )
    validate_policy(survey["policy"], architecture, f"{name}.policy")

    counts = exact_object(
        survey["counts"],
        {
            "roots_walked", "resolved_roots", "unresolved_roots", "not_installable_roots",
            "failed_roots", "error_kinds",
        },
        f"{name}.counts",
    )
    count_values = {
        key: exact_nonnegative_int(counts[key], f"{name}.counts.{key}")
        for key in (
            "roots_walked", "resolved_roots", "unresolved_roots", "not_installable_roots",
            "failed_roots",
        )
    }
    if sum(count_values[key] for key in count_values if key != "roots_walked") != count_values["roots_walked"]:
        fail(f"{name} root counts are inconsistent")
    if not isinstance(counts["error_kinds"], list):
        fail(f"{name} error histogram must be an array")
    histogram_total = 0
    histogram_keys: list[tuple[int, int]] = []
    for index, item in enumerate(counts["error_kinds"]):
        entry = exact_object(item, {"kind", "count"}, f"{name} error histogram {index}")
        histogram_keys.append(validate_error_kind(entry["kind"], f"{name} error histogram {index} kind"))
        histogram_total += exact_nonnegative_int(entry["count"], f"{name} error histogram {index} count")
    if histogram_total > U64_MAX or histogram_total != count_values["failed_roots"]:
        fail(f"{name} error histogram disagrees with failures")
    if histogram_keys != sorted(set(histogram_keys)):
        fail(f"{name} error histogram is noncanonical")

    outcomes = survey["outcomes"]
    failures = survey["failures"]
    if not isinstance(outcomes, (list, StreamingJsonArray)) or not isinstance(
        failures, (list, StreamingJsonArray)
    ):
        fail(f"{name} root records must be arrays")
    limits = {
        key: exact_nonnegative_int(survey[key], f"{name}.{key}")
        for key in (
            "failure_record_limit", "total_failures", "retained_failures",
            "evidence_byte_limit", "retained_evidence_bytes", "retained_explanations",
            "withheld_explanations",
        )
    }
    if (
        limits["failure_record_limit"] != SURVEY_RECORD_LIMIT
        or limits["evidence_byte_limit"] != SURVEY_EVIDENCE_BYTE_LIMIT
        or limits["retained_failures"] != len(failures)
        or limits["retained_failures"]
        != min(limits["total_failures"], limits["failure_record_limit"])
        or limits["retained_failures"] > limits["total_failures"]
        or limits["retained_failures"] > limits["failure_record_limit"]
    ):
        fail(f"{name} retention or evidence counts are inconsistent")

    successful = (
        count_values["resolved_roots"] + count_values["unresolved_roots"]
        + count_values["not_installable_roots"]
    )
    if len(outcomes) != successful:
        fail(f"{name} successful root records disagree with counts")
    previous_outcome_key = ""
    typed_counts = dict.fromkeys(OUTCOME_KINDS, 0)
    for index, item in enumerate(stream_objects(outcomes, CANDIDATE_ROOT_STREAM_SPEC)):
        outcome = exact_object(
            item,
            {"root_package_key_sha256", "name", "version", "release", "architecture", "outcome"},
            f"{name} outcome {index}",
        )
        root_sha256 = require_sha256(outcome["root_package_key_sha256"], f"{name} outcome {index} root")
        if root_sha256 <= previous_outcome_key:
            fail(f"{name} successful roots are noncanonical")
        previous_outcome_key = root_sha256
        for key in ("name", "version", "release"):
            require_rust_identity(outcome[key], f"{name} outcome {index}.{key}")
        require_optional_string(outcome["architecture"], f"{name} outcome {index}.architecture")
        kind = validate_native_outcome(outcome["outcome"], root_sha256, f"{name} outcome {index}.outcome")
        typed_counts[kind] += 1
    if typed_counts != {
        "resolved": count_values["resolved_roots"],
        "unresolved": count_values["unresolved_roots"],
        "not_installable": count_values["not_installable_roots"],
    }:
        fail(f"{name} typed outcomes disagree with counts")

    failure_keys: list[str] = []
    retained_bytes = 0
    retained_explanations = 0
    withheld_explanations = 0
    withholding_started = False
    for index, item in enumerate(failures):
        failure = exact_object(
            item,
            {
                "root_package_key_sha256", "name", "version", "release", "architecture",
                "error_kind", "error_message", "native_explanation",
            },
            f"{name} failure {index}",
        )
        failure_keys.append(require_sha256(failure["root_package_key_sha256"], f"{name} failure {index} root"))
        for key in ("name", "version", "release"):
            require_rust_identity(failure[key], f"{name} failure {index}.{key}")
        require_optional_string(failure["architecture"], f"{name} failure {index}.architecture")
        validate_error_kind(failure["error_kind"], f"{name} failure {index}.error_kind")
        if not isinstance(failure["error_message"], str) or not failure["error_message"]:
            fail(f"{name} failure {index} error message is empty or malformed")
        withheld, size = validate_explanation(
            failure["native_explanation"], f"{name} failure {index}.native_explanation"
        )
        if withheld:
            withholding_started = True
            withheld_explanations += 1
        else:
            if withholding_started:
                fail(f"{name} retained evidence after withholding began")
            retained_explanations += 1
            retained_bytes += size
    if failure_keys != sorted(set(failure_keys)):
        fail(f"{name} failure roots are noncanonical or overlap successful roots")
    failure_key_set = set(failure_keys)
    if any(
        item["root_package_key_sha256"] in failure_key_set
        for item in stream_objects(outcomes, CANDIDATE_ROOT_STREAM_SPEC)
    ):
        fail(f"{name} failure roots are noncanonical or overlap successful roots")
    if (
        limits["total_failures"] != count_values["failed_roots"]
        or not isinstance(survey["truncated"], bool)
        or survey["truncated"] != (limits["retained_failures"] < limits["total_failures"])
        or retained_bytes > limits["evidence_byte_limit"]
        or limits["retained_evidence_bytes"] != retained_bytes
        or limits["retained_explanations"] != retained_explanations
        or limits["withheld_explanations"] != withheld_explanations
        or retained_explanations + withheld_explanations != limits["retained_failures"]
        or not isinstance(survey["truncated_evidence"], bool)
        or survey["truncated_evidence"] != (withheld_explanations > 0)
    ):
        fail(f"{name} retention or evidence counts are inconsistent")


def reconstruct_candidate_manifest_sha256(
    candidate_survey: dict[str, Any],
    package_manifest: dict[str, Any],
    name: str,
) -> str:
    if candidate_survey["total_failures"] != 0:
        fail(f"{name} cannot reconstruct an incomplete candidate manifest")
    artifact = SizedSha256()
    closure_package_references = 0
    unresolved_dependencies = 0
    for item in stream_objects(
        candidate_survey["outcomes"], CANDIDATE_ROOT_STREAM_SPEC
    ):
        artifact.update(b'{"outcome":')
        update_native_outcome_digest(artifact, item["outcome"])
        artifact.update(b',"root_package_key_sha256":')
        root_bytes = canonical_json(item["root_package_key_sha256"])
        artifact.update(root_bytes)
        artifact.update(b"}\n")
        if item["outcome"]["status"] == "resolved":
            closure_package_references += len(
                item["outcome"]["closure_package_keys_sha256"]
            )
        elif item["outcome"]["status"] == "unresolved":
            unresolved_dependencies += len(item["outcome"]["dependencies"])
        if (
            closure_package_references > U64_MAX
            or unresolved_dependencies > U64_MAX
        ):
            fail(f"{name} reconstructed candidate counts exceed u64")
    counts = candidate_survey["counts"]
    artifact_counts = {
        "roots": counts["roots_walked"],
        "resolved_roots": counts["resolved_roots"],
        "unresolved_roots": counts["unresolved_roots"],
        "not_installable_roots": counts["not_installable_roots"],
        "closure_package_references": closure_package_references,
        "unresolved_dependencies": unresolved_dependencies,
    }
    implementation = package_manifest.get("implementation")
    if (
        not isinstance(implementation, dict)
        or implementation.get("ecosystem")
        != candidate_survey["implementation"]["ecosystem"]
    ):
        fail(f"{name} candidate and package implementations disagree")
    candidate_manifest = {
        "schema_version": 3,
        "profile": candidate_survey["profile"],
        "profile_revision_sha256": candidate_survey["profile_revision_sha256"],
        "profile_logical_digest_sha256": package_manifest[
            "profile_logical_digest_sha256"
        ],
        "members": package_manifest["members"],
        "package_oracle_manifest_sha256": candidate_survey[
            "package_oracle_manifest_sha256"
        ],
        "implementation": candidate_survey["implementation"],
        "policy": candidate_survey["policy"],
        "artifact": {
            "sha256": artifact.hexdigest(),
            "size": artifact.size,
            "counts": artifact_counts,
        },
    }
    return sha256_bytes(canonical_json(candidate_manifest))


def validate_comparison_survey(
    value: Any,
    profile: dict[str, Any],
    name: str,
    candidate_survey: dict[str, Any],
    candidate_manifest_sha256: str,
) -> None:
    survey = exact_object(
        value,
        {
            "schema_version", "profile", "profile_revision_sha256",
            "package_oracle_manifest_sha256", "oracle_manifest_sha256",
            "candidate_manifest_sha256", "counts", "mismatch_record_limit",
            "total_mismatches", "retained_mismatches", "truncated", "mismatches",
        },
        name,
    )
    if (
        exact_u32(survey["schema_version"], f"{name}.schema_version") != 2
        or survey["profile"] != profile["profile"]
        or survey["profile_revision_sha256"] != profile["profile_revision_sha256"]
        or survey["package_oracle_manifest_sha256"] != profile["package_oracle_manifest_sha256"]
        or survey["oracle_manifest_sha256"] != profile["native_resolution_manifest_sha256"]
        or survey["candidate_manifest_sha256"] != candidate_manifest_sha256
    ):
        fail(f"{name} identity or oracle binding drifted")
    require_rust_identity(survey["profile"], f"{name}.profile")
    for key in (
        "profile_revision_sha256", "package_oracle_manifest_sha256",
        "oracle_manifest_sha256", "candidate_manifest_sha256",
    ):
        require_sha256(survey[key], f"{name}.{key}")
    counts = exact_object(
        survey["counts"],
        {"roots_walked", "matching_roots", "mismatched_roots", "mismatch_kinds", "outcome_kind_pairs"},
        f"{name}.counts",
    )
    roots = exact_nonnegative_int(counts["roots_walked"], f"{name}.counts.roots_walked")
    matching = exact_nonnegative_int(counts["matching_roots"], f"{name}.counts.matching_roots")
    mismatched = exact_nonnegative_int(counts["mismatched_roots"], f"{name}.counts.mismatched_roots")
    if matching + mismatched > U64_MAX or matching + mismatched != roots:
        fail(f"{name} root counts are inconsistent")
    candidate_roots_walked = exact_nonnegative_int(
        candidate_survey["counts"]["roots_walked"], "candidate roots walked"
    )
    if roots != candidate_roots_walked or len(candidate_survey["outcomes"]) != roots:
        fail(f"{name} root population differs from the candidate survey")
    mismatch_keys: list[int] = []
    mismatch_total = 0
    if not isinstance(counts["mismatch_kinds"], list):
        fail(f"{name} mismatch histogram must be an array")
    for index, item in enumerate(counts["mismatch_kinds"]):
        entry = exact_object(item, {"kind", "count"}, f"{name} mismatch histogram {index}")
        try:
            mismatch_keys.append(MISMATCH_KINDS.index(entry["kind"]))
        except (ValueError, TypeError):
            fail(f"{name} mismatch histogram {index} kind is unsupported")
        mismatch_total += exact_nonnegative_int(entry["count"], f"{name} mismatch histogram {index} count")
    if mismatch_total > U64_MAX or mismatch_total != mismatched or mismatch_keys != sorted(set(mismatch_keys)):
        fail(f"{name} mismatch histogram is inconsistent")
    pair_keys: list[tuple[int, int]] = []
    pair_total = 0
    if not isinstance(counts["outcome_kind_pairs"], list):
        fail(f"{name} outcome-pair histogram must be an array")
    for index, item in enumerate(counts["outcome_kind_pairs"]):
        entry = exact_object(item, {"pair", "count"}, f"{name} outcome histogram {index}")
        pair = exact_object(entry["pair"], {"oracle", "candidate"}, f"{name} outcome histogram {index} pair")
        try:
            pair_keys.append((OUTCOME_KINDS.index(pair["oracle"]), OUTCOME_KINDS.index(pair["candidate"])))
        except (ValueError, TypeError):
            fail(f"{name} outcome histogram {index} pair is unsupported")
        pair_total += exact_nonnegative_int(entry["count"], f"{name} outcome histogram {index} count")
    if pair_total > U64_MAX or pair_total != mismatched or pair_keys != sorted(set(pair_keys)):
        fail(f"{name} outcome-pair histogram is inconsistent")

    limits = {
        key: exact_nonnegative_int(survey[key], f"{name}.{key}")
        for key in ("mismatch_record_limit", "total_mismatches", "retained_mismatches")
    }
    mismatches = survey["mismatches"]
    if (
        not isinstance(mismatches, (list, StreamingJsonArray))
        or limits["mismatch_record_limit"] != SURVEY_RECORD_LIMIT
        or limits["total_mismatches"] != mismatched
        or limits["retained_mismatches"] != len(mismatches)
        or limits["retained_mismatches"]
        != min(limits["total_mismatches"], limits["mismatch_record_limit"])
        or limits["retained_mismatches"] > limits["total_mismatches"]
        or limits["retained_mismatches"] > limits["mismatch_record_limit"]
        or not isinstance(survey["truncated"], bool)
        or survey["truncated"] != (limits["retained_mismatches"] < limits["total_mismatches"])
    ):
        fail(f"{name} mismatch retention counts are inconsistent")
    retained_root_keys: list[str] = []
    retained_candidate_bindings: dict[str, tuple[str, str]] = {}
    for index, item in enumerate(
        stream_objects(mismatches, COMPARISON_MISMATCH_STREAM_SPEC)
    ):
        mismatch = exact_object(item, {"root", "kind", "oracle", "candidate"}, f"{name} mismatch {index}")
        root = exact_object(
            mismatch["root"],
            {"package_key_sha256", "name", "version", "release", "architecture"},
            f"{name} mismatch {index} root",
        )
        root_sha256 = require_sha256(root["package_key_sha256"], f"{name} mismatch {index} root digest")
        retained_root_keys.append(root_sha256)
        for key in ("name", "version", "release"):
            require_rust_identity(root[key], f"{name} mismatch {index} root.{key}")
        require_optional_string(root["architecture"], f"{name} mismatch {index} root.architecture")
        typed: dict[str, tuple[str, dict[str, Any]]] = {}
        for side, manifest_key in (("oracle", "oracle_manifest_sha256"), ("candidate", "candidate_manifest_sha256")):
            evidence = exact_object(mismatch[side], {"manifest_sha256", "outcome"}, f"{name} mismatch {index} {side}")
            if evidence["manifest_sha256"] != survey[manifest_key]:
                fail(f"{name} mismatch {index} {side} manifest binding drifted")
            typed[side] = (
                validate_native_outcome(evidence["outcome"], root_sha256, f"{name} mismatch {index} {side}.outcome"),
                evidence["outcome"],
            )
        oracle_outcome_sha256 = native_outcome_sha256(typed["oracle"][1])
        candidate_outcome_sha256 = native_outcome_sha256(typed["candidate"][1])
        if oracle_outcome_sha256 == candidate_outcome_sha256:
            fail(f"{name} mismatch {index} retains equal outcomes")
        retained_candidate_bindings[root_sha256] = (
            sha256_bytes(
                canonical_json(
                    {
                        "name": root["name"],
                        "version": root["version"],
                        "release": root["release"],
                        "architecture": root["architecture"],
                    }
                )
            ),
            candidate_outcome_sha256,
        )
        if typed["oracle"][0] != typed["candidate"][0]:
            expected_kind = "resolution_outcome"
        elif typed["oracle"][0] == "resolved":
            expected_kind = "dependency_closure"
        elif typed["oracle"][0] == "unresolved":
            expected_kind = "unresolved_dependencies"
        else:
            expected_kind = "not_installable_reason"
        if mismatch["kind"] != expected_kind:
            fail(f"{name} mismatch {index} kind disagrees with its evidence")
    if retained_root_keys != sorted(set(retained_root_keys)):
        fail(f"{name} retained mismatch roots are noncanonical")
    matched_roots: set[str] = set()
    for candidate_root in stream_objects(
        candidate_survey["outcomes"], CANDIDATE_ROOT_STREAM_SPEC
    ):
        root_sha256 = candidate_root["root_package_key_sha256"]
        binding = retained_candidate_bindings.get(root_sha256)
        if binding is None:
            continue
        matched_roots.add(root_sha256)
        identity_sha256, outcome_sha256 = binding
        if identity_sha256 != sha256_bytes(
            canonical_json(
                {
                    "name": candidate_root["name"],
                    "version": candidate_root["version"],
                    "release": candidate_root["release"],
                    "architecture": candidate_root["architecture"],
                }
            )
        ):
            fail(f"{name} mismatch root differs from the candidate survey")
        if outcome_sha256 != native_outcome_sha256(candidate_root["outcome"]):
            fail(f"{name} mismatch candidate outcome differs from its survey")
    if matched_roots != set(retained_candidate_bindings):
        fail(f"{name} mismatch root differs from the candidate survey")


def forbid_private_paths(value: Any, label: str) -> None:
    if isinstance(value, dict):
        for key, item in value.items():
            forbid_private_paths(key, label)
            forbid_private_paths(item, label)
    elif isinstance(value, (list, StreamingJsonArray)):
        for item in value:
            forbid_private_paths(item, label)
    elif isinstance(value, str) and any(
        marker in value for marker in ("/conary/", "/etc/conary/", "/tmp/", "/data/")
    ):
        fail(f"{label} contains a private host path")


def forbid_private_path_bytes(path: Path, label: str) -> None:
    markers = (b"/conary/", b"/etc/conary/", b"/tmp/", b"/data/")
    overlap = max(map(len, markers)) - 1
    previous = b""
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            window = previous + chunk
            if any(marker in window for marker in markers):
                fail(f"{label} contains a private host path")
            previous = window[-overlap:]


def validate_input_evidence(
    path: Path, survey_id: str, export_id: str
) -> tuple[dict[str, Any], list[dict[str, Any]], str, dict[str, Any]]:
    value, _ = load_json(path, "resolution-survey input verification", canonical=True)
    require_envelope_schema(value, INPUT_EVIDENCE_SCHEMA, "survey input verification")
    evidence = exact_object(
        value,
        {
            "schema_version",
            "survey_id",
            "export_id",
            "workflow_runs",
            "oracle_operator",
            "oracle_assembly",
            "export_operator",
            "deployment",
            "profiles",
            "manifest_sha256",
            "transport",
        },
        "resolution-survey input verification",
    )
    if (
        evidence["survey_id"] != survey_id
        or evidence["export_id"] != export_id
    ):
        fail("resolution-survey input verification request binding drifted")
    workflow_runs = exact_object(
        evidence["workflow_runs"], {"oracle", "export", "deployment"}, "input workflow runs"
    )
    for name, run_id in workflow_runs.items():
        if exact_nonnegative_int(run_id, f"input {name} run") == 0:
            fail(f"input {name} run must be positive")
    oracle_operator = exact_object(
        evidence["oracle_operator"],
        {"workflow_commit_sha", "workflow_run_id", "workflow_run_attempt"},
        "input oracle operator",
    )
    if (
        oracle_operator["workflow_run_id"] != workflow_runs["oracle"]
        or exact_nonnegative_int(
            oracle_operator["workflow_run_attempt"], "input oracle run attempt"
        )
        == 0
    ):
        fail("input oracle operator binding drifted")
    require_commit(oracle_operator["workflow_commit_sha"], "input oracle operator commit")
    oracle_assembly = exact_object(
        evidence["oracle_assembly"], {"sha256"}, "input oracle assembly"
    )
    require_sha256(oracle_assembly["sha256"], "input oracle assembly digest")
    export_operator = exact_object(
        evidence["export_operator"],
        {
            "schema_version",
            "workflow_commit_sha",
            "workflow_run_id",
            "workflow_run_attempt",
            "attestation_sha256",
        },
        "input export operator",
    )
    if (
        exact_u32(
            export_operator["schema_version"], "input export operator schema_version"
        )
        != 1
        or export_operator["workflow_run_id"] != workflow_runs["export"]
        or exact_nonnegative_int(
            export_operator["workflow_run_attempt"], "input export run attempt"
        )
        == 0
    ):
        fail("input export operator binding drifted")
    require_commit(export_operator["workflow_commit_sha"], "input export operator commit")
    require_sha256(export_operator["attestation_sha256"], "input export attestation")
    deployment = exact_object(
        evidence["deployment"], {"commit_sha", "binary_sha256"}, "input deployment"
    )
    require_commit(deployment["commit_sha"], "input deployed commit")
    require_sha256(deployment["binary_sha256"], "input binary digest")
    profiles = evidence["profiles"]
    if not isinstance(profiles, list) or len(profiles) != len(PUBLIC_PROFILES):
        fail("resolution-survey input profiles are malformed")
    for index, profile in enumerate(profiles):
        profile = exact_object(
            profile,
            {
                "profile",
                "profile_revision_sha256",
                "target_architecture",
                "package_oracle_manifest_sha256",
                "native_resolution_manifest_sha256",
            },
            f"input profile {index}",
        )
        profile_name = PUBLIC_PROFILES[index]
        if (
            profile["profile"] != profile_name
            or profile["target_architecture"] != PROFILE_ARCHITECTURES[profile_name]
        ):
            fail("resolution-survey input profile order or architecture drifted")
        require_sha256(profile["profile_revision_sha256"], f"input {profile_name} candidate")
        require_sha256(
            profile["package_oracle_manifest_sha256"], f"input {profile_name} package oracle"
        )
        require_sha256(
            profile["native_resolution_manifest_sha256"],
            f"input {profile_name} resolution oracle",
        )
    manifest_sha256 = require_sha256(
        evidence["manifest_sha256"], "input transport manifest"
    )
    transport = exact_object(
        evidence["transport"], {"sha256", "size"}, "input oracle transport"
    )
    require_sha256(transport["sha256"], "input oracle transport digest")
    if exact_nonnegative_int(transport["size"], "input oracle transport size") == 0:
        fail("input oracle transport size must be positive")
    return deployment, profiles, manifest_sha256, transport


def load_input_package_manifests(
    path: Path,
    expected_manifest_sha256: str,
    expected_transport: dict[str, Any],
    profiles: list[dict[str, Any]],
) -> tuple[
    dict[str, dict[str, Any]],
    dict[str, dict[str, Any]],
    dict[str, dict[str, Any]],
]:
    metadata = plain_file(path, "authenticated oracle transport")
    if (
        metadata.st_size != expected_transport["size"]
        or hash_file(path) != expected_transport["sha256"]
    ):
        fail("oracle transport differs from authenticated input evidence")
    try:
        archive = tarfile.open(path, mode="r:")
    except (OSError, tarfile.TarError) as error:
        fail(f"oracle transport is not one uncompressed tar archive: {error}")
    with archive:
        members: dict[str, tarfile.TarInfo] = {}
        for member in archive:
            name = safe_member_name(member.name)
            if name in members:
                fail(f"oracle transport repeats member {name!r}")
            if member.pax_headers or getattr(member, "sparse", None):
                fail(f"oracle transport member {name!r} uses extended metadata")
            members[name] = member
            if len(members) > 1 + len(PUBLIC_PROFILES) * 4:
                fail("oracle transport exceeds its fixed profile member inventory")
        manifest_member = members.get("manifest.json")
        if manifest_member is None:
            fail("oracle transport has no manifest.json")
        manifest_bytes = read_tar_member(archive, manifest_member, MAX_MANIFEST_BYTES)
        input_manifest = decode_json(manifest_bytes, "oracle transport manifest")
        require_envelope_schema(input_manifest, INPUT_MANIFEST_SCHEMA, "survey input manifest")
        if (
            sha256_bytes(manifest_bytes) != expected_manifest_sha256
            or canonical_json(input_manifest) != manifest_bytes
        ):
            fail("oracle transport manifest differs from authenticated input evidence")
        expected_names = {"manifest.json"}
        package_manifests: dict[str, dict[str, Any]] = {}
        package_artifacts: dict[str, dict[str, Any]] = {}
        resolution_artifacts: dict[str, dict[str, Any]] = {}
        for profile in profiles:
            profile_name = profile["profile"]
            package_name = f"{profile_name}/package-oracle/manifest.json"
            expected_names.update(
                {
                    package_name,
                    f"{profile_name}/package-oracle/packages.jsonl",
                    f"{profile_name}/native-resolution/manifest.json",
                    f"{profile_name}/native-resolution/roots.jsonl",
                }
            )
            member = members.get(package_name)
            if member is None:
                fail(f"oracle transport omits {package_name}")
            package_bytes = read_tar_member(archive, member, MAX_MANIFEST_BYTES)
            if sha256_bytes(package_bytes) != profile["package_oracle_manifest_sha256"]:
                fail(f"{profile_name} package manifest differs from authenticated input")
            package = decode_json(package_bytes, f"{profile_name} package manifest")
            if canonical_json(package) != package_bytes or not isinstance(package, dict):
                fail(f"{profile_name} package manifest is not one canonical object")
            if (
                exact_u32(
                    package.get("schema_version"),
                    f"{profile_name} package manifest schema_version",
                )
                != 1
                or package.get("profile") != profile_name
                or package.get("profile_revision_sha256")
                != profile["profile_revision_sha256"]
            ):
                fail(f"{profile_name} package manifest binding drifted")
            require_sha256(
                package.get("profile_logical_digest_sha256"),
                f"{profile_name} profile logical digest",
            )
            if not isinstance(package.get("members"), list):
                fail(f"{profile_name} package manifest members are malformed")
            implementation = package.get("implementation")
            if (
                not isinstance(implementation, dict)
                or implementation.get("ecosystem") != PROFILE_ECOSYSTEMS[profile_name]
            ):
                fail(f"{profile_name} package implementation binding drifted")
            artifact = exact_object(
                package.get("artifact"),
                {"sha256", "size", "counts"},
                f"{profile_name} package artifact",
            )
            artifact_sha256 = require_sha256(
                artifact["sha256"], f"{profile_name} package artifact digest"
            )
            artifact_size = exact_nonnegative_int(
                artifact["size"], f"{profile_name} package artifact size"
            )
            artifact_counts = exact_object(
                artifact["counts"],
                {"packages", "provides", "requirement_groups", "requirement_atoms"},
                f"{profile_name} package artifact counts",
            )
            package_count = exact_nonnegative_int(
                artifact_counts["packages"], f"{profile_name} package count"
            )
            artifact_name = f"{profile_name}/package-oracle/packages.jsonl"
            artifact_member = members.get(artifact_name)
            if (
                artifact_member is None
                or not artifact_member.isreg()
                or artifact_member.size != artifact_size
            ):
                fail(f"{profile_name} package artifact differs from its manifest")
            artifact_stream = archive.extractfile(artifact_member)
            if artifact_stream is None:
                fail(f"{profile_name} package artifact cannot be read")
            artifact_digest = hashlib.sha256()
            copied = 0
            while chunk := artifact_stream.read(1024 * 1024):
                copied += len(chunk)
                if copied > artifact_size:
                    fail(f"{profile_name} package artifact exceeded its manifest size")
                artifact_digest.update(chunk)
            if copied != artifact_size or artifact_digest.hexdigest() != artifact_sha256:
                fail(f"{profile_name} package artifact differs from its manifest")
            package_manifests[profile_name] = package
            package_artifacts[profile_name] = {
                "offset": artifact_member.offset_data,
                "size": artifact_size,
                "packages": package_count,
            }
            resolution_name = f"{profile_name}/native-resolution/manifest.json"
            resolution_member = members.get(resolution_name)
            if resolution_member is None:
                fail(f"oracle transport omits {resolution_name}")
            resolution_bytes = read_tar_member(
                archive, resolution_member, MAX_MANIFEST_BYTES
            )
            if (
                sha256_bytes(resolution_bytes)
                != profile["native_resolution_manifest_sha256"]
            ):
                fail(
                    f"{profile_name} resolution manifest differs from authenticated input"
                )
            resolution = decode_json(
                resolution_bytes, f"{profile_name} resolution manifest"
            )
            require_envelope_schema(resolution, 3, "native resolution bundle")
            if canonical_json(resolution) != resolution_bytes:
                fail(f"{profile_name} resolution manifest is not canonical JSON")
            resolution = exact_object(
                resolution,
                {
                    "schema_version",
                    "profile",
                    "profile_revision_sha256",
                    "profile_logical_digest_sha256",
                    "members",
                    "package_oracle_manifest_sha256",
                    "implementation",
                    "policy",
                    "artifact",
                },
                f"{profile_name} resolution manifest",
            )
            if (
                exact_u32(
                    resolution["schema_version"],
                    f"{profile_name} resolution schema_version",
                )
                != 3
                or resolution["profile"] != profile_name
                or resolution["profile_revision_sha256"]
                != profile["profile_revision_sha256"]
                or resolution["package_oracle_manifest_sha256"]
                != profile["package_oracle_manifest_sha256"]
                or resolution["profile_logical_digest_sha256"]
                != package["profile_logical_digest_sha256"]
                or resolution["members"] != package["members"]
            ):
                fail(f"{profile_name} resolution manifest binding drifted")
            resolution_implementation = exact_object(
                resolution["implementation"],
                {"ecosystem", "name", "version", "projection_schema"},
                f"{profile_name} resolution implementation",
            )
            if resolution_implementation["ecosystem"] != PROFILE_ECOSYSTEMS[profile_name]:
                fail(f"{profile_name} resolution implementation binding drifted")
            validate_policy(
                resolution["policy"],
                profile["target_architecture"],
                f"{profile_name} resolution policy",
            )
            resolution_artifact = exact_object(
                resolution["artifact"],
                {"sha256", "size", "counts"},
                f"{profile_name} resolution artifact",
            )
            resolution_sha256 = require_sha256(
                resolution_artifact["sha256"],
                f"{profile_name} resolution artifact digest",
            )
            resolution_size = exact_nonnegative_int(
                resolution_artifact["size"],
                f"{profile_name} resolution artifact size",
            )
            resolution_counts = exact_object(
                resolution_artifact["counts"],
                {
                    "roots",
                    "resolved_roots",
                    "unresolved_roots",
                    "not_installable_roots",
                    "closure_package_references",
                    "unresolved_dependencies",
                },
                f"{profile_name} resolution artifact counts",
            )
            for key, value in resolution_counts.items():
                exact_nonnegative_int(
                    value, f"{profile_name} resolution artifact counts.{key}"
                )
            roots_name = f"{profile_name}/native-resolution/roots.jsonl"
            roots_member = members.get(roots_name)
            if (
                roots_member is None
                or not roots_member.isreg()
                or roots_member.size != resolution_size
            ):
                fail(f"{profile_name} resolution artifact differs from its manifest")
            roots_stream = archive.extractfile(roots_member)
            if roots_stream is None:
                fail(f"{profile_name} resolution artifact cannot be read")
            roots_digest = hashlib.sha256()
            roots_size = 0
            while chunk := roots_stream.read(1024 * 1024):
                roots_size += len(chunk)
                if roots_size > resolution_size:
                    fail(
                        f"{profile_name} resolution artifact exceeded its manifest size"
                    )
                roots_digest.update(chunk)
            if (
                roots_size != resolution_size
                or roots_digest.hexdigest() != resolution_sha256
            ):
                fail(f"{profile_name} resolution artifact differs from its manifest")
            resolution_artifacts[profile_name] = {
                "offset": roots_member.offset_data,
                "size": resolution_size,
                "counts": resolution_counts,
                "manifest_sha256": profile["native_resolution_manifest_sha256"],
            }
        if set(members) != expected_names:
            fail("oracle transport contains missing or unexpected members")
    return package_manifests, package_artifacts, resolution_artifacts


PACKAGE_ROW_FIELDS = {
    "package_key_sha256",
    "member_ordinal",
    "source_identity",
    "repository_identity",
    "source_snapshot_sha256",
    "source_profile",
    "name",
    "version",
    "package_release",
    "architecture",
    "debian_multi_arch",
    "checksum",
    "size",
    "download_url",
    "version_scheme",
    "provides",
    "requirement_groups",
}
PACKAGE_ROW_SKIP_SPEC = {
    key: "skip"
    for key in PACKAGE_ROW_FIELDS
    if key
    not in {"package_key_sha256", "name", "version", "package_release", "architecture"}
}


def validate_candidate_package_coverage(
    oracle_transport: Path,
    package_artifact: dict[str, Any],
    candidate_survey: dict[str, Any],
    label: str,
) -> None:
    if (
        candidate_survey["counts"]["roots_walked"]
        != package_artifact["packages"]
    ):
        fail(f"{label} root count differs from its authenticated package manifest")
    outcomes = iter(
        stream_objects(candidate_survey["outcomes"], CANDIDATE_ROOT_STREAM_SPEC)
    )
    failures = iter(
        stream_objects(
            candidate_survey["failures"], CANDIDATE_FAILURE_COVERAGE_STREAM_SPEC
        )
    )
    outcome = next(outcomes, None)
    failure = next(failures, None)

    def next_retained_root() -> dict[str, Any] | None:
        nonlocal outcome, failure
        if outcome is None and failure is None:
            return None
        if failure is None or (
            outcome is not None
            and outcome["root_package_key_sha256"]
            < failure["root_package_key_sha256"]
        ):
            retained = outcome
            outcome = next(outcomes, None)
        else:
            retained = failure
            failure = next(failures, None)
        return retained

    retained = next_retained_root()
    with oracle_transport.open("rb") as stream:
        mapping = mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ)
        try:
            package_rows = StreamingJsonLines(
                mapping,
                package_artifact["offset"],
                package_artifact["size"],
                f"{label} authenticated package oracle",
            ).objects(PACKAGE_ROW_SKIP_SPEC)
            packages_seen = 0
            for package_view in package_rows:
                package = exact_object(
                    package_view.value,
                    PACKAGE_ROW_FIELDS,
                    f"{label} package row {packages_seen}",
                )
                if retained is not None:
                    package_key = package["package_key_sha256"]
                    retained_key = retained["root_package_key_sha256"]
                    if retained_key < package_key:
                        fail(f"{label} retains a root absent from its package oracle")
                    if retained_key == package_key:
                        expected = {
                            "root_package_key_sha256": package_key,
                            "name": package["name"],
                            "version": package["version"],
                            "release": package["package_release"],
                            "architecture": package["architecture"],
                        }
                        actual = {key: retained[key] for key in expected}
                        if actual != expected:
                            fail(
                                f"{label} root differs from its authenticated package oracle"
                            )
                        retained = next_retained_root()
                packages_seen += 1
            if retained is not None:
                fail(f"{label} retains a root absent from its package oracle")
            if packages_seen != package_artifact["packages"]:
                fail(f"{label} package root count differs from its authenticated manifest")
        finally:
            mapping.close()


def validate_native_comparison(
    oracle_transport: Path,
    resolution_artifact: dict[str, Any],
    candidate_survey: dict[str, Any],
    comparison_survey: dict[str, Any],
    candidate_manifest_sha256: str,
    label: str,
) -> None:
    candidates = iter(
        stream_objects(candidate_survey["outcomes"], CANDIDATE_ROOT_STREAM_SPEC)
    )
    retained = iter(
        stream_objects(
            comparison_survey["mismatches"], COMPARISON_MISMATCH_STREAM_SPEC
        )
    )
    roots = 0
    matching = 0
    mismatched = 0
    native_kind_counts = dict.fromkeys(OUTCOME_KINDS, 0)
    closure_references = 0
    unresolved_dependencies = 0
    mismatch_counts = dict.fromkeys(MISMATCH_KINDS, 0)
    outcome_pair_counts: dict[tuple[str, str], int] = {}
    previous_root = ""
    with oracle_transport.open("rb") as stream:
        mapping = mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ)
        try:
            native_rows = StreamingJsonLines(
                mapping,
                resolution_artifact["offset"],
                resolution_artifact["size"],
                f"{label} authenticated native resolution",
            ).objects(CANDIDATE_ROOT_STREAM_SPEC)
            for native_view in native_rows:
                native = exact_object(
                    native_view.value,
                    {"root_package_key_sha256", "outcome"},
                    f"{label} native root {roots}",
                )
                root_sha256 = require_sha256(
                    native["root_package_key_sha256"], f"{label} native root {roots}"
                )
                if root_sha256 <= previous_root:
                    fail(f"{label} native roots are duplicated or noncanonical")
                previous_root = root_sha256
                try:
                    candidate = next(candidates)
                except StopIteration:
                    fail(f"{label} candidate omits authenticated native roots")
                if candidate["root_package_key_sha256"] != root_sha256:
                    fail(f"{label} candidate and native root order differs")
                native_kind = validate_native_outcome(
                    native["outcome"], root_sha256, f"{label} native root {roots}.outcome"
                )
                candidate_kind = candidate["outcome"]["status"]
                native_kind_counts[native_kind] += 1
                if native_kind == "resolved":
                    closure_references += len(
                        native["outcome"]["closure_package_keys_sha256"]
                    )
                elif native_kind == "unresolved":
                    unresolved_dependencies += len(native["outcome"]["dependencies"])
                native_sha256 = native_outcome_sha256(native["outcome"])
                candidate_sha256 = native_outcome_sha256(candidate["outcome"])
                roots += 1
                if native_sha256 == candidate_sha256:
                    matching += 1
                    continue
                mismatched += 1
                if native_kind != candidate_kind:
                    kind = "resolution_outcome"
                elif native_kind == "resolved":
                    kind = "dependency_closure"
                elif native_kind == "unresolved":
                    kind = "unresolved_dependencies"
                else:
                    kind = "not_installable_reason"
                mismatch_counts[kind] += 1
                pair = (native_kind, candidate_kind)
                outcome_pair_counts[pair] = outcome_pair_counts.get(pair, 0) + 1
                if mismatched <= comparison_survey["retained_mismatches"]:
                    try:
                        recorded = next(retained)
                    except StopIteration:
                        fail(f"{label} omits retained mismatch evidence")
                    root = recorded["root"]
                    oracle = recorded["oracle"]
                    candidate_evidence = recorded["candidate"]
                    if (
                        recorded["kind"] != kind
                        or root
                        != {
                            "package_key_sha256": root_sha256,
                            "name": candidate["name"],
                            "version": candidate["version"],
                            "release": candidate["release"],
                            "architecture": candidate["architecture"],
                        }
                        or oracle["manifest_sha256"]
                        != resolution_artifact["manifest_sha256"]
                        or candidate_evidence["manifest_sha256"]
                        != candidate_manifest_sha256
                        or native_outcome_sha256(oracle["outcome"]) != native_sha256
                        or native_outcome_sha256(candidate_evidence["outcome"])
                        != candidate_sha256
                    ):
                        fail(f"{label} retained mismatch differs from authenticated roots")
            try:
                next(candidates)
            except StopIteration:
                pass
            else:
                fail(f"{label} candidate contains roots absent from the native oracle")
            try:
                next(retained)
            except StopIteration:
                pass
            else:
                fail(f"{label} retains excess mismatch evidence")
        finally:
            mapping.close()
    native_counts = {
        "roots": roots,
        "resolved_roots": native_kind_counts["resolved"],
        "unresolved_roots": native_kind_counts["unresolved"],
        "not_installable_roots": native_kind_counts["not_installable"],
        "closure_package_references": closure_references,
        "unresolved_dependencies": unresolved_dependencies,
    }
    if native_counts != resolution_artifact["counts"]:
        fail(f"{label} authenticated native root counts differ from its manifest")
    expected_counts = {
        "roots_walked": roots,
        "matching_roots": matching,
        "mismatched_roots": mismatched,
        "mismatch_kinds": [
            {"kind": kind, "count": mismatch_counts[kind]}
            for kind in MISMATCH_KINDS
            if mismatch_counts[kind]
        ],
        "outcome_kind_pairs": [
            {
                "pair": {"oracle": oracle, "candidate": candidate},
                "count": outcome_pair_counts[(oracle, candidate)],
            }
            for oracle in OUTCOME_KINDS
            for candidate in OUTCOME_KINDS
            if (oracle, candidate) in outcome_pair_counts
        ],
    }
    if comparison_survey["counts"] != expected_counts:
        fail(f"{label} comparison counts differ from authenticated native roots")


def verify_staged_output(
    args: argparse.Namespace,
    staging: Path,
    survey_id: str,
    export_id: str,
    input_deployment: dict[str, Any],
    input_profiles: list[dict[str, Any]],
    package_manifests: dict[str, dict[str, Any]],
    package_artifacts: dict[str, dict[str, Any]],
    resolution_artifacts: dict[str, dict[str, Any]],
) -> None:
    metadata = plain_file(args.transport, "survey transport")
    try:
        archive = tarfile.open(args.transport, mode="r:")
    except (OSError, tarfile.TarError) as error:
        fail(f"survey transport is not one uncompressed tar archive: {error}")
    with archive:
        members: dict[str, tarfile.TarInfo] = {}
        for member in archive:
            name = safe_member_name(member.name)
            if name in members:
                fail(f"survey transport repeats member {name!r}")
            if member.pax_headers or getattr(member, "sparse", None):
                fail(f"survey transport member {name!r} uses extended metadata")
            members[name] = member
            if len(members) > MAX_SURVEY_DOCUMENTS + 1:
                fail("survey transport exceeds the fixed profile document inventory")
        manifest_member = members.get("manifest.json")
        if manifest_member is None:
            fail("survey transport has no manifest.json")
        manifest_bytes = read_tar_member(archive, manifest_member, MAX_MANIFEST_BYTES)
        manifest = decode_json(manifest_bytes, "survey manifest")
        require_envelope_schema(manifest, OUTPUT_MANIFEST_SCHEMA, "survey output manifest")
        if canonical_json(manifest) != manifest_bytes:
            fail("survey manifest is not canonical JSON")
        manifest = exact_object(
            manifest,
            {
                "schema_version",
                "survey_id",
                "export_id",
                "deployment",
                "profiles",
                "counts",
                "files",
            },
            "survey manifest",
        )
        if (
            manifest["survey_id"] != survey_id
            or manifest["export_id"] != export_id
        ):
            fail("survey manifest request binding drifted")
        deployment = exact_object(
            manifest["deployment"], {"commit_sha", "binary_sha256"}, "survey deployment"
        )
        require_commit(deployment["commit_sha"], "survey deployed commit")
        require_sha256(deployment["binary_sha256"], "survey binary digest")
        if deployment != input_deployment:
            fail("survey deployment binding differs from authenticated input")
        profiles = manifest["profiles"]
        if (
            not isinstance(profiles, list)
            or [item.get("profile") for item in profiles if isinstance(item, dict)]
            != list(PUBLIC_PROFILES)
        ):
            fail("survey manifest profiles are not in canonical public order")
        files = manifest["files"]
        if not isinstance(files, list):
            fail("survey manifest file inventory must be an array")
        expected_names = {"manifest.json"}
        file_entries: dict[str, tuple[int, str]] = {}
        previous_name = ""
        for index, item in enumerate(files):
            item = exact_object(item, {"path", "sha256", "size"}, f"survey file {index}")
            name = safe_member_name(item["path"])
            if name <= previous_name or "/" in name or not name.endswith(".json"):
                fail("survey file inventory is reordered or uses a non-public path")
            previous_name = name
            expected_names.add(name)
            expected_size = exact_nonnegative_int(item["size"], f"survey file {name} size")
            expected_sha256 = require_sha256(item["sha256"], f"survey file {name} digest")
            member = members.get(name)
            if member is None or member.size != expected_size:
                fail(f"survey file {name} is missing or changed size")
            file_entries[name] = (expected_size, expected_sha256)
        if set(members) != expected_names:
            fail("survey transport contains missing or unexpected members")

    candidate_failures = 0
    comparison_mismatches = 0
    comparison_profiles = 0
    roots_walked = 0
    referenced_files: set[str] = set()
    for profile_index, profile in enumerate(profiles):
        profile = exact_object(
            profile,
            {
                "profile",
                "profile_revision_sha256",
                "target_architecture",
                "package_oracle_manifest_sha256",
                "native_resolution_manifest_sha256",
                "candidate",
                "comparison",
            },
            "survey profile",
        )
        profile_name = profile["profile"]
        require_sha256(profile["profile_revision_sha256"], f"{profile_name} candidate")
        require_sha256(
            profile["package_oracle_manifest_sha256"], f"{profile_name} package oracle"
        )
        require_sha256(
            profile["native_resolution_manifest_sha256"],
            f"{profile_name} resolution oracle",
        )
        if profile["target_architecture"] != PROFILE_ARCHITECTURES[profile_name]:
            fail(f"{profile_name} survey architecture drifted")
        if {
            "profile": profile_name,
            "profile_revision_sha256": profile["profile_revision_sha256"],
            "target_architecture": profile["target_architecture"],
            "package_oracle_manifest_sha256": profile[
                "package_oracle_manifest_sha256"
            ],
            "native_resolution_manifest_sha256": profile[
                "native_resolution_manifest_sha256"
            ],
        } != input_profiles[profile_index]:
            fail(f"{profile_name} survey binding differs from authenticated input")
        candidate = profile["candidate"]
        candidate = exact_object(
            candidate,
            {
                "file",
                "implementation_file",
                "counts",
                "total_failures",
                "error_histogram",
            },
            f"{profile_name} candidate summary",
        )
        expected_candidate_name = f"{profile_name}.candidate-resolution-survey.json"
        if (
            candidate.get("file") != expected_candidate_name
            or expected_candidate_name not in file_entries
        ):
            fail(f"{profile_name} candidate survey file binding is missing")
        referenced_files.add(candidate["file"])
        candidate_implementation_name = candidate["implementation_file"]
        expected_candidate_implementation_name = (
            f"{profile_name}.candidate-resolution-implementation.json"
        )
        if (
            candidate_implementation_name != expected_candidate_implementation_name
            or candidate_implementation_name not in file_entries
        ):
            fail(f"{profile_name} candidate implementation file binding is missing")
        referenced_files.add(candidate_implementation_name)
        candidate_implementation_path = staging / candidate_implementation_name
        implementation_size, implementation_sha256 = file_entries[
            candidate_implementation_name
        ]
        copy_declared_survey_member(
            args.transport,
            candidate_implementation_name,
            candidate_implementation_path,
            implementation_size,
            implementation_sha256,
        )
        forbid_private_path_bytes(candidate_implementation_path, candidate_implementation_name)
        implementation, _ = load_json(
            candidate_implementation_path,
            f"{profile_name} candidate implementation",
            canonical=True,
        )
        validate_resolution_implementation(
            implementation, f"{profile_name} candidate implementation"
        )
        candidate_implementation_path.unlink()
        candidate_path = staging / candidate["file"]
        candidate_size, candidate_sha256 = file_entries[candidate["file"]]
        copy_declared_survey_member(
            args.transport,
            candidate["file"],
            candidate_path,
            candidate_size,
            candidate_sha256,
        )
        forbid_private_path_bytes(candidate_path, candidate["file"])
        with StreamingJsonDocument(
            candidate_path,
            f"{profile_name} candidate survey",
            {"outcomes", "failures"},
        ) as candidate_value:
            validate_candidate_survey(candidate_value, profile, candidate["file"])
            if (
                canonical_json(candidate.get("counts"))
                != canonical_json(candidate_value["counts"])
                or exact_nonnegative_int(
                    candidate.get("total_failures"),
                    f"{profile_name} candidate summary total_failures",
                )
                != candidate_value["total_failures"]
                or canonical_json(candidate.get("error_histogram"))
                != canonical_json(candidate_value["counts"]["error_kinds"])
            ):
                fail(f"{profile_name} candidate summary differs from its survey")
            roots_walked += candidate_value["counts"]["roots_walked"]
            candidate_failures += candidate_value["total_failures"]
            validate_candidate_package_coverage(
                args.oracle_transport,
                package_artifacts[profile_name],
                candidate_value,
                candidate["file"],
            )

            comparison = profile["comparison"]
            if comparison is None:
                if candidate_value["total_failures"] == 0:
                    fail(f"{profile_name} omitted comparison without candidate failures")
                candidate_path.unlink()
                continue
            if candidate_value["total_failures"] != 0 or not isinstance(comparison, dict):
                fail(f"{profile_name} retained comparison for an incomplete candidate")
            comparison = exact_object(
                comparison,
                {
                    "file",
                    "implementation_file",
                    "candidate_manifest_sha256",
                    "counts",
                    "total_mismatches",
                    "mismatch_histogram",
                    "outcome_histogram",
                },
                f"{profile_name} comparison summary",
            )
            expected_candidate_manifest_sha256 = reconstruct_candidate_manifest_sha256(
                candidate_value,
                package_manifests[profile_name],
                candidate["file"],
            )
            if (
                require_sha256(
                    comparison["candidate_manifest_sha256"],
                    f"{profile_name} comparison candidate manifest",
                )
                != expected_candidate_manifest_sha256
            ):
                fail(f"{profile_name} comparison summary binds another candidate manifest")
            comparison_name = comparison.get("file")
            expected_comparison_name = (
                f"{profile_name}.native-resolution-comparison-survey.json"
            )
            if (
                comparison_name != expected_comparison_name
                or comparison_name not in file_entries
            ):
                fail(f"{profile_name} comparison survey file binding is missing")
            referenced_files.add(comparison_name)
            comparison_implementation_name = comparison["implementation_file"]
            expected_comparison_implementation_name = (
                f"{profile_name}.comparison-resolution-implementation.json"
            )
            if (
                comparison_implementation_name != expected_comparison_implementation_name
                or comparison_implementation_name not in file_entries
            ):
                fail(f"{profile_name} comparison implementation file binding is missing")
            referenced_files.add(comparison_implementation_name)
            comparison_implementation_path = staging / comparison_implementation_name
            implementation_size, implementation_sha256 = file_entries[
                comparison_implementation_name
            ]
            copy_declared_survey_member(
                args.transport,
                comparison_implementation_name,
                comparison_implementation_path,
                implementation_size,
                implementation_sha256,
            )
            forbid_private_path_bytes(
                comparison_implementation_path, comparison_implementation_name
            )
            implementation, _ = load_json(
                comparison_implementation_path,
                f"{profile_name} comparison implementation",
                canonical=True,
            )
            validate_resolution_implementation(
                implementation, f"{profile_name} comparison implementation"
            )
            comparison_implementation_path.unlink()
            comparison_path = staging / comparison_name
            comparison_size, comparison_sha256 = file_entries[comparison_name]
            copy_declared_survey_member(
                args.transport,
                comparison_name,
                comparison_path,
                comparison_size,
                comparison_sha256,
            )
            forbid_private_path_bytes(comparison_path, comparison_name)
            with StreamingJsonDocument(
                comparison_path,
                f"{profile_name} comparison survey",
                {"mismatches"},
            ) as comparison_value:
                validate_comparison_survey(
                    comparison_value,
                    profile,
                    comparison_name,
                    candidate_value,
                    expected_candidate_manifest_sha256,
                )
                validate_native_comparison(
                    args.oracle_transport,
                    resolution_artifacts[profile_name],
                    candidate_value,
                    comparison_value,
                    expected_candidate_manifest_sha256,
                    comparison_name,
                )
                if (
                    canonical_json(comparison.get("counts"))
                    != canonical_json(comparison_value["counts"])
                    or exact_nonnegative_int(
                        comparison.get("total_mismatches"),
                        f"{profile_name} comparison summary total_mismatches",
                    )
                    != comparison_value["total_mismatches"]
                    or comparison.get("candidate_manifest_sha256")
                    != comparison_value["candidate_manifest_sha256"]
                    or canonical_json(comparison.get("mismatch_histogram"))
                    != canonical_json(comparison_value["counts"]["mismatch_kinds"])
                    or canonical_json(comparison.get("outcome_histogram"))
                    != canonical_json(comparison_value["counts"]["outcome_kind_pairs"])
                ):
                    fail(f"{profile_name} comparison summary differs from its survey")
                comparison_profiles += 1
                comparison_mismatches += comparison_value["total_mismatches"]
            comparison_path.unlink()
        candidate_path.unlink()

    if referenced_files != set(file_entries):
        fail("survey manifest file inventory contains an unbound JSON document")

    counts = exact_object(
        manifest["counts"],
        {
            "profiles",
            "roots_walked",
            "candidate_failures",
            "comparison_profiles",
            "comparison_mismatches",
        },
        "survey counts",
    )
    for key, value in counts.items():
        exact_nonnegative_int(value, f"survey counts.{key}")
    if counts != {
        "profiles": 3,
        "roots_walked": roots_walked,
        "candidate_failures": candidate_failures,
        "comparison_profiles": comparison_profiles,
        "comparison_mismatches": comparison_mismatches,
    }:
        fail("survey manifest aggregate counts disagree with its files")
    forbid_private_paths(manifest, "survey manifest")

    evidence = {
        "schema_version": OUTPUT_EVIDENCE_SCHEMA,
        "survey_id": survey_id,
        "export_id": export_id,
        "deployment": deployment,
        "profiles": profiles,
        "counts": counts,
        "manifest_sha256": sha256_bytes(manifest_bytes),
        "transport": {"sha256": hash_file(args.transport), "size": metadata.st_size},
    }
    write_new(args.evidence, canonical_json(evidence))
    print(canonical_json(evidence).decode())


def verify_output(args: argparse.Namespace) -> None:
    survey_id = require_identity(args.survey_id, "survey id")
    export_id = require_identity(args.export_id, "export id")
    (
        input_deployment,
        input_profiles,
        input_manifest_sha256,
        input_transport,
    ) = validate_input_evidence(
        args.input_evidence, survey_id, export_id
    )
    (
        package_manifests,
        package_artifacts,
        resolution_artifacts,
    ) = load_input_package_manifests(
        args.oracle_transport,
        input_manifest_sha256,
        input_transport,
        input_profiles,
    )
    with tempfile.TemporaryDirectory(prefix="remi-resolution-survey-verify-") as path:
        staging = Path(path)
        os.chmod(staging, 0o700)
        verify_staged_output(
            args,
            staging,
            survey_id,
            export_id,
            input_deployment,
            input_profiles,
            package_manifests,
            package_artifacts,
            resolution_artifacts,
        )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    build = subparsers.add_parser("build-input")
    build.add_argument("--survey-id", required=True)
    build.add_argument("--repository", required=True)
    build.add_argument("--oracle-run-id", required=True)
    build.add_argument("--workflow-commit", required=True)
    build.add_argument("--oracle-run", required=True, type=Path)
    build.add_argument("--oracle-artifacts", required=True, type=Path)
    build.add_argument("--assembly-evidence", required=True, type=Path)
    build.add_argument("--export-run-id", required=True)
    build.add_argument("--export-run", required=True, type=Path)
    build.add_argument("--export-artifacts", required=True, type=Path)
    build.add_argument("--deployment-run-id", required=True)
    build.add_argument("--deployment-run", required=True, type=Path)
    build.add_argument("--export-root", required=True, type=Path)
    build.add_argument("--lane", action="append", default=[], required=True)
    build.add_argument("--consume-lane-files", action="store_true")
    build.add_argument("--output", required=True, type=Path)
    build.add_argument("--evidence", required=True, type=Path)
    verify = subparsers.add_parser("verify-output")
    verify.add_argument("--survey-id", required=True)
    verify.add_argument("--export-id", required=True)
    verify.add_argument("--input-evidence", required=True, type=Path)
    verify.add_argument("--oracle-transport", required=True, type=Path)
    verify.add_argument("--transport", required=True, type=Path)
    verify.add_argument("--evidence", required=True, type=Path)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    if args.command == "build-input":
        build_input(args)
    else:
        verify_output(args)


if __name__ == "__main__":
    try:
        main()
    except SchemaRebuildRequired as error:
        print(canonical_json(error.evidence).decode(), file=sys.stderr)
        raise SystemExit(3) from error
    except (OSError, tarfile.TarError, ValidationError) as error:
        raise SystemExit(f"Remi resolution-survey transport validation failed: {error}") from error
