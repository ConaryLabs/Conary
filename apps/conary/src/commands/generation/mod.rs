// src/commands/generation/mod.rs
//! Generation management — atomic system state management
//!
//! Types and helpers are defined here ahead of the command implementations
//! that will consume them in subsequent tasks.

pub(crate) mod activation_intents;
pub mod boot;
pub mod builder;
mod cleanup;
pub mod commands;
pub mod composefs;
pub(crate) mod config_transaction;
pub mod export;
pub(crate) mod gc;
pub mod metadata;
pub(crate) mod publication;
pub(crate) mod selected_root;
pub mod switch;
pub mod takeover;
pub mod takeover_state;
