#!/usr/bin/env python3
"""Reject direct dependency drift across workspace member manifests."""

from __future__ import annotations

import pathlib
import sys
import tomllib
from collections import defaultdict
from dataclasses import dataclass


ROOT = pathlib.Path(__file__).resolve().parent.parent
ROOT_MANIFEST = ROOT / "Cargo.toml"
DEPENDENCY_SECTIONS = ("dependencies", "dev-dependencies", "build-dependencies")


@dataclass(frozen=True)
class DepUse:
    member: str
    section: str
    mode: str
    detail: str


def load_toml(path: pathlib.Path) -> dict:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def iter_dependency_tables(data: dict, prefix: str = ""):
    for section in DEPENDENCY_SECTIONS:
        deps = data.get(section)
        if isinstance(deps, dict):
            yield f"{prefix}{section}", deps

    targets = data.get("target")
    if not isinstance(targets, dict):
        return

    for target_name, target_data in targets.items():
        if isinstance(target_data, dict):
            yield from iter_dependency_tables(target_data, f"{prefix}target.{target_name}.")


def classify_spec(spec: object) -> tuple[str, str]:
    if isinstance(spec, str):
        return ("explicit", spec)
    if isinstance(spec, dict):
        if spec.get("workspace") is True:
            return ("workspace", "workspace")
        if "path" in spec and "version" not in spec:
            return ("path", str(spec["path"]))
        if "version" in spec:
            return ("explicit", str(spec["version"]))
        return ("other", repr(spec))
    return ("other", repr(spec))


def main() -> int:
    root = load_toml(ROOT_MANIFEST)
    members = root["workspace"]["members"]

    uses: dict[str, list[DepUse]] = defaultdict(list)
    for member in members:
        manifest_path = ROOT / member / "Cargo.toml"
        manifest = load_toml(manifest_path)
        for section, deps in iter_dependency_tables(manifest):
            for dep_name, spec in deps.items():
                mode, detail = classify_spec(spec)
                uses[dep_name].append(DepUse(member, section, mode, detail))

    failures: list[str] = []
    for dep_name, dep_uses in sorted(uses.items()):
        member_count = len({dep.member for dep in dep_uses})
        if member_count < 2:
            continue

        direct_uses = [dep for dep in dep_uses if dep.mode in {"explicit", "workspace"}]
        if not direct_uses:
            continue

        modes = {dep.mode for dep in direct_uses}
        explicit_versions = {dep.detail for dep in direct_uses if dep.mode == "explicit"}

        if modes == {"workspace"}:
            continue

        lines = [f"{dep_name}:"]
        for dep in direct_uses:
            lines.append(
                f"  - {dep.member} [{dep.section}] uses {dep.mode} {dep.detail}"
            )

        if len(explicit_versions) > 1:
            lines.append("  Fix: move this crate to [workspace.dependencies] and use workspace = true everywhere.")
        elif "workspace" in modes and "explicit" in modes:
            lines.append("  Fix: stop mixing workspace and explicit declarations for the same shared crate.")
        else:
            lines.append("  Fix: shared direct dependencies must come from [workspace.dependencies].")

        failures.append("\n".join(lines))

    if failures:
        print("Rust dependency consistency check failed.\n")
        print("\n\n".join(failures))
        return 1

    print("Rust dependency consistency check passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
