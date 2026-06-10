# Feature Coherency Ledger And Repair Program

## Status

Draft design; user review pending.

## Goal

Create a living feature coherency program that ties project-wide audit findings
directly to repair slices. If a command, route, operation, or active doc claim
exists, Conary should be able to prove one of four honest states:

- it works and has evidence;
- it works but needs a named hardening slice;
- it is intentionally deferred and represented honestly;
- it is stale or misleading and is removed, merged, or fixed.

The audit must not end as a static list of broken promises. Every finding gets
a case-by-case decision, an owner, and a closure path.

## Background

Conary has grown several user-facing and agent-facing surfaces:

- the `conary` CLI command tree, manpage, dispatch routing, and command docs;
- active docs such as `README.md`, `docs/conaryopedia-v2.md`,
  `docs/ARCHITECTURE.md`, `docs/modules/*.md`, and `docs/operations/*.md`;
- Remi HTTP/admin/MCP routes;
- conaryd REST/SSE/package-operation routes;
- `crates/conary-agent-contract` and `crates/conary-mcp`;
- integration-test, bootstrap, and operation guidance used by coding agents.

The repo already has useful orientation docs:

- `docs/modules/feature-ownership.md` maps capabilities to owner files,
  neighbors, proof commands, and docs to update.
- `docs/llms/README.md` and `docs/llms/subsystem-map.md` route agents toward
  canonical subsystem docs.
- Recent command work has been moving large command areas toward clearer child
  module ownership.

The remaining risk is coherence drift: a command appears in help but is thinly
wired, a doc describes a feature more strongly than the implementation
supports, a preview route returns a generic placeholder, or two surfaces solve
the same job with different contracts.

## Non-Goals

- Do not do a broad redesign just because a cleaner architecture is imaginable.
- Do not build speculative features unless an active public claim already
  commits Conary to that behavior.
- Do not preserve stale compatibility surfaces only to avoid deleting them.
- Do not mark deferred work as implemented by documenting that it is deferred.
  Honest deferral documentation is valid and required for `honest-deferred`
  rows, but it does not close a `fix-now` row as repaired.
- Do not rewrite archived historical design or plan records solely to make old
  claims sound current.
- Do not let external model review replace live repo verification.

## Core Artifact

Add and maintain `docs/superpowers/feature-coherency-ledger.tsv`.

The ledger is the active repair queue for project coherency. It should stay
small enough to act on and concrete enough for a fresh session to resume work.
Rows that have been fully closed can either remain with evidence or move to an
archive ledger if the active file becomes noisy.

### Relationship To The Documentation Accuracy Audit

`docs/superpowers/documentation-accuracy-audit-ledger.tsv` remains the
canonical inventory and verification ledger for tracked Markdown files. It is
paired with `docs/superpowers/documentation-accuracy-audit-inventory.tsv` and
enforced by `scripts/check-doc-audit-ledger.sh` plus the PR gate. The feature
coherency ledger supplements that system; it does not replace it.

Use the existing documentation accuracy ledger for file-level documentation
truth, archive state, and tracked Markdown inventory. Use the feature
coherency ledger for implementation-to-claim coherence: commands, routes,
operation contracts, MCP tools, docs claims tied to behavior, and duplicate
public surfaces. When a coherency row causes tracked Markdown to change, update
the existing documentation accuracy audit files in the same commit. If a
finding is purely documentation inventory or historical-doc framing with no
feature surface, keep it only in the documentation accuracy ledger.

### Ledger Columns

- `id`: stable short ID in `{TYPE}-{SUBSYSTEM}-{NNN}` form, such as
  `CLI-BOOTSTRAP-001`.
- `surface`: command, route, MCP tool, doc claim, module feature, or operation.
- `source`: where the surface is advertised, routed, or documented.
- `related_ids`: other coherency rows that must move with this one, or empty.
- `wave_scope`: wave and selected scope, such as `1a-root-cli`.
- `owner`: owning subsystem or feature ownership card.
- `claim`: concise statement of what the surface promises.
- `actual_or_gap`: current behavior, missing behavior, contradiction, or thin
  spot.
- `status`: current classification.
- `disposition`: closure method when the row is resolved.
- `last_verified`: ISO 8601 date when the current status was last checked.
- `evidence_sources`: source pointers, docs, tests, scripts, or command paths.
- `repro`: command, inspection, or source pointer that demonstrates the gap, or
  `none` when the row is already verified.
- `verification`: exact command or inspection gate that proves the decision.
- `decision`: fix, defer, remove, merge, harden, or verify.
- `next_slice`: smallest follow-up that would close or advance the row.
- `notes`: short context, caveat, or link to a plan/spec row.

