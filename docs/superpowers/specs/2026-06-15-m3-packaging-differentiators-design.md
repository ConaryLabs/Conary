# M3 Packaging Differentiators Design

**Date:** 2026-06-15
**Status:** Review-patched umbrella design, pre-implementation planning
**Parent design:** `docs/superpowers/specs/2026-06-10-packaging-toolchain-design.md`
**Prerequisite milestone:** M2 release surface

## Purpose

M3 turns the package authoring loop from a capable CLI into an agent-friendly,
fast-feedback packaging environment. The parent packaging design names four M3
surfaces:

- structured diagnostics via `--json`
- agent-native packaging tools through MCP
- `conary try --watch`
- record mode

These are not four independent inventions. M3 must first establish a shared
diagnostic, event, operation-record, and redaction contract, then let the CLI,
MCP, watch mode, and record mode consume it. The first implementation slice
should therefore be structured diagnostics and events plus the local operation
record store they need. Record mode remains a prototype spike until tracing,
redaction, and draft recipe quality are proven.

The core invariant is:

> M3 may make packaging easier to drive, but it must not weaken M2 release
> gates or try-session safety.

No M3 command, JSON output, MCP tool, watch loop, or record trace may bypass
hermetic publish gates, accepted build-attestation policy, artifact-form publish
checks, or the one-active-try-session invariant.

Risk classification, publish gate evaluation, and M2 provenance checks operate
on the full command and artifact facts before any diagnostic redaction. Redacted
diagnostics, JSON, operation records, and MCP responses are derived views of
those already-classified facts, not inputs to the trust decision.

## Current Repo Facts

- `conary cook`, `conary new`, `conary try`, project-form publish, static
  artifact-form publish, foreign ingestion, and Remi release upload now exist
  as the staged packaging path from M1 through M2.
- `--explain` is already backed by recipe inference data structures under
  `crates/conary-core/src/recipe/inference/`, but M3 still needs a broader
  diagnostic/event model that spans inference, Kitchen execution, try sessions,
  publish gates, and record-mode traces.
- `apps/conary/src/commands/operation_records.rs` already provides atomic JSON
  record helpers for takeover/bootstrap operations. It does not yet define a
  packaging operation store, recent-record listing, retention, or
  redaction-before-write behavior.
- `apps/conary/src/commands/cook.rs`,
  `apps/conary/src/commands/publish.rs`, and
  `apps/conary/src/commands/try_session.rs` currently render human output
  directly in command-owned code. M3 should extract structured reports and keep
  rendering at the CLI edge.
- `apps/conary/src/commands/try_session.rs` is already over 3000 lines. Watch
  mode must not become another large block in that file. It needs a focused
  watch orchestrator and a narrow try-session API.
- Existing MCP patterns live in `apps/remi/src/server/mcp.rs`,
  `apps/conary-test/src/server/mcp.rs`,
  `apps/conary-test/src/server/stateless_mcp.rs`, `crates/conary-agent-contract`,
  and `crates/conary-mcp`. M3 packaging MCP should reuse those transport and
  contract boundaries rather than defining a Conary-CLI-only protocol.
- `crates/conary-core/src/capability/inference/` owns capability declaration
  inference. It does not observe user behavior. Record mode needs new tracing
  infrastructure before it can populate draft recipe/capability suggestions.
- The parent packaging design states that `recorded-draft` artifacts are never
  publishable directly. M2 publish gates already treat recorded drafts as a
  refusal state. M3 must preserve that rule.

## Scope

In scope for the M3 umbrella:

- A shared structured diagnostic model for packaging flows.
- A shared packaging event model for long-running cook, try, publish, watch, and
  record-mode operations.
- A local packaging operation-record store for recent diagnostic/event history.
- `--json` output for M3 packaging commands, rendered from the same diagnostic
  and event structures used by human output and MCP.
