#!/usr/bin/env python3
"""Validate and audit Conary's third-party dependency divergence inventory."""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Iterable


START_MARKER = "<!-- conary-third-party-divergence:start -->"
END_MARKER = "<!-- conary-third-party-divergence:end -->"
HEX_REV = re.compile(r"^[0-9a-f]{40}$")
STABLE_VERSION = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")
DEPENDENCY_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")


class InventoryError(Exception):
    """One or more inventory invariants failed."""


@dataclass(frozen=True)
class Declaration:
    name: str
    kind: str
    declaration: str
    manifest: Path
    value: dict[str, Any]


def load_toml(path: Path) -> dict[str, Any]:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise InventoryError(f"{path}: cannot read TOML: {error}") from error


def require_text(entry: dict[str, Any], field: str, label: str) -> str:
    value = entry.get(field)
    if not isinstance(value, str) or not value.strip():
        raise InventoryError(f"{label}: {field} must be a non-empty string")
    return value


def load_inventory(root: Path) -> dict[str, Any]:
    path = root / "third_party/README.md"
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        raise InventoryError(f"{path}: cannot read divergence inventory: {error}") from error

    if text.count(START_MARKER) != 1 or text.count(END_MARKER) != 1:
        raise InventoryError(
            f"{path}: expected exactly one {START_MARKER!r} and {END_MARKER!r} marker"
        )
    body = text.split(START_MARKER, 1)[1].split(END_MARKER, 1)[0].strip()
    match = re.fullmatch(r"```toml\s*\n(?P<toml>.*)\n```", body, re.DOTALL)
    if match is None:
        raise InventoryError(f"{path}: marked inventory must be one fenced TOML block")
    try:
        inventory = tomllib.loads(match.group("toml"))
    except tomllib.TOMLDecodeError as error:
        raise InventoryError(f"{path}: invalid inventory TOML: {error}") from error
    if inventory.get("schema") != 1:
        raise InventoryError(f"{path}: inventory schema must be 1")
    require_text(inventory, "cadence", "inventory")
    require_text(inventory, "owner", "inventory")
    dependencies = inventory.get("dependency", [])
    if not isinstance(dependencies, list) or not all(
        isinstance(entry, dict) for entry in dependencies
    ):
        raise InventoryError(f"{path}: dependency must be an array of tables")
    return inventory


def workspace_manifests(root: Path) -> list[Path]:
    root_manifest = root / "Cargo.toml"
    data = load_toml(root_manifest)
    workspace = data.get("workspace")
    if not isinstance(workspace, dict):
        raise InventoryError(f"{root_manifest}: missing [workspace] table")
    members = workspace.get("members", [])
    if not isinstance(members, list) or not all(isinstance(item, str) for item in members):
        raise InventoryError(f"{root_manifest}: workspace.members must be an array of paths")
    manifests = {root_manifest}
    for member in members:
        matches = sorted(root.glob(member))
        if not matches:
            raise InventoryError(f"{root_manifest}: workspace member {member!r} matched no path")
        for match in matches:
            manifest = match if match.name == "Cargo.toml" else match / "Cargo.toml"
            if not manifest.is_file():
                raise InventoryError(
                    f"{root_manifest}: workspace member has no Cargo.toml: {match}"
                )
            manifests.add(manifest)
    return sorted(manifests)


def dependency_tables(data: dict[str, Any]) -> Iterable[tuple[str, dict[str, Any]]]:
    for table_name in DEPENDENCY_TABLES:
        table = data.get(table_name)
        if isinstance(table, dict):
            yield table_name, table

    workspace = data.get("workspace")
    if isinstance(workspace, dict):
        table = workspace.get("dependencies")
        if isinstance(table, dict):
            yield "workspace.dependencies", table

    targets = data.get("target")
    if isinstance(targets, dict):
        for target_name, target in targets.items():
            if not isinstance(target, dict):
                continue
            for table_name in DEPENDENCY_TABLES:
                table = target.get(table_name)
                if isinstance(table, dict):
                    yield f"target.{target_name}.{table_name}", table


def discover_declarations(root: Path) -> dict[str, Declaration]:
    declarations: dict[str, Declaration] = {}
    for manifest in workspace_manifests(root):
        data = load_toml(manifest)
        relative_manifest = manifest.relative_to(root)

        patches = data.get("patch")
        if isinstance(patches, dict):
            for registry, table in patches.items():
                if not isinstance(table, dict):
                    continue
                for name, value in table.items():
                    if not isinstance(value, dict):
                        raise InventoryError(
                            f"{relative_manifest}:[patch.{registry}].{name} must use "
                            "a detailed dependency table"
                        )
                    declaration = f"{relative_manifest}:[patch.{registry}].{name}"
                    declarations[declaration] = Declaration(
                        name=name,
                        kind="crates-io-patch",
                        declaration=declaration,
                        manifest=manifest,
                        value=value,
                    )

        for table_name, table in dependency_tables(data):
            for name, value in table.items():
                if not isinstance(value, dict) or "git" not in value:
                    continue
                declaration = f"{relative_manifest}:[{table_name}].{name}"
                declarations[declaration] = Declaration(
                    name=name,
                    kind="git-dependency",
                    declaration=declaration,
                    manifest=manifest,
                    value=value,
                )
    return declarations


