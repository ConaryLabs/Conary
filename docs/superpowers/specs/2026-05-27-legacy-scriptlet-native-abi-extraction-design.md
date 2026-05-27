---
last_updated: 2026-05-27
revision: 8
summary: Design for Goal 2 native ABI extraction for RPM, DEB, Arch install scriptlets and ALPM hook artifacts, byte-preserving parser facts, and RPM verification scriptlet preservation without Remi embedding, bundle conversion, or install behavior changes
---

# Legacy Scriptlet Native ABI Extraction: Goal 2 Design Spec

**Date:** 2026-05-27
**Status:** Draft for implementation planning
**Parent spec:** `docs/superpowers/specs/2026-05-27-legacy-scriptlet-semantics-bundle-design.md`
**Goal queue:** `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`

## Purpose

Goal 1 added the passive `LegacyScriptletBundle` schema and query surface.
Goal 2 supplies the missing parser-side source of truth: native ABI entries for
RPM, DEB, and Arch scriptlets.

The existing parser API exposes a flattened compatibility model:

```rust
Scriptlet {
    phase,
    interpreter,
    content,
    flags,
}
```

That shape is still useful for current query and analyzer paths, but it loses
native ABI details before later goals can classify, embed, or replay scriptlets
safely. Goal 2 preserves those details beside the old API and proves that
format-specific scriptlet slots are not silently dropped.

## Decision

Use **Option A: parser-only native ABI extraction**.

Add a new native ABI model in `conary-core` package parsing code, populate it
from RPM, DEB, and Arch parsers, and leave existing flattened `scriptlets()`
behavior intact. Goal 2 does not embed legacy bundles in Remi, does not run
effect adapters, does not change install/update/remove behavior, and does not
make replay or publication decisions.

Later goals consume the native ABI entries:

- Goal 3a can classify structured invocations and blocked classes.
- Goal 3b can add evidence-driven effect adapters.
- Goal 4 can build and embed passive `LegacyScriptletBundle` values in Remi
  conversion output.
- Goal 6 can consume installed bundle state behind explicit replay gates.

## Non-Goals

- No Remi bundle embedding.
- No Remi database migration.
- No CCS archive format change beyond what Goal 1 already added.
- No install/update/remove behavior change.
- No scriptlet execution or replay.
- No adapter registry decisions.
- No `NativeScriptletEntry` to `LegacyScriptletEntry` converter. That bridge
  belongs to the later conversion and Remi embedding goals.
- No broad helper-specific `replaced` claims.
- No retirement of the existing flattened `Scriptlet` API.

## Official Semantics Grounding

Goal 2 should use upstream package-format documentation as the normative source
for native lifecycle names, trigger names, argument contracts, and ordering.
Implementation can still be limited by parser APIs, but tests should not invent
undocumented lifecycle slots.

Primary sources:

- RPM/Fedora: RPM's `rpm-scriptlets(7)` and spec-file docs define lifecycle
  scriptlets, transaction scriptlets, `%verify`, package triggers, file triggers,
  transaction file triggers, trigger arguments, stdin contracts, and ordering.
  Fedora/DNF uses RPM as the package transaction engine, so DNF documentation is
  useful for frontend transaction context but RPM docs own the scriptlet ABI.
- Debian/Ubuntu: Debian Policy and dpkg manpages define maintainer scripts,
  `config`, call modes, noninteractive expectations, `postinst triggered`, and
  `deb-triggers(5)` await/noawait declarations. Ubuntu/APT layers on Debian
  packages through dpkg, so apt documentation is useful for frontend context but
  dpkg/Debian Policy own the maintainer-script and trigger ABI.
- Arch/ALPM: `alpm-install-scriptlet(5)` defines `.INSTALL` functions and their
  version arguments. `alpm-hooks(5)` defines separate transaction hook files with
  `[Trigger]` and `[Action]` sections, package/path targets, stdin target lists,
  and pre/post-transaction ordering.

Goal 2's Arch implementation target covers `.INSTALL` scriptlets and packaged
ALPM hook files under `/usr/share/libalpm/hooks/*.hook`. Hooks are modeled as
byte-preserved `ControlArtifact` entries with parsed trigger/action metadata
where straightforward. They remain passive parser facts in Goal 2; execution,
ordering reconciliation, and replay are later-goal work.

