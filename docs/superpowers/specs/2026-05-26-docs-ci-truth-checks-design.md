---
last_updated: 2026-05-26
revision: 2
summary: Design for Plan C docs, CI, and public-surface truth checks
---

# Docs And CI Truth Checks: Design Spec

**Date:** 2026-05-26
**Status:** Implementation plan written in `docs/superpowers/plans/2026-05-26-docs-ci-truth-checks.md`
**Umbrella:** `docs/superpowers/specs/2026-05-25-preview-invariant-hardening-design.md`
**Goal:** Make active documentation, command hints, daemon route references, and
preview status claims cheap to check against the current repository state.

---

## Purpose

Plan C closes the remaining track in the preview invariant hardening milestone.
Plans A and B made runtime safety invariants more explicit. Plan C makes the
supporting documentation and CI surfaces hostile to drift.

The target is not a broad documentation rewrite. The target is a small set of
mechanical checks that fail when active docs or app strings claim facts the code
does not support: stale database schema versions, retired command spellings,
preview status contradictions, conaryd route drift, and fail-closed
authorization stubs described as implemented features.

## Current Repo Facts

- `crates/conary-core/src/db/schema.rs` currently exposes `SCHEMA_VERSION: 69`.
- Active docs now mention schema v69 in `docs/ARCHITECTURE.md` and
  `docs/conaryopedia-v2.md`, but prior reviews found this class of drift
  repeatedly.
- `scripts/docs-audit-inventory.sh` and
  `scripts/check-doc-audit-ledger.sh` already define the tracked documentation
  inventory and ledger contract, but `.github/workflows/pr-gate.yml` does not
  run them.
- `.github/workflows/pr-gate.yml` already has small policy jobs for GitHub
  Actions runtime pins and release-matrix alignment. Plan C should follow that
  style: a cheap, explicit job with actionable errors.
- `apps/conaryd/src/daemon/routes/*.rs` contains the actual conaryd v1 route
  surface. README currently points readers at the Conaryopedia for the full REST
  endpoint list, but conaryd deserves a smaller maintained endpoint reference
  because Remi's route surface is much larger and separate.
- `apps/conaryd/src/daemon/auth.rs` safely denies PolicyKit authorization
  attempts today, but its module docs still say non-root users can be authorized
  via PolicyKit. That is a fail-closed implementation with overconfident prose.
- `crates/conary-core/src/lib.rs` exposes many modules directly for workspace
  convenience, and `crates/conary-core/Cargo.toml` does not currently set
  `publish = false`. The umbrella track requires a documented public-surface
  decision and a package-metadata guard rather than an immediate facade
  refactor.
- The completed Plan B docs are active, and the umbrella still needs a final
  lifecycle pass after Plan C lands.

## Decision

Implement Plan C as a strict truth gate:

1. Add `scripts/check-doc-truth.sh`.
2. Add an always-on `docs-truth` job to `.github/workflows/pr-gate.yml`.
3. Run the existing docs-audit ledger check, docs-audit inventory diff, and the
   new truth script on every PR.
4. Add or refresh the small canonical docs and package metadata needed for the
   checks, especially conaryd endpoints and `conary-core` public-surface status.
5. Archive the completed umbrella and Plan A/B/C docs only after Plan C
   implementation lands and the docs-audit inventory/ledger are updated in the
   same change.

The new checks should be explicit and intentionally narrow. They should not try
to parse arbitrary English. They should encode known invariants that have
already drifted or are likely to drift.

## Truth-Check Script

Plan C should create `scripts/check-doc-truth.sh` with one clear failure style:

```text
ERROR: <path>: <specific stale or unsupported claim>
```

The script should run from the repository root, use only common CI tools already
available in the workflow (`bash`, `rg`, `sed`, `awk`, `diff`), and avoid
network access.

### Check Scopes

The script should use explicit path sets per check, not one broad regex over
every tracked Markdown file. This keeps the checks actionable and avoids
flagging planning docs that quote stale text as examples of what the truth gate
should reject.

All checks should exclude historical archive paths by default:

- `docs/superpowers/*/archive/**`
- `docs/superpowers/reviews/archive/**`
- `docs/plans/archive/**`
- `docs/llms/archive/**`
- `recipes/archive/**`

