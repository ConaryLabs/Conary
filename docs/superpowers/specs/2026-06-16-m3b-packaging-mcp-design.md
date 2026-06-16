# M3b Packaging MCP Surface Design

**Date:** 2026-06-16
**Status:** Review-patched design, ready for implementation planning
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
- The current publish code is command-oriented. The static artifact-form path
  writes to a `Write` and returns `Result<()>`; M3b needs a narrow service-safe
  helper that returns `PackagingCommandOutput` without moving publish ownership
  into MCP.
- `crates/conary-core/src/ccs/attestation.rs` already provides
  `canonical_json_hash`, which is the right primitive for plan fingerprinting
  once M3b defines a concrete `PublishPlanMaterial` DTO.

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
  checks, or repository writes. The helper boundary should be `pub(crate)`,
  return `PackagingCommandOutput`, and keep static publish route/gate/write
  logic in `publish.rs`.
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

Implementation order:

1. Add packaging resource helpers, result payload DTOs, and catalog entries to
   `crates/conary-agent-contract`.
2. Add `conary-agent-contract` and `conary-mcp` dependencies to `apps/conary`.
3. Add the thin CLI and dispatch path for the local MCP command. Starting the
   stdio MCP server is read-only; individual tools carry their own risk.
4. Add MCP tool list/call support in `crates/conary-mcp` if the selected
   transport path needs it.
5. Add the packaging agent service and M3a-to-agent projection helpers.
6. Add operation-record helpers for read-by-id, recent list DTOs, event
   extraction, and latest failed packaging operation.
7. Add publish plan/apply with static artifact-form apply only.
8. Add focused tests and docs updates.

Maintainability boundary:

- `apps/conary/src/cli/mod.rs`, `apps/conary/src/commands/cook.rs`,
  `apps/conary/src/commands/publish.rs`, and
  `crates/conary-mcp/src/stateless_http.rs` are already large enough to be
  review signals.
- M3b should add only thin CLI/dispatch branches to existing large files.
- New MCP transport, service, projection, plan registry, and read/diagnostic
  behavior belongs in `packaging_mcp.rs` or a sibling module.
- M3b does not add code to `crates/conary-core/src/ccs/manifest.rs` or
  `crates/conary-core/src/recipe/kitchen/cook.rs`. It consumes existing public
  APIs and records any later decomposition as a separate reviewed slice.

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

The `conary-packaging://` helpers should be first-class helpers in
`crates/conary-agent-contract/src/resource.rs`, not ad hoc strings in the MCP
adapter. Catalog entries for these resources and tools should live in
`crates/conary-agent-contract/src/catalog.rs` so MCP discovery and future docs
share one transport-neutral vocabulary.

## M3a-To-Agent Projection

M3b must define one projection from `PackagingCommandOutput` to
`OperationEnvelope`-based results. MCP responses must not invent a second
packaging result shape.

Status projection:

| M3a status | Agent status |
|------------|--------------|
| `PackagingCommandStatus::Succeeded` | `ok` for inspect/apply, `planned` for plan |
| `PackagingCommandStatus::Failed` | `failed` |

Diagnostic projection:

- The first `PackagingSeverity::Error` diagnostic determines `AgentError`.
- `Info` diagnostics become evidence and may contribute to `data`.
- `Warning` diagnostics become `warnings` and evidence.
- All diagnostic redaction markers become `EvidenceRedaction` entries where
  possible and are also preserved in result `data`.

Diagnostic-code mapping:

| Packaging diagnostic code | Agent error kind |
|---------------------------|------------------|
| `InferenceTrace` | no error by itself |
| `RecipeValidationWarning` | no error by itself |
| `RecipeValidationFailed` | `ValidationFailed` |
| `SourceCacheMiss` | `MissingPrerequisite` |
| `BuildNetworkAccess` | `ValidationFailed` |
| `UnpinnedDependency` | `ValidationFailed` |
| `CommandRiskEvidence` | `ValidationFailed` |
| `CookFailed` | `PartialFailure` |
| `PublishGateFailed` | `ValidationFailed` |
| `ProjectPublishPreflightFailed` | `ValidationFailed` |
| `PublishJsonUnsupported` | `NotSupported` |
| `OperationRecordWriteFailed` | `PartialFailure` |
| `RedactionFailed` | `PartialFailure` |
| `Unknown` | `PartialFailure` |