## Architecture

Add native ABI metadata beside existing common package metadata.

The preferred implementation shape is:

```rust
// crates/conary-core/src/packages/common.rs
pub struct PackageMetadata {
    pub scriptlets: Vec<Scriptlet>,
    pub native_scriptlet_abi: Vec<NativeScriptletEntry>,
    // existing fields...
}

// crates/conary-core/src/packages/traits.rs
pub trait PackageFormat {
    fn scriptlets(&self) -> &[Scriptlet] {
        &[]
    }

    fn native_scriptlet_abi(&self) -> &[NativeScriptletEntry] {
        &[]
    }
}
```

Package parsers populate both fields:

- `scriptlets` remains the compatibility projection used by current callers.
- `native_scriptlet_abi` is the authoritative Goal 2 model for later bundle
  production.

This is an additive parser API change. Do not replace `scriptlets()` or change
its return type in Goal 2. Current callers should compile without needing to
understand native ABI entries.

The ABI layer should live in a focused
`crates/conary-core/src/packages/native_abi.rs` module and be re-exported by
`packages::traits` for current callers. `PackageMetadata` lives in
`packages/common.rs`, so the plan should add the field and default constructor
there and update only struct literals that construct
`conary_core::packages::common::PackageMetadata`. Do not add the field to
unrelated repository metadata structs in `crates/conary-core/src/repository/`.
Keep `ccs::legacy_scriptlets` from depending on parser internals in Goal 2; the
bridge from native ABI to bundle entries belongs to a later conversion goal.

## Data Model

The model should be format-aware and preserve native slots without forcing every
format through one lossy enum.

Recommended core types:

```rust
pub struct NativeScriptletEntry {
    pub id: String,
    pub format: NativeScriptletFormat,
    pub kind: NativeScriptletKind,
    pub native_slot: String,
    pub primary_lifecycle: NativeLifecyclePath,
    pub compatibility_phase: Option<ScriptletPhase>,
    pub lifecycle_paths: Vec<NativeLifecyclePath>,
    pub interpreter: Option<String>,
    pub interpreter_args: Vec<String>,
    pub body: NativeScriptletBody,
    pub invocation: NativeInvocationContract,
    pub order: NativeTransactionOrder,
    pub support: NativeScriptletSupport,
    pub metadata: NativeScriptletMetadata,
}
```

The names below are the implementation target. If implementation discovers a
better local name, the plan must record the equivalent semantics before code
lands:

- `id` is stable and queryable, such as `rpm:%post`, `deb:postinst`, or
  `arch:post_install`.
- `kind` distinguishes executable scriptlets from non-executable control
  artifacts such as Debian `triggers`.
- `native_slot` is the native slot name as close to the source package format
  as possible.
- `primary_lifecycle` is the native lifecycle category, including native-only
  paths that the old compatibility enum cannot represent.
- `compatibility_phase` is `Some` only when the entry can be projected into the
  existing flattened `Scriptlet` API without inventing semantics.
- `lifecycle_paths` records the native call paths that later classification or
  replay must consider.
- `interpreter` and `interpreter_args` split the native interpreter vector or
  shebang instead of storing all flags in one string. `interpreter` is `None`
  only for non-executable control artifacts.
- `body` preserves the original bytes needed by later bundle hashing and
  wrapper work. Text decoding is an additional convenience, not the source of
  truth.
- `invocation` records native arguments, environment assumptions, stdin
  contract, and chroot/install-root expectations.
- `order` records whether the slot runs before payload mutation, after payload
  mutation, during a transaction boundary, or as a trigger.
- `support` records parser support status only; it is not an adapter decision.
- `metadata` stores format-specific details.

Recommended supporting enums and structs:

