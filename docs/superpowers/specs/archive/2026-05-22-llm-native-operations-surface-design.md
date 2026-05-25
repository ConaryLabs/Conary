---
last_updated: 2026-05-22
revision: 4
summary: Draft design for making Conary's operations surfaces explicitly LLM-native, with final review fixes for MCP draft and rmcp readiness boundaries
---

# LLM-Native Operations Surface: Design Spec

**Date:** 2026-05-22
**Status:** Draft design for user review
**Goal:** Turn Conary's existing MCP and automation-adjacent surfaces into a
coherent, future-facing agent operations model for developers and operators,
starting with Remi, `conary-test`, and a small local developer bootstrap path.

---

## Purpose

Conary should treat LLM operation as a first-class product capability, not as a
collection of sidecar MCP endpoints or unfinished AI commands. The immediate
audience is a developer or operator who wants to use Conary, inspect repository
and validation state, run tests, inspect evidence, and recover from failures
with an assistant in the loop.

The longer-term product idea is larger: Conary can become the first package
manager designed around a structured agent operations contract. The first
implementation slice should prove that idea on the existing operations surface
before widening into end-user package and system management.

Backward compatibility is not a product constraint for this work, but Conary
does have a documented authenticated Remi MCP endpoint. The accurate constraint
is that Conary has no stable third-party MCP schema contract, no known external
client compatibility promise, and no need to preserve stale tool names or raw
response shapes. Migration should preserve or deliberately redirect the
production endpoint while allowing the tool/resource/prompt surface underneath
it to be replaced.

## Product Position

The product claim is not "Conary has AI search" or "Conary exposes MCP tools."
The product claim is:

> Conary exposes package, repository, test, and system operations through
> machine-readable state, explicit plans, risk-aware mutation boundaries,
> durable evidence, and recovery paths that an LLM can operate safely.

MCP is one transport and discovery mechanism for this contract. The durable
Conary design should be transport-agnostic enough that CLI JSON, HTTP routes,
MCP tools, MCP resources, MCP prompts, and future daemon APIs can share the same
vocabulary.

## Protocol Strategy

MCP should be the first implementation transport, not an unquestioned
architectural bet. The Conary-owned contract is the inspect, plan, verify,
apply, explain, and recover vocabulary. MCP, OpenAPI, JSON-RPC, A2A, CLI JSON,
and future SDK integrations are ways to expose that contract.

### Candidate Protocols

**MCP:** Best first fit for exposing Conary capabilities directly to LLM hosts.
MCP has separate primitives for prompts, resources, and tools, which maps well
onto Conary's need to separate read-only state, workflow guidance, and mutation
tools. MCP tools also support structured content and output schemas. OpenAI's
current docs describe remote MCP servers as a supported way to connect models
to additional tools and knowledge in ChatGPT apps, deep research, and API
integrations.

As of 2026-05-22, the important MCP target is the current MCP draft stateless
direction associated with the 2026-07-28 release candidate, not the older
session-oriented 2025-era implementation model. The draft docs currently show
`DRAFT-2026-v1` as the protocol-version value, but Conary should treat the
draft as moving until release and re-verify the final version token before
shipping a live adapter. The draft removes protocol-level sessions,
`Mcp-Session-Id`, and the `initialize`/`initialized` handshake; adds
`server/discover`; moves protocol, client identity, and client capability data
into per-request `_meta`; and requires routing-friendly HTTP headers such as
`Mcp-Method` and `Mcp-Name`. That direction fits Conary better than sticky
sessions: Remi and `conary-test` should be able to sit behind ordinary HTTP
infrastructure, with any cross-call state represented by explicit run IDs,
handles, artifact IDs, or resource URIs.

**OpenAPI:** Best companion contract for HTTP surfaces and non-agent clients.
OpenAPI is mature, language-neutral, YAML/JSON-based, and useful for client
generation, infrastructure, and tests. It should remain the source of truth for
REST admin routes where HTTP is the product interface, but by itself it does
not express prompt templates, resources, user confirmation, or model-controlled
tool discovery as directly as MCP.

