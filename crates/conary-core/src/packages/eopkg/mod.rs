// conary-core/src/packages/eopkg/mod.rs

//! Solus eopkg/PISI 1.2 package support.

pub mod authority;
mod inventory;
mod payload;
pub mod query;
pub mod takeover;
pub(crate) mod xml;

use crate::ccs::CCS_BUDGET;
use crate::db::models::{Trove, TroveType};
use crate::error::{Error, Result};
use crate::packages::config_authority::ConfigPayloadAssociation;
use crate::packages::payload::PackagePayload;
use crate::packages::source_authority::SourcePackageAuthority;
use crate::packages::traits::{PackageFile, PackageFormat};
use crate::repository::dependency_model::{
    ProvideArchitectureQualifier, RepositoryCapabilityKind, RepositoryRequirementClause,
    RepositoryRequirementGroup, RepositoryRequirementKind,
};
use crate::repository::versioning::{EopkgVersionConstraint, VersionScheme};
use authority::{EopkgConfigDeclaration, EopkgDeclaredCapability, EopkgPackageAuthority};
use std::fs::File;
use std::io::{Read, Seek, Write};

pub struct EopkgPackage {
    authority: EopkgPackageAuthority,
    description: Option<String>,
    files: Vec<PackageFile>,
    requirements: Vec<RepositoryRequirementGroup>,
    relations: Vec<RepositoryRequirementGroup>,
    payload: PackagePayload,
    parse_metrics: crate::packages::NativePackageParseMetrics,
}

pub(crate) fn has_package_contract(path: &std::path::Path) -> Result<bool> {
    let bounds = CCS_BUDGET.archive_decode_bounds()?;
    let file = File::open(path)?;
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return Ok(false);
    };
    if archive.len() != 3 {
        return Ok(false);
    }
    let mut names = Vec::with_capacity(3);
    for index in 0..archive.len() {
        names.push(
            archive
                .by_index(index)
                .map_err(|error| Error::ParseError(error.to_string()))?
                .name()
                .to_string(),
        );
    }
    names.sort();
    if names != ["files.xml", "install.tar.xz", "metadata.xml"] {
        return Ok(false);
    }
    let metadata = read_zip_member(&mut archive, "metadata.xml", bounds.max_metadata_bytes)?;
    Ok(xml::parse_metadata(&metadata).is_ok())
}

fn read_zip_member<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Result<Vec<u8>> {
    let mut member = archive
        .by_name(name)
        .map_err(|error| Error::ParseError(format!("eopkg lacks exact {name} member: {error}")))?;
    if member.size() > limit {
        return Err(Error::ParseError(format!(
            "eopkg {name} is {} bytes; maximum is {limit}",
            member.size()
        )));
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(member.size())
            .map_err(|_| Error::ParseError(format!("eopkg {name} exceeds addressable memory")))?,
    );
    member
        .by_ref()
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != member.size() {
        return Err(Error::ParseError(format!(
            "eopkg {name} declared size disagrees with decoded bytes"
        )));
    }
    Ok(bytes)
}

fn relation_constraint(record: &xml::RelationRecord) -> Result<Option<String>> {
    let constraint = EopkgVersionConstraint {
        version: record.version.clone(),
        version_from: record.version_from.clone(),
        version_to: record.version_to.clone(),
        release: record.release,
        release_from: record.release_from,
        release_to: record.release_to,
    };
    if constraint.version.is_none()
        && constraint.version_from.is_none()
        && constraint.version_to.is_none()
        && constraint.release.is_none()
        && constraint.release_from.is_none()
        && constraint.release_to.is_none()
    {
        return Ok(None);
    }
    constraint
        .validate()
        .map_err(|error| Error::ParseError(error.to_string()))?;
    Ok(Some(constraint.to_string()))
}

fn relation_clause(record: &xml::RelationRecord) -> Result<RepositoryRequirementClause> {
    Ok(RepositoryRequirementClause {
        name: record.name.clone(),
        capability_kind: None,
        version_constraint: relation_constraint(record)?,
        architecture_qualifier: Default::default(),
        native_text: Some(record.name.clone()),
    })
}

