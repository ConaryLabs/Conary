#!/usr/bin/env python3
"""Regression tests for check-third-party-divergence.py."""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("check-third-party-divergence.py")
sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location("third_party_divergence", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
CHECKER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CHECKER
SPEC.loader.exec_module(CHECKER)


PATCH_ENTRY = """
[[dependency]]
id = "demo-patch"
cargo_name = "demo"
kind = "crates-io-patch"
declaration = "Cargo.toml:[patch.crates-io].demo"
path = "third_party/demo"
baseline = "1.0.0"
upstream = "https://crates.io/crates/demo"
upstream_index = "de/mo/demo"
divergence = "Carries one focused local fix."
reason = "The released crate lacks the required behavior."
exit_type = "crates-io-newer-release"
exit_condition = "Evaluate every newer release and remove the patch when the regression passes."
exit_test = "cargo test -p demo"
"""

GIT_ENTRY = """
[[dependency]]
id = "fork-builder"
cargo_name = "fork"
kind = "git-dependency"
declaration = "Cargo.toml:[workspace.dependencies].fork"
git = "https://github.com/ConaryLabs/fork"
rev = "1111111111111111111111111111111111111111"
upstream = "https://github.com/upstream/fork"
upstream_repo = "upstream/fork"
upstream_ref = "main"
upstream_base = "2222222222222222222222222222222222222222"
divergence = "Carries a builder correction."
reason = "The upstream builder does not preserve required semantics."
exit_type = "upstream-git-integration"
exit_condition = "Drop the fork when upstream contains the correction."
exit_test = "cargo test -p consumer"
"""


def write_fixture(root: Path, entries: str = PATCH_ENTRY + GIT_ENTRY) -> None:
    (root / "third_party/demo").mkdir(parents=True)
    (root / "Cargo.toml").write_text(
        """[workspace]
members = []

[patch.crates-io]
demo = { path = "third_party/demo" }

[workspace.dependencies]
fork = { git = "https://github.com/ConaryLabs/fork", rev = "1111111111111111111111111111111111111111" }
""",
        encoding="utf-8",
    )
    (root / "third_party/demo/Cargo.toml").write_text(
        """[package]
name = "demo"
version = "1.0.0"
""",
        encoding="utf-8",
    )
    (root / "third_party/README.md").write_text(
        f"""# Inventory

<!-- conary-third-party-divergence:start -->
```toml
schema = 1
cadence = "Every six hours"
owner = "Dependency reviewers"
{entries}```
<!-- conary-third-party-divergence:end -->
""",
        encoding="utf-8",
    )


class StructuralInventoryTests(unittest.TestCase):
    def test_complete_inventory_matches_manifest_divergence(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_fixture(root)
            self.assertEqual(len(CHECKER.validate(root)), 2)

    def test_unlisted_patch_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_fixture(root, GIT_ENTRY)
            with self.assertRaisesRegex(CHECKER.InventoryError, "unlisted Cargo divergence.*demo"):
                CHECKER.validate(root)

    def test_unlisted_git_dependency_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_fixture(root, PATCH_ENTRY)
            with self.assertRaisesRegex(CHECKER.InventoryError, "unlisted Cargo divergence.*fork"):
                CHECKER.validate(root)

    def test_stale_git_revision_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_fixture(root)
            readme = root / "third_party/README.md"
            readme.write_text(
                readme.read_text(encoding="utf-8").replace("1" * 40, "3" * 40),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(CHECKER.InventoryError, "does not match Cargo rev"):
                CHECKER.validate(root)

    def test_missing_exit_test_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_fixture(root)
            readme = root / "third_party/README.md"
            readme.write_text(
                readme.read_text(encoding="utf-8").replace(
                    'exit_test = "cargo test -p demo"', 'exit_test = ""', 1
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(CHECKER.InventoryError, "exit_test must be"):
                CHECKER.validate(root)

    def test_missing_scheduled_exit_gate_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_fixture(root)
            workflows = root / ".github/workflows"
            workflows.mkdir(parents=True)
            structural = "\n".join(
                (
                    "python3 scripts/check-third-party-divergence.py",
                    "python3 scripts/test-third-party-divergence.py",
                )
            )
            (workflows / "pr-gate.yml").write_text(structural, encoding="utf-8")
            (workflows / "merge-validation.yml").write_text(structural, encoding="utf-8")
            (workflows / "scheduled-ops.yml").write_text(
                "- cron: '0 */6 * * *'\n"
                "GITHUB_TOKEN: ${{ github.token }}\n"
                "bash scripts/release-cargo-audit.sh\n",
                encoding="utf-8",
            )
            scripts = root / "scripts"
            scripts.mkdir()
            (scripts / "release-cargo-audit.sh").write_text(
                "cargo audit\n", encoding="utf-8"
            )
            with self.assertRaisesRegex(CHECKER.InventoryError, "missing divergence gate wiring"):
                CHECKER.validate(root)


class UpstreamExitTests(unittest.TestCase):
    def test_dependency_floor_detects_exit_candidate(self) -> None:
        entry = {
            "id": "demo",
            "cargo_name": "demo",
            "baseline": "1.0.0",
            "upstream_index": "de/mo/demo",
            "exit_type": "crates-io-dependency-floor",
            "exit_dependency": "quick-xml",
            "exit_requirement": ">=0.41",
            "exit_test": "cargo test",
        }
        response = "\n".join(
            json.dumps(release)
            for release in (
                {"vers": "1.0.0", "yanked": False, "deps": []},
                {
                    "vers": "1.1.0",
                    "yanked": False,
                    "deps": [
                        {"name": "quick-xml", "req": "^0.41", "kind": "normal"}
                    ],
                },
            )
        )
        with self.assertRaisesRegex(CHECKER.InventoryError, "exit condition met.*1.1.0"):
            CHECKER.audit_crates_io_entry(entry, lambda _url: response)

    def test_newer_release_ignores_yanked_and_prerelease_versions(self) -> None:
        entry = {
            "id": "demo",
            "cargo_name": "demo",
            "baseline": "1.0.0",
            "upstream_index": "de/mo/demo",
            "exit_type": "crates-io-newer-release",
            "exit_test": "cargo test",
        }
        pending = "\n".join(
            json.dumps(release)
            for release in (
                {"vers": "1.1.0-alpha.1", "yanked": False, "deps": []},
                {"vers": "1.1.0", "yanked": True, "deps": []},
            )
        )
        self.assertIn(
            "no newer non-yanked release",
            CHECKER.audit_crates_io_entry(entry, lambda _url: pending),
        )

    def test_git_integration_detects_contained_pin(self) -> None:
        entry = {
            "id": "fork",
            "git": "https://github.com/ConaryLabs/fork",
            "rev": "1" * 40,
            "upstream_repo": "upstream/fork",
            "upstream_ref": "main",
            "upstream_base": "2" * 40,
            "exit_test": "cargo test",
        }
        response = json.dumps(
            {
                "merge_base_commit": {"sha": "2" * 40},
                "behind_by": 0,
                "ahead_by": 3,
            }
        )
        with self.assertRaisesRegex(CHECKER.InventoryError, "contained in upstream"):
            CHECKER.audit_git_entry(entry, lambda _url: response)

    def test_requirement_floor_handles_cargo_lower_bounds(self) -> None:
        self.assertEqual(CHECKER.requirement_floor("^0.41"), (0, 41, 0))
        self.assertEqual(CHECKER.requirement_floor(">=0.41, <0.42"), (0, 41, 0))
        self.assertIsNone(CHECKER.requirement_floor("<0.41"))


if __name__ == "__main__":
    unittest.main()
