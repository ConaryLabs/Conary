# Documentation Accuracy Audit Summary

## Scope

This refresh updates the documentation baseline after the limited public
release readiness pass and the completed composefs atomic switching
modernization, then refreshes the Adopt Without Regret docs/integration proof
slice, records the Native Package Manager Parity Matrix design, and catches the
active Slice A/Slice B/Slice C parity plans plus the Slice B preview-doc
refresh.

Audited every tracked documentation-like file returned by
`bash scripts/docs-audit-inventory.sh`: 79 tracked files spanning root docs,
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
- composefs atomic switching completion, with ordinary package mutation and
  transaction recovery selecting complete generation artifacts for the next
  boot instead of relying on live generation remounts
- Adopt Without Regret preview framing, with non-destructive unadoption,
  native package-manager authority, explicit takeover boundaries, and
  active-generation handoff limits documented for the limited preview
- Native Package Manager Parity Matrix framing for Tier 0 and Tier 1
  Conary-owned package-manager flows against dnf, apt, and pacman expectations
- Slice A, Slice B, and Slice C native package-manager parity implementation records,
  including no-generation install/remove/update proof, adopted-package native
  authority, explicit takeover, security-advisory support refusal, and the
  planned Tier 1 daily-driver command parity work

## Major Corrections

- Refreshed user-facing status in `README.md` and `ROADMAP.md` around the
  Fedora 44, Ubuntu 26.04 LTS, and Arch Linux adoption-led limited preview;
  native package-manager authority for adopted packages; the non-destructive
  `system unadopt --all` escape hatch; explicit takeover; Conary-owned updates
  on mutable live roots; security-update advisory support honesty;
  active-generation handoff limits; local QEMU/security gates; raw/qcow2
  generation export; OCI artifact-source alignment; and ISO/bundle follow-ups.
- Updated `SECURITY.md` to 0.8.x support and replaced stale journal language
  with the current database/EROFS generation model and preview distro scope.
- Updated deploy/operator docs and `deploy/remi.toml.example` for Fedora 44,
  Ubuntu 26.04 LTS, Remi admin-origin access, current host assumptions, and
  Forge/conaryd paths.
- Updated `docs/ARCHITECTURE.md` and `docs/conaryopedia-v2.md` for schema v67,
  runtime generation input validation, LFS 13.0 bootstrap phases, and
  self-contained generation export; refreshed them again after composefs atomic
  switching landed so transaction and recovery wording reflects next-boot
  selection through `/conary/current`; then refreshed the package-update,
  adoption, unadoption, repository advisory-support, and explicit-takeover
  command guidance after Slice B landed.
- Updated `CHANGELOG.md`, `CONTRIBUTING.md`, and `SECURITY.md` so public and
  developer-facing docs describe rebuild/reselect semantics rather than older
  live remount recovery language.
- Updated site install/features/compare copy to remove Debian as a public
  support claim and to clarify that non-x86_64 generation boot assets remain
  follow-up work while OCI export source loading has moved onto the generation
  artifact contract.
- Updated conaryd CLI help and public docs so package install/remove/update
  routes are described as explicit `501 Not Implemented` responses.
- Updated integration docs and `apps/conary-test/README.md` to include the
  Phase 1 `T21a`-`T21c` non-destructive unadoption proof, focused live-root
  update/security-refusal proof, Phase 3 Group O generation-export QEMU suite,
  temporary local QEMU release gate, and the TGE04 installed-runtime qcow2 boot
  proof.
- Refreshed assistant-facing docs to route broad doc work through the inventory
  and ledger checker, and added the post-generation export roadmap to the map.
- Reframed completed dated plans/specs as historical implementation records
  and archived the completed 2026-05-12 composefs modernization plan/spec after
  validation landed on `main`.
- Added the Native Package Manager Parity Matrix design and active Slice A/B
  implementation plans for Conary-owned install/remove/update and adjacent
  daily package-manager commands.

## Archive Decisions

- Existing archive docs and recipe READMEs were retained as historical
  reference material.
- Completed top-level superpowers plans were moved to
  `docs/superpowers/plans/archive/`; the limited public release readiness plan
  remains active at the top level, and the completed composefs atomic
  modernization plan/spec moved to archive paths.
- No tracked documentation files were deleted.

## Verification Commands

- `bash scripts/docs-audit-inventory.sh`
- `bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete`
- `bash -n scripts/docs-audit-inventory.sh scripts/check-doc-audit-ledger.sh`
- `cargo test -p conary security_update -- --nocapture`
- `cargo run -p conary -- update --help`
- `cargo run -p conary-test -- list`
- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `git diff --check`
- `rg -n "replace dnf|replace apt|replace pacman|risk-free|unadopt|takeover" README.md ROADMAP.md docs`
- `rg -n "rebuild or remoun[t]|MOUNTE[D]|refresh the Fedora 44 QEMU source imag[e]|active execution pla[n]|active umbrella desig[n]|remain active at the top leve[l]" README.md ROADMAP.md CHANGELOG.md CONTRIBUTING.md SECURITY.md AGENTS.md CLAUDE.md GEMINI.md docs apps/conary-test deploy site web .github`
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
- Active-generation handoff back to native package-manager authority remains a
  follow-up; first-slice unadoption intentionally fails closed when a Conary
  generation is selected.

## Final Counts

- Total tracked doc-like files audited: 79
- `verified-no-change`: 19
- `corrected`: 33
- `reframed-as-historical`: 5
- `archived`: 8
- `retained-historical`: 14
- Remaining pending rows: 0
