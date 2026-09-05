// crates/conary-core/src/repository/catalog/parity/debian/resolution.rs

//! Independent apt-pkg-backed Debian native resolution evidence production.
//!
//! apt-pkg exposes process-global configuration and system pointers, so safe
//! parallelism uses worker processes rather than shared or merely thread-local
//! handles. Each process builds its own cache from the same staged authenticated
//! inputs and opens the package index read-only. A bounded ordered sink in the
//! parent remains the sole canonical artifact writer. Automatic library calls
//! fall back to one in-process worker when their host executable cannot serve
//! the private worker protocol; explicit parallel requests fail instead.

mod worker;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

pub use worker::run_debian_resolution_worker;
pub(super) use worker::select_debian_worker_launch;
use worker::{DebianResolutionProcess, DebianResolutionRoot, debian_worker_executable};

use super::ffi::{AptNativeIdentity, AptRelationKind, AptResolution, AptResolutionOutcome};
use super::{
    DEBIAN_PARITY_PROJECTION_SCHEMA_V1, DebianParityMemberInput, PINNED_APT_PKG_VERSION,
    produce_debian_parity_oracle, stage_verified_packages, validate_inputs,
};
use crate::error::{Error, Result};
use crate::repository::architecture::NativeResolutionArchitectureDecisionV1;
use crate::repository::catalog::ProfileRevisionV2;
use crate::repository::catalog::parity::resolution_parallel::{
    OrderedResolutionMetrics, ResolutionExplanationLimits, ResolutionWalkImplementationEvidenceV1,
    ResolutionWorkerCount, ResolutionWorkerRequest,
};
use crate::repository::catalog::parity::resolution_producer::{
    NativeResolutionEcosystem, Oracle, ResolutionContext, Survey, produce_resolution,
    resolution_producers, walk_resolution_roots,
};
use crate::repository::catalog::parity::resolution_survey::{
    NativeExplanationBudget, NativeRootResolutionError, NativeRootResolutionResult,
    NativeRootResolutionSuccess, RootOutcomeSink,
};
use crate::repository::catalog::parity::{
    NativeParityEcosystemV1, NativeParityImplementationV1, NativeParityOracleReader,
    NativeParityOracleV1, NativeParityPackageV1, NativeResolutionNotInstallableReasonV1,
    NativeResolutionOracleV1, NativeResolutionOutcomeV1, NativeResolutionPolicyV1,
    NativeResolutionSurveyDebianMissingV1, NativeResolutionSurveyDebianPackageV1,
    NativeResolutionSurveyDebianResultV1, NativeResolutionSurveyErrorReasonV1,
    NativeResolutionSurveyEvidenceWithheldReasonV1, NativeResolutionSurveyNativeExplanationV1,
    NativeResolutionSurveyV1, NativeUnresolvedDependencyV1, native_requirement_group_sha256,
};
use crate::repository::dependency_model::RepositoryRequirementKind;

/// Projection contract for apt-pkg transaction selections and broken strong groups.
pub const DEBIAN_RESOLUTION_PROJECTION_SCHEMA_V3: u32 = 3;

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

resolution_producers!(
    DebianEcosystem,
    DebianParityMemberInput,
    produce_debian_resolution_oracle,
    produce_debian_resolution_oracle_with_workers,
    produce_debian_resolution_survey,
    produce_debian_resolution_survey_with_workers
);

struct DebianEcosystem;

impl<'a> NativeResolutionEcosystem<'a> for DebianEcosystem {
    type Input = DebianParityMemberInput<'a>;
    type Prepared = DebianResolutionPrepared;
    type Worker = DebianResolutionProcess;
    const LABEL: &'static str = "Debian";

