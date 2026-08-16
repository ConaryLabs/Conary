// conary-test/src/config/corpus.rs

//! Persisted declarations for attributable just-works corpus cases.

use anyhow::{Result, bail};
use conary_core::corpus::{
    ConversionStage, CorpusSemantic, SourceArtifactDigestSource, SourceArtifactRole,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashSet;
use std::path::{Component, Path};

/// Exact native source package format exercised by a corpus case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorpusSourceFormat {
    Rpm,
    Deb,
    Alpm,
    Template(String),
}

impl CorpusSourceFormat {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Rpm => "rpm",
            Self::Deb => "deb",
            Self::Alpm => "alpm",
            Self::Template(value) => value,
        }
    }

    pub(crate) fn from_value(value: impl Into<String>) -> Self {
        let value = value.into();
        match value.as_str() {
            "rpm" => Self::Rpm,
            "deb" => Self::Deb,
            "alpm" => Self::Alpm,
            _ => Self::Template(value),
        }
    }

    fn is_exact(&self) -> bool {
        !matches!(self, Self::Template(_))
    }

    fn is_variable(&self) -> bool {
        matches!(self, Self::Template(value) if value.starts_with("${")
            && value.ends_with('}')
            && value.len() > 3
            && !value[2..value.len() - 1]
                .chars()
                .any(|character| matches!(character, '$' | '{' | '}')))
    }
}

impl Serialize for CorpusSourceFormat {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CorpusSourceFormat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self::from_value(String::deserialize(deserializer)?))
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

/// Suite-level semantic properties that its cases must collectively claim.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusSuiteDef {
    pub required: Vec<CorpusSemantic>,
}

impl CorpusSuiteDef {
    pub fn validate(&self) -> Result<()> {
        if self.required.is_empty() {
            bail!("suite corpus required coverage must not be empty");
        }
        if !self.required.windows(2).all(|items| items[0] < items[1]) {
            bail!("suite corpus required coverage must be unique and canonically ordered");
        }
        Ok(())
    }
}

/// One persisted semantic claim bound to exact runtime artifact roles.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusCoverageClaimDef {
    pub semantic: CorpusSemantic,
    pub artifact_roles: Vec<SourceArtifactRole>,
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
    pub coverage: Vec<CorpusCoverageClaimDef>,
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
        if !self.source_format.is_exact() && !self.source_format.is_variable() {
            bail!(
                "{}: source_format must be rpm, deb, alpm, or one exact variable reference",
                context()
            );
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
        if self.coverage.is_empty() {
            bail!(
                "{}: at least one semantic coverage claim is required",
                context()
            );
        }
        if !self
            .coverage
            .windows(2)
            .all(|claims| claims[0].semantic < claims[1].semantic)
        {
            bail!(
                "{}: semantic coverage claims must be unique and canonically ordered",
                context()
            );
        }
        for claim in &self.coverage {
            if claim.artifact_roles.is_empty() {
                bail!("{}: coverage artifact_roles must not be empty", context());
            }
            if !claim
                .artifact_roles
                .windows(2)
                .all(|roles| roles[0] < roles[1])
            {
                bail!(
                    "{}: coverage artifact_roles must be unique and canonically ordered",
                    context()
                );
            }
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
            coverage: vec![CorpusCoverageClaimDef {
                semantic: CorpusSemantic::IdentityExactVersion,
                artifact_roles: vec![SourceArtifactRole::InstallRequest],
            }],
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
    fn source_format_accepts_exact_values_or_one_variable() {
        let mut case = valid_case();
        case.source_format = CorpusSourceFormat::from_value("${native_source_format}");
        assert!(case.validate("TC01").is_ok());

        for invalid in ["arch", "${one}-${two}", "${}", "rpm-${target}"] {
            let mut case = valid_case();
            case.source_format = CorpusSourceFormat::from_value(invalid);
            assert!(case.validate("TC01").is_err(), "accepted {invalid}");
        }
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

    #[test]
    fn empty_or_duplicate_coverage_is_rejected() {
        let mut case = valid_case();
        case.coverage.clear();
        assert!(case.validate("TC01").is_err());

        let mut case = valid_case();
        case.coverage.push(case.coverage[0].clone());
        assert!(case.validate("TC01").is_err());

        let mut case = valid_case();
        case.coverage[0]
            .artifact_roles
            .push(SourceArtifactRole::InstallRequest);
        assert!(case.validate("TC01").is_err());
    }

    #[test]
    fn suite_requirements_are_nonempty_and_unique() {
        assert!(
            CorpusSuiteDef {
                required: vec![CorpusSemantic::IdentityExactVersion]
            }
            .validate()
            .is_ok()
        );
        assert!(CorpusSuiteDef { required: vec![] }.validate().is_err());
        assert!(
            CorpusSuiteDef {
                required: vec![
                    CorpusSemantic::IdentityExactVersion,
                    CorpusSemantic::IdentityExactVersion,
                ]
            }
            .validate()
            .is_err()
        );
    }
}
