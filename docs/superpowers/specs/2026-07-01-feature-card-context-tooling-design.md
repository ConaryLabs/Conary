# Feature Card Context Tooling Design

**Date:** 2026-07-01
**Status:** Draft design for external implementation (prepared for a
GPT 5.5 implementation pass; review with `scripts/agentic-plan-review.sh`
before lock-in).
**Scope:** make `docs/modules/feature-ownership.md` the single
machine-readable source of truth for path→feature routing, task context, and
verification commands; cut the two existing duplicate copies of that mapping
over to it; add a CI validator that keeps the map parseable and honest.

## Purpose

The repository keeps three hand-maintained copies of the same
"which feature owns this path, and what proves a change there" mapping:

1. `docs/modules/feature-ownership.md` — 13 canonical ownership cards with a
   documented schema (Capability, Start here, Neighbor systems, Focused
   proof, Interaction gate, Docs to update, Safety notes).
2. `scripts/maintainability-drift-report.sh` — `feature_hint_for_path()`
   (lines 83–130) re-encodes path→feature→proof routing as shell case globs.
3. `scripts/agentic-plan-review.sh` — `build_prompt()` (lines 216–290) bakes
   a frozen, packaging-era context file list and packaging-specific review
   goals into the prompt.

Copies 2 and 3 drift from copy 1 silently. Copy 2 already disagrees with
copy 1: four cards (CLI dispatch, declarative models, packaging/publish,
supported target profiles) have no drift-report routing branch at all, and
the drift report routes `apps/remi/src/federation/*` to a pseudo-feature
that has no card.

This slice compiles the existing map into an executable tool so that an
agent or contributor can run one command and receive the owning card's
read-first files, safety invariants, focused proof, and interaction gate —
and so the drift report and plan reviewer consume the map instead of
shadowing it.

## Core Decision

The Markdown map stays canonical. There is no parallel TOML/JSON source.

This follows existing repo precedent: `scripts/check-coherency-ledger.sh`
already parses `docs/modules/feature-ownership.md` headings to validate
ledger owners. The card schema is regular enough to parse (all 13 cards
carry every field except one missing Interaction gate, fixed in this slice).
Machine-readability is enforced by a validator in CI, not by moving the data
out of the document contributors already read and update.

This is a hard cutover, not a compatibility migration. When the slice lands:

- `feature_hint_for_path()` is deleted from the drift report.
- The hardcoded context list and packaging-specific goals are deleted from
  `agentic-plan-review.sh`.
- There is exactly one parser, living in the new tool.

## Current Repo Facts

Verified 2026-07-01:

- `docs/modules/feature-ownership.md` (682 lines, revision 19) has 13 cards.
  Field counts: Capability 13, Start here 13, Neighbor systems 13, Focused
  proof 13, Docs to update 13, Safety notes 13, Interaction gate 12. The
  card missing Interaction gate is **Supported Target Profiles**.
- Cards have **no path-glob field**. The only path→card mapping lives in the
  drift report's case statement. "Start here" lists concrete files, not
  coverage globs.
- Proof fields hold commands in backticks, semicolon-separated, sometimes
  followed by prose conditions ("when routing crosses query…").
- `scripts/check-coherency-ledger.sh` reads the map's `## ` headings as the
  allowed owner set. Card headings must therefore not be renamed by this
  slice.
- CI wiring: the docs-truth job in `.github/workflows/pr-gate.yml` runs
  check/test pairs for docs truth and feature coherency, then runs the
  docs-audit ledger and coherency wave-scope checks. Most `scripts/check-*.sh`
  files have `scripts/test-*.sh` siblings, but not all do today
  (`check-doc-audit-ledger.sh` and `check-coherency-wave-scopes.sh` are
  currently unpaired). New standalone tooling should still follow the sibling
  test convention.
- Repo tooling convention is bash with `set -euo pipefail`, a `usage()`
  heredoc, `fail()`, and `git rev-parse --show-toplevel` cd.

## Non-Goals

- No `docs/status/current.md` status page (separate decision; only worth
  doing with a docs-truth gate attached).
- No agent-ready GitHub issue templates (deferred until an external or
  cold-start contributor actually takes a slice).
- No JSON/TOML output mode for the tool (stable plain-text packet only;
  JSON can be a later additive flag).
- No hotspot decomposition; the maintainability roadmap already owns that.
- No changes to `conary-agent-contract`, MCP surfaces, or product code.
- No renaming of card headings (would break the coherency ledger owner
  check).

