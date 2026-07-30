// conary-core/src/repository/parsers/fedora.rs

//! Fedora/RPM repository metadata parser
//!
//! Parses Fedora-style repomd.xml and primary.xml files which contain
//! RPM package metadata in XML format.
//! Generator-selected `<file>` records in primary.xml are package-owned file
//! provides. This follows createrepo_c's pinned primary metadata contract:
//! <https://github.com/rpm-software-management/createrepo_c/blob/5cf41fe5d703901d78078ed18c67ab667e446c1a/src/xml_dump.c#L175-L225>.

use super::common::{self, MAX_PACKAGE_SIZE};
use super::{ChecksumType, PackageMetadata, RepositoryParser};
use crate::compression::decompress_metadata_auto;
use crate::error::{Error, Result};
use crate::repository::client::RepositoryClient;
use crate::repository::dependency_model::{
    RepositoryCapabilityKind, RepositoryDependencyFlavor, RepositoryProvide,
    RepositoryRequirementGroup, RepositoryRequirementKind,
};
use crate::repository::package_relation::parse_native_relation;
use crate::repository::trust::openpgp::PreparedOpenPgpTrust;
use crate::repository::trust::{RepositoryTrustPolicy, RpmMetadataAuthority, TrustRole};
use crate::repository::versioning::VersionScheme;
use quick_xml::Reader;
use quick_xml::events::Event;
use serde_json::json;
use tracing::{debug, info};

mod metalink;
mod relation;
use metalink::parse_metalink_repomd_identity;
use relation::{
    RpmProvideConstraint, rpm_provide_constraint, rpm_relation_native_text, rpm_require_to_group,
};

/// Fedora/RPM repository parser
pub struct FedoraParser {
    /// Repository architecture (e.g., "x86_64", "aarch64")
    architecture: String,
    trust: PreparedOpenPgpTrust,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FormatSection {
    Requires,
    Provides,
    Conflicts,
    Obsoletes,
}

impl FormatSection {
    const fn metadata_name(self) -> &'static str {
        match self {
            Self::Requires => "requires",
            Self::Provides => "provides",
            Self::Conflicts => "conflicts",
            Self::Obsoletes => "obsoletes",
        }
    }
}

impl FedoraParser {
    fn local_tag_name(tag_name: &str) -> &str {
        tag_name.rsplit(':').next().unwrap_or(tag_name)
    }

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

