// apps/remi/src/bin/deployment_command.rs

//! Typed deployment administration commands for the standalone Remi binary.

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Initialize the dedicated universe keys and durable public metadata root.
    InitializeUniverseAuthority(UniverseAuthorityArgs),
    /// Back up current state and prepare current config/schema authority.
    Prepare(PrepareArgs),
    /// Restore a prepared deployment transition.
    Rollback(RollbackArgs),
    /// Verify current schema and typed deployment completion state.
    Inspect(InspectArgs),
}

#[derive(Args)]
pub(crate) struct UniverseAuthorityArgs {
    /// Durable exact-profile and universe signing authority root.
    #[arg(long, default_value = "/conary/repository-keys")]
    repository_keys_dir: PathBuf,
}

#[derive(Args)]
pub(crate) struct PrepareArgs {
    /// Current Remi service configuration.
    #[arg(long, default_value = "/etc/conary/remi.toml")]
    config: PathBuf,

    /// Staged typed repository manifest.
    #[arg(long)]
    repository_manifest: PathBuf,

    /// Installed typed repository manifest path.
    #[arg(long, default_value = "/etc/conary/remi-repositories.toml")]
    repository_manifest_target: PathBuf,

    /// Durable exact-profile repository signing authority.
    #[arg(long, default_value = "/conary/repository-keys")]
    repository_keys_dir: PathBuf,

    /// Stable deployment identity used in the recoverable backup name.
    #[arg(long)]
    deployment_id: String,

    /// Maximum concurrent package conversions.
    #[arg(long)]
    max_concurrent: usize,
}

#[derive(Args)]
pub(crate) struct RollbackArgs {
    /// Transition manifest emitted by `deployment prepare`.
    #[arg(long)]
    manifest: PathBuf,
}

#[derive(Args)]
pub(crate) struct InspectArgs {
    /// Current Remi service configuration.
    #[arg(long, default_value = "/etc/conary/remi.toml")]
    config: PathBuf,

    /// Fail until every public profile has an exact nonempty private candidate.
    #[arg(long, conflicts_with = "require_repopulated")]
    require_private_candidates: bool,

    /// Fail until every active profile has conversions and a matching signed universe.
    #[arg(long, conflicts_with = "require_private_candidates")]
    require_repopulated: bool,
}

pub(crate) fn run(command: Command) -> Result<()> {
    match command {
        Command::InitializeUniverseAuthority(args) => {
            let root = remi::deployment::initialize_universe_authority(&args.repository_keys_dir)?;
            println!("{}", root.display());
        }
        Command::Prepare(args) => {
            let manifest = remi::deployment::prepare(&remi::deployment::PrepareOptions {
                config_path: args.config,
                repository_manifest_source: args.repository_manifest,
                repository_manifest_target: args.repository_manifest_target,
                repository_keys_dir: args.repository_keys_dir,
                deployment_id: args.deployment_id,
                max_concurrent: args.max_concurrent,
            })?;
            println!("{}", manifest.display());
        }
        Command::Rollback(args) => remi::deployment::rollback(&args.manifest)?,
        Command::Inspect(args) => {
            let state = remi::deployment::inspect_state(&args.config)?;
            println!("{}", serde_json::to_string_pretty(&state)?);
            if args.require_private_candidates && !state.private_candidates_complete() {
                bail!("Remi private profile candidates are not complete");
            }
            if args.require_repopulated && !state.repopulation_complete() {
                bail!("Remi immutable profile universe and conversions are not populated");
            }
        }
    }
    Ok(())
}
