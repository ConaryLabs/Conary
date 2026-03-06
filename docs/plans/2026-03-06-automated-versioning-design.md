# Automated Versioning Design

**Date:** 2026-03-06
**Status:** Approved

## Problem

All 4 crates are stuck at 0.1.0. Version bumps require manually picking numbers. No convention for when or what to bump. Docs have no freshness signal.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Commit format | Conventional Commits | Already mostly followed; enables automated analysis |
| Crate versioning | 3 independent version tracks | Different change cadences for core vs server vs erofs |
| Version grouping | conary+core share, erofs independent, server independent | CLI is thin wrapper over core |
| Bump determination | Highest-priority prefix wins | Standard semver logic |
| Doc versioning | YAML frontmatter (last_updated + revision) | Lightweight freshness signal |
| Implementation | Shell script + CLAUDE.md rules | No external tool dependencies |

## Conventional Commit Prefixes

| Prefix | Meaning | Version Effect |
|--------|---------|---------------|
| `feat:` | New feature | Minor bump |
| `fix:` | Bug fix | Patch bump |
| `feat!:` / `fix!:` | Breaking change | Major bump (post-1.0) |
| `docs:` | Documentation only | No bump |
| `refactor:` | Code restructure | No bump |
| `test:` | Test changes | No bump |
| `chore:` | Build/tooling | No bump |
| `security:` | Security fix | Patch bump |
| `perf:` | Performance | Patch bump |

## Version Groups

| Group | Crates | Path scope | Tag format |
|-------|--------|-----------|------------|
| conary | `conary` + `conary-core` | `src/` + `conary-core/` | `v0.2.0` |
| conary-erofs | `conary-erofs` | `conary-erofs/` | `erofs-v0.1.1` |
| conary-server | `conary-server` | `conary-server/` | `server-v0.2.0` |

## Release Flow

1. Analyze commits since last tag for the target version group
2. Filter to commits touching that group's path scope
3. Determine bump level (highest-priority prefix wins):
   - Any `feat!:` or `BREAKING CHANGE` -> major
   - Any `feat:` -> minor
   - Any `fix:` / `security:` / `perf:` -> patch
   - Only `docs:` / `refactor:` / `test:` / `chore:` -> no release
4. Update Cargo.toml version(s) for the group
5. Update cross-crate dependency versions if needed
6. Generate CHANGELOG.md entries grouped by prefix
7. Commit: `chore: release conary v0.2.0`
8. Tag: `v0.2.0`

Invoked via: `./scripts/release.sh [conary|erofs|server|all]`

## Doc Versioning

YAML frontmatter on docs in `docs/`:

```yaml
---
last_updated: 2026-03-06
revision: 1
summary: Brief description of last update
---
```

Rules:
- Agent adds/updates header when modifying docs/ files
- `revision` increments on meaningful updates (not typo fixes)
- `last_updated` set to current date
- Excluded: ROADMAP.md, CHANGELOG.md, CONTRIBUTING.md (own structure)

## Components to Build

1. `scripts/release.sh` -- Commit analysis, bump, Cargo.toml update, changelog, tag
2. CLAUDE.md update -- Conventional commit rules for agent self-enforcement
3. Agent integration -- sbuild agent can invoke release script
4. Doc header convention -- Rule in CLAUDE.md for YAML frontmatter on docs/
