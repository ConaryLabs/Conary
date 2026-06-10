---
last_updated: 2026-06-09
revision: 1
summary: Follow-up path for true single-package system adopt dry-run preview
---

# System Adopt Single-Package Dry-Run Follow-Up

## Purpose

This note owns the deferred follow-up for true single-package adoption preview:

```bash
conary system adopt <pkg> --dry-run
```

The current public behavior is intentionally honest rather than implemented.
`conary system adopt --help` says single-package dry-run is rejected until it
has a true non-mutating preview path, and runtime refusal points users to
`conary system adopt --system --dry-run` or to package adoption without
`--dry-run`.

## Current Decision

Keep the current refusal until a reviewed slice can prove one of these outcomes:

- a real non-mutating preview for `conary system adopt <pkg> --dry-run`; or
- removal or narrowing of package-mode dry-run visibility if the preview should
  remain unsupported.

Do not treat the current refusal as broken as long as help text and runtime
errors stay aligned and specific.

## Required Follow-Up Proof

A future implementation plan must prove:

- parser behavior for package-mode `--dry-run`
- help text and runtime wording stay aligned
- no database, CAS, or live-host mutation occurs during preview
- `command_risk` keeps the command classified as dry-run-only if a true preview
  lands
- focused CLI safety coverage checks the actionable alternative text

The Wave 1b feature-coherency ledger row for this surface should point here
while the refusal remains deliberate.
