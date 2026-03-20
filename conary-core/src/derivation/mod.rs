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
pub mod id;
pub mod index;
pub mod output;
pub mod recipe_hash;

pub use capture::{capture_output, CaptureError};
pub use compose::{compose_erofs, compose_file_entries, erofs_image_hash, ComposeError};
pub use environment::{BuildEnvironment, EnvironmentError};
pub use id::{DerivationId, DerivationInputs, SourceDerivationId};
pub use index::{DerivationIndex, DerivationRecord};
pub use output::{OutputFile, OutputManifest, OutputSymlink, PackageOutput};
pub use recipe_hash::{build_script_hash, expand_variables, source_hash};
