---
name: bootstrap_v2_plan_review
description: Review findings for the bootstrap v2 derivation engine implementation plan against spec rev 3 and actual codebase APIs
type: project
---

Plan: docs/superpowers/plans/2026-03-19-bootstrap-v2-derivation-engine.md
Spec: docs/superpowers/specs/2026-03-19-bootstrap-v2-cas-layered-design.md (rev 3)
Reviewed: 2026-03-19

**Why:** Implementation plan has 7 HIGH-severity API mismatches that would block an implementing agent. Findings verified against actual source files.

**How to apply:** These must be fixed in the plan before dispatching emerge. The HIGH findings will cause compile failures in Tasks 2, 4, 6, 7, and 10.

## Key API facts discovered:
- Schema is v53 (not v52 as in MEMORY.md), migration uses function dispatch (migrate_v{N} functions), not inline SQL blocks
- Kitchen::cook() is the public entry point (mod.rs:206), returns Result<CookResult>; Cook struct methods are pub(crate)
- Kitchen::new_cook_with_dest() returns Result<Cook>, Cook has no public cook() method
- CasStore::new() returns Result<Self>, not Self
- SourceSection.additional is Vec<AdditionalSource>, not Option<Vec<...>>
- PackageSection.summary is Option<String>
- BuildSection has no `chroot` field
- BuildSection has setup, post_install, environment, workdir, script_file (all omitted from plan's recipe hashing)
- nix crate is not a dependency of conary-core; mount.rs shells out to `mount` command
- chrono, sha2, hex are all available in conary-core
