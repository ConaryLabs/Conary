// crates/conary-core/src/repository/catalog/parity/rpm/mod.rs

//! Independent libsolv-backed RPM native parity production.

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
use crate::repository::architecture::require_known_package_architecture;
use crate::repository::catalog::parity::support::{
    Counter, RegularFileError, checked_increment, require_regular_file,
};
use crate::repository::catalog::{
    CatalogProvideRecordV1, CatalogRequirementAtomV1, CatalogRequirementGroupV1, ProfileRevisionV2,
    SourceEcosystemV1, SourceMetadataObjectRoleV1, SourceMetadataObjectV1, SourceSnapshotV1,
};
use crate::repository::dependency_model::{
    ProvideArchitectureQualifier, ProvideVersionRelation, RepositoryCapabilityKind,
    RepositoryRequirementClause, RepositoryRequirementExpression, RepositoryRequirementGroup,
    RepositoryRequirementKind,
};
use crate::repository::dependency_source::{CapabilityProvenance, SourcePackageFormat};
use crate::repository::versioning::VersionScheme;

mod ffi;
mod resolution;

#[cfg(test)]
mod tests;

use ffi::{DependencyField, SolvDependency, SolvPackage, SolvPool};

pub const RPM_PARITY_PROJECTION_SCHEMA_V1: u32 = 1;
pub use resolution::{
    RPM_RESOLUTION_PROJECTION_SCHEMA_V5, produce_rpm_resolution_oracle,
    produce_rpm_resolution_oracle_with_workers, produce_rpm_resolution_survey,
    produce_rpm_resolution_survey_with_workers,
};
pub const PINNED_LIBSOLV_VERSION: &str = "0.7.36";

const CREATE_SPOOL: &str = "
CREATE TABLE packages (
    package_key_sha256 TEXT PRIMARY KEY,
    row_json BLOB NOT NULL
) STRICT;
";

/// Exact authenticated inputs for one ordered RPM profile member.
pub struct RpmParityMemberInput<'a> {
    pub source_snapshot: &'a SourceSnapshotV1,
    pub primary: &'a Path,
    pub filelists: &'a Path,
}