- Agent-native MCP packaging surfaces backed by the shared contract, starting
  with read-only and diagnostic-heavy tools.
- `conary try --watch` as source-watch plus cook plus throwaway try-session
  refresh.
- A record-mode spike that validates trace capture, redaction, draft recipe
  generation, and normal cook re-run viability.
- Focused docs and tests for the diagnostic/event contract and each slice.

Out of scope for M3:

- Any relaxation of M2 publish gates.
- Any direct publish path for `recorded-draft` artifacts.
- A general-purpose remote build service.
- An MCP tool that silently mutates the host or publishes artifacts without
  explicit risk labeling and plan/apply confirmation.
- A full record-mode implementation before the spike proves the tracing model.
- A DB-backed packaging operation store or schema migration unless a later task
  explicitly authorizes it.
- Ecosystem registry refs such as `crate:foo` or `pypi:bar`.
- Delta-only repository publishing.

## Milestone Shape

M3 remains one umbrella design executed as reviewable slices:

| Slice | Name | Gate |
|-------|------|------|
| M3a | Structured diagnostics and events | Stable JSON schema, renderer parity, no secret leakage |
| M3b | Agent-native packaging MCP surface | Thin transport over shared contract, read/diagnostic tools first |
| M3c0 | Try-session decomposition | Reviewed move map and parity tests before watch behavior |
| M3c | Watch mode | Source watch composes cook and try through narrow APIs |
| M3d | Record-mode spike | Prototype proves tracing/redaction/draft quality before commitment |

The slices should land in that order. M3c0 is a refactor gate before M3c, not
optional cleanup after watch mode exists. M3b, M3c, and M3d may add new event
kinds, but they must not redefine the core diagnostic schema after M3a
stabilizes it.

## M3a: Structured Diagnostics And Events

M3a adds the foundation. Packaging failures and progress become structured
values, then the CLI chooses how to render them.

The diagnostic model should be shared from `conary-core`, because the causes
mostly belong to core packaging concepts:

```rust
PackagingDiagnostic {
    phase,
    code,
    severity,
    message,
    evidence,
    suggestions,
    redactions,
}
```

Recommended ownership:

- `crates/conary-core/src/diagnostics/` or
  `crates/conary-core/src/recipe/diagnostics/`: shared DTOs, stable codes,
  evidence records, suggestion records, redaction markers, and schema tests.
- `apps/conary/src/commands/diagnostics.rs`: CLI rendering helpers for human
  text and JSON serialization.
- Packaging commands remain orchestration owners, but they should emit or
  convert structured diagnostics instead of building ad hoc error strings at
  every call site.

Human rendering and JSON rendering must both derive from these values. M3a
should not add a parallel JSON-only path while leaving human output on scattered
`println!` or `writeln!` call sites. `apps/conary/src/commands/diagnostics.rs`
owns rendering and small formatting helpers; command files own orchestration.
This preserves the ownership boundary for large command files such as
`cook.rs`, `publish.rs`, and `try_session.rs`.

The event model should cover long-running operations without turning every log
line into API:

```rust
PackagingEvent {
    operation_id,
    sequence,
    phase,
    kind,
    message,
    diagnostic,
    artifact,
    progress,
}
```

M3a should define a small stable set first: operation started, phase started,
phase finished, command started, command failed, diagnostic emitted, artifact
created, and operation finished.

`operation_id` is an instance-correlation id, not an operation kind. It should
reuse the existing `operation_records::new_operation_id` shape. The command
JSON object and streaming event envelope must include `schema_version`; initial
M3 uses schema version `1`. Additive event kinds and optional fields are minor
compatible changes inside the same major version. Removing fields, changing
field meaning, or changing diagnostic code semantics requires a major version
bump and an explicit compatibility note.

M3a also defines the contract relationship with the existing agent vocabulary:

- `PackagingDiagnostic.evidence` projects into
  `conary-agent-contract::EvidenceItem`.
