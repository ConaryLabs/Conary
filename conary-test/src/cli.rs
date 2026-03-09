// conary-test/src/cli.rs

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "conary-test", version, about = "Conary test infrastructure")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a test suite
    Run {
        /// Distro to test against
        #[arg(long)]
        distro: String,

        /// Test phase (1 or 2)
        #[arg(long, default_value = "1")]
        phase: u32,

        /// Path to test suite TOML
        #[arg(long)]
        suite: Option<String>,

        /// Run all distros
        #[arg(long)]
        all_distros: bool,
    },

    /// Start the HTTP + MCP server
    Serve {
        /// Port to listen on
        #[arg(long, default_value = "9090")]
        port: u16,
    },

    /// List available test suites
    List,

    /// Manage container images
    Images {
        #[command(subcommand)]
        command: ImageCommands,
    },
}

#[derive(Subcommand)]
enum ImageCommands {
    /// Build a distro image
    Build {
        /// Distro to build
        #[arg(long)]
        distro: String,
    },

    /// List built images
    List,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run { distro, phase, .. } => {
            tracing::info!(%distro, %phase, "Starting test run");
            Ok(())
        }
        Commands::Serve { port } => {
            tracing::info!(%port, "Starting server");
            Ok(())
        }
        Commands::List => Ok(()),
        Commands::Images { command } => {
            match command {
                ImageCommands::Build { distro } => {
                    tracing::info!(%distro, "Building image");
                }
                ImageCommands::List => {}
            }
            Ok(())
        }
    }
}
