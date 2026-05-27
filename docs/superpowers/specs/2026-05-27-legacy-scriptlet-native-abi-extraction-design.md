---
last_updated: 2026-05-27
revision: 3
summary: Design for Goal 2 native ABI extraction for RPM, DEB, Arch, and RPM verification scriptlet preservation without Remi embedding, bundle conversion, or install behavior changes
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

## Architecture

Add native ABI metadata beside existing common package metadata.

The preferred implementation shape is:

```rust
pub struct PackageMetadata {
    pub scriptlets: Vec<Scriptlet>,
    pub native_scriptlet_abi: Vec<NativeScriptletEntry>,
    // existing fields...
}

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

The ABI layer should live near the existing package parser traits, either in
`crates/conary-core/src/packages/traits.rs` for small types or in a focused
`crates/conary-core/src/packages/native_abi.rs` module re-exported by
`packages::traits`. Keep `ccs::legacy_scriptlets` from depending on parser
internals in Goal 2; the bridge from native ABI to bundle entries belongs to a
later conversion goal.

## Data Model

The model should be format-aware and preserve native slots without forcing every
format through one lossy enum.

Recommended core types:

```rust
pub struct NativeScriptletEntry {
    pub id: String,
    pub format: NativeScriptletFormat,
    pub native_slot: String,
    pub primary_lifecycle: NativeLifecyclePath,
    pub compatibility_phase: Option<ScriptletPhase>,
    pub lifecycle_paths: Vec<NativeLifecyclePath>,
    pub interpreter: String,
    pub interpreter_args: Vec<String>,
    pub body: String,
    pub invocation: NativeInvocationContract,
    pub order: NativeTransactionOrder,
    pub support: NativeScriptletSupport,
    pub metadata: NativeScriptletMetadata,
}
```

The exact names can change during implementation, but the semantics should not:

- `id` is stable and queryable, such as `rpm:%post`, `deb:postinst`, or
  `arch:post_install`.
- `native_slot` is the native slot name as close to the source package format
  as possible.
- `primary_lifecycle` is the native lifecycle category, including native-only
  paths that the old compatibility enum cannot represent.
- `compatibility_phase` is `Some` only when the entry can be projected into the
  existing flattened `Scriptlet` API without inventing semantics.
- `lifecycle_paths` records the native call paths that later classification or
  replay must consider.
- `interpreter` and `interpreter_args` split the native interpreter vector or
  shebang instead of storing all flags in one string.
- `body` is the preserved source body needed by later bundle hashing and
  wrapper work.
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

pub enum NativeScriptletSupport {
    Parsed,
    DeferredReview { reason_code: String },
    Blocked { reason_code: String },
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
```

Use plain Rust data structures, not `toml::Value`, in the parser ABI model.
`toml::Value` remains part of the serialized bundle schema, not the native
parser contract.

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
    pub args: Vec<String>,
    pub environment: Vec<String>,
    pub stdin: Option<String>,
    pub chroot: Option<String>,
}

