# Agent Docs Routing Ergonomics Refresh Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refresh Conary's agent-facing docs so a fresh coding agent can find the right owner files and verification path within a few minutes without loading a duplicate routing manual.

**Architecture:** Keep `AGENTS.md` as the durable repo contract, make `docs/llms/README.md` the command-first fresh-agent landing map, and keep `docs/modules/feature-ownership.md` plus `scripts/agent-context.sh` as the canonical detailed routing surface. Slim `docs/llms/subsystem-map.md` into an orientation index and keep tool-specific files as compatibility shims instead of policy manuals.

**Tech Stack:** Markdown docs, existing `scripts/agent-context.sh`, docs-audit inventory and ledger scripts, coherency ledger checker, focused `rg` stale-reference sweeps.

## Global Constraints

- Scope is active assistant/contributor guidance only: `AGENTS.md`, `docs/llms/README.md`, `docs/llms/subsystem-map.md`, `CLAUDE.md`, `GEMINI.md`, `REASONIX.md`, `.github/copilot-instructions.md`, and required docs-audit metadata.
- Do not change product behavior, release claims, command help, public support claims, Rust code, or generated assets.
- Do not add generated routing tooling.
- Do not revive tracked `.claude/` harness files.
- Do not add nested `AGENTS.md` files unless implementation finds durable subtree-specific rules that differ from the root contract.
- Keep `docs/modules/feature-ownership.md` as the canonical owner-card source; edit it only for a factual routing defect found during this slice.
- Keep tool-specific shims short and pointed at canonical guidance.
- Public package and repository support claims stay limited to Fedora 44, Ubuntu 26.04, and Arch.
- Verification floor: `check-doc-truth`, docs-audit ledger, docs-audit inventory diff, coherency ledger, stale-reference sweeps, and `git diff --check`.

---

## File Structure

- Modify `docs/llms/README.md`
  - Owns the fresh-agent landing map, choose-by-task routing, canonical/planning/archive distinction, and docs proof floor.
- Modify `docs/llms/subsystem-map.md`
  - Becomes a compact workspace and subsystem orientation index; detailed path/proof routing points to `scripts/agent-context.sh`.
- Modify `CLAUDE.md`
  - Uses the `@AGENTS.md` import pattern or explicitly documents why the shim remains pointer-only. The default implementation should use the import because current Claude Code docs recommend it for repos that already use shared `AGENTS.md`.
- Review and usually leave unchanged `GEMINI.md`, `REASONIX.md`, and `.github/copilot-instructions.md`
  - Keep as thin compatibility shims unless a current-format mismatch is found.
- Modify `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
  - Refresh rows for every touched active doc.
- Modify `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
  - Only if the file set changes. The implementation tasks below edit existing active docs only, so this should not change during execution.

## Task 1: Refresh `docs/llms/README.md` Into The Fresh-Agent Landing Map

**Files:**
- Modify: `docs/llms/README.md`
- Reference: `AGENTS.md`
- Reference: `docs/superpowers/specs/archive/2026-07-08-agent-docs-routing-ergonomics-refresh-design.md`
- Reference: `docs/modules/feature-ownership.md`
- Reference: `scripts/agent-context.sh`

**Interfaces:**
- Consumes: Existing `AGENTS.md` repo contract and `scripts/agent-context.sh --list|--feature|--path`.
- Produces: A landing-map structure that Tasks 2 and 3 can point to without restating routing policy.

- [ ] **Step 1: Capture current structure before editing**

Run:

```bash
sed -n '1,180p' docs/llms/README.md
```

Expected: existing frontmatter, `# Conary For Coding Assistants`, guidance order, instruction layers, core/focused docs, working rules, and freshness rules are visible.

- [ ] **Step 2: Replace the opening sections with a first-five-minute path**

Edit `docs/llms/README.md` so the document starts with this structure after frontmatter:

