<!-- .claude/agents/full-cycle-team.md -->
---
name: full-cycle-team
description: Run a full review-implement-verify cycle by chaining three standalone teams. Phase 1 runs expert-review-team, Phase 2 runs implement-team, Phase 3 runs qa-hardening-team. Claude conducts between phases with user approval gates.
---

# Full Cycle Team

An orchestrated pipeline that chains three standalone teams sequentially. Claude acts as conductor between phases, presenting findings and getting approval before moving on.

## The Pipeline

```
expert-review-team  ->  implement-team  ->  qa-hardening-team
    (discover)           (fix)               (verify)
         ^                   ^                    ^
     user gate           user gate           final report
```

## Phase 1 -- Discovery

Run the **expert-review-team** (Nadia, Jiro, Sable, Lena).

All 4 experts analyze the codebase in parallel and report findings.

**Gate:** Conductor synthesizes a unified report and presents it. User decides:
- Approve all -> proceed to Phase 2
- Select specific findings -> proceed with subset
- Stop here -> review-only (same as just running expert-review-team)

## Phase 2 -- Implementation

Run the **implement-team** (Kai plans, parallel agents execute, Rio verifies).

Takes the approved findings from Phase 1 as input. Kai breaks them into non-overlapping batches, parallel agents execute, Rio verifies compilation.

**Gate:** Conductor reports what was implemented and any issues. User decides:
- Proceed to Phase 3 -> run QA hardening
- Fix specific issues first -> iterate
- Stop here -> skip QA pass

## Phase 3 -- QA Verification

Run the **qa-hardening-team** (Hana, Orin, Kali, Zara).

Audits the post-implementation state for test health, error handling, security, and edge cases. Catches anything the implementation introduced or that the expert review missed.

**Final report:** Conductor presents combined results from all 3 phases.

## How to Run

Tell Claude: "Run the full-cycle-team" or "Full cycle [area]"

## When to Use This vs Individual Teams

| Situation | Use |
|-----------|-----|
| Just want expert opinions, no changes | `expert-review-team` |
| Already have a list of fixes to implement | `implement-team` |
| Code is done, need production hardening | `qa-hardening-team` |
| Want review + fixes + verification in one pass | `full-cycle-team` |
| Pre-commit review of your changes | `pr-review-team` |
| Bug investigation | `debug-team` |
| Safe code restructuring | `refactor-team` |
| Improve test coverage | `test-gen-team` |
| Review CLI ergonomics | `cli-review-team` |

## Scoping

- "Full cycle the daemon subsystem" -> all 3 phases focus on conary-server/src/daemon/
- "Full cycle recent changes" -> focus on files changed in last N commits
