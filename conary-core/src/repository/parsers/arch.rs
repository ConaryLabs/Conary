// conary-core/src/repository/parsers/arch.rs

//! Arch Linux repository metadata parser
//!
//! Parses Arch Linux .db.tar.gz files which contain package metadata
//! in a custom text format with %FIELD% markers.

use super::{ChecksumType, Dependency, PackageMetadata, RepositoryParser};
use crate::error::{Error, Result};
use crate::repository::client::RepositoryClient;
use crate::repository::dependency_model::{
    RepositoryCapabilityKind, RepositoryDependencyFlavor, RepositoryProvide,
    RepositoryRequirementClause, RepositoryRequirementGroup, RepositoryRequirementKind,
};
use crate::repository::versioning::VersionScheme;
use std::collections::HashMap;
use std::io::Read;
use tar::Archive;
use tracing::{debug, info, warn};

use super::common::{self, MAX_PACKAGE_SIZE};

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
    async fn download_database(&self, repo_url: &str) -> Result<Vec<u8>> {
        let db_url = format!("{}/{}.db", repo_url.trim_end_matches('/'), self.repo_name);
        debug!("Downloading Arch database from: {}", db_url);

        let client = RepositoryClient::new()?;
        client.fetch_and_decompress(&db_url).await
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

    /// Classify an Arch provide entry.
    fn classify_arch_provide(name: &str) -> RepositoryCapabilityKind {
        if name.contains(".so") {
            RepositoryCapabilityKind::Soname
        } else {
            RepositoryCapabilityKind::Virtual
        }
    }

    /// Build structured requirement groups from a depends file.
    fn parse_structured_depends(&self, content: &str) -> Vec<RepositoryRequirementGroup> {
        let fields = self.parse_desc_file(content);
        let mut groups = Vec::new();

        if let Some(deps) = fields.get("DEPENDS") {
            for dep in deps {
                let (name, constraint) = self.parse_dependency_string(dep);
                let clause = if constraint.is_empty() {
                    RepositoryRequirementClause::name_only(name)
                } else {
                    RepositoryRequirementClause::versioned(name, constraint)
                };
                groups.push(
                    RepositoryRequirementGroup::simple(RepositoryRequirementKind::Depends, clause)
                        .with_native_text(dep.clone()),
                );
            }
        }

        if let Some(opts) = fields.get("OPTDEPENDS") {
            for opt in opts {
                let (pkg_name, desc) = if let Some((pkg, d)) = opt.split_once(':') {
                    let (name, _) = self.parse_dependency_string(pkg.trim());
                    (name, Some(d.trim().to_string()))
                } else {
                    let (name, _) = self.parse_dependency_string(opt);
                    (name, None)
                };
                groups.push(RepositoryRequirementGroup::optional(
                    RepositoryRequirementClause::name_only(pkg_name),
                    desc,
                ));
            }
        }

        groups
    }

    /// Build structured provides from desc fields.
    fn build_structured_provides(
        &self,
        name: &str,
        version: &str,
        desc_fields: &HashMap<String, Vec<String>>,
    ) -> Vec<RepositoryProvide> {
        let mut provides = vec![RepositoryProvide::package_name(
            name.to_string(),
            Some(version.to_string()),
        )];

        if let Some(prov_list) = desc_fields.get("PROVIDES") {
            for prov in prov_list {
                let (prov_name, prov_constraint) = self.parse_dependency_string(prov);
                let kind = if prov_name == name {
                    RepositoryCapabilityKind::PackageName
                } else {
                    Self::classify_arch_provide(&prov_name)
                };

                let prov_version = common::extract_version_from_constraint(&prov_constraint);

                provides.push(RepositoryProvide {
                    name: prov_name,
                    kind,
                    version: prov_version,
                    native_text: Some(prov.clone()),
                });
            }
        }

        provides
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

        if let Err(msg) = common::validate_filename(&filename) {
            return Err(Error::ParseError(msg));
        }

        let download_url = common::join_repo_url(repo_url, &filename);

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

        let requirements = depends_content
            .map(|content| self.parse_structured_depends(content))
            .unwrap_or_default();

        let structured_provides = self.build_structured_provides(&name, &version, desc_fields);

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
            source_distro: Some(RepositoryDependencyFlavor::Arch),
            version_scheme: Some(VersionScheme::Arch),
            requirements,
            provides: structured_provides,
        })
    }

    /// Parse dependency string into name and constraint
    /// Format: "package>=1.0" or "package=1.0" or "package<2.0" or just "package"
    fn parse_dependency_string(&self, dep: &str) -> (String, String) {
        common::split_dependency(dep)
    }
}