Evidence projection:

| Diagnostic evidence kind | Agent evidence kind |
|--------------------------|---------------------|
| `Command` | `Command` |
| `Path` | `Resource` |
| `Uri` | `Resource` |
| `Log` | `Log` |
| `Check` | `Check` |
| `Artifact` | `Artifact` |

The result `data` field should include the redacted `PackagingCommandOutput`
or a stable projection of it, including `schema_version`, `operation_id`,
diagnostic codes, event summaries, artifact summaries, and publish lint report
metadata when present. `raw_logs` should remain absent unless a later reviewed
slice defines a bounded, redacted log contract.

## Publish Plan Contract

`conary.packaging.publish.plan` accepts structured inputs only:

- artifact path or project path
- static target destination for artifact form
- optional recipe path for project-form planning
- optional static key directory and publish state file as confirmed plan
  material
- requested mode: `artifact_static`, `project_static`, or `auto`

It does not accept:

- shell command strings
- environment maps
- bearer tokens
- private key contents
- arbitrary extra CLI args
- `force_reinit`, key rotation, `accept_destination_state`, or Remi bearer-token
  options in M3b

`plan_publish` is strictly read-only. It must not generate keys, create key
directories, create repository directories, take publish locks, write state
files, write operation records, or refresh TUF metadata. Static artifact-form
planning in M3b v1 requires an existing readable static repository publication
context. If the destination lacks readable trust state, the plan returns a
missing-prerequisite result and points the user to the normal CLI path or a
later destructive/reinitialization contract.

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

The plan id is a cryptographically random UUIDv4 or equivalent random id. The
confirmation input for M3b is exact repetition of that `plan_id`.

`PublishPlanMaterial` is the canonical fingerprint input:

```text
schema_version
plan_kind
mode
stored_route_enum
normalized_artifact_or_project_path
artifact_sha256
artifact_size
artifact_manifest_identity_when_available
normalized_static_target
key_dir_path_when_supplied
state_file_path_when_supplied
selected_options
command_risk_projection
destination_root_key_fingerprint
destination_package_key_hash
accepted_signer_set_hash
publish_policy_digest
metadata_versions_or_watermark
expires_at
```

The fingerprint is `canonical_json_hash(PublishPlanMaterial)`, using the same
canonical JSON hashing primitive already used by attestation and publish-gate
evidence. Path normalization records absolute canonical paths after symlink
resolution; apply must resolve the same input paths again and reject route,
path, or digest mismatch. The stored route enum is compared with the route
re-derived during apply; mismatch is treated as
`UnsafeWithoutConfirmation`.

M3b does not need a persistent plan database. A stdio MCP session may keep an
in-memory plan registry capped at 16 live plans, evicting the oldest expired or
non-applied plan first. Plans expire after 15 minutes by default. If a later
stateless transport needs cross-request plan persistence, it should use a
private `0600`/`0700` store with explicit expiry.

## Publish Apply Contract

`conary.packaging.publish.apply` accepts:

- `plan_id`
- `fingerprint`
- explicit confirmation input when the plan requires it
- no changed publish options in M3b

Apply behavior:

1. Look up the plan in the current MCP session.
2. Reject expired, missing, or unconfirmed plans with
   `AgentErrorKind::UnsafeWithoutConfirmation`.
3. Recompute the fingerprint from stored plan material and compare it to the
   caller-supplied fingerprint. Fingerprint mismatch is
   `AgentErrorKind::UnsafeWithoutConfirmation`.
4. Resolve the planned artifact path again, copy the artifact into a private
   staging directory created mode `0700`, create the staged file mode `0600`,
   compute the staged artifact digest, and compare it to the planned digest.