- `PackagingDiagnostic.severity` remains a diagnostic severity. M3b maps it to
  `RiskLevel` only when a diagnostic participates in a mutation or trust gate.
- `PackagingEvent.operation_id` is distinct from the
  `OperationEnvelope.operation` string and from
  `conary_core::operations::OperationKind`.
- MCP adapters project core DTOs into `OperationEnvelope` rather than
  introducing a third evidence or risk vocabulary.

`--json` should have two modes:

- default command JSON: one final object containing outcome, diagnostics,
  artifacts, and summary
- streaming JSON lines for watch/long-running operations, added when a command
  needs event streaming

M3a must include a redaction policy and operation-record store:

- Diagnostics, events, operation records, and MCP responses cannot expose raw
  environment values, bearer tokens, private key paths, full command lines with
  embedded credentials, source URLs with credentials, or unbounded logs.
- Redaction wraps the serialization boundary for `PackagingDiagnostic`,
  `PackagingEvent`, and `EvidenceItem` fields including `command`, `path`,
  `uri`, and free-form metadata.
- Evidence carries redaction status so humans and agents know when data was
  intentionally hidden. The `EvidenceItem` projection either adds an explicit
  redaction metadata field or uses a wrapper that preserves the status.
- `--explain` and inference traces use the same redactor before serializing
  absolute paths, command fragments, source URLs, or inferred metadata.
- The local packaging operation store reuses `operation_records.rs` and adds a
  `packaging_operations_dir` owner. Records are JSON, atomically written,
  schema-versioned, and redacted before write.
- Packaging operation record files are created mode `0600` inside a private
  user-owned directory created with mode `0700`. The initial retention policy
  keeps the newest 50 packaging records and prunes older records after
  successful writes.
- M3a does not add a DB migration for operation records.

## M3b: Agent-Native Packaging MCP Surface

M3b exposes packaging capabilities through the existing MCP/agent-contract
architecture. The MCP layer should adapt stable packaging DTOs; it should not
become the product contract.

Initial tools/resources should be intentionally conservative:

- inspect packaging project
- explain inference for a directory or fetched source
- diagnose the last packaging failure from an operation record
- dry-run cook planning
- read recent packaging operation events

Mutating tools can follow only after the read/diagnostic surface is stable:

- cook
- try
- publish

Those mutating tools must carry risk labels and plan/apply confirmation. The
risk labels must be derived from the same CLI mutation classification used for
ordinary commands, then mapped into `conary-agent-contract` risk and
confirmation values. MCP cannot classify a command as lower-risk because the
serialized response is redacted or because the request arrived through an agent
surface.

Publish tools must also surface the M2 gate outcome rather than retrying,
bypassing, or special-casing it. Static artifact-form publish surfaces should
return the existing structured `PublishLintReport`, including distinct
`RecordedDraftArtifact`, `AbsentOrUnknownProvenanceClass`,
`NonHermeticHardeningLevel`, and other gate failure codes. Project-form publish
preflight failures that still bail directly must be converted to structured
packaging diagnostics or a project-form gate report before M3b exposes them
through MCP.

M3b should not require Remi. It may run locally through the CLI-side app or a
small Conary packaging MCP server, but the transport should be local stdio or
another authenticated local process boundary and should reuse
`crates/conary-agent-contract` and `crates/conary-mcp` patterns. The initial M3
packaging surface must not expose an unauthenticated TCP listener. The design
should avoid copying Remi's admin-heavy MCP catalog into the packaging surface;
packaging needs progressive discovery and narrowly-scoped tools.

M3b depends on the M3a packaging operation store. `diagnose last failure` and
`read recent packaging operation events` read redacted records from that store;
they do not scrape terminal output.

## M3c0: Try-Session Decomposition

`apps/conary/src/commands/try_session.rs` is over 3000 lines. Before watch mode
adds new behavior, M3c0 must move existing behavior into a reviewed ownership
boundary without changing semantics.

