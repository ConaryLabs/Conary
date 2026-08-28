---
last_updated: 2026-08-20
revision: 25
summary: Define exact source-ABI lifecycle authority, selected-root execution, and generation-scoped activation
---

# Scriptlet Security Model

Conary executes package lifecycle programs because RPM, dpkg, libalpm, and
signed CCS packages define them as part of the package transaction. It does
not decide lifecycle behavior from shell text, distro names, risk scores, or a
human-review queue.

The normative source-format contracts are in
[`foreign-package-lifecycle-contracts.md`](specs/foreign-package-lifecycle-contracts.md).
This document owns the execution and security boundary around those contracts.

## Non-Negotiable Invariants

1. The source package format owns lifecycle stage, order, arguments, stdin,
   interpreter, trigger matching, payload visibility, and error recovery.
2. Conary verifies and parses that authority before planning a transaction.
3. Every executable entry is preflighted before package payload or database
   mutation.
4. Lifecycle programs run only inside a materialized selected or explicit
   target root. The host root `/` is rejected.
5. The child enters private mount, network, IPC, UTS, and cgroup namespaces,
   chroots into that root, drops every capability except the selected-root
   filesystem mutation set, and installs `scriptlet-boundary-v2` before
   `exec`.
6. A missing interpreter, helper, namespace, privilege, syscall filter, exact
   semantic, or persisted-state contract fails the transaction. There is no
   direct-host, skip-script, or permissive sandbox path. RPM's source-declared
   warning-only policy applies only to a script execution failure after this
   boundary is established; it cannot suppress preflight, contract, or
   enforcement failures.
7. Successful lifecycle work and its package/database state commit together.
   A fatal failure rolls back the isolated selected root and SQLite
   transaction. A source-declared non-critical RPM script execution failure
   retains its already-applied selected-root effects, emits a warning, and
   continues exactly as the RPM transaction ABI requires.
8. Runtime service-manager, security-policy, bootloader, initramfs, kernel,
   and module-maintenance actions are generation work. They are captured as
   exact typed intents and are not sent to the currently running host during
   package installation.

## Authority Flow

```text
signed repository/package authority
              |
              v
exact source-format parser
              |
              v
typed transaction graph + recovery edges
              |
              v
transaction-wide target capability preflight
              |
              v
selected-root payload/lifecycle execution
              |
              v
atomic package state + generation activation requests
```

RPM header tags, Debian control members and trigger declarations, Arch
`.INSTALL` functions and ALPM hooks, and signed CCS hook fields are authority.
Conary preserves executable bytes and validates their hashes rather than
regenerating programs from diagnostics.

The target provides typed capabilities: interpreters, exact helper paths,
embedded runtimes, package-manager compatibility projections, and the closed
execution boundary. A source package manager and its database are not invoked
at install, update, remove, or rollback time.

## Selected-Root Execution Boundary

`SandboxMode` has one value, `Always`, and `EffectiveSandbox` has one value,
`TargetRoot`. Removed values such as `auto`, `never`, and `none` do not parse.

Before spawning a lifecycle child, Conary:

- requires an absolute root other than `/`;
- validates the event's projected payload view and interpreter availability;
- validates every source-format helper and persisted-state dependency;
- stages the exact program beneath a no-follow, descriptor-owned temporary
  directory inside the target root;
- supplies source-ABI stdin bytes, or an empty non-interactive file when the
  ABI defines no stdin;
- clears the inherited environment and installs only the closed lifecycle
  environment;
- builds the mandatory host-escape deny filter and capability plan before
  mutation.

The child then:

- enters private mount, network, IPC, UTS, and cgroup namespaces with
  recursive-private mount propagation;
- installs only Conary-owned bind mounts used by exact runtime capture;
- chroots and changes directory to `/`;
- becomes a private process-group leader;
- drops the capability bounding, effective, permitted, inheritable, and
  ambient sets to the exact selected-root filesystem mutation capabilities;
- enables `no_new_privs` and applies the `scriptlet-boundary-v2` host-escape
  deny filter;
- executes the declared interpreter, interpreter arguments, staged program,
  and typed lifecycle arguments;
- has stdout and stderr captured, and its entire process group is killed when
  the leader exits or its exact timeout expires.