    /// Download repomd.xml and find primary.xml location
    ///
    /// Uses RepositoryClient for HTTP.
    async fn get_primary_xml_location(&self, repo_url: &str) -> Result<PrimaryMetadataLocation> {
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
        let xml_content = String::from_utf8(xml_bytes)
            .map_err(|e| Error::ParseError(format!("Invalid UTF-8 in repomd.xml: {}", e)))?;

        // Parse repomd.xml to find primary location
        let mut reader = Reader::from_str(&xml_content);
        reader.config_mut().trim_text_end = true;

        let mut buf = Vec::new();
        let mut in_primary = false;
        let mut location = None;
        let mut checksum = None;
        let mut checksum_type = None;
        let mut size = None;
        let mut current_field = None;
        let mut primary_records = 0usize;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) if e.name().as_ref() == b"data" => {
                    // Check if this is the primary data type
                    for attr in e.attributes() {
                        let attr = attr.map_err(|error| {
                            Error::ParseError(format!(
                                "Failed to parse repomd.xml data attribute: {error}"
                            ))
                        })?;
                        if attr.key.as_ref() == b"type"
                            && attr
                                .decoded_and_normalized_value(
                                    quick_xml::XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|error| {
                                    Error::ParseError(format!(
                                        "Failed to decode repomd.xml data type: {error}"
                                    ))
                                })?
                                == "primary"
                        {
                            primary_records += 1;
                            if primary_records > 1 {
                                return Err(Error::ParseError(
                                    "repomd.xml repeats primary metadata records".to_string(),
                                ));
                            }
                            in_primary = true;
                        }
                    }
                }
                Ok(Event::Start(e)) if in_primary && e.name().as_ref() == b"checksum" => {
                    current_field = Some("checksum");
                    for attr in e.attributes() {
                        let attr = attr.map_err(|error| {
                            Error::ParseError(format!(
                                "Failed to parse repomd.xml checksum attribute: {error}"
                            ))
                        })?;
                        if attr.key.as_ref() == b"type" {
                            checksum_type = Some(
                                attr.decoded_and_normalized_value(
                                    quick_xml::XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|error| {
                                    Error::ParseError(format!(
                                        "Failed to decode repomd.xml checksum type: {error}"
                                    ))
                                })?
                                .into_owned(),
                            );
                        }
                    }
                }
                Ok(Event::Start(e)) if in_primary && e.name().as_ref() == b"size" => {
                    current_field = Some("size");
                }
                Ok(Event::Start(e) | Event::Empty(e))
                    if e.name().as_ref() == b"location" && in_primary =>
                {
                    // Extract href attribute
                    for attr in e.attributes() {
                        let attr = attr.map_err(|error| {
                            Error::ParseError(format!(
                                "Failed to parse repomd.xml location attribute: {error}"
                            ))
                        })?;
                        if attr.key.as_ref() == b"href" {
                            location = Some(
                                attr.decoded_and_normalized_value(
                                    quick_xml::XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|error| {
                                    Error::ParseError(format!(
                                        "Failed to decode repomd.xml location: {error}"
                                    ))
                                })?
                                .into_owned(),
                            );
                        }
                    }
                }
                Ok(Event::Text(text)) if in_primary => {
                    let value = text
                        .xml_content(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|error| {
                            Error::ParseError(format!("Failed to decode repomd.xml text: {error}"))
                        })?
                        .trim()
                        .to_string();
                    match current_field {
                        Some("checksum") => checksum = Some(value),
                        Some("size") => size = Some(value),
                        _ => {}
                    }
                }
                Ok(Event::End(e)) if e.name().as_ref() == b"data" => {
                    in_primary = false;
                    current_field = None;
                }
                Ok(Event::End(e)) if matches!(e.name().as_ref(), b"checksum" | b"size") => {
                    current_field = None;
                }
                Ok(Event::Eof) => break,
                Err(e) => {
                    return Err(Error::ParseError(format!(
                        "Failed to parse repomd.xml: {}",
                        e
                    )));
                }
                _ => {}
            }
            buf.clear();
        }

        let href = location.ok_or_else(|| {
            Error::ParseError("Could not find primary data location in repomd.xml".to_string())
        })?;
        common::validate_filename(&href).map_err(Error::ParseError)?;
        if checksum_type.as_deref() != Some("sha256") {
            return Err(Error::ParseError(format!(
                "RPM primary metadata requires exact sha256 identity; found {:?}",
                checksum_type.as_deref()
            )));
        }
        let sha256 = checksum.ok_or_else(|| {
            Error::ParseError("repomd.xml primary record has no checksum".to_string())
        })?;
        validate_sha256(&sha256, "repomd.xml primary")?;
        let size = size
            .ok_or_else(|| {
                Error::ParseError("repomd.xml primary record has no compressed size".to_string())
            })?
            .parse::<u64>()
            .map_err(|error| {
                Error::ParseError(format!(
                    "repomd.xml primary compressed size is invalid: {error}"
                ))
            })?;
        Ok(PrimaryMetadataLocation { href, sha256, size })
    }

    /// Download and decompress primary.xml
    ///
    /// Uses RepositoryClient for HTTP and the compression module for auto-decompression.
    async fn download_primary_xml(
        &self,
        repo_url: &str,
        location: &PrimaryMetadataLocation,
    ) -> Result<String> {
        let primary_url = format!("{}/{}", repo_url.trim_end_matches('/'), location.href);
        debug!("Downloading primary.xml from: {}", primary_url);

        let client = RepositoryClient::new()?;
        let raw_bytes = client.download_to_bytes(&primary_url).await?;
        if raw_bytes.len() as u64 != location.size {
            return Err(Error::GpgVerificationFailed(format!(
                "signed repomd.xml authenticates primary metadata as {} bytes but the repository \
                 served {} bytes",
                location.size,
                raw_bytes.len()
            )));
        }
        let actual = crate::hash::sha256(&raw_bytes);
        if actual != location.sha256 {
            return Err(Error::GpgVerificationFailed(format!(
                "RPM primary metadata identity mismatch: signed repomd.xml SHA256 is {}, \
                 downloaded SHA256 is {}",
                location.sha256, actual
            )));
        }
        let decompressed =
            decompress_metadata_auto(&raw_bytes, &format!("RPM primary metadata {primary_url}"))?;
        let content = String::from_utf8(decompressed).map_err(|error| {
            Error::ParseError(format!("Invalid UTF-8 in primary.xml: {}", error))
        })?;

        debug!("Decompressed primary.xml: {} bytes", content.len());
        Ok(content)
    }

    /// Parse primary.xml and extract package metadata
    fn parse_primary_xml(&self, xml_content: &str, base_url: &str) -> Result<Vec<PackageMetadata>> {
        let mut reader = Reader::from_str(xml_content);
        reader.config_mut().trim_text_end = true;

        let mut packages = Vec::new();
        let mut buf = Vec::new();

        // Current package being built
        let mut current_package: Option<PackageBuilder> = None;
        let mut current_tag = String::new();
        let mut in_format = false;
        let mut format_section = None;
        let mut current_primary_file = None::<String>;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    let local_tag = Self::local_tag_name(&tag_name);
                    if current_primary_file.is_some() {
                        return Err(Error::ParseError(
                            "RPM primary file record cannot contain nested elements".to_string(),
                        ));
                    }
                    current_tag = tag_name.clone();

                    match local_tag {
                        "package" => {
                            current_package = Some(PackageBuilder::new());
                        }
                        "format" => in_format = true,
                        "requires" if in_format => {
                            format_section = Some(FormatSection::Requires);
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
                            current_primary_file = Some(String::new());
                            // A path's trailing XML whitespace is package
                            // identity, not inter-element formatting.
                            reader.config_mut().trim_text_end = false;
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
                    let local_tag = Self::local_tag_name(&tag_name);
                    if current_primary_file.is_some() {
                        return Err(Error::ParseError(
                            "RPM primary file record cannot contain nested elements".to_string(),
                        ));
                    }

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
                                            )? {
                                                pkg.dependencies.push(requirement);
                                            }
                                        }
                                        FormatSection::Provides => {
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
                            return Err(Error::ParseError(
                                "RPM primary file record must contain an absolute path".to_string(),
                            ));
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
                    if let Some(file) = current_primary_file.as_mut() {
                        file.push_str(&text);
                    } else if let Some(ref mut pkg) = current_package {
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
                    if let Some(file) = current_primary_file.as_mut() {
                        file.push_str(&text);
                    } else if let Some(ref mut pkg) = current_package {
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
                    if let Some(file) = current_primary_file.as_mut() {
                        file.push_str(&text);
                    } else if let Some(ref mut pkg) = current_package {
                        append_package_text(pkg, &current_tag, &text);
                    }
                }
                Ok(Event::End(e)) => {
                    let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    let local_tag = Self::local_tag_name(&tag_name);
                    if local_tag == "file" && in_format {
                        let file = current_primary_file.take().ok_or_else(|| {
                            Error::ParseError(
                                "RPM primary file record ended without starting".to_string(),
                            )
                        })?;
                        reader.config_mut().trim_text_end = true;
                        let file = validate_primary_file_path(file)?;
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
                        packages.push(builder.build(base_url)?);
                    } else if local_tag == "format" {
                        in_format = false;
                        format_section = None;
                    } else if matches!(
                        local_tag,
                        "requires" | "provides" | "conflicts" | "obsoletes"
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PrimaryMetadataLocation {
    href: String,
    sha256: String,
    size: u64,
}

fn validate_sha256(value: &str, label: &str) -> Result<()> {
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
        let version = if epoch == "0" {
            format!("{}-{}", ver, rel)
        } else {
            format!("{}:{}-{}", epoch, ver, rel)
        };

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

        // Build structured provides
        let mut structured_provides: Vec<RepositoryProvide> = Vec::new();

        // Implicit self-provide: the package name itself
        structured_provides.push(RepositoryProvide::package_name(
            name.clone(),
            Some(version.clone()),
        ));

        for (prov_name, provide) in &self.provides {
            if crate::repository::rpm_runtime::RpmRuntimeFeature::parse_capability(prov_name)
                .map_err(|error| Error::ParseError(error.to_string()))?
                .is_some()
            {
                return Err(Error::ParseError(format!(
                    "RPM repository package cannot provide package-manager runtime capability '{prov_name}'; Conary's typed runtime ledger is the sole authority"
                )));
            }
            let kind = if prov_name == &name {
                RepositoryCapabilityKind::PackageName
            } else {
                // RPM primary metadata exposes a capability atom, but not a
                // separate path/soname type. Preserve that exact contract
                // instead of inferring a semantic class from its spelling.
                RepositoryCapabilityKind::Generic
            };

            let native_text = if provide.native_constraint.is_empty() {
                prov_name.clone()
            } else {
                format!("{prov_name} {}", provide.native_constraint)
            };

            structured_provides.push(RepositoryProvide {
                name: prov_name.clone(),
                kind,
                version: provide.version.clone(),
                version_relation: provide.version_relation,
                architecture_qualifier:
                    crate::repository::dependency_model::ProvideArchitectureQualifier::Implicit,
                native_text: Some(native_text),
            });
        }
        structured_provides.extend(
            self.primary_files
                .iter()
                .cloned()
                .map(RepositoryProvide::file),
        );

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

fn validate_primary_file_path(path: String) -> Result<String> {
    if path.is_empty() || !path.starts_with('/') {
        return Err(Error::ParseError(
            "RPM primary file record must contain an absolute path".to_string(),
        ));
    }
    Ok(path)
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
    async fn sync_metadata(&self, repo_url: &str) -> Result<Vec<PackageMetadata>> {
        info!("Syncing Fedora repository for {}", self.architecture);

        // Get primary.xml location from repomd.xml
        let primary_location = self.get_primary_xml_location(repo_url).await?;

        // Download and decompress primary.xml
        let primary_xml = self
            .download_primary_xml(repo_url, &primary_location)
            .await?;

        // Parse primary.xml
        let packages = self.parse_primary_xml(&primary_xml, repo_url)?;

        info!("Parsed {} packages from Fedora repository", packages.len());
        Ok(packages)
    }
}

#[cfg(test)]
#[path = "fedora/tests.rs"]
mod tests;
