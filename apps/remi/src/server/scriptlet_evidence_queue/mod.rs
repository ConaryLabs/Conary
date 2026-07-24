// apps/remi/src/server/scriptlet_evidence_queue/mod.rs

pub mod aggregation;
pub mod backfill;
pub mod classification;
pub mod normalization;
pub mod packet;
pub mod reconciliation;
pub mod storage;
pub mod types;

/// Current queue normalization contract for newly materialized evidence.
///
/// Version 2 filters structural unknown-command noise and preserves approved,
/// traversal-free AppArmor policy paths in normalized cluster identity.
pub const SCRIPTLET_EVIDENCE_NORMALIZATION_VERSION: i64 = 2;

pub use storage::record_converted_package;
