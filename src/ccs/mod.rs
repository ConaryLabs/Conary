// src/ccs/mod.rs
//! CCS (Conary Component Specification) Package Format
//!
//! This module implements the CCS native package format, including:
//! - Manifest parsing (ccs.toml)
//! - Binary manifest (CBOR-encoded with Merkle root)
//! - Package building
//! - Package inspection and verification
//! - Package installation (via PackageFormat trait)
//! - Declarative hook execution

pub mod binary_manifest;
pub mod builder;
pub mod chunking;
pub mod export;
pub mod hooks;
pub mod inspector;
pub mod legacy;
pub mod manifest;
pub mod package;
pub mod policy;
pub mod signing;
pub mod verify;

pub use binary_manifest::{BinaryManifest, ComponentRef, Hash, MerkleTree};
pub use builder::{BuildResult, CcsBuilder, ChunkStats, ComponentData, FileEntry, FileType};
pub use chunking::{Chunk, ChunkedFile, Chunker, ChunkStore, DeltaStats, StoreStats};
pub use hooks::{AppliedHook, HookExecutor};
pub use inspector::InspectedPackage;
pub use manifest::CcsManifest;
pub use package::CcsPackage;
pub use signing::SigningKeyPair;
pub use policy::{BuildPolicy, BuildPolicyConfig, PolicyAction, PolicyChain};
pub use verify::{TrustPolicy, VerificationResult};
