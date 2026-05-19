# Documentation Accuracy Audit Summary

## Scope

This refresh updates the documentation baseline after the limited public
release readiness pass and the completed composefs atomic switching
modernization, then refreshes the Adopt Without Regret docs/integration proof
slice, records the Native Package Manager Parity Matrix design, and catches the
completed Slice A/Slice B/Slice C/Slice D parity records, the Slice B preview-doc
refresh, the focused Slice C daily-driver/provider-metadata proof, the Slice D
three-distro parity evidence, the 2026-05-16 limited-preview checkpoint, and the
2026-05-19 generation-export/security refresh.

Audited every tracked documentation-like file returned by
`bash scripts/docs-audit-inventory.sh`: 82 tracked files spanning root docs,
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
- Slice A, Slice B, Slice C, and Slice D native package-manager parity implementation records,
  including no-generation install/remove/update proof, adopted-package native
  authority, explicit takeover, security-advisory support refusal, and the
  focused Tier 1 daily-driver command parity, declared-provider matching work,
  and the three-distro conary-test matrix plan
- 2026-05-16 limited-preview checkpoint evidence, including refreshed
  adoption/unadoption proof and refreshed Group N QEMU proof
- 2026-05-19 refresh evidence, including the restored Group O `TGE04`
  installed-runtime export boot proof, the package-manager-only tester-post
  recommendation, and the removal of the `tough` trust-root dependency path

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
  temporary local QEMU release gate, refreshed Group N proof, the current
  2026-05-19 Group O installed-runtime/bootstrap-run boot proof, and the Phase 4
  native package-manager parity manifest.
- Added the 2026-05-16 limited-preview release checkpoint and refreshed
  README/ROADMAP/bootstrap/generation-export docs so a package-manager-only
  preview ask treats generation export as supporting evidence instead of the
  headline public promise.
- Refreshed assistant-facing docs to route broad doc work through the inventory
  and ledger checker, and added the post-generation export roadmap to the map.
- Reframed completed dated plans/specs as historical implementation records
  and archived completed implementation plans/specs after their validation
  evidence landed on `main`.
- Added the Native Package Manager Parity Matrix design and Slice A/B/C/D
  implementation records for Conary-owned install/remove/update, adjacent daily
  package-manager commands, and the three-distro parity matrix; then moved them
  to archive paths once the current integration docs carried the release
  evidence.

## Archive Decisions

- Existing archive docs and recipe READMEs were retained as historical
  reference material.
- Completed top-level superpowers plans were moved to
  `docs/superpowers/plans/archive/`.
- Completed design specs were moved to `docs/superpowers/specs/archive/` so
  active design directories no longer look like the next implementation queue.
- The current package-manager tester decision is represented by
  `docs/superpowers/limited-preview-release-checkpoint-2026-05-16.md` and
  `docs/superpowers/limited-preview-subreddit-tester-post-2026-05-19.md`, not
  by a broad top-level release plan.
- No tracked documentation files were deleted.

## Verification Commands

- `bash scripts/docs-audit-inventory.sh`
- `bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete`
- `bash -n scripts/docs-audit-inventory.sh scripts/check-doc-audit-ledger.sh`
- `cargo test -p conary security_update -- --nocapture`
- `cargo run -p conary -- update --help`
- `cargo run -p conary-test -- list`
- `cargo run -p conary-test -- run --suite phase1-advanced --distro fedora44 --phase 1`
- `cargo run -p conary-test -- run --suite phase1-advanced --distro ubuntu-26.04 --phase 1`
- `cargo run -p conary-test -- run --suite phase1-advanced --distro arch --phase 1`
- `cargo run -p conary-test -- run --suite phase3-group-n-qemu --distro fedora44 --phase 3`
- `cargo run -p conary-test -- run --suite phase3-group-o-generation-export --distro fedora44 --phase 3`
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

Current limited-preview evidence is tracked by
`docs/superpowers/limited-preview-release-checkpoint-2026-05-16.md`; the narrow
public tester copy is tracked by
`docs/superpowers/limited-preview-subreddit-tester-post-2026-05-19.md`.

## Residual Risks

- The live Remi repository rows and public metadata have been moved to
  26.04/`resolute`, but the root-owned production config file still needs an
  ops follow-up if we want the file contents to mirror the DB state.
- Historical release notes and archived design/spec files may still mention
  older distro names or broad parser support as historical context.
- Active-generation handoff back to native package-manager authority remains a
  follow-up; first-slice unadoption intentionally fails closed when a Conary
  generation is selected.
- Group O `TGE04` failed in the 2026-05-16 checkpoint, but the 2026-05-19
  refresh fixed the installed-runtime initramfs path and restored the Group O
  gate to 4 passed / 0 failed / 0 skipped / 0 cancelled.
- The previous `tough` advisory path has been removed from `Cargo.lock`; the
  remaining RustSec waiver is `RUSTSEC-2023-0071` for `rsa 0.9.10`.

## Final Counts

- Total tracked doc-like files audited: 82
- `verified-no-change`: 18
- `corrected`: 26
- `reframed-as-historical`: 1
- `archived`: 23
- `retained-historical`: 14
- Remaining pending rows: 0
