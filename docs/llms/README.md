---
last_updated: 2026-08-28
revision: 19
summary: Lean vendor-neutral assistant routing into canonical Conary owners, task-sized proof packets, and a shared compiler cache with isolated worktree targets
---

# Conary For Coding Assistants

## Fast Path

`AGENTS.md` is loaded project policy, not a codebase encyclopedia. After reading
it, route the actual task instead of opening every orientation document:

```bash
# discover a capability
bash scripts/agent-context.sh --list

# print one task-sized owner packet
bash scripts/agent-context.sh --feature <slug>
bash scripts/agent-context.sh --path <file>

# use a one-line route during triage or review
bash scripts/agent-context.sh --path <file> --brief
```

The packet names the owning files, neighboring systems, focused proof,
interaction gate, documentation owner, and safety invariants. Read those files
before editing. Use `--run focused` for a narrow change; use `--run gate` only
when the packet's interaction condition applies. Both execution modes consume
`scripts/dev-build.sh`, which shares eligible compiler outputs across linked
worktrees without sharing their Cargo target directories.

## Context Budget

- Do not read all of `docs/modules/feature-ownership.md` during ordinary task
  setup. `agent-context` extracts only the selected card.
- Do not preload `docs/llms/subsystem-map.md` for a known file or feature. Use
  it only for architecture questions or when no owner is known yet.
- Open `docs/roadmaps/development-roadmap.md` for project status, ordering, or
  blocker decisions—not as default implementation context.
- Read the canonical module, specification, testing, or operations document
  named by the task packet; avoid neighboring deep dives unless the change
  crosses that boundary.
- Keep dynamic branch state, failing output, run IDs, and one-off decisions in
  the issue, PR, or current handoff. Do not copy them into durable orientation
  docs.
- Preserve exact bulky evidence in its owning artifact or log, then summarize
  only the causal result and locator in the working conversation.

This keeps startup guidance small while retaining precise, on-demand context.

## Route By Task

| Task | Read next |
| --- | --- |
| Source or feature change | The `agent-context` packet for the path or slug |
| Architecture question | `docs/ARCHITECTURE.md`, then the owning `docs/modules/*.md` |
| Package semantic or persisted contract | The owning file under `docs/specs/` |
| Integration test or fixture | `docs/INTEGRATION-TESTING.md` and `docs/modules/test-fixtures.md` |
| Deploy, MCP, release, or host work | `docs/operations/infrastructure.md` plus the owner packet |
| Current priority or blocker | `docs/roadmaps/development-roadmap.md` and live issue/PR state |
| Public or assistant-doc change | The proof floor below plus the affected behavior owner |

## Truth Owners

1. `AGENTS.md` owns concise repo-wide policy and safety boundaries.
2. `CONTRIBUTING.md` owns issue, branch, PR, review, and closeout workflow.
3. `docs/modules/feature-ownership.md`, consumed through
   `scripts/agent-context.sh`, owns path routing and proof selection.
4. Architecture, module, specification, integration-testing, and operations
   docs own current subsystem behavior and contracts.
5. `docs/roadmaps/` owns ordering, maturity, blockers, and milestone state.
6. Issues and draft PRs own bounded execution status and exact current proof.

One rule has one owner. Link to it rather than restating it. Completed or
superseded planning is removed after its durable truth and resume facts move to
the owners above; Git history retains the old plan.

## Tool Entrypoints

- Codex reads `AGENTS.md` directly; `docs/llms/openai-codex.md` contains only
  current OpenAI-specific notes.
- Claude Code uses the thin `CLAUDE.md` import.
- Google Antigravity and the `agy` CLI use `.agents/rules/conary.md`.
- Reasonix uses the thin `REASONIX.md` shim.
- GitHub Copilot uses `.github/copilot-instructions.md` and can also consume
  `AGENTS.md` on supported agent surfaces.

These files route to shared truth and must not become parallel manuals.
Machine-local preferences, credentials, private paths, and access shortcuts
stay in ignored local files.

## Working Rules

- Use one primary issue and one issue-linked branch for each non-trivial slice;
  follow `CONTRIBUTING.md` for linkage and merge semantics.
- Prefer structured Conary operation surfaces over ad hoc SSH or curl when the
  typed MCP, HTTP, or CLI contract covers the workflow.
- `crates/conary-agent-contract` owns operation vocabulary. MCP code adapts it.
- For local environment validation, start with
  `cargo run -p conary-test -- bootstrap check --json`, then preview
  `bootstrap smoke --dry-run --json` before a mutating smoke run.
- For broad refactor planning, use `scripts/line-count-report.sh` and
  `scripts/maintainability-drift-report.sh` as evidence aids, not replacement
  owners or automatic failure gates.
- Use `scripts/dev-build.sh cargo -- <cargo arguments>` for repeated local
  builds, and `scripts/dev-build.sh status` to inspect the shared compiler
  cache. Caller-selected wrappers, cache locations, and Cargo targets retain
  precedence; cache cleanup never owns a worktree target.
- When tool, SDK, model, or harness behavior matters, verify current official
  vendor documentation before changing durable guidance.

## Agent-Doc Proof Floor

For assistant guidance changes, run:

```bash
bash scripts/check-doc-truth.sh
bash scripts/test-doc-truth.sh
bash scripts/test-agent-context.sh
bash scripts/agent-context.sh --validate
git diff --check
```

Add the affected feature-card proof when a behavior claim, command, route, or
public surface changes. Sweep explicitly for every retired filename or tool
name removed by the slice.
