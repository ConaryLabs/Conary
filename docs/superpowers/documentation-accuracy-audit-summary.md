# Documentation Accuracy Audit Summary

## Scope

This refresh updates the documentation baseline after the previous full audit
closed at `d65a5651 docs: complete documentation accuracy audit`.

Audited every tracked documentation-like file returned by
`bash scripts/docs-audit-inventory.sh`: 65 tracked files spanning root docs,
assistant shims, GitHub templates, canonical docs under `docs/`,
deploy/operator docs, `deploy/remi.toml.example`, app-local READMEs, active
planning/design records, historical/archive docs, and the `site/`/`web`
frontend READMEs.

Ignored local or credential-bearing files were reviewed only as scan findings
and were not edited. That includes `deploy/.credentials.toml`,
`docs/operations/LOCAL_ACCESS.md`, and local ignored plan/spec archives.

## Why This Refresh Happened

The repo moved materially after the last audit:

- four-track release hardening and releases (`conary`, `remi`, `conaryd`,
  `conary-test`)
- conaryd Forge-local staging deployment
- bootstrap self-hosting VM flow with rootful handoff and QEMU validation
- generation artifact export unification
- self-contained installed-runtime generation exports with Fedora 44 QEMU
  validation
- Remi deploy/access and conversion-burst hardening
- schema v67 and architecture-aware conversion cache migration

## Major Corrections

- Refreshed user-facing status in `README.md` and `ROADMAP.md` around Fedora
  44, self-hosting VM validation, raw/qcow2 generation export, and ISO/OCI
  follow-ups.
- Updated `SECURITY.md` to 0.8.x support and replaced stale journal language
  with the current database/EROFS generation model.
- Updated deploy/operator docs and `deploy/remi.toml.example` for Fedora 44,
  Remi admin-origin access, current host assumptions, and Forge/conaryd paths.
- Updated `docs/ARCHITECTURE.md` and `docs/conaryopedia-v2.md` for schema v67,
  runtime generation input validation, LFS 13.0 bootstrap phases, and
  self-contained generation export.
- Updated integration docs and `apps/conary-test/README.md` to include the
  Phase 3 Group O generation-export QEMU suite and the TGE04 installed-runtime
  qcow2 boot proof.
- Refreshed assistant-facing docs to route broad doc work through the inventory
  and ledger checker, and added the post-generation export roadmap to the map.
- Reframed active dated plans/specs as historical implementation records so
  their old step-by-step instructions are not mistaken for current guidance.

## Archive Decisions

- Existing archive docs and recipe READMEs were retained as historical
  reference material.
- Completed but still tracked superpowers plans/specs were not moved in this
  pass; they now carry explicit historical banners and current-doc pointers.
- No tracked documentation files were deleted.

## Verification Commands

- `bash scripts/docs-audit-inventory.sh`
- `bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete`
- `bash -n scripts/docs-audit-inventory.sh scripts/check-doc-audit-ledger.sh`
- `cargo fmt --check`
- `cargo run -p conary-test -- list`
- `git diff --check`

The `conary-test` manifest listing includes `Generation Artifact Export QEMU`
with 4 tests.

## Residual Risks

- `deploy/.credentials.toml` and `docs/operations/LOCAL_ACCESS.md` contain
  local-only stale operational notes, but they are ignored/untracked local files
  and should not be normalized in public docs.
- A future hygiene pass can physically archive or untrack the newly completed
  dated superpowers plans/specs. This pass made them safe to read in place by
  adding historical framing.

## Final Counts

- Total tracked doc-like files audited: 65
- `verified-no-change`: 24
- `corrected`: 19
- `reframed-as-historical`: 8
- `retained-historical`: 14
- Remaining pending rows: 0
