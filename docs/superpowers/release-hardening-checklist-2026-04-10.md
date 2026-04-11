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
  - `conary`: run ID `24268944297`, conclusion `success`, artifact target `/home/peter/.claude/tmp/conary-release-hardening-2026-04-10/artifacts/conary`
  - `remi`: run ID `24269042684`, conclusion `success`, artifact target `/home/peter/.claude/tmp/conary-release-hardening-2026-04-10/artifacts/remi`
  - `conaryd`: run ID `24269042691`, conclusion `success`, artifact target `/home/peter/.claude/tmp/conary-release-hardening-2026-04-10/artifacts/conaryd`
  - `conary-test`: run ID `24269042766`, conclusion `success`, artifact target `/home/peter/.claude/tmp/conary-release-hardening-2026-04-10/artifacts/conary-test`
- Artifact validation:
  - `conary`: `fail` — release metadata reports `product=conary`, `version=0.8.0`, `tag_name=v0.8.0`, `bundle_name=release-bundle`, `deploy_mode=release_bundle`, `dry_run=true`, but the downloaded bundle files are still built as `0.7.0` artifacts: `conary-0.7.0.ccs`, `conary-0.7.0-1.fc43.x86_64.rpm`, `conary_0.7.0-1_amd64.deb`, and `conary-0.7.0-1-x86_64.pkg.tar.zst`
  - `conary`: checksum verification `pass` — `sha256sum -c SHA256SUMS` returned `OK` for every file in `release-bundle`, but the checksums only prove internal consistency of the stale `0.7.0` bundle
  - `remi`: `fail` — bundle filenames and metadata align on `0.6.0`, but the downloaded binary itself reports `remi 0.5.0` when executed with `--version`, proving the dry-run built old code and only renamed the output file
  - `conaryd`: `fail` — bundle filenames and metadata align on `0.6.0`, but the downloaded binary reports `conaryd 0.5.0` under `--version`
  - `conary-test`: `fail` — bundle filenames and metadata align on `0.8.0`, but the downloaded binary reports `conary-test 0.7.0` under `--version`
  - Dry-run rehearsal conclusion: the current `workflow_dispatch` `release-build` path is not a truthful version rehearsal for any track on `main`; it serializes future release metadata while building binaries/packages from the pre-release source tree
- Signature rehearsal:
  - `release-build` bundle job log for `conary` confirms the signing step ran in dry-run mode with an empty `RELEASE_SIGNING_KEY` and printed `[DRY RUN] RELEASE_SIGNING_KEY not set; skipping signature generation`; no `*.sig` file was produced in the downloaded bundle
  - pre-existing repo fact still applies: `crates/conary-core/src/self_update.rs` declares `TRUSTED_UPDATE_KEYS = &[]`
  - `cargo run -p conary -- self-update --help` exposes only the networked `self-update` flow plus `--no-verify`; there is no repo-supported offline operator command to feed a downloaded `sha256` and detached `.sig` into verification without writing new code
  - outcome: `signature rehearsal incomplete`
- `deploy-and-verify` runs:
  - `conary`: run ID `24269729475`, conclusion `success`, `source_run=24268944297`; job graph confirms `resolve`, `validate-routing`, and `verify-conary` succeeded while deploy jobs stayed skipped in `dry_run=true`
  - `remi`: run ID `24269286949`, conclusion `success`, `source_run=24269042684`; job graph confirms `resolve`, `validate-routing`, and `verify-remi` succeeded while deploy jobs stayed skipped in `dry_run=true`
  - `conaryd`: run ID `24269180924`, conclusion `success`, `source_run=24269042691`; job graph confirms `resolve`, `validate-routing`, and `verify-conaryd` succeeded while deploy jobs stayed skipped in `dry_run=true`
  - `conary-test`: intentionally excluded because `deploy_mode=none`
  - Important limit: these deploy rehearsals only validated routing and artifact plumbing against the serialized metadata and filenames; they did not catch the binary-version skew documented above

## Secrets And Environment Readiness

- Repo secrets:
  - Direct `gh secret list` and `gh api repos/ConaryLabs/Conary/actions/secrets` inspection from this session shows exactly one visible repo-level Actions secret: `REMI_SSH_KEY`
- Production environment secrets:
  - Direct `gh secret list --env production` and `gh api repos/ConaryLabs/Conary/environments/production/secrets` inspection shows the `production` environment exists, but this session can see zero environment secrets there
- Usability confirmation:
  - `RELEASE_SIGNING_KEY`: `not confirmed`
  - `REMI_SSH_KEY`: `verified directly`
  - `REMI_SSH_TARGET`: `not confirmed` (workflow has a default fallback target, but no secret-backed confirmation was available from this session)
  - `CONARYD_SSH_KEY`: `not confirmed`
  - `CONARYD_SSH_TARGET`: `not confirmed`
  - `CONARYD_VERIFY_URL`: `not confirmed`
  - `gh secret list --org ConaryLabs` returned `HTTP 403`, so organization-level secret inheritance could not be inspected from this session
  - per the plan rule, any `not confirmed` entry keeps the release `no-go`

## Blockers

- `release-build` dry-run rehearsal is not version-truthful on `main`: all candidate tracks produced artifact version skew between serialized release metadata and the actual packaged binaries/packages
- `conary` self-update signature rehearsal is incomplete: dry-run skipped `.sig` generation because `RELEASE_SIGNING_KEY` was unset, `TRUSTED_UPDATE_KEYS` is still empty in the repo, and there is no supported offline verification path to exercise without new code
- Required live-release secrets are missing or unconfirmed from this session: `RELEASE_SIGNING_KEY`, `REMI_SSH_TARGET`, `CONARYD_SSH_KEY`, `CONARYD_SSH_TARGET`, and `CONARYD_VERIFY_URL`

## Fixes Made

- `apps/conary-test/src/handlers.rs`: moved the test module to the end of the file so `clippy::items_after_test_module` no longer blocks the workspace lint gate.
- `apps/remi/src/server/handlers/self_update.rs`: rewrote the test `ServerConfig` setup to use a struct literal with `..Default::default()` so `clippy::field_reassign_with_default` passes.
- `web/src/lib/types.ts`, `web/src/lib/api.ts`, and `web/src/routes/packages/[distro]/[name]/+page.svelte`: added a typed canonical lookup response and typed page state so the package detail page no longer fails `svelte-check` on an implicit-`any` callback.
- Frontend validation commands for `site` and `web` had to run outside the sandbox because `esbuild` execution inside the sandbox returned `EPERM`.
- `README.md`, `site/src/routes/install/+page.svelte`, and `site/src/routes/compare/+page.svelte`: refreshed stale tracked release-facing version strings from `0.7.0` to the planned `0.8.0` public release surface.
- `apps/conary/man/conary.1`: confirmed the generated local manpage now reflects `0.8.0`, but it is ignored/generated (`/apps/conary/man/`) rather than a tracked repo file.

## Release Decision

- Approved Tracks: none
- Dropped Tracks: none
- Blocked Tracks:
  - `conary`: blocked by version-skewed dry-run artifacts, incomplete signature rehearsal, and unconfirmed `RELEASE_SIGNING_KEY`
  - `remi`: blocked by dry-run binary-version mismatch and unconfirmed live deploy target readiness
  - `conaryd`: blocked by dry-run binary-version mismatch and unconfirmed deploy secrets
  - `conary-test`: blocked by dry-run binary-version mismatch
- Final Release Command: do not cut; release is `no-go`

## Final Commands

- No live release command executed. The hardening pass stopped before any irreversible tag push or production deploy.
