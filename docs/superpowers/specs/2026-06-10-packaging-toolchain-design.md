# Conary Packaging & Publishing Toolchain — Design Spec

**Date:** 2026-06-10
**Status:** Approved design, pre-implementation
**Companion analysis:** `docs/superpowers/distro-adoption-gap-analysis-2026-06-10.md`

## North star

Packaging for Conary must be a pleasure. Convoluted and hard to follow is disqualifying.
Concretely: one file format a human ever sees, one front-door command, and the fastest
build-test-undo loop of any packaging system. The acceptance test for the whole design
is the tutorial: **"Package your first software in 5 minutes"** must fit on one screen.

## Decisions made (with rationale)

| Decision | Choice |
|----------|--------|
| Scope | Build toolchain + publishing designed together, end to end |
| Primary persona | Both upstream devs and distro maintainers via one flow: inference for source trees, recipe file as the explicit escalation path |
| Inference model | Invisible defaults — no file written; `--explain` shows inferences; `conary recipe init` materializes a pre-filled recipe when overrides are needed |
| Build environment | Host by default (fast iteration); hermetic via `--hermetic` and always for publish |
| Publish targets (v1) | Static repo (dir / rsync / S3-compatible) **and** authenticated push to Remi |
| Architecture | "Approach 1+": unify around the existing recipe format; spend innovation budget on differentiators (record mode, agent-native diagnostics, watch mode, universal ingestion), all in v1 scope |

