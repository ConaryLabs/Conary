// src/repository/parsers/fedora.rs

//! Fedora/RPM repository metadata parser
//!
//! Parses Fedora-style repomd.xml and primary.xml files which contain
//! RPM package metadata in XML format.

use super::{ChecksumType, Dependency, PackageMetadata, RepositoryParser};
use crate::error::{Error, Result};
use flate2::read::GzDecoder;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::Read;
use tracing::{debug, info};

/// Fedora/RPM repository parser
pub struct FedoraParser {
    /// Repository architecture (e.g., "x86_64", "aarch64")
    architecture: String,
}

impl FedoraParser {
    /// Create a new Fedora/RPM parser
    pub fn new(architecture: String) -> Self {
        Self { architecture }
    }

    /// Download repomd.xml and find primary.xml location
    fn get_primary_xml_location(&self, repo_url: &str) -> Result<String> {
        let repomd_url = format!("{}/repodata/repomd.xml", repo_url.trim_end_matches('/'));
        debug!("Downloading repomd.xml from: {}", repomd_url);

        let response = reqwest::blocking::get(&repomd_url)
            .map_err(|e| Error::DownloadError(format!("Failed to download {}: {}", repomd_url, e)))?;

        if !response.status().is_success() {
            return Err(Error::DownloadError(format!(
                "Failed to download {}: HTTP {}",
                repomd_url,
                response.status()
            )));
        }

        let xml_content = response
            .text()
            .map_err(|e| Error::DownloadError(format!("Failed to read repomd.xml: {}", e)))?;

        // Parse repomd.xml to find primary location
        let mut reader = Reader::from_str(&xml_content);
        reader.trim_text(true);

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
                    })
                        && attr.value.as_ref() == b"primary"
                    {
                        in_primary = true;
                    }
                }
                Ok(Event::Start(e) | Event::Empty(e)) if e.name().as_ref() == b"location" && in_primary => {
                    // Extract href attribute
                    if let Some(Ok(attr)) = e.attributes().find(|a| {
                        a.as_ref()
                            .map(|attr| attr.key.as_ref() == b"href")
                            .unwrap_or(false)
                    }) {
                        location = Some(
                            String::from_utf8_lossy(attr.value.as_ref()).to_string(),
                        );
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
                    )))
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
    fn download_primary_xml(&self, repo_url: &str, location: &str) -> Result<String> {
        let primary_url = format!("{}/{}", repo_url.trim_end_matches('/'), location);
        debug!("Downloading primary.xml from: {}", primary_url);

        let response = reqwest::blocking::get(&primary_url).map_err(|e| {
            Error::DownloadError(format!("Failed to download {}: {}", primary_url, e))
        })?;

        if !response.status().is_success() {
            return Err(Error::DownloadError(format!(
                "Failed to download {}: HTTP {}",
                primary_url,
                response.status()
            )));
        }

        let bytes = response
            .bytes()
            .map_err(|e| Error::DownloadError(format!("Failed to read response: {}", e)))?;

        // Detect compression format from location extension
        let decompressed = if location.ends_with(".zst") {
            // Decompress zstd
            debug!("Decompressing zstd-compressed primary.xml");
            let decompressed_bytes = zstd::decode_all(bytes.as_ref())
                .map_err(|e| Error::ParseError(format!("Failed to decompress primary.xml.zst: {}", e)))?;
            String::from_utf8(decompressed_bytes)
                .map_err(|e| Error::ParseError(format!("Invalid UTF-8 in primary.xml: {}", e)))?
        } else {
            // Try gzip decompression (default)
            debug!("Decompressing gzip-compressed primary.xml");
            let mut gz = GzDecoder::new(bytes.as_ref());
            let mut decompressed = String::new();
            gz.read_to_string(&mut decompressed)
                .map_err(|e| Error::ParseError(format!("Failed to decompress primary.xml.gz: {}", e)))?;
            decompressed
        };

        debug!("Decompressed primary.xml: {} bytes", decompressed.len());
        Ok(decompressed)
    }

    /// Parse primary.xml and extract package metadata
    fn parse_primary_xml(&self, xml_content: &str, base_url: &str) -> Result<Vec<PackageMetadata>> {
        let mut reader = Reader::from_str(xml_content);
        reader.trim_text(true);

        let mut packages = Vec::new();
        let mut buf = Vec::new();

        // Current package being built
        let mut current_package: Option<PackageBuilder> = None;
        let mut current_tag = String::new();
        let mut in_format = false;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    current_tag = tag_name.clone();

                    match tag_name.as_str() {
                        "package" => {
                            current_package = Some(PackageBuilder::new());
                        }
                        "format" => {
                            in_format = true;
                        }
                        _ => {}
                    }
                }
                Ok(Event::Empty(e)) => {
                    let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();

                    match tag_name.as_str() {
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
                                    if key.as_ref() == "type" {
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
                                    if key.as_ref() == "package" {
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
                                    if key.as_ref() == "href" {
                                        let value = String::from_utf8_lossy(&attr.value);
                                        pkg.location = Some(value.to_string());
                                    }
                                }
                            }
                        }
                        "format" => {
                            in_format = true;
                        }
                        "entry" if in_format => {
                            // This is a dependency entry within <rpm:requires>
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

                                if let Some(name) = dep_name {
                                    // Skip rpmlib and file dependencies
                                    if !name.starts_with("rpmlib(") && !name.starts_with('/') {
                                        let constraint = match (dep_flags, dep_ver) {
                                            (Some(flags), Some(ver)) => {
                                                let op = match flags.as_str() {
                                                    "GE" => ">=",
                                                    "LE" => "<=",
                                                    "EQ" => "=",
                                                    "LT" => "<",
                                                    "GT" => ">",
                                                    _ => "",
                                                };
                                                if op.is_empty() {
                                                    String::new()
                                                } else {
                                                    format!("{} {}", op, ver)
                                                }
                                            }
                                            _ => String::new(),
                                        };
                                        pkg.dependencies.push((name, constraint));
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::Text(e)) => {
                    if let Some(ref mut pkg) = current_package {
                        let text = e.unescape().unwrap_or_default().to_string();
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
                    if tag_name == "package"
                        && let Some(builder) = current_package.take()
                        && let Ok(pkg) = builder.build(base_url)
                    {
                        packages.push(pkg);
                    } else if tag_name == "format" {
                        in_format = false;
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => {
                    return Err(Error::ParseError(format!("Failed to parse primary.xml: {}", e)))
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

        let location = self
            .location
            .ok_or_else(|| Error::ParseError("Missing location".to_string()))?;

        let download_url = format!("{}/{}", base_url.trim_end_matches('/'), location);

        let checksum_type = match self.checksum_type.as_deref() {
            Some("sha256") => ChecksumType::Sha256,
            Some("sha512") => ChecksumType::Sha512,
            _ => ChecksumType::Sha256, // Default
        };

        // Convert dependencies
        let dependencies = self
            .dependencies
            .into_iter()
            .map(|(name, constraint)| {
                if constraint.is_empty() {
                    Dependency::runtime(name)
                } else {
                    Dependency::runtime_versioned(name, constraint)
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
        extra.insert("format".to_string(), serde_json::Value::String("rpm".to_string()));
        extra.insert("epoch".to_string(), serde_json::Value::String(epoch));

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
        })
    }
}

impl RepositoryParser for FedoraParser {
    fn sync_metadata(&self, repo_url: &str) -> Result<Vec<PackageMetadata>> {
        info!("Syncing Fedora repository for {}", self.architecture);

        // Get primary.xml location from repomd.xml
        let primary_location = self.get_primary_xml_location(repo_url)?;

        // Download and decompress primary.xml
        let primary_xml = self.download_primary_xml(repo_url, &primary_location)?;

        // Parse primary.xml
        let packages = self.parse_primary_xml(&primary_xml, repo_url)?;

        info!("Parsed {} packages from Fedora repository", packages.len());
        Ok(packages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_builder() {
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

        let pkg = builder.build("https://example.com").unwrap();
        assert_eq!(pkg.name, "test-package");
        assert_eq!(pkg.version, "1:2.3.4-5.fc43");
        assert_eq!(pkg.size, 1024);
    }
}