**JSON-RPC:** Useful as a low-level request/response substrate, and already
underlies MCP-style calls. It is too small to be the product-level answer on
its own because Conary would need to invent discovery, resource semantics,
prompt templates, risk labels, and approval conventions around it.

**A2A:** Worth watching for future multi-agent scenarios. Google positions A2A
as a protocol for independent agents to discover and communicate with each
other, with Agent Cards, task management, artifacts, and long-running
collaboration. That is not the first Conary need. Conary is currently exposing
tools and operational state to an assistant, not publishing an autonomous
Conary agent that negotiates tasks with other agents. A2A may become useful
later if Conary grows a hosted "repository operator agent" or "system recovery
agent" that other agents call as a peer.

**Provider-native function calling or Agents SDKs:** Useful integration
targets, not the durable product protocol. A custom OpenAI, Anthropic, Google,
or local-agent harness can wrap Conary operations, but the repo should avoid
making the core contract depend on one model vendor or one runtime.

### Protocol Decision

For the first implementation slice:

- Define Conary's agent operation vocabulary in transport-neutral Rust types
  and docs before expanding any transport.
- Expose the first agent-facing surface through MCP because it has the best
  current fit for tools, resources, prompts, discovery, and LLM-host adoption.
- Target the current MCP draft stateless direction associated with the
  2026-07-28 release candidate. Treat the current `DRAFT-2026-v1`
  protocol-version value as a draft token to verify before live adapter work.
  Do not add new session-based MCP behavior while the Rust SDK and final
  protocol support are still moving.
- Keep REST/OpenAPI compatibility possible through the contract types, but do
  not make OpenAPI generation part of the first implementation slice.
- Do not build A2A yet; add a future follow-on only if Conary starts exposing a
  remote agent as a peer service rather than an operations tool server.
- Do not bake OpenAI-specific harness behavior into product code, even though
  current OpenAI APIs can consume remote MCP servers and local function tools.

As of 2026-05-22, local source inspection shows the resolved `rmcp 1.6.0`
dependency does not implement the target stateless draft: it still exposes the
`initialize` handshake, `Mcp-Session-Id`, `RoleServer`, `ServerHandler`, and
session-manager based Streamable HTTP server code. If `rmcp` support lags the
stateless MCP release, the first implementation slice should still aim at the
RC: prepare the transport-neutral contract, prune stale surfaces, and avoid
deepening the current `RoleServer`/`ServerHandler`/local-session shape. A thin
raw HTTP adapter can be considered later if it is smaller than waiting for the
SDK. Building toward the RC and absorbing small release changes is preferable
to adding fresh code for the old session model.

Primary references for this decision:

- <https://blog.modelcontextprotocol.io/tags/mcp/>
- <https://modelcontextprotocol.io/specification/draft/changelog>
- <https://modelcontextprotocol.io/specification/draft/server/discover>
- <https://modelcontextprotocol.io/specification/draft/basic/transports>
- <https://modelcontextprotocol.io/specification/draft/server/tools>
- <https://modelcontextprotocol.io/specification/draft/server/resources>
- <https://modelcontextprotocol.io/specification/draft/server/prompts>
- <https://modelcontextprotocol.io/docs/develop/clients/client-best-practices>
- <https://developers.openai.com/api/docs/mcp>
- <https://developers.openai.com/api/docs/guides/tools>
- <https://www.openapis.org/what-is-openapi>
- <https://www.jsonrpc.org/specification>
- <https://developers.googleblog.com/en/developers-guide-to-ai-agent-protocols/>
- <https://adk.dev/a2a/intro/>
- <https://developers.googleblog.com/en/a2a-a-new-era-of-agent-interoperability/>

### MCP Draft Compliance Notes

Implementation planning must treat the MCP adapter as a compliance boundary,
not as casual `axum` glue. This checklist is aspirational for the target
stateless adapter until either `rmcp` supports the draft or Conary builds a raw
HTTP adapter. If Conary uses a thin raw HTTP adapter before `rmcp` fully
supports the target draft, that adapter must implement and test at least these
draft requirements:

- every JSON-RPC message arrives as a new HTTP `POST` to the MCP endpoint
- `Accept` includes both `application/json` and `text/event-stream`
- every request carries `MCP-Protocol-Version`, `Mcp-Method`, and, for
  `tools/call`, `resources/read`, and `prompts/get`, `Mcp-Name`
- the `MCP-Protocol-Version` header matches
  `_meta.io.modelcontextprotocol/protocolVersion`
- client info and capabilities come from per-request `_meta`, not connection
  setup
- `Origin` is validated for HTTP transports, especially local servers
- `server/discover` is supported before ordinary capability use
- old HTTP GET or SSE session semantics are not required for first-slice work
- list and read results include cache metadata such as `ttlMs` and `cacheScope`
  where the draft requires them

Target MCP tool-result mapping for the stateless adapter should be explicit.
This table is not current behavior for the existing `rmcp 1.6.0` adapter:

| MCP outcome | Conary contract use |
| --- | --- |
| `resultType: "complete"`, `isError: false` | Successful `InspectResult`, `PlanResult`, `VerifyResult`, `ApplyResult`, `ExplainResult`, or `RecoverResult`; `structuredContent` conforms to the matching `outputSchema`. |
| `resultType: "complete"`, `isError: true` | Tool executed but the operation failed as a domain outcome, such as validation failure or remote unavailability; structured content conforms to the matching error-capable contract schema. |
| JSON-RPC error | Protocol, authorization, invalid request, invalid schema, or unknown method/tool/resource/prompt failures. |
| `resultType: "input_required"` with `inputRequests` / `inputResponses` | Confirmation or missing client input that must be supplied before the operation can complete. |

Parameterized read-only state should prefer MCP resource templates before
tools. Use tools for expensive computation, true command semantics, or actions
with side effects.

## Audiences

### Maintainer And Operator

This is the current maintainer/operator persona. The operator needs safer, less
manual remote operations for Remi and `conary-test`:

- inspect Remi health, repositories, federation peers, audit events, canonical
  mapping status, and chunk storage
- run and inspect `conary-test` suites
- understand failing test evidence quickly
- run deployment and service-management paths only through clear, auditable
  mutation boundaries
- inspect whether fixture or package publication paths are available, while
  leaving publication mutations out of the first local bootstrap slice

### Developer Trying Conary

A new developer who has cloned Conary should be able to ask an assistant to
verify that the local development environment works:

- inspect local prerequisites and explain missing dependencies
- validate repository and test configuration that already exists in the repo
- build the minimal packages needed by the smoke path, if that is already part
  of the test harness
- run a smoke validation suite
- inspect logs and artifacts
- receive machine-readable next steps when something is missing

This path should be small and honest. It does not need to solve full production
hosting, package publishing, multi-node federation, or every distro matrix
combination.

### Future End User

The same pattern should later extend to local package and system management:

- inspect installed state, selected generation, trust state, and package
  provenance
- plan a model or package operation
- explain risk, changes, rollback, and native package-manager authority
  boundaries
- apply only after explicit approval
- verify the resulting system and capture evidence
- recover from partial or failed operations

The first slice should not implement this full end-user surface, but it should
avoid design choices that would block it.

## Agent Operations Contract

Every LLM-facing workflow should be expressible through this vocabulary:

- **Inspect:** Read state without mutating anything.
- **Plan:** Produce a bounded proposed action with prerequisites, affected
  resources, risk, and verification commands.
- **Verify:** Run checks and return structured evidence.
- **Apply:** Mutate state only through explicit, auditable commands or tools.
- **Explain:** Summarize status, failures, risks, and next actions for a human.
- **Recover:** Offer safe cleanup, rollback, retry, or escalation paths.

This vocabulary should appear in tool names, JSON fields, CLI output, docs, and
test assertions where it clarifies behavior.

## Current Starting Point

The current codebase has real but uneven foundations:

- `crates/conary-mcp` provides shared MCP helper glue, not a product-level
  agent contract.
- `apps/remi/src/server/mcp.rs` exposes useful admin, federation, test-data,
  chunk garbage-collection, and canonical-map tools through the same service
  layer as HTTP admin routes.