def normalized_local_path(root: Path, declaration: Declaration) -> str:
    declared = declaration.value.get("path")
    if not isinstance(declared, str) or not declared:
        raise InventoryError(f"{declaration.declaration}: Cargo patch must declare path")
    resolved = (declaration.manifest.parent / declared).resolve()
    try:
        return resolved.relative_to(root.resolve()).as_posix()
    except ValueError as error:
        raise InventoryError(
            f"{declaration.declaration}: patch path escapes the repository: {declared}"
        ) from error


def crates_io_index_path(package: str) -> str:
    package = package.lower()
    if len(package) == 1:
        return f"1/{package}"
    if len(package) == 2:
        return f"2/{package}"
    if len(package) == 3:
        return f"3/{package[0]}/{package}"
    return f"{package[:2]}/{package[2:4]}/{package}"


def validate_entry(root: Path, entry: dict[str, Any], actual: Declaration) -> None:
    label = f"inventory entry {entry.get('id', '<missing-id>')}"
    for field in (
        "id",
        "cargo_name",
        "kind",
        "declaration",
        "upstream",
        "divergence",
        "reason",
        "exit_type",
        "exit_condition",
        "exit_test",
    ):
        require_text(entry, field, label)

    if entry["cargo_name"] != actual.name:
        raise InventoryError(
            f"{label}: cargo_name {entry['cargo_name']!r} does not match {actual.name!r}"
        )
    if entry["kind"] != actual.kind:
        raise InventoryError(
            f"{label}: kind {entry['kind']!r} does not match {actual.kind!r}"
        )

    if actual.kind == "crates-io-patch":
        path = normalized_local_path(root, actual)
        if entry.get("path") != path:
            raise InventoryError(
                f"{label}: path {entry.get('path')!r} does not match Cargo path {path!r}"
            )
        vendored_manifest = root / path / "Cargo.toml"
        package = load_toml(vendored_manifest).get("package")
        if not isinstance(package, dict):
            raise InventoryError(f"{vendored_manifest}: missing [package] table")
        baseline = require_text(entry, "baseline", label)
        if package.get("version") != baseline:
            raise InventoryError(
                f"{label}: baseline {baseline!r} does not match vendored version "
                f"{package.get('version')!r}"
            )
        expected_package = actual.value.get("package", actual.name)
        if package.get("name") != expected_package:
            raise InventoryError(
                f"{label}: vendored package name {package.get('name')!r} does not "
                f"match {expected_package!r}"
            )
        expected_upstream = f"https://crates.io/crates/{expected_package}"
        if entry["upstream"] != expected_upstream:
            raise InventoryError(
                f"{label}: upstream {entry['upstream']!r} does not match {expected_upstream!r}"
            )
        upstream_index = require_text(entry, "upstream_index", label)
        expected_index = crates_io_index_path(expected_package)
        if upstream_index != expected_index:
            raise InventoryError(
                f"{label}: upstream_index {upstream_index!r} does not match {expected_index!r}"
            )
        exit_type = entry["exit_type"]
        if exit_type == "crates-io-dependency-floor":
            require_text(entry, "exit_dependency", label)
            require_text(entry, "exit_requirement", label)
        elif exit_type != "crates-io-newer-release":
            raise InventoryError(f"{label}: unsupported crates.io exit_type {exit_type!r}")
    else:
        git = require_text(entry, "git", label)
        rev = require_text(entry, "rev", label)
        if git != actual.value.get("git"):
            raise InventoryError(
                f"{label}: git {git!r} does not match Cargo git {actual.value.get('git')!r}"
            )
        if rev != actual.value.get("rev"):
            raise InventoryError(
                f"{label}: rev {rev!r} does not match Cargo rev {actual.value.get('rev')!r}"
            )
        if not HEX_REV.fullmatch(rev):
            raise InventoryError(f"{label}: git dependency rev must be a full lowercase SHA-1")
        if entry["exit_type"] != "upstream-git-integration":
            raise InventoryError(f"{label}: git dependency must use upstream-git-integration")
        require_text(entry, "upstream_repo", label)
        expected_upstream = f"https://github.com/{entry['upstream_repo']}"
        if entry["upstream"].removesuffix(".git") != expected_upstream:
            raise InventoryError(
                f"{label}: upstream {entry['upstream']!r} does not match {expected_upstream!r}"
            )
        require_text(entry, "upstream_ref", label)
        upstream_base = require_text(entry, "upstream_base", label)
        if not HEX_REV.fullmatch(upstream_base):
            raise InventoryError(f"{label}: upstream_base must be a full lowercase SHA-1")


