// apps/conary/src/commands/install/ccs_transaction.rs
//! Direct CCS package transaction install adapter.
//!
//! This module owns the direct CCS transaction entry point and CCS-specific
//! manifest selection, hook-status, and capability-gate helpers. Shared install
//! transaction mechanics stay in `install/mod.rs`.
