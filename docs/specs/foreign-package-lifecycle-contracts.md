---
last_updated: 2026-08-03
revision: 39
summary: Define source-independent lifecycle, source-authority handoff, generation activation, and configuration transactions for RPM, Debian, and Arch packages
---

# Foreign Package Lifecycle Contracts

This specification defines how Conary preserves and executes lifecycle behavior
from RPM, Debian, and Arch packages. The package managers expose finite,
documented lifecycle ABIs around arbitrary program bodies. Conary models those
ABIs directly; it does not infer lifecycle meaning from program text or
delegate execution to the source package manager.

[`source-package-authority.md`](source-package-authority.md) owns the upstream
package identity, provision, payload-record, and config-declaration models plus
their fallible projection into this lifecycle and transaction contract. This
document remains the owner of event order, arguments, installed config state,
and selected-root execution.

## Cross-Distro Product Contract

Foreign-package conversion is a primary acquisition path:

```text
RPM, DEB, or Arch artifact
        |
        v
Conary parser -> CCS lifecycle bundle -> Conary transaction planner/executor
        |
        v
any supported Conary target
```

RPM, dpkg, and libalpm documentation and source define the input contract.
Conary owns the runtime implementation. Conversion and installation must not
invoke `rpm`, `dpkg`, `apt`, `pacman`, or another source package manager to
decide lifecycle events, mutate its database, or complete the transaction. A
Debian package installed on Fedora, an RPM installed on Arch, and an Arch
package installed on Ubuntu use the same Conary planner and transaction engine
while preserving their source ABI exactly.

The program body may require an interpreter or helper runtime. Conary must
satisfy that requirement through declared dependencies, a Conary-owned
compatibility implementation, or a complete typed lowering. It must not assume
that the target happens to provide the source distribution's package manager or
helper behavior. Every documented RPM, Debian, and ALPM lifecycle semantic in
this specification is required implementation for the supported-format
contract.

System adoption is a migration-continuity feature for machines that already
have native package-manager state. It is not the cross-distro product path and
does not count as proof that conversion or source-independent execution works.

### Orthogonal Source And Target Axes

RPM, Debian, and Arch are source ABI families, not target-distro restrictions.
The running system exposes a typed host-capability inventory; a distro name is
neither a compatibility fact nor a runtime selector. Nothing selects a pairwise
converter, script-text classifier, or hard-coded source/target exception.

The resolver matches typed source requirements against typed target facts,
including at least:

- CPU architecture, ABI, libc, dynamic loader, and symbol/version constraints;
- init/service manager and activation interfaces;
- LSM and policy-store capabilities;
- filesystem layout, ownership, xattrs, capabilities, and immutable-root
  behavior;
- bootloader, initramfs, kernel/module, and firmware interfaces;
- available interpreters, helper contracts, and Conary compatibility services.

Those facts are validated data, not distro-name strings inferred from package
text. Supporting another Linux environment means satisfying or implementing
the same capability contracts and proof fixtures, not creating
RPM-to-that-distro, DEB-to-that-distro, or Arch-to-that-distro code paths.

The current host-inventory schema is version 4 and is persisted under
`system.host-capability-inventory` by `conary system init`. Its structural
contract records the active init interface and exact `systemctl`,
`systemd-sysusers`, `systemd-tmpfiles`, `sysctl`, and `ldconfig` command
interfaces. Each executable descriptor carries the exact command/root grammar,
parsed implementation family and version, absolute target path, and executable
SHA-256. Initialization performs an implementation-specific, non-mutating
functional handshake; install repeats that handshake and verifies the same
digest before mutation. A same-named program that merely exits successfully
for `--version` is not a capability. Probe processes receive a fixed sanitized
environment, bounded output, a five-second deadline, and a private process
group that Conary kills on timeout. Each inventory field accepts only its exact
command contract, and the systemd operation set must be exactly the offline or
live-manager shape discovered at initialization. The version-4 document also
records the opaque `security.selinux` value of the target `/usr` node when
present, for use as immutable carrier-backing authority. It does not parse that
value or derive one from a distro, path, policy name, source package manager, or
pairwise compatibility selector. A version-3 inventory must be replaced by
rerunning `conary system init`.

RPM sysusers planning emits a dedicated typed native event rather than a
generic command. Because RPM schedules it before the incoming payload is
visible, install preflight revalidates the persisted `SystemdSysusers`
descriptor and execution invokes that exact host-side interface with
`--root=<selected-root>`, optional `--replace <guest-path>`, and the decoded
declarations on stdin. The fixed grammar gives the target implementation
semantic authority without reading or mutating host account state. Missing,
wrong-contract, or drifted descriptors fail before package mutation. Generic
native commands remain confined to the selected-root execution boundary and
cannot acquire this target-interface path from argv text or lifecycle stage.

The typed tmpfiles contract preserves type, path, mode, user, group, age, and
argument as seven required strings. Conary parses only the documented type
envelope—one ASCII letter plus tmpfiles modifiers—and the field boundaries
needed for lossless rendering. It does not carry a line-type allowlist or
reinterpret mode, identity, age, specifier, or argument semantics. The target
`systemd-tmpfiles` executable is the semantic authority: live roots execute it
against the exact declaration, while offline roots receive that declaration
for the target executable at boot.

CCS service preflight resolves generic service hooks through the typed init
manager. Systemd-specific hooks resolve through the typed systemctl interface:
unit-file enable/disable operations mutate only the selected root through
systemctl's documented offline-root contract. `enable = false` means disable.
Runtime service operations become generation activation intents; package
installation never signals the host service manager.

Every successful typed Arch add, upgrade, or remove transaction evaluates
libalpm's implicit ldconfig step exactly once after package post hooks and
before post-transaction ALPM hooks. It invokes the persisted adapter inside
the selected root only when that root contains `/etc/ld.so.conf` and the
adapter path is executable there. Absence is libalpm's documented successful
no-op; an invoked ldconfig failure is reported but does not retroactively fail
the package transaction.

### Generation-Scoped Runtime Activation

Service-manager runtime work has two exact inputs. A declarative CCS service
hook emits a typed operation directly. An arbitrary signed CCS hook or native
RPM, Debian, or Arch lifecycle program is observed at the `systemctl` process
boundary inside the isolated selected root: a private mount namespace places a
capture proxy over the target executable, records the actual NUL-delimited
argv, and suppresses a closed runtime action before it can reach any service
manager. Non-runtime behavior is delegated to that same target executable with
`SYSTEMD_OFFLINE=1`. Conary does not inspect script text to decide that a
service action occurred.

