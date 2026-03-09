# Dispatch Plugin Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Create a Claude Code plugin that routes implementation work to cheaper LLMs while the orchestrating session handles planning and review.

**Architecture:** A local plugin at `~/.claude/plugins/local/dispatch/` with 3 slash commands (`/dispatch`, `/dispatch-review`, `/dispatch-inline`), a shared shell script for model dispatch, a workflow skill for plan execution, and a TOML config file for model credentials.

**Tech Stack:** Shell script (bash), Markdown commands, TOML config

---

### Task 1: Plugin Scaffold

**Files:**
- Create: `~/.claude/plugins/local/dispatch/.claude-plugin/plugin.json`
- Create: `~/.claude/plugins/local/dispatch/config.example.toml`

**Step 1: Create plugin manifest**

Create `~/.claude/plugins/local/dispatch/.claude-plugin/plugin.json`:
```json
{
  "name": "dispatch",
  "version": "0.1.0",
  "description": "Route implementation work to cheaper LLMs. Orchestrator plans and reviews; dispatched model implements.",
  "author": {
    "name": "Peter"
  },
  "license": "MIT",
  "keywords": ["dispatch", "models", "cost-optimization", "workflow"]
}
```

**Step 2: Create example config**

Create `~/.claude/plugins/local/dispatch/config.example.toml`:
```toml
# Dispatch plugin configuration
# Copy to ~/.claude/dispatch.toml and fill in credentials

enabled = true
default_model = "claude-glm"
timeout_ms = 300000

[models.claude-glm]
base_url = "https://example.com/anthropic"
auth_token = "sk-your-token-here"
model = "glm-5"

[models.claude-deep]
base_url = "https://api.deepseek.com/anthropic"
auth_token = "sk-your-token-here"
model = "deepseek-reasoner"
```

**Step 3: Verify directory structure**

```bash
ls -la ~/.claude/plugins/local/dispatch/.claude-plugin/
```

Expected: `plugin.json` exists

---

### Task 2: Core Dispatch Script

**Files:**
- Create: `~/.claude/plugins/local/dispatch/lib/dispatch.sh`

**Step 1: Write the dispatch script**

This is the core engine. It reads `~/.claude/dispatch.toml`, resolves model credentials, and runs `claude -p` with the right env vars.