    fn prepare(
        context: &ResolutionContext<'_, Self::Input>,
        package_oracle: &NativeParityOracleReader,
    ) -> Result<Self::Prepared> {
        let ResolutionContext {
            profile,
            inputs,
            policy: _,
        } = *context;
        require_debian_package_oracle(package_oracle.manifest())?;
        verify_package_oracle_reprojection(profile, inputs, package_oracle.manifest())?;
        validate_inputs(profile, inputs)?;
        let staging = tempfile::Builder::new()
            .prefix("conary-debian-resolution-")
            .tempdir()?;
        let staged = stage_verified_packages(inputs, staging.path())?;
        let solver_inputs = stage_solver_inputs(&staged, staging.path())?;
        let package_index = PackageResolutionIndex::create(package_oracle)?;
        Ok(DebianResolutionPrepared {
            _staging: staging,
            solver_inputs,
            package_index,
            worker_executable: None,
        })
    }
    fn implementation() -> NativeParityImplementationV1 {
        NativeParityImplementationV1 {
            ecosystem: NativeParityEcosystemV1::Debian,
            name: "apt-pkg".to_string(),
            version: PINNED_APT_PKG_VERSION.to_string(),
            projection_schema: DEBIAN_RESOLUTION_PROJECTION_SCHEMA_V3,
        }
    }
    fn select_workers(
        prepared: &mut Self::Prepared,
        request: ResolutionWorkerRequest,
        workers: ResolutionWorkerCount,
    ) -> Result<ResolutionWorkerCount> {
        let (workers, executable) =
            select_debian_worker_launch(request, workers, debian_worker_executable())?;
        prepared.worker_executable = executable;
        Ok(workers)
    }
    fn open_worker(
        context: &ResolutionContext<'_, Self::Input>,
        prepared: &Self::Prepared,
    ) -> Result<Self::Worker> {
        let executable = prepared.worker_executable.as_deref().ok_or_else(|| {
            Error::InternalError("parallel Debian walk has no worker executable".to_string())
        })?;
        DebianResolutionProcess::spawn(
            executable,
            &prepared.solver_inputs,
            prepared.package_index.database(),
            &context.policy.architecture,
        )
    }
    fn resolve_root(
        _context: &ResolutionContext<'_, Self::Input>,
        worker: &mut Self::Worker,
        root: &NativeParityPackageV1,
        limits: ResolutionExplanationLimits,
    ) -> Result<NativeRootResolutionResult> {
        worker.resolve(root, limits)
    }
    fn walk(
        context: &ResolutionContext<'_, Self::Input>,
        prepared: &Self::Prepared,
        package_oracle: &NativeParityOracleReader,
        mut sink: RootOutcomeSink<'_>,
        workers: ResolutionWorkerCount,
    ) -> Result<OrderedResolutionMetrics> {
        if workers.get() == 1 {
            let started = std::time::Instant::now();
            let mut apt =
                AptResolution::open(&prepared.solver_inputs, &context.policy.architecture)?;
            let package_index =
                PackageResolutionIndexReader::open(prepared.package_index.database())?;
            let load_milliseconds =
                u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            package_oracle.for_each_package(|root| {
                let projected = DebianResolutionRoot::from(&root);
                let result = resolve_exact_root(
                    &mut apt,
                    &package_index,
                    &projected,
                    context.policy,
                    sink.explanation_limits(),
                );
                sink.root(&root, result)
            })?;
            return Ok(OrderedResolutionMetrics {
                worker_load_milliseconds: vec![load_milliseconds],
            });
        }
        walk_resolution_roots::<Self>(context, prepared, package_oracle, sink, workers)
    }
}

struct DebianResolutionPrepared {
    _staging: tempfile::TempDir,
    solver_inputs: Vec<PathBuf>,
    package_index: PackageResolutionIndex,
    worker_executable: Option<PathBuf>,
}

