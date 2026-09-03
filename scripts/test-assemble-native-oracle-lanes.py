#!/usr/bin/env python3
# scripts/test-assemble-native-oracle-lanes.py

"""Focused tests for bound native-oracle lane assembly."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


REPO_ROOT = Path(__file__).resolve().parents[1]
ASSEMBLER = REPO_ROOT / "scripts" / "assemble-native-oracle-lanes.py"
PROFILES = ("fedora-44", "ubuntu-26.04", "arch")
CONFIG = {
    "fedora-44": (
        "x86_64",
        "registry.fedoraproject.org/fedora@sha256:765b2260aa4b4eff379b9a6f983f15fcf41a6f9dda9b272b790e23e92fcbaafb",
        "conary-rpm-oracle",
        "conary-rpm-resolution-oracle",
        {"ecosystem": "rpm", "name": "libsolv", "projection_schema": 1, "version": "0.7.36"},
        {"ecosystem": "rpm", "name": "libsolv", "projection_schema": 4, "version": "0.7.36"},
    ),
    "ubuntu-26.04": (
        "amd64",
        "docker.io/library/ubuntu:26.04@sha256:678c6550cc43645e08669028bc177f50be4e7c5b8cca677067b1914d4afc7a03",
        "conary-debian-oracle",
        "conary-debian-resolution-oracle",
        {"ecosystem": "debian", "name": "apt-pkg", "projection_schema": 1, "version": "3.2.0"},
        {"ecosystem": "debian", "name": "apt-pkg", "projection_schema": 2, "version": "3.2.0"},
    ),
    "arch": (
        "x86_64",
        "docker.io/library/archlinux@sha256:fe6972d4dc1f660c0c10f4c41b2de8986bab89e7e2955378f8beadb8ebcd7433",
        "conary-alpm-oracle",
        "conary-alpm-resolution-oracle",
        {"ecosystem": "alpm", "name": "libalpm", "projection_schema": 1, "version": "15.0.0"},
        {"ecosystem": "alpm", "name": "libalpm", "projection_schema": 2, "version": "15.0.0"},
    ),
}
EMPTY_SHA256 = hashlib.sha256(b"").hexdigest()
DEPLOYMENT_RUN_ID = 123
EXPORT_RUN_ID = 456
EXPORT_ID = "slice6-test"
TRANSPORT_SHA256 = "a" * 64
INPUT_MANIFEST_SHA256 = "b" * 64


def canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def sha256(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


class NativeOracleAssemblyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.repository = self.root / "repository"
        self.repository.mkdir()
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Native Oracle Test")
        self.git("config", "user.email", "native-oracle@test.invalid")
        (self.repository / "tracked").write_text("base\n")
        self.git("add", "tracked")
        self.git("commit", "-q", "-m", "base")
        self.deployed = self.git("rev-parse", "HEAD")
        self.git("commit", "--allow-empty", "-q", "-m", "producer one")
        self.producer_one = self.git("rev-parse", "HEAD")
        self.git("commit", "--allow-empty", "-q", "-m", "producer two")
        self.producer_two = self.git("rev-parse", "HEAD")
        tree = self.git("rev-parse", f"{self.deployed}^{{tree}}")
        self.unmerged = self.git("commit-tree", tree, "-p", self.deployed, input="unmerged\n")
        self.unrelated = self.git("commit-tree", tree, input="unrelated\n")
        self.lanes_root = self.root / "lanes"
        self.lanes_root.mkdir()
        self.producers = {
            "fedora-44": self.producer_one,
            "ubuntu-26.04": self.producer_two,
            "arch": self.producer_one,
        }
        for ordinal, profile in enumerate(PROFILES, 1):
            self.write_lane(profile, self.producers[profile])
        self.metadata_path = self.root / "artifact-metadata.json"
        self.write_metadata()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def git(self, *arguments: str, input: str | None = None) -> str:
        result = subprocess.run(
            ["git", *arguments],
            cwd=self.repository,
            input=input,
            text=True,
            capture_output=True,
            check=True,
        )
        return result.stdout.strip()

    def write_lane(self, profile: str, producer_commit: str) -> None:
        architecture, image, package_binary, resolution_binary, package_impl, resolution_impl = CONFIG[profile]
        root = self.lanes_root / profile
        package_root = root / "package-oracle"
        resolution_root = root / "resolution-oracle"
        package_root.mkdir(parents=True, exist_ok=True)
        resolution_root.mkdir()
        profile_revision = sha256(profile.encode())
        package_counts = {"packages": 0}
        package_manifest = {
            "artifact": {"counts": package_counts, "sha256": EMPTY_SHA256, "size": 0},
            "implementation": package_impl,
            "profile": profile,
            "profile_revision_sha256": profile_revision,
            "schema_version": 1,
        }
        package_manifest_bytes = canonical(package_manifest)
        (package_root / "manifest.json").write_bytes(package_manifest_bytes)
        (package_root / "packages.jsonl").write_bytes(b"")
        resolution_counts = {"roots": 0}
        resolution_manifest = {
            "artifact": {"counts": resolution_counts, "sha256": EMPTY_SHA256, "size": 0},
            "implementation": resolution_impl,
            "package_oracle_manifest_sha256": sha256(package_manifest_bytes),
            "policy": {"architecture": architecture},
            "profile": profile,
            "profile_revision_sha256": profile_revision,
            "schema_version": 2,
        }
        resolution_manifest_bytes = canonical(resolution_manifest)
        (resolution_root / "manifest.json").write_bytes(resolution_manifest_bytes)
        (resolution_root / "roots.jsonl").write_bytes(b"")
        evidence = {
            "artifact_type": "native-oracle-lane",
            "deployed_commit": self.deployed,
            "deployment_run_id": DEPLOYMENT_RUN_ID,
            "export_id": EXPORT_ID,
            "export_run_id": EXPORT_RUN_ID,
            "input_manifest_sha256": INPUT_MANIFEST_SHA256,
            "lane_image": image,
            "package_oracle": {
                "artifact": {"counts": package_counts, "name": "packages.jsonl", "sha256": EMPTY_SHA256, "size": 0},
                "implementation": package_impl,
                "manifest_sha256": sha256(package_manifest_bytes),
                "schema_version": 1,
            },
            "producer_binaries": {
                "package": {"name": package_binary, "sha256": "c" * 64},
                "resolution": {"name": resolution_binary, "sha256": "d" * 64},
            },
            "producer_commit": producer_commit,
            "profile": profile,
            "profile_revision_sha256": profile_revision,
            "resolution_oracle": {
                "artifact": {"counts": resolution_counts, "name": "roots.jsonl", "sha256": EMPTY_SHA256, "size": 0},
                "implementation": resolution_impl,
                "manifest_sha256": sha256(resolution_manifest_bytes),
                "schema_version": 2,
            },
            "resolution_implementation": {
                "memory_budget_bytes": 8589934592,
                "measured_worker_rss_bytes": 536870912,
                "schema_version": 1,
                "worker_load_milliseconds": [12, 13],
                "workers": 2,
            },
            "schema_version": 4,
            "target_architecture": architecture,
            "transport_sha256": TRANSPORT_SHA256,
        }
        (root / "evidence.json").write_bytes(canonical(evidence))

    def evidence(self, profile: str) -> dict:
        return json.loads((self.lanes_root / profile / "evidence.json").read_bytes())

    def rewrite_evidence(self, profile: str, evidence: dict) -> None:
        (self.lanes_root / profile / "evidence.json").write_bytes(canonical(evidence))

    def write_metadata(self) -> None:
        artifacts = []
        for ordinal, profile in enumerate(PROFILES, 1):
            producer = self.evidence(profile)["producer_commit"]
            artifacts.append(
                {
                    "artifact_id": 1000 + ordinal,
                    "name": f"remi-native-oracle-lane-{profile}-{EXPORT_ID}-{producer}",
                    "profile": profile,
                    "run_id": 2000 + ordinal,
                    "sha256": f"{ordinal}" * 64,
                }
            )
        self.metadata_path.write_bytes(canonical({"artifacts": artifacts, "schema_version": 1}))

    def run_assembler(self, profiles: tuple[str, ...] = PROFILES) -> subprocess.CompletedProcess[str]:
        command = [
            sys.executable,
            str(ASSEMBLER),
            "--repository", str(self.repository),
            "--main-ref", "refs/heads/main",
            "--deployed-commit", self.deployed,
            "--deployment-run-id", str(DEPLOYMENT_RUN_ID),
            "--export-run-id", str(EXPORT_RUN_ID),
            "--export-id", EXPORT_ID,
            "--transport-sha256", TRANSPORT_SHA256,
            "--artifact-metadata", str(self.metadata_path),
            "--output", str(self.root / "assembled.json"),
        ]
        for profile in profiles:
            command.extend(("--lane", f"{profile}={self.lanes_root / profile}"))
        return subprocess.run(command, text=True, capture_output=True, check=False)

    def test_accepts_same_export_with_mixed_descendant_producers(self) -> None:
        result = self.run_assembler()
        self.assertEqual(result.returncode, 0, result.stderr)
        assembled = json.loads(result.stdout)
        self.assertEqual(assembled["artifact_type"], "native-oracle-three-lane-set")
        self.assertEqual([lane["profile"] for lane in assembled["lanes"]], list(PROFILES))
        self.assertEqual(
            [lane["producer_commit"] for lane in assembled["lanes"]],
            [self.producer_one, self.producer_two, self.producer_one],
        )
        self.assertEqual((self.root / "assembled.json").read_bytes(), canonical(assembled))

    def test_rejects_different_export(self) -> None:
        evidence = self.evidence("arch")
        evidence["export_id"] = "slice6-other"
        self.rewrite_evidence("arch", evidence)
        result = self.run_assembler()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("arch lane binding drifted", result.stderr)

    def test_rejects_obsolete_lane_evidence_schema(self) -> None:
        evidence = self.evidence("fedora-44")
        evidence["schema_version"] = 3
        self.rewrite_evidence("fedora-44", evidence)
        result = self.run_assembler()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("fedora-44 lane binding drifted", result.stderr)

    def test_rejects_schema_three_lane_without_worker_evidence(self) -> None:
        evidence = self.evidence("fedora-44")
        evidence["schema_version"] = 3
        del evidence["resolution_implementation"]
        self.rewrite_evidence("fedora-44", evidence)
        result = self.run_assembler()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("incomplete or unknown fields", result.stderr)

    def test_rejects_non_descendant_producer(self) -> None:
        evidence = self.evidence("fedora-44")
        evidence["producer_commit"] = self.unrelated
        self.rewrite_evidence("fedora-44", evidence)
        self.write_metadata()
        result = self.run_assembler()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("deployed-to-producer ancestry is not proved", result.stderr)

    def test_rejects_unmerged_producer(self) -> None:
        evidence = self.evidence("fedora-44")
        evidence["producer_commit"] = self.unmerged
        self.rewrite_evidence("fedora-44", evidence)
        self.write_metadata()
        result = self.run_assembler()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("producer-to-main ancestry is not proved", result.stderr)

    def test_rejects_missing_lane(self) -> None:
        result = self.run_assembler(PROFILES[:2])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exactly one Fedora, Ubuntu, and Arch lane", result.stderr)

    def test_rejects_survey_substitution(self) -> None:
        evidence = self.evidence("ubuntu-26.04")
        evidence["artifact_type"] = "native-resolution-survey-diagnostics"
        self.rewrite_evidence("ubuntu-26.04", evidence)
        result = self.run_assembler()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ubuntu-26.04 lane binding drifted", result.stderr)

    def test_rejects_missing_binary_digest(self) -> None:
        evidence = self.evidence("arch")
        del evidence["producer_binaries"]["resolution"]["sha256"]
        self.rewrite_evidence("arch", evidence)
        result = self.run_assembler()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("arch resolution binary has incomplete or unknown fields", result.stderr)

    def test_rejects_implementation_pin_drift(self) -> None:
        evidence = self.evidence("fedora-44")
        evidence["package_oracle"]["implementation"]["projection_schema"] = 2
        self.rewrite_evidence("fedora-44", evidence)
        result = self.run_assembler()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("manifest or artifact binding drifted", result.stderr)

    def test_rejects_metadata_to_lane_producer_mismatch(self) -> None:
        evidence = self.evidence("fedora-44")
        evidence["producer_commit"] = self.producer_two
        self.rewrite_evidence("fedora-44", evidence)
        result = self.run_assembler()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("artifact name disagrees with producer evidence", result.stderr)


if __name__ == "__main__":
    unittest.main()
