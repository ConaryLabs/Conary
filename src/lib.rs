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

pub mod components;
pub mod container;
pub mod db;
pub mod delta;
pub mod dependencies;
mod error;
pub mod filesystem;
pub mod flavor;
pub mod label;
pub mod packages;
pub mod repository;
pub mod resolver;
pub mod scriptlet;
pub mod trigger;
pub mod version;

pub use components::{ComponentClassifier, ComponentType};
pub use dependencies::{DependencyClass, LanguageDep, LanguageDepDetector};
pub use error::{Error, Result};
pub use flavor::{ArchSpec, FlavorItem, FlavorOp, FlavorSpec, SystemFlavor};
pub use label::{Label, LabelParseError, LabelPath};
