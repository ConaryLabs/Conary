# Documentation Accuracy Audit Summary

## Scope

This refresh updates the documentation baseline after the limited public release
readiness pass.

Audited every tracked documentation-like file returned by
`bash scripts/docs-audit-inventory.sh`: 69 tracked files spanning root docs,
assistant shims, GitHub templates, canonical docs under `docs/`,
deploy/operator docs, `deploy/remi.toml.example`, app-local READMEs, active
planning/design records, historical/archive docs, release security waivers, and
the `site/`/`web` frontend READMEs.

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
- release-readiness dependency/security refresh
- Fedora 44, Ubuntu 26.04 LTS, and Arch Linux public-preview matrix alignment
- conaryd package route honesty (`501 Not Implemented` until executor support)
- live Remi metadata realignment to 26.04/`resolute` while keeping host OS
  claims separate from client distro support

## Major Corrections

- Refreshed user-facing status in `README.md` and `ROADMAP.md` around the
  Fedora 44, Ubuntu 26.04 LTS, and Arch Linux limited preview; Forge/security
  gates; conaryd package-executor gaps; raw/qcow2 generation export; and
  ISO/OCI follow-ups.
- Updated `SECURITY.md` to 0.8.x support and replaced stale journal language
  with the current database/EROFS generation model and preview distro scope.
- Updated deploy/operator docs and `deploy/remi.toml.example` for Fedora 44,
  Ubuntu 26.04 LTS, Remi admin-origin access, current host assumptions, and
  Forge/conaryd paths.
- Updated `docs/ARCHITECTURE.md` and `docs/conaryopedia-v2.md` for schema v67,
  runtime generation input validation, LFS 13.0 bootstrap phases, and
  self-contained generation export.
- Updated site install/features/compare copy to remove Debian as a public
  support claim and to clarify that generation-to-OCI export and non-x86_64
  generation boot assets remain follow-up work.
- Updated conaryd CLI help and public docs so package install/remove/update
  routes are described as explicit `501 Not Implemented` responses.
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
- Completed top-level superpowers plans were moved to
  `docs/superpowers/plans/archive/`, leaving only the limited public release
  readiness plan active at the top level.
- No tracked documentation files were deleted.

## Verification Commands

- `bash scripts/docs-audit-inventory.sh`
- `bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete`
- `bash -n scripts/docs-audit-inventory.sh scripts/check-doc-audit-ledger.sh`
- `cargo test -p conaryd test_package_routes_return_not_implemented`
- `cargo test -p conaryd package_routes`
- `cargo run -p conaryd -- --help`
- `npm run check` in `site/`
- `npm run build` in `site/`
- `npm audit --audit-level=moderate` in `site/`

The final release-readiness verification gate is tracked by
`docs/superpowers/plans/2026-05-06-limited-public-release-readiness-plan.md`.

## Residual Risks

- The live Remi repository rows and public metadata have been moved to
  26.04/`resolute`, but the root-owned production config file still needs an
  ops follow-up if we want the file contents to mirror the DB state.
- Historical release notes and archived design/spec files may still mention
  older distro names or broad parser support as historical context.

## Final Counts

- Total tracked doc-like files audited: 69
- `verified-no-change`: 21
- `corrected`: 23
- `reframed-as-historical`: 5
- `archived`: 6
- `retained-historical`: 14
- Remaining pending rows: 0