```rust
pub enum NativeScriptletFormat {
    Rpm,
    Deb,
    Arch,
}

pub enum NativeScriptletKind {
    Executable,
    ControlArtifact,
}

pub struct NativeScriptletBody {
    pub bytes: Vec<u8>,
    pub text: Option<String>,
    pub encoding: NativeScriptletBodyEncoding,
    pub sha256: String,
}

pub enum NativeScriptletBodyEncoding {
    Utf8,
    Binary,
}

pub enum NativeScriptletSupport {
    Parsed,
    DeferredReview { reason_code: String },
    Unpreservable { reason_code: String },
}

pub enum NativeScriptletMetadata {
    Rpm(RpmNativeScriptletMetadata),
    Deb(DebNativeScriptletMetadata),
    Arch(ArchNativeScriptletMetadata),
}

pub enum NativeLifecyclePath {
    PreInstall,
    PostInstall,
    PreUpgrade,
    PostUpgrade,
    PreRemove,
    PostRemove,
    PreTransaction,
    PostTransaction,
    PreUntransaction,
    PostUntransaction,
    Verify,
    Config,
    Trigger,
    FileTrigger,
    TransactionFileTrigger,
    Purge,
    Abort,
}

pub struct RpmNativeScriptletMetadata {
    pub slot: RpmScriptletSlot,
    pub scriptlet_flags: Option<RpmScriptletFlagsMetadata>,
    pub trigger: Option<RpmTriggerMetadata>,
}

pub enum RpmScriptletSlot {
    Pre,
    Post,
    PreUn,
    PostUn,
    PreTrans,
    PostTrans,
    PreUnTrans,
    PostUnTrans,
    Verify,
    Trigger,
}

pub struct RpmScriptletFlagsMetadata {
    pub names: Vec<String>,
    pub raw_bits: u32,
}

pub struct RpmTriggerMetadata {
    pub family: RpmTriggerFamily,
    pub conditions: Vec<RpmTriggerCondition>,
    pub file_globs: Vec<String>,
}

pub enum RpmTriggerFamily {
    Package,
    File,
    TransactionFile,
}

pub enum RpmTriggerAction {
    PreInstall,
    Install,
    Uninstall,
    PostUninstall,
    Unknown { raw_flags: u32 },
}

pub struct RpmTriggerCondition {
    pub name: String,
    pub action: RpmTriggerAction,
    pub version: Option<String>,
    pub comparison: Option<String>,
    pub raw_flags: u32,
}

pub struct DebNativeScriptletMetadata {
    pub control_member: DebControlMember,
    pub maintainer_modes: Vec<DebMaintainerInvocation>,
    pub trigger_declarations: Vec<DebTriggerDeclaration>,
}

pub enum DebControlMember {
    Config,
    Preinst,
    Postinst,
    Prerm,
    Postrm,
    Triggers,
}

pub struct DebMaintainerInvocation {
    pub mode: DebMaintainerMode,
    pub args: Vec<NativeArgumentContract>,
    pub lifecycle_paths: Vec<NativeLifecyclePath>,
}

pub enum DebMaintainerMode {
    Install,
    Configure,
    Reconfigure,
    Upgrade,
    Remove,
    Purge,
    Triggered,
    Disappear,
    Deconfigure,
    FailedUpgrade,
    AbortInstall,
    AbortUpgrade,
    AbortRemove,
    AbortDeconfigure,
}

pub struct DebTriggerDeclaration {
    pub directive: DebTriggerDirective,
    pub trigger_name: String,
    pub await_mode: DebTriggerAwaitMode,
    pub raw_line: String,
}

pub enum DebTriggerDirective {
    Interest,
    Activate,
}

pub enum DebTriggerAwaitMode {
    Default,
    Await,
    NoAwait,
}

pub enum ArchNativeScriptletMetadata {
    Install(ArchInstallScriptletMetadata),
    AlpmHook(ArchAlpmHookMetadata),
}

pub struct ArchInstallScriptletMetadata {
    pub install_source_sha256: String,
    pub function_name: String,
    pub function_body: Option<String>,
    pub function_body_sha256: Option<String>,
    pub extraction_status: ArchFunctionExtractionStatus,
}

pub enum ArchFunctionExtractionStatus {
    Parsed,
    DeferredReview { reason_code: String },
}

pub struct ArchAlpmHookMetadata {
    pub hook_path: String,
    pub triggers: Vec<ArchAlpmHookTrigger>,
    pub action: Option<ArchAlpmHookAction>,
}

pub struct ArchAlpmHookTrigger {
    pub operations: Vec<ArchAlpmHookOperation>,
    pub trigger_type: ArchAlpmHookTriggerType,
    pub targets: Vec<String>,
}

pub enum ArchAlpmHookOperation {
    Install,
    Upgrade,
    Remove,
}

pub enum ArchAlpmHookTriggerType {
    Package,
    Path,
}

pub struct ArchAlpmHookAction {
    pub description: Option<String>,
    pub when: NativeTransactionPosition,
    pub exec: String,
    pub depends: Vec<String>,
    pub abort_on_fail: bool,
    pub needs_targets: bool,
}
```