The closed action grammar follows systemd's
[`unit_actions` table](https://github.com/systemd/systemd/blob/main/src/systemctl/systemctl-start-unit.c):
`start`, `stop`, `reload`, `restart`, `try-restart`,
`reload-or-restart`, and `try-reload-or-restart`, plus only the aliases that
the table itself maps to those canonical actions. The exact
[`verb_enable()` mapping](https://github.com/systemd/systemd/blob/main/src/systemctl/systemctl-enable.c)
also preserves the runtime half of `enable --now` as start,
`disable --now`/`mask --now` as stop, and `reenable --now` as try-restart,
while the target executable performs the offline unit-file half. The typed
invocation preserves documented job mode, `--all`, `--no-block`,
`--no-ask-password`, `--quiet`, `--no-warn`, `--wait`, and
`--show-transaction` behavior. It passes every unit operand after `--`; the
booted systemctl implementation remains authority for unit names, aliases,
templates, and globs. Empty/NUL operands, user/global/remote/root targets,
invalid action/flag combinations, and unknown persisted fields fail the
contract. A command operand or option value that merely equals an action word
is never action authority.

That raw process boundary is distinct from an author-declared CCS
`hooks.systemd.unit` or service name. Declarative fields must be pathless,
nonempty names before they reach `systemctl --root`; they cannot use a relative
or absolute unit-file path. Captured native argv remains byte-exact and lets
the target systemctl validate its own operand grammar.

The systemctl proxy's conservative live-manager guard is generated from the
same verb and option tables consumed by the typed Rust parser. It is not a
second shell-maintained command list. An option shape outside that closed table
is suppressed before it can reach a live manager and becomes a typed contract
error; `Ok(None)` is reserved for a positively parsed non-activation verb or
explicit dry run.

SELinux and AppArmor use the same process-boundary authority: actual argv plus
the selected root's exact provider executable. Private script-text diagnostics
are engineering evidence only and are not persisted in the lifecycle bundle.
There is no `SecurityPolicyIntent` metadata projection to reconcile or execute.

The SELinux grammar is derived from current policycoreutils
[`restorecon`](https://github.com/SELinuxProject/selinux/blob/master/policycoreutils/setfiles/restorecon.8),
[`semanage`](https://github.com/SELinuxProject/selinux/blob/master/python/semanage/semanage),
[`setsebool`](https://github.com/SELinuxProject/selinux/blob/master/policycoreutils/setsebool/setsebool.c),
and
[`semodule`](https://github.com/SELinuxProject/selinux/blob/master/policycoreutils/semodule/semodule.c)
contracts. Bounded `restorecon` and `semanage fcontext` operations remain in
the selected root. A module install first commits its filesystem policy-store
half with `semodule -N`; the original reload-capable argv becomes generation
work. Persistent `setsebool -P` assignments are fully deferred. A
non-persistent setsebool is a typed unsupported live operation because marking
a one-shot request applied would lose that state when the same generation
reboots.

The AppArmor grammar follows the current
[`apparmor_parser`](https://gitlab.com/apparmor/apparmor/-/blob/master/parser/parser_main.c)
and
[`aa-enforce`](https://gitlab.com/apparmor/apparmor/-/blob/master/utils/aa-enforce),
[`aa-complain`](https://gitlab.com/apparmor/apparmor/-/blob/master/utils/aa-complain),
and
[`aa-disable`](https://gitlab.com/apparmor/apparmor/-/blob/master/utils/aa-disable)
sources. Profile replacement commits its selected-root/cache half with
`apparmor_parser -Q`; mode helpers commit their selected-root half with
`--no-reload`. Their original kernel-changing argv becomes generation work.
Calls already carrying `semodule`/`semanage -N`, `apparmor_parser -Q`, or an
AppArmor mode helper's `--no-reload`, and bounded SELinux label/file-context
work, are selected-root-only and create no runtime request.
Unmodeled provider options fail the transaction instead of falling through to
a live binary or an operator queue.

Each successful selected-root transaction appends the canonical invocation,
source package/version/entry, source kind, exact sequence, canonical JSON, and
SHA-256 to `activation_requests` before its changeset can become applied.
Security-policy requests also retain the exact invoked path, canonical path,
and executable SHA-256 observed inside that selected root. Current-only
database schema revision 9 stores the tagged systemd/SELinux/AppArmor union;
revision-8 databases must be rebuilt and no compatibility decoder or migration
exists.
Every generation build projects all eligible requests through that
generation's applied-changeset high-water mark into
`generation_activation_intents`. Thus a request projected onto generation N
also appears on N+1 if N was never booted; completing either projection
supersedes every other projection of the same request.

The packaged `conary-generation-activation.service` invokes the hidden
`conary system generation activate` continuation. It consumes no work on a
native boot. A Conary boot must contain exactly one non-negative
`conary.generation=N` kernel argument, a matching activatable artifact, and a
matching database state. The consumer then loads only N's intents and
revalidates the persisted live-systemd capability and exact `systemctl`
executable identity before each sequence begins. If a legitimate generation
upgrade changed that interface, the consumer structurally rediscovers and
persists the booted generation's typed capability inventory instead of
requiring operator reconciliation. A security-policy request instead requires
the booted provider's invoked path to resolve to the captured canonical path
with the captured executable SHA-256, then runs the exact captured arguments.
A missing or changed provider is a durable failed request and remains
automatically retryable; it never falls back to a same-named executable,
metadata intent, distro default, or silent dormant state. The consumer never
consults the selected-next-generation symlink, a distro name, or an unbooted
generation.

Intent transitions are durable `pending -> executing -> applied` or
`pending/failed -> executing -> failed`. A durably applied request is never
replayed, including through a later generation. Because a systemd call and
SQLite cannot share one atomic commit, an interrupted `executing` request is
durably requeued and retried with its exact typed argv, matching native package
managers' at-least-once recovery boundary rather than dropping runtime work or
requiring manual reconciliation. One failed request does not starve later
requests in the same generation pass: successful requests become durable and
only the failed subset is retried automatically.

## Authority Boundaries

Lifecycle correctness has three independent authorities:

1. The source package parser owns archive metadata: lifecycle slot, body bytes,
   interpreter and interpreter arguments, invocation arguments and environment,
   trigger conditions, standard input, and package-manager ordering metadata.
2. `crates/conary-core/src/ccs/native_transaction.rs` owns event selection and
   ordering from typed package changes, typed lifecycle bundles, installed
   package state, and exact payload paths.
3. The install transaction owns payload visibility, persisted ownership,
   generation publication, and execution at each planned boundary.

The script body is not lifecycle authority. It is an arbitrary program carried
by a typed lifecycle entry. Shell AST parsing and exact helper grammars can
produce private diagnostics or support a future native lowering, but they
cannot change whether, when, or with which arguments the source package manager
would invoke it.

Heuristics, regular expressions, substring matches, normalized display shapes,
corpus frequency, and manually curated command lists are diagnostic-only. They
may redact evidence, cluster recurring programs, or prioritize engineering.
They never establish compatibility, publication, host mutation, security
authority, event selection, or lifecycle equivalence.

Every executable entry is preserved byte-for-byte with a digest. A native
lowering may replace an entry only after an exact grammar and payload/state
validation prove the replacement complete for every control-flow path. That is
a new schema contract, not a partial suppression marker or a mixture of guessed
diagnostics and partial source-program execution.

Current native lifecycle schema revision 19 has no replacement marker,
arbitrary extension map, reason code, effect projection, unknown-command
evidence, diagnostic-class list/count, adapter-registry digest, publication
policy, or parallel security-policy intent. Every source entry carries an exact
`executable` or `control-artifact` kind, and every Debian trigger declaration
carries its exact `raw_line`. That declaration list is the sole persisted
trigger authority: the bundle does not retain parallel trigger-body or
trigger-name projections, and validation reparses the preserved control
artifact and requires an exact one-to-one match. Entry presence is the exact
lifecycle authority; there is no single-value decision tag or duplicated
preserved-entry counter. Only actual executed argv captured at the provider
process boundary can become selected-root or generation mutation authority.
Revision 18 accepts neither earlier nor unknown revisions: pre-alpha artifacts
and installed rows must be reconverted, rebuilt, or discarded instead of
migrated. Every executable source entry remains preserved; adding a lowering
requires a later typed schema contract with its own execution proof.

Revision 18 also names the exact package-origin authority `source_profile`.
When present it is one exact public supported-profile ID whose declared package
format must match `source_format`; family names, Remi route slugs, repository
display names, and the former ambiguous field name are not aliases. Pre-alpha
artifacts and database rows using the former field are rebuilt rather than
adapted.

Lifecycle format, phase, entry kind, Debian invocation, and source-specific
metadata discriminants are closed enums. Unknown strings and persisted fields
fail deserialization.

## Typed Development Failure, Not Human Reconciliation

A parser must either produce the complete typed contract for a source semantic
or produce a typed missing-semantic failure. It must not flatten an unknown slot
into a generic install/remove hook, silently omit an invocation mode, or guess
an interpreter.

An artifact with a valid current lifecycle bundle may be stored, advertised,
and served without an operator approving its script text. Before a requested
mutation begins, however, every event that operation can require must resolve
to an exact, source-independent plan and pass preflight. A missing semantic is a
pre-alpha implementation defect with machine-readable provenance. It is not a
private-review queue, a publication refusal, an invitation for an operator to
reinterpret shell, or an accepted permanent boundary for a supported format.

Every missing semantic becomes required parser/planner/executor work with an
upstream citation and a focused conformance test. A release cannot claim the
supported-format contract while a documented lifecycle shape reaches this
failure. Exact formal command evidence and typed missing-semantic classes
remain in private conversion diagnostics and focused fixtures; Remi does not
persist a separate clustering, backfill, note, packet, or human-review
workflow.

## Shared Transaction Model

The planner consumes a complete transaction change set:

- operation: install, upgrade, or remove;
- package name plus old and new versions where applicable;
- source-format version comparison scheme;
- installed instance counts before and after the completed change;
- exact archive paths added, retained, replaced, or removed;
- installed packages and paths after the transaction;
- lifecycle bundles for installing, removing, and trigger-owning packages.

Every installed package row has one mandatory typed version scheme.
Conary-authored packages are constructed with `conary`; RPM, Debian, and Arch
packages are constructed with `rpm`, `debian`, or `arch` from the parser or
selected repository contract. There is no placeholder scheme to patch after
construction. Repository provenance, parsed package identity, install
semantics, and an adopted package's exact native identity must agree before
insertion. Missing or contradictory scheme provenance is invalid state, not a
cue to guess from a distro, filename, repository, or version string. Changeset
metadata schema `conary.changeset.metadata.v6` is the only rollback-snapshot
contract accepted by the current pre-alpha build; superseded metadata is reset
with the database rather than adapted. Its typed installed-authority snapshot
retains the trove selection, pin, source, label, repository, versioning, and
native identity; components and their file bindings and relations; package
requirements and provides; config authority; package and file capabilities;
collection membership; provenance and installed-conversion lineage; lifecycle
and CCS remove contracts; and derived-package backlinks. Restoration validates
the complete graph before insertion and fails closed on missing or reassigned
references. The compensating changeset and selected generation are new
causation; historical changeset identity is not copied into restored rows.

The planner emits a typed transaction graph. Transaction-wide pre-events run
first. Each source transaction element then runs its pre-payload events,
crosses `ApplyPayload(change_index)`, runs the events that require the unpacked
payload, crosses `FinalizeOldPayload(change_index)`, and runs its remaining
element events before the next source element begins. Transaction-wide final
events run last. Stage sorting is confined to one graph boundary; it never
hoists every package's `%pre` ahead of every payload or delays every `%post`
until the final payload. Thus package B's `%pre` observes package A's completed
payload boundaries, while package A's `%post` cannot observe package B's
not-yet-applied payload. String ordering is permitted only where the source
contract itself specifies it, such as ALPM hook filenames.

### Payload Visibility

Lifecycle execution and payload mutation share one boundary model:

| Boundary | Required visible state |
| --- | --- |
| Before transaction | The previously committed package set and payload; no new transaction payload |
| Before new-package payload | Old payload remains visible on upgrade; new payload is not visible |
| After new-package unpack | New and overlapping files are visible; old-only files remain visible until the source manager's removal boundary |
| Before old-package removal | Old-only files and old ownership remain available to pre-removal lifecycle entries |
| After old-package removal | Old-only files and old ownership are absent; overlapping paths belong to the new package |
| After transaction | The final package set, payload ownership, and selected generation are visible |

Mutable-root and generation-aware installs must expose the same state at every
event. Mutable-root execution uses one filesystem journal and one SQLite
transaction around the graph. Generation-aware execution uses one isolated
selected-root journal so post-unpack code can observe the new payload while
old-only paths remain available without publishing an intermediate host
generation.

The selected root is publication authority. After the graph completes, Conary
captures its complete supported tree into the shared
`GenerationRootManifest` and `MutableStateManifest`: node kind, hardlink
identity, mode, source and resolved ownership, signed timestamp, xattrs, and
regular-file content authority all survive. Immutable content and mutable
state reference the same SHA-256 CAS used by package payloads. Generation,
retry, bootstrap EROFS, and try-session materialization consume those typed
manifests rather than reconstructing lifecycle effects from package rows.
RPM IMA signatures remain typed `security.ima` payload authority across that
staging boundary. Matching pinned RPM
[`plugins/ima.c`](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/plugins/ima.c#L60-L76),
`EOPNOTSUPP`, or `EPERM` for real UID zero, is non-fatal only when applying
that exact xattr; every other xattr/error pair remains fatal. A selected-root
filesystem that cannot apply the signature does not erase it: capture restores
the deferred value only while the regular-file SHA-256 and size are unchanged,
so a lifecycle content mutation invalidates the signature before generation
serialization.
Payload metadata application also distinguishes package-declared xattrs from
the target LSM label assigned when a staging node is created. An undeclared
`security.selinux` value is preserved as ambient target authority and becomes
part of selected-root capture; when the payload declares `security.selinux`,
that exact opaque value is applied instead. This is not a general
`security.*` exception: every other undeclared xattr is removed, and a removal
failure remains fatal.
Generation-local config projection removes exactly one `/etc` prefix when it
materializes the overlay upper; `/var` and `/srv` entries cannot enter that
upper. Boot-carrier export copies both manifests, reconstructs `/var` and
`/srv`, and seeds the same `/etc` upper from verified, manifest-listed CAS
objects. A read-only carrier may copy that seed to tmpfs for runtime writes,
but the copied seed is not a second state authority. Export also restores the
verified root node and manifest-owned `/usr`, `/etc`, and `/boot` mountpoint
metadata. A carrier's partition topology is distinct from the selected root's
source-host topology: its boot entry disables source-`fstab` generation and,
for writable disks, declares the carrier ESP by partition label. Export does
not rewrite the signed `/etc/fstab` seed to accomplish that projection.
Generation artifact manifest v3 seals the versioned target carrier-capability
projection. Export restores every required root symlink from its exact logical
manifest metadata, then applies the sealed opaque target value only to regular
files and directories in the copied carrier CAS subtree. The logical composefs
node and its external immutable backing object remain distinct security
subjects with distinct authorities. A generation with artifact manifest v2
must be rebuilt; export does not guess missing carrier authority. A bootstrap
target that is not running yet projects the same fact from its exact captured
`/usr` manifest node instead of inspecting or inheriting the build host.

A generation-aware root mutation records a typed selected-root publication
debt and durably installs its cumulative candidate before committing the
package database transaction. Every later root-changing transaction starts
from the newest committed cumulative authority and produces a replacement
candidate. Retry must fail closed if that candidate is missing or invalid; it
must never fall back to a database-only reconstruction. Completion,
abandonment, and rollback own deterministic candidate cleanup. Database-only
publication input is not part of the current contract.

Every normal event, exact-argv command, event-failure recovery branch, and
payload-failure recovery branch is preflighted before the first lifecycle
event, payload mutation, or database mutation. Preflight includes availability
of the selected-root interpreter/helper runtime; it does not wait until a later
stage to discover that an unwind path cannot execute. A failed preflight leaves
the previous payload, package database, and selected generation unchanged.
The event-time projection advances source-declared absolute path capabilities
at the same exact `ApplyPayload` and `FinalizeOldPayload` boundaries as literal
archive paths. Thus a dependency's typed `/bin/sh` provider can authorize a
later lifecycle interpreter even when its payload is represented through the
source filesystem layout as `/usr/bin/sh`; it cannot authorize an event before
that provider payload is applied or after its final provider is removed.
Package names, distro identities, script text, and hard-coded redirect tables
never establish this availability.

Lifecycle execution is inseparable from a mutating install, update, remove,
restore, batch, or autoremove transaction. The CLI, daemon request schema, and
internal transaction graph expose no script-suppression option. A dry run may
plan and report lifecycle without executing it because it performs no mutation;
an applied transaction must execute the complete typed graph or fail before its
first mutation.

### Source Payload Identity

RPM `FILEUSERNAME` and `FILEGROUPNAME` are source-format ownership authority.
Before consulting the selected root's account databases, Conary applies pinned
RPM's
[`rpmugUid()` and `rpmugGid()`](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/rpmug.cc#L164-L220)
contract: the distinguished RPM `root` user and group resolve directly to
numeric zero. Every other named RPM identity must resolve exactly once from
that root's exact passwd or group records before mutation. Converted CCS
retains this rule through its signed native RPM source semantics; author-native
CCS and other source formats cannot acquire it from a matching name. Installed
payload state records both the source identity and resolved numeric identity,
so later generation, rollback, query, and verification paths never repeat or
guess the resolution.

### Source Root Ownership Anchors

The selected root is the transaction container and cannot also be a package
payload path. An RPM may nevertheless declare that root in its source file
table as `DIRNAMES="/"` plus an empty `BASENAMES` value. RPM's pinned
[`fsmFsPath`](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/fsm.cc#L71-L82)
and
[`rpmfnFindFN`](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/rpmfi.cc#L388-L435)
contracts establish that header identity and its standard-CPIO association.

The RPM parser retains the exact root entry long enough to prove parallel
header-array validity, uniqueness, archive association, zero declared size,
zero payload content, and completeness. It accepts only an unflagged directory
without a digest, link target, file capabilities, IMA signature, or device-node
identity.
Conversion then consumes the source ownership anchor without creating a CCS
payload node, installed file row, payload claim, or remove authority for
`/`. Every non-root path still must become one canonical below-root deployment
path through the shared source-path authority; the root exception cannot
normalize another spelling into mutation authority.

### Shared Payload Ownership And Materialization

Every installed non-root payload path has one exact `payload_claims` row per
declaring trove. A claim retains the source node, content authority, component,
source-format sharing policy, and any typed directory materialization edge.
The corresponding `files` row is not an ownership list: it is the one
currently materialized selected-root node plus the claimant used as its
referential anchor. Composite foreign keys prevent a component from being
attached to another trove's claim or anchor.

Non-directory overlap remains exclusive unless both claims carry the same
source-owned sharing policy and its typed comparator accepts them. RPM uses
the pinned
[`rpmfilesCompare()`](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/rpmfi.cc#L898-L949)
contract: node type and mode must agree except that symlink permissions are
ignored; source user and group must agree; regular size and digest, symlink
target, and device identity must agree. Mtime and xattrs do not establish RPM
file-conflict authority. RPM ghost declarations have no payload node and
remain configuration ownership rather than fabricated payload claims.
Conary's explicit hardlink edges can share only when their target topology
agrees; the target claims independently prove the regular content. When a
compatible package extends an existing hardlink group, root mutation validates
the preserved regular target as an explicit graph reference and never rewrites
that target as a second payload. Materialized anchors use one deterministic
path-derived hardlink identity and target metadata across every edge, while
each claim retains its source archive identity. Generation validation resolves
that typed graph independent of lexical entry order. Debian, Arch, and
author-native CCS non-directory payloads remain exclusive until their own
pinned source contract establishes otherwise. Names, paths, distribution
labels, and allowlists never select compatibility.

An existing directory-directory overlap is therefore not a conflict and does
not discard either package's authority. The source package format, not the
target distribution or the archive used as transport, selects how the visible
directory is handled:

| Source contract | Existing materialized directory |
| --- | --- |
| RPM | Apply the incoming directory metadata |
| Debian/dpkg | Preserve the existing directory metadata |
| Arch/libalpm | Preserve the existing directory metadata |
| Conary-authored CCS | Apply the incoming directory metadata |

These choices are encoded from immutable upstream inputs in
`apps/conary/src/commands/install/shared_directory.rs`: RPM
[`lib/fsm.cc`](https://raw.githubusercontent.com/rpm-software-management/rpm/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/fsm.cc),
dpkg 1.23.7
[`src/main/unpack.c`](https://sources.debian.org/data/main/d/dpkg/1.23.7/src/main/unpack.c)
and
[`src/main/remove.c`](https://sources.debian.org/data/main/d/dpkg/1.23.7/src/main/remove.c),
and libalpm
[`lib/libalpm/add.c`](https://gitlab.archlinux.org/pacman/pacman/-/raw/a6f7467d8c7c4d7e9cc846884e74c0ab7215c48d/lib/libalpm/add.c).
Their revisions, paths, SHA-256 digests, and typed results are one production
contract. A CCS archive converted from a native package remains governed by
its validated `native_lifecycle.source_format` and matching version scheme;
the CCS container does not erase the RPM, Debian, or Arch source ABI.

Compatible overlap materializes the path once. Removing or upgrading a package
deletes only its claims while peers remain. Each affected `files` row is
re-anchored deterministically to the lowest surviving `(trove_id, claim_path)`
without rewriting visible node bytes. The final retainer authorizes physical
removal. The reported removal count reflects the filesystem result, so a
nonempty directory that remains is not reported as removed. Package query,
component, SBOM, runtime, derived-package, native-lifecycle, configuration
capture, file-capability projection, rollback, and removal all resolve package
ownership through claims. Generation export and CAS reachability consume the
single materialized anchor, which remains present until the final claim.

Live-root install, removal, rollback, and hardlink operations preserve that
package-path spelling as ownership authority while resolving existing ancestor
symlinks to one effective path inside the selected root. Transaction journals
record the resolved physical mutation path, so recovery uses the same target
that the forward operation changed. In-root layout aliases such as
`/lib64 -> usr/lib64` are therefore valid; an alias that escapes the selected
root or loops fails before mutation.

Rollback captures payload claims and the independently materialized node
before any package in a single or batch transaction mutates the selected root.
It resolves only the package path's existing selected-root ancestor symlinks
before applying the finite publication-domain contract. Thus a native spelling
such as `/var/run/example` retains its package ownership identity while its
effective `/run/example` materialization is excluded as ephemeral; aliases
into non-ephemeral domains still require exact captured nodes. Missing
non-ephemeral nodes and escaping or looping aliases fail before mutation.
Restoration first rebuilds all package bases, then all anchors, then all claims,
so cross-package and cyclic claim graphs do not depend on insertion order.
Additive RPM or CCS metadata changes restore the prior materialized node even
when the pre-existing anchor package itself was not removed. Native adoption
commits each package's files, payload claims, requirements, and provides
atomically under `AdoptedTrack` or `AdoptedFull`; package names and partial
warning paths are not ownership authority.

Schema revision 25 is a pre-alpha hard cut: it retains revision 24's exact
provider provenance and source-authority representation, and adds fail-closed
constraints for trigger and derived-package persisted states. It does not
migrate earlier databases. Operators rebuild disposable local state from
authoritative package and repository inputs.

A complete unfiltered full-system adoption also performs one exact global
selected-root capture after every native package capture succeeds. Existing
package anchors and claims retain ownership. Only retained paths with no
package owner are assigned to one typed `CapturedRoot` authority, which
generation construction consumes alongside other complete payload sources.
Each node is captured through a bounded, pointwise-stable descriptor window
while the tree is walked; hardlink topology is derived from those captured
snapshots rather than from metadata staged before content capture. The capture
preserves global hardlink topology and exact numeric ownership, mode/type,
timestamps, symlink targets, CAS content, and xattrs. Its finite domain contract
retains immutable paths plus `/etc`, `/var`, and `/srv`, while excluding
`/proc`, `/sys`, `/dev`, `/run`, `/tmp`, `/home`, `/root`, `/mnt`, `/media`,
and Conary's own normalized runtime/database subtrees. Runtime and database
exclusions cover both lexical and resolved authority so aliases cannot
reintroduce self-capture, and a runtime root resolving to `/` fails closed.
Runtime generation collection therefore has no source-package-manager or
source-database dependency. `CapturedRoot` is migration-continuity input, not
package install/update/remove ownership. A complete capture refuses stale
`AdoptedTrack` identities absent from the current native inventory rather than
letting metadata-only ownership suppress their paths from the generation.

## Configuration Transaction Contract

Configuration handling is part of the package transaction, not a warning or a
post-install reconciliation task. The authoritative inputs are the package
format's typed config declaration plus three exact content identities:

- `O`: the baseline installed by the previously owned package version;
- `C`: the file or symlink currently visible in the selected root, including
  an intentional absence;
- `N`: the incoming package artifact.

Foreign conversion copies each native parser declaration into signed CCS v3
authority without a path heuristic or package-wide policy collapse. The
per-path authority preserves `noreplace`, RPM ghost ownership, and Debian
remove-on-upgrade. Verified CCS construction projects those signed values into
the named fallible `PackageFormat::config_declarations()` transaction
projection used by direct native installation. Each declaration retains its
source kind and matched/absent payload association; the signed native source
format must agree and selects the exact RPM, Debian, or Arch transaction table
below. An absent ALPM backup declaration is durable authority but performs no
filesystem mutation and supplies no content identity.

The shared decision engine is
`crates/conary-core/src/config_transaction.rs`. Mutable-root journaling and
generation-overlay materialization are adapters around that one engine; they
must produce the same primary and auxiliary paths.

Authoritative source contracts:

- [RPM spec configuration files](https://rpm.org/docs/6.0.x/manual/spec.html#configuration-files)
  and [RPM file actions](https://ftp.osuosl.org/pub/rpm/api/4.18.0/rpmfiles_8h.html);
- [`dpkg(1)` conffile processing and file suffixes](https://manpages.debian.org/trixie/dpkg/dpkg.1.en.html)
  and [`deb-conffiles(5)`](https://manpages.debian.org/unstable/dpkg-dev/deb-conffiles.5.en.html);
- [`pacman(8)` config-file handling](https://man.archlinux.org/man/pacman.8.en#HANDLING_CONFIG_FILES).

### Install And Update

The identity matrix starts with format-independent cases:

| Exact state | Action |
| --- | --- |
| `C = N` | Install/expose `N`; no auxiliary copy |
| `C = O` | Install/expose `N`; no auxiliary copy |
| `O = N`, while `C` is a different artifact | Keep `C`; the package version did not change this path |
| No `O` and no `C` | Install/expose `N` |

When both the local state and incoming package changed, or a package first
claims an existing path, the typed source contract decides the remaining
action:

| Source contract | Existing primary | Incoming artifact | Installed primary |
| --- | --- | --- | --- |
| RPM `%config(noreplace)` | Keep `C` | `path.rpmnew` | `C` |
| RPM replacing `%config` update | Save `C` as `path.rpmsave` | Expose `N` | `N` |
| RPM replacing `%config` first claim | Save `C` as `path.rpmorig` | Expose `N` | `N` |
| Debian conffile | Keep `C` | `path.dpkg-dist` | `C` |
| Arch backup file | Keep `C` | `path.pacnew` | `C` |
| Conary `Auto`, replacing | Save `C` as `path.conary-save` | Expose `N` | `N` |
| Conary `Auto`, `noreplace` | Keep `C` | `path.conary-new` | `C` |

Debian treats a deleted conffile as a local edit. If `C` is absent and `O = N`,
the deletion remains. If `C` is absent and `O != N`, the primary remains
absent and `N` is written as `.dpkg-dist`. A generation upper represents that
absence with an overlay whiteout. RPM and Arch recreate a missing packaged
config according to their documented contracts.

`remove-on-upgrade /path` is a typed Debian conffile declaration, not part of
the pathname. The path must be absent from the incoming payload. On upgrade,
Conary removes an unchanged old conffile, renames a locally modified one to
`.dpkg-old`, removes a stale `.dpkg-dist`, and ignores the declaration while
another installed package owns the path. Mutable-root and generation
transactions persist the same operation and rollback snapshot.

Generation config transaction schema version 4 is the only current contract.
Regular current, incoming, and auxiliary artifacts carry exact SHA-256 plus
`u64` size authority and reopen from CAS; the transaction never serializes
inline base64 file contents. Materialization streams and re-verifies each
object, publication keeps every pending reference live through generation GC,
and missing or corrupt content fails before publication. `Remove` and `Purge`
entries carry the exact prior `ConfigPackageState`, including `ConfigSource`;
validation rejects a missing prior state before any generation staging path is
created. Removal never substitutes `Auto` or any other source because the
source changes residual retention and backup suffix semantics.

Every regular file or symlink below `/etc` participates even when the archive
does not declare native config metadata. Such a path is persisted as
`ConfigSource::Auto` and uses the deterministic `.conary-new` or
`.conary-save` contract. Directories and special files remain ordinary typed
payload ownership. No path is sent to a manual conflict queue.

RPM `%ghost %config` is ownership without payload: install and update neither
create nor back up the path. Erase removes the path if it exists. Ghost
metadata must never be converted into an empty payload artifact.

### Remove And Purge

| Source contract | Ordinary remove | Purge |
| --- | --- | --- |
| RPM config | Remove pristine/absent primary; save a modified primary as `.rpmsave` | Remove primary |
| RPM ghost config | Remove the path if present; never create a backup | Remove the path |
| Debian conffile | Retain the exact current artifact or intentional absence as residual config state | Remove primary, `.dpkg-dist`, and `.dpkg-old` |
| Arch backup file | Remove pristine/absent primary; rotate numeric `.pacsave` history and save a modified primary as `.pacsave` | Remove primary |
| Conary `Auto` | Remove pristine/absent primary; save a modified primary as `.conary-save` | Remove primary |

Arch rotation is structural: existing `.pacsave.N` entries move to
`.pacsave.(N+1)` in descending numeric order, existing `.pacsave` becomes
`.pacsave.1`, and the current modified primary becomes `.pacsave`. Numeric
suffix parsing is the documented auxiliary grammar, not a semantic heuristic.

### Durable Generation Publication

Before package state commits, a generation-aware mutation captures a typed
snapshot for every affected `/etc` path: operation, source contract,
`noreplace`/ghost flags, `O`, exact `C`, exact `N`, mode or symlink target, and
existing auxiliary artifacts. The snapshot is inserted into
`generation_publications.config_transaction_json` in the same SQLite
transaction as the package mutation.

Publication clones the previous generation upper into a private staging
directory, applies the shared decisions, syncs the tree, atomically renames it
to the unpublished generation upper, and only then publishes the generation
link. A failed publication retains the exact snapshot; retry reuses that debt
row and cannot degrade into an empty or freshly inferred transaction. Modified
primaries, deletions/whiteouts, auxiliary rotation, file modes, and symlink
targets therefore survive retry with mutable/generation parity.

### Atomic Upgrade Failure

Native install, upgrade, removal, config, lifecycle, and trigger work is one
selected-root transaction. Before SQLite commits, any failure discards the
isolated root and rolls back package state; Conary does not persist a second
native-upgrade rollback schema or mutate a host root and compensate afterward.
Once SQLite commits, the exact selected-root candidate and typed publication
debt are the retry authority. Config auxiliary ownership remains defined by the
documented suffix grammar and the forward `GenerationConfigTransaction`.

## RPM

Authoritative references:

- [`rpm-scriptlets(7)`](https://rpm.org/docs/latest/man/rpm-scriptlets.7)
- [`rpm-queryformat(7)`](https://rpm.org/docs/latest/man/rpm-queryformat.7)
- [`rpm-lua(7)`](https://rpm.org/docs/latest/man/rpm-lua.7)
- [`rpm-version(7)`](https://rpm.org/docs/latest/man/rpm-version.7)
- [RPM package state machine (`lib/psm.cc`)](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/psm.cc)
- [RPM transaction orchestration (`lib/transaction.cc`)](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/transaction.cc)
- [RPM transaction ordering (`lib/order.cc`)](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/order.cc)
- [RPM trigger implementation (`lib/rpmtriggers.cc`)](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/rpmtriggers.cc)
- [RPM dependency-set comparison (`lib/rpmds.cc`)](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/rpmds.cc)

The manual defines the public ABI. The pinned source defines the exact runtime
ordering when a prose summary combines owner-side and installed-database
trigger passes.

RPM `Provides` is typed package authority, not a flattened display string.
Conary preserves the capability kind, name, architecture qualifier, version
scheme, relation, and EVR boundary from the header. `<`, `<=`, `=`, `>=`, and
`>` are all valid provider relations. Matching reproduces `rpmdsCompare` range
overlap, including inclusive/exclusive endpoints, unversioned existence
provides, and RPM's partial-EVR equality behavior. It never substitutes the
owning package EVR or reinterprets the range through another ecosystem.

RPM rich requirements are typed source ABI, not text to split later. The
parser is pinned to RPM
[`rpmrichParseInternal()`](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/rpmds.cc#L1309-L1396)
and carries `and`, `or`, `if`, `unless`, `else`, `with`, and `without` into
`RepositoryRequirementExpression`. Only `and`, `or`, and `with` may repeat at
one level; repeated `with` is represented by RPM's right-side chain. RPM's
`=<`, `==`, and `=>` comparison aliases canonicalize to `<=`, `=`, and `>=`
before native RPM version evaluation, while `!=` is not an RPM dependency
operator. Requires-like and conflict-like tag contexts retain RPM's distinct
`if`/`unless` nesting checks. The implementation never recognizes a dependency
by package name or adds a skip/allowlist for repository metadata.

RPM install prerequisites are typed ordering authority, not ordinary
dependency text. Direct RPM headers map legacy `Prereq` and `%pre`/`%post`
dependency senses to `PreDepends`; RPM-MD maps the exact signed `pre="1"`
Requires marker to the same kind. `%pretrans`, `%posttrans`, erase, verification,
runtime-feature, and meta senses remain ordinary dependency satisfaction
without becoming strong install edges, matching RPM's `isInstallPreReq()` and
`isUnorderedReq()` transaction-order masks. Conversion preserves the kind in
signed CCS authority and installed requirement state. Within a dependency
cycle, transaction ordering must satisfy those strong edges before applying
the deterministic fallback for the remaining cycle; package names and
capability strings never establish that priority.

### RPM Lifecycle Surface

| Surface | Slots | Invocation contract |
| --- | --- | --- |
| Package scripts | `%pre`, `%post`, `%preun`, `%postun` | `$1` is the installed instance count after the containing operation |
| Transaction scripts | `%pretrans`, `%posttrans`, `%preuntrans`, `%postuntrans` | `$1` is the installed instance count after the containing operation |
| Verification | `%verify` | Verification-only; never part of install, upgrade, or erase |
| Package triggers | `%triggerprein`, `%triggerin`, `%triggerun`, `%triggerpostun` | `$1` is the trigger-owner count and `$2` the triggering-package count after the operation |
| Package file triggers | `%filetriggerin`, `%filetriggerun`, `%filetriggerpostun` | `$1` and `$2` as above; matching absolute paths on stdin, one per line |
| Transaction file triggers | `%transfiletriggerin`, `%transfiletriggerun`, `%transfiletriggerpostun` | `$1` is the trigger-owner count; matching paths on stdin except that `postun` has no path list |

Trigger conditions are package names with optional `<`, `<=`, `=`, `>=`, or
`>` EVR constraints. Conary evaluates them with the RPM version scheme, not
lexicographic or semver comparison. Multiple conditions on one trigger are an
OR, and one trigger runs once even when multiple conditions match.

File trigger prefixes are complete absolute byte prefixes. RPM compares the
prefix length directly: `/usr/lib` also matches `/usr/libfoo`, while
`/usr/lib/` does not. Conary does not trim a trailing slash or invent a path
component boundary. Package file
triggers run once per triggering package. Transaction file triggers run once
per transaction. An installed-database `%transfiletriggerin` owner receives
matching paths added by the transaction. A transaction-owned immediate
`%transfiletriggerin` receives every matching path in the final package
database, including paths that predated the transaction;
`%transfiletriggerun` receives matching removed paths; and
`%transfiletriggerpostun` is selected by removed paths but receives no path
list.

The default file-trigger priority is `100000`. Priorities greater than or equal
to `10000` are the high class; lower priorities are the low class. Larger
priorities execute first within a class. Transaction file-trigger priority is
persisted but must not be used for ordering while RPM itself does not implement
that priority.

RPM treats an upgrade as installation of `new` followed by erasure of `old`.
With one installed old instance, new-side package and transaction scripts see
an install-side count of `2`, while old-side removal scripts see the completed
count of `1`. `%triggerprein` observes counts before the new header enters the
RPM database; `%triggerin` and immediate `%filetriggerin` observe the temporary
install-side count. Uninstall-side trigger arguments use RPM's removal count
correction. The planner derives each of these values from the typed before and
after counts; it does not collapse an upgrade to one final count.

The parser also preserves runtime expansion (`-e`/`EXPAND`), header queryformat
expansion (`-q`/`QFORMAT`), critical flags, interpreter vectors, and embedded
interpreters such as `<lua>`. Those are executable ABI semantics. A plain
process executor cannot run such an entry; the corresponding RPM expansion or
embedded-interpreter contract is required Conary implementation.

The persisted effective critical flag owns RPM script-result handling. Matching
the pinned RPM
[`runScript()`](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/transaction.cc#L1709-L1762),
a failed non-critical script is reported but does not fail its transaction
element, while a critical script failure remains fatal. Conary applies that
source policy only to a program exit or timeout after exact preflight and the
selected-root enforcement boundary have succeeded. Missing interpreters,
malformed contracts, process or sandbox setup failures, and enforcement
failures remain fatal; RPM criticality is not a security-boundary bypass.

### RPM Runtime Compatibility

Conary persists the typed package-header facts, transaction-derived installed
filenames, package-header macro facts, install prefixes, and producer RPM
version required by the RPM runtime. Query-format headers also persist their
typed locale, timezone, and verbose-query inputs. The current deterministic
contract is the POSIX C locale with UTC or an explicit fixed offset; it never
inherits the conversion host's process locale, timezone, or verbosity. Conary applies body transforms in
RPM source order: macro expansion first, then header query-format expansion. It
does not read a host RPM database, load host RPM configuration, invoke an RPM
binary, or infer macro values from a target distro name.

The bundled query-format parser supports placeholders and width, locked and
parallel-array iterators, presence expressions, escapes, case-insensitive tag
lookup, numeric tag lookup, and these formatters:

`armor`, `arraysize`, `base64`, `date`, `day`, `depflags`, `deptype`, `expand`,
`fflags`, `fstate`, `fstatus`, `hashalgo`, `hex`, `humaniec`, `humansi`, `json`,
`octal`, `perms`, `permissions`, `pgpsig`, `shescape`, `string`, `tagname`,
`tagnum`, `triggertype`, `vflags`, and `xml`.

The runtime derives RPM's bundled computed header extensions from the persisted
typed facts. This includes file classes, MIME types, per-file dependency
projections, trigger conditions and types, 64-bit size projections, dependency
NEVR arrays, header color, hard-link counts, sysusers declarations, OpenPGP
signature projection, database instance, and RPM format version. A genuinely
absent known tag retains RPM's `(none)` result, including the `VERBOSE`
extension when the persisted verbose-query input is false; an unknown tag or
formatter is a query-format contract error.

The macro parser implements RPM's delimiter grammar for braced and unbraced
calls, literal percent escapes, positive and negative conditionals, scoped
definitions, recursive expansion, parameter frames and automatic option
macros, `%()` shell expansion, `%[]` expressions, and the builtin table shipped
by the pinned RPM 6.1.90 runtime. `%{rpmversion}` reports that emulated runtime
version, not the producer RPM version retained as package provenance. Shell
expansion uses `/bin/sh -c` in the install target with the
scriptlet's sanitized environment, timeout, and sandbox mode; it never executes
on the conversion host. RPM's nonzero-status and trailing-CR/LF rules are
preserved. The persisted package context supplies `name`, `version`, `release`,
`epoch`, `arch`, and `os` when present. Unknown macros remain literal as RPM
requires; Conary does not infer source-distro configuration such as `_libdir`
from a distro name.

`<lua>` entries run in a bundled Lua 5.4 VM with a memory ceiling and
instruction deadline. The common base language plus `coroutine`, `table`,
`string`, `utf8`, and `math` are available. The mutable `macros` table preserves
RPM's scalar-versus-parametric behavior, including exact table arguments,
automatic `opt`/`arg` frames, programmatic parametric definitions, and
split/unsplit quoting. The pinned RPM module supplies base64, macro, version,
file, glob, hook, input, exact-argv execute, and spawn APIs. Spawn
`stdin`/`stdout`/`stderr` actions resolve inside the install target and preserve
RPM's append and status-result contract.

The bundled `io`, `os`, `posix`, `package`, `require`, `loadfile`, and `dofile`
surfaces resolve files, accounts, working directories, and pure-Lua modules in
the install target, never on the conversion host. Native commands use the
target `PATH`, sanitized environment, timeout, and sandbox mode. Removed RPM
fork/exec/wait APIs retain their upstream removal errors; the safe `debug`
projection exposes introspection but cannot replace Conary's instruction
deadline hook. `posix.stat` returns RPM's selected value or table on success
and `(nil, path-qualified strerror, errno)` when target `lstat` fails, while a
root-confinement violation remains fatal. `posix.dir` and `posix.files`
likewise return RPM's three-value error result when target `opendir` fails;
successful calls return the bundled table or iterator contract, including `.`
and `..`, and confinement failures remain fatal. The remaining bundled
`lposix` filesystem calls preserve the pinned
[`pushresult`/`pusherror`](https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/rpmio/lposix.cc#L104-L127)
contract: a numeric or string result on success and
`(nil, path-qualified strerror, errno)` on syscall failure, except for upstream
operations such as hard-link and symlink creation whose error string is
intentionally not path-qualified. All target paths use one symlink-aware
root-confinement resolver, and confinement failures remain fatal rather than
becoming an ordinary syscall result. APIs backed by leaf-sensitive filesystem
calls (`lstat`, `readlink`, removal, rename, and link creation) resolve
selected-root aliases only in the parent path and retain the exact final
directory entry; APIs whose source syscall follows the leaf retain full
dereferencing.

Standard `io.open` accepts Lua 5.4's pinned `[rwa]%+?b*` mode grammar and
returns `(nil, path-qualified strerror, errno)` when the target `fopen` fails.
RPM's separate `rpm.open` API retains its pinned exception-on-open-failure
contract. Path validation and selected-root confinement occur before either
open and remain fatal.

Target-confined `io` file reads preserve the pinned Lua
[`g_read()`](https://github.com/lua/lua/blob/v5.4.8/liolib.c#L534-L581)
format grammar: an optional leading `*` is skipped and the first remaining
format character selects number, line, line-with-newline, or read-all
behavior. This includes compatible spellings such as `*all`; Conary does not
replace the upstream grammar with a curated spelling list.

### RPM Upgrade Order

For one package upgrade, `new` is the installing version, `old` the removing
version, `rpmdb` an already installed trigger owner, and `any` installed plus
transaction packages. Conary's typed stages preserve this order:

1. `%pretrans` of `new`.
2. `%preuntrans` of `old`.
3. `%transfiletriggerun` of `any`, selected by paths removed with `old`.
4. RPM's implicit sysusers operation for `new`, through the exact persisted
   target sysusers interface with the selected root supplied explicitly.
5. `%triggerprein` passes between `rpmdb`, `new`, and the installing change.
6. `%pre` of `new`.
7. Unpack `new` and make its payload visible.
8. High-priority `%filetriggerin` passes for installed and transaction owners.
9. `%post` of `new`.
10. `%triggerin` passes for installed and transaction owners.
11. Low-priority `%filetriggerin` passes.
12. High-priority `%filetriggerun` passes selected by `old`.
13. `%triggerun` passes selected by `old`.
14. `%preun` of `old`.
15. Low-priority `%filetriggerun` passes.
16. Remove old-only payload paths and old ownership.
17. High-priority `%filetriggerpostun` passes.
18. `%postun` of `old`.
19. `%triggerpostun` passes selected by `old`.
20. Low-priority `%filetriggerpostun` passes.
21. `%posttrans` of `new`.
22. `%postuntrans` of `old`.
23. Installed-database `%transfiletriggerin`, with matching paths added by the
    transaction on stdin.
24. `%transfiletriggerpostun` selected by removed paths.
25. Transaction-owned immediate `%transfiletriggerin`, selected by the final
    package database and receiving every matching final path on stdin.

The two owner directions within a trigger pass remain distinct typed events
even when they share a stage. RPM's source package order and trigger priority
provide the intra-stage key; incidental database row order does not.

Initial install uses the installing half of this graph and final erase uses the
removing half. Instance arguments come from the exact RPM state-machine
boundary above; they must not be replaced by the completed transaction count
or an install/remove Boolean.

## Debian

Authoritative references:

- [Debian Policy: maintainer scripts and installation procedure](https://www.debian.org/doc/debian-policy/ch-maintainerscripts.html)
- [`deb-triggers(5)`](https://manpages.debian.org/trixie/dpkg-dev/deb-triggers.5.en.html)
- [`dpkg-maintscript-helper(1)`](https://manpages.debian.org/trixie/dpkg/dpkg-maintscript-helper.1.en.html)
- [dpkg source](https://sources.debian.org/src/dpkg/)
- [`init-system-helpers` 1.69~deb13u1 source](https://sources.debian.org/src/init-system-helpers/1.69~deb13u1/)
- [`init-system-helpers` 1.69~deb13u1 manpages](https://manpages.debian.org/trixie/init-system-helpers/)

### Debian Dependency Architecture Authority

The dependency architecture contract is pinned to dpkg commit
[`7004a048f4b122c133f1b08661be1399ce0a4dd7`](https://git.dpkg.org/cgit/dpkg/dpkg.git/tree/lib/dpkg/depcon.c?id=7004a048f4b122c133f1b08661be1399ce0a4dd7)
and APT commit
[`b29056212da5d43ac8ac23ebe4c8e90f63166e06`](https://salsa.debian.org/apt-team/apt/-/blob/b29056212da5d43ac8ac23ebe4c8e90f63166e06/apt-pkg/deb/deblistparser.cc).
Conary parses the package name and its architecture qualifier into separate
fields and persists `Multi-Arch` as the closed `no`, `same`, `allowed`, or
`foreign` enum. An absent Debian `Multi-Arch` field means `no`; an unknown
value is invalid metadata. Every repository package must carry an explicit
architecture before it can become a solver candidate.

Native package parsing and signed conversion preserve the source token exactly:
Debian `all` is not rewritten to RPM `noarch`, and Arch `any` is not rewritten
to either value. Compatibility consumes the pair `(package scheme, exact
token)` through the typed target-machine model. The stored token remains
unchanged through repository, CCS, installed-state, snapshot, rollback, and
same-format export paths.

For `Depends`, `Pre-Depends`, `Recommends`, and `Suggests`, the exact provider
rules are:

- an unqualified dependency selects the depending package's effective
  architecture, while a `Multi-Arch: foreign` provider may satisfy it across
  architectures;
- `package:any` selects only a `Multi-Arch: allowed` provider, including when
  that provider has the depending package's architecture;
- `package:native` selects the configured native architecture;
- `package:<architecture>` selects that exact architecture; and
- `Architecture: all` is evaluated as the configured native architecture.

`Provides` has its own typed architecture authority. A provide may be
unqualified, explicitly wildcarded as `:any`, or qualified by one exact
architecture token. An unqualified provide inherits its owner's effective
architecture. An explicit exact qualifier supplies the virtual capability's
architecture independently of the owner's architecture. A literal `:native`
in `Provides` is therefore an exact architecture token, not the dependency-side
native selector. An explicit `:any` provide satisfies a `:any` dependency even
when its owner is not `Multi-Arch: allowed`, but does not satisfy an
unqualified cross-architecture dependency unless its owner is
`Multi-Arch: foreign`. Virtual provides also inherit the owner package's
`Multi-Arch: foreign` and `Multi-Arch: allowed` behavior.

A Debian provide is either unversioned or carries one exact `= version`;
alternatives, comparison relations, empty atoms, and malformed qualifiers are
invalid repository metadata. An unversioned provide never satisfies a
versioned dependency, and the owning package version is never substituted for
a missing provide version.

Debian `Conflicts`, `Replaces`, and `Breaks` remain any-architecture relations
as dpkg specifies. No resolver stage recovers a qualifier by looking for a
colon suffix, infers `Multi-Arch` from a package name, substitutes a package
version for provider authority, or treats missing repository architecture as a
universal match.

Debian lifecycle authority consists of the `config`, `preinst`, `postinst`,
`prerm`, `postrm`, and `triggers` control members. Maintainer-script bodies use
their shebang interpreter. The same member has several typed invocation modes;
the member name alone is not a complete ABI.

### Debian Invocation Surface

The persisted contract must retain every documented argv shape:

| Member | Invocation modes |
| --- | --- |
| `config` | `configure [installed-version]`; `reconfigure [installed-version]` |
| `preinst` | `install [old-version new-version]`; `upgrade old-version new-version`; old `abort-upgrade new-version` |
| `postinst` | `configure [most-recently-configured-version]`; `triggered trigger-name...`; old `abort-upgrade new-version`; `abort-remove [in-favour package new-version]`; `abort-deconfigure in-favour failed-install-package version [removing conflicting-package version]` |
| `prerm` | `remove [in-favour package new-version]`; old `upgrade new-version`; `deconfigure in-favour package-being-installed version [removing conflicting-package version]`; new `failed-upgrade old-version new-version` |
| `postrm` | `remove`; `purge`; old `upgrade new-version`; `disappear overwriter overwriter-version`; new `failed-upgrade old-version new-version`; `abort-install [old-version new-version]`; `abort-upgrade old-version new-version` |

Optional trailing arguments are omitted, not replaced with an empty string.
Error-unwind calls form a conditional transition graph; they are not the
normal calls replayed in reverse. Conary must persist that graph and schedule
the documented recovery call for the stage that failed.

### Debian Maintainer Environment

Conary owns the dpkg maintainer-script process environment from typed package
and transaction state. The implemented contract follows
[`dpkg(1)`'s internal environment](https://manpages.debian.org/unstable/dpkg/dpkg.1.en.html#Internal_environment)
and dpkg 1.23.7
[`src/main/script.c`](https://sources.debian.org/src/dpkg/1.23.7/src/main/script.c/):

- `DPKG_ROOT` is empty because Conary uses dpkg's normal live-root or chrooted
  target-root execution model.
- `DPKG_ADMINDIR` is `/var/lib/conary/dpkg-compat`. Package metadata cannot
  override that reserved path or redirect helpers to a host dpkg database.
- `DPKG_MAINTSCRIPT_PACKAGE`, `DPKG_MAINTSCRIPT_ARCH`, and
  `DPKG_MAINTSCRIPT_NAME` come from the persisted Debian bundle and its typed
  `preinst`, `postinst`, `prerm`, or `postrm` control member.
- `DPKG_MAINTSCRIPT_PACKAGE_REFCOUNT` is the number of package instances whose
  state is greater than `not-installed`. Installing and installed owners use
  the post-transition count; removing owners use the pre-removal count. A
  fresh `preinst install` therefore receives `1`, because dpkg records the new
  package as `half-installed` before invoking it.
- `DPKG_MAINTSCRIPT_DEBUG` is `0`, and `DPKG_RUNNING_VERSION` is `1.23.7`, the
  upstream revision from which this maintainer-environment contract is
  derived. It does not assert helper availability.

`config` is a debconf frontend script, not one of dpkg's four named maintainer
scripts, so it does not receive an invented `DPKG_MAINTSCRIPT_NAME`. Exact
debconf frontend execution remains a separate typed runtime gap.

`/var/lib/conary/dpkg-compat` is Conary's dpkg-compatible administrative
projection. Before every normal or recovery maintainer-script event, Conary
rebuilds it from the exact installed lifecycle rows, conffile rows, trigger
state, and in-flight `NativeTransactionState` for that boundary. The projection
contains dpkg 1.23.7's `status`, `info/format`, package `.list`, `.conffiles`,
and `.triggers` files, explicit and file-trigger registries, `Unincorp`, locks,
and the other mandatory administrative files. A fresh `preinst` therefore sees
the incoming package as `half-installed` with an empty file list; upgrade and
recovery events see the corresponding old/new event-time view.

After an event, Conary strictly parses `dpkg-trigger`'s deferred activation
grammar back into typed pending/awaited state and captures exact live conffile
hash/status changes. Invalid, ambiguous, symlinked, or incomplete projected
state aborts the package transaction. The reserved directory is accepted only
when it carries Conary's format marker, and this runtime refuses `/` as its
selected root. It never reads or writes `/var/lib/dpkg`.

### Debian Lifecycle Service Helper Grammar

`crates/conary-core/src/packages/deb/lifecycle_helpers.rs` and its child modules
are the single typed argv authority for `deb-systemd-helper`,
`deb-systemd-invoke`, `invoke-rc.d`, `update-rc.d`, and `service`. The contract
is pinned to Debian trixie's `init-system-helpers` `1.69~deb13u1`; the module
records each exact Debian Sources URL and source-file SHA-256. It only parses
and canonically renders argv. It does not execute a helper, inspect policy or
target state, or mutate the selected root.

The fixed grammars cover:

- `deb-systemd-helper`'s `--quiet`, `--user`, and `--system` options and all ten
  documented unit-state actions;
- `deb-systemd-invoke`'s system/user instance selection, `--no-dbus`, the
  start/stop/restart unit shape, and the daemon-reload/daemon-reexec manager
  shape;
- every `invoke-rc.d` option, all eight standard init actions, exact trailing
  init-script parameters, the documented custom-action branch, status codes
  0-106, and policy allow, deny, uncertain, fallback, and unsupported results;
- `update-rc.d`'s force/help options, remove/defaults/defaults-disabled,
  enable/disable runlevels, and its still-present legacy start/stop sequence
  branch as typed two-digit clauses; and
- `service` help/version/status-all, full-restart, every standard init action,
  custom init actions, exact trailing parameters, and the three status-all
  markers.

The typed grammar is deliberately stricter than permissive implementation
accidents. Unknown `deb-systemd-helper` actions are errors instead of the Perl
script's silent success. `deb-systemd-invoke` manager actions follow the
published one-action/no-unit shape despite the script's pre-option argument
count bug, and `--no-dbus` is valid only for those manager actions. Options or
runlevels that the pinned scripts would ignore, discard, abbreviate, or merely
warn about are rejected rather than becoming compatibility defaults. Custom
action words remain available only where the upstream contract explicitly
passes them to an init script: `invoke-rc.d` and `service`.

### Debian Normal Transaction Order

| Operation | Exact happy-path sequence |
| --- | --- |
| Fresh install | `new-preinst install`; unpack payload; update conffiles; `postinst configure` with no previous-version argument |
| Upgrade | `old-prerm upgrade new`; `new-preinst upgrade old new`; unpack/replace new paths; `old-postrm upgrade new`; remove old-only paths and commit the new file list; update conffiles; `new-postinst configure old` |
| Relation install | Deconfigure every exact `Breaks` target and newly broken hard dependent with `prerm deconfigure in-favour ...`, deepest dependent first; run conflict removals with `prerm remove in-favour ...`; then run the incoming `preinst` and continue its install |
| Remove | `prerm remove`; remove non-conffile payload; `postrm remove` |
| Purge | Complete remove, delete conffiles and backups, then `postrm purge` |

On upgrade, `old-postrm upgrade` observes the unpacked new payload while
old-only files are still retained. The old-only removal boundary occurs after
that call and before `new-postinst configure`. A generation-aware transaction
must expose the same intermediate and final views.

Relation deconfiguration changes package state and trigger state but does not
remove its payload or Conary ownership. The planner carries the exact incoming
identity and optional exact conflict-removal identity into the revision-11
argument contract. A deconfiguration target must carry an installed Debian
lifecycle bundle; another source ABI cannot be treated as Debian merely
because a relation matched. If the incoming transaction fails, successful
conflict removals unwind in reverse order with `postinst abort-remove`,
followed by successful deconfigurations in reverse order with
`postinst abort-deconfigure`. A failed deconfiguration first runs its own
`abort-deconfigure`, then unwinds earlier deconfigurations. Successful cleanup
restores `installed`; failed cleanup persists `half-configured`.

### Debian Triggers

`DEBIAN/triggers` is a strict control artifact. Current directives are
`interest`, `interest-await`, `interest-noawait`, `activate`,
`activate-await`, and `activate-noawait`, each followed by exactly one trigger
name. Unknown directives are package errors.

Activation and interest are separate typed relationships. Await/noawait changes
the triggering package's dpkg state and therefore scheduling; it is not
diagnostic metadata. When pending triggers are processed, the interested
package's `postinst` is invoked as `triggered trigger-name...`. The planner
deduplicates names, respects await semantics, and runs each interested package
at the corresponding dpkg processing boundary.

### `dpkg-maintscript-helper`

The exact command grammar is:

```text
dpkg-maintscript-helper ACTION ACTION_ARGS [PRIOR_VERSION [PACKAGE]] -- "$@"
```

The four actions are `rm_conffile`, `mv_conffile`, `symlink_to_dir`, and
`dir_to_symlink`. The action arguments, optional version/package, `--`
separator, and forwarded maintainer-script argv must have formal AST
provenance. Paths and owning package identity validate against payload and
persisted configuration state.

An exact `dpkg-maintscript-helper/v1` effect requires
`/usr/bin/dpkg-maintscript-helper`, `/usr/bin/dpkg-query`, and `/usr/bin/dpkg`
in the event-time selected-root path projection. Missing capability fails
transaction preflight with the package and exact missing path. The preserved
maintainer script then runs the upstream helper against Conary's administrative
projection; recognizing the grammar does not suppress or replace the script.
Conffiles carry their package-shipped MD5 solely for this dpkg compatibility
ABI while SHA-256 remains Conary's integrity authority.

### `update-alternatives`

The implemented non-interactive grammar follows
[`update-alternatives(1)`](https://manpages.debian.org/unstable/dpkg/update-alternatives.1.en.html)
and dpkg 1.23.7
[`utils/update-alternatives.c`](https://sources.debian.org/src/dpkg/1.23.7/utils/update-alternatives.c/):

```text
update-alternatives --install LINK NAME PATH PRIORITY
                    [--slave LINK NAME PATH]...
update-alternatives --remove NAME PATH
update-alternatives --auto NAME
```

Only those exact typed `alternatives-registration/v1` argv shapes are modeled.
Interactive, broad, reordered, or option-augmented forms remain an explicit
semantic gap; they are not inferred from script text. A modeled invocation
requires `/usr/bin/update-alternatives` in the event-time selected root.

The helper treats `DPKG_ADMINDIR` as a base, so its records live under
`/var/lib/conary/dpkg-compat/alternatives`, while its generic and selected
links stay under the selected root's `/etc/alternatives` and declared link
paths. Conary persists a normalized group/mode/master/selected-path,
slave-link, choice/priority, and choice-slave model. It renders the exact
line-oriented upstream format before each event, then strictly parses and
atomically replaces typed state afterward. The helper can therefore perform
install, remove, and auto selection without a host dpkg database.

## Arch Linux / ALPM

Authoritative references:

- [`alpm-install-scriptlet(5)`](https://man.archlinux.org/man/alpm-install-scriptlet.5.en)
- [`alpm-hooks(5)`](https://man.archlinux.org/man/alpm-hooks.5.en)
- [`alpm-package-relation(7)`](https://alpm.archlinux.page/specifications/alpm-package-relation.7.html)
- [`alpm-sonamev1(7)`](https://alpm.archlinux.page/specifications/alpm-sonamev1.7.html)
- [`alpm-soname(7)`](https://alpm.archlinux.page/specifications/alpm-soname.7.html)
- [pinned pacman/libalpm dependency parser](https://gitlab.archlinux.org/pacman/pacman/-/blob/a6f7467d8c7c4d7e9cc846884e74c0ab7215c48d/lib/libalpm/deps.c)
- [pinned pacman/libalpm source](https://gitlab.archlinux.org/pacman/pacman/-/blob/a6f7467d8c7c4d7e9cc846884e74c0ab7215c48d/lib/libalpm/trans.c)
- [pinned Arch pacman build contract](https://gitlab.archlinux.org/archlinux/packaging/packages/pacman/-/blob/abdc0dfedf3ca553a02dde8551e972fe745535b7/PKGBUILD)

### ALPM Relations And Sonames

Runtime dependencies and provisions use the official ALPM grammar through
`alpm_types::RelationOrSoname`, whose parse precedence is soname v2, soname
v1, then package relation. This is source grammar, not punctuation matching or
a package exception.

ALPM soname v1 and v2 values are atomic `Soname` capability identities. Conary
stores the complete canonical string, including a v1 identity such as
`libwlroots-0.18.so=libwlroots-0.18.so-64`, without treating any portion as an
Arch package-version constraint. A runtime soname requirement matches only an
identical typed soname provision; a same-named package or virtual provision is
not equivalent.

Ordinary package and virtual provisions remain package relations. They may be
unversioned or use ALPM's exact `=` relation; other comparison operators are
rejected for provisions. Conflicts, replacements, optional dependencies, and
build dependencies continue to use their documented package-relation grammar.

### `.INSTALL` Functions

An Arch `.INSTALL` file is a shell function library run with the
distribution-configured shell; it does not select an interpreter from a
per-package shebang. Conary preserves the complete file and invokes only the
typed function for the transaction:

| Function | Stage | Arguments |
| --- | --- | --- |
| `pre_install` | before installing payload | `new-version` |
| `post_install` | after installing payload | `new-version` |
| `pre_upgrade` | before upgrading payload | `new-version old-version` |
| `post_upgrade` | after upgrading payload | `new-version old-version` |
| `pre_remove` | before removing payload | `old-version` |
| `post_remove` | after removing payload | `old-version` |

An upgrade invokes the new package's `pre_upgrade` and `post_upgrade`; it does
not synthesize old-package remove calls.

Function selection reproduces libalpm's `_alpm_runscriptlet` predicate exactly:
the source is read in 1023-byte `fgets` chunks, each C string is truncated at
the first `#`, and the remaining bytes are tested for the literal lifecycle
name. This deliberately mirrors upstream's finite package-manager ABI; it is
not Conary command classification and it is not used for any other decision.
The selected entry retains the complete `.INSTALL` bytes, sources that complete
library, and invokes the selected function. Execution reproduces
`_alpm_runscriptlet`: Conary writes the preserved bytes to a transaction-local
`/tmp/alpm_*/.INSTALL`, invokes the configured shell with `-c`, sources that
separate file, and then calls the selected function. No script positional
arguments are supplied to the shell, so top-level `$#` is zero, top-level
`$@` is empty, and `$0` is the configured shell path; only the selected
function receives the version arguments. The temporary source is removed after
either success or failure.

Version arguments use PKGBUILD's full-package-version grammar:
`pkgver-pkgrel` or `epoch:pkgver-pkgrel`. `pkgver` is non-empty ASCII without
colon, slash, hyphen, or whitespace; `pkgrel` is a positive integer with at
most one positive integer subrelease; a serialized epoch is a positive
integer. Conary validates that grammar and shell-quotes each value so the
function receives the exact package version without turning package metadata
into shell syntax.

The shell is source-profile data because ALPM packages do not carry it. The
current supported Arch profile records `/usr/bin/bash`, matching Arch's pacman
build (`-Dscriptlet-shell=/usr/bin/bash`). Conversion requires the package's
exact public ALPM source-profile ID. A local package without source provenance
is rejected instead of inheriting whichever ALPM profile happens to be the
only one in the current catalog. Each additional ALPM distribution therefore
owns an explicit shell contract; Conary never guesses from the host, filename,
package format, route alias, or catalog population.

### ALPM Hooks

Package-owned hooks under `/usr/share/libalpm/hooks/*.hook` use the strict
`alpm-hooks(5)` grammar:

- one or more `[Trigger]` sections and a required `[Action]`;
- `Operation = Install|Upgrade|Remove`, repeatable;
- `Type = Package|Path`;
- repeatable POSIX `fnmatch(3)` `Target` values, with leading `!` inversion,
  leading `\` escaping, and the last matching value taking precedence;
- `When = PreTransaction|PostTransaction`;
- a required `Exec` tokenized by libalpm's finite `wordsplit()` grammar, where
  matching single or double quotes group bytes and backslash is special only
  immediately before a quote;
- optional repeatable `Depends`, plus `AbortOnFail` and `NeedsTargets`.

Only a line whose first non-whitespace byte is `#` is a comment; inline `#`
bytes remain part of the value. Repeated `Operation` and flag directives are
idempotent. Repeated `Type`, `Description`, `When`, and `Exec` directives use
the final value, matching current libalpm parser behavior.

Multiple trigger sections are OR alternatives. Install is classified as
upgrade whenever the package or path is already present, regardless of version
ordering. Path candidates are archive-relative and exclude the install-root
prefix. On remove, owned archive paths are candidates even if a path is already
absent from disk.

Pre-transaction hooks run before any payload change. `AbortOnFail` is valid only
there and aborts the transaction on nonzero exit. Post-transaction hooks run
only after a successful transaction. `NeedsTargets` supplies matched targets on
stdin, one per line. `Depends` is checked against the final installed package
set.

Hooks run in alphabetical filename order with the `.hook` suffix ignored.
Host-configured higher-priority hook directories and same-name `/dev/null`
disable overrides are host policy, not package-archive authority. Conary must
model a Conary-owned hook-directory policy before planning package-owned hooks.
It must not query or delegate to a host pacman installation. The selected root's
explicit Conary policy decides whether host overrides participate. Files a
package happens to carry under `/etc/pacman.d/hooks` remain ordinary payload;
they are not extracted as lifecycle control artifacts for that transaction.
The current source lifecycle schema therefore accepts only an immediate
`/usr/share/libalpm/hooks/<basename>.hook` path with a required action; it has no
`/etc` priority or `/dev/null` mask compatibility shape.

## Implementation and Proof

Current ownership:

- source ABI types: `crates/conary-core/src/packages/native_abi.rs`;
- RPM lifecycle extraction: `crates/conary-core/src/packages/rpm/scriptlets.rs`;
- RPM typed header and signature projection:
  `crates/conary-core/src/packages/rpm/scriptlets/runtime_context.rs`;
- RPM install-time query-format runtime:
  `crates/conary-core/src/scriptlet/rpm_runtime/query_format.rs` and
  `scriptlet/rpm_runtime/query_format/`;
- Debian extraction: `crates/conary-core/src/packages/deb/native.rs` and
  `packages/deb/triggers.rs`;
- Debian dependency, provide, and `Multi-Arch` authority:
  `crates/conary-core/src/repository/dependency_model.rs`,
  `repository/package_relation.rs`, `repository/parsers/debian.rs`,
  `db/models/repository_capability.rs`, and `resolver/provider/matching.rs`;
- Arch extraction: `crates/conary-core/src/packages/arch.rs`,
  `packages/arch/install_script.rs`, and `packages/arch/alpm_hook.rs`;
- durable CCS bundle: `crates/conary-core/src/ccs/native_lifecycle.rs`;
- typed planner: `crates/conary-core/src/ccs/native_transaction.rs` with
  RPM, Debian, and Arch ownership modules under `ccs/native_transaction/`;
- install-stage executor: `apps/conary/src/commands/install/native_events.rs`;
- closed systemd activation grammar and durable generation projection:
  `crates/conary-core/src/activation/systemd.rs` and
  `crates/conary-core/src/db/models/generation_activation.rs`;
- selected-root argv capture and booted-generation consumer:
  `crates/conary-core/src/scriptlet/activation_capture.rs` and
  `apps/conary/src/commands/generation/activation_intents.rs`;
- Debian dpkg environment, administrative projection, alternatives state, and
  helper mutation capture:
  `apps/conary/src/commands/install/native_events/debian_runtime.rs` and
  `apps/conary/src/commands/install/native_events/debian_runtime/`;
- transaction-wide runtime preflight:
  `apps/conary/src/commands/install/native_events/preflight.rs`;
- installed-state and exact path-snapshot projection:
  `apps/conary/src/commands/install/native_events/transaction_state.rs`;
- exact CCS authority removed by native upgrades or relations:
  `apps/conary/src/commands/install/ccs_removal_hooks.rs` and
  `apps/conary/src/commands/remove/ccs_hook.rs`;
- shared config decisions and durable snapshot:
  `crates/conary-core/src/config_transaction.rs`;
- selected-root config planner:
  `apps/conary/src/commands/install/config_files.rs`;
- generation capture/materializer and debt:
  `crates/conary-core/src/generation/root_manifest.rs`,
  `crates/conary-core/src/generation/root_manifest/`,
  `crates/conary-core/src/generation/builder/carrier_capabilities.rs`,
  `crates/conary-core/src/generation/artifact.rs`,
  `crates/conary-core/src/generation/export.rs`,
  `apps/conary/src/commands/generation/selected_root.rs`,
  `apps/conary/src/commands/generation/config_transaction.rs`, and
  `apps/conary/src/commands/generation/publication.rs`;
- native source-manager trace oracles and the full source/target lifecycle
  matrix: `apps/conary/tests/fixtures/native-lifecycle-parity/`,
  `apps/conary/tests/fixtures/native/capture-native-lifecycle-oracle.sh`, and
  `apps/conary/tests/fixtures/native/run-cross-source-lifecycle-matrix.sh`.

The minimum proof for a lifecycle change is:

```bash
cargo test -p conary-core native_abi
cargo test -p conary-core native_lifecycle
cargo test -p conary-core native_transaction
cargo test -p conary-core --lib activation
cargo test -p conary-core --lib generation_activation
cargo test -p conary-core --lib config_transaction
cargo test -p conary --lib commands::generation::config_transaction
cargo test -p conary --lib commands::generation::activation_intents
cargo test -p conary commands::install
```

Tests must assert exact event order, argv, stdin, trigger matches, payload
visibility at the lifecycle boundary, no preflight mutation, and no source
package-manager process or database access. The cross-distro matrix must run
the Cartesian product of every supported source ABI family and every supported
target capability profile. Each row runs without that source format's native
package manager and compares the observable lifecycle trace with an
authoritative source-manager fixture. A green command-classification corpus,
same-distro run, hand-picked pairwise converter, or adoption flow is not
lifecycle proof.
