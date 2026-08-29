// conary-core/src/filesystem/mod.rs

//! Filesystem operations for Conary
//!
//! This module provides:
//! - Content-addressable storage (CAS) for files, similar to git's object storage
//! - Virtual filesystem (VFS) tree for building in-memory file hierarchies
//!
//! Files are stored by their SHA-256 hash, enabling deduplication and
//! efficient rollback support. File deployment is handled by composefs-native
//! generation building (see `crate::generation`).

mod cas;
pub mod durable;
pub mod fsverity;
pub mod path;
pub mod selected_root;
pub mod source_path;
pub mod vfs;

pub use cas::{
    CasObjectCollectionSession, CasStore, ExpectedReaderStoreMetrics, PrivateCasWriter,
    PrivateCopyBatch, VerifiedObjectBatch, VerifiedObjectBatchMetrics, VerifiedObjectDisposition,
    VerifiedObjectSet, is_temporary_object_name, object_path,
};
pub(crate) use cas::{CasObjectLivenessLease, EphemeralObjectStore};
pub use path::{safe_join, sanitize_filename, sanitize_path};
pub use source_path::{DeploymentPath, SourcePathBytes};
pub use vfs::{NodeId, NodeKind, VfsNode, VfsStats, VfsTree};
