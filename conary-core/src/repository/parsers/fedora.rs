// conary-core/src/repository/parsers/fedora.rs

//! Fedora/RPM repository metadata parser
//!
//! Parses Fedora-style repomd.xml and primary.xml files which contain
//! RPM package metadata in XML format.

use super::common::{self, MAX_PACKAGE_SIZE};
use super::{ChecksumType, Dependency, PackageMetadata, RepositoryParser};
use crate::compression::decompress_auto;
use crate::error::{Error, Result};
use crate::repository::client::RepositoryClient;
use crate::repository::dependency_model::{
    ConditionalRequirementBehavior, RepositoryCapabilityKind, RepositoryDependencyFlavor,
    RepositoryProvide, RepositoryRequirementClause, RepositoryRequirementGroup,
    RepositoryRequirementKind,
};
use crate::repository::gpg::MetadataSignatureVerifier;
use crate::repository::versioning::VersionScheme;
use quick_xml::Reader;
use quick_xml::events::Event;
use serde_json::json;
use tracing::{debug, info};

/// Fedora/RPM repository parser
pub struct FedoraParser {
    /// Repository architecture (e.g., "x86_64", "aarch64")
    architecture: String,
    metadata_signature_verifier: Option<MetadataSignatureVerifier>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FormatSection {
    Requires,
    Provides,
}

impl FedoraParser {
    fn local_tag_name(tag_name: &str) -> &str {
        tag_name.rsplit(':').next().unwrap_or(tag_name)
    }

    /// Create a new Fedora/RPM parser
    pub fn new(architecture: String) -> Self {
        Self {
            architecture,
            metadata_signature_verifier: None,
        }
    }

    pub fn with_metadata_signature_verifier(
        mut self,
        metadata_signature_verifier: Option<MetadataSignatureVerifier>,
    ) -> Self {
        self.metadata_signature_verifier = metadata_signature_verifier;
        self
    }

    /// Download repomd.xml and find primary.xml location
    ///
    /// Uses RepositoryClient for HTTP.
    async fn get_primary_xml_location(&self, repo_url: &str) -> Result<String> {
        let repomd_url = format!("{}/repodata/repomd.xml", repo_url.trim_end_matches('/'));
        debug!("Downloading repomd.xml from: {}", repomd_url);

        let client = RepositoryClient::new()?;
        let xml_bytes = client.download_to_bytes(&repomd_url).await?;
        if let Some(verifier) = &self.metadata_signature_verifier {
            verifier
                .verify_metadata_bytes(&repomd_url, &xml_bytes, "repomd.xml")
                .await?;
        }
        let xml_content = String::from_utf8(xml_bytes)
            .map_err(|e| Error::ParseError(format!("Invalid UTF-8 in repomd.xml: {}", e)))?;

        // Parse repomd.xml to find primary location
        let mut reader = Reader::from_str(&xml_content);
        reader.config_mut().trim_text_end = true;

        let mut buf = Vec::new();
        let mut in_primary = false;
        let mut location = None;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) if e.name().as_ref() == b"data" => {
                    // Check if this is the primary data type
                    if let Some(Ok(attr)) = e.attributes().find(|a| {
                        a.as_ref()
                            .map(|attr| attr.key.as_ref() == b"type")
                            .unwrap_or(false)
                    }) {
                        let attr_value: &[u8] = attr.value.as_ref();
                        if attr_value == b"primary" {
                            in_primary = true;
                        }
                    }
                }
                Ok(Event::Start(e) | Event::Empty(e))
                    if e.name().as_ref() == b"location" && in_primary =>
                {
                    // Extract href attribute
                    if let Some(Ok(attr)) = e.attributes().find(|a| {
                        a.as_ref()
                            .map(|attr| attr.key.as_ref() == b"href")
                            .unwrap_or(false)
                    }) {
                        location = Some(String::from_utf8_lossy(attr.value.as_ref()).to_string());
                    }
                }
                Ok(Event::End(e)) if e.name().as_ref() == b"data" => {
                    in_primary = false;
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

        location.ok_or_else(|| {
            Error::ParseError("Could not find primary data location in repomd.xml".to_string())
        })
    }

