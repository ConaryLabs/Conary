// conary-core/src/ccs/convert/mod.rs
//! Foreign package to CCS conversion.
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
//! ## Scriptlet Handling: Typed Lifecycle Authority
//!
//! 1. Preserve the source package manager's typed lifecycle ABI and exact body.
//! 2. Execute the preserved source ABI at transaction-planned boundaries.
//! 3. Capture supported external lifecycle commands at the execution boundary.

pub mod command_evidence;
mod converter;
mod input;
pub mod native_provenance;
pub mod payload_hints;
pub mod scriptlet_bundle;

pub use converter::{ConversionOptions, ConversionResult, NativePackageConverter};
pub use input::ForeignConversionInput;
pub use native_provenance::{NativeProvenance, NativeSignatureEvidence};
pub use scriptlet_bundle::{
    ScriptletBundleBuild, ScriptletBundleInput, ScriptletBundleSummary,
    build_direct_native_lifecycle_bundle, build_native_lifecycle_bundle,
};
