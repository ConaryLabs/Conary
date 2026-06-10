# conary-test Evidence Trust Design

## Status

Ready for implementation via
`docs/superpowers/plans/archive/2026-06-10-conary-test-evidence-trust-plan.md`.

## Goal

Make `conary-test` evidence reliable enough that release gates and future
coherency waves can cite it without accidentally treating "never ran" as
"passed" or accepting malformed result files.

## Background

External review found four related failure modes in the integration harness:

- QEMU boot setup failures can return a pass-shaped `ExecResult` with
  `exit_code = 0`.
- `scripts/check-conary-test-result-gate.sh` accepts empty or summary-less JSON.
- Integration manifests can reuse test IDs across files.
- Manifest deserialization ignores unknown assertion keys such as
  `stderr_not_contains`.

Local inspection confirmed the current manifest set has 333 tests and duplicate
IDs including `T138` and `T230` through `T251`. The current `Assertion` struct
contains `stderr_contains` but not `stderr_not_contains`, while
`phase4-group-d.toml` uses `stderr_not_contains = "panic"`.

## Policy Decision

Evidence-producing runs must fail closed:

- A skipped QEMU setup path records `TestStatus::Skipped`, never `Passed`, and
  the skip outcome must survive any `qemu.rs` -> executor -> runner conversion.
- Release evidence rejects `failed`, `skipped`, and `cancelled` by default.
- Result JSON must include `summary.total`, a summary, and at least one result
  unless a caller explicitly asks for a non-release diagnostic mode.
- `summary.total` must match `results.length`, and summary status counts must
  match the actual `results[]` statuses.
- Manifest IDs are unique across the manifest set loaded by a phase or suite.
- Unknown manifest keys fail validation.

## Scope

This track owns harness truth, not QEMU feature expansion. It may update
`apps/conary-test`, the integration manifests, local QEMU validation scripts,
workflow documentation, and integration testing docs. It should not introduce a
new remote runner or make KVM-required suites mandatory on GitHub-hosted
runners unless that infrastructure already exists.

## Implementation Shape

The implementation should proceed in this order:

1. Fix QEMU skip semantics and runner status mapping.
2. Harden the release result gate.
3. Wire the result gate into local QEMU validation and build every binary that
   selected suites can execute.
4. Make manifest parsing fail on unknown keys.
5. Validate duplicate test IDs across loaded manifests.
6. Renumber or explicitly namespace duplicate IDs.
7. Refresh integration docs for actual counts, commands, JSON shape, QEMU skip
   behavior, and CI execution limits.

## Verification Strategy

Required focused gates:

- `cargo test -p conary-test engine::runner`
- `cargo test -p conary-test config::manifest`
- `cargo test -p conary-test config::`
- `bash scripts/check-conary-test-result-gate.sh /tmp/conary-test-good-result.json`
- a negative fixture for `{}` against `scripts/check-conary-test-result-gate.sh`
- `cargo run -p conary-test -- list`
- `bash scripts/test-conary-test-result-gate.sh`
- `bash scripts/check-doc-truth.sh`
- docs-audit inventory and ledger checks
- `git diff --check`

Full local QEMU execution remains host-dependent. The plan should state exactly
which steps require `/dev/kvm` and which steps can run on an ordinary
developer or CI host.

## Documentation Requirements

`docs/INTEGRATION-TESTING.md` must describe:

- whether TOML integration suites execute in CI today;
- the exact Phase 1 through Phase 4 manifest counts and ranges after repairs;
- `images build`, `images list`, and `deploy rebuild`;
- result JSON shape and result-gate behavior;
- QEMU skip semantics and the local KVM requirement.

`apps/conary-test/README.md` should be updated if the CLI or MCP overview repeats
claims about command availability, result status, or execution mode.

## Non-Goals

- Do not make QEMU tests run on GitHub-hosted runners without KVM support.
- Do not change the semantics of existing passing integration assertions except
  where they were silently ignored.
- Do not loosen release evidence to accept skipped tests by default.
