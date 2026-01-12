// src/filesystem/mod.rs

//! Filesystem operations for Conary
//!
//! This module provides content-addressable storage (CAS) for files,
//! similar to git's object storage. Files are stored by their SHA-256
//! hash, enabling deduplication and efficient rollback support.

mod cas;
mod deployer;

pub use cas::CasStore;
pub use deployer::FileDeployer;