Avoid vague entries such as "needs work." If a row cannot name a concrete
surface, owner, and next slice, it is not ready for the ledger.
`actual_or_gap` is mandatory for `fix-now`, `misleading`, `duplicate-stale`,
and `works-but-thin` rows.

TSV fields must stay single-line. Encode embedded newlines as `\n`, tabs as
`\t`, and multi-value lists with semicolons. The validation script should reject
raw tabs inside fields, duplicate IDs, invalid status or disposition values,
unknown ID prefixes, and invalid typed source pointers.

Use typed source pointers so the validator can distinguish filesystem paths
from commands and logical surfaces:

- `path:apps/conary/src/cli/mod.rs:10-40`
- `doc:README.md:120-130`
- `cmd:cargo run -p conary -- --help`
- `test:cargo test -p conary --lib cli::tests`
- `route:GET /v1/transactions`
- `mcp:remi/tool-name`

The validator should require path existence only for `path:` and `doc:`
pointers. It may syntax-check the other pointer forms without pretending they
are filesystem paths.

## Status Taxonomy

- `works`: implemented, represented honestly, and backed by proof.
- `works-but-thin`: usable, but missing focused proof, docs, or UX hardening.
- `fix-now`: important and bounded enough to repair in the current wave.
- `honest-deferred`: intentionally unavailable or preview-only, with active
  docs/help/error text that says so clearly.
- `misleading`: active docs, help, routing, or naming imply more support than
  exists.
- `duplicate-stale`: two surfaces overlap, or an older path conflicts with the
  forward design.

A surface is `misleading` when a reasonable user or agent following the active
claim reaches a dead end, silent no-op, missing documented output, contradicted
behavior, or generic error with no honest explanation of the gap. A surface is
`works-but-thin` when the advertised happy path succeeds but the row is missing
specific proof, docs, UX hardening, or edge-case coverage. Each
`works-but-thin` row must name the missing piece in `next_slice`; if it remains
thin across two later waves without a changed next slice, reclassify it as
`honest-deferred` or `fix-now`.

### Disposition Taxonomy

- `open`: active in the current wave and carrying non-empty `owner`,
  `decision`, `next_slice`, `verification`, and `last_verified` fields.
- `verified-no-change`: proof showed the current surface is honest.
- `resolved-repaired`: implementation, tests, docs, or UX were fixed.
- `resolved-removed`: stale claim or dead surface was removed.
- `resolved-merged`: duplicate surface was folded into the forward path.
- `deferred-owned`: deliberately deferred with a tracked plan or next slice.

No row should close as "broken." A row closes only when the current state is
honest and verified: implemented, repaired, deliberately deferred, merged, or
removed from active claims. `status` describes the current condition;
`disposition` records how the row was closed.

Rows with `works` or `works-but-thin` status should be re-verified before being
cited as current evidence if `last_verified` is more than 90 days old. The
ledger validator may warn on stale verification dates; it should not block
emergency repair work solely because older rows need refresh.

Untriaged findings belong in wave scratch output, not the durable ledger. Once a
row is added to `feature-coherency-ledger.tsv`, it must already have enough
evidence to pick a status, owner, decision, next slice, and verification gate.

### Closure Matrix

- `works` closes as `verified-no-change` or `resolved-repaired`.
- `works-but-thin` can remain `open` only inside the current wave, or close as
  `resolved-repaired`, `verified-no-change`, or `deferred-owned` after the
  missing proof, docs, UX, or edge-case slice is explicit.
- `fix-now` closes only as `resolved-repaired`, `resolved-removed`, or
  `resolved-merged`.
- `misleading` closes only as `resolved-repaired`, `resolved-removed`, or
  `resolved-merged`; if the feature is intentionally unavailable, first make
  the active surface honest, verify that refusal or documentation, then
  reclassify it as `honest-deferred` with `deferred-owned`.
- `duplicate-stale` closes only as `resolved-merged`, `resolved-removed`, or
  `resolved-repaired`.
- `honest-deferred` closes as `deferred-owned`.

## Review And Repair Workflow

Run coherency work in waves. Each wave should produce both evidence and fixes.

1. **Inventory:** collect public and agent-visible surfaces for the selected
   area. Include CLI help, dispatch routes, docs, manpages, MCP catalogs,
   daemon routes, and explicit `TODO` / `not implemented` / `stub` / `future`
   hits when they touch public claims.
2. **Reality check:** inspect the owner files and run cheap proof commands.
   Ask what a user or agent would experience if they followed the claim.
