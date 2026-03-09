// src/commands/generation/mod.rs
//! Generation management — atomic system state management
//!
//! Types and helpers are defined here ahead of the command implementations
//! that will consume them in subsequent tasks.

pub mod boot;
pub mod builder;
pub mod commands;
pub mod composefs;
pub mod metadata;
pub mod switch;
pub mod takeover;