/// Produce and independently reopen one strict RPM native parity bundle.
pub fn produce_rpm_parity_oracle(
    profile: &ProfileRevisionV2,
    inputs: &[RpmParityMemberInput<'_>],
    output: &Path,
) -> Result<NativeParityOracleV1> {
    validate_inputs(profile, inputs)?;
    let libsolv_version = SolvPool::version()?;
    if libsolv_version != PINNED_LIBSOLV_VERSION {
        return Err(Error::ConfigError(format!(
            "RPM parity requires libsolv {PINNED_LIBSOLV_VERSION}, found {libsolv_version}"
        )));
    }
    let staging = tempfile::Builder::new()
        .prefix("conary-rpm-parity-")
        .tempdir()?;
    let staged = stage_verified_metadata(inputs, staging.path())?;
    let mut pool = SolvPool::create()?;
    for (ordinal, member) in staged.iter().enumerate() {
        let ordinal = u32::try_from(ordinal)
            .map_err(|_| Error::ConfigError("RPM member ordinal exceeds u32".to_string()))?;
        pool.load(
            &format!("conary-member-{ordinal}"),
            &member.primary,
            &member.filelists,
            ordinal,
            profile.members[ordinal as usize].precedence,
        )?;
    }

    let spool = Connection::open(staging.path().join("oracle-rows.sqlite"))?;
    spool.execute_batch(CREATE_SPOOL)?;
    let mut native_packages = 0_u64;
    let mut selected_packages = 0_u64;
    let mut exact_duplicates = 0_u64;
    for index in 0..pool.package_count() {
        native_packages = checked_increment(
            native_packages,
            Counter::NativeRows("native RPM package rows"),
        )?;
        let package = pool.package(index)?;
        let row = project_package(profile, inputs, &package)?;
        let bytes = crate::json::canonical_json(&row).map_err(|error| {
            Error::ParseError(format!("serialize RPM parity package row: {error}"))
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
                        "reopen staged RPM parity package '{}': {error}",
                        row.package_key_sha256
                    ))
                })?;
            if !selected.has_same_profile_facts(&row)? {
                return Err(Error::ConflictError(format!(
                    "RPM profile '{}' has contradictory package identity {} {} {:?}",
                    profile.profile, row.name, row.version, row.architecture
                )));
            }
            exact_duplicates = checked_increment(
                exact_duplicates,
                Counter::NativeRows("exact RPM duplicates"),
            )?;
            continue;
        }
        spool.execute(
            "INSERT INTO packages (package_key_sha256, row_json) VALUES (?1, ?2)",
            params![row.package_key_sha256, bytes],
        )?;
        selected_packages = checked_increment(
            selected_packages,
            Counter::NativeRows("selected RPM package rows"),
        )?;
    }
    if native_packages
        != selected_packages
            .checked_add(exact_duplicates)
            .ok_or_else(|| Error::InternalError("RPM package accounting exceeds u64".to_string()))?
    {
        return Err(Error::InternalError(
            "RPM package accounting lost a native row".to_string(),
        ));
    }

    let implementation = NativeParityImplementationV1 {
        ecosystem: NativeParityEcosystemV1::Rpm,
        name: "libsolv".to_string(),
        version: libsolv_version,
        projection_schema: RPM_PARITY_PROJECTION_SCHEMA_V1,
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
            Error::InternalError(format!("reopen staged RPM parity row: {error}"))
        })?;
        writer.package(&package)?;
        written = checked_increment(written, Counter::NativeRows("written RPM package rows"))?;
    }
    if written != selected_packages {
        return Err(Error::InternalError(format!(
            "RPM spool selected {selected_packages} packages but replayed {written}"
        )));
    }
    let manifest = writer.finish()?;
    write_native_parity_oracle_manifest(output, &manifest)?;
    let reopened = verify_native_parity_oracle_bundle(output, profile)?;
    if reopened.manifest() != &manifest {
        return Err(Error::InternalError(
            "reopened RPM parity manifest differs from produced manifest".to_string(),
        ));
    }
    Ok(manifest)
}

struct StagedRpmMetadata {
    primary: PathBuf,
    filelists: PathBuf,
}

