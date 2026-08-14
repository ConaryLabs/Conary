// conary-test/src/config/corpus.rs

//! Persisted declarations for attributable just-works corpus cases.

use anyhow::{Result, bail};
use conary_core::corpus::{ConversionStage, SourceArtifactDigestSource};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Component, Path};

/// Exact native source package format exercised by a corpus case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CorpusSourceFormat {
    Rpm,
    Deb,
    Alpm,
}

impl CorpusSourceFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rpm => "rpm",
            Self::Deb => "deb",
            Self::Alpm => "alpm",
        }
    }
}

/// Target facts that must accompany every corpus result.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusTargetDef {
    pub architecture: String,
    pub init_system: String,
    pub capabilities: Vec<String>,
}

/// Manifest-owned authority for one attributable corpus case.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusCaseDef {
    /// Absolute in-container path of the versioned runtime evidence envelope.
    pub evidence_path: String,
    pub source_profile: String,
    pub source_format: CorpusSourceFormat,
    pub digest_source: SourceArtifactDigestSource,
    pub target: CorpusTargetDef,
    /// Declared journey stages in their canonical execution order.
    pub stages: Vec<ConversionStage>,
}

impl CorpusCaseDef {
    pub fn validate(&self, test_id: &str) -> Result<()> {
        let context = || format!("test {test_id} corpus declaration");
        let evidence = Path::new(&self.evidence_path);
        if !evidence.is_absolute()
            || evidence
                .components()
                .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        {
            bail!(
                "{}: evidence_path must be an absolute normalized path",
                context()
            );
        }
        if self.source_profile.trim().is_empty() {
            bail!("{}: source_profile must not be empty", context());
        }
        if self.target.architecture.trim().is_empty()
            || self.target.init_system.trim().is_empty()
            || self.target.capabilities.is_empty()
            || self
                .target
                .capabilities
                .iter()
                .any(|capability| capability.trim().is_empty())
        {
            bail!(
                "{}: target architecture, init_system, and capabilities are required",
                context()
            );
        }
        if self.stages.is_empty() {
            bail!("{}: at least one journey stage is required", context());
        }
        if self.stages.iter().copied().collect::<HashSet<_>>().len() != self.stages.len() {
            bail!("{}: journey stages must be unique", context());
        }
        if !self.stages.windows(2).all(|stages| stages[0] < stages[1]) {
            bail!(
                "{}: journey stages must follow canonical execution order",
                context()
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_case() -> CorpusCaseDef {
        CorpusCaseDef {
            evidence_path: "/tmp/conary-corpus-rpm.json".into(),
            source_profile: "fedora-44".into(),
            source_format: CorpusSourceFormat::Rpm,
            digest_source: SourceArtifactDigestSource::FixtureBuildManifest,
            target: CorpusTargetDef {
                architecture: "x86_64".into(),
                init_system: "systemd".into(),
                capabilities: vec!["native_lifecycle".into()],
            },
            stages: vec![
                ConversionStage::Installation,
                ConversionStage::Update,
                ConversionStage::Rollback,
                ConversionStage::Removal,
            ],
        }
    }

    #[test]
    fn complete_case_is_valid() {
        assert!(valid_case().validate("TC01").is_ok());
    }

    #[test]
    fn missing_attribution_is_rejected() {
        let mut case = valid_case();
        case.source_profile.clear();
        assert!(case.validate("TC01").is_err());

        let mut case = valid_case();
        case.target.capabilities.clear();
        assert!(case.validate("TC01").is_err());
    }

    #[test]
    fn missing_digest_source_is_rejected_during_deserialization() {
        let declaration = r#"
evidence_path = "/tmp/corpus.json"
source_profile = "fedora-44"
source_format = "rpm"
stages = ["installation"]

[target]
architecture = "x86_64"
init_system = "systemd"
capabilities = ["native_lifecycle"]
"#;
        assert!(toml::from_str::<CorpusCaseDef>(declaration).is_err());
    }

    #[test]
    fn relative_or_traversing_evidence_path_is_rejected() {
        for path in ["corpus.json", "/tmp/../corpus.json"] {
            let mut case = valid_case();
            case.evidence_path = path.into();
            assert!(case.validate("TC01").is_err());
        }
    }

    #[test]
    fn duplicate_stages_are_rejected() {
        let mut case = valid_case();
        case.stages = vec![ConversionStage::Installation, ConversionStage::Installation];
        assert!(case.validate("TC01").is_err());
    }

    #[test]
    fn out_of_order_stages_are_rejected() {
        let mut case = valid_case();
        case.stages = vec![ConversionStage::Rollback, ConversionStage::Update];
        assert!(case.validate("TC01").is_err());
    }
}
