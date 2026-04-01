// conary-core/src/generation/mod.rs

//! Generation management for composefs-based system deployment.
//!
//! This module will contain the core logic for building, managing, and
//! switching between composefs generations (EROFS images backed by CAS).

pub mod builder;
pub mod composefs;
pub mod delta;
pub mod etc_merge;
pub mod gc;
pub mod metadata;
pub mod mount;

#[cfg(feature = "composefs-rs")]
pub mod composefs_rs_eval;
