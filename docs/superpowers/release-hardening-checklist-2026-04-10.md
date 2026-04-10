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

- Rust format/lint:
- Release builds:
- Owning-package tests:
- `conary-test -- list`:
- `site` validation:
- `web` validation:

## Public-Surface Audit

- Grep sweep:
- Manual file review:
- Release-surface fixes:

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

- None recorded yet.

## Fixes Made

- None recorded yet.

## Release Decision

- Approved Tracks:
- Dropped Tracks:
- Blocked Tracks:
- Final Release Command:

## Final Commands

- Pending.
