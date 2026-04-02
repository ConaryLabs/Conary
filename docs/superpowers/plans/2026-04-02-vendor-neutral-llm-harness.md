# Vendor-Neutral LLM Harness Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Re-found Conary's assistant-facing collaboration surface around a canonical `AGENTS.md`, thin tool-specific shims, neutral durable docs, safe local-ops note handling, and clearly tool-coupled helper scripts relocated under `scripts/dev`, while removing stale Claude-era scaffolding without losing valuable Conary knowledge.

**Architecture:** Execute the cleanup in four chunks. First, inspect the local ignored `.claude/settings.local.json` for salvageable non-secret knowledge and credential hygiene before retiring it locally. Second, augment the canonical guidance and add a neutral `docs/llms/` index plus non-secret operations docs, keeping `AGENTS.md` map-like and linking to existing canonical docs such as `docs/ARCHITECTURE.md` and `docs/INTEGRATION-TESTING.md` instead of duplicating churn-heavy detail. Third, relocate useful Claude hook logic under `scripts/dev` while keeping its Claude hook-protocol coupling explicit and confined to `.claude/settings.json`. Fourth, delete legacy `.claude` rules/agents/memory after salvage, then verify that active docs no longer depend on them and that high-churn or secret-bearing information has a safe home.

**Tech Stack:** Markdown docs with YAML frontmatter, shell helper scripts, git tracked and ignored files, `rg`, `git`, `bash -n`, and existing Conary CLI commands such as `cargo run -p conary-test -- list`.

---

## Preconditions

- Work from a clean branch or worktree so the `.claude` removals are easy to review.
- Treat `.claude/settings.local.json` as a potentially sensitive local file until proven otherwise.
- Do not paste secrets into new tracked docs, commit messages, or plan scratch notes.
- Keep the root `AGENTS.md` compact and index-like. If a new section starts reading like a handbook chapter, move that material into a linked doc instead.

## File Map

- Modify: `AGENTS.md`
- Modify: `CLAUDE.md`
- Modify: `CONTRIBUTING.md`
- Modify: `.gitignore`
- Modify: `.claude/settings.json`
- Create: `docs/llms/README.md`
- Create: `docs/llms/subsystem-map.md`
- Create: `docs/llms/archive/claude-era-notes.md`
- Create: `docs/operations/infrastructure.md`
- Create: `docs/operations/LOCAL_ACCESS.example.md`
- Create: `scripts/dev/claude-block-sensitive.sh`
- Create: `scripts/dev/claude-check-workspace-clippy.sh`
- Create locally only, untracked: `docs/operations/LOCAL_ACCESS.md`
- Retire locally only, untracked: `.claude/settings.local.json`
- Delete: `.claude/hooks/block-sensitive.sh`
- Delete: `.claude/hooks/post-edit-clippy.sh`
- Delete: `.claude/skills/deploy-forge.md`
- Delete: `.claude/agents/autopkgtest.md`
- Delete: `.claude/agents/emerge.md`
- Delete: `.claude/agents/lintian.md`
- Delete: `.claude/agents/portage.md`
- Delete: `.claude/agents/sbuild.md`
- Delete: `.claude/agents/valgrind.md`
- Delete: `.claude/agent-memory/lintian/MEMORY.md`
- Delete: `.claude/agent-memory/valgrind/MEMORY.md`
- Delete: `.claude/agent-memory/valgrind/bollard_api_migration.md`
- Delete: `.claude/rules/agents-in-workflow.md`
- Delete: `.claude/rules/architecture.md`
- Delete: `.claude/rules/ccs.md`
- Delete: `.claude/rules/cli.md`
- Delete: `.claude/rules/context7.md`
- Delete: `.claude/rules/db.md`
- Delete: `.claude/rules/delta.md`
- Delete: `.claude/rules/erofs.md`
- Delete: `.claude/rules/filesystem.md`
- Delete: `.claude/rules/generation.md`
- Delete: `.claude/rules/infrastructure.md`
- Delete: `.claude/rules/integration-tests.md`
- Delete: `.claude/rules/packages.md`
- Delete: `.claude/rules/recipe.md`
- Delete: `.claude/rules/repository.md`
- Delete: `.claude/rules/resolver.md`
- Delete: `.claude/rules/server.md`
- Delete: `.claude/rules/transaction.md`
- Delete: `.claude/rules/trust.md`

