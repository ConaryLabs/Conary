// conary-core/src/ccs/convert/debian_adapters.rs

use crate::ccs::convert::adapters::{AdapterInput, ScriptletEffectAdapter};
use crate::ccs::convert::command_evidence::CommandEvidenceSource;
use crate::ccs::convert::effects::{ScriptletClassification, ScriptletEffectEvidence};
use crate::ccs::legacy_scriptlets::{EffectConfidence, EffectReplacement, EffectSource};
use std::collections::BTreeMap;

const COMPLETE_REASON: &str = "helper-complete-deb-systemd-helper-unit-state";
const REVIEW_REASON: &str = "helper-review-deb-systemd-helper-state";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DebSystemdHelperAction {
    Enable,
    Disable,
    Purge,
    Mask,
    Unmask,
    IsEnabled,
    WasEnabled,
    DebianInstalled,
    UpdateState,
    Reenable,
}

impl DebSystemdHelperAction {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "enable" => Self::Enable,
            "disable" => Self::Disable,
            "purge" => Self::Purge,
            "mask" => Self::Mask,
            "unmask" => Self::Unmask,
            "is-enabled" => Self::IsEnabled,
            "was-enabled" => Self::WasEnabled,
            "debian-installed" => Self::DebianInstalled,
            "update-state" => Self::UpdateState,
            "reenable" => Self::Reenable,
            _ => return None,
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Enable => "enable",
            Self::Disable => "disable",
            Self::Purge => "purge",
            Self::Mask => "mask",
            Self::Unmask => "unmask",
            Self::IsEnabled => "is-enabled",
            Self::WasEnabled => "was-enabled",
            Self::DebianInstalled => "debian-installed",
            Self::UpdateState => "update-state",
            Self::Reenable => "reenable",
        }
    }

    fn state_model(self) -> &'static str {
        match self {
            Self::Enable => "first-enable-state-file",
            Self::Disable => "enablement-state-update",
            Self::Purge => "state-file-purge",
            Self::Mask => "mask-state-save",
            Self::Unmask => "mask-state-restore",
            Self::IsEnabled => "enablement-query",
            Self::WasEnabled => "previous-enablement-query",
            Self::DebianInstalled => "state-file-presence-query",
            Self::UpdateState => "state-file-reconcile",
            Self::Reenable => "reenable-from-recorded-state",
        }
    }

    fn can_be_complete(self) -> bool {
        matches!(self, Self::Enable | Self::Disable)
    }
}

#[derive(Debug, Clone)]
struct DebSystemdHelperInvocation {
    action: DebSystemdHelperAction,
    units: Vec<String>,
}

impl DebSystemdHelperInvocation {
    fn parse(input: AdapterInput<'_>) -> Option<Self> {
        if input.invocation.command != "deb-systemd-helper" {
            return None;
        }

        let action = input
            .invocation
            .argv
            .first()
            .and_then(|arg| DebSystemdHelperAction::parse(arg))?;
        let units = input
            .invocation
            .argv
            .iter()
            .skip(1)
            .cloned()
            .collect::<Vec<_>>();
        if units.is_empty() || units.iter().any(|unit| unit.starts_with('-')) {
            return None;
        }

        Some(Self { action, units })
    }

    fn all_units_are_packaged(&self, input: AdapterInput<'_>) -> bool {
        self.units
            .iter()
            .all(|unit| input.payload.systemd_units.contains(unit))
    }
}

pub(super) struct DebSystemdHelperAdapter;

impl ScriptletEffectAdapter for DebSystemdHelperAdapter {
    fn id(&self) -> &'static str {
        "deb-systemd-helper/v1"
    }

    fn digest(&self) -> String {
        crate::hash::sha256_prefixed(b"deb-systemd-helper/v1:documented-actions:state-semantics")
    }

    fn command_names(&self) -> &'static [&'static str] {
        &["deb-systemd-helper"]
    }

    fn matches(&self, input: AdapterInput<'_>) -> bool {
        DebSystemdHelperInvocation::parse(input).is_some()
    }

    fn classify(&self, input: AdapterInput<'_>) -> ScriptletClassification {
        let helper = DebSystemdHelperInvocation::parse(input)
            .expect("matches() must ensure deb-systemd-helper invocation");
        let payload_backed = helper.all_units_are_packaged(input);
        let complete = helper.action.can_be_complete() && payload_backed;
        let replacement = if complete {
            EffectReplacement::Complete
        } else {
            EffectReplacement::Partial
        };
        let reason_code = if complete {
            COMPLETE_REASON
        } else {
            REVIEW_REASON
        };

        let mut extra = BTreeMap::from([
            (
                "debian_helper_action".to_string(),
                toml::Value::String(helper.action.as_str().to_string()),
            ),
            (
                "units".to_string(),
                toml::Value::Array(
                    helper
                        .units
                        .iter()
                        .cloned()
                        .map(toml::Value::String)
                        .collect(),
                ),
            ),
            (
                "state_model".to_string(),
                toml::Value::String(helper.action.state_model().to_string()),
            ),
            (
                "payload_backed".to_string(),
                toml::Value::Boolean(payload_backed),
            ),
            ("documented_action".to_string(), toml::Value::Boolean(true)),
            ("maintscript_only".to_string(), toml::Value::Boolean(true)),
            ("dpkg_root_aware".to_string(), toml::Value::Boolean(true)),
        ]);
        if !complete {
            extra.insert("review_required".to_string(), toml::Value::Boolean(true));
        }

        ScriptletClassification::Known {
            reason_code: reason_code.to_string(),
            effects: vec![ScriptletEffectEvidence {
                kind: "debian-systemd-helper-state".to_string(),
                source: effect_source(input.invocation.source),
                confidence: EffectConfidence::Inferred,
                replacement,
                adapter_id: Some(self.id().to_string()),
                adapter_digest: Some(self.digest()),
                command: Some(input.invocation.command.clone()),
                args: input.invocation.argv.clone(),
                path: helper.units.first().cloned(),
                reason_code: Some(reason_code.to_string()),
                extra,
            }],
        }
    }
}

fn effect_source(source: CommandEvidenceSource) -> EffectSource {
    match source {
        CommandEvidenceSource::StaticSignal => EffectSource::StaticSignal,
        CommandEvidenceSource::CaptureLog => EffectSource::CaptureLog,
        CommandEvidenceSource::NativeMetadata => EffectSource::NativeMetadata,
        CommandEvidenceSource::PayloadHeuristic => EffectSource::PayloadHeuristic,
        CommandEvidenceSource::CuratedRule => EffectSource::CuratedRule,
    }
}