`ArchAlpmHookAction` intentionally models the action fields documented by the
current `alpm-hooks(5)` manual. Nonstandard or future hook directives remain
byte-preserved in `NativeScriptletEntry::body` and should not be projected into
typed fields until an official source defines them.

Use plain Rust data structures, not `toml::Value`, in the parser ABI model.
`toml::Value` remains part of the serialized bundle schema, not the native
parser contract.

Native ABI body storage is byte-preserving. RPM currently exposes script bodies
as Rust strings through the `rpm` crate, but DEB and Arch parsers should read
control and `.INSTALL` members as bytes first, compute `sha256` over those raw
bytes, and only fill `text` when UTF-8 decoding succeeds. Later bundle
conversion can serialize non-UTF-8 bodies with `body_encoding = "base64"`; Goal
2 should not lose bytes just because the flattened compatibility API uses
`String`.

`NativeLifecyclePath` is intentionally richer than the old `ScriptletPhase`
compatibility enum. Goal 2 should not widen `ScriptletPhase` just to represent
native-only lifecycle paths such as RPM `%verify`, RPM untransaction slots, DEB
`config`, file triggers, transaction file triggers, purge, or abort modes.
Flattened compatibility output may keep using the old phase vocabulary while
native ABI entries carry the lossless lifecycle details.

The invocation and ordering records should be concrete enough for tests to
assert exact parser output:

```rust
pub struct NativeInvocationContract {
    pub args: Vec<NativeArgumentContract>,
    pub environment: Vec<NativeEnvironmentFact>,
    pub stdin: NativeStdinContract,
    pub root: NativeRootExpectation,
}

pub struct NativeTransactionOrder {
    pub position: NativeTransactionPosition,
    pub relative_to: Option<String>,
}

pub struct NativeArgumentContract {
    pub index: usize,
    pub name: String,
    pub value: NativeArgumentValue,
    pub required: bool,
}

pub enum NativeArgumentValue {
    Action,
    OldVersion,
    NewVersion,
    PackageInstanceCount,
    PackageName,
    TriggerName,
    TriggerNames,
    TriggerCount,
    FilePath,
    InstalledVersion,
    Raw(String),
}

pub struct NativeEnvironmentFact {
    pub name: String,
    pub value: Option<String>,
}

pub enum NativeStdinContract {
    None,
    Debconf,
    Paths,
    Unknown,
}

pub enum NativeRootExpectation {
    PackageManagerDefault,
    InstallRoot,
    HostRoot,
    Unknown,
}

pub enum NativeTransactionPosition {
    BeforePayload,
    AfterPayload,
    BeforeTransaction,
    AfterTransaction,
    Untransaction,
    Verification,
    Trigger,
    ControlArtifact,
}
```

## RPM ABI

RPM extraction should preserve basic scriptlets, transaction scriptlets, package
triggers, file triggers, and transaction file triggers exposed by the `rpm`
crate.

Required lifecycle scriptlet slots when present:

- `%pre`
- `%post`
- `%preun`
- `%postun`
- `%pretrans`
- `%posttrans`
- `%preuntrans`
- `%postuntrans`

RPM `%verify` scriptlets run during package verification rather than install,
update, or remove. Goal 2 should still preserve a non-empty `%verify` scriptlet
as a native ABI entry with `primary_lifecycle = Verify`,
`compatibility_phase = None`, and `support = DeferredReview` using
`rpm-verify-scriptlet-deferred`. That keeps verification scriptlets visible
without claiming they belong to the install/update/remove replay surface.

