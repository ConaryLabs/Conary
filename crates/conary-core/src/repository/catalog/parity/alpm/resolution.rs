// crates/conary-core/src/repository/catalog/parity/alpm/resolution.rs

//! Independent libalpm-backed native resolution evidence production.
//!
//! Each worker stages the same authenticated database inputs into a private
//! libalpm root, owns that handle and a read-only package-index connection, and
//! returns results to the bounded canonical-order sink. libalpm state never
//! crosses threads.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use alpm::{Alpm, Package, TransFlag};
use rusqlite::{Connection, params};

mod conflict_probe;
mod evidence;

#[cfg(test)]
pub(super) use conflict_probe::native_probe_checks;
#[cfg(test)]
pub(super) use conflict_probe::native_probe_missing_closure;
use conflict_probe::{Preparation, prepare_with_conflict_probe};
use evidence::{alpm_prepared_explanation, alpm_unavailable};
#[cfg(test)]
pub(super) use evidence::{explanation_builds, reset_explanation_builds};

use super::{
    AlpmParityMemberInput, database_name, open_alpm, produce_alpm_parity_oracle, project_package,
};
use crate::error::{Error, Result};
use crate::repository::architecture::NativeResolutionArchitectureDecisionV1;
use crate::repository::catalog::ProfileRevisionV2;
use crate::repository::catalog::parity::resolution_parallel::{
    ResolutionExplanationLimits, ResolutionWalkImplementationEvidenceV1, ResolutionWorkerRequest,
};
use crate::repository::catalog::parity::resolution_producer::{
    NativeResolutionEcosystem, Oracle, ResolutionContext, Survey, produce_resolution,
    resolution_producers,
};
use crate::repository::catalog::parity::resolution_survey::{
    NativeRootResolutionError, NativeRootResolutionResult, NativeRootResolutionSuccess,
};
use crate::repository::catalog::parity::{
    NativeParityEcosystemV1, NativeParityImplementationV1, NativeParityOracleReader,
    NativeParityOracleV1, NativeParityPackageV1, NativeResolutionNotInstallableReasonV1,
    NativeResolutionOracleV1, NativeResolutionOutcomeV1, NativeResolutionPolicyV1,
    NativeResolutionSurveyErrorReasonV1, NativeResolutionSurveyV1, NativeUnresolvedDependencyV1,
    native_requirement_group_sha256,
};
use crate::repository::dependency_model::RepositoryRequirementKind;

/// Projection contract for libalpm transaction results.
pub const ALPM_RESOLUTION_PROJECTION_SCHEMA_V3: u32 = 3;

resolution_producers!(
    AlpmEcosystem,
    AlpmParityMemberInput,
    produce_alpm_resolution_oracle,
    produce_alpm_resolution_oracle_with_workers,
    produce_alpm_resolution_survey,
    produce_alpm_resolution_survey_with_workers
);

struct AlpmEcosystem;

impl<'a> NativeResolutionEcosystem<'a> for AlpmEcosystem {
    type Input = AlpmParityMemberInput<'a>;
    type Prepared = PackageResolutionIndex;
    type Worker = AlpmResolutionWorker;
    const LABEL: &'static str = "ALPM";

