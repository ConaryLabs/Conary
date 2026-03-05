// src/lib.rs
//! Compatibility shim -- re-exports conary_core so existing `use conary::` paths work.

pub use conary_core::*;

// These modules remain in the root crate (moved to conary-server in Phase 2)
#[cfg(feature = "server")]
pub mod federation;

#[cfg(feature = "server")]
pub mod server;

#[cfg(feature = "daemon")]
pub mod daemon;