Required trigger families when present:

- package triggers from `metadata.get_triggers()`: `%triggerprein`,
  `%triggerin`, `%triggerun`, and `%triggerpostun`
- file triggers from `metadata.get_file_triggers()`: `%filetriggerin`,
  `%filetriggerun`, and `%filetriggerpostun`
- transaction file triggers from `metadata.get_trans_file_triggers()`:
  `%transfiletriggerin`, `%transfiletriggerun`, and
  `%transfiletriggerpostun`

The current workspace depends on an `rpm` crate version that exposes basic
scriptlet flags, interpreter vectors, trigger scripts, interpreter vectors, and
trigger conditions through metadata methods. For every RPM scriptlet,
`program[0]` is the interpreter path and `program[1..]` are
`interpreter_args`; if no program is present, use `/bin/sh` and empty args.
Preserve scriptlet flags such as `ScriptletFlags::EXPAND` in RPM-specific
metadata so later goals can distinguish parser-provided binary bodies from
future source-level macro evidence. Preserve both named flags and raw flag bits.
Preserve trigger condition names, versions, action bits, and comparison flags.
Convert comparison flags into stable string comparison operators using the same
logic as the parser's existing dependency flag handling, and preserve raw flag
bits for tests and future semantic refinement.

RPM entries should record:

- native slot name, including trigger family and trigger action where known;
- interpreter program and arguments;
- body after RPM macro expansion as available in the binary package;
- scriptlet flags from the RPM header;
- trigger target constraints for package triggers;
- file globs or monitored paths for file triggers;
- transaction-file-trigger family when applicable;
- trigger action form for every condition;
- ordering relative to payload mutation or transaction boundary;
- lifecycle paths for install, upgrade, remove, transaction, untransaction,
  verification, trigger, and file trigger paths.

RPM invocation contracts should record the native `$1` meaning for lifecycle
slots instead of collapsing install, upgrade, and erase paths. For example,
`%pre` and `%post` can run for install or upgrade, `%preun` and `%postun` can
run for erase or upgrade, and triggers receive trigger-specific package/count
arguments. Goal 2 does not need to emulate those values, but tests should assert
that the metadata records the possible argument kinds and lifecycle paths.

RPM trigger invocation contracts must be family- and action-aware. Package
triggers and package file triggers receive `$1` and `$2` package instance counts.
Transaction file triggers receive `$1` only. File-trigger stdin must distinguish
`%transfiletriggerpostun`, where RPM does not make the triggering file list
available, from the file-trigger forms that receive path lists.

Unsupported trigger semantics must become explicit `DeferredReview` entries
with stable reason codes unless the parser can prove the entry is impossible to
preserve as native ABI metadata. They must not disappear from the native ABI
list.

## DEB ABI

DEB extraction should preserve maintainer scripts and the Debian `triggers`
control-archive file.

Required maintainer scripts:

- `config`
- `preinst`
- `postinst`
- `prerm`
- `postrm`

DEB entries should record:

- native script name as the slot;
- shebang interpreter and arguments, falling back to `/bin/sh` when absent;
- full maintainer script body bytes;
- native invocation modes relevant to that script;
- old/new version argument positions where the mode requires them;
- noninteractive expectation for package-manager execution;
- unpack, configure, remove, purge, and trigger lifecycle paths.

Argument contracts must be mode-specific. Do not assume all old/new version
arguments live at the same index: complex Debian Policy calls such as
`deconfigure in-favour ... [removing ...]` include literal marker arguments and
package/version values at higher positions.

`config` script metadata should include debconf's `configure` and `reconfigure`
actions with action plus installed-version arguments when present. `postinst
triggered` should model the second argument as a space-separated trigger-name
list, not a single trigger name.

Minimum maintainer invocation table:

