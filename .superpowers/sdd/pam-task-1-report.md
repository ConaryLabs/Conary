# Task 1 Report: Extend PAM Blocked-Class And Corpus Hints

## What Changed

- Extended the `pam` blocked-class registry so `BlockedClassRegistry::match_invocation` now recognizes:
  - `authconfig`
  - `authselect`
  - `pam-auth-update`
  - `pam-config`
- Extended Remi corpus hinting so `ScriptletCorpusSummary::from_scriptlets` now adds `blocked_class_hints = ["pam"]` when the same helper family appears in shell scriptlet evidence.
- Added focused regressions in both owned files, placed exactly where the task brief requested.

## TDD Evidence

### RED

Ran the focused tests after adding the regressions and before changing production logic.

- `cargo test -p conary-core blocked_classes_cover_common_pam_stack_helpers --lib`
  - Failed with `missing blocked class for authconfig`
- `cargo test -p remi corpus_summary_marks_common_pam_stack_helpers`
  - Failed with `assertion left == right failed` for `blocked_class_hints`

### GREEN

After extending the PAM command lists, reran the exact verification commands:

- `cargo test -p conary-core blocked_classes_cover_common_pam_stack_helpers --lib`
  - Passed
- `cargo test -p conary-core pam --lib`
  - Passed
- `cargo test -p remi corpus_summary_marks_common_pam_stack_helpers`
  - Passed

Also ran:

- `git diff --check`
  - Passed

## Files Changed

- `crates/conary-core/src/ccs/convert/blocked_classes.rs`
- `apps/remi/src/server/scriptlet_corpus.rs`

## Self-Review

- The change is tightly scoped to the PAM helper family and does not introduce any adapter, manifest projection, or public gate exception.
- The blocked-class registry and corpus hinting now agree on the same helper names, which keeps advisory detection consistent.
- The new tests cover both the registry behavior and the Remi corpus summary behavior.

## Concerns

- None noted for this slice.
