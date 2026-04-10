---
last_updated: 2026-04-09
revision: 2
summary: Design for a release-blocking, evidence-backed audit of every tracked documentation file in the repository, including archival cleanup and cross-doc consistency repair
---

# Documentation Accuracy Audit

## Context

Conary is approaching a release bump, and the tracked documentation surface has
grown across multiple audiences and time horizons:

- root product and contributor docs such as `README.md`, `ROADMAP.md`,
  `CONTRIBUTING.md`, `SECURITY.md`, `CHANGELOG.md`, `AGENTS.md`, and
  `CLAUDE.md`
- canonical product and operator docs under `docs/`
- deploy and host workflow docs under `deploy/`
- app-local READMEs and harness notes under `apps/` and `bootstrap/`
- planning and design material under `docs/superpowers/`
- site/package frontend readmes under `site/` and `web/`
- repository metadata templates under `.github/`
- archived recipe README files under `recipes/archive/`

That surface is no longer safe to treat as “mostly accurate.” It contains a mix
of release-facing guidance, contributor instructions, historical material,
planning artifacts, and templates. Some of those documents are meant to reflect
current behavior exactly. Others are supposed to preserve historical context
without misleading readers about current support status.

For the next release, we need one explicit audit that answers all of the
following:

- which tracked docs are still active and should remain in place
- which planning docs should be archived or deleted
- whether every retained substantive claim is backed by current repository
  evidence
- whether visible-but-incomplete features are described honestly
- whether the repository tells one coherent story across all retained docs

## Goal

Perform a release-blocking, evidence-backed audit of every tracked
documentation-like file in the repository and leave the tree in a state where:

- every retained doc is either current, clearly historical, or explicitly
  labeled as WIP/preview where appropriate
- stale or superseded planning material has been archived or deleted according
  to a consistent rule
- cross-document contradictions in naming, status, commands, paths, release
  tracks, and supported workflows have been resolved
- the release bump can proceed without carrying forward stale documentation debt

## Non-Goals

This design does not attempt to:

- rewrite repository history or preserve every old planning artifact
- claim that all code-visible surfaces are production-ready
- convert every long-form document into a single canonical handbook
- introduce broad new product features just to make docs read more cleanly
- treat archived historical material as if it were current release guidance
- define the full step-by-step implementation plan; that comes in the follow-up
  plan

## Decision

Adopt a tiered, evidence-first documentation audit that covers every tracked
documentation-like file and records an explicit disposition for each file.

The audit will:

- build a complete tracked-doc inventory from the repository
- classify documents by audience and stability
- triage planning/history documents before broad content verification
- verify active docs against primary repository evidence
- preserve historical material only when it is clearly framed as historical
- archive recent superseded specs/plans and delete older stale ones
- finish with a cross-doc consistency pass and a release-readiness summary

This is intentionally not a loose editorial sweep. The output should be a
traceable audit with per-file outcomes rather than an impressionistic
“everything looked okay.”

## Options Considered

### 1. Single-pass full sweep

Read every tracked doc, update what looks stale, and clean up archives at the
end.

Pros:

- simplest execution story
- low upfront process cost

Cons:

- weak traceability
- easy to lose track of what was actually verified
- archival cleanup competes with active release fixes for attention
- high risk of leaving contradictions between doc families

### 2. Tiered evidence-first audit

Build an inventory, classify doc families, triage planning/history material,
verify retained docs against repository evidence, and end with a consistency
pass.

Pros:

- strongest release-readiness signal
- explicit per-file accounting
- separates active guidance from historical cleanup
- supports honest WIP language without hand-waving
- makes archival decisions reviewable

Cons:

- more process than a simple edit pass
- requires disciplined evidence gathering

### 3. Release-facing-first, historical-second

Repair user-facing docs before the release, then process archives and planning
material later.

Pros:

- fastest path to improving the most visible docs

Cons:

- leaves tracked stale material in the tree during the release bump
- does not satisfy the requirement to align everything
- allows old docs to continue contradicting current behavior

Recommended option: `2`.

## Scope

The audit covers every tracked repository file that is intentionally
documentation-like, including at minimum:

- root docs and contributor guidance
- `.github/` issue and PR templates
- tracked example/template docs such as `*.example.md`
- canonical docs under `docs/`
- deploy docs under `deploy/`
- app-local and bootstrap READMEs
- site/package frontend READMEs
- active and archived planning/design docs under `docs/superpowers/`
- formal specs under `docs/specs/`
- historical recipe READMEs under `recipes/archive/`

The practical inventory should be generated from tracked files, not maintained
by hand, so the audit cannot silently skip a newly added doc.

Ignored or untracked local documentation trees are not part of the
release-blocking tracked-doc inventory unless they are promoted into version
control. For example, ignored local archives such as `docs/plans/archive/` and
`docs/superpowers/reviews/archive/` may exist on disk, but they are not part of
the tracked documentation surface unless they become tracked files.

