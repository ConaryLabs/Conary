// crates/conary-core/src/repository/catalog/parity/debian/mod.rs

//! Independent apt-pkg-backed Debian native parity production.

use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

use super::{
    NATIVE_PARITY_PACKAGE_FILE_NAME, NativeParityEcosystemV1, NativeParityImplementationV1,
    NativeParityOracleV1, NativeParityOracleWriter, NativeParityPackageV1,
    verify_native_parity_oracle_bundle, write_native_parity_oracle_manifest,
};
use crate::error::{Error, Result};
use crate::repository::catalog::{
    CatalogProvideRecordV1, CatalogRequirementAtomV1, CatalogRequirementGroupV1, ProfileRevisionV2,
    SourceEcosystemV1, SourceMetadataObjectRoleV1, SourceMetadataObjectV1, SourceSnapshotV1,
};
use crate::repository::dependency_model::{
    DebianMultiArch, ProvideArchitectureQualifier, ProvideVersionRelation,
    RepositoryCapabilityKind, RepositoryRequirementClause, RepositoryRequirementExpression,
    RepositoryRequirementKind, RequirementArchitectureQualifier,
};
use crate::repository::dependency_source::{CapabilityProvenance, SourcePackageFormat};
use crate::repository::versioning::VersionScheme;

mod ffi;
mod resolution;

#[cfg(test)]
mod tests;

use ffi::{AptArchitectureQualifier, AptAtom, AptPackage, AptPackages, AptRelationKind};

pub use resolution::{DEBIAN_RESOLUTION_PROJECTION_SCHEMA_V1, produce_debian_resolution_oracle};

pub const DEBIAN_PARITY_PROJECTION_SCHEMA_V1: u32 = 1;
pub const PINNED_APT_PKG_VERSION: &str = "3.2.0";

const CREATE_SPOOL: &str = "
CREATE TABLE packages (
    package_key_sha256 TEXT PRIMARY KEY,
    row_json BLOB NOT NULL
) STRICT;
";

/// Exact authenticated input for one ordered Debian profile member.
pub struct DebianParityMemberInput<'a> {
    pub source_snapshot: &'a SourceSnapshotV1,
    pub packages: &'a Path,
}

