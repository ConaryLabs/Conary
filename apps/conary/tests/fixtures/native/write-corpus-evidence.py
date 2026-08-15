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


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("fixture_manifest", type=Path)
    parser.add_argument("evidence_path", type=Path)
    parser.add_argument("source_profile")
    parser.add_argument("source_format")
    parser.add_argument("package_name")
    parser.add_argument("package_version")
    parser.add_argument("architecture")
    parser.add_argument("completed_stage")
    args = parser.parse_args()

    fixture = json.loads(args.fixture_manifest.read_text(encoding="utf-8"))
    if fixture.get("schema_version") != 1:
        raise ValueError("unsupported native fixture manifest schema")
    artifact_path = Path(fixture["artifact_path"])
    observed_digest = sha256(artifact_path)
    if observed_digest != fixture["sha256"]:
        raise ValueError("native fixture artifact digest contradicts its build manifest")
    evidence = {
        "schema_version": 1,
        "source_profile": args.source_profile,
        "source_format": args.source_format,
        "source_artifacts": [
            {
                "role": "install_request",
                "digest_source": "fixture_build_manifest",
                "name": args.package_name,
                "version": args.package_version,
                "architecture": args.architecture,
                "digest": observed_digest,
            }
        ],
        "completed_stages": [args.completed_stage],
        "active_stage": None,
    }

    temporary_path = args.evidence_path.with_suffix(args.evidence_path.suffix + ".tmp")
    temporary_path.write_text(
        json.dumps(evidence, separators=(",", ":")) + "\n", encoding="utf-8"
    )
    os.replace(temporary_path, args.evidence_path)


if __name__ == "__main__":
    main()
