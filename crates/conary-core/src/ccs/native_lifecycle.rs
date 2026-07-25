// conary-core/src/ccs/native_lifecycle.rs
//! Preserved native package-manager lifecycle metadata for CCS packages.

use anyhow::{anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

mod debconf;
mod rpm;
mod types;
mod validation;
pub use debconf::*;
pub use rpm::*;
pub use types::*;

pub const NATIVE_LIFECYCLE_SCHEMA_V1: &str = "conary.native-lifecycles.v1";
pub const NATIVE_LIFECYCLE_SCHEMA_REVISION: u16 = 17;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeLifecycleBundle {
    pub schema: String,
    pub schema_revision: u16,
    pub source_format: SourceFormat,
    pub source_family: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_distro: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_release: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_arch: Option<String>,
    pub source_package: String,
    pub source_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_checksum: Option<String>,
    pub version_scheme: VersionScheme,
    pub conversion_tool: String,
    pub conversion_tool_version: String,
    pub conversion_policy: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_digest: Option<String>,
    pub scriptlet_fidelity: ScriptletFidelity,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<NativeLifecycleEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CommandArgumentProvenance {
    Literal,
    Expansion,
    CommandSubstitution,
    ProcessSubstitution,
    Glob,
    Mixed,
    #[default]
    Unresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativeLifecycleEntryKind {
    Executable,
    ControlArtifact,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeLifecycleEntry {
    pub id: String,
    pub native_slot: String,
    pub kind: NativeLifecycleEntryKind,
    pub phase: LifecyclePath,
    pub lifecycle_paths: Vec<String>,
    pub interpreter: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interpreter_args: Vec<String>,
    pub body_sha256: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_encoding: Option<String>,
    pub native_invocation: NativeInvocation,
    pub transaction_order: TransactionOrder,
    pub timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<ScriptletSandboxRequirements>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_evidence_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm_trigger: Option<RpmTriggerMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm_runtime: Option<RpmRuntimeMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm_sysusers: Option<RpmSysusersMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deb_maintainer: Option<DebMaintainerMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch_install: Option<ArchInstallMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch_hook: Option<ArchHookMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub residual_lifecycle: Option<ResidualLifecycleMetadata>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct NativeInvocation {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chroot: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TransactionOrder {
    pub position: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ScriptletSandboxRequirements {
    #[serde(default)]
    pub network: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub namespaces: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seccomp_profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RpmTriggerMetadata {
    pub kind: RpmTriggerKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_constraints: Vec<RpmTriggerTargetConstraint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path_prefixes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RpmTriggerKind {
    Package,
    #[default]
    File,
    TransactionFile,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RpmTriggerTargetConstraint {
    pub package: String,
    pub action: RpmTriggerAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub raw_flags: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RpmTriggerAction {
    PreInstall,
    #[default]
    Install,
    Uninstall,
    PostUninstall,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct DebMaintainerMetadata {
    pub control_member: DebControlMember,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invocations: Vec<DebMaintainerInvocationMetadata>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trigger_declarations: Vec<DebTriggerMetadata>,
    #[serde(default)]
    pub noninteractive: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebMaintainerInvocationMetadata {
    pub mode: DebMaintainerMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<DebMaintainerArgumentMetadata>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lifecycle_paths: Vec<LifecyclePath>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebMaintainerArgumentMetadata {
    pub index: usize,
    pub name: String,
    pub value: DebMaintainerArgumentValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub literal: Option<String>,
    pub required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DebControlMember {
    Config,
    Preinst,
    #[default]
    Postinst,
    Prerm,
    Postrm,
    Triggers,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct DebTriggerMetadata {
    pub directive: DebTriggerDirective,
    pub trigger_name: String,
    pub await_mode: DebTriggerAwaitMode,
    pub raw_line: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DebTriggerDirective {
    #[default]
    Interest,
    Activate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DebTriggerAwaitMode {
    #[default]
    Default,
    Await,
    NoAwait,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArchInstallSelection {
    LibalpmGrepV1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchInstallMetadata {
    pub install_digest: String,
    pub called_function: String,
    pub selection: ArchInstallSelection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchHookMetadata {
    pub hook_path: String,
    pub triggers: Vec<ArchHookTriggerMetadata>,
    pub action: ArchHookActionMetadata,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchHookTriggerMetadata {
    pub operations: Vec<ArchHookOperation>,
    pub trigger_type: ArchHookTriggerType,
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchHookActionMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub when: ArchHookWhen,
    pub exec: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends: Vec<String>,
    #[serde(default)]
    pub abort_on_fail: bool,
    #[serde(default)]
    pub needs_targets: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArchHookOperation {
    Install,
    Upgrade,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ArchHookTriggerType {
    #[default]
    Package,
    Path,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ArchHookWhen {
    PreTransaction,
    #[default]
    PostTransaction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ResidualLifecycleMetadata {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub superseded_effect_kinds: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrapper_strategy: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suppression_markers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub residual_body_digest: Option<String>,
}

impl NativeLifecycleBundle {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.schema != NATIVE_LIFECYCLE_SCHEMA_V1 {
            bail!(
                "native lifecycle bundle schema must be {NATIVE_LIFECYCLE_SCHEMA_V1}, got {}",
                self.schema
            );
        }
        if self.schema_revision != NATIVE_LIFECYCLE_SCHEMA_REVISION {
            bail!(
                "native lifecycle bundle schema_revision must be {NATIVE_LIFECYCLE_SCHEMA_REVISION}, got {}",
                self.schema_revision
            );
        }
        let expected_version_scheme = match &self.source_format {
            SourceFormat::Rpm => VersionScheme::Rpm,
            SourceFormat::Deb => VersionScheme::Deb,
            SourceFormat::Arch => VersionScheme::Arch,
        };
        if self.version_scheme != expected_version_scheme {
            bail!(
                "native lifecycle bundle source_format '{}' requires version_scheme '{}', got '{}'",
                self.source_format.as_str(),
                expected_version_scheme.as_str(),
                self.version_scheme.as_str()
            );
        }

        required_string("source_family", &self.source_family)?;
        required_string("source_package", &self.source_package)?;
        required_string("source_version", &self.source_version)?;
        required_string("conversion_tool", &self.conversion_tool)?;
        required_string("conversion_tool_version", &self.conversion_tool_version)?;
        required_string("conversion_policy", &self.conversion_policy)?;

        validate_optional_sha256("source_checksum", self.source_checksum.as_deref())?;
        validate_optional_sha256("evidence_digest", self.evidence_digest.as_deref())?;

        let mut seen_ids = BTreeSet::new();
        for entry in &self.entries {
            if !seen_ids.insert(entry.id.as_str()) {
                bail!("duplicate entry id '{}'", entry.id);
            }
            entry.validate_source_format(&self.source_format)?;
            if (entry.rpm_trigger.is_some() || entry.rpm_sysusers.is_some())
                && entry.rpm_runtime.is_none()
            {
                bail!(
                    "RPM specialized lifecycle entry '{}' has no typed runtime contract",
                    entry.id
                );
            }
            entry.validate()?;
        }
        self.debconf_templates()?;

        Ok(())
    }
}

impl NativeLifecycleEntry {
    fn validate(&self) -> anyhow::Result<()> {
        required_string("entry.id", &self.id)?;
        required_string("entry.native_slot", &self.native_slot)?;
        required_string("entry.interpreter", &self.interpreter)?;
        required_string("entry.body", &self.body)?;
        required_string(
            "entry.transaction_order.position",
            &self.transaction_order.position,
        )?;
        match self.kind {
            NativeLifecycleEntryKind::Executable
                if self.interpreter == "package-manager-control-artifact" =>
            {
                bail!(
                    "entry '{}' declares executable kind with the control-artifact interpreter",
                    self.id
                )
            }
            NativeLifecycleEntryKind::ControlArtifact
                if self.interpreter != "package-manager-control-artifact" =>
            {
                bail!(
                    "entry '{}' declares control-artifact kind with executable interpreter '{}'",
                    self.id,
                    self.interpreter
                )
            }
            _ => {}
        }

        if self.lifecycle_paths.is_empty() {
            bail!("entry '{}' lifecycle_paths must not be empty", self.id);
        }
        if self.timeout_ms == 0 {
            bail!("entry '{}' timeout_ms must be greater than zero", self.id);
        }

        validate_sha256("entry.body_sha256", &self.body_sha256)?;
        validate_optional_sha256("entry.evidence_digest", self.evidence_digest.as_deref())?;

        let body_bytes = self.body_bytes()?;

        if let Some(metadata) = &self.arch_install {
            metadata.validate(&self.id, &self.body_sha256)?;
        }
        if let Some(metadata) = &self.arch_hook {
            metadata.validate(&self.id)?;
        }
        if let Some(metadata) = &self.deb_maintainer {
            metadata.validate(&self.id, &body_bytes)?;
        }
        if let Some(metadata) = &self.rpm_runtime {
            metadata.validate(&self.id)?;
            match metadata.program {
                RpmProgram::EmbeddedLua
                    if self.interpreter != "<lua>" || !self.interpreter_args.is_empty() =>
                {
                    bail!(
                        "entry '{}' embedded RPM Lua program requires the exact '<lua>' interpreter with no interpreter arguments",
                        self.id
                    )
                }
                RpmProgram::External if self.interpreter == "<lua>" => bail!(
                    "entry '{}' declares '<lua>' with the external RPM program",
                    self.id
                ),
                _ => {}
            }
        }
        if let Some(metadata) = &self.rpm_trigger {
            metadata.validate(&self.id)?;
        }
        if let Some(metadata) = &self.rpm_sysusers {
            metadata.validate(&self.id)?;
        }
        match (
            self.rpm_runtime.as_ref().map(|metadata| metadata.program),
            self.rpm_sysusers.is_some(),
        ) {
            (Some(RpmProgram::Sysusers), true) => {}
            (Some(RpmProgram::Sysusers), false) => bail!(
                "entry '{}' declares the RPM sysusers program without sysusers metadata",
                self.id
            ),
            (_, true) => bail!(
                "entry '{}' carries RPM sysusers metadata without the sysusers program",
                self.id
            ),
            (_, false) => {}
        }
        if let Some(metadata) = &self.residual_lifecycle {
            metadata.validate(&self.id)?;
        }

        Ok(())
    }

    pub fn body_bytes(&self) -> anyhow::Result<Vec<u8>> {
        let body_bytes = match self.body_encoding.as_deref().unwrap_or("utf-8") {
            "utf-8" => Ok(self.body.as_bytes().to_vec()),
            "base64" => {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD
                    .decode(&self.body)
                    .map_err(|error| {
                        anyhow!("entry '{}' body base64 decode failed: {error}", self.id)
                    })
            }
            other => bail!(
                "entry '{}' body_encoding '{}' is unsupported",
                self.id,
                other
            ),
        }?;

        let actual = crate::hash::sha256_prefixed(&body_bytes);
        if !actual.eq_ignore_ascii_case(&self.body_sha256) {
            bail!(
                "entry '{}' body_sha256 mismatch: expected {}, got {}",
                self.id,
                self.body_sha256,
                actual
            );
        }

        Ok(body_bytes)
    }
}

impl RpmTriggerMetadata {
    fn validate(&self, entry_id: &str) -> anyhow::Result<()> {
        if matches!(
            self.kind,
            RpmTriggerKind::File | RpmTriggerKind::TransactionFile
        ) && self.path_prefixes.is_empty()
        {
            bail!("entry '{entry_id}' RPM file trigger has no path prefixes");
        }
        for prefix in &self.path_prefixes {
            if !prefix.starts_with('/') {
                bail!("entry '{entry_id}' RPM file trigger prefix '{prefix}' must begin with '/'");
            }
            if prefix.contains('\0') {
                bail!("entry '{entry_id}' RPM file trigger prefix contains NUL");
            }
        }
        Ok(())
    }
}

impl DebMaintainerMetadata {
    fn validate(&self, entry_id: &str, body_bytes: &[u8]) -> anyhow::Result<()> {
        if self.control_member == DebControlMember::Triggers {
            if !self.invocations.is_empty() {
                bail!(
                    "entry '{entry_id}' Debian triggers control artifact must not declare executable invocations"
                );
            }
            let parsed = crate::packages::deb::parse_trigger_declarations(body_bytes)
                .map_err(|error| anyhow!("entry '{entry_id}' {error}"))?;
            if parsed.len() != self.trigger_declarations.len() {
                bail!(
                    "entry '{entry_id}' Debian trigger declaration count does not match its preserved control artifact"
                );
            }
            for (index, (declaration, parsed)) in
                self.trigger_declarations.iter().zip(&parsed).enumerate()
            {
                required_string("Debian trigger declaration raw line", &declaration.raw_line)?;
                let directive_matches = matches!(
                    (declaration.directive, parsed.directive),
                    (
                        DebTriggerDirective::Interest,
                        crate::packages::native_abi::DebTriggerDirective::Interest
                    ) | (
                        DebTriggerDirective::Activate,
                        crate::packages::native_abi::DebTriggerDirective::Activate
                    )
                );
                let await_mode_matches = matches!(
                    (declaration.await_mode, parsed.await_mode),
                    (
                        DebTriggerAwaitMode::Default,
                        crate::packages::native_abi::DebTriggerAwaitMode::Default
                    ) | (
                        DebTriggerAwaitMode::Await,
                        crate::packages::native_abi::DebTriggerAwaitMode::Await
                    ) | (
                        DebTriggerAwaitMode::NoAwait,
                        crate::packages::native_abi::DebTriggerAwaitMode::NoAwait
                    )
                );
                if !directive_matches
                    || !await_mode_matches
                    || declaration.trigger_name != parsed.trigger_name
                    || declaration.raw_line != parsed.raw_line
                {
                    bail!(
                        "entry '{entry_id}' Debian trigger declaration {index} does not match its exact parsed control-artifact line"
                    );
                }
            }
        } else {
            if self.invocations.is_empty() {
                bail!(
                    "entry '{entry_id}' Debian maintainer script must preserve its invocation contracts"
                );
            }
            if !self.trigger_declarations.is_empty() {
                bail!(
                    "entry '{entry_id}' Debian maintainer script carries trigger declarations outside DEBIAN/triggers"
                );
            }
        }

        for invocation in &self.invocations {
            if invocation.lifecycle_paths.is_empty() {
                bail!(
                    "entry '{entry_id}' Debian maintainer mode '{}' has no fully typed lifecycle path",
                    invocation.mode.as_str()
                );
            }
            let Some(action) = invocation.arguments.first() else {
                bail!(
                    "entry '{entry_id}' Debian maintainer mode '{}' has no action argument",
                    invocation.mode.as_str()
                );
            };
            if action.index != 1
                || action.value != DebMaintainerArgumentValue::Action
                || !action.required
            {
                bail!(
                    "entry '{entry_id}' Debian maintainer mode '{}' must begin with required action argument 1",
                    invocation.mode.as_str()
                );
            }

            let mut saw_optional_argument = false;
            for (argument_offset, argument) in invocation.arguments.iter().enumerate() {
                let expected_index = argument_offset + 1;
                if argument.index != expected_index {
                    bail!(
                        "entry '{entry_id}' Debian maintainer mode '{}' argument indexes must be contiguous from 1",
                        invocation.mode.as_str()
                    );
                }
                if argument.required && saw_optional_argument {
                    bail!(
                        "entry '{entry_id}' Debian maintainer mode '{}' has required argument '{}' after an optional argument; optional arguments must form a positional suffix",
                        invocation.mode.as_str(),
                        argument.name
                    );
                }
                saw_optional_argument |= !argument.required;
                required_string("Debian maintainer argument name", &argument.name)?;
                match (&argument.value, argument.literal.as_deref()) {
                    (DebMaintainerArgumentValue::Literal, Some(literal)) => {
                        required_string("Debian maintainer argument literal", literal)?;
                    }
                    (DebMaintainerArgumentValue::Literal, None) => bail!(
                        "entry '{entry_id}' Debian literal argument '{}' has no value",
                        argument.name
                    ),
                    (_, Some(_)) => bail!(
                        "entry '{entry_id}' Debian non-literal argument '{}' carries a literal value",
                        argument.name
                    ),
                    (_, None) => {}
                }
            }
        }
        Ok(())
    }
}

fn required_string(label: &str, value: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() {
        bail!("{label} must not be empty");
    }
    Ok(())
}

fn validate_optional_sha256(label: &str, value: Option<&str>) -> anyhow::Result<()> {
    if let Some(value) = value {
        validate_sha256(label, value)?;
    }
    Ok(())
}

fn validate_sha256(label: &str, value: &str) -> anyhow::Result<()> {
    let Some((algorithm, digest)) = value.split_once(':') else {
        bail!("{label} must use sha256:<64 hex>");
    };
    if algorithm != "sha256" {
        bail!("{label} must use sha256:<64 hex>");
    }
    if digest.len() != 64
        || !digest
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        bail!("{label} must use sha256:<64 hex>");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
