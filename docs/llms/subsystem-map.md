---
last_updated: 2026-08-23
revision: 78
summary: Compact workspace orientation index that routes typed profile tiers, exact membership, detailed ownership, and proof through agent-context
---

# Assistant Subsystem Map

## Workspace Boundaries

| Area | Responsibility |
| --- | --- |
| `apps/conary/` | Package-manager CLI, command dispatch, local operations, and user output |
| `crates/conary-core/` | Package, repository, resolver, transaction, CCS, generation, and trust domain logic |
| `apps/remi/` | Package service, conversion, publication, admin API, MCP, and federation |
| `apps/conaryd/` | Local daemon authorization, job queue, REST routes, and SSE events |
| `apps/conary-test/` | Declarative integration runner, fixtures, result delivery, and QEMU proof |
| `crates/conary-bootstrap/` | Shared binary startup, tracing, runtime, and exit behavior |
| `crates/conary-agent-contract/` | Transport-neutral operation, resource, risk, and approval vocabulary |
| `crates/conary-mcp/` | Shared MCP adapter plumbing |

These eight Cargo packages are code-ownership boundaries. The four artifact
products are Conary, Remi, conaryd, and conary-test. One root workspace version,
reviewed suite commit, canonical `vMAJOR.MINOR.PATCH` tag, and GitHub release
own them; conaryd and conary-test are currently build-only products.

## Route Instead Of Preloading

This file is only a first orientation index. Resolve a real task through the
canonical feature-card bridge:

```bash
bash scripts/agent-context.sh --list
bash scripts/agent-context.sh --feature <slug>
bash scripts/agent-context.sh --path <file>
```

Do not read the complete feature-ownership map after a route is known. The
printed packet contains the exact start-here files, proof, neighbor gate, docs,
and safety invariant for that task.

## Capability Index

| Slug | Owns | Canonical background |
| --- | --- | --- |
| `database-state` | Current schema, persisted states, rebuilds, and fallible row decoding | `docs/ARCHITECTURE.md` |
| `dispatch` | CLI parsing, dispatch, command risk, and live-mutation labels | `docs/ARCHITECTURE.md` |
| `install` | Install, update, remove, rollback, scriptlets, and selected-root mutation | `docs/specs/foreign-package-lifecycle-contracts.md` |
| `adopt` | Adoption, takeover, unadoption, and native-authority handoff | `docs/modules/source-selection.md` |
| `model` | Declarative model diff, apply, lock, snapshot, and replatform planning | `docs/modules/source-selection.md` |
| `resolution` | Authenticated repositories, typed relations, providers, and SAT selection | `docs/modules/source-selection.md` |
| `generation` | Build, publication, activation, recovery, GC, and carrier export | `docs/ARCHITECTURE.md` |
| `ccs` | CCS authoring, conversion, verification, install, and native lifecycle ABI | `docs/modules/ccs.md` |
| `packaging` | Recipes, try sessions, static repositories, trust, and publish | `docs/modules/recipe.md` |
| `canonical-map` | Versioned cross-profile equivalence and Remi map exchange | `docs/modules/source-selection.md` |
| `profiles` | Typed support tiers, exact source membership, parser selection, and public Remi route slugs | `docs/modules/source-selection.md` |
| `remi` | Ingest, conversion, signing, serving, admin, fixtures, R2, and federation | `docs/modules/remi.md` |
| `conaryd` | Daemon authorization, package jobs, routes, and lifecycle events | `docs/modules/conaryd.md` |
| `bootstrap` | Prerequisites, images, self-hosting seed/run, and local QEMU validation | `docs/modules/bootstrap.md` |
| `release` | Suite version, build, signing, publication, deployment, and independent proof | `docs/operations/release-artifact-matrix.md` |
| `conary-test` | Suite manifests, execution, fixtures, result flow, and QEMU evidence | `docs/INTEGRATION-TESTING.md` |
| `agent-mcp` | Typed operation vocabulary and MCP adapters | `docs/operations/infrastructure.md` |

## Cross-Cutting Invariants

- Repository resolution is SAT-only and consumes authenticated, source-aware
  typed relations.
- Native parser output streams through one versioned authenticated snapshot
  sink while remaining lossless in its source ontology; explicit fallible
  projections serve resolution, CCS, and transaction consumers.
- Transaction and generation state stay coupled through one selected-root
  mutation lock, exact filesystem and SQLite authority, and publication debt.
- Component, provide, payload, configuration, and trust authority comes from
  typed package data, never path classification, text matching, or a distro
  name.
- Adoption preserves native ownership until explicit takeover or generation
  handoff; it is not the primary foreign-package installation path.
- Remi owns the live HTTP and MCP service. `conary-test` remains a local CLI and
  engine.
- Shared operation vocabulary belongs in `conary-agent-contract`; service and
  transport policy remain in their owning adapters.

The exact implementing files and proof commands evolve quickly and therefore
belong only to the feature cards and canonical subsystem docs, not this index.

## Deep Dives

- `docs/ARCHITECTURE.md` — workspace data flow and authority boundaries
- `docs/specs/source-package-authority.md` — lossless source models and projections
- `docs/specs/foreign-package-lifecycle-contracts.md` — source ABI and transaction order
- `docs/modules/test-fixtures.md` — fixture identity and publication proof
- `docs/operations/infrastructure.md` — MCP, deploy, release, and host workflow
- `docs/roadmaps/development-roadmap.md` — current maturity, ordering, and blockers

## Drift Rule

When a path, owner, proof command, or interaction gate moves, update
`docs/modules/feature-ownership.md` first. Update this map only when a workspace
boundary, capability summary, canonical deep dive, or cross-cutting invariant
changes.
