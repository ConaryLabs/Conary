# Conary Packaging & Publishing Toolchain — Design Spec

**Date:** 2026-06-10 (revised same day after external review — see Revision notes)
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

## Milestones (v1 is staged; ordering is mandatory)

Everything in this spec is v1 *design* scope, but implementation lands in gated
milestones — a later milestone does not start until the previous one's integration
suite is green:

- **M1 — the static-repo path.** `conary build` (recipe-driven + inference for the
  core build systems), basic `conary try` (no watch), `conary publish` to a static
  repo, `conary repo add` of a static repo, install from it. This is the
  smallest end-to-end loop that proves the product. The "first package in 5 minutes"
  tutorial must pass at the end of M1.
- **M2 — publish hardening + Remi push.** Hermetic-publish requirements (below),
  provenance classes and publish lint gates, Remi upload endpoint (which requires
  finishing Remi's TUF timestamp refresh — currently a 501 stub in
  `apps/remi/src/server/handlers/tuf.rs`).
- **M3 — differentiators.** Watch mode, agent-native diagnostics / MCP surface,
  record mode (prototype spike first; see Record mode).

Universal ingestion is split: directory/recipe/tarball/git in M1; foreign packages
(.rpm/.deb/.pkg.tar.zst) in M2 alongside the provenance-class gates they require.

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

Output: `./dist/<name>-<version>-<release>-<arch>.ccs`, path printed on success.
Architecture/platform identity is part of the artifact name and the repo index
identity from day one (no arch-less filenames to migrate later).

Every built package carries a **provenance class** in its manifest:
`native-built` (recipe or inference, hermetic), `inferred-source` (inference,
host build), `recorded-draft` (record mode output), or `foreign-converted`
(.rpm/.deb/.pkg.tar.zst conversion). Publish lint gates key off the class — e.g.
`recorded-draft` is never publishable directly (see Record mode).

### `conary try [pkg.ccs]`

Installs the freshly built package into a **throwaway generation**. This loop is the
centerpiece DX: no other packaging system can offer install-with-instant-total-undo.

Because it is the centerpiece, try gets an explicit **state machine**, not vibes:

- `conary try <pkg.ccs>` starts a try session: creates the throwaway generation,
  switches to it, records the session (previous generation id, started-at, package)
  in the database.
- `conary try status` shows the active session; `conary try rollback` ends it and
  restores the previous generation; `conary try keep` promotes it permanently.
- **At most one try session at a time** (v1); starting a second is an error that
  names the active one.
- **Crash/reboot safety:** the session record is durable. On boot or on the next
  conary invocation, an orphaned try session is reported and the user is prompted to
  rollback or keep — a crashed try never silently becomes the permanent system.
- Try requires the same privileges as `conary install`; service restarts triggered by
  the package's hooks apply within the session and revert with rollback.
- `--watch` (M3) composes build + try from the package project directory (it does not
  take a prebuilt `.ccs`): inotify on the source tree → incremental rebuild →
  hot-swap the throwaway generation.

### `conary publish <target>`

Signs and publishes. Targets:

- a static repo: local directory, `rsync://`/SSH, or S3-compatible bucket
- a Remi instance: authenticated upload (bearer token, v1-simple)

Publish **always rebuilds `--hermetic` first**. Host builds are for iterating, never
for shipping. Hermetic alone is not a trust claim, so the publish build additionally
requires (M2):

- **Pinned sources:** every source/patch must carry a checksum (the recipe format
  already has the field); inference-only builds get checksums recorded at first fetch.
- **Offline build:** network access inside the hermetic environment is limited to
  fetching the pinned, checksummed sources — nothing else.
- **Build-dependency lock:** the exact resolved versions of `requires`/`makedepends`
  used in the hermetic build are recorded in the package's provenance.
- **Attestation:** the build emits a provenance record (reusing the
  `DerivationExecutor` CAS capture) embedded in the manifest.
- **Divergence diagnostic:** if the hermetic output differs from the last host build
  of the same tree, publish says so and shows what changed — that delta is exactly
  the dishonest-dependency signal we're hunting.

First-ever publish auto-generates an Ed25519 keypair under `~/.config/conary/keys/`,
prints the fingerprint, and embeds the public key in repo metadata. There is no
separate keygen ceremony.

## One format, one internal representation

- The existing recipe TOML (`crates/conary-core/src/recipe/format.rs`) is the single
  human-facing format, **extended** to absorb the fields currently unique to
  `ccs.toml`: hooks, capability overrides, component classification rules.
- The CCS manifest becomes a **generated artifact**. Humans never write `ccs.toml` in
  the primary flow. Migration boundary: `ccs.toml` and the `conary ccs init/build`
  surface remain supported as the low-level manifest/debug layer until the recipe
  format expresses every install-time field; only then does the primary documentation
  drop them. The recipe is authoritative wherever both could apply.
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

## Differentiators (v1 design scope; land per the milestone ordering)

### Record mode — packaging by demonstration

`conary build --record` opens a shell; the user builds their software the way they
always do. Conary traces the session and **derives the recipe**: commands run, files
read (dependency evidence), files installed (manifest), plus suggested capability
declarations. The `capability/` landlock/seccomp infrastructure informs the approach,
but recording requires a tracing mechanism (seccomp-notify or fanotify) — this is the
riskiest technical bet in the design and gets a prototype spike before commitment.

