// conary-core/src/packages/native_abi.rs
//! Native package-manager scriptlet ABI metadata captured by package parsers.

use crate::hash;
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
}

impl NativeScriptletSupport {
    pub fn reason_code(&self) -> Option<&str> {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeScriptletMetadata {
    Rpm(RpmNativeScriptletMetadata),
    Deb(DebNativeScriptletMetadata),
    /// Package-level debconf template authority from `control.tar/templates`.
    ///
    /// This is deliberately separate from [`DebNativeScriptletMetadata`]:
    /// templates belong to the package control archive and may be consumed by
    /// any maintainer script, not only the optional `config` script.
    DebconfTemplates(DebconfTemplatesMetadata),
    Arch(ArchNativeScriptletMetadata),
}

pub const DEBCONF_TEMPLATES_ENTRY_ID: &str = "deb:templates";
pub const DEBCONF_TEMPLATES_NATIVE_SLOT: &str = "templates";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeLifecyclePath {
    PackageControl,
    PreInstall,
    Sysusers,
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
    InstallingPackageName,
    InstallingPackageVersion,
    ConflictingPackageMarker,
    ConflictingPackageName,
    ConflictingPackageVersion,
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
    Sysusers,
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
    pub runtime: RpmScriptletRuntimeMetadata,
    pub trigger: Option<RpmTriggerMetadata>,
    pub sysusers: Option<RpmSysusersMetadata>,
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
    Sysusers,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpmScriptletFlagsMetadata {
    pub names: Vec<String>,
    pub raw_bits: u32,
    pub unknown_bits: u32,
    pub expand: bool,
    pub query_format: bool,
    pub critical: bool,
    pub criticality: RpmScriptletCriticality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpmScriptletCriticality {
    Header,
    SlotDefault,
    WarningOnly,
    ForcedWarningOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpmScriptletRuntimeMetadata {
    pub program: RpmScriptletProgram,
    pub flags: RpmScriptletFlagsMetadata,
    pub install_prefixes: Vec<String>,
    /// Exact macro definitions available to install-time RPM expansion.
    ///
    /// These are package/header facts captured by the parser. Target distro
    /// configuration and a host RPM installation are never consulted.
    pub macro_context: RpmMacroContextMetadata,
    /// Typed RPM header values used by the `-q`/QFORMAT body transform.
    pub header_context: RpmHeaderContextMetadata,
    /// RPM version that produced the package, when declared by the header.
    pub package_rpm_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpmScriptletProgram {
    External,
    EmbeddedLua,
    Sysusers,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RpmMacroContextMetadata {
    pub definitions: Vec<RpmMacroDefinitionMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpmMacroDefinitionMetadata {
    pub name: String,
    pub body: String,
    pub source: RpmMacroDefinitionSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpmMacroDefinitionSource {
    PackageHeader,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RpmHeaderContextMetadata {
    pub facts: Vec<RpmHeaderFactMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpmHeaderFactMetadata {
    pub tag: u32,
    pub name: Option<String>,
    pub value: RpmHeaderValueMetadata,
    pub source: RpmHeaderFactSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpmHeaderFactSource {
    Header,
    TransactionDerived,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpmHeaderValueMetadata {
    Null,
    Binary(Vec<u8>),
    Integer(Vec<u64>),
    String(String),
    StringArray(Vec<String>),
    I18nString(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpmSysusersMetadata {
    /// Packaged sysusers.d path replaced by the decoded declarations. `None`
    /// identifies a package-level `%add_sysuser` declaration.
    pub source_path: Option<String>,
    pub lines: Vec<String>,
    pub directives: Vec<RpmSysusersDirective>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpmSysusersDirective {
    User {
        name: String,
        id: Option<String>,
        description: Option<String>,
        home: Option<String>,
        shell: Option<String>,
        locked: bool,
    },
    Group {
        name: String,
        id: Option<String>,
    },
    Member {
        user: String,
        group: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpmTriggerMetadata {
    pub family: RpmTriggerFamily,
    pub conditions: Vec<RpmTriggerCondition>,
    pub path_prefixes: Vec<String>,
    pub priority: Option<i32>,
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

/// Parsed package-level authority from a Debian `templates` control member.
///
/// `raw_sha256` identifies the byte-preserving
/// [`NativeScriptletEntry::body`]. Records retain source order, field spelling,
/// localized field suffixes, and continuation lines. The raw body remains the
/// canonical serialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebconfTemplatesMetadata {
    pub raw_sha256: String,
    pub records: Vec<DebconfTemplateRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebconfTemplateRecord {
    /// Semantic value of the required `Template` field.
    pub template: String,
    /// Semantic value of the required `Type` field. Extension types are
    /// preserved instead of being filtered through a local allowlist.
    pub template_type: String,
    /// Fields in source order, including unknown and localized variants.
    pub fields: Vec<DebconfTemplateField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebconfTemplateField {
    /// Original field-name spelling.
    pub name: String,
    /// Original first-line value after the optional RFC822 separator byte.
    pub value: String,
    /// Original continuation lines, including their leading whitespace.
    pub continuation_lines: Vec<String>,
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
    pub selection_contract: ArchInstallSelectionContract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchInstallSelectionContract {
    /// Current libalpm `_alpm_runscriptlet` predicate: 1023-byte `fgets`
    /// chunks, C-string truncation, `#` comment truncation, literal substring.
    LibalpmGrepV1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchAlpmHookMetadata {
    pub hook_path: String,
    pub triggers: Vec<ArchAlpmHookTrigger>,
    pub action: ArchAlpmHookAction,
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
    pub argv: Vec<String>,
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
    fn native_support_is_fully_parsed() {
        assert!(NativeScriptletSupport::Parsed.reason_code().is_none());
    }
}
