// conary-core/src/derivation/mod.rs

//! Derivation data model for the CAS-layered bootstrap build system.
//!
//! A **derivation** is a content-addressed build specification: given a set of
//! inputs (source hash, build script, dependencies, environment, target), it
//! produces a deterministic `DerivationId`. Two builds with identical inputs
//! will always produce the same ID, enabling build caching and verification.

pub mod capture;
pub mod compose;
pub mod environment;
pub mod executor;
pub mod id;
pub mod index;
pub mod manifest;
pub mod output;
pub mod pipeline;
pub mod profile;
pub mod recipe_hash;
pub mod seed;
pub mod build_order;
pub mod install;
pub mod stages;
pub mod substituter;
#[cfg(test)]
pub(crate) mod test_helpers;

pub use capture::{CaptureError, capture_output};
pub use compose::{ComposeError, compose_erofs, compose_file_entries, erofs_image_hash};
pub use environment::{BuildEnvironment, EnvironmentError};
pub use executor::{DerivationExecutor, ExecutionResult, ExecutorError};
pub use id::{DerivationError, DerivationId, DerivationInputs, SourceDerivationId};
pub use index::{DerivationIndex, DerivationRecord};
pub use manifest::{ManifestError, SystemManifest};
pub use output::{OutputFile, OutputManifest, OutputSymlink, PackageOutput};
pub use pipeline::{Pipeline, PipelineConfig, PipelineError, PipelineEvent};
pub use profile::{
    BuildProfile, ProfileDerivation, ProfileDiff, ProfileMetadata, ProfileSeedRef, ProfileStage,
};
pub use recipe_hash::{build_script_hash, expand_variables, source_hash};
pub use seed::{Seed, SeedError, SeedMetadata, SeedSource};
pub use build_order::{BuildOrderError, BuildPhase, BuildStep, compute_build_order};
pub use install::{InstallError, install_to_sysroot};
pub use stages::{Stage, StageAssignment, StageError, assign_stages};