pub(crate) fn relation_groups(
    records: Vec<Vec<xml::RelationRecord>>,
    kind: RepositoryRequirementKind,
) -> Result<Vec<RepositoryRequirementGroup>> {
    records
        .into_iter()
        .map(|group| {
            let clauses = group
                .iter()
                .map(relation_clause)
                .collect::<Result<Vec<_>>>()?;
            if clauses.len() == 1 {
                Ok(RepositoryRequirementGroup::simple(
                    kind,
                    clauses.into_iter().next().unwrap(),
                ))
            } else {
                Ok(RepositoryRequirementGroup::alternatives(kind, clauses))
            }
        })
        .collect()
}

impl PackageFormat for EopkgPackage {
    fn parse(path: &str) -> Result<Self> {
        let bounds = CCS_BUDGET.archive_decode_bounds()?;
        let source_bytes = crate::packages::parse_metrics::ReadCounter::default();
        let mut parse_metrics = crate::packages::NativePackageParseMetrics {
            source_archive_opens: 1,
            archive_passes: 1,
            ..Default::default()
        };
        let file = File::open(path)?;
        let mut archive = zip::ZipArchive::new(source_bytes.wrap(file))
            .map_err(|error| Error::ParseError(format!("invalid eopkg ZIP contract: {error}")))?;
        if archive.len() != 3 {
            return Err(Error::ParseError(format!(
                "eopkg 1.2 requires exactly metadata.xml, files.xml, and install.tar.xz; found {} members",
                archive.len()
            )));
        }
        for index in 0..archive.len() {
            let member = archive
                .by_index(index)
                .map_err(|error| Error::ParseError(error.to_string()))?;
            if !matches!(
                member.name(),
                "metadata.xml" | "files.xml" | "install.tar.xz"
            ) {
                return Err(Error::ParseError(format!(
                    "unexpected eopkg member {}",
                    member.name()
                )));
            }
        }
        parse_metrics.checked_add(crate::packages::NativePackageParseMetrics {
            archive_entries_traversed: u64::try_from(archive.len())
                .map_err(|_| Error::ParseError("eopkg member count exceeds u64".to_string()))?,
            ..Default::default()
        })?;
        let metadata_bytes =
            read_zip_member(&mut archive, "metadata.xml", bounds.max_metadata_bytes)?;
        let files_bytes = read_zip_member(&mut archive, "files.xml", bounds.max_metadata_bytes)?;
        parse_metrics.checked_add(crate::packages::NativePackageParseMetrics {
            archive_entries_traversed: 2,
            decompressed_archive_bytes_read: u64::try_from(metadata_bytes.len())
                .map_err(|_| Error::ParseError("eopkg metadata size exceeds u64".to_string()))?
                .checked_add(u64::try_from(files_bytes.len()).map_err(|_| {
                    Error::ParseError("eopkg files metadata size exceeds u64".to_string())
                })?)
                .ok_or_else(|| {
                    Error::ParseError("eopkg decoded metadata size overflow".to_string())
                })?,
            ..Default::default()
        })?;
        bounds.admit_metadata_bytes(
            "eopkg XML metadata",
            (metadata_bytes.len() + files_bytes.len()) as u64,
        )?;
        let metadata = xml::parse_metadata(&metadata_bytes)?;
        let file_records = xml::parse_files(&files_bytes)?;

        let mut inner = tempfile::NamedTempFile::new()?;
        let inner_bytes;
        {
            let mut member = archive
                .by_name("install.tar.xz")
                .map_err(|error| Error::ParseError(error.to_string()))?;
            if member.size() > bounds.max_total_payload_bytes {
                return Err(Error::ParseError(format!(
                    "eopkg compressed payload exceeds {} bytes",
                    bounds.max_total_payload_bytes
                )));
            }
            inner_bytes = std::io::copy(&mut member, &mut inner)?;
            if inner_bytes != member.size() {
                return Err(Error::ParseError(
                    "eopkg install.tar.xz changed size while being staged".to_string(),
                ));
            }
            inner.flush()?;
        }
        parse_metrics.checked_add(crate::packages::NativePackageParseMetrics {
            archive_entries_traversed: 1,
            decompressed_archive_bytes_read: inner_bytes,
            intermediate_archive_bytes_written: inner_bytes,
            ..Default::default()
        })?;
        parse_metrics.source_archive_bytes_read = source_bytes.bytes();
        let payload = payload::parse(inner.path(), &file_records, &bounds, &mut parse_metrics)?;
        let files = payload
            .files()
            .iter()
            .map(|file| PackageFile {
                path: file.path.clone(),
                node: file.node.clone(),
                content: file.content_authority.clone(),
            })
            .collect::<Vec<_>>();

        let config = file_records
            .iter()
            .enumerate()
            .filter(|(_, file)| file.file_type == "config")
            .map(|(index, file)| {
                Ok(EopkgConfigDeclaration {
                    files_index: u32::try_from(index).map_err(|_| {
                        Error::ParseError("eopkg files index exceeds u32".to_string())
                    })?,
                    path: crate::packages::archive_utils::normalize_path(&file.path)
                        .map_err(|error| Error::ParseError(error.to_string()))?,
                    permanent: file.permanent,
                    payload: ConfigPayloadAssociation::Matched,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut provides = Vec::new();
        for (kind, values) in [
            (RepositoryCapabilityKind::PkgConfig, &metadata.pkg_config),
            (
                RepositoryCapabilityKind::PkgConfig32,
                &metadata.pkg_config_32,
            ),
            (RepositoryCapabilityKind::Comar, &metadata.comar),
        ] {
            for value in values {
                provides.push(EopkgDeclaredCapability {
                    metadata_index: u32::try_from(provides.len()).map_err(|_| {
                        Error::ParseError("eopkg provide index exceeds u32".to_string())
                    })?,
                    kind,
                    name: value.clone(),
                    version: None,
                    version_relation: None,
                    architecture_qualifier: ProvideArchitectureQualifier::Implicit,
                });
            }
        }
        let authority = EopkgPackageAuthority {
            name: metadata.name,
            version: format!("{}-{}", metadata.version, metadata.release),
            release: metadata.release,
            distribution: metadata.distribution,
            distribution_release: metadata.distribution_release,
            architecture: metadata.architecture,
            package_format: metadata.package_format,
            provides,
            config,
        };
        crate::repository::versioning::validate_repo_version(
            VersionScheme::Eopkg,
            &authority.version,
        )
        .map_err(|error| Error::ParseError(error.to_string()))?;
        let requirements =
            relation_groups(metadata.dependencies, RepositoryRequirementKind::Depends)?;
        let mut relations = relation_groups(
            metadata
                .conflicts
                .into_iter()
                .map(|record| vec![record])
                .collect(),
            RepositoryRequirementKind::Conflict,
        )?;
        relations.extend(relation_groups(
            metadata
                .replaces
                .into_iter()
                .map(|record| vec![record])
                .collect(),
            RepositoryRequirementKind::Replace,
        )?);
        Ok(Self {
            authority,
            description: metadata.description.or(metadata.summary),
            files,
            requirements,
            relations,
            payload,
            parse_metrics,
        })
    }

    fn name(&self) -> &str {
        &self.authority.name
    }
    fn version(&self) -> &str {
        &self.authority.version
    }
    fn version_scheme(&self) -> VersionScheme {
        VersionScheme::Eopkg
    }
    fn architecture(&self) -> Option<&str> {
        Some(&self.authority.architecture)
    }
    fn source_authority(&self) -> Result<SourcePackageAuthority> {
        Ok(SourcePackageAuthority::Eopkg(self.authority.clone()))
    }
    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
    fn files(&self) -> &[PackageFile] {
        &self.files
    }
    fn requirements(&self) -> &[RepositoryRequirementGroup] {
        &self.requirements
    }
    fn relations(&self) -> &[RepositoryRequirementGroup] {
        &self.relations
    }
    fn resolution_capabilities(
        &self,
    ) -> Result<Vec<crate::repository::dependency_model::ProvidedCapability>> {
        self.source_authority()?.resolution_capabilities()
    }
    fn package_payload(&self) -> Result<PackagePayload> {
        Ok(self.payload.clone())
    }
    fn to_trove(&self) -> Trove {
        let mut trove = Trove::new(
            self.name().to_string(),
            self.version().to_string(),
            TroveType::Package,
            VersionScheme::Eopkg,
        );
        trove.architecture = self.architecture().map(str::to_string);
        trove.description = self.description.clone();
        trove.native_package_identity = crate::packages::InstalledPackageIdentity::eopkg(
            self.authority.name.clone(),
            self.authority.name.clone(),
            self.authority
                .version
                .strip_suffix(&format!("-{}", self.authority.release))
                .unwrap_or(&self.authority.version),
            self.authority.release,
            self.authority.architecture.clone(),
        )
        .ok();
        trove
    }
}

impl EopkgPackage {
    #[must_use]
    pub fn parse_metrics(&self) -> crate::packages::NativePackageParseMetrics {
        self.parse_metrics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;

    fn package(extra_member: bool) -> tempfile::NamedTempFile {
        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_path("usr/bin/demo").unwrap();
        header.set_size(5);
        header.set_mode(0o755);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_cksum();
        tar.append(&header, Cursor::new(b"hello")).unwrap();
        let tar = tar.into_inner().unwrap();
        let stream =
            liblzma::stream::Stream::new_easy_encoder(6, liblzma::stream::Check::Crc64).unwrap();
        let mut encoder = liblzma::read::XzEncoder::new_stream(tar.as_slice(), stream);
        let mut compressed = Vec::new();
        encoder.read_to_end(&mut compressed).unwrap();

        let metadata = br#"<PISI><Package><Name>demo</Name><Summary>demo</Summary><RuntimeDependencies><Dependency releaseFrom="1">glibc</Dependency></RuntimeDependencies><History><Update release="2"><Version>1.0</Version></Update></History><Distribution>Solus</Distribution><DistributionRelease>1</DistributionRelease><Architecture>x86_64</Architecture><PackageFormat>1.2</PackageFormat></Package></PISI>"#;
        let files = br#"<Files><File><Path>usr/bin/demo</Path><Type>executable</Type><Size>5</Size><Uid>0</Uid><Gid>0</Gid><Mode>0755</Mode><Hash>aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d</Hash></File></Files>"#;
        let output = tempfile::NamedTempFile::new().unwrap();
        {
            let mut zip = zip::ZipWriter::new(output.reopen().unwrap());
            for (name, bytes) in [
                ("metadata.xml", metadata.as_slice()),
                ("files.xml", files.as_slice()),
                ("install.tar.xz", compressed.as_slice()),
            ] {
                zip.start_file(name, SimpleFileOptions::default()).unwrap();
                zip.write_all(bytes).unwrap();
            }
            if extra_member {
                zip.start_file("unknown", SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(b"authority").unwrap();
            }
            zip.finish().unwrap();
        }
        output
    }

    #[test]
    fn exact_archive_contract_parses_payload_and_relations() {
        let fixture = package(false);
        let parsed = EopkgPackage::parse(fixture.path().to_str().unwrap()).unwrap();
        assert_eq!(parsed.name(), "demo");
        assert_eq!(parsed.version(), "1.0-2");
        assert_eq!(parsed.architecture(), Some("x86_64"));
        assert_eq!(parsed.files().len(), 1);
        assert_eq!(parsed.requirements().len(), 1);
        let mut content = Vec::new();
        parsed.package_payload().unwrap().files()[0]
            .open_content()
            .unwrap()
            .read_to_end(&mut content)
            .unwrap();
        assert_eq!(content, b"hello");
    }

    #[test]
    fn unknown_archive_member_fails_closed() {
        let fixture = package(true);
        let error = EopkgPackage::parse(fixture.path().to_str().unwrap())
            .err()
            .expect("unknown member must fail");
        assert!(error.to_string().contains("requires exactly"));
    }
}
