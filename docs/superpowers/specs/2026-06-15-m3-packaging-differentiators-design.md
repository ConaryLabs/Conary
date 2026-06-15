# M3 Packaging Differentiators Design

**Date:** 2026-06-15
**Status:** Approved umbrella design, pre-implementation planning
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
diagnostic and event contract, then let the CLI, MCP, watch mode, and record
mode consume it. The first implementation slice should therefore be structured
diagnostics and events. Record mode remains a prototype spike until tracing,
redaction, and draft recipe quality are proven.

The core invariant is:

> M3 may make packaging easier to drive, but it must not weaken M2 release
> gates or try-session safety.

No M3 command, JSON output, MCP tool, watch loop, or record trace may bypass
hermetic publish gates, accepted build-attestation policy, artifact-form publish
checks, or the one-active-try-session invariant.

## Current Repo Facts

- `conary cook`, `conary new`, `conary try`, project-form publish, static
  artifact-form publish, foreign ingestion, and Remi release upload now exist
  as the staged packaging path from M1 through M2.
- `--explain` is already backed by recipe inference data structures under
  `crates/conary-core/src/recipe/inference/`, but M3 still needs a broader
  diagnostic/event model that spans inference, Kitchen execution, try sessions,
  publish gates, and record-mode traces.
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
- Ecosystem registry refs such as `crate:foo` or `pypi:bar`.
- Delta-only repository publishing.

## Milestone Shape

M3 remains one umbrella design executed as reviewable slices:

| Slice | Name | Gate |
|-------|------|------|
| M3a | Structured diagnostics and events | Stable JSON schema, renderer parity, no secret leakage |
| M3b | Agent-native packaging MCP surface | Thin transport over shared contract, read/diagnostic tools first |
| M3c | Watch mode | Source watch composes cook and try through narrow APIs |
| M3d | Record-mode spike | Prototype proves tracing/redaction/draft quality before commitment |

The slices should land in that order. M3b, M3c, and M3d may add new event kinds,
but they must not redefine the core diagnostic schema after M3a stabilizes it.

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

`--json` should have two modes:

- default command JSON: one final object containing outcome, diagnostics,
  artifacts, and summary
- streaming JSON lines for watch/long-running operations, added when a command
  needs event streaming

M3a must include a redaction policy. Diagnostics and events cannot expose raw
environment values, bearer tokens, private key paths, full command lines with
embedded credentials, or unbounded logs. Evidence should carry redaction status
so humans and agents know when data was intentionally hidden.

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

Those mutating tools must carry risk labels and plan/apply confirmation. Publish
tools must also surface the M2 gate outcome rather than retrying, bypassing, or
special-casing it.

M3b should not require Remi. It may run locally through the CLI-side app or a
small Conary packaging MCP server, but the transport should reuse
`crates/conary-agent-contract` and `crates/conary-mcp` patterns. The design
should avoid copying Remi's admin-heavy MCP catalog into the packaging surface;
packaging needs progressive discovery and narrowly-scoped tools.

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

Ownership should be split:

- `apps/conary/src/commands/try_watch.rs` or a child module under
  `apps/conary/src/commands/try_session/`: watch orchestration, debounce,
  cancellation, and command wiring.
- `apps/conary/src/commands/try_session.rs`: keep/rollback/session state owner
  only through a narrow API.
- `crates/conary-core` remains owner of recipe, Kitchen, transaction, and
  try-session model concepts.

Watch mode must preserve:

- one active try session at a time
- rollback/keep semantics
- non-interactive fail-closed orphan handling
- hook and scriptlet safety policy
- M2 artifact/provenance truth

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
explicitly and name its privilege model. It must also include a kill switch and
clear cleanup behavior.

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
                          -> operation record -> MCP resource/tool response
```

M3b data flow:

```text
agent request -> MCP adapter -> shared packaging contract
              -> command/core operation -> diagnostics/events
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
- JSON schema version mismatches are visible in the output.
- Redaction failures fail closed when secrets may be present.
- MCP mutations require explicit risk metadata and confirmation.
- Watch mode must stop cleanly on repeated cook failures without leaving an
  untracked active try session.
- Watch mode must not auto-keep or publish.
- Record mode must not publish, install globally, or silently preserve traces
  containing secrets.
- All M3 publish-related diagnostics report the existing M2 gate result.

## Testing Strategy

M3a tests:

- unit tests for diagnostic code serialization, severity, suggestions, evidence,
  and redaction markers
- golden JSON tests for representative cook, inference, try, and publish gate
  outcomes
- renderer parity tests proving human and JSON output come from the same
  diagnostic values

M3b tests:

- MCP catalog tests proving tool names, risk labels, and progressive discovery
- read-only tool tests for project inspection and inference explanation
- mutation guard tests proving cook/try/publish tools require confirmation
- tests proving publish tools surface M2 gate failures without bypass

M3c tests:

- unit tests for debounce, ignore rules, cancellation, and event ordering
- focused CLI tests for `try --watch` startup, rebuild failure reporting, and
  rollback cleanup
- tests proving the one-active-try-session invariant still holds

M3d spike tests:

- fixture recording smoke for one simple build/install flow
- redaction tests for env secrets, token-like args, private key paths, and logs
- generated draft recipe can be cooked after review on the fixture
- recorded-draft publish refusal remains visible

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
- JSON and MCP surfaces share DTOs rather than duplicating contracts.
- Secrets are redacted by default.
- Watch mode preserves try-session invariants.
- `try_session.rs` gains a decomposition path before watch behavior lands.
- Help text does not advertise unavailable M3 features.

## Ready For Planning

M3 is ready for implementation planning when this design has passed local
review and any review-derived fixes are committed. The first implementation
plan should cover M3a only: structured diagnostics and events. Later plans can
consume that foundation for MCP, watch mode, and record-mode spike work.
