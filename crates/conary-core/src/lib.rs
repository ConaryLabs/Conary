// crates/conary-core/src/lib.rs

//! Conary Core Library
//!
//! Shared types, database, package parsing, and filesystem operations
//! used by the Conary workspace apps.

pub mod automation;
pub mod bootstrap;
pub mod canonical;
pub mod capability;
pub mod ccs;
mod child_wait;
pub mod components;
pub mod compression;
pub mod container;
pub mod db;
pub mod delta;
pub mod dependencies;
pub mod derivation;
pub mod derived;
mod error;
pub mod federation_discovery;
pub mod filesystem;
pub mod flavor;
pub mod generation;
pub mod hash;
pub mod image;
pub mod json;
pub mod label;
pub mod model;
pub mod operations;
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

pub use automation::{AiSuggestion, AutomationManager, AutomationSummary, PendingAction};
pub use bootstrap::{
    Bootstrap, BootstrapConfig, BootstrapStage, Prerequisites, StageManager, TargetArch, Toolchain,
    ToolchainKind,
};
pub use capability::enforcement::{EnforcementMode, EnforcementPolicy};
pub use capability::{CapabilityDeclaration, SyscallCapabilities};
pub use components::{ComponentClassifier, ComponentType};
pub use dependencies::{DependencyClass, LanguageDep, LanguageDepDetector};
pub use error::{Error, Result};
pub use flavor::ArchSpec;
pub use hash::{Hash, Hasher};
pub use label::Label;
pub use model::parser::{
    AiAssistMode, AutomationCategory, AutomationConfig, AutomationMode, FederationConfig,
};
pub use model::{
    ApplyOptions, DEFAULT_MODEL_PATH, DiffAction, ModelDiff, SystemModel, SystemState,
    compute_diff, compute_diff_with_includes_offline, load_model, model_exists, snapshot_to_model,
};
pub use operations::OperationKind;
pub use progress::{MultiProgress, ProgressStyle};
pub use provenance::{Provenance, Signature};
pub use recipe::{Cook, Kitchen, KitchenConfig, Recipe};
pub use transaction::{TransactionConfig, TransactionEngine};
