// src/repository/parsers/arch.rs

//! Arch Linux repository metadata parser
//!
//! Parses Arch Linux .db.tar.gz files which contain package metadata
//! in a custom text format with %FIELD% markers.

use super::{ChecksumType, Dependency, PackageMetadata, RepositoryParser};
use crate::error::{Error, Result};
use crate::repository::client::RepositoryClient;
use std::collections::HashMap;
use std::io::Read;
use tar::Archive;
use tracing::{debug, info};

/// Arch Linux repository parser
pub struct ArchParser {
    /// Repository name (e.g., "core", "extra", "community")
    repo_name: String,
}

impl ArchParser {
    /// Create a new Arch Linux parser for a specific repository
    pub fn new(repo_name: String) -> Self {
        Self { repo_name }
    }

    /// Download and decompress the repository database
    ///
    /// Uses RepositoryClient for HTTP and the compression module for auto-decompression.
    fn download_database(&self, repo_url: &str) -> Result<Vec<u8>> {
        let db_url = format!("{}/{}.db", repo_url.trim_end_matches('/'), self.repo_name);
        debug!("Downloading Arch database from: {}", db_url);

        let client = RepositoryClient::new()?;
        client.fetch_and_decompress(&db_url)
    }

    /// Parse a desc file from the tarball
    fn parse_desc_file(&self, content: &str) -> HashMap<String, Vec<String>> {
        let mut fields = HashMap::new();
        let mut current_field: Option<String> = None;
        let mut values: Vec<String> = Vec::new();

        for line in content.lines() {
            let trimmed = line.trim();

            if trimmed.starts_with('%') && trimmed.ends_with('%') {
                // Save previous field
                if let Some(field) = current_field.take() {
                    fields.insert(field, values.clone());
                    values.clear();
                }

                // Start new field
                current_field = Some(trimmed[1..trimmed.len() - 1].to_string());
            } else if !trimmed.is_empty() {
                // Add value to current field
                values.push(trimmed.to_string());
            }
        }

        // Save last field
        if let Some(field) = current_field {
            fields.insert(field, values);
        }

        fields
    }

    /// Parse dependencies from depends file
    fn parse_depends_file(&self, content: &str) -> Vec<Dependency> {
        let fields = self.parse_desc_file(content);
        let mut dependencies = Vec::new();

        // Runtime dependencies
        if let Some(deps) = fields.get("DEPENDS") {
            for dep in deps {
                let (name, constraint) = self.parse_dependency_string(dep);
                dependencies.push(Dependency::runtime_versioned(name, constraint));
            }
        }

        // Optional dependencies
        if let Some(opts) = fields.get("OPTDEPENDS") {
            for opt in opts {
                // Format: "package: description" or just "package"
                if let Some((pkg, desc)) = opt.split_once(':') {
                    let (name, _) = self.parse_dependency_string(pkg.trim());
                    dependencies.push(Dependency::optional(name, Some(desc.trim().to_string())));
                } else {
                    let (name, _) = self.parse_dependency_string(opt);
                    dependencies.push(Dependency::optional(name, None));
                }
            }
        }

        dependencies
    }

    /// Parse dependency string into name and constraint
    /// Format: "package>=1.0" or "package=1.0" or "package<2.0" or just "package"
    fn parse_dependency_string(&self, dep: &str) -> (String, String) {
        for op in &[">=", "<=", "=", "<", ">"] {
            if let Some(pos) = dep.find(op) {
                let name = dep[..pos].to_string();
                let version = dep[pos..].to_string();
                return (name, version);
            }
        }

        // No version constraint
        (dep.to_string(), String::new())
    }
}