Product-truth checks should primarily target user, contributor, and operator
docs such as README, ROADMAP, `docs/ARCHITECTURE.md`,
`docs/conaryopedia-v2.md`, `docs/modules/*.md`, and
`docs/operations/*.md`. Planning specs and implementation plans may be checked
by specific rules only when those rules name them explicitly.

The script should also search code comments and app strings where the claim is
directly operator-facing, parser-relevant, or security-relevant, such as conaryd
auth module docs, CLI hint text, and Clap parser definitions.

### Schema Version Drift

The script should parse the current DB schema version from:

```text
crates/conary-core/src/db/schema.rs::SCHEMA_VERSION
```

It should scan only a small allowlist of active-schema declaration docs for
Conary DB schema mentions. The initial allowlist should be stored near the top
of the script as `DOCS_TRUTH_SCHEMA_CHECK_PATHS` and include:

- `docs/ARCHITECTURE.md`
- `docs/conaryopedia-v2.md`

Those files should be checked for mentions such as:

- `schema vNN`
- `Schema vNN`
- `currently schema vNN`
- `schema version NN`

The script should fail when an allowlisted active-schema declaration path
mentions a Conary DB schema version other than the code value.

The schema check should not broadly scan all active docs. Changelogs,
implementation plans, migration notes, tests, and design specs may legitimately
mention older schema migrations. JSON Schema references, Remi test fixture
schema versions, OpenAPI schema references, and historical docs are also out of
scope for this check unless a future maintainer explicitly adds a path to
`DOCS_TRUTH_SCHEMA_CHECK_PATHS`.

### Retired Command Fossils

The script should fail when active product docs, active operator-facing app
strings, or Clap parser definitions recommend or accept retired command
spellings, including:

- `adopt-system`
- `conary adopt`
- `conary-adopt`
- `system-adopt`

The parser check should include `apps/conary/src/cli/**`,
`apps/conary/src/dispatch.rs`, and `apps/conary/src/command_risk.rs` so hidden
aliases or compatibility shims cannot silently reintroduce retired spellings.

It should allow historical archives, the truth-check script itself, and tests
or fixtures that intentionally assert a retired spelling is rejected. Current
active user docs should use `conary system adopt ...`.

### Preview Status Invariants

The script should enforce a small set of known preview-status invariants:

- `README.md` and `ROADMAP.md` both describe the preview as adoption-led.
- `README.md`, `ROADMAP.md`, and `docs/INTEGRATION-TESTING.md` agree that
  remote Forge validation is paused pending a KVM-capable runner.
- `README.md`, `ROADMAP.md`, and `docs/INTEGRATION-TESTING.md` describe Group O
  and Group P QEMU evidence as dated local evidence, not as a universal claim
  that every current run is always green.

These checks should use anchored phrases rather than broad sentiment analysis.
If the wording changes intentionally, the phrase list should be updated in the
same commit.

### PolicyKit Stub Honesty

The script should fail when active docs or `apps/conaryd/src/daemon/auth.rs`
claim non-root PolicyKit authorization is implemented today.

Acceptable active wording should say that PolicyKit authorization is currently
stubbed, unavailable, or fail-closed until a real DBus check and policy-file
contract exist. The implementation should keep the safe runtime behavior:
non-root write authorization is denied unless the code later implements a real
PolicyKit check.

The check should also inspect `apps/conaryd/src/daemon/mod.rs` and confirm the
current `DaemonConfig::default()` value keeps `require_polkit: true`. If that
default changes, the docs-truth check should fail until daemon auth docs and
operator docs are updated to describe the new behavior explicitly.

### conaryd Route Truth

Plan C should add `docs/modules/conaryd.md` as the canonical maintained conaryd
route reference. README should point to that focused route list instead of a
broad Conaryopedia endpoint section for conaryd.

The route truth check should compare documented conaryd routes with method-aware
route pairs extracted from:

- `apps/conaryd/src/daemon/routes/system.rs`
- `apps/conaryd/src/daemon/routes/transactions.rs`
- `apps/conaryd/src/daemon/routes/query.rs`
- `apps/conaryd/src/daemon/routes/events.rs`