Create `~/.claude/plugins/local/dispatch/lib/dispatch.sh`:
```bash
#!/usr/bin/env bash
set -euo pipefail

# dispatch.sh -- Core dispatch engine for the dispatch plugin.
# Reads ~/.claude/dispatch.toml, resolves model credentials, and runs
# claude -p with the correct environment variables.
#
# Usage: dispatch.sh [--model NAME] [--tools TOOLS] [--dir DIR] [--timeout MS] <prompt>
#
# Arguments:
#   --model NAME    Model name from config (default: config's default_model)
#   --tools TOOLS   Comma-separated tool allowlist (default: Edit,Read,Write,Bash,Glob,Grep)
#   --dir DIR       Working directory to run in (default: current directory)
#   --timeout MS    Timeout in milliseconds (default: config's timeout_ms or 300000)
#   <prompt>        The prompt to send to the dispatched claude instance
#
# Environment:
#   DISPATCH_CONFIG  Path to config file (default: ~/.claude/dispatch.toml)
#
# Exit codes:
#   0  Success
#   1  Config error (missing file, missing model, etc.)
#   2  Claude process failed

CONFIG="${DISPATCH_CONFIG:-$HOME/.claude/dispatch.toml}"

die() { echo "dispatch: error: $*" >&2; exit 1; }

# --- Parse arguments ---
MODEL=""
TOOLS="Edit,Read,Write,Bash,Glob,Grep"
DIR=""
TIMEOUT=""
PROMPT=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --model)  MODEL="$2"; shift 2 ;;
        --tools)  TOOLS="$2"; shift 2 ;;
        --dir)    DIR="$2"; shift 2 ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        --)       shift; PROMPT="$*"; break ;;
        -*)       die "unknown flag: $1" ;;
        *)        PROMPT="$*"; break ;;
    esac
done

[[ -n "$PROMPT" ]] || die "no prompt provided"
[[ -f "$CONFIG" ]] || die "config not found: $CONFIG"

# --- Read config ---
# Read a top-level key (before any [section])
read_top_key() {
    local key="$1"
    awk -v k="$key" '
        /^\[/ { exit }
        $0 ~ "^"k"[[:space:]]*=" {
            sub(/^[^=]*=[[:space:]]*/, "")
            gsub(/^"|"$/, "")
            print
            exit
        }
    ' "$CONFIG"
}

# Read a key from a [models.NAME] section
read_model_key() {
    local model_name="$1" key="$2"
    awk -v section="[models.$model_name]" -v k="$key" '
        $0 == section { found=1; next }
        found && /^\[/ { exit }
        found && $0 ~ "^"k"[[:space:]]*=" {
            sub(/^[^=]*=[[:space:]]*/, "")
            gsub(/^"|"$/, "")
            print
            exit
        }
    ' "$CONFIG"
}

# Check enabled
ENABLED=$(read_top_key "enabled")
[[ "$ENABLED" != "false" ]] || die "dispatch is disabled in config"

# Resolve model name
[[ -n "$MODEL" ]] || MODEL=$(read_top_key "default_model")
[[ -n "$MODEL" ]] || die "no model specified and no default_model in config"

# Resolve timeout
[[ -n "$TIMEOUT" ]] || TIMEOUT=$(read_top_key "timeout_ms")
[[ -n "$TIMEOUT" ]] || TIMEOUT="300000"

# Read model credentials
BASE_URL=$(read_model_key "$MODEL" "base_url")
AUTH_TOKEN=$(read_model_key "$MODEL" "auth_token")
MODEL_ID=$(read_model_key "$MODEL" "model")

[[ -n "$BASE_URL" ]]   || die "model '$MODEL': missing base_url"
[[ -n "$AUTH_TOKEN" ]]  || die "model '$MODEL': missing auth_token"
[[ -n "$MODEL_ID" ]]   || die "model '$MODEL': missing model"

# --- Dispatch ---
CMD=(
    env -u CLAUDECODE -u CLAUDE_CODE
    ANTHROPIC_BASE_URL="$BASE_URL"
    ANTHROPIC_AUTH_TOKEN="$AUTH_TOKEN"
    ANTHROPIC_MODEL="$MODEL_ID"
    ANTHROPIC_SMALL_FAST_MODEL="$MODEL_ID"
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1
    API_TIMEOUT_MS="$TIMEOUT"
    claude -p "$PROMPT" --allowedTools "$TOOLS"
)

if [[ -n "$DIR" ]]; then
    cd "$DIR"
fi

exec "${CMD[@]}"
```

**Step 2: Make executable**

```bash
chmod +x ~/.claude/plugins/local/dispatch/lib/dispatch.sh
```

**Step 3: Test the script parses config correctly**

Create `~/.claude/dispatch.toml` from the user's existing bash function credentials, then test:

```bash
~/.claude/plugins/local/dispatch/lib/dispatch.sh --model claude-glm -- "echo hello"
```

Expected: Claude runs with GLM model, responds to "echo hello"

**Step 4: Commit**

```bash
cd ~/.claude/plugins/local/dispatch && git init && git add -A
git commit -m "feat: add core dispatch script"
```

---

### Task 3: /dispatch Command (Worktree Implementation)

**Files:**
- Create: `~/.claude/plugins/local/dispatch/commands/dispatch.md`

**Step 1: Write the command**

