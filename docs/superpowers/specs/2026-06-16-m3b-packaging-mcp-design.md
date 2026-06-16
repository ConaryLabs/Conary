# M3b Packaging MCP Surface Design

**Date:** 2026-06-16
**Status:** User-approved design, ready for implementation planning
**Parent design:** `docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md`
**Prerequisite milestone:** M3a structured diagnostics and operation records

## Purpose

M3b exposes packaging workflows to agents through a local MCP surface that
adapts the M3a packaging diagnostic, event, redaction, and operation-record
contract. The MCP layer is an adapter. It must not become the product contract,
invent a parallel risk model, scrape terminal output, or weaken M2 publish
gates.

M3b includes the first mutation tool contract. The first mutation target is
publish, not cook, because publish exercises the M2 trust boundary directly:
artifact signatures, attestation truth, hermetic provenance, static repository
state, and fail-closed lint reports.

The core invariant is:

> MCP may project packaging facts for agents, but all trust and mutation
> decisions remain owned by the same CLI/core code paths that human commands
> use.

## Current Repo Facts

- `crates/conary-agent-contract` owns the transport-neutral operation envelope,
  risk levels, evidence items, confirmation requirements, resource references,
  and catalog metadata.
- `crates/conary-mcp` owns shared MCP adapter helpers. Its draft stateless path
  currently supports discovery and resources; live tool execution exists in
  the session-based Remi and `conary-test` MCP servers.
- `apps/conary` does not currently expose an MCP server command and does not
  depend on `conary-agent-contract` or `conary-mcp`.
- `apps/conary/src/command_risk.rs` owns CLI mutation classification. Packaging
  MCP risk must project from this vocabulary and can only be equal-or-higher
  than the CLI risk.
- `crates/conary-core/src/diagnostics/` owns the M3a packaging JSON schema,
  diagnostics, events, redaction markers, and command output DTOs.
- `apps/conary/src/commands/diagnostics.rs` owns CLI rendering, redaction
  before serialization, and operation-record write integration.
- `apps/conary/src/commands/operation_records.rs` owns the private packaging
  operation-record store and recent-record helpers.
- `apps/conary/src/commands/publish.rs` owns publish routing, static
  artifact-form gate checks, project-form publish, and Remi artifact-form
  routing.
- `conary publish --json` already emits structured M3a output for static
  artifact-form gate failures, project-form preflight failures, and explicit
  Remi JSON unsupported diagnostics.

## Scope

M3b should expose:

- inspect a packaging project or artifact
- explain recipe inference for a local source tree, archive, or git target
- diagnose the latest packaging failure from the M3a operation-record store
- list and read recent packaging operation records and events
- plan publish
- apply publish for the first supported mutation route

M3b should not expose:

- unauthenticated TCP MCP listeners
- arbitrary command execution
- raw environment maps, bearer tokens, private key material, or shell snippets
- `try`, watch mode, or record mode mutation tools
- Remi publish apply
- publish gate bypasses or special MCP-only exemptions

The first successful mutation path is static artifact-form publish:

```text
conary publish <pkg.ccs> <static-target>
```

Project-form publish may be planned and explained in M3b, but apply can remain
unsupported until the implementation plan explicitly wires the full hermetic
cook plus static publish path through the same structured report boundary.
Remi publish returns a clear `not_supported` or `unavailable` envelope until
Remi artifact-form JSON/apply behavior is deliberately added.

## Architecture

M3b adds a local packaging MCP surface behind a developer-facing command such
as `conary mcp packaging`. The transport is local stdio for M3b. If a later
slice adds stateless HTTP, it must be authenticated or local-only and reuse the
same provider/service contract.

Recommended ownership:

- `crates/conary-agent-contract`: packaging resource helpers, result payload
  DTOs that are transport-neutral, and catalog entries for the new packaging
  operations.
- `crates/conary-mcp`: reusable MCP tool discovery/call helpers, kept separate
  from product semantics. If the draft stateless helper grows tool support, it
  should stay framework-neutral like the existing resource path.
- `apps/conary/src/commands/packaging_mcp.rs` or
  `apps/conary/src/commands/mcp/packaging.rs`: local MCP command, tool
  registration, request validation, and projection between MCP requests and the
  packaging agent service.
- `apps/conary/src/commands/publish.rs`: publish implementation owner. M3b may
  add helper entrypoints, but MCP must not duplicate publish routing, gate
  checks, or repository writes.
- `apps/conary/src/commands/diagnostics.rs`: projection from
  `PackagingCommandOutput` to agent evidence and error envelopes.
- `apps/conary/src/commands/operation_records.rs`: operation-record lookup and
  recent-record listing.

