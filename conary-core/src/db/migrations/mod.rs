// conary-core/src/db/migrations/mod.rs
//! Database migration implementations
//!
//! This module contains the individual migration functions for evolving
//! the Conary database schema. Each migration function handles a specific
//! version upgrade.
//!
//! Range files:
//! - v1_v20: Migrations 1-20 (core tables, repos, components, labels)
//! - v21_v40: Migrations 21-40 (config, security, federation, derived packages)
//! - v41_current: Migrations 41-57 (collections, TUF, canonical, derivations)

mod v1_v20;
mod v21_v40;
mod v41_current;

pub use v1_v20::*;
pub use v21_v40::*;
pub use v41_current::*;
