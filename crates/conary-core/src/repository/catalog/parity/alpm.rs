// crates/conary-core/src/repository/catalog/parity/alpm.rs

//! Independent libalpm-backed native parity production.

use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use alpm::{Alpm, Dep, DepMod, Package, SigLevel};
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
    ProvideArchitectureQualifier, ProvideVersionRelation, RepositoryCapabilityKind,
    RepositoryRequirementClause, RepositoryRequirementExpression, RepositoryRequirementKind,
};
use crate::repository::dependency_source::{CapabilityProvenance, SourcePackageFormat};
use crate::repository::versioning::VersionScheme;

mod resolution;

pub use resolution::ALPM_RESOLUTION_PROJECTION_SCHEMA_V1;

pub const ALPM_PARITY_PROJECTION_SCHEMA_V1: u32 = 1;

const CREATE_SPOOL: &str = "
CREATE TABLE packages (
    package_key_sha256 TEXT PRIMARY KEY,
    row_json BLOB NOT NULL
) STRICT;
";

/// Exact authenticated input for one ordered ALPM profile member.
pub struct AlpmParityMemberInput<'a> {
    pub source_snapshot: &'a SourceSnapshotV1,
    pub database: &'a Path,
}

/// Produce and independently reopen one strict ALPM parity bundle.
///
/// The implementation deliberately accepts source database artifacts rather
/// than a Conary catalog. Inputs correspond one-for-one with profile members
/// in ordinal and precedence order.
pub fn produce_alpm_parity_oracle(
    profile: &ProfileRevisionV2,
    inputs: &[AlpmParityMemberInput<'_>],
    output: &Path,
) -> Result<NativeParityOracleV1> {
    validate_inputs(profile, inputs)?;

    let staging = tempfile::Builder::new()
        .prefix("conary-alpm-parity-")
        .tempdir()?;
    let root = staging.path().join("root");
    let database_root = staging.path().join("database");
    let sync_root = database_root.join("sync");
    fs::create_dir(&root)?;
    fs::create_dir(&database_root)?;
    fs::create_dir(&sync_root)?;

    let staged_databases = stage_verified_databases(inputs, &sync_root)?;
    let alpm = Alpm::new(path_text(&root)?, path_text(&database_root)?)
        .map_err(|error| Error::InitError(format!("initialize libalpm: {error}")))?;
    for (ordinal, _) in staged_databases.iter().enumerate() {
        let name = database_name(ordinal)?;
        let database = alpm
            .register_syncdb(name, SigLevel::NONE)
            .map_err(|error| Error::ParseError(format!("register ALPM database: {error}")))?;
        database.is_valid().map_err(|error| {
            Error::ParseError(format!(
                "open ALPM database at member ordinal {ordinal}: {error}"
            ))
        })?;
    }
    if alpm.syncdbs().len() != inputs.len() {
        return Err(Error::InternalError(format!(
            "libalpm registered {} databases for {} profile members",
            alpm.syncdbs().len(),
            inputs.len()
        )));
    }

    let spool = Connection::open(staging.path().join("oracle-rows.sqlite"))?;
    spool.execute_batch(CREATE_SPOOL)?;
    let mut native_packages = 0_u64;
    let mut selected_packages = 0_u64;
    let mut exact_duplicates = 0_u64;

    for (ordinal, (database, input)) in alpm.syncdbs().iter().zip(inputs).enumerate() {
        let expected_database = database_name(ordinal)?;
        if database.name() != expected_database {
            return Err(Error::InternalError(format!(
                "libalpm returned database '{}' at profile ordinal {ordinal}; expected '{expected_database}'",
                database.name()
            )));
        }
        let member_ordinal = u32::try_from(ordinal)
            .map_err(|_| Error::ConfigError("ALPM member ordinal exceeds u32".to_string()))?;
        for package in database.pkgs().iter() {
            native_packages = checked_increment(native_packages, "native package rows")?;
            let row = project_package(profile, member_ordinal, input.source_snapshot, package)?;
            let bytes = crate::json::canonical_json(&row).map_err(|error| {
                Error::ParseError(format!("serialize ALPM parity package row: {error}"))
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
                            "reopen staged ALPM parity package '{}': {error}",
                            row.package_key_sha256
                        ))
                    })?;
                if !selected.has_same_profile_facts(&row) {
                    return Err(Error::ConflictError(format!(
                        "ALPM profile '{}' has contradictory package identity {} {} {:?}",
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
                Error::InternalError("ALPM package accounting exceeds u64".to_string())
            })?
    {
        return Err(Error::InternalError(
            "ALPM package accounting lost a native row".to_string(),
        ));
    }

    let implementation = NativeParityImplementationV1 {
        ecosystem: NativeParityEcosystemV1::Alpm,
        name: "libalpm".to_string(),
        version: alpm::version().to_string(),
        projection_schema: ALPM_PARITY_PROJECTION_SCHEMA_V1,
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
            Error::InternalError(format!("reopen staged ALPM parity row: {error}"))
        })?;
        writer.package(&package)?;
        written = checked_increment(written, "written package rows")?;
    }
    if written != selected_packages {
        return Err(Error::InternalError(format!(
            "ALPM spool selected {selected_packages} packages but replayed {written}"
        )));
    }
    let manifest = writer.finish()?;
    write_native_parity_oracle_manifest(output, &manifest)?;
    let reopened = verify_native_parity_oracle_bundle(output, profile)?;
    if reopened.manifest() != &manifest {
        return Err(Error::InternalError(
            "reopened ALPM parity manifest differs from produced manifest".to_string(),
        ));
    }
    Ok(manifest)
}

fn validate_inputs(
    profile: &ProfileRevisionV2,
    inputs: &[AlpmParityMemberInput<'_>],
) -> Result<()> {
    profile.validate()?;
    if inputs.len() != profile.members.len() {
        return Err(Error::ConfigError(format!(
            "ALPM parity received {} member inputs for {} profile members",
            inputs.len(),
            profile.members.len()
        )));
    }
    for pair in profile.members.windows(2) {
        if pair[0].precedence <= pair[1].precedence {
            return Err(Error::ConfigError(format!(
                "ALPM profile member precedence must strictly descend; ordinals {} and {} carry {} and {}",
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
                "ALPM source snapshot disagrees with profile member ordinal {}",
                member.ordinal
            )));
        }
        if snapshot.provenance.ecosystem != SourceEcosystemV1::Alpm {
            return Err(Error::ConfigError(format!(
                "profile member ordinal {} is not an ALPM source snapshot",
                member.ordinal
            )));
        }
        arch_database_object(snapshot)?;
        require_regular_file(input.database, "ALPM source database")?;
    }
    Ok(())
}

fn stage_verified_databases(
    inputs: &[AlpmParityMemberInput<'_>],
    sync_root: &Path,
) -> Result<Vec<PathBuf>> {
    let mut staged = Vec::with_capacity(inputs.len());
    for (ordinal, input) in inputs.iter().enumerate() {
        let object = arch_database_object(input.source_snapshot)?;
        let destination = sync_root.join(format!("{}.db", database_name(ordinal)?));
        fs::copy(input.database, &destination)?;
        require_regular_file(&destination, "staged ALPM database")?;
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
        staged.push(destination);
    }
    Ok(staged)
}

fn project_package(
    profile: &ProfileRevisionV2,
    member_ordinal: u32,
    snapshot: &SourceSnapshotV1,
    package: &Package,
) -> Result<NativeParityPackageV1> {
    let name = package.name().to_string();
    let version = package.version().to_string();
    let filename = required_text(package.filename(), &name, "filename")?;
    if filename == "." || filename == ".." || filename.contains('/') || filename.contains('\\') {
        return Err(Error::ParseError(format!(
            "ALPM package '{name}' has invalid repository filename '{filename}'"
        )));
    }
    let checksum = required_text(package.sha256sum(), &name, "SHA-256")?;
    if checksum.len() != 64
        || !checksum
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::ParseError(format!(
            "ALPM package '{name}' has invalid SHA-256 '{checksum}'"
        )));
    }
    let size = u64::try_from(package.size()).map_err(|_| {
        Error::ParseError(format!(
            "ALPM package '{name}' has negative compressed size"
        ))
    })?;
    let member = &profile.members[usize::try_from(member_ordinal)
        .map_err(|_| Error::ConfigError("ALPM member ordinal exceeds usize".to_string()))?];
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
        version_scheme: VersionScheme::Arch,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: CapabilityProvenance::ExactIdentity,
    }];
    for (index, dependency) in package.provides().iter().enumerate() {
        provides.push(project_provide(&name, dependency, index)?);
    }

    let mut requirement_groups = Vec::new();
    extend_requirements(
        &mut requirement_groups,
        package.depends().iter(),
        RepositoryRequirementKind::Depends,
    )?;
    extend_requirements(
        &mut requirement_groups,
        package.optdepends().iter(),
        RepositoryRequirementKind::Optional,
    )?;
    extend_requirements(
        &mut requirement_groups,
        package.conflicts().iter(),
        RepositoryRequirementKind::Conflict,
    )?;
    extend_requirements(
        &mut requirement_groups,
        package.replaces().iter(),
        RepositoryRequirementKind::Replace,
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
        architecture: package.arch().map(str::to_string),
        debian_multi_arch: None,
        checksum: format!("sha256:{checksum}"),
        size,
        download_url: format!("{}/{filename}", base_url.trim_end_matches('/')),
        version_scheme: VersionScheme::Arch,
        provides,
        requirement_groups,
    };
    row.canonicalize_for_profile(&profile.profile)?;
    Ok(row)
}

