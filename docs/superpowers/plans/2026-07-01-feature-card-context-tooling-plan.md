# Feature Card Context Tooling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. If you are an external implementer (GPT/Codex) without those skills, execute tasks strictly in order, run every verification step, and commit after each task.

**Goal:** Make `docs/modules/feature-ownership.md` the single machine-readable source of path→feature routing by adding `Slug`/`Paths` fields to all 13 cards, building `scripts/agent-context.sh` (packet/routing/validation/run tool), and hard-cutting `scripts/maintainability-drift-report.sh` and `scripts/agentic-plan-review.sh` over to it.

**Architecture:** One bash+awk parser lives inside `scripts/agent-context.sh`; everything else (drift report, plan reviewer) shells out to it. The Markdown map stays canonical; a `--validate` mode enforced in CI keeps it parseable and honest. This is a hard cutover: the drift report's `feature_hint_for_path()` and the plan reviewer's hardcoded context list are deleted, not shimmed.

**Tech Stack:** bash (`set -euo pipefail`), POSIX awk, git plumbing (`ls-files`, `diff --name-only`). No Rust/product code changes.

**Source spec:** `docs/superpowers/specs/2026-07-01-feature-card-context-tooling-design.md` (read it before starting; this plan implements it 1:1).

## Global Constraints

- Script conventions: `#!/usr/bin/env bash`, `set -euo pipefail`, `usage()` heredoc to stderr, `fail()` printing `ERROR: ...` to stderr with exit 1, `cd "$(git rev-parse --show-toplevel)"` at top. New scripts get `chmod +x`.
- **Never rename any `## ` card heading** in `docs/modules/feature-ownership.md` — `scripts/check-coherency-ledger.sh` reads headings as the allowed owner set.
- Public distro support claims stay exactly: Fedora 44, Ubuntu 26.04, and Arch.
- Hard cutover: exactly one map parser (in `agent-context.sh`); no compatibility fallbacks left in the two cutover scripts.
- Tool output is stable plain text only. No JSON/TOML mode.
- Under `set -e`, never use `[[ ... ]] && cmd` as the last statement of a function or loop body — use `if` blocks (a false test would abort the script).
- awk must stay POSIX (no gawk-isms); use `\037` (octal unit separator) in printf, not `\x1f`.
- Commit style: conventional prefixes, short imperative subjects (e.g. `feat(scripts): ...`, `docs(feature-ownership): ...`).
- Every doc file that becomes tracked must appear in the regenerated docs-audit inventory and have exactly one ledger row (CI enforces completeness).

## Locked Decisions (answers to the spec's "Plan Questions")

These were left open by the design; this plan locks them. Flag to the card owner in the PR description, but implement as written:

1. **Paths globs for the four "derive" cards** — see the full translation table in Task 2. All proposed globs were verified against `git ls-files` on 2026-07-01 (every glob matches ≥1 tracked file; no equal-specificity overlaps). One deliberate extension beyond the spec's translation table: `install` also gets `apps/conary/src/commands/remove/*` (the card's Start here already lists eight `remove/` child modules; without the glob they route to "no hint").
2. **`--run` ships in this slice.** The spec's testing strategy already specifies `--run` fixtures, and it is ~30 lines once packet extraction exists (Task 8).
3. **Supported Target Profiles Interaction gate wording** — exact text in Task 2, authored from the card's Neighbor systems and existing proof commands (all three commands verified to exist as test targets).
4. **Brief-hint format stays shape-compatible** with the current drift report: `<card heading> | focused: <Focused proof text, backticks stripped> | gate: <Interaction gate text, backticks stripped>`, under a `- <path>` line indented by two spaces. Content now comes from the cards (so hint text changes from the old hand-condensed strings — headings are now the full card headings, e.g. `Agent/MCP Operation Surfaces`); the updated tests in Task 10 pin the new content.

## File Structure

- Create: `scripts/agent-context.sh` (single parser + all modes)
- Create: `scripts/test-agent-context.sh` (fixture + real-map tests)
- Modify: `docs/modules/feature-ownership.md` (schema fields, 13 cards, profiles gate, packaging safety notes)
- Modify: `scripts/maintainability-drift-report.sh` (delete `feature_hint_for_path()` + `collect_paths()`, delegate)
- Modify: `scripts/test-maintainability-drift-report.sh` (new expected hint strings)
- Modify: `scripts/agentic-plan-review.sh` (`--feature` flag, generated context, pressure points)
- Modify: `scripts/test-agentic-plan-review.sh` (new assertions)
- Modify: `.github/workflows/pr-gate.yml` (two new docs-truth steps)
- Modify: `AGENTS.md`, `docs/llms/README.md`, `.github/PULL_REQUEST_TEMPLATE.md` (pointers)
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`, `docs/superpowers/documentation-accuracy-audit-ledger.tsv` (track spec + this plan)

---

### Task 1: Track the design spec and this plan in the docs audit

The spec file is currently untracked, so `scripts/docs-audit-inventory.sh` (which reads `git ls-files`) cannot see it. Once either doc is committed without audit metadata, the docs-truth CI job fails. Fix that first so every later commit is CI-green.

**Files:**
- Stage: `docs/superpowers/specs/2026-07-01-feature-card-context-tooling-design.md`
- Stage: `docs/superpowers/plans/2026-07-01-feature-card-context-tooling-plan.md` (this file)
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

**Interfaces:**
- Consumes: nothing.
- Produces: a tracked spec/plan pair with complete audit rows; all later tasks commit on top.

- [ ] **Step 1: Stage both docs so the inventory script can see them**

```bash
git add docs/superpowers/specs/2026-07-01-feature-card-context-tooling-design.md \
        docs/superpowers/plans/2026-07-01-feature-card-context-tooling-plan.md
```

- [ ] **Step 2: Regenerate the inventory**

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
git diff docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Expected: exactly two added rows, both `planning`/`maintainer`:
`docs/superpowers/plans/2026-07-01-feature-card-context-tooling-plan.md` and
`docs/superpowers/specs/2026-07-01-feature-card-context-tooling-design.md`.

- [ ] **Step 3: Append two ledger rows**

Append these two lines to `docs/superpowers/documentation-accuracy-audit-ledger.tsv` (fields are TAB-separated; 9 fields per row matching the header `origin_path path family audience claim_clusters evidence_sources status disposition notes`):

```text
docs/superpowers/specs/2026-07-01-feature-card-context-tooling-design.md	docs/superpowers/specs/2026-07-01-feature-card-context-tooling-design.md	planning	maintainer	feature-card-tooling; agent-context; drift-report-cutover; plan-review-cutover	docs/modules/feature-ownership.md; scripts/maintainability-drift-report.sh; scripts/agentic-plan-review.sh	verified	verified-no-change	Design spec for scripts/agent-context.sh, the Slug/Paths card schema fields, and the drift-report/plan-review hard cutovers.
docs/superpowers/plans/2026-07-01-feature-card-context-tooling-plan.md	docs/superpowers/plans/2026-07-01-feature-card-context-tooling-plan.md	planning	maintainer	feature-card-tooling; agent-context; implementation-plan	docs/superpowers/specs/2026-07-01-feature-card-context-tooling-design.md; docs/modules/feature-ownership.md	verified	verified-no-change	Implementation plan for the feature-card context tooling slice; tasks mirror the 2026-07-01 design spec.
```

- [ ] **Step 4: Verify the docs gates pass**

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv <(bash scripts/docs-audit-inventory.sh)
bash scripts/check-doc-truth.sh
```

Expected: ledger check prints `Documentation audit ledger check passed (--require-complete).`; diff is empty; doc-truth passes. If `check-doc-truth.sh` flags wording in the new docs, adjust the flagged phrase in the doc (do not touch the checker).

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/documentation-accuracy-audit-inventory.tsv docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs(superpowers): track feature-card context tooling spec and plan"
```

---

### Task 2: Add Slug/Paths schema fields and complete the map

**Files:**
- Modify: `docs/modules/feature-ownership.md`

**Interfaces:**
- Consumes: nothing.
- Produces: 13 cards each carrying nine fields — `Slug`, `Capability`, `Start here`, `Neighbor systems`, `Paths`, `Focused proof`, `Interaction gate`, `Docs to update`, `Safety notes` — which the Task 4 parser and Task 7 validator depend on. Slugs (in card order): `dispatch`, `install`, `adopt`, `model`, `generation`, `ccs`, `packaging`, `profiles`, `remi`, `conaryd`, `bootstrap`, `conary-test`, `agent-mcp`.

- [ ] **Step 1: Bump frontmatter**

Change the frontmatter to:

```yaml
---
last_updated: 2026-07-01
revision: 20
summary: Add machine-readable Slug and Paths routing fields
---
```

- [ ] **Step 2: Document the two new fields in the Card Schema section**

In the `## Card Schema` bullet list, add a `Slug` bullet **before** the `Capability` bullet and a `Paths` bullet **after** the `Neighbor systems` bullet:

```markdown
- **Slug:** short unique kebab-case identifier; the first field of each card,
  used by `scripts/agent-context.sh` to select cards.
```

