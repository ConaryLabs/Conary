## Summary

Brief description of what this PR does.

## Changes

-

## Ownership / Boundary

- Owning subsystem:
- Boundary changed or preserved:
- Persisted state or public surface impact:

## Verification

- [ ] Listed the exact verification commands run below
- [ ] Added or updated tests when behavior changed
- [ ] Ran affected-package verification directly when touching service or daemon code
- [ ] Updated subsystem docs or maps when the "look here first" path changed

```text
- cargo fmt --check
- cargo clippy --workspace --all-targets -- -D warnings
- cargo test -p conary
```

## Related Issues / Plans

Closes #
Plan / Roadmap:
