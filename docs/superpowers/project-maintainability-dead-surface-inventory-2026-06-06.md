# Project Maintainability Dead Surface Inventory 2026-06-06

## Purpose

This inventory records stale or potentially stale surfaces found during Phase 2
of the project maintainability roadmap. It is a pruning queue, not permission
to delete every listed surface. Each item records whether it was fixed now,
kept as intentional behavior, or deferred until stronger tests exist.

## Current Supported Public Targets

Conary public distro support is limited to:

- Fedora 44: `fedora-44`
- Ubuntu 26.04 LTS: `ubuntu-26.04`
- Arch Linux: `arch`

Internal parsers, package-format support, version-scheme helpers, tests, and
historical archives may mention broader ecosystem families when they are not
claiming public distro support.

## Fixed In Phase 2 First Slice

| Surface | Path | Reason | Proof |
|---------|------|--------|-------|
| CCS init next step printed `conary ccs-build` | `apps/conary/src/commands/ccs/init.rs` | The current command is `conary ccs build` | `cargo test -p conary --test cli_daily_ux phase2_pruning` |
| `conary repo add --remi-distro` examples listed `debian` | `apps/conary/src/cli/repo.rs` | Public Remi targets are Fedora 44, Ubuntu 26.04, and Arch | `cargo test -p conary --test cli_daily_ux phase2_pruning` |
| Remi `index-gen`, `prewarm`, and `conversion-benchmark` help listed `debian` | `apps/remi/src/bin/remi.rs` | Remi rejects Debian as an unsupported public target | `cargo test -p remi --test cli_help phase2_pruning` |

## Intentional Or Inventory-Only Surfaces

| Surface | Paths | Current Decision |
|---------|-------|------------------|
| DEB package-format handling | `packaging/deb/`, `crates/conary-core/src/ccs/legacy/deb.rs`, `crates/conary-core/src/packages/`, repository parser/versioning modules | Keep. Ubuntu 26.04 support needs DEB-family parsing and package-format logic. |
| Debian version-scheme tests | resolver, repository, selector, provider, and update tests that use `VersionScheme::Debian` or `"debian"` as a scheme string | Keep unless a child plan replaces the scheme vocabulary. These tests are about version ordering and parser behavior, not public distro support. |
| Linux Mint unsupported-distro tests | `apps/conary/src/commands/distro.rs`, `crates/conary-core/src/repository/distro.rs` | Keep. These tests prove parser-recognized or adjacent distro names do not become supported public targets. |
| Archived historical docs mentioning retired tools or broader distros | `docs/**/archive/`, `docs/plans/archive/`, `docs/superpowers/reviews/archive/` | Keep as historical evidence unless a separate archive cleanup plan decides to redact or summarize them. |
| Local `.claude/settings.local.json` | ignored by `.gitignore`, untracked by `git ls-files` | Do not delete tracked repo content because there is none. Treat as host-local state outside this pruning slice. |
| Future distro-expansion note | `ROADMAP.md` | Keep for now. The CCS native ecosystem roadmap explicitly says future distro expansion is out of current scope. |
| Broad code comments with maintenance notes | Rust comments containing future-work notes | Inventory only. Remove or rewrite only when the owning subsystem confirms they are stale. |

## Deferred Candidates

| Candidate | Why Deferred | Required Next Proof |
|-----------|--------------|---------------------|
| Narrow Remi command validation to reject unsupported target IDs at Clap parse time | This changes behavior, not just help text | Focused Remi command tests plus confirmation that config-driven Remi prewarm/index paths still accept current target IDs |
| Normalize Remi runtime target IDs | Remi currently has unversioned runtime defaults and handler validation for `arch`, `fedora`, and `ubuntu`, while public support is tracked as `arch`, `fedora-44`, and `ubuntu-26.04` | Focused Remi tests for `generate_indices`, handler validation, prewarm, and conversion benchmark using supported public IDs plus any required compatibility decision for existing internal family names |
| Review `apps/conary/src/cli/repo.rs` Remi distro validation | The current CLI stores a string and validation may happen later in repository or Remi flow | Focused CLI tests for `repo add --default-strategy remi --remi-distro <target>` and source-selection behavior |
| Review internal `debian` distro identifiers in Remi conversion service tests | Some tests assert Debian is rejected, while others use Debian-family conversion internals | Phase 3 fixture ownership map for Remi conversion and supported-target test data |
| Prune dead helper APIs found by usage search | Usage search alone is insufficient for public or persisted behavior | Existing focused tests or new tests proving the helper is unreachable and undesired |
| Normalize old archive references to retired assistant tooling | Historical docs intentionally preserve context | Archive policy decision and docs-audit update |

## Refresh Commands

Use these commands when extending this inventory:

```bash
grep -R -n -E "ccs-build|conary ccs-build|Distribution to generate index|Distribution to pre-warm|Distribution to benchmark|Examples: fedora, arch, debian, ubuntu" apps docs README.md CONTRIBUTING.md .github
grep -R -n -E "CentOS|RHEL|Debian stable|Linux Mint|linux-mint|linux mint|openSUSE|Alpine|centos|rhel|opensuse|alpine" README.md ROADMAP.md CONTRIBUTING.md AGENTS.md docs apps crates data recipes deploy packaging .github
git ls-files .claude
git check-ignore -v .claude/settings.local.json
```