Required move map:

- `apps/conary/src/commands/try_session/mod.rs`: command entrypoint and narrow
  public API re-exports.
- `apps/conary/src/commands/try_session/validation.rs`: package, manifest,
  hook, scriptlet, and policy validation.
- `apps/conary/src/commands/try_session/session.rs`: active/orphan detection,
  session metadata, one-active enforcement, keep, rollback, and cleanup.
- `apps/conary/src/commands/try_session/namespace.rs`: namespace-mode mounts,
  protected paths, and sandbox setup.
- `apps/conary/src/commands/try_session/executor.rs`: command execution inside
  the prepared try environment.

Persisted state and public CLI behavior do not change in M3c0. The parity gate
is a focused test proving existing begin, rollback, keep, active-session
refusal, and orphan handling behave the same after the move. The watch slice may
then call only the narrow API exposed by the decomposed try-session module.

## M3c: Watch Mode

`conary try --watch` is a package project workflow:

```text
source change -> debounce -> cook -> create or refresh throwaway generation
              -> emit event -> wait for next change
```

It does not take a prebuilt `.ccs`; watch mode owns the rebuild loop. The
initial implementation should run from a directory with a recipe or inference
target, honor the same source ignore rules as normal cook, and emit structured
events from M3a.

Watch mode is namespace-mode only in M3. It must not combine `--watch` with
host activation. The first watch implementation also does not support keeping
or publishing watch-created artifacts directly. A user who wants to keep or
publish uses a normal non-watch cook/try/publish path after the source has
stabilized.

Ownership should be split:

- `apps/conary/src/commands/try_watch.rs` or a child module under
  `apps/conary/src/commands/try_session/`: watch orchestration, debounce,
  cancellation, and command wiring.
- `apps/conary/src/commands/try_session/`: keep/rollback/session state owner
  only through the narrow API created in M3c0.
- `crates/conary-core` remains owner of recipe, Kitchen, transaction, and
  try-session model concepts.

Watch mode must preserve:

- one active try session at a time
- rollback/keep semantics
- non-interactive fail-closed orphan handling
- hook and scriptlet safety policy
- M2 artifact/provenance truth

Refresh semantics must be explicit. Each debounce iteration either rolls back
the previous throwaway session before starting the next cook/try generation, or
uses a narrow `refresh_throwaway_session` API that is atomic with respect to the
try-session state machine. Cancellation cannot leave a half-applied active
session. If cleanup fails, the watch loop stops and reports the orphan instead
of applying another generation.

Every rebuild reruns the same try package, manifest, hook, and scriptlet policy
validation as a normal try session. A watched source edit that introduces a
non-generation-scoped hook, unsafe scriptlet, or unsupported manifest feature
fails the current iteration closed and does not reuse a prior safe decision.

Source identity is recalculated after debounce and before each rebuild.
Network/source-fetch behavior follows M2 hermetic expectations: a watch session
may prefetch once through the normal source policy, then rebuilds use
offline-cache-only inputs unless the user explicitly restarts the watch session
with a fresh prefetch. This keeps watch artifacts useful for iteration without
turning them into publishable evidence.

The first watch implementation can be whole-package rebuild plus try refresh.
Incremental build optimizations are later work unless an existing build system
already provides them safely.

## M3d: Record-Mode Spike

Record mode is the most distinctive M3 feature and the least proven. It must
start as a spike, not a full user-facing promise.

The spike should answer four questions:

1. Can Conary observe enough of a build/install session to derive useful build
   and install recipe steps?
2. Can it redact secrets from env, args, paths, logs, and file samples without
   destroying useful evidence?
3. Can the generated draft recipe pass a normal `conary cook` after human
   review on at least one simple fixture?
4. Can the trace report explain what it trusted, guessed, ignored, and redacted?

