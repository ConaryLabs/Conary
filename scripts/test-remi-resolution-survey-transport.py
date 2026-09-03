#!/usr/bin/env python3
# scripts/test-remi-resolution-survey-transport.py

"""Focused mutation tests for the Remi resolution-survey transport contract."""

from __future__ import annotations

import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest


REPO_ROOT = Path(__file__).resolve().parents[1]
TOOL = REPO_ROOT / "scripts" / "remi-resolution-survey-transport.py"
sys.dont_write_bytecode = True
TOOL_SPEC = importlib.util.spec_from_file_location("remi_resolution_survey_transport", TOOL)
assert TOOL_SPEC is not None and TOOL_SPEC.loader is not None
TRANSPORT_TOOL = importlib.util.module_from_spec(TOOL_SPEC)
TOOL_SPEC.loader.exec_module(TRANSPORT_TOOL)
PROFILES = ("fedora-44", "ubuntu-26.04", "arch")
ARCHITECTURES = {"fedora-44": "x86_64", "ubuntu-26.04": "amd64", "arch": "x86_64"}
ORACLE_RUN_ID = "300"
EXPORT_RUN_ID = "200"
DEPLOYMENT_RUN_ID = "100"
EXPORT_ID = "slice6-100-200-1"
DEPLOYED_COMMIT = "d" * 40
BINARY_SHA256 = "e" * 64
PRODUCER_COMMITS = {
    "fedora-44": "4" * 40,
    "ubuntu-26.04": "5" * 40,
    "arch": "6" * 40,
}
PRODUCER_BINARIES = {
    "fedora-44": ("conary-rpm-oracle", "conary-rpm-resolution-oracle"),
    "ubuntu-26.04": ("conary-debian-oracle", "conary-debian-resolution-oracle"),
    "arch": ("conary-alpm-oracle", "conary-alpm-resolution-oracle"),
}
ECOSYSTEMS = {"fedora-44": "rpm", "ubuntu-26.04": "debian", "arch": "alpm"}


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
        "run_attempt": 1,
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
        self.assembly_evidence = root / "assembly-evidence.json"
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
                    f"remi-native-oracle-lane-{profile}-{EXPORT_ID}-{PRODUCER_COMMITS[profile]}"
                    for profile in PROFILES
                ]
                + [f"remi-native-oracle-set-{EXPORT_ID}-{ORACLE_RUN_ID}"]
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
            self.export_root / "native-oracle-export-operator-v1.json",
            {
                "schema_version": 1,
                "export_id": EXPORT_ID,
                "workflow_commit_sha": "a" * 40,
                "workflow_run_id": int(EXPORT_RUN_ID),
                "workflow_run_attempt": 1,
                "ssh_host_key_contract": "protected-pinned-known-hosts-v1",
            },
        )
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
        assembled_lanes = []
        for index, profile in enumerate(PROFILES, start=1):
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
            package_binary, resolution_binary = PRODUCER_BINARIES[profile]
            producer_binaries = {
                "package": {"name": package_binary, "sha256": str(index) * 64},
                "resolution": {"name": resolution_binary, "sha256": str(index + 3) * 64},
            }
            evidence = {
                "schema_version": 3,
                "artifact_type": "native-oracle-lane",
                "deployment_run_id": int(DEPLOYMENT_RUN_ID),
                "export_run_id": int(EXPORT_RUN_ID),
                "export_id": EXPORT_ID,
                "transport_sha256": "0" * 64,
                "deployed_commit": DEPLOYED_COMMIT,
                "producer_commit": PRODUCER_COMMITS[profile],
                "producer_binaries": producer_binaries,
                "lane_image": f"example.invalid/{profile}@sha256:{str(index + 6) * 64}",
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
            assembled_lanes.append(
                {
                    "profile": profile,
                    "profile_revision_sha256": self.candidates[profile],
                    "target_architecture": ARCHITECTURES[profile],
                    "lane_image": evidence["lane_image"],
                    "producer_commit": PRODUCER_COMMITS[profile],
                    "producer_binaries": producer_binaries,
                    "lane_evidence_sha256": digest(canonical(evidence)),
                    "package_oracle": evidence["package_oracle"],
                    "resolution_oracle": evidence["resolution_oracle"],
                    "github_artifact": {
                        "artifact_id": index,
                        "run_id": int(ORACLE_RUN_ID),
                        "name": (
                            f"remi-native-oracle-lane-{profile}-{EXPORT_ID}-"
                            f"{PRODUCER_COMMITS[profile]}"
                        ),
                        "sha256": str(index + 6) * 64,
                    },
                }
            )
            self.lanes[profile] = lane
        write_json(
            self.assembly_evidence,
            {
                "schema_version": 1,
                "artifact_type": "native-oracle-three-lane-set",
                "deployment_run_id": int(DEPLOYMENT_RUN_ID),
                "export_run_id": int(EXPORT_RUN_ID),
                "export_id": EXPORT_ID,
                "transport_sha256": "0" * 64,
                "deployed_commit": DEPLOYED_COMMIT,
                "input_manifest_sha256": self.input_manifest_sha256,
                "lanes": assembled_lanes,
            },
        )

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
            "--assembly-evidence",
            str(self.assembly_evidence),
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
    architecture = ARCHITECTURES[profile]
    outcome_root = "1" * 64
    failure_root = "2" * 64
    error_kind = {"error_variant": "config_error", "reason": "solver_failed"}
    counts = {
        "roots_walked": 2,
        "resolved_roots": 1,
        "unresolved_roots": 0,
        "not_installable_roots": 0,
        "failed_roots": 1,
        "error_kinds": [{"kind": error_kind, "count": 1}],
    }
    return {
        "schema_version": 1,
        "profile": profile,
        "profile_revision_sha256": revision,
        "package_oracle_manifest_sha256": package_manifest,
        "implementation": {
            "ecosystem": ECOSYSTEMS[profile],
            "name": "conary-sat",
            "version": "1",
            "projection_schema": 2,
        },
        "policy": {
            "architecture": architecture,
            "architecture_admission": "native_only",
            "installed_state": "empty",
            "roots": "every_exact_package",
            "positive_requirements": "required_only",
            "provider_selection": "native_precedence",
        },
        "target_architecture": architecture,
        "counts": counts,
        "outcomes": [
            {
                "root_package_key_sha256": outcome_root,
                "name": "example",
                "version": "1:2.0~rc1",
                "release": "1.fc44",
                "architecture": architecture,
                "outcome": {
                    "status": "resolved",
                    "closure_package_keys_sha256": [outcome_root],
                },
            }
        ],
        "failure_record_limit": 5000,
        "total_failures": 1,
        "retained_failures": 1,
        "truncated": False,
        "evidence_byte_limit": 67108864,
        "retained_evidence_bytes": 0,
        "retained_explanations": 0,
        "withheld_explanations": 1,
        "truncated_evidence": True,
        "failures": [
            {
                "root_package_key_sha256": failure_root,
                "name": "broken-example",
                "version": "1",
                "release": "1",
                "architecture": architecture,
                "error_kind": error_kind,
                "error_message": "solver failed",
                "native_explanation": {
                    "source": "withheld",
                    "reason": "evidence_budget_exhausted",
                },
            }
        ],
    }


class ResolutionSurveyTransportTests(unittest.TestCase):
    def test_complete_comparison_schema_and_mismatch_evidence(self) -> None:
        root_sha256 = "7" * 64
        oracle_manifest = "8" * 64
        candidate_manifest = "9" * 64
        profile = {
            "profile": "fedora-44",
            "profile_revision_sha256": "1" * 64,
            "target_architecture": "x86_64",
            "package_oracle_manifest_sha256": "2" * 64,
            "native_resolution_manifest_sha256": oracle_manifest,
        }
        resolved = {
            "status": "resolved",
            "closure_package_keys_sha256": [root_sha256],
        }
        unresolved = {
            "status": "unresolved",
            "dependencies": [
                {
                    "requiring_package_key_sha256": root_sha256,
                    "requirement_group_sha256": "a" * 64,
                }
            ],
        }
        comparison = {
            "schema_version": 1,
            "profile": profile["profile"],
            "profile_revision_sha256": profile["profile_revision_sha256"],
            "package_oracle_manifest_sha256": profile[
                "package_oracle_manifest_sha256"
            ],
            "oracle_manifest_sha256": oracle_manifest,
            "candidate_manifest_sha256": candidate_manifest,
            "counts": {
                "roots_walked": 1,
                "matching_roots": 0,
                "mismatched_roots": 1,
                "mismatch_kinds": [{"kind": "resolution_outcome", "count": 1}],
                "outcome_kind_pairs": [
                    {
                        "pair": {"oracle": "resolved", "candidate": "unresolved"},
                        "count": 1,
                    }
                ],
            },
            "mismatch_record_limit": 5000,
            "total_mismatches": 1,
            "retained_mismatches": 1,
            "truncated": False,
            "mismatches": [
                {
                    "root": {
                        "package_key_sha256": root_sha256,
                        "name": "example",
                        "version": "1:2.0~rc1",
                        "release": "1.fc44",
                        "architecture": "x86_64",
                    },
                    "kind": "resolution_outcome",
                    "oracle": {
                        "manifest_sha256": oracle_manifest,
                        "outcome": resolved,
                    },
                    "candidate": {
                        "manifest_sha256": candidate_manifest,
                        "outcome": unresolved,
                    },
                }
            ],
        }
        TRANSPORT_TOOL.validate_comparison_survey(comparison, profile, "comparison.json")

        malformed = json.loads(canonical(comparison))
        malformed["mismatches"][0]["candidate"]["outcome"] = resolved
        with self.assertRaisesRegex(ValueError, "equal outcomes"):
            TRANSPORT_TOOL.validate_comparison_survey(
                malformed, profile, "comparison.json"
            )

        malformed = json.loads(canonical(comparison))
        malformed["retained_mismatches"] = 0
        with self.assertRaisesRegex(ValueError, "retention counts"):
            TRANSPORT_TOOL.validate_comparison_survey(
                malformed, profile, "comparison.json"
            )

        malformed = json.loads(canonical(comparison))
        malformed["mismatch_record_limit"] = 5001
        with self.assertRaisesRegex(ValueError, "retention counts"):
            TRANSPORT_TOOL.validate_comparison_survey(
                malformed, profile, "comparison.json"
            )

        malformed = json.loads(canonical(comparison))
        malformed["schema_version"] = True
        with self.assertRaisesRegex(ValueError, "unsigned 64-bit integer"):
            TRANSPORT_TOOL.validate_comparison_survey(
                malformed, profile, "comparison.json"
            )

    def test_build_input_binds_all_runs_and_oracle_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fixture = TransportFixture(Path(temporary))
            result = subprocess.run(fixture.command(), text=True, capture_output=True, check=False)
            self.assertEqual(result.returncode, 0, result.stderr)
            evidence = json.loads(fixture.evidence.read_bytes())
            self.assertEqual(evidence["workflow_runs"], {"oracle": 300, "export": 200, "deployment": 100})
            self.assertEqual(
                evidence["export_operator"]["workflow_commit_sha"], "a" * 40
            )
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
            operator = fixture.export_root / "native-oracle-export-operator-v1.json"
            value = json.loads(operator.read_bytes())
            value["ssh_host_key_contract"] = "live-discovery"
            write_json(operator, value)
            result = subprocess.run(fixture.command(), text=True, capture_output=True, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("pinned SSH operator attestation", result.stderr)

        with tempfile.TemporaryDirectory() as temporary:
            fixture = TransportFixture(Path(temporary))
            artifact = fixture.lanes["arch"] / "resolution-oracle" / "roots.jsonl"
            artifact.write_bytes(b"tampered\n")
            result = subprocess.run(fixture.command(), text=True, capture_output=True, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("bindings disagree", result.stderr)

        with tempfile.TemporaryDirectory() as temporary:
            fixture = TransportFixture(Path(temporary))
            assembly = json.loads(fixture.assembly_evidence.read_bytes())
            assembly["lanes"][0]["lane_evidence_sha256"] = "0" * 64
            write_json(fixture.assembly_evidence, assembly)
            result = subprocess.run(fixture.command(), text=True, capture_output=True, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("authenticated three-lane assembly", result.stderr)

        with tempfile.TemporaryDirectory() as temporary:
            fixture = TransportFixture(Path(temporary))
            fixture.oracle_run.write_bytes(b'{"id":300,"id":300}')
            result = subprocess.run(fixture.command(), text=True, capture_output=True, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("repeats key", result.stderr)

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

            def write_output() -> None:
                manifest["files"] = [
                    {"path": name, "sha256": digest(data), "size": len(data)}
                    for name, data in sorted(survey_files.items())
                ]
                write_json(manifest_path, manifest)
                for file_name, file_data in survey_files.items():
                    (root / file_name).write_bytes(file_data)
                with tarfile.open(output, mode="w", format=tarfile.USTAR_FORMAT) as archive:
                    archive.add(manifest_path, arcname="manifest.json")
                    for file_name in sorted(survey_files):
                        archive.add(root / file_name, arcname=file_name)

            write_output()
            verification = root / "verification.json"
            command = [
                "python3",
                str(TOOL),
                "verify-output",
                "--survey-id",
                fixture.survey_id,
                "--export-id",
                EXPORT_ID,
                "--input-evidence",
                str(fixture.evidence),
                "--transport",
                str(output),
                "--evidence",
                str(verification),
            ]
            result = subprocess.run(command, text=True, capture_output=True, check=False)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(
                json.loads(verification.read_bytes())["counts"]["candidate_failures"], 3
            )

            first_name = sorted(survey_files)[0]
            valid_candidate = survey_files[first_name]
            malformed_candidate = json.loads(valid_candidate)
            del malformed_candidate["failure_record_limit"]
            survey_files[first_name] = canonical(malformed_candidate)
            write_output()
            verification.unlink(missing_ok=True)
            result = subprocess.run(command, text=True, capture_output=True, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("fields differ from the exact schema", result.stderr)

            malformed_candidate = json.loads(valid_candidate)
            malformed_candidate["counts"]["error_kinds"][0]["count"] = 2
            survey_files[first_name] = canonical(malformed_candidate)
            write_output()
            result = subprocess.run(command, text=True, capture_output=True, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("histogram disagrees with failures", result.stderr)

            malformed_candidate = json.loads(valid_candidate)
            malformed_candidate["implementation"]["ecosystem"] = "rpm"
            survey_files[first_name] = canonical(malformed_candidate)
            write_output()
            result = subprocess.run(command, text=True, capture_output=True, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("fixed Conary candidate producer", result.stderr)

            malformed_candidate = json.loads(valid_candidate)
            malformed_candidate["evidence_byte_limit"] = 67108865
            survey_files[first_name] = canonical(malformed_candidate)
            write_output()
            result = subprocess.run(command, text=True, capture_output=True, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("retention or evidence counts", result.stderr)
            survey_files[first_name] = valid_candidate
            write_output()

            input_evidence_bytes = fixture.evidence.read_bytes()
            wrong_input = json.loads(input_evidence_bytes)
            wrong_input["deployment"]["binary_sha256"] = "0" * 64
            fixture.evidence.write_bytes(canonical(wrong_input))
            verification.unlink(missing_ok=True)
            result = subprocess.run(command, text=True, capture_output=True, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("deployment binding differs", result.stderr)

            wrong_input = json.loads(input_evidence_bytes)
            wrong_input["profiles"][0]["profile_revision_sha256"] = "0" * 64
            fixture.evidence.write_bytes(canonical(wrong_input))
            result = subprocess.run(command, text=True, capture_output=True, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("survey binding differs", result.stderr)
            fixture.evidence.write_bytes(input_evidence_bytes)

            tampered = bytearray(output.read_bytes())
            marker = survey_files[sorted(survey_files)[0]]
            offset = tampered.find(marker)
            self.assertGreater(offset, 0)
            tampered[offset] = ord("[")
            output.write_bytes(tampered)
            verification.unlink(missing_ok=True)
            result = subprocess.run(command, text=True, capture_output=True, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("changed digest", result.stderr)


if __name__ == "__main__":
    unittest.main()
