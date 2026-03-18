// conary-core/src/generation/mod.rs

//! Generation management for composefs-based system deployment.
//!
//! This module will contain the core logic for building, managing, and
//! switching between composefs generations (EROFS images backed by CAS).

#[cfg(feature = "composefs-rs")]
pub mod composefs_rs_eval;
