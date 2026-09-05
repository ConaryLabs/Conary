// crates/conary-core/src/bin/conary-alpm-resolution-oracle.rs

//! Emit one independently produced ALPM dependency-resolution parity bundle.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{ArgGroup, Parser};
use conary_core::repository::catalog::{
    AlpmParityMemberInput, ProfileRevisionV2, ResolutionWorkerCount, ResolutionWorkerRequest,
    SourceSnapshotV1, ensure_resolution_walk_evidence_outside_bundle,
    produce_alpm_resolution_oracle_with_workers, produce_alpm_resolution_survey_with_workers,
    produce_alpm_resolution_walk_with_workers, write_resolution_walk_implementation_evidence,
};
use serde::de::DeserializeOwned;

const MAX_INPUT_MANIFEST_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Parser)]
#[command(
    about = "Produce a strict libalpm resolution bundle and/or diagnostics survey",
    group(ArgGroup::new("destination").required(true).multiple(true).args(["output", "survey"]))
)]
struct Arguments {
    /// Exact ProfileRevisionV2 manifest.
    #[arg(long)]
    profile_manifest: PathBuf,

    /// Ordered SourceSnapshotV1 manifest; repeat once per profile member.
    #[arg(long, required = true)]
    source_snapshot: Vec<PathBuf>,

    /// Ordered authenticated ALPM database; repeat once per profile member.
    #[arg(long, required = true)]
    database: Vec<PathBuf>,

    /// Independently produced and reopened NativeParityOracleV1 bundle.
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

    /// Worker threads; defaults to detected CPU and measured memory capacity.
    #[arg(long)]
    workers: Option<ResolutionWorkerCount>,

    /// New JSON file recording worker count and per-worker pool-load time.
    #[arg(long)]
    implementation_evidence: PathBuf,
}

fn main() {
    conary_bootstrap::init_cli_tracing("info");
    if let Err(error) = run(Arguments::parse()) {
        tracing::error!("conary-alpm-resolution-oracle: {error:#}");
        std::process::exit(1);
    }
}

fn run(arguments: Arguments) -> Result<()> {
    if let Some(output) = &arguments.output {
        ensure_resolution_walk_evidence_outside_bundle(output, &arguments.implementation_evidence)
            .context("validate ALPM resolution implementation evidence destination")?;
    }
    if arguments.source_snapshot.len() != arguments.database.len() {
        bail!(
            "received {} source snapshots but {} ALPM databases",
            arguments.source_snapshot.len(),
            arguments.database.len()
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
        .zip(&arguments.database)
        .map(|(source_snapshot, database)| AlpmParityMemberInput {
            source_snapshot,
            database,
        })
        .collect::<Vec<_>>();
    let worker_request = arguments.workers.map_or(
        ResolutionWorkerRequest::Automatic,
        ResolutionWorkerRequest::explicit,
    );
    let mut survey_failures = None;
    let mut strict_failure = None;
    let evidence = match (arguments.output, arguments.survey) {
        (Some(output), None) => {
            let (_, evidence) = produce_alpm_resolution_oracle_with_workers(
                &profile,
                &inputs,
                &arguments.package_oracle,
                &arguments.architecture,
                &output,
                worker_request,
            )
            .context("produce ALPM resolution oracle")?;
            evidence
        }
        (None, Some(output)) => {
            let (survey, evidence) = produce_alpm_resolution_survey_with_workers(
                &profile,
                &inputs,
                &arguments.package_oracle,
                &arguments.architecture,
                &output,
                worker_request,
            )
            .context("produce ALPM resolution survey")?;
            if survey.total_failures != 0 {
                survey_failures = Some((survey.total_failures, output));
            }
            evidence
        }
        (Some(output), Some(survey_path)) => {
            let (survey, strict, evidence) = produce_alpm_resolution_walk_with_workers(
                &profile,
                &inputs,
                &arguments.package_oracle,
                &arguments.architecture,
                &survey_path,
                &output,
                worker_request,
            )
            .context("produce ALPM resolution survey and oracle")?;
            if survey.total_failures != 0 {
                survey_failures = Some((survey.total_failures, survey_path));
            } else if let Err(error) = strict {
                strict_failure = Some(error);
            }
            evidence
        }
        (None, None) => unreachable!("clap requires at least one resolution destination"),
    };
    write_resolution_walk_implementation_evidence(&arguments.implementation_evidence, &evidence)
        .context("write ALPM resolution implementation evidence")?;
    if let Some((failures, output)) = survey_failures {
        bail!(
            "ALPM resolution survey recorded {failures} failed roots; inventory written to {}",
            output.display()
        );
    }
    if let Some(error) = strict_failure {
        bail!("ALPM {error}; survey and implementation evidence written");
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