impl RepositoryParser for ArchParser {
    async fn sync_metadata(&self, repo_url: &str) -> Result<Vec<PackageMetadata>> {
        info!("Syncing Arch Linux repository: {}", self.repo_name);

        // Download and decompress database (handled by RepositoryClient)
        let decompressed = self.download_database(repo_url).await?;

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

    #[test]
    fn test_source_distro_and_version_scheme() {
        let parser = ArchParser::new("core".to_string());
        let desc = "\
%NAME%
bash

%VERSION%
5.2.037-1

%FILENAME%
bash-5.2.037-1-x86_64.pkg.tar.zst

%SHA256SUM%
deadbeef

%CSIZE%
123

%ARCH%
x86_64
";
        let fields = parser.parse_desc_file(desc);
        let pkg = parser
            .package_from_fields("https://example.test", &fields, None)
            .unwrap();

        assert_eq!(pkg.source_distro, Some(RepositoryDependencyFlavor::Arch));
        assert_eq!(pkg.version_scheme, Some(VersionScheme::Arch));
    }

    #[test]
    fn test_structured_versioned_depends() {
        let parser = ArchParser::new("core".to_string());
        let desc = "\
%NAME%
bash

%VERSION%
5.2.037-1

%FILENAME%
bash-5.2.037-1-x86_64.pkg.tar.zst

%SHA256SUM%
deadbeef

%CSIZE%
123

%ARCH%
x86_64
";
        let depends = "\
%DEPENDS%
glibc>=2.36
readline
ncurses
";
        let fields = parser.parse_desc_file(desc);
        let pkg = parser
            .package_from_fields("https://example.test", &fields, Some(&depends.to_string()))
            .unwrap();

        assert_eq!(pkg.requirements.len(), 3);

        let glibc = &pkg.requirements[0];
        assert_eq!(glibc.kind, RepositoryRequirementKind::Depends);
        assert_eq!(glibc.alternatives[0].name, "glibc");
        assert_eq!(
            glibc.alternatives[0].version_constraint.as_deref(),
            Some(">=2.36")
        );

        let readline = &pkg.requirements[1];
        assert_eq!(readline.alternatives[0].name, "readline");
        assert!(readline.alternatives[0].version_constraint.is_none());
    }

    #[test]
    fn test_structured_versioned_provides() {
        let parser = ArchParser::new("core".to_string());
        let desc = "\
%NAME%
glibc

%VERSION%
2.40-1

%FILENAME%
glibc-2.40-1-x86_64.pkg.tar.zst

%SHA256SUM%
deadbeef

%CSIZE%
200

%ARCH%
x86_64

%PROVIDES%
libm.so=6-64
libpthread.so
";

        let fields = parser.parse_desc_file(desc);
        let pkg = parser
            .package_from_fields("https://example.test", &fields, None)
            .unwrap();

        // Self-provide + 2 explicit provides
        assert!(pkg.provides.len() >= 3);

        let self_prov = pkg
            .provides
            .iter()
            .find(|p| p.name == "glibc" && p.kind == RepositoryCapabilityKind::PackageName)
            .expect("self-provide missing");
        assert_eq!(self_prov.version.as_deref(), Some("2.40-1"));

        let libm = pkg
            .provides
            .iter()
            .find(|p| p.name == "libm.so")
            .expect("libm.so provide missing");
        assert_eq!(libm.kind, RepositoryCapabilityKind::Soname);
        assert_eq!(libm.version.as_deref(), Some("6-64"));

        let libpthread = pkg
            .provides
            .iter()
            .find(|p| p.name == "libpthread.so")
            .expect("libpthread.so provide missing");
        assert_eq!(libpthread.kind, RepositoryCapabilityKind::Soname);
        assert!(libpthread.version.is_none());
    }

    #[test]
    fn test_implicit_self_provide_always_present() {
        let parser = ArchParser::new("core".to_string());
        let desc = "\
%NAME%
coreutils

%VERSION%
9.5-1

%FILENAME%
coreutils-9.5-1-x86_64.pkg.tar.zst

%SHA256SUM%
deadbeef

%CSIZE%
100

%ARCH%
x86_64
";
        let fields = parser.parse_desc_file(desc);
        let pkg = parser
            .package_from_fields("https://example.test", &fields, None)
            .unwrap();

        assert!(!pkg.provides.is_empty());
        let self_prov = &pkg.provides[0];
        assert_eq!(self_prov.name, "coreutils");
        assert_eq!(self_prov.kind, RepositoryCapabilityKind::PackageName);
        assert_eq!(self_prov.version.as_deref(), Some("9.5-1"));
    }

    #[test]
    fn test_dependency_string_with_operator() {
        let parser = ArchParser::new("core".to_string());

        let (name, constraint) = parser.parse_dependency_string("openssl>=3.0");
        assert_eq!(name, "openssl");
        assert_eq!(constraint, ">=3.0");

        let (name2, constraint2) = parser.parse_dependency_string("zlib=1.3.1-1");
        assert_eq!(name2, "zlib");
        assert_eq!(constraint2, "=1.3.1-1");
    }
}