| Control member | Invocation modes to preserve |
| --- | --- |
| `config` | `configure`, `reconfigure`, and any parser-visible debconf configuration mode |
| `preinst` | `install`, `upgrade`, `abort-upgrade` |
| `postinst` | `configure`, `triggered`, `abort-upgrade`, `abort-remove`, `abort-deconfigure` |
| `prerm` | `remove`, `upgrade`, `deconfigure`, `failed-upgrade` |
| `postrm` | `remove`, `purge`, `upgrade`, `disappear`, `failed-upgrade`, `abort-install`, `abort-upgrade` |

Interpreter extraction must parse the shebang line. The first token after `#!`
is the interpreter path and remaining tokens become `interpreter_args`; for
example, `#!/usr/bin/perl -w` becomes interpreter `/usr/bin/perl` and args
`["-w"]`.

The `triggers` artifact is a separate member of the DEB control tarball, not a
field in the RFC822-style `control` file. `parse_control_tar_all()` should
recognize a basename of `triggers`, preserve the raw file content, and parse
trigger declarations such as `interest`, `interest-await`, `interest-noawait`,
`activate`, `activate-await`, and `activate-noawait` into metadata when
straightforward. The parsed trigger metadata must preserve whether the
declaration was default, await, or noawait because that affects later ordering
decisions. Trigger content is deferred evidence in Goal 2: preserve it and list
trigger names, but mark complex trigger execution paths as `DeferredReview`
until later goals implement replay or publication gates.

Because the Debian `triggers` member is not executable, represent it as a native
ABI `ControlArtifact` entry with `id = "deb:triggers"`,
`native_slot = "triggers"`, `primary_lifecycle = Trigger`,
`compatibility_phase = None`, `interpreter = None`, and body bytes equal to the
raw `triggers` file. Do not attach the triggers content only to package-level
metadata; it must be queryable and testable as preserved native ABI evidence.

The existing flattened projection can continue to expose one `Scriptlet` per
package lifecycle maintainer script it already models. The DEB `config` script
may remain native-ABI-only in Goal 2 unless the implementation deliberately adds
a compatible old-API phase and updates every current caller.

## Arch ABI

Arch extraction should preserve the full `.INSTALL` file, not only detached
function bodies.

Arch ALPM hook files are a separate documented transaction-trigger mechanism,
not `.INSTALL` functions. Goal 2 includes package-provided
`/usr/share/libalpm/hooks/*.hook` files as native ABI `ControlArtifact` entries
with parsed trigger/action metadata and raw bytes preserved in all cases. This
keeps package-provided pacman hook semantics visible without claiming Goal 2
executes or replays them.

Required callable functions:

- `pre_install`
- `post_install`
- `pre_upgrade`
- `post_upgrade`
- `pre_remove`
- `post_remove`

Arch entries should record:

- the called function name as the native slot;
- full `.INSTALL` source context, plus a digest of that source for later bundle
  metadata;
- per-function body for compatibility projection and diagnostics;
- pacman-style old/new version argument expectations;
- chroot/install-root execution expectation;
- ordering relative to file extraction or removal.

Later replay should source the preserved `.INSTALL` file and call the function
with native-compatible arguments. Goal 2 only preserves enough metadata for that
future wrapper; it does not generate or execute the wrapper.

Function extraction must not become a new silent-drop path. If a `.INSTALL`
file exists and the parser recognizes a required function declaration but cannot
extract the callable body, preserve a native ABI entry with the full `.INSTALL`
body, `function_body = None`, and `support = DeferredReview` using a stable
reason such as `arch-install-function-extraction-deferred`. The flattened
compatibility API may omit that function body until extraction succeeds, but the
native ABI list must retain evidence that the callable slot exists.

## Compatibility Projection

The existing `scriptlets()` API must remain compatible.

Goal 2 can implement compatibility projection in either direction:

- populate native ABI first, then derive flattened `Scriptlet` values; or
- keep current flattened extraction paths and add native ABI extraction beside
  them.

The implementation should prefer the first option where it removes duplication,
but only if tests prove current behavior is unchanged. For Arch packages, the
compatibility projection must extract individual callable function bodies from
the preserved `.INSTALL` source with the parser's existing function-extraction
logic, rather than returning the raw full file content.

