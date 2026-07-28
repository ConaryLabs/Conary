// conary-core/src/ccs/mod.rs
//! CCS (Conary Component Specification) Package Format
//!
//! This module implements the CCS native package format, including:
//! - Manifest parsing (ccs.toml)
//! - Signed v2 authority manifest (canonical CBOR)
//! - Package building
//! - Package inspection and verification
//! - Package installation (via PackageFormat trait)
//! - Declarative hook execution

pub mod archive_layout;
pub mod archive_reader;
pub mod attestation;
pub mod budget;
pub mod builder;
pub mod chunking;
pub mod convert;
pub mod enhancement;
pub mod evidence_normalization;
pub mod export;
pub mod hooks;
pub mod inspector;
pub mod manifest;
pub mod manifest_provenance;
pub mod native_export;
pub mod native_lifecycle;
pub mod native_transaction;
pub mod package;
pub mod policy;
pub mod signing;
pub mod v2;
pub mod verify;

pub use budget::{
    ArchiveDecodeBounds, AuthorityCensus, BudgetDimension, BudgetError, CCS_BUDGET,
    CcsStructuralBudget,
};
pub use builder::{BuildResult, BuilderError, CcsBuilder, ChunkStats, ComponentData, FileEntry};
pub use chunking::{Chunk, ChunkStore, ChunkedFile, Chunker, DeltaStats, StoreStats};
pub use convert::{ConversionOptions, ConversionResult, NativePackageConverter};
pub use enhancement::{
    ENHANCEMENT_VERSION, EnhancementContext, EnhancementEngine, EnhancementError,
    EnhancementRegistry, EnhancementRunner, EnhancementStatus, EnhancementType,
};
pub use hooks::{
    AppliedHook, CapturedCcsActivation, CcsActivationSource, ExecutableInterface,
    HOST_CAPABILITY_INVENTORY_SCHEMA_VERSION, HOST_CAPABILITY_INVENTORY_SETTING,
    HookExecutionResults, HookExecutor, HookResult, HookType, HostCapabilityInventory,
    HostCapabilityInventoryError, HostCapabilityPreflightError, HostCapabilityRequirement,
    HostExecutableContract, HostExecutableImplementation, InitSystemCapability, SystemdInterface,
    SystemdOperation, TmpfilesInterface,
};
pub use inspector::UntrustedPackageInspection;
pub use manifest::CcsManifest;
pub use native_lifecycle::{
    NATIVE_LIFECYCLE_SCHEMA_REVISION, NATIVE_LIFECYCLE_SCHEMA_V1, NativeLifecycleBundle,
    NativeLifecycleEntry, NativeLifecycleEntryKind,
};
pub use package::CcsPackage;
pub use policy::{BuildPolicy, BuildPolicyConfig, PolicyAction, PolicyChain};
pub use signing::SigningKeyPair;
pub use verify::{TrustPolicy, VerifiedCcsArchive};