## Migration Map

- `AGENTS.md`
  - canonical project contract
  - compact verification rules
  - links to deeper docs
- `CLAUDE.md`
  - thin shim pointing to `AGENTS.md`
  - tool-specific note only if required
- `docs/llms/README.md`
  - assistant-facing map of the repo
  - links to canonical docs already maintained elsewhere
- `docs/llms/subsystem-map.md`
  - durable low-churn gotchas and subsystem orientation distilled from
    `.claude/rules/*` and `.claude/agent-memory/*`
- `docs/llms/archive/claude-era-notes.md`
  - historically interesting Claude-era reasoning worth preserving without
    leaving it active
- `docs/operations/infrastructure.md`
  - non-secret host, MCP, deploy, and service workflow notes distilled from
    `.claude/rules/infrastructure.md` and `.claude/skills/deploy-forge.md`
- `docs/operations/LOCAL_ACCESS.example.md`
  - tracked template for local operator notes
- `docs/operations/LOCAL_ACCESS.md`
  - ignored local-only note with machine-specific or sensitive details
- `scripts/dev/claude-block-sensitive.sh`
  - relocated sensitive-file guard that still speaks the Claude hook protocol
- `scripts/dev/claude-check-workspace-clippy.sh`
  - relocated post-edit workspace lint helper that still speaks the Claude hook protocol
- `.claude/settings.json`
  - optional Claude compatibility config that points to the relocated Claude hook helpers

## Freshness Rules For This Plan

- Keep high-churn facts out of `AGENTS.md`.
- Reuse existing canonical docs when they already cover a topic well.
- Do not copy exact counts, workflow matrix sizes, schema counts, or machine-specific allowances into root guidance.
- If a volatile fact must remain tracked, put it in the narrowest canonical doc and update it in the same change that changes reality.
- If a detail does not have a realistic maintenance path, omit it instead of copying it stale.

## Chunk 1: Secure The Migration Surface

### Task 1: Confirm the local settings file is untracked and scan it for credential awareness

**Files:**
- Read: `.claude/settings.local.json`
- Read: `.claude/rules/infrastructure.md`
- Read: `.claude/skills/deploy-forge.md`
- Read: `.claude/agent-memory/lintian/MEMORY.md`
- Read: `.claude/agent-memory/valgrind/MEMORY.md`
- Read: `.claude/agent-memory/valgrind/bollard_api_migration.md`

- [ ] **Step 1: Confirm the file is local-only, then review it line-by-line**

Run:

```bash
git ls-files .claude/settings.local.json
nl -ba .claude/settings.local.json | sed -n '1,260p'
```

Expected:

- `git ls-files` prints nothing, confirming the file was never tracked
- the numbered dump shows host-specific command allowances and any suspicious
  credential-like strings for local hygiene review

- [ ] **Step 2: Search for obviously sensitive patterns**

Run:

```bash
rg -n 'SSHPASS|sshpass|token|secret|password|Authorization|Bearer|api[_-]?key' .claude/settings.local.json
```

Expected: one or more hits that need manual classification as secret, stale
credential, host-specific note, or non-sensitive command allowance.

- [ ] **Step 3: Extract only durable non-secret facts into scratch notes**

Create a local scratch note outside the repository or under `/tmp/` that lists
only:

- hostnames worth keeping
- non-secret service roles
- durable deploy patterns
- MCP endpoint expectations

Do **not** copy raw credentials, tokens, or passwords into tracked files.

- [ ] **Step 4: Decide whether any credentials need rotation before merge**

If the review in Steps 1-2 finds anything that looks plausibly live, stop here
and rotate or invalidate it before the cleanup branch is merged.

Expected outcome: a binary decision recorded in your working notes:

- `no live credentials found`, or
- `credentials rotated before merge`

- [ ] **Step 5: Create a carry-forward checklist for Chunk 2 docs**

Create a short checklist in your local scratch note with two buckets:

- facts that belong in `docs/llms/subsystem-map.md`
- facts that belong in `docs/operations/infrastructure.md`

