#!/usr/bin/env python3
"""Typed nightly recovery and whole-release retention; tags are never deleted."""

import argparse
from datetime import datetime, timedelta, timezone
from enum import Enum
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parent.parent


class Failure(Exception):
    def __init__(self, outcome, **details):
        self.record = {"schema_version": 1, "outcome": outcome, **details}
        super().__init__(json.dumps(self.record, sort_keys=True))


class State(str, Enum):
    NO_TAG = "no_tag"
    TAG_WITHOUT_RELEASE = "tag_without_release"
    DRAFT_RELEASE = "draft_release"
    PUBLISHED_WITHOUT_PROOF = "published_without_proof"
    PROVED = "proved"


def git(*args):
    return subprocess.check_output(["git", *args], text=True).strip()


def tag_metadata(tag):
    result = subprocess.run(
        ["bash", str(ROOT / "scripts/release-matrix.sh"), "resolve-tag", tag, "--format", "json"],
        text=True, capture_output=True,
    )
    if result.returncode:
        raise Failure("invalid_tag", tag_name=tag)
    return json.loads(result.stdout)


def discovered_tag_metadata(tag):
    try:
        return tag_metadata(tag)
    except Failure as error:
        if error.record["outcome"] != "invalid_tag":
            raise
        report({"schema_version": 1, "outcome": "ignored_malformed_tag", "tag_name": tag}, diagnostic=True)
        return {"channel": None}


class GitHub:
    def __init__(self):
        self.repo = os.environ["GITHUB_REPOSITORY"]
        if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", self.repo):
            raise Failure("invalid_repository")
        self.base = f"https://api.github.com/repos/{self.repo}/"

    def request(self, method, path):
        request = urllib.request.Request(self.base + path, method=method, headers={
            "Authorization": f"Bearer {os.environ['GH_TOKEN']}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2026-03-10",
        })
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                body = response.read()
                return response.status, json.loads(body) if body else None
        except urllib.error.HTTPError as error:
            return error.code, None
        except urllib.error.URLError:
            return "transport_error", None

    def get(self, path):
        status, body = self.request("GET", path)
        if status != 200:
            raise Failure("api_read_failed", api_status=status, path=path)
        return body

    def pages(self, path, key=None):
        separator = "&" if "?" in path else "?"
        page = 1
        while True:
            body = self.get(f"{path}{separator}per_page=100&page={page}")
            rows = body[key] if key else body
            yield from rows
            if len(rows) < 100:
                return
            page += 1


def release_for_tag(api, tag):
    status, release = api.request("GET", "releases/tags/" + urllib.parse.quote(tag, safe=""))
    if status == 404:
        # Authenticated listings include drafts, even when lookup by tag does not.
        matches = [row for row in api.pages("releases") if row.get("tag_name") == tag]
        if not matches:
            return None
        if len(matches) != 1:
            raise Failure("ambiguous_release", tag_name=tag)
        release = matches[0]
    elif status != 200:
        raise Failure("api_read_failed", tag_name=tag, api_status=status)
    if (release.get("tag_name") != tag or type(release.get("id")) is not int
            or type(release.get("draft")) is not bool):
        raise Failure("invalid_release", tag_name=tag)
    return release


def require_published_nightly(release):
    if (release.get("draft") is not False or release.get("prerelease") is not True
            or release.get("immutable") is not True or not release.get("published_at")):
        raise Failure("invalid_published_nightly", release_id=release["id"])


def proof_name(release_id, commit, attempt):
    return f"nightly-proof-v1-{release_id}-{commit}-{attempt}"