- `apps/conary-test/src/server/mcp.rs` exposes run control, logs, artifacts,
  images, deploy, fixture publishing, deployment status, and WAL flushing.
- `apps/conary-test/src/server/mcp.rs` also includes several command-wrapper
  tools that return long text logs and mutate deployment state.
- `docs/llms/README.md` correctly says there is no active OpenAI/LLM prompt
  harness in the repository today.
- `apps/conary/src/commands/automation.rs` still has experimental AI commands
  that print `[NOT IMPLEMENTED]`.
- Some docs describe MCP as an operations preference but do not define the
  larger Conary agent contract.

These are raw materials, not constraints.

## Design Principles

### Future Over Compatibility

Remove stale, fake, or weak surfaces when they block the better model. Existing
MCP tool names, counts, response text, and experimental CLI commands do not
need compatibility preservation.

### Resources Before Actions

An assistant should read state through resources and query tools before it asks
to mutate anything. Tests, repository configuration, audit summaries, deploy
status, local bootstrap state, and package/service health should be available
without pretending that observation is an action.

### Plans Before Mutations

Mutating workflows should have a plan or dry-run form. Where a workflow cannot
offer a meaningful plan, the tool description and response must say what it can
and cannot guarantee.

### Structured Evidence Over Logs

Long command text is useful as an attachment or detail field, not as the main
interface. Primary responses should include status, IDs, paths, command exit
codes, changed resources, warnings, and next actions as structured JSON.

### Shared Service Logic

MCP tools should delegate to the same service layer used by CLI and HTTP
surfaces. Tool handlers should not grow their own business logic, deployment
scripts, or parsing rules unless the behavior is genuinely MCP-specific.

### Honest Capability Boundaries

If a workflow depends on missing credentials, inactive Forge capacity, local
KVM, a disabled service, or an unimplemented product feature, return a typed
unavailable/deferred response with remediation hints. Do not return successful
empty stubs.

### Human Approval Is A Feature

The LLM-native surface should make confirmation, risk labels, and audit trails
clear. The goal is not blind autonomy; it is making safe, high-context
operation easy.

### Confirmation Is A Contract

High-risk and destructive operations should use a plan-then-apply boundary.
`PlanResult` should produce a stable plan ID, the affected resource URIs, the
required confirmation level, the risk rationale, and the verification command
or resource that will prove the result. `ApplyResult` for medium, high, or
destructive operations should require explicit confirmation input that matches
the plan ID and current operation fingerprint. Replayed, stale, or missing
confirmation should return `unsafe_without_confirmation`, not attempt the
mutation.

### RC-First, Session-Free Transport

New MCP work should target the current MCP draft stateless model associated
with the 2026-07-28 release candidate. Treat the draft's current
`DRAFT-2026-v1` protocol-version token as non-final until release. Do not rely on
`Mcp-Session-Id`, per-connection tool/resource/prompt lists, or implicit
transport sessions. Any state that must survive across calls should be
represented by explicit run IDs, plan IDs, artifact IDs, resource URIs, or
bounded server-minted handles in the Conary contract.

### Context Budget Is A Product Constraint

An LLM-operable package manager should not require the model to load every
possible operation up front. Conary should avoid a single giant MCP server full
of loosely described tools. Prefer resources for read-heavy state, concise tool
descriptions that say when a tool should be used, deterministic listing order,
cacheable resource/tool metadata, and separate surfaces when capability groups
diverge. As a working guardrail, if one MCP surface grows beyond roughly
fifteen active tools, or tool definitions consume more than a small share of the
target context window, implementation planning must either justify that count
with measured tool-definition size or introduce progressive discovery, a
smaller capability catalog, or a split such as read-only, admin, test-runner,
and local bootstrap surfaces.

### Prompts Are Not Magic Orchestrators

Prompts should not pretend they can enforce a multi-step workflow by instruction
alone. A prompt may frame the task for the assistant, but deterministic checks
should run server-side where possible and return structured state for the model
to reason over. Confirmation gates should be represented in the contract and,
when the target MCP version supports it cleanly, mapped to `input_required`
results with `inputRequests` / `inputResponses` rather than relying on prose
that asks the model to pause.

