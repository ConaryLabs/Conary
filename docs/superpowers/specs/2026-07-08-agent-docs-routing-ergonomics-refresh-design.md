---
last_updated: 2026-07-08
revision: 1
summary: Design and audit for refreshing Conary's agent-facing documentation routing around fresh-agent ergonomics, single-source ownership, cross-tool compatibility, and lower startup token load.
status: draft
---

# Agent Docs Routing Ergonomics Refresh - Design And Audit

## Purpose

Refresh Conary's agent-facing documentation so a fresh coding agent can land in
the repo, choose the right owner files and verification path within a few
minutes, and avoid loading a parallel manual of stale or duplicated routing
facts. This is an ergonomics and drift-control slice, not a product-doc rewrite.

The active docs are not currently broken. The docs audit, inventory, coherency,
and whitespace checks were green before this spec was written. The opportunity
is to make the agent path easier to follow and cheaper to keep true.

## Current-State Audit

Checked against `main` at `21870771513b31800ce3bf8ec17530770b8f4f9a` after
`git pull --ff-only` reported the repo was current.

- `AGENTS.md` is compact at 94 lines and already follows the modern pattern:
  durable repo contract first, deeper routing through linked canonical docs.
- Tool-specific entrypoints are thin:
  - `CLAUDE.md` points to `AGENTS.md`, `docs/llms/README.md`, and
    `agent-context.sh`.
  - `GEMINI.md`, `REASONIX.md`, and `.github/copilot-instructions.md` point
    back to the same canonical layer instead of restating policy.
- `docs/llms/README.md` is 115 lines and already acts as a vendor-neutral map,
  but it can be sharper as a first-five-minute landing path.
- `docs/llms/subsystem-map.md` is 342 lines and overlaps with
  `docs/modules/feature-ownership.md`.
- `docs/modules/feature-ownership.md` is 805 lines and is already the detailed
  owner-card source behind `scripts/agent-context.sh`.
- The strongest drift pressure is duplicated ownership/routing detail between
  `docs/llms/subsystem-map.md` and `docs/modules/feature-ownership.md`.

Baseline checks run before design:

- `bash scripts/check-doc-truth.sh`
- `bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete`
- `LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -`
- `bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv`
- `git diff --check`
- `bash scripts/maintainability-drift-report.sh`

## External Guidance Checked

This design was checked against current public agent-doc guidance on
2026-07-08 local time, in response to the user's requested 2026-07-07 review.

- OpenAI Codex guidance still recommends `AGENTS.md` for durable repo guidance,
  keeping it practical, and splitting larger guidance into linked docs.
- OpenAI Codex documents a 32 KiB default project-doc budget for loaded
  instruction files, which makes startup-doc size and routing clarity material.
- OpenAI Codex positions skills and MCP as complementary to `AGENTS.md`, not as
  replacements for checked-in repo guidance.
- Claude Code documents `CLAUDE.md` as the active Claude instruction file and
  recommends importing `AGENTS.md` from `CLAUDE.md` when a repo already uses
  shared agent guidance.
- GitHub Copilot documents repository instructions in
  `.github/copilot-instructions.md`, path-specific `.github/instructions/*`
  instructions, and agent instructions through `AGENTS.md`.
- Gemini CLI documents hierarchical `GEMINI.md` context files and imports,
  which supports keeping Conary's `GEMINI.md` thin while pointing to the shared
  contract.

## Goals

- A fresh coding agent can find the right owner card, start-here files, focused
  proof, and interaction gate in 2-3 minutes.
- Maintainers can review changes to agent docs without rereading a second
  ownership database.
- Cross-tool entrypoints keep working for Codex, Claude, Gemini, Reasonix, and
  Copilot without creating conflicting policy.
- Detailed routing and verification facts have one canonical active home.
- Startup instruction/token load stays compact; deeper subsystem detail is
  loaded only when the task needs it.
- Future implementation is a small docs slice, not a new tooling project.

## Non-Goals

- No broad rewrite of product docs, architecture docs, or operation guides.
- No generated routing system in this slice.
- No `.claude/` harness revival.
- No new nested `AGENTS.md` files unless the implementation audit finds a
  subtree with durable rules that differ from the repo root.
