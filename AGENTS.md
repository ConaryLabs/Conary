# Repository Guidelines

## Project Structure & Module Organization
Conary is a virtual Rust workspace. The package-manager CLI lives in `apps/conary/src/` (`commands/`, `cli/`, `main.rs`). Core package-management logic is in `crates/conary-core/src/`. Remi lives in `apps/remi/src/` (`server/`, `federation/`, `bin/remi.rs`), and conaryd lives in `apps/conaryd/src/` (`daemon/`, `bin/conaryd.rs`). Test helpers and integration coverage live in `apps/conary/tests/`, `crates/conary-core/tests/`, and `apps/conary-test/src/`. Packaging assets are under `packaging/` and `deploy/`; design notes, plans, and reviews are in `docs/`.

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

## Coding Style, Safety, and Commits
Use standard Rust formatting (`cargo fmt`) and keep Clippy clean. Indentation is 4 spaces. Follow Rust naming conventions: `snake_case` for functions/modules, `CamelCase` for types, `SCREAMING_SNAKE_CASE` for constants. Keep modules focused by subsystem. This repository expects each Rust source file to begin with a path comment such as `// conary-core/src/...`.

Recent history uses conventional-style prefixes such as `fix:`, `security:`, and `docs:`. Keep commit subjects short and imperative, e.g. `security(federation): pin https peer identity`. PRs should explain the problem, summarize the fix, list verification commands run, and link the relevant issue/plan entry. Include logs or API examples when behavior changes are not obvious from the diff.

## Maintainability & Refactor Discipline
Treat large files as review signals, not automatic failures. When a change adds
substantial behavior to a Rust file over 1000 lines, or adds or changes
behavior in a Rust file over 1500 lines, name the ownership boundary you are
preserving or improving before editing. Files over 2500 lines should get a
reviewed decomposition path before major feature work unless the task is an
urgent fix.

Refactor and pruning slices must say which behavior moves, which module owns it
afterward, which persisted state or public surface is affected, and which
focused test proves behavior stayed the same or changed intentionally. Do not
split files mechanically, keep command and route handlers thin, and update
`docs/llms/subsystem-map.md` or the relevant `docs/modules/*.md` file when the
"look here first" path changes.

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
- The documentation accuracy ledger tracks per-file doc audit coverage; the feature coherency ledger tracks per-claim implementation truth. Before editing a public claim, command help, route, or agent-facing surface, grep `docs/superpowers/feature-coherency-ledger.tsv` for the touched path and rerun the coherency checks when rows point at it.
- Add nested `AGENTS.md` files only when a subtree genuinely needs durable instructions that differ from the repo root.
- Keep host-local, credential-bearing, or personal notes in ignored local files such as `docs/operations/LOCAL_ACCESS.md`, not in tracked assistant guidance.

Keep this file map-like. If a detail changes often or needs more than a short paragraph to explain, move it into a linked canonical doc instead of expanding this file.

## Security & Contributor Notes
Do not weaken trust defaults casually. HTTPS federation peers should use pinned fingerprints, and service changes should be verified with `cargo test -p remi` and `cargo test -p conaryd`. Avoid destructive Git commands in shared worktrees, and do not add schema migrations unless the task explicitly calls for one.
Historical review prompts and finished design docs belong under archive subdirectories, not in the active doc tree.