Expected: every durable non-secret fact extracted in Task 1 has an intended
destination before `.claude` material starts getting deleted.

### Task 2: Introduce the local-ops note pattern and retire the local settings file

**Files:**
- Modify: `.gitignore`
- Create: `docs/operations/LOCAL_ACCESS.example.md`
- Create locally only, untracked: `docs/operations/LOCAL_ACCESS.md`
- Retire locally only, untracked: `.claude/settings.local.json`

- [ ] **Step 1: Keep the existing Claude ignore entry and add the new local note path**

Confirm `.gitignore` already includes `.claude/settings.local.json`, then add
this line near the existing local/credential entries:

```gitignore
docs/operations/LOCAL_ACCESS.md
```

- [ ] **Step 2: Write the tracked example template**

Create `docs/operations/LOCAL_ACCESS.example.md` with YAML frontmatter and
sections like:

```md
---
last_updated: 2026-04-02
revision: 1
summary: Template for local machine-specific access notes that must not be committed
---

# Local Access Notes Template

## Purpose

This file is a local-only place for machine-specific or sensitive operational
notes. Copy it to `docs/operations/LOCAL_ACCESS.md` and keep the real file
untracked.

## Suggested Sections

## SSH And Host Aliases

- preferred SSH aliases
- usernames if they differ by host

## Service Endpoints

- local-only URLs or ports
- notes about which host is Forge, Remi, or other infrastructure

## Credential Storage Locations

- where credentials live locally
- how they are loaded
- rotation reminders

## Local Workflow Notes

- workstation-specific caveats
- useful local wrappers or shortcuts
- anything that should not be promoted into tracked project docs
```

- [ ] **Step 3: Create the ignored local file from the template**

Run:

```bash
mkdir -p docs/operations
cp docs/operations/LOCAL_ACCESS.example.md docs/operations/LOCAL_ACCESS.md
```

Then replace the template prose in the local file with the sanitized non-secret
notes from Task 1. Keep secrets out of the tracked example.

- [ ] **Step 4: Retire the local settings file locally**

Run:

```bash
rm -f .claude/settings.local.json
```

Expected: the local file is removed from the workspace after its durable
non-secret content has been re-homed, and there is no git index change because
the path was never tracked.

- [ ] **Step 5: Verify the replacement pattern**

Run:

```bash
git check-ignore -v .claude/settings.local.json docs/operations/LOCAL_ACCESS.md
git ls-files .claude/settings.local.json docs/operations/LOCAL_ACCESS.example.md
```

Expected:

- `git check-ignore` reports that both the legacy local settings path and the
  new local note path are ignored
- `git ls-files` returns nothing for `.claude/settings.local.json`
- `docs/operations/LOCAL_ACCESS.example.md` is trackable

- [ ] **Step 6: Commit the local-access scaffold**

Run:

```bash
git add .gitignore docs/operations/LOCAL_ACCESS.example.md
git commit -m "docs: add local access notes template for operator-specific details"
```

## Chunk 2: Rebuild The Canonical Guidance

### Task 3: Augment `AGENTS.md` as the canonical map, not a manual

**Files:**
- Modify: `AGENTS.md`
- Create: `docs/llms/README.md`

- [ ] **Step 1: Create the neutral assistant-facing index first**

Create `docs/llms/README.md` with YAML frontmatter and sections such as:

- purpose of the `docs/llms/` area
- canonical guidance order (`AGENTS.md` first, deeper docs second)
- links to `docs/ARCHITECTURE.md`
- links to `docs/INTEGRATION-TESTING.md`
- links to `docs/modules/*.md`
- links to `docs/operations/infrastructure.md`
- freshness rule: prefer linked canonical docs over duplicated volatile facts
- thin-shim rule: future tool-specific files such as `CLAUDE.md` or `GEMINI.md`
  should stay short and point back to `AGENTS.md`
- external-docs rule: prefer checking current library or framework docs over
  guessing APIs when version-specific behavior matters

Keep this file index-like and short.

- [ ] **Step 2: Augment the existing root `AGENTS.md` around stable invariants**