The canonical docs should list routes as `METHOD /path` pairs, not bare paths.
This matters because routes such as `/v1/transactions` and
`/v1/transactions/{id}` support multiple methods with different semantics.

The comparison should account for the `/v1` nesting applied by
`apps/conaryd/src/daemon/routes.rs`, while leaving the root `/health` route
unnested. The docs should also describe the auth boundary: `/health` is outside
the v1 auth middleware, while `/v1/*` routes are behind the v1 gate.

The extraction should fail loudly if it finds too few routes. The current route
surface has 25 method/path pairs:

- `GET /health`
- `GET /v1/version`
- `GET /v1/metrics`
- `GET /v1/system/states`
- `POST /v1/system/rollback`
- `POST /v1/system/verify`
- `POST /v1/system/gc`
- `GET /v1/transactions`
- `POST /v1/transactions`
- `POST /v1/transactions/dry-run`
- `GET /v1/transactions/{id}`
- `DELETE /v1/transactions/{id}`
- `GET /v1/transactions/{id}/stream`
- `POST /v1/packages/install`
- `POST /v1/packages/remove`
- `POST /v1/packages/update`
- `POST /v1/enhance`
- `GET /v1/packages`
- `GET /v1/packages/{name}`
- `GET /v1/packages/{name}/files`
- `GET /v1/search`
- `GET /v1/depends/{name}`
- `GET /v1/rdepends/{name}`
- `GET /v1/history`
- `GET /v1/events`

Plan C's initial script should require at least 25 extracted method/path pairs.
If the router structure changes, a zero-route or partial extraction should fail
instead of producing a false sense of coverage.

The maintained docs should distinguish implemented routes from preview-stubbed
routes. Current system routes such as `/v1/system/states`,
`/v1/system/rollback`, `/v1/system/verify`, and `/v1/system/gc` should be listed
as real routes whose handlers return preview-not-implemented responses today.
Implementation status should be verified from the current handler behavior, not
from older changelog notes. For example, package job routes should be documented
according to the current `transactions.rs` handlers, not stale prose that may
still claim they return 501.

Plan C should not attempt to document or truth-check Remi's much larger route
surface. Remi API truth checks can be a future, separate plan if needed.

### Docs-Audit Bookkeeping

