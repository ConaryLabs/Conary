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
| Inference model | Invisible defaults — no file written; `--explain` shows inferences; `conary new` materializes a pre-filled recipe when overrides are needed |
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

- **M0 — static-repo child spec (hard gate).** The standalone static-repo spec in
  `docs/specs/` is written, reviewed, and approved before any M1a implementation
  begins. No publish/repo-add code lands against an unapproved format.
- **M1a — recipe-only static path.** `conary build` for recipe-driven builds (no
  inference yet); `conary publish` to a static repo; `conary repo add` of a static
  repo; install from it. The smallest end-to-end loop that proves the format and the
  trust story. Packages published before M2 hardening carry an honest **hardening
  level** in provenance (`hermetic` but not `attested`) and publish prints that this
  is a preview repo, not reproducible release evidence; M2 flips the default gate to
  require `attested`.
- **M1b — inference + try.** Build-system inference for the core build systems,
  `conary new` materialization, `conary try` (state machine, no watch). The "first
  package in 5 minutes" tutorial must pass at the end of M1b — that is the M1 exit
  gate.
- **M2 — publish hardening + Remi push.** Hermetic-publish requirements (below),
  provenance classes and publish lint gates, foreign-package ingestion
  (.rpm/.deb/.pkg.tar.zst), Remi upload endpoint (which requires finishing Remi's TUF
  timestamp refresh — currently a 501 stub in `apps/remi/src/server/handlers/tuf.rs`).
- **M3 — differentiators.** Watch mode, agent-native diagnostics / MCP surface,
  record mode (prototype spike first; see Record mode).

Universal ingestion is split: directory/recipe in M1a, tarball/git in M1b, foreign
packages in M2 alongside the provenance-class gates they require.

### Command/flag availability by milestone

A verb or flag appears in `--help` only once its milestone lands — no aspirational
surface:

| Surface | M1a | M1b | M2 | M3 |
|---------|-----|-----|----|----|
| `build` (recipe), `--recipe`, `--hermetic`, `publish` (project form, static), `repo add` static | ✓ | | | |
| `build` (inference, tarball/git), `new`, `try`/`status`/`rollback`/`keep`, `--explain` | | ✓ | | |
| `build` (foreign pkgs), `publish` (artifact form, attestation-gated), publish lint gates, Remi push, hermetic-publish enforcement | | | ✓ | |
| `--record`, `--json`, `try --watch`, MCP packaging tools | | | | ✓ |

## The four verbs

### `conary new [name]`

One verb, two modes, both meaning "give me a recipe file":

- `conary new <name>` — scaffolds a package project for third-party software
  (distro-maintainer flow): a minimal `recipe.toml` and nothing else.
- `conary new --from .` (or bare `conary new` inside a source tree — same thing) —
  materializes the recipe that inference would use, pre-filled, for when invisible
  defaults need overriding. Help text and tutorials teach the explicit `--from .`
  form; the bare form is a convenience, not the documentation surface.

Upstream developers who never need overrides never run it — `conary build` alone
suffices. There is no separate `conary recipe init`; recipe materialization is not a
fifth concept.

### `conary build [TARGET]`

The universal front door. TARGET may be:

- nothing → current directory
- a directory or `recipe.toml` path
- a git URL or source tarball (fetch, then re-route)
- a foreign package: `.rpm`, `.deb`, `.pkg.tar.zst` (route through existing conversion
  machinery)

Routing: recipe present → recipe drives the build; bare source tree → build-system
inference (cargo, cmake, meson, autotools, npm, python, go); foreign package → convert.

Flags (milestone in brackets; a flag does not appear in `--help` before its
milestone — see the availability matrix):

- `--recipe <path>` [M1a] — explicit recipe selection
- `--hermetic` [M1a] — run in Kitchen isolation instead of on the host
- `--explain` [M1b] — print every inferred decision (build system, steps, deps,
  components)
- `--record` [M3] — packaging by demonstration (see Differentiators)
- `--json` [M3] — structured diagnostics output

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
- **Crash/reboot safety:** the session record is durable. The next conary invocation
  detects an orphaned try session and prompts to rollback or keep — a crashed try
  never silently becomes the permanent system. (Proactive boot-time notification — a
  systemd unit or login message — is later hardening, not M1; detection in M1 happens
  at next invocation.)
