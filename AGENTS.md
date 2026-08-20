# Repository Guidelines

## Start With The Smallest Useful Context

Conary is a virtual Rust workspace. The package-manager CLI is in
`apps/conary/`, core package behavior in `crates/conary-core/`, Remi in
`apps/remi/`, conaryd in `apps/conaryd/`, and the integration harness in
`apps/conary-test/`. Shared bootstrap, agent-contract, and MCP helpers live
under `crates/`; packaging and deployment assets live under `packaging/` and
`deploy/`.

Before feature-scoped work, run one routing command:

```bash
bash scripts/agent-context.sh --feature <slug>
bash scripts/agent-context.sh --path <file>
```

Use `--list` to discover slugs and `--brief` for a one-line route. Read the
packet's start-here files and only the canonical docs relevant to the task.
Do not preload the complete feature-ownership map or broad subsystem docs for
ordinary scoped work. Use `--run focused` for the card's narrow proof and
`--run gate` only when its stated interaction condition applies.

Canonical orientation begins at `docs/llms/README.md`. Detailed roadmap state
lives under `docs/roadmaps/`; durable architecture and behavior live in
`docs/ARCHITECTURE.md`, `docs/modules/`, and `docs/specs/`.

## Build And Verification

- `cargo build -p conary`, `-p remi`, `-p conaryd`, or `-p conary-test` builds
  one product boundary.
- `cargo test -p conary`, `-p conary-core`, `-p remi`, or `-p conaryd` runs the
  owning package tests.
- `cargo run -p conary-test -- list` validates integration manifests and suite
  inventory.
- `cargo fmt --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` are repository gates.

Verification means the reported command actually ran. Preserve exact failure
evidence, explain the causal leaf failure, and make a new head before rerunning
an unchanged failed gate. Prefer the focused proof first; add broader tests only
when the owner card's interaction boundary or the changed behavior requires it.

## Issue, Branch, And Pull-Request Workflow

Follow `CONTRIBUTING.md`. Non-trivial implementation, bug, refactor,
documentation, operations, and maintenance work uses one primary GitHub issue,
an issue-linked branch, and a pull request; never push repository changes
directly to `main`. Search existing issues first. Open substantial work as a
draft PR early, keep verification current there, resolve review conversations,
and merge only through the protected GitHub path.

Read-only scoping remains read-only until a change is requested. Security
reports use private advisories. `Closes #...` means the PR satisfies the
issue's acceptance criteria; otherwise use `Refs #...` and leave the larger
issue open. Preserve unrelated dirty work and avoid destructive Git commands in
shared worktrees.

## Product And Authority Contract

Conary is pre-alpha until a durable roadmap milestone says otherwise. Make
issue-backed hard cuts: replace the current schema or interface, state the
rebuild impact, and remove superseded migrations, adapters, routes, flags, and
compatibility paths in the same slice. Do not run old and new authorities in
parallel.

Cross-distribution package installation is the primary product path. The
source package format owns lifecycle ABI, dependencies, versions, payload, and
configuration semantics; Conary owns install, update, remove, rollback, and
generation publication; the target supplies typed capabilities. Runtime
conversion must not invoke the source package manager or its database.
Adoption/takeover is the sole migration-continuity exception. The canonical
contract is `docs/specs/foreign-package-lifecycle-contracts.md`.

Derive package behavior from pinned upstream documentation and source, then
encode it as typed grammars, state machines, and conformance tests. Heuristics,
regexes, substring matching, curated token lists, distro-name gates, and silent
defaults may aid diagnostics, redaction, discovery, or prioritization; they may
not establish compatibility, mutation, publication, security, or event
authority. A typed preflight failure is a defect to engineer, not a permanent
unsupported class or human-review queue.

Agent operations use versioned, typed, inspectable resources and plan/apply
results through `conary-agent-contract`; MCP adapts that contract and is never
a second authority. Do not weaken trust or approvals for agents or make an
essential operation available only through ad hoc shell or free-form output.

## Defect And Maintainability Discipline

- Fix a defect, duplicated authority, or half-implementation found in scope.
  File an exact-evidence issue when it belongs elsewhere; do not silently route
  around it.
- Fix causes and prove the contract or property, not only the observed input.
- Treat intermittent or unexplained failures as evidence of a defect, not as a
  reason to retry until green.
- A slice adding behavior to a Rust source file over 1,000 lines must include
  an ownership-based reorganization in the same issue or plan. Thin dispatch,
  registration, and re-export wiring may remain in a large hub.
- Before changing behavior in a Rust file over 1,500 lines, name the ownership
  boundary being preserved or improved. Files over 2,500 lines need a reviewed
  decomposition path before major feature work unless the fix is urgent.
- Refactors name what moves, its new owner, persisted/public impact, and the
  focused proof. Update the subsystem map or owning module doc when the
  look-here-first path changes.
- Meta-layer work is allowed only for factual drift, a touched path, or a
  failing gate and remains capped at one meta slice per four product slices
  until the first external tester milestone.

## Rust And CLI Conventions

Use standard Rust formatting and naming, four-space indentation, `thiserror`
for library errors, and `anyhow` at application boundaries. Keep modules
focused; each Rust file begins with its repo-relative path comment. Use short
imperative Conventional Commit subjects such as
`security(federation): pin https peer identity`.

For `apps/conary`, route user-facing status through `apps/conary/src/ui/`.
Never hand-roll status prefixes. `Status` renders the guarded lowercase ASCII
tags `[ok]`, `[fail]`, `[warn]`, `[skip]`, `[info]`, `[off]`, `[missing]`, and
`[pending]`; `apps/conary/tests/output_vocabulary_guard.rs` enforces them.
Internal `tracing` logs are not primary user output. Logging defaults to
`warn`; top-level `--verbose`, `--quiet`, and `RUST_LOG` retain their documented
precedence.

## Documentation And Safety

`AGENTS.md` is the concise repo-wide contract; `CONTRIBUTING.md` owns the full
contribution lifecycle; `docs/llms/README.md` routes assistants; feature cards
own exact paths and proof. Tool entrypoints stay thin and point back to these
owners. Add nested `AGENTS.md` only for genuinely different subtree rules.

Update canonical truth and YAML frontmatter when behavior changes. Run
`bash scripts/check-doc-truth.sh` plus the owning feature proof when changing a
public claim, command help, route, or agent surface. Remove completed or
superseded planning after its truth and resume facts move to canonical owners;
Git history is the archive.

Keep credentials, private paths, raw review artifacts, host-local state, and
personal notes out of tracked guidance and public evidence. Use ignored files
such as `docs/operations/LOCAL_ACCESS.md`. Do not weaken HTTPS fingerprint
pinning or other trust defaults casually; Remi or conaryd service changes run
their owning package tests.
