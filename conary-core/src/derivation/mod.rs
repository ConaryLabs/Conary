// conary-core/src/derivation/mod.rs

//! Derivation data model for the CAS-layered bootstrap build system.
//!
//! A **derivation** is a content-addressed build specification: given a set of
//! inputs (source hash, build script, dependencies, environment, target), it
//! produces a deterministic `DerivationId`. Two builds with identical inputs
//! will always produce the same ID, enabling build caching and verification.

pub mod id;
pub mod output;

pub use id::{DerivationId, DerivationInputs, SourceDerivationId};
pub use output::{OutputFile, OutputManifest, OutputSymlink, PackageOutput};
