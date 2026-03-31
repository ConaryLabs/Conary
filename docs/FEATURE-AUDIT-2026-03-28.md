---
last_updated: 2026-03-31
revision: 2
summary: Track whether active feature claims are implemented, command-aligned, and positively covered
---

# Feature Audit

Status labels:

- `working`: implemented, command-aligned, and covered by a positive test path
- `partial`: usable, but some subcommands or maturity claims still need to stay narrow
- `alpha`: intentionally best-effort or still operationally rough

| Surface | Status | Proof | Notes |
|---|---|---|---|
| Install / remove / update / rollback | `working` | `cargo test --features server`, Phase 1 + workflow tests | Core package-manager path is the strongest-covered surface. |
| Native local RPM / DEB / Arch installs | `working` | `tests/workflow.rs`, Phase 4 Group E | Generated native fixtures are installed and removed on the matching distro family. |
| CCS build / install / reinstall | `working` | unit tests + Phase 4 Group C | Includes signature/policy checks and CAS-backed deployment. |
| `ccs shell` / `ccs run` | `working` | Phase 4 Group C | Proven with a real local fixture package. |
| CCS selective component installs | `working` | `tests/component.rs` | Requested components are the only files persisted and scriptlets are gated appropriately. |
| Config diff / backup / restore | `working` | Phase 4 Group A | Uses a tracked config file from the local runtime fixture. |
| Labels and delegation / linking | `working` | Phase 4 Group C | Positive add, show, link, and delegate flows. |
| Trigger mutation | `working` | Phase 4 Group D | Built-in enable/disable and custom add/remove paths are covered. |
| Derived packages | `working` | `tests/features.rs`, `tests/workflow.rs`, Phase 4 Group B | `derive build` now records a real installable artifact and parent upgrades mark derived rows stale. |
| Capability declarations and runtime execution | `working` | unit tests + Phase 4 Group D | Public surface is `capability list/show/validate/run`; there is no separate `enforce` command. |
| Trust bootstrap and local trust state | `working` | Phase 4 Group D | Key generation, signed-root bootstrap, status, disable, and re-enable are proved locally. |
| Provenance show / export / audit / diff | `working` | Phase 4 Group D | `provenance verify` still depends on whether the package actually has transparency-log data. |
| Federation peer management | `working` | unit tests + Phase 4 Group D | HTTPS peers now require `--tls-fingerprint`; peer list, status, stats, test, add, and remove are covered. |
| Declarative system model | `working` | `tests/features.rs`, Phase 4 Groups A/B/E | Diff, apply, snapshot, and replatform planning are all exercised. |
| System generations | `working` | generation tests + Phase 2/3 coverage | Includes build, list, switch, rollback, GC, verity, and pending-build protection. |
| Takeover | `partial` | unit tests + Phase 2/4 manifest coverage | `generation` now stops ready to activate and uses `conary system generation switch <N>` for activation; real-machine proof still matters more than optimistic wording. |
| Bootstrap pipeline | `partial` | command tests + `tests/bootstrap_workflow.rs` | Manifest-driven runs now persist operation records, `verify-convergence` is run-workdir based, and `diff-seeds` reports metadata/hash/top-level artifact differences. |
| Federation mDNS discovery | `partial` | server-side tests | Discovery is gated by allowlists or authenticated transport and remains a server-oriented path. |
| Automation status / check / config preview | `partial` | Phase 4 Group D | Status and preview flows work, but persistence is intentionally limited. |
| Automation history / persistent configure / daemon background mode | `partial` | explicit “not yet implemented” outputs | Keep docs narrow until those paths gain real storage and lifecycle support. |

## Immediate Follow-Up Rules

- Do not add new public examples for removed or nonexistent commands such as
  the old phase-style bootstrap names, the legacy group-update spelling, or the
  retired capability-enforcement alias.
- Keep takeover and bootstrap wording narrow and operationally specific.
  Avoid blanket “stable everywhere” claims until the real-machine proof set is
  consistently green.
- Treat automation configuration persistence and history as preview-only until
  they have backing storage and passing happy-path tests.
