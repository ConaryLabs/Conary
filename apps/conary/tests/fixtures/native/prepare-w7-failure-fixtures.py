#!/usr/bin/env python3
"""Derive exact interrupted-download and corrupt-native W7 fixture authority."""

import hashlib
import json
import os
import sys
from pathlib import Path


def atomic_json(path: Path, value: object) -> None:
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, separators=(",", ":")) + "\n", encoding="utf-8")
    os.replace(temporary, path)


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def main() -> None:
    if len(sys.argv) != 4:
        raise SystemExit("usage: prepare-w7-failure-fixtures.py <pinned-ccs> <native-package> <output-dir>")
    pinned_ccs = Path(sys.argv[1])
    native_package = Path(sys.argv[2])
    output = Path(sys.argv[3])
    output.mkdir(parents=True, exist_ok=True)

    ccs_bytes = pinned_ccs.read_bytes()
    source_metadata = json.loads((pinned_ccs.parent / "metadata.json").read_text(encoding="utf-8"))
    source_package = source_metadata["packages"][0]
    interrupted_package = dict(source_package)
    interrupted_package.update({
        "name": "phase4-w7-interrupted-download",
        "description": "Exact unreachable W7 download fixture",
        "checksum": sha256(ccs_bytes),
        "size": len(ccs_bytes),
        "download_url": "http://127.0.0.1:9/phase4-w7-interrupted-download-1.0.0-1.ccs",
    })
    interrupted = {
        "name": "phase4-w7-interrupted",
        "version": "1",
        "security_advisory_source": None,
        "packages": [interrupted_package],
    }
    atomic_json(output / "interrupted-metadata.json", interrupted)
    atomic_json(output / "interrupted-fixture-manifest.json", {
        "schema_version": 1,
        "artifact_path": str(pinned_ccs),
        "builder_target": "conary",
        "sha256": sha256(ccs_bytes),
    })

    native_bytes = native_package.read_bytes()
    if len(native_bytes) < 64:
        raise ValueError("native fixture is too small to truncate deterministically")
    corrupt_path = output / ("corrupt-" + native_package.name)
    corrupt_bytes = native_bytes[: len(native_bytes) // 2]
    corrupt_path.write_bytes(corrupt_bytes)
    atomic_json(output / "corrupt-native-fixture-manifest.json", {
        "schema_version": 1,
        "artifact_path": str(corrupt_path),
        "builder_target": "corrupt-native",
        "sha256": sha256(corrupt_bytes),
    })


if __name__ == "__main__":
    main()