fn project_provide(
    package_name: &str,
    dependency: &Dep,
    index: usize,
) -> Result<CatalogProvideRecordV1> {
    let raw = dependency.to_string();
    let parsed = raw
        .parse::<alpm_types::RelationOrSoname>()
        .map_err(|error| {
            Error::ParseError(format!(
                "libalpm provided invalid ALPM capability '{raw}': {error}"
            ))
        })?;
    let (capability, kind, version) = match parsed {
        alpm_types::RelationOrSoname::Relation(relation) => {
            let version = relation
                .version_requirement
                .map(|requirement| {
                    if requirement.comparison != alpm_types::VersionComparison::Equal {
                        return Err(Error::ParseError(format!(
                            "ALPM provide '{raw}' is not an exact version relation"
                        )));
                    }
                    Ok(requirement.version.to_string())
                })
                .transpose()?;
            let capability = relation.name.to_string();
            let kind = if capability == package_name {
                RepositoryCapabilityKind::PackageName
            } else {
                RepositoryCapabilityKind::Virtual
            };
            (capability, kind, version)
        }
        alpm_types::RelationOrSoname::SonameV1(soname) => {
            (soname.to_string(), RepositoryCapabilityKind::Soname, None)
        }
        alpm_types::RelationOrSoname::SonameV2(soname) => {
            (soname.to_string(), RepositoryCapabilityKind::Soname, None)
        }
    };
    if dependency.name() != capability || dependency.version().map(ToString::to_string) != version {
        return Err(Error::ConflictError(format!(
            "libalpm dependency fields disagree with normalized provide '{raw}'"
        )));
    }
    let record_index = u32::try_from(index)
        .map_err(|_| Error::ParseError("ALPM provide index exceeds u32".to_string()))?;
    Ok(CatalogProvideRecordV1 {
        capability,
        version_relation: version.as_ref().map(|_| ProvideVersionRelation::Equal),
        version,
        kind: capability_kind(kind).to_string(),
        raw: Some(raw),
        version_scheme: VersionScheme::Arch,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: CapabilityProvenance::SourceDeclared {
            format: SourcePackageFormat::Alpm,
            record_index,
        },
    })
}

