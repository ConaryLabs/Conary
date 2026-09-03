// crates/conary-core/src/repository/catalog/parity/debian/resolution.rs

//! Independent apt-pkg-backed Debian native resolution evidence production.
//!
//! apt-pkg exposes process-global configuration and system pointers, so safe
//! parallelism uses worker processes rather than shared or merely thread-local
//! handles. Each process builds its own cache from the same staged authenticated
//! inputs and opens the package index read-only. A bounded ordered sink in the
//! parent remains the sole canonical artifact writer.

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use super::ffi::{AptNativeIdentity, AptRelationKind, AptResolution, AptResolutionOutcome};
use super::{
    DEBIAN_PARITY_PROJECTION_SCHEMA_V1, DebianParityMemberInput, PINNED_APT_PKG_VERSION,
    produce_debian_parity_oracle, stage_verified_packages, validate_inputs,
};
use crate::error::{Error, Result};
use crate::repository::architecture::NativeResolutionArchitectureDecisionV1;
use crate::repository::catalog::ProfileRevisionV2;
use crate::repository::catalog::parity::resolution_parallel::{
    OrderedResolutionMetrics, RESOLUTION_WORKER_RSS_BYTES, ResolutionWalkImplementationEvidenceV1,
    ResolutionWorkerCount, ResolutionWorkerRequest, resolution_walk_memory_budget_bytes,
    walk_ordered_parallel,
};
use crate::repository::catalog::parity::resolution_survey::{
    NativeExplanationBudget, NativeResolutionSurveyCollector, NativeRootResolutionError,
    NativeRootResolutionResult, RootOutcomeSink,
};
use crate::repository::catalog::parity::{
    NATIVE_RESOLUTION_ROOT_FILE_NAME, NativeParityEcosystemV1, NativeParityImplementationV1,
    NativeParityOracleReader, NativeParityOracleV1, NativeResolutionArchitectureAdmissionV1,
    NativeResolutionInstalledStateV1, NativeResolutionNotInstallableReasonV1,
    NativeResolutionOracleV1, NativeResolutionOracleWriter, NativeResolutionOutcomeV1,
    NativeResolutionPolicyV1, NativeResolutionProviderPolicyV1,
    NativeResolutionRequirementPolicyV1, NativeResolutionRootPolicyV1,
    NativeResolutionSurveyDebianMissingV1, NativeResolutionSurveyDebianPackageV1,
    NativeResolutionSurveyDebianResultV1, NativeResolutionSurveyErrorReasonV1,
    NativeResolutionSurveyErrorVariantV1, NativeResolutionSurveyEvidenceWithheldReasonV1,
    NativeResolutionSurveyNativeExplanationV1, NativeResolutionSurveyV1,
    NativeUnresolvedDependencyV1, native_requirement_group_sha256,
    verify_native_parity_oracle_bundle, verify_native_resolution_oracle_bundle,
    write_native_resolution_oracle_manifest, write_native_resolution_survey,
};
use crate::repository::dependency_model::RepositoryRequirementKind;
use crate::repository::versioning::VersionScheme;

/// Projection contract for apt-pkg transaction selections and broken strong groups.
pub const DEBIAN_RESOLUTION_PROJECTION_SCHEMA_V2: u32 = 2;

const CREATE_INDEX: &str = "
CREATE TABLE packages (
    package_key_sha256 TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    architecture TEXT NOT NULL,
    UNIQUE (name, version, architecture)
) STRICT;
CREATE TABLE requirements (
    package_key_sha256 TEXT NOT NULL REFERENCES packages(package_key_sha256),
    kind TEXT NOT NULL,
    native_text TEXT NOT NULL,
    requirement_group_sha256 TEXT NOT NULL,
    PRIMARY KEY (package_key_sha256, kind, native_text)
) STRICT;
";