fn validate_inputs(profile: &ProfileRevisionV2, inputs: &[RpmParityMemberInput<'_>]) -> Result<()> {
    profile.validate()?;
    if inputs.len() != profile.members.len() {
        return Err(Error::ConfigError(format!(
            "RPM parity received {} member inputs for {} profile members",
            inputs.len(),
            profile.members.len()
        )));
    }
    for pair in profile.members.windows(2) {
        if pair[0].precedence <= pair[1].precedence {
            return Err(Error::ConfigError(format!(
                "RPM profile member precedence must strictly descend; ordinals {} and {} carry {} and {}",
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
                "RPM source snapshot disagrees with profile member ordinal {}",
                member.ordinal
            )));
        }
        if snapshot.provenance.ecosystem != SourceEcosystemV1::Rpm {
            return Err(Error::ConfigError(format!(
                "profile member ordinal {} is not an RPM source snapshot",
                member.ordinal
            )));
        }
        rpm_metadata_objects(snapshot)?;
        require_regular_file(
            input.primary,
            "RPM primary metadata",
            RegularFileError::Config,
        )?;
        require_regular_file(
            input.filelists,
            "RPM filelists metadata",
            RegularFileError::Config,
        )?;
    }
    Ok(())
}

fn stage_verified_metadata(
    inputs: &[RpmParityMemberInput<'_>],
    staging: &Path,
) -> Result<Vec<StagedRpmMetadata>> {
    let mut result = Vec::with_capacity(inputs.len());
    for (ordinal, input) in inputs.iter().enumerate() {
        let (primary_object, filelists_object) = rpm_metadata_objects(input.source_snapshot)?;
        let member = staging.join(format!("member-{ordinal}"));
        fs::create_dir(&member)?;
        let primary = stage_verified_object(input.primary, primary_object, &member, "primary")?;
        let filelists =
            stage_verified_object(input.filelists, filelists_object, &member, "filelists")?;
        result.push(StagedRpmMetadata { primary, filelists });
    }
    Ok(result)
}

fn stage_verified_object(
    source: &Path,
    object: &SourceMetadataObjectV1,
    directory: &Path,
    label: &str,
) -> Result<PathBuf> {
    let basename = Path::new(&object.source_path)
        .file_name()
        .ok_or_else(|| Error::ConfigError(format!("RPM {label} source path has no file name")))?;
    let destination = directory.join(format!("{label}-{}", basename.to_string_lossy()));
    fs::copy(source, &destination)?;
    require_regular_file(
        &destination,
        &format!("staged RPM {label} metadata"),
        RegularFileError::Config,
    )?;
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
    Ok(destination)
}

fn rpm_metadata_objects(
    snapshot: &SourceSnapshotV1,
) -> Result<(&SourceMetadataObjectV1, &SourceMetadataObjectV1)> {
    if snapshot.authenticated_objects.len() != 2 {
        return Err(Error::ConfigError(format!(
            "RPM source snapshot '{}' must bind exactly primary and filelists metadata",
            snapshot.source_identity
        )));
    }
    let primary = &snapshot.authenticated_objects[0];
    let filelists = &snapshot.authenticated_objects[1];
    if primary.role != SourceMetadataObjectRoleV1::RpmPrimary
        || filelists.role != SourceMetadataObjectRoleV1::RpmFilelists
    {
        return Err(Error::ConfigError(format!(
            "RPM source snapshot '{}' must bind ordered primary and filelists metadata",
            snapshot.source_identity
        )));
    }
    Ok((primary, filelists))
}

fn project_package(
    profile: &ProfileRevisionV2,
    inputs: &[RpmParityMemberInput<'_>],
    package: &SolvPackage<'_>,
) -> Result<NativeParityPackageV1> {
    let member_ordinal = package.member();
    let member_index = usize::try_from(member_ordinal)
        .map_err(|_| Error::ConfigError("RPM member ordinal exceeds usize".to_string()))?;
    let member = profile.members.get(member_index).ok_or_else(|| {
        Error::InternalError(format!(
            "libsolv package names absent member ordinal {member_ordinal}"
        ))
    })?;
    let snapshot = inputs
        .get(member_index)
        .ok_or_else(|| Error::InternalError("RPM input/member accounting drift".to_string()))?
        .source_snapshot;
    let name = package.name()?;
    let version = canonical_rpm_evr(&package.evr()?)?.to_string();
    let architecture = package.arch()?;
    require_known_package_architecture(VersionScheme::Rpm, &architecture)?;
    let architecture = Some(architecture);
    let checksum = package.checksum()?;
    if !crate::hash::is_canonical_sha256(&checksum) {
        return Err(Error::ParseError(format!(
            "libsolv package '{name}' has invalid SHA-256 '{checksum}'"
        )));
    }
    let location = package.location()?;
    crate::repository::parsers::common::validate_filename(&location).map_err(Error::ParseError)?;
    let base_url = snapshot
        .provenance
        .content_url
        .as_deref()
        .unwrap_or(&snapshot.provenance.metadata_url);

    let mut provides = vec![CatalogProvideRecordV1 {
        capability: name.clone(),
        version: Some(version.clone()),
        version_relation: Some(ProvideVersionRelation::Equal),
        kind: "package".to_string(),
        raw: None,
        version_scheme: VersionScheme::Rpm,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: CapabilityProvenance::ExactIdentity,
    }];
    for (record_index, dependency) in package
        .dependencies(DependencyField::Provides)?
        .into_iter()
        .enumerate()
    {
        provides.push(project_provide(&name, dependency, record_index)?);
    }
    for path in package.files()? {
        if !path.starts_with('/') {
            return Err(Error::ParseError(format!(
                "libsolv package '{name}' has non-absolute file path '{path}'"
            )));
        }
        provides.push(CatalogProvideRecordV1 {
            capability: path,
            version: None,
            version_relation: None,
            kind: "file".to_string(),
            raw: None,
            version_scheme: VersionScheme::Rpm,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            provenance: CapabilityProvenance::SourceDerivedFile {
                format: SourcePackageFormat::Rpm,
            },
        });
    }

    let mut requirement_groups = Vec::new();
    extend_requires(
        &mut requirement_groups,
        package.dependencies(DependencyField::Requires)?,
    )?;
    extend_relations(
        &mut requirement_groups,
        package.dependencies(DependencyField::Recommends)?,
        RepositoryRequirementKind::Recommends,
    )?;
    extend_relations(
        &mut requirement_groups,
        package.dependencies(DependencyField::Suggests)?,
        RepositoryRequirementKind::Suggests,
    )?;
    extend_supplements(
        &mut requirement_groups,
        package.dependencies(DependencyField::Supplements)?,
        &provides,
    )?;
    extend_relations(
        &mut requirement_groups,
        package.dependencies(DependencyField::Enhances)?,
        RepositoryRequirementKind::Enhances,
    )?;
    extend_relations(
        &mut requirement_groups,
        package.dependencies(DependencyField::Conflicts)?,
        RepositoryRequirementKind::Conflict,
    )?;
    extend_relations(
        &mut requirement_groups,
        package.dependencies(DependencyField::Obsoletes)?,
        RepositoryRequirementKind::Obsolete,
    )?;

    let mut row = NativeParityPackageV1 {
        package_key_sha256: String::new(),
        member_ordinal,
        source_identity: member.source_identity.clone(),
        repository_identity: member.repository_identity.clone(),
        source_snapshot_sha256: member.source_snapshot_sha256.clone(),
        source_profile: profile.profile.clone(),
        name,
        version,
        package_release: String::new(),
        architecture,
        debian_multi_arch: None,
        checksum: format!("sha256:{checksum}"),
        size: package.size()?,
        download_url: crate::repository::parsers::common::join_repo_url(base_url, &location),
        version_scheme: VersionScheme::Rpm,
        provides,
        requirement_groups,
    };
    row.canonicalize_for_profile(&profile.profile)?;
    Ok(row)
}

fn project_provide(
    package_name: &str,
    dependency: SolvDependency<'_>,
    record_index: usize,
) -> Result<CatalogProvideRecordV1> {
    if dependency.is_prereq_marker() {
        return Err(Error::ParseError(
            "libsolv returned a prerequisite marker in RPM provides".to_string(),
        ));
    }
    let raw = dependency.text()?;
    let (capability, version, version_relation) = if dependency.is_relation() {
        let relation = dependency.relation()?;
        let version_relation = provide_relation(relation.flags)?;
        let capability = relation.name_dependency()?.atom()?;
        let version = canonical_rpm_evr(&relation.evr_dependency()?.atom()?)?.to_string();
        (capability, Some(version), Some(version_relation))
    } else {
        (dependency.atom()?, None, None)
    };
    let record_index = u32::try_from(record_index)
        .map_err(|_| Error::ParseError("RPM provide index exceeds u32".to_string()))?;
    Ok(CatalogProvideRecordV1 {
        kind: if capability == package_name {
            "package".to_string()
        } else {
            "generic".to_string()
        },
        capability,
        version,
        version_relation,
        raw: Some(raw),
        version_scheme: VersionScheme::Rpm,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: CapabilityProvenance::SourceDeclared {
            format: SourcePackageFormat::Rpm,
            record_index,
        },
    })
}

fn extend_requires(
    groups: &mut Vec<CatalogRequirementGroupV1>,
    dependencies: Vec<SolvDependency<'_>>,
) -> Result<()> {
    let mut prerequisite = false;
    for dependency in dependencies {
        if dependency.is_prereq_marker() {
            if prerequisite {
                return Err(Error::ParseError(
                    "libsolv returned repeated RPM prerequisite markers".to_string(),
                ));
            }
            prerequisite = true;
            continue;
        }
        groups.push(project_requirement(
            dependency,
            if prerequisite {
                RepositoryRequirementKind::PreDepends
            } else {
                RepositoryRequirementKind::Depends
            },
        )?);
    }
    Ok(())
}

fn extend_relations(
    groups: &mut Vec<CatalogRequirementGroupV1>,
    dependencies: Vec<SolvDependency<'_>>,
    kind: RepositoryRequirementKind,
) -> Result<()> {
    for dependency in dependencies {
        if dependency.is_prereq_marker() {
            return Err(Error::ParseError(format!(
                "libsolv returned an RPM prerequisite marker in {}",
                kind.as_str()
            )));
        }
        groups.push(project_requirement(dependency, kind)?);
    }
    Ok(())
}

fn extend_supplements(
    groups: &mut Vec<CatalogRequirementGroupV1>,
    dependencies: Vec<SolvDependency<'_>>,
    provides: &[CatalogProvideRecordV1],
) -> Result<()> {
    for dependency in dependencies {
        if classify_split_provides_supplement(&dependency, provides)? {
            continue;
        }
        if dependency.is_prereq_marker() {
            return Err(Error::ParseError(
                "libsolv returned an RPM prerequisite marker in supplements".to_string(),
            ));
        }
        groups.push(project_requirement(
            dependency,
            RepositoryRequirementKind::Supplements,
        )?);
    }
    Ok(())
}

fn classify_split_provides_supplement(
    dependency: &SolvDependency<'_>,
    provides: &[CatalogProvideRecordV1],
) -> Result<bool> {
    if !dependency.is_relation() {
        return Ok(false);
    }
    let relation = dependency.relation()?;
    if relation.flags != ffi::REL_NAMESPACE {
        return Ok(false);
    }
    let namespace = atomic_dependency(&relation.name_dependency()?, "namespace name")?;
    let value = relation.evr_dependency()?;
    let (nested_flags, prefix, path) = if value.is_relation() {
        let nested = value.relation()?;
        let prefix = nested.name_dependency()?;
        let path = nested.evr_dependency()?;
        (
            nested.flags,
            if prefix.is_relation() {
                None
            } else {
                Some(prefix.atom()?)
            },
            if path.is_relation() {
                None
            } else {
                Some(path.atom()?)
            },
        )
    } else {
        (0, None, None)
    };
    let (capability, path) = validate_split_provides_shape(
        &namespace,
        nested_flags,
        prefix.as_deref(),
        path.as_deref(),
    )?;
    let declared = provides.iter().any(|provide| {
        provide.capability == capability
            && provide.version.is_none()
            && provide.version_relation.is_none()
            && provide.kind == "generic"
            && provide.raw.as_deref() == Some(capability.as_str())
            && matches!(
                &provide.provenance,
                CapabilityProvenance::SourceDeclared {
                    format: SourcePackageFormat::Rpm,
                    ..
                }
            )
    });
    let file = provides.iter().any(|provide| {
        split_provides_path_is_covered(&provide.capability, &path)
            && provide.kind == "file"
            && matches!(
                &provide.provenance,
                CapabilityProvenance::SourceDerivedFile {
                    format: SourcePackageFormat::Rpm
                }
            )
    });
    if !declared || !file {
        return Err(Error::ConflictError(format!(
            "libsolv split-provides supplement '{capability}' has no matching declared capability and file"
        )));
    }
    Ok(true)
}

fn split_provides_path_is_covered(candidate: &str, path: &str) -> bool {
    is_canonical_split_provides_path(candidate)
        && (candidate == path
            || candidate
                .strip_prefix(path)
                .is_some_and(|suffix| suffix.starts_with('/') && suffix.len() > 1))
}

fn is_canonical_split_provides_path(path: &str) -> bool {
    path.starts_with('/')
        && path != "/"
        && !path.ends_with('/')
        && !path.chars().any(char::is_whitespace)
        && !path
            .split('/')
            .skip(1)
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
}

fn atomic_dependency(dependency: &SolvDependency<'_>, field: &str) -> Result<String> {
    if dependency.is_relation() {
        return Err(Error::ConflictError(format!(
            "libsolv split-provides {field} is another relation"
        )));
    }
    dependency.atom()
}

fn validate_split_provides_shape(
    namespace: &str,
    nested_flags: i32,
    prefix: Option<&str>,
    path: Option<&str>,
) -> Result<(String, String)> {
    if namespace != "namespace:splitprovides" {
        return Err(Error::ConflictError(format!(
            "libsolv returned unsupported RPM supplement namespace '{namespace}'"
        )));
    }
    if nested_flags != ffi::REL_WITH {
        return Err(Error::ConflictError(format!(
            "libsolv split-provides namespace contains relation flag {nested_flags}, expected REL_WITH"
        )));
    }
    let prefix = prefix.ok_or_else(|| {
        Error::ConflictError(
            "libsolv split-provides namespace has a non-atomic capability prefix".to_string(),
        )
    })?;
    let path = path.ok_or_else(|| {
        Error::ConflictError(
            "libsolv split-provides namespace has a non-atomic file path".to_string(),
        )
    })?;
    if prefix.is_empty()
        || prefix.contains(':')
        || prefix.chars().any(char::is_whitespace)
        || !is_canonical_split_provides_path(path)
    {
        return Err(Error::ConflictError(format!(
            "libsolv split-provides namespace has invalid operands '{prefix}' and '{path}'"
        )));
    }
    Ok((format!("{prefix}:{path}"), path.to_string()))
}

fn project_requirement(
    dependency: SolvDependency<'_>,
    kind: RepositoryRequirementKind,
) -> Result<CatalogRequirementGroupV1> {
    let native_expression = decode_expression(&dependency)?;
    let native_text = canonical_rpm_dependency_text(&native_expression);
    project_decoded_requirement(kind, native_text, native_expression)
}

fn project_decoded_requirement(
    kind: RepositoryRequirementKind,
    native_text: String,
    native_expression: RepositoryRequirementExpression,
) -> Result<CatalogRequirementGroupV1> {
    let group = if kind.is_negative_relation() {
        crate::repository::package_relation::parse_native_relation(
            kind,
            VersionScheme::Rpm,
            &native_text,
        )
    } else {
        crate::repository::requirement::parse_native_requirement(
            kind,
            VersionScheme::Rpm,
            &native_text,
        )
    }
    .map_err(Error::ParseError)?;
    if group.expression != native_expression {
        return Err(Error::ConflictError(format!(
            "libsolv typed relation tree disagrees with canonical RPM dependency '{native_text}'"
        )));
    }
    catalog_requirement_group(group)
}

fn canonical_rpm_dependency_text(expression: &RepositoryRequirementExpression) -> String {
    match expression {
        RepositoryRequirementExpression::Atom(clause) => {
            clause.version_constraint.as_ref().map_or_else(
                || clause.name.clone(),
                |constraint| format!("{} {constraint}", clause.name),
            )
        }
        RepositoryRequirementExpression::And(operands) => {
            canonical_rpm_boolean_text(operands, "and")
        }
        RepositoryRequirementExpression::Or(operands) => canonical_rpm_boolean_text(operands, "or"),
        RepositoryRequirementExpression::If {
            requirement,
            condition,
            otherwise,
        } => canonical_rpm_conditional_text(requirement, condition, otherwise.as_deref(), "if"),
        RepositoryRequirementExpression::Unless {
            requirement,
            condition,
            otherwise,
        } => canonical_rpm_conditional_text(requirement, condition, otherwise.as_deref(), "unless"),
        RepositoryRequirementExpression::With { left, right } => {
            let mut text = format!("({} with ", canonical_rpm_dependency_text(left));
            canonical_rpm_with_right_text(right, &mut text);
            text.push(')');
            text
        }
        RepositoryRequirementExpression::Without { left, right } => format!(
            "({} without {})",
            canonical_rpm_dependency_text(left),
            canonical_rpm_dependency_text(right)
        ),
    }
}

fn canonical_rpm_boolean_text(
    operands: &[RepositoryRequirementExpression],
    operator: &str,
) -> String {
    let separator = format!(" {operator} ");
    format!(
        "({})",
        operands
            .iter()
            .map(canonical_rpm_dependency_text)
            .collect::<Vec<_>>()
            .join(&separator)
    )
}

fn canonical_rpm_conditional_text(
    requirement: &RepositoryRequirementExpression,
    condition: &RepositoryRequirementExpression,
    otherwise: Option<&RepositoryRequirementExpression>,
    operator: &str,
) -> String {
    let mut text = format!(
        "({} {operator} {}",
        canonical_rpm_dependency_text(requirement),
        canonical_rpm_dependency_text(condition)
    );
    if let Some(otherwise) = otherwise {
        text.push_str(" else ");
        text.push_str(&canonical_rpm_dependency_text(otherwise));
    }
    text.push(')');
    text
}

fn canonical_rpm_with_right_text(expression: &RepositoryRequirementExpression, text: &mut String) {
    if let RepositoryRequirementExpression::With { left, right } = expression {
        text.push_str(&canonical_rpm_dependency_text(left));
        text.push_str(" with ");
        canonical_rpm_with_right_text(right, text);
    } else {
        text.push_str(&canonical_rpm_dependency_text(expression));
    }
}

fn decode_expression(dependency: &SolvDependency<'_>) -> Result<RepositoryRequirementExpression> {
    if !dependency.is_relation() {
        return Ok(RepositoryRequirementExpression::Atom(
            RepositoryRequirementClause::name_only(dependency.atom()?),
        ));
    }
    let relation = dependency.relation()?;
    match relation.flags {
        flags
            if matches!(flags, ffi::REL_GT | ffi::REL_EQ | ffi::REL_LT)
                || flags == ffi::REL_GT | ffi::REL_EQ
                || flags == ffi::REL_LT | ffi::REL_EQ =>
        {
            let name = relation.name_dependency()?.atom()?;
            let version = canonical_rpm_evr(&relation.evr_dependency()?.atom()?)?.to_string();
            let operator = match relation.flags {
                ffi::REL_GT => ">",
                ffi::REL_EQ => "=",
                ffi::REL_LT => "<",
                flags if flags == ffi::REL_GT | ffi::REL_EQ => ">=",
                flags if flags == ffi::REL_LT | ffi::REL_EQ => "<=",
                _ => unreachable!("matched RPM version relation"),
            };
            Ok(RepositoryRequirementExpression::Atom(
                RepositoryRequirementClause::versioned(name, format!("{operator} {version}")),
            ))
        }
        ffi::REL_AND | ffi::REL_OR => {
            let left = decode_expression(&relation.name_dependency()?)?;
            let right = decode_expression(&relation.evr_dependency()?)?;
            let mut operands = Vec::new();
            flatten_boolean(relation.flags, left, &mut operands);
            flatten_boolean(relation.flags, right, &mut operands);
            if relation.flags == ffi::REL_AND {
                Ok(RepositoryRequirementExpression::And(operands))
            } else {
                Ok(RepositoryRequirementExpression::Or(operands))
            }
        }
        ffi::REL_WITH | ffi::REL_WITHOUT => {
            let left = Box::new(decode_expression(&relation.name_dependency()?)?);
            let right = Box::new(decode_expression(&relation.evr_dependency()?)?);
            if relation.flags == ffi::REL_WITH {
                Ok(RepositoryRequirementExpression::With { left, right })
            } else {
                Ok(RepositoryRequirementExpression::Without { left, right })
            }
        }
        ffi::REL_COND | ffi::REL_UNLESS => {
            let requirement = Box::new(decode_expression(&relation.name_dependency()?)?);
            let condition_dependency = relation.evr_dependency()?;
            let (condition, otherwise) = if condition_dependency.is_relation()
                && condition_dependency.relation()?.flags == ffi::REL_ELSE
            {
                let otherwise_relation = condition_dependency.relation()?;
                (
                    Box::new(decode_expression(&otherwise_relation.name_dependency()?)?),
                    Some(Box::new(decode_expression(
                        &otherwise_relation.evr_dependency()?,
                    )?)),
                )
            } else {
                (Box::new(decode_expression(&condition_dependency)?), None)
            };
            if relation.flags == ffi::REL_COND {
                Ok(RepositoryRequirementExpression::If {
                    requirement,
                    condition,
                    otherwise,
                })
            } else {
                Ok(RepositoryRequirementExpression::Unless {
                    requirement,
                    condition,
                    otherwise,
                })
            }
        }
        flags => Err(Error::ParseError(format!(
            "libsolv returned unsupported RPM relation flag {flags} for dependency {}",
            dependency.id()
        ))),
    }
}

fn flatten_boolean(
    flags: i32,
    expression: RepositoryRequirementExpression,
    operands: &mut Vec<RepositoryRequirementExpression>,
) {
    match (flags, expression) {
        (ffi::REL_AND, RepositoryRequirementExpression::And(nested))
        | (ffi::REL_OR, RepositoryRequirementExpression::Or(nested)) => operands.extend(nested),
        (_, expression) => operands.push(expression),
    }
}

fn catalog_requirement_group(
    group: RepositoryRequirementGroup,
) -> Result<CatalogRequirementGroupV1> {
    let expression_json = String::from_utf8(
        crate::json::canonical_json(&group.expression).map_err(|error| {
            Error::ParseError(format!("serialize RPM requirement expression: {error}"))
        })?,
    )
    .map_err(|error| Error::InternalError(format!("RPM expression is not UTF-8: {error}")))?;
    let dependency_type = if matches!(
        group.kind,
        RepositoryRequirementKind::Optional
            | RepositoryRequirementKind::Recommends
            | RepositoryRequirementKind::Suggests
            | RepositoryRequirementKind::Supplements
            | RepositoryRequirementKind::Enhances
    ) {
        "optional"
    } else if group.kind == RepositoryRequirementKind::Build {
        "build"
    } else {
        "runtime"
    };
    Ok(CatalogRequirementGroupV1 {
        kind: group.kind.as_str().to_string(),
        behavior: if group.expression.is_conditional() {
            "conditional".to_string()
        } else {
            "hard".to_string()
        },
        description: group.description,
        native_text: group.native_text,
        expression_json,
        atoms: group
            .alternatives
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

fn provide_relation(flags: i32) -> Result<ProvideVersionRelation> {
    match flags {
        ffi::REL_LT => Ok(ProvideVersionRelation::LessThan),
        flags if flags == ffi::REL_LT | ffi::REL_EQ => Ok(ProvideVersionRelation::LessOrEqual),
        ffi::REL_EQ => Ok(ProvideVersionRelation::Equal),
        flags if flags == ffi::REL_GT | ffi::REL_EQ => Ok(ProvideVersionRelation::GreaterOrEqual),
        ffi::REL_GT => Ok(ProvideVersionRelation::GreaterThan),
        _ => Err(Error::ParseError(format!(
            "libsolv returned unsupported RPM provide relation flags {flags}"
        ))),
    }
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

fn canonical_rpm_evr(value: &str) -> Result<&str> {
    let value = crate::repository::rpm_dependency::canonicalize_source_rpm_evr(value)
        .map_err(Error::ParseError)?;
    Ok(value.strip_prefix("0:").unwrap_or(value))
}
