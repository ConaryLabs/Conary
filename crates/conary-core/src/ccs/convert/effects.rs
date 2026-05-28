// conary-core/src/ccs/convert/effects.rs

use crate::ccs::legacy_scriptlets::{EffectConfidence, EffectReplacement, EffectSource};
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
    },
    Blocked {
        reason_code: String,
        class_id: String,
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
            },
        );
        report.push(
            "rpm:%post",
            ScriptletClassification::Blocked {
                reason_code: "blocked-class-network".to_string(),
                class_id: "network".to_string(),
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