```markdown
- **Paths:** semicolon-separated, backtick-quoted glob patterns that route
  repository paths to this card. Globs match shell-style over repo-relative
  paths (`*` may span `/`); the most specific match wins, where specificity is
  the length of the literal prefix before the first `*`, `?`, or `[`. Two
  cards matching a path at equal specificity is a validation error
  (`scripts/agent-context.sh --validate`).
```

- [ ] **Step 3: Add Slug and Paths to all 13 cards**

For each card: insert `**Slug:** <slug>` as its own paragraph immediately after the card's `## ` heading (before Capability), and insert the `**Paths:**` paragraph immediately after the `**Neighbor systems:**` paragraph. Exact values (globs backtick-quoted, semicolon-separated, paragraph ends with a period):

| Card heading | Slug | Paths globs |
|---|---|---|
| CLI Dispatch And Command Routing | `dispatch` | `apps/conary/src/dispatch.rs`; `apps/conary/src/dispatch/*`; `apps/conary/src/cli/*`; `apps/conary/src/command_risk.rs`; `apps/conary/src/live_host_safety.rs` |
| Native Package Install, Update, Remove, And Live-Root Mutation | `install` | `apps/conary/src/commands/install/*`; `apps/conary/src/commands/update/*`; `apps/conary/src/commands/remove.rs`; `apps/conary/src/commands/remove/*` |
| Adoption, Unadoption, And Native-Authority Handoff | `adopt` | `apps/conary/src/commands/adopt/*` |
| Declarative System Models And Replatform Planning | `model` | `apps/conary/src/commands/model.rs`; `apps/conary/src/commands/model/*`; `crates/conary-core/src/model/*` |
| Generation Build, Switch, Recovery, And Export | `generation` | `crates/conary-core/src/generation/*` |
| CCS Authoring, Conversion, Install, And Legacy Replay | `ccs` | `crates/conary-core/src/ccs/*`; `apps/conary/src/commands/ccs/*` |
| Packaging, Try Sessions, And Static Repository Publishing | `packaging` | see block below |
| Supported Target Profiles | `profiles` | `crates/conary-core/src/repository/supported_profiles/*` |
| Remi Publication, Serving, Admin, And Fixture Artifacts | `remi` | `apps/remi/*` |
| conaryd Package Jobs And Daemon Routes | `conaryd` | `apps/conaryd/*` |
| Bootstrap And Self-Hosting | `bootstrap` | `apps/conary/src/commands/bootstrap/*`; `apps/conary-test/src/bootstrap.rs`; `crates/conary-bootstrap/*`; `docs/modules/bootstrap.md`; `docs/operations/bootstrap-selfhosting-vm.md`; `docs/operations/bootstrap-follow-up-investigations.md` |
| conary-test Integration Execution | `conary-test` | `apps/conary-test/*`; `apps/conary/tests/integration/remi/manifests/*` |
| Agent/MCP Operation Surfaces | `agent-mcp` | `crates/conary-agent-contract/*`; `crates/conary-mcp/*`; `apps/remi/src/server/mcp.rs`; `apps/conary-test/src/server/mcp.rs` |

Packaging card Paths paragraph (verbatim):

```markdown
**Paths:** `docs/specs/static-repo-format-v1.md`;
`docs/guides/first-package.md`; `crates/conary-core/src/recipe/*`;
`crates/conary-core/src/diagnostics/*`;
`apps/conary/src/commands/packaging_mcp/*`;
`crates/conary-core/src/db/models/try_session.rs`;
`apps/conary/src/commands/new.rs`; `apps/conary/src/commands/publish.rs`;
`apps/conary/src/commands/cook.rs`; `apps/conary/src/commands/record_mode/*`;
`apps/conary/src/commands/diagnostics.rs`;
`apps/conary/src/commands/operation_records.rs`;
`apps/conary/src/commands/hermetic_config.rs`;
`apps/conary/src/commands/hermetic_state.rs`;
`apps/conary/src/commands/try_session/*`;
`apps/conary/src/commands/repo_static.rs`;
`apps/conary/tests/packaging_m*.rs`;
`crates/conary-core/src/ccs/attestation.rs`;
`crates/conary-core/src/ccs/signing.rs`;
`crates/conary-core/src/repository/static_repo/*`;
`crates/conary-core/src/trust/*`; `crates/conary-core/src/container/*`.
```

Intended most-specific-wins overlaps (all resolve by specificity, no ties):
`apps/remi/src/server/mcp.rs` → agent-mcp (beats `apps/remi/*`);
`apps/conary-test/src/server/mcp.rs` → agent-mcp and
`apps/conary-test/src/bootstrap.rs` → bootstrap (both beat `apps/conary-test/*`);
`crates/conary-core/src/ccs/attestation.rs` and `.../signing.rs` → packaging
(beat `crates/conary-core/src/ccs/*`). `apps/remi/src/federation/*` folds into
the `remi` card via `apps/remi/*` (the drift report's federation pseudo-branch
dies in Task 10).

- [ ] **Step 4: Add the missing Interaction gate to Supported Target Profiles**

Insert after that card's `**Focused proof:**` paragraph:

```markdown
**Interaction gate:** `cargo test -p remi`;
`cargo test -p conary --test packaging_m4c`;
`cargo test -p conary --test conversion_integration golden_conversion` when
profile changes cross Remi serving routes, conversion lookup or parser
dispatch, native release upload, or CCS v2 lifecycle policy.
```

- [ ] **Step 5: Preserve plan-review goal 7's pressure points in the packaging card**

Append this sentence to the end of the packaging card's `**Safety notes:**` paragraph (before the `- Record-mode spike:` bullet block):

```markdown
Recorded-draft recipes must keep refusing publication until validated —
`publish_context.rs` and `publish_gate.rs` enforce that refusal — and Remi
release uploads stay behind the trusted build-attestation signer policy.
```

- [ ] **Step 6: Verify the map gates still pass**

```bash
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

Expected: all pass (headings unchanged, so owner validation is unaffected).

- [ ] **Step 7: Commit**

```bash
git add docs/modules/feature-ownership.md
git commit -m "docs(feature-ownership): add slug and paths routing fields to all cards"
```

---

### Task 3: Scaffold `scripts/agent-context.sh` — argument contract

**Files:**
- Create: `scripts/agent-context.sh`
- Create: `scripts/test-agent-context.sh`

**Interfaces:**
- Consumes: nothing.
- Produces: CLI contract — modes `--feature <slug>`, `--path <path>`, `--changed`, `--list`, `--validate` (exactly one); options `--base <ref>`, `--all`, `--brief`, `--run <focused|gate>`, `--map <path>`. Globals `mode`, `feature_slug`, `route_path_arg`, `base_ref`, `scan_all`, `brief`, `run_kind`, `map_file` that later tasks read.

- [ ] **Step 1: Write the failing test**

Create `scripts/test-agent-context.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

script="$repo_root/scripts/agent-context.sh"
[[ -x "$script" ]] || fail "scripts/agent-context.sh is not executable"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

help_output="$("$script" --help 2>&1)"
grep -q "Usage: scripts/agent-context.sh" <<<"$help_output" \
    || fail "help output did not include usage"

if "$script" >"$tmp/no-mode.out" 2>&1; then
    fail "missing mode unexpectedly succeeded"
fi
grep -q "Usage: scripts/agent-context.sh" "$tmp/no-mode.out" \
    || fail "missing mode did not print usage"

if "$script" --list --validate >"$tmp/two-modes.out" 2>&1; then
    fail "two modes unexpectedly succeeded"
fi

if "$script" --list --nonsense >"$tmp/bad-flag.out" 2>&1; then
    fail "unknown flag unexpectedly succeeded"
fi

if "$script" --list --map "$tmp/does-not-exist.md" >"$tmp/bad-map.out" 2>&1; then
    fail "missing map file unexpectedly succeeded"
fi
grep -q "map file not found" "$tmp/bad-map.out" \
    || fail "missing map file did not produce a clear error"

if "$script" --list --base HEAD >"$tmp/bad-base-combo.out" 2>&1; then
    fail "--base without --changed unexpectedly succeeded"
fi

if "$script" --changed --brief >"$tmp/bad-brief-combo.out" 2>&1; then
    fail "--brief with --changed unexpectedly succeeded"
fi

if "$script" --feature alpha --run nonsense >"$tmp/bad-run.out" 2>&1; then
    fail "invalid --run kind unexpectedly succeeded"
fi
grep -q "invalid --run kind" "$tmp/bad-run.out" \
    || fail "invalid --run kind did not produce a clear error"

echo "agent-context tests passed."
```

```bash
chmod +x scripts/test-agent-context.sh
```

- [ ] **Step 2: Run it to verify it fails**

Run: `bash scripts/test-agent-context.sh`
Expected: FAIL with `scripts/agent-context.sh is not executable`.

- [ ] **Step 3: Write the scaffold**

Create `scripts/agent-context.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

usage() {
    cat >&2 <<'EOF'
Usage: scripts/agent-context.sh <mode> [options]

Print feature-card task context from docs/modules/feature-ownership.md.

Modes (exactly one):
  --feature <slug>        Print the task packet for one card.
  --path <path>           Route one repo path to its owning card; print packet.
  --changed               Route all changed paths; print brief hints per path.
  --list                  Print slug + capability summary for all cards.
  --validate              Validate the map schema; non-zero exit on violation.

Options:
  --base <ref>            With --changed: diff base. Defaults to HEAD.
  --all                   With --changed: route all tracked files instead of
                          changed, cached, and untracked paths.
  --brief                 With --feature/--path: one-line summary instead of
                          full packet (drift-report format).
  --run <focused|gate>    With --feature: execute the extracted proof
                          commands sequentially, fail-fast, echoing each.
  --map <path>            Map file override (for tests). Defaults to
                          docs/modules/feature-ownership.md.
  -h, --help              Show this help.
EOF
}

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

mode=""
feature_slug=""
route_path_arg=""
base_ref="HEAD"
base_ref_set=0
scan_all=0
brief=0
run_kind=""
map_file="docs/modules/feature-ownership.md"

set_mode() {
    if [[ -n "$mode" ]]; then
        usage
        exit 2
    fi
    mode="$1"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --feature)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            set_mode feature
            feature_slug="$2"
            shift 2
            ;;
        --path)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            set_mode path
            route_path_arg="$2"
            shift 2
            ;;
        --changed)
            set_mode changed
            shift
            ;;
        --list)
            set_mode list
            shift
            ;;
        --validate)
            set_mode validate
            shift
            ;;
        --base)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            base_ref="$2"
            base_ref_set=1
            shift 2
            ;;
        --all)
            scan_all=1
            shift
            ;;
        --brief)
            brief=1
            shift
            ;;
        --run)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            run_kind="$2"
            shift 2
            ;;
        --map)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            map_file="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage
            exit 2
            ;;
    esac
done

if [[ -z "$mode" ]]; then
    usage
    exit 2
fi

if [[ "$scan_all" -eq 1 || "$base_ref_set" -eq 1 ]]; then
    if [[ "$mode" != "changed" ]]; then
        usage
        exit 2
    fi
fi

if [[ "$brief" -eq 1 ]]; then
    case "$mode" in
        feature|path) ;;
        *)
            usage
            exit 2
            ;;
    esac
fi

if [[ -n "$run_kind" ]]; then
    if [[ "$mode" != "feature" || "$brief" -eq 1 ]]; then
        usage
        exit 2
    fi
    case "$run_kind" in
        focused|gate) ;;
        *)
            fail "invalid --run kind: $run_kind (expected focused or gate)"
            ;;
    esac
fi

[[ -f "$map_file" ]] || fail "map file not found: $map_file"

case "$mode" in
    list|feature|path|changed|validate)
        fail "mode not implemented yet: $mode"
        ;;
esac
```

```bash
chmod +x scripts/agent-context.sh
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `bash scripts/test-agent-context.sh`
Expected: `agent-context tests passed.` (Note: `--feature alpha --run nonsense` reaches the run-kind check before map/mode dispatch, so it fails with the right message even though modes are unimplemented.)

- [ ] **Step 5: Commit**

```bash
git add scripts/agent-context.sh scripts/test-agent-context.sh
git commit -m "feat(scripts): scaffold agent-context tool with mode and option contract"
```

---

### Task 4: Card parser, `--list`, `--feature` packet, `--brief`

**Files:**
- Modify: `scripts/agent-context.sh`
- Modify: `scripts/test-agent-context.sh`

**Interfaces:**
- Consumes: globals from Task 3.
- Produces: `load_map()` filling array `card_headings` and assoc `card_fields["<heading>|<Field name>"]`; helpers `extract_spans(text)` (backticked spans, one per line, backticks stripped), `print_entries(text)` (semicolon-split entries), `gate_when_prose(text)`, `strip_backticks(text)`, `heading_for_slug(slug)`, `print_packet(heading)`, `brief_line(heading)` (no trailing newline). Later tasks call all of these.

- [ ] **Step 1: Add fixture + parse tests**

In `scripts/test-agent-context.sh`, insert before the final `echo "agent-context tests passed."` line:

```bash
# --- fixture map: parsing, --list, --feature, --brief ---

write_fixture_map() {
    cat > "$1" <<'EOF'
# Fixture Ownership Map

## How To Use This Map

Ignore me.

## Card Schema

Fields are described here; this section must not parse as a card.

## Alpha Feature

**Slug:** alpha

**Capability:** own alpha things.

**Start here:** `a/alpha.rs`;
`docs/alpha.md`.

**Neighbor systems:** beta runtime.

**Paths:** `a/*`.

**Focused proof:** `true`; `echo alpha-focused`.

**Interaction gate:** `echo alpha-gate` when alpha crosses beta.

**Docs to update:** `docs/alpha.md`.

**Safety notes:** never break alpha invariants.

## Beta Feature

**Slug:** beta

**Capability:** own beta things.

**Start here:** `a/b.rs`.

**Neighbor systems:** alpha runtime.

**Paths:** `a/b.rs`; `b/*`.

**Focused proof:** `echo beta-focused`.

**Interaction gate:** `echo beta-gate`.

**Docs to update:** `docs/beta.md`.

**Safety notes:** never break beta invariants.
EOF
}

fixture_map="$tmp/map.md"
write_fixture_map "$fixture_map"

list_out="$("$script" --list --map "$fixture_map")"
expected_list="$(printf 'alpha\town alpha things.\nbeta\town beta things.')"
[[ "$list_out" == "$expected_list" ]] \
    || fail "--list output mismatch; got: $list_out"

cat > "$tmp/alpha-packet.expected" <<'EOF'
# Task Packet: Alpha Feature
slug: alpha
capability: own alpha things.

## Read first
`a/alpha.rs`
`docs/alpha.md`.

## Paths owned
`a/*`.

## Neighbor systems
beta runtime.

## Focused proof
`true`
`echo alpha-focused`

## Interaction gate
`echo alpha-gate`
when: alpha crosses beta.

## Docs to update
`docs/alpha.md`.

## Safety invariants
never break alpha invariants.
EOF

"$script" --feature alpha --map "$fixture_map" > "$tmp/alpha-packet.out"
diff -u "$tmp/alpha-packet.expected" "$tmp/alpha-packet.out" \
    || fail "alpha task packet did not match expected format"

brief_out="$("$script" --feature alpha --brief --map "$fixture_map")"
expected_brief='Alpha Feature | focused: true; echo alpha-focused. | gate: echo alpha-gate when alpha crosses beta.'
[[ "$brief_out" == "$expected_brief" ]] \
    || fail "--brief output mismatch; got: $brief_out"

if "$script" --feature no-such-card --map "$fixture_map" >"$tmp/bad-slug.out" 2>&1; then
    fail "unknown slug unexpectedly succeeded"
fi
grep -q "unknown feature slug" "$tmp/bad-slug.out" \
    || fail "unknown slug did not produce a clear error"
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bash scripts/test-agent-context.sh`
Expected: FAIL with `mode not implemented yet: list`.

- [ ] **Step 3: Implement the parser and packet printer**

In `scripts/agent-context.sh`, insert after the `[[ -f "$map_file" ]] || fail ...` line (and before the mode dispatch):

```bash
declare -a card_headings=()
declare -A card_fields=()

load_map() {
    local kind a b c
    while IFS=$'\037' read -r kind a b c; do
        case "$kind" in
            C)
                card_headings+=("$a")
                ;;
            F)
                card_fields["$a|$b"]="$c"
                ;;
        esac
    done < <(awk '
        function flush_field() {
            if (card != "" && field != "") {
                gsub(/[ \t]+/, " ", value)
                sub(/^ /, "", value)
                sub(/ $/, "", value)
                printf "F\037%s\037%s\037%s\n", card, field, value
            }
            field = ""
            value = ""
        }
        /^## / {
            flush_field()
            heading = substr($0, 4)
            if (heading == "How To Use This Map" || heading == "Card Schema") {
                card = ""
                next
            }
            card = heading
            printf "C\037%s\n", card
            next
        }
        card == "" { next }
        /^[ \t]*$/ { flush_field(); next }
        /^\*\*[A-Za-z ]+:\*\*/ {
            flush_field()
            match($0, /^\*\*[A-Za-z ]+:\*\*/)
            field = substr($0, 3, RLENGTH - 5)
            value = substr($0, RLENGTH + 1)
            next
        }
        {
            if (field != "") {
                value = value " " $0
            }
        }
        END { flush_field() }
    ' "$map_file")
}