## Schema Change: Two New Card Fields

`docs/modules/feature-ownership.md` gains two fields per card, documented in
its "Card Schema" section:

- **Slug:** short unique kebab-case identifier, first field of each card.
  Proposed slugs: `dispatch`, `install`, `adopt`, `model`, `generation`,
  `ccs`, `packaging`, `profiles`, `remi`, `conaryd`, `bootstrap`,
  `conary-test`, `agent-mcp`.
- **Paths:** semicolon-separated glob patterns (backtick-quoted, matching the
  existing field style) that route repository paths to this card.

Initial Paths values translate the drift report's existing case globs, then
extend them so all 13 cards route. Translation table:

| Card (slug) | Paths (initial) |
|---|---|
| `agent-mcp` | `crates/conary-agent-contract/*`; `crates/conary-mcp/*`; `apps/remi/src/server/mcp.rs`; `apps/conary-test/src/server/mcp.rs` |
| `install` | `apps/conary/src/commands/install/*`; `apps/conary/src/commands/update/*`; `apps/conary/src/commands/remove.rs` |
| `adopt` | `apps/conary/src/commands/adopt/*` |
| `generation` | `crates/conary-core/src/generation/*` |
| `ccs` | `crates/conary-core/src/ccs/*`; `apps/conary/src/commands/ccs/*` |
| `remi` | `apps/remi/*` |
| `conaryd` | `apps/conaryd/*` |
| `bootstrap` | `apps/conary/src/commands/bootstrap/*`; `apps/conary-test/src/bootstrap.rs`; `crates/conary-bootstrap/*`; `docs/modules/bootstrap.md`; `docs/operations/bootstrap-selfhosting-vm.md`; `docs/operations/bootstrap-follow-up-investigations.md` |
| `conary-test` | `apps/conary-test/*`; `apps/conary/tests/integration/remi/manifests/*` |
| `dispatch` | derive from the card's Start here files (`apps/conary/src/dispatch.rs`, `apps/conary/src/dispatch/*`, `apps/conary/src/cli/*`, `apps/conary/src/command_risk.rs`, `apps/conary/src/live_host_safety.rs`) |
| `model` | derive from Start here files |
| `packaging` | derive from Start here files (publish, cook, recipe/kitchen, pkgbuild, container analysis paths) |
| `profiles` | derive from Start here files |

Implementer note: for the four "derive" rows, read the card's Start here
field and the owning directories, propose globs, and confirm each glob
matches at least one tracked file (the validator enforces this).

Routing semantics: **most-specific match wins**. Paths use shell
`case`/`[[ ... == pattern ]]`-style globs over repo-relative path strings, so
`*` may span path separators. Specificity is the character count of the
leading substring before the first glob wildcard character (`*`, `?`, or
`[`). This resolves the intended overlaps
(`apps/remi/src/server/mcp.rs` → `agent-mcp`, not `remi`;
`apps/conary-test/src/server/mcp.rs` → `agent-mcp`, not `conary-test`;
`apps/conary-test/src/bootstrap.rs` → `bootstrap`, not `conary-test`). Two
different cards matching a path with equal specificity is a validation
error.

Federation note: `apps/remi/src/federation/*` folds into the `remi` card
(it is Remi-owned code today). The drift report's "Remi federation"
pseudo-branch is deleted. If federation later becomes a first-class card,
it gets its own slug and Paths then.

This slice also adds the missing **Interaction gate** field to the
Supported Target Profiles card (author it from that card's Neighbor systems
and existing proof commands).

## New Tool: `scripts/agent-context.sh`

One script owns the parser. Modes:

```
Usage: scripts/agent-context.sh <mode> [options]

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
```

`--changed` collects paths exactly as the drift report does today in changed
mode (`git diff --name-only <base>`, `git diff --cached --name-only`,
`git ls-files --others --exclude-standard`, sorted unique). `--changed --all`
uses `git ls-files`, preserving `maintainability-drift-report.sh --all`.

### Task packet output (stable plain text)

```
# Task Packet: <card heading>
slug: <slug>
capability: <Capability text>

## Read first
<Start here entries, one per line>

## Paths owned
<Paths globs, one per line>

## Neighbor systems
<Neighbor systems text>

## Focused proof
<one backticked command per line>

## Interaction gate
<one backticked command per line>
when: <trailing prose condition from the field, if any>

## Docs to update
<Docs to update entries, one per line>

## Safety invariants
<Safety notes text>
```

