// crates/conary-core/src/repository/catalog/parity/rpm/resolution.rs

//! Independent libsolv-backed RPM native resolution evidence production.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use super::ffi::{RequiredKind, SolvProblemRule, SolvResolution};
use super::{
    PINNED_LIBSOLV_VERSION, RPM_PARITY_PROJECTION_SCHEMA_V1, RpmParityMemberInput, SolvPool,
    produce_rpm_parity_oracle, project_package, project_requirement, stage_verified_metadata,
    validate_inputs,
};
use crate::error::{Error, Result};
use crate::repository::catalog::ProfileRevisionV2;
use crate::repository::catalog::parity::{
    NATIVE_RESOLUTION_ROOT_FILE_NAME, NativeParityEcosystemV1, NativeParityImplementationV1,
    NativeParityOracleReader, NativeParityOracleV1, NativeResolutionInstalledStateV1,
    NativeResolutionOracleV1, NativeResolutionOracleWriter, NativeResolutionOutcomeV1,
    NativeResolutionPolicyV1, NativeResolutionProviderPolicyV1,
    NativeResolutionRequirementPolicyV1, NativeResolutionRootPolicyV1, NativeResolutionRootV1,
    NativeUnresolvedDependencyV1, native_requirement_group_sha256,
    verify_native_parity_oracle_bundle, verify_native_resolution_oracle_bundle,
    write_native_resolution_oracle_manifest,
};
use crate::repository::dependency_model::RepositoryRequirementKind;

/// Projection contract for libsolv transaction results and typed problem rules.
pub const RPM_RESOLUTION_PROJECTION_SCHEMA_V1: u32 = 1;

const SOLVER_RULE_PKG_NOT_INSTALLABLE: i32 = 0x101;
const SOLVER_RULE_PKG_NOTHING_PROVIDES_DEP: i32 = 0x102;
const SOLVER_RULE_PKG_REQUIRES: i32 = 0x103;
const SOLVER_RULE_JOB: i32 = 0x400;