extract_spans() {
    grep -o '`[^`]*`' <<< "$1" | sed 's/^`//; s/`$//' || true
}

strip_backticks() {
    tr -d '`' <<< "$1"
}

print_entries() {
    local value="$1" entry
    local -a entries=()
    local IFS=';'
    read -ra entries <<< "$value"
    for entry in "${entries[@]}"; do
        entry="${entry#"${entry%%[![:space:]]*}"}"
        entry="${entry%"${entry##*[![:space:]]}"}"
        if [[ -n "$entry" ]]; then
            printf '%s\n' "$entry"
        fi
    done
}

print_commands_backticked() {
    local cmd
    while IFS= read -r cmd; do
        if [[ -n "$cmd" ]]; then
            printf '`%s`\n' "$cmd"
        fi
    done < <(extract_spans "$1")
}

gate_when_prose() {
    local prose
    prose="$(sed 's/`[^`]*`//g; s/;/ /g' <<< "$1" | tr -s ' ' | sed 's/^ //; s/ $//')"
    prose="${prose#when }"
    printf '%s\n' "$prose"
}

heading_for_slug() {
    local slug="$1" heading
    for heading in "${card_headings[@]}"; do
        if [[ "${card_fields["$heading|Slug"]:-}" == "$slug" ]]; then
            printf '%s\n' "$heading"
            return 0
        fi
    done
    return 1
}

print_packet() {
    local heading="$1" gate when
    gate="${card_fields["$heading|Interaction gate"]:-}"
    printf '# Task Packet: %s\n' "$heading"
    printf 'slug: %s\n' "${card_fields["$heading|Slug"]:-}"
    printf 'capability: %s\n' "${card_fields["$heading|Capability"]:-}"
    printf '\n## Read first\n'
    print_entries "${card_fields["$heading|Start here"]:-}"
    printf '\n## Paths owned\n'
    print_entries "${card_fields["$heading|Paths"]:-}"
    printf '\n## Neighbor systems\n'
    printf '%s\n' "${card_fields["$heading|Neighbor systems"]:-}"
    printf '\n## Focused proof\n'
    print_commands_backticked "${card_fields["$heading|Focused proof"]:-}"
    printf '\n## Interaction gate\n'
    print_commands_backticked "$gate"
    when="$(gate_when_prose "$gate")"
    if [[ -n "$when" ]]; then
        printf 'when: %s\n' "$when"
    fi
    printf '\n## Docs to update\n'
    print_entries "${card_fields["$heading|Docs to update"]:-}"
    printf '\n## Safety invariants\n'
    printf '%s\n' "${card_fields["$heading|Safety notes"]:-}"
}

brief_line() {
    local heading="$1"
    printf '%s | focused: %s | gate: %s' \
        "$heading" \
        "$(strip_backticks "${card_fields["$heading|Focused proof"]:-}")" \
        "$(strip_backticks "${card_fields["$heading|Interaction gate"]:-}")"
}

mode_list() {
    local heading
    for heading in "${card_headings[@]}"; do
        printf '%s\t%s\n' \
            "${card_fields["$heading|Slug"]:-}" \
            "${card_fields["$heading|Capability"]:-}"
    done
}
```

Replace the mode dispatch `case` block at the bottom of the file with:

```bash
load_map

case "$mode" in
    list)
        mode_list
        ;;
    feature)
        heading="$(heading_for_slug "$feature_slug")" \
            || fail "unknown feature slug: $feature_slug (list slugs with --list)"
        if [[ "$brief" -eq 1 ]]; then
            printf '%s\n' "$(brief_line "$heading")"
        else
            print_packet "$heading"
        fi
        ;;
    path|changed|validate)
        fail "mode not implemented yet: $mode"
        ;;