Create `~/.claude/plugins/local/dispatch/commands/dispatch.md`:
```markdown
---
description: Dispatch implementation work to a cheaper LLM in an isolated git worktree
argument-hint: [--model name] [--tools list] <prompt>
---

# /dispatch -- Worktree Implementation Dispatch

The user wants to dispatch implementation work to a cheaper LLM. Follow these steps exactly:

## Step 1: Parse Arguments

The user's input is: $ARGUMENTS

Parse the following from the arguments:
- `--model <name>` (optional, default: from config)
- `--tools <list>` (optional, default: Edit,Read,Write,Bash,Glob,Grep)
- Everything else is the prompt

## Step 2: Create Git Worktree

Create an isolated worktree for the dispatched work:

```bash
BRANCH="dispatch/$(date +%s)"
WORKTREE_DIR=".worktrees/$BRANCH"
git worktree add "$WORKTREE_DIR" -b "$BRANCH"
```

Verify `.worktrees` is in `.gitignore`. If not, add it.

## Step 3: Dispatch to Cheaper LLM

Run the dispatch script. Pass the full prompt and the worktree directory:

```bash
bash ${CLAUDE_PLUGIN_ROOT}/lib/dispatch.sh \
  --dir "$WORKTREE_DIR" \
  [--model <model>] \
  [--tools <tools>] \
  -- "<prompt>"
```

Wait for it to complete. Capture the output.

## Step 4: Collect Results

After the dispatch completes, gather:

1. The diff: `git -C "$WORKTREE_DIR" diff HEAD`
2. Test results (if applicable): run `cargo test` or equivalent in the worktree
3. The dispatch output (what the LLM reported)

## Step 5: Review and Present

Review the diff yourself (you are the orchestrator on the more capable model). Then present to the user:

1. Summary of what the dispatched LLM did
2. The diff (key changes)
3. Test results if available
4. Your assessment: approve, needs fixes, or discard
5. Options:
   - **Merge**: `git merge dispatch/<timestamp>` then cleanup worktree
   - **Fix**: Run another `/dispatch-inline` in the worktree to fix issues
   - **Discard**: `git worktree remove "$WORKTREE_DIR" && git branch -D "$BRANCH"`

Wait for user decision before acting.
```

**Step 2: Commit**

```bash
cd ~/.claude/plugins/local/dispatch && git add commands/dispatch.md
git commit -m "feat: add /dispatch command (worktree implementation)"
```

---

### Task 4: /dispatch-review Command (Read-Only)

**Files:**
- Create: `~/.claude/plugins/local/dispatch/commands/dispatch-review.md`

**Step 1: Write the command**

Create `~/.claude/plugins/local/dispatch/commands/dispatch-review.md`:
```markdown
---
description: Dispatch read-only analysis/review work to a cheaper LLM
argument-hint: [--model name] [--tools list] <prompt>
---

# /dispatch-review -- Read-Only Analysis Dispatch

The user wants to dispatch read-only analysis work to a cheaper LLM. This runs in the current directory with no git changes.

## Step 1: Parse Arguments

The user's input is: $ARGUMENTS

Parse the following:
- `--model <name>` (optional, default: from config)
- `--tools <list>` (optional, default: Read,Bash,Glob,Grep)
- Everything else is the prompt

Note the default tools are READ-ONLY (no Edit, Write).

## Step 2: Dispatch to Cheaper LLM

Run the dispatch script in the current directory:

```bash
bash ${CLAUDE_PLUGIN_ROOT}/lib/dispatch.sh \
  [--model <model>] \
  --tools "${tools:-Read,Bash,Glob,Grep}" \
  -- "<prompt>"
```

Wait for it to complete. Capture the output.

## Step 3: Present Results

Present the dispatched LLM's analysis directly to the user. Add your own brief assessment if the analysis seems incomplete or incorrect (you are the more capable orchestrator model).
```

**Step 2: Commit**

```bash
cd ~/.claude/plugins/local/dispatch && git add commands/dispatch-review.md
git commit -m "feat: add /dispatch-review command (read-only analysis)"
```

---

### Task 5: /dispatch-inline Command (In-Place Edits)