def require_file_text(path: Path, needles: tuple[str, ...]) -> None:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        raise InventoryError(f"{path}: cannot verify divergence gate wiring: {error}") from error
    for needle in needles:
        if needle not in text:
            raise InventoryError(f"{path}: missing divergence gate wiring: {needle}")


def validate_gate_wiring(root: Path) -> None:
    workflow_root = root / ".github/workflows"
    if not workflow_root.is_dir():
        return
    structural_commands = (
        "python3 scripts/check-third-party-divergence.py",
        "python3 scripts/test-third-party-divergence.py",
    )
    require_file_text(workflow_root / "pr-gate.yml", structural_commands)
    require_file_text(workflow_root / "merge-validation.yml", structural_commands)
    require_file_text(
        workflow_root / "scheduled-ops.yml",
        (
            "- cron: '0 */6 * * *'",
            "GITHUB_TOKEN: ${{ github.token }}",
            "bash scripts/release-cargo-audit.sh",
        ),
    )
    require_file_text(
        root / "scripts/release-cargo-audit.sh",
        ("python3 scripts/check-third-party-divergence.py --check-upstream-exits",),
    )


def validate(root: Path) -> list[dict[str, Any]]:
    inventory = load_inventory(root)
    entries = inventory.get("dependency", [])
    declarations = discover_declarations(root)
    by_declaration: dict[str, dict[str, Any]] = {}
    ids: set[str] = set()

    for entry in entries:
        label = f"inventory entry {entry.get('id', '<missing-id>')}"
        entry_id = require_text(entry, "id", label)
        declaration = require_text(entry, "declaration", label)
        if entry_id in ids:
            raise InventoryError(f"duplicate inventory id: {entry_id}")
        if declaration in by_declaration:
            raise InventoryError(f"duplicate inventory declaration: {declaration}")
        ids.add(entry_id)
        by_declaration[declaration] = entry

    missing = sorted(set(declarations) - set(by_declaration))
    extra = sorted(set(by_declaration) - set(declarations))
    errors = [f"unlisted Cargo divergence: {item}" for item in missing]
    errors.extend(f"inventory entry has no Cargo divergence: {item}" for item in extra)
    if errors:
        raise InventoryError("\n".join(errors))

    for declaration, actual in declarations.items():
        validate_entry(root, by_declaration[declaration], actual)
    validate_gate_wiring(root)
    return entries


def parse_stable_version(value: str, label: str) -> tuple[int, int, int]:
    match = STABLE_VERSION.fullmatch(value)
    if match is None:
        raise InventoryError(f"{label}: expected stable MAJOR.MINOR.PATCH version, got {value!r}")
    return tuple(int(part) for part in match.groups())


def requirement_floor(requirement: str) -> tuple[int, int, int] | None:
    lower: tuple[int, int, int] | None = None
    for clause in requirement.split(","):
        clause = clause.strip()
        if not clause or clause.startswith(("<", "*")):
            continue
        match = re.fullmatch(r"(?:\^|~|=|>=|>)?\s*(\d+)(?:\.(\d+))?(?:\.(\d+))?", clause)
        if match is None:
            continue
        candidate = tuple(int(part or 0) for part in match.groups())
        if lower is None or candidate > lower:
            lower = candidate
    return lower


def fetch_url(url: str) -> str:
    headers = {"Accept": "application/json", "User-Agent": "Conary-third-party-audit/1"}
    token = os.environ.get("GITHUB_TOKEN")
    if token and urllib.parse.urlparse(url).netloc == "api.github.com":
        headers["Authorization"] = f"Bearer {token}"
        headers["X-GitHub-Api-Version"] = "2022-11-28"
    request = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.read().decode("utf-8")
    except (urllib.error.URLError, TimeoutError, UnicodeDecodeError) as error:
        raise InventoryError(f"upstream audit request failed for {url}: {error}") from error


