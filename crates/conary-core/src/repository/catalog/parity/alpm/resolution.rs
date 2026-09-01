// crates/conary-core/src/repository/catalog/parity/alpm/resolution.rs

//! Independent libalpm-backed native resolution evidence production.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use alpm::{Alpm, Package, PrepareData, TransFlag};
use rusqlite::{Connection, params};

use super::{
    AlpmParityMemberInput, database_name, open_alpm, produce_alpm_parity_oracle, project_package,
    project_requirement,
};
use crate::error::{Error, Result};
use crate::repository::catalog::ProfileRevisionV2;
use crate::repository::catalog::parity::resolution_survey::{
    NativeResolutionSurveyCollector, NativeRootResolutionError, NativeRootResolutionResult,
    RootOutcomeSink,
};
use crate::repository::catalog::parity::{
    NATIVE_RESOLUTION_ROOT_FILE_NAME, NativeParityEcosystemV1, NativeParityOracleReader,
    NativeParityOracleV1, NativeParityPackageV1, NativeResolutionInstalledStateV1,
    NativeResolutionOracleV1, NativeResolutionOracleWriter, NativeResolutionOutcomeV1,
    NativeResolutionPolicyV1, NativeResolutionProviderPolicyV1,
    NativeResolutionRequirementPolicyV1, NativeResolutionRootPolicyV1,
    NativeResolutionSurveyAlpmConflictV1, NativeResolutionSurveyAlpmMissingV1,
    NativeResolutionSurveyAlpmPackageV1, NativeResolutionSurveyAlpmResultV1,
    NativeResolutionSurveyErrorReasonV1, NativeResolutionSurveyNativeExplanationV1,
    NativeResolutionSurveyV1, NativeUnresolvedDependencyV1, native_requirement_group_sha256,
    verify_native_parity_oracle_bundle, verify_native_resolution_oracle_bundle,
    write_native_resolution_oracle_manifest, write_native_resolution_survey,
};
use crate::repository::dependency_model::RepositoryRequirementKind;

/// Projection contract for libalpm transaction results.
pub const ALPM_RESOLUTION_PROJECTION_SCHEMA_V1: u32 = 1;