3. **Case decision:** classify the row and choose fix, defer, remove, merge,
   harden, or verify.
4. **Repair slice:** when the fix is bounded, implement it in the same wave.
   Examples include wiring a routed command, changing misleading help text,
   adding a missing regression test, removing stale docs, or making a preview
   route return an honest specific error.
   Before deleting, renaming, or merging a command, route, script, or public
   string, run a repo-wide reference search and update callers, helper scripts,
   docs, integration manifests, and CI references in the same slice. Record
   discovered dependents in `notes`; if they cannot all move safely in one
   change, split the removal into related rows.
5. **Ledger update:** record the decision, evidence sources, repro, verification,
   and next slice.
6. **Verification:** run the focused proof from
   `docs/modules/feature-ownership.md` for touched areas, plus broader gates
   when a boundary is crossed.
7. **Docs-audit sync:** when a wave adds, moves, archives, or deletes tracked
   Markdown, update `docs/superpowers/documentation-accuracy-audit-ledger.tsv`
   and `docs/superpowers/documentation-accuracy-audit-inventory.tsv` in the
   same commit and rerun the docs-audit checks.

The ledger is not a substitute for tests. It is the connective tissue that says
why a surface is considered coherent today and what closes any remaining gap.

## Program Ownership

The maintainer owns the program and decides wave boundaries. Any contributor or
agent may add a row when it includes evidence, but changing a row to a resolved
disposition requires either the row owner or maintainer to verify the closure
criteria. Before starting a new wave, review open rows from the previous wave;
do not let `fix-now` rows accumulate while opening unrelated inventory work.

Codex is the default local operator for inventory, source inspection, cheap
proof commands, repair edits, and final verification because it has repository
access. External long-context reviewers can analyze bounded packets for
cross-surface contradictions, duplication, and missing proof. Their findings
become review input until Codex or the maintainer verifies them in the repo.

## First Wave Scope

Start with the CLI command tree only:

- root CLI help, subcommand help, dispatch routing, and `apps/conary/man`;
- the active docs claims needed to understand those CLI surfaces;
- focused `TODO`, `not implemented`, `stub`, `future`, and `unsupported` hits
  only when they affect a CLI command in this wave.

Sequence the first wave into sub-waves:

- **Wave 1a:** root CLI help, root examples, top-level dispatch coverage, and
  generated manpage behavior.
- **Wave 1b:** one high-visibility command family selected from
  `docs/modules/feature-ownership.md`.
- **Wave 1c+:** one additional command family at a time.

Each sub-wave must leave behind at least one resolved row or one implemented
repair, no unresolved `fix-now`, `misleading`, or `duplicate-stale` row for the
selected scope, and passing focused verification before the next sub-wave
begins. Carry-over is allowed only after the active surface is already honest
and the row is reclassified to `honest-deferred` with `deferred-owned`, an
owner, a next slice, and verification evidence. The ledger validator or report
script should have a scope-completion mode that rejects open `fix-now`,
`misleading`, and `duplicate-stale` rows in a completed scope.

Defer these to later, separate waves:

- broad active-docs claim review across `README.md`,
  `docs/conaryopedia-v2.md`, `docs/ARCHITECTURE.md`, `docs/modules/*.md`, and
  `docs/operations/*.md`;
- conaryd routes that imply package, system, query, transaction, or event
  operations;
- Remi MCP and HTTP/admin routes that advertise operation capabilities;
- `crates/conary-agent-contract` and `crates/conary-mcp` catalogs and
  operation vocabulary;
- codebase-wide comment sweeps not tied to the selected public surface.

The first repair target is:

> Advertised CLI commands must either work, refuse honestly, or be removed from
> active claims/help.

## Case-By-Case Deferral Rule

Intentional preview surfaces may remain when they are useful forward
infrastructure, but only if the active surface is honest:

- help text, docs, or route response must say the feature is preview-only or
  unavailable today;
- the deferred row must name the owner and next slice, and must reference a
  tracked plan, issue, or follow-up artifact when the repair cannot fit in the
  current wave;
- the refusal should be specific enough for a user or agent to know the current
  supported path;
- the row must not block unrelated cleanup from removing stale duplicate claims.

This keeps future-facing architecture visible without letting future wording
masquerade as implemented behavior.

## Verification Strategy

Each wave should choose the smallest meaningful proof set:

