// conary-core/src/ccs/convert/effects.rs

use crate::ccs::convert::command_evidence::CommandInvocation;
use crate::ccs::legacy_scriptlets::{EffectConfidence, EffectReplacement, EffectSource};
use crate::packages::native_abi::NativeScriptletEntry;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct ScriptletEffectEvidence {
    pub kind: String,
    pub source: EffectSource,
    pub confidence: EffectConfidence,
    pub replacement: EffectReplacement,
    pub adapter_id: Option<String>,
    pub adapter_digest: Option<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub path: Option<String>,
    pub reason_code: Option<String>,
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptletCommandEvidence {
    pub command: String,
    pub argv: Vec<String>,
    pub phase: Option<String>,
    pub lifecycle_paths: Vec<String>,
    pub raw_line: Option<String>,
    pub source: String,
    pub environment: Vec<String>,
}

impl ScriptletCommandEvidence {
    pub fn from_invocation(invocation: &CommandInvocation) -> Self {
        Self {
            command: invocation.command.clone(),
            argv: sanitize_command_argv(&invocation.argv),
            phase: invocation.phase.clone(),
            lifecycle_paths: invocation.lifecycle_paths.clone(),
            raw_line: invocation.raw_line.clone(),
            source: invocation.source.as_str().to_string(),
            environment: invocation
                .environment
                .iter()
                .map(|fact| fact.name.clone())
                .collect(),
        }
    }
}

fn sanitize_command_argv(argv: &[String]) -> Vec<String> {
    argv.iter().map(|arg| sanitize_command_arg(arg)).collect()
}

fn sanitize_command_arg(arg: &str) -> String {
    if let Some(rest) = arg.strip_prefix("/boot/") {
        return format!("<boot>/{}", sanitize_path_segments(rest));
    }

    sanitize_path_segments(arg)
}

fn sanitize_path_segments(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if looks_like_kernel_version(segment) {
                "<kver>"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn looks_like_kernel_version(segment: &str) -> bool {
    let mut parts = segment.split('.');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(major), Some(minor), Some(patch))
            if major.chars().all(|ch| ch.is_ascii_digit())
                && minor.chars().all(|ch| ch.is_ascii_digit())
                && patch.chars().next().is_some_and(|ch| ch.is_ascii_digit())
    )
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScriptletClassification {
    Known {
        reason_code: String,
        effects: Vec<ScriptletEffectEvidence>,
    },
    Unknown {
        reason_code: String,
        command: String,
    },
    Review {
        reason_code: String,
        class_id: Option<String>,
        command: Option<ScriptletCommandEvidence>,
    },
    Blocked {
        reason_code: String,
        class_id: String,
        command: Option<ScriptletCommandEvidence>,
    },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScriptletClassificationReport {
    pub entries: Vec<EntryClassification>,
    pub known_count: u32,
    pub unknown_count: u32,
    pub review_count: u32,
    pub blocked_count: u32,
    pub unsupported_class_counts: BTreeMap<String, u32>,
}

impl ScriptletClassificationReport {
    pub fn push(&mut self, entry_id: impl Into<String>, classification: ScriptletClassification) {
        match &classification {
            ScriptletClassification::Known { .. } => {
                self.known_count += 1;
            }
            ScriptletClassification::Unknown { .. } => {
                self.unknown_count += 1;
            }
            ScriptletClassification::Review { class_id, .. } => {
                self.review_count += 1;
                if let Some(class_id) = class_id {
                    increment_class_count(&mut self.unsupported_class_counts, class_id);
                }
            }
            ScriptletClassification::Blocked { class_id, .. } => {
                self.blocked_count += 1;
                increment_class_count(&mut self.unsupported_class_counts, class_id);
            }
        }

        self.entries.push(EntryClassification {
            entry_id: entry_id.into(),
            classification,
        });
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntryClassification {
    pub entry_id: String,
    pub classification: ScriptletClassification,
}

fn increment_class_count(counts: &mut BTreeMap<String, u32>, class_id: &str) {
    *counts.entry(class_id.to_string()).or_default() += 1;
}

pub(crate) fn native_deferred_support_can_be_adapter_covered(
    _entry: &NativeScriptletEntry,
) -> bool {
    // Command adapters can replace individual shell effects, but deferred
    // native package-manager semantics need explicit format/lifecycle coverage.
    false
}

pub(crate) fn classification_is_complete_adapter_coverage(
    classification: &ScriptletClassification,
) -> bool {
    let ScriptletClassification::Known { effects, .. } = classification else {
        return false;
    };

    !effects.is_empty()
        && effects.iter().all(|effect| {
            effect.adapter_id.is_some() && effect.replacement == EffectReplacement::Complete
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::legacy_scriptlets::{EffectConfidence, EffectReplacement, EffectSource};

    #[test]
    fn classification_report_counts_known_unknown_review_and_blocked() {
        let mut report = ScriptletClassificationReport::default();

        report.push(
            "rpm:%post",
            ScriptletClassification::Known {
                reason_code: "known-helper-requires-adapter-coverage".to_string(),
                effects: vec![ScriptletEffectEvidence {
                    kind: "dynamic-linker-cache".to_string(),
                    source: EffectSource::StaticSignal,
                    confidence: EffectConfidence::Inferred,
                    replacement: EffectReplacement::None,
                    adapter_id: Some("ldconfig/v1".to_string()),
                    adapter_digest: Some("sha256:test".to_string()),
                    command: Some("ldconfig".to_string()),
                    args: vec![],
                    path: None,
                    reason_code: Some("known-helper-requires-adapter-coverage".to_string()),
                    extra: BTreeMap::from([(
                        "cache".to_string(),
                        toml::Value::String("ld.so.cache".to_string()),
                    )]),
                }],
            },
        );
        report.push(
            "rpm:%post",
            ScriptletClassification::Unknown {
                reason_code: "unknown-command".to_string(),
                command: "custom-helper".to_string(),
            },
        );
        report.push(
            "deb:config",
            ScriptletClassification::Review {
                reason_code: "review-class-debconf".to_string(),
                class_id: Some("debconf".to_string()),
                command: None,
            },
        );
        report.push(
            "rpm:%post",
            ScriptletClassification::Blocked {
                reason_code: "blocked-class-network".to_string(),
                class_id: "network".to_string(),
                command: None,
            },
        );

        assert_eq!(report.known_count, 1);
        assert_eq!(report.unknown_count, 1);
        assert_eq!(report.review_count, 1);
        assert_eq!(report.blocked_count, 1);
        assert_eq!(report.unsupported_class_counts.get("debconf"), Some(&1));
        assert_eq!(report.unsupported_class_counts.get("network"), Some(&1));
    }
}
