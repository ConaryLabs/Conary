// crates/conary-core/src/bin/conary-rpm-oracle.rs

//! Emit one independently produced RPM full-catalog parity bundle.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Parser;
use conary_core::repository::catalog::{
    ProfileRevisionV2, RpmParityMemberInput, SourceSnapshotV1, produce_rpm_parity_oracle,
};
use serde::de::DeserializeOwned;

const MAX_INPUT_MANIFEST_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Parser)]
#[command(about = "Produce a strict libsolv RPM full-catalog parity bundle")]
struct Arguments {
    /// Exact ProfileRevisionV2 manifest.
    #[arg(long)]
    profile_manifest: PathBuf,

    /// Ordered SourceSnapshotV1 manifest; repeat once per profile member.
    #[arg(long, required = true)]
    source_snapshot: Vec<PathBuf>,

    /// Ordered authenticated RPM primary metadata; repeat once per profile member.
    #[arg(long, required = true)]
    primary: Vec<PathBuf>,

    /// Ordered authenticated RPM filelists metadata; repeat once per profile member.
    #[arg(long, required = true)]
    filelists: Vec<PathBuf>,

    /// New directory that will receive manifest.json and packages.jsonl.
    #[arg(long)]
    output: PathBuf,
}

fn main() {
    if let Err(error) = run(Arguments::parse()) {
        eprintln!("conary-rpm-oracle: {error:#}");
        std::process::exit(1);
    }
}

fn run(arguments: Arguments) -> Result<()> {
    let members = arguments.source_snapshot.len();
    if arguments.primary.len() != members || arguments.filelists.len() != members {
        bail!(
            "received {members} source snapshots, {} primary objects, and {} filelists objects",
            arguments.primary.len(),
            arguments.filelists.len()
        );
    }
    let profile: ProfileRevisionV2 = load_manifest(&arguments.profile_manifest, "profile")?;
    let snapshots = arguments
        .source_snapshot
        .iter()
        .map(|path| load_manifest(path, "source snapshot"))
        .collect::<Result<Vec<SourceSnapshotV1>>>()?;
    let inputs = snapshots
        .iter()
        .zip(&arguments.primary)
        .zip(&arguments.filelists)
        .map(
            |((source_snapshot, primary), filelists)| RpmParityMemberInput {
                source_snapshot,
                primary,
                filelists,
            },
        )
        .collect::<Vec<_>>();
    produce_rpm_parity_oracle(&profile, &inputs, &arguments.output)
        .context("produce RPM parity oracle")?;
    Ok(())
}

fn load_manifest<T: DeserializeOwned>(path: &Path, label: &str) -> Result<T> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} manifest {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        bail!(
            "{label} manifest {} must be a regular file, never a symlink",
            path.display()
        );
    }
    if metadata.len() > MAX_INPUT_MANIFEST_BYTES {
        bail!(
            "{label} manifest {} exceeds {} bytes",
            path.display(),
            MAX_INPUT_MANIFEST_BYTES
        );
    }
    let bytes =
        fs::read(path).with_context(|| format!("read {label} manifest {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parse {label} manifest {}", path.display()))
}