The spike may use seccomp-notify, fanotify, ptrace, bwrap logging, or another
reviewed Linux tracing technique, but the implementation plan must choose one
explicitly and name its privilege model, process scope, and cleanup behavior.
The spike must not rely on an unbounded host-root tracer or broad
`CAP_SYS_ADMIN` session. If the selected technique requires a capability or
kernel feature that is unavailable, record mode fails closed instead of
silently degrading capture.

Trace output is secret-bearing until proven otherwise. The default trace
location is private, ephemeral, and user-owned; directories are mode `0700`,
trace files are mode `0600`, and persistence requires an explicit opt-in flag.
Redaction runs before writing any trace report or operation record. Cleanup must
remove raw trace fragments on success, failure, cancellation, and kill-switch
exit.

Record-mode output is always:

- a draft recipe
- a trace report
- optional capability suggestions
- `origin_class = "recorded-draft"` for any artifact produced in the recording
  path

Recorded drafts are never publishable directly. The graduation path is normal:
review or edit the recipe, run regular `conary cook`, then use the existing M2
publish path.

## Data Flow

M3a data flow:

```text
core packaging operation -> PackagingDiagnostic / PackagingEvent
                          -> CLI renderer -> human text or JSON
                          -> redacted operation record
```

The operation record is written only after redaction. Trust and risk decisions
use the raw in-memory facts before the redacted projection is produced.

M3b data flow:

```text
agent request -> MCP adapter -> shared packaging contract
              -> command/core operation -> diagnostics/events
              -> optional redacted operation record lookup
              -> MCP response with risk/outcome metadata
```

M3c data flow:

```text
watcher -> debounce -> cook operation -> try-session refresh API
        -> event stream -> human/JSON/MCP consumer
```

M3d spike data flow:

```text
record shell -> trace collector -> redactor -> draft recipe + trace report
             -> normal cook validation -> recorded-draft refusal evidence
```

## Error Handling And Safety

M3 errors should remain boring and explicit:

- Unknown diagnostic codes serialize as unknown and render with generic next
  steps; they do not panic the renderer.
- JSON schema version mismatches are visible in the output, and every command
  JSON object or event line includes `schema_version`.
- Redaction failures fail closed when secrets may be present.
- MCP mutations require explicit risk metadata and confirmation.
- MCP local transport must be stdio or another authenticated local process
  boundary in M3; unauthenticated TCP is out of scope.
- Watch mode must stop cleanly on repeated cook failures without leaving an
  untracked active try session.
- Watch mode must not auto-keep or publish.
- Record mode must not publish, install globally, or silently preserve traces
  containing secrets.
- All M3 publish-related diagnostics report the existing M2 gate result as
  structured diagnostics. Static artifact-form gates include the existing
  `PublishLintReport`; project-form preflight failures must not remain raw
  string-only bail errors once exposed through M3 JSON or MCP.
- Every `PublishGateFailureCode` maps to a stable diagnostic code in the same
  change that adds the gate code. An exhaustiveness test reserves `unknown` for
  future or foreign codes, not local mapping gaps.

## Testing Strategy

M3a tests:

- unit tests for diagnostic code serialization, severity, suggestions, evidence,
  and redaction markers
- tests asserting `schema_version` appears in command JSON and streaming event
  envelopes
- golden JSON tests for representative cook, inference, try, and publish gate
  outcomes
- golden JSON tests for project-form publish preflight failures that currently
  originate as direct bail errors
- renderer parity tests proving human and JSON output come from the same
  diagnostic values
- redaction leak tests for env secrets, bearer tokens, credentialed URLs,
  private key paths, command evidence, inference traces, and operation records
- operation-record tests for atomic write, `0600` file mode, recent-record
  ordering, retention, and redaction-before-write

M3b tests:

- MCP catalog tests proving tool names, risk labels, and progressive discovery
- read-only tool tests for project inspection and inference explanation
- mutation guard tests proving cook/try/publish tools require confirmation
- tests proving MCP risk labels map from CLI command risk rather than from
  redacted output