/// Produce and independently reopen one strict Debian resolution parity bundle.
pub fn produce_debian_resolution_oracle(
    profile: &ProfileRevisionV2,
    inputs: &[DebianParityMemberInput<'_>],
    package_oracle_directory: &Path,
    architecture: &str,
    output: &Path,
) -> Result<NativeResolutionOracleV1> {
    produce_debian_resolution_oracle_with_workers(
        profile,
        inputs,
        package_oracle_directory,
        architecture,
        output,
        ResolutionWorkerRequest::Automatic,
    )
    .map(|(manifest, _)| manifest)
}

/// Produce a strict Debian bundle with isolated apt-pkg worker processes.
pub fn produce_debian_resolution_oracle_with_workers(
    profile: &ProfileRevisionV2,
    inputs: &[DebianParityMemberInput<'_>],
    package_oracle_directory: &Path,
    architecture: &str,
    output: &Path,
    worker_request: ResolutionWorkerRequest,
) -> Result<(
    NativeResolutionOracleV1,
    ResolutionWalkImplementationEvidenceV1,
)> {
    let ResolutionProduct::Oracle(manifest) = produce_debian_resolution(
        profile,
        inputs,
        package_oracle_directory,
        architecture,
        ResolutionDestination::Oracle(output),
        worker_request,
    )?
    else {
        unreachable!("Debian oracle destination returned survey")
    };
    Ok(manifest)
}

/// Walk every exact Debian root and write one diagnostics-only failure survey.
pub fn produce_debian_resolution_survey(
    profile: &ProfileRevisionV2,
    inputs: &[DebianParityMemberInput<'_>],
    package_oracle_directory: &Path,
    architecture: &str,
    output: &Path,
) -> Result<NativeResolutionSurveyV1> {
    produce_debian_resolution_survey_with_workers(
        profile,
        inputs,
        package_oracle_directory,
        architecture,
        output,
        ResolutionWorkerRequest::Automatic,
    )
    .map(|(survey, _)| survey)
}

/// Produce a Debian survey with isolated apt-pkg worker processes.
pub fn produce_debian_resolution_survey_with_workers(
    profile: &ProfileRevisionV2,
    inputs: &[DebianParityMemberInput<'_>],
    package_oracle_directory: &Path,
    architecture: &str,
    output: &Path,
    worker_request: ResolutionWorkerRequest,
) -> Result<(
    NativeResolutionSurveyV1,
    ResolutionWalkImplementationEvidenceV1,
)> {
    let ResolutionProduct::Survey(survey) = produce_debian_resolution(
        profile,
        inputs,
        package_oracle_directory,
        architecture,
        ResolutionDestination::Survey(output),
        worker_request,
    )?
    else {
        unreachable!("Debian survey destination returned oracle")
    };
    Ok(survey)
}

enum ResolutionDestination<'a> {
    Oracle(&'a Path),
    Survey(&'a Path),
}

enum ResolutionProduct {
    Oracle(
        (
            NativeResolutionOracleV1,
            ResolutionWalkImplementationEvidenceV1,
        ),
    ),
    Survey(
        (
            NativeResolutionSurveyV1,
            ResolutionWalkImplementationEvidenceV1,
        ),
    ),
}

fn produce_debian_resolution(
    profile: &ProfileRevisionV2,
    inputs: &[DebianParityMemberInput<'_>],
    package_oracle_directory: &Path,
    architecture: &str,
    destination: ResolutionDestination<'_>,
    worker_request: ResolutionWorkerRequest,
) -> Result<ResolutionProduct> {
    let architecture = profile.require_target_architecture(architecture)?;
    let policy = NativeResolutionPolicyV1 {
        architecture: architecture.to_string(),
        architecture_admission: NativeResolutionArchitectureAdmissionV1::NativeOnly,
        installed_state: NativeResolutionInstalledStateV1::Empty,
        roots: NativeResolutionRootPolicyV1::EveryExactPackage,
        positive_requirements: NativeResolutionRequirementPolicyV1::RequiredOnly,
        provider_selection: NativeResolutionProviderPolicyV1::NativePrecedence,
    };
    policy.validate()?;

    let package_oracle = verify_native_parity_oracle_bundle(package_oracle_directory, profile)?;
    require_debian_package_oracle(package_oracle.manifest())?;
    verify_package_oracle_reprojection(profile, inputs, package_oracle.manifest())?;
    validate_inputs(profile, inputs)?;

    let staging = tempfile::Builder::new()
        .prefix("conary-debian-resolution-")
        .tempdir()?;
    let staged = stage_verified_packages(inputs, staging.path())?;
    let solver_inputs = stage_solver_inputs(&staged, staging.path())?;
    let package_index = PackageResolutionIndex::create(&package_oracle)?;
    let memory_budget_bytes = resolution_walk_memory_budget_bytes()?;
    let workers = worker_request.resolve(
        package_oracle.manifest().artifact.counts.packages,
        memory_budget_bytes,
        RESOLUTION_WORKER_RSS_BYTES,
    )?;

    let implementation = NativeParityImplementationV1 {
        ecosystem: NativeParityEcosystemV1::Debian,
        name: "apt-pkg".to_string(),
        version: PINNED_APT_PKG_VERSION.to_string(),
        projection_schema: DEBIAN_RESOLUTION_PROJECTION_SCHEMA_V2,
    };
    match destination {
        ResolutionDestination::Oracle(output) => {
            fs::create_dir(output)?;
            let mut writer = NativeResolutionOracleWriter::create(
                output.join(NATIVE_RESOLUTION_ROOT_FILE_NAME),
                profile,
                package_oracle.manifest(),
                implementation,
                policy.clone(),
            )?;
            let metrics = walk_resolution_roots(
                &package_oracle,
                &package_index,
                &policy,
                RootOutcomeSink::Strict(&mut writer),
                &solver_inputs,
                workers,
            )?;
            let manifest = writer.finish()?;
            write_native_resolution_oracle_manifest(output, &manifest)?;
            let reopened =
                verify_native_resolution_oracle_bundle(output, profile, &package_oracle)?;
            if reopened.manifest() != &manifest {
                return Err(Error::InternalError(
                    "reopened Debian resolution manifest differs from produced manifest"
                        .to_string(),
                ));
            }
            Ok(ResolutionProduct::Oracle((
                manifest,
                implementation_evidence(workers, metrics, memory_budget_bytes)?,
            )))
        }
        ResolutionDestination::Survey(output) => {
            let mut collector = NativeResolutionSurveyCollector::new(
                profile,
                package_oracle.manifest(),
                implementation,
                policy.clone(),
            )?;
            let metrics = walk_resolution_roots(
                &package_oracle,
                &package_index,
                &policy,
                RootOutcomeSink::Survey(&mut collector),
                &solver_inputs,
                workers,
            )?;
            let survey = collector.finish()?;
            write_native_resolution_survey(output, &survey)?;
            Ok(ResolutionProduct::Survey((
                survey,
                implementation_evidence(workers, metrics, memory_budget_bytes)?,
            )))
        }
    }
}

fn walk_resolution_roots(
    package_oracle: &NativeParityOracleReader,
    package_index: &PackageResolutionIndex,
    policy: &NativeResolutionPolicyV1,
    mut sink: RootOutcomeSink<'_>,
    solver_inputs: &[PathBuf],
    workers: ResolutionWorkerCount,
) -> Result<OrderedResolutionMetrics> {
    let explanation_byte_limit = sink.explanation_byte_limit();
    if workers.get() == 1 {
        let started = std::time::Instant::now();
        let mut apt = AptResolution::open(solver_inputs, &policy.architecture)?;
        let package_index = PackageResolutionIndexReader::open(package_index.database())?;
        let load_milliseconds = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        package_oracle.for_each_package(|root| {
            let projected = DebianResolutionRoot::from(&root);
            let result = resolve_exact_root(
                &mut apt,
                &package_index,
                &projected,
                policy,
                sink.explanation_byte_limit(),
            );
            sink.root(&root, result)
        })?;
        return Ok(OrderedResolutionMetrics {
            worker_load_milliseconds: vec![load_milliseconds],
        });
    }
    let executable = debian_worker_executable()?;
    walk_ordered_parallel(
        package_oracle,
        workers,
        explanation_byte_limit,
        |_| {
            DebianResolutionProcess::spawn(
                &executable,
                solver_inputs,
                package_index.database(),
                &policy.architecture,
            )
        },
        |worker, root, byte_limit| worker.resolve(root, byte_limit),
        |root, result| sink.root(root, result),
    )
}

fn implementation_evidence(
    workers: ResolutionWorkerCount,
    metrics: OrderedResolutionMetrics,
    memory_budget_bytes: u64,
) -> Result<ResolutionWalkImplementationEvidenceV1> {
    ResolutionWalkImplementationEvidenceV1::new(
        workers,
        metrics.worker_load_milliseconds,
        memory_budget_bytes,
        RESOLUTION_WORKER_RSS_BYTES,
    )
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DebianResolutionWorkerRequest {
    root: DebianResolutionRoot,
    explanation_byte_limit: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DebianResolutionRoot {
    package_key_sha256: String,
    source_profile: String,
    name: String,
    version: String,
    architecture: Option<String>,
    version_scheme: VersionScheme,
}

impl From<&crate::repository::catalog::parity::NativeParityPackageV1> for DebianResolutionRoot {
    fn from(root: &crate::repository::catalog::parity::NativeParityPackageV1) -> Self {
        Self {
            package_key_sha256: root.package_key_sha256.clone(),
            source_profile: root.source_profile.clone(),
            name: root.name.clone(),
            version: root.version.clone(),
            architecture: root.architecture.clone(),
            version_scheme: root.version_scheme,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum DebianResolutionWorkerResponse {
    Ready,
    Outcome {
        outcome: NativeResolutionOutcomeV1,
    },
    Failure {
        error_variant: NativeResolutionSurveyErrorVariantV1,
        error_message: String,
        reason: NativeResolutionSurveyErrorReasonV1,
        explanation: NativeResolutionSurveyNativeExplanationV1,
    },
}

struct DebianResolutionProcess {
    child: Child,
    input: BufWriter<ChildStdin>,
    output: BufReader<ChildStdout>,
}

impl DebianResolutionProcess {
    fn spawn(
        executable: &Path,
        solver_inputs: &[PathBuf],
        package_index: &Path,
        architecture: &str,
    ) -> Result<Self> {
        let mut command = Command::new(executable);
        command
            .arg("--internal-resolution-worker")
            .arg("--architecture")
            .arg(architecture)
            .arg("--package-index")
            .arg(package_index)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        for input in solver_inputs {
            command.arg("--solver-input").arg(input);
        }
        let mut child = command.spawn()?;
        let input = BufWriter::new(child.stdin.take().ok_or_else(|| {
            Error::InternalError("Debian worker stdin was not piped".to_string())
        })?);
        let mut output = BufReader::new(child.stdout.take().ok_or_else(|| {
            Error::InternalError("Debian worker stdout was not piped".to_string())
        })?);
        let response = read_worker_response(&mut output)?;
        if !matches!(response, DebianResolutionWorkerResponse::Ready) {
            return Err(Error::InternalError(
                "Debian worker emitted a root before readiness".to_string(),
            ));
        }
        Ok(Self {
            child,
            input,
            output,
        })
    }

    fn resolve(
        &mut self,
        root: &crate::repository::catalog::parity::NativeParityPackageV1,
        explanation_byte_limit: u64,
    ) -> NativeRootResolutionResult {
        let request = DebianResolutionWorkerRequest {
            root: root.into(),
            explanation_byte_limit,
        };
        if let Err(error) = write_worker_message(&mut self.input, &request) {
            return Err(NativeRootResolutionError::new(
                error,
                NativeResolutionSurveyErrorReasonV1::NativeSolverUnexpectedFailure,
                debian_unavailable(),
            ));
        }
        match read_worker_response(&mut self.output) {
            Ok(DebianResolutionWorkerResponse::Outcome { outcome }) => Ok(outcome),
            Ok(DebianResolutionWorkerResponse::Failure {
                error_variant,
                error_message,
                reason,
                explanation,
            }) => Err(NativeRootResolutionError::from_wire(
                error_variant,
                error_message,
                reason,
                explanation,
            )),
            Ok(DebianResolutionWorkerResponse::Ready) => Err(NativeRootResolutionError::new(
                Error::InternalError("Debian worker repeated readiness".to_string()),
                NativeResolutionSurveyErrorReasonV1::NativeSolverUnexpectedFailure,
                debian_unavailable(),
            )),
            Err(error) => Err(NativeRootResolutionError::new(
                error,
                NativeResolutionSurveyErrorReasonV1::NativeSolverUnexpectedFailure,
                debian_unavailable(),
            )),
        }
    }
}

impl Drop for DebianResolutionProcess {
    fn drop(&mut self) {
        let _ = self.input.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write_worker_message<T: Serialize>(writer: &mut impl Write, value: &T) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn read_worker_response(
    reader: &mut BufReader<impl std::io::Read>,
) -> Result<DebianResolutionWorkerResponse> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Err(Error::InternalError(
            "Debian resolution worker closed its output".to_string(),
        ));
    }
    serde_json::from_str(&line).map_err(Into::into)
}

fn debian_worker_executable() -> Result<PathBuf> {
    let current = std::env::current_exe()?;
    if current.file_stem().is_some_and(|name| {
        name.to_string_lossy()
            .starts_with("conary-debian-resolution-oracle")
    }) {
        return Ok(current);
    }
    let debug = current
        .parent()
        .and_then(Path::parent)
        .map(|parent| parent.join("conary-debian-resolution-oracle"))
        .filter(|candidate| candidate.is_file());
    debug.ok_or_else(|| {
        Error::ConfigError(
            "cannot locate conary-debian-resolution-oracle worker executable".to_string(),
        )
    })
}

/// Run the private line-delimited apt-pkg worker protocol.
pub fn run_debian_resolution_worker(
    solver_inputs: &[PathBuf],
    package_index: &Path,
    architecture: &str,
) -> Result<()> {
    let mut apt = AptResolution::open(solver_inputs, architecture)?;
    let package_index = PackageResolutionIndexReader::open(package_index)?;
    let policy = NativeResolutionPolicyV1 {
        architecture: architecture.to_string(),
        architecture_admission: NativeResolutionArchitectureAdmissionV1::NativeOnly,
        installed_state: NativeResolutionInstalledStateV1::Empty,
        roots: NativeResolutionRootPolicyV1::EveryExactPackage,
        positive_requirements: NativeResolutionRequirementPolicyV1::RequiredOnly,
        provider_selection: NativeResolutionProviderPolicyV1::NativePrecedence,
    };
    policy.validate()?;
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut input = stdin.lock();
    let mut output = BufWriter::new(stdout.lock());
    write_worker_message(&mut output, &DebianResolutionWorkerResponse::Ready)?;
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let request: DebianResolutionWorkerRequest = serde_json::from_str(&line)?;
        let response = match resolve_exact_root(
            &mut apt,
            &package_index,
            &request.root,
            &policy,
            request.explanation_byte_limit,
        ) {
            Ok(outcome) => DebianResolutionWorkerResponse::Outcome { outcome },
            Err(failure) => {
                let (error_variant, error_message, reason, explanation) = (*failure).into_wire();
                DebianResolutionWorkerResponse::Failure {
                    error_variant,
                    error_message,
                    reason,
                    explanation,
                }
            }
        };
        write_worker_message(&mut output, &response)?;
    }
}

fn resolve_exact_root(
    apt: &mut AptResolution,
    package_index: &PackageResolutionIndexReader,
    root: &DebianResolutionRoot,
    policy: &NativeResolutionPolicyV1,
    explanation_byte_limit: u64,
) -> NativeRootResolutionResult {
    let Some(root_architecture) = root.architecture.as_deref() else {
        return Err(NativeRootResolutionError::new(
            Error::ConflictError(format!(
                "Debian package-oracle root '{}' has no architecture",
                root.name
            )),
            NativeResolutionSurveyErrorReasonV1::ExactRootProjectionFailed,
            debian_unavailable(),
        ));
    };
    match policy
        .architecture_admission
        .admits(&root.source_profile, root.version_scheme, root_architecture)
        .and_then(NativeResolutionArchitectureDecisionV1::into_result)
    {
        Ok(NativeResolutionArchitectureDecisionV1::Admitted) => {}
        Ok(NativeResolutionArchitectureDecisionV1::Excluded { .. }) => {
            return Ok(NativeResolutionOutcomeV1::NotInstallable {
                reason: NativeResolutionNotInstallableReasonV1::ArchitectureExcluded,
            });
        }
        Ok(NativeResolutionArchitectureDecisionV1::UnknownArchitectureToken { .. }) => {
            unreachable!("unknown admission decision returned from into_result")
        }
        Err(error) => {
            return Err(NativeRootResolutionError::new(
                error,
                NativeResolutionSurveyErrorReasonV1::UnknownArchitectureToken,
                debian_unavailable(),
            ));
        }
    }
    let native = apt
        .resolve(&root.name, &root.version, root_architecture)
        .map_err(|error| {
            NativeRootResolutionError::new(
                error,
                NativeResolutionSurveyErrorReasonV1::NativeSolverFailed,
                debian_unavailable(),
            )
        })?;
    match &native {
        AptResolutionOutcome::Resolved(packages) => {
            let closure = packages
                .iter()
                .map(|identity| package_index.package_key(identity))
                .collect::<Result<BTreeSet<_>>>()
                .map_err(|error| {
                    NativeRootResolutionError::new(
                        error,
                        NativeResolutionSurveyErrorReasonV1::ResolvedClosureProjectionFailed,
                        debian_explanation(&native, explanation_byte_limit),
                    )
                })?;
            if !closure.contains(&root.package_key_sha256) {
                return Err(NativeRootResolutionError::new(
                    Error::ConflictError(format!(
                        "apt-pkg closure for '{}' omits its exact root",
                        root.name
                    )),
                    NativeResolutionSurveyErrorReasonV1::ResolvedClosureOmittedRoot,
                    debian_explanation(&native, explanation_byte_limit),
                ));
            }
            Ok(NativeResolutionOutcomeV1::Resolved {
                closure_package_keys_sha256: closure.into_iter().collect(),
            })
        }
        AptResolutionOutcome::Unresolved(missing) => {
            let dependencies = missing
                .iter()
                .map(|missing| package_index.missing_requirement(missing))
                .collect::<Result<BTreeSet<_>>>()
                .map_err(|error| {
                    NativeRootResolutionError::new(
                        error,
                        NativeResolutionSurveyErrorReasonV1::UnresolvedProjectionFailed,
                        debian_explanation(&native, explanation_byte_limit),
                    )
                })?;
            if dependencies.is_empty() {
                return Err(NativeRootResolutionError::new(
                    Error::ConflictError(format!(
                        "apt-pkg reported exact root '{}' unresolved without a typed missing requirement",
                        root.name
                    )),
                    NativeResolutionSurveyErrorReasonV1::UnresolvedProjectionFailed,
                    debian_explanation(&native, explanation_byte_limit),
                ));
            }
            Ok(NativeResolutionOutcomeV1::Unresolved {
                dependencies: dependencies.into_iter().collect(),
            })
        }
    }
}

fn debian_explanation(
    outcome: &AptResolutionOutcome,
    byte_limit: u64,
) -> NativeResolutionSurveyNativeExplanationV1 {
    record_explanation_build();
    match outcome {
        AptResolutionOutcome::Resolved(source_packages) => {
            let mut explanation = NativeResolutionSurveyNativeExplanationV1::Debian {
                result: NativeResolutionSurveyDebianResultV1::Resolved {
                    packages: Vec::new(),
                },
            };
            let Some(mut budget) =
                NativeExplanationBudget::for_explanation(&explanation, byte_limit)
            else {
                return evidence_withheld();
            };
            let NativeResolutionSurveyNativeExplanationV1::Debian {
                result: NativeResolutionSurveyDebianResultV1::Resolved { packages },
            } = &mut explanation
            else {
                unreachable!("new Debian explanation has the wrong result")
            };
            for source_package in source_packages {
                let package = debian_package(source_package);
                if !budget.retain(&package, !packages.is_empty()) {
                    return evidence_withheld();
                }
                packages.push(package);
            }
            explanation
        }
        AptResolutionOutcome::Unresolved(source_missing) => {
            let mut explanation = NativeResolutionSurveyNativeExplanationV1::Debian {
                result: NativeResolutionSurveyDebianResultV1::Unresolved {
                    missing: Vec::new(),
                },
            };
            let Some(mut budget) =
                NativeExplanationBudget::for_explanation(&explanation, byte_limit)
            else {
                return evidence_withheld();
            };
            let NativeResolutionSurveyNativeExplanationV1::Debian {
                result: NativeResolutionSurveyDebianResultV1::Unresolved { missing },
            } = &mut explanation
            else {
                unreachable!("new Debian explanation has the wrong result")
            };
            for source in source_missing {
                let entry = NativeResolutionSurveyDebianMissingV1 {
                    requiring: debian_package(&source.requiring),
                    relation_kind: match source.kind {
                        AptRelationKind::Depends => "depends",
                        AptRelationKind::PreDepends => "pre_depends",
                        AptRelationKind::Recommends => "recommends",
                        AptRelationKind::Suggests => "suggests",
                        AptRelationKind::Enhances => "enhances",
                        AptRelationKind::Conflicts => "conflicts",
                        AptRelationKind::Breaks => "breaks",
                        AptRelationKind::Replaces => "replaces",
                    }
                    .to_string(),
                    dependency: source.native_text.clone(),
                };
                if !budget.retain(&entry, !missing.is_empty()) {
                    return evidence_withheld();
                }
                missing.push(entry);
            }
            explanation
        }
    }
}

#[cfg(test)]
thread_local! {
    static EXPLANATION_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn record_explanation_build() {
    #[cfg(test)]
    EXPLANATION_BUILDS.with(|count| count.set(count.get() + 1));
}

#[cfg(test)]
pub(super) fn reset_explanation_builds() {
    EXPLANATION_BUILDS.with(|count| count.set(0));
}

#[cfg(test)]
pub(super) fn explanation_builds() -> usize {
    EXPLANATION_BUILDS.with(std::cell::Cell::get)
}

fn evidence_withheld() -> NativeResolutionSurveyNativeExplanationV1 {
    NativeResolutionSurveyNativeExplanationV1::Withheld {
        reason: NativeResolutionSurveyEvidenceWithheldReasonV1::EvidenceBudgetExhausted,
    }
}

fn debian_package(identity: &AptNativeIdentity) -> NativeResolutionSurveyDebianPackageV1 {
    NativeResolutionSurveyDebianPackageV1 {
        name: identity.name.clone(),
        version: identity.version.clone(),
        architecture: identity.architecture.clone(),
    }
}

fn debian_unavailable() -> NativeResolutionSurveyNativeExplanationV1 {
    NativeResolutionSurveyNativeExplanationV1::Debian {
        result: NativeResolutionSurveyDebianResultV1::Unavailable {
            reason: "apt_pkg_returned_no_typed_resolution".to_string(),
        },
    }
}

fn stage_solver_inputs(
    staged: &[std::path::PathBuf],
    directory: &Path,
) -> Result<Vec<std::path::PathBuf>> {
    let solver_directory = directory.join("apt-indexes");
    fs::create_dir(&solver_directory)?;
    staged
        .iter()
        .enumerate()
        .map(|(ordinal, source)| {
            let extension = source.extension().and_then(|value| value.to_str());
            let file_name = match extension {
                Some(extension) if !extension.is_empty() => {
                    format!("member-{ordinal}_Packages.{extension}")
                }
                _ => format!("member-{ordinal}_Packages"),
            };
            let destination = solver_directory.join(file_name);
            fs::copy(source, &destination)?;
            Ok(destination)
        })
        .collect()
}

fn require_debian_package_oracle(manifest: &NativeParityOracleV1) -> Result<()> {
    if manifest.implementation.ecosystem != NativeParityEcosystemV1::Debian
        || manifest.implementation.name != "apt-pkg"
        || manifest.implementation.version != PINNED_APT_PKG_VERSION
        || manifest.implementation.projection_schema != DEBIAN_PARITY_PROJECTION_SCHEMA_V1
    {
        return Err(Error::ConfigError(format!(
            "Debian resolution requires the pinned apt-pkg {PINNED_APT_PKG_VERSION} package oracle"
        )));
    }
    Ok(())
}

fn verify_package_oracle_reprojection(
    profile: &ProfileRevisionV2,
    inputs: &[DebianParityMemberInput<'_>],
    expected: &NativeParityOracleV1,
) -> Result<()> {
    let scratch = tempfile::Builder::new()
        .prefix("conary-debian-resolution-package-proof-")
        .tempdir()?;
    let produced =
        produce_debian_parity_oracle(profile, inputs, &scratch.path().join("package-oracle"))?;
    if &produced != expected {
        return Err(Error::ConflictError(
            "Debian package oracle does not match a fresh apt-pkg projection of its authenticated Packages inputs"
                .to_string(),
        ));
    }
    Ok(())
}

struct PackageResolutionIndex {
    _scratch: tempfile::TempDir,
    database: PathBuf,
}

struct PackageResolutionIndexReader {
    connection: Connection,
}

impl PackageResolutionIndex {
    fn create(package_oracle: &NativeParityOracleReader) -> Result<Self> {
        let scratch = tempfile::Builder::new()
            .prefix("conary-debian-resolution-index-")
            .tempdir()?;
        let database = scratch.path().join("packages.sqlite3");
        let mut connection = Connection::open(&database)?;
        connection.execute_batch(CREATE_INDEX)?;
        let transaction = connection.transaction()?;
        package_oracle.for_each_package(|package| {
            let architecture = package.architecture.as_deref().ok_or_else(|| {
                Error::ConflictError(format!(
                    "Debian package-oracle row '{}' has no architecture",
                    package.name
                ))
            })?;
            transaction.execute(
                "INSERT INTO packages (package_key_sha256, name, version, architecture)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    package.package_key_sha256,
                    package.name,
                    package.version,
                    architecture
                ],
            )?;
            for group in &package.requirement_groups {
                if group.kind != RepositoryRequirementKind::Depends.as_str()
                    && group.kind != RepositoryRequirementKind::PreDepends.as_str()
                {
                    continue;
                }
                let native_text = group.native_text.as_deref().ok_or_else(|| {
                    Error::ConflictError(format!(
                        "Debian required group on '{}' has no native text",
                        package.name
                    ))
                })?;
                transaction.execute(
                    "INSERT INTO requirements (
                         package_key_sha256, kind, native_text, requirement_group_sha256
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        package.package_key_sha256,
                        group.kind,
                        native_text,
                        native_requirement_group_sha256(group)?
                    ],
                )?;
            }
            Ok(())
        })?;
        transaction.commit()?;
        drop(connection);
        Ok(Self {
            _scratch: scratch,
            database,
        })
    }

    fn database(&self) -> &Path {
        &self.database
    }
}

impl PackageResolutionIndexReader {
    fn open(database: &Path) -> Result<Self> {
        Ok(Self {
            connection: Connection::open_with_flags(
                database,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?,
        })
    }

    fn package_key(&self, identity: &AptNativeIdentity) -> Result<String> {
        self.connection
            .query_row(
                "SELECT package_key_sha256 FROM packages
                 WHERE name = ?1 AND version = ?2 AND architecture = ?3",
                params![identity.name, identity.version, identity.architecture],
                |row| row.get(0),
            )
            .map_err(|error| {
                Error::ConflictError(format!(
                    "apt-pkg selected package '{}:{}={}' absent from the bound Debian package oracle: {error}",
                    identity.name, identity.architecture, identity.version
                ))
            })
    }

    fn missing_requirement(
        &self,
        missing: &super::ffi::AptMissingRequirement,
    ) -> Result<NativeUnresolvedDependencyV1> {
        let package_key = self.package_key(&missing.requiring)?;
        let kind = match missing.kind {
            AptRelationKind::Depends => RepositoryRequirementKind::Depends.as_str(),
            AptRelationKind::PreDepends => RepositoryRequirementKind::PreDepends.as_str(),
            _ => {
                return Err(Error::ConflictError(
                    "apt-pkg returned a non-required missing relation".to_string(),
                ));
            }
        };
        let requirement_group_sha256 = self
            .connection
            .query_row(
                "SELECT requirement_group_sha256 FROM requirements
                 WHERE package_key_sha256 = ?1 AND kind = ?2 AND native_text = ?3",
                params![package_key, kind, missing.native_text],
                |row| row.get(0),
            )
            .map_err(|error| {
                Error::ConflictError(format!(
                    "apt-pkg missing dependency '{}' does not bind an exact required group on '{}': {error}",
                    missing.native_text, missing.requiring.name
                ))
            })?;
        Ok(NativeUnresolvedDependencyV1 {
            requiring_package_key_sha256: package_key,
            requirement_group_sha256,
        })
    }
}
