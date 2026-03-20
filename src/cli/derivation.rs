// src/cli/derivation.rs

//! Derivation engine CLI commands

use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum DerivationCommands {
    /// Build a single recipe into CAS via the derivation engine
    Build {
        /// Path to the recipe TOML file
        recipe: PathBuf,
        /// Build environment EROFS image
        #[arg(long)]
        env: PathBuf,
        /// CAS objects directory
        #[arg(long, default_value = "/var/lib/conary/objects")]
        cas_dir: PathBuf,
        /// Database path
        #[arg(long)]
        db_path: Option<PathBuf>,
    },
    /// Show derivation ID for a recipe without building
    Show {
        /// Path to the recipe TOML file
        recipe: PathBuf,
        /// Build environment hash
        #[arg(long)]
        env_hash: String,
    },
}
