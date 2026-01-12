// src/packages/mod.rs

//! Package format support for Conary
//!
//! This module provides parsers and utilities for various package formats
//! (RPM, DEB, Arch). Each format implements the `PackageFormat` trait.

pub mod arch;
pub mod deb;
pub mod rpm;
pub mod rpm_query;
pub mod traits;

pub use rpm_query::{InstalledFileInfo, InstalledRpmInfo};
pub use traits::PackageFormat;
