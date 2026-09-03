#!/usr/bin/env python3
# scripts/test-verify-native-oracle-producer.py

"""Focused tests for the shared native-oracle producer source predicate."""

from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


VERIFIER = Path(__file__).with_name("verify-native-oracle-producer.py")


class NativeOracleProducerVerificationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.repository = Path(self.temporary.name) / "repository"
        self.repository.mkdir()
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Producer Verification Test")
        self.git("config", "user.email", "producer-verification@test.invalid")
        (self.repository / "tracked").write_text("base\n")
        self.git("add", "tracked")
        self.git("commit", "-q", "-m", "deployed")
        self.deployed = self.git("rev-parse", "HEAD")
        self.git("commit", "--allow-empty", "-q", "-m", "producer")
        self.producer = self.git("rev-parse", "HEAD")
        tree = self.git("rev-parse", f"{self.deployed}^{{tree}}")
        self.unmerged = self.git("commit-tree", tree, "-p", self.deployed, input="unmerged\n")
        self.unrelated = self.git("commit-tree", tree, input="unrelated\n")
        self.git("remote", "add", "origin", ".")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def git(self, *arguments: str, input: str | None = None) -> str:
        return subprocess.run(
            ["git", *arguments],
            cwd=self.repository,
            input=input,
            text=True,
            capture_output=True,
            check=True,
        ).stdout.strip()

    def run_verifier(
        self, deployed: str | None = None, producer: str | None = None
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                sys.executable,
                str(VERIFIER),
                "--repository", str(self.repository),
                "--deployed-commit", deployed or self.deployed,
                "--producer-commit", producer or self.producer,
            ],
            text=True,
            capture_output=True,
            check=False,
        )

    def test_accepts_merged_descendant(self) -> None:
        result = self.run_verifier()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            json.loads(result.stdout),
            {"deployed_commit": self.deployed, "producer_commit": self.producer},
        )

    def test_rejects_malformed_commit(self) -> None:
        result = self.run_verifier(producer="main")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("full lowercase 40-hex SHA", result.stderr)

    def test_rejects_non_descendant(self) -> None:
        result = self.run_verifier(producer=self.unrelated)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("deployed-to-producer ancestry failed", result.stderr)

    def test_rejects_unmerged_descendant(self) -> None:
        result = self.run_verifier(producer=self.unmerged)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("producer-to-origin/main ancestry failed", result.stderr)


if __name__ == "__main__":
    unittest.main()