def has_proof(api, release, commit):
    # Receipts are emitted only after the aggregate native lifecycle gate.
    # A receipt must belong to a successful terminal job in a trusted main run.
    prefix = f"nightly-proof-v1-{release['id']}-{commit}-"
    workflows = {".github/workflows/nightly-release.yml", ".github/workflows/release-artifact-proof.yml"}
    for artifact in api.pages("actions/artifacts", "artifacts"):
        name = artifact["name"]
        if artifact.get("expired") is not False or not name.startswith(prefix):
            continue
        attempt = name[len(prefix):]
        if not re.fullmatch(r"[1-9][0-9]*", attempt):
            continue
        run = api.get(f"actions/runs/{artifact['workflow_run']['id']}")
        if (run.get("path") not in workflows or run.get("head_branch") != "main"
                or run.get("event") not in {"schedule", "workflow_dispatch"}
                or run.get("repository", {}).get("full_name") != api.repo):
            continue
        if subprocess.run(["git", "merge-base", "--is-ancestor", run["head_sha"], "origin/main"],
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode:
            continue
        jobs = api.pages(f"actions/runs/{run['id']}/attempts/{attempt}/jobs", "jobs")
        for job in jobs:
            if (job["name"].split(" / ")[-1] == "release-artifact-proof"
                    and job.get("status") == "completed" and job.get("conclusion") == "success"
                    and release["published_at"] <= job["started_at"]
                    <= artifact["created_at"] <= job["completed_at"]):
                return True
    return False


def resolve(api, candidate_tag, commit):
    if not re.fullmatch(r"[0-9a-f]{40}", commit) or tag_metadata(candidate_tag)["channel"] != "nightly":
        raise Failure("invalid_nightly_target")
    matches = []
    for tag in git("tag", "--list", "v*-nightly.*").splitlines():
        if discovered_tag_metadata(tag)["channel"] == "nightly" and git("rev-parse", f"refs/tags/{tag}^{{}}") == commit:
            if git("cat-file", "-t", f"refs/tags/{tag}") != "tag":
                raise Failure("unannotated_nightly_tag", tag_name=tag)
            matches.append(tag)
    if len(matches) > 1:
        raise Failure("ambiguous_nightly_tags", tag_names=matches, commit_sha=commit)
    tag = matches[0] if matches else candidate_tag
    release = None
    if not matches:
        if subprocess.run(["git", "show-ref", "--verify", "--quiet", f"refs/tags/{tag}"]).returncode == 0:
            raise Failure("nightly_tag_collision", tag_name=tag, commit_sha=commit)
        state = State.NO_TAG
    else:
        release = release_for_tag(api, tag)
        if release is None:
            state = State.TAG_WITHOUT_RELEASE
        elif release["draft"]:
            state = State.DRAFT_RELEASE
        else:
            require_published_nightly(release)
            state = State.PROVED if has_proof(api, release, commit) else State.PUBLISHED_WITHOUT_PROOF
    outcome = {State.PROVED: "skipped", State.PUBLISHED_WITHOUT_PROOF: "proof"}.get(state, "build")
    return {"schema_version": 1, "state": state.value, "outcome": outcome,
            "tag_name": tag, "commit_sha": commit, "release_id": release["id"] if release else None}


def select_candidate(api, now):
    date = now.astimezone(timezone.utc).strftime("%Y%m%d")
    matches = []
    for tag in git("tag", "--list", "v*-nightly.*").splitlines():
        metadata = discovered_tag_metadata(tag)
        if metadata["channel"] != "nightly" or metadata["version"].split("-nightly.")[1] != date:
            continue
        if git("cat-file", "-t", f"refs/tags/{tag}") != "tag":
            raise Failure("unannotated_nightly_tag", tag_name=tag)
        matches.append(tag)
    if len(matches) > 1:
        raise Failure("ambiguous_nightly_date", date=date, tag_names=matches)
    if matches:
        tag = matches[0]
        commit = git("rev-parse", f"refs/tags/{tag}^{{}}")
        outcome = "selected_by_existing_date_tag"
    else:
        runs = api.get("actions/workflows/merge-validation.yml/runs?branch=main&status=success&per_page=100")
        rows = runs.get("workflow_runs", [])
        if not rows:
            raise Failure("green_commit_missing", date=date)
        commit = rows[0].get("head_sha", "")
        tag = None
        outcome = "selected_by_green_run"
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise Failure("invalid_selected_commit")
    return {"schema_version": 1, "outcome": outcome, "commit_sha": commit,
            "tag_name": tag, "date": date}


def preflight(commit, workflow_commit, now):
    if not all(re.fullmatch(r"[0-9a-f]{40}", value) for value in (commit, workflow_commit)):
        raise Failure("invalid_preflight_commit")
    if git("rev-parse", "HEAD") != commit:
        raise Failure("preflight_checkout_mismatch", commit_sha=commit)

    def unsupported(reason, status=None):
        return {"schema_version": 1, "state": "unsupported_commit", "outcome": "skipped",
                "commit_sha": commit, "workflow_commit": workflow_commit,
                "reason": reason, "command_status": status}

    def run(*command):
        return subprocess.run(list(command), text=True, capture_output=True)

    ancestry = run("git", "merge-base", "--is-ancestor", workflow_commit, commit)
    if ancestry.returncode == 1:
        return unsupported("workflow_not_ancestor", ancestry.returncode)
    if ancestry.returncode:
        raise Failure("preflight_ancestry_failed", command_status=ancestry.returncode, commit_sha=commit)
    # Relative paths deliberately execute the selected checkout, not ROOT's workflow authority.
    dry_run = run("bash", "scripts/release.sh", "suite", "--dry-run")
    if dry_run.returncode:
        return unsupported("release_dry_run_failed", dry_run.returncode)
    versions = [line.removeprefix("  Next version: ") for line in dry_run.stdout.splitlines()
                if line.startswith("  Next version: ")]
    if len(versions) != 1:
        return unsupported("missing_next_version")
    stable = versions[0]
    version = f"{stable}-nightly.{now.astimezone(timezone.utc):%Y%m%d}"
    checks = [
        ("stable_grammar", ["bash", "scripts/release-matrix.sh", "validate-version", stable, "stable"]),
        ("nightly_grammar", ["bash", "scripts/release-matrix.sh", "validate-version", version, "nightly"]),
        ("nightly_release_target", ["bash", "scripts/release.sh", "suite", "--dry-run", "--target", version]),
    ]
    checks.extend((f"render_{target}", ["bash", "scripts/release-matrix.sh", "render-version", version, target])
                  for target in ("cargo", "rpm", "deb", "arch", "ccs", "tag"))
    for reason, command in checks:
        result = run(*command)
        if result.returncode:
            return unsupported(reason, result.returncode)
    return {"schema_version": 1, "state": "supported_commit", "outcome": "supported",
            "commit_sha": commit, "workflow_commit": workflow_commit,
            "version": version, "tag_name": f"v{version}"}


def notes_boundary(current_tag):
    if tag_metadata(current_tag)["channel"] != "nightly":
        raise Failure("invalid_nightly_target")
    candidates = {"nightly": [], "stable": []}
    for row in git("for-each-ref", "--format=%(refname:short)%09%(creatordate:unix)", "refs/tags/v*").splitlines():
        tag, timestamp = row.split("\t")
        if tag == current_tag:
            continue
        metadata = discovered_tag_metadata(tag)
        channel = metadata["channel"]
        if channel not in candidates:
            continue
        # Grammar has already established the numeric version and real date.
        version = tuple(int(part) for part in metadata["stable_version"].split("."))
        date = int(metadata["version"].split("-nightly.")[1]) if channel == "nightly" else 0
        candidates[channel].append((int(timestamp), version, date, tag))
    channel = "nightly" if candidates["nightly"] else "stable"
    if not candidates[channel]:
        raise Failure("notes_boundary_missing", tag_name=current_tag)
    return {"schema_version": 1, "outcome": "notes_boundary_selected",
            "boundary_channel": channel, "fallback_to_stable": channel == "stable",
            "previous_tag_name": max(candidates[channel])[-1], "tag_name": current_tag}


def retain(api, now):
    cutoff = now - timedelta(days=14)
    # Snapshot before deleting: mutation would otherwise shift subsequent pages.
    releases = list(api.pages("releases"))
    for release in releases:
        tag = release.get("tag_name", "")
        # Discovery narrows candidates; matrix grammar establishes authority.
        if "-nightly." not in tag:
            continue
        if discovered_tag_metadata(tag)["channel"] != "nightly":
            continue
        if release.get("draft") is not False or release.get("prerelease") is not True:
            continue
        published = datetime.fromisoformat(release["published_at"].replace("Z", "+00:00"))
        if published >= cutoff:
            continue
        release_id = release["id"]
        if type(release_id) is not int or release_id <= 0:
            raise Failure("invalid_release_id")
        # Whole immutable releases may be deleted. Never delete individual assets or tags.
        status, _ = api.request("DELETE", f"releases/{release_id}")
        if status != 204:
            raise Failure("retention_delete_failed", release_id=release_id, api_status=status)
        report({"schema_version": 1, "outcome": "release_deleted", "release_id": release_id, "api_status": status})


def report(record, diagnostic=False):
    rendered = json.dumps(record, sort_keys=True)
    print(rendered, file=sys.stderr if diagnostic else sys.stdout)
    if path := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(path, "a") as stream:
            stream.write(f"\n```json\n{rendered}\n```\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    selection = sub.add_parser("resolve")
    selection.add_argument("--tag", required=True)
    selection.add_argument("--commit", required=True)
    sub.add_parser("retain")
    sub.add_parser("select")
    capability = sub.add_parser("preflight")
    capability.add_argument("--commit", required=True)
    capability.add_argument("--workflow-commit", required=True)
    capability.add_argument("--date", required=True)
    notes = sub.add_parser("notes-boundary")
    notes.add_argument("--tag", required=True)
    receipt = sub.add_parser("receipt")
    receipt.add_argument("--tag", required=True)
    receipt.add_argument("--output", required=True)
    args = parser.parse_args()
    if args.command == "preflight":
        try:
            date = datetime.strptime(args.date, "%Y%m%d").replace(tzinfo=timezone.utc)
            if date.strftime("%Y%m%d") != args.date:
                raise ValueError
        except ValueError:
            raise Failure("invalid_nightly_date", date=args.date)
        report(preflight(args.commit, args.workflow_commit, date))
        return
    if args.command == "notes-boundary":
        report(notes_boundary(args.tag))
        return
    api = GitHub()
    if args.command == "select":
        report(select_candidate(api, datetime.now(timezone.utc)))
    elif args.command == "resolve":
        report(resolve(api, args.tag, args.commit))
    elif args.command == "retain":
        retain(api, datetime.now(timezone.utc))
    elif tag_metadata(args.tag)["channel"] == "nightly":
        release = release_for_tag(api, args.tag)
        if release is None:
            raise Failure("proof_release_missing", tag_name=args.tag)
        require_published_nightly(release)
        commit = git("rev-parse", f"refs/tags/{args.tag}^{{}}")
        record = {"schema_version": 1, "release_id": release["id"], "tag_name": args.tag,
                  "commit_sha": commit, "run_id": int(os.environ["GITHUB_RUN_ID"]),
                  "run_attempt": int(os.environ["GITHUB_RUN_ATTEMPT"])}
        Path(args.output).write_text(json.dumps(record) + "\n")
        with open(os.environ["GITHUB_OUTPUT"], "a") as stream:
            stream.write(f"name={proof_name(release['id'], commit, record['run_attempt'])}\n")


if __name__ == "__main__":
    try:
        main()
    except Failure as error:
        report(error.record)
        raise SystemExit(1)
