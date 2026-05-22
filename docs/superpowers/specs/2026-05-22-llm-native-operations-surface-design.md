---
last_updated: 2026-05-22
revision: 1
summary: Draft design for making Conary's operations surfaces explicitly LLM-native, starting with Remi, conary-test, and local developer bootstrap workflows
---

# LLM-Native Operations Surface: Design Spec

**Date:** 2026-05-22
**Status:** Draft design for user review
**Goal:** Turn Conary's existing MCP and automation-adjacent surfaces into a
coherent, future-facing agent operations model for developers and operators,
starting with Remi, `conary-test`, and a small local repository bootstrap path.

---

## Purpose

Conary should treat LLM operation as a first-class product capability, not as a
collection of sidecar MCP endpoints or unfinished AI commands. The immediate
audience is a developer or operator who wants to use Conary, set up a package
repository, publish test content, run validation, inspect evidence, and recover
from failures with an assistant in the loop.

The longer-term product idea is larger: Conary can become the first package
manager designed around a structured agent operations contract. The first
implementation slice should prove that idea on the existing operations surface
before widening into end-user package and system management.

Backward compatibility is not a constraint for this work. The repository is not
serving external MCP clients today, and stale or misleading surfaces may be
removed wholesale when replacement paths are clearer and more future-facing.

## Product Position

The product claim is not "Conary has AI search" or "Conary exposes MCP tools."
The product claim is:

> Conary exposes package, repository, test, and system operations through
> machine-readable state, explicit plans, risk-aware mutation boundaries,
> durable evidence, and recovery paths that an LLM can operate safely.

MCP is one transport and discovery mechanism for this contract. The durable
Conary design should be transport-agnostic enough that CLI JSON, HTTP routes,
MCP tools, MCP resources, and future daemon APIs can share the same vocabulary.

## Protocol Strategy

MCP should be the first implementation transport, not an unquestioned
architectural bet. The Conary-owned contract is the inspect, plan, verify,
apply, explain, and recover vocabulary. MCP, OpenAPI, JSON-RPC, A2A, CLI JSON,
and future SDK integrations are ways to expose that contract.

### Candidate Protocols

**MCP:** Best first fit for exposing Conary capabilities directly to LLM hosts.
The current MCP server model has separate primitives for prompts, resources,
and tools, which maps well onto Conary's need to separate read-only state,
workflow guidance, and mutation tools. MCP tools also support structured
content and output schemas. OpenAI's current docs describe remote MCP servers
as a supported way to connect models to additional tools and knowledge in
ChatGPT apps, deep research, and API integrations.

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
  and docs.
- Expose the first agent-facing surface through MCP because it has the best
  current fit for tools, resources, prompts, discovery, and LLM-host adoption.
- Keep or generate OpenAPI for HTTP surfaces where REST is already the stable
  product boundary.
- Do not build A2A yet; add a future follow-on only if Conary starts exposing a
  remote agent as a peer service rather than an operations tool server.
- Do not bake OpenAI-specific harness behavior into product code, even though
  current OpenAI APIs can consume remote MCP servers and local function tools.

Primary references for this decision:

- <https://modelcontextprotocol.io/specification/2025-06-18/server/index>
- <https://modelcontextprotocol.io/specification/2025-06-18/server/tools>
- <https://developers.openai.com/api/docs/mcp>
- <https://developers.openai.com/api/docs/guides/tools>
- <https://www.openapis.org/what-is-openapi>
- <https://www.jsonrpc.org/specification>
- <https://developers.googleblog.com/en/developers-guide-to-ai-agent-protocols/>
- <https://adk.dev/a2a/intro/>
- <https://developers.googleblog.com/en/a2a-a-new-era-of-agent-interoperability/>

## Audiences

### Maintainer And Operator

This is the current user. The operator needs safer, less manual remote
operations for Remi and `conary-test`:

- inspect Remi health, repositories, federation peers, audit events, canonical
  mapping status, and chunk storage
- run and inspect `conary-test` suites
- publish fixtures and packages
- understand failing test evidence quickly
- run deployment and service-management paths only through clear, auditable
  mutation boundaries

### Developer Trying Conary

A new developer should be able to ask an assistant to set up a small local
Conary repository and validation loop:

- initialize local repository configuration
- create or verify a local Remi-like package service path, when available
- publish fixture packages or a small sample package
- run a smoke validation suite
- inspect logs and artifacts
- receive machine-readable next steps when something is missing

This path should be small and honest. It does not need to solve full production
hosting, multi-node federation, or every distro matrix combination.

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

## First Implementation Slice

The first implementation slice should be called **LLM-native operations
surface**. It covers remote operations plus local developer bootstrap.

### 1. Inventory And Prune

Review all active MCP and AI-adjacent surfaces:

- `crates/conary-mcp/src/lib.rs`
- `apps/remi/src/server/mcp.rs`
- `apps/remi/src/server/admin_service.rs`
- `apps/conary-test/src/server/mcp.rs`
- `apps/conary-test/src/server/service.rs`
- `apps/conary/src/cli/automation.rs`
- `apps/conary/src/commands/automation.rs`
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
- Keep `conary-test` run-control and evidence-inspection tools, but separate
  read resources from action tools.
- Reshape deploy/build/restart/fixture tools so they return structured results
  and use shared service orchestration where possible.
- Remove or hide experimental `conary automation ai ...` commands unless the
  first slice makes them honest and useful.
- Replace "MCP-first operations" wording with the broader agent operations
  contract.

### 2. Shared Agent Response Model

Introduce a shared response vocabulary, probably in `crates/conary-mcp` or a
new transport-neutral module if the vocabulary should also serve CLI and HTTP
JSON:

- `status`: `ok`, `planned`, `running`, `unavailable`, `failed`, `partial`
- `operation`: stable operation name
- `risk`: `read_only`, `low`, `medium`, `high`, `destructive`
- `requires_confirmation`: boolean
- `changed`: list of changed resources
- `evidence`: paths, run IDs, artifact IDs, checks, command summaries
- `warnings`: human-readable warnings
- `next_actions`: bounded suggested follow-ups
- `raw_logs`: optional detail for command-heavy workflows

This model should stay small. Do not invent a full orchestration framework in
the first slice.

### 3. MCP Resources

Add resources for stable state that assistants should inspect:

- Remi repositories
- Remi federation peers
- Remi audit summary
- Remi test health
- recent test runs
- known test suites
- `conary-test` deployment status
- `conary-test` local service health
- local developer bootstrap status
- local fixture/package publication status, if available

Resources should be read-only and cheap enough to call during orientation.

### 4. MCP Prompts

Add prompts for common operator/developer workflows:

- set up local developer repository
- run preview validation
- debug a failing test
- inspect Remi health
- publish fixtures or a sample package
- prepare a safe federation change
- summarize recent validation evidence

Prompts should guide the assistant through inspect, plan, verify, apply,
explain, and recover. They should not replace tool schemas or contain volatile
host secrets.

### 5. Local Developer Bootstrap Path

Add a small local path that an assistant can drive from a clean checkout. The
exact command shape can be decided during implementation, but the behavior
should include:

- inspect prerequisites and report missing local dependencies
- initialize or validate local repository/test configuration
- build or publish a minimal sample/fixture package
- run a small smoke validation
- emit structured JSON describing state, evidence, and next steps

This path should avoid pretending to install a full production environment. It
is a developer bootstrap and proof loop.

### 6. Docs And Positioning

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
- Do not implement autonomous package mutation.
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
- `partial`: some steps succeeded and others failed

Responses should include enough evidence for an assistant to decide whether to
retry, ask the user, run a narrower diagnostic, or stop.

## Testing Strategy

The implementation plan should include focused tests at three levels:

- unit tests for response model serialization, risk labels, and error mapping
- service-layer tests for local bootstrap inspection and structured operation
  summaries
- MCP/server tests for tool/resource/prompt registration and expected response
  shapes

Documentation and inventory checks should run when active docs change.

Baseline verification for the first slice:

```bash
cargo fmt --check
cargo test -p conary-mcp
cargo test -p remi mcp
cargo test -p conary-test mcp
cargo test -p conary --features experimental automation
cargo run -p conary-test -- list
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

The implementation may adjust focused package tests once the exact touched
files are known. Any removed experimental AI surface should have tests or help
snapshots updated so stale commands do not remain visible by accident.

## Acceptance Criteria

The first slice is complete when:

- active docs describe an LLM-native operations model, not just an MCP endpoint
  inventory
- stale or fake AI command surfaces are removed, hidden, or made explicitly
  honest
- Remi and `conary-test` MCP surfaces expose read-only resources for key state
- mutating MCP tools return structured status, risk, evidence, and next actions
- local developer bootstrap can inspect prerequisites and run a small proof loop
  with machine-readable output
- tests cover response shape, tool/resource registration, and the local
  bootstrap path
- no active docs claim end-user autonomous package management is implemented
  before it exists

## Open Design Decisions For Implementation Planning

- Whether the shared response vocabulary belongs in `crates/conary-mcp` or a
  more transport-neutral crate/module.
- Whether local developer bootstrap should live under `conary-test`, `conary`,
  or a new subcommand family.
- Whether MCP prompts and resources should be implemented first for Remi,
  `conary-test`, or both together.
- Whether experimental `conary automation ai ...` should be deleted outright or
  hidden behind a clearer future-feature flag with no user-facing docs.
- How much of deploy/build/restart should remain MCP-accessible versus being
  moved to CLI-only workflows with MCP prompt guidance.
- Whether the first transport-neutral response types should be reflected into
  OpenAPI schemas at the same time as MCP output schemas.

## Suggested Follow-On Slices

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