The CLI app should expose a narrow packaging agent service with typed methods:

```text
inspect_project(input) -> InspectResult
explain_inference(input) -> ExplainResult
diagnose_latest_failure(input) -> ExplainResult
list_operation_records(input) -> InspectResult
read_operation_record(input) -> InspectResult
plan_publish(input) -> PlanResult
apply_publish(input) -> ApplyResult
```

MCP calls this service. Existing human CLI commands keep their current command
paths. This boundary keeps MCP transport details out of `publish.rs` while
making the agent contract testable without a live MCP session.

## Resources And Tool Names

Use agent resource helpers for stable references:

- `conary-packaging://operations/recent`
- `conary-packaging://operations/{operation_id}`
- `conary-packaging://operations/{operation_id}/events`
- `conary-packaging://projects/{encoded-project-id}`
- `conary-packaging://artifacts/{encoded-artifact-id}`

Local paths may appear in redacted evidence metadata, but resource URIs should
not be raw absolute paths. If a stable id cannot be computed safely, return a
resource with a generated operation-local id and include redacted path evidence
separately.

Initial tool names:

- `conary.packaging.inspect_project`
- `conary.packaging.explain_inference`
- `conary.packaging.diagnose_latest_failure`
- `conary.packaging.operation_records.list`
- `conary.packaging.operation_records.read`
- `conary.packaging.publish.plan`
- `conary.packaging.publish.apply`

Read-only record data may also be exposed as MCP resources. Path-dependent
operations should remain tools because the caller supplies local filesystem
inputs.

## Publish Plan Contract

`conary.packaging.publish.plan` accepts structured inputs only:

- artifact path or project path
- static target destination for artifact form
- optional recipe path
- optional static key directory and publish state file
- safe publish flags such as `refresh`
- requested mode: `artifact_static`, `project_static`, or `auto`

It does not accept:

- shell command strings
- environment maps
- bearer tokens
- private key contents
- arbitrary extra CLI args

The plan output is a `PlanResult` with:

- `operation = "conary.packaging.publish.plan"`
- `status = planned` for coherent inputs
- `risk` projected from CLI command risk and publish route
- subject resource for the artifact or project
- evidence for target classification, target route, normalized options,
  static-vs-Remi decision, and any preflight checks that are safe before apply
- `ConfirmationRequirement` with `plan_id`, `fingerprint`, reason, input label,
  and expiry
- next action pointing to `conary.packaging.publish.apply`

The fingerprint is computed over canonical plan material, including normalized
paths, target destination, route, selected options, artifact identity when
available, command-risk projection, and schema version. M3b does not need a
persistent plan database. A stdio MCP session may keep an in-memory plan
registry. If a later stateless transport needs cross-request plan persistence,
it should use a private `0600`/`0700` store with explicit expiry.

## Publish Apply Contract

`conary.packaging.publish.apply` accepts:

- `plan_id`
- `fingerprint`
- explicit confirmation input when the plan requires it
- no changed publish options except a documented narrow allowlist

Apply behavior:

1. Look up the plan in the current MCP session.
2. Recompute and compare the fingerprint.
3. Reject expired, missing, mismatched, or unconfirmed plans with
   `AgentErrorKind::UnsafeWithoutConfirmation`.
4. Revalidate inputs and target route.
5. Rerun M2 publish gates immediately before any static repository write.
6. Call the existing publish implementation path.
7. Project the resulting M3a `PackagingCommandOutput` into `ApplyResult`.
8. Include the redacted operation-record reference instead of scraping stdout.

For static artifact-form publish, a gate refusal is a failed apply result, not
a transport failure. The envelope should use `status = failed`,
`error.kind = validation_failed`, and evidence containing the structured
`PublishLintReport`, including codes such as `RecordedDraftArtifact`,
`AbsentOrUnknownProvenanceClass`, and `NonHermeticHardeningLevel`.

Project-form apply can initially return `not_supported` unless the
implementation plan chooses to wire the full hermetic cook and static publish
path in M3b. Remi apply returns `not_supported` or `unavailable` and must not
resolve bearer tokens in M3b.

## Risk Projection

MCP risk must come from the existing CLI mutation classifier. Recommended
mapping:

| CLI command risk | Agent risk |
|------------------|------------|
| `ReadOnly` | `read_only` |
| `DryRunOnly` | `read_only` |
| `LocalStateMutation` | `medium` |
| `DbMutation` | `high` |
| `ActiveHostMutation` | `high` |
| `AlwaysLive` | `destructive` |
| `HookRefreshDbMutation` | `high` |

Static artifact-form publish writes repository state, keys, metadata, or publish
state files, so it must not be reported as read-only. MCP may raise risk for a
route or option, but it must never lower the CLI-derived risk because the
response is redacted or because the request arrived through MCP.

