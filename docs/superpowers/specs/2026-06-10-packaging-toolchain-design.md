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
| Build environment | Host by default (fast iteration); isolated via `--isolated` and always for publish. The flag is stable across milestones — it requests the strongest isolation available (M1a: sandboxed; M2+: hermetic); the provenance hardening field, not the flag, carries the truth claim |
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
  Deliverable: `docs/specs/static-repo-format-v1.md` (drafted; gate opens on
  review approval).
- **M1a — recipe-only static path.** `conary cook` for recipe-driven builds (no
  inference yet); `conary publish` to a static repo; `conary repo add` of a static
  repo; install from it. The smallest end-to-end loop that proves the format and the
  trust story. Packages published before M2 hardening carry an honest **hardening
  level** in provenance (`sandboxed` — Kitchen container isolation with network
  still allowed; the `hermetic` label is reserved for M2's offline builds) and
  publish prints that this is a preview repo, not reproducible release evidence; M2
  flips the default gate to require `attested`.
- **M1b — inference + try.** Build-system inference for the core build systems,
  `conary new` materialization, `conary try` (state machine, no watch). Internal
  ordering: inference lands first via the `conary new --from .` path (materialize →
  inspect → build with explicit recipe), then bare inference-mode `conary cook` —
  so the two surfaces share one tested engine and cannot diverge. The "first package
  in 5 minutes" tutorial must pass at the end of M1b — that is the M1 exit gate.
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
| `cook` (recipe), `--recipe`, `--isolated`, `publish` (project form, static), `repo add` static | ✓ | | | |
| `cook` (inference, tarball/git), `new`, `try`/`status`/`rollback`/`keep`, `--explain` | | ✓ | | |
| `cook` (foreign pkgs), `publish` (artifact form, attestation-gated), publish lint gates, Remi push, hermetic-publish enforcement | | | ✓ | |
| `--record`, `--json`, `try --watch`, MCP packaging tools | | | | ✓ |

## The four verbs

The journey is `new → cook → try → publish` — recipe, kitchen, cook: the vocabulary
is a deliberate nod to the original Conary, and it is the *only* documented path.
`conary build` exists as a **hidden compatibility alias** for `cook` (muscle memory
from cargo/go/npm, and agents that reflexively type it) — it works, help points it
at `cook`, and no tutorial or doc ever uses it. One visible path; the alias is a
courtesy, not a fifth concept. (`BuildPipeline` stays as the internal engine name.)

### `conary new [name]`

One verb, two modes, both meaning "give me a recipe file":

- `conary new <name>` — scaffolds a package project for third-party software
  (distro-maintainer flow): a minimal `recipe.toml` and nothing else.
- `conary new --from .` (or bare `conary new` inside a source tree — same thing) —
  materializes the recipe that inference would use, pre-filled, for when invisible
  defaults need overriding. Help text and tutorials teach the explicit `--from .`
  form; the bare form is a convenience, not the documentation surface.

Upstream developers who never need overrides never run it — `conary cook` alone
suffices. There is no separate `conary recipe init`; recipe materialization is not a
fifth concept.

### `conary cook [TARGET]`

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
- `--isolated` [M1a] — run in Kitchen isolation instead of on the host. The flag
  name is deliberately not `--hermetic`: it requests the strongest isolation the
  current milestone provides (M1a: container sandbox, network allowed; M2+: offline
  hermetic), and the provenance hardening field records which one you actually got
- `--explain` [M1b] — print every inferred decision (build system, steps, deps,
  components)
- `--record` [M3] — packaging by demonstration (see Differentiators)
- `--json` [M3] — structured diagnostics output

Output: `./dist/<name>-<version>-<release>-<arch>.ccs`, path printed on success.
Architecture/platform identity is part of the artifact name and the repo index
identity from day one (no arch-less filenames to migrate later). Arch is determined
as: host architecture by default (`uname -m`), overridable via a `[package]` `arch`
field in the recipe; cross builds take it from the existing `[cross].target`.

`--explain` output is human-readable but backed by a structured `InferenceTrace`
type (each node: detector, confidence, decision, evidence) so it is testable and
serializable — not debug-printf. The earlier `build`-verb collision with
`conary system generation build` is mooted by `cook` being canonical; the hidden
`build` alias's help text points to `cook` and cross-references the generation
surface for anyone who meant that.

Every built package carries two **orthogonal** provenance fields in its manifest:

- **Origin class** (how the build was described): `native-built` (explicit recipe),
  `inferred-source` (build-system inference), `recorded-draft` (record mode output),
  or `foreign-converted` (.rpm/.deb/.pkg.tar.zst conversion).
- **Hardening level** (how the build was executed): `host`, `sandboxed` (container
  isolation, network allowed), `hermetic` (offline, pinned inputs — M2), `attested`
  (hermetic + signed attestation — M2).

Publish lint gates key off both — e.g. `recorded-draft` is never publishable
directly regardless of hardening (see Record mode), and from M2 the default publish
gate requires `attested`. An M1a project-form publish is typically
`native-built` + `sandboxed`; the old single-axis taxonomy conflated these.

### `conary try [pkg.ccs]`

Installs the freshly built package into a **throwaway generation**. This loop is the
centerpiece DX: no other packaging system can offer install-with-instant-total-undo.

Because it is the centerpiece, try gets an explicit **state machine**, not vibes —
and a **guest execution model**, because the obvious implementation is dangerous: the
existing live generation switch bind-mounts the new `/usr` over the host's
(`apps/conary/src/commands/generation/switch.rs` warns in-code that processes with
open file descriptors under `/usr` may crash). A try that can crash your desktop is
the opposite of "try means safe."

- `conary try <pkg.ccs>` creates the throwaway generation and opens a shell (or runs
  `try <pkg.ccs> -- <cmd>`) **inside that generation's mount namespace**
  (bubblewrap/chroot-style). The host's global root is untouched; other users and
  running processes are unaffected. Exiting the shell tears the namespace down —
  rollback in the default model is trivially total.
- `conary try status` shows the active session; `conary try rollback` ends it;
  `conary try keep` promotes the generation permanently (global activation happens
  here, and only here, with the same semantics and caveats as any generation switch).
- `conary try <pkg.ccs> --activate` (the flag rides the same form as a normal try,
  no separate subcommand) exists for the cases that genuinely need host-global
  activation before keep/rollback (e.g. testing a system daemon under the real init);
  it prints the live-switch risk plainly and is not the documented default path.
- **At most one try session at a time** (v1); starting a second is an error that
  names the active one.
- **Crash/reboot safety:** the session record is durable. The next conary invocation
  detects an orphaned try session and prompts to rollback or keep — a crashed try
  never silently becomes the permanent system. In non-interactive contexts (daemons,
  scripts, CI) orphan detection **fails closed**: no prompt, log the error, and for
  an orphaned `--activate` session trigger automatic rollback to the previous
  generation. (Proactive boot-time notification — a systemd unit or login message —
  is later hardening, not M1; detection in M1 happens at next invocation.)
- **Service teardown on activated rollback:** rolling back an `--activate` session
  must `systemctl stop`/`disable` every service the package's manifest declares
  *before* reverting the filesystem — otherwise the unit file vanishes while the
  process still runs, leaving orphans systemd can no longer manage.
- **Rollback scope stated honestly (applies chiefly to `--activate` sessions;
  namespace sessions are contained by construction):** rollback reverts
  generation-owned filesystem state (the package's files, links, units). It does not
  un-happen runtime side effects — services that ran, `/var` mutations, data the
  package wrote, external effects. Hook policy: try **refuses** packages that declare
  hooks with non-generation-scoped lifecycle effects (db migrations, irreversible
  state changes) unless the package declares reversible cleanup or the user passes
  `--allow-irreversible`. The policy **fails closed**: hooks that are unknown,
  unclassified, or legacy scriptlets are treated as non-generation-scoped — an
  incomplete declaration is never an escape hatch. The classification field this
  gate reads does not exist yet (the current `Hooks` struct in
  `ccs/manifest.rs` carries no reversibility metadata); minimal schema, defined
  before M1b lands: an optional `reversible: bool` per hook, defaulting to `true`
  for declarative variants (users/groups/directories/systemd/tmpfiles/sysctl/
  alternatives) and `false` for `ScriptHook` (`post_install`/`pre_remove`) and all
  legacy scriptlets. One guard on that default: declarative hooks are only
  generation-scoped when the hook executor targets the generation/namespace root —
  a declarative hook executed against the host root fails closed exactly like a
  scriptlet. The M1b plan verifies the executor's targeting before the defaults
  apply.
- **Session data model (new — no precedent in the generations system):** a
  `try_sessions` table: `id`, `previous_generation_id`, `try_generation_id`,
  `package_path`, `started_at`, `status` (`active`|`orphaned`|`kept`|`rolled_back`),
  with single-active-session enforced by a partial unique index on
  `status = 'active'`. Crash recovery queries for `active`, compares against actual
  generation state, then applies the orphan policy above. "Try means safe" must be literally true by default; most
  CCS hooks are declarative (units/tmpfiles/sysctl) and generation-scoped, so the
  refusal bites rarely. Hook reversibility is a manifest field and a publish lint.
- Try requires the same privileges as `conary install`.
- `--watch` (M3) composes cook + try from the package project directory (it does not
  take a prebuilt `.ccs`): inotify on the source tree → incremental rebuild →
  hot-swap the throwaway generation.

### `conary publish [WHAT] <target>`

Signs and publishes. Concrete forms (no ambiguity about what gets published):

- `conary publish <target>` — publishes the **current project** (a recipe is present
  or inference applies); triggers the isolated rebuild (`sandboxed` in M1a,
  `hermetic` from M2 — see below). The default, and the only form where rebuild
  happens.
- `conary publish <pkg.ccs> <target>` — publishes an existing artifact. Gated on its
  provenance class: the artifact must carry a hermetic-build attestation (M2), else
  publish refuses and says to run the project form.
- Publishing "everything in dist/" is deliberately not a form; multi-package publish
  is one project at a time (CI loops over projects).

Targets:

- a static repo: local directory, `rsync://`/SSH, or S3-compatible bucket
- a Remi instance: authenticated upload (bearer token, v1-simple)

Publish **always rebuilds in isolation first**. Host builds are for iterating, never
for shipping. What "isolation" means is milestone-honest, because the label must
never overclaim:

- **M1a:** publish rebuilds with Kitchen container isolation, which already exists
  (`use_isolation` in `recipe/kitchen/config.rs`) — but network is still allowed, so
  provenance is stamped `hardening: sandboxed`, **not** `hermetic`. Publish prints
  that the repo is a preview, not reproducible release evidence.
- **M2:** the offline prefetch-then-build model below lands; only those builds earn
  `hardening: hermetic` (plus `attested` once signed). M2 flips the publish gate to
  require it.

The full M2 requirements:

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
  against a named repo snapshot — concretely, the **TUF snapshot version pinned at
  build time**; the lock records repo URL, snapshot version, and resolved versions,
  and re-resolution replays against that snapshot. Exact schema defined in the M2
  plan before publish hardening lands.
- **Attestation (defined, not hand-waved):** an Ed25519 signature **by the
  publisher's key** over the build's input identities (recipe hash, source
  checksums/commit/tree hashes, dependency-lock snapshot version) and output
  identity (file-manifest merkle root), embedded in the manifest provenance as a
  `build-attestation` entry. The existing `DerivationExecutor` CAS capture informs
  this but is build-log-level today — the structured, signed attestation is new
  work, not reuse.
- **Divergence diagnostic:** if the hermetic output differs from the last host build
  of the same tree, publish says so and shows what changed — that delta is exactly
  the dishonest-dependency signal we're hunting. To make this implementable and not
  a false-positive firehose: host builds record a `host_build_record` (tree hash,
  output manifest hash) at build time, and both build modes set standard
  reproducibility controls (`SOURCE_DATE_EPOCH`, build-path mapping) so known
  nondeterminism doesn't drown the signal.

First-ever publish auto-generates an Ed25519 keypair under `~/.config/conary/keys/`,
prints the fingerprint, and embeds the public key in repo metadata. There is no
separate keygen ceremony.

## One format, one internal representation

- The existing recipe TOML (`crates/conary-core/src/recipe/format.rs`) is the single
  human-facing format, **extended** to absorb the fields currently unique to
  `ccs.toml`: hooks, capability overrides, component classification rules.
- **The recipe schema gains a local-source variant.** Today `SourceSection` mandates
  a remote `archive` URL + checksum, and the cook path unconditionally downloads it —
  which silently breaks the upstream-developer persona (a materialized recipe would
  ignore the developer's local tree). `source` becomes an enum: remote archive (as
  today) or `path = "."`, where the build uses the local workspace (bind-mounted into
  the sandbox for isolated builds). For hermetic builds of a path source, the sandbox
  receives **exactly the git-tracked files** — never untracked files — so the
  recorded tree hash is an honest description of the build input; the CLI warns
  about untracked files before building.
- The CCS manifest becomes a **generated artifact**. Humans never write `ccs.toml` in
  the primary flow. Migration boundary: `ccs.toml` and the `conary ccs init/build`
  surface remain supported as the low-level manifest/debug layer until the recipe
  format expresses every install-time field; only then does the primary documentation
  drop them. The recipe is authoritative wherever both could apply.
- Inference produces a synthetic in-memory `Recipe`, so the engine has exactly one
  input type. `conary new` (in a source tree) serializes the inferred recipe to disk,
  pre-filled, when the user needs to override something.
- `conary cook` is **promoted, not demoted**: the existing recipe-only cook surface
  grows into the universal front door described above (its CLI shape is extended,
  not forked). `conary ccs build` remains as plumbing and leaves the primary
  documentation.
- The bootstrap pipeline becomes a consumer of the same build pipeline — conaryOS
  dogfoods the toolchain third parties use.

## Engine: `BuildPipeline`

New module in conary-core:

```
resolve input → plan (recipe or inference) → execute via Kitchen (host | isolated)
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

`conary cook --record` opens a shell; the user builds their software the way they
always do. Conary traces the session and **derives the recipe**: commands run, files
read (dependency evidence), files installed (manifest), plus suggested capability
declarations. To be precise about what exists: the `capability/` module provides the
**declaration schema** record mode would populate — it is not an observation
mechanism. The recording path (seccomp-notify/fanotify → trace → derived recipe and
capabilities) is entirely new infrastructure. This is the riskiest technical bet in
the design and gets a prototype spike before commitment.

Reliability bar: record mode output is **always a draft** — it emits a recipe marked
`recorded-draft` plus a trace report, and a recorded session is never directly
publishable. The draft must pass a normal (non-recorded) `conary cook`, at which
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
packaging tools (`cook`, `diagnose`, `try`, `publish`) so an agent can drive the
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
   command, and generic next steps (`--explain`, rerun in `--isolated`, open a recipe).

The rule stands in tiered form: a newcomer never sees a bare exit code with *no* next
step — but tier 3 promises a starting point, not a diagnosis.

### Watch mode

`conary try --watch` runs from the package project directory (it composes cook + try,
rather than taking a prebuilt `.ccs`): inotify on the source tree → incremental rebuild
→ hot-swap the throwaway generation. Small lift over `try`; outsized demo and
daily-use value.

### Universal ingestion

`conary cook <git-url | tarball | .deb | .rpm | .pkg.tar.zst>` — one verb, always
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
`docs/specs/` is a mandatory gate (M0) before M1 implementation of publish/repo-add.**
It must define:

- **The index↔TUF bridge** — currently disconnected systems: `RepositoryMetadata`
  (the index shape) has only a free-form `version: String`, and nothing ties index
  content to TUF target entries. The child spec defines: index.json gains a
  monotonic `index_version: u64` matching the TUF targets version; index.json's
  sha256 is listed as a target in `targets.json`; and the client fetch order is
  **download → verify hash against the TUF-verified target entry → only then
  parse**. No code path parses an unverified index.
- **Atomic publish for non-transactional backends** (S3, rsync, plain HTTP dirs):
  upload in reverse verification order — packages/chunks first, then
  `targets.json` + `index.json`, then `snapshot.json`, then `timestamp.json` (and
  `root.json` only on rotation) — so a concurrent client never verifies against
  metadata that references missing files.
- **`conary-repo.toml`'s exact schema** and its relationship to TUF `root.json`
  (the parent spec commits only to: human-readable identity + key fingerprints for
  TOFU; it never carries full key material).
- **Metadata expirations and refresh expectations**, rollback/freeze protection.
- **The operator key lifecycle** — placement (where the pubkey lives and which copy
  is authoritative), rotation, revocation, backup, and loss recovery, stated
  plainly, because the auto-generated first-publish key *is* the repo authority.
  Loss recovery includes the client side: a reset repo with a new key requires the
  user to explicitly re-trust (e.g. `conary repo reset-trust <name>`), never a
  silent re-pin. Until M0 defines key placement, M1a key generation warns that
  lifecycle management is pending and the private key must be stored securely.
- **A file-based TUF metadata generator.** Remi's TUF path regenerates from its
  database; a static repo has no database — publish needs a generator that signs
  metadata from files at publish time. These are different architectures sharing
  the `trust/` types, and the child spec owns the file-based one.

Client-side work:

- Lift the `file://` rejection in **all three** places it lives, not one: the
  repository client (`validate_url_scheme` in `repository/client.rs`), the Kitchen
  source fetcher (`download_file` in `recipe/kitchen/archive.rs`, so local-path/
  `file://` recipe sources work), and the TUF transport (`TufClient` uses bare
  `reqwest::get`, which cannot fetch `file://` — it needs a filesystem fallback for
  local-path repos).
- `conary repo add <name> <url|path> --fingerprint <fp>` is the **documented happy
  path** (matching the existing `repo add <name> <url>` CLI shape): repo operators
  publish their key fingerprint out-of-band (website, README) and the add verifies
  it. Without `--fingerprint`, the add shows the fingerprint and requires an explicit
  trust-on-first-use confirmation; the key is pinned thereafter.
- **Trust-mechanism exclusivity:** `--fingerprint` marks a CCS/static (TUF) repo;
  the existing GPG flags (`--gpg-key`, `--no-gpg-check`, `--gpg-strict`) apply to
  legacy/distro repos only. The two sets are mutually exclusive: clap rejects
  explicit `--fingerprint` + GPG flag combinations at parse time, and command
  execution rejects GPG flags after probing a repo as static because static-ness is
  not knowable before the probe — three parallel trust paths is two too many for
  one repo.
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
- **Integration (conary-test, new suite):** cook → try → rollback → publish →
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
- `conary cook` is the primary packaging front door; `conary ccs build` remains
  supported as plumbing and leaves the primary documentation.
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
- **Try session scope:** system-global state (it requires install privileges), but
  the default guest execution model means other users are unaffected until
  `try keep` or `--activate`.
- **Attestation signer:** the publisher's Ed25519 key (the same authority that signs
  packages) — no separate machine key.
- **GPG repo infrastructure:** stays, for legacy/distro repos; CCS/static repos use
  fingerprint/TUF exclusively (clap-enforced). No removal in v1.

## Revision notes (2026-06-10)

**M0 drafting (2026-06-10):** the child spec resolves three delegated
decisions: `consistent_snapshot = false` in v1 (the current client only
fetches unversioned snapshot/targets filenames; reverse-order upload + hash
pinning fail safe — versioned filenames are the v2 path); two operator
keypairs (root + publish) rather than one, so a publish-key compromise is
root-recoverable; timestamp expiry defaults to 30 days with
`conary publish --refresh` as the re-sign path.

**Round 7 (same day — verb naming):** the canonical front door is `conary cook`,
not `conary build`. Rationale: cook already exists and already means "build a
package from a recipe" (promoting it is less churn than introducing a competing
verb); it dissolves the `conary system generation build` ambiguity outright; and
recipe → kitchen → cook → try → publish preserves the project's heritage vocabulary
as a coherent user journey. `conary build` becomes a hidden compatibility alias
(works, help redirects to `cook`, never documented) — one visible path, the alias
is a courtesy for muscle memory and agents. `BuildPipeline` remains the internal
engine name. The earlier round-1 decision to demote cook to plumbing is reversed:
cook is promoted into the front door; only `conary ccs build` stays plumbing.

