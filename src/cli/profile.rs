// src/cli/profile.rs

//! Build profile CLI commands

use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum ProfileCommands {
    /// Generate a build profile from a system manifest
    Generate {
        /// Path to the system manifest TOML
        manifest: PathBuf,
        /// Output profile path
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Display a build profile
    Show {
        /// Path to profile or manifest
        path: PathBuf,
    },
    /// Compare two profiles
    Diff {
        /// First profile
        old: PathBuf,
        /// Second profile
        new: PathBuf,
    },
}