Keep the current structure that already works well, then add only the missing
pieces so it covers:

- workspace structure at a high level
- package-scoped build and test commands
- coding and safety rules
- expectations for verification and docs updates
- links to `docs/llms/README.md` and other canonical docs for details
- a short freshness reminder that high-churn detail belongs in linked docs

Remove or avoid:

- named-agent rosters
- deep subsystem prose
- volatile counts
- operational trivia

- [ ] **Step 3: Add an explicit freshness sentence to `AGENTS.md`**

Include a short rule such as:

```md
Keep this file map-like. If a detail changes often or needs more than a short
paragraph to explain, move it into a linked canonical doc instead of expanding
this file.
```

- [ ] **Step 4: Sanity-check the size and shape of `AGENTS.md`**

Run:

```bash
wc -l AGENTS.md
sed -n '1,220p' AGENTS.md
```

Expected: the file stays compact enough to skim in one pass, no section reads
like a handbook chapter, and the diff looks more like an augmentation than a
rewrite.

- [ ] **Step 5: Commit the canonical guidance rewrite**

Run:

```bash
git add AGENTS.md docs/llms/README.md
git commit -m "docs: make AGENTS the canonical assistant map"
```

### Task 4: Replace active Claude-era guidance with thin shims and neutral docs

**Files:**
- Modify: `CLAUDE.md`
- Modify: `CONTRIBUTING.md`
- Create: `docs/llms/subsystem-map.md`
- Create: `docs/llms/archive/claude-era-notes.md`
- Create: `docs/operations/infrastructure.md`

- [ ] **Step 1: Write the subsystem map from durable low-churn knowledge**

Create `docs/llms/subsystem-map.md` with YAML frontmatter and concise sections:

- workspace crates and their responsibilities
- durable "look here first" pointers for repository, resolver, trust, services,
  and `conary-test`
- stable gotchas extracted from `.claude/rules/*` and `.claude/agent-memory/*`
- a warning that volatile counts and rapidly changing matrices belong elsewhere

Do **not** one-for-one recreate every former `.claude/rules/*.md` file.
Consolidate only the durable, high-signal parts.

Use this salvage heuristic while drafting:

- `architecture.md` and `integration-tests.md`: do not duplicate; point to
  `docs/ARCHITECTURE.md` and `docs/INTEGRATION-TESTING.md`
- `infrastructure.md`: move only non-secret operational patterns into
  `docs/operations/infrastructure.md`
- `db.md`, `transaction.md`, `resolver.md`, `generation.md`, `ccs.md`,
  `repository.md`, and `trust.md`: keep stable invariants and "look here first"
  guidance only
- `server.md`: keep high-level service roles and subsystem boundaries, not full
  type inventories
- `erofs.md`, `delta.md`, `filesystem.md`, `packages.md`, `recipe.md`, and
  `cli.md`: skim for non-obvious gotchas, otherwise drop as code-derivable
- `agents-in-workflow.md`: drop after preserving any non-proprietary reasoning
  in the archive if it is still genuinely useful
- `context7.md`: do not preserve tool branding, but do preserve the underlying
  habit of checking current external docs before guessing APIs

Before carrying content forward, strip any `paths:` frontmatter so the new docs
preserve subsystem knowledge rather than Claude-specific loading rules.

Use `.claude/rules/db.md` as a concrete freshness warning: it still claims the
schema is at `v57` with `71 tables across 57 migrations`, which is exactly the
kind of volatile count that should not be copied forward into active guidance.

- [ ] **Step 2: Preserve any historically interesting reasoning in an explicit archive path**

Create `docs/llms/archive/claude-era-notes.md` with YAML frontmatter and record
only the historical reasoning worth preserving from the old `.claude/rules/*`
or agent-memory files.

Good candidates:

- why certain assistant-facing conventions existed
- historical tradeoff notes still useful for future cleanup
- durable observations that are informative but not suitable for active docs

Do **not** dump full old rule files into the archive. Summarize the reasoning in
neutral language.

If no reasoning worth archiving survives the salvage, skip this file and omit
it from the commit.

- [ ] **Step 3: Write the non-secret infrastructure and MCP doc**

Create `docs/operations/infrastructure.md` with YAML frontmatter and sections:

