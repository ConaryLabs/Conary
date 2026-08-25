// crates/conary-core/src/repository/catalog/parity/candidate_resolution.rs

//! Complete Conary candidate dependency-resolution evidence production.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use super::compare::compare_native_parity_oracle;
use super::contract::NativeParityImplementationV1;
use super::io::verify_native_parity_oracle_bundle;
use super::resolution_compare::{NativeResolutionComparisonV1, compare_native_resolution_oracle};
use super::resolution_contract::{
    NativeResolutionInstalledStateV1, NativeResolutionOracleV1, NativeResolutionOutcomeV1,
    NativeResolutionPolicyV1, NativeResolutionProviderPolicyV1,
    NativeResolutionRequirementPolicyV1, NativeResolutionRootPolicyV1, NativeResolutionRootV1,
    NativeUnresolvedDependencyV1, native_requirement_group_sha256,
};
use super::resolution_io::{
    NATIVE_RESOLUTION_ROOT_FILE_NAME, NativeResolutionOracleWriter,
    verify_native_resolution_oracle_bundle, write_native_resolution_oracle_manifest,
};
use crate::db::models::{
    Repository, RepositoryPackage, RepositoryProvide, RepositoryRequirement,
    RepositoryRequirementGroup,
};
use crate::error::{Error, Result};
use crate::repository::catalog::{CatalogPackageRecordV1, CatalogReader, ProfileRevisionV2};
use crate::repository::resolution_policy::ResolutionPolicy;
use crate::resolver::sat::{SatExactResolution, solve_exact_repository_package_with_policy};

/// Projection contract for the Conary SAT candidate evidence producer.
pub const CONARY_RESOLUTION_PROJECTION_SCHEMA_V1: u32 = 1;

/// Produced candidate manifest and its exact successful native comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConaryResolutionCandidateV1 {
    pub manifest: NativeResolutionOracleV1,
    pub comparison: NativeResolutionComparisonV1,
}

/// Produce, independently reopen, and compare one complete Conary resolution bundle.
pub fn produce_conary_resolution_candidate(
    profile: &ProfileRevisionV2,
    catalog: &CatalogReader,
    package_oracle_directory: &Path,
    native_resolution_directory: &Path,
    architecture: &str,
    output: &Path,
) -> Result<ConaryResolutionCandidateV1> {
    let package_oracle = verify_native_parity_oracle_bundle(package_oracle_directory, profile)?;
    compare_native_parity_oracle(profile, catalog, &package_oracle).map_err(|error| {
        Error::ConflictError(format!(
            "candidate catalog does not match the pinned package oracle: {error}"
        ))
    })?;
    let native_resolution = verify_native_resolution_oracle_bundle(
        native_resolution_directory,
        profile,
        &package_oracle,
    )?;
    let policy = resolution_policy(architecture);
    if native_resolution.manifest().policy != policy {
        return Err(Error::ConflictError(
            "native resolution oracle uses a different candidate policy".to_string(),
        ));
    }

    let projection = CandidateResolutionProjection::create(profile, catalog)?;
    fs::create_dir(output)?;
    let implementation = NativeParityImplementationV1 {
        ecosystem: package_oracle.manifest().implementation.ecosystem,
        name: "conary-sat".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        projection_schema: CONARY_RESOLUTION_PROJECTION_SCHEMA_V1,
    };
    let mut writer = NativeResolutionOracleWriter::create(
        output.join(NATIVE_RESOLUTION_ROOT_FILE_NAME),
        profile,
        package_oracle.manifest(),
        implementation,
        policy,
    )?;
    package_oracle.for_each_package(|root| {
        writer.root(&NativeResolutionRootV1 {
            root_package_key_sha256: root.package_key_sha256.clone(),
            outcome: projection.resolve(&root.package_key_sha256, architecture)?,
        })
    })?;
    let manifest = writer.finish()?;
    write_native_resolution_oracle_manifest(output, &manifest)?;

    let reopened = verify_native_resolution_oracle_bundle(output, profile, &package_oracle)?;
    if reopened.manifest() != &manifest {
        return Err(Error::InternalError(
            "reopened Conary resolution manifest differs from produced manifest".to_string(),
        ));
    }
    let comparison =
        compare_native_resolution_oracle(profile, &package_oracle, &native_resolution, &reopened)
            .map_err(|error| {
            Error::ConflictError(format!(
                "Conary candidate resolution diverges from the pinned native oracle: {error}"
            ))
        })?;
    Ok(ConaryResolutionCandidateV1 {
        manifest,
        comparison,
    })
}