**Files:**
- Create: `~/.claude/plugins/local/dispatch/commands/dispatch-inline.md`

**Step 1: Write the command**

Create `~/.claude/plugins/local/dispatch/commands/dispatch-inline.md`:
```markdown
---
description: Dispatch in-place edits to a cheaper LLM (no worktree isolation)
argument-hint: [--model name] [--tools list] <prompt>
---

# /dispatch-inline -- In-Place Edit Dispatch

The user wants to dispatch targeted edit work to a cheaper LLM directly in the current working directory. No worktree isolation -- use this for small, well-scoped changes.

## Step 1: Parse Arguments

The user's input is: $ARGUMENTS

Parse the following:
- `--model <name>` (optional, default: from config)
- `--tools <list>` (optional, default: Edit,Read,Write,Bash,Glob,Grep)
- Everything else is the prompt

## Step 2: Snapshot Current State

Before dispatching, capture the current state so we can review/revert:

```bash
git stash push -m "dispatch-inline-backup-$(date +%s)" --include-untracked 2>/dev/null || true
git stash pop
```

Actually, just record the current HEAD and any uncommitted changes exist:

```bash
BEFORE_SHA=$(git rev-parse HEAD)
git diff --stat
```

## Step 3: Dispatch to Cheaper LLM

Run the dispatch script in the current directory:

```bash
bash ${CLAUDE_PLUGIN_ROOT}/lib/dispatch.sh \
  [--model <model>] \
  [--tools <tools>] \
  -- "<prompt>"
```

Wait for it to complete. Capture the output.

## Step 4: Review Changes

After dispatch completes:

1. Run `git diff` to see what changed
2. Review the diff yourself (orchestrator model)
3. Run tests if applicable
4. Present to user:
   - Summary of changes
   - The diff
   - Your assessment
   - Options:
     - **Accept**: Keep changes as-is
     - **Fix**: Make targeted corrections yourself
     - **Revert**: `git checkout .` to undo all changes
```

**Step 2: Commit**

```bash
cd ~/.claude/plugins/local/dispatch && git add commands/dispatch-inline.md
git commit -m "feat: add /dispatch-inline command (in-place edits)"
```

---

### Task 6: Dispatch-Driven Development Skill

**Files:**
- Create: `~/.claude/plugins/local/dispatch/skills/dispatch-driven-development/SKILL.md`

**Step 1: Write the skill**

This is the workflow skill that replaces superpowers:subagent-driven-development with dispatch-based execution. The orchestrator reads this skill and follows it instead of using the Agent tool.

