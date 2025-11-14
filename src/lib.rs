// src/lib.rs

//! Conary Package Manager
//!
//! Modern package manager with atomic operations, rollback capabilities,
//! and support for multiple package formats (RPM, DEB, Arch).
//!
//! # Architecture
//!
//! - Database-first: All state in SQLite, no config files
//! - Changesets: Atomic transactional operations
//! - Troves: Hierarchical package units (packages, components, collections)
//! - Flavors: Build-time variations tracked in metadata
//! - File-level tracking: SHA-256 hashes, delta updates, conflict detection

pub mod db;
mod error;
pub mod filesystem;
pub mod packages;
pub mod resolver;
pub mod version;

pub use error::{Error, Result};