/// Produce and independently reopen one strict Debian native parity bundle.
pub fn produce_debian_parity_oracle(
    profile: &ProfileRevisionV2,
    inputs: &[DebianParityMemberInput<'_>],
    output: &Path,
) -> Result<NativeParityOracleV1> {
    validate_inputs(profile, inputs)?;
    let apt_version = AptPackages::version()?;
    if apt_version != PINNED_APT_PKG_VERSION {
        return Err(Error::ConfigError(format!(
            "Debian parity requires apt-pkg {PINNED_APT_PKG_VERSION}, found {apt_version}"
        )));
    }
    let staging = tempfile::Builder::new()
        .prefix("conary-debian-parity-")
        .tempdir()?;
    let staged = stage_verified_packages(inputs, staging.path())?;
    let spool = Connection::open(staging.path().join("oracle-rows.sqlite"))?;
    spool.execute_batch(CREATE_SPOOL)?;
    let mut native_packages = 0_u64;
    let mut selected_packages = 0_u64;
    let mut exact_duplicates = 0_u64;

    for (ordinal, (input, packages_path)) in inputs.iter().zip(&staged).enumerate() {
        let member_ordinal = u32::try_from(ordinal)
            .map_err(|_| Error::ConfigError("Debian member ordinal exceeds u32".to_string()))?;
        let packages = AptPackages::open(packages_path)?.packages()?;
        for package in packages {
            native_packages = checked_increment(native_packages, "native package rows")?;
            let row = project_package(profile, member_ordinal, input.source_snapshot, package)?;
            let bytes = crate::json::canonical_json(&row).map_err(|error| {
                Error::ParseError(format!("serialize Debian parity package row: {error}"))
            })?;
            let existing: Option<Vec<u8>> = spool
                .query_row(
                    "SELECT row_json FROM packages WHERE package_key_sha256 = ?1",
                    [&row.package_key_sha256],
                    |result| result.get(0),
                )
                .optional()?;
            if let Some(existing) = existing {
                let selected: NativeParityPackageV1 =
                    serde_json::from_slice(&existing).map_err(|error| {
                        Error::InternalError(format!(
                            "reopen staged Debian parity package '{}': {error}",
                            row.package_key_sha256
                        ))
                    })?;
                if !selected.has_same_profile_facts(&row)? {
                    return Err(Error::ConflictError(format!(
                        "Debian profile '{}' has contradictory package identity {} {} {:?}",
                        profile.profile, row.name, row.version, row.architecture
                    )));
                }
                exact_duplicates = checked_increment(exact_duplicates, "exact duplicates")?;
                continue;
            }
            spool.execute(
                "INSERT INTO packages (package_key_sha256, row_json) VALUES (?1, ?2)",
                params![row.package_key_sha256, bytes],
            )?;
            selected_packages = checked_increment(selected_packages, "selected package rows")?;
        }
    }
    if native_packages
        != selected_packages
            .checked_add(exact_duplicates)
            .ok_or_else(|| {
                Error::InternalError("Debian package accounting exceeds u64".to_string())
            })?
    {
        return Err(Error::InternalError(
            "Debian package accounting lost a native row".to_string(),
        ));
    }

    let implementation = NativeParityImplementationV1 {
        ecosystem: NativeParityEcosystemV1::Debian,
        name: "apt-pkg".to_string(),
        version: apt_version,
        projection_schema: DEBIAN_PARITY_PROJECTION_SCHEMA_V1,
    };
    fs::create_dir(output)?;
    let package_path = output.join(NATIVE_PARITY_PACKAGE_FILE_NAME);
    let mut writer = NativeParityOracleWriter::create(&package_path, profile, implementation)?;
    let mut statement =
        spool.prepare("SELECT row_json FROM packages ORDER BY package_key_sha256")?;
    let mut rows = statement.query([])?;
    let mut written = 0_u64;
    while let Some(row) = rows.next()? {
        let bytes: Vec<u8> = row.get(0)?;
        let package: NativeParityPackageV1 = serde_json::from_slice(&bytes).map_err(|error| {
            Error::InternalError(format!("reopen staged Debian parity row: {error}"))
        })?;
        writer.package(&package)?;
        written = checked_increment(written, "written package rows")?;
    }
    if written != selected_packages {
        return Err(Error::InternalError(format!(
            "Debian spool selected {selected_packages} packages but replayed {written}"
        )));
    }
    let manifest = writer.finish()?;
    write_native_parity_oracle_manifest(output, &manifest)?;
    let reopened = verify_native_parity_oracle_bundle(output, profile)?;
    if reopened.manifest() != &manifest {
        return Err(Error::InternalError(
            "reopened Debian parity manifest differs from produced manifest".to_string(),
        ));
    }
    Ok(manifest)
}

fn validate_inputs(
    profile: &ProfileRevisionV2,
    inputs: &[DebianParityMemberInput<'_>],
) -> Result<()> {
    profile.validate()?;
    if inputs.len() != profile.members.len() {
        return Err(Error::ConfigError(format!(
            "Debian parity received {} member inputs for {} profile members",
            inputs.len(),
            profile.members.len()
        )));
    }
    for pair in profile.members.windows(2) {
        if pair[0].precedence <= pair[1].precedence {
            return Err(Error::ConfigError(format!(
                "Debian profile member precedence must strictly descend; ordinals {} and {} carry {} and {}",
                pair[0].ordinal, pair[1].ordinal, pair[0].precedence, pair[1].precedence
            )));
        }
    }
    for (member, input) in profile.members.iter().zip(inputs) {
        let snapshot = input.source_snapshot;
        snapshot.validate()?;
        if snapshot.manifest_sha256()? != member.source_snapshot_sha256
            || snapshot.source_profile != profile.profile
            || snapshot.source_identity != member.source_identity
            || snapshot.repository_identity != member.repository_identity
            || snapshot.stream != member.stream
        {
            return Err(Error::ConflictError(format!(
                "Debian source snapshot disagrees with profile member ordinal {}",
                member.ordinal
            )));
        }
        if snapshot.provenance.ecosystem != SourceEcosystemV1::Deb {
            return Err(Error::ConfigError(format!(
                "profile member ordinal {} is not a Debian source snapshot",
                member.ordinal
            )));
        }
        debian_packages_object(snapshot)?;
        require_regular_file(input.packages, "Debian Packages object")?;
    }
    Ok(())
}

fn stage_verified_packages(
    inputs: &[DebianParityMemberInput<'_>],
    staging: &Path,
) -> Result<Vec<PathBuf>> {
    let mut result = Vec::with_capacity(inputs.len());
    for (ordinal, input) in inputs.iter().enumerate() {
        let object = debian_packages_object(input.source_snapshot)?;
        let basename = Path::new(&object.source_path).file_name().ok_or_else(|| {
            Error::ConfigError("Debian Packages source path has no file name".to_string())
        })?;
        let member = staging.join(format!("member-{ordinal}"));
        fs::create_dir(&member)?;
        let destination = member.join(basename);
        fs::copy(input.packages, &destination)?;
        require_regular_file(&destination, "staged Debian Packages object")?;
        let metadata = fs::metadata(&destination)?;
        if metadata.len() != object.size {
            return Err(Error::ChecksumMismatch {
                expected: format!("{} bytes", object.size),
                actual: format!("{} bytes", metadata.len()),
            });
        }
        let mut reader = BufReader::new(File::open(&destination)?);
        let digest = crate::hash::sha256_reader_hex(&mut reader)?;
        if digest != object.sha256 {
            return Err(Error::ChecksumMismatch {
                expected: object.sha256.clone(),
                actual: digest,
            });
        }
        result.push(destination);
    }
    Ok(result)
}

fn debian_packages_object(snapshot: &SourceSnapshotV1) -> Result<&SourceMetadataObjectV1> {
    if snapshot.authenticated_objects.len() != 1
        || snapshot.authenticated_objects[0].role != SourceMetadataObjectRoleV1::DebianPackages
    {
        return Err(Error::ConfigError(format!(
            "Debian source snapshot '{}' must bind exactly one DebianPackages object",
            snapshot.source_identity
        )));
    }
    Ok(&snapshot.authenticated_objects[0])
}

fn project_package(
    profile: &ProfileRevisionV2,
    member_ordinal: u32,
    snapshot: &SourceSnapshotV1,
    package: AptPackage,
) -> Result<NativeParityPackageV1> {
    validate_sha256(&package.sha256, &package.name)?;
    let size = package.size.parse::<u64>().map_err(|error| {
        Error::ParseError(format!(
            "apt-pkg package '{}' has invalid Size '{}': {error}",
            package.name, package.size
        ))
    })?;
    crate::repository::parsers::common::validate_filename(&package.filename)
        .map_err(Error::ParseError)?;
    let multi_arch = package
        .multi_arch
        .as_deref()
        .map(DebianMultiArch::parse_exact)
        .transpose()
        .map_err(Error::ParseError)?
        .unwrap_or_default();
    let member = &profile.members[usize::try_from(member_ordinal)
        .map_err(|_| Error::ConfigError("Debian member ordinal exceeds usize".to_string()))?];
    let base_url = snapshot
        .provenance
        .content_url
        .as_deref()
        .unwrap_or(&snapshot.provenance.metadata_url);

    let mut provides = vec![CatalogProvideRecordV1 {
        capability: package.name.clone(),
        version: Some(package.version.clone()),
        version_relation: Some(ProvideVersionRelation::Equal),
        kind: "package".to_string(),
        raw: None,
        version_scheme: VersionScheme::Debian,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: CapabilityProvenance::ExactIdentity,
    }];
    for (record_index, provide) in package.provides.into_iter().enumerate() {
        let record_index = u32::try_from(record_index)
            .map_err(|_| Error::ParseError("Debian provide index exceeds u32".to_string()))?;
        let version_relation = provide
            .version
            .as_ref()
            .map(|_| ProvideVersionRelation::Equal);
        provides.push(CatalogProvideRecordV1 {
            kind: if provide.name == package.name {
                "package".to_string()
            } else {
                "virtual".to_string()
            },
            capability: provide.name,
            version: provide.version,
            version_relation,
            raw: Some(provide.native_text),
            version_scheme: VersionScheme::Debian,
            architecture_qualifier: provide_architecture_qualifier(
                provide.architecture_qualifier,
                provide.architecture,
            )?,
            provenance: CapabilityProvenance::SourceDeclared {
                format: SourcePackageFormat::Debian,
                record_index,
            },
        });
    }

    let requirement_groups = package
        .relation_groups
        .into_iter()
        .map(project_requirement_group)
        .collect::<Result<Vec<_>>>()?;
    let mut row = NativeParityPackageV1 {
        package_key_sha256: String::new(),
        member_ordinal,
        source_identity: member.source_identity.clone(),
        repository_identity: member.repository_identity.clone(),
        source_snapshot_sha256: member.source_snapshot_sha256.clone(),
        source_profile: profile.profile.clone(),
        name: package.name,
        version: package.version,
        package_release: String::new(),
        architecture: Some(package.architecture),
        debian_multi_arch: Some(multi_arch),
        checksum: format!("sha256:{}", package.sha256),
        size,
        download_url: crate::repository::parsers::common::join_repo_url(
            base_url,
            &package.filename,
        ),
        version_scheme: VersionScheme::Debian,
        provides,
        requirement_groups,
    };
    row.canonicalize_for_profile(&profile.profile)?;
    Ok(row)
}

fn project_requirement_group(group: ffi::AptRelationGroup) -> Result<CatalogRequirementGroupV1> {
    let kind = requirement_kind(group.kind);
    let clauses = group
        .atoms
        .iter()
        .map(project_clause)
        .collect::<Result<Vec<_>>>()?;
    if clauses.is_empty() {
        return Err(Error::ParseError(
            "apt-pkg returned an empty Debian requirement group".to_string(),
        ));
    }
    let expression = if clauses.len() == 1 {
        RepositoryRequirementExpression::Atom(clauses[0].clone())
    } else {
        RepositoryRequirementExpression::Or(
            clauses
                .iter()
                .cloned()
                .map(RepositoryRequirementExpression::Atom)
                .collect(),
        )
    };
    let expression_json =
        String::from_utf8(crate::json::canonical_json(&expression).map_err(|error| {
            Error::ParseError(format!("serialize Debian requirement expression: {error}"))
        })?)
        .map_err(|error| {
            Error::InternalError(format!("Debian expression is not UTF-8: {error}"))
        })?;
    let dependency_type = if matches!(
        kind,
        RepositoryRequirementKind::Recommends
            | RepositoryRequirementKind::Suggests
            | RepositoryRequirementKind::Enhances
    ) {
        "optional"
    } else {
        "runtime"
    };
    Ok(CatalogRequirementGroupV1 {
        kind: kind.as_str().to_string(),
        behavior: "hard".to_string(),
        description: None,
        native_text: Some(group.native_text),
        expression_json,
        atoms: clauses
            .into_iter()
            .map(|clause| CatalogRequirementAtomV1 {
                capability: clause.name,
                version_constraint: clause.version_constraint,
                kind: capability_kind(
                    clause
                        .capability_kind
                        .unwrap_or(RepositoryCapabilityKind::PackageName),
                )
                .to_string(),
                dependency_type: dependency_type.to_string(),
                raw: clause.native_text,
            })
            .collect(),
    })
}

fn project_clause(atom: &AptAtom) -> Result<RepositoryRequirementClause> {
    let version_constraint = match (atom.relation, atom.version.as_deref()) {
        (0, None) => None,
        (1, Some(version)) => Some(format!("<= {version}")),
        (2, Some(version)) => Some(format!(">= {version}")),
        (3, Some(version)) => Some(format!("<< {version}")),
        (4, Some(version)) => Some(format!(">> {version}")),
        (5, Some(version)) => Some(format!("= {version}")),
        (relation, version) => {
            return Err(Error::ParseError(format!(
                "apt-pkg returned inconsistent Debian relation {relation} and version {version:?}"
            )));
        }
    };
    Ok(RepositoryRequirementClause {
        name: atom.name.clone(),
        capability_kind: None,
        version_constraint,
        architecture_qualifier: requirement_architecture_qualifier(
            atom.architecture_qualifier,
            atom.architecture.clone(),
        )?,
        native_text: Some(atom.native_text.clone()),
    })
}

fn requirement_kind(kind: AptRelationKind) -> RepositoryRequirementKind {
    match kind {
        AptRelationKind::Depends => RepositoryRequirementKind::Depends,
        AptRelationKind::PreDepends => RepositoryRequirementKind::PreDepends,
        AptRelationKind::Recommends => RepositoryRequirementKind::Recommends,
        AptRelationKind::Suggests => RepositoryRequirementKind::Suggests,
        AptRelationKind::Enhances => RepositoryRequirementKind::Enhances,
        AptRelationKind::Conflicts => RepositoryRequirementKind::Conflict,
        AptRelationKind::Breaks => RepositoryRequirementKind::Breaks,
        AptRelationKind::Replaces => RepositoryRequirementKind::Replace,
    }
}

fn requirement_architecture_qualifier(
    qualifier: AptArchitectureQualifier,
    architecture: Option<String>,
) -> Result<RequirementArchitectureQualifier> {
    match (qualifier, architecture) {
        (AptArchitectureQualifier::Unqualified, None) => {
            Ok(RequirementArchitectureQualifier::Unqualified)
        }
        (AptArchitectureQualifier::Any, None) => Ok(RequirementArchitectureQualifier::Any),
        (AptArchitectureQualifier::Native, None) => Ok(RequirementArchitectureQualifier::Native),
        (AptArchitectureQualifier::Exact, Some(architecture)) => {
            Ok(RequirementArchitectureQualifier::Exact(architecture))
        }
        (qualifier, architecture) => Err(Error::ParseError(format!(
            "apt-pkg returned inconsistent architecture qualifier {qualifier:?} and value {architecture:?}"
        ))),
    }
}

fn provide_architecture_qualifier(
    qualifier: AptArchitectureQualifier,
    architecture: Option<String>,
) -> Result<ProvideArchitectureQualifier> {
    match (qualifier, architecture) {
        (AptArchitectureQualifier::Unqualified, None) => Ok(ProvideArchitectureQualifier::Implicit),
        (AptArchitectureQualifier::Any, None) => Ok(ProvideArchitectureQualifier::Any),
        (AptArchitectureQualifier::Native, None) => {
            Ok(ProvideArchitectureQualifier::Exact("native".to_string()))
        }
        (AptArchitectureQualifier::Exact, Some(architecture)) => {
            Ok(ProvideArchitectureQualifier::Exact(architecture))
        }
        (qualifier, architecture) => Err(Error::ParseError(format!(
            "apt-pkg returned inconsistent provide architecture qualifier {qualifier:?} and value {architecture:?}"
        ))),
    }
}

fn validate_sha256(value: &str, package: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::ParseError(format!(
            "apt-pkg package '{package}' has invalid SHA256 '{value}'"
        )));
    }
    Ok(())
}

fn require_regular_file(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(Error::InvalidPath(format!(
            "{label} {} must be a regular file, never a symlink",
            path.display()
        )));
    }
    Ok(())
}

fn capability_kind(kind: RepositoryCapabilityKind) -> &'static str {
    match kind {
        RepositoryCapabilityKind::PackageName => "package",
        RepositoryCapabilityKind::Virtual => "virtual",
        RepositoryCapabilityKind::Soname => "soname",
        RepositoryCapabilityKind::File => "file",
        RepositoryCapabilityKind::Path => "path",
        RepositoryCapabilityKind::Binary => "binary",
        RepositoryCapabilityKind::PkgConfig => "pkgconfig",
        RepositoryCapabilityKind::PkgConfig32 => "pkgconfig32",
        RepositoryCapabilityKind::Comar => "comar",
        RepositoryCapabilityKind::Generic => "generic",
    }
}

fn checked_increment(value: u64, label: &str) -> Result<u64> {
    value
        .checked_add(1)
        .ok_or_else(|| Error::InternalError(format!("Debian {label} exceed u64")))
}
