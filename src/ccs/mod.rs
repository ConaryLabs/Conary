// src/ccs/mod.rs
//! CCS (Conary Component Specification) Package Format
//!
//! This module implements the CCS native package format, including:
//! - Manifest parsing (ccs.toml)
//! - Package building
//! - Package inspection and verification

pub mod builder;
pub mod inspector;
pub mod legacy;
pub mod manifest;
pub mod signing;
pub mod verify;

pub use builder::{BuildResult, CcsBuilder, ComponentData, FileEntry, FileType};
pub use inspector::InspectedPackage;
pub use manifest::CcsManifest;
pub use signing::SigningKeyPair;
pub use verify::{TrustPolicy, VerificationResult};