Compatibility output must preserve the current public API shape unless a test
intentionally documents a narrower, approved behavior change:

- RPM flattened scriptlets keep using the first RPM program component as
  `Scriptlet.interpreter`, leave `flags` as `None`, and omit native-only
  untransaction, verification, file-trigger, and transaction-file-trigger
  entries by default.
- DEB flattened scriptlets keep the current interpreter string behavior: the
  `Scriptlet.interpreter` value is the full shebang tail after `#!` when present
  and `/bin/sh` otherwise. Native ABI entries still split the shebang into
  interpreter plus args.
- Arch flattened scriptlets keep returning callable function bodies, not the
  full `.INSTALL` source.
- Non-UTF-8 bodies remain native-ABI-only unless an explicit compatibility
  strategy is added; the old `Scriptlet` API is string-based.

The flattened API may continue to omit native-only entries if that matches
existing public behavior; the native ABI API must include them. If a native-only
path cannot be represented by `ScriptletPhase`, keep it in
`NativeLifecyclePath` instead of forcing an inaccurate compatibility phase.

Compatibility projection should use this mapping:

| `NativeLifecyclePath` | `ScriptletPhase` projection | Notes |
| --- | --- | --- |
| `PreInstall` | `PreInstall` | Direct |
| `PostInstall` | `PostInstall` | Direct |
| `PreUpgrade` | `PreUpgrade` | Direct |
| `PostUpgrade` | `PostUpgrade` | Direct |
| `PreRemove` | `PreRemove` | Direct |
| `PostRemove` | `PostRemove` | Direct |
| `PreTransaction` | `PreTransaction` | Direct |
| `PostTransaction` | `PostTransaction` | Direct |
| `Trigger` | `Trigger` | Direct when old API exposure is intended |
| `Config` | none | Native-ABI-only |
| `FileTrigger` | none by default | Native-ABI-only unless a caller explicitly asks for approximate trigger projection |
| `TransactionFileTrigger` | none by default | Native-ABI-only unless a caller explicitly asks for approximate trigger projection |
| `PreUntransaction` | none | Native-ABI-only |
| `PostUntransaction` | none | Native-ABI-only |
| `Verify` | none | Native-ABI-only |
| `Purge` | none by default | Native-ABI-only; do not disguise as remove without a caller opting into approximation |
| `Abort` | none | Native-ABI-only |

## Error Handling

Malformed package archives should continue to fail parsing as they do today.

Parseable but unsupported native scriptlet semantics should not fail the whole
package parse by default. Preserve the entry with explicit support metadata:

- `Parsed` when the parser has the native body and enough metadata for later
  classification.
- `DeferredReview` when the parser preserved the body/metadata but later goals
  classify semantic behavior.
- `Unpreservable` only for parser-visible semantics that are known impossible
  to preserve as native ABI metadata. This is not a publication, install, or
  replay block; unknown interpreters are not parser-level blockers in Goal 2.
  Preserve the interpreter string and let later sandbox or replay gates decide
  safety.

Empty or whitespace-only basic scriptlet bodies are not native ABI entries.
They are equivalent to absent executable scriptlets for Goal 2 and should not be
counted as dropped slots. If a trigger family exposes conditions with an empty
script body, preserve a `DeferredReview` entry so the condition metadata is not
lost.

Goal 2 should use stable reason codes, for example:

- `rpm-trigger-semantics-deferred`
- `rpm-file-trigger-semantics-deferred`
- `rpm-trans-file-trigger-semantics-deferred`
- `rpm-verify-scriptlet-deferred`
- `deb-trigger-semantics-deferred`
- `arch-install-function-extraction-deferred`
- `arch-alpm-hook-semantics-deferred`
- `native-abi-parser-limitation`

These reason codes are parser evidence only. They do not replace the later
bundle-level decision rubric.

## Fixture Strategy

Prefer generated fixtures in tests over committed binary packages unless a
format requires a binary fixture that cannot be assembled deterministically.
End-to-end parser tests should write temporary package files because
`PackageFormat::parse()` is path-based. In-memory buffers are fine for private
helper tests, but the cross-format parser contract must exercise real temp
`.rpm`, `.deb`, and `.pkg.tar.*` archives.

