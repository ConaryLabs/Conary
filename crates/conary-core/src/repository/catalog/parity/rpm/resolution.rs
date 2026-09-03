// crates/conary-core/src/repository/catalog/parity/rpm/resolution.rs

//! Independent libsolv-backed RPM native resolution evidence production.
//!
//! Each worker loads a private libsolv pool from the same staged, authenticated
//! metadata and opens its own read-only package-index connection. Pool and
//! solver pointers never cross threads. A bounded ordered sink alone writes
//! roots, preserving package-oracle order and canonical bytes.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use super::ffi::{RequiredKind, SolvProblem, SolvProblemRule, SolvResolution};
use super::{
    PINNED_LIBSOLV_VERSION, RPM_PARITY_PROJECTION_SCHEMA_V1, RpmParityMemberInput, SolvPool,
    produce_rpm_parity_oracle, project_package, project_requirement, stage_verified_metadata,
    validate_inputs,
};
use crate::error::{Error, Result};
use crate::repository::architecture::NativeResolutionArchitectureDecisionV1;
use crate::repository::catalog::ProfileRevisionV2;
use crate::repository::catalog::parity::resolution_parallel::{
    OrderedResolutionMetrics, RESOLUTION_WALK_MEMORY_BUDGET_BYTES, RESOLUTION_WORKER_RSS_BYTES,
    ResolutionWalkImplementationEvidenceV1, ResolutionWorkerCount, ResolutionWorkerRequest,
    walk_ordered_parallel,
};
use crate::repository::catalog::parity::resolution_survey::{
    NativeResolutionSurveyCollector, NativeRootResolutionError, NativeRootResolutionResult,
    RootOutcomeSink,
};
use crate::repository::catalog::parity::{
    NATIVE_RESOLUTION_ROOT_FILE_NAME, NativeParityEcosystemV1, NativeParityImplementationV1,
    NativeParityOracleReader, NativeParityOracleV1, NativeResolutionArchitectureAdmissionV1,
    NativeResolutionInstalledStateV1, NativeResolutionNotInstallableReasonV1,
    NativeResolutionOracleV1, NativeResolutionOracleWriter, NativeResolutionOutcomeV1,
    NativeResolutionPolicyV1, NativeResolutionProviderPolicyV1,
    NativeResolutionRequirementPolicyV1, NativeResolutionRootPolicyV1,
    NativeResolutionSurveyErrorReasonV1, NativeResolutionSurveyNativeExplanationV1,
    NativeResolutionSurveyV1, NativeUnresolvedDependencyV1, native_requirement_group_sha256,
    verify_native_parity_oracle_bundle, verify_native_resolution_oracle_bundle,
    write_native_resolution_oracle_manifest, write_native_resolution_survey,
};
use crate::repository::dependency_model::RepositoryRequirementKind;

mod evidence;

use evidence::rpm_explanation;

/// Projection contract for libsolv transaction results and typed problem rules.
pub const RPM_RESOLUTION_PROJECTION_SCHEMA_V4: u32 = 4;

const SOLVER_RULE_PKG_NOT_INSTALLABLE: i32 = 0x101;
const SOLVER_RULE_PKG_NOTHING_PROVIDES_DEP: i32 = 0x102;
const SOLVER_RULE_PKG_REQUIRES: i32 = 0x103;
const SOLVER_RULE_PKG_CONFLICTS: i32 = 0x105;
const SOLVER_RULE_JOB: i32 = 0x400;
const SOLVER_RULE_JOB_UNSUPPORTED: i32 = 0x404;
const SOLVER_RULE_INFARCH: i32 = 0x600;
const SOLVER_RULE_STRICT_REPO_PRIORITY: i32 = 0xd00;

const CREATE_INDEX: &str = "
CREATE TABLE oracle_packages (
    package_key_sha256 TEXT PRIMARY KEY,
    selected_native_index INTEGER
) STRICT;
CREATE TABLE native_packages (
    native_index INTEGER PRIMARY KEY,
    package_key_sha256 TEXT NOT NULL REFERENCES oracle_packages(package_key_sha256)
) STRICT;
";

