// conary-core/src/repository/parsers/debian.rs

//! Debian/Ubuntu repository metadata parser
//!
//! Parses Debian-style Packages.gz files which use RFC 822-like format
//! (similar to email headers with key: value pairs).

use super::common::{self, MAX_PACKAGE_SIZE};
use super::{ChecksumType, Dependency, PackageMetadata, RepositoryParser};
use crate::error::{Error, Result};
use crate::repository::client::RepositoryClient;
use crate::repository::dependency_model::{
    RepositoryCapabilityKind, RepositoryDependencyFlavor, RepositoryProvide,
    RepositoryRequirementClause, RepositoryRequirementGroup, RepositoryRequirementKind,
};
use crate::repository::versioning::VersionScheme;
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

            // Handle alternatives (pkg1 | pkg2) - take first alternative.
            // split() always yields at least one element, so indexing is safe.
            let dep = dep_group.split('|').next().unwrap_or(dep_group).trim();

            // Parse package name and version constraint
            if let Some((name, constraint)) = self.parse_dependency(dep) {
                dependencies.push(Dependency::runtime_versioned(name, constraint));
            }
        }

        dependencies
    }

    fn parse_provides(&self, provides_str: &str) -> Vec<String> {
        let mut provides = Vec::new();

        for provide in provides_str.split(',') {
            let provide = provide.trim();
            if provide.is_empty() {
                continue;
            }

            if let Some(paren_pos) = provide.find('(') {
                let name = provide[..paren_pos].trim();
                let constraint = provide[paren_pos + 1..].trim_end_matches(')').trim();
                if constraint.is_empty() {
                    provides.push(name.to_string());
                } else {
                    provides.push(format!("{name} {constraint}"));
                }
            } else {
                provides.push(provide.to_string());
            }
        }

        provides
    }

    /// Parse a single dependency string
    /// Format: "package (>= 1.0)" or "package (= 1.0-1)" or "package"
    fn parse_dependency(&self, dep: &str) -> Option<(String, String)> {
        Some(common::split_dependency(dep))
    }

    /// Parse a Debian dependency field into structured requirement groups.
    ///
    /// Each comma-separated entry becomes one group. OR alternatives (`a | b`)
    /// produce multiple clauses within one group.
    fn parse_requirement_groups(
        &self,
        deps_str: &str,
        kind: RepositoryRequirementKind,
    ) -> Vec<RepositoryRequirementGroup> {
        let mut groups = Vec::new();

        for dep_group in deps_str.split(',') {
            let dep_group = dep_group.trim();
            if dep_group.is_empty() {
                continue;
            }

            let alternatives: Vec<&str> = dep_group.split('|').map(str::trim).collect();
            let clauses: Vec<RepositoryRequirementClause> = alternatives
                .iter()
                .filter_map(|alt| {
                    let (name, constraint) = self.parse_dependency(alt)?;
                    if constraint.is_empty() {
                        Some(RepositoryRequirementClause::name_only(name))
                    } else {
                        Some(RepositoryRequirementClause::versioned(name, constraint))
                    }
                })
                .collect();

            if clauses.is_empty() {
                continue;
            }

            let group = if clauses.len() == 1 {
                RepositoryRequirementGroup::simple(kind, clauses.into_iter().next().unwrap())
            } else {
                RepositoryRequirementGroup::alternatives(kind, clauses)
            };

            groups.push(group.with_native_text(dep_group.to_string()));
        }

        groups
    }

    /// Parse a Provides field into structured `RepositoryProvide` entries.
    fn parse_structured_provides(&self, provides_str: &str) -> Vec<RepositoryProvide> {
        let mut result = Vec::new();

        for provide in provides_str.split(',') {
            let provide = provide.trim();
            if provide.is_empty() {
                continue;
            }

            let (name, version) = if let Some(paren_pos) = provide.find('(') {
                let pname = provide[..paren_pos].trim();
                let constraint = provide[paren_pos + 1..].trim_end_matches(')').trim();
                (pname, common::extract_version_from_constraint(constraint))
            } else {
                (provide, None)
            };

            result.push(RepositoryProvide {
                name: name.to_string(),
                kind: RepositoryCapabilityKind::Virtual,
                version,
                native_text: Some(provide.to_string()),
            });
        }

        result
    }

    fn package_from_entry(
        &self,
        repo_url: &str,
        entry: DebianPackageEntry,
    ) -> Result<PackageMetadata> {
        let size: u64 = entry
            .size
            .parse()
            .map_err(|e| Error::ParseError(format!("Invalid size '{}': {}", entry.size, e)))?;

        if size > MAX_PACKAGE_SIZE {
            return Err(Error::ParseError(format!(
                "Package {} size {} exceeds maximum allowed (5GB)",
                entry.package, size
            )));
        }

        let dependencies = if let Some(deps) = &entry.depends {
            self.parse_dependencies(deps)
        } else {
            Vec::new()
        };

        if let Err(msg) = common::validate_filename(&entry.filename) {
            return Err(Error::ParseError(msg));
        }

        let download_url = common::join_repo_url(repo_url, &entry.filename);

        // Build structured requirements
        let mut requirements = Vec::new();
        if let Some(deps) = &entry.depends {
            requirements
                .extend(self.parse_requirement_groups(deps, RepositoryRequirementKind::Depends));
        }
        if let Some(pre_deps) = &entry.pre_depends {
            requirements.extend(
                self.parse_requirement_groups(pre_deps, RepositoryRequirementKind::PreDepends),
            );
        }

        // Build structured provides
        let mut structured_provides = vec![RepositoryProvide::package_name(
            entry.package.clone(),
            Some(entry.version.clone()),
        )];
        if let Some(provides_str) = &entry.provides {
            structured_provides.extend(self.parse_structured_provides(provides_str));
        }

        // Build extra metadata (legacy)
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
        if let Some(provides) = entry.provides {
            extra.insert(
                "deb_provides".to_string(),
                serde_json::Value::Array(
                    self.parse_provides(&provides)
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            );
        }
        extra.insert(
            "format".to_string(),
            serde_json::Value::String("deb".to_string()),
        );
        extra.insert(
            "distribution".to_string(),
            serde_json::Value::String(self.distribution.clone()),
        );
        extra.insert(
            "component".to_string(),
            serde_json::Value::String(self.component.clone()),
        );

        Ok(PackageMetadata {
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
            source_distro: Some(RepositoryDependencyFlavor::Deb),
            version_scheme: Some(VersionScheme::Debian),
            requirements,
            provides: structured_provides,
        })
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
    #[serde(rename = "Pre-Depends", default)]
    pre_depends: Option<String>,
    #[serde(default)]
    provides: Option<String>,
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

        let mut packages = Vec::new();
        for entry in entries {
            packages.push(self.package_from_entry(repo_url, entry)?);
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
        let parser =
            DebianParser::new("noble".to_string(), "main".to_string(), "amd64".to_string());

        let (name, constraint) = parser.parse_dependency("libc6 (>= 2.34)").unwrap();
        assert_eq!(name, "libc6");
        assert_eq!(constraint, ">= 2.34");

        let (name2, constraint2) = parser.parse_dependency("bash").unwrap();
        assert_eq!(name2, "bash");
        assert_eq!(constraint2, "");
    }

    #[test]
    fn test_parse_dependencies() {
        let parser =
            DebianParser::new("noble".to_string(), "main".to_string(), "amd64".to_string());

        let deps = parser.parse_dependencies("libc6 (>= 2.34), bash (= 5.2-1), coreutils");
        assert_eq!(deps.len(), 3);
        assert_eq!(deps[0].name, "libc6");
        assert_eq!(deps[1].name, "bash");
        assert_eq!(deps[2].name, "coreutils");
    }

    #[test]
    fn test_parse_alternatives() {
        let parser =
            DebianParser::new("noble".to_string(), "main".to_string(), "amd64".to_string());

        // Should take first alternative
        let deps = parser.parse_dependencies("package-a | package-b, other-package");
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].name, "package-a");
        assert_eq!(deps[1].name, "other-package");
    }

    #[test]
    fn test_sync_metadata_persists_debian_provides_in_extra_metadata() {
        let entry = DebianPackageEntry {
            package: "mail-transport-agent".to_string(),
            version: "1.0-1".to_string(),
            architecture: "amd64".to_string(),
            description: Some("Test package".to_string()),
            sha256: "deadbeef".to_string(),
            size: "123".to_string(),
            filename: "pool/main/m/mail-transport-agent.deb".to_string(),
            depends: None,
            pre_depends: None,
            homepage: None,
            section: None,
            installed_size: None,
            provides: Some("mail-transport-agent, smtp-server (= 1.0-1)".to_string()),
        };

        let parser =
            DebianParser::new("noble".to_string(), "main".to_string(), "amd64".to_string());
        let package = parser
            .package_from_entry("https://example.test", entry)
            .unwrap();
        let metadata = package.extra_metadata.as_object().unwrap();
        let provides = metadata
            .get("deb_provides")
            .and_then(|value| value.as_array())
            .unwrap();
        let provides: Vec<&str> = provides.iter().filter_map(|value| value.as_str()).collect();

        assert!(provides.contains(&"mail-transport-agent"));
        assert!(provides.contains(&"smtp-server = 1.0-1"));
    }

    #[test]
    fn test_source_distro_and_version_scheme() {
        let entry = DebianPackageEntry {
            package: "test".to_string(),
            version: "1.0-1".to_string(),
            architecture: "amd64".to_string(),
            description: None,
            sha256: "deadbeef".to_string(),
            size: "100".to_string(),
            filename: "pool/main/t/test.deb".to_string(),
            depends: None,
            pre_depends: None,
            provides: None,
            homepage: None,
            section: None,
            installed_size: None,
        };
        let parser =
            DebianParser::new("noble".to_string(), "main".to_string(), "amd64".to_string());
        let pkg = parser
            .package_from_entry("https://example.test", entry)
            .unwrap();
        assert_eq!(pkg.source_distro, Some(RepositoryDependencyFlavor::Deb));
        assert_eq!(pkg.version_scheme, Some(VersionScheme::Debian));
    }

    #[test]
    fn test_structured_versioned_depends() {
        let entry = DebianPackageEntry {
            package: "curl".to_string(),
            version: "8.0-1".to_string(),
            architecture: "amd64".to_string(),
            description: None,
            sha256: "abcd".to_string(),
            size: "200".to_string(),
            filename: "pool/main/c/curl.deb".to_string(),
            depends: Some("libc6 (>= 2.34), libssl3 (>= 3.0)".to_string()),
            pre_depends: None,
            provides: None,
            homepage: None,
            section: None,
            installed_size: None,
        };
        let parser =
            DebianParser::new("noble".to_string(), "main".to_string(), "amd64".to_string());
        let pkg = parser
            .package_from_entry("https://example.test", entry)
            .unwrap();

        assert_eq!(pkg.requirements.len(), 2);
        assert_eq!(pkg.requirements[0].kind, RepositoryRequirementKind::Depends);
        assert_eq!(pkg.requirements[0].alternatives[0].name, "libc6");
        assert_eq!(
            pkg.requirements[0].alternatives[0]
                .version_constraint
                .as_deref(),
            Some(">= 2.34")
        );
        assert_eq!(pkg.requirements[1].alternatives[0].name, "libssl3");
    }

    #[test]
    fn test_structured_or_deps() {
        let entry = DebianPackageEntry {
            package: "postfix".to_string(),
            version: "3.8-1".to_string(),
            architecture: "amd64".to_string(),
            description: None,
            sha256: "abcd".to_string(),
            size: "300".to_string(),
            filename: "pool/main/p/postfix.deb".to_string(),
            depends: Some("default-mta | mail-transport-agent, libc6".to_string()),
            pre_depends: None,
            provides: None,
            homepage: None,
            section: None,
            installed_size: None,
        };
        let parser =
            DebianParser::new("noble".to_string(), "main".to_string(), "amd64".to_string());
        let pkg = parser
            .package_from_entry("https://example.test", entry)
            .unwrap();

        assert_eq!(pkg.requirements.len(), 2);
        // First group has alternatives
        assert_eq!(pkg.requirements[0].alternatives.len(), 2);
        assert_eq!(pkg.requirements[0].alternatives[0].name, "default-mta");
        assert_eq!(
            pkg.requirements[0].alternatives[1].name,
            "mail-transport-agent"
        );
        // Second group is simple
        assert_eq!(pkg.requirements[1].alternatives.len(), 1);
        assert_eq!(pkg.requirements[1].alternatives[0].name, "libc6");
    }

    #[test]
    fn test_structured_versioned_and_unversioned_provides() {
        let entry = DebianPackageEntry {
            package: "exim4".to_string(),
            version: "4.97-1".to_string(),
            architecture: "amd64".to_string(),
            description: None,
            sha256: "abcd".to_string(),
            size: "400".to_string(),
            filename: "pool/main/e/exim4.deb".to_string(),
            depends: None,
            pre_depends: None,
            provides: Some("mail-transport-agent, smtp-server (= 1.0)".to_string()),
            homepage: None,
            section: None,
            installed_size: None,
        };
        let parser =
            DebianParser::new("noble".to_string(), "main".to_string(), "amd64".to_string());
        let pkg = parser
            .package_from_entry("https://example.test", entry)
            .unwrap();

        // Self-provide + 2 explicit provides
        assert!(pkg.provides.len() >= 3);

        let self_prov = pkg
            .provides
            .iter()
            .find(|p| p.name == "exim4" && p.kind == RepositoryCapabilityKind::PackageName)
            .expect("self-provide missing");
        assert_eq!(self_prov.version.as_deref(), Some("4.97-1"));

        let mta = pkg
            .provides
            .iter()
            .find(|p| p.name == "mail-transport-agent")
            .expect("virtual provide missing");
        assert_eq!(mta.kind, RepositoryCapabilityKind::Virtual);
        assert!(mta.version.is_none());

        let smtp = pkg
            .provides
            .iter()
            .find(|p| p.name == "smtp-server")
            .expect("versioned virtual provide missing");
        assert_eq!(smtp.kind, RepositoryCapabilityKind::Virtual);
        assert_eq!(smtp.version.as_deref(), Some("1.0"));
    }

    #[test]
    fn test_structured_pre_depends() {
        let entry = DebianPackageEntry {
            package: "libc6".to_string(),
            version: "2.39-1".to_string(),
            architecture: "amd64".to_string(),
            description: None,
            sha256: "abcd".to_string(),
            size: "500".to_string(),
            filename: "pool/main/g/glibc.deb".to_string(),
            depends: Some("libgcc-s1".to_string()),
            pre_depends: Some("ld-linux-x86-64 (>= 2.39)".to_string()),
            provides: None,
            homepage: None,
            section: None,
            installed_size: None,
        };
        let parser =
            DebianParser::new("noble".to_string(), "main".to_string(), "amd64".to_string());
        let pkg = parser
            .package_from_entry("https://example.test", entry)
            .unwrap();

        // Should have 1 Depends + 1 PreDepends
        let depends: Vec<_> = pkg
            .requirements
            .iter()
            .filter(|r| r.kind == RepositoryRequirementKind::Depends)
            .collect();
        let pre_depends: Vec<_> = pkg
            .requirements
            .iter()
            .filter(|r| r.kind == RepositoryRequirementKind::PreDepends)
            .collect();

        assert_eq!(depends.len(), 1);
        assert_eq!(depends[0].alternatives[0].name, "libgcc-s1");

        assert_eq!(pre_depends.len(), 1);
        assert_eq!(pre_depends[0].alternatives[0].name, "ld-linux-x86-64");
        assert_eq!(
            pre_depends[0].alternatives[0].version_constraint.as_deref(),
            Some(">= 2.39")
        );
    }
}