### Contract Schemas Are The Source

Conary's response vocabulary should be implemented first as transport-neutral
Rust types, not as `rmcp` wrappers. MCP `outputSchema` should be derived from or
manually matched to those contract types. The goal is one semantic result shape
adapted into MCP structured content, CLI JSON, HTTP JSON, and future daemon
APIs, not nested Conary envelopes wrapped inside unrelated MCP-specific
envelopes.

## First Implementation Slice

The first implementation slice should be called **LLM-native operations
surface**. It covers remote operations plus local developer bootstrap, but it
is staged so Conary does not spend new effort on the old session-based MCP
shape.

### 0. Prepare For The MCP RC

Before expanding the live MCP surface:

- Track the current MCP draft stateless direction associated with the
  2026-07-28 release candidate and treat it as the target. Re-verify the final
  protocol-version token before live adapter work; the draft docs currently use
  `DRAFT-2026-v1`.
- Record that the workspace requirement `rmcp = "1.1"` currently resolves to
  `rmcp 1.6.0` in `Cargo.lock`, and that local source inspection shows this
  resolved SDK does not implement the target stateless draft.
- Record the current
  `RoleServer`/`ServerHandler`/local session-manager usage as legacy transport
  facts, not architectural commitments.
- Avoid adding new MCP resources, tools, prompts, or list/discovery behavior on
  the session-based path. Short-lived implementation probes are acceptable only
  when they help delete, replace, or isolate existing legacy transport code.
- Make the first milestone contract-only plus inventory/prune unless the
  stateless adapter decision is settled.
- Plan an `rmcp` upgrade when stateless support exists. If the SDK lags and the
  raw adapter would be small, consider implementing Streamable HTTP directly
  only if it satisfies the MCP draft compliance checklist above.
- Delete the experimental `conary automation ai ...` commands if implementation
  review confirms they still only emit not-implemented behavior.

### 1. Transport-Neutral Contract Crate

Create `crates/conary-agent-contract` as the source of truth for LLM-facing
operation results. This crate must not depend on `rmcp` or any MCP-specific
type. It should define serde-serializable Rust types for:

- `InspectResult`
- `PlanResult`
- `VerifyResult`
- `ApplyResult`
- `ExplainResult`
- `RecoverResult`
- `OperationStatus`: `ok`, `planned`, `running`, `unavailable`, `failed`,
  `partial`
- `RiskLevel`: `read_only`, `low`, `medium`, `high`, `destructive`
- `AgentError`: `missing_prerequisite`, `not_supported`, `deferred`,
  `unsafe_without_confirmation`, `remote_unavailable`, `validation_failed`,
  `partial_failure`
- shared evidence, changed-resource, warning, confirmation, and next-action
  fields

Every result type should use a shared minimal envelope:

- `operation`: stable operation name, such as `remi.audit.purge.plan`
- `status`: `OperationStatus`
- `subject`: primary resource URI or operation subject
- `risk`: `RiskLevel`
- `summary`: short human-readable status
- `changed`: resource URIs changed or planned to change
- `evidence`: structured evidence items with kind, URI/path/ID, command summary,
  exit code where relevant, and optional artifact metadata
- `warnings`: bounded human-readable warnings
- `next_actions`: bounded follow-up actions with stable action labels
- `confirmation`: optional `ConfirmationRequirement` with plan ID, reason,
  required input shape, and expiry/fingerprint when applicable
- `raw_logs`: optional detail field or artifact link, never the primary result

Canonical resource URI patterns should be defined with the contract crate before
the MCP adapter is expanded. Initial patterns should include:

- `conary://remi/health`
- `conary://remi/repositories/{name}`
- `conary://remi/federation/peers/{peer_id}`
- `conary://remi/audit/summary`
- `conary://remi/chunks/stats`
- `conary-test://suites/{suite_id}`
- `conary-test://runs/{run_id}`
- `conary-test://runs/{run_id}/artifacts/{artifact_id}`
- `conary-local://bootstrap/status`

The contract crate should derive JSON Schema 2020-12 with `schemars` where that
fits the desired schema. Any handwritten MCP `outputSchema` must have snapshot
or round-trip tests proving parity with the Rust contract types.

