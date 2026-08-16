#!/usr/bin/env python3
"""Write one atomic, versioned corpus evidence envelope for a native fixture."""

import argparse
import hashlib
import json
import os
from pathlib import Path


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def artifact_identity(
    fixture_manifest: Path,
    role: str,
    package_name: str,
    package_version: str,
    architecture: str,
) -> dict[str, str]:
    fixture = json.loads(fixture_manifest.read_text(encoding="utf-8"))
    if fixture.get("schema_version") != 1:
        raise ValueError("unsupported native fixture manifest schema")
    artifact_path = Path(fixture["artifact_path"])
    observed_digest = sha256(artifact_path)
    if observed_digest != fixture["sha256"]:
        raise ValueError("native fixture artifact digest contradicts its build manifest")
    return {
        "role": role,
        "digest_source": "fixture_build_manifest",
        "name": package_name,
        "version": package_version,
        "architecture": architecture,
        "digest": observed_digest,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("fixture_manifest", type=Path)
    parser.add_argument("evidence_path", type=Path)
    parser.add_argument("source_profile")
    parser.add_argument("source_format")
    parser.add_argument("package_name")
    parser.add_argument("package_version")
    parser.add_argument("architecture")
    parser.add_argument("completed_stages", nargs="+")
    parser.add_argument("--dependency-fixture-manifest", type=Path)
    parser.add_argument("--dependency-name")
    parser.add_argument("--dependency-version")
    parser.add_argument("--dependency-architecture")
    args = parser.parse_args()

    dependency_values = [
        args.dependency_fixture_manifest,
        args.dependency_name,
        args.dependency_version,
        args.dependency_architecture,
    ]
    if any(value is not None for value in dependency_values) and not all(
        value is not None for value in dependency_values
    ):
        parser.error("dependency artifact identity requires all dependency arguments")

    source_artifacts = [
        artifact_identity(
            args.fixture_manifest,
            "install_request",
            args.package_name,
            args.package_version,
            args.architecture,
        )
    ]
    if args.dependency_fixture_manifest is not None:
        source_artifacts.append(
            artifact_identity(
                args.dependency_fixture_manifest,
                "install_dependency",
                args.dependency_name,
                args.dependency_version,
                args.dependency_architecture,
            )
        )

    evidence = {
        "schema_version": 1,
        "source_profile": args.source_profile,
        "source_format": args.source_format,
        "source_artifacts": source_artifacts,
        "completed_stages": args.completed_stages,
        "active_stage": None,
    }

    temporary_path = args.evidence_path.with_suffix(args.evidence_path.suffix + ".tmp")
    temporary_path.write_text(
        json.dumps(evidence, separators=(",", ":")) + "\n", encoding="utf-8"
    )
    os.replace(temporary_path, args.evidence_path)


if __name__ == "__main__":
    main()
