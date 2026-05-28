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
    TriggerNames,
    TriggerCount,
    FilePath,
    InstalledVersion,
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
    Unknown { raw_flags: u32 },
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
