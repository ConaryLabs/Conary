# Repository Guidelines

## Project Structure & Module Organization
Conary is a virtual Rust workspace. The package-manager CLI lives in `apps/conary/src/` (`commands/`, `cli/`, `main.rs`). Core package-management logic is in `crates/conary-core/src/`. Remi lives in `apps/remi/src/` (`server/`, `federation/`, `bin/remi.rs`), and conaryd lives in `apps/conaryd/src/` (`daemon/`, `bin/conaryd.rs`). Test helpers and integration coverage live in `apps/conary/tests/`, `crates/conary-core/tests/`, and `apps/conary-test/src/`. Packaging assets are under `packaging/` and `deploy/`; current roadmap state lives under `docs/roadmaps/`, while durable design decisions and contracts live in the architecture, module, and specification docs that own the affected surface.

## Build, Test, and Verification Commands
- `cargo build -p conary`: build the package-manager CLI.
- `cargo build -p remi`: build the Remi service.
- `cargo build -p conaryd`: build the daemon.
- `cargo build -p conary-test`: build the test harness.
- `cargo test -p conary` or `cargo test -p conary-core`: target the CLI or core library.
- `cargo test -p remi` or `cargo test -p conaryd`: target service-owned code directly.
- `cargo run -p conary-test -- list`: check manifest parsing and suite inventory when touching integration-test inputs.
- `cargo clippy --workspace --all-targets -- -D warnings`: enforce zero-warning linting across the workspace.
- `cargo fmt --check`: verify formatting before you push.

When starting a feature-scoped slice, run
`bash scripts/agent-context.sh --feature <slug>` (or `--path <file>` to route a
path) first: it prints the owning card's read-first files, safety invariants,
focused proof, and interaction gate from `docs/modules/feature-ownership.md`.
`--list` shows the slugs; `--run focused` / `--run gate` execute the card's own
proof commands.

## GitHub Workflow
Use GitHub Issues as the normal work record and pull requests as the integration
path. Before a non-trivial implementation, bug fix, refactor, documentation,
operations, or maintenance slice, search for an existing issue and confirm or
open one primary Bug, Feature, or Task with scope, acceptance criteria,
ownership, and expected proof. Read-only scoping stays read-only until a write
is authorized, and security reports use private advisories instead of public
issues.

Refresh `main`, then work on an issue-linked branch such as
`fix/42-rpm-parser-overflow`; do not commit or push repository changes directly
to `main`. Every PR links its primary issue. Use `Closes #...` only when the PR
satisfies the issue's acceptance criteria; use `Refs #...` when it advances a
larger issue that must remain open. Open substantial work as a draft PR early,
keep decisions and verification current there, and merge only through GitHub
after the required checks and review conversations are complete. Roadmaps and
canonical architecture, module, and specification docs remain the durable
truth owners described below; link them from the issue rather than replacing
durable repo truth with issue comments.
See `CONTRIBUTING.md` for the full lifecycle and the narrow trivial-change and
urgent-bypass rules.

## Coding Style, Safety, and Commits
Use standard Rust formatting (`cargo fmt`) and keep Clippy clean. Indentation is 4 spaces. Follow Rust naming conventions: `snake_case` for functions/modules, `CamelCase` for types, `SCREAMING_SNAKE_CASE` for constants. Keep modules focused by subsystem. This repository expects each Rust source file to begin with a path comment such as `// conary-core/src/...`.

Recent history uses conventional-style prefixes such as `fix:`, `security:`, and `docs:`. Keep commit subjects short and imperative, e.g. `security(federation): pin https peer identity`. PRs should explain the problem, summarize the fix, list verification commands run, and link the relevant issue/plan entry. Include logs or API examples when behavior changes are not obvious from the diff.

## Pre-Alpha Product And Engineering Contract
Until a durable roadmap milestone explicitly establishes an external
compatibility promise, Conary is pre-alpha: do not maintain backward
compatibility. Make issue-backed hard cuts, replace the current schema and
rebuild disposable state, and delete superseded migrations, adapters, routes,
bypass flags, and compatibility paths in the same slice. When a better
architecture wins, finish the switch and remove the old authority instead of
running both.

