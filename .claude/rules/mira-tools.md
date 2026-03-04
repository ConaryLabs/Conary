# Mira Tool Selection

Use Mira tools proactively for semantic operations. Fall back to Grep/Glob only for literal string searches or exact filename patterns.

## When to Use Mira

| Task | Use |
|------|-----|
| Find code by intent | `semantic_code_search("authentication")` |
| What calls this function? | `find_callers("function_name")` |
| What does X call? | `find_callees("function_name")` |
| List functions in file | `get_symbols(file_path="file.rs")` |
| Check if feature exists | `check_capability("feature description")` |
| Past decisions | `recall("topic")` |
| Store decisions | `remember("key insight")` |

## Memory System

Use `remember` for decisions and context, `recall` to retrieve. Evidence threshold:
- Only store patterns observed **multiple times** across sessions
- Decisions **explicitly requested** by the user
- Mistakes that caused **real problems**

## Expert Consultation

Use `consult_experts` for second opinions before major decisions:
```
consult_experts(roles=["architect"], context="...", question="...")
consult_experts(roles=["code_reviewer", "security"], context="...")
```

Available roles: `architect`, `plan_reviewer`, `code_reviewer`, `security`, `scope_analyst`

## Task and Goal Management

- **This session**: Use Claude's `TaskCreate` / `TaskUpdate` / `TaskList`
- **Across sessions**: Use Mira's `goal` tool with milestones
- Create goals for multi-session objectives, add weighted milestones, mark complete

## Sub-Agent Context

Sub-agents do NOT have Mira access. Before spawning sub-agents for significant work:
1. `recall()` relevant context
2. Include key info in the prompt
3. Be explicit about project conventions