fn resolution_policy(architecture: &str) -> NativeResolutionPolicyV1 {
    NativeResolutionPolicyV1 {
        architecture: architecture.to_string(),
        installed_state: NativeResolutionInstalledStateV1::Empty,
        roots: NativeResolutionRootPolicyV1::EveryExactPackage,
        positive_requirements: NativeResolutionRequirementPolicyV1::RequiredOnly,
        provider_selection: NativeResolutionProviderPolicyV1::NativePrecedence,
    }
}

struct CandidateResolutionProjection {
    _scratch: tempfile::TempDir,
    connection: Connection,
    source_identity: String,
}

impl CandidateResolutionProjection {
    fn create(profile: &ProfileRevisionV2, catalog: &CatalogReader) -> Result<Self> {
        let scratch = tempfile::Builder::new()
            .prefix("conary-candidate-resolution-")
            .tempdir()?;
        let database = scratch.path().join("candidate.sqlite3");
        crate::db::init(&database)?;
        let mut connection = crate::db::open(&database)?;
        connection.execute_batch(
            "CREATE TABLE candidate_resolution_package_keys (
                 repository_package_id INTEGER PRIMARY KEY
                     REFERENCES repository_packages(id) ON DELETE CASCADE,
                 package_key_sha256 TEXT NOT NULL UNIQUE
                     CHECK(length(package_key_sha256) = 64)
             ) STRICT;
             CREATE TABLE candidate_resolution_group_keys (
                 repository_requirement_group_id INTEGER PRIMARY KEY
                     REFERENCES repository_requirement_groups(id) ON DELETE CASCADE,
                 repository_package_id INTEGER NOT NULL
                     REFERENCES repository_packages(id) ON DELETE CASCADE,
                 requirement_group_sha256 TEXT NOT NULL
                     CHECK(length(requirement_group_sha256) = 64),
                 UNIQUE(repository_package_id, requirement_group_sha256)
             ) STRICT;",
        )?;
        let mut repository = Repository::new(
            format!("candidate-{}", profile.profile),
            "file:///conary-candidate-resolution".to_string(),
        );
        repository.source_profile = Some(profile.profile.clone());
        let repository_id = repository.insert(&connection)?;
        let transaction = connection.transaction()?;
        catalog.for_each_package(|package| {
            insert_catalog_package(&transaction, repository_id, package)
        })?;
        transaction.commit()?;
        Ok(Self {
            _scratch: scratch,
            connection,
            source_identity: profile.profile.clone(),
        })
    }

    fn resolve(
        &self,
        root_package_key_sha256: &str,
        architecture: &str,
    ) -> Result<NativeResolutionOutcomeV1> {
        let root_id = self
            .connection
            .query_row(
                "SELECT repository_package_id FROM candidate_resolution_package_keys
                 WHERE package_key_sha256 = ?1",
                [root_package_key_sha256],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .ok_or_else(|| {
                Error::ConflictError(format!(
                    "candidate resolver projection omits package key {root_package_key_sha256}"
                ))
            })?;
        let policy =
            ResolutionPolicy::new().with_primary_source_identity(self.source_identity.clone());
        match solve_exact_repository_package_with_policy(
            &self.connection,
            root_id,
            architecture,
            &policy,
        )? {
            SatExactResolution::Resolved { install_order } => {
                let mut closure = BTreeSet::new();
                for package in install_order {
                    let repository_package_id = package.repo_package_id.ok_or_else(|| {
                        Error::InternalError(
                            "empty-state candidate resolution selected an installed package"
                                .to_string(),
                        )
                    })?;
                    closure.insert(self.package_key(repository_package_id)?);
                }
                if !closure.contains(root_package_key_sha256) {
                    return Err(Error::ConflictError(format!(
                        "Conary candidate closure omits exact root {root_package_key_sha256}"
                    )));
                }
                Ok(NativeResolutionOutcomeV1::Resolved {
                    closure_package_keys_sha256: closure.into_iter().collect(),
                })
            }
            SatExactResolution::Unresolved { dependencies } => {
                let mut unresolved = BTreeSet::new();
                for dependency in dependencies {
                    let (requiring_package_key_sha256, requirement_group_sha256) = self
                        .connection
                        .query_row(
                            "SELECT package.package_key_sha256, requirement.requirement_group_sha256
                             FROM candidate_resolution_group_keys requirement
                             JOIN candidate_resolution_package_keys package
                               ON package.repository_package_id = requirement.repository_package_id
                             WHERE requirement.repository_requirement_group_id = ?1
                               AND requirement.repository_package_id = ?2",
                            params![
                                dependency.repository_requirement_group_id,
                                dependency.repository_package_id
                            ],
                            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                        )
                        .optional()?
                        .ok_or_else(|| {
                            Error::ConflictError(format!(
                                "candidate unresolved group {} is absent from exact package {}",
                                dependency.repository_requirement_group_id,
                                dependency.repository_package_id
                            ))
                        })?;
                    unresolved.insert(NativeUnresolvedDependencyV1 {
                        requiring_package_key_sha256,
                        requirement_group_sha256,
                    });
                }
                Ok(NativeResolutionOutcomeV1::Unresolved {
                    dependencies: unresolved.into_iter().collect(),
                })
            }
        }
    }

    fn package_key(&self, repository_package_id: i64) -> Result<String> {
        self.connection
            .query_row(
                "SELECT package_key_sha256 FROM candidate_resolution_package_keys
                 WHERE repository_package_id = ?1",
                [repository_package_id],
                |row| row.get(0),
            )
            .map_err(|error| {
                Error::ConflictError(format!(
                    "candidate closure package {repository_package_id} has no exact catalog key: {error}"
                ))
            })
    }
}

