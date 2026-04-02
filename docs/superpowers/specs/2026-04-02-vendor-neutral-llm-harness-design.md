---
last_updated: 2026-04-02
revision: 1
summary: Re-found Conary's LLM collaboration harness around vendor-neutral guidance, MCP-first workflows, and salvaged durable project knowledge
---

# Vendor-Neutral LLM Collaboration Harness

## Summary

This design replaces Conary's older Claude-shaped agent orchestration layer
with a vendor-neutral collaboration harness optimized for modern LLM coding
assistants as of April 2, 2026.

The core decision is simple:

- keep the root `AGENTS.md` as the canonical source of truth
- treat tool-specific files such as `CLAUDE.md` as thin compatibility shims
- preserve durable Conary-specific knowledge from legacy `.claude/` materials
  in tracked, tool-agnostic documentation
- move machine-specific or sensitive operational details into a local,
  untracked operator file with a tracked example template
- present Conary's existing MCP surfaces and `conary-test` harness as the
  center of the collaboration story, rather than a named-agent roster

The goal is for Conary to feel naturally workable with "LLM coding buddies" in
general, not merely with one assistant product.

## Problem Statement

Conary already has unusual strengths for agent-assisted development:

- a real integration and adversarial test harness in `conary-test`
- multiple MCP surfaces for infrastructure and service operations
- strong project-level guidance in the root `AGENTS.md`
- a codebase organized enough to support targeted subsystem guidance

But the active collaboration layer still reflects an older, more
tool-proprietary model:

- `CLAUDE.md` currently contains active project guidance rather than acting as a
  compatibility shim
- `.claude/agents/` encodes a named-agent workflow that is not portable across
  assistants
- `.claude/rules/` mixes durable Conary knowledge with tool-specific loading
  assumptions and stale implementation facts
- local `.claude/settings.local.json` files can contain host-specific
  allowances and plaintext secrets, which is a portability problem and a local
  security hygiene problem even when the file is ignored
- archived orchestration documents frame the harness as a persona/roster system
  more than a repository/eval/tooling system

That mismatch creates friction:

- contributors using other assistants inherit Claude-era concepts that do not
  necessarily map cleanly
- durable Conary knowledge is trapped in proprietary directories
- sensitive operational notes risk being copied forward in unsafe ways
- the repo's real differentiators, MCP and executable eval harnesses, are not
  the primary story

## Goals

- Make `AGENTS.md` the single canonical, vendor-neutral contributor contract.
- Keep tool-specific compatibility files thin and subordinate to `AGENTS.md`.
- Preserve durable, reusable Conary-specific knowledge from legacy agent docs.
- Remove or archive named-agent and tool-proprietary scaffolding that no longer
  fits.
- Centralize sensitive operator-only notes in an untracked local file with a
  safe tracked template.
- Present MCP and `conary-test` as first-class collaboration primitives.
- Reduce stale instructions, ghost workflows, and duplicate guidance.
- Make the repo easy to approach with a wide range of coding assistants.

## Non-Goals

- Designing a new proprietary agent roster for another tool.
- Building a fully automated self-improving agent loop in this slice.
- Replacing `conary-test` with a second LLM-specific harness.
- Documenting every operational secret or every personal workstation tweak in
  the tracked repository.
- Solving all future agent-eval needs in one pass.

## Current State

### Canonical Guidance Is Split Across Two Eras

The repository root already has a concise and useful `AGENTS.md`.
That file is close to current best practice for portable instruction loading.

However, `CLAUDE.md` still carries substantial active guidance, including:

- build and test commands
- infrastructure notes
- MCP server descriptions
- a six-agent roster
- workflow instructions that assume Claude-specific agent dispatch

This means the effective "source of truth" is ambiguous.

### Valuable Knowledge Exists Inside Legacy `.claude/`

The `.claude/rules/` tree contains a large amount of genuinely useful
Conary-specific material:

- subsystem maps
- invariants and gotchas
- deployment and infrastructure notes
- integration-testing workflows
- package-manager, repository, trust, transaction, and service guidance

Much of that content is worth preserving.
What is not worth preserving is the assumption that it should live forever in a
Claude-specific loading mechanism.

### Some Legacy Local Files Are Unsafe To Keep As-Is

The local `.claude/settings.local.json` in this workspace includes
machine-specific permissions and plaintext credentials.

That is an immediate warning sign for the cleanup:

- the file is not portable
- the file is not appropriate as canonical collaboration guidance
- any still-live credentials in it should be treated as candidates for rotation

### The Repo's True Differentiators Already Exist

Conary is already unusually well-suited to agent-assisted work because it has:

- `conary-test`, a containerized, machine-readable integration and adversarial
  test harness
- Remi and conary-test MCP surfaces for infrastructure and test operations
- docs and code structured enough to support targeted instructions

