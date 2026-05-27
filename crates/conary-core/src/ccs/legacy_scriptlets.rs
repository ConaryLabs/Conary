// conary-core/src/ccs/legacy_scriptlets.rs
//! Passive Legacy Scriptlet Semantics Bundle metadata for CCS packages.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

pub const LEGACY_SCRIPTLET_SCHEMA_V1: &str = "conary.legacy-scriptlets.v1";

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
        !matches!(self, Self::Unknown(_) | Self::ReviewRequired | Self::Blocked)
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
        NativeMetadata => "native-metadata",
        PayloadHeuristic => "payload-heuristic",
        CaptureLog => "capture-log",
        WrapperObservation => "wrapper-observation",
        CuratedRule => "curated-rule",
        StaticSignal => "static-signal",
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
    pub entries: Vec<LegacyScriptletEntry>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
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
    pub unknown_commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_classes: Vec<String>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn sha256_prefixed(body: &str) -> String {
        crate::hash::sha256_prefixed(body.as_bytes())
    }

    fn sample_effect() -> ScriptletEffect {
        ScriptletEffect {
            kind: "ldconfig".to_string(),
            source: EffectSource::StaticSignal,
            confidence: EffectConfidence::Declared,
            replacement: EffectReplacement::Complete,
            adapter_id: Some("ldconfig/v1".to_string()),
            adapter_digest: Some(
                "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                    .to_string(),
            ),
            command: Some("ldconfig".to_string()),
            args: vec!["-X".to_string()],
            path: Some("/usr/lib64".to_string()),
            reason_code: Some("ldconfig-cache-refresh".to_string()),
            extra: BTreeMap::new(),
        }
    }

    fn sample_entry(id: &str, decision: ScriptletDecision, body: &str) -> LegacyScriptletEntry {
        LegacyScriptletEntry {
            id: id.to_string(),
            native_slot: "%post".to_string(),
            phase: LifecyclePath::PostInstall,
            lifecycle_paths: vec!["install:first".to_string()],
            interpreter: "/bin/sh".to_string(),
            interpreter_args: vec!["-e".to_string()],
            body_sha256: sha256_prefixed(body),
            body: body.to_string(),
            body_encoding: None,
            native_invocation: NativeInvocation {
                args: vec!["1".to_string()],
                environment: vec!["RPM_INSTALL_PREFIX=/".to_string()],
                stdin: Some("none".to_string()),
                chroot: Some("install-root".to_string()),
                extra: BTreeMap::new(),
            },
            transaction_order: TransactionOrder {
                position: "after-payload".to_string(),
                before: vec![],
                after: vec!["payload".to_string()],
                extra: BTreeMap::new(),
            },
            timeout_ms: 30_000,
            sandbox: Some(ScriptletSandboxRequirements {
                network: false,
                namespaces: vec!["mount".to_string(), "pid".to_string()],
                seccomp_profile: Some("legacy-scriptlet/default".to_string()),
                extra: BTreeMap::new(),
            }),
            capabilities: vec!["ldconfig".to_string()],
            decision,
            reason_code: "test-fixture".to_string(),
            human_reason: Some("fixture entry".to_string()),
            evidence_digest: Some(
                "sha256:2222222222222222222222222222222222222222222222222222222222222222"
                    .to_string(),
            ),
            source_evidence_refs: vec!["capture:rpm:%post".to_string()],
            effects: vec![sample_effect()],
            unknown_commands: vec![],
            blocked_classes: vec![],
            rpm_trigger: None,
            deb_maintainer: None,
            arch_install: None,
            residual_replay: None,
            extra: BTreeMap::new(),
        }
    }

    fn sample_bundle() -> LegacyScriptletBundle {
        LegacyScriptletBundle {
            schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
            schema_revision: 1,
            source_format: SourceFormat::Rpm,
            source_family: "fedora-rhel".to_string(),
            source_distro: Some("fedora".to_string()),
            source_release: Some("44".to_string()),
            source_arch: Some("x86_64".to_string()),
            source_package: "nginx".to_string(),
            source_version: "1.28.0-1.fc44".to_string(),
            source_checksum: Some(
                "sha256:3333333333333333333333333333333333333333333333333333333333333333"
                    .to_string(),
            ),
            version_scheme: VersionScheme::Rpm,
            conversion_tool: "remi".to_string(),
            conversion_tool_version: "0.8.0".to_string(),
            conversion_policy: "safe-or-legacy".to_string(),
            adapter_registry_digest: Some(
                "sha256:4444444444444444444444444444444444444444444444444444444444444444"
                    .to_string(),
            ),
            target_policy_digest: Some(
                "sha256:5555555555555555555555555555555555555555555555555555555555555555"
                    .to_string(),
            ),
            evidence_digest: Some(
                "sha256:6666666666666666666666666666666666666666666666666666666666666666"
                    .to_string(),
            ),
            target_compatibility: TargetCompatibility::SourceNative,
            allowed_targets: vec!["rpm/fedora/44/x86_64".to_string()],
            foreign_replay_policy: ForeignReplayPolicy::Deny,
            publication_policy: PublicationPolicy::PublicIfNoBlocked,
            publication_status: PublicationStatus::PrivateReview,
            scriptlet_fidelity: ScriptletFidelity::Mixed,
            decision_counts: DecisionCounts {
                replaced: 1,
                legacy: 1,
                blocked: 0,
                review: 0,
                extra: BTreeMap::new(),
            },
            unsupported_class_counts: BTreeMap::new(),
            entries: vec![
                sample_entry("rpm:%preun", ScriptletDecision::Replaced, "ldconfig\n"),
                sample_entry("rpm:%post", ScriptletDecision::Legacy, "systemctl daemon-reload\n"),
            ],
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn legacy_scriptlet_bundle_round_trips_core_fields() {
        let bundle = sample_bundle();

        let encoded = toml::to_string_pretty(&bundle).expect("serialize bundle");
        let decoded: LegacyScriptletBundle = toml::from_str(&encoded).expect("parse bundle");

        assert_eq!(decoded.schema, LEGACY_SCRIPTLET_SCHEMA_V1);
        assert_eq!(decoded.source_format, SourceFormat::Rpm);
        assert_eq!(decoded.target_compatibility, TargetCompatibility::SourceNative);
        assert_eq!(decoded.foreign_replay_policy, ForeignReplayPolicy::Deny);
        assert_eq!(decoded.entries.len(), 2);
        assert_eq!(decoded.entries[0].decision, ScriptletDecision::Replaced);
        assert_eq!(decoded.entries[1].decision, ScriptletDecision::Legacy);
        assert_eq!(decoded.entries[0].effects[0].replacement, EffectReplacement::Complete);
    }

    #[test]
    fn legacy_scriptlet_bundle_round_trips_reserved_metadata() {
        let mut bundle = sample_bundle();
        let entry = bundle.entries.first_mut().expect("fixture entry");
        entry.rpm_trigger = Some(RpmTriggerMetadata {
            kind: "file-trigger".to_string(),
            condition: Some("in".to_string()),
            target_constraints: vec![RpmTriggerTargetConstraint {
                package: "systemd".to_string(),
                operator: Some(">=".to_string()),
                version: Some("255".to_string()),
                extra: BTreeMap::new(),
            }],
            priority: Some(100),
            file_globs: vec!["/usr/lib/systemd/system/*.service".to_string()],
            stdin_contract: Some("paths".to_string()),
            transaction_order: Some("post-transaction".to_string()),
            extra: BTreeMap::new(),
        });
        entry.deb_maintainer = Some(DebMaintainerMetadata {
            invocation_mode: Some("configure".to_string()),
            old_version: Some("1.27".to_string()),
            new_version: Some("1.28".to_string()),
            triggers_content: Some("interest-noawait nginx-reload".to_string()),
            trigger_names: vec!["nginx-reload".to_string()],
            purge: true,
            abort: true,
            noninteractive: true,
            extra: BTreeMap::new(),
        });
        entry.arch_install = Some(ArchInstallMetadata {
            install_digest: Some(
                "sha256:7777777777777777777777777777777777777777777777777777777777777777"
                    .to_string(),
            ),
            called_function: Some("post_install".to_string()),
            old_version: Some("1.27-1".to_string()),
            new_version: Some("1.28-1".to_string()),
            wrapper_source_digest: Some(
                "sha256:8888888888888888888888888888888888888888888888888888888888888888"
                    .to_string(),
            ),
            extra: BTreeMap::new(),
        });
        entry.residual_replay = Some(ResidualReplayMetadata {
            superseded_effect_kinds: vec!["ldconfig".to_string()],
            wrapper_strategy: Some("source-and-suppress".to_string()),
            suppression_markers: vec!["CONARY_SUPPRESS_LDCONFIG=1".to_string()],
            residual_body_digest: Some(
                "sha256:9999999999999999999999999999999999999999999999999999999999999999"
                    .to_string(),
            ),
            extra: BTreeMap::new(),
        });

        let encoded = toml::to_string_pretty(&bundle).expect("serialize bundle");
        let decoded: LegacyScriptletBundle = toml::from_str(&encoded).expect("parse bundle");
        let decoded_entry = decoded.entries.first().expect("round-tripped entry");

        assert_eq!(
            decoded_entry
                .rpm_trigger
                .as_ref()
                .expect("rpm trigger")
                .file_globs,
            vec!["/usr/lib/systemd/system/*.service"]
        );
        assert!(decoded_entry.deb_maintainer.as_ref().expect("deb").purge);
        assert_eq!(
            decoded_entry
                .arch_install
                .as_ref()
                .expect("arch")
                .called_function
                .as_deref(),
            Some("post_install")
        );
        assert_eq!(
            decoded_entry
                .residual_replay
                .as_ref()
                .expect("residual")
                .superseded_effect_kinds,
            vec!["ldconfig"]
        );
    }

    #[test]
    fn legacy_scriptlet_bundle_preserves_unknown_optional_fields() {
        let mut bundle = sample_bundle();
        bundle.extra.insert(
            "future_top_level".to_string(),
            toml::Value::String("kept".to_string()),
        );
        bundle.entries[0].extra.insert(
            "future_entry_field".to_string(),
            toml::Value::String("also-kept".to_string()),
        );
        bundle.entries[0].effects[0].extra.insert(
            "future_effect_field".to_string(),
            toml::Value::Integer(7),
        );

        let encoded = toml::to_string_pretty(&bundle).expect("serialize bundle");
        let decoded: LegacyScriptletBundle = toml::from_str(&encoded).expect("parse bundle");

        assert_eq!(
            decoded.extra.get("future_top_level").and_then(toml::Value::as_str),
            Some("kept")
        );
        assert_eq!(
            decoded.entries[0]
                .extra
                .get("future_entry_field")
                .and_then(toml::Value::as_str),
            Some("also-kept")
        );
        assert_eq!(
            decoded.entries[0].effects[0]
                .extra
                .get("future_effect_field")
                .and_then(toml::Value::as_integer),
            Some(7)
        );
    }

    #[test]
    fn legacy_scriptlet_bundle_retains_unknown_typed_enum_values() {
        let toml = r#"
schema = "conary.legacy-scriptlets.v1"
schema_revision = 1
source_format = "apk"
source_family = "alpine"
source_package = "busybox"
source_version = "1.37.0"
version_scheme = "apk"
conversion_tool = "remi"
conversion_tool_version = "0.8.0"
conversion_policy = "passive-test"
target_compatibility = "future-compatible"
foreign_replay_policy = "operator-review"
publication_policy = "curated-lane"
publication_status = "staged"
scriptlet_fidelity = "machine-reviewed"

[decision_counts]
review = 0
"#;

        let decoded: LegacyScriptletBundle = toml::from_str(toml).expect("parse bundle");

        assert_eq!(decoded.source_format, SourceFormat::Unknown("apk".to_string()));
        assert_eq!(decoded.version_scheme, VersionScheme::Unknown("apk".to_string()));
        assert_eq!(
            decoded.target_compatibility,
            TargetCompatibility::Unknown("future-compatible".to_string())
        );
        assert_eq!(
            decoded.foreign_replay_policy,
            ForeignReplayPolicy::Unknown("operator-review".to_string())
        );
        assert_eq!(
            decoded.publication_policy,
            PublicationPolicy::Unknown("curated-lane".to_string())
        );
        assert_eq!(
            decoded.publication_status,
            PublicationStatus::Unknown("staged".to_string())
        );
        assert_eq!(
            decoded.scriptlet_fidelity,
            ScriptletFidelity::Unknown("machine-reviewed".to_string())
        );
    }

    #[test]
    fn legacy_scriptlet_bundle_accepts_zero_entry_native_free_package() {
        let mut bundle = sample_bundle();
        bundle.entries.clear();
        bundle.decision_counts = DecisionCounts::default();
        bundle.scriptlet_fidelity = ScriptletFidelity::NativeFree;

        let encoded = toml::to_string_pretty(&bundle).expect("serialize bundle");
        let decoded: LegacyScriptletBundle = toml::from_str(&encoded).expect("parse bundle");

        assert!(decoded.entries.is_empty());
        assert_eq!(decoded.scriptlet_fidelity, ScriptletFidelity::NativeFree);
    }
}
