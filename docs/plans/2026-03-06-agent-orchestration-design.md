# Agent Orchestration Design

Date: 2026-03-06

## Problem

Conary has 30+ modules across 4 crates and 11 ad-hoc team agents that are hard to
remember and don't compose well. Need: (1) a task dispatcher that picks the right
agents automatically, (2) per-module context that loads only when relevant, (3) agents
that learn the codebase over time.

## Design

### 1. Agent Roster (6 agents, replaces 11)

| Agent | Role | Tools | Memory |
|-------|------|-------|--------|
| **portage** | Dispatcher/orchestrator — classifies tasks, picks pipeline, conducts phases | All | No |
| **lintian** | Code reviewer/auditor — correctness, security, conventions, architecture | Read-only | project |
| **emerge** | Implementer — parallel execution with file ownership, dependency-aware | All | No |
| **valgrind** | Debugger — root-cause analysis, competing hypotheses, methodical elimination | All | project |
| **autopkgtest** | QA/test hardener — coverage gaps, edge cases, error paths, writes tests | All | No |
| **sbuild** | Release/build verifier — clean build matrix, versioning, changelog, final gate | All | No |

Personalities are Linux/packaging themed:
- **portage**: Greybeard sysadmin. Seen every distro since Slackware 3.0. Compiles the plan before building.
- **lintian**: Debian policy pedant. Finds every violation. Files reports with severity ratings.
- **emerge**: The builder. Thinks in dependency graphs and parallel make jobs.
- **valgrind**: Memory leak hunter. Traces symptoms to root causes nobody else finds.
- **autopkgtest**: Integration test fanatic. Untested code is broken code you don't know about yet.
- **sbuild**: Clean-room builder. Nothing ships without sbuild's sign-off.

### 2. Pipelines

portage classifies the task and selects a pipeline:

| Pipeline | Sequence | Trigger phrases |
|----------|----------|-----------------|
| bugfix | valgrind → emerge → autopkgtest | "fix", "broken", "bug", stack traces |
| feature | lintian(arch) → emerge → autopkgtest → lintian(review) | "add", "implement", "new" |
| review | lintian | "review", "audit", "how's the code" |
| harden | lintian → emerge → autopkgtest | "harden", "improve coverage" |
| refactor | lintian(analysis) → emerge → autopkgtest | "refactor", "clean up" |
| release | autopkgtest → sbuild | "release", "ship", "prep" |
| custom | user-specified | explicit agent names |

**Orchestration model:**
- Sequential phases → subagents (cheaper, results flow forward via handoff docs)
- Parallel independent work → agent team (when scope is huge or user says "in parallel")
- Each phase produces a structured handoff doc for the next
- User gates between phases (unless told to run hands-off)

### 3. Path-Scoped Module Rules (13 files)

`.claude/rules/` files with `paths:` frontmatter. Only load when touching matching files.
Each file is 30-50 lines: key types, invariants, gotchas, operational context.

| File | Paths | Content |
|------|-------|---------|
| db.md | `conary-core/src/db/**` | Schema v45, migration conventions, SQLite pragmas |
| packages.md | `conary-core/src/packages/**` | RPM/DEB/Arch parsers, PackageMetadata via common.rs |
| repository.md | `conary-core/src/repository/**` | Sync protocol, mirror health, Remi client, URL rules |
| resolver.md | `conary-core/src/resolver/**` | SAT solver (resolvo), provider matching |
| filesystem.md | `conary-core/src/filesystem/**` | CAS layout, deployer safety, VFS tree |
| transaction.md | `conary-core/src/transaction/**` | Journal recovery, crash safety, atomic ops |
| ccs.md | `conary-core/src/ccs/**` | CCS format, builder, policy, OCI export, CBOR manifest |
| trust.md | `conary-core/src/trust/**` | TUF supply chain, ceremony, key rotation |
| recipe.md | `conary-core/src/recipe/**`, `conary-core/src/bootstrap/**` | Recipe system, cook pipeline, 8-stage bootstrap |
| server.md | `conary-server/**` | Remi server, federation, daemon REST API |
| erofs.md | `conary-erofs/**` | EROFS format, composefs, inode layout |
| cli.md | `src/commands/**` | CLI conventions, command structure, output format |
| delta.md | `conary-core/src/delta/**` | Binary deltas, zstd dict compression |

### 4. Persistent Agent Memory

- **lintian** (`memory: project`): Accumulates codebase patterns, recurring issues, architecture decisions
- **valgrind** (`memory: project`): Remembers past bugs, root causes, debugging paths

Stored in `.claude/agent-memory/{agent}/`. Project-scoped, could be version-controlled.

### 5. CLAUDE.md Cleanup

- Remove Mira references (done)
- Update tool selection section (done)
- Keep concise (<200 lines)
- Rules files handle module-specific context

### 6. What Gets Deleted

All 11 existing agents in `.claude/agents/` are replaced:
- cli-review-team → folded into lintian
- debug-team → replaced by valgrind
- expert-review-team → replaced by lintian
- full-cycle-team → replaced by portage
- growth-team → removed (marketing, not code)
- implement-team → replaced by emerge
- pr-review-team → folded into lintian
- qa-hardening-team → replaced by autopkgtest
- refactor-team → pipeline (lintian → emerge → autopkgtest)
- release-team → replaced by sbuild
- test-gen-team → folded into autopkgtest

Stale team configs in `~/.claude/teams/` cleaned up.

## Implementation Order

1. Write the 6 agent `.md` files
2. Write the 13 path-scoped rule files
3. Delete the 11 old agents
4. Clean up stale team configs
5. Update CLAUDE.md if needed
6. Test: run portage on a real task