    fn prepare(
        context: &ResolutionContext<'_, Self::Input>,
        package_oracle: &NativeParityOracleReader,
    ) -> Result<Self::Prepared> {
        let ResolutionContext {
            profile,
            inputs,
            policy: _,
        } = *context;
        require_alpm_package_oracle(package_oracle.manifest())?;
        verify_package_oracle_reprojection(profile, inputs, package_oracle.manifest())?;
        PackageResolutionIndex::create(package_oracle)
    }
    fn implementation() -> NativeParityImplementationV1 {
        NativeParityImplementationV1 {
            ecosystem: NativeParityEcosystemV1::Alpm,
            name: "libalpm".to_string(),
            version: alpm::version().to_string(),
            projection_schema: ALPM_RESOLUTION_PROJECTION_SCHEMA_V3,
        }
    }
    fn open_worker(
        context: &ResolutionContext<'_, Self::Input>,
        prepared: &Self::Prepared,
    ) -> Result<Self::Worker> {
        let (staging, alpm) = open_alpm(
            context.profile,
            context.inputs,
            &[&context.policy.architecture],
        )?;
        Ok(AlpmResolutionWorker {
            _staging: staging,
            alpm,
            package_index: prepared.worker()?,
        })
    }
    fn resolve_root(
        context: &ResolutionContext<'_, Self::Input>,
        worker: &mut Self::Worker,
        root: &NativeParityPackageV1,
        limits: ResolutionExplanationLimits,
    ) -> Result<NativeRootResolutionResult> {
        Ok(resolve_exact_root(
            &mut worker.alpm,
            context.profile,
            context.inputs,
            &worker.package_index,
            root,
            context.policy,
            limits,
        ))
    }
}

struct AlpmResolutionWorker {
    _staging: tempfile::TempDir,
    alpm: Alpm,
    package_index: PackageResolutionIndexReader,
}

struct PackageResolutionIndex {
    _scratch: tempfile::TempDir,
    database: std::path::PathBuf,
}

struct PackageResolutionIndexReader {
    connection: Connection,
}