Command extraction rule: proof/gate fields yield the backtick-quoted spans
as commands, in order; prose outside backticks in the Interaction gate field
is preserved as the `when:` note. `--run` executes only the backticked
commands. Execute each extracted command from the repo root through the shell
(`bash -lc "$command"`) so existing proof spans with redirection, quoted
arguments, or shell metacharacters keep their documented meaning.

### Fallback routes for non-card paths

Three doc-routing hints in the drift report are tooling policy, not feature
ownership, and stay out of the map. `agent-context.sh` carries them as a
small built-in fallback table, consulted only when no card's Paths match:

- `AGENTS.md`, `CONTRIBUTING.md`, `.github/PULL_REQUEST_TEMPLATE.md`,
  `docs/llms/*`, `docs/modules/feature-ownership.md`,
  `docs/superpowers/documentation-accuracy-audit-*`,
  `scripts/maintainability-drift-report.sh`, `scripts/agent-context.sh` →
  assistant/contributor guidance hint (docs-audit ledger check).
- `docs/modules/*`, `docs/operations/*`, `docs/INTEGRATION-TESTING.md`,
  `docs/ARCHITECTURE.md` → canonical docs hint.
- `docs/superpowers/plans/*`, `docs/superpowers/specs/*` → planning/design
  docs hint (agentic review before lock-in).

Paths matching neither a card nor a fallback print the current drift-report
"No feature-card hint matched" message.

### Validator rules (`--validate`)

- Every card (every `## ` section after "Card Schema") has all nine fields:
  Slug, Capability, Start here, Neighbor systems, Paths, Focused proof,
  Interaction gate, Docs to update, Safety notes.
- Slugs are unique and kebab-case.
- Every backtick-quoted repository path in Start here and Docs to update
  that does not contain a wildcard exists as a tracked file or directory
  (this catches doc rot the current gates miss). Non-path entries (prose,
  command examples) are skipped by only checking spans that resolve
  syntactically to repo paths.
- Every Paths glob matches at least one tracked file (no dead globs).
- No two cards match any tracked file at equal specificity.
- Focused proof contains at least one backticked command.

Implementation language: bash + awk, matching repo conventions
(`set -euo pipefail`, `usage()`, `fail()`, repo-root cd). The awk card
parser lives once, inside `agent-context.sh`.

## Cutover 1: `scripts/maintainability-drift-report.sh`

- Delete `feature_hint_for_path()` (lines 83–130).
- The "Changed Path Hints" section calls
  `scripts/agent-context.sh --changed --base "$base_ref"` in changed mode, or
  `scripts/agent-context.sh --changed --all` when the drift report is invoked
  with `--all`, and prints its output. The brief per-path format matches the
  current
  `- <path>\n  <feature> | focused: … | gate: …` shape so existing readers
  and `scripts/test-maintainability-drift-report.sh` expectations move, not
  break silently.
- The report stays warn-only.

## Cutover 2: `scripts/agentic-plan-review.sh`

- New repeatable flag `--feature <slug>`; at least one is required (hard
  cutover — no hidden default context). Usage text shows a packaging
  example: `--feature packaging --feature ccs --feature remi`.
- The "Required local context" list in `build_prompt()` is generated as the
  union, over selected cards, of: Start here entries, Docs to update
  entries, and Paths globs. The stable preamble docs (`AGENTS.md`,
  `docs/llms/README.md`, `docs/ARCHITECTURE.md`,
  `docs/INTEGRATION-TESTING.md`, `docs/modules/feature-ownership.md`)
  remain always-included.
- Shared review goals 1–4 and the evidence rules stay verbatim. Packaging-
  specific goals 5 and 7 are deleted; in their place the prompt injects each
  selected card's Safety notes under a "Review pressure points from feature
  ownership" heading. Goal 6 (supported-distro scope) stays — it is global
  repo policy, already stated in the map preamble.
- Before deleting packaging-specific goal 7, update the packaging card's
  Safety notes so no hardening pressure point is lost. At minimum preserve
  attestation requirements, static publish trust, Remi upload trust, and the
  recorded-draft publication refusal enforced by `publish_context.rs` and
  `publish_gate.rs`.
- `--context` stays for extras (e.g., the archived M2/M3 packaging specs
  when reviewing packaging plans).
- `scripts/test-agentic-plan-review.sh` updates: dry-run assertions check
  that selected cards' Start here files appear in the planned prompt and
  that the old hardcoded list is gone; invoking the script without at least
  one `--feature` fails non-zero with a helpful error.

## Cutover 3: verification wrapper