def audit_crates_io_entry(
    entry: dict[str, Any], fetch: Callable[[str], str]
) -> str:
    label = f"inventory entry {entry['id']}"
    baseline = parse_stable_version(entry["baseline"], label)
    index_url = f"https://index.crates.io/{entry['upstream_index']}"
    try:
        releases = [json.loads(line) for line in fetch(index_url).splitlines() if line.strip()]
    except (json.JSONDecodeError, TypeError) as error:
        raise InventoryError(
            f"{label}: invalid crates.io sparse-index response: {error}"
        ) from error
    newer = []
    for release in releases:
        version = release.get("vers")
        if release.get("yanked") or not isinstance(version, str):
            continue
        match = STABLE_VERSION.fullmatch(version)
        if match is None:
            continue
        parsed = tuple(int(part) for part in match.groups())
        if parsed > baseline:
            newer.append((parsed, release))
    newer.sort(key=lambda item: item[0])

    if entry["exit_type"] == "crates-io-newer-release":
        if newer:
            version = newer[-1][1]["vers"]
            raise InventoryError(
                f"{label}: exit review required; crates.io published newer non-yanked "
                f"{entry['cargo_name']} {version}; run {entry['exit_test']} without the patch"
            )
        return f"{entry['id']}: pending; no newer non-yanked release after {entry['baseline']}"

    target = requirement_floor(entry["exit_requirement"])
    if target is None:
        raise InventoryError(
            f"{label}: cannot parse exit_requirement {entry['exit_requirement']!r}"
        )
    for _, release in newer:
        dependencies = release.get("deps", [])
        for dependency in dependencies:
            package = dependency.get("package") or dependency.get("name")
            if package != entry["exit_dependency"] or dependency.get("kind") == "dev":
                continue
            floor = requirement_floor(str(dependency.get("req", "")))
            if floor is not None and floor >= target:
                raise InventoryError(
                    f"{label}: exit condition met by {entry['cargo_name']} "
                    f"{release['vers']} with {package} {dependency['req']}; remove the "
                    f"patch after running {entry['exit_test']}"
                )
    newest = newer[-1][1]["vers"] if newer else entry["baseline"]
    return (
        f"{entry['id']}: pending; newest checked release {newest} does not require "
        f"{entry['exit_dependency']} {entry['exit_requirement']}"
    )


def audit_git_entry(entry: dict[str, Any], fetch: Callable[[str], str]) -> str:
    label = f"inventory entry {entry['id']}"
    fork_path = urllib.parse.urlparse(entry["git"]).path.strip("/").removesuffix(".git")
    fork_owner = fork_path.split("/", 1)[0]
    compare = f"{fork_owner}:{entry['rev']}...{entry['upstream_ref']}"
    url = f"https://api.github.com/repos/{entry['upstream_repo']}/compare/{compare}"
    try:
        result = json.loads(fetch(url))
    except (json.JSONDecodeError, TypeError) as error:
        raise InventoryError(f"{label}: invalid GitHub compare response: {error}") from error
    merge_base = result.get("merge_base_commit", {}).get("sha")
    if merge_base != entry["upstream_base"]:
        raise InventoryError(
            f"{label}: upstream merge base changed from {entry['upstream_base']} to "
            f"{merge_base}; review and re-pin or exit the fork"
        )
    behind_by = result.get("behind_by")
    ahead_by = result.get("ahead_by")
    if behind_by == 0:
        raise InventoryError(
            f"{label}: pinned fork commit is contained in upstream "
            f"{entry['upstream_ref']}; run {entry['exit_test']} against upstream and "
            "drop the fork"
        )
    if not isinstance(behind_by, int) or not isinstance(ahead_by, int):
        raise InventoryError(f"{label}: GitHub compare response omitted ahead/behind counts")
    return (
        f"{entry['id']}: pending; fork pin retains {behind_by} commit(s) outside upstream "
        f"and upstream has {ahead_by} commit(s) after the recorded comparison base"
    )


def audit_upstream(
    entries: list[dict[str, Any]], fetch: Callable[[str], str] = fetch_url
) -> list[str]:
    results = []
    errors = []
    for entry in entries:
        try:
            if entry["kind"] == "crates-io-patch":
                results.append(audit_crates_io_entry(entry, fetch))
            else:
                results.append(audit_git_entry(entry, fetch))
        except InventoryError as error:
            errors.append(str(error))
    if errors:
        raise InventoryError("\n".join(errors))
    return results


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=None)
    parser.add_argument("--check-upstream-exits", action="store_true")
    args = parser.parse_args()
    root = args.root.resolve() if args.root else Path(__file__).resolve().parent.parent
    try:
        entries = validate(root)
        print(f"Third-party divergence inventory check passed ({len(entries)} entries).")
        if args.check_upstream_exits:
            for result in audit_upstream(entries):
                print(result)
            print(
                "Third-party upstream exit audit passed; all recorded divergences "
                "remain necessary."
            )
    except InventoryError as error:
        print(f"Third-party divergence inventory check failed:\n{error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
