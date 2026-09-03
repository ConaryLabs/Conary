// crates/conary-core/src/bin/conary-debian-resolution-oracle.rs

//! Emit one independently produced Debian full-universe resolution parity bundle.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{ArgGroup, Parser};
use conary_core::repository::catalog::{
    DebianParityMemberInput, ProfileRevisionV2, ResolutionWorkerCount, ResolutionWorkerRequest,
    SourceSnapshotV1, ensure_resolution_walk_evidence_outside_bundle,
    produce_debian_resolution_oracle_with_workers, produce_debian_resolution_survey_with_workers,
    run_debian_resolution_worker, write_resolution_walk_implementation_evidence,
};
use serde::de::DeserializeOwned;

const MAX_INPUT_MANIFEST_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Parser)]
#[command(
    about = "Produce a strict apt-pkg Debian resolution bundle or diagnostics survey",
    group(ArgGroup::new("destination").required(true).multiple(false).args(["output", "survey"]))
)]
struct Arguments {
    /// Exact ProfileRevisionV2 manifest.
    #[arg(long)]
    profile_manifest: PathBuf,

    /// Ordered SourceSnapshotV1 manifest; repeat once per profile member.
    #[arg(long, required = true)]
    source_snapshot: Vec<PathBuf>,

    /// Ordered authenticated Debian Packages object; repeat once per profile member.
    #[arg(long, required = true)]
    packages: Vec<PathBuf>,

    /// Independently verified NativeParityOracleV1 bundle directory.
    #[arg(long)]
    package_oracle: PathBuf,

    /// Assertion matching the profile revision's typed target architecture.
    #[arg(long)]
    architecture: String,

    /// New directory that will receive manifest.json and roots.jsonl.
    #[arg(long)]
    output: Option<PathBuf>,

    /// New diagnostics-only NativeResolutionSurveyV1 JSON file.
    #[arg(long)]
    survey: Option<PathBuf>,

    /// Worker processes; defaults to detected CPU and measured memory capacity.
    #[arg(long)]
    workers: Option<ResolutionWorkerCount>,

    /// New JSON file recording worker count and per-worker pool-load time.
    #[arg(long)]
    implementation_evidence: PathBuf,
}

#[derive(Debug, Parser)]
struct WorkerArguments {
    #[arg(long)]
    architecture: String,
    #[arg(long)]
    package_index: PathBuf,
    #[arg(long, required = true)]
    solver_input: Vec<PathBuf>,
}

fn main() {
    let mut raw = std::env::args_os();
    let program = raw.next().unwrap_or_default();
    if raw
        .next()
        .is_some_and(|argument| argument == "--internal-resolution-worker")
    {
        let arguments = WorkerArguments::parse_from(std::iter::once(program).chain(raw));
        if let Err(error) = run_debian_resolution_worker(
            &arguments.solver_input,
            &arguments.package_index,
            &arguments.architecture,
        ) {
            eprintln!("conary-debian-resolution-oracle worker: {error:#}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = run(Arguments::parse()) {
        eprintln!("conary-debian-resolution-oracle: {error:#}");
        std::process::exit(1);
    }
}

fn run(arguments: Arguments) -> Result<()> {
    if let Some(output) = &arguments.output {
        ensure_resolution_walk_evidence_outside_bundle(output, &arguments.implementation_evidence)
            .context("validate Debian resolution implementation evidence destination")?;
    }
    let members = arguments.source_snapshot.len();
    if arguments.packages.len() != members {
        bail!(
            "received {members} source snapshots and {} Packages objects",
            arguments.packages.len()
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
        .zip(&arguments.packages)
        .map(|(source_snapshot, packages)| DebianParityMemberInput {
            source_snapshot,
            packages,
        })
        .collect::<Vec<_>>();
    let worker_request = arguments.workers.map_or(
        ResolutionWorkerRequest::Automatic,
        ResolutionWorkerRequest::explicit,
    );
    let mut survey_failures = None;
    let evidence = match (arguments.output, arguments.survey) {
        (Some(output), None) => {
            let (_, evidence) = produce_debian_resolution_oracle_with_workers(
                &profile,
                &inputs,
                &arguments.package_oracle,
                &arguments.architecture,
                &output,
                worker_request,
            )
            .context("produce Debian resolution oracle")?;
            evidence
        }
        (None, Some(output)) => {
            let (survey, evidence) = produce_debian_resolution_survey_with_workers(
                &profile,
                &inputs,
                &arguments.package_oracle,
                &arguments.architecture,
                &output,
                worker_request,
            )
            .context("produce Debian resolution survey")?;
            if survey.total_failures != 0 {
                survey_failures = Some((survey.total_failures, output));
            }
            evidence
        }
        _ => unreachable!("clap requires exactly one resolution destination"),
    };
    write_resolution_walk_implementation_evidence(&arguments.implementation_evidence, &evidence)
        .context("write Debian resolution implementation evidence")?;
    if let Some((failures, output)) = survey_failures {
        bail!(
            "Debian resolution survey recorded {failures} failed roots; inventory written to {}",
            output.display()
        );
    }
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
