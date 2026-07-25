// conary-core/src/recipe/hermetic/evidence.rs

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use super::divergence::DivergenceReport;
use crate::security::command_risk::CommandRiskSeverity;

pub const HERMETIC_EVIDENCE_SCHEMA: HermeticEvidenceSchema = HermeticEvidenceSchema;
pub const COMMAND_RISK_CLASSIFIER_VERSION: &str =
    crate::security::command_risk::COMMAND_RISK_CLASSIFIER_VERSION;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HermeticEvidenceSchema;

impl Serialize for HermeticEvidenceSchema {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u32(2)
    }
}

impl<'de> Deserialize<'de> for HermeticEvidenceSchema {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match u32::deserialize(deserializer)? {
            2 => Ok(Self),
            version => Err(de::Error::custom(format!(
                "unsupported hermetic evidence schema {version}; expected 2"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HermeticBuildEvidence {
    pub schema_version: HermeticEvidenceSchema,
    pub build_input: BuildInputIdentity,
    pub dependency_lock: DependencyLock,
    pub command_risk: BuildCommandRiskReport,
    pub reproducibility: ReproducibilityRecord,
    pub divergence: DivergenceReport,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildInputIdentity {
    pub recipe: RecipeIdentity,
    pub source: SourceIdentity,
    pub additional_sources: Vec<SourceArchiveIdentity>,
    pub patches: Vec<InputFileIdentity>,
    pub local_tree: Option<LocalTreeIdentity>,
    pub builder_environment: BuilderEnvironmentIdentity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RecipeIdentity {
    ExplicitRecipe {
        path: String,
        hash: String,
    },
    ForeignPackageConversion {
        format: String,
        contract_hash: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SourceIdentity {
    Archive {
        url: String,
        checksum: String,
    },
    Git {
        original: String,
        commit: String,
    },
    LocalTree {
        root_display: String,
        tree_hash: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceArchiveIdentity {
    pub url: String,
    pub checksum: String,
    pub extracted: bool,
    pub target: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalTreeIdentity {
    pub tree_hash: String,
    pub file_count: usize,
    pub mode: LocalTreeMode,
    pub dirty: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LocalTreeMode {
    GitTracked,
    FilesystemWalk,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InputFileIdentity {
    pub path: String,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuilderEnvironmentIdentity {
    pub kind: BuilderEnvironmentKind,
    pub sysroot_hash: Option<String>,
    pub toolchain_hash: Option<String>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BuilderEnvironmentKind {
    Pristine,
    HostMounted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DependencyLock {
    pub repository_dependencies: Vec<LockedRepositoryDependency>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedRepositoryDependency {
    pub repository_url: String,
    pub snapshot_version: String,
    pub package: String,
    pub version: String,
    pub release: String,
    pub architecture: Option<String>,
    pub content_identity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildCommandRiskReport {
    /// Highest diagnostic severity recorded by the classifier.
    ///
    /// This field is evidence only and never grants or denies build,
    /// execution, mutation, trust, or publication.
    pub highest_severity: CommandRiskSeverity,
    pub classifier_version: String,
    pub entries: Vec<BuildCommandRiskEntry>,
}

impl BuildCommandRiskReport {
    pub fn no_findings() -> Self {
        Self {
            highest_severity: CommandRiskSeverity::None,
            classifier_version: COMMAND_RISK_CLASSIFIER_VERSION.to_string(),
            entries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildCommandRiskEntry {
    pub phase: String,
    pub command: String,
    pub reason_code: String,
    pub severity: CommandRiskSeverity,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReproducibilityRecord {
    pub source_date_epoch: Option<i64>,
    pub path_remap_count: usize,
    pub env_keys: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hermetic_evidence_serializes_stable_schema_version() {
        let evidence = HermeticBuildEvidence {
            schema_version: HERMETIC_EVIDENCE_SCHEMA,
            build_input: BuildInputIdentity {
                recipe: RecipeIdentity::ExplicitRecipe {
                    path: "recipe.toml".to_string(),
                    hash: "sha256:recipe".to_string(),
                },
                source: SourceIdentity::Archive {
                    url: "https://example.invalid/pkg.tar.gz".to_string(),
                    checksum: "sha256:source".to_string(),
                },
                additional_sources: vec![],
                patches: vec![],
                local_tree: None,
                builder_environment: BuilderEnvironmentIdentity {
                    kind: BuilderEnvironmentKind::Pristine,
                    sysroot_hash: Some("sha256:sysroot".to_string()),
                    toolchain_hash: None,
                    diagnostics: vec![],
                },
            },
            dependency_lock: DependencyLock::default(),
            command_risk: BuildCommandRiskReport::no_findings(),
            reproducibility: ReproducibilityRecord {
                source_date_epoch: Some(1),
                path_remap_count: 1,
                env_keys: vec!["SOURCE_DATE_EPOCH".to_string()],
            },
            divergence: DivergenceReport::default(),
            diagnostics: vec![],
        };

        let json = serde_json::to_value(&evidence).unwrap();

        assert_eq!(json["schema_version"], 2);
        assert_eq!(json["build_input"]["source"]["kind"], "archive");
        assert_eq!(json["command_risk"]["highest_severity"], "none");
        assert_eq!(json["divergence"]["status"], "no-host-record");
    }

    #[test]
    fn hermetic_evidence_rejects_obsolete_schema() {
        let error =
            serde_json::from_value::<HermeticEvidenceSchema>(serde_json::json!(1)).unwrap_err();
        assert!(error.to_string().contains("expected 2"));
    }

    #[test]
    fn schema_two_rejects_removed_generated_recipe_identity() {
        let error = serde_json::from_value::<RecipeIdentity>(serde_json::json!({
            "kind": "generated-recipe",
            "generator": "conary-recipe-inference",
            "canonical_hash": "sha256:recipe",
            "inference_trace_hash": "sha256:trace"
        }))
        .unwrap_err();

        assert!(error.to_string().contains("unknown variant"));
    }

    #[test]
    fn build_input_identity_rejects_omitted_exact_inputs() {
        let json = serde_json::json!({
            "recipe": {
                "kind": "explicit-recipe",
                "path": "recipe.toml",
                "hash": "sha256:recipe"
            },
            "source": {
                "kind": "archive",
                "url": "https://example.invalid/pkg.tar.gz",
                "checksum": "sha256:source"
            },
            "builder_environment": {
                "kind": "pristine",
                "sysroot_hash": "sha256:sysroot",
                "toolchain_hash": null,
                "diagnostics": []
            }
        });

        let error = serde_json::from_value::<BuildInputIdentity>(json).unwrap_err();
        assert!(error.to_string().contains("additional_sources"));
    }

    #[test]
    fn public_api_types_match_task2_contract() {
        fn assert_copy<T: Copy>() {}

        assert_copy::<LocalTreeMode>();
        assert_copy::<BuilderEnvironmentKind>();
        assert_copy::<HermeticEvidenceSchema>();
        assert_copy::<CommandRiskSeverity>();

        let local_tree = LocalTreeIdentity {
            tree_hash: "sha256:tree".to_string(),
            file_count: 3usize,
            mode: LocalTreeMode::GitTracked,
            dirty: false,
            warnings: vec![],
        };
        let file_count: usize = local_tree.file_count;
        assert_eq!(file_count, 3);

        let dependency = LockedRepositoryDependency {
            repository_url: "https://repo.example.invalid".to_string(),
            snapshot_version: "2026-06-14".to_string(),
            package: "pkg".to_string(),
            version: "1.0".to_string(),
            release: "1".to_string(),
            architecture: None,
            content_identity: "sha256:pkg".to_string(),
        };
        assert_eq!(dependency.architecture, None);

        let reproducibility = ReproducibilityRecord {
            source_date_epoch: Some(-1),
            path_remap_count: 2usize,
            env_keys: vec![],
        };
        let source_date_epoch: Option<i64> = reproducibility.source_date_epoch;
        let path_remap_count: usize = reproducibility.path_remap_count;
        assert_eq!(source_date_epoch, Some(-1));
        assert_eq!(path_remap_count, 2);
    }
}
