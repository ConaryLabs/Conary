// conary-core/src/repository/parsers/arch.rs

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
use tracing::{debug, info, warn};

/// Maximum allowed package size (5 GB)
const MAX_PACKAGE_SIZE: u64 = 5 * 1024 * 1024 * 1024;

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

    fn package_from_fields(
        &self,
        repo_url: &str,
        desc_fields: &HashMap<String, Vec<String>>,
        depends_content: Option<&String>,
    ) -> Result<PackageMetadata> {
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

        if size > MAX_PACKAGE_SIZE {
            return Err(Error::ParseError(format!(
                "Package {} size {} exceeds maximum allowed (5GB)",
                name, size
            )));
        }

        let architecture = desc_fields.get("ARCH").and_then(|v| v.first()).cloned();
        let description = desc_fields.get("DESC").and_then(|v| v.first()).cloned();

        if filename.contains("..") || filename.starts_with('/') || filename.contains("://") {
            return Err(Error::ParseError(format!(
                "Suspicious filename in Arch database: {}",
                filename
            )));
        }

        let download_url = format!("{}/{}", repo_url.trim_end_matches('/'), filename);

        let mut extra = serde_json::Map::new();
        if let Some(url) = desc_fields.get("URL").and_then(|v| v.first()) {
            extra.insert(
                "homepage".to_string(),
                serde_json::Value::String(url.clone()),
            );
        }
        if let Some(license) = desc_fields.get("LICENSE").and_then(|v| v.first()) {
            extra.insert(
                "license".to_string(),
                serde_json::Value::String(license.clone()),
            );
        }
        if let Some(builddate) = desc_fields.get("BUILDDATE").and_then(|v| v.first()) {
            extra.insert(
                "builddate".to_string(),
                serde_json::Value::String(builddate.clone()),
            );
        }
        if let Some(installed_size_str) = desc_fields.get("ISIZE").and_then(|v| v.first()) {
            extra.insert(
                "installed_size".to_string(),
                serde_json::Value::String(installed_size_str.clone()),
            );
        }
        if let Some(provides) = desc_fields.get("PROVIDES") {
            extra.insert(
                "arch_provides".to_string(),
                serde_json::Value::Array(
                    provides
                        .iter()
                        .cloned()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            );
        }
        extra.insert(
            "format".to_string(),
            serde_json::Value::String("arch".to_string()),
        );

        let dependencies = depends_content
            .map(|content| self.parse_depends_file(content))
            .unwrap_or_default();

        Ok(PackageMetadata {
            name,
            version,
            architecture,
            description,
            checksum,
            checksum_type: ChecksumType::Sha256,
            size,
            download_url,
            dependencies,
            extra_metadata: serde_json::Value::Object(extra),
            source_distro: None,
            version_scheme: None,
            requirements: Vec::new(),
            provides: Vec::new(),
        })
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

        // Single-pass: collect desc and depends data keyed by directory name.
        // Directory names in .db.tar.gz are "{name}-{version}-{pkgrel}/".
        let mut archive = Archive::new(decompressed.as_slice());
        let mut desc_data: HashMap<String, String> = HashMap::new();
        let mut depends_data: HashMap<String, String> = HashMap::new();

        for entry in archive.entries()? {
            let mut entry = entry
                .map_err(|e| Error::ParseError(format!("Failed to read tarball entry: {}", e)))?;

            let path = entry
                .path()
                .map_err(|e| Error::ParseError(format!("Invalid path in tarball: {}", e)))?;

            let path_str = path.to_string_lossy().to_string();

            if let Some(dir) = path_str.split('/').next() {
                let dir_key = dir.to_string();

                if path_str.ends_with("/desc") {
                    let mut content = String::new();
                    entry.read_to_string(&mut content).map_err(|e| {
                        Error::ParseError(format!("Failed to read desc file: {}", e))
                    })?;
                    desc_data.insert(dir_key, content);
                } else if path_str.ends_with("/depends") {
                    let mut content = String::new();
                    entry.read_to_string(&mut content).map_err(|e| {
                        Error::ParseError(format!("Failed to read depends file: {}", e))
                    })?;
                    depends_data.insert(dir_key, content);
                }
            }
        }

        // Build packages from collected data
        let mut packages = Vec::new();
        for (dir_key, desc_content) in &desc_data {
            let desc_fields = self.parse_desc_file(desc_content);

            match self.package_from_fields(repo_url, &desc_fields, depends_data.get(dir_key)) {
                Ok(package) => packages.push(package),
                Err(Error::ParseError(message))
                    if message.contains("exceeds maximum allowed (5GB)") =>
                {
                    warn!("{}", message);
                }
                Err(err) => return Err(err),
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
        let content =
            "%NAME%\nbash\n\n%VERSION%\n5.2.037-1\n\n%DESC%\nThe GNU Bourne Again shell\n";

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

    #[test]
    fn test_parse_desc_file_provides_persisted_in_extra_metadata() {
        let parser = ArchParser::new("core".to_string());
        let desc = "\
%NAME%
mailer

%VERSION%
1.0-1

%FILENAME%
mailer-1.0-1-x86_64.pkg.tar.zst

%SHA256SUM%
deadbeef

%CSIZE%
123

%ARCH%
x86_64

%PROVIDES%
mail-transport-agent
smtp-server=1.0
";

        let fields = parser.parse_desc_file(desc);
        let package = parser
            .package_from_fields("https://example.test", &fields, None)
            .unwrap();
        let metadata = package.extra_metadata.as_object().unwrap();
        let provides = metadata
            .get("arch_provides")
            .and_then(|value| value.as_array())
            .unwrap();
        let provides: Vec<&str> = provides.iter().filter_map(|value| value.as_str()).collect();

        assert!(provides.contains(&"mail-transport-agent"));
        assert!(provides.contains(&"smtp-server=1.0"));
    }
}