`crates/conary-mcp` should become an adapter that maps these types into MCP
structured content and `outputSchema`. CLI JSON, HTTP JSON, and future daemon
APIs should be able to import the contract crate without pulling in MCP.

### 2. Local Bootstrap Inspection Service

Implement the local developer bootstrap inspection service before exposing the
bootstrap resource or prompt. Initial ownership should live in
`apps/conary-test`, because the first proof loop is a test-harness health and
smoke-validation path, not package installation.

The first command shape should be planned around
`conary-test bootstrap check --json`, with room to rename only if the
implementation plan documents an equivalent user-facing path. It should:

- inspect required tools: Rust/Cargo, Podman or Docker API access, SQLite, and
  the repository paths that `conary-test` expects
- inspect optional tools: `/dev/kvm`, QEMU, and SSH helpers, reporting them as
  optional unless the selected smoke suite needs them
- run no network credential checks and publish no packages
- validate `CONARY_TEST_CONFIG`, `CONARY_TEST_MANIFESTS`, and default path
  resolution
- run `cargo run -p conary-test -- list` or the equivalent manifest parser
  inventory without starting containers
- select a default non-QEMU smoke candidate, initially `phase1-core` on
  `fedora44`, only when container prerequisites are available
- write JSON evidence under a repo-local ignored path such as
  `target/conary-agent/bootstrap/`
- report missing prerequisites as `missing_prerequisite` without pretending the
  bootstrap succeeded
- keep the first proof loop bounded to a configurable timeout, with a planning
  target of under twenty minutes after binaries are already built

### 3. Resources First

Add resources for stable state that assistants should inspect before invoking
tools:

- Remi health
- Remi repositories
- Remi federation peers
- Remi audit summary
- Remi canonical mapping status
- Remi chunk storage stats
- Remi test health
- recent `conary-test` runs
- known `conary-test` suites
- `conary-test` deployment status
- `conary-test` local service health
- local developer bootstrap status

Resources should be read-only, cheap enough to call during orientation, and
return draft-required cache metadata such as `ttlMs` and `cacheScope`.
Parameterized read-only state should prefer resource templates first. Read-only
operation-shaped tools should be reclassified as resources unless they require
expensive computation, true command semantics, or side effects.

### 4. Tool Audit And Mutation Tools

Review all active MCP and AI-adjacent surfaces:

- `crates/conary-mcp/src/lib.rs`
- `apps/remi/src/server/mcp.rs`
- `apps/remi/src/server/admin_service.rs`
- `apps/conary-test/src/server/mcp.rs`
- `apps/conary-test/src/server/service.rs`
- `apps/conary-test/README.md`
- `apps/conary/src/cli/automation.rs`
- `apps/conary/src/cli/mod.rs`
- `apps/conary/src/commands/automation.rs`
- `crates/conary-core/src/automation/mod.rs`
- `crates/conary-core/src/model/parser.rs`
- `AGENTS.md`
- `docs/ARCHITECTURE.md`
- `docs/INTEGRATION-TESTING.md`
- `docs/operations/infrastructure.md`
- `docs/llms/README.md`
- `docs/llms/subsystem-map.md`
- `README.md`
- `docs/conaryopedia-v2.md`

Classify each surface as keep, reshape, replace, or remove. The review should
prefer the new contract over preserving historical names.

Expected decisions:

- Keep Remi test/repo/federation/canonical/chunk tools that share service
  logic, but normalize response shape and docs.
- Explicitly classify Remi token-creation, token-deletion, and audit-purge
  tools. They should be `high` or `destructive`, require plan-then-apply
  confirmation, or be removed from the first MCP mutation surface.
- Keep `conary-test` run-control tools that mutate or launch work, but move
  evidence inspection and status reads into resources where possible.
- Reshape or remove deploy/build/restart/fixture tools. Any tool that remains
  in the first slice must return structured results and use shared service
  orchestration where possible.
- Remove experimental `conary automation ai ...` commands unless the first
  slice replaces them with honest behavior.
