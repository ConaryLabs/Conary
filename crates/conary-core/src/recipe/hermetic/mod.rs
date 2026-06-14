// conary-core/src/recipe/hermetic/mod.rs

pub mod ecosystem;
pub mod evidence;
pub mod source_identity;

pub use ecosystem::evaluate_ecosystem_policy;
pub use evidence::{
    BuildCommandRiskEntry, BuildCommandRiskReport, BuildInputIdentity, BuilderEnvironmentIdentity,
    BuilderEnvironmentKind, COMMAND_RISK_CLASSIFIER_VERSION, DependencyLock,
    EcosystemDependencyIdentity, EcosystemPolicyReport, HERMETIC_EVIDENCE_SCHEMA_V1,
    HermeticBuildEvidence, InputFileIdentity, LocalTreeIdentity, LocalTreeMode, PolicyStatus,
    RecipeIdentity, ReproducibilityRecord, SourceArchiveIdentity, SourceIdentity,
};
pub use source_identity::{
    CanonicalLocalFile, CanonicalLocalFileKind, CiMode, canonical_local_file_list, detect_ci_mode,
    local_tree_identity,
};