The PR gate should run:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv <(bash scripts/docs-audit-inventory.sh)
```

This makes inventory and ledger drift a normal CI failure rather than a manual
wrap-up chore.

The inventory diff is intentionally strict for every path tracked by
`scripts/docs-audit-inventory.sh`, including root-level docs, module docs,
planning docs, app-local docs, and frontend docs. Adding, moving, or deleting a
tracked doc should update the committed inventory and ledger in the same change.

## CI Contract

Plan C should add a dedicated `docs-truth` job to
`.github/workflows/pr-gate.yml`. The job should run on every PR and manual
dispatch, like the existing runtime-policy and release-matrix jobs.

The job should:

1. check out the repository;
2. run the docs-audit ledger check in `--require-complete` mode;
3. run the docs-audit inventory diff;
4. run `bash scripts/check-doc-truth.sh`.

The job should not require Rust setup, cargo builds, or network access.

## Documentation Updates

Plan C should make the narrow docs updates required by the truth gate:

- Correct conaryd auth module docs so PolicyKit is described as unimplemented
  and fail-closed.
- Add `docs/modules/conaryd.md` as the focused conaryd endpoint and daemon
  behavior reference.
- Update README's conaryd endpoint pointer to the focused module doc.
- Document in `docs/ARCHITECTURE.md` and `crates/conary-core/src/lib.rs` that
  `conary-core` is currently an internal workspace crate with broad public
  modules for workspace convenience, not a stable external API facade.
- Set `publish = false` in `crates/conary-core/Cargo.toml` so the internal
  workspace-crate decision has a Cargo-level guard.
- Update the umbrella hardening spec status once Plan C is split out, and
  archive completed Plan A/B/C and umbrella docs after Plan C implementation
  lands.

## conary-core Public Surface Decision

For this milestone, the decision is:

`conary-core` is an internal workspace crate. Its broad module exports exist for
workspace app convenience and integration-test reuse. They are not a stable
external API promise.

Plan C should document this plainly and set `publish = false` in
`crates/conary-core/Cargo.toml`. It should not hide modules, introduce a curated
facade, or break workspace callers. A facade refactor can become a future design
only when a concrete invariant or downstream compatibility need requires it.

The truth script should also guard against active user/operator docs promising a
stable external `conary-core` API. A negative phrase check is enough for Plan C:
fail on active docs that describe `conary-core` as a stable public API, stable
SDK, or external library contract unless the same line clearly says the crate is
internal or unstable.

## Error Handling

The truth script should fail closed for ambiguous matches in active docs. If a
line looks like a stale Conary DB schema version or retired command hint, the
script should print the file and matching line, then exit non-zero.

False positives should be resolved by narrowing the script's active-doc scope or
allowlist with comments explaining why the ignored phrase is not a live product
claim. The script should not silently ignore whole directories beyond the
historical archive paths listed above.

Unknown conaryd route parsing cases should fail with a message asking the
implementer to update either the route extraction helper or the canonical
endpoint list. A route can be documented as preview-stubbed, but it should not
be absent.

The implementation plan should include lightweight self-tests for the truth
script itself. Fixture tests should prove that known-bad schema claims,
retired-command claims, PolicyKit overclaims, missing conaryd routes, and stable
`conary-core` API claims fail, while known-good snippets pass.

## Acceptance Criteria

- `.github/workflows/pr-gate.yml` has an always-on `docs-truth` job.
- CI runs docs-audit ledger, docs-audit inventory, and the new truth script.
- `scripts/check-doc-truth.sh` fails on active-doc Conary DB schema drift.
- The truth script fails on retired adoption command spellings in active docs or
  active operator-facing strings.
- The truth script fails on retired adoption command spellings or hidden aliases
  in active Clap parser source.
- The truth script fails when active docs contradict the agreed preview status
  invariants.
- The truth script fails when active docs or conaryd auth module docs claim
  PolicyKit write authorization works today.
- The truth script fails if `DaemonConfig::default()` no longer keeps
  `require_polkit: true` without updated auth/operator documentation.
- `docs/modules/conaryd.md` lists the current conaryd route surface and marks
  preview-stubbed system routes as not implemented.
- The route truth check compares method/path pairs, documents the `/health`
  versus `/v1/*` auth boundary, and fails when conaryd routes and the maintained
  endpoint list drift.
- README points conaryd endpoint readers at the focused conaryd module doc.
- `conary-core` is documented as an internal workspace crate without claiming a
  stable external public API.
- `crates/conary-core/Cargo.toml` sets `publish = false`.
- Truth-script fixture tests cover representative failing and passing cases.
- Docs-audit inventory and ledger remain complete after adding, moving, or
  archiving Plan C docs.

## Out Of Scope

- No package-manager feature work.
- No QEMU or Forge validation rerun.
- No schema migration.
- No generated OpenAPI pipeline.
- No broad `conary-core` facade refactor.
- No Remi API route truth-check sweep.
- No attempt to make shell scripts understand arbitrary prose.
- No weakening of docs-audit inventory strictness for tracked documentation
  paths.

## Verification Strategy

The Plan C implementation should end with:

```bash
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv <(bash scripts/docs-audit-inventory.sh)
bash scripts/check-doc-truth.sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p conary-test -- list
git diff --check
```

Focused development checks should run the new truth script after each check
family is added.

## Review Questions

External reviewers should not rubberstamp this design. They should specifically
check:

- whether a Bash truth script is strong enough for the conaryd route comparison,
  or whether a Rust/Python helper is worth the extra surface;
- whether the explicit schema declaration path allowlist is too narrow or still
  likely to false-positive;
- whether method-aware conaryd route extraction plus a route-count floor is
  enough to prevent silent route-parser misses;
- whether the preview-status invariants are too brittle or too weak;
- whether the PolicyKit honesty check catches the important overclaim without
  blocking accurate future implementation docs;
- whether `docs/modules/conaryd.md` is the right canonical endpoint location;
- whether the `conary-core` internal-crate decision plus `publish = false` is
  enough without a facade refactor;
- whether the always-on PR gate is acceptably cheap.
