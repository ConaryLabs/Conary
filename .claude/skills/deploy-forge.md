---
name: deploy-forge
description: Sync current source to Forge for integration testing. Use when the user wants to test on Forge, run integration tests, or deploy code to the CI server.
disable-model-invocation: true
---

# Deploy to Forge

Sync source code to Forge (`peter@forge.conarylabs.com:~/Conary/`) for integration testing.

## Default: Sync only

```bash
./scripts/deploy-forge.sh
```

## With build

```bash
./scripts/deploy-forge.sh --build
```

## Sync a worktree instead of main repo

```bash
./scripts/deploy-forge.sh --path /home/peter/Conary/.worktrees/<worktree-name>
```

## After syncing, common next steps

- Run Phase 1 tests: `ssh peter@forge.conarylabs.com 'cd ~/Conary && DOCKER_HOST=unix:///run/user/$(id -u)/podman/podman.sock cargo run -p conary-test -- run --distro fedora43 --phase 1'`
- Run Phase 2 tests: `ssh peter@forge.conarylabs.com 'cd ~/Conary && DOCKER_HOST=unix:///run/user/$(id -u)/podman/podman.sock cargo run -p conary-test -- run --distro fedora43 --phase 2'`