esac
```

- [ ] **Step 4: Run test to verify it passes**

Run: `bash scripts/test-agent-context.sh`
Expected: `agent-context tests passed.`

- [ ] **Step 5: Commit**

```bash
git add scripts/agent-context.sh scripts/test-agent-context.sh
git commit -m "feat(scripts): parse ownership cards into task packets"
```

---

### Task 5: Path routing — specificity, fallback table, `--path`

**Files:**
- Modify: `scripts/agent-context.sh`
- Modify: `scripts/test-agent-context.sh`

**Interfaces:**
- Consumes: `load_map`, `card_fields`, `extract_spans`, `print_packet`, `brief_line` (Task 4).
- Produces: `load_globs()` filling parallel arrays `glob_patterns`, `glob_headings`, `glob_specificities`; `route_path(path)` (returns 0 on card match; sets `route_heading` and array `route_tied` of headings tied at best specificity); `require_unambiguous_route(path)`; `fallback_hint_for_path(path)`; constant `no_hint_message`. Tasks 6 and 7 reuse all of these.

- [ ] **Step 1: Add routing tests**

Insert before the final success echo in `scripts/test-agent-context.sh`:

```bash
# --- routing: most-specific wins, fallback table, no-hint ---

path_brief_out="$("$script" --path a/b.rs --brief --map "$fixture_map")"
grep -q '^Beta Feature |' <<<"$path_brief_out" \
    || fail "a/b.rs did not route to the more specific Beta card; got: $path_brief_out"

path_brief_out="$("$script" --path a/alpha.rs --brief --map "$fixture_map")"
grep -q '^Alpha Feature |' <<<"$path_brief_out" \
    || fail "a/alpha.rs did not route to Alpha; got: $path_brief_out"

"$script" --path a/alpha.rs --map "$fixture_map" | grep -q '^# Task Packet: Alpha Feature$' \
    || fail "--path without --brief did not print the full packet"

fallback_out="$("$script" --path docs/superpowers/specs/2099-01-01-example-design.md --map "$fixture_map")"
grep -q '^Planning docs |' <<<"$fallback_out" \
    || fail "specs path did not use the planning fallback; got: $fallback_out"

fallback_out="$("$script" --path docs/modules/anything-at-all.md --map "$fixture_map")"
grep -q '^Canonical docs |' <<<"$fallback_out" \
    || fail "docs/modules path did not use the canonical docs fallback"

fallback_out="$("$script" --path AGENTS.md --map "$fixture_map")"
grep -q '^Assistant/contributor guidance |' <<<"$fallback_out" \
    || fail "AGENTS.md did not use the guidance fallback"

nohint_out="$("$script" --path zzz/nowhere.c --map "$fixture_map")"
grep -q '^No feature-card hint matched' <<<"$nohint_out" \
    || fail "unmatched path did not print the no-hint message"
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bash scripts/test-agent-context.sh`
Expected: FAIL with `mode not implemented yet: path`.

- [ ] **Step 3: Implement routing**

In `scripts/agent-context.sh`, insert after the `mode_list()` function:

```bash
declare -a glob_patterns=()
declare -a glob_headings=()
declare -a glob_specificities=()

load_globs() {
    local heading glob prefix
    for heading in "${card_headings[@]}"; do
        while IFS= read -r glob; do
            if [[ -z "$glob" ]]; then
                continue
            fi
            glob_patterns+=("$glob")
            glob_headings+=("$heading")
            prefix="${glob%%[\*\?\[]*}"
            glob_specificities+=("${#prefix}")
        done < <(extract_spans "${card_fields["$heading|Paths"]:-}")
    done
}

route_heading=""
declare -a route_tied=()

route_path() {
    local path="$1"
    local i best=-1
    route_heading=""
    route_tied=()
    for i in "${!glob_patterns[@]}"; do
        # The glob must stay unquoted so [[ == ]] treats it as a pattern.
        if [[ "$path" == ${glob_patterns[$i]} ]]; then
            if (( glob_specificities[i] > best )); then
                best="${glob_specificities[$i]}"
                route_heading="${glob_headings[$i]}"
                route_tied=("${glob_headings[$i]}")
            elif (( glob_specificities[i] == best )); then
                route_tied+=("${glob_headings[$i]}")
            fi
        fi
    done
    [[ -n "$route_heading" ]]
}

distinct_tied_count() {
    printf '%s\n' "${route_tied[@]}" | sort -u | wc -l
}

require_unambiguous_route() {
    local path="$1"
    if (( $(distinct_tied_count) > 1 )); then
        fail "ambiguous Paths routing for $path: $(printf '%s\n' "${route_tied[@]}" | sort -u | paste -sd ';' -) (run --validate and fix the map)"
    fi
}

