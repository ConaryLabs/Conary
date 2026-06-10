# conaryd Authorization Fail-Closed Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the approved Track 0 policy so `conaryd` no longer grants non-root write access through default `sudo`/`wheel` GID shortcuts while PolicyKit is stubbed.

**Architecture:** Keep authorization centralized in `apps/conaryd/src/daemon/auth.rs`. Root and daemon identity remain trusted; all other write authorization fails closed until a later PolicyKit design provides an action-specific policy path.

**Tech Stack:** Rust, conaryd daemon auth tests, docs-truth, docs-audit.

---

## Design Source

- `docs/superpowers/specs/2026-06-10-conaryd-authorization-fail-closed-policy-design.md`

## File Map

| Path | Purpose |
| --- | --- |
| `apps/conaryd/src/daemon/auth.rs` | Remove hardcoded admin-group fallbacks and align default authorization. |
| `apps/conaryd/src/daemon/routes/auth.rs` | Adjust route-level tests if they assume group-based full access. |
| `docs/modules/conaryd.md` | Describe root/daemon identity write access and non-root fail-closed behavior. |
| `docs/ARCHITECTURE.md` | Update only if it repeats daemon write-auth policy. |
| `docs/llms/subsystem-map.md` | Update only if the auth ownership path moves. |

## Task 0: Baseline

- [ ] Run `git status --short --branch`.
  Expected: synced `main`; unrelated local docs drafts are acceptable only if they are outside this plan.
- [ ] Run `cargo test -p conaryd daemon::auth`.
  Expected: current tests pass before edits.
- [ ] Run `bash scripts/check-doc-truth.sh`.
  Expected: `Documentation truth checks passed.`

## Task 1: Add Failing Authorization Tests

- [ ] In `apps/conaryd/src/daemon/auth.rs`, replace the current default-admin-group expectation with tests that pin the strict policy:

```rust
    #[test]
    fn test_default_checker_does_not_trust_distribution_admin_gids() {
        let checker = AuthChecker::new();

        for gid in [10, 27] {
            let user = PeerCredentials {
                pid: 1000,
                uid: 1000,
                gid,
            };

            assert_eq!(checker.check(&user, Action::Query), Permission::ReadOnly);
            assert_eq!(checker.check(&user, Action::Install), Permission::Denied);
            assert_eq!(checker.check(&user, Action::CancelJob), Permission::Denied);
        }
    }
```

- [ ] Add a root/daemon preservation test if one does not already cover both identities:

```rust
    #[test]
    fn test_fail_closed_policy_preserves_root_and_daemon_identity() {
        let checker = AuthChecker::new();
        let root = PeerCredentials {
            pid: 1,
            uid: 0,
            gid: 0,
        };
        let daemon = PeerCredentials {
            pid: std::process::id(),
            uid: nix::unistd::geteuid().as_raw(),
            gid: nix::unistd::getegid().as_raw(),
        };

        assert_eq!(checker.check(&root, Action::Install), Permission::Full);
        assert_eq!(checker.check(&daemon, Action::Install), Permission::Full);
    }
```

- [ ] Run `cargo test -p conaryd daemon::auth::tests`.
  Expected before implementation: at least the new distribution-admin-GID test fails.

## Task 2: Remove Default Group Bypass

- [ ] In `apps/conaryd/src/daemon/auth.rs`, remove hardcoded `10` and `27` fallback behavior from `resolve_admin_gids`.
- [ ] Change `AuthChecker::default()` so default `trusted_gids` cannot grant non-root write access while PolicyKit is stubbed. The simplest acceptable end state is an empty default trusted-GID list.
- [ ] Update or remove comments that say members of `wheel` or `sudo` get full access by default.
- [ ] Remove `PeerCredentials::is_admin_group()` and its GID 10/27 tests if no production path uses it. If a helper remains for future policy work, rename or document it so it is not authoritative for `AuthChecker` authorization decisions.
- [ ] Decide whether `disable_admin_groups()` still has value after the default checker stops trusting groups. Remove it if it becomes dead code; otherwise document it as a test/future-policy helper rather than a production safety requirement.
- [ ] If `add_trusted_gid()` remains, document it as an explicit test/future-policy helper and keep it unused by `Default`.
- [ ] Run `cargo test -p conaryd daemon::auth::tests`.
  Expected: the new strict-policy tests pass.

## Task 3: Align Route Tests

- [ ] Inspect `apps/conaryd/src/daemon/routes/auth.rs` for tests that imply default admin-group access.
- [ ] If `test_require_auth_admin_group_allowed` still exists, either remove it or rename it to cover the remaining explicit identity path. Do not preserve a test that says default group membership grants daemon write access.
- [ ] Run `cargo test -p conaryd daemon::routes::auth`.
  Expected: route-level auth tests pass.

## Task 4: Update Docs

- [ ] Update `docs/modules/conaryd.md` so the active auth section states:
  - UID `0` has full daemon access.
  - The daemon identity has full daemon access.
  - Non-root peers may read only read-only query surfaces.
  - Non-root writes are denied while PolicyKit remains unavailable.
  - `sudo`, `wheel`, and numeric GID fallbacks are not default daemon admin paths.
- [ ] Sweep for stale wording:

```bash
rg -n "wheel|sudo|trusted GID|PolicyKit|non-root" docs/modules/conaryd.md docs/ARCHITECTURE.md docs/llms/subsystem-map.md apps/conaryd/src/daemon/auth.rs
```

- [ ] Update only the lines that describe this policy.

## Task 5: Final Verification And Commit

- [ ] Run:

```bash
cargo test -p conaryd daemon::auth::tests
cargo test -p conaryd daemon::routes::auth
cargo test -p conaryd
bash scripts/check-doc-truth.sh
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
git diff --check
```

- [ ] Commit:

```bash
git add apps/conaryd/src/daemon/auth.rs apps/conaryd/src/daemon/routes/auth.rs docs/modules/conaryd.md docs/ARCHITECTURE.md docs/llms/subsystem-map.md
git commit -m "security(conaryd): fail closed daemon write auth"
```

Only stage docs that actually changed.