fn extend_requirements<'a>(
    groups: &mut Vec<CatalogRequirementGroupV1>,
    dependencies: impl Iterator<Item = &'a Dep>,
    kind: RepositoryRequirementKind,
) -> Result<()> {
    for dependency in dependencies {
        groups.push(project_requirement(dependency, kind)?);
    }
    Ok(())
}

fn project_requirement(
    dependency: &Dep,
    kind: RepositoryRequirementKind,
) -> Result<CatalogRequirementGroupV1> {
    let raw = dependency_relation_text(dependency)?;
    let clause = dependency_clause(dependency, kind == RepositoryRequirementKind::Depends)?;
    let description = (kind == RepositoryRequirementKind::Optional)
        .then(|| {
            dependency
                .desc()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .flatten();
    let native_text = description.as_ref().map_or_else(
        || raw.clone(),
        |description| format!("{raw}: {description}"),
    );
    if dependency.to_string() != native_text {
        return Err(Error::ConflictError(format!(
            "libalpm dependency fields disagree with native text '{}'",
            dependency
        )));
    }
    let expression = RepositoryRequirementExpression::Atom(clause.clone());
    let expression_json =
        String::from_utf8(crate::json::canonical_json(&expression).map_err(|error| {
            Error::ParseError(format!("serialize ALPM requirement expression: {error}"))
        })?)
        .map_err(|error| Error::InternalError(format!("ALPM expression is not UTF-8: {error}")))?;
    Ok(CatalogRequirementGroupV1 {
        kind: kind.as_str().to_string(),
        behavior: "hard".to_string(),
        description,
        native_text: Some(native_text),
        expression_json,
        atoms: vec![CatalogRequirementAtomV1 {
            capability: clause.name,
            version_constraint: clause.version_constraint,
            kind: capability_kind(
                clause
                    .capability_kind
                    .unwrap_or(RepositoryCapabilityKind::PackageName),
            )
            .to_string(),
            dependency_type: if kind == RepositoryRequirementKind::Optional {
                "optional".to_string()
            } else {
                "runtime".to_string()
            },
            raw: clause.native_text,
        }],
    })
}

fn dependency_clause(dependency: &Dep, allow_soname: bool) -> Result<RepositoryRequirementClause> {
    let raw = dependency_relation_text(dependency)?;
    if allow_soname
        && matches!(
            raw.parse::<alpm_types::RelationOrSoname>(),
            Ok(alpm_types::RelationOrSoname::SonameV1(_)
                | alpm_types::RelationOrSoname::SonameV2(_))
        )
    {
        let mut clause = RepositoryRequirementClause::name_only(raw.clone());
        clause.capability_kind = Some(RepositoryCapabilityKind::Soname);
        clause.native_text = Some(raw);
        return Ok(clause);
    }
    let constraint = dependency.version().and_then(|version| {
        dependency_operator(dependency.depmod()).map(|operator| format!("{operator} {version}"))
    });
    Ok(match constraint {
        Some(constraint) => {
            RepositoryRequirementClause::versioned(dependency.name().to_string(), constraint)
        }
        None => RepositoryRequirementClause::name_only(dependency.name().to_string()),
    })
}

fn dependency_relation_text(dependency: &Dep) -> Result<String> {
    match (dependency.depmod(), dependency.version()) {
        (DepMod::Any, None) => Ok(dependency.name().to_string()),
        (DepMod::Any, Some(_)) | (_, None) => Err(Error::ConflictError(format!(
            "libalpm dependency fields disagree for '{}'",
            dependency.name()
        ))),
        (mode, Some(version)) => Ok(format!(
            "{}{}{version}",
            dependency.name(),
            dependency_operator(mode).expect("non-Any dependency mode has one operator")
        )),
    }
}

fn dependency_operator(mode: DepMod) -> Option<&'static str> {
    match mode {
        DepMod::Any => None,
        DepMod::Eq => Some("="),
        DepMod::Ge => Some(">="),
        DepMod::Le => Some("<="),
        DepMod::Gt => Some(">"),
        DepMod::Lt => Some("<"),
    }
}

fn arch_database_object(snapshot: &SourceSnapshotV1) -> Result<&SourceMetadataObjectV1> {
    snapshot
        .authenticated_objects
        .iter()
        .find(|object| object.role == SourceMetadataObjectRoleV1::ArchDatabase)
        .ok_or_else(|| {
            Error::ConfigError(format!(
                "ALPM source snapshot '{}' has no authenticated Arch database",
                snapshot.repository_identity
            ))
        })
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

fn required_text<'a>(value: Option<&'a str>, package: &str, field: &str) -> Result<&'a str> {
    value
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::ParseError(format!("ALPM package '{package}' is missing {field}")))
}

fn database_name(ordinal: usize) -> Result<String> {
    let ordinal = u32::try_from(ordinal)
        .map_err(|_| Error::ConfigError("ALPM member ordinal exceeds u32".to_string()))?;
    Ok(format!("member-{ordinal:08}"))
}

fn path_text(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| Error::InvalidPath(format!("path is not UTF-8: {}", path.display())))
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
        .ok_or_else(|| Error::InternalError(format!("ALPM {label} exceed u64")))
}

#[cfg(test)]
mod tests;