The seccomp contract is a default-allow deny boundary, not a target-libc
allowlist. Ordinary helper ABI evolution such as `vfork`, `clone`, `mkdir`,
`mkdirat`, `statx`, `io_uring`, and `userfaultfd` does not require Conary
changes. The denied classes are host escape, cross-process control and
inspection, kernel/module control, mount-namespace escape, host keyring
access, and host time/accounting mutation. Packages cannot select, widen, or
suppress this boundary.

Root privilege is currently required for this boundary. Lack of privilege or
kernel support is a preflight failure, not a request to run less safely.

## Exact Runtime Activation

Package lifecycle programs commonly call service-manager and security-policy
helpers while operating on an offline root. Treating those calls as either
harmless strings, metadata projections, or immediate host actions is
incorrect.

Conary bind-mounts a private proxy over every resolved target-root
`systemctl`, `restorecon`, `semanage`, `setsebool`, `semodule`,
`apparmor_parser`, `aa-enforce`, `aa-complain`, and `aa-disable` executable.
Each proxy:

- records NUL-delimited argv without reparsing shell source;
- identifies the exact invoked and canonical provider paths plus executable
  SHA-256;
- delegates only provider-documented selected-root work: systemd offline work
  with `SYSTEMD_OFFLINE=1`, SELinux policy-store work with `-N`, AppArmor
  parser work with `-Q`, and AppArmor mode-file work with `--no-reload`;
- suppresses the live-manager/kernel half before it can escape the selected
  root;
- returns the source-compatible result for the delegated and deferred halves.

The parent parses every captured argv through the typed activation grammar.
The systemctl proxy scan is generated from the same typed verb/option tables,
so shell token lists cannot drift from parser authority. Ambiguous pre-verb
options, malformed invocations, unsafe paths, unsupported compound actions,
and unmodeled provider operations fail before commit.

Selected-root-only SELinux label/file-context work creates no runtime request.
SELinux module install, persistent boolean assignment, AppArmor profile reload,
and AppArmor mode changes become exact generation requests after their
documented no-live selected-root half succeeds. Non-persistent setsebool is a
typed unsupported live operation because it cannot survive a same-generation
reboot after one-shot completion.

Accepted requests are stored against the source changeset with exact source and
provider identity, projected into generated systems, and consumed only when
that generation is proven active at boot. The consumer verifies the booted
provider path and SHA-256 before running the exact captured argv. A missing,
changed, or failing provider leaves a durable retryable failure; it is never a
silent no-op, distro fallback, or manual reconciliation item. Skipped
generations carry unapplied requests forward; completed requests are not
replayed into later generations.

The same boundary intercepts the pinned `depmod`, `modprobe`, `dracut`,
`mkinitcpio`, `update-initramfs`, `kernel-install`, `installkernel`, `dkms`,
`grub-mkconfig`, `update-grub`, and `bootctl` contracts. Help, version, query,
and status forms execute immediately through the staged selected-root provider
and create no durable request. A mutation form succeeds only when Conary can
bind it to that provider's exact invoked path, canonical path, and executable
SHA-256; otherwise the lifecycle event fails. Successful mutations append a
closed, versioned generation request with their exact argv. At boot, provider
drift or execution failure remains a durable automatic retry under the same
generation activation state machine.

That typed request stream also owns boot-artifact invalidation. A later
ordinary package publication may reuse the completed current generation's
independently verified kernel, initramfs, and EFI files only when no applied
boot-runtime request exists after that generation's published changeset
high-water mark. Any such request forces exact sysroot preparation and boot
asset regeneration; package names and filesystem-path guesses do not decide
the boundary.

## Source-Format Failure Semantics

The typed transaction graph owns failure behavior:

- RPM stage order, instance counts, triggers, embedded Lua, macro expansion,
  and the persisted critical flag follow RPM's documented ABI. A non-critical
  RPM program exit or timeout is diagnosed and the graph continues; a critical
  RPM failure aborts.
- Debian maintainer scripts, trigger state, conffiles, alternatives, dpkg
  compatibility projection, deconfiguration, and error-unwind edges follow
  dpkg's documented ABI.
- Arch `.INSTALL` functions, ALPM hook ordering, stdin/argv, and implicit
  transaction finalizers follow libalpm's documented ABI.

An upstream ABI may intentionally ignore or downgrade a particular result. For
example, libalpm ignores its implicit `ldconfig` return value, while RPM maps a
non-critical script result to transaction-element success and still reports
the script error. That behavior is encoded in the typed source-format adapter.
It is not a generic operator warning policy and cannot be selected by message
text, package name, or command-line bypass.