- Remi and Forge roles
- MCP endpoints and intended usage
- preferred deploy patterns
- safe fallback SSH/rsync playbooks
- link to `docs/operations/LOCAL_ACCESS.example.md` for local-only notes

Keep secrets out. Keep hostnames and service roles only if they are acceptable
to track.

- [ ] **Step 4: Verify the carry-forward checklist from Chunk 1 is satisfied**

Compare the Task 1 scratch checklist against:

- `docs/llms/subsystem-map.md`
- `docs/operations/infrastructure.md`
- `docs/llms/archive/claude-era-notes.md`

Expected: every durable non-secret fact from Task 1 either appears in one of
those docs or is intentionally dropped as too volatile to keep.

- [ ] **Step 5: Rewrite `CLAUDE.md` as a shim**

Reduce `CLAUDE.md` to:

- a short statement that `AGENTS.md` is canonical
- a pointer to `docs/llms/README.md`
- only any minimal Claude-specific note that is still truly needed

Remove the agent roster, `.claude/rules/` references, and duplicated project
instructions.

- [ ] **Step 6: Sanity-check that `CLAUDE.md` stays thin**

Run:

```bash
wc -l CLAUDE.md
sed -n '1,160p' CLAUDE.md
```

Expected: `CLAUDE.md` stays short enough to read in one pass and does not read
like a second source of truth. Aim to keep it under 40 lines unless a specific
tool-compatibility need justifies more.

- [ ] **Step 7: Update `CONTRIBUTING.md` to point to the new map**

Add a short section near the onboarding or contributing flow that tells
contributors using coding assistants to start with:

- `AGENTS.md`
- `docs/llms/README.md`
- `docs/INTEGRATION-TESTING.md` when validation spans `conary-test`

- [ ] **Step 8: Commit the neutral docs, archive note, and shims**

Run:

```bash
git add CLAUDE.md CONTRIBUTING.md docs/llms docs/operations/infrastructure.md
git commit -m "docs: replace Claude-era guidance with neutral assistant docs"
```

## Chunk 3: Repurpose Reusable Tooling And Remove Legacy Scaffolding

### Task 5: Relocate useful Claude hook logic into clearly tool-coupled helper scripts

**Files:**
- Create: `scripts/dev/claude-block-sensitive.sh`
- Create: `scripts/dev/claude-check-workspace-clippy.sh`
- Modify: `.claude/settings.json`
- Delete: `.claude/hooks/block-sensitive.sh`
- Delete: `.claude/hooks/post-edit-clippy.sh`

- [ ] **Step 1: Create the shared helper directory**

Run:

```bash
mkdir -p scripts/dev
```

- [ ] **Step 2: Port the sensitive-file guard with honest Claude coupling**

Create `scripts/dev/claude-block-sensitive.sh` by adapting the current
`.claude/hooks/block-sensitive.sh` logic:

- keep the file-protection behavior
- keep the Claude hook contract explicit: JSON on stdin, `hookSpecificOutput`
  JSON on stdout, and exit code `2` for hook intervention
- rename the file and comments so the protocol coupling is obvious rather than
  pretending the script is portable across assistants

Make the script executable:

```bash
chmod +x scripts/dev/claude-block-sensitive.sh
```

- [ ] **Step 3: Port the workspace clippy helper with honest Claude coupling**

Create `scripts/dev/claude-check-workspace-clippy.sh` from the current
`.claude/hooks/post-edit-clippy.sh` logic:

- keep the workspace-wide clippy invocation
- keep the Claude hook contract explicit: JSON on stdin, `hookSpecificOutput`
  JSON on stdout, and exit code `2` when additional context should be surfaced
- rename the file and comments so the protocol coupling is obvious rather than
  pretending the script is portable across assistants

Make it executable:

```bash
chmod +x scripts/dev/claude-check-workspace-clippy.sh
```

- [ ] **Step 4: Repoint the thin Claude config to the relocated Claude hook helpers**

Update `.claude/settings.json` so any remaining Claude hooks call:

- `scripts/dev/claude-block-sensitive.sh`
- `scripts/dev/claude-check-workspace-clippy.sh`

