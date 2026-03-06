// conary-core/src/canonical/mod.rs

//! Canonical package mapping: cross-distro name resolution and auto-discovery.
//!
//! This module provides:
//! - A YAML-based rules engine for mapping distro package names to canonical names
//!   (Repology-compatible format)
//! - Multi-strategy auto-discovery that groups packages across distros by name,
//!   provides, binary paths, sonames, and stem matching

pub mod discovery;
pub mod rules;