- Classify existing AI-assist configuration and model-schema fields as keep
  with deferred wording, reshape into the new contract, or remove.
- Replace "MCP-first operations" wording with the broader agent operations
  contract.

Every mutation tool should return the matching contract type, usually
`PlanResult`, `VerifyResult`, or `ApplyResult`, with risk, changed resources,
evidence, warnings, and next actions. Every MCP mutation tool should publish an
`outputSchema` matching the contract type. Long raw command output should move
to optional detail fields or artifacts, not the main result.

### 5. Hybrid Prompts

Add only three prompts in the first slice:

- inspect Remi health
- debug a failing test
- bootstrap local dev environment

Each prompt should combine a concise task frame with deterministic server-side
inspection where possible. For example, "debug a failing test" should fetch the
run resource and artifact summary before presenting the assistant with the
evidence. "bootstrap local dev environment" should inspect prerequisites and
test inventory before suggesting a mutation. Prompts should not contain volatile
host secrets, and they should not be the only place where workflow correctness
lives.

### 6. Local Developer Bootstrap Path

Add a small local path that an assistant can drive from a clean checkout for
one audience: a developer who has cloned Conary and wants to verify that the
local development loop works. The first implementation should treat
`apps/conary-test` as the owner and should build around the bootstrap
inspection service defined above.

- inspect prerequisites and report missing local dependencies
- validate local repository and test configuration
- run a focused build check for the packages needed by the smoke path
- list available integration suites
- run one small non-QEMU smoke validation only when container prerequisites are
  present
- emit structured JSON describing state, evidence, and next steps

This path should avoid pretending to install a full production environment. It
is a developer bootstrap and proof loop, not repository publishing or end-user
system management.

### 7. Docs And Positioning

Document the agent operations model as a real Conary feature:

- README should introduce Conary as LLM-operable without overclaiming end-user
  local package autonomy before it exists.
- `docs/llms/README.md` should remain a contributor-assistant map, but point to
  the product-facing agent operations doc once it exists.
- `docs/operations/infrastructure.md` should distinguish MCP as a transport
  from the broader contract.
- `docs/conaryopedia-v2.md` should stop describing stale AI automation as real
  if the implementation is not real.

## Explicit Non-Goals For The First Slice

- Do not build a full local end-user package-management agent.
- Do not add an OpenAI-specific runtime harness.
- Do not require cloud model credentials for normal Conary operations.
- Do not preserve old MCP tool names if a cleaner contract requires change.
- Do not add new session-based MCP surface area for the old protocol shape.
- Do not implement OpenAPI generation in this slice; keep the contract ready
  for it later.
- Do not implement autonomous package mutation.
- Do not include package publishing or fixture publication in the local
  developer bootstrap path.
- Do not expand beyond the three first prompts unless implementation evidence
  shows one is necessary for acceptance.
- Do not implement A2A before Conary has a real peer-agent use case.
- Do not rework Remi, `conary-test`, and `conaryd` into one monolithic service.
- Do not expose host-local secrets or personal access notes in tracked docs.

## Error Handling

LLM-facing operations should use typed failures and remediation hints:

- `missing_prerequisite`: dependency, service, credential, KVM, or local config
  is absent
- `not_supported`: product feature is intentionally unavailable in this slice
- `deferred`: planned future work, not current behavior
- `unsafe_without_confirmation`: mutation requires explicit human approval
- `remote_unavailable`: Remi, Forge, or other remote service could not be
  reached
- `validation_failed`: check ran and produced negative evidence
- `partial_failure`: some steps succeeded and others failed; use
  `status = partial` for the overall operation state

Responses should include enough evidence for an assistant to decide whether to
retry, ask the user, run a narrower diagnostic, or stop.

## Security And Authorization

MCP resources, tools, and prompts must inherit the same authentication and
authorization boundary as the owning HTTP or CLI surface. Remi admin resources
and tools remain bearer-authenticated admin operations unless the implementation
plan explicitly carves out a narrower read-only scope. Discovery must not reveal
tools or resources beyond the caller's granted scope.

