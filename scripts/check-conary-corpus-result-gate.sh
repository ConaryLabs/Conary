#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 <result.json> [<result.json> ...]" >&2
  exit 64
fi

for result in "$@"; do
  if [[ ! -f "$result" ]]; then
    echo "missing corpus result file: $result" >&2
    exit 1
  fi

  python3 - "$result" <<'PY'
import json
import pathlib
import re
import sys

path = pathlib.Path(sys.argv[1])
data = json.loads(path.read_text())
corpus = data.get("corpus") if isinstance(data, dict) else None
if not isinstance(corpus, dict) or corpus.get("schema_version") != 2:
    print(f"{path}: corpus gate failed: missing schema version 2 corpus object", file=sys.stderr)
    sys.exit(1)

aggregate = corpus.get("aggregate")
cases = corpus.get("cases")
expected_cases = corpus.get("expected_cases")
coverage = corpus.get("coverage")
if (
    not isinstance(expected_cases, int)
    or expected_cases <= 0
    or not isinstance(aggregate, dict)
    or not isinstance(cases, list)
    or not cases
    or not isinstance(coverage, dict)
):
    print(f"{path}: corpus gate failed: expected count, aggregate, coverage, and non-empty cases are required", file=sys.stderr)
    sys.exit(1)

required = coverage.get("required")
covered = coverage.get("covered")
missing = coverage.get("missing")
unattributed = coverage.get("unattributed_claims")
if (
    not isinstance(required, list)
    or not required
    or not isinstance(covered, list)
    or not isinstance(missing, list)
    or not isinstance(unattributed, int)
    or unattributed != 0
    or any(not isinstance(item, str) or not item for item in required + covered + missing)
    or len(set(required)) != len(required)
    or len(set(covered)) != len(covered)
    or len(set(missing)) != len(missing)
):
    print(f"{path}: corpus gate failed: typed coverage aggregate is malformed", file=sys.stderr)
    sys.exit(1)
if missing or set(required) != set(covered):
    print(f"{path}: corpus gate failed: required semantic coverage is incomplete", file=sys.stderr)
    sys.exit(1)

required_counts = ["cases", "completed", "failed", "skipped", "unclassified"]
for key in required_counts:
    if not isinstance(aggregate.get(key), int) or aggregate[key] < 0:
        print(f"{path}: corpus gate failed: aggregate.{key} must be a non-negative integer", file=sys.stderr)
        sys.exit(1)

if aggregate["cases"] != len(cases) or expected_cases != len(cases):
    print(f"{path}: corpus gate failed: declared, aggregate, and emitted case counts disagree", file=sys.stderr)
    sys.exit(1)
if (
    aggregate["completed"] != len(cases)
    or aggregate["failed"] != 0
    or aggregate["skipped"] != 0
    or aggregate["unclassified"] != 0
    or aggregate.get("stages") != []
):
    print(
        f"{path}: corpus gate failed: completed={aggregate['completed']} "
        f"failed={aggregate['failed']} skipped={aggregate['skipped']} "
        f"unclassified={aggregate['unclassified']}",
        file=sys.stderr,
    )
    sys.exit(1)