Reliability bar: record mode output is **always a draft** — it emits a recipe marked
`recorded-draft` plus a trace report, and a recorded session is never directly
publishable. The draft must pass a normal (non-recorded) `conary build`, at which
point it is an ordinary recipe like any other. Derive build/install steps and the
file manifest dependably; dependency suggestions are advisory. Tracing must also
handle the unglamorous realities (secrets in environment, network fetches during the
session, generated files, root-requiring installs) — the prototype spike validates
these before the feature is committed.

### Agent-native diagnostics

Every build failure is a structured value:

```
Diagnostic { phase, code, message, evidence, suggestions: [{description, patch}] }
```

Rendered for humans as: what happened, why probably, and the exact next command or
recipe edit to try. Emitted as JSON under `--json`. Exposed through `conary-mcp` as
packaging tools (`build`, `diagnose`, `try`, `publish`) so an agent can drive the
entire package-fix-rebuild loop.

Diagnostics come in three honest tiers, because arbitrary build systems fail in
arbitrary ways:

1. **Structured Conary failures** — everything Conary itself detects (missing source,
   checksum mismatch, classification conflicts, publish gate failures): full
   `Diagnostic` with a concrete suggested fix.
2. **Known-pattern extraction** — recognized compiler/linker/build-system error
   shapes (missing header → candidate dependency suggestions, undefined reference →
   missing link dep, etc.): `Diagnostic` with ranked suggestions.
3. **Fallback capture** — anything else: the relevant log excerpt, the failing
   command, and generic next steps (`--explain`, rerun in `--hermetic`, open a recipe).

The rule stands in tiered form: a newcomer never sees a bare exit code with *no* next
step — but tier 3 promises a starting point, not a diagnosis.

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
  packages/<name>/<name>-<version>-<release>-<arch>.ccs
  chunks/<hash>           # optional CDC chunk store enabling delta fetch
```

This layout is normative only in outline. **A standalone static-repo spec in
`docs/specs/` is a mandatory gate before M1 implementation of publish/repo-add.** It
must define: `index.json` and all packages/chunks as TUF *targets* (the index sits
beside `metadata/` but is protected by it — that is the standard TUF pattern, not a
parallel trust path); atomic publish (write-new-then-flip, so a reader never sees a
torn repo); metadata expirations and refresh expectations; rollback/freeze protection;
and key rotation/revocation.

Client-side work:

- Lift the current `file://` rejection in the repository client.
- `conary repo add <url|path> --fingerprint <fp>` is the **documented happy path**:
  repo operators publish their key fingerprint out-of-band (website, README) and the
  add verifies it. Bare `repo add` without a fingerprint shows the fingerprint and
  requires an explicit trust-on-first-use confirmation; the key is pinned thereafter.
- Metadata trust via the existing TUF implementation; package signatures via Ed25519
  per the CCS format.

On publish, package and chunk files are append-only, but TUF snapshot/timestamp (and
the index target metadata) are **re-signed on every publish** — that is inherent to
TUF, and "incremental" means we never rewrite published artifacts, not that metadata
is static. Remi becomes *one producer* of this same format; its upload endpoint
ingests a `.ccs` into its store with bearer-token auth. Prerequisite work item: Remi's
TUF timestamp refresh is currently a 501 stub
(`apps/remi/src/server/handlers/tuf.rs`) and must be implemented in M2.

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
- The new commands ship gated until the integration suite is green, then graduate.
  The conary CLI currently has **no** cargo features (`default = []` in
  `apps/conary/Cargo.toml`), so the gate is introduced as part of this work: either a
  new `unstable-packaging` cargo feature or hidden clap subcommands — decided in the
  implementation plan.

## Deferred (roadmap)

- `conary why <path>` — surface CAS build provenance from any file back to recipe line
- Ecosystem ingestion refs (`crate:`, `pypi:`)
- Repo federation across static repos / Remi instances
- Delta-only publishing (chunk-level repo updates)
- Data-driven distro profiles (separate workstream; see gap analysis)

## Open questions (resolved defaults, revisit if they pinch)

- **Output dir:** `./dist/` (familiar; configurable later if it collides).
- **Trust model for repo add:** fingerprint verification (`--fingerprint`) is the
  documented happy path; explicit-confirm TOFU with pinning is the fallback. Rotation
  and revocation are defined in the static-repo child spec.
- **Remi push auth:** static bearer token in v1; token management UX deferred.

## Revision notes (2026-06-10)

Revised after external review (GPT). Adopted: mandatory milestone ordering within v1
(static-repo path first); hermetic-publish trust requirements (pinned sources, offline
build, dep lock, attestation, divergence diagnostic); static-repo child spec as a
hard pre-implementation gate with index-as-TUF-target and corrected
incremental-publish semantics; fingerprint-first trust for `repo add`; an explicit
try-session state machine with crash/reboot recovery; a ccs.toml migration boundary;
record-mode output demoted to never-directly-publishable drafts; provenance classes
with publish lint gates; arch in artifact and repo naming; tiered diagnostics; and a
corrected rollout gate (the previously-referenced `experimental` cargo feature does
not exist). Rejected: retreating from the four top-level verbs to a `conary package`
namespace — the claimed `publish` command collision does not exist in the current CLI,
and the cook overlap is already handled by demoting cook to plumbing.