fn resolve_exact_root(
    apt: &mut AptResolution,
    package_index: &PackageResolutionIndexReader,
    root: &DebianResolutionRoot,
    policy: &NativeResolutionPolicyV1,
    explanation_limits: ResolutionExplanationLimits,
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
        .map(|decision| decision.is_admitted())
    {
        Ok(true) => {}
        Ok(false) => {
            return Ok(NativeRootResolutionSuccess::plain(
                NativeResolutionOutcomeV1::NotInstallable {
                    reason: NativeResolutionNotInstallableReasonV1::ArchitectureExcluded,
                },
            ));
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
                        debian_explanation(&native, explanation_limits.failure_bytes()),
                    )
                })?;
            if !closure.contains(&root.package_key_sha256) {
                return Ok(NativeRootResolutionSuccess::explained(
                    NativeResolutionOutcomeV1::NotInstallable {
                        reason: NativeResolutionNotInstallableReasonV1::ConflictingClosure,
                    },
                    debian_explanation(&native, explanation_limits.diagnostic_outcome_bytes()),
                ));
            }
            Ok(NativeRootResolutionSuccess::plain(
                NativeResolutionOutcomeV1::Resolved {
                    closure_package_keys_sha256: closure.into_iter().collect(),
                },
            ))
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
                        debian_explanation(&native, explanation_limits.failure_bytes()),
                    )
                })?;
            if dependencies.is_empty() {
                return Err(NativeRootResolutionError::new(
                    Error::ConflictError(format!(
                        "apt-pkg reported exact root '{}' unresolved without a typed missing requirement",
                        root.name
                    )),
                    NativeResolutionSurveyErrorReasonV1::UnresolvedProjectionFailed,
                    debian_explanation(&native, explanation_limits.failure_bytes()),
                ));
            }
            Ok(NativeRootResolutionSuccess::plain(
                NativeResolutionOutcomeV1::Unresolved {
                    dependencies: dependencies.into_iter().collect(),
                },
            ))
        }
        AptResolutionOutcome::ConflictingClosure => Ok(NativeRootResolutionSuccess::explained(
            NativeResolutionOutcomeV1::NotInstallable {
                reason: NativeResolutionNotInstallableReasonV1::ConflictingClosure,
            },
            debian_explanation(&native, explanation_limits.diagnostic_outcome_bytes()),
        )),
    }
}

fn debian_explanation(
    outcome: &AptResolutionOutcome,
    byte_limit: u64,
) -> NativeResolutionSurveyNativeExplanationV1 {
    if byte_limit == 0 {
        return evidence_withheld();
    }
    record_explanation_build();
    match outcome {
        AptResolutionOutcome::Resolved(source_packages) => {
            let explanation = NativeResolutionSurveyNativeExplanationV1::Debian {
                result: NativeResolutionSurveyDebianResultV1::Resolved {
                    packages: Vec::new(),
                },
            };
            let Some(mut budget) =
                NativeExplanationBudget::for_explanation(&explanation, byte_limit)
            else {
                return evidence_withheld();
            };
            let mut packages = Vec::new();
            for source_package in source_packages {
                let package = debian_package(source_package);
                if !budget.retain(&package, !packages.is_empty()) {
                    return evidence_withheld();
                }
                packages.push(package);
            }
            NativeResolutionSurveyNativeExplanationV1::Debian {
                result: NativeResolutionSurveyDebianResultV1::Resolved { packages },
            }
        }
        AptResolutionOutcome::Unresolved(source_missing) => {
            let explanation = NativeResolutionSurveyNativeExplanationV1::Debian {
                result: NativeResolutionSurveyDebianResultV1::Unresolved {
                    missing: Vec::new(),
                },
            };
            let Some(mut budget) =
                NativeExplanationBudget::for_explanation(&explanation, byte_limit)
            else {
                return evidence_withheld();
            };
            let mut missing = Vec::new();
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
            NativeResolutionSurveyNativeExplanationV1::Debian {
                result: NativeResolutionSurveyDebianResultV1::Unresolved { missing },
            }
        }
        AptResolutionOutcome::ConflictingClosure => {
            NativeResolutionSurveyNativeExplanationV1::Debian {
                result: NativeResolutionSurveyDebianResultV1::Conflicts {
                    detail_unavailable_reason: Some(
                        "apt_pkg_exposes_conflict_class_state_without_a_stable_rule_graph"
                            .to_string(),
                    ),
                },
            }
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