Cross-distribution package installation is the primary product path. The source
package format owns its lifecycle ABI, dependency, version, payload, and
configuration semantics; Conary owns install, update, remove, rollback, and
generation publication; the target exposes typed capabilities. Runtime
conversion and installation must not invoke or depend on the source package
manager or its database. Adoption/takeover is the only migration-continuity
exception. The canonical contract is
`docs/specs/foreign-package-lifecycle-contracts.md`.

Derive supported package behavior from pinned upstream documentation and source
code, then encode it as typed grammars, state machines, and conformance tests.
Correctness authority must not come from heuristics, regexes, substring
matching, manually curated token lists, distro-name gates, or silent defaults.
Those mechanisms may produce diagnostics, privacy redaction, discovery
evidence, or work prioritization only; they cannot block publication or
establish compatibility, mutation, security, or event authority. A typed
preflight failure may stop mutation while an exact semantic is missing, but it
is a required implementation defect to engineer, not a permanent unsupported
class, human-review queue, blocklist, or operator reconciliation workflow.

Agent-assisted contribution and operation are first-class product paths. Keep
ownership, safety invariants, and proof discoverable from tracked repository
truth so a contributor and coding agent need no private prompt lore. Runtime
and fleet workflows must expose versioned, typed, inspectable resources plus
plan/apply results through `conary-agent-contract`; MCP is an adapter to that
contract, not a separate authority. Do not make an essential operation
available only through ad hoc shell, interactive human judgment, or untyped
free-form output, and do not weaken trust or approval boundaries for agents.

## Defect Discipline

Conary's product promise is that ordinary packages just work. A defect that is
routed around rather than fixed is a promise that has not been kept, so treat
the following as binding rather than aspirational.

**Fix what you find.** When work uncovers a half-implementation, a duplicated
authority, or a path that only works by luck, fix it or file it with the exact
evidence. Do not build around it and leave it for the next person to rediscover.
Scope discipline is real, and a finding outside the current slice belongs in an
issue rather than in the slice; what is not acceptable is knowing and saying
nothing.

**A failure is evidence until it is explained.** Do not re-run a failing check
hoping for a different result, and do not label a failure flaky, environmental,
or unrelated without a specific reason that survives being checked. Two failures
with different names may be one defect. An intermittent failure usually means
the system is genuinely nondeterministic somewhere, which is itself the defect.
Retrying until green converts a known failure into an unknown one.

**Fix the cause, not the symptom.** Restoring a deleted value, widening a bound,
or adding a special case makes a check pass without removing the condition that
produced it. Ask what structure allowed the defect, and correct that. If the
same fact is maintained in two places, the fix is one owner, not two updates.

**Prove the property, not the instance.** A regression test that pins the exact
input that failed will not catch the next instance of the same class. Where a
contract exists between two components, test the contract: that everything one
side can emit, the other admits.

**Verification means the command ran.** Report the commands and their real
output. Never describe an expected result as an observed one. When something
could not be verified, say so plainly and name what remains unproven; a partial
result reported accurately is worth more than an overstated one.

## Maintainability & Refactor Discipline

Treat large files as review signals, not automatic failures. Planning has a
hard gate: when a proposed slice would add behavior to a source file that is
already over 1000 lines, the issue, design, or plan must include an
ownership-based refactor or reorganization in that same slice. Put the new
behavior in the resulting focused module; do not defer the reorganization to a
follow-up. Thin hub registration, dispatch, and re-export wiring may remain in
the large file when they do not add business logic. When changing existing
behavior in a Rust file over 1500 lines, name the ownership boundary being
preserved or improved before editing. Files over 2500 lines should get a
reviewed decomposition path before major feature work unless the task is an
urgent fix.

Refactor and pruning slices must say which behavior moves, which module owns it
afterward, which persisted state or public surface is affected, and which
focused test proves behavior stayed the same or changed intentionally. Do not
split files mechanically, keep command and route handlers thin, and update
`docs/llms/subsystem-map.md` or the relevant `docs/modules/*.md` file when the
"look here first" path changes.

Meta-layer budget: roadmap, ownership-card, gate, and agent-tooling changes are
allowed only when product work forces them -- a touched path, a failing gate,
or a factual drift. Discretionary meta-layer improvement is capped at one meta
slice per four product slices. This budget holds at least until the first
external tester milestone in `docs/roadmaps/development-roadmap.md` is met.

