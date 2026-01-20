// src/lib.rs

//! Conary Package Manager
//!
//! Modern package manager with atomic operations, rollback capabilities,
//! and support for multiple package formats (RPM, DEB, Arch).
//!
//! # Architecture
//!
//! - Database-first: All state in SQLite, no config files
//! - Changesets: Atomic transactional operations
//! - Troves: Hierarchical package units (packages, components, collections)
//! - Flavors: Build-time variations tracked in metadata
//! - File-level tracking: SHA-256 hashes, delta updates, conflict detection

pub mod bootstrap;
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
pub mod recipe;
pub mod repository;
pub mod resolver;
pub mod scriptlet;
pub mod transaction;
pub mod trigger;
pub mod version;
pub mod automation;
pub mod provenance;

#[cfg(feature = "server")]
pub mod federation;

#[cfg(feature = "server")]
pub mod server;

#[cfg(feature = "daemon")]
pub mod daemon;

pub use components::{ComponentClassifier, ComponentType};
pub use dependencies::{DependencyClass, LanguageDep, LanguageDepDetector};
pub use error::{Error, Result};
pub use flavor::{ArchSpec, FlavorItem, FlavorOp, FlavorSpec, SystemFlavor};
pub use hash::{Hash, HashAlgorithm, Hasher};
pub use label::{Label, LabelParseError, LabelPath};
pub use model::{
    compute_diff, load_model, model_exists, snapshot_to_model, ApplyOptions, DiffAction,
    ModelConfig, ModelDiff, ModelError, SystemModel, SystemState, DEFAULT_MODEL_PATH,
};
pub use model::parser::{
    AiAssistConfig, AiAssistMode, AiFeature, AutomationCategory, AutomationConfig,
    AutomationMode, FederationConfig, FederationTier, RepairAutomation, RollbackTrigger,
    SecurityAutomation,
};
pub use automation::{
    ActionDecision, ActionStatus, AiSuggestion, AutomationManager, AutomationSummary,
    PendingAction,
};
pub use progress::{
    CallbackProgress, LogProgress, MultiProgress, ProgressEvent, ProgressStyle, ProgressTracker,
    SilentProgress,
};
pub use transaction::{
    RecoveryOutcome, Transaction, TransactionConfig, TransactionEngine, TransactionPlan,
    TransactionState,
};
pub use recipe::{Cook, CookResult, Kitchen, KitchenConfig, Recipe};
pub use bootstrap::{
    Bootstrap, BootstrapConfig, BootstrapStage, Prerequisites, Stage0Builder, StageManager,
    TargetArch, Toolchain, ToolchainKind,
};
pub use capability::{
    CapabilityDeclaration, CapabilityError, FilesystemCapabilities, NetworkCapabilities,
    SyscallCapabilities, SyscallProfile,
};
pub use provenance::{
    BuildDependency, BuildProvenance, ComponentHash, ContentProvenance, DnaHash, HostAttestation,
    PackageDna, PatchInfo, Provenance, ReproducibilityInfo, Signature, SignatureProvenance,
    SignatureScope, SourceProvenance, TransparencyLog,
};
#[cfg(feature = "server")]
pub use federation::{
    CircuitBreaker, CircuitBreakerRegistry, CircuitState, Federation, FederatedChunkFetcher,
    FederationStats, Peer, PeerId, PeerRegistry, PeerScore, PeerTier, RendezvousRouter,
    RequestCoalescer,
};
