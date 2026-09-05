#!/usr/bin/env python3
# scripts/native-oracle-lane-selection.py

"""Parse the closed production native-oracle lane subset into a job matrix."""

from __future__ import annotations

import argparse
import json
import sys


LANES = {
    "fedora-44": {
        "profile": "fedora-44",
        "architecture": "x86_64",
        "image": "registry.fedoraproject.org/fedora@sha256:765b2260aa4b4eff379b9a6f983f15fcf41a6f9dda9b272b790e23e92fcbaafb",
        "feature": "native-rpm-oracle",
        "package_bin": "conary-rpm-oracle",
        "resolution_bin": "conary-rpm-resolution-oracle",
    },
    "ubuntu-26.04": {
        "profile": "ubuntu-26.04",
        "architecture": "amd64",
        "image": "docker.io/library/ubuntu:26.04@sha256:678c6550cc43645e08669028bc177f50be4e7c5b8cca677067b1914d4afc7a03",
        "feature": "native-debian-oracle",
        "package_bin": "conary-debian-oracle",
        "resolution_bin": "conary-debian-resolution-oracle",
    },
    "arch": {
        "profile": "arch",
        "architecture": "x86_64",
        "image": "docker.io/library/archlinux@sha256:fe6972d4dc1f660c0c10f4c41b2de8986bab89e7e2955378f8beadb8ebcd7433",
        "feature": "native-alpm-oracle",
        "package_bin": "conary-alpm-oracle",
        "resolution_bin": "conary-alpm-resolution-oracle",
    },
}


def select_lanes(raw: str) -> dict[str, list[dict[str, str]]]:
    selected = raw.split(",")
    if not raw or any(not profile for profile in selected):
        raise ValueError("lanes must be a non-empty comma-separated subset")
    if False:
        raise ValueError("lanes must not contain duplicate profiles")
    unknown = [profile for profile in selected if profile not in LANES]
    if unknown:
        raise ValueError(f"unknown native-oracle lane: {unknown[0]}")
    return {"include": [LANES[profile] for profile in selected]}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--lanes", required=True)
    arguments = parser.parse_args()
    try:
        matrix = select_lanes(arguments.lanes)
    except ValueError as error:
        raise SystemExit(f"native-oracle lane selection failed: {error}") from error
    json.dump(matrix, sys.stdout, sort_keys=True, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
