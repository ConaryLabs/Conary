#!/usr/bin/env python3
# scripts/test-native-oracle-lane-selection.py

"""Focused tests for the closed production native-oracle lane subset."""

from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys
import unittest


SELECTOR = Path(__file__).with_name("native-oracle-lane-selection.py")


class NativeOracleLaneSelectionTests(unittest.TestCase):
    def run_selection(self, lanes: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SELECTOR), "--lanes", lanes],
            text=True,
            capture_output=True,
            check=False,
        )

    def test_default_canonical_set(self) -> None:
        result = self.run_selection("fedora-44,ubuntu-26.04,arch")
        self.assertEqual(result.returncode, 0, result.stderr)
        matrix = json.loads(result.stdout)
        self.assertEqual(
            [entry["profile"] for entry in matrix["include"]],
            ["fedora-44", "ubuntu-26.04", "arch"],
        )

    def test_accepts_nonempty_subsets_in_requested_order(self) -> None:
        for lanes, expected in (
            ("arch", ["arch"]),
            ("ubuntu-26.04,fedora-44", ["ubuntu-26.04", "fedora-44"]),
        ):
            with self.subTest(lanes=lanes):
                result = self.run_selection(lanes)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(
                    [entry["profile"] for entry in json.loads(result.stdout)["include"]],
                    expected,
                )

    def test_rejects_empty_duplicate_unknown_or_noncanonical_values(self) -> None:
        for lanes in (
            "",
            ",fedora-44",
            "fedora-44,",
            "fedora-44,,arch",
            "arch,arch",
            "debian",
            "fedora-44, arch",
        ):
            with self.subTest(lanes=lanes):
                result = self.run_selection(lanes)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("native-oracle lane selection failed", result.stderr)


if __name__ == "__main__":
    unittest.main()