- **Rollback scope is generation/filesystem rollback, stated honestly:** it reverts
  generation-owned filesystem state (the package's files, links, units). It does not
  un-happen runtime side effects — services that ran, `/var` mutations, data the
  package wrote, external effects. Hook policy: try **refuses** packages that declare
  hooks with non-generation-scoped lifecycle effects (db migrations, irreversible
  state changes) unless the package declares reversible cleanup or the user passes
  `--allow-irreversible`. The policy **fails closed**: hooks that are unknown,
  unclassified, or legacy scriptlets are treated as non-generation-scoped — an
  incomplete declaration is never an escape hatch. "Try means safe" must be literally true by default; most
  CCS hooks are declarative (units/tmpfiles/sysctl) and generation-scoped, so the
  refusal bites rarely. Hook reversibility is a manifest field and a publish lint.
- Try requires the same privileges as `conary install`.
- `--watch` (M3) composes build + try from the package project directory (it does not
  take a prebuilt `.ccs`): inotify on the source tree → incremental rebuild →
  hot-swap the throwaway generation.

### `conary publish [WHAT] <target>`

Signs and publishes. Concrete forms (no ambiguity about what gets published):

- `conary publish <target>` — publishes the **current project** (a recipe is present
  or inference applies); triggers the hermetic rebuild. The default, and the only
  form where rebuild happens.
- `conary publish <pkg.ccs> <target>` — publishes an existing artifact. Gated on its
  provenance class: the artifact must carry a hermetic-build attestation (M2), else
  publish refuses and says to run the project form.
- Publishing "everything in dist/" is deliberately not a form; multi-package publish
  is one project at a time (CI loops over projects).

Targets:

- a static repo: local directory, `rsync://`/SSH, or S3-compatible bucket
- a Remi instance: authenticated upload (bearer token, v1-simple)

Publish **always rebuilds `--hermetic` first**. Host builds are for iterating, never
for shipping. Hermetic alone is not a trust claim, so the publish build additionally
requires (M2):

- **Pinned sources, fetch split from build:** publish *prefetches* all sources into
  the source cache, verifying identity per input kind — tarballs/patches by checksum
  (the recipe format already has the field; inference-only builds record checksums at
  first fetch), git inputs by commit hash, local directories by tree hash with a
  dirty-state policy (uncommitted changes → publish warns and records the tree hash
  as untracked-dirty; CI mode refuses). Tree-hash scope is defined, not implied: in a
  git repo, hash exactly the git-tracked files (ignored files, `.git/`, build outputs
  like `dist/` and `target/` are out — this also keeps secrets and generated
  artifacts out of provenance); outside a git repo, hash all files minus a default
  ignore set, with a warning that identity is weaker.
- **Offline build:** after prefetch, the hermetic build environment has **no network
  access at all**. Fetching happens before the sandbox, never inside it.
- **Build-dependency lock:** the exact resolved versions of `requires`/`makedepends`
  used in the hermetic build are recorded in the package's provenance, resolved
  against a named repo snapshot (not "whatever the resolver finds today"). The lock
  schema is defined in the M2 plan before publish hardening lands.
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
  input type. `conary new` (in a source tree) serializes the inferred recipe to disk,
  pre-filled, when the user needs to override something.
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
and the operator key lifecycle — rotation, revocation, backup, and loss recovery,
stated plainly, because the auto-generated first-publish key *is* the repo authority
and users must understand what losing it means.

Client-side work:

- Lift the current `file://` rejection in the repository client.
- `conary repo add <name> <url|path> --fingerprint <fp>` is the **documented happy
  path** (matching the existing `repo add <name> <url>` CLI shape): repo operators
  publish their key fingerprint out-of-band (website, README) and the add verifies
  it. Without `--fingerprint`, the add shows the fingerprint and requires an explicit
  trust-on-first-use confirmation; the key is pinned thereafter.
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
  beneath them, validated by the existing bootstrap and integration suites — child
  plans name the exact suites and result-gate commands rather than citing the matrix
  in aggregate.
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

**Round 4 (same day, final):** availability matrix distinguishes project-form publish
(M1a) from artifact-form publish (M2, attestation-gated); try hook policy fails
closed on unknown/unclassified/legacy-scriptlet hooks; static-repo child spec must
cover the operator key lifecycle including backup/loss recovery; rollout no longer
cites the integration matrix in aggregate — child plans name exact suites and
result-gate commands. Reviewer reported no critical findings; spec declared ready for
child-spec planning.

**Round 3 (same day):** static-repo child spec promoted to an explicit M0 hard gate;
per-flag milestone labels on the `build` surface and `--hermetic`/`--recipe` added to
the availability matrix; try now **refuses** irreversibly-hooked packages by default
(`--allow-irreversible` escape hatch; reversibility is a manifest field + publish
lint); tree-hash scope defined (git-tracked files only in a repo; default ignore set
plus warning outside one); pre-M2 publishes carry an honest hardening level
(`hermetic`, not `attested`) and announce preview status; `conary new --from .` added
as the explicit, documented form of bare `new`.

**Round 2 (same day):** try rollback scope stated honestly (generation/filesystem
only, with a hook policy for non-generation-scoped effects; boot-time orphan
notification deferred to hardening); hermetic publish split into
prefetch-then-offline with per-input-kind source identity (checksum / commit hash /
tree hash + dirty policy) and a repo-snapshot-resolved dependency lock; `publish`
argument forms made concrete; M1 split into M1a (recipe-only static path) and M1b
(inference + try, tutorial exit gate); command/flag availability matrix added so no
milestone ships aspirational `--help` surface; `repo add` aligned to the existing
`<name> <url>` shape; `conary recipe init` eliminated by folding recipe
materialization into `conary new` (rejected the suggested `build --init-recipe` — an
action flag that doesn't build is worse DX than reusing the existing verb).

**Round 1:** Revised after external review (GPT). Adopted: mandatory milestone ordering within v1
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
