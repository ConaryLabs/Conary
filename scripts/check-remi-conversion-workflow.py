#!/usr/bin/env python3
# scripts/check-remi-conversion-workflow.py

"""Structurally validate the protected Remi conversion workflow authority."""

from __future__ import annotations

import hashlib
from pathlib import Path
import re
import sys
from typing import Any, NoReturn

try:
    import yaml
except ModuleNotFoundError as error:
    print(
        "ERROR: python3-yaml is required for structural GitHub workflow policy",
        file=sys.stderr,
    )
    raise SystemExit(2) from error


class PolicyError(ValueError):
    """An effective workflow differs from the reviewed production authority."""


class GitHubWorkflowLoader(yaml.SafeLoader):
    """Safe YAML loader with GitHub's YAML 1.2 boolean behavior."""


# PyYAML defaults to YAML 1.1, where the GitHub key `on` becomes boolean true.
# Copy and narrow the boolean resolver without mutating the process-global loader.
GitHubWorkflowLoader.yaml_implicit_resolvers = {
    key: [
        (tag, pattern)
        for tag, pattern in resolvers
        if tag != "tag:yaml.org,2002:bool"
    ]
    for key, resolvers in yaml.SafeLoader.yaml_implicit_resolvers.items()
}
GitHubWorkflowLoader.add_implicit_resolver(
    "tag:yaml.org,2002:bool",
    re.compile(r"^(?:true|True|TRUE|false|False|FALSE)$"),
    list("tTfF"),
)


def construct_unique_mapping(
    loader: GitHubWorkflowLoader,
    node: yaml.MappingNode,
    deep: bool = False,
) -> dict[Any, Any]:
    """Reject duplicate and merge keys instead of accepting shadow authority."""

    mapping: dict[Any, Any] = {}
    for key_node, value_node in node.value:
        if key_node.tag == "tag:yaml.org,2002:merge":
            raise yaml.constructor.ConstructorError(
                "while constructing a mapping",
                node.start_mark,
                "YAML merge keys are forbidden",
                key_node.start_mark,
            )
        key = loader.construct_object(key_node, deep=deep)
        try:
            duplicate = key in mapping
        except TypeError as error:
            raise yaml.constructor.ConstructorError(
                "while constructing a mapping",
                node.start_mark,
                "workflow mapping key is not scalar",
                key_node.start_mark,
            ) from error
        if duplicate:
            raise yaml.constructor.ConstructorError(
                "while constructing a mapping",
                node.start_mark,
                f"duplicate key {key!r}",
                key_node.start_mark,
            )
        mapping[key] = loader.construct_object(value_node, deep=deep)
    return mapping


GitHubWorkflowLoader.add_constructor(
    yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG,
    construct_unique_mapping,
)


BENCHMARK_WORKFLOW = ".github/workflows/remi-conversion-benchmark.yml"
SHARED_CONCURRENCY = (
    (
        ".github/workflows/deploy-and-verify.yml",
        "release deployment serialized with production host authority",
    ),
    (
        ".github/workflows/deploy-remi-candidate.yml",
        "candidate deployment serialized with production host authority",
    ),
    (
        ".github/workflows/deploy-site.yml",
        "site deployment serialized with production host authority",
    ),
    (
        ".github/workflows/export-remi-native-oracle-inputs.yml",
        "native-oracle export serialized with candidate and release deployment",
    ),
    (
        BENCHMARK_WORKFLOW,
        "conversion benchmark serialized with deployment and verification",
    ),
    (
        ".github/workflows/remi-r2-durability.yml",
        "R2 durability serialized with production host authority",
    ),
)

EXPECTED_INPUTS = {
    "deployment_run_id": ("string", None),
    "profile": ("choice", ["fedora-44", "ubuntu-26.04", "arch"]),
    "profile_revision_sha256": ("string", None),
    "package_key_sha256": ("string", None),
    "source_url": ("string", None),
    "source_sha256": ("string", None),
    "source_size_bytes": ("string", None),
}

EXPECTED_STEP_ORDER = [
    "Check out exact protected benchmark operator",
    "Require a protected-main workflow revision",
    "Bind successful exact private-candidate deployment",
    "Download exact sanitized deployment evidence",
    "Reopen exact deployment and candidate identities",
    "Acquire and authenticate exact source bytes",
    "Run through the fixed production helper",
    "Upload public-sanitized benchmark failure evidence",
    "Upload public-sanitized benchmark evidence",
    "Record exact production benchmark evidence",
    "Fail closed on unsuccessful benchmark result",
]