## Testing and Documentation Guidance
Prefer small unit tests near the code they cover and integration tests in `apps/conary/tests/` for end-to-end CLI flows. Name tests descriptively, for example `test_prepare_discovered_peer_rejects_https_without_pinned_fingerprint`. When touching service code, rerun the owning packages directly with `cargo test -p remi` and `cargo test -p conaryd`. Security and transaction changes should include regression coverage.

Start assistant-facing work with:

- `AGENTS.md` for the repo contract and verification expectations
- `docs/llms/README.md` for the vendor-neutral assistant map
- `docs/ARCHITECTURE.md` and `docs/modules/*.md` for subsystem background
- `docs/INTEGRATION-TESTING.md` when validation spans `conary-test`
- `docs/operations/infrastructure.md` for MCP, deploy, and host workflow notes

Assistant doc model:

- `AGENTS.md` is the canonical repo-wide assistant contract.
- `docs/llms/README.md` is the vendor-neutral routing layer into canonical docs.
- Tool-specific entrypoints such as `CLAUDE.md`, `GEMINI.md`, `REASONIX.md`, or `.github/copilot-instructions.md` should point back here instead of restating repo-wide rules.
- Keep `CLAUDE.md` as a thin compatibility shim for Claude setups, and keep old `.claude/` harness files retired unless the repository adopts a shared Claude-specific harness again.
- Public claims are protected by `scripts/check-doc-truth.sh`; feature cards
  own subsystem routing and interaction proof; focused tests own behavior.
  Rerun all three layers named by the touched feature card when a public claim,
  command help, route, or agent-facing surface changes.
- Detailed roadmap state lives in `docs/roadmaps/`. Record durable design
  decisions in the owning architecture, module, or `docs/specs/` document.
  Track bounded multi-step execution in the primary issue and draft pull
  request; stable public or persisted contracts live in `docs/specs/`.
- After canonical truth, proof, roadmap state, and resume facts are durable,
  delete completed, superseded, or abandoned planning from the current tree.
  Git history is the planning-history source; do not create a replacement
  archive.
- Add nested `AGENTS.md` files only when a subtree genuinely needs durable instructions that differ from the repo root.
- Keep host-local, credential-bearing, or personal notes in ignored local files such as `docs/operations/LOCAL_ACCESS.md`, not in tracked assistant guidance.

Keep this file map-like. If a detail changes often or needs more than a short paragraph to explain, move it into a linked canonical doc instead of expanding this file.

## CLI Output Conventions
For `apps/conary`, all user-facing status output goes through `apps/conary/src/ui/`.
Do not print raw status tags; `apps/conary/tests/output_vocabulary_guard.rs`
enforces the guarded vocabulary.

`Status { Ok, Fail, Warn, Skip, Info, Off, Missing, Pending }` renders lowercase
bracketed tags (`[ok]`, `[fail]`, `[warn]`, `[skip]`, `[info]`, `[off]`,
`[missing]`, `[pending]`), and `ui::row` aligns the following columns. Use
`ui::warn`, `ui::error`, `ui::note`, `ui::status`, `ui::row`, `ui::heading`, and
`ui::field` instead of hand-rolled prefixes. Tags stay ASCII-only; color comes
from `console` and respects non-TTY output plus `NO_COLOR`.

CLI logging defaults to `warn`. Top-level `--verbose` is repeatable for
info/debug/trace, top-level `-q`/`--quiet` leaves errors only, and `RUST_LOG`
overrides both. Command-local `--verbose`/`--quiet` flags keep their
command-specific meanings. Internal `tracing` logs are not primary user output.

## Security & Contributor Notes
Do not weaken trust defaults casually. HTTPS federation peers should use pinned
fingerprints, and service changes should be verified with `cargo test -p remi`
and `cargo test -p conaryd`. Avoid destructive Git commands in shared
worktrees. Pre-alpha schema changes replace the current schema definition and
state their rebuild impact; do not introduce a compatibility chain.
Historical review prompts and finished planning do not remain in the active
documentation tree. Preserve durable truth first, then delete them and use Git
history when historical context is needed.
