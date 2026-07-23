## Primary Issue

<!-- Every non-trivial PR names one primary issue. Keep one linkage form. -->
Closes #
<!-- Use `Refs #` instead when this is one slice and the issue must remain open. -->

Design / Plan / Roadmap:

## Problem And Outcome

What problem does this solve, and what should be true after merge?

## Changes

-

## Scope

- In scope:
- Out of scope:

## Ownership / Boundary

<!-- bash scripts/agent-context.sh --path <changed-file> prints the owning card and its verification commands -->

- Owning subsystem:
- Boundary changed or preserved:
- Persisted state or public surface impact:
- [ ] Checked `docs/modules/feature-ownership.md` when this changes a user-visible capability

## Verification

- [ ] Listed the exact verification commands run below
- [ ] Added or updated tests when behavior changed
- [ ] Ran affected-package verification directly when touching service or daemon code
- [ ] Updated subsystem docs or maps when the "look here first" path changed
- [ ] Ran the broader interaction gate when the feature ownership card required it
- [ ] Updated the primary issue with any acceptance criteria left for later PRs

```text
- cargo fmt --check
- cargo clippy --workspace --all-targets -- -D warnings
- cargo test -p conary
```

## Review And Merge Notes

- Review focus:
- User or developer impact:
- Required-check bypass: not used