    /// Download and decompress primary.xml
    ///
    /// Uses RepositoryClient for HTTP and the compression module for auto-decompression.
    async fn download_primary_xml(&self, repo_url: &str, location: &str) -> Result<String> {
        let primary_url = format!("{}/{}", repo_url.trim_end_matches('/'), location);
        debug!("Downloading primary.xml from: {}", primary_url);

        let client = RepositoryClient::new()?;
        let raw_bytes = client.download_to_bytes(&primary_url).await?;
        if let Some(verifier) = &self.metadata_signature_verifier {
            verifier
                .verify_metadata_bytes(&primary_url, &raw_bytes, "primary.xml")
                .await?;
        }
        let decompressed = decompress_auto(&raw_bytes).map_err(|error| {
            Error::ParseError(format!("Failed to decompress {}: {}", primary_url, error))
        })?;
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

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    let local_tag = Self::local_tag_name(&tag_name);
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
                        _ => {}
                    }
                }
                Ok(Event::Empty(e)) => {
                    let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    let local_tag = Self::local_tag_name(&tag_name);

                    match local_tag {
                        "version" => {
                            if let Some(ref mut pkg) = current_package {
                                // Extract epoch, ver, rel attributes
                                for attr in e.attributes().filter_map(|a| a.ok()) {
                                    let key = String::from_utf8_lossy(attr.key.as_ref());
                                    let value = String::from_utf8_lossy(&attr.value);
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
                                for attr in e.attributes().filter_map(|a| a.ok()) {
                                    let key = String::from_utf8_lossy(attr.key.as_ref());
                                    if key == "type" {
                                        let value = String::from_utf8_lossy(&attr.value);
                                        pkg.checksum_type = Some(value.to_string());
                                    }
                                }
                            }
                        }
                        "size" => {
                            if let Some(ref mut pkg) = current_package {
                                for attr in e.attributes().filter_map(|a| a.ok()) {
                                    let key = String::from_utf8_lossy(attr.key.as_ref());
                                    if key == "package" {
                                        let value = String::from_utf8_lossy(&attr.value);
                                        pkg.size = Some(value.to_string());
                                    }
                                }
                            }
                        }
                        "location" => {
                            if let Some(ref mut pkg) = current_package {
                                for attr in e.attributes().filter_map(|a| a.ok()) {
                                    let key = String::from_utf8_lossy(attr.key.as_ref());
                                    if key == "href" {
                                        let value = String::from_utf8_lossy(&attr.value);
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
                                let mut dep_ver = None;

                                for attr in e.attributes().filter_map(|a| a.ok()) {
                                    let key = String::from_utf8_lossy(attr.key.as_ref());
                                    let value = String::from_utf8_lossy(&attr.value);
                                    match key.as_ref() {
                                        "name" => dep_name = Some(value.to_string()),
                                        "flags" => dep_flags = Some(value.to_string()),
                                        "ver" => dep_ver = Some(value.to_string()),
                                        _ => {}
                                    }
                                }

                                if let Some(name) = dep_name
                                    && !name.starts_with("rpmlib(")
                                    && !name.starts_with("config(")
                                {
                                    let constraint = match (dep_flags, dep_ver) {
                                        (Some(flags), Some(ver)) => {
                                            let op = rpm_flags_to_op(&flags);
                                            if op.is_empty() {
                                                String::new()
                                            } else {
                                                format!("{op} {ver}")
                                            }
                                        }
                                        _ => String::new(),
                                    };

                                    if name.starts_with('/') {
                                        // File-path entry: only include in
                                        // structured output
                                        match format_section {
                                            Some(FormatSection::Requires) => {
                                                pkg.file_requires.push(name);
                                            }
                                            Some(FormatSection::Provides) => {
                                                pkg.file_provides.push(name);
                                            }
                                            None => {}
                                        }
                                    } else {
                                        match format_section {
                                            Some(FormatSection::Requires) => {
                                                pkg.dependencies.push((name, constraint));
                                            }
                                            Some(FormatSection::Provides) => {
                                                pkg.provides.push((name, constraint));
                                            }
                                            None => {}
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::Text(e)) => {
                    if let Some(ref mut pkg) = current_package {
                        let text = e.decode().unwrap_or_default().to_string();
                        // Skip inter-element whitespace that quick_xml emits as
                        // text events -- without this guard, trailing whitespace
                        // between tags overwrites fields like pkg.name with "".
                        if text.is_empty() {
                            continue;
                        }
                        match current_tag.as_str() {
                            "name" => pkg.name = Some(text),
                            "arch" => pkg.arch = Some(text),
                            "summary" => pkg.summary = Some(text),
                            "description" => pkg.description = Some(text),
                            "checksum" => pkg.checksum = Some(text),
                            "url" => pkg.url = Some(text),
                            _ => {}
                        }
                    }
                }
                Ok(Event::End(e)) => {
                    let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    let local_tag = Self::local_tag_name(&tag_name);
                    if local_tag == "package"
                        && let Some(builder) = current_package.take()
                        && let Ok(pkg) = builder.build(base_url)
                    {
                        packages.push(pkg);
                    } else if local_tag == "format" {
                        in_format = false;
                        format_section = None;
                    } else if local_tag == "requires" || local_tag == "provides" {
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
    dependencies: Vec<(String, String)>,
    provides: Vec<(String, String)>,
    /// File-path provides (e.g. `/usr/bin/foo`) -- filtered from legacy output
    /// but included in structured provides.
    file_provides: Vec<String>,
    /// File-path requires (e.g. `/usr/bin/sh`) -- filtered from legacy output
    /// but included in structured requirements.
    file_requires: Vec<String>,
}

/// Classify an RPM provide name into the appropriate capability kind.
fn classify_rpm_provide(name: &str) -> RepositoryCapabilityKind {
    if name.starts_with('/') {
        RepositoryCapabilityKind::File
    } else if name.contains(".so") {
        RepositoryCapabilityKind::Soname
    } else {
        // Will be refined to PackageName for self-provides in the builder
        RepositoryCapabilityKind::Generic
    }
}

/// Convert RPM flags string to a version constraint operator.
fn rpm_flags_to_op(flags: &str) -> &str {
    match flags {
        "GE" => ">=",
        "LE" => "<=",
        "EQ" => "=",
        "LT" => "<",
        "GT" => ">",
        _ => "",
    }
}

/// Check whether an RPM requires entry looks like a rich/conditional dep.
///
/// Rich deps have the form `(foo if bar)`, `(foo or bar)`, etc.
fn is_rich_dep(name: &str) -> bool {
    name.starts_with('(') && name.ends_with(')')
}

/// Try to parse an RPM rich dependency `(A or B)` into individual clause names.
///
/// Returns `Some(vec!["A", "B"])` for `(A or B)` style deps.
/// Returns `None` for conditionals like `(A if B)`, `(A unless B)`,
/// nested rich deps, or anything we cannot safely decompose.
fn parse_rich_dep_or(name: &str) -> Option<Vec<String>> {
    // Strip outer parens
    let inner = name.strip_prefix('(')?.strip_suffix(')')?;

    // Reject conditionals and nested rich deps
    if inner.contains(" if ")
        || inner.contains(" unless ")
        || inner.contains(" else ")
        || inner.contains(" with ")
        || inner.contains(" without ")
        || inner.contains('(')
    {
        return None;
    }

    // Split on " or " (RPM rich dep OR separator)
    let parts: Vec<String> = inner.split(" or ").map(|s| s.trim().to_string()).collect();
    if parts.len() >= 2 && parts.iter().all(|p| !p.is_empty()) {
        Some(parts)
    } else {
        None
    }
}

/// Build a `RepositoryRequirementGroup` from a single RPM requires entry.
fn rpm_require_to_group(name: &str, constraint: &str) -> RepositoryRequirementGroup {
    let native_text = if constraint.is_empty() {
        name.to_string()
    } else {
        format!("{name} {constraint}")
    };

    if is_rich_dep(name) {
        // Try to decompose `(A or B)` into a proper OR-group
        if let Some(alternatives) = parse_rich_dep_or(name) {
            let clauses: Vec<RepositoryRequirementClause> = alternatives
                .into_iter()
                .map(|alt| {
                    // Each alternative may have a version constraint, e.g. "foo >= 1.0".
                    // Split into name and optional constraint at the first comparison op.
                    if let Some(idx) = alt.find(['>', '<', '=']) {
                        let pkg_name = alt[..idx].trim();
                        let constraint = alt[idx..].trim();
                        RepositoryRequirementClause {
                            name: pkg_name.to_string(),
                            version_constraint: Some(constraint.to_string()),
                            ..RepositoryRequirementClause::name_only(pkg_name.to_string())
                        }
                    } else {
                        RepositoryRequirementClause::name_only(alt)
                    }
                })
                .collect();
            return RepositoryRequirementGroup::alternatives(
                RepositoryRequirementKind::Depends,
                clauses,
            )
            .with_native_text(native_text);
        }

        // Other rich/conditional dep -- mark as conditional, keep opaque text
        let clause = RepositoryRequirementClause {
            name: name.to_string(),
            capability_kind: None,
            version_constraint: None,
            native_text: Some(native_text.clone()),
        };
        RepositoryRequirementGroup {
            kind: RepositoryRequirementKind::Depends,
            behavior: ConditionalRequirementBehavior::Conditional,
            alternatives: vec![clause],
            description: None,
            native_text: Some(native_text),
        }
    } else {
        let clause = if constraint.is_empty() {
            RepositoryRequirementClause::name_only(name.to_string())
        } else {
            RepositoryRequirementClause::versioned(name.to_string(), constraint.to_string())
        };
        RepositoryRequirementGroup::simple(RepositoryRequirementKind::Depends, clause)
            .with_native_text(native_text)
    }
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
            Some("sha512") => ChecksumType::Sha512,
            _ => ChecksumType::Sha256, // Default
        };

        let rpm_requires: Vec<String> = self
            .dependencies
            .iter()
            .map(|(name, constraint)| {
                if constraint.is_empty() {
                    name.clone()
                } else {
                    format!("{name} {constraint}")
                }
            })
            .collect();

        // Build structured requirements
        let mut requirements: Vec<RepositoryRequirementGroup> = self
            .dependencies
            .iter()
            .map(|(dep_name, constraint)| rpm_require_to_group(dep_name, constraint))
            .collect();

        // Include file-path requires in structured output
        for file_req in &self.file_requires {
            let clause = RepositoryRequirementClause {
                name: file_req.clone(),
                capability_kind: Some(RepositoryCapabilityKind::File),
                version_constraint: None,
                native_text: Some(file_req.clone()),
            };
            requirements.push(
                RepositoryRequirementGroup::simple(RepositoryRequirementKind::Depends, clause)
                    .with_native_text(file_req.clone()),
            );
        }

        // Build structured provides
        let mut structured_provides: Vec<RepositoryProvide> = Vec::new();

        // Implicit self-provide: the package name itself
        structured_provides.push(RepositoryProvide::package_name(
            name.clone(),
            Some(version.clone()),
        ));

        for (prov_name, prov_constraint) in &self.provides {
            let kind = if prov_name == &name {
                RepositoryCapabilityKind::PackageName
            } else {
                classify_rpm_provide(prov_name)
            };

            let prov_version = common::extract_version_from_constraint(prov_constraint);

            let native_text = if prov_constraint.is_empty() {
                prov_name.clone()
            } else {
                format!("{prov_name} {prov_constraint}")
            };

            structured_provides.push(RepositoryProvide {
                name: prov_name.clone(),
                kind,
                version: prov_version,
                native_text: Some(native_text),
            });
        }

        // Include file-path provides in structured output
        for file_prov in &self.file_provides {
            structured_provides.push(RepositoryProvide::file(file_prov.clone()));
        }

        // Convert dependencies (legacy)
        let dependencies = self
            .dependencies
            .into_iter()
            .map(|(dep_name, constraint)| {
                if constraint.is_empty() {
                    Dependency::runtime(dep_name)
                } else {
                    Dependency::runtime_versioned(dep_name, constraint)
                }
            })
            .collect();
        let rpm_provides: Vec<String> = self
            .provides
            .iter()
            .map(|(prov_name, constraint)| {
                if constraint.is_empty() {
                    prov_name.clone()
                } else {
                    format!("{prov_name} {constraint}")
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
        extra.insert("rpm_requires".to_string(), json!(rpm_requires));
        extra.insert("rpm_provides".to_string(), json!(rpm_provides));

        Ok(PackageMetadata {
            name,
            version,
            architecture: self.arch,
            description: self.description,
            checksum,
            checksum_type,
            size,
            download_url,
            dependencies,
            extra_metadata: serde_json::Value::Object(extra),
            source_distro: Some(RepositoryDependencyFlavor::Rpm),
            version_scheme: Some(VersionScheme::Rpm),
            requirements,
            provides: structured_provides,
        })
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
mod tests {
    use super::*;

    fn valid_builder() -> PackageBuilder {
        let mut builder = PackageBuilder::new();
        builder.name = Some("test-package".to_string());
        builder.epoch = Some("1".to_string());
        builder.ver = Some("2.3.4".to_string());
        builder.rel = Some("5.fc43".to_string());
        builder.arch = Some("x86_64".to_string());
        builder.checksum = Some("abc123".to_string());
        builder.checksum_type = Some("sha256".to_string());
        builder.size = Some("1024".to_string());
        builder.location = Some("Packages/t/test-package-2.3.4-5.fc43.x86_64.rpm".to_string());
        builder
    }

    #[test]
    fn test_package_builder() {
        let builder = valid_builder();
        let pkg = builder.build("https://example.com").unwrap();
        assert_eq!(pkg.name, "test-package");
        assert_eq!(pkg.version, "1:2.3.4-5.fc43");
        assert_eq!(pkg.size, 1024);
    }

    #[test]
    fn test_package_builder_rejects_path_traversal() {
        let mut builder = valid_builder();
        builder.location = Some("../../../etc/passwd".to_string());
        let result = builder.build("https://example.com");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path traversal"));
    }

    #[test]
    fn test_package_builder_rejects_absolute_path() {
        let mut builder = valid_builder();
        builder.location = Some("/etc/passwd".to_string());
        let result = builder.build("https://example.com");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not relative"));
    }

    #[test]
    fn test_package_builder_rejects_url_scheme() {
        let mut builder = valid_builder();
        builder.location = Some("https://evil.com/malware.rpm".to_string());
        let result = builder.build("https://example.com");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not relative"));
    }

    #[test]
    fn test_package_builder_rejects_oversized() {
        let mut builder = valid_builder();
        builder.size = Some("10000000000000".to_string()); // 10TB
        let result = builder.build("https://example.com");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("exceeds maximum"));
    }

    #[test]
    fn test_parse_primary_xml_captures_namespaced_requires_and_provides() {
        let parser = FedoraParser::new("x86_64".to_string());
        let xml = r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>kernel-core</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="6.19.6" rel="200.fc43"/>
    <checksum type="sha256">deadbeef</checksum>
    <summary>kernel core</summary>
    <description>kernel core</description>
    <size package="123"/>
    <location href="Packages/k/kernel-core-6.19.6-200.fc43.x86_64.rpm"/>
    <format>
      <rpm:provides>
        <rpm:entry name="kernel-core-uname-r" flags="EQ" ver="6.19.6-200.fc43.x86_64"/>
      </rpm:provides>
      <rpm:requires>
        <rpm:entry name="systemd" flags="GE" ver="255"/>
      </rpm:requires>
    </format>
  </package>
</metadata>
"#;

        let packages = parser
            .parse_primary_xml(xml, "https://example.com")
            .unwrap();
        let pkg = packages.first().unwrap();
        let metadata = pkg.extra_metadata.as_object().unwrap();
        let provides = metadata
            .get("rpm_provides")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>();
        let requires = metadata
            .get("rpm_requires")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>();

        assert!(provides.contains(&"kernel-core-uname-r = 6.19.6-200.fc43.x86_64"));
        assert!(requires.contains(&"systemd >= 255"));
    }

    #[test]
    fn test_builder_sets_source_distro_and_version_scheme() {
        let builder = valid_builder();
        let pkg = builder.build("https://example.com").unwrap();
        assert_eq!(pkg.source_distro, Some(RepositoryDependencyFlavor::Rpm));
        assert_eq!(pkg.version_scheme, Some(VersionScheme::Rpm));
    }

    #[test]
    fn test_structured_kernel_uname_r_provide() {
        let parser = FedoraParser::new("x86_64".to_string());
        let xml = r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>kernel-core</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="6.19.6" rel="200.fc43"/>
    <checksum type="sha256">deadbeef</checksum>
    <summary>kernel core</summary>
    <description>kernel core</description>
    <size package="123"/>
    <location href="Packages/k/kernel-core-6.19.6-200.fc43.x86_64.rpm"/>
    <format>
      <rpm:provides>
        <rpm:entry name="kernel-core-uname-r" flags="EQ" ver="6.19.6-200.fc43.x86_64"/>
      </rpm:provides>
    </format>
  </package>
</metadata>
"#;

        let packages = parser
            .parse_primary_xml(xml, "https://example.com")
            .unwrap();
        let pkg = &packages[0];

        // Should have self-provide + kernel-core-uname-r
        assert!(pkg.provides.len() >= 2);

        let uname_provide = pkg
            .provides
            .iter()
            .find(|p| p.name == "kernel-core-uname-r")
            .expect("kernel-core-uname-r provide not found");
        assert_eq!(uname_provide.kind, RepositoryCapabilityKind::Generic);
        assert_eq!(
            uname_provide.version.as_deref(),
            Some("6.19.6-200.fc43.x86_64")
        );
        assert_eq!(
            uname_provide.native_text.as_deref(),
            Some("kernel-core-uname-r = 6.19.6-200.fc43.x86_64")
        );
    }

    #[test]
    fn test_structured_versioned_provide_coreutils() {
        let parser = FedoraParser::new("x86_64".to_string());
        let xml = r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>coreutils-common</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="9.7" rel="1.fc43"/>
    <checksum type="sha256">abcd1234</checksum>
    <summary>Common files for coreutils</summary>
    <description>Common files</description>
    <size package="456"/>
    <location href="Packages/c/coreutils-common-9.7-1.fc43.x86_64.rpm"/>
    <format>
      <rpm:provides>
        <rpm:entry name="coreutils-common" flags="EQ" ver="9.7"/>
      </rpm:provides>
    </format>
  </package>
</metadata>
"#;

        let packages = parser
            .parse_primary_xml(xml, "https://example.com")
            .unwrap();
        let pkg = &packages[0];

        // Self-provide from the explicit entry should be PackageName kind
        let self_provide = pkg
            .provides
            .iter()
            .find(|p| p.name == "coreutils-common" && p.native_text.is_some())
            .expect("explicit self-provide not found");
        assert_eq!(self_provide.kind, RepositoryCapabilityKind::PackageName);
        assert_eq!(self_provide.version.as_deref(), Some("9.7"));
    }

    #[test]
    fn test_structured_rich_conditional_dep() {
        let parser = FedoraParser::new("x86_64".to_string());
        let xml = r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>test-pkg</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="1.0" rel="1.fc43"/>
    <checksum type="sha256">beef1234</checksum>
    <summary>Test</summary>
    <description>Test</description>
    <size package="100"/>
    <location href="Packages/t/test-pkg-1.0-1.fc43.x86_64.rpm"/>
    <format>
      <rpm:requires>
        <rpm:entry name="(foo if bar)"/>
        <rpm:entry name="systemd" flags="GE" ver="255"/>
      </rpm:requires>
    </format>
  </package>
</metadata>
"#;

        let packages = parser
            .parse_primary_xml(xml, "https://example.com")
            .unwrap();
        let pkg = &packages[0];

        assert_eq!(pkg.requirements.len(), 2);

        // Rich dep should be conditional
        let rich = &pkg.requirements[0];
        assert_eq!(rich.behavior, ConditionalRequirementBehavior::Conditional);
        assert_eq!(rich.alternatives[0].name, "(foo if bar)");

        // Normal dep should be hard
        let normal = &pkg.requirements[1];
        assert_eq!(normal.behavior, ConditionalRequirementBehavior::Hard);
        assert_eq!(normal.alternatives[0].name, "systemd");
        assert_eq!(
            normal.alternatives[0].version_constraint.as_deref(),
            Some(">= 255")
        );
    }

    #[test]
    fn test_structured_soname_provide() {
        let parser = FedoraParser::new("x86_64".to_string());
        let xml = r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>glibc</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="2.40" rel="1.fc43"/>
    <checksum type="sha256">cafe0001</checksum>
    <summary>glibc</summary>
    <description>glibc</description>
    <size package="200"/>
    <location href="Packages/g/glibc-2.40-1.fc43.x86_64.rpm"/>
    <format>
      <rpm:provides>
        <rpm:entry name="libc.so.6()(64bit)"/>
      </rpm:provides>
    </format>
  </package>
</metadata>
"#;

        let packages = parser
            .parse_primary_xml(xml, "https://example.com")
            .unwrap();
        let pkg = &packages[0];

        let soname = pkg
            .provides
            .iter()
            .find(|p| p.name == "libc.so.6()(64bit)")
            .expect("soname provide not found");
        assert_eq!(soname.kind, RepositoryCapabilityKind::Soname);
        assert!(soname.version.is_none());
    }

    #[test]
    fn test_implicit_self_provide() {
        let builder = valid_builder();
        let pkg = builder.build("https://example.com").unwrap();

        let self_provide = pkg
            .provides
            .iter()
            .find(|p| p.name == "test-package" && p.kind == RepositoryCapabilityKind::PackageName)
            .expect("implicit self-provide not found");
        assert_eq!(self_provide.version.as_deref(), Some("1:2.3.4-5.fc43"));
    }

    #[test]
    fn test_rich_dep_or_parsed_into_alternatives() {
        let parser = FedoraParser::new("x86_64".to_string());
        let xml = r#"
<metadata xmlns:rpm="http://linux.duke.edu/metadata/rpm">
  <package type="rpm">
    <name>test-pkg</name>
    <arch>x86_64</arch>
    <version epoch="0" ver="1.0" rel="1.fc43"/>
    <checksum type="sha256">beef1234</checksum>
    <summary>Test</summary>
    <description>Test</description>
    <size package="100"/>
    <location href="Packages/t/test-pkg-1.0-1.fc43.x86_64.rpm"/>
    <format>
      <rpm:requires>
        <rpm:entry name="(foo or bar)"/>
        <rpm:entry name="(foo if bar)"/>
        <rpm:entry name="(a or b or c)"/>
      </rpm:requires>
    </format>
  </package>
</metadata>
"#;

        let packages = parser
            .parse_primary_xml(xml, "https://example.com")
            .unwrap();
        let pkg = &packages[0];

        assert_eq!(pkg.requirements.len(), 3);

        // `(foo or bar)` should be parsed into an OR-group with two alternatives
        let or_group = &pkg.requirements[0];
        assert_eq!(or_group.behavior, ConditionalRequirementBehavior::Hard);
        assert_eq!(or_group.alternatives.len(), 2);
        assert_eq!(or_group.alternatives[0].name, "foo");
        assert_eq!(or_group.alternatives[1].name, "bar");

        // `(foo if bar)` should remain conditional (not decomposed)
        let conditional = &pkg.requirements[1];
        assert_eq!(
            conditional.behavior,
            ConditionalRequirementBehavior::Conditional
        );
        assert_eq!(conditional.alternatives.len(), 1);
        assert_eq!(conditional.alternatives[0].name, "(foo if bar)");

        // `(a or b or c)` should be parsed into a 3-way OR-group
        let triple = &pkg.requirements[2];
        assert_eq!(triple.behavior, ConditionalRequirementBehavior::Hard);
        assert_eq!(triple.alternatives.len(), 3);
        assert_eq!(triple.alternatives[0].name, "a");
        assert_eq!(triple.alternatives[1].name, "b");
        assert_eq!(triple.alternatives[2].name, "c");
    }

    #[test]
    fn test_parse_rich_dep_or_helper() {
        // Simple binary OR
        assert_eq!(
            parse_rich_dep_or("(foo or bar)"),
            Some(vec!["foo".to_string(), "bar".to_string()])
        );
        // Triple OR
        assert_eq!(
            parse_rich_dep_or("(a or b or c)"),
            Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
        );
        // Conditional -- not an OR
        assert_eq!(parse_rich_dep_or("(foo if bar)"), None);
        // Unless -- not an OR
        assert_eq!(parse_rich_dep_or("(foo unless bar)"), None);
        // Nested -- not safe to decompose
        assert_eq!(parse_rich_dep_or("((foo or bar) if baz)"), None);
        // Not a rich dep at all
        assert_eq!(parse_rich_dep_or("foo"), None);
    }
}
