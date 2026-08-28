#!/usr/bin/env python3
# scripts/verify-native-oracle-input-transport.py

"""Verify and safely reopen a production native-oracle input transport."""

from __future__ import annotations

import argparse
import hashlib
import ipaddress
import json
import os
from pathlib import Path, PurePosixPath
import stat
import tarfile
from typing import Any, NoReturn
from urllib.parse import urlsplit


PUBLIC_PROFILES = ("fedora-44", "ubuntu-26.04", "arch")
PROFILE_ROLES = {
    "base",
    "updates",
    "security",
    "backports",
    "overlay",
    "optional",
    "debug",
    "source",
}
OBJECT_ROLE_ORDER = {
    "rpm_primary": 0,
    "rpm_filelists": 1,
    "debian_packages": 2,
    "arch_database": 3,
    "eopkg_index": 4,
}
STREAM_KINDS = {"release", "channel", "rolling"}
MAX_MANIFEST_BYTES = 16 * 1024 * 1024
MAX_TRANSPORT_BYTES = 16 * 1024 * 1024 * 1024


class VerificationError(ValueError):
    """A transport or manifest failed its exact contract."""


def fail(message: str) -> NoReturn:
    raise VerificationError(message)


def exact_keys(value: Any, expected: set[str], label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{label} must be an object")
    actual = set(value)
    if actual != expected:
        fail(
            f"{label} fields differ: missing={sorted(expected - actual)} "
            f"unexpected={sorted(actual - expected)}"
        )
    return value


def exact_list(value: Any, label: str) -> list[Any]:
    if not isinstance(value, list):
        fail(f"{label} must be an array")
    return value


def exact_string(value: Any, label: str) -> str:
    if not isinstance(value, str):
        fail(f"{label} must be a string")
    return value


def exact_bool(value: Any, label: str) -> bool:
    if not isinstance(value, bool):
        fail(f"{label} must be a boolean")
    return value


def exact_int(value: Any, label: str, *, minimum: int | None = None) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        fail(f"{label} must be an integer")
    if minimum is not None and value < minimum:
        fail(f"{label} must be at least {minimum}")
    return value


def sha256_string(value: Any, label: str) -> str:
    value = exact_string(value, label)
    if len(value) != 64 or any(ch not in "0123456789abcdef" for ch in value):
        fail(f"{label} must be 64 lowercase hexadecimal characters")
    return value


def identity(value: Any, label: str) -> str:
    value = exact_string(value, label)
    if (
        not value
        or len(value) > 255
        or value.strip() != value
        or any(ord(ch) < 0x20 or ord(ch) > 0x7E for ch in value)
    ):
        fail(f"{label} must be 1..255 printable ASCII characters without padding")
    return value


def canonical_json(value: Any) -> bytes:
    return json.dumps(
        value,
        ensure_ascii=False,
        allow_nan=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")


def digest_json(value: Any) -> str:
    return hashlib.sha256(canonical_json(value)).hexdigest()


def reject_duplicate_key(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            fail(f"manifest repeats JSON key {key!r}")
        result[key] = value
    return result


def load_manifest(data: bytes) -> dict[str, Any]:
    try:
        value = json.loads(
            data,
            object_pairs_hook=reject_duplicate_key,
            parse_constant=lambda constant: fail(
                f"manifest contains invalid JSON constant {constant!r}"
            ),
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        fail(f"manifest is not strict UTF-8 JSON: {error}")
    if canonical_json(value) != data:
        fail("manifest is not canonical JSON")
    return exact_keys(value, {"schema_version", "profiles", "objects"}, "manifest")


def validate_public_string(value: Any, label: str) -> None:
    if isinstance(value, str):
        if value.startswith("/") or value.startswith("file://"):
            fail(f"{label} contains a host-local path")
        if "://" in value:
            parsed = urlsplit(value)
            if (
                parsed.scheme != "https"
                or not parsed.hostname
                or parsed.username is not None
                or parsed.password is not None
            ):
                fail(f"{label} contains a non-public or credential-bearing URL")
            host = parsed.hostname.lower()
            if host == "localhost" or host.endswith(".localhost"):
                fail(f"{label} contains a localhost URL")
            try:
                address = ipaddress.ip_address(host)
            except ValueError:
                pass
            else:
                if not address.is_global:
                    fail(f"{label} contains a non-public IP URL")
    elif isinstance(value, list):
        for index, item in enumerate(value):
            validate_public_string(item, f"{label}[{index}]")
    elif isinstance(value, dict):
        for key, item in value.items():
            validate_public_string(item, f"{label}.{key}")


def validate_artifact(value: Any, label: str) -> tuple[str, int]:
    value = exact_keys(value, {"sha256", "size"}, label)
    return (
        sha256_string(value["sha256"], f"{label}.sha256"),
        exact_int(value["size"], f"{label}.size", minimum=0),
    )


def validate_counts(value: Any, label: str) -> None:
    value = exact_keys(
        value,
        {
            "packages",
            "provides",
            "requirement_groups",
            "requirement_atoms",
            "source_evidence",
        },
        label,
    )
    for key, item in value.items():
        exact_int(item, f"{label}.{key}", minimum=0)


def validate_stream(value: Any, label: str) -> None:
    value = exact_keys(value, {"kind", "identity"}, label)
    kind = exact_string(value["kind"], f"{label}.kind")
    if kind not in STREAM_KINDS:
        fail(f"{label}.kind is unsupported")
    identity(value["identity"], f"{label}.identity")


def validate_provenance(value: Any, label: str) -> None:
    value = exact_keys(
        value,
        {
            "ecosystem",
            "metadata_url",
            "content_url",
            "parser_config",
            "parser_config_sha256",
            "trust_policy",
            "trust_policy_sha256",
        },
        label,
    )
    ecosystem = exact_string(value["ecosystem"], f"{label}.ecosystem")
    formats = {
        "rpm": ("rpm", "rpm"),
        "deb": ("deb", "debian"),
        "alpm": ("arch", "arch"),
        "eopkg": ("eopkg", "eopkg"),
    }
    if ecosystem not in formats:
        fail(f"{label}.ecosystem is unsupported")
    metadata_url = exact_string(value["metadata_url"], f"{label}.metadata_url")
    if not metadata_url:
        fail(f"{label}.metadata_url must not be empty")
    content_url = value["content_url"]
    if content_url is not None:
        exact_string(content_url, f"{label}.content_url")
    parser = value["parser_config"]
    trust = value["trust_policy"]
    if not isinstance(parser, dict) or not isinstance(trust, dict):
        fail(f"{label} parser and trust policies must be objects")
    parser_format, trust_format = formats[ecosystem]
    if parser.get("package_format") != parser_format:
        fail(f"{label}.parser_config disagrees with its ecosystem")
    if trust.get("ecosystem") != trust_format:
        fail(f"{label}.trust_policy disagrees with its ecosystem")
    parser_digest = sha256_string(
        value["parser_config_sha256"], f"{label}.parser_config_sha256"
    )
    trust_digest = sha256_string(
        value["trust_policy_sha256"], f"{label}.trust_policy_sha256"
    )
    if digest_json(parser) != parser_digest:
        fail(f"{label}.parser_config digest drifted")
    if digest_json(trust) != trust_digest:
        fail(f"{label}.trust_policy digest drifted")
    validate_public_string(value, label)


def validate_source(
    value: Any,
    label: str,
    profile_name: str,
    member: dict[str, Any],
) -> list[tuple[str, int]]:
    value = exact_keys(
        value,
        {
            "schema_version",
            "source_profile",
            "source_identity",
            "repository_identity",
            "stream",
            "stream_binding_sha256",
            "parser_projection_version",
            "provenance",
            "authenticated_root",
            "authenticated_objects",
            "catalog",
            "logical_digest_sha256",
            "counts",
        },
        label,
    )
    if exact_int(value["schema_version"], f"{label}.schema_version") != 1:
        fail(f"{label} uses an unsupported schema")
    if exact_string(value["source_profile"], f"{label}.source_profile") != profile_name:
        fail(f"{label} names the wrong public profile")
    source_identity = identity(value["source_identity"], f"{label}.source_identity")
    repository_identity = identity(
        value["repository_identity"], f"{label}.repository_identity"
    )
    validate_stream(value["stream"], f"{label}.stream")
    sha256_string(value["stream_binding_sha256"], f"{label}.stream_binding_sha256")
    if exact_int(
        value["parser_projection_version"], f"{label}.parser_projection_version"
    ) != 2:
        fail(f"{label} uses an unsupported parser projection")
    validate_provenance(value["provenance"], f"{label}.provenance")
    validate_artifact(value["authenticated_root"], f"{label}.authenticated_root")
    validate_artifact(value["catalog"], f"{label}.catalog")
    sha256_string(value["logical_digest_sha256"], f"{label}.logical_digest_sha256")
    validate_counts(value["counts"], f"{label}.counts")

    if (
        source_identity != member["source_identity"]
        or repository_identity != member["repository_identity"]
        or value["stream"] != member["stream"]
    ):
        fail(f"{label} disagrees with its profile member")
    expected_digest = sha256_string(
        member["source_snapshot_sha256"],
        f"{label}.member_source_snapshot_sha256",
    )
    if digest_json(value) != expected_digest:
        fail(f"{label} canonical digest disagrees with its profile member")

    objects = exact_list(value["authenticated_objects"], f"{label}.authenticated_objects")
    result: list[tuple[str, int]] = []
    previous: tuple[int, str] | None = None
    roles: set[str] = set()
    for index, item in enumerate(objects):
        item_label = f"{label}.authenticated_objects[{index}]"
        item = exact_keys(item, {"role", "source_path", "sha256", "size"}, item_label)
        role = exact_string(item["role"], f"{item_label}.role")
        if role not in OBJECT_ROLE_ORDER or role in roles:
            fail(f"{item_label}.role is unsupported or repeated")
        roles.add(role)
        source_path = exact_string(item["source_path"], f"{item_label}.source_path")
        path = PurePosixPath(source_path)
        if (
            not source_path
            or path.is_absolute()
            or any(part in {"", ".", ".."} for part in path.parts)
            or path.as_posix() != source_path
        ):
            fail(f"{item_label}.source_path is not a canonical relative path")
        key = (OBJECT_ROLE_ORDER[role], source_path)
        if previous is not None and previous >= key:
            fail(f"{label}.authenticated_objects are not strictly ordered")
        previous = key
        result.append(
            (
                sha256_string(item["sha256"], f"{item_label}.sha256"),
                exact_int(item["size"], f"{item_label}.size", minimum=0),
            )
        )
    validate_public_string(value, label)
    return result


def validate_revision(value: Any, label: str, profile_name: str) -> list[dict[str, Any]]:
    value = exact_keys(
        value,
        {
            "schema_version",
            "profile",
            "projection_version",
            "members",
            "catalog",
            "logical_digest_sha256",
            "counts",
        },
        label,
    )
    if exact_int(value["schema_version"], f"{label}.schema_version") != 2:
        fail(f"{label} uses an unsupported schema")
    if exact_string(value["profile"], f"{label}.profile") != profile_name:
        fail(f"{label} names the wrong public profile")
    exact_int(value["projection_version"], f"{label}.projection_version", minimum=1)
    validate_artifact(value["catalog"], f"{label}.catalog")
    sha256_string(value["logical_digest_sha256"], f"{label}.logical_digest_sha256")
    validate_counts(value["counts"], f"{label}.counts")

    members = exact_list(value["members"], f"{label}.members")
    if not members:
        fail(f"{label}.members must not be empty")
    repositories: set[str] = set()
    for index, item in enumerate(members):
        item_label = f"{label}.members[{index}]"
        item = exact_keys(
            item,
            {
                "ordinal",
                "role",
                "source_identity",
                "repository_identity",
                "stream",
                "precedence",
                "required",
                "source_snapshot_sha256",
            },
            item_label,
        )
        if exact_int(item["ordinal"], f"{item_label}.ordinal") != index:
            fail(f"{item_label}.ordinal is noncanonical")
        role = exact_string(item["role"], f"{item_label}.role")
        if role not in PROFILE_ROLES:
            fail(f"{item_label}.role is unsupported")
        identity(item["source_identity"], f"{item_label}.source_identity")
        repository = identity(
            item["repository_identity"], f"{item_label}.repository_identity"
        )
        if repository in repositories:
            fail(f"{label} repeats repository identity {repository!r}")
        repositories.add(repository)
        validate_stream(item["stream"], f"{item_label}.stream")
        exact_int(item["precedence"], f"{item_label}.precedence")
        exact_bool(item["required"], f"{item_label}.required")
        sha256_string(
            item["source_snapshot_sha256"], f"{item_label}.source_snapshot_sha256"
        )
    if value["counts"]["source_evidence"] != len(members):
        fail(f"{label}.counts.source_evidence disagrees with its members")
    return members


def validate_manifest(
    manifest: dict[str, Any], expected_candidates: list[tuple[str, str]]
) -> dict[str, Any]:
    if exact_int(manifest["schema_version"], "manifest.schema_version") != 1:
        fail("manifest uses an unsupported schema")
    profiles = exact_list(manifest["profiles"], "manifest.profiles")
    if len(profiles) != len(PUBLIC_PROFILES):
        fail("manifest must contain exactly the three public profiles")

    object_authority: dict[str, int] = {}
    profile_evidence: list[dict[str, Any]] = []
    total_sources = 0
    for index, ((expected_profile, expected_digest), profile) in enumerate(
        zip(expected_candidates, profiles, strict=True)
    ):
        label = f"manifest.profiles[{index}]"
        profile = exact_keys(
            profile, {"profile_revision_sha256", "revision", "sources"}, label
        )
        revision = profile["revision"]
        members = validate_revision(revision, f"{label}.revision", expected_profile)
        observed_digest = sha256_string(
            profile["profile_revision_sha256"], f"{label}.profile_revision_sha256"
        )
        if observed_digest != expected_digest or digest_json(revision) != observed_digest:
            fail(f"{label} does not match the expected candidate revision")
        sources = exact_list(profile["sources"], f"{label}.sources")
        if len(sources) != len(members):
            fail(f"{label}.sources disagree with the revision member count")
        profile_objects: set[str] = set()
        profile_bytes = 0
        for source_index, (source, member) in enumerate(zip(sources, members, strict=True)):
            for digest, size in validate_source(
                source,
                f"{label}.sources[{source_index}]",
                expected_profile,
                member,
            ):
                existing = object_authority.get(digest)
                if existing is not None and existing != size:
                    fail(f"metadata object {digest} has conflicting sizes")
                object_authority[digest] = size
                if digest not in profile_objects:
                    profile_objects.add(digest)
                    profile_bytes += size
        total_sources += len(sources)
        profile_evidence.append(
            {
                "profile": expected_profile,
                "profile_revision_sha256": observed_digest,
                "sources": len(sources),
                "objects": len(profile_objects),
                "object_bytes": profile_bytes,
            }
        )

    objects = exact_list(manifest["objects"], "manifest.objects")
    observed_inventory: list[tuple[str, int]] = []
    for index, item in enumerate(objects):
        label = f"manifest.objects[{index}]"
        item = exact_keys(item, {"sha256", "size"}, label)
        observed_inventory.append(
            (
                sha256_string(item["sha256"], f"{label}.sha256"),
                exact_int(item["size"], f"{label}.size", minimum=0),
            )
        )
    expected_inventory = sorted(object_authority.items())
    if not observed_inventory or observed_inventory != expected_inventory:
        fail("manifest object inventory is incomplete, reordered, or inconsistent")

    validate_public_string(manifest, "manifest")
    return {
        "profiles": profile_evidence,
        "sources": total_sources,
        "objects": len(expected_inventory),
        "object_bytes": sum(size for _, size in expected_inventory),
        "inventory": expected_inventory,
    }


def plain_file(path: Path, label: str, maximum: int) -> os.stat_result:
    try:
        metadata = path.lstat()
    except OSError as error:
        fail(f"cannot inspect {label}: {error}")
    if not stat.S_ISREG(metadata.st_mode) or path.is_symlink():
        fail(f"{label} must be a plain file")
    if metadata.st_size <= 0 or metadata.st_size > maximum:
        fail(f"{label} size is outside its bounded contract")
    return metadata


def validate_member_name(name: str) -> str:
    path = PurePosixPath(name)
    if (
        not name
        or name.startswith("/")
        or name.endswith("/")
        or path.as_posix() != name
        or any(part in {"", ".", ".."} for part in path.parts)
    ):
        fail(f"transport contains unsafe or noncanonical member {name!r}")
    return name


def read_exact_member(archive: tarfile.TarFile, member: tarfile.TarInfo) -> bytes:
    stream = archive.extractfile(member)
    if stream is None:
        fail(f"transport member {member.name!r} could not be opened")
    data = stream.read(member.size + 1)
    if len(data) != member.size:
        fail(f"transport member {member.name!r} is truncated")
    if len(data) > member.size:
        fail(f"transport member {member.name!r} exceeds its declared size")
    return data


def open_transport(
    path: Path, export_id: str, expected_candidates: list[tuple[str, str]]
) -> tuple[bytes, dict[str, bytes], dict[str, Any]]:
    metadata = plain_file(path, "native-oracle transport", MAX_TRANSPORT_BYTES)
    try:
        archive = tarfile.open(path, mode="r:")
    except (tarfile.TarError, OSError) as error:
        fail(f"transport is not a valid uncompressed tar archive: {error}")

    with archive:
        members: dict[str, tarfile.TarInfo] = {}
        aggregate_size = 0
        for member in archive:
            name = validate_member_name(member.name)
            if name in members:
                fail(f"transport repeats member {name!r}")
            if not (member.isdir() or member.isreg()):
                fail(f"transport member {name!r} is not a plain file or directory")
            if member.pax_headers or getattr(member, "sparse", None):
                fail(f"transport member {name!r} uses unsupported extended metadata")
            if member.isreg():
                aggregate_size += member.size
                if aggregate_size > MAX_TRANSPORT_BYTES:
                    fail("transport member bytes exceed their bounded contract")
            members[name] = member

        manifest_name = f"{export_id}/manifest.json"
        objects_name = f"{export_id}/objects"
        required_directories = {export_id, objects_name}
        for name in required_directories:
            if name not in members or not members[name].isdir():
                fail(f"transport is missing exact directory {name!r}")
        manifest_member = members.get(manifest_name)
        if (
            manifest_member is None
            or not manifest_member.isreg()
            or manifest_member.size <= 0
            or manifest_member.size > MAX_MANIFEST_BYTES
        ):
            fail("transport manifest is not a bounded plain file")
        manifest_bytes = read_exact_member(archive, manifest_member)
        manifest = load_manifest(manifest_bytes)
        summary = validate_manifest(manifest, expected_candidates)

        object_bytes: dict[str, bytes] = {}
        expected_names = required_directories | {manifest_name}
        for digest, expected_size in summary["inventory"]:
            name = f"{objects_name}/{digest}"
            expected_names.add(name)
            member = members.get(name)
            if member is None or not member.isreg() or member.size != expected_size:
                fail(f"metadata object {digest} is missing or has the wrong size")
            data = read_exact_member(archive, member)
            if hashlib.sha256(data).hexdigest() != digest:
                fail(f"metadata object {digest} failed SHA-256 verification")
            object_bytes[digest] = data
        if set(members) != expected_names:
            fail("transport contains missing or unexpected members")

    summary["transport_size"] = metadata.st_size
    summary["transport_sha256"] = hash_file(path)
    summary["manifest_sha256"] = hashlib.sha256(manifest_bytes).hexdigest()
    return manifest_bytes, object_bytes, summary


def hash_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def require_new_path(path: Path, label: str) -> None:
    if path.exists() or path.is_symlink():
        fail(f"{label} already exists")
    parent = path.parent
    metadata = parent.lstat()
    if not stat.S_ISDIR(metadata.st_mode) or parent.is_symlink():
        fail(f"{label} parent must be a plain directory")


def write_new_file(path: Path, data: bytes) -> None:
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = os.open(path, flags, 0o600)
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


def publish_reopened_bundle(
    output_dir: Path, manifest_bytes: bytes, objects: dict[str, bytes]
) -> None:
    require_new_path(output_dir, "reopened output directory")
    os.mkdir(output_dir, 0o700)
    objects_dir = output_dir / "objects"
    os.mkdir(objects_dir, 0o700)
    write_new_file(output_dir / "manifest.json", manifest_bytes)
    for digest, data in sorted(objects.items()):
        write_new_file(objects_dir / digest, data)


def parse_candidates(values: list[str]) -> list[tuple[str, str]]:
    if len(values) != len(PUBLIC_PROFILES):
        fail("exactly three --expected-candidate bindings are required")
    result: list[tuple[str, str]] = []
    for index, value in enumerate(values):
        profile, separator, digest = value.partition("=")
        if not separator or profile != PUBLIC_PROFILES[index]:
            fail("expected candidates must use canonical Fedora, Ubuntu, Arch order")
        result.append((profile, sha256_string(digest, f"candidate {profile}")))
    return result


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--transport", required=True, type=Path)
    parser.add_argument("--export-id", required=True)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument(
        "--expected-candidate", required=True, action="append", default=[]
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    if (
        not args.export_id
        or len(args.export_id) > 128
        or args.export_id[0] not in "abcdefghijklmnopqrstuvwxyz0123456789"
        or any(ch not in "abcdefghijklmnopqrstuvwxyz0123456789._-" for ch in args.export_id)
    ):
        fail("export identity is not canonical")
    candidates = parse_candidates(args.expected_candidate)
    require_new_path(args.evidence, "verification evidence")
    manifest_bytes, objects, summary = open_transport(
        args.transport, args.export_id, candidates
    )
    publish_reopened_bundle(args.output_dir, manifest_bytes, objects)
    evidence = {
        "schema_version": 1,
        "export_id": args.export_id,
        "transport": {
            "sha256": summary["transport_sha256"],
            "size": summary["transport_size"],
        },
        "manifest": {"sha256": summary["manifest_sha256"]},
        "profiles": summary["profiles"],
        "counts": {
            "profiles": len(summary["profiles"]),
            "sources": summary["sources"],
            "objects": summary["objects"],
            "object_bytes": summary["object_bytes"],
        },
    }
    write_new_file(args.evidence, canonical_json(evidence))
    print(canonical_json(evidence).decode("utf-8"))


if __name__ == "__main__":
    try:
        main()
    except (OSError, tarfile.TarError, VerificationError) as error:
        raise SystemExit(f"native-oracle transport verification failed: {error}") from error