Keep the config minimal; it is now a compatibility layer, not a source of
project policy.

- [ ] **Step 5: Delete the old hook path copies**

Run:

```bash
git rm .claude/hooks/block-sensitive.sh .claude/hooks/post-edit-clippy.sh
```

- [ ] **Step 6: Syntax-check and commit the hook migration**

Run:

```bash
bash -n scripts/dev/claude-block-sensitive.sh scripts/dev/claude-check-workspace-clippy.sh
git add .claude/settings.json scripts/dev
git commit -m "chore: relocate Claude hook helpers under scripts/dev"
```

### Task 6: Remove the obsolete `.claude` personas, rules, memory, and skill glue

**Files:**
- Delete: `.claude/agents/*.md`
- Delete: `.claude/agent-memory/**`
- Delete: `.claude/rules/*.md`
- Delete: `.claude/skills/deploy-forge.md`

- [ ] **Step 1: Re-read the salvage targets before deletion**

Run:

```bash
find .claude -maxdepth 3 -type f | sort
```

Expected: a final checklist confirming that the durable parts have already been
moved into `docs/llms/` and `docs/operations/`, and that anything deleted is
either archived, distilled, or intentionally dropped.

- [ ] **Step 2: Delete the named-agent prompts**

Run:

```bash
git rm .claude/agents/autopkgtest.md \
       .claude/agents/emerge.md \
       .claude/agents/lintian.md \
       .claude/agents/portage.md \
       .claude/agents/sbuild.md \
       .claude/agents/valgrind.md
```

- [ ] **Step 3: Delete the old rule, memory, and skill files**

Run:

```bash
git rm .claude/agent-memory/lintian/MEMORY.md \
       .claude/agent-memory/valgrind/MEMORY.md \
       .claude/agent-memory/valgrind/bollard_api_migration.md \
       .claude/skills/deploy-forge.md \
       .claude/rules/agents-in-workflow.md \
       .claude/rules/architecture.md \
       .claude/rules/ccs.md \
       .claude/rules/cli.md \
       .claude/rules/context7.md \
       .claude/rules/db.md \
       .claude/rules/delta.md \
       .claude/rules/erofs.md \
       .claude/rules/filesystem.md \
       .claude/rules/generation.md \
       .claude/rules/infrastructure.md \
       .claude/rules/integration-tests.md \
       .claude/rules/packages.md \
       .claude/rules/recipe.md \
       .claude/rules/repository.md \
       .claude/rules/resolver.md \
       .claude/rules/server.md \
       .claude/rules/transaction.md \
       .claude/rules/trust.md
```

- [ ] **Step 4: Confirm only the intended Claude compatibility file remains**

Run:

```bash
git ls-files .claude
```

Expected: only `.claude/settings.json` remains tracked.

- [ ] **Step 5: Commit the legacy scaffolding removal**

Run:

```bash
git commit -m "chore: remove legacy Claude-specific scaffolding"
```

## Chunk 4: Clean References And Verify The New Contract

### Task 7: Remove any active ghost references to the old `.claude` world

**Files:**
- Modify as needed: `AGENTS.md`
- Modify as needed: `CLAUDE.md`
- Modify as needed: `CONTRIBUTING.md`
- Modify as needed: `docs/llms/README.md`
- Modify as needed: `docs/operations/infrastructure.md`

- [ ] **Step 1: Search the active contributor-facing docs for stale references**

Run:

```bash
rg -n --glob '!docs/llms/archive/**' '\.claude/|settings\.local\.json|deploy-forge\.md|agents-in-workflow|portage|lintian|autopkgtest|valgrind|sbuild|emerge' \
  AGENTS.md CLAUDE.md CONTRIBUTING.md docs/llms docs/operations README.md
```

Expected: no hits outside `docs/llms/archive/` or other intentional historical
or explanatory text.

- [ ] **Step 2: Patch any remaining ghost references**

Replace stale references with:

- `AGENTS.md`
- `docs/llms/README.md`
- `docs/operations/infrastructure.md`
- existing canonical docs like `docs/INTEGRATION-TESTING.md`

- [ ] **Step 3: Confirm the new docs still read like maps**

Read:

