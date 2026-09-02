#!/usr/bin/env python3
# scripts/test-produce-native-oracle-lane.py

"""Focused mutation tests for production native-oracle lane adaptation."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest


REPO_ROOT = Path(__file__).resolve().parents[1]
PRODUCER = REPO_ROOT / "scripts" / "produce-native-oracle-lane.py"
COMMIT = "a" * 40
EMPTY_SHA256 = hashlib.sha256(b"").hexdigest()


def canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def digest(value: object) -> str:
    return hashlib.sha256(canonical(value)).hexdigest()


FAKE_PRODUCER = r'''#!/usr/bin/env python3
import hashlib
import json
from pathlib import Path
import sys

def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()

args = sys.argv[1:]
def one(flag):
    return args[args.index(flag) + 1]

profile = json.loads(Path(one("--profile-manifest")).read_bytes())
output = Path(one("--output"))
output.mkdir()
name = Path(sys.argv[0]).name
if "rpm" in name:
    ecosystem, implementation, version = "rpm", "libsolv", "0.7.36"
elif "debian" in name:
    ecosystem, implementation, version = "debian", "apt-pkg", "3.2.0"
else:
    ecosystem, implementation, version = "alpm", "libalpm", "15.0.0"
impl = {"ecosystem": ecosystem, "name": implementation, "projection_schema": 1, "version": version}
revision_sha = hashlib.sha256(canonical(profile)).hexdigest()
if "--package-oracle" not in args:
    artifact = output / "packages.jsonl"
    artifact.write_bytes(b"")
    manifest = {
        "artifact": {"counts": {"packages": 0, "provides": 0, "requirement_atoms": 0, "requirement_groups": 0}, "sha256": hashlib.sha256(b"").hexdigest(), "size": 0},
        "implementation": impl,
        "members": profile["members"],
        "profile": profile["profile"],
        "profile_logical_digest_sha256": "b" * 64,
        "profile_revision_sha256": revision_sha,
        "schema_version": 1,
    }
else:
    artifact = output / "roots.jsonl"
    artifact.write_bytes(b"")
    package_manifest = (Path(one("--package-oracle")) / "manifest.json").read_bytes()
    manifest = {
        "artifact": {"counts": {"closure_package_references": 0, "resolved_roots": 0, "roots": 0, "unresolved_dependencies": 0, "unresolved_roots": 0}, "sha256": hashlib.sha256(b"").hexdigest(), "size": 0},
        "implementation": impl,
        "members": profile["members"],
        "package_oracle_manifest_sha256": hashlib.sha256(package_manifest).hexdigest(),
        "policy": {"architecture": one("--architecture"), "installed_state": "empty", "positive_requirements": "required_only", "provider_selection": "native_precedence", "roots": "every_exact_package"},
        "profile": profile["profile"],
        "profile_logical_digest_sha256": "b" * 64,
        "profile_revision_sha256": revision_sha,
        "schema_version": 1,
    }
(output / "manifest.json").write_bytes(canonical(manifest))
'''


class NativeOracleLaneTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.input = self.root / "input"
        self.input.mkdir()
        (self.input / "objects").mkdir()
        self.manifest = self.make_manifest()
        self.write_manifest()
        self.fake = self.root / "fake"
        self.fake.write_text(FAKE_PRODUCER)
        self.fake.chmod(self.fake.stat().st_mode | stat.S_IXUSR)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def make_manifest(self) -> dict:
        profiles = []
        objects: dict[str, int] = {}
        configurations = (
            ("fedora-44", "x86_64", ("rpm_primary", "rpm_filelists")),
            ("ubuntu-26.04", "amd64", ("debian_packages",)),
            ("arch", "x86_64", ("arch_database",)),
        )
        for profile, target_architecture, roles in configurations:
            authenticated = []
            for role in roles:
                data = f"{profile}:{role}".encode()
                object_digest = hashlib.sha256(data).hexdigest()
                (self.input / "objects" / object_digest).write_bytes(data)
                objects[object_digest] = len(data)
                authenticated.append({"role": role, "sha256": object_digest, "size": len(data)})
            source = {"authenticated_objects": authenticated, "source_profile": profile}
            revision = {
                "members": [{"ordinal": 0, "source_snapshot_sha256": digest(source)}],
                "profile": profile,
                "schema_version": 3,
                "target_architecture": target_architecture,
            }
            profiles.append({"profile_revision_sha256": digest(revision), "revision": revision, "sources": [source]})
        return {
            "objects": [{"sha256": key, "size": objects[key]} for key in sorted(objects)],
            "profiles": profiles,
            "schema_version": 1,
        }

    def write_manifest(self) -> None:
        (self.input / "manifest.json").write_bytes(canonical(self.manifest))

    def run_lane(self, profile: str = "fedora-44", architecture: str = "x86_64") -> subprocess.CompletedProcess[str]:
        ecosystem = {"fedora-44": "rpm", "ubuntu-26.04": "debian", "arch": "alpm"}[profile]
        package = self.root / f"conary-{ecosystem}-oracle"
        resolution = self.root / f"conary-{ecosystem}-resolution-oracle"
        package.symlink_to(self.fake)
        resolution.symlink_to(self.fake)
        return subprocess.run(
            [
                sys.executable,
                str(PRODUCER),
                "--input-root", str(self.input),
                "--profile", profile,
                "--architecture", architecture,
                "--package-producer", str(package),
                "--resolution-producer", str(resolution),
                "--output-root", str(self.root / "output"),
                "--export-id", "slice6-test",
                "--deployed-commit", COMMIT,
            ],
            text=True,
            capture_output=True,
            check=False,
        )

    def test_produces_exact_package_and_resolution_evidence(self) -> None:
        result = self.run_lane()
        self.assertEqual(result.returncode, 0, result.stderr)
        evidence = json.loads(result.stdout)
        self.assertEqual(evidence["profile"], "fedora-44")
        self.assertEqual(evidence["package_oracle"]["implementation"]["version"], "0.7.36")
        self.assertEqual(evidence["resolution_oracle"]["implementation"]["name"], "libsolv")
        self.assertEqual((self.root / "output" / "evidence.json").read_bytes(), canonical(evidence))

    def test_rejects_reordered_public_profiles(self) -> None:
        self.manifest["profiles"].reverse()
        self.write_manifest()
        result = self.run_lane()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("canonical Fedora, Ubuntu, and Arch order", result.stderr)

    def test_rejects_source_manifest_drift(self) -> None:
        self.manifest["profiles"][0]["sources"][0]["source_profile"] = "other"
        self.write_manifest()
        result = self.run_lane()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("source 0 digest drifted", result.stderr)

    def test_rejects_authenticated_role_drift(self) -> None:
        source = self.manifest["profiles"][0]["sources"][0]
        source["authenticated_objects"].reverse()
        revision = self.manifest["profiles"][0]["revision"]
        revision["members"][0]["source_snapshot_sha256"] = digest(source)
        self.manifest["profiles"][0]["profile_revision_sha256"] = digest(revision)
        self.write_manifest()
        result = self.run_lane()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("authenticated roles changed", result.stderr)

    def test_rejects_object_tamper(self) -> None:
        object_digest = self.manifest["objects"][0]["sha256"]
        (self.input / "objects" / object_digest).write_bytes(b"tampered")
        result = self.run_lane()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("changed size or digest", result.stderr)

    def test_rejects_wrong_target_architecture(self) -> None:
        result = self.run_lane(architecture="amd64")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("architecture must match profile authority x86_64", result.stderr)


if __name__ == "__main__":
    unittest.main()
