// conary-core/src/ccs/legacy_scriptlets.rs
//! Passive Legacy Scriptlet Semantics Bundle metadata for CCS packages.

use crate::ccs::security_policy::SecurityPolicyIntent;
use anyhow::{anyhow, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub const LEGACY_SCRIPTLET_SCHEMA_V1: &str = "conary.legacy-scriptlets.v1";
pub const LEGACY_SCRIPTLET_SCHEMA_REVISION: u16 = 2;

macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($variant:ident => $value:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum $name {
            $($variant,)+
            Unknown(String),
        }

        impl $name {
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $value,)+
                    Self::Unknown(value) => value.as_str(),
                }
            }

            pub fn is_known(&self) -> bool {
                !matches!(self, Self::Unknown(_))
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct Visitor;

                impl<'de> serde::de::Visitor<'de> for Visitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("a string enum value")
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        Ok(match value {
                            $($value => $name::$variant,)+
                            other => $name::Unknown(other.to_string()),
                        })
                    }
                }

                deserializer.deserialize_str(Visitor)
            }
        }
    };
}

string_enum! {
    pub enum SourceFormat {
        Rpm => "rpm",
        Deb => "deb",
        Arch => "arch",
    }
}

string_enum! {
    pub enum VersionScheme {
        Rpm => "rpm",
        Deb => "deb",
        Arch => "arch",
        Semver => "semver",
    }
}

string_enum! {
    pub enum TargetCompatibility {
        SourceNative => "source-native",
        FamilyCompatible => "family-compatible",
        ConaryPortable => "conary-portable",
        ReviewRequired => "review-required",
        Blocked => "blocked",
    }
}

impl TargetCompatibility {
    pub fn is_actionable_for_replay(&self) -> bool {
        !matches!(
            self,
            Self::Unknown(_) | Self::ReviewRequired | Self::Blocked
        )
    }
}

string_enum! {
    pub enum ForeignReplayPolicy {
        Deny => "deny",
        Guarded => "guarded",
        Permissive => "permissive",
    }
}

string_enum! {
    pub enum PublicationPolicy {
        PublicIfNoBlocked => "public-if-no-blocked",
        PrivateReview => "private-review",
        LocalOnly => "local-only",
        Blocked => "blocked",
    }
}

impl PublicationPolicy {
    pub fn is_publication_eligible(&self) -> bool {
        matches!(self, Self::PublicIfNoBlocked)
    }
}

string_enum! {
    pub enum PublicationStatus {
        Public => "public",
        PrivateReview => "private-review",
        Blocked => "blocked",
        LocalOnly => "local-only",
    }
}

impl PublicationStatus {
    pub fn is_publication_eligible(&self) -> bool {
        matches!(self, Self::Public)
    }
}

string_enum! {
    pub enum ScriptletFidelity {
        NativeFree => "native-free",
        FullyReplaced => "fully-replaced",
        LegacyReplay => "legacy-replay",
        Mixed => "mixed",
        ReviewRequired => "review-required",
        Blocked => "blocked",
    }
}

string_enum! {
    pub enum ScriptletDecision {
        Replaced => "replaced",
        Legacy => "legacy",
        Blocked => "blocked",
        Review => "review",
    }
}

impl ScriptletDecision {
    pub fn is_actionable_for_replay(&self) -> bool {
        matches!(self, Self::Replaced | Self::Legacy)
    }
}

string_enum! {
    pub enum LifecyclePath {
        PreInstall => "pre-install",
        PostInstall => "post-install",
        PreUpgrade => "pre-upgrade",
        PostUpgrade => "post-upgrade",
        PreRemove => "pre-remove",
        PostRemove => "post-remove",
        PreTransaction => "pre-transaction",
        PostTransaction => "post-transaction",
        Trigger => "trigger",
        FileTrigger => "file-trigger",
    }
}

