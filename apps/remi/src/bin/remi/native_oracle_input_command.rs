// apps/remi/src/bin/remi/native_oracle_input_command.rs

//! Exact private-candidate native oracle input materialization command.

use anyhow::Result;
use clap::Args;
use remi::server::{
    NativeOracleInputConfig, ProfileRevisionSelection, materialize_native_oracle_inputs,
};
use std::path::PathBuf;

#[derive(Args)]
pub(crate) struct CommandArgs {
    /// Current Remi operational database.
    #[arg(long, default_value = "/conary/metadata/conary.db")]
    db: PathBuf,

    /// Root containing immutable source and profile catalogs.
    #[arg(long, default_value = "/conary/catalogs")]
    catalog_dir: PathBuf,

    /// Exact public private candidate as PROFILE=REVISION; repeat in canonical order.
    #[arg(
        long = "candidate",
        value_name = "PROFILE=SHA256",
        required = true,
        value_parser = super::parse_candidate
    )]
    candidates: Vec<ProfileRevisionSelection>,

    /// New durable output directory on the destination filesystem.
    #[arg(long)]
    output_dir: PathBuf,
}

pub(crate) fn run(args: CommandArgs) -> Result<()> {
    let config = NativeOracleInputConfig {
        db_path: args.db,
        catalog_dir: args.catalog_dir,
        candidates: args.candidates,
        output_dir: args.output_dir,
    };
    conary_bootstrap::run_with_runtime(move || async move {
        let outcome = materialize_native_oracle_inputs(&config).await?;
        println!("{}", serde_json::to_string_pretty(&outcome)?);
        Ok(())
    })
}