## Audit Policies

### Truthfulness policy

- Current release-facing docs must match the current repository exactly.
- Visible but incomplete functionality may remain documented only if the status
  language is explicit and honest.
- WIP surfaces must not be described with wording that implies supported,
  production-ready behavior when that is not true.

### Planning-material policy

- Tracked planning or review docs that are already inside archive subtrees are
  retained by default and reviewed as historical records unless they are
  duplicated, misleading, or explicitly selected for later cleanup.
- Superseded tracked planning or review docs outside archive subtrees dated
  `2026-04-01` or later should be moved into the appropriate archive subtree if
  they are still worth retaining as recent release-cycle history.
- Superseded tracked planning or review docs outside archive subtrees dated
  before `2026-04-01` may be deleted only if they are unreferenced by retained
  docs, no longer needed as active historical records, and their important
  decisions are already captured elsewhere in retained documentation.
- Retained planning/history docs should be framed as design records or archive
  material, not as active source-of-truth guidance.
- If tracked repo guidance such as `AGENTS.md` needs wording updates so the
  cleanup policy is no longer ambiguous, update that guidance as part of the
  audit instead of silently diverging from it.

### Historical-material policy

- Historical documents may remain historical.
- Historical documents that are already archived are not deleted by default as
  part of this audit.
- Historical documents must not read like current instructions unless they are
  still accurate and intentionally retained for present-day use.
- Old references to removed topology, retired harnesses, renamed products, or
  dead workflows should be reframed, updated, archived, or removed.

### Evidence policy

- Substantive claims require repository evidence.
- If a claim cannot be verified cleanly, it must be narrowed, reframed, or
  removed.
- “Looks plausible” is not enough for release-blocking docs.

## Document Families

The audit should classify each tracked doc into a workstream so the right
verification standard is applied.

### 1. Active release-facing and contributor docs

Examples:

- `README.md`
- `ROADMAP.md`
- `CONTRIBUTING.md`
- `SECURITY.md`
- `AGENTS.md`
- `CLAUDE.md`
- canonical docs under `docs/`
- deploy/operator docs under `deploy/`
- app-local READMEs under `apps/` and `bootstrap/`
- `site/README.md` and `web/README.md`

These receive the strictest accuracy standard because they can influence user,
operator, contributor, or release behavior directly.

### 2. Templates and metadata docs

Examples:

- `.github/ISSUE_TEMPLATE/*.md`
- `.github/PULL_REQUEST_TEMPLATE.md`
- tracked example docs such as `docs/operations/LOCAL_ACCESS.example.md`

These should be checked for current workflow names, expected release/process
language, and references to current product structure.

### 3. Formal specifications and design docs

Examples:

- `docs/specs/*.md`
- active planning/design docs under `docs/superpowers/`

These need triage first: active, archive, or delete. Retained docs should still
be checked for misleading present-tense claims and path/workflow drift.

### 4. Historical and archive docs

Examples:

- `docs/llms/archive/*.md`
- `docs/superpowers/archive/*.md`
- archived recipe READMEs under `recipes/archive/`

These may remain historical, but they still need status framing, link hygiene,
and freedom from misleading current-tense claims.

## Verification Standard

Every retained active or template document should have its substantive claim
clusters validated against one or more primary repository sources. Historical
documents should still be checked for framing, stale present-tense language, and
link/status hygiene even when they are not re-proved line by line against
current code.

### Hard claims

These must be directly proven:

- command names, flags, and examples
- file paths, module ownership, and workspace layout
- release tracks, tag formats, and deployment steps
- URLs, ports, endpoints, and host-role descriptions
- workflow names, script entrypoints, and automation behavior
- package, crate, or app boundaries
- support status claims such as “available,” “required,” or “the supported
  path”

Preferred evidence:

- source files and directory structure
- CLI definitions and `--help` output where useful
- Cargo manifests and workspace configuration
- scripts and checked-in config files
- GitHub workflow files
- integration manifests and test code

### Interpretive claims

Examples:

- architecture summaries
- subsystem ownership descriptions
- “look here first” assistant guidance
- contributor workflow recommendations

These still need to be grounded in code layout and current patterns, but can be
kept at a stable abstraction level to reduce churn.

### Historical claims

These must be obviously historical and not mistaken for active instructions.
Where necessary, add framing that makes the time horizon explicit.

### Claim-cluster recording

The audit does not need a separate ledger row for every sentence, but it does
need more than a single file-level boolean. Each retained active or template doc
should record the major substantive claim clusters that were checked, such as:

- command/CLI surface
- paths and module ownership
- workflow/deploy/release behavior
- support-status and WIP language
- URLs, ports, hosts, and endpoint descriptions

If a claim cluster could not be proven as originally written, the ledger should
record whether it was narrowed, reframed, or removed.