RPM tests can use the `rpm` crate builder because the local crate exposes
scriptlet and trigger builder APIs. Build fixtures with:

- all eight lifecycle scriptlets;
- a `%verify` scriptlet;
- every package trigger action: `%triggerprein`, `%triggerin`,
  `%triggerun`, and `%triggerpostun`;
- every file trigger action: `%filetriggerin`, `%filetriggerun`, and
  `%filetriggerpostun`;
- every transaction file trigger action: `%transfiletriggerin`,
  `%transfiletriggerun`, and `%transfiletriggerpostun`.

DEB tests should build minimal in-memory or temporary `.deb` archives with a
control tar containing:

- all five maintainer scripts: `config`, `preinst`, `postinst`, `prerm`, and
  `postrm`;
- a `triggers` file;
- shebangs that prove native interpreter and args are split while flattened
  compatibility output remains unchanged;
- a non-UTF-8 maintainer script fixture in a helper-level test, to prove native
  ABI body bytes are preserved even when the old `Scriptlet` projection cannot
  represent them.

Arch tests must directly exercise `.INSTALL` parsing and build a minimal package
archive containing `.PKGINFO`, `.INSTALL`, and one packaged ALPM hook under
`usr/share/libalpm/hooks/`. Include one fixture where a recognizable function
declaration cannot be safely extracted, so the native ABI fallback emits
`DeferredReview` instead of dropping the slot.

Tests must assert both sides of the contract:

- native ABI contains every native slot and deferred trigger artifact;
- existing flattened `scriptlets()` output remains compatible for current
  callers.

## Verification

Goal 2 implementation should stop only after these commands pass:

```bash
cargo test -p conary-core native_abi
cargo test -p conary-core rpm_scriptlet
cargo test -p conary-core deb_scriptlet
cargo test -p conary-core arch_scriptlet
cargo test -p conary-core
cargo test -p conary
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

The implementation plan may add narrower intermediate tests, but the final gate
must include the full set above.

## Plan Handoff

The detailed implementation plan should split Goal 2 into these tasks:

1. Add the byte-preserving native ABI model, concrete per-format metadata
   structs, and compatibility API with failing tests.
2. Implement Arch `.INSTALL` full-source and callable-function ABI extraction.
3. Implement DEB maintainer-script, `config`, and triggers-file ABI extraction.
4. Implement RPM lifecycle scriptlet, `%verify`, package-trigger,
   file-trigger, and transaction-file-trigger ABI extraction across every
   exposed action form.
5. Add cross-format native ABI fixture tests proving no native slot is silently
   dropped.
6. Run final verification and update module docs if parser API documentation
   changes.

Do not add a `NativeScriptletEntry` to `LegacyScriptletEntry` conversion task to
Goal 2. That mapping needs adapter, fidelity, publication, and evidence-digest
context that this parser-only goal intentionally does not own.

Keep each task reviewable and avoid broad conversion or Remi changes in this
goal.

## References

- RPM scriptlets and triggers: <https://rpm.org/docs/latest/man/rpm-scriptlets.7>
- RPM spec runtime scriptlets: <https://rpm.org/docs/4.20.x/manual/spec.html#runtime-scriptlets>
- DNF transactions passing package operations to RPM: <https://dnf.readthedocs.io/en/latest/api_transaction.html>
- Debian Policy maintainer scripts: <https://www.debian.org/doc/debian-policy/ch-maintainerscripts.html>
- Ubuntu package management overview: <https://ubuntu.com/server/docs/how-to/software/package-management/>
- Ubuntu `deb-postinst(5)` maintainer-script call modes: <https://manpages.ubuntu.com/manpages/noble/man5/deb-postinst.5.html>
- Ubuntu `deb-triggers(5)` trigger declarations: <https://manpages.ubuntu.com/manpages/questing/man5/deb-triggers.5.html>
- Arch `.INSTALL` scriptlets: <https://man.archlinux.org/man/alpm-install-scriptlet.5.en>
- Arch ALPM hooks: <https://man.archlinux.org/man/alpm-hooks.5>