const CREATE_INDEX: &str = "
CREATE TABLE oracle_packages (
    package_key_sha256 TEXT PRIMARY KEY,
    row_json BLOB NOT NULL,
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
    let policy = NativeResolutionPolicyV1 {
        architecture: architecture.to_string(),
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
    let mut pool = SolvPool::create()?;
    if SolvPool::version()? != PINNED_LIBSOLV_VERSION {
        return Err(Error::ConfigError(format!(
            "RPM resolution requires libsolv {PINNED_LIBSOLV_VERSION}"
        )));
    }
    for (ordinal, (member, metadata)) in profile.members.iter().zip(&staged).enumerate() {
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
    let package_index = PackageResolutionIndex::create(profile, inputs, &pool, &package_oracle)?;

    let implementation = NativeParityImplementationV1 {
        ecosystem: NativeParityEcosystemV1::Rpm,
        name: "libsolv".to_string(),
        version: PINNED_LIBSOLV_VERSION.to_string(),
        projection_schema: RPM_RESOLUTION_PROJECTION_SCHEMA_V1,
    };
    fs::create_dir(output)?;
    let mut writer = NativeResolutionOracleWriter::create(
        output.join(NATIVE_RESOLUTION_ROOT_FILE_NAME),
        profile,
        package_oracle.manifest(),
        implementation,
        policy,
    )?;
    package_oracle.for_each_package(|root| {
        let root_index = package_index.selected_native_index(&root.package_key_sha256)?;
        let outcome = resolve_exact_root(&mut pool, &package_index, root_index, &root)?;
        writer.root(&NativeResolutionRootV1 {
            root_package_key_sha256: root.package_key_sha256,
            outcome,
        })
    })?;
    let manifest = writer.finish()?;
    write_native_resolution_oracle_manifest(output, &manifest)?;
    let reopened = verify_native_resolution_oracle_bundle(output, profile, &package_oracle)?;
    if reopened.manifest() != &manifest {
        return Err(Error::InternalError(
            "reopened RPM resolution manifest differs from produced manifest".to_string(),
        ));
    }
    Ok(manifest)
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
        let mut connection = Connection::open(scratch.path().join("packages.sqlite3"))?;
        connection.execute_batch(CREATE_INDEX)?;
        let transaction = connection.transaction()?;
        package_oracle.for_each_package(|package| {
            transaction.execute(
                "INSERT INTO oracle_packages (package_key_sha256, row_json)
                 VALUES (?1, ?2)",
                params![
                    package.package_key_sha256,
                    crate::json::canonical_json(&package).map_err(|error| {
                        Error::ParseError(format!(
                            "serialize indexed RPM package-oracle row: {error}"
                        ))
                    })?,
                ],
            )?;
            Ok(())
        })?;
        for native_index in 0..pool.package_count() {
            let native = pool.package(native_index)?;
            let package = project_package(profile, inputs, &native)?;
            let expected: Option<Vec<u8>> = transaction
                .query_row(
                    "SELECT row_json FROM oracle_packages WHERE package_key_sha256 = ?1",
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
            let actual = crate::json::canonical_json(&package).map_err(|error| {
                Error::ParseError(format!("serialize indexed native RPM package: {error}"))
            })?;
            if actual != expected {
                return Err(Error::ConflictError(format!(
                    "libsolv native package '{}' disagrees with the bound RPM package oracle",
                    package.name
                )));
            }
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
        Ok(Self {
            _scratch: scratch,
            connection,
        })
    }

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
    package_index: &PackageResolutionIndex,
    root_index: usize,
    root: &crate::repository::catalog::parity::NativeParityPackageV1,
) -> Result<NativeResolutionOutcomeV1> {
    match pool.solve(root_index)? {
        SolvResolution::Resolved(packages) => {
            let closure = packages
                .into_iter()
                .map(|index| package_index.package_key(index))
                .collect::<Result<BTreeSet<_>>>()?;
            if !closure.contains(&root.package_key_sha256) {
                return Err(Error::ConflictError(format!(
                    "libsolv closure for '{}' omits its exact root",
                    root.name
                )));
            }
            Ok(NativeResolutionOutcomeV1::Resolved {
                closure_package_keys_sha256: closure.into_iter().collect(),
            })
        }
        SolvResolution::Unresolved(rules) => unresolved_outcome(pool, package_index, root, rules),
    }
}

fn unresolved_outcome(
    pool: &SolvPool,
    package_index: &PackageResolutionIndex,
    root: &crate::repository::catalog::parity::NativeParityPackageV1,
    rules: Vec<SolvProblemRule>,
) -> Result<NativeResolutionOutcomeV1> {
    let mut dependencies = BTreeSet::new();
    for rule in rules {
        match rule.rule_type {
            SOLVER_RULE_PKG_NOTHING_PROVIDES_DEP => {
                let requiring_index = rule.from_index.ok_or_else(|| {
                    Error::ConflictError(format!(
                        "libsolv missing-dependency rule for '{}' has no requiring package",
                        root.name
                    ))
                })?;
                if rule.dependency == 0 {
                    return Err(Error::ConflictError(format!(
                        "libsolv missing-dependency rule for '{}' has no dependency ID",
                        root.name
                    )));
                }
                let kind = match pool.required_kind(requiring_index, rule.dependency)? {
                    RequiredKind::Depends => RepositoryRequirementKind::Depends,
                    RequiredKind::PreDepends => RepositoryRequirementKind::PreDepends,
                };
                let group = project_requirement(pool.dependency(rule.dependency)?, kind)?;
                dependencies.insert(NativeUnresolvedDependencyV1 {
                    requiring_package_key_sha256: package_index.package_key(requiring_index)?,
                    requirement_group_sha256: native_requirement_group_sha256(&group)?,
                });
            }
            SOLVER_RULE_PKG_REQUIRES | SOLVER_RULE_JOB => {}
            SOLVER_RULE_PKG_NOT_INSTALLABLE => {
                return Err(Error::ConfigError(format!(
                    "libsolv found exact root '{}' not installable for the target architecture",
                    root.name
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
