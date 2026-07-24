// conary-core/src/ccs/convert/apparmor_adapters.rs

use crate::ccs::convert::adapters::{AdapterInput, ScriptletEffectAdapter};
use crate::ccs::convert::command_evidence::CommandEvidenceSource;
use crate::ccs::convert::effects::{ScriptletClassification, ScriptletEffectEvidence};
use crate::ccs::convert::payload_hints::PayloadHints;
use crate::ccs::legacy_scriptlets::{EffectConfidence, EffectReplacement, EffectSource};
use std::collections::BTreeMap;

const COMPLETE_REASON: &str = "helper-complete-apparmor-policy";
const HOST_POLICY_BEHAVIOR: &str = "apply-when-apparmor-present-dormant-when-absent";
const TARGET_SECURITY_POLICY: &str = "apparmor-optional";

#[derive(Debug, Clone, PartialEq, Eq)]
enum ApparmorPolicyInvocation {
    ProfileReload { path: String, profile_name: String },
}

impl ApparmorPolicyInvocation {
    fn parse(input: AdapterInput<'_>) -> Option<Self> {
        match input.invocation.command.as_str() {
            "apparmor_parser" => parse_apparmor_parser(&input.invocation.argv, input.payload),
            _ => None,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::ProfileReload { .. } => "apparmor-profile-reload",
        }
    }

    fn operation(&self) -> &'static str {
        match self {
            Self::ProfileReload { .. } => "profile-reload",
        }
    }

    fn path(&self) -> &str {
        match self {
            Self::ProfileReload { path, .. } => path,
        }
    }

    fn extra(&self) -> BTreeMap<String, toml::Value> {
        let mut extra = BTreeMap::from([
            (
                "apparmor_operation".to_string(),
                toml::Value::String(self.operation().to_string()),
            ),
            (
                "target_security_policy".to_string(),
                toml::Value::String(TARGET_SECURITY_POLICY.to_string()),
            ),
            (
                "host_policy_behavior".to_string(),
                toml::Value::String(HOST_POLICY_BEHAVIOR.to_string()),
            ),
            ("payload_backed".to_string(), toml::Value::Boolean(true)),
        ]);

        match self {
            Self::ProfileReload { path, profile_name } => {
                extra.insert(
                    "profile_path".to_string(),
                    toml::Value::String(path.clone()),
                );
                extra.insert(
                    "profile_name".to_string(),
                    toml::Value::String(profile_name.clone()),
                );
                extra.insert(
                    "paths".to_string(),
                    toml::Value::Array(vec![toml::Value::String(path.clone())]),
                );
            }
        }

        extra
    }
}

pub(super) struct ApparmorPolicyAdapter;

impl ScriptletEffectAdapter for ApparmorPolicyAdapter {
    fn id(&self) -> &'static str {
        "apparmor-policy/v1"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(b"apparmor-policy/v1:optional-policy:payload-backed-profile")
    }

    fn command_names(&self) -> &'static [&'static str] {
        &["apparmor_parser"]
    }

    fn matches(&self, input: AdapterInput<'_>) -> bool {
        ApparmorPolicyInvocation::parse(input).is_some()
    }

    fn classify(&self, input: AdapterInput<'_>) -> ScriptletClassification {
        let policy = ApparmorPolicyInvocation::parse(input)
            .expect("matches() must ensure AppArmor policy invocation");

        ScriptletClassification::Known {
            reason_code: COMPLETE_REASON.to_string(),
            effects: vec![ScriptletEffectEvidence {
                kind: policy.kind().to_string(),
                source: effect_source(input.invocation.source),
                confidence: EffectConfidence::Inferred,
                replacement: EffectReplacement::Complete,
                adapter_id: Some(self.id().to_string()),
                adapter_digest: Some(self.digest()),
                command: Some(input.invocation.command.clone()),
                args: input.invocation.argv.clone(),
                path: Some(policy.path().to_string()),
                reason_code: Some(COMPLETE_REASON.to_string()),
                extra: policy.extra(),
            }],
        }
    }
}

fn parse_apparmor_parser(
    argv: &[String],
    payload: &PayloadHints,
) -> Option<ApparmorPolicyInvocation> {
    let mut replace = false;
    let mut paths = Vec::new();

    for arg in argv {
        match arg.as_str() {
            "-r" | "--replace" => replace = true,
            value if value.starts_with('-') => return None,
            value => paths.push(value.to_string()),
        }
    }

    if !replace || paths.len() != 1 {
        return None;
    }
    let path = paths.pop()?;
    let profile_name = profile_name_from_path(&path)?;
    if !payload.payload_paths.contains(&path) {
        return None;
    }

    Some(ApparmorPolicyInvocation::ProfileReload { path, profile_name })
}

fn profile_name_from_path(path: &str) -> Option<String> {
    let profile_name = path.strip_prefix("/etc/apparmor.d/")?;
    if profile_name.is_empty()
        || profile_name.contains('/')
        || profile_name.contains('\0')
        || !profile_name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        return None;
    }
    Some(profile_name.to_string())
}

fn effect_source(source: CommandEvidenceSource) -> EffectSource {
    match source {
        CommandEvidenceSource::ShellAst => EffectSource::ShellAst,
        CommandEvidenceSource::NativeShellAst => EffectSource::NativeShellAst,
        CommandEvidenceSource::PackageMetadata => EffectSource::PackageMetadata,
        CommandEvidenceSource::HelperGrammar => EffectSource::HelperGrammar,
        CommandEvidenceSource::Unresolved => EffectSource::Unknown("unresolved".to_string()),
    }
}