```bash
sed -n '1,220p' AGENTS.md
sed -n '1,220p' docs/llms/README.md
sed -n '1,260p' docs/llms/subsystem-map.md
```

Expected: the root file stays compact, while the linked docs carry the deeper
context without drifting into volatile trivia.

- [ ] **Step 4: Commit any ghost-reference cleanup**

Run:

```bash
git add AGENTS.md CLAUDE.md CONTRIBUTING.md docs/llms docs/operations README.md
git commit -m "docs: remove stale references to legacy assistant scaffolding"
```

### Task 8: Run the final verification sweep

**Files:**
- Verify: `AGENTS.md`
- Verify: `CLAUDE.md`
- Verify: `CONTRIBUTING.md`
- Verify: `docs/llms/README.md`
- Verify: `docs/llms/subsystem-map.md`
- Verify: `docs/operations/infrastructure.md`
- Verify: `docs/operations/LOCAL_ACCESS.example.md`
- Verify: `scripts/dev/claude-block-sensitive.sh`
- Verify: `scripts/dev/claude-check-workspace-clippy.sh`
- Verify: `.claude/settings.json`

- [ ] **Step 1: Verify the shell helpers are valid**

Run:

```bash
bash -n scripts/dev/claude-block-sensitive.sh scripts/dev/claude-check-workspace-clippy.sh
```

Expected: no output and exit code 0.

- [ ] **Step 2: Verify the local note path is ignored and the example is tracked**

Run:

```bash
git check-ignore -v docs/operations/LOCAL_ACCESS.md
git ls-files docs/operations/LOCAL_ACCESS.example.md
```

Expected:

- the local note path is ignored
- the example template is tracked

- [ ] **Step 3: Verify the remaining local-only ignore entries are still intentional**

Run:

```bash
rg -n '^(.mcp.json|CLAUDE\.local\.md|\.claude/settings\.local\.json|\.claude/\*\.local\.md|docs/operations/LOCAL_ACCESS.md)$' .gitignore
```

Expected: the old local-only Claude ignore entries remain intentionally in
place, and the new `docs/operations/LOCAL_ACCESS.md` entry sits alongside them
as the vendor-neutral local-notes mechanism.

- [ ] **Step 4: Verify the Claude footprint is intentionally tiny**

Run:

```bash
git ls-files .claude
```

Expected: only `.claude/settings.json` remains tracked.

- [ ] **Step 5: Verify there are no active `.claude` references in the new docs**

Run:

```bash
rg -n --glob '!docs/llms/archive/**' '\.claude/' AGENTS.md CLAUDE.md CONTRIBUTING.md docs/llms docs/operations
```

Expected: no hits outside `docs/llms/archive/`.

- [ ] **Step 6: Verify the new docs have frontmatter and sane local links**

Run:

```bash
find docs/llms docs/operations -type f -name '*.md' | sort | while read -r f; do
  echo "===== $f ====="
  sed -n '1,12p' "$f"
done

rg -n '\]\((AGENTS\.md|CLAUDE\.md|docs/[^)#]+\.md)(#[^)]+)?\)' docs/llms docs/operations
```

Expected:

- each new doc begins with YAML frontmatter
- local markdown links in the new assistant-facing docs point at real repo docs

- [ ] **Step 7: Verify one representative assistant-facing command is still honest**

Run:

```bash
cargo run -p conary-test -- list
```

Expected: the manifest inventory prints successfully, confirming the documented
test-harness entrypoint still works.

- [ ] **Step 8: Review the final diff with a freshness lens**

Run:

```bash
git diff --stat HEAD~5..HEAD
git diff -- AGENTS.md CLAUDE.md CONTRIBUTING.md docs/llms docs/operations scripts/dev .claude/settings.json
```

Check specifically that:

- `AGENTS.md` still reads like a map
- high-churn facts were linked rather than copied
- no secrets were copied into tracked docs
- no valuable reusable guardrails were lost during `.claude` cleanup

- [ ] **Step 9: Record final verification evidence**

Create a short note in your PR description or handoff summary listing:

- the secret triage result
- the tracked/untracked local-note pattern
- the remaining tracked `.claude` footprint
- the verification commands and outcomes