impl PackageResolutionIndex {
    fn create(package_oracle: &NativeParityOracleReader) -> Result<Self> {
        let scratch = tempfile::Builder::new()
            .prefix("conary-alpm-resolution-index-")
            .tempdir()?;
        let database = scratch.path().join("packages.sqlite3");
        let mut connection = Connection::open(&database)?;
        connection.execute_batch(
            "PRAGMA journal_mode = OFF;
             PRAGMA synchronous = OFF;
             CREATE TABLE packages (
                 package_key_sha256 TEXT PRIMARY KEY,
                 name TEXT NOT NULL
             ) STRICT;
             CREATE TABLE requirements (
                 package_key_sha256 TEXT NOT NULL,
                 name TEXT NOT NULL,
                 requirement_group_sha256 TEXT NOT NULL,
                 PRIMARY KEY (package_key_sha256, requirement_group_sha256)
             ) STRICT;
             CREATE INDEX requirements_binding
             ON requirements(name, requirement_group_sha256, package_key_sha256);",
        )?;
        let transaction = connection.transaction()?;
        package_oracle.for_each_package(|package| {
            transaction.execute(
                "INSERT INTO packages (package_key_sha256, name) VALUES (?1, ?2)",
                params![package.package_key_sha256, package.name],
            )?;
            for group in &package.requirement_groups {
                if group.kind == RepositoryRequirementKind::Depends.as_str()
                    || group.kind == RepositoryRequirementKind::PreDepends.as_str()
                {
                    transaction.execute(
                        "INSERT OR IGNORE INTO requirements (
                             package_key_sha256, name, requirement_group_sha256
                         ) VALUES (?1, ?2, ?3)",
                        params![
                            package.package_key_sha256,
                            package.name,
                            native_requirement_group_sha256(group)?
                        ],
                    )?;
                }
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
    fn bind_required_group(
        &self,
        requiring_name: &str,
        requirement_group_sha256: &str,
        selected_package_key: Option<&str>,
    ) -> Result<String> {
        if let Some(package_key) = selected_package_key {
            let exists: bool = self.connection.query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM requirements
                     WHERE package_key_sha256 = ?1
                       AND name = ?2
                       AND requirement_group_sha256 = ?3
                 )",
                params![package_key, requiring_name, requirement_group_sha256],
                |row| row.get(0),
            )?;
            if exists {
                return Ok(package_key.to_string());
            }
            return Err(Error::ConflictError(format!(
                "libalpm unresolved dependency does not bind a required group on selected package '{requiring_name}'"
            )));
        }

        let mut statement = self.connection.prepare(
            "SELECT package_key_sha256 FROM requirements
             WHERE name = ?1 AND requirement_group_sha256 = ?2
             ORDER BY package_key_sha256
             LIMIT 2",
        )?;
        let mut rows = statement.query(params![requiring_name, requirement_group_sha256])?;
        let Some(first) = rows.next()? else {
            return Err(Error::ConflictError(format!(
                "libalpm unresolved dependency names no exact required group for package '{requiring_name}'"
            )));
        };
        let package_key: String = first.get(0)?;
        if rows.next()?.is_some() {
            return Err(Error::ConflictError(format!(
                "libalpm unresolved dependency is ambiguous across exact packages named '{requiring_name}'"
            )));
        }
        Ok(package_key)
    }
}

fn require_alpm_package_oracle(manifest: &NativeParityOracleV1) -> Result<()> {
    if manifest.implementation.ecosystem != NativeParityEcosystemV1::Alpm {
        return Err(Error::ConfigError(
            "ALPM resolution producer requires an ALPM package oracle".to_string(),
        ));
    }
    Ok(())
}

fn verify_package_oracle_reprojection(
    profile: &ProfileRevisionV2,
    inputs: &[AlpmParityMemberInput<'_>],
    expected: &NativeParityOracleV1,
) -> Result<()> {
    let scratch = tempfile::Builder::new()
        .prefix("conary-alpm-resolution-package-proof-")
        .tempdir()?;
    let produced =
        produce_alpm_parity_oracle(profile, inputs, &scratch.path().join("package-oracle"))?;
    if &produced != expected {
        return Err(Error::ConflictError(
            "ALPM package oracle does not match a fresh libalpm projection of its authenticated databases"
                .to_string(),
        ));
    }
    Ok(())
}

fn resolve_exact_root(
    alpm: &mut Alpm,
    profile: &ProfileRevisionV2,
    inputs: &[AlpmParityMemberInput<'_>],
    package_index: &PackageResolutionIndexReader,
    root: &NativeParityPackageV1,
    policy: &NativeResolutionPolicyV1,
    explanation_limits: ResolutionExplanationLimits,
) -> NativeRootResolutionResult {
    let Some(root_architecture) = root.architecture.as_deref() else {
        return Err(NativeRootResolutionError::new(
            Error::ConflictError(format!(
                "ALPM package-oracle root '{}' has no architecture",
                root.name
            )),
            NativeResolutionSurveyErrorReasonV1::ExactRootProjectionFailed,
            alpm_unavailable("exact_root_architecture_missing"),
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
                alpm_unavailable("unknown_architecture_token"),
            ));
        }
    }
    let root_package = locate_exact_root(alpm, profile, inputs, root).map_err(|error| {
        NativeRootResolutionError::new(
            error,
            NativeResolutionSurveyErrorReasonV1::ExactRootProjectionFailed,
            alpm_unavailable("exact_root_not_available_to_native_solver"),
        )
    })?;
    let flags = TransFlag::DB_ONLY | TransFlag::NO_LOCK | TransFlag::NO_HOOKS;
    alpm.trans_init(flags).map_err(|error| {
        NativeRootResolutionError::new(
            Error::InitError(format!("initialize libalpm transaction: {error}")),
            NativeResolutionSurveyErrorReasonV1::TransactionInitializationFailed,
            alpm_unavailable("transaction_initialization_failed"),
        )
    })?;
    if let Err(error) = alpm.trans_add_pkg(root_package) {
        let error = error.error.to_string();
        let release = alpm.trans_release();
        let (error, reason) = match release {
            Ok(()) => (
                Error::ConfigError(format!(
                    "add exact ALPM root '{}' to transaction: {error}",
                    root.name
                )),
                NativeResolutionSurveyErrorReasonV1::TransactionAddRootFailed,
            ),
            Err(release_error) => (
                Error::InternalError(format!(
                    "add exact ALPM root failed with {error}; releasing transaction also failed: {release_error}"
                )),
                NativeResolutionSurveyErrorReasonV1::TransactionReleaseFailed,
            ),
        };
        return Err(NativeRootResolutionError::new(
            error,
            reason,
            alpm_unavailable("transaction_add_root_failed"),
        ));
    }

    let outcome = resolve_initialized_transaction(
        alpm,
        profile,
        inputs,
        package_index,
        root,
        explanation_limits,
    );
    let release = alpm.trans_release();
    match (outcome, release) {
        (Ok(outcome), Ok(())) => Ok(outcome),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(release_error)) => Err(NativeRootResolutionError::new(
            Error::InternalError(format!(
                "release prepared ALPM transaction for '{}': {release_error}",
                root.name
            )),
            NativeResolutionSurveyErrorReasonV1::TransactionReleaseFailed,
            alpm_unavailable("transaction_release_failed"),
        )),
        (Err(mut error), Err(release_error)) => {
            let message = error.error_message();
            error.replace_error(
                Error::InternalError(format!(
                    "ALPM resolution for '{}' failed with {message}; releasing transaction also failed: {release_error}",
                    root.name
                )),
                NativeResolutionSurveyErrorReasonV1::TransactionReleaseFailed,
            );
            Err(error)
        }
    }
}

fn resolve_initialized_transaction(
    alpm: &mut Alpm,
    profile: &ProfileRevisionV2,
    inputs: &[AlpmParityMemberInput<'_>],
    package_index: &PackageResolutionIndexReader,
    root: &NativeParityPackageV1,
    explanation_limits: ResolutionExplanationLimits,
) -> NativeRootResolutionResult {
    let preparation = prepare_with_conflict_probe(alpm, root, explanation_limits, &|alpm| {
        transaction_packages(alpm, profile, inputs)
    })?;

    match preparation {
        Preparation::Conflicting(conflict) => Ok(NativeRootResolutionSuccess::explained(
            NativeResolutionOutcomeV1::NotInstallable {
                reason: NativeResolutionNotInstallableReasonV1::ConflictingClosure,
            },
            conflict.explanation,
        )),
        Preparation::Prepared => {
            let closure = transaction_packages(alpm, profile, inputs)
                .map_err(|error| {
                    NativeRootResolutionError::new(
                        error,
                        NativeResolutionSurveyErrorReasonV1::ResolvedClosureProjectionFailed,
                        alpm_prepared_explanation(alpm, explanation_limits.failure_bytes()),
                    )
                })?
                .into_values()
                .collect::<BTreeSet<_>>();
            if !closure.contains(&root.package_key_sha256) {
                return Ok(NativeRootResolutionSuccess::explained(
                    NativeResolutionOutcomeV1::NotInstallable {
                        reason: NativeResolutionNotInstallableReasonV1::ConflictingClosure,
                    },
                    alpm_prepared_explanation(alpm, explanation_limits.diagnostic_outcome_bytes()),
                ));
            }
            Ok(NativeRootResolutionSuccess::plain(
                NativeResolutionOutcomeV1::Resolved {
                    closure_package_keys_sha256: closure.into_iter().collect(),
                },
            ))
        }
        Preparation::Unsatisfied(missing) => {
            let packages = missing.selected_packages;
            let mut dependencies = BTreeSet::new();
            for (requiring_name, requirement_group_sha256) in missing.dependencies {
                let requiring_package_key_sha256 = package_index
                    .bind_required_group(
                        &requiring_name,
                        &requirement_group_sha256,
                        packages
                            .get(&requiring_name)
                            .map(String::as_str)
                            .or_else(|| {
                                (requiring_name == root.name)
                                    .then_some(root.package_key_sha256.as_str())
                            }),
                    )
                    .map_err(|error| {
                        NativeRootResolutionError::new(
                            error,
                            NativeResolutionSurveyErrorReasonV1::UnresolvedProjectionFailed,
                            alpm_unavailable(
                                "native_unsatisfied_data_expired_before_requirement_binding_failed",
                            ),
                        )
                    })?;
                dependencies.insert(NativeUnresolvedDependencyV1 {
                    requiring_package_key_sha256,
                    requirement_group_sha256,
                });
            }
            Ok(NativeRootResolutionSuccess::plain(
                NativeResolutionOutcomeV1::Unresolved {
                    dependencies: dependencies.into_iter().collect(),
                },
            ))
        }
    }
}

fn transaction_packages(
    alpm: &Alpm,
    profile: &ProfileRevisionV2,
    inputs: &[AlpmParityMemberInput<'_>],
) -> Result<BTreeMap<String, String>> {
    let mut packages = BTreeMap::new();
    for package in alpm.trans_add().iter() {
        let projected = project_transaction_package(profile, inputs, package)?;
        if let Some(existing) =
            packages.insert(projected.name.clone(), projected.package_key_sha256.clone())
            && existing != projected.package_key_sha256
        {
            return Err(Error::ConflictError(format!(
                "libalpm transaction contains multiple exact packages named '{}'",
                projected.name
            )));
        }
    }
    Ok(packages)
}

fn project_transaction_package(
    profile: &ProfileRevisionV2,
    inputs: &[AlpmParityMemberInput<'_>],
    package: &Package,
) -> Result<NativeParityPackageV1> {
    let database = package.db().ok_or_else(|| {
        Error::ConflictError(format!(
            "libalpm transaction package '{}' has no source database",
            package.name()
        ))
    })?;
    let ordinal = database_ordinal(database.name())?;
    let input = inputs
        .get(usize::try_from(ordinal).map_err(|_| {
            Error::ConfigError("ALPM transaction member ordinal exceeds usize".to_string())
        })?)
        .ok_or_else(|| {
            Error::ConflictError(format!(
                "libalpm transaction package '{}' names absent member ordinal {ordinal}",
                package.name()
            ))
        })?;
    project_package(profile, ordinal, input.source_snapshot, package)
}

fn locate_exact_root<'a>(
    alpm: &'a Alpm,
    profile: &ProfileRevisionV2,
    inputs: &[AlpmParityMemberInput<'_>],
    root: &NativeParityPackageV1,
) -> Result<&'a Package> {
    let expected_database = database_name(root.member_ordinal as usize)?;
    let database = alpm
        .syncdbs()
        .iter()
        .find(|database| database.name() == expected_database)
        .ok_or_else(|| {
            Error::ConflictError(format!(
                "ALPM package-oracle root '{}' names absent member ordinal {}",
                root.name, root.member_ordinal
            ))
        })?;
    let package = database.pkg(root.name.as_str()).map_err(|error| {
        Error::ConflictError(format!(
            "ALPM package-oracle root '{}' is absent from member ordinal {}: {error}",
            root.name, root.member_ordinal
        ))
    })?;
    let projected = project_transaction_package(profile, inputs, package)?;
    let same_profile_facts = projected.has_same_profile_facts(root)?;
    if projected.package_key_sha256 != root.package_key_sha256 || !same_profile_facts {
        return Err(Error::ConflictError(format!(
            "ALPM package-oracle root '{}' does not match its exact native package",
            root.name
        )));
    }
    Ok(package)
}

fn database_ordinal(name: &str) -> Result<u32> {
    let Some(ordinal) = name
        .strip_prefix("member-")
        .filter(|ordinal| ordinal.len() == 8 && ordinal.bytes().all(|byte| byte.is_ascii_digit()))
    else {
        return Err(Error::ConflictError(format!(
            "libalpm returned noncanonical database name '{name}'"
        )));
    };
    let ordinal = ordinal.parse::<u32>().map_err(|error| {
        Error::ConflictError(format!("parse libalpm database ordinal '{name}': {error}"))
    })?;
    if database_name(ordinal as usize)? != name {
        return Err(Error::ConflictError(format!(
            "libalpm returned noncanonical database ordinal '{name}'"
        )));
    }
    Ok(ordinal)
}
