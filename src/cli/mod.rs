// src/cli/mod.rs
//! CLI definitions for the Conary package manager
//!
//! This module contains all command-line interface definitions using clap.
//! The actual command implementations are in the `commands` module.
//!
//! Commands are organized into logical groups as subcommands:
//! - `package` - Install, remove, and manage packages
//! - `query` - Query installed packages and dependencies
//! - `repo` - Repository management
//! - `config` - Configuration file management
//! - `state` - System state snapshots and rollback
//! - `trigger` - Trigger management
//! - `label` - Label/provenance management
//! - `collection` - Package collection/group management
//! - `ccs` - Native CCS package format commands
//! - `derive` - Derived package management
//! - `model` - System model commands
//! - `system` - System-level commands (init, completions, etc.)

use clap::{Parser, Subcommand};

mod ccs;
mod collection;
mod config;
mod derive;
mod label;
mod model;
mod package;
mod query;
mod repo;
mod state;
mod system;
mod trigger;

pub use ccs::CcsCommands;
pub use collection::CollectionCommands;
pub use config::ConfigCommands;
pub use derive::DeriveCommands;
pub use label::LabelCommands;
pub use model::ModelCommands;
pub use package::PackageCommands;
pub use query::QueryCommands;
pub use repo::RepoCommands;
pub use state::StateCommands;
pub use system::SystemCommands;
pub use trigger::TriggerCommands;

#[derive(Parser)]
#[command(name = "conary")]
#[command(author = "Conary Project")]
#[command(version)]
#[command(about = "A next-generation package manager with atomic transactions", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Package installation, removal, and updates
    #[command(subcommand)]
    Package(PackageCommands),

    /// Query installed packages and dependencies
    #[command(subcommand)]
    Query(QueryCommands),

    /// Repository management
    #[command(subcommand)]
    Repo(RepoCommands),

    /// Configuration file management
    #[command(subcommand)]
    Config(ConfigCommands),

    /// System state snapshots and rollback
    #[command(subcommand)]
    State(StateCommands),

    /// Trigger management
    #[command(subcommand)]
    Trigger(TriggerCommands),

    /// Label and provenance management
    #[command(subcommand)]
    Label(LabelCommands),

    /// Package collection/group management
    #[command(subcommand)]
    Collection(CollectionCommands),

    /// Native CCS package format
    #[command(subcommand)]
    Ccs(CcsCommands),

    /// Derived package management
    #[command(subcommand)]
    Derive(DeriveCommands),

    /// System model management
    #[command(subcommand)]
    Model(ModelCommands),

    /// System-level commands
    #[command(subcommand)]
    System(SystemCommands),
}