- No historical archive churn unless an active stale reference requires it.
- No changes to release claims, public product support claims, or command help.

## Recommended Approach

Use a routing contract refresh.

`AGENTS.md` remains the short durable repo contract. It should keep build/test
commands, repo-wide safety rules, CLI output conventions, and the pointer to
`scripts/agent-context.sh`, but it should not absorb detailed subsystem routing.

`docs/llms/README.md` becomes the fresh-agent landing map. It should answer:

1. What do I read first?
2. How do I identify the owner for my task?
3. Which command tells me the start-here files and proof commands?
4. Which docs are canonical versus planning or historical?
5. What proof floor applies to docs-only agent-guidance edits?

`docs/modules/feature-ownership.md` remains the canonical detailed owner-card
source. It owns slugs, path routing, start-here files, neighbor systems, focused
proof, interaction gates, docs to update, and safety notes. Agents should reach
it through `scripts/agent-context.sh` first whenever possible.

`docs/llms/subsystem-map.md` becomes a compact orientation index rather than a
parallel route table. It can keep workspace orientation, a short list of slugs,
and high-level "look here first" pointers, but long duplicated path/proof lists
should move out of this file or be replaced with command-first routing.

Tool-specific files stay compatibility shims. They should help each tool load
the shared repo contract in the format that tool expects, then get out of the
way.

## Information Architecture

```text
AGENTS.md
  Repo contract: commands, safety, verification, durable rules
  Points to docs/llms/README.md and scripts/agent-context.sh

docs/llms/README.md
  Fresh-agent landing map
  Choose-by-task routing
  Canonical/planning/archive distinction
  Docs proof floor

scripts/agent-context.sh
  Command-first owner lookup
  Reads docs/modules/feature-ownership.md

docs/modules/feature-ownership.md
  Canonical detailed owner cards
  Slugs, paths, proofs, gates, docs-to-update, safety notes

docs/llms/subsystem-map.md
  Compact workspace and subsystem index
  No long duplicated ownership database

CLAUDE.md / GEMINI.md / REASONIX.md / .github/copilot-instructions.md
  Thin compatibility shims
```

## Fresh-Agent Flow

A new agent should be able to follow this path:

1. Read `AGENTS.md`.
2. Read `docs/llms/README.md`.
3. Run one of:
   - `bash scripts/agent-context.sh --list`
   - `bash scripts/agent-context.sh --feature <slug>`
   - `bash scripts/agent-context.sh --path <file>`
4. Read only the returned start-here files before editing.
5. Use the returned focused proof for small local edits.
6. Use the returned interaction gate when the change crosses neighbor systems.
7. For docs/agent-guidance edits, run the docs proof floor before claiming
   completion.

This turns routing into a command-first workflow instead of a manual skim of two
large route maps.

## Implementation Boundary

The future implementation slice should stay small and mostly edit active
assistant guidance:

- Refresh `docs/llms/README.md` into a sharper landing map.
- Slim `docs/llms/subsystem-map.md` so it no longer duplicates detailed
  owner-card routing.
- Update `CLAUDE.md` to use or explicitly discuss the current `@AGENTS.md`
  import pattern.
- Keep `GEMINI.md`, `REASONIX.md`, and `.github/copilot-instructions.md` thin
  unless a current-format mismatch is found during implementation.
- Update docs-audit metadata for touched files.

Do not edit `docs/modules/feature-ownership.md` unless the refresh exposes a
factual routing drift, unclear owner card, or broken `agent-context.sh` lookup.
The point is to make the existing ownership source easier to reach, not to
rebuild it.

## File-Level Design Notes

### `AGENTS.md`

Expected outcome: likely no structural change. At most, tighten one pointer if
the implementation makes `docs/llms/README.md` more explicitly command-first.

Keep:

- build/test commands,
- feature-scoped `agent-context.sh` instruction,
- coding style and safety rules,
- maintainability/refactor discipline,
- assistant doc model,
- CLI output conventions,
- security notes.

Avoid:

- expanded vendor-specific guidance,
- long subsystem route lists,
- copied external-doc facts that will drift.