/// Produce and independently reopen one strict RPM resolution parity bundle.
pub fn produce_rpm_resolution_oracle(
    profile: &ProfileRevisionV2,
    inputs: &[RpmParityMemberInput<'_>],
    package_oracle_directory: &Path,
    architecture: &str,
    output: &Path,
) -> Result<NativeResolutionOracleV1> {
    produce_rpm_resolution_oracle_with_workers(
        profile,
        inputs,
        package_oracle_directory,
        architecture,
        output,
        ResolutionWorkerRequest::Automatic,
    )
    .map(|(manifest, _)| manifest)
}

/// Produce a strict RPM bundle with an explicit or capacity-derived worker request.
pub fn produce_rpm_resolution_oracle_with_workers(
    profile: &ProfileRevisionV2,
    inputs: &[RpmParityMemberInput<'_>],
    package_oracle_directory: &Path,
    architecture: &str,
    output: &Path,
    worker_request: ResolutionWorkerRequest,
) -> Result<(
    NativeResolutionOracleV1,
    ResolutionWalkImplementationEvidenceV1,
)> {
    let ResolutionProduct::Oracle(manifest) = produce_rpm_resolution(
        profile,
        inputs,
        package_oracle_directory,
        architecture,
        ResolutionDestination::Oracle(output),
        worker_request,
    )?
    else {
        unreachable!("RPM oracle destination returned survey")
    };
    Ok(manifest)
}

/// Walk every exact RPM root and write one diagnostics-only failure survey.
pub fn produce_rpm_resolution_survey(
    profile: &ProfileRevisionV2,
    inputs: &[RpmParityMemberInput<'_>],
    package_oracle_directory: &Path,
    architecture: &str,
    output: &Path,
) -> Result<NativeResolutionSurveyV1> {
    produce_rpm_resolution_survey_with_workers(
        profile,
        inputs,
        package_oracle_directory,
        architecture,
        output,
        ResolutionWorkerRequest::Automatic,
    )
    .map(|(survey, _)| survey)
}

/// Produce an RPM survey with an explicit or capacity-derived worker request.
pub fn produce_rpm_resolution_survey_with_workers(
    profile: &ProfileRevisionV2,
    inputs: &[RpmParityMemberInput<'_>],
    package_oracle_directory: &Path,
    architecture: &str,
    output: &Path,
    worker_request: ResolutionWorkerRequest,
) -> Result<(
    NativeResolutionSurveyV1,
    ResolutionWalkImplementationEvidenceV1,
)> {
    let ResolutionProduct::Survey(survey) = produce_rpm_resolution(
        profile,
        inputs,
        package_oracle_directory,
        architecture,
        ResolutionDestination::Survey(output),
        worker_request,
    )?
    else {
        unreachable!("RPM survey destination returned oracle")
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

fn produce_rpm_resolution(
    profile: &ProfileRevisionV2,
    inputs: &[RpmParityMemberInput<'_>],
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
    require_rpm_package_oracle(package_oracle.manifest())?;
    verify_package_oracle_reprojection(profile, inputs, package_oracle.manifest())?;

    validate_inputs(profile, inputs)?;
    let staging = tempfile::Builder::new()
        .prefix("conary-rpm-resolution-")
        .tempdir()?;
    let staged = stage_verified_metadata(inputs, staging.path())?;
    let index_pool = load_resolution_pool(profile, &staged, architecture)?;
    let package_index =
        PackageResolutionIndex::create(profile, inputs, &index_pool, &package_oracle)?;
    drop(index_pool);
    let workers = worker_request.resolve(
        package_oracle.manifest().artifact.counts.packages,
        RESOLUTION_WALK_MEMORY_BUDGET_BYTES,
        RESOLUTION_WORKER_RSS_BYTES,
    )?;

    let implementation = NativeParityImplementationV1 {
        ecosystem: NativeParityEcosystemV1::Rpm,
        name: "libsolv".to_string(),
        version: PINNED_LIBSOLV_VERSION.to_string(),
        projection_schema: RPM_RESOLUTION_PROJECTION_SCHEMA_V4,
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
                profile,
                &staged,
                &policy,
                RootOutcomeSink::Strict(&mut writer),
                workers,
            )?;
            let manifest = writer.finish()?;
            write_native_resolution_oracle_manifest(output, &manifest)?;
            let reopened =
                verify_native_resolution_oracle_bundle(output, profile, &package_oracle)?;
            if reopened.manifest() != &manifest {
                return Err(Error::InternalError(
                    "reopened RPM resolution manifest differs from produced manifest".to_string(),
                ));
            }
            Ok(ResolutionProduct::Oracle((
                manifest,
                implementation_evidence(workers, metrics)?,
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
                profile,
                &staged,
                &policy,
                RootOutcomeSink::Survey(&mut collector),
                workers,
            )?;
            let survey = collector.finish()?;
            write_native_resolution_survey(output, &survey)?;
            Ok(ResolutionProduct::Survey((
                survey,
                implementation_evidence(workers, metrics)?,
            )))
        }
    }
}

fn walk_resolution_roots(
    package_oracle: &NativeParityOracleReader,
    package_index: &PackageResolutionIndex,
    profile: &ProfileRevisionV2,
    staged: &[super::StagedRpmMetadata],
    policy: &NativeResolutionPolicyV1,
    mut sink: RootOutcomeSink<'_>,
    workers: ResolutionWorkerCount,
) -> Result<OrderedResolutionMetrics> {
    let explanation_byte_limit = sink.explanation_byte_limit();
    walk_ordered_parallel(
        package_oracle,
        workers,
        explanation_byte_limit,
        |_| {
            Ok(RpmResolutionWorker {
                pool: load_resolution_pool(profile, staged, &policy.architecture)?,
                package_index: package_index.worker()?,
            })
        },
        |worker, root, byte_limit| match worker
            .package_index
            .selected_native_index(&root.package_key_sha256)
        {
            Ok(root_index) => resolve_exact_root(
                &mut worker.pool,
                &worker.package_index,
                root_index,
                root,
                policy,
                byte_limit,
            ),
            Err(error) => Err(NativeRootResolutionError::new(
                error,
                NativeResolutionSurveyErrorReasonV1::ExactRootProjectionFailed,
                NativeResolutionSurveyNativeExplanationV1::Rpm {
                    problems: Vec::new(),
                },
            )),
        },
        |root, result| sink.root(root, result),
    )
}

struct RpmResolutionWorker {
    pool: SolvPool,
    package_index: PackageResolutionIndexReader,
}

fn load_resolution_pool(
    profile: &ProfileRevisionV2,
    staged: &[super::StagedRpmMetadata],
    architecture: &str,
) -> Result<SolvPool> {
    let mut pool = SolvPool::create()?;
    if SolvPool::version()? != PINNED_LIBSOLV_VERSION {
        return Err(Error::ConfigError(format!(
            "RPM resolution requires libsolv {PINNED_LIBSOLV_VERSION}"
        )));
    }
    for (ordinal, (member, metadata)) in profile.members.iter().zip(staged).enumerate() {
        let ordinal = u32::try_from(ordinal)
            .map_err(|_| Error::ConfigError("RPM member ordinal exceeds u32".to_string()))?;
        pool.load(
            &format!("conary-member-{ordinal}"),
            &metadata.primary,
            &metadata.filelists,
            ordinal,
            member.precedence,
        )?;
    }
    pool.set_architecture(architecture)?;
    Ok(pool)
}

fn implementation_evidence(
    workers: ResolutionWorkerCount,
    metrics: OrderedResolutionMetrics,
) -> Result<ResolutionWalkImplementationEvidenceV1> {
    ResolutionWalkImplementationEvidenceV1::new(
        workers,
        metrics.worker_load_milliseconds,
        RESOLUTION_WALK_MEMORY_BUDGET_BYTES,
        RESOLUTION_WORKER_RSS_BYTES,
    )
}

fn require_rpm_package_oracle(manifest: &NativeParityOracleV1) -> Result<()> {
    if manifest.implementation.ecosystem != NativeParityEcosystemV1::Rpm
        || manifest.implementation.name != "libsolv"
        || manifest.implementation.version != PINNED_LIBSOLV_VERSION
        || manifest.implementation.projection_schema != RPM_PARITY_PROJECTION_SCHEMA_V1
    {
        return Err(Error::ConfigError(format!(
            "RPM resolution requires the pinned libsolv {PINNED_LIBSOLV_VERSION} package oracle"
        )));
    }
    Ok(())
}

fn verify_package_oracle_reprojection(
    profile: &ProfileRevisionV2,
    inputs: &[RpmParityMemberInput<'_>],
    expected: &NativeParityOracleV1,
) -> Result<()> {
    let scratch = tempfile::Builder::new()
        .prefix("conary-rpm-resolution-package-proof-")
        .tempdir()?;
    let produced =
        produce_rpm_parity_oracle(profile, inputs, &scratch.path().join("package-oracle"))?;
    if &produced != expected {
        return Err(Error::ConflictError(
            "RPM package oracle does not match a fresh libsolv projection of its authenticated metadata"
                .to_string(),
        ));
    }
    Ok(())
}

struct PackageResolutionIndex {
    _scratch: tempfile::TempDir,
    database: std::path::PathBuf,
}

struct PackageResolutionIndexReader {
    connection: Connection,
}

impl PackageResolutionIndex {
    fn create(
        profile: &ProfileRevisionV2,
        inputs: &[RpmParityMemberInput<'_>],
        pool: &SolvPool,
        package_oracle: &NativeParityOracleReader,
    ) -> Result<Self> {
        let scratch = tempfile::Builder::new()
            .prefix("conary-rpm-resolution-index-")
            .tempdir()?;
        let database = scratch.path().join("packages.sqlite3");
        let mut connection = Connection::open(&database)?;
        connection.execute_batch(CREATE_INDEX)?;
        let transaction = connection.transaction()?;
        package_oracle.for_each_package(|package| {
            transaction.execute(
                "INSERT INTO oracle_packages (package_key_sha256) VALUES (?1)",
                params![package.package_key_sha256],
            )?;
            Ok(())
        })?;
        for native_index in 0..pool.package_count() {
            let native = pool.package(native_index)?;
            let package = project_package(profile, inputs, &native)?;
            let expected: Option<String> = transaction
                .query_row(
                    "SELECT package_key_sha256 FROM oracle_packages WHERE package_key_sha256 = ?1",
                    [&package.package_key_sha256],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(expected) = expected else {
                return Err(Error::ConflictError(format!(
                    "libsolv native package '{}' is absent from the bound RPM package oracle",
                    package.name
                )));
            };
            debug_assert_eq!(expected, package.package_key_sha256);
            let native_index = i64::try_from(native_index).map_err(|_| {
                Error::ConfigError("libsolv package index exceeds SQLite i64".to_string())
            })?;
            transaction.execute(
                "INSERT INTO native_packages (native_index, package_key_sha256)
                 VALUES (?1, ?2)",
                params![native_index, package.package_key_sha256],
            )?;
            transaction.execute(
                "UPDATE oracle_packages SET selected_native_index = ?1
                 WHERE package_key_sha256 = ?2 AND selected_native_index IS NULL",
                params![native_index, package.package_key_sha256],
            )?;
        }
        let missing: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM oracle_packages WHERE selected_native_index IS NULL",
            [],
            |row| row.get(0),
        )?;
        if missing != 0 {
            return Err(Error::ConflictError(format!(
                "RPM package oracle contains {missing} packages absent from the native solver pool"
            )));
        }
        transaction.commit()?;
        drop(connection);
        Ok(Self {
            _scratch: scratch,
            database,
        })
    }

    fn worker(&self) -> Result<PackageResolutionIndexReader> {
        Ok(PackageResolutionIndexReader {
            connection: Connection::open_with_flags(
                &self.database,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?,
        })
    }
}

impl PackageResolutionIndexReader {
    fn selected_native_index(&self, package_key: &str) -> Result<usize> {
        let index: i64 = self.connection.query_row(
            "SELECT selected_native_index FROM oracle_packages WHERE package_key_sha256 = ?1",
            [package_key],
            |row| row.get(0),
        )?;
        usize::try_from(index).map_err(|_| {
            Error::InternalError("negative or oversized native RPM package index".to_string())
        })
    }

    fn package_key(&self, native_index: usize) -> Result<String> {
        let native_index = i64::try_from(native_index).map_err(|_| {
            Error::InternalError("native RPM package index exceeds SQLite i64".to_string())
        })?;
        self.connection
            .query_row(
                "SELECT package_key_sha256 FROM native_packages WHERE native_index = ?1",
                [native_index],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }
}

fn resolve_exact_root(
    pool: &mut SolvPool,
    package_index: &PackageResolutionIndexReader,
    root_index: usize,
    root: &crate::repository::catalog::parity::NativeParityPackageV1,
    policy: &NativeResolutionPolicyV1,
    explanation_byte_limit: u64,
) -> NativeRootResolutionResult {
    let root_architecture = root.architecture.as_deref().ok_or_else(|| {
        NativeRootResolutionError::new(
            Error::ConflictError(format!(
                "RPM package-oracle root '{}' has no architecture",
                root.name
            )),
            NativeResolutionSurveyErrorReasonV1::ExactRootProjectionFailed,
            NativeResolutionSurveyNativeExplanationV1::Rpm {
                problems: Vec::new(),
            },
        )
    })?;
    let root_is_admitted = match policy
        .architecture_admission
        .admits(&root.source_profile, root.version_scheme, root_architecture)
        .and_then(NativeResolutionArchitectureDecisionV1::into_result)
    {
        Ok(NativeResolutionArchitectureDecisionV1::Admitted) => true,
        Ok(NativeResolutionArchitectureDecisionV1::Excluded { .. }) => false,
        Ok(NativeResolutionArchitectureDecisionV1::UnknownArchitectureToken { .. }) => {
            unreachable!("unknown admission decision returned from into_result")
        }
        Err(error) => {
            return Err(NativeRootResolutionError::new(
                error,
                NativeResolutionSurveyErrorReasonV1::UnknownArchitectureToken,
                NativeResolutionSurveyNativeExplanationV1::Rpm {
                    problems: Vec::new(),
                },
            ));
        }
    };
    let resolution = pool.solve(root_index).map_err(|error| {
        NativeRootResolutionError::new(
            error,
            NativeResolutionSurveyErrorReasonV1::NativeSolverFailed,
            NativeResolutionSurveyNativeExplanationV1::Rpm {
                problems: Vec::new(),
            },
        )
    })?;
    match resolution {
        SolvResolution::Resolved(packages) => {
            if !root_is_admitted {
                return Err(NativeRootResolutionError::new(
                    Error::ConflictError(format!(
                        "libsolv resolved architecture-excluded exact root '{}'",
                        root.name
                    )),
                    NativeResolutionSurveyErrorReasonV1::UnresolvedProjectionFailed,
                    NativeResolutionSurveyNativeExplanationV1::Rpm {
                        problems: Vec::new(),
                    },
                ));
            }
            let closure = packages
                .into_iter()
                .map(|index| package_index.package_key(index))
                .collect::<Result<BTreeSet<_>>>()
                .map_err(|error| {
                    NativeRootResolutionError::new(
                        error,
                        NativeResolutionSurveyErrorReasonV1::ResolvedClosureProjectionFailed,
                        NativeResolutionSurveyNativeExplanationV1::Rpm {
                            problems: Vec::new(),
                        },
                    )
                })?;
            if !closure.contains(&root.package_key_sha256) {
                return Err(NativeRootResolutionError::new(
                    Error::ConflictError(format!(
                        "libsolv closure for '{}' omits its exact root",
                        root.name
                    )),
                    NativeResolutionSurveyErrorReasonV1::ResolvedClosureOmittedRoot,
                    NativeResolutionSurveyNativeExplanationV1::Rpm {
                        problems: Vec::new(),
                    },
                ));
            }
            Ok(NativeResolutionOutcomeV1::Resolved {
                closure_package_keys_sha256: closure.into_iter().collect(),
            })
        }
        SolvResolution::Unresolved(problems) => {
            match unresolved_outcome(pool, package_index, root_index, root, policy, &problems) {
                Ok(outcome) => Ok(outcome),
                Err(error) => {
                    let explanation =
                        rpm_explanation(pool, package_index, &problems, explanation_byte_limit);
                    Err(NativeRootResolutionError::new(
                        error,
                        NativeResolutionSurveyErrorReasonV1::UnresolvedProjectionFailed,
                        explanation,
                    ))
                }
            }
        }
    }
}

fn unresolved_outcome(
    pool: &SolvPool,
    package_index: &PackageResolutionIndexReader,
    root_index: usize,
    root: &crate::repository::catalog::parity::NativeParityPackageV1,
    policy: &NativeResolutionPolicyV1,
    problems: &[SolvProblem],
) -> Result<NativeResolutionOutcomeV1> {
    if let Some(outcome) = architecture_excluded_outcome(root_index, root, policy, problems)? {
        return Ok(outcome);
    }
    let mut dependencies = BTreeSet::new();
    let shadowed = pool
        .strict_shadowed_packages()?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let visibility = StrictVisibility::derive(pool, root_index, &shadowed, problems)?;
    for problem in problems {
        let problem_dependencies = project_unresolved_problem(
            pool,
            package_index,
            root,
            &policy.architecture,
            &problem.rules,
            &visibility,
        )?;
        dependencies.extend(problem_dependencies);
    }
    if dependencies.is_empty() {
        return Err(Error::ConflictError(format!(
            "libsolv reported exact root '{}' unresolved without a typed missing requirement",
            root.name
        )));
    }
    Ok(NativeResolutionOutcomeV1::Unresolved {
        dependencies: dependencies.into_iter().collect(),
    })
}

fn architecture_excluded_outcome(
    root_index: usize,
    root: &crate::repository::catalog::parity::NativeParityPackageV1,
    policy: &NativeResolutionPolicyV1,
    problems: &[SolvProblem],
) -> Result<Option<NativeResolutionOutcomeV1>> {
    let architecture = root.architecture.as_deref().ok_or_else(|| {
        Error::ConflictError(format!(
            "RPM package-oracle root '{}' has no architecture",
            root.name
        ))
    })?;
    let admitted = match policy
        .architecture_admission
        .admits(&root.source_profile, root.version_scheme, architecture)?
        .into_result()?
    {
        NativeResolutionArchitectureDecisionV1::Admitted => true,
        NativeResolutionArchitectureDecisionV1::Excluded { .. } => false,
        NativeResolutionArchitectureDecisionV1::UnknownArchitectureToken { .. } => {
            unreachable!("unknown admission decision returned from into_result")
        }
    };
    let has_not_installable = problems
        .iter()
        .flat_map(|problem| &problem.rules)
        .any(|rule| rule.rule_type == SOLVER_RULE_PKG_NOT_INSTALLABLE);
    if !has_not_installable {
        if admitted {
            return Ok(None);
        }
        return Err(Error::ConflictError(format!(
            "libsolv rejected architecture-excluded exact root '{}' without SOLVER_RULE_PKG_NOT_INSTALLABLE",
            root.name
        )));
    }
    if admitted {
        return Err(Error::ConfigError(format!(
            "libsolv found admitted exact root '{}' not installable under target architecture '{}'",
            root.name, policy.architecture
        )));
    }
    for rule in problems.iter().flat_map(|problem| &problem.rules) {
        match rule.rule_type {
            SOLVER_RULE_JOB => {}
            SOLVER_RULE_PKG_NOT_INSTALLABLE
                if rule.from_index == Some(root_index)
                    && rule.to_index.is_none()
                    && rule.dependency == 0 => {}
            rule_type => {
                return Err(Error::ConflictError(format!(
                    "libsolv architecture exclusion for exact root '{}' carried unexpected rule {rule_type:#x} (from={:?}, to={:?}, dependency={})",
                    root.name, rule.from_index, rule.to_index, rule.dependency
                )));
            }
        }
    }
    Ok(Some(NativeResolutionOutcomeV1::NotInstallable {
        reason: NativeResolutionNotInstallableReasonV1::ArchitectureExcluded,
    }))
}

#[cfg(test)]
pub(super) fn reset_explanation_builds() {
    evidence::reset_explanation_builds();
}

#[cfg(test)]
pub(super) fn explanation_builds() -> usize {
    evidence::explanation_builds()
}

/// Requiring packages that Conary's candidate resolver can also reach under
/// the strict solve's repository-priority authority.
struct StrictVisibility {
    /// The exact root plus every provider reachable from it through hard
    /// required edges that strict repository priority does not shadow.
    reachable: BTreeSet<usize>,
    /// Packages owning a missing-dependency or required edge of their own.
    edge_owners: BTreeSet<usize>,
}

impl StrictVisibility {
    /// `shadowed` comes from the strict solve's own priority rules because a
    /// problem explanation may cite a shadowed provider's other defects
    /// without citing the strict-priority rule itself.
    fn derive<'a>(
        pool: &SolvPool,
        root_index: usize,
        shadowed: &BTreeSet<usize>,
        problems: impl IntoIterator<Item = &'a SolvProblem>,
    ) -> Result<Self> {
        let mut edge_owners = BTreeSet::new();
        let mut required: BTreeMap<usize, Vec<i32>> = BTreeMap::new();
        for rule in problems.into_iter().flat_map(|problem| &problem.rules) {
            let Some(from_index) = rule.from_index else {
                continue;
            };
            match rule.rule_type {
                SOLVER_RULE_PKG_NOTHING_PROVIDES_DEP => {
                    edge_owners.insert(from_index);
                }
                SOLVER_RULE_PKG_REQUIRES if rule.dependency != 0 => {
                    edge_owners.insert(from_index);
                    required
                        .entry(from_index)
                        .or_default()
                        .push(rule.dependency);
                }
                _ => {}
            }
        }
        let mut reachable = BTreeSet::from([root_index]);
        let mut frontier = vec![root_index];
        while let Some(index) = frontier.pop() {
            for dependency in required.get(&index).into_iter().flatten() {
                for provider in pool.providers(*dependency)? {
                    if !shadowed.contains(&provider) && reachable.insert(provider) {
                        frontier.push(provider);
                    }
                }
            }
        }
        Ok(Self {
            reachable,
            edge_owners,
        })
    }

    fn requiring_package_is_visible(&self, requiring_index: Option<usize>) -> bool {
        requiring_index.is_some_and(|index| self.reachable.contains(&index))
    }

    /// A required edge is terminal for Conary only when none of its visible
    /// providers explains the failure with an edge of its own; otherwise the
    /// deeper edge is the one Conary reports.
    fn required_edge_is_terminal(&self, pool: &SolvPool, dependency: i32) -> Result<bool> {
        Ok(!pool.providers(dependency)?.iter().any(|provider| {
            self.reachable.contains(provider) && self.edge_owners.contains(provider)
        }))
    }
}

fn project_unresolved_problem(
    pool: &SolvPool,
    package_index: &PackageResolutionIndexReader,
    root: &crate::repository::catalog::parity::NativeParityPackageV1,
    architecture: &str,
    rules: &[SolvProblemRule],
    visibility: &StrictVisibility,
) -> Result<BTreeSet<NativeUnresolvedDependencyV1>> {
    let mut dependencies = BTreeSet::new();
    for rule in rules {
        match rule.rule_type {
            SOLVER_RULE_PKG_NOTHING_PROVIDES_DEP => {
                let dependency = project_unresolved_dependency(
                    pool,
                    package_index,
                    root,
                    rule.from_index,
                    rule.dependency,
                    "missing-dependency",
                )?;
                if visibility.requiring_package_is_visible(rule.from_index) {
                    dependencies.insert(dependency);
                }
            }
            SOLVER_RULE_PKG_REQUIRES => {
                let dependency = project_unresolved_dependency(
                    pool,
                    package_index,
                    root,
                    rule.from_index,
                    rule.dependency,
                    "uninstallable requirement",
                )?;
                if visibility.requiring_package_is_visible(rule.from_index)
                    && visibility.required_edge_is_terminal(pool, rule.dependency)?
                {
                    dependencies.insert(dependency);
                }
            }
            SOLVER_RULE_JOB => {}
            SOLVER_RULE_STRICT_REPO_PRIORITY => {
                let excluded_index = rule.from_index.ok_or_else(|| {
                    Error::ConflictError(format!(
                        "libsolv strict-priority rule for '{}' has no excluded package",
                        root.name
                    ))
                })?;
                if rule.to_index.is_some() || rule.dependency != 0 {
                    return Err(Error::ConflictError(format!(
                        "libsolv strict-priority rule for '{}' has unexpected target or dependency",
                        root.name
                    )));
                }
                package_index.package_key(excluded_index)?;
            }
            SOLVER_RULE_PKG_NOT_INSTALLABLE => {
                return Err(Error::ConfigError(format!(
                    "libsolv found exact root '{}' not installable under target architecture '{}'",
                    root.name, architecture
                )));
            }
            SOLVER_RULE_PKG_CONFLICTS => {
                return Err(Error::ConflictError(format!(
                    "libsolv found exact root '{}' unsatisfiable with problem rule {:#x} (from={:?}, to={:?}, dependency={})",
                    root.name, rule.rule_type, rule.from_index, rule.to_index, rule.dependency
                )));
            }
            SOLVER_RULE_JOB_UNSUPPORTED | SOLVER_RULE_INFARCH => {
                return Err(Error::ConfigError(format!(
                    "libsolv rejected exact root '{}' for target architecture '{}' with rule {:#x} (from={:?}, to={:?}, dependency={})",
                    root.name,
                    architecture,
                    rule.rule_type,
                    rule.from_index,
                    rule.to_index,
                    rule.dependency
                )));
            }
            rule_type => {
                return Err(Error::ConflictError(format!(
                    "libsolv found unexpected problem rule {rule_type:#x} while resolving exact root '{}' (from={:?}, to={:?}, dependency={})",
                    root.name, rule.from_index, rule.to_index, rule.dependency
                )));
            }
        }
    }
    Ok(dependencies)
}

fn project_unresolved_dependency(
    pool: &SolvPool,
    package_index: &PackageResolutionIndexReader,
    root: &crate::repository::catalog::parity::NativeParityPackageV1,
    requiring_index: Option<usize>,
    dependency: i32,
    rule_description: &str,
) -> Result<NativeUnresolvedDependencyV1> {
    let requiring_index = requiring_index.ok_or_else(|| {
        Error::ConflictError(format!(
            "libsolv {rule_description} rule for '{}' has no requiring package",
            root.name
        ))
    })?;
    if dependency == 0 {
        return Err(Error::ConflictError(format!(
            "libsolv {rule_description} rule for '{}' has no dependency ID",
            root.name
        )));
    }
    let kind = match pool.required_kind(requiring_index, dependency)? {
        RequiredKind::Depends => RepositoryRequirementKind::Depends,
        RequiredKind::PreDepends => RepositoryRequirementKind::PreDepends,
    };
    let group = project_requirement(pool.dependency(dependency)?, kind)?;
    Ok(NativeUnresolvedDependencyV1 {
        requiring_package_key_sha256: package_index.package_key(requiring_index)?,
        requirement_group_sha256: native_requirement_group_sha256(&group)?,
    })
}
