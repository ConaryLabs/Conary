#!/usr/bin/env python3
"""Table-driven recovery, proof-receipt, and retention conformance fixtures."""

import copy
from contextlib import redirect_stdout, redirect_stderr
from datetime import datetime, timezone
import importlib.util
import io
import json
import os
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("nightly", Path(__file__).with_name("nightly-release.py"))
nightly = importlib.util.module_from_spec(spec)
spec.loader.exec_module(nightly)
TAG = "v0.17.0-nightly.20260905"
COMMIT = "a" * 40
RELEASE = {"id": 42, "tag_name": TAG, "draft": False, "prerelease": True,
           "immutable": True, "published_at": "2026-09-05T06:40:00Z"}


class API:
    repo = "FieldmouseWorks/Conary"

    def __init__(self, release=None, status=200, rows=None, objects=None):
        self.release, self.status = release, status
        self.rows, self.objects = rows or {}, objects or {}
        self.calls = []

    def request(self, method, path):
        self.calls.append((method, path))
        return self.status, self.release

    def pages(self, path, key=None):
        return iter(self.rows.get(path, []))

    def get(self, path):
        return self.objects[path]


class NightlyTests(unittest.TestCase):
    def test_existing_date_tag_precedes_newer_green_commit(self):
        newer = "b" * 40
        malformed = "v0.17.0-nightly.20260905.preview"
        endpoint = "actions/workflows/merge-validation.yml/runs?branch=main&status=success&per_page=100"
        cases = (
            ([TAG], COMMIT, "selected_by_existing_date_tag"),
            ([malformed, TAG], COMMIT, "selected_by_existing_date_tag"),
            ([malformed], newer, "selected_by_green_run"),
            (["v0.17.0-nightly.20260904"], newer, "selected_by_green_run"),
            ([], newer, "selected_by_green_run"),
        )
        for tags, expected, outcome in cases:
            with self.subTest(tags=tags), tempfile.TemporaryDirectory() as directory:
                def git(*args):
                    if args[0] == "tag":
                        return "\n".join(tags)
                    self.assertNotIn(malformed, " ".join(args))
                    return "tag" if args[0] == "cat-file" else COMMIT

                api = API(objects={endpoint: {"workflow_runs": [{"head_sha": newer}]}})
                summary = Path(directory) / "summary"
                with patch.object(nightly, "git", side_effect=git), \
                        patch.object(api, "get", wraps=api.get) as get, \
                        patch.dict(os.environ, GITHUB_STEP_SUMMARY=str(summary)), \
                        redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                    result = nightly.select_candidate(api, datetime(2026, 9, 5, tzinfo=timezone.utc))
                    nightly.report(result)
                    self.assertEqual(result["commit_sha"], expected)
                    self.assertEqual(result["outcome"], outcome)
                    self.assertIn(expected, summary.read_text())
                    if TAG in tags:
                        get.assert_not_called()
                        self.assertIn("selected_by_existing_date_tag", summary.read_text())
                        # Existing date selection feeds every recovery state at A, never B.
                        for release, status, proved, state in (
                            (None, 404, False, "tag_without_release"),
                            ({**RELEASE, "draft": True}, 200, False, "draft_release"),
                            (RELEASE, 200, False, "published_without_proof"),
                            (RELEASE, 200, True, "proved"),
                        ):
                            with patch.object(nightly, "has_proof", return_value=proved):
                                recovery = nightly.resolve(API(release, status), result["tag_name"], result["commit_sha"])
                            self.assertEqual(recovery["state"], state)
                            self.assertEqual(recovery["commit_sha"], COMMIT)
                    else:
                        get.assert_called_once_with(endpoint)
                    if malformed in tags:
                        self.assertIn("ignored_malformed_tag", summary.read_text())

    def test_two_valid_tags_for_today_fail_without_selecting_green(self):
        api = API()
        def git(*args):
            return TAG + "\nv0.18.0-nightly.20260905" if args[0] == "tag" else "tag"
        with patch.object(nightly, "git", side_effect=git), self.assertRaises(nightly.Failure) as error:
            nightly.select_candidate(api, datetime(2026, 9, 5, tzinfo=timezone.utc))
        self.assertEqual(error.exception.record["outcome"], "ambiguous_nightly_date")

    def test_selected_commit_capability_table(self):
        workflow = "b" * 40
        version = "0.17.0-nightly.20260905"
        commands = [
            ["git", "merge-base", "--is-ancestor", workflow, COMMIT],
            ["bash", "scripts/release.sh", "suite", "--dry-run"],
            ["bash", "scripts/release-matrix.sh", "validate-version", "0.17.0", "stable"],
            ["bash", "scripts/release-matrix.sh", "validate-version", version, "nightly"],
            ["bash", "scripts/release.sh", "suite", "--dry-run", "--target", version],
            *[["bash", "scripts/release-matrix.sh", "render-version", version, target]
              for target in ("cargo", "rpm", "deb", "arch", "ccs", "tag")],
        ]
        cases = [(None, None), (0, "workflow_not_ancestor"), (1, "release_dry_run_failed"),
                 (2, "stable_grammar"), (3, "nightly_grammar"), (4, "nightly_release_target"),
                 *[(index + 5, f"render_{target}")
                   for index, target in enumerate(("cargo", "rpm", "deb", "arch", "ccs", "tag"))]]
        for failed_index, reason in cases:
            with self.subTest(reason=reason), tempfile.TemporaryDirectory() as directory:
                calls = []

                def run(command, **kwargs):
                    index = len(calls)
                    self.assertEqual(command, commands[index])
                    calls.append(command)
                    status = (1 if index == 0 else 2) if index == failed_index else 0
                    return SimpleNamespace(returncode=status, stdout="  Next version: 0.17.0\n")

                summary = Path(directory) / "summary"
                stdout = io.StringIO()
                with patch.object(nightly, "git", return_value=COMMIT), \
                        patch.object(nightly.subprocess, "run", side_effect=run), \
                        patch.dict(os.environ, GITHUB_STEP_SUMMARY=str(summary)), redirect_stdout(stdout):
                    result = nightly.preflight(COMMIT, workflow, datetime(2026, 9, 5, tzinfo=timezone.utc))
                    nightly.report(result)
                self.assertEqual(json.loads(stdout.getvalue()), result)
                self.assertIn(COMMIT, summary.read_text())
                if reason:
                    self.assertEqual(result["state"], "unsupported_commit")
                    self.assertEqual(result["outcome"], "skipped")
                    self.assertEqual(result["reason"], reason)
                    self.assertEqual(len(calls), failed_index + 1)
                else:
                    self.assertEqual(result["state"], "supported_commit")
                    self.assertEqual(result["tag_name"], "v" + version)
                    self.assertEqual(calls, commands)

    def test_preflight_operational_errors_are_not_unsupported_content(self):
        with patch.object(nightly, "git", return_value="c" * 40), \
                self.assertRaises(nightly.Failure) as error:
            nightly.preflight(COMMIT, "b" * 40, datetime.now(timezone.utc))
        self.assertEqual(error.exception.record["outcome"], "preflight_checkout_mismatch")
        with patch.object(nightly, "git", return_value=COMMIT), \
                patch.object(nightly.subprocess, "run", return_value=SimpleNamespace(returncode=128)), \
                self.assertRaises(nightly.Failure) as error:
            nightly.preflight(COMMIT, "b" * 40, datetime.now(timezone.utc))
        self.assertEqual(error.exception.record["outcome"], "preflight_ancestry_failed")

    def test_preflight_cli_skips_without_api_or_failure_exit(self):
        with patch.object(sys, "argv", ["nightly-release.py", "preflight", "--commit", COMMIT,
                                       "--workflow-commit", "b" * 40, "--date", "20260905"]), \
                patch.object(nightly, "git", return_value=COMMIT), \
                patch.object(nightly.subprocess, "run", return_value=SimpleNamespace(returncode=1)), \
                patch.object(nightly, "GitHub", side_effect=AssertionError("preflight must not access GitHub")), \
                redirect_stdout(io.StringIO()) as stdout:
            self.assertIsNone(nightly.main())
        self.assertEqual(json.loads(stdout.getvalue())["outcome"], "skipped")

    def test_notes_boundary_table(self):
        malformed = "v0.17.0-nightly.preview\t999999"
        earlier = "v0.17.0-nightly.20260903\t100"
        latest = "v0.17.0-nightly.20260904\t200"
        stable = "v0.16.1\t50"
        cases = (
            ([malformed, latest, earlier, stable], "v0.17.0-nightly.20260904", "nightly"),
            ([earlier, latest, malformed, stable], "v0.17.0-nightly.20260904", "nightly"),
            (["v0.9.0-nightly.20260904\t200", "v0.10.0-nightly.20260904\t200"],
             "v0.10.0-nightly.20260904", "nightly"),
            (["v0.17.0-nightly.20260903\t200", latest], "v0.17.0-nightly.20260904", "nightly"),
            ([malformed, "v0.16.0\t40", stable], "v0.16.1", "stable"),
        )
        for rows, expected, channel in cases:
            with self.subTest(rows=rows), tempfile.TemporaryDirectory() as directory:
                summary = Path(directory) / "summary"
                stdout, stderr = io.StringIO(), io.StringIO()
                with patch.object(nightly, "git", return_value="\n".join([TAG + "\t1000000", *rows])), \
                        patch.dict(os.environ, GITHUB_STEP_SUMMARY=str(summary)), \
                        redirect_stdout(stdout), redirect_stderr(stderr):
                    nightly.report(nightly.notes_boundary(TAG))
                result = json.loads(stdout.getvalue())
                self.assertEqual(result["previous_tag_name"], expected)
                self.assertEqual(result["boundary_channel"], channel)
                self.assertEqual(result["fallback_to_stable"], channel == "stable")
                self.assertIn(json.dumps(result, sort_keys=True), summary.read_text())
                if malformed in rows:
                    self.assertIn("ignored_malformed_tag", summary.read_text())
        with patch.object(nightly, "git", return_value=malformed), \
                redirect_stderr(io.StringIO()), self.assertRaises(nightly.Failure) as error:
            nightly.notes_boundary(TAG)
        self.assertEqual(error.exception.record["outcome"], "notes_boundary_missing")

    def test_malformed_discovery_is_non_authority(self):
        malformed = "v0.17.0-nightly.preview"

        def git(*args):
            if args[0] == "tag":
                return malformed + "\n" + TAG
            self.assertNotIn(malformed, " ".join(args))
            return "tag" if args[0] == "cat-file" else COMMIT

        with tempfile.TemporaryDirectory() as directory:
            summary = Path(directory) / "summary"
            stdout, stderr = io.StringIO(), io.StringIO()
            with patch.object(nightly, "git", side_effect=git), \
                    patch.dict(os.environ, GITHUB_STEP_SUMMARY=str(summary)), \
                    redirect_stdout(stdout), redirect_stderr(stderr):
                result = nightly.resolve(API(status=404), TAG, COMMIT)
                nightly.report(result)
            self.assertEqual(json.loads(stdout.getvalue()), result)
            self.assertEqual(result["state"], "tag_without_release")
            self.assertEqual(result["tag_name"], TAG)
            ignored = json.loads(stderr.getvalue())
            self.assertEqual(ignored["outcome"], "ignored_malformed_tag")
            self.assertEqual(ignored["tag_name"], malformed)
            self.assertIn("ignored_malformed_tag", summary.read_text())
        # Explicit targets remain strict; only historical discovery may skip.
        with self.assertRaises(nightly.Failure):
            nightly.resolve(API(status=404), malformed, COMMIT)

    def test_malformed_release_is_not_retention_authority(self):
        api = API(status=204, rows={"releases": [
            {**RELEASE, "tag_name": "v0.17.0-nightly.preview"}, RELEASE,
        ]})
        with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()) as diagnostics:
            nightly.retain(api, datetime(2026, 9, 20, tzinfo=timezone.utc))
        self.assertEqual(api.calls, [("DELETE", "releases/42")])
        self.assertEqual(json.loads(diagnostics.getvalue())["outcome"], "ignored_malformed_tag")

    def test_recovery_state_table(self):
        cases = (
            (False, None, 404, False, "no_tag", "build"),
            (True, None, 404, False, "tag_without_release", "build"),
            (True, {**RELEASE, "draft": True, "immutable": False}, 200, False, "draft_release", "build"),
            (True, RELEASE, 200, False, "published_without_proof", "proof"),
            (True, RELEASE, 200, True, "proved", "skipped"),
        )
        for exists, release, status, proved, state, outcome in cases:
            with self.subTest(state=state):
                def git(*args):
                    if args[0] == "tag":
                        return TAG if exists else ""
                    return "tag" if args[0] == "cat-file" else COMMIT
                with patch.object(nightly, "git", side_effect=git), \
                        patch.object(nightly, "tag_metadata", return_value={"channel": "nightly"}), \
                        patch.object(nightly, "has_proof", return_value=proved), \
                        patch.object(nightly.subprocess, "run", return_value=SimpleNamespace(returncode=1)):
                    result = nightly.resolve(API(release, status), "v0.17.0-nightly.20260906", COMMIT)
                self.assertEqual((result["state"], result["outcome"]), (state, outcome))
                self.assertEqual(result["tag_name"], TAG if exists else "v0.17.0-nightly.20260906")

    def test_release_api_failures_are_not_missing_releases(self):
        for status in (401, 403, 429, 500, "transport_error"):
            with self.subTest(status=status), self.assertRaises(nightly.Failure) as caught:
                nightly.release_for_tag(API(status=status), TAG)
            self.assertEqual(caught.exception.record["api_status"], status)

    def test_draft_is_found_in_authenticated_listing(self):
        draft = {**RELEASE, "draft": True, "immutable": False}
        self.assertEqual(nightly.release_for_tag(API(status=404, rows={"releases": [draft]}), TAG), draft)

    def test_published_release_identity(self):
        for field, value in (("draft", True), ("immutable", False), ("prerelease", False)):
            with self.subTest(field=field), self.assertRaises(nightly.Failure):
                nightly.require_published_nightly({**RELEASE, field: value})

    def test_proof_receipt_authority_table(self):
        artifact = {"name": nightly.proof_name(42, COMMIT, 2), "expired": False,
                    "workflow_run": {"id": 7}, "created_at": "2026-09-05T08:00:00Z"}
        run = {"id": 7, "path": ".github/workflows/nightly-release.yml", "head_branch": "main",
               "event": "schedule", "head_sha": "b" * 40, "repository": {"full_name": API.repo}}
        job = {"name": "build-and-publish / prove-nightly-release / release-artifact-proof",
               "status": "completed", "conclusion": "success", "started_at": "2026-09-05T07:59:00Z",
               "completed_at": "2026-09-05T08:01:00Z"}
        cases = (
            ("valid", None, None, None, True),
            ("wrong release", "artifact", "name", nightly.proof_name(41, COMMIT, 2), False),
            ("wrong commit", "artifact", "name", nightly.proof_name(42, "c" * 40, 2), False),
            ("expired", "artifact", "expired", True, False),
            ("incomplete", "job", "status", "in_progress", False),
            ("failed", "job", "conclusion", "failure", False),
            ("wrong job", "job", "name", "workspace-validation", False),
            ("old proof", "job", "started_at", "2026-09-04T00:00:00Z", False),
            ("other attempt receipt", "artifact", "created_at", "2026-09-06T00:00:00Z", False),
            ("untrusted branch", "run", "head_branch", "feature", False),
            ("untrusted event", "run", "event", "pull_request", False),
            ("untrusted workflow", "run", "path", ".github/workflows/pr-gate.yml", False),
        )
        for label, kind, field, value, expected in cases:
            with self.subTest(case=label):
                data = copy.deepcopy({"artifact": artifact, "run": run, "job": job})
                if kind:
                    data[kind][field] = value
                api = API(rows={"actions/artifacts": [data["artifact"]],
                                "actions/runs/7/attempts/2/jobs": [data["job"]]},
                          objects={"actions/runs/7": data["run"]})
                with patch.object(nightly.subprocess, "run", return_value=SimpleNamespace(returncode=0)):
                    self.assertEqual(nightly.has_proof(api, RELEASE, COMMIT), expected)

    def test_retention_boundary_and_immutable_whole_release(self):
        now = datetime(2026, 9, 20, tzinfo=timezone.utc)
        rows = [RELEASE, {**RELEASE, "id": 43, "immutable": False},
                {**RELEASE, "id": 44, "published_at": "2026-09-06T00:00:00Z"},
                {**RELEASE, "id": 45, "draft": True}, {**RELEASE, "id": 46, "prerelease": False},
                {**RELEASE, "id": 47, "tag_name": "v0.17.0"}]
        api = API(status=204, rows={"releases": rows})
        with patch.object(nightly, "report"):
            nightly.retain(api, now)
        self.assertEqual(api.calls, [("DELETE", "releases/42"), ("DELETE", "releases/43")])

    def test_retention_rejected_delete_is_typed(self):
        for status in (403, 404, 422, 500, "transport_error"):
            with self.subTest(status=status), self.assertRaises(nightly.Failure) as caught:
                nightly.retain(API(status=status, rows={"releases": [RELEASE]}),
                               datetime(2026, 9, 20, tzinfo=timezone.utc))
            self.assertEqual(caught.exception.record, {"schema_version": 1,
                             "outcome": "retention_delete_failed", "release_id": 42, "api_status": status})

    def test_retention_snapshots_all_pages_before_deletion(self):
        api = API(status=204)
        def pages(path):
            yield RELEASE
            self.assertEqual(api.calls, [], "deletion must not shift unread API pages")
            yield {**RELEASE, "id": 43}
        api.pages = pages
        with patch.object(nightly, "report"):
            nightly.retain(api, datetime(2026, 9, 20, tzinfo=timezone.utc))
        self.assertEqual(api.calls, [("DELETE", "releases/42"), ("DELETE", "releases/43")])


if __name__ == "__main__":
    unittest.main()
