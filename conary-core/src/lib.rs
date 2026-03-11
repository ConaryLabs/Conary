// conary-core/src/lib.rs

//! Conary Core Library
//!
//! Shared types, database, package parsing, and filesystem operations
//! used by both the CLI client and the Remi server.

pub mod automation;
pub mod bootstrap;
pub mod canonical;
pub mod capability;
pub mod ccs;
pub mod components;
pub mod compression;
pub mod container;
pub mod db;
pub mod delta;
pub mod dependencies;
pub mod derived;
mod error;
pub mod filesystem;
pub mod flavor;
pub mod hash;
pub mod label;
pub mod model;
pub mod packages;
pub mod progress;
pub mod provenance;
pub mod recipe;
pub mod repository;
pub mod resolver;
pub mod scriptlet;
pub mod self_update;
pub mod transaction;
pub mod trigger;
pub mod trust;
pub mod util;
pub mod version;

pub use automation::{
    ActionDecision, ActionStatus, AiSuggestion, AutomationManager, AutomationSummary, PendingAction,
};
pub use bootstrap::{
    Bootstrap, BootstrapConfig, BootstrapStage, Prerequisites, Stage0Builder, StageManager,
    TargetArch, Toolchain, ToolchainKind,
};
pub use capability::enforcement::{
    EnforcementError, EnforcementMode, EnforcementPolicy, EnforcementReport, EnforcementSupport,
    EnforcementWarning,
};
pub use capability::{
    CapabilityDeclaration, CapabilityError, FilesystemCapabilities, NetworkCapabilities,
    SyscallCapabilities, SyscallProfile,
};
pub use components::{ComponentClassifier, ComponentType};
pub use dependencies::{DependencyClass, LanguageDep, LanguageDepDetector};
pub use error::{Error, Result};
pub use flavor::{ArchSpec, FlavorItem, FlavorOp, FlavorSpec, SystemFlavor};
pub use hash::{Hash, HashAlgorithm, Hasher};
pub use label::{Label, LabelParseError, LabelPath};
pub use model::parser::{
    AiAssistConfig, AiAssistMode, AiFeature, AutomationCategory, AutomationConfig, AutomationMode,
    FederationConfig, FederationTier, RepairAutomation, RollbackTrigger, SecurityAutomation,
};
pub use model::{
    ApplyOptions, DEFAULT_MODEL_PATH, DiffAction, ModelConfig, ModelDiff, ModelError, SystemModel,
    SystemState, compute_diff, compute_diff_with_includes_offline, load_model, model_exists,
    snapshot_to_model,
};
pub use progress::{
    CallbackProgress, LogProgress, MultiProgress, ProgressEvent, ProgressStyle, ProgressTracker,
    SilentProgress,
};
pub use provenance::{
    BuildDependency, BuildProvenance, ComponentHash, ContentProvenance, DnaHash, HostAttestation,
    PackageDna, PatchInfo, Provenance, ReproducibilityInfo, Signature, SignatureProvenance,
    SignatureScope, SourceProvenance, TransparencyLog,
};
pub use recipe::{Cook, CookResult, Kitchen, KitchenConfig, Recipe};
pub use transaction::{
    RecoveryOutcome, Transaction, TransactionConfig, TransactionEngine, TransactionPlan,
    TransactionState,
};
pub use trust::TrustError;