## Evidence Sources By Doc Type

The audit should preferentially verify against the sources most likely to be
authoritative for each family.

### CLI and command docs

Verify against:

- `apps/conary/src/cli/`
- `apps/conary/src/commands/`
- `apps/conary-test/src/cli.rs`
- command help output when needed for flag/wording confirmation

### Architecture and subsystem docs

Verify against:

- `Cargo.toml`
- workspace directory layout
- crate/module paths under `apps/` and `crates/`
- current code ownership boundaries

### Testing and harness docs

Verify against:

- `apps/conary-test/src/`
- integration manifests under `tests/`
- fixture and harness readmes
- relevant scripts and workflow files

### Deploy and operations docs

Verify against:

- `deploy/`
- `scripts/`
- GitHub workflows under `.github/workflows/`
- current config files and service boundaries documented in source

### Planning and release-process docs

Verify against:

- active specs/plans under `docs/superpowers/`
- release automation scripts
- workflow files
- current product/version ownership in manifests

## File-Level Outcomes

Every tracked doc file should end the audit with one explicit disposition:

- `verified-no-change`
- `corrected`
- `clarified-as-wip`
- `reframed-as-historical`
- `retained-historical`
- `archived`
- `deleted`

Those outcomes should be recorded in an audit ledger so completion can be
measured objectively.

## Audit Ledger

The implementation should maintain a machine-generated or systematically updated
ledger that includes, for each tracked doc:

- file path
- document family
- intended audience
- verification status
- evidence sources checked
- substantive claim clusters reviewed
- claims narrowed, reframed, or removed
- final disposition
- notable follow-up risk, if any

The ledger is the mechanism that converts this from a thoughtful reading pass
into a release-quality audit.

## Execution Model

### Phase 1: Build the inventory

Generate the authoritative list of tracked documentation-like files and classify
them by family.

### Phase 2: Triage planning/history material

Before broad content editing, decide which active/superseded plans and specs
should remain active, move to archive, or be deleted.

This reduces the amount of stale planning material that can confuse later
verification.

### Phase 3: Verify active docs by family

Audit and repair:

- root docs
- canonical `docs/`
- deploy/operator docs
- app-local and bootstrap readmes
- site/web readmes
- templates and remaining retained historical docs

Each document should be verified against its evidence sources before being
marked complete.

### Phase 4: Cross-doc consistency pass

After per-file verification, run a consistency pass across the retained tree for
shared facts such as:

- product naming
- release track vocabulary
- deploy workflow names
- supported-vs-preview wording
- paths, URLs, and ports
- host-role descriptions
- product boundaries and workspace layout

The ledger is not final until this pass is complete. If Phase 4 changes a file,
update that file's ledger entry to reflect the normalization change and confirm
that the relevant claim cluster still has evidence behind it.

### Phase 5: Release-readiness wrap-up

Produce:

- the updated docs
- archived/deleted stale planning material
- the completed audit ledger
- a concise audit summary of major corrections and policy decisions

## Release-Facing Language Rules

For retained docs, wording should follow these rules:

- use present tense only for current, supported behavior
- use explicit labels such as “preview,” “WIP,” “not yet supported,” or
  equivalent when the code surface exists but is not fully operational
- avoid aspirational wording that sounds like a supported workflow unless the
  workflow has been verified
- prefer narrow, provable statements over broad claims that cannot be defended

## Risks And Mitigations

### Risk: scope sprawl

Auditing every tracked doc can turn into unbounded editorial work.

Mitigation:

- keep the audit anchored to verification and disposition, not prose polish
- prioritize factual correctness and consistency over stylistic rewrites

### Risk: historical documents misleading current readers

Mitigation:

- add explicit historical framing
- archive or delete stale planning material aggressively
- remove present-tense operational language from documents that are no longer
  active guidance

### Risk: code-visible but incomplete features being overstated

Mitigation:

- require explicit WIP/preview language
- verify support claims against tests, workflows, and actual entrypoints

### Risk: inconsistent facts surviving across documents

Mitigation:

- reserve a dedicated final consistency pass
- normalize shared vocabulary across the retained tree

## Acceptance Criteria

This design is complete when the implementation leaves the repo with all of the
following true:

- every tracked documentation-like file has been inventoried
- every file has a recorded final disposition
- every retained active or template document has a verification record covering
  its substantive claim clusters, and every retained historical document has a
  historical-framing disposition that confirms it is not being treated as active
  guidance
- every retained active/template substantive claim has been verified or narrowed
  to something provable
- recent superseded planning docs have been archived
- older stale planning docs have been deleted
- visible-but-incomplete surfaces are documented honestly
- cross-doc contradictions around release, deploy, testing, and product
  structure have been resolved
- the repository is ready for a release bump without known stale tracked docs
  undermining the release story