5. Publish only from the staged artifact path. Gate checks and repository writes
   must use the same staged bytes.
6. Revalidate inputs and rederive the target route. Compare it to the stored
   route enum.
7. Acquire the publish lock if the static publish path requires one, then
   rederive destination trust state: root key identity, active package-key hash,
   accepted signer-set hash, publish policy digest, and metadata
   versions/watermark. Reject stale state with `UnsafeWithoutConfirmation`.
8. Rerun M2 publish gates immediately before any static repository write.
9. Call the service-safe publish helper that preserves `publish.rs` ownership
   and returns `PackagingCommandOutput`.
10. Project the resulting M3a `PackagingCommandOutput` into `ApplyResult`.
11. Include the redacted operation-record reference instead of scraping stdout.

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

For M3b, `conary.packaging.publish.apply` for static artifact-form publish is
`high`, even though the current CLI classifier reports `conary publish` as a
local-state mutation. Static publish mutates repository trust material and can
affect every consumer of that repository. If later slices expose key rotation,
root reinitialization, destination-state downgrade acceptance, or Remi release
publish apply, those tools are `destructive` unless a separate design proves a
narrower risk class.

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
records and event lists. M3b should add helpers for read-by-id, recent-list
DTOs, event extraction, and latest-failed lookup. Missing records return
`MissingPrerequisite` or resource-not-found errors, not raw filesystem errors.

## Error Model

Use `AgentErrorKind` consistently:

- malformed input, invalid mode, invalid target route:
  `ValidationFailed`
- missing artifact or missing record:
  `MissingPrerequisite`
- missing, stale, expired, mismatched, or unconfirmed apply plan:
  `UnsafeWithoutConfirmation`
- Remi apply or project apply when not wired:
  `NotSupported`
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
- plan fingerprints bind route, target, options, artifact identity, destination
  trust state, and expiry
- plan is read-only and does not create key, repository, lock, state, metadata,
  or operation-record files
- apply stages artifact bytes privately and publishes from the staged artifact
- pure stdio relies on the local process boundary; any later Unix-socket,
  local HTTP, or stateless transport must add explicit peer authentication
  such as same-user credential checks where the platform supports them

The MCP response should include redaction metadata so agents can distinguish
"not known" from "known but intentionally hidden."

## Testing Strategy

Contract tests:

- `cargo test -p conary-agent-contract`
- resource helper URI stability
- packaging tool catalog entries
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
- publish plan returns high risk and confirmation for static artifact apply
- publish plan does not create key dirs, repo dirs, locks, state files, metadata,
  or operation records
- publish apply refuses missing, expired, or mismatched plans
- publish apply refuses changed options
- publish apply rejects changed artifact bytes after plan
- publish apply stages the artifact privately and publishes the staged bytes
- publish apply rejects destination trust-state drift after plan
- publish apply reruns static artifact gate checks
- static artifact-form MCP gate refusal returns the same `PublishLintReport`
  evidence as CLI `publish --json`
- redaction leak tests for paths, credentials, bearer tokens, private key paths,
  and operation records

Create the new focused integration target as
`apps/conary/tests/packaging_m3b.rs`.

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
- Artifact bytes are staged privately and bound to the confirmed plan.
- Destination trust state is disclosed, fingerprinted, and rechecked after lock.
- `plan_publish` is read-only.
- Gate refusal returns structured evidence and does not publish.
- Project-form and Remi apply are explicit unsupported/unavailable paths until
  wired.
- Static artifact publish apply is `high` risk, and risk is never lowered by
  MCP.
- M3a diagnostics have one explicit projection into agent envelopes.
- Operation records are read from the private M3a store.
- No public unauthenticated listener is added.
- No shell/env/token/key passthrough is accepted.

## Ready For Planning

M3b is ready for implementation planning once this spec is reviewed. The plan
should split the work into transport-neutral contract additions, MCP adapter
tool support, CLI packaging agent service methods, publish plan/apply wiring,
focused tests, and docs/ledger updates.
