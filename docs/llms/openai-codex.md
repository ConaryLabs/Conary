---
last_updated: 2026-07-02
revision: 1
summary: OpenAI/Codex-specific assistant workflow notes for Conary
---

# OpenAI/Codex Notes

This repo keeps durable assistant guidance model-neutral in `AGENTS.md` and
`docs/llms/README.md`. Use this file only for OpenAI/Codex prompt and harness
notes that should not live in the vendor-neutral map.

When OpenAI/Codex prompt or harness behavior matters, check current OpenAI docs:

- [Codex AGENTS.md guidance](https://developers.openai.com/codex/guides/agents-md)
- [Codex best practices](https://developers.openai.com/codex/learn/best-practices)
- [Prompt guidance](https://developers.openai.com/api/docs/guides/prompt-guidance)
- [Prompt engineering](https://developers.openai.com/api/docs/guides/prompt-engineering)
- [Reasoning best practices](https://developers.openai.com/api/docs/guides/reasoning-best-practices)
- [Using GPT-5.5](https://developers.openai.com/api/docs/guides/latest-model#using-reasoning-models)

For Codex or other OpenAI agents, keep stable repo policy near the top of the
prompt by pointing to `AGENTS.md` and linked canonical docs. Put dynamic context
such as branch state, failing commands, run IDs, and one-off user notes near the
end so repeated prompts stay cache-friendly and less prone to stale copied lore.

State the desired mode plainly: plan, design, implement, review, debug, or
verify. Include acceptance criteria and exact verification commands when they
are known, while leaving room for the agent to inspect the codebase and adjust
the path. Prefer outcome-focused constraints over long, brittle scripts. For
long-running Codex work, ask for an explicit plan/TODO list, short notable tool
preambles, persistence until the request is fully handled, and final evidence
before success claims.

Keep OpenAI reasoning-model prompts simple and direct. Use Markdown headings,
XML tags, or other clear delimiters when mixing logs, diffs, requirements, and
expected output. Do not ask for hidden chain-of-thought; ask for findings,
decisions, verification evidence, and concise rationale.

Treat output length and reasoning depth as separate concerns. Use harness
controls such as `text.verbosity` and `reasoning.effort` when available; in repo
prompts, ask for concrete budgets, section counts, or machine-readable output
only when the workflow needs them.

For tool-heavy sessions, short tool preambles are useful. If a future harness
manages Responses API state directly, preserve returned assistant output item
metadata such as `phase`, use `previous_response_id` where appropriate, and make
compaction summaries preserve completed actions, active assumptions, IDs, tool
outcomes, unresolved blockers, and the next concrete goal.

Keep tool-specific behavior in tool descriptions, MCP schemas, or harness
configuration when possible. `AGENTS.md` and `docs/llms/README.md` should carry
cross-tool policy, source-of-truth pointers, and durable repo workflow
expectations. Use structured outputs or schema validation in a harness instead
of prose-only JSON schema instructions.

There is no active OpenAI/LLM prompt harness in this repository today.
`crates/conary-core/src/automation/prompt.rs` is product automation UI, not a
model prompt layer. If a future agentic harness is added, prefer the Responses
API plus current Agents SDK patterns over custom orchestration, and document the
runtime contract outside the repo-wide assistant map.

Do not bake a "current date" into durable assistant docs. Add explicit dates or
time zones only when a workflow needs user-local, release, policy-effective, or
other non-UTC context.
