// conary-core/src/repository/parsers/fedora.rs

//! Fedora/RPM repository metadata parser
//!
//! Parses Fedora-style repomd.xml, primary.xml, and filelists.xml files which
//! contain RPM package metadata in XML format.
//! Generator-selected `<file>` records in primary.xml are package-owned file
//! provides. This follows createrepo_c's pinned primary metadata contract:
//! <https://github.com/rpm-software-management/createrepo_c/blob/5cf41fe5d703901d78078ed18c67ab667e446c1a/src/xml_dump.c#L175-L225>.
//! primary.xml carries only the paths that contract selects, so complete
//! package file ownership comes from the signed filelists.xml document
//! (`fedora/filelists.rs`), which is projected into the same typed file
//! capabilities.

use super::common::{self, MAX_PACKAGE_SIZE};
use super::{
    AuthenticatedMetadataObject, AuthenticatedMetadataObjectRole, AuthenticatedSnapshotIdentity,
    ChecksumType, PackageMetadata, RepositoryParser, RepositorySnapshotSink,
};
use crate::error::{Error, Result};
use crate::repository::catalog::{CatalogMetadataObjectScratchV1, CatalogMetadataScratchV1};
use crate::repository::client::RepositoryClient;
use crate::repository::dependency_model::{
    RepositoryDependencyFlavor, RepositoryRequirementGroup, RepositoryRequirementKind,
};
use crate::repository::package_relation::parse_native_relation;
use crate::repository::trust::openpgp::PreparedOpenPgpTrust;
use crate::repository::trust::{RepositoryTrustPolicy, RpmMetadataAuthority, TrustRole};
use crate::repository::versioning::VersionScheme;
use quick_xml::Reader;
use quick_xml::events::Event;
use serde_json::json;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

pub(in crate::repository) mod audit;
mod filelists;
mod files;
mod metalink;
mod provides;
mod relation;
mod repomd;
use files::FileRecordReader;
use metalink::parse_metalink_repomd_identity;
use relation::{
    RpmProvideConstraint, rpm_provide_constraint, rpm_relation_native_text, rpm_require_to_group,
};
use repomd::{RepoMdDocument, RepoMdIndex, RepoMdRecord};

fn authenticated_repomd_snapshot(bytes: &[u8]) -> AuthenticatedSnapshotIdentity {
    AuthenticatedSnapshotIdentity::for_bytes(bytes)
}

fn authenticated_metadata_scratch(repomd: &RepoMdIndex) -> Result<CatalogMetadataScratchV1> {
    let mut objects = vec![CatalogMetadataObjectScratchV1 {
        role: AuthenticatedMetadataObjectRole::RpmPrimary,
        source_path: repomd.primary.href.clone(),
        size: repomd.primary.size,
    }];
    if let Some(filelists) = &repomd.filelists {
        objects.push(CatalogMetadataObjectScratchV1 {
            role: AuthenticatedMetadataObjectRole::RpmFilelists,
            source_path: filelists.href.clone(),
            size: filelists.size,
        });
    }
    CatalogMetadataScratchV1::from_signed_objects(objects)
}

/// Fedora/RPM repository parser
pub struct FedoraParser {
    /// Repository architecture (e.g., "x86_64", "aarch64")
    architecture: String,
    trust: PreparedOpenPgpTrust,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FormatSection {
    Requires,
    Recommends,
    Suggests,
    Supplements,
    Enhances,
    Provides,
    Conflicts,
    Obsoletes,
}

impl FormatSection {
    const fn metadata_name(self) -> &'static str {
        match self {
            Self::Requires => "requires",
            Self::Recommends => "recommends",
            Self::Suggests => "suggests",
            Self::Supplements => "supplements",
            Self::Enhances => "enhances",
            Self::Provides => "provides",
            Self::Conflicts => "conflicts",
            Self::Obsoletes => "obsoletes",
        }
    }
}

/// Namespace-prefixed and unprefixed element names name the same element.
fn local_tag_name(tag_name: &str) -> &str {
    tag_name.rsplit(':').next().unwrap_or(tag_name)
}