fn insert_catalog_package(
    connection: &Connection,
    repository_id: i64,
    record: CatalogPackageRecordV1,
) -> Result<()> {
    let mut package = RepositoryPackage::new(
        repository_id,
        record.name,
        record.version,
        record.version_scheme,
        record.checksum,
        i64::try_from(record.size).map_err(|_| {
            Error::ConfigError(format!(
                "catalog package {} size exceeds SQLite i64",
                record.package_key_sha256
            ))
        })?,
        record.download_url,
    );
    package.package_release = record.package_release;
    package.architecture = record.architecture;
    package.debian_multi_arch = record.debian_multi_arch;
    package.description = record.description;
    package.metadata = record.metadata;
    package.is_security_update = record.is_security_update;
    package.severity = record.severity;
    package.cve_ids = record.cve_ids;
    package.advisory_id = record.advisory_id;
    package.advisory_url = record.advisory_url;
    package.source_profile = Some(record.source_profile);
    let package_id = package.insert(connection)?;
    connection.execute(
        "INSERT INTO candidate_resolution_package_keys (
             repository_package_id, package_key_sha256
         ) VALUES (?1, ?2)",
        params![package_id, record.package_key_sha256],
    )?;

    for provide in record.provides {
        RepositoryProvide::new(
            package_id,
            provide.capability,
            provide.version,
            provide.kind,
            provide.raw,
            provide.version_scheme,
        )
        .with_version_relation(provide.version_relation)
        .with_architecture_qualifier(provide.architecture_qualifier)
        .with_provenance(provide.provenance)
        .insert(connection)?;
    }
    for group in record.requirement_groups {
        let digest = native_requirement_group_sha256(&group)?;
        let mut persisted = RepositoryRequirementGroup::new(
            package_id,
            group.kind,
            group.behavior,
            group.expression_json,
        );
        persisted.description = group.description;
        persisted.native_text = group.native_text;
        let group_id = persisted.insert(connection)?;
        connection.execute(
            "INSERT INTO candidate_resolution_group_keys (
                 repository_requirement_group_id, repository_package_id,
                 requirement_group_sha256
             ) VALUES (?1, ?2, ?3)",
            params![group_id, package_id, digest],
        )?;
        for atom in group.atoms {
            RepositoryRequirement::new(
                package_id,
                group_id,
                atom.capability,
                atom.version_constraint,
                atom.kind,
                atom.dependency_type,
                atom.raw,
            )
            .insert(connection)?;
        }
    }
    Ok(())
}
