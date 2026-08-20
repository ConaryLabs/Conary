---
last_updated: 2026-08-20
revision: 2
summary: Current OpenAI-specific notes for lean Codex context, task prompts, and verification
---

# OpenAI/Codex Notes

Conary's durable assistant contract stays vendor-neutral in `AGENTS.md` and
`docs/llms/README.md`. Keep only OpenAI-specific behavior here.

Verify time-sensitive behavior against current official documentation:

- [Codex `AGENTS.md` discovery](https://learn.chatgpt.com/docs/agent-configuration/agents-md)
- [Codex best practices](https://learn.chatgpt.com/guides/best-practices)
- [Prompting](https://learn.chatgpt.com/docs/prompting)
- [Current OpenAI model guidance](https://developers.openai.com/api/docs/guides/latest-model)

## Keep Startup Context Lean

Codex loads `AGENTS.md` automatically. Do not tell it to reread that file or
preload the full ownership map. Route a task through `agent-context`, then open
only the selected card's sources and canonical docs.

State each durable rule once. Remove repeated instructions, examples, and tool
descriptions unless they encode a measured project requirement. Track both
startup context and growing conversation context; compare prompt changes on
representative Conary tasks rather than assuming more instruction is better.

## Prompt The Outcome

A strong task packet names:

- the goal and relevant owner or path;
- constraints and authority boundaries;
- what completion means;
- the exact evidence required.

State whether the request is to inspect, plan, implement, review, debug, or
verify. Keep dynamic branch state, failing commands, run IDs, and one-off notes
in the current issue, PR, or prompt rather than durable docs. Ask for findings,
decisions, concise rationale, and observed verification—not hidden
chain-of-thought.

Use harness controls for reasoning effort and response verbosity. Reserve
high-cost modes for tasks whose measured quality benefit justifies them; do not
encode model aliases, pricing, or a dated default in repository policy.

## Long Work

Use a plan for genuinely multi-step work and keep it synchronized with actual
progress. Preserve completed actions, exact identities, tool results, active
assumptions, unresolved blockers, and the next concrete step across compaction
or handoff. Retain bulky evidence in its owning log or artifact and return the
smallest complete summary.

Keep tool behavior in tool schemas, skills, plugins, or harness configuration.
There is no active OpenAI API prompt harness in this repository; product
automation UI under `conary-core` is not a model prompt layer.
