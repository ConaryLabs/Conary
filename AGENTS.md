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

## Coding Style, Safety, and Commits
Use standard Rust formatting (`cargo fmt`) and keep Clippy clean. Indentation is 4 spaces. Follow Rust naming conventions: `snake_case` for functions/modules, `CamelCase` for types, `SCREAMING_SNAKE_CASE` for constants. Keep modules focused by subsystem. This repository expects each Rust source file to begin with a path comment such as `// conary-core/src/...`.

Recent history uses conventional-style prefixes such as `fix:`, `security:`, and `docs:`. Keep commit subjects short and imperative, e.g. `security(federation): pin https peer identity`. PRs should explain the problem, summarize the fix, list verification commands run, and link the relevant issue/plan entry. Include logs or API examples when behavior changes are not obvious from the diff.

## Testing and Documentation Guidance
Prefer small unit tests near the code they cover and integration tests in `apps/conary/tests/` for end-to-end CLI flows. Name tests descriptively, for example `test_prepare_discovered_peer_rejects_https_without_pinned_fingerprint`. When touching service code, rerun the owning packages directly with `cargo test -p remi` and `cargo test -p conaryd`. Security and transaction changes should include regression coverage.

Start assistant-facing work with:

- `AGENTS.md` for the repo contract and verification expectations
- `docs/llms/README.md` for the vendor-neutral assistant map
- `docs/ARCHITECTURE.md` and `docs/modules/*.md` for subsystem background
- `docs/INTEGRATION-TESTING.md` when validation spans `conary-test`
- `docs/operations/infrastructure.md` for MCP, deploy, and host workflow notes

Keep this file map-like. If a detail changes often or needs more than a short paragraph to explain, move it into a linked canonical doc instead of expanding this file.

## Security & Contributor Notes
Do not weaken trust defaults casually. HTTPS federation peers should use pinned fingerprints, and service changes should be verified with `cargo test -p remi` and `cargo test -p conaryd`. Avoid destructive Git commands in shared worktrees, and do not add schema migrations unless the task explicitly calls for one.
Historical review prompts and finished design docs belong under archive subdirectories, not in the active doc tree.
