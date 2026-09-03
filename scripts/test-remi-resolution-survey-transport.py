#!/usr/bin/env python3
# scripts/test-remi-resolution-survey-transport.py

"""Focused mutation tests for the Remi resolution-survey transport contract."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest


REPO_ROOT = Path(__file__).resolve().parents[1]
TOOL = REPO_ROOT / "scripts" / "remi-resolution-survey-transport.py"
PROFILES = ("fedora-44", "ubuntu-26.04", "arch")
ARCHITECTURES = {"fedora-44": "x86_64", "ubuntu-26.04": "amd64", "arch": "x86_64"}
ORACLE_RUN_ID = "300"
EXPORT_RUN_ID = "200"
DEPLOYMENT_RUN_ID = "100"
EXPORT_ID = "slice6-100-200-1"
DEPLOYED_COMMIT = "d" * 40
BINARY_SHA256 = "e" * 64


def canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def write_json(path: Path, value: object) -> None:
    path.write_bytes(canonical(value))


def run_metadata(run_id: str, workflow: str) -> dict[str, object]:
    return {
        "id": int(run_id),
        "event": "workflow_dispatch",
        "status": "completed",
        "conclusion": "success",
        "head_branch": "main",
        "head_repository": {"full_name": "FieldmouseWorks/Conary"},
        "head_sha": "a" * 40,
        "path": workflow,
    }


def artifact_metadata(names: list[str]) -> dict[str, object]:
    return {"artifacts": [{"name": name, "expired": False} for name in names]}


class TransportFixture:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.survey_id = "survey-300-400-1"
        self.input_manifest_sha256 = "f" * 64
        self.candidates = {
            "fedora-44": "1" * 64,
            "ubuntu-26.04": "2" * 64,
            "arch": "3" * 64,
        }
        self.oracle_run = root / "oracle-run.json"
        self.oracle_artifacts = root / "oracle-artifacts.json"
        self.export_run = root / "export-run.json"
        self.export_artifacts = root / "export-artifacts.json"
        self.deployment_run = root / "deployment-run.json"
        self.export_root = root / "export"
        self.transport = root / "oracle-transport.tar"
        self.evidence = root / "oracle-evidence.json"
        self.lanes: dict[str, Path] = {}
        self._write()

    def _write(self) -> None:
        write_json(
            self.oracle_run,
            run_metadata(ORACLE_RUN_ID, ".github/workflows/produce-remi-native-oracles.yml"),
        )
        write_json(
            self.oracle_artifacts,
            artifact_metadata(
                [
                    f"remi-native-oracles-{profile}-{EXPORT_RUN_ID}-{ORACLE_RUN_ID}"
                    for profile in PROFILES
                ]
            ),
        )
        write_json(
            self.export_run,
            run_metadata(EXPORT_RUN_ID, ".github/workflows/export-remi-native-oracle-inputs.yml"),
        )
        write_json(
            self.export_artifacts,
            artifact_metadata(
                [f"remi-native-oracle-input-{DEPLOYMENT_RUN_ID}-{EXPORT_RUN_ID}"]
            ),
        )
        write_json(
            self.deployment_run,
            run_metadata(DEPLOYMENT_RUN_ID, ".github/workflows/deploy-remi-candidate.yml"),
        )
        self.export_root.mkdir()
        write_json(
            self.export_root / "native-oracle-input-verification.json",
            {
                "schema_version": 1,
                "export_id": EXPORT_ID,
                "transport": {"sha256": "0" * 64, "size": 1},
                "manifest": {"sha256": self.input_manifest_sha256},
                "profiles": [
                    {"profile": profile, "profile_revision_sha256": self.candidates[profile]}
                    for profile in PROFILES
                ],
                "counts": {"profiles": 3},
            },
        )
        write_json(
            self.export_root / "remi-deployment-inspection.json",
            {
                "deployment_evidence_schema_version": 3,
                "deployment": {
                    "commit_sha": DEPLOYED_COMMIT,
                    "binary_sha256": BINARY_SHA256,
                    "completion_mode": "private-candidates",
                    "outcome": "complete",
                    "failure_phase": None,
                },
                "candidates": [
                    {"profile": profile, "profile_revision_sha256": self.candidates[profile]}
                    for profile in PROFILES
                ],
            },
        )
        for profile in PROFILES:
            lane = self.root / profile
            package_root = lane / "package-oracle"
            resolution_root = lane / "resolution-oracle"
            package_root.mkdir(parents=True)
            resolution_root.mkdir()
            package_artifact = f"{profile} package rows\n".encode()
            resolution_artifact = f"{profile} resolution rows\n".encode()
            (package_root / "packages.jsonl").write_bytes(package_artifact)
            (resolution_root / "roots.jsonl").write_bytes(resolution_artifact)
            package_manifest = {
                "schema_version": 1,
                "profile": profile,
                "profile_revision_sha256": self.candidates[profile],
                "artifact": {
                    "sha256": digest(package_artifact),
                    "size": len(package_artifact),
                    "counts": {"packages": 1},
                },
            }
            package_manifest_bytes = canonical(package_manifest)
            (package_root / "manifest.json").write_bytes(package_manifest_bytes)
            resolution_manifest = {
                "schema_version": 2,
                "profile": profile,
                "profile_revision_sha256": self.candidates[profile],
                "package_oracle_manifest_sha256": digest(package_manifest_bytes),
                "policy": {"architecture": ARCHITECTURES[profile]},
                "artifact": {
                    "sha256": digest(resolution_artifact),
                    "size": len(resolution_artifact),
                    "counts": {"roots": 1},
                },
            }
            resolution_manifest_bytes = canonical(resolution_manifest)
            (resolution_root / "manifest.json").write_bytes(resolution_manifest_bytes)
            evidence = {
                "schema_version": 1,
                "export_id": EXPORT_ID,
                "deployed_commit": DEPLOYED_COMMIT,
                "input_manifest_sha256": self.input_manifest_sha256,
                "profile": profile,
                "profile_revision_sha256": self.candidates[profile],
                "target_architecture": ARCHITECTURES[profile],
                "package_oracle": {
                    "schema_version": 1,
                    "manifest_sha256": digest(package_manifest_bytes),
                    "artifact": {
                        "name": "packages.jsonl",
                        "sha256": digest(package_artifact),
                        "size": len(package_artifact),
                        "counts": {"packages": 1},
                    },
                },
                "resolution_oracle": {
                    "schema_version": 2,
                    "manifest_sha256": digest(resolution_manifest_bytes),
                    "artifact": {
                        "name": "roots.jsonl",
                        "sha256": digest(resolution_artifact),
                        "size": len(resolution_artifact),
                        "counts": {"roots": 1},
                    },
                },
            }
            write_json(lane / "evidence.json", evidence)
            self.lanes[profile] = lane

    def command(self) -> list[str]:
        command = [
            "python3",
            str(TOOL),
            "build-input",
            "--survey-id",
            self.survey_id,
            "--repository",
            "FieldmouseWorks/Conary",
            "--oracle-run-id",
            ORACLE_RUN_ID,
            "--oracle-run",
            str(self.oracle_run),
            "--oracle-artifacts",
            str(self.oracle_artifacts),
            "--export-run-id",
            EXPORT_RUN_ID,
            "--export-run",
            str(self.export_run),
            "--export-artifacts",
            str(self.export_artifacts),
            "--deployment-run-id",
            DEPLOYMENT_RUN_ID,
            "--deployment-run",
            str(self.deployment_run),
            "--export-root",
            str(self.export_root),
        ]
        for profile in PROFILES:
            command.extend(("--lane", f"{profile}={self.lanes[profile]}"))
        command.extend(("--output", str(self.transport), "--evidence", str(self.evidence)))
        return command


def candidate_survey(profile: str, revision: str, package_manifest: str) -> dict[str, object]:
    counts = {
        "roots_walked": 2,
        "resolved_roots": 1,
        "unresolved_roots": 0,
        "not_installable_roots": 0,
        "failed_roots": 1,
        "error_kinds": [{"kind": {"error_variant": "config_error", "reason": "solver_failed"}, "count": 1}],
    }
    return {
        "schema_version": 1,
        "profile": profile,
        "profile_revision_sha256": revision,
        "package_oracle_manifest_sha256": package_manifest,
        "implementation": {},
        "policy": {"architecture": ARCHITECTURES[profile]},
        "target_architecture": ARCHITECTURES[profile],
        "counts": counts,
        "outcomes": [],
        "total_failures": 1,
        "failures": [],
    }


class ResolutionSurveyTransportTests(unittest.TestCase):
    def test_build_input_binds_all_runs_and_oracle_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fixture = TransportFixture(Path(temporary))
            result = subprocess.run(fixture.command(), text=True, capture_output=True, check=False)
            self.assertEqual(result.returncode, 0, result.stderr)
            evidence = json.loads(fixture.evidence.read_bytes())
            self.assertEqual(evidence["workflow_runs"], {"oracle": 300, "export": 200, "deployment": 100})
            self.assertEqual(evidence["deployment"]["binary_sha256"], BINARY_SHA256)
            with tarfile.open(fixture.transport, mode="r:") as archive:
                self.assertEqual(
                    archive.getnames(),
                    ["manifest.json"]
                    + [
                        f"{profile}/{kind}/{name}"
                        for profile in PROFILES
                        for kind, name in (
                            ("package-oracle", "manifest.json"),
                            ("package-oracle", "packages.jsonl"),
                            ("native-resolution", "manifest.json"),
                            ("native-resolution", "roots.jsonl"),
                        )
                    ],
                )

    def test_build_input_rejects_run_and_lane_binding_drift(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fixture = TransportFixture(Path(temporary))
            value = json.loads(fixture.oracle_run.read_bytes())
            value["conclusion"] = "failure"
            write_json(fixture.oracle_run, value)
            result = subprocess.run(fixture.command(), text=True, capture_output=True, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("successful protected-main", result.stderr)

        with tempfile.TemporaryDirectory() as temporary:
            fixture = TransportFixture(Path(temporary))
            artifact = fixture.lanes["arch"] / "resolution-oracle" / "roots.jsonl"
            artifact.write_bytes(b"tampered\n")
            result = subprocess.run(fixture.command(), text=True, capture_output=True, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("bindings disagree", result.stderr)

    def test_verify_output_reopens_manifest_files_and_findings(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixture = TransportFixture(root)
            subprocess.run(fixture.command(), check=True, capture_output=True)
            with tarfile.open(fixture.transport, mode="r:") as oracle:
                input_manifest = json.loads(oracle.extractfile("manifest.json").read())
            survey_files: dict[str, bytes] = {}
            profiles = []
            for binding in input_manifest["profiles"]:
                profile = binding["profile"]
                name = f"{profile}.candidate-resolution-survey.json"
                value = candidate_survey(
                    profile,
                    binding["profile_revision_sha256"],
                    binding["package_oracle"]["manifest_sha256"],
                )
                data = canonical(value)
                survey_files[name] = data
                profiles.append(
                    {
                        "profile": profile,
                        "profile_revision_sha256": binding["profile_revision_sha256"],
                        "target_architecture": binding["target_architecture"],
                        "package_oracle_manifest_sha256": binding["package_oracle"]["manifest_sha256"],
                        "native_resolution_manifest_sha256": binding["native_resolution"]["manifest_sha256"],
                        "candidate": {
                            "file": name,
                            "counts": value["counts"],
                            "total_failures": 1,
                            "error_histogram": value["counts"]["error_kinds"],
                        },
                        "comparison": None,
                    }
                )
            manifest = {
                "schema_version": 1,
                "survey_id": fixture.survey_id,
                "export_id": EXPORT_ID,
                "deployment": input_manifest["deployment"],
                "profiles": profiles,
                "counts": {
                    "profiles": 3,
                    "roots_walked": 6,
                    "candidate_failures": 3,
                    "comparison_profiles": 0,
                    "comparison_mismatches": 0,
                },
                "files": [
                    {"path": name, "sha256": digest(data), "size": len(data)}
                    for name, data in sorted(survey_files.items())
                ],
            }
            output = root / "survey.tar"
            manifest_path = root / "manifest.json"
            write_json(manifest_path, manifest)
            for name, data in survey_files.items():
                (root / name).write_bytes(data)
            with tarfile.open(output, mode="w", format=tarfile.USTAR_FORMAT) as archive:
                archive.add(manifest_path, arcname="manifest.json")
                for name in sorted(survey_files):
                    archive.add(root / name, arcname=name)
            verification = root / "verification.json"
            command = [
                "python3",
                str(TOOL),
                "verify-output",
                "--survey-id",
                fixture.survey_id,
                "--export-id",
                EXPORT_ID,
                "--transport",
                str(output),
                "--evidence",
                str(verification),
            ]
            result = subprocess.run(command, text=True, capture_output=True, check=False)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(verification.read_bytes())["counts"]["candidate_failures"], 3)

            tampered = bytearray(output.read_bytes())
            marker = survey_files[sorted(survey_files)[0]]
            offset = tampered.find(marker)
            self.assertGreater(offset, 0)
            tampered[offset] = ord("[")
            output.write_bytes(tampered)
            verification.unlink()
            result = subprocess.run(command, text=True, capture_output=True, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("changed digest", result.stderr)


if __name__ == "__main__":
    unittest.main()
