#!/usr/bin/env python3
# scripts/remi-resolution-survey-transport.py

"""Build authenticated survey inputs and verify sanitized survey outputs."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import tarfile
from typing import Any, NoReturn


PUBLIC_PROFILES = ("fedora-44", "ubuntu-26.04", "arch")
PROFILE_ARCHITECTURES = {
    "fedora-44": "x86_64",
    "ubuntu-26.04": "amd64",
    "arch": "x86_64",
}
SHA256 = re.compile(r"^[0-9a-f]{64}$")
COMMIT = re.compile(r"^[0-9a-f]{40}$")
IDENTITY = re.compile(r"^[a-z0-9][a-z0-9._-]{0,127}$")
RUN_ID = re.compile(r"^[1-9][0-9]*$")
MAX_MANIFEST_BYTES = 1024 * 1024
MAX_SURVEY_TRANSPORT_BYTES = 640 * 1024 * 1024


class ValidationError(ValueError):
    """An input or output differs from the reviewed transport contract."""


def fail(message: str) -> NoReturn:
    raise ValidationError(message)


def canonical_json(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def reject_duplicate_key(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            fail(f"JSON repeats key {key!r}")
        value[key] = item
    return value


def decode_json(data: bytes, label: str) -> Any:
    try:
        return json.loads(data, object_pairs_hook=reject_duplicate_key)
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
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        fail(f"{label} must be a nonnegative integer")
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


def validate_lane(
    root: Path,
    profile: str,
    export_id: str,
    deployed_commit: str,
    input_manifest_sha256: str,
    candidate_sha256: str,
) -> tuple[dict[str, Any], list[tuple[str, Path]]]:
    plain_directory(root, f"{profile} lane")
    if sorted(path.name for path in root.iterdir()) != [
        "evidence.json",
        "package-oracle",
        "resolution-oracle",
    ]:
        fail(f"{profile} lane has missing or unexpected entries")
    evidence_value, _ = load_json(root / "evidence.json", f"{profile} lane evidence", canonical=True)
    evidence = exact_object(
        evidence_value,
        {
            "schema_version",
            "export_id",
            "deployed_commit",
            "input_manifest_sha256",
            "profile",
            "profile_revision_sha256",
            "target_architecture",
            "package_oracle",
            "resolution_oracle",
        },
        f"{profile} lane evidence",
    )
    architecture = PROFILE_ARCHITECTURES[profile]
    if (
        evidence["schema_version"] != 1
        or evidence["export_id"] != export_id
        or evidence["deployed_commit"] != deployed_commit
        or evidence["input_manifest_sha256"] != input_manifest_sha256
        or evidence["profile"] != profile
        or evidence["profile_revision_sha256"] != candidate_sha256
        or evidence["target_architecture"] != architecture
    ):
        fail(f"{profile} lane differs from its export and deployment authority")

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
    if (
        package_value.get("schema_version") != 1
        or package_value.get("profile") != profile
        or package_value.get("profile_revision_sha256") != candidate_sha256
        or resolution_value.get("schema_version") != 2
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
        attestation["schema_version"] != 1
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
    oracle_artifacts, _ = load_json(args.oracle_artifacts, "oracle artifact metadata")
    oracle_names = unexpired_artifact_names(oracle_artifacts, "oracle run")
    patterns = {
        profile: re.compile(
            rf"^remi-native-oracles-{re.escape(profile)}-([1-9][0-9]*)-{oracle_id}$"
        )
        for profile in PUBLIC_PROFILES
    }
    export_ids: list[str] = []
    for profile in PUBLIC_PROFILES:
        matches = [match for name in oracle_names if (match := patterns[profile].fullmatch(name))]
        if len(matches) != 1:
            fail(f"oracle run must retain exactly one {profile} lane artifact")
        export_ids.append(matches[0].group(1))
    if len(set(export_ids)) != 1 or len(oracle_names) != 3:
        fail("oracle lane artifacts do not bind one exact export run")
    export_id_run = int(export_ids[0])
    if export_id_run != require_run_id(args.export_run_id, "export run id"):
        fail("requested export run differs from the oracle artifact binding")

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
    deployment_id = int(export_matches[0].group(1))
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
        inspection.get("deployment_evidence_schema_version") != 3
        or deployment.get("completion_mode") != "private-candidates"
        or deployment.get("outcome") != "complete"
        or deployment.get("failure_phase") is not None
        or verification.get("schema_version") != 1
    ):
        fail("export evidence does not prove a complete private-candidate deployment")
    export_id = require_identity(verification.get("export_id"), "export identity")
    export_operator = validate_export_operator(
        export_root, export_run, export_id_run, export_id
    )
    input_manifest_sha256 = require_sha256(
        verification.get("manifest", {}).get("sha256"), "export input manifest"
    )
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
    for profile in PUBLIC_PROFILES:
        profile_binding, profile_files = validate_lane(
            lane_arguments[profile],
            profile,
            export_id,
            deployed_commit,
            input_manifest_sha256,
            candidate_digests[profile],
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
        "schema_version": 1,
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
        with tarfile.open(temporary, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            tar_add_plain(archive, manifest_path, "manifest.json")
            for name, path in files:
                tar_add_plain(archive, path, name)
        os.chmod(temporary, 0o600)
        os.replace(temporary, args.output)
    finally:
        temporary.unlink(missing_ok=True)
        manifest_path.unlink(missing_ok=True)

    evidence = {
        "schema_version": 1,
        "survey_id": survey_id,
        "export_id": export_id,
        "workflow_runs": manifest["workflow_runs"],
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


def validate_counts(value: Any, label: str, parts: tuple[str, ...], total: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{label} counts are malformed")
    total_value = exact_nonnegative_int(value.get(total), f"{label}.{total}")
    part_total = sum(exact_nonnegative_int(value.get(key), f"{label}.{key}") for key in parts)
    if part_total != total_value:
        fail(f"{label} counts are inconsistent")
    return value


def validate_candidate_survey(value: Any, profile: dict[str, Any], name: str) -> None:
    if not isinstance(value, dict) or value.get("schema_version") != 1:
        fail(f"{name} is not a Conary resolution survey schema 1 document")
    counts = validate_counts(
        value.get("counts"),
        name,
        ("resolved_roots", "unresolved_roots", "not_installable_roots", "failed_roots"),
        "roots_walked",
    )
    if (
        value.get("profile") != profile["profile"]
        or value.get("profile_revision_sha256") != profile["profile_revision_sha256"]
        or value.get("package_oracle_manifest_sha256")
        != profile["package_oracle_manifest_sha256"]
        or value.get("target_architecture") != profile["target_architecture"]
        or value.get("policy", {}).get("architecture") != profile["target_architecture"]
        or value.get("total_failures") != counts["failed_roots"]
        or not isinstance(counts.get("error_kinds"), list)
    ):
        fail(f"{name} binding or failure counts drifted")


def validate_comparison_survey(value: Any, profile: dict[str, Any], name: str) -> None:
    if not isinstance(value, dict) or value.get("schema_version") != 1:
        fail(f"{name} is not a native resolution comparison survey schema 1 document")
    counts = validate_counts(
        value.get("counts"), name, ("matching_roots", "mismatched_roots"), "roots_walked"
    )
    if (
        value.get("profile") != profile["profile"]
        or value.get("profile_revision_sha256") != profile["profile_revision_sha256"]
        or value.get("package_oracle_manifest_sha256")
        != profile["package_oracle_manifest_sha256"]
        or value.get("oracle_manifest_sha256")
        != profile["native_resolution_manifest_sha256"]
        or value.get("total_mismatches") != counts["mismatched_roots"]
        or not isinstance(counts.get("mismatch_kinds"), list)
        or not isinstance(counts.get("outcome_kind_pairs"), list)
    ):
        fail(f"{name} binding or mismatch counts drifted")


def forbid_private_paths(value: Any, label: str) -> None:
    if isinstance(value, dict):
        for key, item in value.items():
            forbid_private_paths(key, label)
            forbid_private_paths(item, label)
    elif isinstance(value, list):
        for item in value:
            forbid_private_paths(item, label)
    elif isinstance(value, str) and any(
        marker in value for marker in ("/conary/", "/etc/conary/", "/tmp/", "/data/")
    ):
        fail(f"{label} contains a private host path")


def validate_input_evidence(
    path: Path, survey_id: str, export_id: str
) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    value, _ = load_json(path, "resolution-survey input verification", canonical=True)
    evidence = exact_object(
        value,
        {
            "schema_version",
            "survey_id",
            "export_id",
            "workflow_runs",
            "export_operator",
            "deployment",
            "profiles",
            "manifest_sha256",
            "transport",
        },
        "resolution-survey input verification",
    )
    if (
        evidence["schema_version"] != 1
        or evidence["survey_id"] != survey_id
        or evidence["export_id"] != export_id
    ):
        fail("resolution-survey input verification request binding drifted")
    workflow_runs = exact_object(
        evidence["workflow_runs"], {"oracle", "export", "deployment"}, "input workflow runs"
    )
    for name, run_id in workflow_runs.items():
        if exact_nonnegative_int(run_id, f"input {name} run") == 0:
            fail(f"input {name} run must be positive")
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
        export_operator["schema_version"] != 1
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
    require_sha256(evidence["manifest_sha256"], "input transport manifest")
    transport = exact_object(
        evidence["transport"], {"sha256", "size"}, "input oracle transport"
    )
    require_sha256(transport["sha256"], "input oracle transport digest")
    if exact_nonnegative_int(transport["size"], "input oracle transport size") == 0:
        fail("input oracle transport size must be positive")
    return deployment, profiles


def verify_output(args: argparse.Namespace) -> None:
    survey_id = require_identity(args.survey_id, "survey id")
    export_id = require_identity(args.export_id, "export id")
    input_deployment, input_profiles = validate_input_evidence(
        args.input_evidence, survey_id, export_id
    )
    metadata = plain_file(args.transport, "survey transport", MAX_SURVEY_TRANSPORT_BYTES)
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
        manifest_member = members.get("manifest.json")
        if manifest_member is None:
            fail("survey transport has no manifest.json")
        manifest_bytes = read_tar_member(archive, manifest_member, MAX_MANIFEST_BYTES)
        manifest = decode_json(manifest_bytes, "survey manifest")
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
            manifest["schema_version"] != 1
            or manifest["survey_id"] != survey_id
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
        file_bytes: dict[str, bytes] = {}
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
            data = read_tar_member(archive, member, metadata.st_size)
            if sha256_bytes(data) != expected_sha256:
                fail(f"survey file {name} changed digest")
            file_bytes[name] = data
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
            {"file", "counts", "total_failures", "error_histogram"},
            f"{profile_name} candidate summary",
        )
        expected_candidate_name = f"{profile_name}.candidate-resolution-survey.json"
        if (
            candidate.get("file") != expected_candidate_name
            or expected_candidate_name not in file_bytes
        ):
            fail(f"{profile_name} candidate survey file binding is missing")
        candidate_data = file_bytes[candidate["file"]]
        referenced_files.add(candidate["file"])
        candidate_value = decode_json(candidate_data, f"{profile_name} candidate survey")
        if canonical_json(candidate_value) != candidate_data:
            fail(f"{profile_name} candidate survey is not canonical JSON")
        validate_candidate_survey(candidate_value, profile, candidate["file"])
        if (
            candidate.get("counts") != candidate_value["counts"]
            or candidate.get("total_failures") != candidate_value["total_failures"]
            or candidate.get("error_histogram") != candidate_value["counts"]["error_kinds"]
        ):
            fail(f"{profile_name} candidate summary differs from its survey")
        roots_walked += candidate_value["counts"]["roots_walked"]
        candidate_failures += candidate_value["total_failures"]

        comparison = profile["comparison"]
        if comparison is None:
            if candidate_value["total_failures"] == 0:
                fail(f"{profile_name} omitted comparison without candidate failures")
            continue
        if candidate_value["total_failures"] != 0 or not isinstance(comparison, dict):
            fail(f"{profile_name} retained comparison for an incomplete candidate")
        comparison = exact_object(
            comparison,
            {
                "file",
                "counts",
                "total_mismatches",
                "mismatch_histogram",
                "outcome_histogram",
            },
            f"{profile_name} comparison summary",
        )
        comparison_name = comparison.get("file")
        expected_comparison_name = (
            f"{profile_name}.native-resolution-comparison-survey.json"
        )
        if comparison_name != expected_comparison_name or comparison_name not in file_bytes:
            fail(f"{profile_name} comparison survey file binding is missing")
        comparison_data = file_bytes[comparison_name]
        referenced_files.add(comparison_name)
        comparison_value = decode_json(
            comparison_data, f"{profile_name} comparison survey"
        )
        if canonical_json(comparison_value) != comparison_data:
            fail(f"{profile_name} comparison survey is not canonical JSON")
        validate_comparison_survey(comparison_value, profile, comparison_name)
        if (
            comparison.get("counts") != comparison_value["counts"]
            or comparison.get("total_mismatches") != comparison_value["total_mismatches"]
            or comparison.get("mismatch_histogram")
            != comparison_value["counts"]["mismatch_kinds"]
            or comparison.get("outcome_histogram")
            != comparison_value["counts"]["outcome_kind_pairs"]
        ):
            fail(f"{profile_name} comparison summary differs from its survey")
        comparison_profiles += 1
        comparison_mismatches += comparison_value["total_mismatches"]

    if referenced_files != set(file_bytes):
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
    if counts != {
        "profiles": 3,
        "roots_walked": roots_walked,
        "candidate_failures": candidate_failures,
        "comparison_profiles": comparison_profiles,
        "comparison_mismatches": comparison_mismatches,
    }:
        fail("survey manifest aggregate counts disagree with its files")
    forbid_private_paths(manifest, "survey manifest")
    for name, data in file_bytes.items():
        forbid_private_paths(decode_json(data, name), name)

    evidence = {
        "schema_version": 1,
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


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    build = subparsers.add_parser("build-input")
    build.add_argument("--survey-id", required=True)
    build.add_argument("--repository", required=True)
    build.add_argument("--oracle-run-id", required=True)
    build.add_argument("--oracle-run", required=True, type=Path)
    build.add_argument("--oracle-artifacts", required=True, type=Path)
    build.add_argument("--export-run-id", required=True)
    build.add_argument("--export-run", required=True, type=Path)
    build.add_argument("--export-artifacts", required=True, type=Path)
    build.add_argument("--deployment-run-id", required=True)
    build.add_argument("--deployment-run", required=True, type=Path)
    build.add_argument("--export-root", required=True, type=Path)
    build.add_argument("--lane", action="append", default=[], required=True)
    build.add_argument("--output", required=True, type=Path)
    build.add_argument("--evidence", required=True, type=Path)
    verify = subparsers.add_parser("verify-output")
    verify.add_argument("--survey-id", required=True)
    verify.add_argument("--export-id", required=True)
    verify.add_argument("--input-evidence", required=True, type=Path)
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
    except (OSError, tarfile.TarError, ValidationError) as error:
        raise SystemExit(f"Remi resolution-survey transport validation failed: {error}") from error