### `docs/llms/README.md`

Expected outcome: primary refresh target.

Add or clarify:

- "First 5 minutes" section.
- "Choose by task" section that points to `agent-context.sh`.
- "Canonical versus planning versus historical" section.
- "Proof floor for agent-doc changes" section.
- "When to check current external docs" section for Codex/OpenAI, Claude,
  Gemini, Copilot, MCP, SDKs, and version-specific library behavior.

Remove or compress:

- any guidance that repeats root `AGENTS.md` policy without adding routing
  value,
- any stale-tool caution that belongs only in archive notes.

### `docs/llms/subsystem-map.md`

Expected outcome: compact index.

Keep:

- workspace orientation,
- slug list with one-line meanings,
- pointers to canonical module docs,
- the instruction to use `agent-context.sh` for path/proof routing.

Compress or remove:

- long duplicated route lists when `feature-ownership.md` already owns the
  exact paths,
- detailed proof commands when the owner card already owns them,
- historical phase-routing detail that is now better represented by active
  owner cards or archived plans.

### Tool-Specific Shims

Expected outcome: compatibility, not policy.

- `CLAUDE.md`: prefer an `@AGENTS.md` import plus minimal Claude-specific
  compatibility text if that works with the repo's current Claude setup. If the
  implementation chooses not to import, it must say why and keep the file
  shorter than a parallel manual.
- `GEMINI.md`: keep pointing to the shared contract. Do not expand with
  Gemini-only policy unless a Gemini-specific format need is proven.
- `REASONIX.md`: keep as a thin compatibility shim.
- `.github/copilot-instructions.md`: keep as the Copilot repository
  instruction pointer; do not duplicate the full repo contract.

## Acceptance Criteria

- `AGENTS.md` remains concise and map-like.
- A fresh agent can identify an owner card and proof command by reading
  `AGENTS.md`, `docs/llms/README.md`, and running `agent-context.sh`.
- `docs/llms/subsystem-map.md` no longer acts as a second detailed ownership
  database.
- Tool-specific shims remain short and point to canonical guidance.
- The Claude shim is explicitly checked against the current import guidance.
- Docs-audit metadata records every touched active file.
- No active stale references are introduced to retired Claude harness paths.
- No public product behavior claim changes without coherency-ledger review.

## Verification Floor

Run this after the implementation edit set:

```bash
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

For the implementation slice, add focused stale-reference sweeps based on the
actual edit set. At minimum, sweep active docs for retired `.claude/` harness
references if `CLAUDE.md` changes, and sweep for duplicated "look here first"
route blocks if `subsystem-map.md` is slimmed.

## Risks And Mitigations

- Risk: `subsystem-map.md` becomes too thin to be useful.
  Mitigation: keep workspace orientation and high-level slug meanings, but
  require detailed routing to come from `agent-context.sh`.
- Risk: Claude import behavior increases token load by loading both the import
  and repeated shim text.
  Mitigation: if using `@AGENTS.md`, keep the rest of `CLAUDE.md` minimal and
  avoid restating root policy.
- Risk: docs-audit metadata becomes busywork.
  Mitigation: touch only active agent guidance files and the required ledger
  rows; do not change inventory unless the file set changes.
- Risk: this violates the meta-layer budget.
  Mitigation: keep this as a factual-drift and ergonomics maintenance slice for
  existing agent-facing docs, with no new tooling and no broad ledger/card
  redesign.

## Open Questions For Implementation

- Should `CLAUDE.md` literally import `@AGENTS.md`, or should it remain a
  pointer-only shim because another Claude setup already loads `AGENTS.md`?
- How aggressive should `subsystem-map.md` slimming be: remove only duplicated
  proof/path details, or also compress historical phase notes?
- Should `docs/llms/README.md` include a short "model/tool docs freshness"
  reminder that names the vendor docs checked for this spec, or keep external
  references in `docs/llms/openai-codex.md` and tool-specific shims?

## Done Means

This design is done when it is registered in the docs-audit metadata, passes the
docs proof floor, and is reviewed by the user before any active agent-doc edit
slice starts.