/// Build one RPM version string from the exact epoch, version, and release a
/// signed document records.
///
/// primary.xml and filelists.xml both carry `<version epoch ver rel/>`, so one
/// owner keeps the corpus version and the filelists join comparable.
fn rpm_version_text(epoch: Option<&str>, ver: &str, rel: &str) -> String {
    let epoch = epoch.unwrap_or("0");
    if epoch == "0" {
        format!("{ver}-{rel}")
    } else {
        format!("{epoch}:{ver}-{rel}")
    }
}

impl FedoraParser {
    /// Create a new Fedora/RPM parser
    pub fn new(architecture: String, trust: PreparedOpenPgpTrust) -> Result<Self> {
        if !matches!(trust.policy(), RepositoryTrustPolicy::Rpm { .. }) {
            return Err(Error::ConfigError(
                "RPM parser requires an RPM repository trust policy".to_string(),
            ));
        }
        Ok(Self {
            architecture,
            trust,
        })
    }

    /// Download repomd.xml and admit the metadata records Conary reads.
    ///
    /// Uses RepositoryClient for HTTP.
    async fn fetch_repomd_index(
        &self,
        repo_url: &str,
    ) -> Result<(RepoMdIndex, AuthenticatedSnapshotIdentity)> {
        let repomd_url = format!("{}/repodata/repomd.xml", repo_url.trim_end_matches('/'));
        debug!("Downloading repomd.xml from: {}", repomd_url);

        let client = RepositoryClient::new()?;
        let xml_bytes = client.download_to_bytes(&repomd_url).await?;
        let RepositoryTrustPolicy::Rpm { metadata, .. } = self.trust.policy() else {
            return Err(Error::ConfigError(
                "RPM parser lost its RPM trust policy".to_string(),
            ));
        };
        match metadata {
            RpmMetadataAuthority::OpenPgp { .. } => {
                let signature_url = format!("{repomd_url}.asc");
                let signature =
                    client
                        .download_to_bytes(&signature_url)
                        .await
                        .map_err(|error| {
                            Error::GpgVerificationFailed(format!(
                                "RPM repository metadata signature {signature_url} is required: \
                                 {error}"
                            ))
                        })?;
                self.trust
                    .verify_detached(TrustRole::RpmMetadata, &xml_bytes, &signature)?;
            }
            RpmMetadataAuthority::Metalink { url } => {
                let metalink = client.download_to_bytes(url).await?;
                let identity = parse_metalink_repomd_identity(&metalink)?;
                identity.verify(&xml_bytes)?;
            }
        }
        let snapshot = authenticated_repomd_snapshot(&xml_bytes);
        let xml_content = String::from_utf8(xml_bytes)
            .map_err(|e| Error::ParseError(format!("Invalid UTF-8 in repomd.xml: {}", e)))?;

        // Parse repomd.xml into the records Conary reads.
        Ok((repomd::parse_repomd(&xml_content)?, snapshot))
    }

    /// Download one metadata document and verify it against its signed record.
    ///
    /// The served (still compressed) bytes are what the signed repomd.xml
    /// records, so identity is established before a decoder ever sees them.
    async fn download_verified_document(
        &self,
        repo_url: &str,
        record: &RepoMdRecord,
        work_directory: &Path,
        file_name: &str,
    ) -> Result<(
        String,
        PathBuf,
        crate::repository::client::DownloadedFileIdentity,
    )> {
        let document_url = format!("{}/{}", repo_url.trim_end_matches('/'), record.href);
        debug!(
            "Downloading {} metadata from: {}",
            record.document.label(),
            document_url
        );

        let client = RepositoryClient::new()?;
        let path = work_directory.join(file_name);
        let identity = client
            .download_file_with_identity_limit(&document_url, &path, record.size)
            .await?;
        record.verify_served_download(&identity)?;
        Ok((document_url, path, identity))
    }

    /// Download and verify primary.xml without materializing its contents.
    async fn download_primary_xml(
        &self,
        repo_url: &str,
        record: &RepoMdRecord,
        work_directory: &Path,
    ) -> Result<(String, PathBuf, AuthenticatedMetadataObject)> {
        let (primary_url, path, identity) = self
            .download_verified_document(repo_url, record, work_directory, "rpm-primary")
            .await?;
        let authenticated_object = AuthenticatedMetadataObject {
            role: AuthenticatedMetadataObjectRole::RpmPrimary,
            source_path: record.href.clone(),
            sha256: identity.sha256,
            size: identity.size,
        };
        Ok((primary_url, path, authenticated_object))
    }

    /// Parse primary.xml one complete package record at a time.
    fn parse_primary_reader<R: BufRead>(
        &self,
        document: R,
        base_url: &str,
        mut admit: impl FnMut(PackageMetadata) -> Result<()>,
    ) -> Result<usize> {
        let mut reader = Reader::from_reader(document);
        reader.config_mut().trim_text_end = true;

        let mut packages = 0_usize;
        let mut buf = Vec::new();

        // Current package being built
        let mut current_package: Option<PackageBuilder> = None;
        let mut current_tag = String::new();
        let mut in_format = false;
        let mut format_section = None;
        let mut file_record = FileRecordReader::new(RepoMdDocument::Primary);

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    let local_tag = local_tag_name(&tag_name);
                    file_record.reject_nested_element()?;
                    current_tag = tag_name.clone();

                    match local_tag {
                        "package" => {
                            current_package = Some(PackageBuilder::new());
                        }
                        "format" => in_format = true,
                        "requires" if in_format => {
                            format_section = Some(FormatSection::Requires);
                        }
                        "recommends" if in_format => {
                            format_section = Some(FormatSection::Recommends);
                        }
                        "suggests" if in_format => {
                            format_section = Some(FormatSection::Suggests);
                        }
                        "supplements" if in_format => {
                            format_section = Some(FormatSection::Supplements);
                        }
                        "enhances" if in_format => {
                            format_section = Some(FormatSection::Enhances);
                        }
                        "provides" if in_format => {
                            format_section = Some(FormatSection::Provides);
                        }
                        "conflicts" if in_format => {
                            format_section = Some(FormatSection::Conflicts);
                        }
                        "obsoletes" if in_format => {
                            format_section = Some(FormatSection::Obsoletes);
                        }
                        "file" if in_format => {
                            if current_package.is_none() {
                                return Err(Error::ParseError(
                                    "RPM primary file record appears outside a package".to_string(),
                                ));
                            }
                            file_record.open(&mut reader);
                        }
                        "checksum" => {
                            if let Some(ref mut pkg) = current_package {
                                for attr in e.attributes() {
                                    let attr = attr.map_err(|error| {
                                        Error::ParseError(format!(
                                            "Failed to parse RPM checksum attribute: {error}"
                                        ))
                                    })?;
                                    if attr.key.as_ref() == b"type" {
                                        pkg.checksum_type = Some(
                                            attr.decoded_and_normalized_value(
                                                quick_xml::XmlVersion::Implicit1_0,
                                                reader.decoder(),
                                            )
                                            .map_err(|error| {
                                                Error::ParseError(format!(
                                                    "Failed to decode RPM checksum attribute: \
                                                     {error}"
                                                ))
                                            })?
                                            .into_owned(),
                                        );
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::Empty(e)) => {
                    let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    let local_tag = local_tag_name(&tag_name);
                    file_record.reject_nested_element()?;

                    match local_tag {
                        "version" => {
                            if let Some(ref mut pkg) = current_package {
                                // Extract epoch, ver, rel attributes
                                for attr in e.attributes() {
                                    let attr = attr.map_err(|error| {
                                        Error::ParseError(format!(
                                            "Failed to parse RPM version attribute: {error}"
                                        ))
                                    })?;
                                    let key = String::from_utf8_lossy(attr.key.as_ref());
                                    let value = attr
                                        .decoded_and_normalized_value(
                                            quick_xml::XmlVersion::Implicit1_0,
                                            reader.decoder(),
                                        )
                                        .map_err(|error| {
                                            Error::ParseError(format!(
                                                "Failed to decode RPM version attribute: {error}"
                                            ))
                                        })?;
                                    match key.as_ref() {
                                        "epoch" => pkg.epoch = Some(value.to_string()),
                                        "ver" => pkg.ver = Some(value.to_string()),
                                        "rel" => pkg.rel = Some(value.to_string()),
                                        _ => {}
                                    }
                                }
                            }
                        }
                        "checksum" => {
                            if let Some(ref mut pkg) = current_package {
                                for attr in e.attributes() {
                                    let attr = attr.map_err(|error| {
                                        Error::ParseError(format!(
                                            "Failed to parse RPM checksum attribute: {error}"
                                        ))
                                    })?;
                                    let key = String::from_utf8_lossy(attr.key.as_ref());
                                    if key == "type" {
                                        let value = attr
                                            .decoded_and_normalized_value(
                                                quick_xml::XmlVersion::Implicit1_0,
                                                reader.decoder(),
                                            )
                                            .map_err(|error| {
                                                Error::ParseError(format!(
                                                    "Failed to decode RPM checksum attribute: \
                                                     {error}"
                                                ))
                                            })?;
                                        pkg.checksum_type = Some(value.to_string());
                                    }
                                }
                            }
                        }
                        "size" => {
                            if let Some(ref mut pkg) = current_package {
                                for attr in e.attributes() {
                                    let attr = attr.map_err(|error| {
                                        Error::ParseError(format!(
                                            "Failed to parse RPM size attribute: {error}"
                                        ))
                                    })?;
                                    let key = String::from_utf8_lossy(attr.key.as_ref());
                                    if key == "package" {
                                        let value = attr
                                            .decoded_and_normalized_value(
                                                quick_xml::XmlVersion::Implicit1_0,
                                                reader.decoder(),
                                            )
                                            .map_err(|error| {
                                                Error::ParseError(format!(
                                                    "Failed to decode RPM size attribute: {error}"
                                                ))
                                            })?;
                                        pkg.size = Some(value.to_string());
                                    }
                                }
                            }
                        }
                        "location" => {
                            if let Some(ref mut pkg) = current_package {
                                for attr in e.attributes() {
                                    let attr = attr.map_err(|error| {
                                        Error::ParseError(format!(
                                            "Failed to parse RPM location attribute: {error}"
                                        ))
                                    })?;
                                    let key = String::from_utf8_lossy(attr.key.as_ref());
                                    if key == "href" {
                                        let value = attr
                                            .decoded_and_normalized_value(
                                                quick_xml::XmlVersion::Implicit1_0,
                                                reader.decoder(),
                                            )
                                            .map_err(|error| {
                                                Error::ParseError(format!(
                                                    "Failed to decode RPM location attribute: \
                                                     {error}"
                                                ))
                                            })?;
                                        pkg.location = Some(value.to_string());
                                    }
                                }
                            }
                        }
                        "format" => in_format = true,
                        "entry" if in_format => {
                            if let Some(ref mut pkg) = current_package {
                                let mut dep_name = None;
                                let mut dep_flags = None;
                                let mut dep_epoch = None;
                                let mut dep_ver = None;
                                let mut dep_rel = None;
                                let mut dep_pre = None;

                                for attr in e.attributes() {
                                    let attr = attr.map_err(|error| {
                                        Error::ParseError(format!(
                                            "Failed to parse RPM dependency attribute: {error}"
                                        ))
                                    })?;
                                    let key = String::from_utf8_lossy(attr.key.as_ref());
                                    let value = attr
                                        .decoded_and_normalized_value(
                                            quick_xml::XmlVersion::Implicit1_0,
                                            reader.decoder(),
                                        )
                                        .map_err(|error| {
                                            Error::ParseError(format!(
                                                "Failed to decode RPM dependency attribute: {error}"
                                            ))
                                        })?;
                                    match key.as_ref() {
                                        "name" => dep_name = Some(value.to_string()),
                                        "flags" => dep_flags = Some(value.to_string()),
                                        "epoch" => dep_epoch = Some(value.to_string()),
                                        "ver" => dep_ver = Some(value.to_string()),
                                        "rel" => dep_rel = Some(value.to_string()),
                                        "pre" => dep_pre = Some(value.to_string()),
                                        _ => {}
                                    }
                                }

                                if let Some(section) = format_section {
                                    let name = dep_name.ok_or_else(|| {
                                        Error::ParseError(format!(
                                            "RPM primary metadata {} entry is missing its required name attribute",
                                            section.metadata_name()
                                        ))
                                    })?;
                                    match section {
                                        FormatSection::Requires => {
                                            if let Some(requirement) = rpm_require_to_group(
                                                &name,
                                                dep_flags.as_deref(),
                                                dep_epoch.as_deref(),
                                                dep_ver.as_deref(),
                                                dep_rel.as_deref(),
                                                dep_pre.as_deref(),
                                            )? {
                                                pkg.dependencies.push(requirement);
                                            }
                                        }
                                        FormatSection::Recommends
                                        | FormatSection::Suggests
                                        | FormatSection::Supplements
                                        | FormatSection::Enhances => {
                                            if dep_pre.is_some() {
                                                return Err(Error::ParseError(format!(
                                                    "RPM primary pre marker is invalid on a {} entry",
                                                    section.metadata_name()
                                                )));
                                            }
                                            let kind = match section {
                                                FormatSection::Recommends => {
                                                    RepositoryRequirementKind::Recommends
                                                }
                                                FormatSection::Suggests => {
                                                    RepositoryRequirementKind::Suggests
                                                }
                                                FormatSection::Supplements => {
                                                    RepositoryRequirementKind::Supplements
                                                }
                                                FormatSection::Enhances => {
                                                    RepositoryRequirementKind::Enhances
                                                }
                                                _ => unreachable!("weak RPM section selected"),
                                            };
                                            pkg.dependencies.push(
                                                relation::rpm_weak_require_to_group(
                                                    kind,
                                                    &name,
                                                    dep_flags.as_deref(),
                                                    dep_epoch.as_deref(),
                                                    dep_ver.as_deref(),
                                                    dep_rel.as_deref(),
                                                )?,
                                            );
                                        }
                                        FormatSection::Provides => {
                                            if dep_pre.is_some() {
                                                return Err(Error::ParseError(
                                                    "RPM primary pre marker is valid only on a requires entry"
                                                        .to_string(),
                                                ));
                                            }
                                            let provide = rpm_provide_constraint(
                                                &name,
                                                dep_flags.as_deref(),
                                                dep_epoch.as_deref(),
                                                dep_ver.as_deref(),
                                                dep_rel.as_deref(),
                                            )?;
                                            pkg.provides.push((name, provide));
                                        }
                                        FormatSection::Conflicts => {
                                            if dep_pre.is_some() {
                                                return Err(Error::ParseError(
                                                    "RPM primary pre marker is valid only on a requires entry"
                                                        .to_string(),
                                                ));
                                            }
                                            pkg.relations.push((
                                                RepositoryRequirementKind::Conflict,
                                                rpm_relation_native_text(
                                                    &name,
                                                    dep_flags.as_deref(),
                                                    dep_epoch.as_deref(),
                                                    dep_ver.as_deref(),
                                                    dep_rel.as_deref(),
                                                )?,
                                            ));
                                        }
                                        FormatSection::Obsoletes => {
                                            if dep_pre.is_some() {
                                                return Err(Error::ParseError(
                                                    "RPM primary pre marker is valid only on a requires entry"
                                                        .to_string(),
                                                ));
                                            }
                                            pkg.relations.push((
                                                RepositoryRequirementKind::Obsolete,
                                                rpm_relation_native_text(
                                                    &name,
                                                    dep_flags.as_deref(),
                                                    dep_epoch.as_deref(),
                                                    dep_ver.as_deref(),
                                                    dep_rel.as_deref(),
                                                )?,
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                        "file" if in_format => {
                            return Err(file_record.empty_record_error());
                        }
                        _ => {}
                    }
                }
                Ok(Event::Text(e)) => {
                    let decoded =
                        e.xml_content(quick_xml::XmlVersion::Implicit1_0)
                            .map_err(|error| {
                                Error::ParseError(format!(
                                    "Failed to decode primary.xml text: {error}"
                                ))
                            })?;
                    let text = quick_xml::escape::unescape(&decoded)
                        .map_err(|error| {
                            Error::ParseError(format!(
                                "Failed to unescape primary.xml text: {error}"
                            ))
                        })?
                        .into_owned();
                    // Skip inter-element whitespace that quick_xml emits as
                    // text events -- without this guard, trailing whitespace
                    // between tags overwrites fields like pkg.name with "".
                    if text.is_empty() {
                        continue;
                    }
                    if !file_record.push_text(&text)
                        && let Some(ref mut pkg) = current_package
                    {
                        append_package_text(pkg, &current_tag, &text);
                    }
                }
                Ok(Event::GeneralRef(reference)) => {
                    let text = match reference.resolve_char_ref().map_err(|error| {
                        Error::ParseError(format!(
                            "Failed to resolve primary.xml character reference: {error}"
                        ))
                    })? {
                        Some(character) => character.to_string(),
                        None => {
                            let entity = reference.decode().map_err(|error| {
                                Error::ParseError(format!(
                                    "Failed to decode primary.xml entity reference: {error}"
                                ))
                            })?;
                            quick_xml::escape::resolve_xml_entity(&entity)
                                .ok_or_else(|| {
                                    Error::ParseError(format!(
                                        "primary.xml uses undeclared entity '&{entity};'"
                                    ))
                                })?
                                .to_string()
                        }
                    };
                    if !file_record.push_text(&text)
                        && let Some(ref mut pkg) = current_package
                    {
                        append_package_text(pkg, &current_tag, &text);
                    }
                }
                Ok(Event::CData(e)) => {
                    let text =
                        e.xml_content(quick_xml::XmlVersion::Implicit1_0)
                            .map_err(|error| {
                                Error::ParseError(format!(
                                    "Failed to decode primary.xml CDATA: {error}"
                                ))
                            })?;
                    if !file_record.push_text(&text)
                        && let Some(ref mut pkg) = current_package
                    {
                        append_package_text(pkg, &current_tag, &text);
                    }
                }
                Ok(Event::End(e)) => {
                    let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    let local_tag = local_tag_name(&tag_name);
                    if local_tag == "file" && in_format {
                        let file = file_record.close(&mut reader)?;
                        current_package
                            .as_mut()
                            .ok_or_else(|| {
                                Error::ParseError(
                                    "RPM primary file record appears outside a package".to_string(),
                                )
                            })?
                            .primary_files
                            .push(file);
                    } else if local_tag == "package" {
                        let builder = current_package.take().ok_or_else(|| {
                            Error::ParseError(
                                "RPM metadata ended a package that was never started".to_string(),
                            )
                        })?;
                        admit(builder.build(base_url)?)?;
                        packages += 1;
                    } else if local_tag == "format" {
                        in_format = false;
                        format_section = None;
                    } else if matches!(
                        local_tag,
                        "requires"
                            | "recommends"
                            | "suggests"
                            | "supplements"
                            | "enhances"
                            | "provides"
                            | "conflicts"
                            | "obsoletes"
                    ) {
                        format_section = None;
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => {
                    return Err(Error::ParseError(format!(
                        "Failed to parse primary.xml: {}",
                        e
                    )));
                }
                _ => {}
            }
            buf.clear();
        }

        Ok(packages)
    }

    #[cfg(test)]
    fn parse_primary_xml(&self, xml_content: &str, base_url: &str) -> Result<Vec<PackageMetadata>> {
        let mut packages = Vec::new();
        self.parse_primary_reader(xml_content.as_bytes(), base_url, |package| {
            packages.push(package);
            Ok(())
        })?;
        Ok(packages)
    }
}

pub(super) fn validate_sha256(value: &str, label: &str) -> Result<()> {
    if value.len() != 64
        || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        || value.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return Err(Error::ParseError(format!(
            "{label} SHA256 must be exactly 64 lowercase hexadecimal digits"
        )));
    }
    Ok(())
}

/// Builder for constructing PackageMetadata from XML parsing
#[derive(Default)]
struct PackageBuilder {
    name: Option<String>,
    epoch: Option<String>,
    ver: Option<String>,
    rel: Option<String>,
    arch: Option<String>,
    summary: Option<String>,
    description: Option<String>,
    checksum: Option<String>,
    checksum_type: Option<String>,
    size: Option<String>,
    location: Option<String>,
    url: Option<String>,
    dependencies: Vec<RepositoryRequirementGroup>,
    provides: Vec<(String, RpmProvideConstraint)>,
    primary_files: Vec<String>,
    relations: Vec<(RepositoryRequirementKind, String)>,
}

impl PackageBuilder {
    fn new() -> Self {
        Self::default()
    }

    fn build(self, base_url: &str) -> Result<PackageMetadata> {
        let name = self
            .name
            .ok_or_else(|| Error::ParseError("Missing package name".to_string()))?;

        // Build version string: epoch:ver-rel
        let epoch = self.epoch.unwrap_or_else(|| "0".to_string());
        let ver = self
            .ver
            .ok_or_else(|| Error::ParseError("Missing version".to_string()))?;
        let rel = self
            .rel
            .ok_or_else(|| Error::ParseError("Missing release".to_string()))?;
        let version = rpm_version_text(Some(&epoch), &ver, &rel);

        let checksum = self
            .checksum
            .ok_or_else(|| Error::ParseError("Missing checksum".to_string()))?;
        validate_sha256(&checksum, "RPM package")?;

        let size: u64 = self
            .size
            .ok_or_else(|| Error::ParseError("Missing size".to_string()))?
            .parse()
            .map_err(|e| Error::ParseError(format!("Invalid size: {}", e)))?;

        if size > MAX_PACKAGE_SIZE {
            return Err(Error::ParseError(format!(
                "Package size {} exceeds maximum allowed (5GB)",
                size
            )));
        }

        let location = self
            .location
            .ok_or_else(|| Error::ParseError("Missing location".to_string()))?;

        if let Err(msg) = common::validate_filename(&location) {
            return Err(Error::ParseError(msg));
        }

        let download_url = common::join_repo_url(base_url, &location);

        let checksum_type = match self.checksum_type.as_deref() {
            Some("sha256") => ChecksumType::Sha256,
            other => {
                return Err(Error::ParseError(format!(
                    "RPM package checksum must declare sha256; found {other:?}"
                )));
            }
        };

        // Build structured requirements
        let mut requirements = self.dependencies;
        requirements.extend(
            self.relations
                .iter()
                .map(|(kind, native_text)| {
                    parse_native_relation(*kind, VersionScheme::Rpm, native_text)
                        .map_err(Error::ParseError)
                })
                .collect::<Result<Vec<_>>>()?,
        );

        let structured_provides = provides::project_repository_provides(
            &name,
            &version,
            &self.provides,
            &self.primary_files,
        )?;

        let rpm_provides: Vec<String> = self
            .provides
            .iter()
            .map(|(prov_name, provide)| {
                if provide.native_constraint.is_empty() {
                    prov_name.clone()
                } else {
                    format!("{prov_name} {}", provide.native_constraint)
                }
            })
            .collect();

        // Build extra metadata
        let mut extra = serde_json::Map::new();
        if let Some(url) = self.url {
            extra.insert("homepage".to_string(), serde_json::Value::String(url));
        }
        if let Some(summary) = self.summary {
            extra.insert("summary".to_string(), serde_json::Value::String(summary));
        }
        extra.insert(
            "format".to_string(),
            serde_json::Value::String("rpm".to_string()),
        );
        extra.insert("epoch".to_string(), serde_json::Value::String(epoch));
        extra.insert("rpm_provides".to_string(), json!(rpm_provides));

        Ok(PackageMetadata {
            name,
            version,
            architecture: self.arch,
            debian_multi_arch: None,
            description: self.description,
            checksum,
            checksum_type,
            size,
            download_url,
            extra_metadata: serde_json::Value::Object(extra),
            dependency_flavor: RepositoryDependencyFlavor::Rpm,
            version_scheme: VersionScheme::Rpm,
            requirements,
            provides: structured_provides,
        })
    }
}

fn append_package_text(package: &mut PackageBuilder, tag: &str, text: &str) {
    let field = match tag {
        "name" => Some(&mut package.name),
        "arch" => Some(&mut package.arch),
        "summary" => Some(&mut package.summary),
        "description" => Some(&mut package.description),
        "checksum" => Some(&mut package.checksum),
        "url" => Some(&mut package.url),
        _ => None,
    };
    if let Some(field) = field {
        match field {
            Some(existing) => existing.push_str(text),
            None => *field = Some(text.to_string()),
        }
    }
}

impl RepositoryParser for FedoraParser {
    async fn ingest_snapshot<S: RepositorySnapshotSink + Send>(
        &self,
        repo_url: &str,
        sink: &mut S,
    ) -> Result<AuthenticatedSnapshotIdentity> {
        info!("Syncing Fedora repository for {}", self.architecture);

        // Admit the signed repomd.xml records Conary reads.
        let (repomd, snapshot) = self.fetch_repomd_index(repo_url).await?;
        sink.reserve_authenticated_metadata(authenticated_metadata_scratch(&repomd)?)?;

        // Authenticate every distribution-sized child before cache lookup or parsing.
        let (primary_url, primary_path, primary_object) = self
            .download_primary_xml(repo_url, &repomd.primary, sink.work_directory())
            .await?;
        let filelists_download = if let Some(record) = repomd.filelists.as_ref() {
            let (url, path, identity) = self
                .download_verified_document(
                    repo_url,
                    record,
                    sink.work_directory(),
                    "rpm-filelists",
                )
                .await?;
            let object = AuthenticatedMetadataObject {
                role: AuthenticatedMetadataObjectRole::RpmFilelists,
                source_path: record.href.clone(),
                sha256: identity.sha256,
                size: identity.size,
            };
            let compressed = common::metadata_file_is_compressed(&path)?;
            let open_size = record.authenticated_open_size(compressed)?;
            Some((url, path, object, open_size))
        } else {
            None
        };
        let mut objects = vec![primary_object.clone()];
        if let Some((_, _, object, _)) = &filelists_download {
            objects.push(object.clone());
        }
        if sink.reuse_cached_projection(&snapshot, &objects)? {
            info!("Reused cached Fedora repository projection");
            return Ok(snapshot);
        }

        // Parse primary.xml one package at a time.
        let compressed = common::metadata_file_is_compressed(&primary_path)?;
        let open_size = repomd.primary.authenticated_open_size(compressed)?;
        let package_count = {
            let decoder = common::open_metadata_decoder(
                &primary_path,
                &format!("RPM primary metadata {primary_url}"),
            )?;
            let mut reader = std::io::BufReader::with_capacity(
                256 * 1024,
                common::AuthenticatedLengthReader::new(decoder, open_size, "RPM primary metadata"),
            );
            let package_count =
                self.parse_primary_reader(&mut reader, repo_url, |package| sink.package(package))?;
            let decoded = reader.get_ref().read_bytes();
            if decoded != open_size {
                return Err(Error::GpgVerificationFailed(format!(
                    "signed repomd.xml authenticates primary metadata as {open_size} decompressed bytes but {primary_url} decoded to {decoded} bytes"
                )));
            }
            package_count
        };
        sink.authenticated_object(primary_object)?;

        // primary.xml carries only the generator-selected file set, so
        // complete package file ownership comes from filelists.xml.
        match filelists_download {
            Some((filelists_url, filelists_path, filelists_object, open_size)) => {
                let ingest = filelists::ingest_verified_filelists_into(
                    sink,
                    &filelists_path,
                    open_size,
                    &filelists_url,
                )?;
                sink.authenticated_object(filelists_object)?;
                info!(
                    "Ingested {} filelists package records: {} file capabilities added, {} already \
                     published by primary.xml",
                    ingest.records, ingest.files_added, ingest.files_already_known
                );
            }
            None => {
                sink.validate_rpm_primary_file_requirements(repo_url)?;
            }
        }

        info!("Parsed {package_count} packages from Fedora repository");
        Ok(snapshot)
    }
}

#[cfg(test)]
#[path = "fedora/tests.rs"]
mod tests;
