# Public Surface And Agent Routing Truth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Repair stale public-surface and assistant-routing claims so agents can find the right code owners without guessing.

**Architecture:** Start with inventory and comparisons, then make small docs or validator repairs. Avoid broad code movement; this track is about truth and routing, not decomposition. This is a sequenced multi-slice track, not one `/goal`; finish and verify one slice before starting the next.

**Tech Stack:** Rust, Clap manpage generation, Axum route inspection, docs-audit, docs-truth.

---

## Design Source

- `docs/superpowers/specs/2026-06-10-public-surface-agent-routing-truth-design.md`

## File Map

| Path | Purpose |
| --- | --- |
| `apps/conary/build.rs` | Existing generated manpage path. |
| `apps/conary/man/conary.1` | Ignored/generated manpage inspection output after `cargo build -p conary`; do not stage it. |
| `docs/modules/query.md` | Correct top-level and system SBOM routing. |
| `apps/conary/src/dispatch/root.rs` | Evidence for top-level command routing. |
| `apps/conary/src/commands/derivation_sbom.rs` | Top-level derivation SBOM implementation. |
| `apps/conary/src/commands/query/sbom.rs` | Nested/system DB SBOM implementation. |
| `apps/remi/src/server/routes/admin.rs` | Remi admin route source. |
| `apps/remi/src/server/handlers/openapi.rs` | Remi OpenAPI spec. |
| `docs/llms/subsystem-map.md` | Assistant routing map. |
| `docs/modules/feature-ownership.md` | Capability ownership cards. |
| `docs/modules/conaryd.md` | conaryd layering and daemon package-job docs. |
| `apps/conaryd/src/daemon/package_ops.rs` | Evidence for daemon dependency on CLI command functions. |
| `apps/conary/Cargo.toml` | `experimental` feature flag evidence. |

## Task 0: Baseline

- [ ] Run:

```bash
cargo build -p conary
cargo test -p remi openapi
cargo test -p conary-test server::mcp
bash scripts/check-doc-truth.sh
```

Expected: pass before edits or record existing failures before changing files.

## Slice 4a: Generated Manpage And SBOM Routing

This is the first executable `/goal` slice for Track 4. Later slices should not
start until this slice is verified and committed.

## Task 1: Verify Generated Manpage Policy

- [ ] Confirm no manpage output is currently tracked, then generate and inspect the ignored app manpage:

```bash
cargo build -p conary
if git ls-files --error-unmatch man/conary.1 >/dev/null 2>&1; then
  echo "tracked root manpage exists; decide delete or add a generated comparison before continuing" >&2
  exit 1
fi
if git ls-files --error-unmatch apps/conary/man/conary.1 >/dev/null 2>&1; then
  echo "tracked app manpage exists; decide delete or add a generated comparison before continuing" >&2
  exit 1
fi
test -f apps/conary/man/conary.1
rg -n "Daily workflow examples|conary system adopt --refresh" apps/conary/man/conary.1
```

- [ ] If any docs claim `man/conary.1` is tracked source truth, correct them to say manpages are generated/ignored inspection output.
- [ ] Do not stage `man/conary.1` or `apps/conary/man/conary.1` unless the repository intentionally changes generated-manpage policy in this slice.

## Task 2: Repair SBOM Routing Docs

- [ ] In `docs/modules/query.md`, change the top-level `conary sbom` route to point at `apps/conary/src/commands/derivation_sbom.rs`.
- [ ] Ensure nested installed-package SBOM behavior points at `apps/conary/src/commands/query/sbom.rs`.
- [ ] Run:

```bash
cargo run -p conary -- sbom --help
cargo run -p conary -- system sbom --help
rg -n "commands/query/sbom|derivation_sbom|conary sbom" docs/modules/query.md docs/llms/subsystem-map.md docs/modules/feature-ownership.md
```

- [ ] If `conary system sbom --help` fails, record that as the current command reality and fix docs so they do not claim a live nested `system sbom` surface.

## Task 3: Align Remi OpenAPI With Routes