impl RepositoryParser for ArchParser {
    fn sync_metadata(&self, repo_url: &str) -> Result<Vec<PackageMetadata>> {
        info!("Syncing Arch Linux repository: {}", self.repo_name);

        // Download and decompress database (handled by RepositoryClient)
        let decompressed = self.download_database(repo_url)?;

        // Extract tarball
        let mut archive = Archive::new(decompressed.as_slice());
        let mut packages = Vec::new();

        // Iterate through tarball entries
        for entry in archive.entries()? {
            let mut entry = entry.map_err(|e| {
                Error::ParseError(format!("Failed to read tarball entry: {}", e))
            })?;

            let path = entry
                .path()
                .map_err(|e| Error::ParseError(format!("Invalid path in tarball: {}", e)))?;

            let path_str = path.to_string_lossy();

            // Each package has a directory with desc and depends files
            if path_str.ends_with("/desc") {
                let mut content = String::new();
                entry.read_to_string(&mut content).map_err(|e| {
                    Error::ParseError(format!("Failed to read desc file: {}", e))
                })?;

                let desc_fields = self.parse_desc_file(&content);

                // Extract required fields
                let name = desc_fields
                    .get("NAME")
                    .and_then(|v| v.first())
                    .ok_or_else(|| Error::ParseError("Missing %NAME% field".to_string()))?
                    .clone();

                let version = desc_fields
                    .get("VERSION")
                    .and_then(|v| v.first())
                    .ok_or_else(|| Error::ParseError("Missing %VERSION% field".to_string()))?
                    .clone();

                let filename = desc_fields
                    .get("FILENAME")
                    .and_then(|v| v.first())
                    .ok_or_else(|| Error::ParseError("Missing %FILENAME% field".to_string()))?
                    .clone();

                let checksum = desc_fields
                    .get("SHA256SUM")
                    .and_then(|v| v.first())
                    .ok_or_else(|| Error::ParseError("Missing %SHA256SUM% field".to_string()))?
                    .clone();

                let size: u64 = desc_fields
                    .get("CSIZE")
                    .and_then(|v| v.first())
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| Error::ParseError("Missing or invalid %CSIZE% field".to_string()))?;

                let architecture = desc_fields.get("ARCH").and_then(|v| v.first()).cloned();

                let description = desc_fields.get("DESC").and_then(|v| v.first()).cloned();

                // Build download URL
                let download_url = format!("{}/{}", repo_url.trim_end_matches('/'), filename);

                // Build extra metadata
                let mut extra = serde_json::Map::new();
                if let Some(url) = desc_fields.get("URL").and_then(|v| v.first()) {
                    extra.insert("homepage".to_string(), serde_json::Value::String(url.clone()));
                }
                if let Some(license) = desc_fields.get("LICENSE").and_then(|v| v.first()) {
                    extra.insert("license".to_string(), serde_json::Value::String(license.clone()));
                }
                if let Some(builddate) = desc_fields.get("BUILDDATE").and_then(|v| v.first()) {
                    extra.insert("builddate".to_string(), serde_json::Value::String(builddate.clone()));
                }
                if let Some(isize) = desc_fields.get("ISIZE").and_then(|v| v.first()) {
                    extra.insert("installed_size".to_string(), serde_json::Value::String(isize.clone()));
                }
                extra.insert("format".to_string(), serde_json::Value::String("arch".to_string()));

                let package = PackageMetadata {
                    name,
                    version,
                    architecture,
                    description,
                    checksum,
                    checksum_type: ChecksumType::Sha256,
                    size,
                    download_url,
                    dependencies: Vec::new(), // Will be populated if depends file exists
                    extra_metadata: serde_json::Value::Object(extra),
                };

                packages.push(package);
            }
        }

        // Second pass: parse depends files and update dependencies
        let mut archive = Archive::new(decompressed.as_slice());
        let mut package_deps: HashMap<String, Vec<Dependency>> = HashMap::new();

        for entry in archive.entries()? {
            let mut entry = entry.map_err(|e| {
                Error::ParseError(format!("Failed to read tarball entry: {}", e))
            })?;

            let path = entry
                .path()
                .map_err(|e| Error::ParseError(format!("Invalid path in tarball: {}", e)))?;

            let path_str = path.to_string_lossy();

            if path_str.ends_with("/depends") {
                // Extract package name from path (e.g., "bash-5.2.037-1/depends" -> "bash")
                let pkg_name = path_str
                    .split('/')
                    .next()
                    .and_then(|s| s.split('-').next())
                    .unwrap_or("")
                    .to_string();

                let mut content = String::new();
                entry.read_to_string(&mut content).map_err(|e| {
                    Error::ParseError(format!("Failed to read depends file: {}", e))
                })?;

                let deps = self.parse_depends_file(&content);
                package_deps.insert(pkg_name, deps);
            }
        }

        // Update packages with their dependencies
        for pkg in &mut packages {
            if let Some(deps) = package_deps.remove(&pkg.name) {
                pkg.dependencies = deps;
            }
        }

        info!("Parsed {} packages from Arch repository", packages.len());
        Ok(packages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_desc_file() {
        let parser = ArchParser::new("core".to_string());
        let content = "%NAME%\nbash\n\n%VERSION%\n5.2.037-1\n\n%DESC%\nThe GNU Bourne Again shell\n";

        let fields = parser.parse_desc_file(content);

        assert_eq!(fields.get("NAME"), Some(&vec!["bash".to_string()]));
        assert_eq!(fields.get("VERSION"), Some(&vec!["5.2.037-1".to_string()]));
        assert_eq!(
            fields.get("DESC"),
            Some(&vec!["The GNU Bourne Again shell".to_string()])
        );
    }

    #[test]
    fn test_parse_dependency_string() {
        let parser = ArchParser::new("core".to_string());

        let (name, constraint) = parser.parse_dependency_string("glibc>=2.17");
        assert_eq!(name, "glibc");
        assert_eq!(constraint, ">=2.17");

        let (name2, constraint2) = parser.parse_dependency_string("readline");
        assert_eq!(name2, "readline");
        assert_eq!(constraint2, "");
    }
}
