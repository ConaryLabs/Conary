// conary/tests/features.rs

//! Feature-specific tests: install reasons, collections, config files, and system state.

mod common;

use conary_core::db;

#[path = "features/collections.rs"]
mod collections;
#[path = "features/config_files.rs"]
mod config_files;
#[path = "features/derived_packages.rs"]
mod derived_packages;
#[path = "features/install_reasons.rs"]
mod install_reasons;
#[path = "features/reference_mirrors.rs"]
mod reference_mirrors;
#[path = "features/state_snapshots.rs"]
mod state_snapshots;
#[path = "features/system_model.rs"]
mod system_model;
