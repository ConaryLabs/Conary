# Release Hardening Checklist - 2026-04-10

## Scope

Execution artifact root:
- `/home/peter/.claude/tmp/conary-release-hardening-2026-04-10`

Execution note:
- Planned `/tmp/conary-release-hardening-2026-04-10` workspace could not be used for file writes in this environment because `tee` failed with `Disk quota exceeded`; use the artifact root above for this pass.

Untouched worktree baseline:

```text
?? all_mds.txt
?? test_untracked.md
```

Untracked file dispositions:
- `all_mds.txt`: approved to ignore during release hardening
- `test_untracked.md`: approved to ignore during release hardening

## Phase 1 Release Matrix

| track | current_tag | next_version | next_tag | bundle_name | deploy_mode | decision | notes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| conary | v0.7.0 | 0.8.0 | v0.8.0 | release-bundle | release_bundle | candidate | canonical history baseline matches owned manifest baseline |
| remi | remi-v0.5.0 | 0.6.0 | remi-v0.6.0 | remi-bundle | remote_bundle | candidate | history baseline is derived from legacy `server-v*` tags |
| conaryd | conaryd-v0.5.0 | 0.6.0 | conaryd-v0.6.0 | conaryd-bundle | remote_bundle | candidate | no prior canonical tags; history baseline fell back to `0.0.0` while owned manifest baseline remained `0.5.0` |
| conary-test | conary-test-v0.7.0 | 0.8.0 | conary-test-v0.8.0 | conary-test-bundle | none | candidate | deploy intentionally skipped because `deploy_mode=none` |

## Local Gates

- Rust format/lint: `pass`
  `cargo fmt --check` passed immediately. `cargo clippy --workspace --all-targets -- -D warnings` initially failed on:
  - `apps/conary-test/src/handlers.rs` with `clippy::items_after_test_module`
  - `apps/remi/src/server/handlers/self_update.rs` with `clippy::field_reassign_with_default`
  Both issues were fixed and the workspace clippy gate then passed cleanly.
- Release builds: `pass`
  - `cargo build -p conary --release`
  - `cargo build -p remi --release`
  - `cargo build -p conaryd --release`
  - `cargo build -p conary-test --release`
- Owning-package tests: `pass`
  - `cargo test -p conary`
  - `cargo test -p conary-core`
  - `cargo test -p remi`
  - `cargo test -p conaryd`
  - `cargo test -p conary-test`
- `conary-test -- list`: `pass`
  Printed the expected suite inventory from Phase 1 through Phase 4.
- `site` validation: `pass`
  `npm ci`, `npm run check`, and `npm run build` all passed when rerun outside the sandbox. Inside the sandbox, `esbuild` failed with `EPERM`, so this was treated as an execution-environment limitation rather than a repo blocker.
- `web` validation: `pass`
  `npm ci`, `npm run check`, and `npm run build` passed when rerun outside the sandbox. `npm run check` initially failed on `web/src/routes/packages/[distro]/[name]/+page.svelte` because the canonical lookup response was untyped and the `filter` callback parameter became implicit `any`; fixed by adding shared frontend types and typing the page state/API response.

## Public-Surface Audit

- Grep sweep: `pass`
  Final targeted sweep used `0.7.0|0.8.0|0.5.0|0.6.0|version-|Release |Conary is a ` across `README.md`, `site`, `web`, and `apps/conary/man`, with results captured in `/home/peter/.claude/tmp/conary-release-hardening-2026-04-10/release-surface-grep.txt`.
  Actionable release-facing hits were:
  - `README.md` version badge and project-status version callout
  - `site/src/routes/install/+page.svelte` sample `conary --version` output
  - `site/src/routes/compare/+page.svelte` early-release wording
  - `apps/conary/man/conary.1` local generated manpage version string
  Non-blocking noise:
  - `site/package-lock.json` and `web/package-lock.json` `0.6.0` hits were dependency metadata, not public release copy
  - `web/src/routes/packages/[distro]/[name]/+page.svelte` `version-` hits were CSS class names, not release claims
- Manual file review: `pass`
  Reviewed the named release-facing files from the plan:
  - `README.md`: updated stale top-level version badge and status section
  - `site/src/routes/install/+page.svelte`: updated stale sample CLI version
  - `site/src/routes/compare/+page.svelte`: updated stale release-version sentence
  - `apps/conary/man/conary.1`: reviewed and locally regenerated at `0.8.0`, but confirmed this file is ignored/generated under `.gitignore` rather than tracked in `HEAD`
  - `web/src/routes/+layout.svelte`: reviewed, no stale version or misleading release/install claim found
  - `web/src/routes/+page.svelte`: reviewed, no stale version or misleading release/install claim found
- Release-surface fixes: `pass`
  Updated release-facing copy to the planned `conary 0.8.0` public surface, then reran:
  - `cargo build -p conary --release`
  - `(cd site && npm run check && npm run build)` outside the sandbox
  Both passed cleanly.

## GitHub Dry-Run Rehearsal

- `release-build` runs:
- Artifact validation:
- Signature rehearsal:
- `deploy-and-verify` runs:

## Secrets And Environment Readiness

- Repo secrets:
- Production environment secrets:
- Usability confirmation:

## Blockers

- No active blockers.

## Fixes Made

- `apps/conary-test/src/handlers.rs`: moved the test module to the end of the file so `clippy::items_after_test_module` no longer blocks the workspace lint gate.
- `apps/remi/src/server/handlers/self_update.rs`: rewrote the test `ServerConfig` setup to use a struct literal with `..Default::default()` so `clippy::field_reassign_with_default` passes.
- `web/src/lib/types.ts`, `web/src/lib/api.ts`, and `web/src/routes/packages/[distro]/[name]/+page.svelte`: added a typed canonical lookup response and typed page state so the package detail page no longer fails `svelte-check` on an implicit-`any` callback.
- Frontend validation commands for `site` and `web` had to run outside the sandbox because `esbuild` execution inside the sandbox returned `EPERM`.
- `README.md`, `site/src/routes/install/+page.svelte`, and `site/src/routes/compare/+page.svelte`: refreshed stale tracked release-facing version strings from `0.7.0` to the planned `0.8.0` public release surface.
- `apps/conary/man/conary.1`: confirmed the generated local manpage now reflects `0.8.0`, but it is ignored/generated (`/apps/conary/man/`) rather than a tracked repo file.

## Release Decision

- Approved Tracks:
- Dropped Tracks:
- Blocked Tracks:
- Final Release Command:

## Final Commands

- Pending.