/// Produce and independently reopen one strict ALPM resolution parity bundle.
pub fn produce_alpm_resolution_oracle(
    profile: &ProfileRevisionV2,
    inputs: &[AlpmParityMemberInput<'_>],
    package_oracle_directory: &Path,
    architecture: &str,
    output: &Path,
) -> Result<NativeResolutionOracleV1> {
    let ResolutionProduct::Oracle(manifest) = produce_alpm_resolution(
        profile,
        inputs,
        package_oracle_directory,
        architecture,
        ResolutionDestination::Oracle(output),
    )?
    else {
        unreachable!("ALPM oracle destination returned survey")
    };
    Ok(manifest)
}

/// Walk every exact ALPM root and write one diagnostics-only failure survey.
pub fn produce_alpm_resolution_survey(
    profile: &ProfileRevisionV2,
    inputs: &[AlpmParityMemberInput<'_>],
    package_oracle_directory: &Path,
    architecture: &str,
    output: &Path,
) -> Result<NativeResolutionSurveyV1> {
    let ResolutionProduct::Survey(survey) = produce_alpm_resolution(
        profile,
        inputs,
        package_oracle_directory,
        architecture,
        ResolutionDestination::Survey(output),
    )?
    else {
        unreachable!("ALPM survey destination returned oracle")
    };
    Ok(survey)
}

enum ResolutionDestination<'a> {
    Oracle(&'a Path),
    Survey(&'a Path),
}

enum ResolutionProduct {
    Oracle(NativeResolutionOracleV1),
    Survey(NativeResolutionSurveyV1),
}

fn produce_alpm_resolution(
    profile: &ProfileRevisionV2,
    inputs: &[AlpmParityMemberInput<'_>],
    package_oracle_directory: &Path,
    architecture: &str,
    destination: ResolutionDestination<'_>,
) -> Result<ResolutionProduct> {
    let policy = NativeResolutionPolicyV1 {
        architecture: architecture.to_string(),
        installed_state: NativeResolutionInstalledStateV1::Empty,
        roots: NativeResolutionRootPolicyV1::EveryExactPackage,
        positive_requirements: NativeResolutionRequirementPolicyV1::RequiredOnly,
        provider_selection: NativeResolutionProviderPolicyV1::NativePrecedence,
    };
    policy.validate()?;

    let package_oracle = verify_native_parity_oracle_bundle(package_oracle_directory, profile)?;
    require_alpm_package_oracle(package_oracle.manifest())?;
    verify_package_oracle_reprojection(profile, inputs, package_oracle.manifest())?;
    let package_index = PackageResolutionIndex::create(&package_oracle)?;

    let (_staging, mut alpm) = open_alpm(profile, inputs, &[architecture])?;
    let implementation = crate::repository::catalog::parity::NativeParityImplementationV1 {
        ecosystem: NativeParityEcosystemV1::Alpm,
        name: "libalpm".to_string(),
        version: alpm::version().to_string(),
        projection_schema: ALPM_RESOLUTION_PROJECTION_SCHEMA_V1,
    };

    match destination {
        ResolutionDestination::Oracle(output) => {
            fs::create_dir(output)?;
            let mut writer = NativeResolutionOracleWriter::create(
                output.join(NATIVE_RESOLUTION_ROOT_FILE_NAME),
                profile,
                package_oracle.manifest(),
                implementation,
                policy,
            )?;
            walk_resolution_roots(
                &package_oracle,
                &mut alpm,
                profile,
                inputs,
                &package_index,
                RootOutcomeSink::Strict(&mut writer),
            )?;
            let manifest = writer.finish()?;
            write_native_resolution_oracle_manifest(output, &manifest)?;
            let reopened =
                verify_native_resolution_oracle_bundle(output, profile, &package_oracle)?;
            if reopened.manifest() != &manifest {
                return Err(Error::InternalError(
                    "reopened ALPM resolution manifest differs from produced manifest".to_string(),
                ));
            }
            Ok(ResolutionProduct::Oracle(manifest))
        }
        ResolutionDestination::Survey(output) => {
            let mut collector = NativeResolutionSurveyCollector::new(
                profile,
                package_oracle.manifest(),
                implementation,
                policy,
            )?;
            walk_resolution_roots(
                &package_oracle,
                &mut alpm,
                profile,
                inputs,
                &package_index,
                RootOutcomeSink::Survey(&mut collector),
            )?;
            let survey = collector.finish()?;
            write_native_resolution_survey(output, &survey)?;
            Ok(ResolutionProduct::Survey(survey))
        }
    }
}

fn walk_resolution_roots(
    package_oracle: &NativeParityOracleReader,
    alpm: &mut Alpm,
    profile: &ProfileRevisionV2,
    inputs: &[AlpmParityMemberInput<'_>],
    package_index: &PackageResolutionIndex,
    mut sink: RootOutcomeSink<'_>,
) -> Result<()> {
    package_oracle.for_each_package(|root| {
        let result = resolve_exact_root(alpm, profile, inputs, package_index, &root);
        sink.root(&root, result)
    })
}

struct PackageResolutionIndex {
    _scratch: tempfile::TempDir,
    connection: Connection,
}

impl PackageResolutionIndex {
    fn create(package_oracle: &NativeParityOracleReader) -> Result<Self> {
        let scratch = tempfile::Builder::new()
            .prefix("conary-alpm-resolution-index-")
            .tempdir()?;
        let mut connection = Connection::open(scratch.path().join("packages.sqlite3"))?;
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
        Ok(Self {
            _scratch: scratch,
            connection,
        })
    }

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
    package_index: &PackageResolutionIndex,
    root: &NativeParityPackageV1,
) -> NativeRootResolutionResult {
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

    let outcome = resolve_initialized_transaction(alpm, profile, inputs, package_index, root);
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
    package_index: &PackageResolutionIndex,
    root: &NativeParityPackageV1,
) -> NativeRootResolutionResult {
    enum Preparation {
        Prepared,
        Unsatisfied(
            Vec<(String, String)>,
            NativeResolutionSurveyNativeExplanationV1,
        ),
        InvalidArchitecture(NativeResolutionSurveyNativeExplanationV1),
        Conflict(NativeResolutionSurveyNativeExplanationV1),
        Unexpected(alpm::Error),
    }

    let preparation = {
        let prepare_result = alpm.trans_prepare();
        match prepare_result {
            Ok(()) => Preparation::Prepared,
            Err(error) => {
                let error_class = error.error();
                match error.data() {
                    Some(PrepareData::UnsatisfiedDeps(dependencies)) => {
                        let explanation = NativeResolutionSurveyNativeExplanationV1::Alpm {
                            result: NativeResolutionSurveyAlpmResultV1::Unsatisfied {
                                missing: dependencies
                                    .iter()
                                    .map(|dependency| NativeResolutionSurveyAlpmMissingV1 {
                                        target: dependency.target().to_string(),
                                        dependency: dependency.depend().to_string(),
                                        causing_package: dependency
                                            .causing_pkg()
                                            .map(str::to_string),
                                    })
                                    .collect(),
                            },
                        };
                        let missing = dependencies
                            .iter()
                            .map(|dependency| {
                                Ok((
                                    dependency.target().to_string(),
                                    native_requirement_group_sha256(&project_requirement(
                                        dependency.depend(),
                                        RepositoryRequirementKind::Depends,
                                    )?)?,
                                ))
                            })
                            .collect::<Result<Vec<_>>>()
                            .map_err(|error| {
                                NativeRootResolutionError::new(
                                    error,
                                    NativeResolutionSurveyErrorReasonV1::UnresolvedProjectionFailed,
                                    explanation.clone(),
                                )
                            })?;
                        Preparation::Unsatisfied(missing, explanation)
                    }
                    Some(PrepareData::PkgInvalidArch(_)) => {
                        Preparation::InvalidArchitecture(
                            NativeResolutionSurveyNativeExplanationV1::Alpm {
                                result: NativeResolutionSurveyAlpmResultV1::InvalidArchitecture {
                                    packages: Vec::new(),
                                    detail_unavailable_reason: Some(
                                        "pinned_alpm_binding_does_not_safely_expose_invalid_architecture_entries"
                                            .to_string(),
                                    ),
                                },
                            },
                        )
                    }
                    Some(PrepareData::ConflictingDeps(conflicts)) => {
                        Preparation::Conflict(NativeResolutionSurveyNativeExplanationV1::Alpm {
                            result: NativeResolutionSurveyAlpmResultV1::Conflicts {
                                conflicts: conflicts
                                    .iter()
                                    .map(|conflict| NativeResolutionSurveyAlpmConflictV1 {
                                        package1: alpm_package_explanation(conflict.package1()),
                                        package2: alpm_package_explanation(conflict.package2()),
                                        reason: conflict.reason().to_string(),
                                    })
                                    .collect(),
                            },
                        })
                    }
                    None => Preparation::Unexpected(error_class),
                }
            }
        }
    };

    match preparation {
        Preparation::Prepared => {
            let explanation = alpm_prepared_explanation(alpm);
            let closure = transaction_packages(alpm, profile, inputs, root)
                .map_err(|error| {
                    NativeRootResolutionError::new(
                        error,
                        NativeResolutionSurveyErrorReasonV1::ResolvedClosureProjectionFailed,
                        explanation.clone(),
                    )
                })?
                .into_values()
                .collect::<BTreeSet<_>>();
            if !closure.contains(&root.package_key_sha256) {
                return Err(NativeRootResolutionError::new(
                    Error::ConflictError(format!(
                        "libalpm prepared closure for '{}' omits its exact root",
                        root.name
                    )),
                    NativeResolutionSurveyErrorReasonV1::ResolvedClosureOmittedRoot,
                    explanation,
                ));
            }
            Ok(NativeResolutionOutcomeV1::Resolved {
                closure_package_keys_sha256: closure.into_iter().collect(),
            })
        }
        Preparation::Unsatisfied(missing, explanation) => {
            let packages = transaction_packages(alpm, profile, inputs, root).map_err(|error| {
                NativeRootResolutionError::new(
                    error,
                    NativeResolutionSurveyErrorReasonV1::UnresolvedProjectionFailed,
                    explanation.clone(),
                )
            })?;
            let mut dependencies = BTreeSet::new();
            for (requiring_name, requirement_group_sha256) in missing {
                let requiring_package_key_sha256 = package_index
                    .bind_required_group(
                        &requiring_name,
                        &requirement_group_sha256,
                        packages.get(&requiring_name).map(String::as_str),
                    )
                    .map_err(|error| {
                        NativeRootResolutionError::new(
                            error,
                            NativeResolutionSurveyErrorReasonV1::UnresolvedProjectionFailed,
                            explanation.clone(),
                        )
                    })?;
                dependencies.insert(NativeUnresolvedDependencyV1 {
                    requiring_package_key_sha256,
                    requirement_group_sha256,
                });
            }
            if dependencies.is_empty() {
                return Err(NativeRootResolutionError::new(
                    Error::ConflictError(format!(
                        "libalpm reported unsatisfied dependencies for '{}' without typed records",
                        root.name
                    )),
                    NativeResolutionSurveyErrorReasonV1::UnresolvedProjectionFailed,
                    explanation,
                ));
            }
            Ok(NativeResolutionOutcomeV1::Unresolved {
                dependencies: dependencies.into_iter().collect(),
            })
        }
        Preparation::InvalidArchitecture(explanation) => Err(NativeRootResolutionError::new(
            Error::ConfigError(format!(
                "libalpm rejected architecture '{}' while resolving exact root '{}'",
                alpm.architectures().first().unwrap_or("<unset>"),
                root.name
            )),
            NativeResolutionSurveyErrorReasonV1::NativeArchitectureRejected,
            explanation,
        )),
        Preparation::Conflict(explanation) => Err(NativeRootResolutionError::new(
            Error::ConflictError(format!(
                "libalpm found a package conflict while resolving exact root '{}'",
                root.name
            )),
            NativeResolutionSurveyErrorReasonV1::NativePackageConflict,
            explanation,
        )),
        Preparation::Unexpected(error) => Err(NativeRootResolutionError::new(
            Error::ConfigError(format!(
                "libalpm failed to prepare exact root '{}' with unexpected error {error}",
                root.name
            )),
            NativeResolutionSurveyErrorReasonV1::NativeSolverUnexpectedFailure,
            alpm_unavailable("libalpm_returned_no_typed_prepare_data"),
        )),
    }
}

fn alpm_prepared_explanation(alpm: &Alpm) -> NativeResolutionSurveyNativeExplanationV1 {
    NativeResolutionSurveyNativeExplanationV1::Alpm {
        result: NativeResolutionSurveyAlpmResultV1::Prepared {
            packages: alpm
                .trans_add()
                .iter()
                .map(alpm_package_explanation)
                .collect(),
        },
    }
}

fn alpm_package_explanation(package: &Package) -> NativeResolutionSurveyAlpmPackageV1 {
    NativeResolutionSurveyAlpmPackageV1 {
        name: package.name().to_string(),
        version: package.version().to_string(),
        architecture: package.arch().map(str::to_string),
    }
}

fn alpm_unavailable(reason: &str) -> NativeResolutionSurveyNativeExplanationV1 {
    NativeResolutionSurveyNativeExplanationV1::Alpm {
        result: NativeResolutionSurveyAlpmResultV1::Unavailable {
            reason: reason.to_string(),
        },
    }
}

fn transaction_packages(
    alpm: &Alpm,
    profile: &ProfileRevisionV2,
    inputs: &[AlpmParityMemberInput<'_>],
    root: &NativeParityPackageV1,
) -> Result<BTreeMap<String, String>> {
    let mut packages = BTreeMap::new();
    packages.insert(root.name.clone(), root.package_key_sha256.clone());
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
