#!/usr/bin/env python3
# scripts/test-remi-survey-ssh-diagnostic.py
"""Exercise public survey diagnostics with synthetic SSH connection failures."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("remi-survey-ssh-diagnostic.py")
SPEC = importlib.util.spec_from_file_location("ssh_diagnostic", SCRIPT)
diagnostic = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(diagnostic)
TARGET = "surveyoperator@destination.example.invalid"
CONFIG = "Host production-alias\n HostName backend.example.invalid\n User surveyoperator\n"
KNOWN_HOSTS = "@cert-authority pin.example.invalid,[2001:db8::9]:2222 ssh-ed25519 synthetic-key\n"
IDENTIFIERS = diagnostic.connection_identifiers(TARGET, CONFIG, KNOWN_HOSTS)
FAILURES = (
    "ssh: connect to host destination.example.invalid port 22: Connection timed out",
    "surveyoperator@destination.example.invalid: Permission denied (publickey).",
    "Permission denied (publickey) for surveyoperator@destination.example.invalid",
    "Host key verification failed for pin.example.invalid (backend.example.invalid).",
    "kex_exchange_identification: Connection closed by production-alias [2001:db8::9] port 2222",
    "Connection reset by 192.0.2.31 port 22",
    "Connection closed by 2001:0db8:0000:0000:0000:0000:0000:0042 port 22",
    "Connection closed by ::ffff:198.51.100.8 port 22",
    "Connection closed by [fe80::1%eth7]:22",
    "Host DESTINATION.EXAMPLE.INVALID: user SURVEYOPERATOR failed",
)


class SshDiagnosticTests(unittest.TestCase):
    def assert_private_bytes_absent(self, result):
        serialized = json.dumps(result).casefold()
        for identifier in IDENTIFIERS:
            self.assertNotIn(identifier, serialized)
        self.assertIsNone(diagnostic.IP_LITERAL.search(serialized))

    def test_openssh_failures_redact_all_connection_identifiers(self):
        for line in FAILURES:
            with self.subTest(line=line):
                result = diagnostic.sanitize(line, IDENTIFIERS)
                self.assertEqual(result["outcome"], "sanitized")
                self.assertIn("<", result["message"])
                self.assert_private_bytes_absent(result)
        self.assertIn("<ssh-target>", diagnostic.sanitize(FAILURES[1], IDENTIFIERS)["message"])
        self.assertIn("<ip>", diagnostic.sanitize(FAILURES[5], IDENTIFIERS)["message"])

    def test_residual_connection_identifier_withholds_the_stderr(self):
        for line in FAILURES:
            with self.subTest(line=line):
                result = diagnostic.public_diagnostic(line, IDENTIFIERS)
                self.assertEqual(result, diagnostic.withheld("connection_identifier_remaining"))
                self.assertNotIn("message", result)

    def test_second_line_is_checked_before_first_line_is_selected(self):
        result = diagnostic.public_diagnostic("Host key verification failed.\n" + FAILURES[0], IDENTIFIERS)
        self.assertEqual(result["outcome"], "withheld")

    def test_obfuscated_terminal_diagnostics_are_withheld(self):
        result = diagnostic.sanitize("destination\x1b[0m.example.invalid", IDENTIFIERS)
        self.assertEqual(result, diagnostic.withheld("unsupported_diagnostic_encoding"))

    def test_paths_are_redacted_and_helper_clause_is_preserved(self):
        result = diagnostic.sanitize("outcome.document_count; /var/lib/remi/private", IDENTIFIERS)
        self.assertEqual(result["message"], "outcome.document_count; <redacted-path>")

    def test_cli_uses_private_metadata_and_fails_closed_when_unavailable(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            config, known_hosts, stderr = (root / name for name in ("config", "known_hosts", "stderr"))
            config.write_text(CONFIG)
            known_hosts.write_text(KNOWN_HOSTS)
            command = ["python3", str(SCRIPT), "--stderr", str(stderr), "--ssh-config", str(config), "--known-hosts", str(known_hosts)]
            for line in FAILURES:
                stderr.write_text(line)
                process = subprocess.run(command, env={**os.environ, "REMI_SSH_TARGET": TARGET}, capture_output=True, text=True, check=True)
                self.assertEqual(process.stderr, "")
                self.assert_private_bytes_absent(json.loads(process.stdout))
            config.unlink()
            process = subprocess.run(command, env={**os.environ, "REMI_SSH_TARGET": TARGET}, capture_output=True, text=True, check=True)
            result = json.loads(process.stdout)
            self.assertEqual(result["outcome"], "withheld")
            self.assertNotIn("message", result)
            self.assertEqual(process.stderr, "")


if __name__ == "__main__":
    unittest.main()