Create `~/.claude/plugins/local/dispatch/skills/dispatch-driven-development/SKILL.md`:
```markdown
---
name: dispatch-driven-development
description: Use when executing implementation plans with independent tasks. Routes implementation work to cheaper LLMs via /dispatch while orchestrator handles planning, task extraction, and review. Replaces subagent-driven-development for cost optimization.
---

# Dispatch-Driven Development

Execute implementation plans by dispatching work to cheaper LLMs via `/dispatch`. The orchestrator (you, on the capable model) handles planning, context extraction, and quality review. The dispatched model handles implementation grunt work.

**Core principle:** Orchestrator plans and reviews (expensive model, minimal tokens). Dispatched model implements (cheap model, heavy tokens).

## When to Use

Use this instead of superpowers:subagent-driven-development when:
- User has dispatch plugin configured (`~/.claude/dispatch.toml` exists with `enabled = true`)
- Tasks are independent enough to execute in isolation
- Implementation is well-specified (the cheap model needs clear instructions)

Fall back to native subagents when:
- `~/.claude/dispatch.toml` has `enabled = false` or doesn't exist
- User explicitly says "use native subagents"
- Task requires back-and-forth dialogue (cheap models can't ask questions via dispatch)

## The Process

### Phase 1: Plan Extraction (Orchestrator)

1. Read the plan file once, extract ALL tasks with full text
2. Note dependencies between tasks
3. Create TodoWrite with all tasks
4. Identify which tasks can run in parallel (no shared files, no dependencies)

### Phase 2: Per-Task Dispatch

For each task (or group of parallel tasks):

**Step 1: Craft the prompt (Orchestrator)**

Write a detailed, self-contained prompt for the dispatched model. Include:
- Full task text from the plan
- Relevant context (file paths, architecture, conventions)
- CLAUDE.md rules the model must follow
- Exact acceptance criteria
- Build/test commands to run

The dispatched model has zero context -- give it everything it needs.

**Step 2: Dispatch implementation**

For independent tasks that can run in parallel, launch multiple dispatches as background Bash processes:

```bash
bash ${CLAUDE_PLUGIN_ROOT}/lib/dispatch.sh --dir ".worktrees/dispatch/task-1" -- "<prompt>" &
bash ${CLAUDE_PLUGIN_ROOT}/lib/dispatch.sh --dir ".worktrees/dispatch/task-2" -- "<prompt>" &
wait
```

For sequential tasks, use `/dispatch` one at a time.

**Step 3: Review (Orchestrator)**

After dispatch completes:
1. Read the diff from the worktree
2. Verify it matches the spec (you are the spec compliance reviewer)
3. Run tests: `cargo test` or equivalent
4. If issues found: `/dispatch-inline` in the worktree to fix, or fix yourself if trivial
5. If clean: merge the worktree branch

**Step 4: Quality spot-check (Orchestrator)**

Optionally dispatch a review to the cheap model too:

```
/dispatch-review "Review this diff for code quality issues: [paste diff]"
```

Then spot-check the review yourself. This is cheaper than doing a full review on the expensive model.

### Phase 3: Integration (Orchestrator)

After all tasks complete:
1. Run full test suite
2. Review overall changes
3. Use superpowers:finishing-a-development-branch

## Prompt Template for Dispatched Model

```
You are implementing a task in the [project-name] codebase at [directory].

## Task

[FULL task text from plan]

## Context

[Architecture notes, relevant file paths, conventions]

## Rules

[Key rules from CLAUDE.md that apply -- file headers, naming, etc.]

## Your Job

1. Read the relevant files to understand existing code
2. Implement exactly what the task specifies
3. Write tests if specified
4. Run: [test command]
5. If tests pass, commit with message: "[conventional commit message]"
6. Report what you implemented and test results

Do NOT over-engineer. Do NOT add features not in the spec.
Do NOT modify files outside the task scope.
```

## Parallel Dispatch

When tasks are independent (no shared files, no dependency chain):

1. Create one worktree per task
2. Launch dispatch processes in background
3. Wait for all to complete
4. Review each result
5. Merge sequentially (check for conflicts)

```bash
# Create worktrees
git worktree add .worktrees/dispatch/task-1 -b dispatch/task-1
git worktree add .worktrees/dispatch/task-2 -b dispatch/task-2

# Dispatch in parallel
bash ${CLAUDE_PLUGIN_ROOT}/lib/dispatch.sh --dir .worktrees/dispatch/task-1 -- "prompt 1" &
PID1=$!
bash ${CLAUDE_PLUGIN_ROOT}/lib/dispatch.sh --dir .worktrees/dispatch/task-2 -- "prompt 2" &
PID2=$!
wait $PID1 $PID2