fallback_hint_for_path() {
    local path="$1"
    case "$path" in
        AGENTS.md|CONTRIBUTING.md|.github/PULL_REQUEST_TEMPLATE.md|docs/llms/*|docs/modules/feature-ownership.md|docs/superpowers/documentation-accuracy-audit-*|scripts/maintainability-drift-report.sh|scripts/agent-context.sh)
            printf 'Assistant/contributor guidance | focused: bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete | gate: docs-audit inventory diff and stale-term added-line sweep'
            ;;
        docs/modules/*|docs/operations/*|docs/INTEGRATION-TESTING.md|docs/ARCHITECTURE.md)
            printf 'Canonical docs | focused: docs-audit ledger and inventory checks | gate: affected feature card proof if behavior claims changed'
            ;;
        docs/superpowers/plans/*|docs/superpowers/specs/*)
            printf 'Planning docs | focused: docs-audit ledger and inventory checks | gate: agentic review before lock-in'
            ;;
        *)
            return 1
            ;;
    esac
}

no_hint_message='No feature-card hint matched. Use the owning package tests and update docs/modules/feature-ownership.md if this should be routed.'

mode_path() {
    local hint
    if route_path "$route_path_arg"; then
        require_unambiguous_route "$route_path_arg"
        if [[ "$brief" -eq 1 ]]; then
            printf '%s\n' "$(brief_line "$route_heading")"
        else
            print_packet "$route_heading"
        fi
    elif hint="$(fallback_hint_for_path "$route_path_arg")"; then
        printf '%s\n' "$hint"
    else
        printf '%s\n' "$no_hint_message"
    fi
}
```

Update the dispatch `case`: add `load_globs` on the line after `load_map`, and replace the `path|changed|validate)` arm with:

```bash
    path)
        mode_path
        ;;
    changed|validate)
        fail "mode not implemented yet: $mode"
        ;;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `bash scripts/test-agent-context.sh`
Expected: `agent-context tests passed.`

- [ ] **Step 5: Commit**

```bash
git add scripts/agent-context.sh scripts/test-agent-context.sh
git commit -m "feat(scripts): route paths to cards by glob specificity with doc fallbacks"
```

---

### Task 6: `--changed` / `--changed --all` collection

**Files:**
- Modify: `scripts/agent-context.sh`
- Modify: `scripts/test-agent-context.sh`

**Interfaces:**
- Consumes: `route_path`, `brief_line`, `fallback_hint_for_path`, `no_hint_message` (Task 5).
- Produces: `collect_paths()` and `mode_changed()`; output contract used by the drift report in Task 10 — `[ok] no changed paths detected` when empty, otherwise `changed_paths: <n>` followed by `- <path>` / two-space-indented hint lines.

- [ ] **Step 1: Add changed-collection tests (fixture git repo)**

Insert before the final success echo in `scripts/test-agent-context.sh`:

```bash
# --- --changed and --changed --all collection (fixture git repo) ---

changed_repo="$tmp/changed-repo"
mkdir -p "$changed_repo/a"
git -C "$changed_repo" init -q
git -C "$changed_repo" config user.email "test@example.com"
git -C "$changed_repo" config user.name "test"
write_fixture_map "$changed_repo/map.md"
printf 'tracked one\n' > "$changed_repo/t1.rs"
printf 'alpha\n' > "$changed_repo/a/alpha.rs"
git -C "$changed_repo" add -A
git -C "$changed_repo" commit -qm init

printf 'tracked one modified\n' > "$changed_repo/t1.rs"
printf 'staged\n' > "$changed_repo/staged.rs"
git -C "$changed_repo" add staged.rs
printf 'untracked\n' > "$changed_repo/untracked.rs"

changed_out="$( (cd "$changed_repo" && bash "$script" --changed --map map.md) )"
grep -q '^changed_paths: 3$' <<<"$changed_out" \
    || fail "--changed did not count modified+staged+untracked; got: $changed_out"
grep -q -- '^- t1.rs$' <<<"$changed_out" || fail "--changed missed modified path"
grep -q -- '^- staged.rs$' <<<"$changed_out" || fail "--changed missed staged path"
grep -q -- '^- untracked.rs$' <<<"$changed_out" || fail "--changed missed untracked path"
if grep -q -- '^- a/alpha.rs$' <<<"$changed_out"; then
    fail "--changed included an unchanged tracked path"
fi

all_out="$( (cd "$changed_repo" && bash "$script" --changed --all --map map.md) )"
grep -q -- '^- a/alpha.rs$' <<<"$all_out" \
    || fail "--changed --all missed a tracked path"
grep -A1 -- '^- a/alpha.rs$' <<<"$all_out" | grep -q 'Alpha Feature |' \
    || fail "--changed --all did not route a/alpha.rs to Alpha"
grep -A1 -- '^- t1.rs$' <<<"$all_out" | grep -q 'No feature-card hint matched' \
    || fail "--changed --all did not print no-hint for unrouted path"

if (cd "$changed_repo" && bash "$script" --changed --base definitely-not-a-ref --map map.md) >"$tmp/bad-base.out" 2>&1; then
    fail "invalid base ref unexpectedly succeeded"
fi
grep -q "base ref not found" "$tmp/bad-base.out" \
    || fail "invalid base ref did not print a clear error"

clean_out="$( (cd "$changed_repo" && git stash -q --include-untracked && bash "$script" --changed --map map.md) )"
grep -q '^\[ok\] no changed paths detected$' <<<"$clean_out" \
    || fail "clean tree did not report no changed paths"
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bash scripts/test-agent-context.sh`
Expected: FAIL with `mode not implemented yet: changed`.

- [ ] **Step 3: Implement `--changed`**

In `scripts/agent-context.sh`, insert after `mode_path()`:

```bash
collect_paths() {
    if [[ "$scan_all" -eq 1 ]]; then
        git ls-files
        return
    fi

    {
        git diff --name-only "$base_ref" --
        git diff --cached --name-only --
        git ls-files --others --exclude-standard
    } | awk 'NF' | sort -u
}

mode_changed() {
    local p hint
    local -a changed=()
    mapfile -t changed < <(collect_paths)

    if [[ "${#changed[@]}" -eq 0 ]]; then
        printf '[ok] no changed paths detected\n'
        return
    fi

    printf 'changed_paths: %s\n' "${#changed[@]}"
    for p in "${changed[@]}"; do
        if route_path "$p"; then
            require_unambiguous_route "$p"
            printf -- '- %s\n  %s\n' "$p" "$(brief_line "$route_heading")"
        elif hint="$(fallback_hint_for_path "$p")"; then
            printf -- '- %s\n  %s\n' "$p" "$hint"
        else
            printf -- '- %s\n  %s\n' "$p" "$no_hint_message"
        fi
    done
}
```

Update the dispatch `case`: replace the `changed|validate)` arm with:

```bash
    changed)
        if [[ "$scan_all" -eq 0 ]] \
            && ! git rev-parse --verify --quiet "$base_ref^{commit}" >/dev/null; then
            echo "ERROR: base ref not found: $base_ref" >&2
            exit 2
        fi
        mode_changed
        ;;
    validate)
        fail "mode not implemented yet: $mode"
        ;;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `bash scripts/test-agent-context.sh`
Expected: `agent-context tests passed.`

- [ ] **Step 5: Commit**

```bash
git add scripts/agent-context.sh scripts/test-agent-context.sh
git commit -m "feat(scripts): collect changed and all-tracked paths for card routing"
```

---

### Task 7: `--validate`

**Files:**
- Modify: `scripts/agent-context.sh`
- Modify: `scripts/test-agent-context.sh`

**Interfaces:**
- Consumes: `card_headings`, `card_fields`, `extract_spans`, `glob_patterns`/`glob_headings`/`glob_specificities`, `route_path`, `route_tied`, `distinct_tied_count` (Tasks 4–5).
- Produces: `mode_validate()`; every violation prints `INVALID: card '<heading>' ...` (or an overlap line) to stderr and the run exits non-zero via `fail`. CI calls this in Task 12.

- [ ] **Step 1: Add validation tests (one fixture repo per failure)**

Insert before the final success echo in `scripts/test-agent-context.sh`:

```bash
# --- --validate: good map passes; six distinct violations fail ---

make_validate_repo() {
    local dir="$1"
    mkdir -p "$dir/a" "$dir/b" "$dir/docs"
    git -C "$dir" init -q
    git -C "$dir" config user.email "test@example.com"
    git -C "$dir" config user.name "test"
    printf 'alpha\n' > "$dir/a/alpha.rs"
    printf 'b\n' > "$dir/a/b.rs"
    printf 'x\n' > "$dir/b/x.rs"
    printf 'alpha docs\n' > "$dir/docs/alpha.md"
    printf 'beta docs\n' > "$dir/docs/beta.md"
    git -C "$dir" add a b docs
    write_fixture_map "$dir/map.md"
}

run_validate_expect_fail() {
    local dir="$1" expect="$2" out
    if out="$( (cd "$dir" && bash "$script" --map map.md --validate) 2>&1 )"; then
        fail "validate unexpectedly passed; wanted error: $expect"
    fi
    grep -q "$expect" <<<"$out" \
        || fail "validate error missing '$expect'; got: $out"
}

good_repo="$tmp/validate-good"
make_validate_repo "$good_repo"
good_out="$( (cd "$good_repo" && bash "$script" --map map.md --validate) )"
grep -q "validation passed" <<<"$good_out" \
    || fail "well-formed fixture map did not validate; got: $good_out"

vr="$tmp/validate-missing-field"
make_validate_repo "$vr"
sed -i '/^\*\*Safety notes:\*\* never break beta invariants\.$/d' "$vr/map.md"
run_validate_expect_fail "$vr" "card 'Beta Feature' is missing field: Safety notes"

vr="$tmp/validate-dup-slug"
make_validate_repo "$vr"
sed -i 's/^\*\*Slug:\*\* beta$/**Slug:** alpha/' "$vr/map.md"
run_validate_expect_fail "$vr" "duplicates slug: alpha"

vr="$tmp/validate-dead-glob"
make_validate_repo "$vr"
sed -i 's|`b/\*`|`c/*`|' "$vr/map.md"
run_validate_expect_fail "$vr" "dead Paths glob: c/\*"

vr="$tmp/validate-overlap"
make_validate_repo "$vr"
cat >> "$vr/map.md" <<'EOF'

## Gamma Feature

**Slug:** gamma

**Capability:** own gamma things.

**Start here:** `a/alpha.rs`.

**Neighbor systems:** alpha runtime.

**Paths:** `a/*`.

**Focused proof:** `echo gamma-focused`.

**Interaction gate:** `echo gamma-gate`.

**Docs to update:** `docs/alpha.md`.

**Safety notes:** never break gamma invariants.
EOF
run_validate_expect_fail "$vr" "equal-specificity Paths overlap for a/alpha.rs"

vr="$tmp/validate-missing-start"
make_validate_repo "$vr"
sed -i 's|`a/alpha.rs`;|`a/missing.rs`;|' "$vr/map.md"
run_validate_expect_fail "$vr" "references untracked path: a/missing.rs"

vr="$tmp/validate-no-proof-command"
make_validate_repo "$vr"
sed -i 's/^\*\*Focused proof:\*\* `true`; `echo alpha-focused`\.$/**Focused proof:** run the alpha tests by hand./' "$vr/map.md"
run_validate_expect_fail "$vr" "Focused proof has no backticked command"
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bash scripts/test-agent-context.sh`
Expected: FAIL with `mode not implemented yet: validate`.

- [ ] **Step 3: Implement `--validate`**

In `scripts/agent-context.sh`, insert after `mode_changed()`:

```bash
required_fields=("Slug" "Capability" "Start here" "Neighbor systems" "Paths" "Focused proof" "Interaction gate" "Docs to update" "Safety notes")

validation_errors=0

validate_err() {
    printf 'INVALID: %s\n' "$*" >&2
    validation_errors=$((validation_errors + 1))
}

is_repo_path_span() {
    local span="$1"
    [[ "$span" =~ ^[A-Za-z0-9][A-Za-z0-9._/-]*$ ]] || return 1
    if [[ "$span" == */* ]]; then
        return 0
    fi
    [[ "$span" == *.md ]]
}

