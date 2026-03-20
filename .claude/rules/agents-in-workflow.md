# Agent Dispatch Points in Development Workflow

This project has 6 custom agents. Use them at these workflow points instead of generic approaches.

## During Implementation (subagent-driven-development, executing-plans)

**After each task implementation → dispatch lintian for code review**
Use `lintian` as the code-quality review step. It knows this codebase's conventions, has project memory, and checks architectural fit. When dispatching lintian, include the superpowers review structure in the prompt (Strengths / Critical / Important / Minor / Assessment / "Ready to merge?") — lintian adds codebase-specific checks on top of that methodology. For the spec-compliance step, use the superpowers spec-reviewer as-is — lintian doesn't replace spec checking.

**When tests fail unexpectedly → dispatch valgrind**
Instead of guessing at fixes, dispatch `valgrind` to trace root cause. It has project memory and uses competing hypotheses. Use it instead of `superpowers:systematic-debugging` when working in this codebase.

**For parallel independent tasks → dispatch emerge**
When the plan has 2+ independent tasks (like parser conversions), dispatch `emerge` to parallelize with strict file ownership. It manages dependency order and verifies compilation.

## At Chunk Boundaries

**After completing a chunk → dispatch autopkgtest**
Before moving to the next chunk, dispatch `autopkgtest` on the files touched in that chunk. It audits test coverage, writes missing tests, checks error handling paths, and hunts edge cases. This catches gaps before they compound.

## Before Finishing a Branch

**Before merge/PR → dispatch sbuild**
As part of `superpowers:finishing-a-development-branch`, dispatch `sbuild` to run the full build/test/clippy matrix, validate versioning, and verify changelogs. Nothing ships without sbuild's sign-off.

## Quick Reference

| Situation | Agent | Instead Of |
|-----------|-------|------------|
| Code quality review after task | **lintian** | generic code-reviewer (lintian adds codebase context) |
| Test failure during work | **valgrind** | systematic-debugging |
| Parallel implementation | **emerge** | multiple sequential subagents |
| Chunk complete, check quality | **autopkgtest** | moving on without QA |
| Ready to merge/PR | **sbuild** | manual build verification |
| Not sure which to use | **portage** | deciding yourself |