# These are SHA-256 digests of the effective YAML `run` scalar bytes, after safe
# parsing. Any shell edit, including a commented or echoed decoy for removed
# authority, requires an explicit reviewed update to the corresponding digest.
REVIEWED_RUN_BLOCK_SHA256 = {
    "Require a protected-main workflow revision":
        "ce2b2a65a3e517545403f42bd329b0dfd091c998aebd96d6569c727edd26bf2a",
    "Bind successful exact private-candidate deployment":
        "b0c12ead3f575381a53dc16f01f7993859c32dbc94f59d056464b4c2bdd4eec4",
    "Reopen exact deployment and candidate identities":
        "1ed0a56346d18f5b187856b3404beed11c095d7e62bb652b03d16a2caa50c0a2",
    "Acquire and authenticate exact source bytes":
        "4e8c8e91fb4b79803aed47afc34c1e51034f7e9ccc45a23fd213df924c6d0856",
    "Run through the fixed production helper":
        "b668d296d97a1708bb7ceb1a50a0b35b998ee9546b6be1a7f7f3731de3339926",
    "Record exact production benchmark evidence":
        "bb5378af4528b9f711cead85b40e9831d0df6a43a30e5859fd792a333a1aa700",
    "Fail closed on unsuccessful benchmark result":
        "9097f565c79005bd7cb2b3d928f284c1606c6d28203e48d85222006af4239b15",
}

RUN_BLOCK_ERRORS = {
    "Require a protected-main workflow revision":
        "conversion benchmark protected merged-main operator boundary",
    "Bind successful exact private-candidate deployment":
        "conversion benchmark exact successful protected deployment source",
    "Reopen exact deployment and candidate identities":
        "conversion benchmark deployment inspection and retained revision run authority",
    "Acquire and authenticate exact source bytes":
        "conversion benchmark credential-free bounded exact source acquisition",
    "Run through the fixed production helper":
        "conversion benchmark reviewed helper, pinned-host, transport, and public-proof run authority",
    "Record exact production benchmark evidence":
        "conversion benchmark public-only summary authority",
    "Fail closed on unsuccessful benchmark result":
        "conversion benchmark typed terminal failure result",
}

EXPECTED_RUN_ENV = {
    "Require a protected-main workflow revision": {
        "WORKFLOW_SHA": "${{ github.workflow_sha }}",
    },
    "Bind successful exact private-candidate deployment": {
        "DEPLOYMENT_RUN_ID": "${{ inputs.deployment_run_id }}",
        "GH_TOKEN": "${{ github.token }}",
    },
    "Reopen exact deployment and candidate identities": {
        "DEPLOYED_COMMIT_SHA": "${{ steps.deployment.outputs.head_sha }}",
        "INSPECTION_DIR": "${{ runner.temp }}/deployment-inspection",
        "PROFILE": "${{ inputs.profile }}",
        "REQUESTED_REVISION": "${{ inputs.profile_revision_sha256 }}",
    },
    "Acquire and authenticate exact source bytes": {
        "DEPLOYED_COMMIT_SHA": "${{ steps.deployment.outputs.head_sha }}",
        "DEPLOYMENT_RUN_ID": "${{ inputs.deployment_run_id }}",
        "PACKAGE_KEY_SHA256": "${{ inputs.package_key_sha256 }}",
        "PROFILE": "${{ inputs.profile }}",
        "REVISION": "${{ steps.authority.outputs.revision }}",
        "SOURCE_SHA256": "${{ inputs.source_sha256 }}",
        "SOURCE_SIZE_BYTES": "${{ inputs.source_size_bytes }}",
        "SOURCE_URL": "${{ inputs.source_url }}",
    },
    "Run through the fixed production helper": {
        "BINARY_SHA256": "${{ steps.authority.outputs.binary_sha256 }}",
        "DEPLOYED_COMMIT_SHA": "${{ steps.deployment.outputs.head_sha }}",
        "DEPLOYMENT_RUN_ID": "${{ inputs.deployment_run_id }}",
        "PACKAGE_KEY_SHA256": "${{ inputs.package_key_sha256 }}",
        "PROFILE": "${{ inputs.profile }}",
        "REMI_SSH_KNOWN_HOSTS": "${{ secrets.REMI_SSH_KNOWN_HOSTS }}",
        "REMI_SSH_KEY": "${{ secrets.REMI_SSH_KEY }}",
        "REMI_SSH_TARGET": "${{ secrets.REMI_SSH_TARGET }}",
        "REMI_VERSION": "${{ steps.authority.outputs.version }}",
        "REVISION": "${{ steps.authority.outputs.revision }}",
        "SOURCE_SHA256": "${{ inputs.source_sha256 }}",
        "SOURCE_SIZE_BYTES": "${{ inputs.source_size_bytes }}",
        "WORKFLOW_RUN_ATTEMPT": "${{ github.run_attempt }}",
        "WORKFLOW_RUN_ID": "${{ github.run_id }}",
        "WORKFLOW_SHA": "${{ github.workflow_sha }}",
    },
    "Record exact production benchmark evidence": {
        "ARTIFACT_DIGEST": "${{ steps.upload.outputs.artifact-digest }}",
        "ARTIFACT_ID": "${{ steps.upload.outputs.artifact-id }}",
        "BENCHMARK_ID": "${{ steps.benchmark.outputs.benchmark_id }}",
        "BINARY_SHA256": "${{ steps.authority.outputs.binary_sha256 }}",
        "DEPLOYED_COMMIT_SHA": "${{ steps.deployment.outputs.head_sha }}",
        "PROFILE": "${{ inputs.profile }}",
        "PUBLIC_BYTES": "${{ steps.benchmark.outputs.public_bytes }}",
        "PUBLIC_SHA256": "${{ steps.benchmark.outputs.public_sha256 }}",
        "RAW_BYTES": "${{ steps.benchmark.outputs.raw_bytes }}",
        "RAW_SHA256": "${{ steps.benchmark.outputs.raw_sha256 }}",
        "REVISION": "${{ steps.authority.outputs.revision }}",
    },
    "Fail closed on unsuccessful benchmark result": {
        "BENCHMARK_RESULT": "${{ steps.benchmark.outputs.result }}",
    },
}