Rejected alternatives: a thin orchestrator over the existing `cook` + `ccs build`
commands (leaves two user-facing formats — exactly the convolution we're avoiding), and
a clean-slate format/tool (the differentiators live in the verbs and engine, not file
syntax; a new format discards a proven one for no DX gain).

## The four verbs

### `conary new <name>`

Scaffolds a package project for third-party software (distro-maintainer flow): a
minimal `recipe.toml` and nothing else. Upstream developers in their own source tree
never need it — they run `conary build` directly.

### `conary build [TARGET]`

The universal front door. TARGET may be:

- nothing → current directory
- a directory or `recipe.toml` path
- a git URL or source tarball (fetch, then re-route)
- a foreign package: `.rpm`, `.deb`, `.pkg.tar.zst` (route through existing conversion
  machinery)

Routing: recipe present → recipe drives the build; bare source tree → build-system
inference (cargo, cmake, meson, autotools, npm, python, go); foreign package → convert.

Flags:

- `--explain` — print every inferred decision (build system, steps, deps, components)
- `--hermetic` — run in Kitchen isolation instead of on the host
- `--record` — packaging by demonstration (see Differentiators)
- `--json` — structured diagnostics output
- `--recipe <path>` — explicit recipe selection

Output: `./dist/<name>-<version>-<release>.ccs`, path printed on success.

### `conary try [pkg.ccs]`

Installs the freshly built package into a **throwaway generation**. Exiting try (or
`conary try --rollback`) restores the previous generation instantly. `--watch` rebuilds
on source change and hot-swaps the throwaway generation — a dev-server feedback loop
for system packages. `--keep` promotes the generation permanently. This loop is the
centerpiece DX: no other packaging system can offer install-with-instant-total-undo.

### `conary publish <target>`

Signs and publishes. Targets:

- a static repo: local directory, `rsync://`/SSH, or S3-compatible bucket
- a Remi instance: authenticated upload (bearer token, v1-simple)

Publish **always rebuilds `--hermetic` first**. Host builds are for iterating, never
for shipping; this keeps dependency declarations honest without taxing the inner loop.

First-ever publish auto-generates an Ed25519 keypair under `~/.config/conary/keys/`,
prints the fingerprint, and embeds the public key in repo metadata. There is no
separate keygen ceremony.

## One format, one internal representation

- The existing recipe TOML (`crates/conary-core/src/recipe/format.rs`) is the single
  human-facing format, **extended** to absorb the fields currently unique to
  `ccs.toml`: hooks, capability overrides, component classification rules.
- The CCS manifest becomes a **generated artifact**. Humans never write `ccs.toml`.
- Inference produces a synthetic in-memory `Recipe`, so the engine has exactly one
  input type. `conary recipe init` serializes the inferred recipe to disk, pre-filled,
  when the user needs to override something.
- `conary cook` and `conary ccs build` remain as plumbing commands but leave the
  primary documentation.
- The bootstrap pipeline becomes a consumer of the same build pipeline — conaryOS
  dogfoods the toolchain third parties use.

## Engine: `BuildPipeline`

New module in conary-core:

```
resolve input → plan (recipe or inference) → execute via Kitchen (host | hermetic)
  → capture DESTDIR → classify components → generate CCS manifest → CcsBuilder
  → sign (if key configured) → .ccs
```

Kitchen prerequisites: isolation is already a flag (`use_isolation`); the
bootstrap-specific residue (seed EROFS mounting, stage markers) must be extracted so
Kitchen runs cleanly outside bootstrap context. `DerivationExecutor`'s CAS output
capture is reused for provenance.

Inference engine: one detector per build system, each emitting a synthetic `Recipe`.
Detectors are ranked; ambiguity (e.g. both `Cargo.toml` and `Makefile`) is reported
with `--explain` and resolved by an explicit recipe.

## Differentiators (all v1)

### Record mode — packaging by demonstration

`conary build --record` opens a shell; the user builds their software the way they
always do. Conary traces the session and **derives the recipe**: commands run, files
read (dependency evidence), files installed (manifest), plus suggested capability
declarations. The `capability/` landlock/seccomp infrastructure informs the approach,
but recording requires a tracing mechanism (seccomp-notify or fanotify) — this is the
riskiest technical bet in the design and gets a prototype spike before commitment.

V1 reliability bar: derive build/install steps and the file manifest dependably;
dependency suggestions may be advisory.

### Agent-native diagnostics

Every build failure is a structured value:

```
Diagnostic { phase, code, message, evidence, suggestions: [{description, patch}] }
```

Rendered for humans as: what happened, why probably, and the exact next command or
recipe edit to try. Emitted as JSON under `--json`. Exposed through `conary-mcp` as
packaging tools (`build`, `diagnose`, `try`, `publish`) so an agent can drive the
entire package-fix-rebuild loop. Rule: a newcomer never sees a bare compiler dump with
no next step.

### Watch mode

`conary try --watch` runs from the package project directory (it composes build + try,
rather than taking a prebuilt `.ccs`): inotify on the source tree → incremental rebuild
→ hot-swap the throwaway generation. Small lift over `try`; outsized demo and
daily-use value.

### Universal ingestion

`conary build <git-url | tarball | .deb | .rpm | .pkg.tar.zst>` — one verb, always
ends in a `.ccs`. Ecosystem refs (`crate:foo`, `pypi:bar`) are roadmap, not v1.

## Static repo format (new spec, to live in `docs/specs/`)

```
repo/
  conary-repo.toml        # repo identity: name, description, pubkey fingerprints
  metadata/               # TUF root / snapshot / timestamp / targets (reuses trust/)
  index.json              # package index matching the client RepositoryMetadata shape
  packages/<name>/<name>-<version>-<release>.ccs
  chunks/<hash>           # optional CDC chunk store enabling delta fetch
```

Client-side work:

- Lift the current `file://` rejection in the repository client.
- `conary repo add <url|path>` detects a static repo by the presence of
  `conary-repo.toml` and runs a TOFU fingerprint prompt (fingerprint shown; user
  confirms; pinned thereafter).
- Metadata trust via the existing TUF implementation; package signatures via Ed25519
  per the CCS format.

`conary publish` maintains `index.json` and TUF metadata incrementally — publishing one
package does not rewrite the repo. Remi becomes *one producer* of this same format;
its upload endpoint ingests a `.ccs` into its store with bearer-token auth.

## Testing

- **Unit:** inference detectors against fixture source trees (one per build system,
  plus ambiguous-tree cases); recipe parsing/serialization golden tests; static repo
  index round-trip.
- **Integration (conary-test, new suite):** build → try → rollback → publish →
  `repo add` → install-from-static-repo, with a plain nginx container serving the
  repo, across fedora44 / ubuntu-26.04 / arch. Record-mode smoke test on a simple
  autotools package. Remi push test against the existing test deployment.
- **DX acceptance:** the "first package in 5 minutes" tutorial is written first and
  kept passing — if a step grows, the design regressed.

## Rollout and compatibility

- Bootstrap recipes continue to work unchanged; Kitchen extraction is refactoring
  beneath them, validated by the existing 333-test integration matrix.
- `conary cook` / `conary ccs build` are not removed in v1; they are demoted in docs.
- The new commands land behind the existing `experimental` feature flag until the
  integration suite is green, then graduate.

## Deferred (roadmap)

- `conary why <path>` — surface CAS build provenance from any file back to recipe line
- Ecosystem ingestion refs (`crate:`, `pypi:`)
- Repo federation across static repos / Remi instances
- Delta-only publishing (chunk-level repo updates)
- Data-driven distro profiles (separate workstream; see gap analysis)

## Open questions (resolved defaults, revisit if they pinch)

- **Output dir:** `./dist/` (familiar; configurable later if it collides).
- **Trust model for repo add:** TOFU with pinned fingerprint (upgrade path: publish
  fingerprint out-of-band, `--fingerprint` flag for verification at add time).
- **Remi push auth:** static bearer token in v1; token management UX deferred.
