// src/cli/distro.rs
//! Named source-feed diagnostics

use super::DbArgs;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum DistroCommands {
    /// Show named feed presets and their repository status
    List {
        #[command(flatten)]
        db: DbArgs,
    },
    /// Show diagnostic installed-package source affinity
    Info {
        #[command(flatten)]
        db: DbArgs,
    },
}