`agent-context.sh --feature <slug> --run focused` and `--run gate` are the
verify-slice wrapper: they execute the card's own proof commands. No
separate `verify-slice.sh` script — the commands already live on the cards,
so a second wrapper would be a fourth copy of the mapping.

## CI Wiring

In the docs job of `.github/workflows/pr-gate.yml`, alongside the existing
doc-truth/coherency steps, add:

```yaml
- name: Test agent context tool
  run: bash scripts/test-agent-context.sh
- name: Validate feature ownership cards
  run: bash scripts/agent-context.sh --validate
```

## Testing Strategy

`scripts/test-agent-context.sh` (sibling convention), using small fixture
map files written to a temp dir and passed via `--map`:

- Parse: a well-formed two-card fixture yields correct packets for
  `--feature`, `--list`, `--brief`.
- Routing: most-specific-wins (fixture with `a/*` and `a/b.rs` on different
  cards routes `a/b.rs` to the specific card); fallback table used when no
  card matches, including `docs/superpowers/specs/*`; unmatched path prints
  the no-hint message.
- Changed/all collection: a fixture git repo proves `--changed` includes
  modified, staged, and untracked paths, while `--changed --all` includes all
  tracked paths and preserves the drift report's `--all` behavior.
- Command extraction: semicolon-separated backticked commands split
  correctly; Interaction gate prose lands in `when:`.
- Validation failures (each a distinct fixture, each must exit non-zero
  with a message naming the card): missing field, duplicate slug, dead
  glob, equal-specificity overlap, missing Start-here file, proof without
  a backticked command.
- Real-map smoke assertions (no fixture): `--validate` passes;
  `--path apps/conary/src/commands/install/mod.rs` routes to `install`;
  `--path apps/remi/src/server/mcp.rs` routes to `agent-mcp`;
  `--path apps/conary-test/src/bootstrap.rs` routes to `bootstrap`.
- `--run` uses a fixture card whose focused proof is `true`; a fixture with
  `false` proves fail-fast non-zero exit.

Update `scripts/test-maintainability-drift-report.sh` for the cutover and
run `scripts/test-agentic-plan-review.sh` after its changes.

## Documentation And Audit

- `docs/modules/feature-ownership.md`: Card Schema section documents Slug
  and Paths; all 13 cards gain both fields; Supported Target Profiles gains
  its Interaction gate; bump the frontmatter revision and `last_updated`.
- `AGENTS.md` and `docs/llms/README.md`: one short paragraph each pointing
  contributors and agents at `scripts/agent-context.sh` as the first
  command to run when starting a slice.
- `.github/PULL_REQUEST_TEMPLATE.md`: optionally reference the tool as the
  way to fill the boundary/verification fields (keep the template fields
  unchanged).
- Regenerate the docs-audit inventory if the touched docs are inventoried
  (`bash scripts/docs-audit-inventory.sh` diffed against
  `docs/superpowers/documentation-accuracy-audit-inventory.tsv`, per the
  existing drift-report check).
- Before locking this design doc itself, stage or otherwise include it in the
  tracked-file set used by `scripts/docs-audit-inventory.sh`, then refresh the
  docs-audit inventory and ledger rows. The inventory script only sees tracked
  Markdown files, so an untracked spec can look clean locally and still fail
  CI once committed without audit metadata.
- `docs/modules/feature-ownership.md` heading set is unchanged, so
  `scripts/check-coherency-ledger.sh` owner validation is unaffected —
  verify by running it.

## Verification Guidance

Before claiming the slice done:

```
bash scripts/agent-context.sh --validate
bash scripts/test-agent-context.sh
bash scripts/test-maintainability-drift-report.sh
bash scripts/maintainability-drift-report.sh --all   # eyeball routing output
bash scripts/test-agentic-plan-review.sh
bash scripts/agentic-plan-review.sh <any spec> --feature packaging --dry-run
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
```

## Plan Questions

Decisions the implementation plan should surface rather than assume:

1. Exact Paths globs for the four cards without existing drift-report
   branches (`dispatch`, `model`, `packaging`, `profiles`) — propose from
   Start here fields, confirm with the card owner before lock-in.
2. Whether `--run` ships in this slice or as an immediate follow-up once
   packet output is stable (packet/routing/validation are the load-bearing
   parts; `--run` is additive).
3. Wording of the Supported Target Profiles Interaction gate field.
4. Whether the drift report's brief-hint line format should stay
   byte-compatible or is allowed to improve (tests pin whichever is
   chosen).
