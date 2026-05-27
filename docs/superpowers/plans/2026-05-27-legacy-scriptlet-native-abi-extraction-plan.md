# Legacy Scriptlet Native ABI Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add byte-preserving native ABI entries beside the current flattened
scriptlet API for RPM, DEB, and Arch, without changing install/update/remove
behavior.

**Architecture:** Add a parser-owned native ABI model under
`crates/conary-core/src/packages/native_abi.rs` and expose it through
`PackageMetadata` and `PackageFormat`. Populate it in the RPM, DEB, and Arch
parsers while preserving the current `scriptlets()` compatibility projection.
Represent DEB `triggers` and Arch ALPM `.hook` files as passive
`ControlArtifact` entries so documented native transaction behavior is visible
without replay.

**Tech Stack:** Rust, existing package parsers, `rpm` crate metadata APIs,
`tar`/`ar` archive parsing, `crate::hash::sha256_prefixed`, Cargo unit and
integration tests.

---

## `/goal` Objective

Use this exact objective when starting execution:

```text
/goal Implement Goal 2: add byte-preserving native ABI entries beside the current flattened scriptlet API for RPM, DEB, and Arch, preserving lifecycle paths, parser support status, DEB trigger artifacts, Arch ALPM hook artifacts, RPM trigger families, RPM untransaction slots, and RPM verification scriptlets. Stop when parser fixture tests prove no native scriptlet or control-artifact slot is silently dropped, current flattened scriptlet behavior remains compatible, and no install/update/remove behavior changes.
```

## Read First

