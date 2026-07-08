Status: DONE_WITH_CONCERNS

Summary of changes
- Added sysctl public-policy support in `crates/conary-core/src/ccs/convert/public_policy.rs`.
- Introduced `SYSCTL_PUBLIC_REVIEW_REASON` with the exact required value: `public-policy-sysctl-target-profile-unsupported`.
- Added `sysctl_public_review_reason(key, profile)` using `TargetProfileQuery` / `ProfileConstraintStatus`.
- Updated `entry_public_policy_review_reasons(entry, profile)` to preserve existing file-capability behavior and also review complete `sysctl/v1` `sysctl-setting` effects against the optional target profile.
- Updated `crates/conary-core/src/ccs/convert/scriptlet_bundle/summary.rs` call sites to pass `None` temporarily via closures / two-arg calls, matching the brief for Task 2.
- Added focused unit coverage in `public_policy.rs` for helper behavior and entry-level sysctl effect scanning.

Commits created
- `security: require target policy for sysctl public status`

Tests run, with pass/fail results
- `cargo test -p conary-core public_policy --lib` - PASS
- `cargo fmt --check` - PASS
- `git diff --check` - PASS

Self-review notes and any concerns
- The implementation is narrowly scoped to the allowed files and follows the brief’s exact constant name, function signature, and fallback behavior.
- Existing file-capability public-policy behavior remains intact.
- Concern: this did not achieve a strict red/green TDD proof. I added the tests and compile-preserving implementation changes in the same edit pass, so the required focused test target passed on its first run instead of demonstrating an initial failure. The final behavior and verification are good, but the TDD evidence is weaker than requested.

Review-fix addendum (Task 2 Important findings)
- Tightened `entry_public_policy_review_reasons` so `sysctl/v1` effects only count as public-policy evidence when `effect.replacement == EffectReplacement::Complete`.
- Removed key trimming from `sysctl_public_review_reason`, so target-profile lookup now uses the exact persisted sysctl key.
- Added focused regressions in `public_policy.rs` proving:
  - whitespace-padded sysctl keys remain private-review even when the trimmed key would otherwise be accepted;
  - partial `sysctl/v1` effects do not count as sysctl public-policy evidence.

Red/green verification
- `cargo test -p conary-core public_policy --lib` - FAIL first, with the new regressions failing on trimmed-key acceptance and partial-effect handling.
- `cargo test -p conary-core public_policy --lib` - PASS after the fix.
- `cargo fmt --check` - PASS
- `git diff --check` - PASS
