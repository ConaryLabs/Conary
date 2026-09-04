#!/usr/bin/env python3
# scripts/test-produce-native-oracle-lane.py

"""Focused mutation tests for production native-oracle lane adaptation."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import unittest


REPO_ROOT = Path(__file__).resolve().parents[1]
PRODUCER = REPO_ROOT / "scripts" / "produce-native-oracle-lane.py"
COMMIT = "a" * 40
PRODUCER_COMMIT = "b" * 40
EMPTY_SHA256 = hashlib.sha256(b"").hexdigest()


def canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def digest(value: object) -> str:
    return hashlib.sha256(canonical(value)).hexdigest()


FAKE_PRODUCER = r'''#!/usr/bin/env python3
import hashlib
import json
import os
from pathlib import Path
import sys

def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()

args = sys.argv[1:]
def one(flag):
    return args[args.index(flag) + 1]

profile = json.loads(Path(one("--profile-manifest")).read_bytes())
name = Path(sys.argv[0]).name
if "rpm" in name:
    ecosystem, implementation, version = "rpm", "libsolv", "0.7.36"
elif "debian" in name:
    ecosystem, implementation, version = "debian", "apt-pkg", "3.2.0"
else:
    ecosystem, implementation, version = "alpm", "libalpm", "15.0.0"
revision_sha = hashlib.sha256(canonical(profile)).hexdigest()
if "--package-oracle" not in args:
    output = Path(one("--output"))
    output.mkdir()
    impl = {"ecosystem": ecosystem, "name": implementation, "projection_schema": 1, "version": version}
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
    resolution_projection = {"rpm": 5, "debian": 3, "alpm": 3}[ecosystem]
    impl = {"ecosystem": ecosystem, "name": implementation, "projection_schema": resolution_projection, "version": version}
    policy = {"architecture": one("--architecture"), "architecture_admission": "native_only", "installed_state": "empty", "positive_requirements": "required_only", "provider_selection": "native_precedence", "roots": "every_exact_package"}
    package_manifest = (Path(one("--package-oracle")) / "manifest.json").read_bytes()
    Path(one("--implementation-evidence")).write_bytes(canonical({
        "memory_budget_bytes": 8589934592,
        "measured_worker_rss_bytes": 536870912,
        "schema_version": 1,
        "worker_load_milliseconds": [12, 13],
        "workers": 2,
    }))
    if "--survey" in args:
        explanation = {"ecosystem": "rpm", "result": {"problems": [], "status": "problems"}}
        diagnostic_outcome = {
            "architecture": one("--architecture"),
            "name": "conflict-root",
            "native_explanation": explanation,
            "outcome": {"reason": "conflicting_closure", "status": "not_installable"},
            "release": "1",
            "root_package_key_sha256": "f" * 64,
            "version": "1",
        }
        survey = {
            "counts": {"error_kinds": [], "failed_roots": 0, "not_installable_roots": 1, "resolved_roots": 0, "roots_walked": 1, "unresolved_roots": 0},
            "evidence_byte_limit": 33554432,
            "failure_record_limit": 5000,
            "diagnostic_outcome_record_limit": 5000,
            "diagnostic_outcomes": [diagnostic_outcome],
            "diagnostic_outcomes_truncated": False,
            "failures": [],
            "implementation": impl,
            "package_oracle_manifest_sha256": hashlib.sha256(package_manifest).hexdigest(),
            "policy": policy,
            "profile": profile["profile"],
            "profile_revision_sha256": revision_sha,
            "retained_evidence_bytes": len(canonical(explanation)),
            "retained_diagnostic_outcomes": 1,
            "retained_explanations": 1,
            "retained_failures": 0,
            "schema_version": 3,
            "target_architecture": one("--architecture"),
            "total_failures": 0,
            "total_diagnostic_outcomes": 1,
            "truncated": False,
            "truncated_evidence": False,
            "withheld_explanations": 0,
        }
        Path(one("--survey")).write_bytes(canonical(survey))
        sys.exit(0)
    if os.environ.get("FAKE_STRICT_FAIL") == "1":
        sys.exit(17)
    output = Path(one("--output"))
    output.mkdir()
    artifact = output / "roots.jsonl"
    artifact.write_bytes(b"")
    manifest = {
        "artifact": {"counts": {"closure_package_references": 0, "resolved_roots": 0, "roots": 0, "unresolved_dependencies": 0, "unresolved_roots": 0}, "sha256": hashlib.sha256(b"").hexdigest(), "size": 0},
        "implementation": impl,
        "members": profile["members"],
        "package_oracle_manifest_sha256": hashlib.sha256(package_manifest).hexdigest(),
        "policy": policy,
        "profile": profile["profile"],
        "profile_logical_digest_sha256": "b" * 64,
        "profile_revision_sha256": revision_sha,
        "schema_version": 3,
    }
(output / "manifest.json").write_bytes(canonical(manifest))
'''


class NativeOracleLaneTests(unittest.TestCase):
    def test_pinned_oracle_schemas_match_rust_authority(self) -> None:
        def constant(path: Path, name: str) -> int:
            match = re.search(
                rf"^pub const {name}: u32 = ([0-9]+);$",
                path.read_text(),
                re.MULTILINE,
            )
            self.assertIsNotNone(match, f"missing Rust constant {name}")
            return int(match.group(1))

        script = PRODUCER.read_text()
        package_pin = re.search(r"^NATIVE_PACKAGE_ORACLE_SCHEMA = ([0-9]+)$", script, re.MULTILINE)
        resolution_pin = re.search(r"^NATIVE_RESOLUTION_ORACLE_SCHEMA = ([0-9]+)$", script, re.MULTILINE)
        self.assertIsNotNone(package_pin)
        self.assertIsNotNone(resolution_pin)
        parity_root = REPO_ROOT / "crates/conary-core/src/repository/catalog/parity"
        self.assertEqual(
            int(package_pin.group(1)),
            constant(parity_root / "contract.rs", "NATIVE_PARITY_ORACLE_SCHEMA_V1"),
        )
        self.assertEqual(
            int(resolution_pin.group(1)),
            constant(
                parity_root / "resolution_contract.rs",
                "NATIVE_RESOLUTION_ORACLE_SCHEMA_V3",
            ),
        )

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

    def run_lane(
        self,
        profile: str = "fedora-44",
        architecture: str = "x86_64",
        producer_commit: str = PRODUCER_COMMIT,
        strict_failure: bool = False,
    ) -> subprocess.CompletedProcess[str]:
        ecosystem = {"fedora-44": "rpm", "ubuntu-26.04": "debian", "arch": "alpm"}[profile]
        package = self.root / f"conary-{ecosystem}-oracle"
        resolution = self.root / f"conary-{ecosystem}-resolution-oracle"
        package.write_text(FAKE_PRODUCER)
        resolution.write_text(FAKE_PRODUCER)
        package.chmod(package.stat().st_mode | stat.S_IXUSR)
        resolution.chmod(resolution.stat().st_mode | stat.S_IXUSR)
        return subprocess.run(
            [
                sys.executable,
                str(PRODUCER),
                "--input-root", str(self.input),
                "--profile", profile,
                "--architecture", architecture,
                "--package-producer", str(package),
                "--resolution-producer", str(resolution),
                "--output-root", str(self.root / f"output-{profile}"),
                "--survey-output-root", str(self.root / f"survey-{profile}"),
                "--deployment-run-id", "123",
                "--export-run-id", "456",
                "--export-id", "slice6-test",
                "--transport-sha256", "c" * 64,
                "--deployed-commit", COMMIT,
                "--producer-commit", producer_commit,
                "--lane-image", "example.invalid/native@sha256:" + "d" * 64,
            ],
            text=True,
            capture_output=True,
            check=False,
            env={**os.environ, "FAKE_STRICT_FAIL": "1" if strict_failure else "0"},
        )

    def test_produces_exact_package_and_resolution_evidence(self) -> None:
        result = self.run_lane()
        self.assertEqual(result.returncode, 0, result.stderr)
        evidence = json.loads(result.stdout)
        self.assertEqual(evidence["profile"], "fedora-44")
        self.assertEqual(evidence["schema_version"], 5)
        self.assertEqual(evidence["artifact_type"], "native-oracle-lane")
        self.assertEqual(evidence["deployment_run_id"], 123)
        self.assertEqual(evidence["export_run_id"], 456)
        self.assertEqual(evidence["transport_sha256"], "c" * 64)
        self.assertEqual(evidence["deployed_commit"], COMMIT)
        self.assertEqual(evidence["producer_commit"], PRODUCER_COMMIT)
        self.assertEqual(
            evidence["producer_binaries"]["package"]["sha256"],
            hashlib.sha256(FAKE_PRODUCER.encode()).hexdigest(),
        )
        self.assertEqual(
            evidence["producer_binaries"]["resolution"]["sha256"],
            hashlib.sha256(FAKE_PRODUCER.encode()).hexdigest(),
        )
        self.assertEqual(evidence["package_oracle"]["schema_version"], 1)
        self.assertEqual(evidence["resolution_oracle"]["schema_version"], 3)
        self.assertEqual(evidence["package_oracle"]["implementation"]["version"], "0.7.36")
        self.assertEqual(evidence["resolution_oracle"]["implementation"]["projection_schema"], 5)
        self.assertEqual(evidence["resolution_oracle"]["implementation"]["name"], "libsolv")
        self.assertEqual(evidence["resolution_implementation"]["workers"], 2)
        self.assertEqual(
            (self.root / "output-fedora-44" / "evidence.json").read_bytes(),
            canonical(evidence),
        )
        survey_root = self.root / "survey-fedora-44"
        survey = json.loads((survey_root / "survey.json").read_bytes())
        manifest = json.loads((survey_root / "manifest.json").read_bytes())
        self.assertEqual(manifest["schema_version"], 2)
        self.assertEqual(manifest["artifact_type"], "native-resolution-survey-diagnostics")
        self.assertEqual(manifest["producer_commit"], PRODUCER_COMMIT)
        self.assertEqual(manifest["survey"]["sha256"], digest(survey))
        self.assertEqual(manifest["survey"]["schema_version"], 3)
        self.assertEqual(
            survey["diagnostic_outcomes"][0]["outcome"],
            {"reason": "conflicting_closure", "status": "not_installable"},
        )
        self.assertEqual(manifest["resolution_implementation"]["workers"], 2)

    def test_fake_matches_current_resolution_projection_schemas(self) -> None:
        for profile, architecture, projection_schema in (
            ("fedora-44", "x86_64", 5),
            ("ubuntu-26.04", "amd64", 3),
            ("arch", "x86_64", 3),
        ):
            with self.subTest(profile=profile):
                result = self.run_lane(profile, architecture)
                self.assertEqual(result.returncode, 0, result.stderr)
                evidence = json.loads(result.stdout)
                self.assertEqual(evidence["resolution_oracle"]["schema_version"], 3)
                self.assertEqual(
                    evidence["resolution_oracle"]["implementation"]["projection_schema"],
                    projection_schema,
                )

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

    def test_retains_survey_when_strict_resolution_fails(self) -> None:
        result = self.run_lane(strict_failure=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("native resolution producer failed with exit status 17", result.stderr)
        survey_root = self.root / "survey-fedora-44"
        self.assertTrue((survey_root / "survey.json").is_file())
        manifest = json.loads((survey_root / "manifest.json").read_bytes())
        self.assertEqual(manifest["artifact_type"], "native-resolution-survey-diagnostics")
        self.assertFalse((self.root / "output-fedora-44" / "evidence.json").exists())

    def test_rejects_malformed_producer_commit(self) -> None:
        result = self.run_lane(producer_commit="main")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("producer commit must be a full lowercase commit digest", result.stderr)

    def test_rejects_symlinked_producer_binary(self) -> None:
        ecosystem = "rpm"
        package = self.root / f"conary-{ecosystem}-oracle"
        resolution = self.root / f"conary-{ecosystem}-resolution-oracle"
        package.symlink_to(self.fake)
        resolution.write_text(FAKE_PRODUCER)
        resolution.chmod(resolution.stat().st_mode | stat.S_IXUSR)
        result = subprocess.run(
            [
                sys.executable,
                str(PRODUCER),
                "--input-root", str(self.input),
                "--profile", "fedora-44",
                "--architecture", "x86_64",
                "--package-producer", str(package),
                "--resolution-producer", str(resolution),
                "--output-root", str(self.root / "output-symlink"),
                "--survey-output-root", str(self.root / "survey-symlink"),
                "--deployment-run-id", "123",
                "--export-run-id", "456",
                "--export-id", "slice6-test",
                "--transport-sha256", "c" * 64,
                "--deployed-commit", COMMIT,
                "--producer-commit", PRODUCER_COMMIT,
                "--lane-image", "example.invalid/native@sha256:" + "d" * 64,
            ],
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must be a regular file, never a symlink", result.stderr)


if __name__ == "__main__":
    unittest.main()