- `AGENTS.md`
- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-native-abi-extraction-design.md`
- `docs/superpowers/specs/2026-05-27-legacy-scriptlet-semantics-bundle-design.md`
- `docs/superpowers/plans/2026-05-27-legacy-scriptlet-semantics-bundle-goal-queue.md`

Primary external semantics references are already linked from the spec. Use them
to resolve lifecycle names and argument contracts; do not invent new native
slots.

## File Structure

Create:

- `crates/conary-core/src/packages/native_abi.rs`
- `crates/conary-core/tests/native_abi.rs`

Modify:

- `crates/conary-core/src/packages/mod.rs`
- `crates/conary-core/src/packages/traits.rs`
- `crates/conary-core/src/packages/common.rs`
- `crates/conary-core/src/packages/arch.rs`
- `crates/conary-core/src/packages/deb.rs`
- `crates/conary-core/src/packages/rpm.rs`
- `apps/conary/tests/conversion_integration.rs`
- `apps/conary/src/commands/install/conversion.rs`
- `apps/remi/src/server/conversion.rs`
- `crates/conary-core/src/ccs/convert/converter.rs`

Only `conary_core::packages::common::PackageMetadata` receives the
`native_scriptlet_abi` field. Use this command to audit name collisions, but do
not edit repository metadata structs under `crates/conary-core/src/repository/`:

```bash
rg -n "PackageMetadata \\{" apps crates -g '*.rs'
```

Expected local package metadata literal sites to update in Task 1 are:

- `crates/conary-core/src/packages/arch.rs`
- `crates/conary-core/src/packages/deb.rs`
- `crates/conary-core/src/packages/rpm.rs`
- `apps/conary/src/commands/install/conversion.rs`
- `apps/conary/tests/conversion_integration.rs`
- `apps/remi/src/server/conversion.rs`
- `crates/conary-core/src/ccs/convert/converter.rs`

## Safety Rules

- Do not change install, update, remove, adoption, unadoption, replay, or Remi
  publication behavior.
- Do not add Remi bundle embedding, database migrations, CCS archive format
  changes, or `NativeScriptletEntry` to `LegacyScriptletEntry` conversion.
- Keep `scriptlets()` returning the current flattened `Scriptlet` slice.
- Keep native-only entries out of the flattened API unless the old API already
  represented them.
- Preserve bytes first. `String` decoding is a convenience for UTF-8 bodies, not
  the source of truth.
- Treat parser `DeferredReview` and `Unpreservable` as parser evidence only;
  they are not bundle-level publication decisions.

## Task 1: Native ABI Model And Public Parser API

**Files:**

- Create: `crates/conary-core/src/packages/native_abi.rs`
- Modify: `crates/conary-core/src/packages/mod.rs`
- Modify: `crates/conary-core/src/packages/traits.rs`
- Modify: `crates/conary-core/src/packages/common.rs`
- Modify: local package metadata literals in
  `crates/conary-core/src/packages/arch.rs`,
  `crates/conary-core/src/packages/deb.rs`,
  `crates/conary-core/src/packages/rpm.rs`,
  `apps/conary/src/commands/install/conversion.rs`,
  `apps/conary/tests/conversion_integration.rs`,
  `apps/remi/src/server/conversion.rs`, and
  `crates/conary-core/src/ccs/convert/converter.rs`
- Do not modify: `crates/conary-core/src/repository/metadata.rs` or
  `crates/conary-core/src/repository/parsers/*.rs`

- [ ] **Step 1: Write failing native ABI model tests**

Create `crates/conary-core/src/packages/native_abi.rs` with the path comment,
minimal imports, and this test module. The module will not compile until Step 3
adds the types.

```rust
// conary-core/src/packages/native_abi.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_scriptlet_body_preserves_binary_bytes_and_digest() {
        let bytes = b"#!/bin/sh\nprintf '\\xff'\n\xff".to_vec();

        let body = NativeScriptletBody::from_bytes(bytes.clone());

        assert_eq!(body.bytes, bytes);
        assert_eq!(body.text, None);
        assert_eq!(body.encoding, NativeScriptletBodyEncoding::Binary);
        assert_eq!(
            body.sha256,
            crate::hash::sha256_prefixed(b"#!/bin/sh\nprintf '\\xff'\n\xff")
        );
    }

    #[test]
    fn native_scriptlet_body_records_utf8_text() {
        let body = NativeScriptletBody::from_bytes(b"echo ok\n".to_vec());

        assert_eq!(body.text.as_deref(), Some("echo ok\n"));
        assert_eq!(body.encoding, NativeScriptletBodyEncoding::Utf8);
        assert_eq!(body.bytes, b"echo ok\n");
    }

    #[test]
    fn split_shebang_preserves_interpreter_arguments() {
        let (interpreter, args) = split_shebang("#!/usr/bin/perl -w -T");

        assert_eq!(interpreter.as_deref(), Some("/usr/bin/perl"));
        assert_eq!(args, vec!["-w".to_string(), "-T".to_string()]);
    }

    #[test]
    fn split_shebang_defaults_to_bin_sh_without_shebang() {
        let (interpreter, args) = split_shebang("echo no shebang");

        assert_eq!(interpreter.as_deref(), Some("/bin/sh"));
        assert!(args.is_empty());
    }

    #[test]
    fn native_support_uses_parser_neutral_names() {
        assert!(NativeScriptletSupport::Parsed.reason_code().is_none());
        assert_eq!(
            NativeScriptletSupport::DeferredReview {
                reason_code: "rpm-verify-scriptlet-deferred".to_string(),
            }
            .reason_code(),
            Some("rpm-verify-scriptlet-deferred")
        );
        assert_eq!(
            NativeScriptletSupport::Unpreservable {
                reason_code: "native-abi-parser-limitation".to_string(),
            }
            .reason_code(),
            Some("native-abi-parser-limitation")
        );
    }
}
```

- [ ] **Step 2: Run the first failing test**

Run:

```bash
cargo test -p conary-core native_scriptlet_body_preserves_binary_bytes_and_digest
```

Expected: compile failure mentioning missing native ABI types.

- [ ] **Step 3: Implement `native_abi.rs` types and helpers**

Replace the temporary file with this module body. Keep the field names stable
because later tasks and tests rely on them.

```rust
// conary-core/src/packages/native_abi.rs
//! Native package-manager scriptlet ABI metadata captured by package parsers.

use crate::hash;
use crate::packages::traits::ScriptletPhase;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeScriptletFormat {
    Rpm,
    Deb,
    Arch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeScriptletKind {
    Executable,
    ControlArtifact,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeScriptletBody {
    pub bytes: Vec<u8>,
    pub text: Option<String>,
    pub encoding: NativeScriptletBodyEncoding,
    pub sha256: String,
}

impl NativeScriptletBody {
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        let text = String::from_utf8(bytes.clone()).ok();
        let encoding = if text.is_some() {
            NativeScriptletBodyEncoding::Utf8
        } else {
            NativeScriptletBodyEncoding::Binary
        };

        Self {
            sha256: hash::sha256_prefixed(&bytes),
            bytes,
            text,
            encoding,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeScriptletBodyEncoding {
    Utf8,
    Binary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeScriptletSupport {
    Parsed,
    DeferredReview { reason_code: String },
    Unpreservable { reason_code: String },
}

impl NativeScriptletSupport {
    pub fn reason_code(&self) -> Option<&str> {
        match self {
            Self::Parsed => None,
            Self::DeferredReview { reason_code } | Self::Unpreservable { reason_code } => {
                Some(reason_code.as_str())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeScriptletMetadata {
    Rpm(RpmNativeScriptletMetadata),
    Deb(DebNativeScriptletMetadata),
    Arch(ArchNativeScriptletMetadata),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeInvocationContract {
    pub args: Vec<NativeArgumentContract>,
    pub environment: Vec<NativeEnvironmentFact>,
    pub stdin: NativeStdinContract,
    pub root: NativeRootExpectation,
}

impl NativeInvocationContract {
    pub fn none() -> Self {
        Self {
            args: Vec::new(),
            environment: Vec::new(),
            stdin: NativeStdinContract::None,
            root: NativeRootExpectation::PackageManagerDefault,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeArgumentContract {
    pub index: usize,
    pub name: String,
    pub value: NativeArgumentValue,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeArgumentValue {
    Action,
    OldVersion,
    NewVersion,
    PackageInstanceCount,
    PackageName,
    TriggerName,
    TriggerCount,
    FilePath,
    Raw(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeEnvironmentFact {
    pub name: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeStdinContract {
    None,
    Debconf,
    Paths,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeRootExpectation {
    PackageManagerDefault,
    InstallRoot,
    HostRoot,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeTransactionOrder {
    pub position: NativeTransactionPosition,
    pub relative_to: Option<String>,
}

impl NativeTransactionOrder {
    pub fn new(position: NativeTransactionPosition) -> Self {
        Self {
            position,
            relative_to: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpmNativeScriptletMetadata {
    pub slot: RpmScriptletSlot,
    pub scriptlet_flags: Option<RpmScriptletFlagsMetadata>,
    pub trigger: Option<RpmTriggerMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpmScriptletFlagsMetadata {
    pub names: Vec<String>,
    pub raw_bits: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpmTriggerMetadata {
    pub family: RpmTriggerFamily,
    pub conditions: Vec<RpmTriggerCondition>,
    pub file_globs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpmTriggerFamily {
    Package,
    File,
    TransactionFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpmTriggerAction {
    PreInstall,
    Install,
    Uninstall,
    PostUninstall,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpmTriggerCondition {
    pub name: String,
    pub action: RpmTriggerAction,
    pub version: Option<String>,
    pub comparison: Option<String>,
    pub raw_flags: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebNativeScriptletMetadata {
    pub control_member: DebControlMember,
    pub maintainer_modes: Vec<DebMaintainerInvocation>,
    pub trigger_declarations: Vec<DebTriggerDeclaration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebControlMember {
    Config,
    Preinst,
    Postinst,
    Prerm,
    Postrm,
    Triggers,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebMaintainerInvocation {
    pub mode: DebMaintainerMode,
    pub args: Vec<NativeArgumentContract>,
    pub lifecycle_paths: Vec<NativeLifecyclePath>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebMaintainerMode {
    Install,
    Configure,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebTriggerDeclaration {
    pub directive: DebTriggerDirective,
    pub trigger_name: String,
    pub await_mode: DebTriggerAwaitMode,
    pub raw_line: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebTriggerDirective {
    Interest,
    Activate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebTriggerAwaitMode {
    Default,
    Await,
    NoAwait,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchNativeScriptletMetadata {
    Install(ArchInstallScriptletMetadata),
    AlpmHook(ArchAlpmHookMetadata),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchInstallScriptletMetadata {
    pub install_source_sha256: String,
    pub function_name: String,
    pub function_body: Option<String>,
    pub function_body_sha256: Option<String>,
    pub extraction_status: ArchFunctionExtractionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchFunctionExtractionStatus {
    Parsed,
    DeferredReview { reason_code: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchAlpmHookMetadata {
    pub hook_path: String,
    pub triggers: Vec<ArchAlpmHookTrigger>,
    pub action: Option<ArchAlpmHookAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchAlpmHookTrigger {
    pub operations: Vec<ArchAlpmHookOperation>,
    pub trigger_type: ArchAlpmHookTriggerType,
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchAlpmHookOperation {
    Install,
    Upgrade,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchAlpmHookTriggerType {
    Package,
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchAlpmHookAction {
    pub description: Option<String>,
    pub when: NativeTransactionPosition,
    pub exec: String,
    pub depends: Vec<String>,
    pub abort_on_fail: bool,
    pub needs_targets: bool,
}

// The typed ALPM action model follows the current alpm-hooks(5) fields.
// Unknown or future directives remain preserved in NativeScriptletEntry::body.
pub fn split_shebang(script_text: &str) -> (Option<String>, Vec<String>) {
    let Some(first_line) = script_text.lines().next() else {
        return (Some("/bin/sh".to_string()), Vec::new());
    };
    let Some(rest) = first_line.strip_prefix("#!") else {
        return (Some("/bin/sh".to_string()), Vec::new());
    };
    let mut parts = rest.split_whitespace();
    let interpreter = parts.next().map(str::to_string);
    let args = parts.map(str::to_string).collect();
    (interpreter.or_else(|| Some("/bin/sh".to_string())), args)
}
```

- [ ] **Step 4: Wire the public API**

Modify `crates/conary-core/src/packages/mod.rs`:

```rust
pub mod native_abi;
```

Modify `crates/conary-core/src/packages/traits.rs`:

```rust
pub use crate::packages::native_abi::*;
```

Add the trait method near `scriptlets()`:

```rust
fn native_scriptlet_abi(&self) -> &[NativeScriptletEntry] {
    &[]
}
```

Modify `crates/conary-core/src/packages/common.rs`:

```rust
use crate::packages::traits::{
    ConfigFileInfo, Dependency, NativeScriptletEntry, PackageFile, Scriptlet,
};

pub struct PackageMetadata {
    // existing fields...
    pub scriptlets: Vec<Scriptlet>,
    pub native_scriptlet_abi: Vec<NativeScriptletEntry>,
    pub config_files: Vec<ConfigFileInfo>,
}

pub fn new(package_path: PathBuf, name: String, version: String) -> Self {
    Self {
        // existing fields...
        scriptlets: Vec::new(),
        native_scriptlet_abi: Vec::new(),
        config_files: Vec::new(),
    }
}

pub fn native_scriptlet_abi(&self) -> &[NativeScriptletEntry] {
    &self.native_scriptlet_abi
}
```

- [ ] **Step 5: Update `PackageMetadata` literals**

Run:

```bash
rg -n "PackageMetadata \\{" apps crates -g '*.rs'
```

Update only literals that construct
`conary_core::packages::common::PackageMetadata`. Do not add
`native_scriptlet_abi` to `crates/conary-core/src/repository/metadata.rs` or
`crates/conary-core/src/repository/parsers/*.rs`; those are unrelated
repository index metadata types with the same Rust name.

For these package metadata literals, add:

- `crates/conary-core/src/packages/arch.rs`
- `crates/conary-core/src/packages/deb.rs`
- `crates/conary-core/src/packages/rpm.rs`
- `apps/conary/src/commands/install/conversion.rs`
- `apps/conary/tests/conversion_integration.rs`
- `apps/remi/src/server/conversion.rs`
- `crates/conary-core/src/ccs/convert/converter.rs`

For every package metadata literal that already sets `scriptlets`, add:

```rust
native_scriptlet_abi: Vec::new(),
```

For literals that use `Default::default()` on vectors, use:

```rust
native_scriptlet_abi: Default::default(),
```

Parser literals in `rpm.rs`, `deb.rs`, and `arch.rs` will be replaced with real
native ABI vectors in later tasks.

- [ ] **Step 6: Prove the scaffolding compiles and tests pass**

Run:

```bash
cargo test -p conary-core native_abi
cargo test -p conary-core test_package_metadata_new
cargo test -p conary conversion_integration
```

Expected: all three commands pass. `cargo test -p conary conversion_integration`
is included because several app tests use `PackageMetadata` literals.

- [ ] **Step 7: Commit Task 1**

```bash
git add crates/conary-core/src/packages/native_abi.rs \
        crates/conary-core/src/packages/mod.rs \
        crates/conary-core/src/packages/traits.rs \
        crates/conary-core/src/packages/common.rs \
        apps/conary/tests/conversion_integration.rs \
        apps/conary/src/commands/install/conversion.rs \
        apps/remi/src/server/conversion.rs \
        crates/conary-core/src/ccs/convert/converter.rs
git commit -m "feat(packages): add native scriptlet abi model"
```

## Task 2: Arch `.INSTALL` And ALPM Hook Native ABI

**Files:**

- Modify: `crates/conary-core/src/packages/arch.rs`

- [ ] **Step 1: Write failing Arch native ABI tests**

Add tests to the existing `#[cfg(test)] mod tests` in `arch.rs`:

```rust
#[test]
fn arch_native_abi_preserves_full_install_source_and_function_body() {
    let install = br#"
pre_install() {
    echo "before $1"
}

post_upgrade() {
    echo "after $2 -> $1"
}
"#;

    let entries = ArchPackage::native_abi_from_install_bytes(install);

    assert_eq!(entries.len(), 2);
    let pre = entries
        .iter()
        .find(|entry| entry.native_slot == "pre_install")
        .expect("pre_install entry");
    assert_eq!(pre.format, NativeScriptletFormat::Arch);
    assert_eq!(pre.primary_lifecycle, NativeLifecyclePath::PreInstall);
    assert_eq!(pre.compatibility_phase, Some(ScriptletPhase::PreInstall));
    assert_eq!(pre.interpreter.as_deref(), Some("/bin/sh"));
    assert_eq!(pre.body.bytes, install);

    let NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::Install(meta)) =
        &pre.metadata
    else {
        panic!("expected arch install metadata");
    };
    assert_eq!(meta.function_name, "pre_install");
    assert_eq!(
        meta.function_body.as_deref(),
        Some("\n    echo \"before $1\"\n")
    );
    assert_eq!(
        meta.function_body_sha256.as_deref(),
        Some(crate::hash::sha256_prefixed(b"\n    echo \"before $1\"\n").as_str())
    );
}

#[test]
fn arch_native_abi_preserves_alpm_hook_control_artifact() {
    let hook = br#"
[Trigger]
Operation = Install
Operation = Upgrade
Type = Path
Target = usr/share/mime/*

[Action]
Description = update mime cache
When = PostTransaction
Exec = /usr/bin/update-mime-database /usr/share/mime
Depends = shared-mime-info
NeedsTargets
"#;

    let entry = ArchPackage::native_abi_from_alpm_hook(
        "/usr/share/libalpm/hooks/30-update-mime.hook",
        hook,
    );

    assert_eq!(entry.kind, NativeScriptletKind::ControlArtifact);
    assert_eq!(entry.native_slot, "alpm-hook:/usr/share/libalpm/hooks/30-update-mime.hook");
    assert_eq!(entry.primary_lifecycle, NativeLifecyclePath::Trigger);
    assert_eq!(entry.order.position, NativeTransactionPosition::ControlArtifact);
    assert_eq!(entry.interpreter, None);
    assert_eq!(entry.body.bytes, hook);

    let NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::AlpmHook(meta)) =
        &entry.metadata
    else {
        panic!("expected arch alpm hook metadata");
    };
    assert_eq!(meta.triggers.len(), 1);
    assert_eq!(meta.triggers[0].operations, vec![
        ArchAlpmHookOperation::Install,
        ArchAlpmHookOperation::Upgrade,
    ]);
    assert_eq!(meta.triggers[0].trigger_type, ArchAlpmHookTriggerType::Path);
    assert_eq!(meta.triggers[0].targets, vec!["usr/share/mime/*".to_string()]);
    assert_eq!(meta.action.as_ref().expect("action").when, NativeTransactionPosition::AfterTransaction);
    assert!(meta.action.as_ref().expect("action").needs_targets);
}

#[test]
fn arch_native_abi_falls_back_when_function_extraction_fails() {
    let install = b"pre_install() {\necho '{' unbalanced\n";
    let entries = ArchPackage::native_abi_from_install_bytes(install);

    let pre = entries
        .iter()
        .find(|entry| entry.native_slot == "pre_install")
        .expect("pre_install entry");

    assert_eq!(
        pre.support.reason_code(),
        Some("arch-install-function-extraction-deferred")
    );

    let NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::Install(meta)) =
        &pre.metadata
    else {
        panic!("expected arch install metadata");
    };
    assert_eq!(
        meta.extraction_status,
        ArchFunctionExtractionStatus::DeferredReview {
            reason_code: "arch-install-function-extraction-deferred".to_string(),
        }
    );
    assert_eq!(meta.function_body, None);
}

#[test]
fn arch_compat_scriptlets_still_return_function_bodies() {
    let install = r#"
post_install() {
    echo "compat body"
}
"#;

    let scriptlets = ArchPackage::parse_install_script(install);

    assert_eq!(scriptlets.len(), 1);
    assert_eq!(scriptlets[0].phase, ScriptletPhase::PostInstall);
    assert_eq!(scriptlets[0].content, "\n    echo \"compat body\"\n");
}
```

- [ ] **Step 2: Run the failing Arch tests**

Run:

```bash
cargo test -p conary-core arch_native_abi_preserves_full_install_source_and_function_body
cargo test -p conary-core arch_native_abi_preserves_alpm_hook_control_artifact
cargo test -p conary-core arch_native_abi_falls_back_when_function_extraction_fails
cargo test -p conary-core arch_compat_scriptlets_still_return_function_bodies
```

Expected: the first three commands fail because the native ABI helpers do not
exist. The compatibility test should pass before implementation and continue to
pass afterward.

- [ ] **Step 3: Add Arch native ABI imports and constants**

Add imports near the existing package imports in `arch.rs`:

```rust
use crate::packages::traits::{
    ArchAlpmHookAction, ArchAlpmHookMetadata, ArchAlpmHookOperation,
    ArchAlpmHookTrigger, ArchAlpmHookTriggerType, ArchFunctionExtractionStatus,
    ArchInstallScriptletMetadata, ArchNativeScriptletMetadata, ConfigFileInfo,
    Dependency, DependencyType, ExtractedFile, NativeArgumentContract,
    NativeArgumentValue, NativeInvocationContract, NativeLifecyclePath,
    NativeRootExpectation, NativeScriptletBody, NativeScriptletEntry,
    NativeScriptletFormat, NativeScriptletKind, NativeScriptletMetadata,
    NativeScriptletSupport, NativeStdinContract, NativeTransactionOrder,
    NativeTransactionPosition, PackageFile, PackageFormat, Scriptlet,
    ScriptletPhase,
};
```

Add lifecycle metadata beside `ARCH_METADATA_FILES`:

```rust
const ARCH_INSTALL_FUNCTIONS: &[(&str, ScriptletPhase, NativeLifecyclePath)] = &[
    ("pre_install", ScriptletPhase::PreInstall, NativeLifecyclePath::PreInstall),
    ("post_install", ScriptletPhase::PostInstall, NativeLifecyclePath::PostInstall),
    ("pre_upgrade", ScriptletPhase::PreUpgrade, NativeLifecyclePath::PreUpgrade),
    ("post_upgrade", ScriptletPhase::PostUpgrade, NativeLifecyclePath::PostUpgrade),
    ("pre_remove", ScriptletPhase::PreRemove, NativeLifecyclePath::PreRemove),
    ("post_remove", ScriptletPhase::PostRemove, NativeLifecyclePath::PostRemove),
];
```

- [ ] **Step 4: Implement `.INSTALL` native ABI extraction**

Add these helpers to `impl ArchPackage`:

```rust
fn native_abi_from_install_bytes(bytes: &[u8]) -> Vec<NativeScriptletEntry> {
    let Some(source) = String::from_utf8(bytes.to_vec()).ok() else {
        return vec![NativeScriptletEntry {
            id: "arch:.INSTALL".to_string(),
            format: NativeScriptletFormat::Arch,
            kind: NativeScriptletKind::ControlArtifact,
            native_slot: ".INSTALL".to_string(),
            primary_lifecycle: NativeLifecyclePath::Trigger,
            compatibility_phase: None,
            lifecycle_paths: Vec::new(),
            interpreter: Some("/bin/sh".to_string()),
            interpreter_args: Vec::new(),
            body: NativeScriptletBody::from_bytes(bytes.to_vec()),
            invocation: NativeInvocationContract {
                args: Vec::new(),
                environment: Vec::new(),
                stdin: NativeStdinContract::None,
                root: NativeRootExpectation::InstallRoot,
            },
            order: NativeTransactionOrder::new(NativeTransactionPosition::ControlArtifact),
            support: NativeScriptletSupport::DeferredReview {
                reason_code: "native-abi-parser-limitation".to_string(),
            },
            metadata: NativeScriptletMetadata::Arch(
                ArchNativeScriptletMetadata::Install(ArchInstallScriptletMetadata {
                    install_source_sha256: crate::hash::sha256_prefixed(bytes),
                    function_name: ".INSTALL".to_string(),
                    function_body: None,
                    function_body_sha256: None,
                    extraction_status: ArchFunctionExtractionStatus::DeferredReview {
                        reason_code: "native-abi-parser-limitation".to_string(),
                    },
                }),
            ),
        }];
    };

    ARCH_INSTALL_FUNCTIONS
        .iter()
        .filter(|(function_name, _, _)| Self::contains_function_declaration(&source, function_name))
        .map(|(function_name, phase, lifecycle)| {
            let function_body = Self::extract_function(&source, function_name);
            let extraction_status = if function_body.is_some() {
                ArchFunctionExtractionStatus::Parsed
            } else {
                ArchFunctionExtractionStatus::DeferredReview {
                    reason_code: "arch-install-function-extraction-deferred".to_string(),
                }
            };
            let function_body_sha256 = function_body
                .as_ref()
                .map(|body| crate::hash::sha256_prefixed(body.as_bytes()));

            NativeScriptletEntry {
                id: format!("arch:{function_name}"),
                format: NativeScriptletFormat::Arch,
                kind: NativeScriptletKind::Executable,
                native_slot: (*function_name).to_string(),
                primary_lifecycle: *lifecycle,
                compatibility_phase: function_body.as_ref().map(|_| *phase),
                lifecycle_paths: vec![*lifecycle],
                interpreter: Some("/bin/sh".to_string()),
                interpreter_args: Vec::new(),
                body: NativeScriptletBody::from_bytes(bytes.to_vec()),
                invocation: Self::arch_invocation_for_function(function_name),
                order: NativeTransactionOrder::new(match lifecycle {
                    NativeLifecyclePath::PreInstall
                    | NativeLifecyclePath::PreUpgrade
                    | NativeLifecyclePath::PreRemove => NativeTransactionPosition::BeforePayload,
                    _ => NativeTransactionPosition::AfterPayload,
                }),
                support: if function_body.is_some() {
                    NativeScriptletSupport::Parsed
                } else {
                    NativeScriptletSupport::DeferredReview {
                        reason_code: "arch-install-function-extraction-deferred".to_string(),
                    }
                },
                metadata: NativeScriptletMetadata::Arch(
                    ArchNativeScriptletMetadata::Install(ArchInstallScriptletMetadata {
                        install_source_sha256: crate::hash::sha256_prefixed(bytes),
                        function_name: (*function_name).to_string(),
                        function_body,
                        function_body_sha256,
                        extraction_status,
                    }),
                ),
            }
        })
        .collect()
}

fn contains_function_declaration(source: &str, function_name: &str) -> bool {
    source.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with(&format!("{function_name}()"))
            || trimmed.starts_with(&format!("{function_name} ()"))
            || trimmed.starts_with(&format!("function {function_name}"))
    })
}

fn arch_invocation_for_function(function_name: &str) -> NativeInvocationContract {
    let args = match function_name {
        // alpm-install-scriptlet(5) passes new version as $1 and old version as
        // $2 for both pre_upgrade and post_upgrade.
        "pre_upgrade" | "post_upgrade" => vec![
            NativeArgumentContract {
                index: 1,
                name: "new-version".to_string(),
                value: NativeArgumentValue::NewVersion,
                required: true,
            },
            NativeArgumentContract {
                index: 2,
                name: "old-version".to_string(),
                value: NativeArgumentValue::OldVersion,
                required: true,
            },
        ],
        "pre_install" | "post_install" => vec![NativeArgumentContract {
            index: 1,
            name: "new-version".to_string(),
            value: NativeArgumentValue::NewVersion,
            required: true,
        }],
        "pre_remove" | "post_remove" => vec![NativeArgumentContract {
            index: 1,
            name: "old-version".to_string(),
            value: NativeArgumentValue::OldVersion,
            required: true,
        }],
        _ => Vec::new(),
    };

    NativeInvocationContract {
        args,
        environment: Vec::new(),
        stdin: NativeStdinContract::None,
        root: NativeRootExpectation::InstallRoot,
    }
}
```

- [ ] **Step 5: Implement ALPM hook artifact extraction**

Add this helper pair:

```rust
fn native_abi_from_alpm_hook(path: &str, bytes: &[u8]) -> NativeScriptletEntry {
    let text = String::from_utf8(bytes.to_vec()).unwrap_or_default();
    let (triggers, action) = Self::parse_alpm_hook_metadata(&text);

    NativeScriptletEntry {
        id: format!("arch:alpm-hook:{path}"),
        format: NativeScriptletFormat::Arch,
        kind: NativeScriptletKind::ControlArtifact,
        native_slot: format!("alpm-hook:{path}"),
        primary_lifecycle: NativeLifecyclePath::Trigger,
        compatibility_phase: None,
        lifecycle_paths: vec![NativeLifecyclePath::Trigger],
        interpreter: None,
        interpreter_args: Vec::new(),
        body: NativeScriptletBody::from_bytes(bytes.to_vec()),
        invocation: NativeInvocationContract {
            args: Vec::new(),
            environment: Vec::new(),
            stdin: if action.as_ref().is_some_and(|action| action.needs_targets) {
                NativeStdinContract::Paths
            } else {
                NativeStdinContract::None
            },
            root: NativeRootExpectation::InstallRoot,
        },
        order: NativeTransactionOrder::new(NativeTransactionPosition::ControlArtifact),
        support: NativeScriptletSupport::DeferredReview {
            reason_code: "arch-alpm-hook-semantics-deferred".to_string(),
        },
        metadata: NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::AlpmHook(
            ArchAlpmHookMetadata {
                hook_path: path.to_string(),
                triggers,
                action,
            },
        )),
    }
}

fn parse_alpm_hook_metadata(text: &str) -> (Vec<ArchAlpmHookTrigger>, Option<ArchAlpmHookAction>) {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Section {
        None,
        Trigger,
        Action,
    }

    let mut section = Section::None;
    let mut triggers = Vec::new();
    let mut current_trigger: Option<ArchAlpmHookTrigger> = None;
    let mut action = ArchAlpmHookAction {
        description: None,
        when: NativeTransactionPosition::AfterTransaction,
        exec: String::new(),
        depends: Vec::new(),
        abort_on_fail: false,
        needs_targets: false,
    };

    for raw_line in text.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        match line {
            "[Trigger]" => {
                if let Some(trigger) = current_trigger.take() {
                    triggers.push(trigger);
                }
                current_trigger = Some(ArchAlpmHookTrigger {
                    operations: Vec::new(),
                    trigger_type: ArchAlpmHookTriggerType::Package,
                    targets: Vec::new(),
                });
                section = Section::Trigger;
                continue;
            }
            "[Action]" => {
                if let Some(trigger) = current_trigger.take() {
                    triggers.push(trigger);
                }
                section = Section::Action;
                continue;
            }
            "AbortOnFail" if section == Section::Action => {
                action.abort_on_fail = true;
                continue;
            }
            "NeedsTargets" if section == Section::Action => {
                action.needs_targets = true;
                continue;
            }
            _ => {}
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().to_string();

        match (section, key) {
            (Section::Trigger, "Operation") => {
                if let Some(trigger) = current_trigger.as_mut() {
                    match value.as_str() {
                        "Install" => trigger.operations.push(ArchAlpmHookOperation::Install),
                        "Upgrade" => trigger.operations.push(ArchAlpmHookOperation::Upgrade),
                        "Remove" => trigger.operations.push(ArchAlpmHookOperation::Remove),
                        _ => {}
                    }
                }
            }
            (Section::Trigger, "Type") => {
                if let Some(trigger) = current_trigger.as_mut() {
                    trigger.trigger_type = if value == "Path" || value == "File" {
                        ArchAlpmHookTriggerType::Path
                    } else {
                        ArchAlpmHookTriggerType::Package
                    };
                }
            }
            (Section::Trigger, "Target") => {
                if let Some(trigger) = current_trigger.as_mut() {
                    trigger.targets.push(value);
                }
            }
            (Section::Action, "Description") => action.description = Some(value),
            (Section::Action, "When") => {
                action.when = if value == "PreTransaction" {
                    NativeTransactionPosition::BeforeTransaction
                } else {
                    NativeTransactionPosition::AfterTransaction
                };
            }
            (Section::Action, "Exec") => action.exec = value,
            (Section::Action, "Depends") => action.depends.push(value),
            _ => {}
        }
    }

    if let Some(trigger) = current_trigger {
        triggers.push(trigger);
    }

    let action = if action.exec.is_empty() { None } else { Some(action) };
    (triggers, action)
}
```

- [ ] **Step 6: Populate Arch package metadata**

In `parse()`, replace `install_content: Option<String>` with
`install_bytes: Option<Vec<u8>>` and add:

```rust
let mut alpm_hook_bytes: Vec<(String, Vec<u8>)> = Vec::new();
```

Read `.INSTALL` as bytes:

```rust
".INSTALL" => {
    let mut content = Vec::new();
    entry
        .read_to_end(&mut content)
        .map_err(|e| Error::InitError(format!("Failed to read .INSTALL: {}", e)))?;
    install_bytes = Some(content);
}
```

When processing regular payload files, normalize the path once and preserve
packaged hook bytes:

```rust
let normalized_path = normalize_path(&entry_path)
    .map_err(|e| Error::InitError(format!("Path normalization failed: {}", e)))?;
let is_alpm_hook = normalized_path.starts_with("/usr/share/libalpm/hooks/")
    && normalized_path.ends_with(".hook");

if is_alpm_hook && !entry_type.is_symlink() {
    let mut content = Vec::new();
    entry
        .read_to_end(&mut content)
        .map_err(|e| Error::InitError(format!("Failed to read ALPM hook: {}", e)))?;
    alpm_hook_bytes.push((normalized_path.clone(), content));
}
```

Build both fields:

```rust
let scriptlets = install_bytes
    .as_ref()
    .and_then(|bytes| String::from_utf8(bytes.clone()).ok())
    .map(|content| Self::parse_install_script(&content))
    .unwrap_or_default();

let mut native_scriptlet_abi = install_bytes
    .as_ref()
    .map(|bytes| Self::native_abi_from_install_bytes(bytes))
    .unwrap_or_default();
native_scriptlet_abi.extend(
    alpm_hook_bytes
        .iter()
        .map(|(path, bytes)| Self::native_abi_from_alpm_hook(path, bytes)),
);
```

Set the metadata field:

```rust
native_scriptlet_abi,
```

Add the trait accessor:

```rust
fn native_scriptlet_abi(&self) -> &[NativeScriptletEntry] {
    self.meta.native_scriptlet_abi()
}
```

- [ ] **Step 7: Run Arch tests**

Run:

```bash
cargo test -p conary-core arch_native_abi
cargo test -p conary-core arch_scriptlet
```

Expected: all Arch native ABI and compatibility tests pass.

- [ ] **Step 8: Commit Task 2**

```bash
git add crates/conary-core/src/packages/arch.rs
git commit -m "feat(packages): extract arch native scriptlet abi"
```

## Task 3: DEB Maintainer Scripts And Trigger Artifacts

**Files:**

- Modify: `crates/conary-core/src/packages/deb.rs`

- [ ] **Step 1: Write failing DEB native ABI helper tests**

Add tests to `deb.rs`:

```rust
#[test]
fn deb_shebang_split_preserves_flattened_interpreter_behavior() {
    let body = b"#!/usr/bin/perl -w\nprint qq(ok\\n);\n";
    let native = DebPackage::native_abi_from_control_member("postinst", body)
        .expect("native postinst");

    assert_eq!(native.interpreter.as_deref(), Some("/usr/bin/perl"));
    assert_eq!(native.interpreter_args, vec!["-w".to_string()]);

    let flattened = DebPackage::flattened_scriptlet_from_control_member("postinst", body)
        .expect("flattened postinst");
    assert_eq!(flattened.interpreter, "/usr/bin/perl -w");
}

#[test]
fn deb_native_abi_includes_config_as_native_only() {
    let entry = DebPackage::native_abi_from_control_member("config", b"#!/bin/sh\ndb_input high pkg/question\n")
        .expect("config entry");

    assert_eq!(entry.native_slot, "config");
    assert_eq!(entry.primary_lifecycle, NativeLifecyclePath::Config);
    assert_eq!(entry.compatibility_phase, None);
    assert_eq!(entry.invocation.stdin, NativeStdinContract::Debconf);
}

#[test]
fn deb_triggers_file_is_control_artifact_with_await_mode() {
    let body = b"interest-noawait update-icon-caches\nactivate-await ldconfig\n";
    let entry = DebPackage::native_abi_from_triggers_file(body);

    assert_eq!(entry.kind, NativeScriptletKind::ControlArtifact);
    assert_eq!(entry.native_slot, "triggers");
    assert_eq!(entry.interpreter, None);
    assert_eq!(entry.body.bytes, body);

    let NativeScriptletMetadata::Deb(meta) = &entry.metadata else {
        panic!("expected deb metadata");
    };
    assert_eq!(meta.control_member, DebControlMember::Triggers);
    assert_eq!(meta.trigger_declarations.len(), 2);
    assert_eq!(meta.trigger_declarations[0].directive, DebTriggerDirective::Interest);
    assert_eq!(meta.trigger_declarations[0].await_mode, DebTriggerAwaitMode::NoAwait);
    assert_eq!(meta.trigger_declarations[1].directive, DebTriggerDirective::Activate);
    assert_eq!(meta.trigger_declarations[1].await_mode, DebTriggerAwaitMode::Await);
}
```

- [ ] **Step 2: Run the failing DEB tests**

Run:

```bash
cargo test -p conary-core deb_shebang_split_preserves_flattened_interpreter_behavior
cargo test -p conary-core deb_native_abi_includes_config_as_native_only
cargo test -p conary-core deb_triggers_file_is_control_artifact_with_await_mode
```

Expected: compile failure for missing helper methods.

- [ ] **Step 3: Extend `ControlTarContents`**

Change the struct near the top of `deb.rs`:

```rust
#[derive(Default)]
struct ControlTarContents {
    control_text: Option<String>,
    scriptlets: Vec<Scriptlet>,
    native_scriptlet_abi: Vec<NativeScriptletEntry>,
    config_files: Vec<ConfigFileInfo>,
}
```

Update imports to include native ABI types and `split_shebang`.

- [ ] **Step 4: Implement DEB control-member helpers**

Add these helpers to `impl DebPackage`:

```rust
fn native_abi_from_control_member(name: &str, body: &[u8]) -> Option<NativeScriptletEntry> {
    let (control_member, lifecycle, compatibility_phase, stdin) = match name {
        "config" => (DebControlMember::Config, NativeLifecyclePath::Config, None, NativeStdinContract::Debconf),
        "preinst" => (DebControlMember::Preinst, NativeLifecyclePath::PreInstall, Some(ScriptletPhase::PreInstall), NativeStdinContract::None),
        "postinst" => (DebControlMember::Postinst, NativeLifecyclePath::PostInstall, Some(ScriptletPhase::PostInstall), NativeStdinContract::None),
        "prerm" => (DebControlMember::Prerm, NativeLifecyclePath::PreRemove, Some(ScriptletPhase::PreRemove), NativeStdinContract::None),
        "postrm" => (DebControlMember::Postrm, NativeLifecyclePath::PostRemove, Some(ScriptletPhase::PostRemove), NativeStdinContract::None),
        _ => return None,
    };

    if body.iter().all(|byte| byte.is_ascii_whitespace()) {
        return None;
    }

    let text = String::from_utf8(body.to_vec()).ok();
    let (interpreter, interpreter_args) = text
        .as_deref()
        .map(split_shebang)
        .unwrap_or((Some("/bin/sh".to_string()), Vec::new()));

    Some(NativeScriptletEntry {
        id: format!("deb:{name}"),
        format: NativeScriptletFormat::Deb,
        kind: NativeScriptletKind::Executable,
        native_slot: name.to_string(),
        primary_lifecycle: lifecycle,
        compatibility_phase,
        lifecycle_paths: Self::deb_lifecycle_paths(control_member),
        interpreter,
        interpreter_args,
        body: NativeScriptletBody::from_bytes(body.to_vec()),
        invocation: NativeInvocationContract {
            args: Vec::new(),
            environment: Vec::new(),
            stdin,
            root: NativeRootExpectation::PackageManagerDefault,
        },
        order: NativeTransactionOrder::new(match lifecycle {
            NativeLifecyclePath::PreInstall | NativeLifecyclePath::PreRemove => {
                NativeTransactionPosition::BeforePayload
            }
            NativeLifecyclePath::Config => NativeTransactionPosition::ControlArtifact,
            _ => NativeTransactionPosition::AfterPayload,
        }),
        support: NativeScriptletSupport::Parsed,
        metadata: NativeScriptletMetadata::Deb(DebNativeScriptletMetadata {
            control_member,
            maintainer_modes: Self::deb_maintainer_invocations(control_member),
            trigger_declarations: Vec::new(),
        }),
    })
}

fn flattened_scriptlet_from_control_member(name: &str, body: &[u8]) -> Option<Scriptlet> {
    let phase = match name {
        "preinst" => ScriptletPhase::PreInstall,
        "postinst" => ScriptletPhase::PostInstall,
        "prerm" => ScriptletPhase::PreRemove,
        "postrm" => ScriptletPhase::PostRemove,
        _ => return None,
    };
    let content = String::from_utf8(body.to_vec()).ok()?;
    if content.is_empty() {
        return None;
    }
    let interpreter = content
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("#!"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "/bin/sh".to_string());
    Some(Scriptlet {
        phase,
        interpreter,
        content,
        flags: None,
    })
}
```

Add the static Debian Policy invocation table. The argument indexes are shell
argument indexes: `$1` is the action word such as `upgrade`, so old/new version
arguments appear at `$2` and `$3` when the documented call mode includes them.

```rust
fn deb_lifecycle_paths(control_member: DebControlMember) -> Vec<NativeLifecyclePath> {
    match control_member {
        DebControlMember::Config => vec![NativeLifecyclePath::Config],
        DebControlMember::Preinst => vec![
            NativeLifecyclePath::PreInstall,
            NativeLifecyclePath::PreUpgrade,
            NativeLifecyclePath::Abort,
        ],
        DebControlMember::Postinst => vec![
            NativeLifecyclePath::PostInstall,
            NativeLifecyclePath::PostUpgrade,
            NativeLifecyclePath::Trigger,
            NativeLifecyclePath::Abort,
        ],
        DebControlMember::Prerm => vec![
            NativeLifecyclePath::PreRemove,
            NativeLifecyclePath::PreUpgrade,
            NativeLifecyclePath::Abort,
        ],
        DebControlMember::Postrm => vec![
            NativeLifecyclePath::PostRemove,
            NativeLifecyclePath::PostUpgrade,
            NativeLifecyclePath::Purge,
            NativeLifecyclePath::Abort,
        ],
        DebControlMember::Triggers => vec![NativeLifecyclePath::Trigger],
    }
}

fn deb_action_arg() -> NativeArgumentContract {
    NativeArgumentContract {
        index: 1,
        name: "action".to_string(),
        value: NativeArgumentValue::Action,
        required: true,
    }
}

fn deb_arg(
    index: usize,
    name: &str,
    value: NativeArgumentValue,
    required: bool,
) -> NativeArgumentContract {
    NativeArgumentContract {
        index,
        name: name.to_string(),
        value,
        required,
    }
}

fn deb_old_new_args(required: bool) -> Vec<NativeArgumentContract> {
    vec![
        Self::deb_arg(2, "old-version", NativeArgumentValue::OldVersion, required),
        Self::deb_arg(3, "new-version", NativeArgumentValue::NewVersion, required),
    ]
}

fn deb_new_version_arg(index: usize, required: bool) -> NativeArgumentContract {
    Self::deb_arg(index, "new-version", NativeArgumentValue::NewVersion, required)
}

fn deb_marker_arg(index: usize, marker: &str, required: bool) -> NativeArgumentContract {
    Self::deb_arg(
        index,
        marker,
        NativeArgumentValue::Raw(marker.to_string()),
        required,
    )
}

fn deb_package_arg(index: usize, name: &str, required: bool) -> NativeArgumentContract {
    Self::deb_arg(index, name, NativeArgumentValue::PackageName, required)
}

fn deb_version_arg(
    index: usize,
    name: &str,
    value: NativeArgumentValue,
    required: bool,
) -> NativeArgumentContract {
    Self::deb_arg(index, name, value, required)
}

fn deb_invocation(
    mode: DebMaintainerMode,
    mut args: Vec<NativeArgumentContract>,
    lifecycle_paths: Vec<NativeLifecyclePath>,
) -> DebMaintainerInvocation {
    let mut full_args = vec![Self::deb_action_arg()];
    full_args.append(&mut args);
    DebMaintainerInvocation {
        mode,
        args: full_args,
        lifecycle_paths,
    }
}

fn deb_maintainer_invocations(control_member: DebControlMember) -> Vec<DebMaintainerInvocation> {
    match control_member {
        DebControlMember::Config => vec![Self::deb_invocation(
            DebMaintainerMode::Configure,
            Vec::new(),
            vec![NativeLifecyclePath::Config],
        )],
        DebControlMember::Preinst => vec![
            Self::deb_invocation(
                DebMaintainerMode::Install,
                Self::deb_old_new_args(false),
                vec![NativeLifecyclePath::PreInstall],
            ),
            Self::deb_invocation(
                DebMaintainerMode::Upgrade,
                Self::deb_old_new_args(true),
                vec![NativeLifecyclePath::PreUpgrade],
            ),
            Self::deb_invocation(
                DebMaintainerMode::AbortUpgrade,
                vec![Self::deb_new_version_arg(2, true)],
                vec![NativeLifecyclePath::Abort],
            ),
        ],
        DebControlMember::Postinst => vec![
            Self::deb_invocation(
                DebMaintainerMode::Configure,
                vec![Self::deb_arg(
                    2,
                    "most-recently-configured-version",
                    NativeArgumentValue::Raw("most-recently-configured-version".to_string()),
                    false,
                )],
                vec![NativeLifecyclePath::PostInstall, NativeLifecyclePath::PostUpgrade],
            ),
            Self::deb_invocation(
                DebMaintainerMode::Triggered,
                vec![Self::deb_arg(2, "trigger-name", NativeArgumentValue::TriggerName, true)],
                vec![NativeLifecyclePath::Trigger],
            ),
            Self::deb_invocation(
                DebMaintainerMode::AbortUpgrade,
                vec![Self::deb_new_version_arg(2, true)],
                vec![NativeLifecyclePath::Abort],
            ),
            Self::deb_invocation(
                DebMaintainerMode::AbortRemove,
                Vec::new(),
                vec![NativeLifecyclePath::Abort],
            ),
            Self::deb_invocation(
                DebMaintainerMode::AbortRemove,
                vec![
                    Self::deb_marker_arg(2, "in-favour", true),
                    Self::deb_package_arg(3, "package", true),
                    Self::deb_new_version_arg(4, true),
                ],
                vec![NativeLifecyclePath::Abort],
            ),
            Self::deb_invocation(
                DebMaintainerMode::AbortDeconfigure,
                vec![
                    Self::deb_marker_arg(2, "in-favour", true),
                    Self::deb_package_arg(3, "failed-install-package", true),
                    Self::deb_version_arg(4, "failed-install-version", NativeArgumentValue::NewVersion, true),
                    Self::deb_marker_arg(5, "removing", false),
                    Self::deb_package_arg(6, "conflicting-package", false),
                    Self::deb_version_arg(7, "conflicting-version", NativeArgumentValue::OldVersion, false),
                ],
                vec![NativeLifecyclePath::Abort],
            ),
        ],
        DebControlMember::Prerm => vec![
            Self::deb_invocation(
                DebMaintainerMode::Remove,
                Vec::new(),
                vec![NativeLifecyclePath::PreRemove],
            ),
            Self::deb_invocation(
                DebMaintainerMode::Remove,
                vec![
                    Self::deb_marker_arg(2, "in-favour", true),
                    Self::deb_package_arg(3, "package", true),
                    Self::deb_new_version_arg(4, true),
                ],
                vec![NativeLifecyclePath::PreRemove],
            ),
            Self::deb_invocation(
                DebMaintainerMode::Upgrade,
                vec![Self::deb_new_version_arg(2, true)],
                vec![NativeLifecyclePath::PreUpgrade],
            ),
            Self::deb_invocation(
                DebMaintainerMode::Deconfigure,
                vec![
                    Self::deb_marker_arg(2, "in-favour", true),
                    Self::deb_package_arg(3, "package-being-installed", true),
                    Self::deb_version_arg(4, "package-being-installed-version", NativeArgumentValue::NewVersion, true),
                    Self::deb_marker_arg(5, "removing", false),
                    Self::deb_package_arg(6, "conflicting-package", false),
                    Self::deb_version_arg(7, "conflicting-version", NativeArgumentValue::OldVersion, false),
                ],
                vec![NativeLifecyclePath::Abort],
            ),
            Self::deb_invocation(
                DebMaintainerMode::FailedUpgrade,
                Self::deb_old_new_args(true),
                vec![NativeLifecyclePath::Abort],
            ),
        ],
        DebControlMember::Postrm => vec![
            Self::deb_invocation(
                DebMaintainerMode::Remove,
                Vec::new(),
                vec![NativeLifecyclePath::PostRemove],
            ),
            Self::deb_invocation(
                DebMaintainerMode::Purge,
                Vec::new(),
                vec![NativeLifecyclePath::Purge],
            ),
            Self::deb_invocation(
                DebMaintainerMode::Upgrade,
                vec![Self::deb_new_version_arg(2, true)],
                vec![NativeLifecyclePath::PostUpgrade],
            ),
            Self::deb_invocation(
                DebMaintainerMode::Disappear,
                vec![
                    Self::deb_arg(2, "overwriter-package", NativeArgumentValue::PackageName, true),
                    Self::deb_arg(3, "overwriter-version", NativeArgumentValue::NewVersion, true),
                ],
                vec![NativeLifecyclePath::PostRemove],
            ),
            Self::deb_invocation(
                DebMaintainerMode::FailedUpgrade,
                Self::deb_old_new_args(true),
                vec![NativeLifecyclePath::Abort],
            ),
            Self::deb_invocation(
                DebMaintainerMode::AbortInstall,
                Self::deb_old_new_args(false),
                vec![NativeLifecyclePath::Abort],
            ),
            Self::deb_invocation(
                DebMaintainerMode::AbortUpgrade,
                Self::deb_old_new_args(true),
                vec![NativeLifecyclePath::Abort],
            ),
        ],
        DebControlMember::Triggers => Vec::new(),
    }
}
```

- [ ] **Step 5: Implement DEB trigger artifact parsing**

Add:

```rust
fn native_abi_from_triggers_file(body: &[u8]) -> NativeScriptletEntry {
    let text = String::from_utf8(body.to_vec()).unwrap_or_default();
    let declarations = Self::parse_deb_trigger_declarations(&text);

    NativeScriptletEntry {
        id: "deb:triggers".to_string(),
        format: NativeScriptletFormat::Deb,
        kind: NativeScriptletKind::ControlArtifact,
        native_slot: "triggers".to_string(),
        primary_lifecycle: NativeLifecyclePath::Trigger,
        compatibility_phase: None,
        lifecycle_paths: vec![NativeLifecyclePath::Trigger],
        interpreter: None,
        interpreter_args: Vec::new(),
        body: NativeScriptletBody::from_bytes(body.to_vec()),
        invocation: NativeInvocationContract {
            args: Vec::new(),
            environment: Vec::new(),
            stdin: NativeStdinContract::None,
            root: NativeRootExpectation::PackageManagerDefault,
        },
        order: NativeTransactionOrder::new(NativeTransactionPosition::ControlArtifact),
        support: NativeScriptletSupport::DeferredReview {
            reason_code: "deb-trigger-semantics-deferred".to_string(),
        },
        metadata: NativeScriptletMetadata::Deb(DebNativeScriptletMetadata {
            control_member: DebControlMember::Triggers,
            maintainer_modes: Vec::new(),
            trigger_declarations: declarations,
        }),
    }
}

fn parse_deb_trigger_declarations(text: &str) -> Vec<DebTriggerDeclaration> {
    text.lines()
        .filter_map(|line| {
            let raw_line = line.to_string();
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                return None;
            }
            let mut parts = line.split_whitespace();
            let directive = parts.next()?;
            let trigger_name = parts.next()?.to_string();
            let (directive, await_mode) = match directive {
                "interest" => (DebTriggerDirective::Interest, DebTriggerAwaitMode::Default),
                "interest-await" => (DebTriggerDirective::Interest, DebTriggerAwaitMode::Await),
                "interest-noawait" => (DebTriggerDirective::Interest, DebTriggerAwaitMode::NoAwait),
                "activate" => (DebTriggerDirective::Activate, DebTriggerAwaitMode::Default),
                "activate-await" => (DebTriggerDirective::Activate, DebTriggerAwaitMode::Await),
                "activate-noawait" => (DebTriggerDirective::Activate, DebTriggerAwaitMode::NoAwait),
                _ => return None,
            };

            Some(DebTriggerDeclaration {
                directive,
                trigger_name,
                await_mode,
                raw_line,
            })
        })
        .collect()
}
```

- [ ] **Step 6: Populate DEB parser output**

In `parse_control_tar_all()`, read `config`, all four maintainer scripts, and
`triggers` as bytes:

```rust
"config" | "preinst" | "postinst" | "prerm" | "postrm" => {
    let mut body = Vec::new();
    entry
        .read_to_end(&mut body)
        .map_err(|e| Error::InitError(format!("Failed to read maintainer script: {}", e)))?;
    if let Some(native) = Self::native_abi_from_control_member(basename, &body) {
        contents.native_scriptlet_abi.push(native);
    }
    if let Some(flattened) = Self::flattened_scriptlet_from_control_member(basename, &body) {
        contents.scriptlets.push(flattened);
    }
}
"triggers" => {
    let mut body = Vec::new();
    entry
        .read_to_end(&mut body)
        .map_err(|e| Error::InitError(format!("Failed to read triggers file: {}", e)))?;
    if !body.iter().all(|byte| byte.is_ascii_whitespace()) {
        contents.native_scriptlet_abi.push(Self::native_abi_from_triggers_file(&body));
    }
}
```

In `parse()`, move `control_tar.native_scriptlet_abi` into the `PackageMetadata`
literal and add the trait accessor:

```rust
let native_scriptlet_abi = control_tar.native_scriptlet_abi;

PackageMetadata {
    // existing fields...
    scriptlets,
    native_scriptlet_abi,
    config_files,
}

fn native_scriptlet_abi(&self) -> &[NativeScriptletEntry] {
    self.meta.native_scriptlet_abi()
}
```

- [ ] **Step 7: Run DEB tests**

Run:

```bash
cargo test -p conary-core deb_shebang_split_preserves_flattened_interpreter_behavior
cargo test -p conary-core deb_native_abi_includes_config_as_native_only
cargo test -p conary-core deb_triggers_file_is_control_artifact_with_await_mode
cargo test -p conary-core deb_scriptlet
```

Expected: all commands pass.

- [ ] **Step 8: Commit Task 3**

```bash
git add crates/conary-core/src/packages/deb.rs
git commit -m "feat(packages): extract deb native scriptlet abi"
```

## Task 4: RPM Lifecycle Scriptlets And Trigger Families

**Files:**

- Modify: `crates/conary-core/src/packages/rpm.rs`

- [ ] **Step 1: Write failing RPM native ABI tests**

Add tests to `rpm.rs` that exercise the helper functions without needing to
write a full RPM first:

```rust
#[test]
fn rpm_program_vector_splits_interpreter_and_args() {
    let scriptlet = rpm::Scriptlet::new("echo posttrans")
        .prog(vec!["/usr/lib/rpm/lua", "--", "script.lua"]);

    let (interpreter, args) = RpmPackage::rpm_scriptlet_program(&scriptlet);

    assert_eq!(interpreter, "/usr/lib/rpm/lua");
    assert_eq!(args, vec!["--".to_string(), "script.lua".to_string()]);
}

#[test]
fn rpm_scriptlet_flags_preserve_names_and_bits() {
    let flags = RpmPackage::rpm_scriptlet_flags_metadata(rpm::ScriptletFlags::EXPAND);

    assert!(flags.names.contains(&"EXPAND".to_string()));
    assert_eq!(flags.raw_bits, rpm::ScriptletFlags::EXPAND.bits());
}

#[test]
fn rpm_trigger_action_and_comparison_are_split_from_raw_flags() {
    let flags = rpm::DependencyFlags::TRIGGERPOSTUN | rpm::DependencyFlags::LE;

    assert_eq!(RpmPackage::rpm_trigger_action(flags), Some(RpmTriggerAction::PostUninstall));
    assert_eq!(RpmPackage::rpm_trigger_comparison(flags), Some("<=".to_string()));
}
```

Add one parser-level fixture test after helper tests are green:

```rust
#[test]
fn rpm_native_abi_preserves_untransaction_verify_and_all_trigger_actions() {
    let mut builder = rpm::PackageBuilder::new(
        "native-abi-fixture",
        "1.0.0",
        "MIT",
        "x86_64",
        "native abi fixture",
    );
    builder
        .pre_install_script("echo pre")
        .post_install_script("echo post")
        .pre_uninstall_script("echo preun")
        .post_uninstall_script("echo postun")
        .pre_trans_script("echo pretrans")
        .post_trans_script("echo posttrans")
        .pre_untrans_script("echo preuntrans")
        .post_untrans_script("echo postuntrans")
        .verify_script("echo verify")
        .trigger_prein("bash", None, "echo triggerprein")
        .trigger_in("bash", Some((rpm::DependencyFlags::GREATER, "5.0")), "echo triggerin")
        .trigger_un("bash", None, "echo triggerun")
        .trigger_postun("bash", None, "echo triggerpostun")
        .file_trigger_in("/usr/lib", None, "echo filetriggerin")
        .file_trigger_un("/usr/lib", None, "echo filetriggerun")
        .file_trigger_postun("/usr/lib", None, "echo filetriggerpostun")
        .trans_file_trigger_in("/usr/bin", None, "echo transfiletriggerin")
        .trans_file_trigger_un("/usr/bin", None, "echo transfiletriggerun")
        .trans_file_trigger_postun("/usr/bin", None, "echo transfiletriggerpostun");

    let package = builder.build().expect("fixture rpm package");
    let entries = RpmPackage::extract_native_scriptlet_abi(&package);
    let slots: Vec<_> = entries.iter().map(|entry| entry.native_slot.as_str()).collect();

    for slot in [
        "%pre",
        "%post",
        "%preun",
        "%postun",
        "%pretrans",
        "%posttrans",
        "%preuntrans",
        "%postuntrans",
        "%verify",
        "%triggerprein",
        "%triggerin",
        "%triggerun",
        "%triggerpostun",
        "%filetriggerin",
        "%filetriggerun",
        "%filetriggerpostun",
        "%transfiletriggerin",
        "%transfiletriggerun",
        "%transfiletriggerpostun",
    ] {
        assert!(slots.contains(&slot), "missing native slot {slot}");
    }

    let verify = entries
        .iter()
        .find(|entry| entry.native_slot == "%verify")
        .expect("verify entry");
    assert_eq!(verify.primary_lifecycle, NativeLifecyclePath::Verify);
    assert_eq!(verify.compatibility_phase, None);
    assert_eq!(
        verify.support.reason_code(),
        Some("rpm-verify-scriptlet-deferred")
    );

    let file_trigger = entries
        .iter()
        .find(|entry| entry.native_slot == "%filetriggerin")
        .expect("file trigger entry");
    let NativeScriptletMetadata::Rpm(meta) = &file_trigger.metadata else {
        panic!("expected rpm metadata");
    };
    let trigger = meta.trigger.as_ref().expect("trigger metadata");
    assert_eq!(trigger.file_globs, vec!["/usr/lib".to_string()]);
}
```

- [ ] **Step 2: Run the failing RPM tests**

Run:

```bash
cargo test -p conary-core rpm_program_vector_splits_interpreter_and_args
cargo test -p conary-core rpm_scriptlet_flags_preserve_names_and_bits
cargo test -p conary-core rpm_trigger_action_and_comparison_are_split_from_raw_flags
cargo test -p conary-core rpm_native_abi_preserves_untransaction_verify_and_all_trigger_actions
```

Expected: helper tests fail to compile until Step 3; fixture test fails until
Step 5.

- [ ] **Step 3: Implement RPM helper functions**

Add native ABI imports to `rpm.rs`, then add:

```rust
fn rpm_scriptlet_program(scriptlet: &rpm::Scriptlet) -> (String, Vec<String>) {
    let mut program = scriptlet.program.clone().unwrap_or_default().into_iter();
    let interpreter = program.next().unwrap_or_else(|| "/bin/sh".to_string());
    let args = program.collect();
    (interpreter, args)
}

fn rpm_scriptlet_flags_metadata(
    flags: rpm::ScriptletFlags,
) -> RpmScriptletFlagsMetadata {
    let mut names = Vec::new();
    if flags.contains(rpm::ScriptletFlags::EXPAND) {
        names.push("EXPAND".to_string());
    }
    if flags.contains(rpm::ScriptletFlags::QFORMAT) {
        names.push("QFORMAT".to_string());
    }
    if flags.contains(rpm::ScriptletFlags::CRITICAL) {
        names.push("CRITICAL".to_string());
    }

    RpmScriptletFlagsMetadata {
        names,
        raw_bits: flags.bits(),
    }
}

fn rpm_trigger_action(flags: rpm::DependencyFlags) -> Option<RpmTriggerAction> {
    if flags.contains(rpm::DependencyFlags::TRIGGERPREIN) {
        Some(RpmTriggerAction::PreInstall)
    } else if flags.contains(rpm::DependencyFlags::TRIGGERIN) {
        Some(RpmTriggerAction::Install)
    } else if flags.contains(rpm::DependencyFlags::TRIGGERUN) {
        Some(RpmTriggerAction::Uninstall)
    } else if flags.contains(rpm::DependencyFlags::TRIGGERPOSTUN) {
        Some(RpmTriggerAction::PostUninstall)
    } else {
        None
    }
}

fn rpm_trigger_comparison(flags: rpm::DependencyFlags) -> Option<String> {
    let operator = flags_to_operator(flags).trim();
    if operator.is_empty() {
        None
    } else {
        Some(operator.to_string())
    }
}
```

- [ ] **Step 4: Implement lifecycle scriptlet extraction**

Add `extract_native_scriptlet_abi()` and `add_rpm_scriptlet_entry()`:

```rust
fn extract_native_scriptlet_abi(pkg: &Package) -> Vec<NativeScriptletEntry> {
    let mut entries = Vec::new();

    Self::add_rpm_scriptlet_entry(&mut entries, "%pre", RpmScriptletSlot::Pre, NativeLifecyclePath::PreInstall, Some(ScriptletPhase::PreInstall), pkg.metadata.get_pre_install_script());
    Self::add_rpm_scriptlet_entry(&mut entries, "%post", RpmScriptletSlot::Post, NativeLifecyclePath::PostInstall, Some(ScriptletPhase::PostInstall), pkg.metadata.get_post_install_script());
    Self::add_rpm_scriptlet_entry(&mut entries, "%preun", RpmScriptletSlot::PreUn, NativeLifecyclePath::PreRemove, Some(ScriptletPhase::PreRemove), pkg.metadata.get_pre_uninstall_script());
    Self::add_rpm_scriptlet_entry(&mut entries, "%postun", RpmScriptletSlot::PostUn, NativeLifecyclePath::PostRemove, Some(ScriptletPhase::PostRemove), pkg.metadata.get_post_uninstall_script());
    Self::add_rpm_scriptlet_entry(&mut entries, "%pretrans", RpmScriptletSlot::PreTrans, NativeLifecyclePath::PreTransaction, Some(ScriptletPhase::PreTransaction), pkg.metadata.get_pre_trans_script());
    Self::add_rpm_scriptlet_entry(&mut entries, "%posttrans", RpmScriptletSlot::PostTrans, NativeLifecyclePath::PostTransaction, Some(ScriptletPhase::PostTransaction), pkg.metadata.get_post_trans_script());
    Self::add_rpm_scriptlet_entry(&mut entries, "%preuntrans", RpmScriptletSlot::PreUnTrans, NativeLifecyclePath::PreUntransaction, None, pkg.metadata.get_pre_untrans_script());
    Self::add_rpm_scriptlet_entry(&mut entries, "%postuntrans", RpmScriptletSlot::PostUnTrans, NativeLifecyclePath::PostUntransaction, None, pkg.metadata.get_post_untrans_script());
    Self::add_rpm_scriptlet_entry(&mut entries, "%verify", RpmScriptletSlot::Verify, NativeLifecyclePath::Verify, None, pkg.metadata.get_verify_script());

    Self::add_rpm_triggers(&mut entries, RpmTriggerFamily::Package, pkg.metadata.get_triggers());
    Self::add_rpm_triggers(&mut entries, RpmTriggerFamily::File, pkg.metadata.get_file_triggers());
    Self::add_rpm_triggers(&mut entries, RpmTriggerFamily::TransactionFile, pkg.metadata.get_trans_file_triggers());

    entries
}
```

Add the helper functions used by `extract_native_scriptlet_abi()`. The entry
helper skips empty basic script bodies, marks `%verify` as `DeferredReview`,
preserves `ScriptletFlags`, and records RPM's documented `$1` package instance
count contract.

```rust
fn rpm_scriptlet_invocation() -> NativeInvocationContract {
    NativeInvocationContract {
        args: vec![NativeArgumentContract {
            index: 1,
            name: "package-instance-count".to_string(),
            value: NativeArgumentValue::PackageInstanceCount,
            required: true,
        }],
        environment: Vec::new(),
        stdin: NativeStdinContract::None,
        root: NativeRootExpectation::PackageManagerDefault,
    }
}

fn rpm_order_for_lifecycle(lifecycle: NativeLifecyclePath) -> NativeTransactionOrder {
    NativeTransactionOrder::new(match lifecycle {
        NativeLifecyclePath::PreInstall
        | NativeLifecyclePath::PreUpgrade
        | NativeLifecyclePath::PreRemove => NativeTransactionPosition::BeforePayload,
        NativeLifecyclePath::PostInstall
        | NativeLifecyclePath::PostUpgrade
        | NativeLifecyclePath::PostRemove => NativeTransactionPosition::AfterPayload,
        NativeLifecyclePath::PreTransaction => NativeTransactionPosition::BeforeTransaction,
        NativeLifecyclePath::PostTransaction => NativeTransactionPosition::AfterTransaction,
        NativeLifecyclePath::PreUntransaction | NativeLifecyclePath::PostUntransaction => {
            NativeTransactionPosition::Untransaction
        }
        NativeLifecyclePath::Verify => NativeTransactionPosition::Verification,
        _ => NativeTransactionPosition::ControlArtifact,
    })
}

fn add_rpm_scriptlet_entry(
    entries: &mut Vec<NativeScriptletEntry>,
    native_slot: &str,
    slot: RpmScriptletSlot,
    lifecycle: NativeLifecyclePath,
    compatibility_phase: Option<ScriptletPhase>,
    scriptlet: std::result::Result<rpm::Scriptlet, rpm::Error>,
) {
    let Ok(scriptlet) = scriptlet else {
        return;
    };
    if scriptlet.script.trim().is_empty() {
        return;
    }

    let (interpreter, interpreter_args) = Self::rpm_scriptlet_program(&scriptlet);
    let support = if slot == RpmScriptletSlot::Verify {
        NativeScriptletSupport::DeferredReview {
            reason_code: "rpm-verify-scriptlet-deferred".to_string(),
        }
    } else {
        NativeScriptletSupport::Parsed
    };

    entries.push(NativeScriptletEntry {
        id: format!("rpm:{native_slot}"),
        format: NativeScriptletFormat::Rpm,
        kind: NativeScriptletKind::Executable,
        native_slot: native_slot.to_string(),
        primary_lifecycle: lifecycle,
        compatibility_phase,
        lifecycle_paths: vec![lifecycle],
        interpreter: Some(interpreter),
        interpreter_args,
        body: NativeScriptletBody::from_bytes(scriptlet.script.as_bytes().to_vec()),
        invocation: Self::rpm_scriptlet_invocation(),
        order: Self::rpm_order_for_lifecycle(lifecycle),
        support,
        metadata: NativeScriptletMetadata::Rpm(RpmNativeScriptletMetadata {
            slot,
            scriptlet_flags: scriptlet.flags.map(Self::rpm_scriptlet_flags_metadata),
            trigger: None,
        }),
    });
}
```

Use `NativeScriptletBody::from_bytes(scriptlet.script.as_bytes().to_vec())` for
RPM bodies because the `rpm` crate exposes scriptlets as strings.

- [ ] **Step 5: Implement RPM trigger extraction**

Add:

```rust
fn add_rpm_triggers(
    entries: &mut Vec<NativeScriptletEntry>,
    family: RpmTriggerFamily,
    triggers: std::result::Result<Vec<rpm::Trigger>, rpm::Error>,
) {
    let Ok(triggers) = triggers else {
        return;
    };

    for (trigger_index, trigger) in triggers.into_iter().enumerate() {
        let native_slot = Self::rpm_trigger_slot_name(family, &trigger);
        let (interpreter, interpreter_args) = {
            let mut program = trigger.program.clone().into_iter();
            (
                program.next().unwrap_or_else(|| "/bin/sh".to_string()),
                program.collect(),
            )
        };
        let lifecycle = match family {
            RpmTriggerFamily::Package => NativeLifecyclePath::Trigger,
            RpmTriggerFamily::File => NativeLifecyclePath::FileTrigger,
            RpmTriggerFamily::TransactionFile => NativeLifecyclePath::TransactionFileTrigger,
        };
        let file_globs = if family == RpmTriggerFamily::File
            || family == RpmTriggerFamily::TransactionFile
        {
            trigger
                .conditions
                .iter()
                .map(|condition| condition.name.clone())
                .collect()
        } else {
            Vec::new()
        };
        let conditions = trigger
            .conditions
            .iter()
            .filter_map(|condition| {
                Some(RpmTriggerCondition {
                    name: condition.name.clone(),
                    action: Self::rpm_trigger_action(condition.flags)?,
                    version: if condition.version.is_empty() {
                        None
                    } else {
                        Some(condition.version.clone())
                    },
                    comparison: Self::rpm_trigger_comparison(condition.flags),
                    raw_flags: condition.flags.bits(),
                })
            })
            .collect();

        entries.push(NativeScriptletEntry {
            id: format!("rpm:{native_slot}:{trigger_index}"),
            format: NativeScriptletFormat::Rpm,
            kind: NativeScriptletKind::Executable,
            native_slot,
            primary_lifecycle: lifecycle,
            compatibility_phase: None,
            lifecycle_paths: vec![lifecycle],
            interpreter: Some(interpreter),
            interpreter_args,
            body: NativeScriptletBody::from_bytes(trigger.script.as_bytes().to_vec()),
            invocation: NativeInvocationContract {
                args: vec![
                    NativeArgumentContract {
                        index: 1,
                        name: "triggered-package-count".to_string(),
                        value: NativeArgumentValue::TriggerCount,
                        required: true,
                    },
                    NativeArgumentContract {
                        index: 2,
                        name: "triggering-package-count".to_string(),
                        value: NativeArgumentValue::TriggerCount,
                        required: family != RpmTriggerFamily::TransactionFile,
                    },
                ],
                environment: Vec::new(),
                stdin: if family == RpmTriggerFamily::File
                    || family == RpmTriggerFamily::TransactionFile
                {
                    NativeStdinContract::Paths
                } else {
                    NativeStdinContract::None
                },
                root: NativeRootExpectation::PackageManagerDefault,
            },
            order: NativeTransactionOrder::new(NativeTransactionPosition::Trigger),
            support: NativeScriptletSupport::DeferredReview {
                reason_code: match family {
                    RpmTriggerFamily::Package => "rpm-trigger-semantics-deferred",
                    RpmTriggerFamily::File => "rpm-file-trigger-semantics-deferred",
                    RpmTriggerFamily::TransactionFile => {
                        "rpm-trans-file-trigger-semantics-deferred"
                    }
                }
                .to_string(),
            },
            metadata: NativeScriptletMetadata::Rpm(RpmNativeScriptletMetadata {
                slot: RpmScriptletSlot::Trigger,
                scriptlet_flags: None,
                trigger: Some(RpmTriggerMetadata {
                    family,
                    conditions,
                    file_globs,
                }),
            }),
        });
    }
}
```

Implement `rpm_trigger_slot_name()` so tests see documented slot names:

```rust
fn rpm_trigger_slot_name(family: RpmTriggerFamily, trigger: &rpm::Trigger) -> String {
    let action = trigger
        .conditions
        .first()
        .and_then(|condition| Self::rpm_trigger_action(condition.flags));
    match (family, action) {
        (RpmTriggerFamily::Package, Some(RpmTriggerAction::PreInstall)) => "%triggerprein",
        (RpmTriggerFamily::Package, Some(RpmTriggerAction::Install)) => "%triggerin",
        (RpmTriggerFamily::Package, Some(RpmTriggerAction::Uninstall)) => "%triggerun",
        (RpmTriggerFamily::Package, Some(RpmTriggerAction::PostUninstall)) => "%triggerpostun",
        (RpmTriggerFamily::File, Some(RpmTriggerAction::Install)) => "%filetriggerin",
        (RpmTriggerFamily::File, Some(RpmTriggerAction::Uninstall)) => "%filetriggerun",
        (RpmTriggerFamily::File, Some(RpmTriggerAction::PostUninstall)) => "%filetriggerpostun",
        (RpmTriggerFamily::TransactionFile, Some(RpmTriggerAction::Install)) => "%transfiletriggerin",
        (RpmTriggerFamily::TransactionFile, Some(RpmTriggerAction::Uninstall)) => "%transfiletriggerun",
        (RpmTriggerFamily::TransactionFile, Some(RpmTriggerAction::PostUninstall)) => "%transfiletriggerpostun",
        _ => "%trigger",
    }
    .to_string()
}
```

- [ ] **Step 6: Populate RPM package metadata**

In `parse()`, after `let scriptlets = Self::extract_scriptlets(&pkg);`, add:

```rust
let native_scriptlet_abi = Self::extract_native_scriptlet_abi(&pkg);
```

Set the metadata field:

```rust
native_scriptlet_abi,
```

Add the trait accessor:

```rust
fn native_scriptlet_abi(&self) -> &[NativeScriptletEntry] {
    self.meta.native_scriptlet_abi()
}
```

- [ ] **Step 7: Run RPM tests**

Run:

```bash
cargo test -p conary-core rpm_program_vector_splits_interpreter_and_args
cargo test -p conary-core rpm_scriptlet_flags_preserve_names_and_bits
cargo test -p conary-core rpm_trigger_action_and_comparison_are_split_from_raw_flags
cargo test -p conary-core rpm_native_abi_preserves_untransaction_verify_and_all_trigger_actions
cargo test -p conary-core rpm_scriptlet
```

Expected: all commands pass.

- [ ] **Step 8: Commit Task 4**

```bash
git add crates/conary-core/src/packages/rpm.rs
git commit -m "feat(packages): extract rpm native scriptlet abi"
```

## Task 5: Cross-Format Parser Contract Tests

**Files:**

- Create: `crates/conary-core/tests/native_abi.rs`

- [ ] **Step 1: Write integration tests for the public API**

Create `crates/conary-core/tests/native_abi.rs`:

```rust
// conary-core/tests/native_abi.rs

use conary_core::packages::traits::PackageFormat;
use conary_core::packages::{ArchPackage, DebPackage, RpmPackage};

#[test]
fn package_format_trait_exposes_native_abi_default_empty_for_test_double() {
    struct EmptyPackage;

    impl PackageFormat for EmptyPackage {
        fn parse(_path: &str) -> conary_core::error::Result<Self> {
            Ok(Self)
        }
        fn name(&self) -> &str {
            "empty"
        }
        fn version(&self) -> &str {
            "0"
        }
        fn architecture(&self) -> Option<&str> {
            None
        }
        fn description(&self) -> Option<&str> {
            None
        }
        fn files(&self) -> &[conary_core::packages::traits::PackageFile] {
            &[]
        }
        fn dependencies(&self) -> &[conary_core::packages::traits::Dependency] {
            &[]
        }
        fn extract_file_contents(
            &self,
        ) -> conary_core::error::Result<Vec<conary_core::packages::traits::ExtractedFile>> {
            Ok(Vec::new())
        }
        fn to_trove(&self) -> conary_core::db::models::Trove {
            conary_core::db::models::Trove::new(
                "empty".to_string(),
                "0".to_string(),
                conary_core::db::models::TroveType::Package,
            )
        }
    }

    let package = EmptyPackage;
    assert!(package.native_scriptlet_abi().is_empty());
}

#[test]
fn parser_types_expose_native_abi_method() {
    fn assert_native_abi_method<P: PackageFormat>() {}

    assert_native_abi_method::<RpmPackage>();
    assert_native_abi_method::<DebPackage>();
    assert_native_abi_method::<ArchPackage>();
}
```

- [ ] **Step 2: Run the integration test**

Run:

```bash
cargo test -p conary-core --test native_abi
```

Expected: pass.

- [ ] **Step 3: Run the focused Goal 2 parser tests**

Run:

```bash
cargo test -p conary-core native_abi
cargo test -p conary-core rpm_scriptlet
cargo test -p conary-core deb_scriptlet
cargo test -p conary-core arch_scriptlet
```

Expected: all commands pass.

- [ ] **Step 4: Commit Task 5**

```bash
git add crates/conary-core/tests/native_abi.rs
git commit -m "test(packages): cover native abi parser contract"
```

## Task 6: Final Verification And Documentation Check

**Files:**

- Modify: no docs are required unless the grep in Step 2 finds active parser API
  documentation that names `PackageFormat` or `scriptlets()`.

- [ ] **Step 1: Run the final required test suite**

Run each command separately:

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

Expected: every command exits with status 0.

- [ ] **Step 2: Check whether module docs mention parser API**

Run:

```bash
rg -n "PackageFormat|scriptlets\\(\\)|native_scriptlet_abi|package parser" README.md docs AGENTS.md
```

Expected for the current repo: only planning/spec docs mention
`native_scriptlet_abi`. If `docs/modules/*.md` or `README.md` describes package
parser output, update that doc to say native ABI metadata is parser-only and is
not consumed by install/update/remove in Goal 2.

- [ ] **Step 3: Prove no install behavior changed in the diff**

Run:

```bash
git diff --stat origin/main..HEAD
git diff origin/main..HEAD -- apps/conary/src/commands/install apps/conary/src/commands/remove apps/conary/src/commands/update
```

Expected: no behavioral install/remove/update diff beyond struct-literal
compile fixes for `PackageMetadata`. If code under these paths changed for any
other reason, move that change out of Goal 2 before final review.

- [ ] **Step 4: Final commit for verification-only doc or compile fixes**

If Step 2 or Step 3 required a small doc or compile fix, commit it:

```bash
git add README.md docs apps crates
git commit -m "docs(packages): document native abi parser boundary"
```

If no files changed after Step 1, do not create an empty commit.

## Review Checklist Before Merge

- [ ] `PackageFormat::scriptlets()` behavior remains compatible for RPM, DEB,
  and Arch.
- [ ] `PackageFormat::native_scriptlet_abi()` is additive and defaults to an
  empty slice.
- [ ] `PackageMetadata::new()` initializes an empty native ABI vector.
- [ ] RPM native ABI includes `%pre`, `%post`, `%preun`, `%postun`,
  `%pretrans`, `%posttrans`, `%preuntrans`, `%postuntrans`, `%verify`,
  package triggers, file triggers, and transaction file triggers.
- [ ] DEB native ABI includes `config`, `preinst`, `postinst`, `prerm`,
  `postrm`, and `triggers`.
- [ ] Arch native ABI includes `.INSTALL` functions and packaged ALPM
  `/usr/share/libalpm/hooks/*.hook` artifacts.
- [ ] Native bodies hash raw bytes with `sha256:` prefixes.
- [ ] Native-only entries do not get inaccurate `ScriptletPhase` projections.
- [ ] No Remi database migration, CCS conversion, bundle embedding, adapter
  classification, scriptlet execution, replay, install, update, or remove
  behavior changed.

## Final Handoff

When all tasks and verification pass, request a code review before merging.
Include:

- branch name and commit range;
- the exact verification commands and outcomes;
- any parser API limitations discovered in the local `rpm` crate;
- whether ALPM hook parsing preserved every packaged hook artifact.