digest_pattern = re.compile(r"^[0-9a-f]{64}$")
case_ids = set()
claimed_semantics = set()
for case in cases:
    if not isinstance(case, dict):
        print(f"{path}: corpus gate failed: case must be an object", file=sys.stderr)
        sys.exit(1)
    case_id = case.get("case_id")
    if not isinstance(case_id, str) or not case_id or case_id in case_ids:
        print(f"{path}: corpus gate failed: case IDs must be non-empty and unique", file=sys.stderr)
        sys.exit(1)
    case_ids.add(case_id)

    source = case.get("source_profile")
    target = case.get("target_profile")
    artifacts = case.get("source_artifacts")
    stages = case.get("stage_results")
    claims = case.get("coverage_claims")
    if (
        not isinstance(source, dict)
        or not source.get("profile")
        or not source.get("format")
        or not isinstance(target, dict)
        or not target.get("profile")
        or not target.get("architecture")
        or not target.get("init_system")
        or not isinstance(target.get("capabilities"), list)
        or not target["capabilities"]
        or not isinstance(artifacts, list)
        or not artifacts
        or not isinstance(stages, list)
        or not stages
        or not isinstance(claims, list)
        or not claims
    ):
        print(f"{path}: corpus gate failed: {case_id} lacks attributable authority", file=sys.stderr)
        sys.exit(1)

    request_roles = set()
    artifact_identities = set()
    for artifact in artifacts:
        role = artifact.get("role") if isinstance(artifact, dict) else None
        if (
            role not in {
                "install_request",
                "install_dependency",
                "update_request",
                "update_dependency",
            }
            or artifact.get("digest_source") != "fixture_build_manifest"
            or not artifact.get("name")
            or not artifact.get("version")
            or not digest_pattern.fullmatch(str(artifact.get("digest", "")))
        ):
            print(f"{path}: corpus gate failed: {case_id} has invalid artifact authority", file=sys.stderr)
            sys.exit(1)
        identity = (
            role,
            artifact["name"],
            artifact["version"],
            artifact.get("architecture"),
            artifact["digest"],
        )
        if identity in artifact_identities:
            print(f"{path}: corpus gate failed: {case_id} repeats an exact artifact", file=sys.stderr)
            sys.exit(1)
        artifact_identities.add(identity)
        if role in {"install_request", "update_request"}:
            if role in request_roles:
                print(f"{path}: corpus gate failed: {case_id} repeats a requested artifact role", file=sys.stderr)
                sys.exit(1)
            request_roles.add(role)

    stage_names = {stage.get("stage") for stage in stages if isinstance(stage, dict)}
    if "installation" in stage_names and "install_request" not in request_roles:
        print(f"{path}: corpus gate failed: {case_id} lacks its install request", file=sys.stderr)
        sys.exit(1)
    if "update" in stage_names and "update_request" not in request_roles:
        print(f"{path}: corpus gate failed: {case_id} lacks its update request", file=sys.stderr)
        sys.exit(1)

    claim_keys = set()
    case_semantics = set()
    for claim in claims:
        semantic = claim.get("semantic") if isinstance(claim, dict) else None
        roles = claim.get("artifact_roles") if isinstance(claim, dict) else None
        if (
            not isinstance(semantic, str)
            or not semantic
            or not isinstance(roles, list)
            or not roles
            or len(set(roles)) != len(roles)
            or any(not isinstance(role, str) or not role for role in roles)
        ):
            print(f"{path}: corpus gate failed: {case_id} has malformed semantic coverage", file=sys.stderr)
            sys.exit(1)
        claim_key = (semantic, tuple(roles))
        if claim_key in claim_keys:
            print(f"{path}: corpus gate failed: {case_id} repeats a semantic coverage claim", file=sys.stderr)
            sys.exit(1)
        claim_keys.add(claim_key)
        if semantic in case_semantics:
            print(f"{path}: corpus gate failed: {case_id} repeats a semantic property", file=sys.stderr)
            sys.exit(1)
        case_semantics.add(semantic)
        for role in roles:
            if sum(1 for artifact in artifacts if artifact.get("role") == role) != 1:
                print(f"{path}: corpus gate failed: {case_id} has an unbound semantic coverage claim", file=sys.stderr)
                sys.exit(1)
        claimed_semantics.add(semantic)

    if any(stage.get("outcome") != "passed" for stage in stages if isinstance(stage, dict)) \
       or any(not isinstance(stage, dict) for stage in stages):
        print(f"{path}: corpus gate failed: {case_id} has a non-passing stage", file=sys.stderr)
        sys.exit(1)
    if case.get("final_outcome") != {"outcome": "completed"}:
        print(f"{path}: corpus gate failed: {case_id} did not complete", file=sys.stderr)
        sys.exit(1)

if claimed_semantics != set(required):
    print(f"{path}: corpus gate failed: case claims contradict the coverage aggregate", file=sys.stderr)
    sys.exit(1)

print(f"{path}: corpus ok ({len(cases)} cases)")
PY
done
