// crates/conary-core/src/repository/catalog/parity/debian/resolution.rs

//! Independent apt-pkg-backed Debian native resolution evidence production.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use rusqlite::{Connection, params};

use super::ffi::{AptNativeIdentity, AptRelationKind, AptResolution, AptResolutionOutcome};
use super::{
    DEBIAN_PARITY_PROJECTION_SCHEMA_V1, DebianParityMemberInput, PINNED_APT_PKG_VERSION,
    produce_debian_parity_oracle, stage_verified_packages, validate_inputs,
};
use crate::error::{Error, Result};
use crate::repository::catalog::ProfileRevisionV2;
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
    NativeResolutionSurveyEvidenceWithheldReasonV1, NativeResolutionSurveyNativeExplanationV1,
    NativeResolutionSurveyV1, NativeUnresolvedDependencyV1, native_requirement_group_sha256,
    verify_native_parity_oracle_bundle, verify_native_resolution_oracle_bundle,
    write_native_resolution_oracle_manifest, write_native_resolution_survey,
};
use crate::repository::dependency_model::RepositoryRequirementKind;

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
    let ResolutionProduct::Oracle(manifest) = produce_debian_resolution(
        profile,
        inputs,
        package_oracle_directory,
        architecture,
        ResolutionDestination::Oracle(output),
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
    let ResolutionProduct::Survey(survey) = produce_debian_resolution(
        profile,
        inputs,
        package_oracle_directory,
        architecture,
        ResolutionDestination::Survey(output),
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
    Oracle(NativeResolutionOracleV1),
    Survey(NativeResolutionSurveyV1),
}

fn produce_debian_resolution(
    profile: &ProfileRevisionV2,
    inputs: &[DebianParityMemberInput<'_>],
    package_oracle_directory: &Path,
    architecture: &str,
    destination: ResolutionDestination<'_>,
) -> Result<ResolutionProduct> {
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
    let mut apt = AptResolution::open(&solver_inputs, architecture)?;

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
            walk_resolution_roots(
                &package_oracle,
                &mut apt,
                &package_index,
                &policy,
                RootOutcomeSink::Strict(&mut writer),
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
            Ok(ResolutionProduct::Oracle(manifest))
        }
        ResolutionDestination::Survey(output) => {
            let mut collector = NativeResolutionSurveyCollector::new(
                profile,
                package_oracle.manifest(),
                implementation,
                policy.clone(),
            )?;
            walk_resolution_roots(
                &package_oracle,
                &mut apt,
                &package_index,
                &policy,
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
    apt: &mut AptResolution,
    package_index: &PackageResolutionIndex,
    policy: &NativeResolutionPolicyV1,
    mut sink: RootOutcomeSink<'_>,
) -> Result<()> {
    package_oracle.for_each_package(|root| {
        let result = resolve_exact_root(
            apt,
            package_index,
            &root,
            policy,
            sink.explanation_byte_limit(),
        );
        sink.root(&root, result)
    })
}

fn resolve_exact_root(
    apt: &mut AptResolution,
    package_index: &PackageResolutionIndex,
    root: &crate::repository::catalog::parity::NativeParityPackageV1,
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
    if !policy.architecture_admission.admits(
        root.version_scheme,
        Some(root_architecture),
        &policy.architecture,
    ) {
        return Ok(NativeResolutionOutcomeV1::NotInstallable {
            reason: NativeResolutionNotInstallableReasonV1::ArchitectureExcluded,
        });
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
    connection: Connection,
}

impl PackageResolutionIndex {
    fn create(package_oracle: &NativeParityOracleReader) -> Result<Self> {
        let scratch = tempfile::Builder::new()
            .prefix("conary-debian-resolution-index-")
            .tempdir()?;
        let mut connection = Connection::open(scratch.path().join("packages.sqlite3"))?;
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
        Ok(Self {
            _scratch: scratch,
            connection,
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
