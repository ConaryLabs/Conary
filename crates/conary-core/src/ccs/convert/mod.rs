// conary-core/src/ccs/convert/mod.rs
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
//! ## Scriptlet Handling: Typed Lifecycle Evidence
//!
//! 1. Parse shell scriptlets with a formal grammar.
//! 2. Dispatch exact native and helper contracts to typed adapters.
//! 3. Promote only complete adapter effects into declarative manifest hooks.
//! 4. Preserve original scriptlets and unresolved evidence for replay policy.

pub mod adapters;
mod apparmor_adapters;
pub mod blocked_classes;
pub mod command_evidence;
mod converter;
mod debian_adapters;
pub mod effects;
#[cfg(test)]
mod golden_fixtures;
pub mod legacy_provenance;
pub mod payload_hints;
mod public_policy;
pub mod scriptlet_bundle;
mod security_policy;
mod selinux_adapters;
pub mod support_matrix;

pub use converter::{ConversionOptions, ConversionResult, LegacyConverter};
pub use legacy_provenance::LegacyProvenance;
pub use scriptlet_bundle::{
    ScriptletBundleBuild, ScriptletBundleInput, ScriptletBundleSummary,
    ScriptletDecisionCountsSummary, build_legacy_scriptlet_bundle,
};
