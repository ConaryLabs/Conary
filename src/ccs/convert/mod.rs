// src/ccs/convert/mod.rs
//! Legacy Package to CCS Conversion
//!
//! This module converts foreign packages (RPM/DEB/Arch) to CCS format during
//! installation, enabling CAS deduplication, component selection, and atomic
//! transactions.
//!
//! ## Value Proposition
//!
//! | Benefit | Local Install | Server-Side (Future) |
//! |---------|---------------|---------------------|
//! | Delta updates | No | Yes (~80% savings) |
//! | CAS deduplication | Yes | Yes |
//! | Component selection | Yes (:runtime only) | Yes |
//! | Atomic transactions | Yes | Yes |
//! | Unified verification | Yes | Yes |
//!
//! ## Scriptlet Handling: Idempotent Overlay
//!
//! 1. Extract declarative hooks (users, groups, systemd, etc.)
//! 2. Run declarative hooks first
//! 3. Run original scriptlet as-is (don't modify/strip)
//! 4. Assume scripts are idempotent (standard practice)

mod analyzer;
pub mod capture;
mod converter;
mod fidelity;
pub mod legacy_provenance;
pub mod mock;

pub use analyzer::{DetectedHook, ScriptletAnalyzer};
pub use converter::{ConversionOptions, ConversionResult, LegacyConverter};
pub use fidelity::{FidelityLevel, FidelityReport};
pub use legacy_provenance::LegacyProvenance;