pub struct NativeTransactionOrder {
    pub position: String,
    pub relative_to: Option<String>,
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

- package triggers from `metadata.get_triggers()`
- file triggers from `metadata.get_file_triggers()`
- transaction file triggers from `metadata.get_trans_file_triggers()`

The current workspace depends on an `rpm` crate version that exposes basic
scriptlet flags, interpreter vectors, trigger scripts, interpreter vectors, and
trigger conditions through metadata methods. For every RPM scriptlet,
`program[0]` is the interpreter path and `program[1..]` are
`interpreter_args`; if no program is present, use `/bin/sh` and empty args.
Preserve scriptlet flags such as `ScriptletFlags::EXPAND` in RPM-specific
metadata so later goals can distinguish parser-provided binary bodies from
future source-level macro evidence. Preserve trigger condition names, versions,
and flags. Convert trigger flags into stable string comparison operators using
the same logic as the parser's existing dependency flag handling, and preserve
the raw flag bits or debug value where a complete semantic split is not yet
implemented.

RPM entries should record:

- native slot name, including trigger family and trigger action where known;
- interpreter program and arguments;
- body after RPM macro expansion as available in the binary package;
- scriptlet flags from the RPM header;
- trigger target constraints for package triggers;
- file globs or monitored paths for file triggers;
- transaction-file-trigger family when applicable;
- ordering relative to payload mutation or transaction boundary;
- lifecycle paths for install, upgrade, remove, transaction, untransaction,
  verification, trigger, and file trigger paths.

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
- full maintainer script body;
- native invocation modes relevant to that script, such as `install`,
  `configure`, `upgrade`, `remove`, `purge`, and `abort-*`;
- old/new version argument positions where the mode requires them;
- noninteractive expectation for package-manager execution;
- unpack, configure, remove, purge, and trigger lifecycle paths.

Interpreter extraction must parse the shebang line. The first token after `#!`
is the interpreter path and remaining tokens become `interpreter_args`; for
example, `#!/usr/bin/perl -w` becomes interpreter `/usr/bin/perl` and args
`["-w"]`.

The `triggers` artifact is a separate member of the DEB control tarball, not a
field in the RFC822-style `control` file. `parse_control_tar_all()` should
recognize a basename of `triggers`, preserve the raw file content, and parse
trigger declarations such as `interest`, `interest-noawait`, `activate`, and
`activate-noawait` into metadata when straightforward. The parsed trigger
metadata must preserve whether the declaration was await or noawait because that
affects later ordering decisions. Trigger content is deferred evidence in Goal
2: preserve it and list trigger names, but mark complex trigger execution paths
as review until later goals implement replay or publication gates.

The existing flattened projection can continue to expose one `Scriptlet` per
package lifecycle maintainer script it already models. The DEB `config` script
may remain native-ABI-only in Goal 2 unless the implementation deliberately adds
a compatible old-API phase and updates every current caller.

## Arch ABI

Arch extraction should preserve the full `.INSTALL` file, not only detached
function bodies.

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
  must decide semantics.
- `Blocked` only for parser-visible semantics that are known impossible to
  preserve as native ABI metadata. Unknown interpreters are not parser-level
  blockers in Goal 2; preserve the interpreter string and let later sandbox or
  replay gates decide safety.

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
- `arch-install-wrapper-deferred`
- `native-abi-parser-limitation`

These reason codes are parser evidence only. They do not replace the later
bundle-level decision rubric.

## Fixture Strategy

Prefer generated fixtures in tests over committed binary packages unless a
format requires a binary fixture that cannot be assembled deterministically.

RPM tests can use the `rpm` crate builder because the local crate exposes
scriptlet and trigger builder APIs. Build fixtures with:

- all eight lifecycle scriptlets;
- a `%verify` scriptlet;
- at least one package trigger;
- at least one file trigger;
- at least one transaction file trigger.

DEB tests should build minimal in-memory or temporary `.deb` archives with a
control tar containing:

- all four package lifecycle maintainer scripts plus `config`;
- a `triggers` file;
- shebangs that prove interpreter and args are split.

Arch tests can directly exercise `.INSTALL` parsing and, where practical, build
a minimal package archive containing `.PKGINFO` and `.INSTALL`.

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
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

The implementation plan may add narrower intermediate tests, but the final gate
must include the full set above.

## Plan Handoff

The detailed implementation plan should split Goal 2 into these tasks:

1. Add the native ABI model and compatibility API with failing tests.
2. Implement Arch `.INSTALL` full-source and callable-function ABI extraction.
3. Implement DEB maintainer-script, `config`, and triggers-file ABI extraction.
4. Implement RPM lifecycle scriptlet, `%verify`, package-trigger,
   file-trigger, and transaction-file-trigger ABI extraction.
5. Add cross-format native ABI fixture tests proving no native slot is silently
   dropped.
6. Run final verification and update module docs if parser API documentation
   changes.

Do not add a `NativeScriptletEntry` to `LegacyScriptletEntry` conversion task to
Goal 2. That mapping needs adapter, fidelity, publication, and evidence-digest
context that this parser-only goal intentionally does not own.

Keep each task reviewable and avoid broad conversion or Remi changes in this
goal.