Start this as a separate slice after Slice 4a is committed.

- [ ] Extract current admin routes from `apps/remi/src/server/routes/admin.rs`.
- [ ] Compare them to `paths` in `apps/remi/src/server/handlers/openapi.rs`.
- [ ] Either add missing live paths to OpenAPI or narrow the `info.description` so it does not claim full test-harness operation coverage.
- [ ] Add a focused test in `apps/remi/src/server/handlers/openapi.rs` that checks every route family intentionally covered by the spec.
- [ ] Run `cargo test -p remi openapi`.

## Task 4: Document conaryd Package-Job Layering

Start this as a separate slice after the OpenAPI slice is committed unless the
same doc edit naturally touches the boundary.

- [ ] In `docs/modules/conaryd.md`, add a concise note that daemon package jobs currently call CLI command functions through `apps/conaryd/src/daemon/package_ops.rs`.
- [ ] In `docs/llms/subsystem-map.md`, route daemon package-job changes through both `apps/conaryd/src/daemon/package_ops.rs` and the relevant CLI command owner.
- [ ] Run:

```bash
rg -n "package_ops|conary::commands|package jobs" docs/modules/conaryd.md docs/llms/subsystem-map.md apps/conaryd/src/daemon/package_ops.rs
```

Expected: docs and code point to the same boundary.

## Task 5: Refresh Assistant Routing Maps

- [ ] In `docs/llms/subsystem-map.md` and `docs/modules/feature-ownership.md`, add missing load-bearing install files or route the card to `apps/conary/src/commands/install/` with named key files.
- [ ] Foreground adoption routing as:

```text
apps/conary/src/cli/system.rs -> apps/conary/src/dispatch/system.rs -> apps/conary/src/commands/adopt/
```

- [ ] Add `apps/conary/src/commands/system.rs`, `apps/conary/src/commands/state.rs`, `apps/conary/src/commands/provenance.rs`, and `apps/conary/src/commands/live_root.rs` only where the ownership cards need them.
- [ ] Keep the map compact; do not turn it into a full file listing.

## Task 6: Document CLI/MCP Asymmetry

Start this as a separate slice after the assistant-routing slice is committed.

- [ ] Compare `apps/conary-test/src/server/mcp.rs` tool names with `apps/conary-test/src/cli.rs` subcommands.
- [ ] If MCP-only tools remain intentional, document that in `apps/conary-test/README.md` or `docs/llms/subsystem-map.md`.
- [ ] If any doc claims exact mirroring, replace it with partial-overlap wording.
- [ ] Run `cargo test -p conary-test server::mcp`.

## Task 7: Verify Small Routing Oddities And Cleanup Candidates

Treat these as cleanup candidates after the higher-priority routing slices. Do
not expand the first Track 4 `/goal` to include helper deduplication unless the
earlier slices are already committed and the diff is still small.

- [ ] Add a short comment or docs note for `cli/verify.rs` routing through `dispatch/verify_derivation.rs` if current docs leave that unclear.
- [ ] Verify duplicated legacy replay helpers:

```bash
rg -n "legacy_replay_refusal|legacy replay" apps/conary/src/commands/install apps/conary/src/commands/remove apps/conary/src/commands/system.rs
```

- [ ] If duplication is mechanical and small, hoist it in this track only with focused tests. If not, record a follow-up row in the umbrella.
- [ ] Verify `experimental` feature usage:

```bash
rg -n "experimental" apps/conary/Cargo.toml crates/ docs/
```

- [ ] Remove or document the feature flag based on actual usage.

## Task 8: Final Verification And Commit

- [ ] Run:

```bash
cargo build -p conary
cargo test -p remi openapi
cargo test -p conary-test server::mcp
cargo test -p conary --lib
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
```

- [ ] Review `git diff --name-only`, then stage only the exact files changed by
  the completed Track 4 slice.
- [ ] Commit:

```bash
git commit -m "docs: align public surface routing"
```

Only stage paths that changed. Do not stage ignored generated manpage outputs.
