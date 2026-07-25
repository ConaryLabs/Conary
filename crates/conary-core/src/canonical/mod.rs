// conary-core/src/canonical/mod.rs

//! Canonical package mapping: cross-distro name resolution from explicit sources.
//!
//! This module provides:
//! - A YAML-based rules engine for mapping distro package names to canonical names
//!   (Repology-compatible format)
//! - Repology and AppStream contract ingestion
//!
//! Payload paths, package-name similarity, and inferred capabilities are not
//! mapping authority.

pub mod appstream;
pub mod client;
pub mod repology;
pub mod rules;
