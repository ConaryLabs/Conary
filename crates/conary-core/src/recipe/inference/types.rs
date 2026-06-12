// conary-core/src/recipe/inference/types.rs

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildSystem {
    Cargo,
    #[serde(rename = "cmake")]
    CMake,
    Meson,
    Autotools,
    Npm,
    Python,
    Go,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InferenceOptions {
    pub source_root: PathBuf,
    pub package_name_override: Option<String>,
    pub version_override: Option<String>,
}

impl InferenceOptions {
    pub fn for_source_root(source_root: impl Into<PathBuf>) -> Self {
        Self {
            source_root: source_root.into(),
            package_name_override: None,
            version_override: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InferenceTrace {
    pub events: Vec<InferenceEvent>,
}

impl InferenceTrace {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_detector(
        &mut self,
        detector: impl Into<String>,
        confidence: u8,
        evidence: impl Into<String>,
        detail: impl Into<String>,
    ) {
        self.events.push(InferenceEvent::Detector {
            detector: detector.into(),
            confidence,
            evidence: evidence.into(),
            detail: detail.into(),
        });
    }

    pub fn record_decision(
        &mut self,
        field: impl Into<String>,
        value: impl Into<String>,
        reason: impl Into<String>,
    ) {
        self.events.push(InferenceEvent::Decision {
            field: field.into(),
            value: value.into(),
            reason: reason.into(),
        });
    }

    pub fn warn(&mut self, message: impl Into<String>) {
        self.events.push(InferenceEvent::Warning {
            message: message.into(),
        });
    }

    pub fn render_human(&self) -> String {
        self.events
            .iter()
            .map(|event| match event {
                InferenceEvent::Detector {
                    detector,
                    confidence,
                    evidence,
                    detail,
                } => {
                    format!(
                        "detector {detector}: confidence {confidence}, evidence {evidence} ({detail})"
                    )
                }
                InferenceEvent::Decision {
                    field,
                    value,
                    reason,
                } => {
                    format!("decision {field}: {value} ({reason})")
                }
                InferenceEvent::Warning { message } => format!("warning: {message}"),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InferenceEvent {
    Detector {
        detector: String,
        confidence: u8,
        evidence: String,
        detail: String,
    },
    Decision {
        field: String,
        value: String,
        reason: String,
    },
    Warning {
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct InferenceResult {
    pub build_system: BuildSystem,
    pub recipe: crate::recipe::format::Recipe,
    pub trace: InferenceTrace,
    pub source_root: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_system_serializes_canonical_names() {
        let cases = [
            (BuildSystem::Cargo, "cargo"),
            (BuildSystem::CMake, "cmake"),
            (BuildSystem::Meson, "meson"),
            (BuildSystem::Autotools, "autotools"),
            (BuildSystem::Npm, "npm"),
            (BuildSystem::Python, "python"),
            (BuildSystem::Go, "go"),
        ];

        for (build_system, expected) in cases {
            assert_eq!(
                serde_json::to_value(build_system).unwrap(),
                serde_json::json!(expected)
            );
        }
    }

    #[test]
    fn inference_trace_serializes_decisions() {
        let mut trace = InferenceTrace::new();
        trace.record_detector("cargo", 100, "Cargo.toml", "found [package] name/version");
        trace.record_decision("build-system", "cargo", "highest-confidence detector");
        trace.warn("optional metadata skipped");

        let value = serde_json::to_value(&trace).unwrap();
        assert_eq!(value["events"][0]["kind"], "detector");
        assert_eq!(value["events"][0]["detector"], "cargo");
        assert_eq!(value["events"][0]["confidence"], 100);
        assert_eq!(value["events"][0]["evidence"], "Cargo.toml");
        assert_eq!(value["events"][0]["detail"], "found [package] name/version");
        assert_eq!(value["events"][1]["kind"], "decision");
        assert_eq!(value["events"][1]["field"], "build-system");
        assert_eq!(value["events"][1]["value"], "cargo");
        assert_eq!(value["events"][1]["reason"], "highest-confidence detector");
        assert_eq!(value["events"][2]["kind"], "warning");
        assert_eq!(value["events"][2]["message"], "optional metadata skipped");

        let round_trip: InferenceTrace = serde_json::from_value(value).unwrap();
        assert_eq!(round_trip, trace);

        let json = serde_json::to_string(&trace).unwrap();
        assert!(json.contains("\"detector\":\"cargo\""));
        assert!(json.contains("\"confidence\":100"));

        let rendered = trace.render_human();
        assert!(rendered.contains("cargo"));
        assert!(rendered.contains("Cargo.toml"));
    }
}
