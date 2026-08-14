#!/usr/bin/env python3
# tests/fixtures/distro-roots/run-native-repository-takeover.py

"""Exercise typed ALPM or Zypper takeover against a live distro root."""

import argparse
import configparser
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import tomllib


def run(command: list[str]) -> str:
    result = subprocess.run(command, check=False, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(
            f"command failed ({result.returncode}): {command!r}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result.stdout


def preview(conary: str, db: str, root: str, manifest: dict) -> dict:
    with tempfile.NamedTemporaryFile(mode="w", suffix=".json") as handle:
        json.dump(manifest, handle, sort_keys=True)
        handle.flush()
        stdout = run(
            [
                conary,
                "system",
                "repository-takeover",
                "--db-path",
                db,
                "--root",
                root,
                "--manifest",
                handle.name,
                "--dry-run",
            ]
        )
    envelope, _ = json.JSONDecoder().raw_decode(stdout.lstrip())
    if envelope.get("schema") != 1 or len(envelope.get("preview_sha256", "")) != 64:
        raise RuntimeError("takeover preview lacks its typed schema or exact digest")
    return envelope


def file_url(root: Path, guest_path: str) -> str:
    return (root / guest_path.lstrip("/")).resolve().as_uri()


def zypper_values(root: Path, reference: dict) -> tuple[str, int]:
    parser = configparser.ConfigParser(interpolation=None)
    parser.read(root / reference["location"]["path"].lstrip("/"))
    section = parser[reference["alias"]]
    return section["baseurl"], int(section.get("priority", "99"))


def binding_by_repository(target_root: dict) -> dict[str, dict]:
    bindings = {}
    for binding in target_root["repository_trust"]:
        repository = binding["repository"]
        if repository in bindings:
            raise RuntimeError(f"duplicate repository trust binding: {repository}")
        bindings[repository] = binding
    return bindings


def arch_trust(binding: dict) -> dict:
    return {
        "ecosystem": "arch",
        "keyring": {
            "url": binding["keyring_url"],
            "format": "alpm-package-zstd",
            "master_fingerprints": binding["master_fingerprints"],
            "packager_key_threshold": binding["packager_key_threshold"],
        },
        "sig_level": {
            "database": "optional",
            "package": "required",
            "trust": "trusted-only",
        },
    }


def rpm_trust(root: Path, binding: dict, signing_roots: dict[str, dict]) -> dict:
    roots = []
    for path in binding["signing_root_paths"]:
        authority = signing_roots[path]
        roots.extend(
            {"url": file_url(root, path), "fingerprint": fingerprint}
            for fingerprint in authority["fingerprints"]
        )
    return {
        "ecosystem": "rpm",
        "metadata": {"kind": "open-pgp", "keys": roots},
        "package_keys": roots,
    }


def build_manifest(target: str, target_root: dict, root: Path, initial: dict) -> dict:
    bindings = binding_by_repository(target_root)
    signing_roots = {item["path"]: item for item in target_root["signing_roots"]}
    repositories = []
    for index, plan in enumerate(initial["trust"]["repositories"], start=1):
        reference = plan["repository"]
        ecosystem = reference["ecosystem"]
        repository = reference.get("name") or reference.get("alias")
        binding = bindings.pop(repository, None)
        if binding is None:
            raise RuntimeError(f"no exact trust binding for native repository {repository}")
        identity = f"{target}:{repository}:x86_64"
        if ecosystem == "alpm" and binding["ecosystem"] == "arch":
            metadata_url = binding["metadata_url"]
            parser = {"package_format": "arch", "database": repository}
            trust = arch_trust(binding)
            priority = 20 + index
        elif ecosystem == "zypper" and binding["ecosystem"] == "rpm-global":
            metadata_url, priority = zypper_values(root, reference)
            parser = {"package_format": "rpm", "architecture": "x86_64"}
            trust = rpm_trust(root, binding, signing_roots)
        else:
            raise RuntimeError(
                f"trust binding {binding['ecosystem']} does not match {ecosystem}"
            )
        repositories.append(
            {
                "declaration": reference,
                "name": f"native-{repository}",
                "source_identity": f"{target}:rolling:x86_64",
                "repository_identity": identity,
                "scope": {"kind": "repository", "identity": identity},
                "stream": {"kind": "rolling", "identity": target},
                "update": {"mode": "follow"},
                "metadata_url": metadata_url,
                "content_url": None,
                "parser": parser,
                "trust": trust,
                "enabled": plan.get("enabled", True),
                "priority": priority,
                "metadata_expire": 3600,
                "source_profile": None,
            }
        )
    if bindings:
        raise RuntimeError(f"unused repository trust bindings: {sorted(bindings)}")
    if not repositories:
        raise RuntimeError("native discovery produced no repositories")
    return {"schema": 1, "repositories": repositories}


def projection_state(root: Path, preview_value: dict) -> dict[str, str]:
    observed = {}
    for projection in preview_value["projections"]:
        path = projection["path"]
        digest = hashlib.sha256((root / path.lstrip("/")).read_bytes()).hexdigest()
        if digest != projection["sha256"]:
            raise RuntimeError(f"projection digest mismatch: {path}")
        observed[path] = digest
    return observed


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--conary", required=True)
    parser.add_argument("--config", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--db", required=True)
    parser.add_argument("--root", default="/")
    parser.add_argument("--evidence", required=True)
    args = parser.parse_args()

    with open(args.config, "rb") as handle:
        target_root = tomllib.load(handle)["distros"][args.target]["target_root"]
    root = Path(args.root)
    initial = preview(args.conary, args.db, args.root, {"schema": 1, "repositories": []})
    manifest = build_manifest(args.target, target_root, root, initial["preview"])
    envelope = preview(args.conary, args.db, args.root, manifest)
    if envelope["preview"]["blockers"]:
        raise RuntimeError(f"takeover preview has blockers: {envelope['preview']['blockers']!r}")
    before = projection_state(root, envelope["preview"])

    with tempfile.NamedTemporaryFile(mode="w", suffix=".json") as handle:
        json.dump(manifest, handle, sort_keys=True)
        handle.flush()
        command = [
            args.conary,
            "system",
            "repository-takeover",
            "--db-path",
            args.db,
            "--root",
            args.root,
            "--manifest",
            handle.name,
            "--preview-sha256",
            envelope["preview_sha256"],
            "--yes",
        ]
        run(command)
        run(command)

    if projection_state(root, envelope["preview"]) != before:
        raise RuntimeError("repository projection bytes changed after apply/repeat")
    run(
        [
            args.conary,
            "system",
            "repository-takeover",
            "--db-path",
            args.db,
            "--root",
            args.root,
            "--rollback",
            "--yes",
        ]
    )
    if projection_state(root, envelope["preview"]) != before:
        raise RuntimeError("repository projection bytes changed after rollback")

    evidence = {
        "schema_version": 1,
        "target_profile": args.target,
        "preview_sha256": envelope["preview_sha256"],
        "repositories": len(manifest["repositories"]),
        "projection_sha256": before,
        "stages": ["preview", "apply", "repeat", "rollback"],
    }
    Path(args.evidence).write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
