#!/usr/bin/env python3
# scripts/test-native-oracle-input-transport.py

"""Focused tests for the native-oracle production transport verifier."""

from __future__ import annotations

import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest


REPO_ROOT = Path(__file__).resolve().parent.parent
VERIFIER = REPO_ROOT / "scripts" / "verify-native-oracle-input-transport.py"
PROFILES = ("fedora-44", "ubuntu-26.04", "arch")


def canonical_json(value: object) -> bytes:
    return json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")


def digest_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def digest_json(value: object) -> str:
    return digest_bytes(canonical_json(value))


def artifact(seed: str) -> dict[str, object]:
    return {"sha256": digest_bytes(seed.encode()), "size": len(seed)}


def counts(*, sources: int) -> dict[str, int]:
    return {
        "packages": 1,
        "provides": 1,
        "requirement_groups": 0,
        "requirement_atoms": 0,
        "source_evidence": sources,
    }


def build_fixture() -> tuple[dict[str, object], dict[str, bytes], list[str]]:
    profiles: list[dict[str, object]] = []
    objects: dict[str, bytes] = {}
    candidates: list[str] = []
    source_facts = (
        ("fedora-44", "rpm", "rpm", "rpm", "rpm_primary", "repodata/primary.xml.gz"),
        (
            "ubuntu-26.04",
            "deb",
            "deb",
            "debian",
            "debian_packages",
            "dists/resolute/main/binary-amd64/Packages.xz",
        ),
        ("arch", "alpm", "arch", "arch", "arch_database", "extra.db"),
    )
    for ordinal, (
        profile,
        ecosystem,
        parser_format,
        trust_format,
        object_role,
        source_path,
    ) in enumerate(source_facts):
        object_data = f"native metadata for {profile}\n".encode()
        object_digest = digest_bytes(object_data)
        objects[object_digest] = object_data
        parser_config = {"package_format": parser_format, "fixture": profile}
        trust_policy = {"ecosystem": trust_format, "fixture": profile}
        source_identity = f"{profile}-source"
        repository_identity = f"{profile}-repository"
        stream = {"kind": "release", "identity": f"{profile}-stream"}
        source = {
            "schema_version": 1,
            "source_profile": profile,
            "source_identity": source_identity,
            "repository_identity": repository_identity,
            "stream": stream,
            "stream_binding_sha256": digest_bytes(f"stream-{profile}".encode()),
            "parser_projection_version": 2,
            "provenance": {
                "ecosystem": ecosystem,
                "metadata_url": f"https://packages.example.test/{profile}/metadata",
                "content_url": f"https://packages.example.test/{profile}/content",
                "parser_config": parser_config,
                "parser_config_sha256": digest_json(parser_config),
                "trust_policy": trust_policy,
                "trust_policy_sha256": digest_json(trust_policy),
            },
            "authenticated_root": artifact(f"root-{profile}"),
            "authenticated_objects": [
                {
                    "role": object_role,
                    "source_path": source_path,
                    "sha256": object_digest,
                    "size": len(object_data),
                }
            ],
            "catalog": artifact(f"source-catalog-{profile}"),
            "logical_digest_sha256": digest_bytes(f"logical-source-{profile}".encode()),
            "counts": counts(sources=1),
        }
        source_digest = digest_json(source)
        revision = {
            "schema_version": 2,
            "profile": profile,
            "projection_version": 2,
            "members": [
                {
                    "ordinal": 0,
                    "role": "base",
                    "source_identity": source_identity,
                    "repository_identity": repository_identity,
                    "stream": stream,
                    "precedence": 100 - ordinal,
                    "required": True,
                    "source_snapshot_sha256": source_digest,
                }
            ],
            "catalog": artifact(f"profile-catalog-{profile}"),
            "logical_digest_sha256": digest_bytes(f"logical-profile-{profile}".encode()),
            "counts": counts(sources=1),
        }
        revision_digest = digest_json(revision)
        candidates.append(f"{profile}={revision_digest}")
        profiles.append(
            {
                "profile_revision_sha256": revision_digest,
                "revision": revision,
                "sources": [source],
            }
        )
    manifest = {
        "schema_version": 1,
        "profiles": profiles,
        "objects": [
            {"sha256": digest, "size": len(data)}
            for digest, data in sorted(objects.items())
        ],
    }
    return manifest, objects, candidates


def add_directory(archive: tarfile.TarFile, name: str) -> None:
    member = tarfile.TarInfo(name)
    member.type = tarfile.DIRTYPE
    member.mode = 0o700
    archive.addfile(member)


def add_file(archive: tarfile.TarFile, name: str, data: bytes) -> None:
    member = tarfile.TarInfo(name)
    member.size = len(data)
    member.mode = 0o600
    archive.addfile(member, io.BytesIO(data))


def write_transport(
    path: Path,
    export_id: str,
    manifest: dict[str, object],
    objects: dict[str, bytes],
    *,
    manifest_bytes: bytes | None = None,
    extra: str | None = None,
    symlink_object: bool = False,
) -> None:
    with tarfile.open(path, "w", format=tarfile.USTAR_FORMAT) as archive:
        add_directory(archive, export_id)
        add_file(
            archive,
            f"{export_id}/manifest.json",
            manifest_bytes if manifest_bytes is not None else canonical_json(manifest),
        )
        add_directory(archive, f"{export_id}/objects")
        for index, (digest, data) in enumerate(sorted(objects.items())):
            name = f"{export_id}/objects/{digest}"
            if symlink_object and index == 0:
                member = tarfile.TarInfo(name)
                member.type = tarfile.SYMTYPE
                member.linkname = "../manifest.json"
                archive.addfile(member)
            else:
                add_file(archive, name, data)
        if extra is not None:
            add_file(archive, extra, b"unexpected")


class NativeOracleTransportTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.export_id = "slice6-test-export"
        self.manifest, self.objects, self.candidates = build_fixture()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def verify(
        self,
        transport: Path,
        *,
        candidates: list[str] | None = None,
        suffix: str = "run",
    ) -> subprocess.CompletedProcess[str]:
        command = [
            sys.executable,
            str(VERIFIER),
            "--transport",
            str(transport),
            "--export-id",
            self.export_id,
            "--output-dir",
            str(self.root / f"output-{suffix}"),
            "--evidence",
            str(self.root / f"evidence-{suffix}.json"),
        ]
        for candidate in candidates if candidates is not None else self.candidates:
            command.extend(("--expected-candidate", candidate))
        return subprocess.run(command, text=True, capture_output=True, check=False)

    def assert_rejected(self, result: subprocess.CompletedProcess[str], needle: str) -> None:
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn(needle, result.stderr)

    def test_reopens_exact_transport_and_emits_sanitized_evidence(self) -> None:
        transport = self.root / "input.tar"
        write_transport(transport, self.export_id, self.manifest, self.objects)

        result = self.verify(transport)

        self.assertEqual(result.returncode, 0, result.stderr)
        evidence = json.loads((self.root / "evidence-run.json").read_bytes())
        self.assertEqual(evidence["schema_version"], 1)
        self.assertEqual([item["profile"] for item in evidence["profiles"]], list(PROFILES))
        self.assertEqual(evidence["counts"]["objects"], 3)
        self.assertEqual(
            (self.root / "output-run" / "manifest.json").read_bytes(),
            canonical_json(self.manifest),
        )
        for digest, data in self.objects.items():
            self.assertEqual((self.root / "output-run" / "objects" / digest).read_bytes(), data)

    def test_rejects_candidate_identity_drift(self) -> None:
        transport = self.root / "candidate.tar"
        write_transport(transport, self.export_id, self.manifest, self.objects)
        candidates = list(self.candidates)
        candidates[0] = f"fedora-44={'0' * 64}"

        self.assert_rejected(
            self.verify(transport, candidates=candidates, suffix="candidate"),
            "expected candidate revision",
        )

    def test_rejects_noncanonical_manifest(self) -> None:
        transport = self.root / "noncanonical.tar"
        write_transport(
            transport,
            self.export_id,
            self.manifest,
            self.objects,
            manifest_bytes=json.dumps(self.manifest, indent=2).encode(),
        )

        self.assert_rejected(
            self.verify(transport, suffix="noncanonical"), "not canonical JSON"
        )

    def test_rejects_duplicate_json_key(self) -> None:
        transport = self.root / "duplicate.tar"
        canonical = canonical_json(self.manifest)
        duplicate = canonical.replace(b'{"objects":', b'{"schema_version":1,"objects":', 1)
        write_transport(
            transport,
            self.export_id,
            self.manifest,
            self.objects,
            manifest_bytes=duplicate,
        )

        self.assert_rejected(
            self.verify(transport, suffix="duplicate"), "repeats JSON key"
        )

    def test_rejects_nonstandard_json_constant(self) -> None:
        transport = self.root / "nan.tar"
        manifest_bytes = canonical_json(self.manifest).replace(
            b'"schema_version":1', b'"schema_version":NaN', 1
        )
        write_transport(
            transport,
            self.export_id,
            self.manifest,
            self.objects,
            manifest_bytes=manifest_bytes,
        )

        self.assert_rejected(
            self.verify(transport, suffix="nan"), "invalid JSON constant"
        )

    def test_rejects_metadata_object_tamper(self) -> None:
        transport = self.root / "tamper.tar"
        objects = dict(self.objects)
        digest = next(iter(objects))
        objects[digest] = b"x" * len(objects[digest])
        write_transport(transport, self.export_id, self.manifest, objects)

        self.assert_rejected(
            self.verify(transport, suffix="tamper"), "failed SHA-256 verification"
        )

    def test_rejects_unexpected_member(self) -> None:
        transport = self.root / "extra.tar"
        write_transport(
            transport,
            self.export_id,
            self.manifest,
            self.objects,
            extra=f"{self.export_id}/extra",
        )

        self.assert_rejected(
            self.verify(transport, suffix="extra"), "missing or unexpected members"
        )

    def test_rejects_symlink_member(self) -> None:
        transport = self.root / "symlink.tar"
        write_transport(
            transport,
            self.export_id,
            self.manifest,
            self.objects,
            symlink_object=True,
        )

        self.assert_rejected(
            self.verify(transport, suffix="symlink"), "not a plain file or directory"
        )

    def test_rejects_traversal_member(self) -> None:
        transport = self.root / "traversal.tar"
        write_transport(
            transport,
            self.export_id,
            self.manifest,
            self.objects,
            extra="../escape",
        )

        self.assert_rejected(
            self.verify(transport, suffix="traversal"), "unsafe or noncanonical member"
        )


if __name__ == "__main__":
    unittest.main()