The cleanup should elevate those strengths rather than burying them under agent
persona lore.

## Approach Options

### Option 1: Minimal Refresh

Keep the existing `.claude/` structure mostly intact, rewrite `AGENTS.md`, and
trim only the most stale items.

Pros:

- lowest immediate effort
- least churn

Cons:

- keeps Conary feeling Claude-shaped
- preserves duplicate guidance
- makes future drift likely

### Option 2: Vendor-Neutral Re-Foundation

Make `AGENTS.md` canonical, convert `CLAUDE.md` to a compatibility shim,
salvage durable knowledge into tracked tool-agnostic docs, move sensitive local
notes to an untracked operator file, and remove legacy named-agent workflow
scaffolding.

Pros:

- aligns with current best practices
- makes Conary friendly to many assistants
- preserves durable knowledge without keeping proprietary structure
- highlights MCP and eval harnesses as the main story

Cons:

- requires deliberate extraction and reorganization work
- forces decisions about what to archive, delete, or convert

### Option 3: Full Productized Harness Overhaul

Do Option 2, then immediately add nested overrides, repo-local skills,
long-running task artifacts, and agent-specific eval suites.

Pros:

- strongest long-term position
- produces the most complete harness

Cons:

- larger scope
- mixes foundational cleanup with follow-on enhancements
- increases the risk of overbuilding before the base is clean

## Chosen Direction

Choose Option 2.

Conary should first establish a clean, vendor-neutral base:

- one canonical instruction file
- thin compatibility shims
- preserved durable knowledge
- explicit local handling for sensitive operator notes
- MCP-first and eval-first project framing

Follow-on harness enhancements can then build on that base without carrying
legacy tool assumptions forward.

## Design

### 1. Canonical Instruction Model

The root `AGENTS.md` becomes the single canonical contributor contract.

It should answer only the questions that matter across assistants:

- what this workspace contains
- what commands prove a change is correct
- what safety and security constraints matter
- what "good" looks like in this project
- where deeper subsystem and operations docs live

It should remain concise enough to avoid context bloat.
The file should act as an index and contract, not as a dumping ground for every
fact about the repo.

The design standard here is:

- `AGENTS.md` is a map, not a manual
- it should route assistants toward the right commands, docs, and verification
  paths
- it should not try to inline whole subsystem references, runbooks, or
  architecture primers

In practice, the root file should bias toward:

- stable project invariants
- concise workflow expectations
- links to deeper source-of-truth docs

And it should bias away from:

- long subsystem explanations
- volatile implementation inventories
- operational detail that changes frequently
- duplicated copies of material already maintained elsewhere

If a section starts reading like a handbook chapter, it belongs in a linked doc,
not the root `AGENTS.md`.

### 2. Thin Compatibility Shims

Tool-specific files should remain only as edge-case compatibility layers.

For `CLAUDE.md`, the active version should be thin:

- point readers and tools to `AGENTS.md` as canonical
- include only the minimum tool-specific note needed for compatibility
- avoid restating substantive repository guidance unless absolutely required

This preserves usability for tools that still look for a proprietary filename
without making that file the real authority.

Future tool-specific compatibility files should follow the same rule.
If files such as `GEMINI.md` or similar ever appear, they should also stay thin
and defer back to `AGENTS.md` rather than becoming parallel sources of truth.

### 3. Salvage Durable Knowledge Into Tool-Agnostic Docs

Useful Conary-specific knowledge currently trapped in `.claude/rules/` and
agent memory should be preserved in tracked, tool-neutral documentation.

The intended split is:

- root `AGENTS.md` for global contributor contract
- focused docs for subsystem and operations knowledge
- archived historical docs only where they retain reasoning value

The content to salvage includes:

- architecture and workspace layout
- integration and deployment workflows
- MCP surfaces and intended usage
- stable subsystem invariants and gotchas
- durable debugging or review heuristics that will still matter later

The content not worth carrying forward includes:

- named-agent persona descriptions
- dispatch rosters
- proprietary hook-loading assumptions
- stale schema/version/test-count claims
- point-in-time review verdicts and ephemeral bug memories

This salvage process should also classify information by churn level.

Low-churn information is a good fit for tracked guidance:

- stable repo structure
- durable command patterns
- long-lived safety invariants
- subsystem boundaries that rarely change

High-churn information is a poor fit for root guidance and should either live in
a canonical maintained doc or be omitted entirely:

- exact tool counts
- exact test counts
- exact schema counts
- frequently changing workflow matrices
- rapidly evolving host or deployment details
- ephemeral environment-specific command allowances

The default rule is conservative:

- if information changes often and does not have a clear freshness owner, do not
  place it in `AGENTS.md`
- if information must be tracked despite churn, place it in the narrowest
  possible canonical doc and give it an explicit review path