string_enum! {
    pub enum EffectSource {
        NativeShellAst => "native-shell-ast",
        PackageMetadata => "package-metadata",
        WrapperObservation => "wrapper-observation",
        HelperGrammar => "helper-grammar",
        ShellAst => "shell-ast",
    }
}

string_enum! {
    pub enum EffectConfidence {
        Declared => "declared",
        Observed => "observed",
        Inferred => "inferred",
        Uncertain => "uncertain",
    }
}

string_enum! {
    pub enum EffectReplacement {
        Complete => "complete",
        Partial => "partial",
        None => "none",
        Blocked => "blocked",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LegacyScriptletBundle {
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
    pub adapter_registry_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_policy_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_digest: Option<String>,
    pub target_compatibility: TargetCompatibility,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_targets: Vec<String>,
    pub foreign_replay_policy: ForeignReplayPolicy,
    pub publication_policy: PublicationPolicy,
    pub publication_status: PublicationStatus,
    pub scriptlet_fidelity: ScriptletFidelity,
    pub decision_counts: DecisionCounts,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unsupported_class_counts: BTreeMap<String, u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security_policy_intents: Vec<SecurityPolicyIntent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<LegacyScriptletEntry>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BootSecurityIntentEvidence {
    pub class_id: String,
    pub reason_code: String,
    pub command: String,
    pub command_provenance: CommandArgumentProvenance,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argument_provenance: Vec<CommandArgumentProvenance>,
    pub execution_context: CommandExecutionContext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lifecycle_paths: Vec<String>,
    pub source: CommandEvidenceSource,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline_id: Option<usize>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CommandExecutionContext {
    Unconditional,
    Conditional,
    Loop,
    Function,
    Pipeline,
    Subshell,
    CommandSubstitution,
    #[default]
    Unresolved,
}

impl CommandExecutionContext {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unconditional => "unconditional",
            Self::Conditional => "conditional",
            Self::Loop => "loop",
            Self::Function => "function",
            Self::Pipeline => "pipeline",
            Self::Subshell => "subshell",
            Self::CommandSubstitution => "command-substitution",
            Self::Unresolved => "unresolved",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CommandEvidenceSource {
    ShellAst,
    NativeShellAst,
    PackageMetadata,
    HelperGrammar,
    #[default]
    Unresolved,
}

impl CommandEvidenceSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ShellAst => "shell-ast",
            Self::NativeShellAst => "native-shell-ast",
            Self::PackageMetadata => "package-metadata",
            Self::HelperGrammar => "helper-grammar",
            Self::Unresolved => "unresolved",
        }
    }

    pub fn is_resolved(self) -> bool {
        self != Self::Unresolved
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct UnknownCommandEvidence {
    pub command: String,
    pub command_provenance: CommandArgumentProvenance,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argument_provenance: Vec<CommandArgumentProvenance>,
    pub execution_context: CommandExecutionContext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lifecycle_paths: Vec<String>,
    pub source: CommandEvidenceSource,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline_id: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LegacyScriptletEntry {
    pub id: String,
    pub native_slot: String,
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
    pub decision: ScriptletDecision,
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_evidence_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<ScriptletEffect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown_command_evidence: Vec<UnknownCommandEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_classes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boot_security_intents: Vec<BootSecurityIntentEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security_policy_intents: Vec<SecurityPolicyIntent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm_trigger: Option<RpmTriggerMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deb_maintainer: Option<DebMaintainerMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch_install: Option<ArchInstallMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub residual_replay: Option<ResidualReplayMetadata>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct NativeInvocation {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chroot: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TransactionOrder {
    pub position: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ScriptletSandboxRequirements {
    #[serde(default)]
    pub network: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub namespaces: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seccomp_profile: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptletEffect {
    pub kind: String,
    pub source: EffectSource,
    pub confidence: EffectConfidence,
    pub replacement: EffectReplacement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DecisionCounts {
    #[serde(default)]
    pub replaced: u32,
    #[serde(default)]
    pub legacy: u32,
    #[serde(default)]
    pub blocked: u32,
    #[serde(default)]
    pub review: u32,
    #[serde(flatten)]
    pub extra: BTreeMap<String, u32>,
}

impl DecisionCounts {
    pub fn total(&self) -> u32 {
        self.replaced + self.legacy + self.blocked + self.review + self.extra.values().sum::<u32>()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RpmTriggerMetadata {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_constraints: Vec<RpmTriggerTargetConstraint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_globs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin_contract: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction_order: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RpmTriggerTargetConstraint {
    pub package: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DebMaintainerMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triggers_content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trigger_names: Vec<String>,
    #[serde(default)]
    pub purge: bool,
    #[serde(default)]
    pub abort: bool,
    #[serde(default)]
    pub noninteractive: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ArchInstallMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub called_function: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrapper_source_digest: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResidualReplayMetadata {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub superseded_effect_kinds: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrapper_strategy: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suppression_markers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub residual_body_digest: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

impl LegacyScriptletBundle {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.schema != LEGACY_SCRIPTLET_SCHEMA_V1 {
            bail!(
                "legacy scriptlet bundle schema must be {LEGACY_SCRIPTLET_SCHEMA_V1}, got {}",
                self.schema
            );
        }
        if self.schema_revision != LEGACY_SCRIPTLET_SCHEMA_REVISION {
            bail!(
                "legacy scriptlet bundle schema_revision must be {LEGACY_SCRIPTLET_SCHEMA_REVISION}, got {}",
                self.schema_revision
            );
        }

        required_string("source_family", &self.source_family)?;
        required_string("source_package", &self.source_package)?;
        required_string("source_version", &self.source_version)?;
        required_string("conversion_tool", &self.conversion_tool)?;
        required_string("conversion_tool_version", &self.conversion_tool_version)?;
        required_string("conversion_policy", &self.conversion_policy)?;

        validate_optional_sha256("source_checksum", self.source_checksum.as_deref())?;
        validate_optional_sha256(
            "adapter_registry_digest",
            self.adapter_registry_digest.as_deref(),
        )?;
        validate_optional_sha256("target_policy_digest", self.target_policy_digest.as_deref())?;
        validate_optional_sha256("evidence_digest", self.evidence_digest.as_deref())?;

        for target in &self.allowed_targets {
            validate_allowed_target(target)?;
        }

        let mut seen_ids = BTreeSet::new();
        let mut expected_counts = DecisionCounts::default();
        for entry in &self.entries {
            if !seen_ids.insert(entry.id.as_str()) {
                bail!("duplicate entry id '{}'", entry.id);
            }
            expected_counts.record(&entry.decision);
            entry.validate()?;
        }

        if self.decision_counts != expected_counts {
            bail!(
                "decision counts do not match entries: expected {:?}, got {:?}",
                expected_counts,
                self.decision_counts
            );
        }
        if self.decision_counts.total() != self.entries.len() as u32 {
            bail!(
                "decision counts total {} does not match entry count {}",
                self.decision_counts.total(),
                self.entries.len()
            );
        }

        Ok(())
    }
}

impl LegacyScriptletEntry {
    fn validate(&self) -> anyhow::Result<()> {
        required_string("entry.id", &self.id)?;
        required_string("entry.native_slot", &self.native_slot)?;
        required_string("entry.interpreter", &self.interpreter)?;
        required_string("entry.body", &self.body)?;
        required_string("entry.reason_code", &self.reason_code)?;
        required_string(
            "entry.transaction_order.position",
            &self.transaction_order.position,
        )?;

        if self.lifecycle_paths.is_empty() {
            bail!("entry '{}' lifecycle_paths must not be empty", self.id);
        }
        if self.timeout_ms == 0 {
            bail!("entry '{}' timeout_ms must be greater than zero", self.id);
        }

        validate_sha256("entry.body_sha256", &self.body_sha256)?;
        validate_optional_sha256("entry.evidence_digest", self.evidence_digest.as_deref())?;

        self.body_bytes()?;

        for effect in &self.effects {
            effect.validate(&self.id)?;
        }
        for evidence in &self.unknown_command_evidence {
            evidence.validate(&self.id)?;
        }
        for evidence in &self.boot_security_intents {
            evidence.validate(&self.id)?;
        }
        if let Some(metadata) = &self.arch_install {
            metadata.validate(&self.id)?;
        }
        if let Some(metadata) = &self.residual_replay {
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

impl UnknownCommandEvidence {
    fn validate(&self, entry_id: &str) -> anyhow::Result<()> {
        required_string("unknown command evidence command", &self.command)
            .map_err(|error| anyhow!("entry '{entry_id}' {error}"))?;
        if !self.source.is_resolved() {
            bail!("entry '{entry_id}' unknown command evidence source is unresolved");
        }
        if self.argv.len() != self.argument_provenance.len() {
            bail!(
                "entry '{}' unknown command evidence argv/provenance length mismatch",
                entry_id
            );
        }
        Ok(())
    }
}

impl BootSecurityIntentEvidence {
    fn validate(&self, entry_id: &str) -> anyhow::Result<()> {
        required_string("boot security evidence class_id", &self.class_id)
            .map_err(|error| anyhow!("entry '{entry_id}' {error}"))?;
        required_string("boot security evidence reason_code", &self.reason_code)
            .map_err(|error| anyhow!("entry '{entry_id}' {error}"))?;
        required_string("boot security evidence command", &self.command)
            .map_err(|error| anyhow!("entry '{entry_id}' {error}"))?;
        if !self.source.is_resolved() {
            bail!("entry '{entry_id}' boot security evidence source is unresolved");
        }
        if self.argv.len() != self.argument_provenance.len() {
            bail!(
                "entry '{}' boot security evidence argv/provenance length mismatch",
                entry_id
            );
        }
        Ok(())
    }
}

impl ScriptletEffect {
    fn validate(&self, entry_id: &str) -> anyhow::Result<()> {
        required_string("effect.kind", &self.kind)?;
        validate_optional_sha256("effect.adapter_digest", self.adapter_digest.as_deref())
            .map_err(|error| anyhow!("entry '{entry_id}' {error}"))?;
        Ok(())
    }
}

impl DecisionCounts {
    fn record(&mut self, decision: &ScriptletDecision) {
        match decision {
            ScriptletDecision::Replaced => self.replaced += 1,
            ScriptletDecision::Legacy => self.legacy += 1,
            ScriptletDecision::Blocked => self.blocked += 1,
            ScriptletDecision::Review => self.review += 1,
            ScriptletDecision::Unknown(value) => {
                *self.extra.entry(value.clone()).or_insert(0) += 1;
            }
        }
    }
}

impl ArchInstallMetadata {
    fn validate(&self, entry_id: &str) -> anyhow::Result<()> {
        validate_optional_sha256(
            "arch_install.install_digest",
            self.install_digest.as_deref(),
        )
        .map_err(|error| anyhow!("entry '{entry_id}' {error}"))?;
        validate_optional_sha256(
            "arch_install.wrapper_source_digest",
            self.wrapper_source_digest.as_deref(),
        )
        .map_err(|error| anyhow!("entry '{entry_id}' {error}"))?;
        Ok(())
    }
}

impl ResidualReplayMetadata {
    fn validate(&self, entry_id: &str) -> anyhow::Result<()> {
        validate_optional_sha256(
            "residual_replay.residual_body_digest",
            self.residual_body_digest.as_deref(),
        )
        .map_err(|error| anyhow!("entry '{entry_id}' {error}"))?;
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

fn validate_allowed_target(value: &str) -> anyhow::Result<()> {
    let parts: Vec<&str> = value.split('/').collect();
    if parts.len() != 4 || parts.iter().any(|part| part.trim().is_empty()) {
        bail!("allowed target '{value}' must use <format>/<distro>/<release>/<arch>");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
