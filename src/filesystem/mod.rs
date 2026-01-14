// src/filesystem/mod.rs

//! Filesystem operations for Conary
//!
//! This module provides:
//! - Content-addressable storage (CAS) for files, similar to git's object storage
//! - Virtual filesystem (VFS) tree for building in-memory file hierarchies
//! - File deployment from CAS to the actual filesystem
//!
//! Files are stored by their SHA-256 hash, enabling deduplication and
//! efficient rollback support.

mod cas;
mod deployer;
pub mod vfs;

pub use cas::CasStore;
pub use deployer::FileDeployer;
pub use vfs::{NodeId, NodeKind, VfsNode, VfsStats, VfsTree};