```markdown
# Conary For Coding Assistants

## Purpose

This directory is the vendor-neutral landing map for coding assistants working
on Conary. It should get a fresh agent from "I know nothing about this repo" to
"I know the owner files and proof commands for this task" without becoming a
second subsystem manual.

## First Five Minutes

1. Read [`AGENTS.md`](../../AGENTS.md) for the repo contract, safety rules, and
   verification expectations.
2. Use this file to decide which canonical docs to open next.
3. Run one command to route the work:
   - `bash scripts/agent-context.sh --list`
   - `bash scripts/agent-context.sh --feature <slug>`
   - `bash scripts/agent-context.sh --path <file>`
4. Read the start-here files printed by that command before editing.
5. Use the focused proof for narrow edits and the interaction gate when the
   change crosses a neighbor system.

## Choose By Task

- **Feature or source edit:** run `bash scripts/agent-context.sh --path <file>`
  for each touched path, or `--feature <slug>` when starting from a capability.
- **Docs-only assistant guidance edit:** start with `AGENTS.md`, this file,
  `docs/llms/subsystem-map.md`, and the affected tool shim; use the docs proof
  floor below.
- **Architecture or subsystem question:** use `docs/ARCHITECTURE.md` and the
  relevant `docs/modules/*.md` file after owner-card routing.
- **Integration-test or fixture work:** read `docs/INTEGRATION-TESTING.md` and
  `docs/modules/test-fixtures.md`.
- **Deploy, MCP, or host workflow:** read `docs/operations/infrastructure.md`.
- **Version-specific tool, SDK, MCP, or model behavior:** check current vendor
  documentation before editing durable guidance.

## Guidance Order

1. `AGENTS.md` is the canonical repo-wide contract.
2. This file is the vendor-neutral routing layer into canonical docs.
3. `scripts/agent-context.sh` and `docs/modules/feature-ownership.md` own exact
   feature slugs, path routing, start-here files, focused proof, and interaction
   gates.
4. Linked canonical docs own subsystem, testing, and operations detail.
```

Preserve the existing frontmatter but update `last_updated`, increment
`revision`, and update `summary` to mention fresh-agent routing ergonomics.

- [ ] **Step 3: Keep the instruction layers concise**

Update the existing `## Instruction Layers` section so it says:

```markdown
## Instruction Layers

- `AGENTS.md` is the shared repo-wide contract for coding agents.
- `docs/llms/README.md` is the vendor-neutral landing map into canonical docs.
- `docs/modules/feature-ownership.md` is the detailed owner-card source, reached
  through `scripts/agent-context.sh` whenever possible.
- `docs/llms/subsystem-map.md` is a compact workspace orientation index, not a
  second ownership database.
- Canonical subsystem and operations detail belongs in human-readable docs such
  as `docs/ARCHITECTURE.md`, `docs/modules/*.md`, and
  `docs/operations/*.md`.
- Tool-specific entrypoints such as `CLAUDE.md`, `GEMINI.md`, `REASONIX.md`, and
  `.github/copilot-instructions.md` stay intentionally thin and point back to
  this layered doc system instead of becoming parallel manuals.
- Historical tool notes belong under `docs/llms/archive/`.
```

- [ ] **Step 4: Add canonical/planning/archive distinction**

Add this section after the focused docs list:

```markdown
## Doc Families

- **Contract:** `AGENTS.md` and thin tool shims. These are durable instructions
  loaded by agents or coding tools.
- **Routing:** this file, `docs/llms/subsystem-map.md`,
  `docs/modules/feature-ownership.md`, and `scripts/agent-context.sh`.
- **Canonical docs:** architecture, module, integration-testing, operations,
  guide, and spec docs that describe current repo behavior or intended active
  product contracts.
- **Planning docs:** active files under `docs/superpowers/plans/` and
  `docs/superpowers/specs/`. They define scoped future work and must not be
  mistaken for already-shipped behavior.
- **Historical docs:** archive directories and rows marked historical in the
  documentation accuracy audit inventory. Use them for provenance, not current
  instructions.
```

- [ ] **Step 5: Add the docs proof floor**

Add this section before `## Freshness Rules`:

````markdown
## Agent-Doc Proof Floor

For docs-only edits to assistant guidance, run:

```bash
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

Add a focused stale-reference sweep for any retired path, tool shim, or routing
phrase touched by the edit.
````

When inserting the nested command block inside Markdown, keep the outer document
syntax valid. Use ordinary fenced blocks in the file; do not indent the section
under a list item.

- [ ] **Step 6: Tighten existing working/freshness rules**

Edit the existing `## Working Rules` and `## Freshness Rules` sections so they
keep these exact requirements:

```markdown
- Treat `AGENTS.md` as a contract and this file as a map.
- Prefer `AGENTS.md` as the shared cross-tool filename where the tool supports
  it.
- Use `scripts/agent-context.sh` before feature-scoped edits instead of
  manually skimming long route lists.
- Do not duplicate detailed owner-card routing outside
  `docs/modules/feature-ownership.md`.
- Keep tool-specific files short and pointed back at the shared contract.
- When version-specific library, SDK, MCP, model, or coding-agent behavior
  matters, check current external documentation before editing durable guidance.
```

Do not remove existing Conary-specific operational rules unless they are now
duplicated by the new sections.

- [ ] **Step 7: Verify the landing map**

Run:

```bash
rg -n "First Five Minutes|Choose By Task|Doc Families|Agent-Doc Proof Floor|agent-context.sh" docs/llms/README.md
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected: all four new headings are found, `agent-context.sh` appears in the
new routing sections, and the ledger check still passes.

- [ ] **Step 8: Commit Task 1**

```bash
git add docs/llms/README.md
git commit -m "docs(llms): sharpen fresh-agent landing map"
```

## Task 2: Slim `docs/llms/subsystem-map.md` Into A Compact Orientation Index

**Files:**
- Modify: `docs/llms/subsystem-map.md`
- Reference: `docs/modules/feature-ownership.md`
- Reference: `scripts/agent-context.sh`
- Reference: `docs/superpowers/specs/archive/2026-07-08-agent-docs-routing-ergonomics-refresh-design.md`

**Interfaces:**
- Consumes: Task 1's command-first landing map.
- Produces: A shorter subsystem index that no longer duplicates detailed owner-card route lists.

- [ ] **Step 1: Capture current length and route blocks**

Run:

```bash
wc -l docs/llms/subsystem-map.md
rg -n "^## Workspace Orientation|^## Look Here First|Repository sync|Install orchestration|Generation building|Agent/MCP" docs/llms/subsystem-map.md
```

Expected: current length is over 300 lines and the long route blocks are visible.

- [ ] **Step 2: Update frontmatter**

Update `last_updated`, increment `revision`, and set the summary to:

```yaml
summary: Compact assistant subsystem orientation index with detailed path and proof routing delegated to feature ownership cards.
```

- [ ] **Step 3: Keep workspace orientation**

Keep the `## Workspace Orientation` section as a compact list of workspace roots.
Do not expand it with per-file route lists.

- [ ] **Step 4: Replace `## Look Here First` with command-first routing**

Replace the opening paragraph under `## Look Here First` with:

````markdown
Use this file for quick subsystem orientation only. For exact owner files,
path matches, focused proof, and interaction gates, use the feature-card bridge:

```bash
bash scripts/agent-context.sh --list
bash scripts/agent-context.sh --feature <slug>
bash scripts/agent-context.sh --path <file>
```

`docs/modules/feature-ownership.md` is the canonical source behind those
commands.
````

- [ ] **Step 5: Keep the slug list**

Keep the existing one-line slug list for:

```text
dispatch, install, adopt, model, generation, ccs, packaging, profiles, remi,
conaryd, bootstrap, conary-test, agent-mcp
```

Ensure each slug line stays one sentence or shorter.

- [ ] **Step 6: Replace long duplicated route blocks with canonical doc pointers**

Remove the long per-feature path lists that begin after the slug list. Replace
them with this compact section:

```markdown
## Canonical Detail Pointers

- CLI dispatch and command routing: `docs/modules/feature-ownership.md` slug
  `dispatch`, plus `docs/ARCHITECTURE.md`.
- Install, update, remove, scriptlet replay, and live-root mutation:
  `docs/modules/feature-ownership.md` slugs `install`, `adopt`, and `ccs`, plus
  `docs/modules/test-fixtures.md`.
- Declarative models, source selection, and replatforming:
  `docs/modules/feature-ownership.md` slug `model`, plus
  `docs/modules/source-selection.md`.
- Generation, bootstrap, and QEMU proof:
  `docs/modules/feature-ownership.md` slugs `generation` and `bootstrap`, plus
  `docs/modules/bootstrap.md` and `docs/operations/bootstrap-selfhosting-vm.md`.
- CCS authoring, conversion, native package contracts, and supported profiles:
  `docs/modules/feature-ownership.md` slugs `ccs`, `packaging`, and `profiles`,
  plus `docs/modules/ccs.md` and `docs/modules/recipe.md`.
- Remi, federation, publication, and service-owned conversion:
  `docs/modules/feature-ownership.md` slug `remi`, plus `docs/modules/remi.md`
  and `docs/modules/federation.md`.
- conaryd routes and package jobs: `docs/modules/feature-ownership.md` slug
  `conaryd`, plus `docs/modules/conaryd.md`.
- `conary-test`, fixtures, and declarative suites:
  `docs/modules/feature-ownership.md` slug `conary-test`, plus
  `docs/INTEGRATION-TESTING.md` and `docs/modules/test-fixtures.md`.
- Agent/MCP operation vocabulary and adapters:
  `docs/modules/feature-ownership.md` slug `agent-mcp`, plus
  `docs/operations/infrastructure.md`.
```

- [ ] **Step 7: Add a drift rule**

Add this section near the end:

```markdown
## Drift Rule

If a "look here first" path, owner slug, proof command, or interaction gate
changes, update `docs/modules/feature-ownership.md` first. Then update this file
only when the high-level orientation or canonical pointer changes.
```

- [ ] **Step 8: Verify the map is slimmer but still useful**

Run:

```bash
wc -l docs/llms/subsystem-map.md
rg -n "Canonical Detail Pointers|Drift Rule|agent-context.sh|feature-ownership.md" docs/llms/subsystem-map.md
bash scripts/agent-context.sh --feature install
bash scripts/agent-context.sh --path docs/llms/subsystem-map.md
```

Expected: line count is materially lower than the pre-edit count, the new
sections are present, `--feature install` prints the install owner card, and the
path route still reports Assistant/contributor guidance.

- [ ] **Step 9: Commit Task 2**

```bash
git add docs/llms/subsystem-map.md
git commit -m "docs(llms): slim subsystem map to orientation index"
```

## Task 3: Refresh Tool Shims And Run Stale-Reference Sweeps

**Files:**
- Modify: `CLAUDE.md`
- Review: `GEMINI.md`
- Review: `REASONIX.md`
- Review: `.github/copilot-instructions.md`
- Reference: `AGENTS.md`
- Reference: `docs/llms/README.md`

**Interfaces:**
- Consumes: Task 1's landing map and Task 2's slim subsystem map.
- Produces: Tool-specific shims that remain compatible without becoming policy sources.

- [ ] **Step 1: Inspect current shims**

Run:

```bash
sed -n '1,120p' CLAUDE.md
sed -n '1,80p' GEMINI.md
sed -n '1,100p' REASONIX.md
sed -n '1,80p' .github/copilot-instructions.md
```

Expected: all four shims point to `AGENTS.md` and canonical docs.

- [ ] **Step 2: Replace `CLAUDE.md` with an import-first shim**

Replace the full contents of `CLAUDE.md` with:

```markdown
@AGENTS.md

# CLAUDE.md

Claude Code reads `CLAUDE.md`, while Conary's shared assistant contract lives in
`AGENTS.md`. The import above keeps Claude aligned with the repo-wide contract
without duplicating it here.

After the imported contract, use:

1. `docs/llms/README.md`
2. `bash scripts/agent-context.sh --feature <slug>` or
   `bash scripts/agent-context.sh --path <file>`
3. The linked canonical docs for architecture, testing, modules, and operations

This file is intentionally thin. Do not turn it into a second source of truth.
If a rule, command, or workflow matters for the repository as a whole, update
`AGENTS.md` or a linked canonical doc instead.

Keep old `.claude/` harness files out of the tracked repo unless the project
adopts a shared Claude-specific harness that needs durable versioned
configuration.
```

- [ ] **Step 3: Confirm the other shims can stay thin**

Run:

```bash
rg -n "AGENTS.md|docs/llms/README.md|agent-context.sh|source of truth" GEMINI.md REASONIX.md .github/copilot-instructions.md
```

Expected: each file points to `AGENTS.md` and/or `docs/llms/README.md`, with no
full repo-policy copy.

- [ ] **Step 4: Only patch non-Claude shims if the previous command shows a gap**

If a non-Claude shim lacks a pointer to both `AGENTS.md` and `docs/llms/README.md`,
add this sentence below its start-here list:

```markdown
Use `scripts/agent-context.sh --feature <slug>` or
`scripts/agent-context.sh --path <file>` for exact owner-card routing.
```

If the previous command already shows adequate pointers, leave the file
unchanged and record that fact in the ledger row update in Task 4.

- [ ] **Step 5: Run stale-reference sweeps**

Run:

```bash
rg -n "\.claude/|Claude-era|old \.claude|parallel manual|second source of truth" AGENTS.md CLAUDE.md GEMINI.md REASONIX.md .github/copilot-instructions.md docs/llms docs/modules docs/superpowers/documentation-accuracy-audit-ledger.tsv
rg -n "Repository sync, remote metadata|Install orchestration|Generation building|long route lists|manual skim" docs/llms/subsystem-map.md
```

Expected: active `.claude/` mentions are limited to intentional "keep retired"
guidance or historical archive notes; the old long route-block phrases no
longer appear in `docs/llms/subsystem-map.md`.

- [ ] **Step 6: Run the focused proof**

Run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
git diff --check
```

Expected: both commands exit 0.

- [ ] **Step 7: Commit Task 3**

```bash
git add CLAUDE.md GEMINI.md REASONIX.md .github/copilot-instructions.md
git commit -m "docs(agents): refresh tool shims for shared routing"
```

If only `CLAUDE.md` changed, stage and commit only `CLAUDE.md`.

## Task 4: Update Audit Metadata And Verify The Slice

**Files:**
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
- Modify if regenerated inventory differs: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Reference: `docs/llms/README.md`
- Reference: `docs/llms/subsystem-map.md`
- Reference: `CLAUDE.md`
- Reference: `GEMINI.md`
- Reference: `REASONIX.md`
- Reference: `.github/copilot-instructions.md`

**Interfaces:**
- Consumes: final edit set from Tasks 1-3.
- Produces: docs-audit metadata and verification evidence for closeout.

- [ ] **Step 1: Identify touched active docs**

Run:

```bash
git diff --name-only HEAD~3..HEAD
git status --short
```

Expected: changed files are limited to active assistant guidance and this plan's
docs-audit metadata when Task 4 is in progress. There should be no Rust code or
product docs unless a prior task explicitly justified it.

- [ ] **Step 2: Update ledger rows for touched active docs**

Update existing rows in `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
for every touched active doc:

- `docs/llms/README.md`: mention fresh-agent landing map, command-first
  `agent-context.sh` routing, doc-family distinction, and agent-doc proof floor.
- `docs/llms/subsystem-map.md`: mention compact orientation index and detailed
  routing delegated to `feature-ownership.md`/`agent-context.sh`.
- `CLAUDE.md`: mention `@AGENTS.md` import-first shim and no `.claude/` harness
  revival.
- Any touched `GEMINI.md`, `REASONIX.md`, or `.github/copilot-instructions.md`:
  mention thin compatibility shim and canonical routing pointers.

Use existing ledger vocabulary:

```text
verified	corrected
```

Do not add new disposition vocabulary.

- [ ] **Step 3: Regenerate inventory only if the file set changed**

Run:

```bash
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
```

Expected: no diff. If there is a diff because a file was added, removed, or
moved during implementation, apply that exact inventory change and rerun the
command until it exits 0.

- [ ] **Step 4: Run focused stale-reference sweeps**

Run:

```bash
rg -n "\.claude/|Claude-era|old \.claude|parallel manual|second source of truth" AGENTS.md CLAUDE.md GEMINI.md REASONIX.md .github/copilot-instructions.md docs/llms docs/modules docs/superpowers/documentation-accuracy-audit-ledger.tsv
rg -n "Repository sync, remote metadata|Install orchestration|Generation building|long route lists|manual skim" docs/llms/subsystem-map.md
rg -n "AGENTS.md|docs/llms/README.md|agent-context.sh" CLAUDE.md GEMINI.md REASONIX.md .github/copilot-instructions.md
```

Expected: no unintended active stale references, no old long-route phrases in
`subsystem-map.md`, and all tool shims still point to the shared contract or
landing map.

- [ ] **Step 5: Run the full verification floor**

Run:

```bash
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
git diff --check
```

Expected:

```text
Documentation truth checks passed.
Documentation audit ledger check passed (--require-complete).
Coherency ledger check passed.
```

The inventory diff and `git diff --check` commands should produce no output and
exit 0.

- [ ] **Step 6: Commit Task 4**

```bash
git add docs/superpowers/documentation-accuracy-audit-ledger.tsv docs/superpowers/documentation-accuracy-audit-inventory.tsv
git commit -m "docs(agents): update audit metadata for routing refresh"
```

If the inventory file did not change, stage and commit only the ledger.

- [ ] **Step 7: Final status proof**

Run:

```bash
git status --short --branch
git log --oneline -5
```

Expected: branch is ahead only by the implementation commits, with no uncommitted
changes.