The first slice should add tests for denied access to sensitive resources and
mutations. Token creation, token deletion, audit purge, deploy/restart, fixture
publication, and any host-level cleanup action are high-risk or destructive
until proven otherwise. Local bootstrap may run without cloud credentials, but
it must not expose host-local secrets, bearer tokens, SSH identities, or ignored
personal access notes.

## Testing Strategy

The implementation plan should include focused tests across these areas:

- unit tests for contract serialization, risk labels, and error mapping
- service-layer tests for local bootstrap inspection and structured operation
  summaries
- schema tests that confirm MCP `outputSchema` matches the contract types used
  by each tool
- MCP/server tests for resource, tool, and prompt registration against the
  selected stateless transport path, or contract-level stateless readiness when
  live transport registration is deferred
- authorization and confirmation tests for sensitive resources and mutation
  tools
- catalog/context-budget checks that report tool count and approximate
  tool-definition size for each MCP surface

Documentation and inventory checks should run when active docs change.

Baseline verification for the first slice:

```bash
cargo fmt --check
cargo test -p conary-agent-contract
cargo test -p conary-mcp
cargo test -p remi mcp
cargo test -p conary-test mcp
cargo test -p conary-test bootstrap
cargo run -p conary-test -- list
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

The implementation may adjust focused package tests once the exact touched
files are known. If bootstrap ownership moves out of `conary-test`, replace the
bootstrap command above with the owning package's focused tests. Any removed
experimental AI surface should have tests or help snapshots updated so stale
commands do not remain visible by accident.

## Acceptance Criteria

The first slice is complete when:

- active docs describe an LLM-native operations model, not just an MCP endpoint
  inventory
- `crates/conary-agent-contract` exists, has no MCP dependency, and is the
  source for response semantics
- stale or fake AI command surfaces are removed or replaced with honest
  behavior
- new MCP work targets the current MCP draft stateless direction associated
  with the 2026-07-28 release candidate, with final protocol-version token
  verification before live adapter work and no new reliance on protocol
  sessions or sticky connection state
- Remi and `conary-test` MCP surfaces expose read-only resources for key state
- mutating MCP tools return structured contract results with status, risk,
  evidence, and next actions
- medium, high, and destructive mutation tools require plan-then-apply
  confirmation
- each MCP surface either meets the context-budget guardrail or documents a
  split/progressive-discovery plan with measured catalog size
- resource, tool, and prompt list/read results include draft-required cache
  metadata where applicable
- first-slice prompts are limited to Remi health inspection, failing-test
  debugging, and local developer bootstrap
- local developer bootstrap can inspect prerequisites and run a small proof loop
  with machine-readable output
- tests cover contract serialization/schema shape, tool/resource/prompt
  registration, and the local bootstrap path
- no active docs claim end-user autonomous package management is implemented
  before it exists

## Open Design Decisions For Implementation Planning

- Whether the stateless MCP adapter should wait for `rmcp` support or use a thin
  raw HTTP adapter temporarily.
- Whether the initial `conary-test bootstrap check --json` command should be
  paired with a separate `bootstrap smoke` command or one command with modes.
- Whether `phase1-core` on `fedora44` is the right default smoke suite after
  timing and prerequisite checks.
- How much of deploy/build/restart should remain MCP-accessible versus being
  moved to CLI-only workflows with MCP prompt guidance.
- Whether generated JSON Schema is sufficient for every MCP `outputSchema`, or
  whether a small number of handwritten schemas need parity tests.

## Out-Of-Scope Follow-On Notes

These are product roadmap notes, not first-slice implementation scope:

1. **Local Package-Agent Surface:** apply the same contract to `conary` package
   operations: inspect, plan, explain, apply, verify, and recover.
2. **conaryd Agent Boundary:** expose daemon job queues, package-job progress,
   event streams, and recovery state through the same vocabulary.
3. **Repository Publisher Experience:** make creating and operating a personal
   Conary repository a polished assistant-driven workflow.
4. **End-User System Assistant:** support local system questions and planned
   package/model changes without autonomous mutation.
5. **Peer Agent Bridge:** revisit A2A only if Conary exposes a long-running
   autonomous repository, validation, or recovery agent that other agents call
   across a network.