# Review and merge each
```

## Cost Model

| Role | Model | Token Usage |
|------|-------|-------------|
| Plan extraction | Orchestrator (expensive) | Low -- reading plan, crafting prompts |
| Implementation | Dispatched (cheap) | High -- reading code, writing code, running tests |
| Spec review | Orchestrator (expensive) | Low -- reading a diff |
| Quality review | Dispatched (cheap) | Medium -- reading diff, analyzing |
| Spot-check | Orchestrator (expensive) | Very low -- scanning review output |

## Red Flags

**Never:**
- Dispatch without a clear, self-contained prompt
- Skip the orchestrator review of dispatch results
- Merge without running tests
- Dispatch tasks with tight dependencies in parallel
- Assume the dispatched model knows project conventions (include them in prompt)

**Always:**
- Include full task spec in dispatch prompt
- Include relevant CLAUDE.md rules
- Review every diff before merging
- Run tests after merging
- Clean up worktrees after merging

## Integration

**Replaces:** superpowers:subagent-driven-development (when dispatch is enabled)
**Uses:** superpowers:finishing-a-development-branch (after all tasks)
**Toggle:** `~/.claude/dispatch.toml` enabled flag, or CLAUDE.md preference line
```

**Step 2: Commit**

```bash
cd ~/.claude/plugins/local/dispatch && git add skills/
git commit -m "feat: add dispatch-driven-development skill"
```

---

### Task 7: Create User Config

**Files:**
- Create: `~/.claude/dispatch.toml`

**Step 1: Create config with user's actual credentials**

Create `~/.claude/dispatch.toml` using the credentials from the user's `.bashrc` functions:

```toml
enabled = true
default_model = "claude-glm"
timeout_ms = 300000

[models.claude-glm]
base_url = "https://coding-intl.dashscope.aliyuncs.com/apps/anthropic"
auth_token = "sk-sp-13fa54ae083048279762e81ddbed2a78"
model = "glm-5"

[models.claude-deep]
base_url = "https://api.deepseek.com/anthropic"
auth_token = "sk-61de9e86a55c476087fdbc4ba34bf1b5"
model = "deepseek-reasoner"

[models.claude-qwen]
base_url = "https://coding-intl.dashscope.aliyuncs.com/apps/anthropic"
auth_token = "sk-sp-13fa54ae083048279762e81ddbed2a78"
model = "qwen3.5-plus"

[models.claude-kimi]
base_url = "https://coding-intl.dashscope.aliyuncs.com/apps/anthropic"
auth_token = "sk-sp-13fa54ae083048279762e81ddbed2a78"
model = "kimi-k2.5"

[models.claude-minimax]
base_url = "https://coding-intl.dashscope.aliyuncs.com/apps/anthropic"
auth_token = "sk-sp-13fa54ae083048279762e81ddbed2a78"
model = "MiniMax-M2.5"
```

**Step 2: Verify config is readable by dispatch script**

```bash
~/.claude/plugins/local/dispatch/lib/dispatch.sh --model claude-glm -- "respond with just the word HELLO"
```

Expected: GLM model responds with HELLO (or similar)

---

### Task 8: Add CLAUDE.md Preference

**Files:**
- Modify: `/home/peter/Conary/CLAUDE.md`

**Step 1: Add dispatch preference**

Add to the end of the `## Agents` section in CLAUDE.md:

```markdown
## Dispatch Preference

When executing implementation plans, prefer `/dispatch` over Agent tool for implementation subagents. This routes work to cheaper LLMs while the orchestrator handles planning and review. See `~/.claude/dispatch.toml` for model configuration. To disable, set `enabled = false` in that file or remove this section.
```

**Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: add dispatch preference to CLAUDE.md"
```

---

### Task 9: Install and Test Plugin

**Step 1: Install the plugin**

```bash
claude plugins add ~/.claude/plugins/local/dispatch
```

Or if local plugins are loaded automatically, verify it appears:

```bash
claude plugins list
```

**Step 2: End-to-end test**

Start a new Claude Code session and test:

1. `/dispatch-review "list the files in conary-test/src/ and describe what each module does"`
2. `/dispatch-inline "add a comment to the top of conary-test/src/lib.rs saying // test comment" ` -- then verify and revert
3. `/dispatch "create a file called /tmp/dispatch-test.txt with the text 'dispatch works'"` -- verify worktree was created, file exists, cleanup

**Step 3: Verify dispatch-driven-development skill loads**

The skill should appear when relevant work is being done. Verify it's listed in available skills.
