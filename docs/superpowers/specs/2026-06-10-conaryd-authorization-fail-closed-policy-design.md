# conaryd Authorization Fail-Closed Policy

## Status

Policy approved for implementation via
`docs/superpowers/plans/2026-06-10-conaryd-authorization-fail-closed-implementation-plan.md`.
Track 0 from the external audit response queue.

## Goal

Make `conaryd` write authorization fail closed until Conary has a real
PolicyKit DBus authorization path and installed policy-file contract.

The immediate repair is to remove default admin-group bypasses from daemon
authorization. Root and the daemon service identity remain trusted. Other
non-root peers can perform read-only query operations, but package mutations,
system operations, job cancellation, and enhancement requests must be denied
while PolicyKit remains a stub.

## Background

External review found that `apps/conaryd/src/daemon/auth.rs` currently resolves
trusted admin groups with hardcoded numeric fallbacks:

- missing `wheel` falls back to GID `10`;
- missing `sudo` falls back to GID `27`.

That is unsafe across distributions. On Debian/Ubuntu-family systems, GID `10`
is commonly `uucp`, not `wheel`. A non-root process in that group can satisfy
the current trusted-GID check and receive `Permission::Full` before PolicyKit is
consulted.

The current docs already describe the desired stricter posture:

- root can perform daemon operations;
- the daemon identity can operate its own API;
- non-root PolicyKit write authorization is not implemented and should fail
  closed.

The implementation and tests need to match that model.

## Policy Decision

Adopt the strict fail-closed policy:

- UID `0` gets `Permission::Full`.
- The daemon service identity gets `Permission::Full`.
- Non-root, non-daemon peers get `Permission::ReadOnly` only for read-only
  actions.
- Non-root, non-daemon peers get `Permission::Denied` for write actions while
  PolicyKit is unimplemented.
- No group receives `Permission::Full` by default.
- No missing group name lookup may fall back to a hardcoded numeric GID.
- Future admin delegation must be explicit, reviewed, documented, and tested.

This deliberately removes the current default `sudo`/`wheel` bypass. A future
PolicyKit implementation can restore non-root write authorization through a
proper action-specific policy contract.

## Non-Goals

- Do not implement PolicyKit DBus authorization in this slice.
- Do not add a new daemon configuration surface for trusted groups.
- Do not preserve `sudo` or `wheel` as default write-authorized groups.
- Do not change route-level apply-intent requirements.
- Do not redesign daemon routes, job visibility, or package execution.

## Implementation Shape

The implementation plan should keep this small:

1. Add failing auth tests that show non-root GID `10` and GID `27` do not
   receive full access from the default checker.
2. Remove hardcoded group fallback behavior from `resolve_admin_gids`.
3. Change `AuthChecker::default()` so default trusted groups do not grant
   non-root write access.
4. Remove `PeerCredentials::is_admin_group()` if no production path uses it, or
   rename/document it so it cannot be mistaken for the default authorization
   policy.
5. Keep `AuthChecker::disable_admin_groups()` only if it remains useful for
   tests; it should not be needed for production safety.
6. Decide whether `AuthChecker::add_trusted_gid()` remains as a test-only or
   future-policy helper. If retained, it must not be used by default and must be
   documented as an explicit override that is outside the production default.
7. Update module comments and `docs/modules/conaryd.md` so they describe the
   same policy as the code.

The preferred end state is simpler than the current model: authorization checks
do not need distribution-specific group guesses at all.

## Verification Strategy

Focused verification for the implementation plan:

- `cargo test -p conaryd daemon::auth`
- `cargo test -p conaryd`
- `bash scripts/check-doc-truth.sh`
- `bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete`
- `LC_ALL=C bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -`
- `git diff --check`

Required behavior proof:

- A non-root peer with primary GID `10` is denied write access.
- A non-root peer with primary GID `27` is denied write access.
- A non-root peer with no trusted identity keeps read-only query access.
- UID `0` keeps full access.
- The daemon service identity keeps full access.
- PolicyKit stub paths still deny non-root writes.

## Documentation Requirements

Update active docs only where they describe daemon authorization:

- `docs/modules/conaryd.md` should say root and daemon identity can write, and
  other non-root peers are denied write operations until real PolicyKit exists.
- If `AuthChecker::add_trusted_gid()` remains in code, docs or code comments
  should make clear that it is an explicit override/future hook, not the default
  production policy.
- The feature ownership and subsystem map already route conaryd auth work to
  `apps/conaryd/src/daemon/auth.rs`; update them only if the implementation
  moves the ownership boundary.

## Risks And Tradeoffs

This policy may remove a convenient local admin shortcut for users who expected
membership in `sudo` or `wheel` to control the daemon. That is intentional for
now. The current shortcut is not portable, is not action-specific, and runs
before the documented PolicyKit fail-closed boundary.

The tradeoff is acceptable because Conary is still in limited preview and the
safe default is to require root or the daemon service identity for daemon write
operations until a real policy system exists.

## Follow-Up

Future PolicyKit work should be a separate design. It should define:

- action IDs and policy-file installation;
- DBus authorization behavior;
- how route-level apply intent composes with PolicyKit decisions;
- how package-job ownership and SSE visibility interact with authorized
  non-root operators;
- regression tests for allow, deny, and unavailable-PolicyKit cases.
