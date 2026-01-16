// src/repository/parsers/debian.rs

//! Debian/Ubuntu repository metadata parser
//!
//! Parses Debian-style Packages.gz files which use RFC 822-like format
//! (similar to email headers with key: value pairs).

use super::{ChecksumType, Dependency, PackageMetadata, RepositoryParser};
use crate::error::{Error, Result};
use crate::repository::client::RepositoryClient;
use serde::Deserialize;
use tracing::{debug, info};

/// Debian/Ubuntu repository parser
pub struct DebianParser {
    /// Distribution name (e.g., "noble", "jammy")
    distribution: String,
    /// Component (e.g., "main", "universe")
    component: String,
    /// Architecture (e.g., "amd64", "arm64")
    architecture: String,
}

impl DebianParser {
    /// Create a new Debian/Ubuntu parser
    pub fn new(distribution: String, component: String, architecture: String) -> Self {
        Self {
            distribution,
            component,
            architecture,
        }
    }

    /// Download and decompress the Packages file
    ///
    /// Uses RepositoryClient for HTTP and the compression module for auto-decompression.
    fn download_packages_file(&self, repo_url: &str) -> Result<String> {
        let packages_url = format!(
            "{}/dists/{}/{}/binary-{}/Packages.gz",
            repo_url.trim_end_matches('/'),
            self.distribution,
            self.component,
            self.architecture
        );

        debug!("Downloading Debian Packages file from: {}", packages_url);

        let client = RepositoryClient::new()?;
        let content = client.fetch_and_decompress_string(&packages_url)?;

        debug!("Decompressed Packages file: {} bytes", content.len());
        Ok(content)
    }

    /// Parse dependencies from Depends field
    /// Format: "libc6 (>= 2.34), package (= 1.0-1), other | alternative"
    fn parse_dependencies(&self, depends_str: &str) -> Vec<Dependency> {
        let mut dependencies = Vec::new();

        for dep_group in depends_str.split(',') {
            let dep_group = dep_group.trim();

            // Handle alternatives (pkg1 | pkg2) - take first alternative
            let dep = if let Some(first_alt) = dep_group.split('|').next() {
                first_alt.trim()
            } else {
                dep_group
            };

            // Parse package name and version constraint
            if let Some((name, constraint)) = self.parse_dependency(dep) {
                dependencies.push(Dependency::runtime_versioned(name, constraint));
            }
        }

        dependencies
    }

    /// Parse a single dependency string
    /// Format: "package (>= 1.0)" or "package (= 1.0-1)" or "package"
    fn parse_dependency(&self, dep: &str) -> Option<(String, String)> {
        if let Some(paren_pos) = dep.find('(') {
            let name = dep[..paren_pos].trim().to_string();
            let constraint = dep[paren_pos + 1..]
                .trim_end_matches(')')
                .trim()
                .to_string();
            Some((name, constraint))
        } else {
            // No version constraint
            Some((dep.trim().to_string(), String::new()))
        }
    }
}

/// Debian package entry structure for rfc822-like parsing
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DebianPackageEntry {
    package: String,
    version: String,
    architecture: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(rename = "SHA256")]
    sha256: String,
    size: String,
    filename: String,
    #[serde(default)]
    depends: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    section: Option<String>,
    #[serde(rename = "Installed-Size", default)]
    installed_size: Option<String>,
}

impl RepositoryParser for DebianParser {
    fn sync_metadata(&self, repo_url: &str) -> Result<Vec<PackageMetadata>> {
        info!(
            "Syncing Debian repository: {}/{}/{}",
            self.distribution, self.component, self.architecture
        );

        // Download and decompress Packages file
        let packages_content = self.download_packages_file(repo_url)?;

        // Parse RFC 822-like format
        let entries: Vec<DebianPackageEntry> = rfc822_like::from_str(&packages_content)
            .map_err(|e| Error::ParseError(format!("Failed to parse Packages file: {}", e)))?;

        debug!("Parsed {} package entries", entries.len());

        // Convert to PackageMetadata
        let mut packages = Vec::new();
        for entry in entries {
            let size: u64 = entry
                .size
                .parse()
                .map_err(|e| Error::ParseError(format!("Invalid size '{}': {}", entry.size, e)))?;

            // Parse dependencies
            let dependencies = if let Some(deps) = &entry.depends {
                self.parse_dependencies(deps)
            } else {
                Vec::new()
            };

            // Build download URL
            let download_url = format!("{}/{}", repo_url.trim_end_matches('/'), entry.filename);

            // Build extra metadata
            let mut extra = serde_json::Map::new();
            if let Some(homepage) = entry.homepage {
                extra.insert("homepage".to_string(), serde_json::Value::String(homepage));
            }
            if let Some(section) = entry.section {
                extra.insert("section".to_string(), serde_json::Value::String(section));
            }
            if let Some(installed_size) = entry.installed_size {
                extra.insert(
                    "installed_size".to_string(),
                    serde_json::Value::String(installed_size),
                );
            }
            extra.insert("format".to_string(), serde_json::Value::String("deb".to_string()));
            extra.insert(
                "distribution".to_string(),
                serde_json::Value::String(self.distribution.clone()),
            );
            extra.insert(
                "component".to_string(),
                serde_json::Value::String(self.component.clone()),
            );

            let package = PackageMetadata {
                name: entry.package,
                version: entry.version,
                architecture: Some(entry.architecture),
                description: entry.description,
                checksum: entry.sha256,
                checksum_type: ChecksumType::Sha256,
                size,
                download_url,
                dependencies,
                extra_metadata: serde_json::Value::Object(extra),
            };

            packages.push(package);
        }

        info!("Parsed {} packages from Debian repository", packages.len());
        Ok(packages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dependency() {
        let parser = DebianParser::new("noble".to_string(), "main".to_string(), "amd64".to_string());

        let (name, constraint) = parser.parse_dependency("libc6 (>= 2.34)").unwrap();
        assert_eq!(name, "libc6");
        assert_eq!(constraint, ">= 2.34");

        let (name2, constraint2) = parser.parse_dependency("bash").unwrap();
        assert_eq!(name2, "bash");
        assert_eq!(constraint2, "");
    }

    #[test]
    fn test_parse_dependencies() {
        let parser = DebianParser::new("noble".to_string(), "main".to_string(), "amd64".to_string());

        let deps = parser.parse_dependencies("libc6 (>= 2.34), bash (= 5.2-1), coreutils");
        assert_eq!(deps.len(), 3);
        assert_eq!(deps[0].name, "libc6");
        assert_eq!(deps[1].name, "bash");
        assert_eq!(deps[2].name, "coreutils");
    }

    #[test]
    fn test_parse_alternatives() {
        let parser = DebianParser::new("noble".to_string(), "main".to_string(), "amd64".to_string());

        // Should take first alternative
        let deps = parser.parse_dependencies("package-a | package-b, other-package");
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].name, "package-a");
        assert_eq!(deps[1].name, "other-package");
    }
}
