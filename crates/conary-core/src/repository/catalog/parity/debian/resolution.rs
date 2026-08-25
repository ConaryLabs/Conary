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

/// Projection contract for apt-pkg transaction selections and broken strong groups.
pub const DEBIAN_RESOLUTION_PROJECTION_SCHEMA_V1: u32 = 1;

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
    let policy = NativeResolutionPolicyV1 {
        architecture: architecture.to_string(),
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
        projection_schema: DEBIAN_RESOLUTION_PROJECTION_SCHEMA_V1,
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
        let root_architecture = root.architecture.as_deref().ok_or_else(|| {
            Error::ConflictError(format!(
                "Debian package-oracle root '{}' has no architecture",
                root.name
            ))
        })?;
        let outcome = match apt.resolve(&root.name, &root.version, root_architecture)? {
            AptResolutionOutcome::Resolved(packages) => {
                let closure = packages
                    .iter()
                    .map(|identity| package_index.package_key(identity))
                    .collect::<Result<BTreeSet<_>>>()?;
                if !closure.contains(&root.package_key_sha256) {
                    return Err(Error::ConflictError(format!(
                        "apt-pkg closure for '{}' omits its exact root",
                        root.name
                    )));
                }
                NativeResolutionOutcomeV1::Resolved {
                    closure_package_keys_sha256: closure.into_iter().collect(),
                }
            }
            AptResolutionOutcome::Unresolved(missing) => {
                let dependencies = missing
                    .iter()
                    .map(|missing| package_index.missing_requirement(missing))
                    .collect::<Result<BTreeSet<_>>>()?;
                if dependencies.is_empty() {
                    return Err(Error::ConflictError(format!(
                        "apt-pkg reported exact root '{}' unresolved without a typed missing requirement",
                        root.name
                    )));
                }
                NativeResolutionOutcomeV1::Unresolved {
                    dependencies: dependencies.into_iter().collect(),
                }
            }
        };
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
            "reopened Debian resolution manifest differs from produced manifest".to_string(),
        ));
    }
    Ok(manifest)
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