**Round 6 (same day, GPT consistency pass — final):** the isolation flag is renamed
`--isolated` and held stable across milestones (it requests the strongest isolation
available; the provenance hardening field, not the flag name, carries the truth
claim — avoiding both M1a overclaim and an M2 flag rename); provenance split into
two orthogonal fields, origin class (native-built / inferred-source / recorded-draft
/ foreign-converted) and hardening level (host / sandboxed / hermetic / attested);
declarative hooks default reversible **only** when the hook executor targets the
generation root — host-root execution fails closed like a scriptlet; `--activate`
written unambiguously as `conary try <pkg.ccs> --activate`. Reviewer reported no
critical findings and confirmed the guest execution model as the right pivot.

**Round 5 (same day — Gemini + DeepSeek, reviewed independently):** the two biggest
catches both survived four GPT rounds. (1) **Try got a guest execution model**: the
naive implementation live-bind-mounts the new `/usr` over the host's (per the in-code
warning in `generation/switch.rs`, running processes may crash) — so try now runs
inside the throwaway generation's mount namespace by default, host untouched; global
activation only via `try keep` or explicit `--activate`. This also resolved
multi-user semantics and made default rollback containment-by-construction, with
service teardown ordering defined for activated sessions. (2) **The recipe schema
gains a local-source variant**: `SourceSection` mandates a remote archive+checksum
and cook unconditionally downloads, which would have silently broken the
upstream-developer persona; `source` becomes remote-or-path, with hermetic path
builds receiving exactly git-tracked files. Also adopted: all three `file://`
rejection sites enumerated (repo client, Kitchen fetcher, TufClient transport); the
index↔TUF bridge defined (monotonic `index_version: u64`, index hash as TUF target,
verify-before-parse); reverse-verification-order uploads for non-transactional
backends; M1a publish semantics pinned (Kitchen isolation, network allowed,
`hardening: sandboxed` — `hermetic` reserved for M2 offline builds); hook
reversibility schema sketched (`reversible: bool`, declarative hooks default true,
script hooks false); `try_sessions` table sketched; attestation defined (publisher
Ed25519 signature over input/output identities — DerivationExecutor capture is
build-log-level, so this is new work, not reuse); divergence diagnostic made
implementable (`host_build_record` + `SOURCE_DATE_EPOCH`/path mapping);
GPG-vs-fingerprint exclusivity via clap conflicts; `conary-repo.toml` schema and key
placement deferred to M0 with a file-based TUF generator named as M0 scope;
key-loss recovery requires explicit client re-trust; non-interactive orphan
handling fails closed; M1b internal ordering (inference via `new --from .` first);
arch determination defined; `--explain` backed by structured `InferenceTrace`;
record mode's declaration-vs-observation gap stated.

**Round 4 (same day):** availability matrix distinguishes project-form publish
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
