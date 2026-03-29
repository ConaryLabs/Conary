# Repository Guidelines

## Project Structure & Module Organization
Conary is a Rust workspace. The CLI lives in `src/` (`src/commands/`, `src/cli/`, `src/bin/`). Core package-management logic is in `conary-core/src/`, and server/daemon code is in `conary-server/src/`. Test helpers and integration coverage live in `tests/`, `conary-core/tests/`, and `conary-test/src/`. Packaging assets are under `packaging/` and `deploy/`; design notes, plans, and reviews are in `docs/`.

## Build, Test, and Development Commands
- `cargo build`: build the default CLI workspace.
- `cargo build --features server`: build CLI plus server-backed paths.
- `cargo test`: run the default workspace test suite.
- `cargo test --features server`: run tests that include server code.
- `cargo test -p conary-core` or `cargo test -p conary-server`: target a single crate.
- `cargo clippy -- -D warnings`: enforce zero-warning linting.
- `cargo clippy --features server -- -D warnings`: lint server-enabled builds.

## Coding Style & Naming Conventions
Use standard Rust formatting (`cargo fmt`) and keep Clippy clean. Indentation is 4 spaces. Follow Rust naming conventions: `snake_case` for functions/modules, `CamelCase` for types, `SCREAMING_SNAKE_CASE` for constants. Keep modules focused by subsystem. This repository expects each Rust source file to begin with a path comment such as `// conary-core/src/...`.

## Testing Guidelines
Prefer small unit tests near the code they cover and integration tests in `tests/` for end-to-end flows. Name tests descriptively, for example `test_prepare_discovered_peer_rejects_https_without_pinned_fingerprint`. When touching server code, always rerun with `--features server`. Security and transaction changes should include regression coverage.

## Commit & Pull Request Guidelines
Recent history uses conventional-style prefixes such as `fix:`, `security:`, and `docs:`. Keep commit subjects short and imperative, e.g. `security(federation): pin https peer identity`. PRs should explain the problem, summarize the fix, list verification commands run, and link the relevant issue/plan entry. Include logs or API examples when behavior changes are not obvious from the diff.

## Security & Contributor Notes
Do not weaken trust defaults casually. HTTPS federation peers should use pinned fingerprints, and server paths usually require `--features server` during verification. Avoid destructive Git commands in shared worktrees, and do not add schema migrations unless the task explicitly calls for one.
Historical review prompts and finished design docs belong under archive subdirectories, not in the active doc tree.