span_exists_tracked() {
    local span="${1%/}"
    if git ls-files --error-unmatch -- "$span" >/dev/null 2>&1; then
        return 0
    fi
    [[ -n "$(git ls-files -- "$span/" | head -n 1)" ]]
}

mode_validate() {
    local heading field slug span i t matched
    local -a tracked_files=()
    declare -A seen_slugs=()

    if [[ "${#card_headings[@]}" -eq 0 ]]; then
        fail "no ownership cards parsed from $map_file"
    fi

    for heading in "${card_headings[@]}"; do
        for field in "${required_fields[@]}"; do
            if [[ -z "${card_fields["$heading|$field"]:-}" ]]; then
                validate_err "card '$heading' is missing field: $field"
            fi
        done

        slug="${card_fields["$heading|Slug"]:-}"
        if [[ -n "$slug" ]]; then
            if [[ ! "$slug" =~ ^[a-z0-9]+(-[a-z0-9]+)*$ ]]; then
                validate_err "card '$heading' slug is not kebab-case: $slug"
            fi
            if [[ -n "${seen_slugs["$slug"]:-}" ]]; then
                validate_err "card '$heading' duplicates slug: $slug"
            fi
            seen_slugs["$slug"]=1
        fi

        if [[ -n "${card_fields["$heading|Focused proof"]:-}" ]] \
            && [[ -z "$(extract_spans "${card_fields["$heading|Focused proof"]}")" ]]; then
            validate_err "card '$heading' Focused proof has no backticked command"
        fi

        if [[ -n "${card_fields["$heading|Paths"]:-}" ]] \
            && [[ -z "$(extract_spans "${card_fields["$heading|Paths"]}")" ]]; then
            validate_err "card '$heading' Paths has no backticked glob"
        fi

        for field in "Start here" "Docs to update"; do
            while IFS= read -r span; do
                if [[ -n "$span" ]] && is_repo_path_span "$span" && ! span_exists_tracked "$span"; then
                    validate_err "card '$heading' $field references untracked path: $span"
                fi
            done < <(extract_spans "${card_fields["$heading|$field"]:-}")
        done
    done

    mapfile -t tracked_files < <(git ls-files)

    for i in "${!glob_patterns[@]}"; do
        matched=0
        for t in "${tracked_files[@]}"; do
            # The glob must stay unquoted so [[ == ]] treats it as a pattern.
            if [[ "$t" == ${glob_patterns[$i]} ]]; then
                matched=1
                break
            fi
        done
        if [[ "$matched" -eq 0 ]]; then
            validate_err "card '${glob_headings[$i]}' has dead Paths glob: ${glob_patterns[$i]}"
        fi
    done

    for t in "${tracked_files[@]}"; do
        if route_path "$t"; then
            if (( $(distinct_tied_count) > 1 )); then
                validate_err "equal-specificity Paths overlap for $t: $(printf '%s\n' "${route_tied[@]}" | sort -u | paste -sd ';' -)"
            fi
        fi
    done

    if (( validation_errors > 0 )); then
        fail "feature ownership map validation failed with $validation_errors problem(s): $map_file"
    fi
    printf 'Feature ownership map validation passed (%s cards).\n' "${#card_headings[@]}"
}
```

Replace the remaining `validate)` placeholder arm in the dispatch `case` with:

```bash
    validate)
        mode_validate
        ;;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `bash scripts/test-agent-context.sh`
Expected: `agent-context tests passed.`

- [ ] **Step 5: Commit**

```bash
git add scripts/agent-context.sh scripts/test-agent-context.sh
git commit -m "feat(scripts): validate the feature ownership map schema and routing"
```

---

### Task 8: `--run focused|gate`

**Files:**
- Modify: `scripts/agent-context.sh`
- Modify: `scripts/test-agent-context.sh`

**Interfaces:**
- Consumes: `heading_for_slug`, `extract_spans`, `card_fields` (Task 4).
- Produces: `mode_run(heading)` — echoes each command as `+ <command>`, executes via `bash -lc "<command>"` from the repo root, fail-fast; success footer `All <n> <kind> command(s) passed for <heading>.`

- [ ] **Step 1: Add --run tests**

Insert before the final success echo in `scripts/test-agent-context.sh`:

```bash
# --- --run focused|gate: executes card commands, fail-fast ---

run_out="$("$script" --feature alpha --run focused --map "$fixture_map")"
grep -q '^+ true$' <<<"$run_out" || fail "--run did not echo the first command"
grep -q '^alpha-focused$' <<<"$run_out" || fail "--run did not execute echo command"
grep -q 'command(s) passed for Alpha Feature' <<<"$run_out" \
    || fail "--run did not print the success footer"

run_out="$("$script" --feature alpha --run gate --map "$fixture_map")"
grep -q '^alpha-gate$' <<<"$run_out" || fail "--run gate did not execute the gate command"

failing_map="$tmp/failing-map.md"
write_fixture_map "$failing_map"
sed -i 's/^\*\*Focused proof:\*\* `true`; `echo alpha-focused`\.$/**Focused proof:** `false`; `echo never-runs`./' "$failing_map"
if "$script" --feature alpha --run focused --map "$failing_map" >"$tmp/run-fail.out" 2>&1; then
    fail "--run with failing command unexpectedly succeeded"
fi
if grep -q "never-runs" "$tmp/run-fail.out"; then
    fail "--run did not stop at the first failing command"
fi
grep -q "command failed: false" "$tmp/run-fail.out" \
    || fail "--run failure did not name the failing command"
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bash scripts/test-agent-context.sh`
Expected: FAIL with `--run did not echo the first command` — the Task 4 dispatch still prints the packet for `--feature` and ignores `run_kind`, so `grep -q '^+ true$'` finds nothing.

- [ ] **Step 3: Implement `--run`**

In `scripts/agent-context.sh`, insert after `mode_validate()`:

```bash
mode_run() {
    local heading="$1" field cmd
    local -a cmds=()

    case "$run_kind" in
        focused) field="Focused proof" ;;
        gate) field="Interaction gate" ;;
    esac

    mapfile -t cmds < <(extract_spans "${card_fields["$heading|$field"]:-}")
    if [[ "${#cmds[@]}" -eq 0 ]]; then
        fail "card '$heading' has no $field commands to run"
    fi

    for cmd in "${cmds[@]}"; do
        printf '+ %s\n' "$cmd"
        bash -lc "$cmd" || fail "command failed: $cmd"
    done
    printf 'All %s %s command(s) passed for %s.\n' "${#cmds[@]}" "$run_kind" "$heading"
}
```

Update the `feature)` dispatch arm to:

```bash
    feature)
        heading="$(heading_for_slug "$feature_slug")" \
            || fail "unknown feature slug: $feature_slug (list slugs with --list)"
        if [[ -n "$run_kind" ]]; then
            mode_run "$heading"
        elif [[ "$brief" -eq 1 ]]; then
            printf '%s\n' "$(brief_line "$heading")"
        else
            print_packet "$heading"
        fi
        ;;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `bash scripts/test-agent-context.sh`
Expected: `agent-context tests passed.`

- [ ] **Step 5: Commit**

```bash
git add scripts/agent-context.sh scripts/test-agent-context.sh
git commit -m "feat(scripts): execute card proof commands via --run focused/gate"
```

---

### Task 9: Real-map smoke assertions and first live validation

**Files:**
- Modify: `scripts/test-agent-context.sh`

**Interfaces:**
- Consumes: the Task 2 map and the complete tool.
- Produces: CI-grade proof that the real map parses, validates, and routes the three intended most-specific-wins overlaps.

- [ ] **Step 1: Add real-map smoke tests**

Insert before the final success echo in `scripts/test-agent-context.sh`:

```bash
# --- real-map smoke assertions (default --map) ---

"$script" --validate >/dev/null \
    || fail "real feature-ownership map failed --validate"

real_list="$("$script" --list)"
grep -q $'^packaging\t' <<<"$real_list" || fail "real map --list missing packaging slug"
grep -q $'^profiles\t' <<<"$real_list" || fail "real map --list missing profiles slug"
[[ "$(wc -l <<<"$real_list")" -eq 13 ]] || fail "real map --list did not print 13 cards"

"$script" --path apps/conary/src/commands/install/mod.rs | grep -q '^slug: install$' \
    || fail "install path did not route to the install card"
"$script" --path apps/remi/src/server/mcp.rs | grep -q '^slug: agent-mcp$' \
    || fail "remi mcp.rs did not route to agent-mcp (specificity)"