- CLI routing or help changes: `cargo check -p conary`, relevant CLI tests, and
  generated help/manpage checks where applicable. `apps/conary/build.rs`
  regenerates the ignored local `apps/conary/man/conary.1` via `clap_mangen`
  during `cargo build -p conary`; inspect or sweep that generated output for
  help-text drift, but do not commit ignored generated manpages unless the
  tracking policy changes.
- Wave 1a proof: run `cargo check -p conary`, run the focused CLI tests from
  the feature ownership card, run `cargo build -p conary` so the ignored
  manpage is regenerated locally, capture root help and selected subcommand
  help, then sweep the selected CLI/help/manpage/docs scope for
  `TODO|not implemented|stub|future|unsupported|broken`. The sweep output must
  be fixed, ledgered, or explicitly marked non-public or out of scope.
- Command behavior changes: focused package tests listed in
  `docs/modules/feature-ownership.md`, plus integration tests when the change
  crosses command boundaries.
- conaryd changes: `cargo test -p conaryd` focused to the touched route or job
  module, plus compatibility tests for request/response shapes.
  If method/path routing changes, update the route block in
  `docs/modules/conaryd.md` and run `bash scripts/check-doc-truth.sh`.
- Remi or MCP changes: `cargo test -p remi` or `cargo test -p conary-mcp`
  focused to the touched handler/adapter, plus catalog proof when operation
  visibility changes.
- Docs-only honesty changes: docs-audit checks where relevant,
  `git diff --check`, stale path/placeholder sweeps, and ledger consistency
  review.
- Tracked Markdown changes: run
  `bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete`
  and
  `bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -`.
- Coherency ledger changes: add a validation script, such as
  `scripts/check-coherency-ledger.sh`, with basic checks for the header,
  duplicate IDs, valid ID namespaces, valid status and disposition values,
  closure-matrix compatibility, single-line TSV fields, valid `last_verified`
  dates, accepted source-pointer prefixes, existing `path:` and `doc:`
  references, stale-date warnings, and scope-completion checks before the first
  ledger is treated as durable.

Broad gates such as `cargo clippy --workspace --all-targets -- -D warnings`
and `cargo fmt --check` remain end-of-branch or high-risk verification gates,
not mandatory after every small ledger row.

## Done Criteria

The design is successful when:

- the first ledger exists and is populated by evidence, not vibes;
- each first-wave finding has a wave scope, status, decision, `actual_or_gap`
  summary, next slice, and verification;
- bounded `fix-now`, `misleading`, and `duplicate-stale` rows are repaired,
  merged, removed, or reclassified to verified honest deferral rather than
  parked;
- active docs/help no longer overstate known unsupported surfaces discovered in
  the wave;
- deferred rows are honest, specific, and owned;
- capability, owner-file, command-route, or "look here first" changes are
  reflected in `docs/modules/feature-ownership.md` and
  `docs/llms/subsystem-map.md` in the same slice;
- the documentation accuracy audit inventory and ledger remain in sync for any
  tracked Markdown added, moved, archived, or deleted by the wave;
- the follow-up implementation plan can execute one slice at a time without
  re-litigating the whole program.

## Implementation Planning Notes

The implementation plan should start with the CLI help, dispatch routing,
generated manpage, and CLI-relevant active-doc claim inventory plus ledger
scaffolding, then execute one bounded repair wave. HTTP, MCP, and conaryd
public routes are out of Wave 1 unless a CLI command in the selected scope
directly advertises or depends on that route. The plan should prefer small
commits grouped by coherent surface area:

- ledger scaffold and inventory script or manual extraction notes;
- `scripts/check-coherency-ledger.sh` or an equivalent basic integrity gate;
- first public-surface audit rows;
- immediate repairs for bounded misleading claims;
- verification and ledger closure updates.

The plan should keep architecture duplication findings tied to concrete
surfaces. If a duplication concern is real but not user-visible in the first
wave, record it as a later `duplicate-stale` or `works-but-thin` row with a
specific owner and proof target rather than expanding the first wave.

## Appendix: Long-Context Review Packets

An external long-context reviewer, including GPT-5.5 Pro, can be used on
bounded packets after Codex has assembled current repo evidence.

Each packet should include:

- the selected scope and non-goals;
- relevant help output, docs excerpts, and owner file paths;
- route or dispatch snippets when command wiring matters;
- known `TODO`, `stub`, `not implemented`, `future`, and `unsupported` hits;
- the expected output format: findings with surface, claim, actual behavior,
  risk, proposed decision, and proof needed.

Do not ask the model for hidden reasoning. Ask for findings, contradictions,
duplication, missing proof, and concise rationale. Treat the model's answer as
review input, not repository truth, until Codex verifies it locally.
