---
last_updated: 2026-07-07
revision: 1
summary: Design for conary CLI output UX — quiet-by-default logging with verbosity control, a single ui module with a unified status vocabulary, and CI + snapshot enforcement so output stays consistent.
status: draft
---

# Conary CLI Output UX — Design

## Purpose

Make what a person running `conary` actually *sees* quiet, consistent, and
regression-proof. Today the CLI has good bones (curated help tiers, directional
error messages, semantic progress bars) but two problems undercut the
experience:

1. **Log flood.** Every command prints `tracing` INFO logs — schema-migration
   chatter, RFC3339 timestamps, module targets — to stderr by default. The
   actual answer to the user's command is buried under database internals.
2. **Inconsistent vocabulary.** ~2,096 hand-rolled `println!`/`eprintln!` call
   sites, no single source of truth. The same three states are spelled a
   half-dozen ways with random casing (`[OK]`/`[COMPLETE]`/`[VALID]`,
   `[FAIL]`/`[FAILED]`/`[ERROR]`, `Warning:`/`[WARN]`/`[WARNING]`).

This spec covers only the CLI (`apps/conary`). It does not change the wording or
routing of refusal/guidance text — the
[`daily-driver-ux-matrix`](../../operations/daily-driver-ux-matrix.md) owns that
contract, and this design stays consistent with its "keep common commands
boring, testable, and honest" ethos.

## Current-state facts (verified 2026-07-07)

These premises were checked against source and a fresh debug build:

- `conary_bootstrap::init_tracing()` (`crates/conary-bootstrap/src/lib.rs:5`)
  defaults the `EnvFilter` to `info` when `RUST_LOG` is unset, writes to stderr
  with the full `fmt()` format (timestamp + level + target).
- `init_tracing()` has exactly **two** callers: `apps/conary/src/app.rs:11`
  (the CLI) and `apps/remi/src/bin/remi.rs:255` (the long-running server, where
  `info`-to-stderr is correct and must not change).
- `init_tracing()` runs **before** `Cli::parse()` in `app.rs`, so honoring a
  `--verbose` flag requires reordering.
- The top-level `Cli` struct (`apps/conary/src/cli/mod.rs:139`) already has
  `--seccomp-warn` and `--help-advanced` globals. There is **no** global
  verbosity flag; several subcommands (`derive`, `query`, `model`, `verify`,
  `federation`, `capability`, `label`, `redirect`) each define their own local
  `verbose` bool.
- **Command tiering is already implemented.** `conary --help` shows 15 everyday
  commands; `conary --help-advanced` + `#[command(hide = true)]` cover the rest.
  Re-tiering is out of scope.
- The only styling/output crate in the tree is `indicatif` (progress bars),
  which transitively provides `console`. `progress.rs` already color-codes
  progress semantically (green install, red remove, yellow update, cyan adopt).
- Good patterns to preserve and emulate: the provenance "package DNA" tree
  (`apps/conary/src/commands/provenance.rs:1217`), directional errors in
  `app.rs::render_error_lines`, and clean empty states (`No packages found.`).

## Goals

- Default CLI output is curated: internal `tracing` logs are hidden unless the
  user opts in.
- One canonical visual vocabulary for status, messages, and detail lines, owned
  by a single module.
- Verbosity is user-controllable with familiar top-level flags.
- Consistency is enforced automatically so it cannot silently regress.

## Non-goals