"$script" --path apps/conary-test/src/bootstrap.rs | grep -q '^slug: bootstrap$' \
    || fail "conary-test bootstrap.rs did not route to bootstrap (specificity)"
"$script" --path apps/remi/src/federation/mod.rs | grep -q '^slug: remi$' \
    || fail "federation path did not fold into the remi card"
```

- [ ] **Step 2: Run test to verify it passes**

Run: `bash scripts/test-agent-context.sh`
Expected: `agent-context tests passed.` If `--validate` reports `INVALID:` lines here, the map data from Task 2 has a typo (dead glob, stale path, or missing field) — fix the map, not the validator, and re-run. All Task 2 globs and card paths were pre-verified against `git ls-files` on 2026-07-01, so failures indicate a transcription error.

- [ ] **Step 3: Eyeball one full packet**

Run: `bash scripts/agent-context.sh --feature packaging | head -40`
Expected: packet with the packaging heading, read-first files, the 22 Paths globs, and safety invariants including the recorded-draft sentence added in Task 2.

- [ ] **Step 4: Commit**

```bash
git add scripts/test-agent-context.sh
git commit -m "test(scripts): prove agent-context against the real ownership map"
```

---

### Task 10: Cutover 1 — drift report delegates to agent-context

**Files:**
- Modify: `scripts/maintainability-drift-report.sh`
- Modify: `scripts/test-maintainability-drift-report.sh`

**Interfaces:**
- Consumes: `agent-context.sh --changed [--base <ref>] [--all]` (Task 6 output contract).
- Produces: a drift report whose "Changed Path Hints" section is entirely generated by the tool. The report stays warn-only.

- [ ] **Step 1: Update the drift-report test expectations**

In `scripts/test-maintainability-drift-report.sh`, replace the four hint assertions (currently the `grep -q "Agent/MCP operation surfaces"`, `"Bootstrap and self-hosting"`, `"conaryd jobs/routes"`, and `"Remi federation"` blocks) with:

```bash
grep -q "Agent/MCP Operation Surfaces" <<<"$all_output" \
    || fail "all-path report did not include Agent/MCP hint"
grep -q "Bootstrap And Self-Hosting" <<<"$all_output" \
    || fail "all-path report did not include Bootstrap hint"
grep -q "conaryd Package Jobs And Daemon Routes" <<<"$all_output" \
    || fail "all-path report did not include conaryd hint"
grep -A1 -- '^- apps/remi/src/federation/' <<<"$all_output" | grep -q "Remi Publication, Serving, Admin, And Fixture Artifacts" \
    || fail "federation paths did not route to the Remi card"
```

Leave every other assertion (usage, invalid limit, invalid base ref, section headers, hotspot table, `Canonical docs` fixture) unchanged.

- [ ] **Step 2: Run test to verify it fails**

Run: `bash scripts/test-maintainability-drift-report.sh`
Expected: FAIL with `all-path report did not include Agent/MCP hint` (old script still prints the old lowercase strings).

- [ ] **Step 3: Cut the drift report over**

In `scripts/maintainability-drift-report.sh`:

1. Delete the entire `feature_hint_for_path()` function (the `case "$path" in ... esac` block, currently lines 83–130).
2. Delete the entire `collect_paths()` function (currently lines 132–143).
3. Replace the "Changed Path Hints" section body (currently the `mapfile -t changed_paths ...` block through its closing `fi`) with:

```bash
section "Changed Path Hints"
if [[ "$scan_all" -eq 1 ]]; then
    bash scripts/agent-context.sh --changed --all
else
    bash scripts/agent-context.sh --changed --base "$base_ref"
fi
```

Keep the existing base-ref validation near the top of the script and everything else unchanged.

- [ ] **Step 4: Run tests to verify they pass**

```bash
bash scripts/test-maintainability-drift-report.sh
bash scripts/maintainability-drift-report.sh --all --limit 3 | head -60
```

Expected: test passes; the `--all` report shows `changed_paths: <n>` and per-path hints sourced from card headings. Eyeball that hints look sane.

- [ ] **Step 5: Commit**

```bash
git add scripts/maintainability-drift-report.sh scripts/test-maintainability-drift-report.sh
git commit -m "feat(scripts): cut drift report path hints over to agent-context"
```

---

### Task 11: Cutover 2 — plan reviewer builds its prompt from cards

**Files:**
- Modify: `scripts/agentic-plan-review.sh`
- Modify: `scripts/test-agentic-plan-review.sh`

**Interfaces:**
- Consumes: `agent-context.sh --feature <slug>` packet sections `## Read first`, `## Paths owned`, `## Docs to update`, `## Safety invariants` (Task 4 format, byte-pinned by the Task 4 diff test).
- Produces: repeatable `--feature <slug>` flag (≥1 required); generated "Required local context" list; "Review pressure points from feature ownership" prompt section; dry-run prints the full planned prompt after a `--- prompt ---` marker.

- [ ] **Step 1: Update the plan-review tests**

In `scripts/test-agentic-plan-review.sh`:

1. Add `--feature packaging` to the four success-path invocations (the main stub run, the `--only deepseek` run, the `--dry-run` run, and the design-target run). Example for the main run:

```bash
PATH="$bin_dir:$PATH" "$script" "$target" --feature packaging --context docs/modules/remi.md --out-dir "$out_dir" >"$tmp/run.out"
```

2. After the existing bad-context test block, add:

```bash
if "$script" "$target" --out-dir "$out_dir" --dry-run >"$tmp/no-feature.out" 2>&1; then
    fail "missing --feature unexpectedly succeeded"
fi
grep -q "at least one --feature" "$tmp/no-feature.out" \
    || fail "missing --feature did not produce a clear error"

if "$script" "$target" --feature not-a-real-slug --out-dir "$out_dir" --dry-run >"$tmp/bad-feature.out" 2>&1; then
    fail "unknown feature slug unexpectedly succeeded"
fi
grep -q "unknown feature slug" "$tmp/bad-feature.out" \
    || fail "unknown feature slug did not produce a clear error"
```

3. Extend the dry-run assertions block with:

```bash
grep -q -- "--- prompt ---" "$tmp/dry-run.out" \
    || fail "dry-run did not print the planned prompt"
grep -q "apps/conary/src/commands/publish.rs" "$tmp/dry-run.out" \
    || fail "dry-run prompt missing packaging card Start here file"
grep -q "Review pressure points from feature ownership" "$tmp/dry-run.out" \
    || fail "dry-run prompt missing safety-note pressure points"
grep -q "docs/modules/feature-ownership.md" "$tmp/dry-run.out" \
    || fail "dry-run prompt missing stable preamble docs"
grep -qi "recorded-draft" "$tmp/dry-run.out" \
    || fail "dry-run prompt lost the recorded-draft refusal pressure point"
if grep -q "crates/conary-core/src/ccs/binary_manifest.rs" "$tmp/dry-run.out"; then
    fail "dry-run prompt still contains the old hardcoded context list"
fi
if grep -q "M2 hardening gates" "$tmp/dry-run.out"; then
    fail "dry-run prompt still contains packaging-specific goal 7"
fi
if grep -q "file-size/refactor hazards" "$tmp/dry-run.out"; then
    fail "dry-run prompt still contains packaging-specific goal 5"
fi
```

4. Add to the help assertions:

```bash
grep -q -- "--feature <slug>" <<<"$help_output" \
    || fail "help output did not name --feature"
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bash scripts/test-agentic-plan-review.sh`
Expected: FAIL with `help output did not name --feature`.

- [ ] **Step 3: Implement the cutover**

In `scripts/agentic-plan-review.sh`:

1. In `usage()`, after the `--context <path>` line, add:

```text
  --feature <slug>               Feature ownership card whose Start here,
                                 Docs to update, Paths, and Safety notes feed
                                 the prompt. Repeatable; at least one required.
                                 Example: --feature packaging --feature ccs --feature remi
```

2. Add `features=()` next to the other variable defaults, and this arg-parse case alongside `--context`:

```bash
        --feature)
            [[ $# -ge 2 ]] || {
                usage
                exit 2
            }
            features+=("$2")
            shift 2
            ;;
```

3. After the existing extra-context existence loop (`for context_path in ...`), add:

```bash
[[ "${#features[@]}" -ge 1 ]] \
    || fail "at least one --feature <slug> is required (list slugs with: bash scripts/agent-context.sh --list)"

feature_context_file="$(mktemp)"
feature_pressure_file="$(mktemp)"
trap 'rm -f "$feature_context_file" "$feature_pressure_file"' EXIT

packet_section() {
    local section_heading="$1"
    awk -v want="## $section_heading" '
        $0 == want { on = 1; next }
        /^## /     { on = 0 }
        on && NF   { print }
    '
}

resolve_features() {
    local slug packet heading
    for slug in "${features[@]}"; do
        packet="$(bash scripts/agent-context.sh --feature "$slug")" \
            || fail "unknown feature slug: $slug (list slugs with: bash scripts/agent-context.sh --list)"
        heading="$(sed -n 's/^# Task Packet: //p' <<<"$packet")"
        {
            packet_section "Read first" <<<"$packet"
            packet_section "Docs to update" <<<"$packet"
            packet_section "Paths owned" <<<"$packet"
        } | tr -d '`' >> "$feature_context_file"
        printf -- '- %s: %s\n' "$heading" "$(packet_section "Safety invariants" <<<"$packet")" \
            >> "$feature_pressure_file"
    done
}
```

4. Replace the whole `build_prompt()` function with:

```bash
build_prompt() {
    cat <<EOF
You are conducting a senior security/packaging review for the Conary repository. This is review only: do not modify files, do not create files, do not run writes, and do not make commits.

Do not rush. Treat this as a deep pre-lock review. Before producing findings, perform separate passes for repository-context fit, scope and milestone boundaries, failure/security/trust behavior, migration and compatibility hazards, implementation-plan readiness, and missing tests.

Review target:
- $review_target

Review kind: $resolved_review_kind

Required local context to inspect before judging:
- AGENTS.md
- docs/llms/README.md
- docs/ARCHITECTURE.md
- docs/INTEGRATION-TESTING.md
- docs/modules/feature-ownership.md
EOF

    LC_ALL=C sort -u "$feature_context_file" | sed 's/^/- /'

    if [[ "${#extra_context[@]}" -gt 0 ]]; then
        printf '\nAdditional local context requested by caller:\n'
        for context_path in "${extra_context[@]}"; do
            printf -- '- %s\n' "$context_path"
        done
    fi

    cat <<'EOF'

Shared review goals:
1. Check whether the target is implementable or lockable from the current codebase without hidden prerequisites.
2. Find security, provenance, trust, migration, and failure-behavior gaps.
3. Find task ordering problems, underspecified ownership boundaries, or missing regression tests.
4. Check whether it respects parent design boundaries and does not pull later slices forward without a deliberate gate.
5. Check whether supported target language stays limited to Fedora 44, Ubuntu 26.04, and Arch unless the target explicitly scopes otherwise.

Review pressure points from feature ownership:
EOF

    cat "$feature_pressure_file"

    cat <<'EOF'

Evidence rules:
- Cite the target section and repository file/line when available.
- If a required path is irrelevant to the target, say so briefly instead of forcing a finding.
- Do not invent code or behavior that is not in the repository.
- Treat broad compatibility claims, silent defaulting, live host I/O in core validation, and conversion-only evidence as review pressure points.
EOF

    case "$resolved_review_kind" in
        design)
            cat <<'EOF'

Design-specific review rubric:
1. Is the problem statement grounded in current repo facts?
2. Are scope, non-goals, invariants, and slice boundaries sharp enough to prevent accidental implementation drift?
3. Are migration and compatibility stances explicit, especially where old behavior should be deleted, narrowed, or fail-closed?
4. Are failure semantics visible, retryable where relevant, and non-destructive where the design promises that?
5. Are child designs/plans given the right decision questions without pretending this design already solved them?
6. Are docs-audit, feature-coherency, and verification expectations named at the right level?
EOF
            ;;
        plan)
            cat <<'EOF'

Plan-specific review rubric:
1. Are tasks ordered so tests, ownership boundaries, schema changes, and call-site migrations happen safely?
2. Does every behavior change have a focused regression test and an expected failing state before implementation?
3. Are code ownership boundaries clear enough to avoid expanding existing hotspots unnecessarily?
4. Are rollback, retry, failure recovery, and diagnostic outputs covered where the design requires them?
5. Are verification commands specific, sufficient, and sequenced after the relevant tasks?
6. Are docs/coherency updates included when public claims, command help, routes, or assistant-facing surfaces change?
EOF
            ;;
        implementation)
            cat <<'EOF'

Implementation-specific review rubric:
1. Does the described implementation match the target design or plan without unreviewed scope creep?
2. Are changed files owned by the expected subsystem, with large-file or hotspot risks acknowledged?
3. Are security, trust, migration, and compatibility behaviors represented in code and tests?
4. Are failure paths covered with regression tests instead of only happy-path proof?
5. Are verification results enough to support merge/commit/push decisions?
EOF
            ;;
    esac

    cat <<'EOF'

Output format:
- Start with a one-paragraph verdict.
- Then list findings ordered by severity. Each finding must include: severity, blocking/non-blocking status, affected target section or repository file/line if available, repository evidence, why it matters, and the exact adjustment you recommend.
- Include open questions only if they block implementation.
- End with a short list of recommended patch bullets for the target.

Be tough but practical. Prefer concrete, actionable findings over general advice. If there are no blocking findings, say that explicitly and still list any non-blocking tightening suggestions.
EOF
}
```

5. Immediately after the `resolved_review_kind="$(infer_review_kind "$review_target")"` line, add:

```bash
resolve_features
```

6. In the dry-run block, after the Gemini command print and before `exit 0`, add:

```bash
    printf 'features:'
    printf ' %s' "${features[@]}"
    printf '\n'
    printf -- '--- prompt ---\n'
    build_prompt
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
bash scripts/test-agentic-plan-review.sh
bash scripts/agentic-plan-review.sh docs/superpowers/specs/2026-07-01-feature-card-context-tooling-design.md --feature packaging --dry-run | head -60
```

Expected: test prints `Agentic plan review wrapper fixtures passed.`; the dry-run shows the generated context list (packaging Start here files) and pressure points, and none of the old hardcoded list.

- [ ] **Step 5: Commit**

```bash
git add scripts/agentic-plan-review.sh scripts/test-agentic-plan-review.sh
git commit -m "feat(scripts): build plan-review prompts from feature ownership cards"
```

---

### Task 12: CI wiring

**Files:**
- Modify: `.github/workflows/pr-gate.yml`

**Interfaces:**
- Consumes: `scripts/test-agent-context.sh`, `scripts/agent-context.sh --validate` (no Rust toolchain needed — bash/awk/git only, which the docs-truth job already has).
- Produces: the map validator and tool tests run on every PR.

- [ ] **Step 1: Add two steps to the docs-truth job**

In `.github/workflows/pr-gate.yml`, inside the `docs-truth` job, after the `Check feature coherency completed scopes` step and before `Check support bundle privacy`, add:

```yaml
      - name: Test agent context tool
        run: bash scripts/test-agent-context.sh
      - name: Validate feature ownership cards
        run: bash scripts/agent-context.sh --validate
```

- [ ] **Step 2: Verify the workflow parses and the steps pass locally**

```bash
python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/pr-gate.yml'))" && echo yaml-ok
bash scripts/test-agent-context.sh
bash scripts/agent-context.sh --validate
```

Expected: `yaml-ok`, tests pass, `Feature ownership map validation passed (13 cards).`

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/pr-gate.yml
git commit -m "ci(pr-gate): validate feature ownership cards and agent-context tool"
```

---

### Task 13: Contributor docs and final verification

**Files:**
- Modify: `AGENTS.md`
- Modify: `docs/llms/README.md`
- Modify: `.github/PULL_REQUEST_TEMPLATE.md`

**Interfaces:**
- Consumes: the finished tool.
- Produces: contributor/agent-facing pointers; full-slice verification evidence.

- [ ] **Step 1: AGENTS.md paragraph**

At the end of the `## Build, Test, and Verification Commands` section of `AGENTS.md`, add:

```markdown
When starting a feature-scoped slice, run
`bash scripts/agent-context.sh --feature <slug>` (or `--path <file>` to route a
path) first: it prints the owning card's read-first files, safety invariants,
focused proof, and interaction gate from `docs/modules/feature-ownership.md`.
`--list` shows the slugs; `--run focused` / `--run gate` execute the card's own
proof commands.
```

- [ ] **Step 2: docs/llms/README.md bullet**

Replace the existing bullet

```markdown
- For feature-scoped work, use `docs/modules/feature-ownership.md` to find the
  start-here files, neighboring systems, focused proof, and broader interaction
  gate before editing.
```

with:

```markdown
- For feature-scoped work, run `bash scripts/agent-context.sh --feature <slug>`
  (or `--path <file>` to route a path) to print the owning card's start-here
  files, safety invariants, focused proof, and interaction gate before editing.
  `docs/modules/feature-ownership.md` stays the canonical map behind the tool.
```

- [ ] **Step 3: PR template hint**

In `.github/PULL_REQUEST_TEMPLATE.md`, add one comment line directly under the `## Ownership / Boundary` heading (template fields stay unchanged):

```markdown
<!-- bash scripts/agent-context.sh --path <changed-file> prints the owning card and its verification commands -->
```

- [ ] **Step 4: Run the full verification suite from the design**

```bash
bash scripts/agent-context.sh --validate
bash scripts/test-agent-context.sh
bash scripts/test-maintainability-drift-report.sh
bash scripts/maintainability-drift-report.sh --all | head -80
bash scripts/test-agentic-plan-review.sh
bash scripts/agentic-plan-review.sh docs/superpowers/specs/2026-07-01-feature-card-context-tooling-design.md --feature packaging --dry-run >/dev/null && echo dry-run-ok
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv <(bash scripts/docs-audit-inventory.sh)
bash scripts/check-doc-truth.sh
bash scripts/test-doc-truth.sh
bash scripts/test-coherency-ledger.sh
```

Expected: every command passes; eyeball the drift-report routing output for obviously wrong hints.

- [ ] **Step 5: Commit**

```bash
git add AGENTS.md docs/llms/README.md .github/PULL_REQUEST_TEMPLATE.md
git commit -m "docs: point contributors at agent-context as the slice entrypoint"
```
