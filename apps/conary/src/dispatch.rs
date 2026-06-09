// apps/conary/src/dispatch.rs
//! Conary CLI command dispatch.

mod automation;
mod bootstrap;
mod cache;
mod capability;
mod catalog;
mod ccs;
mod collection;
mod config;
mod context;
mod derivation;
mod derive;
mod federation;
mod model;
mod profile;
mod provenance;
mod query;
mod repo;
mod root;
mod system;
mod system_generation;
mod system_redirect;
mod system_state;
mod system_trigger;
mod system_update_channel;
mod trust;
mod verify_derivation;

use crate::cli::Cli;
use crate::command_risk;
use anyhow::Result;

pub async fn dispatch(cli: Cli) -> Result<()> {
    let allow_live_system_mutation = cli.allow_live_system_mutation;
    command_risk::enforce_cli_policy(allow_live_system_mutation, &cli)?;
    root::dispatch_command(cli.command, allow_live_system_mutation).await
}
