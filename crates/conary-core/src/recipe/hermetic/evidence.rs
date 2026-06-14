// conary-core/src/recipe/hermetic/evidence.rs

use serde::{Deserialize, Serialize};

use super::divergence::DivergenceReport;

pub const HERMETIC_EVIDENCE_SCHEMA_V1: u32 = 1;
pub const COMMAND_RISK_CLASSIFIER_VERSION: &str = "m2a-command-risk-v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HermeticBuildEvidence {
    pub schema_version: u32,
    pub build_input: BuildInputIdentity,
    pub dependency_lock: DependencyLock,
    pub ecosystem_policy: EcosystemPolicyReport,
    pub command_risk: BuildCommandRiskReport,
    pub reproducibility: ReproducibilityRecord,
    #[serde(default)]
    pub divergence: DivergenceReport,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildInputIdentity {
    pub recipe: RecipeIdentity,
    pub source: SourceIdentity,
    #[serde(default)]
    pub additional_sources: Vec<SourceArchiveIdentity>,
    #[serde(default)]
    pub patches: Vec<InputFileIdentity>,
    #[serde(default)]
    pub local_tree: Option<LocalTreeIdentity>,
    #[serde(default)]
    pub ecosystem_dependencies: Vec<EcosystemDependencyIdentity>,
    pub builder_environment: BuilderEnvironmentIdentity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RecipeIdentity {
    ExplicitRecipe {
        path: String,
        hash: String,
    },
    GeneratedRecipe {
        generator: String,
        canonical_hash: String,
        inference_trace_hash: String,
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
pub struct EcosystemDependencyIdentity {
    pub ecosystem: String,
    pub evidence_path: String,
    pub evidence_hash: String,
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
    #[serde(default)]
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyStatus {
    Clean,
    Review,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EcosystemPolicyReport {
    pub ecosystem: String,
    pub status: PolicyStatus,
    pub identities: Vec<EcosystemDependencyIdentity>,
    pub diagnostics: Vec<String>,
}

impl EcosystemPolicyReport {
    pub fn clean(ecosystem: impl Into<String>) -> Self {
        Self {
            ecosystem: ecosystem.into(),
            status: PolicyStatus::Clean,
            identities: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildCommandRiskReport {
    pub status: PolicyStatus,
    pub classifier_version: String,
    pub entries: Vec<BuildCommandRiskEntry>,
}

impl BuildCommandRiskReport {
    pub fn clean() -> Self {
        Self {
            status: PolicyStatus::Clean,
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
    pub severity: PolicyStatus,
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
            schema_version: HERMETIC_EVIDENCE_SCHEMA_V1,
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
                ecosystem_dependencies: vec![],
                builder_environment: BuilderEnvironmentIdentity {
                    kind: BuilderEnvironmentKind::Pristine,
                    sysroot_hash: Some("sha256:sysroot".to_string()),
                    toolchain_hash: None,
                    diagnostics: vec![],
                },
            },
            dependency_lock: DependencyLock::default(),
            ecosystem_policy: EcosystemPolicyReport::clean("cargo"),
            command_risk: BuildCommandRiskReport::clean(),
            reproducibility: ReproducibilityRecord {
                source_date_epoch: Some(1),
                path_remap_count: 1,
                env_keys: vec!["SOURCE_DATE_EPOCH".to_string()],
            },
            divergence: DivergenceReport::default(),
            diagnostics: vec![],
        };

        let json = serde_json::to_value(&evidence).unwrap();

        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["build_input"]["source"]["kind"], "archive");
        assert_eq!(json["ecosystem_policy"]["status"], "clean");
        assert_eq!(json["command_risk"]["status"], "clean");
        assert_eq!(json["divergence"]["status"], "no-host-record");
    }

    #[test]
    fn hermetic_evidence_defaults_missing_divergence_for_older_evidence() {
        let json = serde_json::json!({
            "schema_version": HERMETIC_EVIDENCE_SCHEMA_V1,
            "build_input": {
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
            },
            "dependency_lock": {
                "repository_dependencies": []
            },
            "ecosystem_policy": {
                "ecosystem": "unknown",
                "status": "clean",
                "identities": [],
                "diagnostics": []
            },
            "command_risk": {
                "status": "clean",
                "classifier_version": COMMAND_RISK_CLASSIFIER_VERSION,
                "entries": []
            },
            "reproducibility": {
                "source_date_epoch": null,
                "path_remap_count": 0,
                "env_keys": []
            },
            "diagnostics": []
        });

        let evidence: HermeticBuildEvidence = serde_json::from_value(json).unwrap();

        assert_eq!(
            evidence.divergence.status,
            super::super::divergence::DivergenceStatus::NoHostRecord
        );
    }

    #[test]
    fn build_input_identity_defaults_omitted_optional_inputs() {
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

        let input: BuildInputIdentity = serde_json::from_value(json).unwrap();

        assert!(input.additional_sources.is_empty());
        assert!(input.patches.is_empty());
        assert_eq!(input.local_tree, None);
        assert!(input.ecosystem_dependencies.is_empty());
    }

    #[test]
    fn public_api_types_match_task2_contract() {
        fn assert_copy<T: Copy>() {}

        assert_copy::<LocalTreeMode>();
        assert_copy::<BuilderEnvironmentKind>();
        assert_copy::<PolicyStatus>();

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
