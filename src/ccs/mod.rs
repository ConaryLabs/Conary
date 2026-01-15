// src/ccs/mod.rs
//! CCS (Conary Component Specification) Package Format
//!
//! This module implements the CCS native package format, including:
//! - Manifest parsing (ccs.toml)
//! - Package building
//! - Package inspection and verification
//! - Package installation (via PackageFormat trait)
//! - Declarative hook execution

pub mod builder;
pub mod hooks;
pub mod inspector;
pub mod legacy;
pub mod manifest;
pub mod package;
pub mod signing;
pub mod verify;

pub use builder::{BuildResult, CcsBuilder, ComponentData, FileEntry, FileType};
pub use hooks::{AppliedHook, HookExecutor};
pub use inspector::InspectedPackage;
pub use manifest::CcsManifest;
pub use package::CcsPackage;
pub use signing::SigningKeyPair;
pub use verify::{TrustPolicy, VerificationResult};