EXPECTED_RUN_IF = {
    "Record exact production benchmark evidence":
        "${{ steps.benchmark.outputs.result == 'success' }}",
    "Fail closed on unsuccessful benchmark result":
        "${{ always() && steps.benchmark.outputs.result != 'success' }}",
}

EXPECTED_RUN_IDS = {
    "Bind successful exact private-candidate deployment": "deployment",
    "Reopen exact deployment and candidate identities": "authority",
    "Run through the fixed production helper": "benchmark",
}


def fail(message: str) -> NoReturn:
    raise PolicyError(message)


def require(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def exact_mapping(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict) or any(not isinstance(key, str) for key in value):
        fail(f"{label} must be an object with string keys")
    return value


def exact_keys(value: dict[str, Any], expected: set[str], label: str) -> None:
    actual = set(value)
    if actual != expected:
        fail(
            f"{label} fields differ: missing={sorted(expected - actual)} "
            f"unexpected={sorted(actual - expected)}"
        )


def load_workflow(repo_root: Path, relative: str) -> dict[str, Any]:
    path = repo_root / relative
    require(path.is_file() and not path.is_symlink(), f"missing plain workflow {relative}")
    try:
        contents = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        fail(f"could not read workflow {relative}: {error}")
    require(len(contents.encode("utf-8")) <= 1024 * 1024, f"workflow {relative} is unbounded")
    try:
        for token in yaml.scan(contents, Loader=GitHubWorkflowLoader):
            if isinstance(token, (yaml.tokens.AnchorToken, yaml.tokens.AliasToken)):
                fail(f"workflow {relative} uses forbidden YAML anchors or aliases")
        value = yaml.load(contents, Loader=GitHubWorkflowLoader)
    except yaml.YAMLError as error:
        fail(f"workflow {relative} is invalid or ambiguous YAML: {error}")
    return exact_mapping(value, f"workflow {relative}")


def validate_shared_concurrency(repo_root: Path) -> dict[str, dict[str, Any]]:
    workflows: dict[str, dict[str, Any]] = {}
    expected = {"group": "deploy-and-verify", "cancel-in-progress": False}
    for relative, error in SHARED_CONCURRENCY:
        workflow = load_workflow(repo_root, relative)
        workflows[relative] = workflow
        concurrency = exact_mapping(workflow.get("concurrency"), f"{relative} concurrency")
        require(concurrency == expected, error)
    return workflows


def validate_dispatch(workflow: dict[str, Any]) -> None:
    trigger = exact_mapping(workflow.get("on"), "conversion benchmark on")
    exact_keys(trigger, {"workflow_dispatch"}, "conversion benchmark triggers")
    dispatch = exact_mapping(trigger.get("workflow_dispatch"), "conversion benchmark dispatch")
    exact_keys(dispatch, {"inputs"}, "conversion benchmark dispatch")
    inputs = exact_mapping(dispatch.get("inputs"), "conversion benchmark inputs")
    require(set(inputs) == set(EXPECTED_INPUTS), "conversion benchmark typed dispatch inputs")
    for name, (expected_type, expected_options) in EXPECTED_INPUTS.items():
        specification = exact_mapping(inputs.get(name), f"conversion benchmark input {name}")
        expected_keys = {"description", "required", "type"}
        if expected_options is not None:
            expected_keys.add("options")
        exact_keys(specification, expected_keys, f"conversion benchmark input {name}")
        require(
            isinstance(specification["description"], str)
            and bool(specification["description"].strip()),
            f"conversion benchmark input {name} description",
        )
        require(specification["required"] is True, "conversion benchmark typed dispatch inputs")
        require(specification["type"] == expected_type, "conversion benchmark typed dispatch inputs")
        if expected_options is not None:
            require(specification["options"] == expected_options, "conversion benchmark typed dispatch inputs")


def validate_action_steps(steps: dict[str, dict[str, Any]]) -> None:
    checkout = steps["Check out exact protected benchmark operator"]
    exact_keys(checkout, {"name", "uses", "with"}, "conversion benchmark checkout step")
    require(
        checkout["uses"] == "actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd",
        "conversion benchmark protected checkout action",
    )
    checkout_with = exact_mapping(checkout["with"], "conversion benchmark checkout inputs")
    require(
        checkout_with
        == {
            "ref": "${{ github.workflow_sha }}",
            "fetch-depth": 0,
            "persist-credentials": False,
        },
        "conversion benchmark exact workflow-revision checkout ref",
    )

    download = steps["Download exact sanitized deployment evidence"]
    exact_keys(download, {"name", "uses", "with"}, "conversion benchmark download step")
    require(
        download["uses"]
        == "actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c",
        "conversion benchmark exact deployment artifact action",
    )
    require(
        exact_mapping(download["with"], "conversion benchmark download inputs")
        == {
            "name": "remi-deployment-inspection-${{ inputs.deployment_run_id }}",
            "path": "${{ runner.temp }}/deployment-inspection",
            "github-token": "${{ github.token }}",
            "repository": "${{ github.repository }}",
            "run-id": "${{ inputs.deployment_run_id }}",
        },
        "conversion benchmark exact successful protected deployment source",
    )

    failure_upload = steps["Upload public-sanitized benchmark failure evidence"]
    exact_keys(
        failure_upload,
        {"name", "if", "uses", "with"},
        "conversion benchmark failure upload step",
    )
    require(
        failure_upload["if"] == "${{ steps.benchmark.outputs.result == 'failure' }}",
        "conversion benchmark mutually exclusive result publication",
    )
    require(
        failure_upload["uses"]
        == "actions/upload-artifact@bbbca2ddaa5d8feaa63e36b76fdaad77386f024f",
        "conversion benchmark failure-only retained evidence",
    )
    require(
        exact_mapping(
            failure_upload["with"],
            "conversion benchmark failure upload inputs",
        )
        == {
            "name": "remi-conversion-benchmark-failure-${{ inputs.deployment_run_id }}-${{ github.run_id }}-${{ github.run_attempt }}",
            "path": "remi-conversion-benchmark-failure-v1.json",
            "if-no-files-found": "error",
            "compression-level": 0,
            "retention-days": 30,
        },
        "conversion benchmark failure-only retained evidence",
    )

    upload = steps["Upload public-sanitized benchmark evidence"]
    exact_keys(
        upload,
        {"name", "if", "id", "uses", "with"},
        "conversion benchmark upload step",
    )
    require(
        upload["if"] == "${{ steps.benchmark.outputs.result == 'success' }}",
        "conversion benchmark mutually exclusive result publication",
    )
    require(upload["id"] == "upload", "conversion benchmark public-only retained evidence")
    require(
        upload["uses"] == "actions/upload-artifact@bbbca2ddaa5d8feaa63e36b76fdaad77386f024f",
        "conversion benchmark public-only retained evidence",
    )
    upload_with = exact_mapping(upload["with"], "conversion benchmark upload inputs")
    exact_keys(
        upload_with,
        {"name", "path", "if-no-files-found", "compression-level", "retention-days"},
        "conversion benchmark upload inputs",
    )
    require(
        upload_with["name"]
        == "remi-conversion-benchmark-${{ inputs.deployment_run_id }}-${{ github.run_id }}-${{ github.run_attempt }}",
        "conversion benchmark public-only retained evidence",
    )
    require(
        isinstance(upload_with["path"], str)
        and upload_with["path"].splitlines()
        == [
            "remi-conversion-benchmark-public-v3.json",
            "remi-conversion-source-verification-v1.json",
            "remi-deployment-inspection.json",
            "remi-candidate-manifest.json",
        ],
        "conversion benchmark public-only retained evidence",
    )
    require(
        upload_with["if-no-files-found"] == "error"
        and upload_with["compression-level"] == 0
        and upload_with["retention-days"] == 30,
        "conversion benchmark public-only retained evidence",
    )


def validate_run_steps(steps: dict[str, dict[str, Any]]) -> None:
    for name, expected_digest in REVIEWED_RUN_BLOCK_SHA256.items():
        step = steps[name]
        expected_keys = {"name", "env", "shell", "run"}
        expected_id = EXPECTED_RUN_IDS.get(name)
        if expected_id is not None:
            expected_keys.add("id")
        expected_condition = EXPECTED_RUN_IF.get(name)
        if expected_condition is not None:
            expected_keys.add("if")
        exact_keys(step, expected_keys, f"conversion benchmark run step {name}")
        require(step.get("id") == expected_id, RUN_BLOCK_ERRORS[name])
        require(step.get("if") == expected_condition, RUN_BLOCK_ERRORS[name])
        require(step["shell"] == "bash", RUN_BLOCK_ERRORS[name])
        env = exact_mapping(step["env"], f"conversion benchmark run environment {name}")
        environment_error = RUN_BLOCK_ERRORS[name]
        if name == "Reopen exact deployment and candidate identities":
            environment_error = "conversion benchmark explicit comparable registered revision"
        elif name == "Run through the fixed production helper":
            environment_error = "conversion benchmark pinned production SSH host identity"
        require(env == EXPECTED_RUN_ENV[name], environment_error)
        run = step["run"]
        require(isinstance(run, str), RUN_BLOCK_ERRORS[name])
        observed_digest = hashlib.sha256(run.encode("utf-8")).hexdigest()
        require(observed_digest == expected_digest, RUN_BLOCK_ERRORS[name])


def validate_benchmark_workflow(workflow: dict[str, Any]) -> None:
    exact_keys(workflow, {"name", "on", "permissions", "concurrency", "jobs"}, "conversion benchmark workflow")
    require(workflow["name"] == "remi-conversion-benchmark", "conversion benchmark workflow name")
    validate_dispatch(workflow)
    require(
        exact_mapping(workflow.get("permissions"), "conversion benchmark permissions")
        == {"actions": "read", "contents": "read"},
        "conversion benchmark read-only permissions",
    )
    jobs = exact_mapping(workflow.get("jobs"), "conversion benchmark jobs")
    exact_keys(jobs, {"benchmark"}, "conversion benchmark jobs")
    job = exact_mapping(jobs["benchmark"], "conversion benchmark job")
    exact_keys(
        job,
        {"name", "runs-on", "timeout-minutes", "environment", "steps"},
        "conversion benchmark job",
    )
    require(
        job["name"] == "benchmark exact production conversion"
        and job["runs-on"] == "ubuntu-latest"
        and type(job["timeout-minutes"]) is int
        and job["timeout-minutes"] == 180
        and job["environment"] == "production",
        "conversion benchmark protected merged-main operator boundary",
    )
    raw_steps = job["steps"]
    require(isinstance(raw_steps, list), "conversion benchmark steps must be an array")
    names = [
        exact_mapping(step, "conversion benchmark step").get("name")
        for step in raw_steps
    ]
    require(names == EXPECTED_STEP_ORDER, "conversion benchmark exact reviewed step inventory")
    steps = {step["name"]: step for step in raw_steps}
    validate_action_steps(steps)
    validate_run_steps(steps)


def main() -> int:
    if len(sys.argv) > 2:
        print(f"usage: {Path(sys.argv[0]).name} [REPOSITORY]", file=sys.stderr)
        return 2
    repo_root = Path(sys.argv[1] if len(sys.argv) == 2 else ".").resolve()
    try:
        require(repo_root.is_dir(), f"repository root is not a directory: {repo_root}")
        workflows = validate_shared_concurrency(repo_root)
        validate_benchmark_workflow(workflows[BENCHMARK_WORKFLOW])
    except PolicyError as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("Remi conversion workflow structural checks passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
