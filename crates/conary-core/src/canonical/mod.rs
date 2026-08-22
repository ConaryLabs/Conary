// conary-core/src/canonical/mod.rs

//! Canonical package mapping: cross-distro name resolution from explicit sources.
//!
//! This module provides:
//! - Versioned exact contracts mapping literal profile/package identities
//! - Remi map exchange for repository-authorized contract snapshots
//! - Repology and AppStream discovery caches that cannot create equivalence
//!
//! Regexes, filename precedence, payload paths, package-name similarity,
//! inferred capabilities, Repology co-occurrence, and AppStream co-occurrence
//! are not mapping authority.

/// Current canonical-map HTTP contract version.
pub const CANONICAL_MAP_SCHEMA_VERSION: u32 = 1;

pub use exchange::{
    CanonicalMapEntry, CanonicalMapSnapshot, parse_snapshot,
    validate_snapshot as validate_canonical_map_snapshot,
};

pub mod appstream;
pub mod client;
pub(crate) mod exchange;
pub mod repology;
pub mod rules;
