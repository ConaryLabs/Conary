#!/usr/bin/env python3
# tests/fixtures/distro-roots/run-apt-repository-takeover.py

"""Exercise typed APT discovery and repository takeover on a live test root."""

import argparse
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


def trust_roots(release_root: dict, root: Path, plan: dict) -> list[dict]:
    disposition = plan["disposition"]
    if disposition["status"] == "importable":
        roots = []
        for evidence in plan["evidence"]:
            if evidence["role"] != "debian-release":
                continue
            source = evidence["source"]
            if source["kind"] != "selected-root-file":
                raise RuntimeError("derivative lane requires selected-root APT trust files")
            for certificate in evidence["certificates"]:
                roots.append(
                    {
                        "url": file_url(root, source["path"]),
                        "fingerprint": certificate["certificate_fingerprint"],
                    }
                )
        if not roots:
            raise RuntimeError("importable APT declaration exposed no release roots")
        return roots

    findings = disposition.get("findings", [])
    if not findings or any(item["kind"] != "implicit-global-authority" for item in findings):
        raise RuntimeError(f"APT trust is not explicitly resolvable: {disposition!r}")
    reference = plan["repository"]
    matches = [
        binding
        for binding in release_root.get("apt_global_trust", [])
        if binding["document"] == reference["document"]
        and reference["entry_index"] in binding["entry_indexes"]
    ]
    if len(matches) != 1:
        raise RuntimeError(f"legacy APT declaration has {len(matches)} exact trust bindings")
    signing_roots = {item["path"]: item for item in release_root["signing_roots"]}
    roots = []
    for path in matches[0]["signing_root_paths"]:
        authority = signing_roots[path]
        for fingerprint in authority["fingerprints"]:
            roots.append({"url": file_url(root, path), "fingerprint": fingerprint})
    return roots


def missing_products(preview_value: dict, reference: dict) -> list[dict]:
    matches = [
        blocker["products"]
        for blocker in preview_value["blockers"]
        if blocker["kind"] == "missing-enrollment" and blocker["repository"] == reference
    ]
    if len(matches) != 1:
        raise RuntimeError(f"APT declaration has {len(matches)} product sets")
    return matches[0]


def build_manifest(target: str, release_root: dict, root: Path, initial: dict) -> dict:
    repositories = []
    counter = 0
    trust_by_reference = {
        json.dumps(plan["repository"], sort_keys=True): trust_roots(release_root, root, plan)
        for plan in initial["trust"]["repositories"]
    }
    for plan in initial["trust"]["repositories"]:
        reference = plan["repository"]
        roots = trust_by_reference[json.dumps(reference, sort_keys=True)]
        for product in missing_products(initial, reference):
            if product["kind"] != "apt" or product.get("component") is None:
                raise RuntimeError(f"unsupported APT enrollment product: {product!r}")
            counter += 1
            identity = f"{target}:apt:{counter:03d}:amd64"
            repositories.append(
                {
                    "declaration": reference,
                    "name": f"native-apt-{counter:03d}",
                    "source_identity": f"{target}:apt:amd64",
                    "repository_identity": identity,
                    "scope": {"kind": "repository", "identity": identity},
                    "stream": {"kind": "release", "identity": product["suite"]},
                    "update": {"mode": "follow"},
                    "metadata_url": product["uri"],
                    "content_url": None,
                    "parser": {
                        "package_format": "deb",
                        "distribution": product["suite"],
                        "component": product["component"],
                        "architecture": "amd64",
                    },
                    "trust": {"ecosystem": "debian", "release_keys": roots},
                    "enabled": plan.get("enabled", True),
                    "priority": 50,
                    "metadata_expire": 3600,
                    "source_profile": None,
                }
            )
    if not repositories:
        raise RuntimeError("discovery produced no APT repository products")
    return {"schema": 1, "repositories": repositories}


def projection_state(root: Path, preview_value: dict) -> dict[str, str]:
    observed = {}
    for projection in preview_value["projections"]:
        path = projection["path"]
        content = (root / path.lstrip("/")).read_bytes()
        digest = hashlib.sha256(content).hexdigest()
        if digest != projection["sha256"]:
            raise RuntimeError(f"projection digest mismatch before mutation: {path}")
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
        config = tomllib.load(handle)
    release_root = config["distros"][args.target]["release_root"]
    root = Path(args.root)
    initial_envelope = preview(args.conary, args.db, args.root, {"schema": 1, "repositories": []})
    manifest = build_manifest(args.target, release_root, root, initial_envelope["preview"])
    envelope = preview(args.conary, args.db, args.root, manifest)
    if envelope["preview"]["blockers"]:
        raise RuntimeError(f"takeover preview has blockers: {envelope['preview']['blockers']!r}")
    before = projection_state(root, envelope["preview"])

    with tempfile.NamedTemporaryFile(mode="w", suffix=".json") as handle:
        json.dump(manifest, handle, sort_keys=True)
        handle.flush()
        apply_command = [
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
        run(apply_command)
        run(apply_command)

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
        "repository_products": len(manifest["repositories"]),
        "projection_sha256": before,
        "stages": ["preview", "apply", "repeat", "rollback"],
    }
    Path(args.evidence).write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