- Migrating all ~2,000 `println!` sites (bounded migration only; see
  [Migration boundary](#migration-boundary)).
- Any change to `remi` or `conaryd` logging.
- Emoji, Unicode symbol glyphs, or a full-screen TUI. Output stays ASCII-only.
- Rewording error/refusal/guidance text (owned by the daily-driver UX matrix).
- Re-tiering the command surface (already shipped).

## Architecture

The work is two independent pillars plus an enforcement layer. Pillar A and
Pillar B share no state and can ship in either order; enforcement lands after
Pillar B's migration so it starts green.

```
                 apps/conary
  ┌────────────────────────────────────────────────┐
  │ app.rs                                          │
  │   Cli::parse()  ──►  derive level  ──►          │
  │   conary_bootstrap::init_cli_tracing(level)     │  ◄── Pillar A
  │                                                 │
  │ ui/  (new)  ── single source of output styling  │  ◄── Pillar B
  │   error()/warn()/note()/status()                │
  │   row(Status, …) / field(label, value)          │
  │   color backend: console (TTY + NO_COLOR aware) │
  │                                                 │
  │ commands/*  ── call ui::* instead of raw print  │
  │ commands/progress.rs ── phase strings via ui    │
  └────────────────────────────────────────────────┘
           │
           ▼
  crates/conary-bootstrap
    init_server_tracing()      ◄── remi (unchanged: info, full fmt)
    init_cli_tracing(level)    ◄── conary (warn default, compact fmt)
```

## Pillar A — Quiet by default + verbosity control

### Logging backend split

Refactor `conary-bootstrap` so the CLI and the server express intent
explicitly instead of sharing one `info` default:

- `init_server_tracing()` — preserves today's behavior (`EnvFilter` default
  `info`, full `fmt()` with timestamp + target, stderr). `remi` calls this. No
  behavior change for the server.
- `init_cli_tracing(level: LevelFilter)` — compact formatter with **no
  timestamp and no target** (`.without_time().with_target(false)`), stderr,
  `EnvFilter` seeded from `level`.

Both continue to honor `RUST_LOG` when it is set (see precedence below), keeping
the power-user escape hatch.

**Should-have (same phase):** give `init_cli_tracing` a minimal custom event
formatter so a tracing `WARN`/`ERROR` renders as `warning: …` / `error: …`,
matching Pillar B's vocabulary rather than tracing's `WARN`/`ERROR` labels. If
this proves fiddly it can be deferred; the compact formatter is the baseline
requirement.

### Verbosity flags

Add two top-level logging flags to `Cli`. They are intentionally **not**
`global = true`: existing subcommands already own local `--verbose` and
`--quiet` flags for command-specific detail/refresh behavior, and Clap
propagated globals cannot share those long option names. The accepted logging
forms are `conary --verbose ...` and `conary -q ...`; command-local
`conary query scripts --verbose` keeps its existing meaning.

- `--verbose` — repeatable count: one use → `info`, two → `debug`,
  three or more → `trace`. No `-v` short because subcommands already use
  `-v` for `--version`/local verbosity.
- `-q, --quiet` — `error`-only. Conflicts with top-level `--verbose`.

**Level precedence** (highest wins):

1. `RUST_LOG` environment variable, if set (unchanged escape hatch).
2. Top-level `-q` / `--verbose` flags.
3. Default: `warn`.

### app.rs reordering

Move tracing init to **after** `Cli::parse()`:

```
Cli::parse()  →  compute LevelFilter from (RUST_LOG?, quiet, verbose count)
              →  init_cli_tracing(level)
              →  dispatch
```

`Cli::parse()` already handles `--help`, `--version`, and parse errors by
exiting; none of those paths need tracing, so nothing user-visible is lost by
initializing later. Logging emitted before parse is effectively nonexistent in
the current code, so the reorder has negligible cost.

### Optional cleanup

Keep per-subcommand `verbose` / `quiet` bools where they mean command detail or
workflow behavior rather than log level.

## Pillar B — The `ui` module + unified vocabulary

### Module

New `apps/conary/src/ui/` module — the **only** place user-facing output styling
is defined. Everything else calls into it.

### API

Message helpers (cargo-style, lowercase, colored+bold prefix):

| Function | Renders | Color | Stream |
|---|---|---|---|
| `ui::error(msg)` | `error: {msg}` | red | stderr |
| `ui::warn(msg)` | `warning: {msg}` | yellow | stderr |
| `ui::note(msg)` | `note: {msg}` | cyan | stderr |
| `ui::status(verb, msg)` | `{verb} {msg}` (bold green verb, cargo-style) | green | stdout |

Row helper for per-item lists — column-aligned tags:

- `ui::row(status: Status, cells: &[&str])` → `[ok]      nginx    1.27.2`
- `enum Status { Ok, Fail, Warn, Skip, Info, Off, Missing, Pending }`. A code
  survey of the actual bracket tags in use (filtering out code noise like
  `#[test]` and `&[String]`) found the real indicator vocabulary is these
  eight states — success/fail/warn/skip plus the multi-state indicators
  `off`/`missing`/`pending` (e.g. `federation.rs:198` renders `[OK]`/`[OFF]`,
  `bootstrap/setup.rs:51,108` render `[OK]`/`[MISSING]` and
  `[COMPLETE]`/`[PENDING]`) and `info`.
- Tags are `[ok]`, `[fail]`, `[warn]`, `[skip]`, `[info]`, `[off]`, `[missing]`,
  `[pending]` — bracket plus a lowercase word, no inner padding. Because the
  words differ in length, `row` right-pads the tag (by **visible** width, so
  ANSI is not counted) to the widest tag so the following columns line up. This
  supersedes the earlier fixed-width `[ ok ]` sketch, which only worked when
  every state fit in four characters.
- Colors: Ok=green, Fail=red, Warn=yellow, Skip=dim, Info=cyan, Off=dim,
  Missing=red, Pending=yellow.

Detail helper for key/value screens:

- `ui::field(label: &str, value: &str)` — consistent label/value rendering for
  `info`/`show` output, visually compatible with the provenance tree.

### Canonical vocabulary

Lowercase everywhere. This is the single mapping that replaces every existing
spelling:

| State | Message form | Row tag | Color | Replaces |
|---|---|---|---|---|
| success | green verb via `ui::status` | `[ok]` | green | `[OK]`, `[COMPLETE]`, `[DONE]`, `[VALID]` |
| failure | `ui::error` → `error:` | `[fail]` | red | `[FAILED]`, `[FAIL]`, `[ERROR]` |
| warning | `ui::warn` → `warning:` | `[warn]` | yellow | `Warning:`, `[WARN]`, `[WARNING]`, `[warning]` |
| info | `ui::note` → `note:` | `[info]` | cyan | `[INFO]` |
| skipped | — | `[skip]` | dim | ad-hoc "skipped"/"already" strings |
| off | — | `[off]` | dim | `[OFF]` |
| missing | — | `[missing]` | red | `[MISSING]` |
| pending | — | `[pending]` | yellow | `[PENDING]` |

The "replaces" column is exactly the literal word-list the
[guardrail](#ci-guardrail) enforces, so the vocabulary table and the guard stay
in lockstep. Tags are lowercase and ASCII-only. Lowercase descriptive markers
that are *not* status indicators (e.g. `[circular]`, `[blocked]`, `[deferred]`,
`[already shown]`) are prose, not vocabulary, and are out of scope.

The canonical table is copied into `AGENTS.md` (the repo's canonical conventions
doc) so future contributors have one reference.

### Color backend

Reuse **`console`** (already in the tree via `indicatif`, already how
`progress.rs` colors output). No new top-level dependency. Color is enabled only
when the target stream is a TTY; `NO_COLOR` disables and `CLICOLOR_FORCE`
forces, per the de-facto conventions. `console` provides all of this.

### Progress alignment

`progress.rs` keeps its `indicatif` bars but its phase strings (`[done]`,
`[FAILED: …]`, etc.) are routed through the vocabulary so a completed/ failed
package reads the same as everywhere else.

### Migration boundary

In scope for this spec: **every** site emitting a guarded-vocabulary literal —
roughly 100 occurrences across ~30 files (about 33 warnings, 18 errors, and 54
success/info/state tags), plus `progress.rs`'s phase strings. When the guard is
green, the whole guarded vocabulary is centralized.

Out of scope: the ~2,000-site long tail of general `println!` prose that carries
no status tag. It is converted opportunistically; the guard prevents new drift.

Two kinds of enforcement, with different reach:

- **Guard** — machine-guarantees the *vocabulary* is centralized everywhere in
  `apps/conary/src`.
- **Snapshots** — lock the *exact output* of `list` and `search` only (the two
  daily commands that are cheap to drive deterministically). Other daily
  commands (`install`, `update`, …) benefit from the sweep but are not
  snapshot-locked here, because exercising them needs heavy repo/package
  fixtures. This is a deliberate coverage boundary, not an oversight.

## Enforcement

### CI guardrail

A test (`apps/conary/tests/output_vocabulary_guard.rs`) scans `apps/conary/src`,
excluding `apps/conary/src/ui/`, and fails if any line contains a literal from an
explicit word-list — `[OK] [COMPLETE] [DONE] [VALID] [FAIL] [FAILED] [ERROR]
[WARN] [WARNING] [INFO] [OFF] [MISSING] [PENDING]` (case-insensitive) or a
printed string beginning `Warning:`.

It is a **line-level lint**, not a semantic analyzer: it matches the exact
listed words only, so unrelated bracket tags (`[SUMMARY]`, `[ORPHANS]`, section
headers) and code noise (`#[test]`, `&[String]`) are untouched. Two consequences
to plan around: (1) it also flags a listed word appearing in a *comment*, so
such comments get reworded during migration; (2) it matches only the closed
form, so `progress.rs`'s inline `[FAILED: {err}]` (colon before `]`) is **not**
caught and is converted by hand.

The word-list is introduced incrementally alongside the migration families
(warnings, then errors, then success/info/state) so that each migration step
ends with the guard green rather than deferring one large red-to-green jump.

### Snapshot tests

`cargo test -p conary --test cli_output_snapshots` — golden-output tests for the
daily-driver commands:

- Output captured with `NO_COLOR=1` for determinism (no ANSI in golden files).
- A fixed database fixture and no timestamps, so runs are reproducible.
- Committed expected files; a mismatch fails CI. This mirrors the repo's
  existing focused `cli_daily_ux` test style.

## Testing strategy

- **Unit** — `ui` module: each helper renders the exact expected string; color
  is present when forced and absent under `NO_COLOR`; tag widths align.
- **Behavioral** — the log-flood fix: `conary list` on an initialized db emits
  no `INFO` lines by default; top-level `--verbose` restores `info`;
  top-level `-q` suppresses `warn`; `RUST_LOG` overrides flags.
- **Snapshot** — daily-driver command output matches golden files.
- **Guardrail** — the vocabulary scan passes after migration and fails when a
  raw tag is reintroduced (verified with a deliberate temporary violation).

## Phasing

Independently shippable, in this order:

1. **P0 logging.** `init_server_tracing`/`init_cli_tracing` split, `warn`
   default, compact formatter, top-level `--verbose`/`-q`, `app.rs` reorder. Highest
   perceived-quality win per unit of effort.
2. **`ui` module.** API, `console`-backed color/TTY handling, unit tests. No
   output change yet — the module simply exists.
3. **Migration.** Convert status-tag sites and daily-driver command paths to
   `ui`; align `progress.rs` phase strings.
4. **Enforcement.** Vocabulary guardrail + snapshot tests; document the
   vocabulary table in `AGENTS.md`. Lands last so the guard starts green.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Reordering tracing init drops very-early logs | None exist in current code; negligible. |
| Snapshot nondeterminism from color/timestamps | Force `NO_COLOR=1`, fixed fixture, strip/avoid timestamps. |
| Guard false-positives on non-status bracket tags | Word-list matches only the explicit status/indicator literals; section headers and code noise are excluded. Comments containing a listed word are reworded during migration. |
| Changing shared `init_tracing` breaks `remi` logging | Split into two functions; `remi` keeps `init_server_tracing` with identical behavior. Only 2 callers total. |
| Custom CLI tracing formatter proves fiddly | Compact formatter (no time/target) is the baseline; `warning:`/`error:` alignment is a should-have that can defer. |

## Success criteria

- `conary list` on a healthy system prints only its answer — no `INFO` chatter —
  by default; top-level `--verbose`/`-q` plus `RUST_LOG` control verbosity as
  specified.
- Every status/message/detail line in the daily-driver commands and the
  status-tag sites goes through `ui`, using the one canonical vocabulary.
- The guardrail and snapshot tests pass in CI and fail on regression.
- `remi` logging is byte-for-byte unchanged.