When salvaging from legacy `.claude/rules/*.md`, strip any Claude-specific
loading metadata such as `paths:` frontmatter.
That metadata explains how Claude loaded the file, not how the underlying
Conary subsystem works.

The salvage pass should also use an explicit triage heuristic rather than
relying on vague judgment.

Reference, do not duplicate:

- `architecture.md` when `docs/ARCHITECTURE.md` already covers the topic
- `integration-tests.md` when `docs/INTEGRATION-TESTING.md` already covers the
  topic

Distill stable invariants only:

- `db.md`
- `transaction.md`
- `resolver.md`
- `generation.md`
- `ccs.md`
- `repository.md`
- `trust.md`
- `server.md` at the level of subsystem roles and boundaries rather than full
  type inventories

Skim for rare non-obvious gotchas, otherwise drop:

- `erofs.md`
- `delta.md`
- `filesystem.md`
- `packages.md`
- `recipe.md`
- `cli.md`

Drop as active guidance after preserving only any reusable reasoning:

- `agents-in-workflow.md`
- `context7.md`

One durable habit from `context7.md` is worth preserving in neutral language:
assistants should prefer checking current external library or framework
documentation over guessing at APIs when version-specific behavior matters.

The stale database rule file is a useful concrete warning here.
At the time of this design it still claims schema facts such as version and
table counts that have already drifted, which is exactly why volatile
inventories should not be copied into active assistant guidance.

### 4. Sensitive Local Ops Notes Move Out Of Tracked Guidance

The cleanup should introduce a clear split between tracked project knowledge and
local operator-only notes.

This split should be made mechanically explicit rather than left conceptual.
The migration needs one tracked example file, one ignored local file, and one
clear retirement path for legacy local settings files.

Tracked:

- a concrete example template such as `docs/operations/LOCAL_ACCESS.example.md`
- non-secret deployment workflow structure
- hostnames, roles, and safe operational patterns that are acceptable to share

Untracked:

- a concrete ignored local note such as `docs/operations/LOCAL_ACCESS.md`
- local usernames
- credential material
- SSH shortcuts or workstation-specific notes
- operator conveniences that should not be canonical project instructions

This design intentionally centralizes any information that would otherwise be
lost from `.claude` memory files or local settings into one safe local place
rather than scattering it across proprietary tooling files.

The cleanup should also explicitly handle the current local
`.claude/settings.local.json` file:

- confirm it is ignored and untracked rather than assuming it is a repository
  security event
- scan it for secrets before local deletion or retirement so nothing important
  is lost silently
- migrate any non-secret operational knowledge out of it into tracked or local
  replacement files as appropriate
- rotate any still-live credentials found there before the cleanup is considered
  complete

### 5. MCP-First Collaboration Story

The active docs should frame Conary as a project with first-class machine
interfaces for assistants.

That means documenting, in vendor-neutral language:

- which MCP surfaces exist
- which workflows should prefer MCP tools over ad hoc SSH or curl
- when local CLI fallback is appropriate
- how `conary-test` and service MCP endpoints support verifiable work

The point is not "Conary has agent integrations."
The point is "Conary is intentionally shaped to be operable by both humans and
agents."

### 5a. Reusable Safety Hooks Need An Explicit Fate

Not everything inside `.claude/` is mere persona scaffolding.
At least two existing scripts encode useful safeguards:

- `block-sensitive.sh` prevents casual edits to likely secret-bearing files
- `post-edit-clippy.sh` provides an immediate Rust lint loop after source edits

The cleanup should make an explicit choice for such scripts instead of treating
all hook material as disposable proprietary glue.

The intended rule is:

- if a script provides reusable project value but still depends on a
  proprietary hook protocol, move it under a generic helper location such as
  `scripts/dev/` and name or document it honestly as tool-coupled
- if a script only makes sense inside a proprietary hook system and its value
  is already covered by other safeguards, remove it intentionally

This avoids an accidental regression where useful safeguards disappear merely
because their current loader is tool-specific.

For Conary's current Claude hooks, "tool-coupled" means the scripts still
expect Claude's hook wire contract, including JSON on stdin, structured
`hookSpecificOutput` JSON on stdout, and hook-specific exit semantics.

### 6. `conary-test` As The Primary Eval Backbone

The harness should explicitly present `conary-test` as the primary executable
verification system for agent-assisted changes.

That means the active collaboration docs should steer assistants toward:

- package-scoped Rust tests where appropriate
- `cargo run -p conary-test -- list` for manifest sanity
- phase and suite selection for integration validation
- MCP or HTTP access to test runs when available

Conary does not need a second test harness just for LLMs.
It needs its existing test harness documented and used as the default
verification backbone.

### 7. Delete, Archive, Or Convert With Intent

Legacy `.claude/` material should be handled according to content value.

Convert:

- durable subsystem rules
- infrastructure and integration notes
- stable debugging patterns

