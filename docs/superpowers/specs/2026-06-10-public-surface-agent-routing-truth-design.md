# Public Surface And Agent Routing Truth Design

## Status

Ready for sequenced implementation via
`docs/superpowers/plans/2026-06-10-public-surface-agent-routing-truth-plan.md`.

## Goal

Align public and assistant-facing maps with the code that actually owns the
behavior, so humans and LLM agents edit the right files and do not trust stale
manual surfaces.

## Background

External reviews found drift across several surfaces:

- `docs/modules/query.md` maps top-level `conary sbom` to the nested query SBOM
  module, while dispatch routes top-level derivation SBOM through
  `apps/conary/src/commands/derivation_sbom.rs`.
- Remi admin OpenAPI claims to cover test-harness operations but omits many
  registered admin paths.
- External review treated root `man/conary.1` as tracked, but local inspection
  shows root and app manpage outputs are ignored/generated inspection artifacts.
  Docs should not treat ignored generated output as durable source truth.
- Agent routing docs omit or understate several load-bearing command files,
  including install submodules and the `system adopt` routing path.
- conaryd currently depends on the CLI crate for package job execution, but the
  module docs do not make that dependency obvious.

## Policy Decision

Active docs should route by real ownership rather than by old command names:

- Generated surfaces are either generated during verification or removed from
  tracked manual docs.
- API specs should either include registered routes or narrow their description.
- Assistant-facing maps should point to directories plus key files when a
  subsystem is too broad for a small file list.
- Intentional asymmetry between CLI, HTTP, and MCP surfaces must be named.
- Retained technical debt such as duplicated helpers or inverted layering must
  have a visible owner and follow-up.

## Scope

This track owns docs, generated/manual public surface checks, OpenAPI route
truth, SBOM routing docs, agent routing maps, conaryd layering documentation,
and small comments for naming oddities. It is intentionally a sequenced track,
not a single `/goal`: execute one slice at a time and stop after each slice has
focused verification and a commit boundary. Avoid broad code refactors unless a
verifier requires a tiny helper extraction.

## Implementation Shape

1. Verify generated manpage policy: ignored/generated manpages may be inspected
   during a slice, but they are not tracked truth unless the repo intentionally
   starts tracking them.
2. Add or update a manpage truth check only if a tracked manpage is introduced.
3. Diff Remi admin routes against OpenAPI paths and repair either the spec or
   the description.
4. Correct SBOM routing docs and any feature ownership entries that repeat the
   wrong file mapping.
5. Document the conaryd package-job dependency on CLI command functions.
6. Refresh subsystem-map and feature-ownership for install, system, adoption,
   provenance, state, and conary-test MCP/CLI asymmetry.
7. Add narrow comments for dispatch naming oddities where renaming is not worth
   the risk.
8. Verify legacy replay duplication and the `experimental` feature flag before
   choosing remove, hoist, or document.

## Verification Strategy

Required gates:

- `cargo build -p conary` when manpage behavior is touched
- `cargo test -p remi openapi`
- `cargo test -p conary-test server::mcp`
- `cargo test -p conary --lib`
- `bash scripts/check-doc-truth.sh`
- docs-audit inventory and ledger checks
- `git diff --check`

## Non-Goals

- Do not redesign Remi admin auth or MCP behavior.
- Do not extract conaryd package execution out of the CLI crate in this track.
- Do not perform a broad command-module decomposition.