- tests proving publish tools surface M2 gate failures without bypass, including
  distinct recorded-draft and absent/unknown provenance codes, plus
  project-form preflight failures
- catalog guard tests proving the packaging MCP surface does not grow
  accidental tools or unauthenticated TCP listeners

M3c0 tests:

- parity tests for begin, rollback, keep, one-active refusal, and orphan
  handling before and after the try-session decomposition
- module-boundary tests or compile-time checks proving watch code can call only
  the narrow try-session API

M3c tests:

- unit tests for debounce, ignore rules, cancellation, and event ordering
- focused CLI tests for `try --watch` startup, rebuild failure reporting, and
  rollback cleanup
- tests proving the one-active-try-session invariant still holds
- tests proving refresh rolls back or atomically replaces the previous
  throwaway session before applying a new one
- tests proving hook/scriptlet policy is rerun on every rebuild
- tests proving source identity changes are rehashed after debounce and watch
  rebuilds use offline-cache-only inputs after the initial prefetch
- stale watch-session cleanup tests using the normal active/orphan policy

M3d spike tests:

- fixture recording smoke for one simple build/install flow
- redaction tests for env secrets, token-like args, private key paths, and logs
- generated draft recipe can be cooked after review on the fixture
- recorded-draft publish refusal remains visible
- trace storage tests for private permissions, default ephemerality, cleanup,
  and fail-closed behavior when the selected tracing privilege is unavailable

Final gates for implementation slices should include the owning package tests,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`,
and the doc truth/coherency checks whenever docs or public claims change.

## Documentation And Rollout

M3 surfaces appear in help only as they land. Before an implementation slice is
complete:

- `--json` should not be advertised for packaging commands that still emit only
  ad hoc text.
- MCP tools should not appear in a live catalog until their contract, risk
  labels, and tests exist.
- `try --watch` should remain hidden or rejected with an honest message until
  the watch orchestrator lands.
- `--record` should remain hidden or clearly experimental until the spike
  graduates into an implementation plan.

Docs to update as M3 lands:

- `docs/superpowers/specs/2026-06-10-packaging-toolchain-design.md`
- `docs/guides/first-package.md` after user-facing command behavior changes
- `docs/llms/subsystem-map.md`
- `docs/modules/feature-ownership.md`
- `docs/operations/infrastructure.md` only if a deployed MCP/service surface is
  introduced

## Review Checklist

- M3a is the first implementation slice.
- Record mode is a spike before full commitment.
- No M3 feature weakens M2 publish gates.
- M2 risk and gate decisions run before redacted diagnostic projection.
- M3a owns the file-based packaging operation-record store.
- JSON and MCP surfaces share DTOs rather than duplicating contracts.
- JSON and event envelopes carry `schema_version`.
- Secrets are redacted by default.
- MCP risk labels map from CLI mutation risk into the agent contract.
- MCP publish returns structured M2 gate reports, including static
  `PublishLintReport` data and project-form preflight diagnostics.
- Local MCP transport is stdio or an authenticated local process boundary.
- M3c0 decomposes `try_session.rs` before watch behavior lands.
- Watch mode preserves try-session invariants.
- Watch mode is namespace-only, does not auto-keep, and does not publish.
- Watch rebuilds rerun hook/scriptlet policy and use offline-cache-only inputs
  after the initial prefetch.
- Record mode has explicit privilege, storage, redaction, and cleanup bounds.
- Help text does not advertise unavailable M3 features.

## Ready For Planning

M3 is ready for implementation planning when this design has passed local
agentic review and any review-derived fixes are committed. The first
implementation plan should cover M3a only: structured diagnostics, events,
schema versioning, redaction, and the packaging operation-record store. Later
plans can consume that foundation for MCP, try-session decomposition, watch
mode, and record-mode spike work.