Archive:

- historical design documents that still explain important reasoning

If no historically useful reasoning survives triage, no archive file needs to
be created merely for symmetry.

Delete:

- named-agent prompts
- persona rosters
- proprietary workflow glue
- obsolete local settings files after their useful content is intentionally
  retired or re-homed
- duplicate or obsolete instructions once their useful content is preserved

The cleanup should not leave a large shadow architecture in place.
If something is no longer part of the current collaboration model, it should not
continue to look active.

## Content Mapping Strategy

The intended migration map is:

- `AGENTS.md`
  - canonical contributor contract
- `CLAUDE.md`
  - thin shim pointing to `AGENTS.md`
- new tool-agnostic docs under a dedicated docs area
  - salvaged subsystem and operations knowledge
- local ignored operator note
  - preserved sensitive or machine-specific knowledge
- `docs/.../archive`
  - historically interesting but inactive design and review material

The exact directory names can be chosen during planning, but the conceptual
split should remain stable.

## Security Considerations

This cleanup is partly a security hardening pass.

At minimum:

- local settings with secrets or plaintext credentials must not be treated as
  canonical project guidance
- any still-valid credentials discovered in local assistant settings should be
  rotated
- the local settings file should be scanned and reviewed before retirement so
  credentials, endpoints, and operational notes are handled intentionally
- future guidance should avoid embedding sensitive data in tool-specific config
  files

The new local operator note mechanism should make the safe path easy enough that
secrets do not drift back into tracked docs.

## Freshness Rules

This cleanup only succeeds if the new guidance remains fresh.

The repository should adopt an explicit rule for high-churn tracked information:

- every volatile fact kept in tracked docs must have a clear canonical home
- that canonical home must be narrow enough that updates are realistic
- updates should happen as part of the same change that invalidates the fact
- if that discipline is not realistic, the volatile fact should be removed from
  tracked guidance rather than copied forward stale

For `AGENTS.md`, the freshness bar should be stricter still:

- prefer principles, commands, and links over volatile inventories
- avoid embedding counts, rapidly changing matrices, or operational trivia
- treat every added volatile detail as a maintenance burden that needs a reason

The project should assume that stale guidance is worse than missing guidance for
agent-assisted work.
An outdated map routes both humans and assistants into the wrong terrain.

## Risks

### Risk: Useful Knowledge Gets Deleted

Mitigation:

- review `.claude/rules/`, `.claude/skills/`, and agent memory before removal
- preserve durable facts in the new tracked docs
- use archive locations only for material with real historical value

### Risk: The New `AGENTS.md` Becomes Bloated

Mitigation:

- keep the root file index-like and concise
- push subsystem detail into dedicated docs
- treat context economy as a design requirement
- reject additions that mostly duplicate linked docs
- treat volatile factual detail as suspect unless it has a clear maintenance
  path

### Risk: Tracked Guidance Goes Stale Again

Mitigation:

- keep high-churn facts out of `AGENTS.md`
- require volatile tracked facts to live in narrow canonical docs
- update those docs in the same change that alters the underlying reality
- remove drift-prone facts when their maintenance burden exceeds their value

### Risk: Cleanup Leaves Broken References

Mitigation:

- update all active docs that point at deleted `.claude` content
- verify no active contributor doc still instructs readers to rely on removed
  agent personas or rules
- verify the chosen local-ops note path is ignored and the tracked example path
  is documented from `AGENTS.md` or the new docs index

### Risk: Tool Compatibility Regresses

Mitigation:

- keep thin shims for edge-case tools such as Claude-specific file discovery
- ensure the shim points clearly to the canonical root guidance

## Success Criteria

This design is successful when:

- `AGENTS.md` is clearly the canonical project guidance
- `AGENTS.md` reads like a map and contract rather than a handbook
- `CLAUDE.md` is a thin compatibility shim rather than a second source of truth
- durable Conary knowledge from `.claude` has a maintained, tool-agnostic home
- sensitive machine-specific data no longer lives in tracked collaboration docs
- the project uses a safe tracked-example plus ignored-local-note pattern for
  operator-specific details
- any useful `.claude/hooks/*` safeguards have either been repurposed into
  honestly tool-coupled helper scripts or intentionally retired with
  replacement coverage
- active docs describe MCP and `conary-test` as first-class collaboration tools
- obsolete named-agent workflow scaffolding is no longer part of the active repo
- high-churn facts are either excluded from root guidance or have an explicit
  canonical doc and review path

## Follow-On Work

This cleanup intentionally prepares, but does not itself require, later
enhancements such as:

- nested `AGENTS.override.md` files near specialized subsystems
- vendor-neutral playbooks for recurring workflows
- explicit long-running task handoff artifacts
- agent-oriented eval suites layered on top of `conary-test`

Those are good next steps, but only after the foundation is clean.
