---
last_updated: 2026-04-02
revision: 1
summary: Non-secret infrastructure, MCP, and deployment guidance for Conary contributors and coding assistants
---

# Infrastructure Overview

## Host Roles

- Remi is the production package service behind `https://packages.conary.io`.
- Forge is the CI and integration-test host used for `conary-test` service work
  and source-sync validation.
- Sensitive usernames, credentials, or workstation-only shortcuts belong in the
  ignored `docs/operations/LOCAL_ACCESS.md`, not in tracked docs.

## MCP-First Operations

Prefer MCP tools when they already cover the workflow:

- Remi admin and package-service operations
- `conary-test` run control, deploy/restart flows, image management, and fixture publishing

Use manual SSH, rsync, or curl only when the MCP surface does not cover the
task or when you are debugging the underlying service path itself.

## Safe Public And Admin Endpoints

- Public package service: `https://packages.conary.io`
- Remi admin API and MCP surface: `https://packages.conary.io:8082`
- Remi OpenAPI spec: `https://packages.conary.io:8082/v1/admin/openapi.json`
- Forge-local `conary-test` health endpoint: `http://127.0.0.1:9090/v1/health`

## Source Deploy Patterns

### Forge

- Sync a checkout or worktree with `./scripts/deploy-forge.sh`
- Build the needed binaries on Forge and restart the `conary-test` user service
- Use the worktree-aware `--path` option when deploying from a feature worktree

### Remi

- Use rsync to `/root/conary-src/`
- Exclude `target/`, `.git/`, and `.worktrees/`
- Build `remi`, stop the service before replacing the live binary, then restart
  and verify the local health endpoint

Do not overwrite the live Remi binary while `remi.service` is still running the
old process. That can fail with `Text file busy`.

## Release Flow

- Run `./scripts/release.sh [conary|remi|conaryd|conary-test|all]` to bump
  versions, update changelog state, and create tags
- Push the relevant tags to trigger the GitHub release pipeline
- GitHub Actions builds the release artifacts and deploys them to Remi
- Forge-side release checks verify that the new release landed correctly

## Contributor Notes

- Prefer the tracked docs for stable roles and workflows, and keep local-only
  access details in `docs/operations/LOCAL_ACCESS.md`
- For suite layout, phase selection, and manifest-run behavior, use
  `docs/INTEGRATION-TESTING.md`
- For legacy historical context, use `docs/llms/archive/claude-era-notes.md`