## Read And Diagnostic Tools

`inspect_project` should report packaging-relevant facts without building:
target kind, recipe presence, inferred package metadata when safe, detected
publish route hints, and next actions.

`explain_inference` should project the existing inference trace through the M3a
redactor. It should not write a recipe unless a later mutation contract adds a
plan/apply path for recipe materialization.

`diagnose_latest_failure` reads the newest failed packaging operation record
from the private M3a store. It summarizes diagnostics, evidence, redactions,
and suggested next actions. It must not scrape terminal output.

`operation_records.list` and `operation_records.read` expose redacted operation
records and event lists. Missing records return `MissingPrerequisite` or
resource-not-found errors, not raw filesystem errors.

## Error Model

Use `AgentErrorKind` consistently:

- malformed input, invalid mode, invalid target route:
  `ValidationFailed`
- missing artifact, missing record, missing plan:
  `MissingPrerequisite`
- Remi apply or project apply when not wired:
  `NotSupported`
- stale, expired, mismatched, or unconfirmed plan:
  `UnsafeWithoutConfirmation`
- publish gate refusal:
  `ValidationFailed` with `PublishLintReport` evidence
- unavailable local prerequisites:
  `RemoteUnavailable` or `Unavailable` status, depending on the existing
  envelope vocabulary available during implementation
- unexpected publish failure:
  failed envelope with M3a diagnostics and operation-record evidence

Errors should preserve the operation envelope. Agents should always be able to
see the operation name, risk, subject when available, summary, evidence, and
next action.

## Security And Redaction

M3b must preserve these rules:

- no public network listener
- no arbitrary shell command execution
- no raw env/token/key inputs
- no bearer-token resolution for Remi apply
- no path evidence or command evidence before redaction
- no publish gate bypass
- apply reruns gates even when plan already checked them
- operation records remain private and redacted
- plan fingerprints bind route, target, options, and artifact identity

The MCP response should include redaction metadata so agents can distinguish
"not known" from "known but intentionally hidden."

## Testing Strategy

Contract tests:

- `cargo test -p conary-agent-contract`
- resource helper URI stability
- packaging result payload serialization
- confirmation requirement shape and fingerprint fields

MCP adapter tests:

- `cargo test -p conary-mcp`
- tool list/call request validation
- tool name/header mismatch failures
- method-not-found behavior
- local-only transport assumptions

Conary packaging service tests:

- inspect project does not build or publish
- inference explain uses redacted trace data
- latest-failure diagnosis reads operation records, not stdout
- operation record list/read preserves redactions
- publish plan classifies static artifact-form, project-form, and Remi routes
- publish plan returns risk and confirmation
- publish apply refuses missing, expired, or mismatched plans
- publish apply reruns static artifact gate checks
- static artifact-form MCP gate refusal returns the same `PublishLintReport`
  evidence as CLI `publish --json`
- redaction leak tests for paths, credentials, bearer tokens, private key paths,
  and operation records

Likely focused commands:

```bash
cargo test -p conary-agent-contract
cargo test -p conary-mcp
cargo test -p conary commands::diagnostics::tests
cargo test -p conary --test packaging_m3a
cargo test -p conary --test packaging_m3b
```

Run the doc audit and coherency ledger checks whenever public docs or routing
maps change.

## Documentation And Rollout

M3b should update:

- `docs/superpowers/specs/2026-06-15-m3-packaging-differentiators-design.md`
  when implementation status changes
- `docs/operations/infrastructure.md` when a local packaging MCP entrypoint
  exists
- `docs/llms/subsystem-map.md` and `docs/modules/feature-ownership.md` when
  the "look here first" path changes
- command help only when the MCP command is intentionally visible

Do not advertise `try`, watch mode, record mode, or Remi publish apply through
MCP until those contracts and tests exist.

## Review Checklist

- MCP is an adapter over M3a, not a new product contract.
- Publish is the first mutation contract.
- Static artifact-form publish is the first supported apply success path.
- M2 gates rerun at apply time.
- Gate refusal returns structured evidence and does not publish.
- Project-form and Remi apply are explicit unsupported/unavailable paths until
  wired.
- Risk derives from CLI command classification and is never lowered by MCP.
- Operation records are read from the private M3a store.
- No public unauthenticated listener is added.
- No shell/env/token/key passthrough is accepted.

## Ready For Planning

M3b is ready for implementation planning once this spec is reviewed. The plan
should split the work into transport-neutral contract additions, MCP adapter
tool support, CLI packaging agent service methods, publish plan/apply wiring,
focused tests, and docs/ledger updates.