RPM warning-only handling is limited to `ScriptExited` and `ScriptTimedOut`
after successful typed preflight. A malformed lifecycle contract, unavailable
program, process or sandbox setup failure, or enforcement failure remains
fatal regardless of RPM criticality. Every other lifecycle failure aborts the
transaction. Conary has no generic `post_hooks_failed` bypass state; a
source-declared non-critical RPM event completes its graph boundary with an
explicit warning.

## Signed CCS Hooks

CCS hook declarations are covered by signed CCS v3 authority. Package-scoped
pre-install, post-install, and persisted pre-remove hooks are preflighted
against the selected root.

Post-install hooks run after the typed native graph but before the changeset
and selected-root input commit. Their filesystem effects, captured activation
requests, and package state therefore succeed or roll back as one operation.
Pre-remove hook identity is bound to the exact installed Conary trove; upgrade,
replacement, conflict removal, and explicit removal load that persisted
authority rather than trusting a new archive.

## Diagnostic Analysis Is Not Authority

`security/command_risk.rs` parses shell with tree-sitter and produces typed
diagnostic evidence for destructive filesystem operations, network access,
dynamic execution, persistence, credential paths, and similar risks.

That evidence is useful for explanation, corpus measurement, and prioritizing
native models. It cannot:

- create or suppress a lifecycle event;
- select a source or target ABI;
- change ordering, argv, stdin, payload visibility, or recovery;
- approve compatibility, execution, serving, or publication;
- weaken the selected-root boundary;
- replace exact source metadata or runtime capture.

Malformed or dynamic shell produces unresolved diagnostic findings. Literal
words in comments and data do not become commands. Remi may expose
privacy-normalized diagnostic aggregates, but raw program artifacts,
credentials, environment values, and host-local paths remain private.

## Unsupported Semantics

An unsupported interpreter, expansion flag, helper contract, recovery edge, or
runtime action is an implementation gap in a supported source format. Conary
returns a typed preflight error naming the missing contract. It does not place
the package into indefinite human review, silently discard the behavior, or
turn a diagnostic pattern into a substitute implementation.

Closing such a gap requires one of:

- an exact source-ABI implementation;
- a Conary-owned typed compatibility service with equivalent persisted state
  and failure behavior;
- a complete lowering whose grammar, payload effects, state transitions, and
  unwind paths are proven by focused tests.

## Implementation Map

| Ownership | Start here |
|---|---|
| Source lifecycle schemas | `crates/conary-core/src/ccs/native_lifecycle.rs` |
| Source transaction graphs | `crates/conary-core/src/ccs/native_transaction/` |
| Exact lifecycle executor | `crates/conary-core/src/scriptlet/native_lifecycle.rs` |
| Selected-root process boundary | `crates/conary-core/src/scriptlet/process.rs` |
| Namespace, capability, and seccomp contract | `crates/conary-core/src/scriptlet/boundary.rs` |
| Closed sandbox types | `crates/conary-core/src/scriptlet/sandbox.rs` |
| Output and subprocess runtime | `crates/conary-core/src/scriptlet/runtime.rs`, `crates/conary-core/src/child_wait.rs` |
| Exact activation capture | `crates/conary-core/src/scriptlet/activation_capture.rs` |
| Shared systemctl parser/proxy grammar | `crates/conary-core/src/activation/systemd/grammar.rs` |
| SELinux/AppArmor argv grammars | `crates/conary-core/src/activation/security_policy/` |
| Generation activation model | `crates/conary-core/src/activation/` |
| Transaction planning and preflight | `apps/conary/src/commands/install/native_events/` |
| Debian target-state projection | `apps/conary/src/commands/install/native_events/debian_runtime/` |
| CCS hook transaction boundary | `apps/conary/src/commands/install/ccs_transaction.rs` |
| Persisted CCS remove authority | `crates/conary-core/src/db/models/installed_ccs_remove_hook.rs` |
| Generation intent persistence | `crates/conary-core/src/db/models/generation_activation.rs` |

## Proof Floor

Run the `ccs`, `install`, and `generation` ownership-card proofs after changing
this model. At minimum, changes to lifecycle execution owe:

```bash
cargo test -p conary-core --lib scriptlet
cargo test -p conary-core --lib native_lifecycle
cargo test -p conary-core --lib native_transaction
cargo test -p conary-core --lib activation
cargo test -p conary --lib commands::install::native_events
cargo test -p conary --lib commands::ccs::install
```
