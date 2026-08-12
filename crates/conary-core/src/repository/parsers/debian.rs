// conary-core/src/repository/parsers/debian.rs

//! Debian/Ubuntu repository metadata parser
//!
//! Parses Debian-style Packages.gz files which use RFC 822-like format
//! (similar to email headers with key: value pairs).

use super::common::{self, MAX_PACKAGE_SIZE};
use super::{
    AuthenticatedRepositoryMetadata, AuthenticatedSnapshotIdentity, ChecksumType, PackageMetadata,
    RepositoryParser,
};
use crate::compression::decompress_metadata_auto;
use crate::error::{Error, Result};
use crate::repository::client::RepositoryClient;
use crate::repository::dependency_model::{
    DebianMultiArch, RepositoryDependencyFlavor, RepositoryProvide, RepositoryRequirementGroup,
    RepositoryRequirementKind,
};
use crate::repository::package_relation::parse_native_relation;
use crate::repository::trust::TrustRole;
use crate::repository::trust::openpgp::PreparedOpenPgpTrust;
use crate::repository::versioning::VersionScheme;
use serde::Deserialize;
use tracing::{debug, info};

fn authenticated_release_snapshot(bytes: &[u8]) -> AuthenticatedSnapshotIdentity {
    AuthenticatedSnapshotIdentity::for_bytes(bytes)
}

/// Debian/Ubuntu repository parser
pub struct DebianParser {
    /// Distribution name (e.g., "noble", "jammy")
    distribution: String,
    /// Component (e.g., "main", "universe")
    component: String,
    /// Architecture (e.g., "amd64", "arm64")
    architecture: String,
    trust: PreparedOpenPgpTrust,
}

impl DebianParser {
    /// Create a new Debian/Ubuntu parser
    pub fn new(
        distribution: String,
        component: String,
        architecture: String,
        trust: PreparedOpenPgpTrust,
    ) -> Result<Self> {
        if trust.policy().format() != crate::repository::RepositoryFormat::Debian {
            return Err(Error::ConfigError(
                "Debian parser requires a Debian repository trust policy".to_string(),
            ));
        }
        Ok(Self {
            distribution,
            component,
            architecture,
            trust,
        })
    }

    /// Download and decompress the Packages file
    ///
    /// Uses RepositoryClient for HTTP and the compression module for auto-decompression.
    async fn download_packages_file(
        &self,
        repo_url: &str,
    ) -> Result<(String, AuthenticatedSnapshotIdentity)> {
        let release_path = format!(
            "{}/binary-{}/Packages.gz",
            self.component, self.architecture
        );
        let release = self.download_authenticated_release(repo_url).await?;
        let snapshot = authenticated_release_snapshot(&release);
        let authenticated = parse_release_sha256_entry(&release, &release_path)?;
        let packages_url = format!(
            "{}/dists/{}/{}",
            repo_url.trim_end_matches('/'),
            self.distribution,
            release_path
        );

        debug!("Downloading Debian Packages file from: {}", packages_url);

        let client = RepositoryClient::new()?;
        let raw_bytes = client.download_to_bytes(&packages_url).await?;
        if raw_bytes.len() as u64 != authenticated.size {
            return Err(Error::GpgVerificationFailed(format!(
                "Debian Release authenticates {} as {} bytes but the repository served {} bytes",
                release_path,
                authenticated.size,
                raw_bytes.len()
            )));
        }
        let actual = crate::hash::sha256(&raw_bytes);
        if actual != authenticated.sha256 {
            return Err(Error::GpgVerificationFailed(format!(
                "Debian Packages identity mismatch for {}: Release SHA256 is {}, downloaded \
                 SHA256 is {}",
                release_path, authenticated.sha256, actual
            )));
        }
        let decompressed = decompress_metadata_auto(
            &raw_bytes,
            &format!("Debian Packages metadata {packages_url}"),
        )?;
        let content = String::from_utf8(decompressed).map_err(|error| {
            Error::ParseError(format!("Invalid UTF-8 in Packages.gz: {}", error))
        })?;

        debug!("Decompressed Packages file: {} bytes", content.len());
        Ok((content, snapshot))
    }

    async fn download_authenticated_release(&self, repo_url: &str) -> Result<Vec<u8>> {
        let release_base = format!(
            "{}/dists/{}",
            repo_url.trim_end_matches('/'),
            self.distribution
        );
        let inrelease_url = format!("{release_base}/InRelease");
        let client = RepositoryClient::new()?;
        match client.download_to_bytes(&inrelease_url).await {
            Ok(inrelease) => self
                .trust
                .verify_inline(TrustRole::DebianRelease, &inrelease),
            Err(Error::HttpStatus {
                status: 403 | 404, ..
            }) => {
                let release_url = format!("{release_base}/Release");
                let signature_url = format!("{release_base}/Release.gpg");
                let release = client.download_to_bytes(&release_url).await?;
                let signature = client.download_to_bytes(&signature_url).await.map_err(|error| {
                    Error::GpgVerificationFailed(format!(
                        "Debian repository has no InRelease and Release.gpg could not be loaded: \
                         {error}"
                    ))
                })?;
                self.trust
                    .verify_detached(TrustRole::DebianRelease, &release, &signature)?;
                Ok(release)
            }
            Err(error) => Err(error),
        }
    }

    /// Parse a Debian dependency field into structured requirement groups.
    ///
    /// Each comma-separated entry becomes one group. OR alternatives (`a | b`)
    /// produce multiple clauses within one group.
    fn parse_requirement_groups(
        &self,
        deps_str: &str,
        kind: RepositoryRequirementKind,
    ) -> Result<Vec<RepositoryRequirementGroup>> {
        let mut groups = Vec::new();

        for dep_group in deps_str.split(',') {
            let dep_group = dep_group.trim();
            if dep_group.is_empty() {
                return Err(Error::ParseError(format!(
                    "invalid Debian dependency field with an empty comma-separated entry: {deps_str}"
                )));
            }
            groups.push(
                crate::repository::requirement::parse_native_requirement(
                    kind,
                    VersionScheme::Debian,
                    dep_group,
                )
                .map_err(|error| {
                    Error::ParseError(format!(
                        "invalid Debian {} dependency '{dep_group}': {error}",
                        kind.as_str()
                    ))
                })?,
            );
        }

        Ok(groups)
    }

    /// Parse transaction-authoritative package relations with the native
    /// Debian grammar. Invalid constraints are fatal; they must not silently
    /// become unversioned matches.
    fn parse_relation_groups(
        &self,
        relations: &str,
        kind: RepositoryRequirementKind,
    ) -> Result<Vec<RepositoryRequirementGroup>> {
        let mut groups = Vec::new();
        for entry in relations.split(',').map(str::trim) {
            if entry.is_empty() {
                return Err(Error::ParseError(format!(
                    "invalid Debian {} field with an empty comma-separated entry: {relations}",
                    kind.as_str()
                )));
            }
            groups.push(
                parse_native_relation(kind, VersionScheme::Debian, entry).map_err(|error| {
                    Error::ParseError(format!(
                        "invalid Debian {} relation '{entry}': {error}",
                        match kind {
                            RepositoryRequirementKind::Conflict => "Conflicts",
                            RepositoryRequirementKind::Breaks => "Breaks",
                            RepositoryRequirementKind::Replace => "Replaces",
                            _ => "package",
                        }
                    ))
                })?,
            );
        }
        Ok(groups)
    }

    /// Parse a Provides field into structured `RepositoryProvide` entries.
    fn parse_structured_provides(&self, provides_str: &str) -> Result<Vec<RepositoryProvide>> {
        let mut result = Vec::new();

        for (record_index, provide) in provides_str.split(',').enumerate() {
            let provide = provide.trim();
            if provide.is_empty() {
                return Err(Error::ParseError(format!(
                    "invalid Debian Provides field with an empty comma-separated entry: {provides_str}"
                )));
            }
            let mut parsed = crate::repository::package_relation::parse_debian_provide(provide)
                .map_err(|error| {
                    Error::ParseError(format!("invalid Debian Provides atom '{provide}': {error}"))
                })?;
            parsed.provenance =
                crate::repository::dependency_model::CapabilityProvenance::SourceDeclared {
                    format: crate::repository::dependency_model::SourcePackageFormat::Debian,
                    record_index: u32::try_from(record_index).map_err(|_| {
                        Error::ParseError("Debian repository provide index exceeds u32".to_string())
                    })?,
                };
            result.push(parsed);
        }

        Ok(result)
    }

    fn package_from_entry(
        &self,
        repo_url: &str,
        entry: DebianPackageEntry,
    ) -> Result<PackageMetadata> {
        let debian_multi_arch = entry
            .multi_arch
            .as_deref()
            .map(DebianMultiArch::parse_exact)
            .transpose()
            .map_err(Error::ParseError)?
            .unwrap_or_default();
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

        if let Err(msg) = common::validate_filename(&entry.filename) {
            return Err(Error::ParseError(msg));
        }
        validate_sha256(&entry.sha256, "Debian package")?;

        let download_url = common::join_repo_url(repo_url, &entry.filename);

        // Build structured requirements
        let mut requirements = Vec::new();
        if let Some(deps) = &entry.depends {
            requirements
                .extend(self.parse_requirement_groups(deps, RepositoryRequirementKind::Depends)?);
        }
        if let Some(pre_deps) = &entry.pre_depends {
            requirements.extend(
                self.parse_requirement_groups(pre_deps, RepositoryRequirementKind::PreDepends)?,
            );
        }
        if let Some(conflicts) = &entry.conflicts {
            requirements.extend(
                self.parse_relation_groups(conflicts, RepositoryRequirementKind::Conflict)?,
            );
        }
        if let Some(breaks) = &entry.breaks {
            requirements
                .extend(self.parse_relation_groups(breaks, RepositoryRequirementKind::Breaks)?);
        }
        if let Some(replaces) = &entry.replaces {
            requirements
                .extend(self.parse_relation_groups(replaces, RepositoryRequirementKind::Replace)?);
        }

        // Build structured provides
        let parsed_provides = entry
            .provides
            .as_deref()
            .map(|provides| self.parse_structured_provides(provides))
            .transpose()?
            .unwrap_or_default();
        let mut structured_provides = vec![RepositoryProvide::package_name(
            entry.package.clone(),
            Some(entry.version.clone()),
        )];
        structured_provides.extend(parsed_provides.iter().cloned());

        // Preserve source metadata that is not represented by typed columns.
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
        if !parsed_provides.is_empty() {
            extra.insert(
                "deb_provides".to_string(),
                serde_json::Value::Array(
                    parsed_provides
                        .into_iter()
                        .map(|provide| {
                            serde_json::Value::String(
                                provide
                                    .native_text
                                    .expect("parsed Debian provide retains native text"),
                            )
                        })
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
            debian_multi_arch: Some(debian_multi_arch),
            description: entry.description,
            checksum: entry.sha256,
            checksum_type: ChecksumType::Sha256,
            size,
            download_url,
            extra_metadata: serde_json::Value::Object(extra),
            dependency_flavor: RepositoryDependencyFlavor::Deb,
            version_scheme: VersionScheme::Debian,
            requirements,
            provides: structured_provides,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReleaseSha256Entry {
    sha256: String,
    size: u64,
}

fn parse_release_sha256_entry(release: &[u8], target: &str) -> Result<ReleaseSha256Entry> {
    let release = std::str::from_utf8(release)
        .map_err(|error| Error::ParseError(format!("Debian Release is not UTF-8: {error}")))?;
    let mut in_sha256 = false;
    let mut saw_sha256 = false;
    let mut matched = None;
    for line in release.lines() {
        if line == "SHA256:" {
            if saw_sha256 {
                return Err(Error::ParseError(
                    "Debian Release repeats the SHA256 field".to_string(),
                ));
            }
            saw_sha256 = true;
            in_sha256 = true;
            continue;
        }
        if !line.starts_with([' ', '\t']) {
            in_sha256 = false;
            continue;
        }
        if !in_sha256 {
            continue;
        }
        let columns = line.split_ascii_whitespace().collect::<Vec<_>>();
        if columns.len() != 3 {
            return Err(Error::ParseError(format!(
                "Debian Release SHA256 entry must have checksum, size, and path: {line:?}"
            )));
        }
        validate_sha256(columns[0], "Debian Release")?;
        let size = columns[1].parse::<u64>().map_err(|error| {
            Error::ParseError(format!(
                "Debian Release SHA256 size '{}' is invalid: {error}",
                columns[1]
            ))
        })?;
        common::validate_filename(columns[2]).map_err(Error::ParseError)?;
        if columns[2] == target {
            if matched.is_some() {
                return Err(Error::ParseError(format!(
                    "Debian Release repeats SHA256 authority for {target}"
                )));
            }
            matched = Some(ReleaseSha256Entry {
                sha256: columns[0].to_string(),
                size,
            });
        }
    }
    if !saw_sha256 {
        return Err(Error::GpgVerificationFailed(
            "authenticated Debian Release has no SHA256 field".to_string(),
        ));
    }
    matched.ok_or_else(|| {
        Error::GpgVerificationFailed(format!(
            "authenticated Debian Release has no SHA256 identity for {target}"
        ))
    })
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

/// Debian package entry structure for rfc822-like parsing
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DebianPackageEntry {
    package: String,
    version: String,
    architecture: String,
    #[serde(rename = "Multi-Arch", default)]
    multi_arch: Option<String>,
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
    conflicts: Option<String>,
    #[serde(default)]
    breaks: Option<String>,
    #[serde(default)]
    replaces: Option<String>,
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
    async fn sync_metadata(&self, repo_url: &str) -> Result<AuthenticatedRepositoryMetadata> {
        info!(
            "Syncing Debian repository: {}/{}/{}",
            self.distribution, self.component, self.architecture
        );

        // Download and decompress Packages file
        let (packages_content, snapshot) = self.download_packages_file(repo_url).await?;

        // Parse RFC 822-like format
        let entries: Vec<DebianPackageEntry> = rfc822_like::from_str(&packages_content)
            .map_err(|e| Error::ParseError(format!("Failed to parse Packages file: {}", e)))?;

        debug!("Parsed {} package entries", entries.len());

        let mut packages = Vec::new();
        for entry in entries {
            packages.push(self.package_from_entry(repo_url, entry)?);
        }

        info!("Parsed {} packages from Debian repository", packages.len());
        Ok(AuthenticatedRepositoryMetadata { packages, snapshot })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::RepositoryTrustPolicy;
    use crate::repository::dependency_model::RepositoryCapabilityKind;

    fn parser() -> DebianParser {
        let trust = PreparedOpenPgpTrust::for_test(RepositoryTrustPolicy::Debian {
            release_keys: vec![crate::repository::OpenPgpTrustRoot {
                url: "https://keys.example.test/debian.gpg".to_string(),
                fingerprint: "A".repeat(40),
            }],
        });
        DebianParser::new(
            "noble".to_string(),
            "main".to_string(),
            "amd64".to_string(),
            trust,
        )
        .unwrap()
    }

    #[test]
    fn repository_dependency_uses_canonical_debian_parser() {
        let parser = parser();

        let groups = parser
            .parse_requirement_groups("libc6:any (>= 2.34)", RepositoryRequirementKind::Depends)
            .unwrap();
        assert_eq!(groups[0].alternatives[0].name, "libc6");
        assert_eq!(
            groups[0].alternatives[0].version_constraint.as_deref(),
            Some(">= 2.34")
        );
        assert_eq!(
            groups[0].alternatives[0].architecture_qualifier,
            crate::repository::dependency_model::RequirementArchitectureQualifier::Any
        );
    }

    #[test]
    fn test_sync_metadata_persists_debian_provides_in_extra_metadata() {
        let entry = DebianPackageEntry {
            package: "mail-transport-agent".to_string(),
            version: "1.0-1".to_string(),
            architecture: "amd64".to_string(),
            multi_arch: None,
            description: Some("Test package".to_string()),
            sha256: "d".repeat(64),
            size: "123".to_string(),
            filename: "pool/main/m/mail-transport-agent.deb".to_string(),
            depends: None,
            pre_depends: None,
            conflicts: None,
            breaks: None,
            replaces: None,
            homepage: None,
            section: None,
            installed_size: None,
            provides: Some("mail-transport-agent, smtp-server (= 1.0-1)".to_string()),
        };

        let parser = parser();
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
        assert!(provides.contains(&"smtp-server (= 1.0-1)"));
    }

    #[test]
    fn test_source_distro_and_version_scheme() {
        let entry = DebianPackageEntry {
            package: "test".to_string(),
            version: "1.0-1".to_string(),
            architecture: "amd64".to_string(),
            multi_arch: Some("foreign".to_string()),
            description: None,
            sha256: "d".repeat(64),
            size: "100".to_string(),
            filename: "pool/main/t/test.deb".to_string(),
            depends: None,
            pre_depends: None,
            conflicts: None,
            breaks: None,
            replaces: None,
            provides: None,
            homepage: None,
            section: None,
            installed_size: None,
        };
        let parser = parser();
        let pkg = parser
            .package_from_entry("https://example.test", entry)
            .unwrap();
        assert_eq!(pkg.dependency_flavor, RepositoryDependencyFlavor::Deb);
        assert_eq!(pkg.version_scheme, VersionScheme::Debian);
        assert_eq!(pkg.debian_multi_arch, Some(DebianMultiArch::Foreign));
    }

    #[test]
    fn repository_rejects_unknown_debian_multi_arch_value() {
        let entry = DebianPackageEntry {
            package: "test".to_string(),
            version: "1.0-1".to_string(),
            architecture: "amd64".to_string(),
            multi_arch: Some("sometimes".to_string()),
            description: None,
            sha256: "d".repeat(64),
            size: "100".to_string(),
            filename: "pool/main/t/test.deb".to_string(),
            depends: None,
            pre_depends: None,
            conflicts: None,
            breaks: None,
            replaces: None,
            provides: None,
            homepage: None,
            section: None,
            installed_size: None,
        };
        let error = parser()
            .package_from_entry("https://example.test", entry)
            .unwrap_err();
        assert!(error.to_string().contains("Multi-Arch"), "{error}");
    }

    #[test]
    fn test_structured_versioned_depends() {
        let entry = DebianPackageEntry {
            package: "curl".to_string(),
            version: "8.0-1".to_string(),
            architecture: "amd64".to_string(),
            multi_arch: None,
            description: None,
            sha256: "a".repeat(64),
            size: "200".to_string(),
            filename: "pool/main/c/curl.deb".to_string(),
            depends: Some("libc6 (>= 2.34), libssl3 (>= 3.0)".to_string()),
            pre_depends: None,
            conflicts: None,
            breaks: None,
            replaces: None,
            provides: None,
            homepage: None,
            section: None,
            installed_size: None,
        };
        let parser = parser();
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
    fn repository_metadata_preserves_debian_negative_relations() {
        let entry = DebianPackageEntry {
            package: "newpkg".to_string(),
            version: "2".to_string(),
            architecture: "amd64".to_string(),
            multi_arch: None,
            description: None,
            sha256: "a".repeat(64),
            size: "200".to_string(),
            filename: "pool/main/n/newpkg.deb".to_string(),
            depends: None,
            pre_depends: None,
            conflicts: Some("old-conflict (<< 2)".to_string()),
            breaks: Some("old-breaks (<= 1)".to_string()),
            replaces: Some("old-owner (= 1)".to_string()),
            provides: None,
            homepage: None,
            section: None,
            installed_size: None,
        };
        let parser = parser();

        let package = parser
            .package_from_entry("https://example.test", entry)
            .unwrap();

        assert_eq!(
            package
                .requirements
                .iter()
                .map(|relation| relation.kind)
                .collect::<Vec<_>>(),
            vec![
                RepositoryRequirementKind::Conflict,
                RepositoryRequirementKind::Breaks,
                RepositoryRequirementKind::Replace,
            ]
        );
        assert_eq!(
            package
                .requirements
                .iter()
                .map(|relation| relation.native_text.as_deref())
                .collect::<Vec<_>>(),
            vec![
                Some("old-conflict (<< 2)"),
                Some("old-breaks (<= 1)"),
                Some("old-owner (= 1)"),
            ]
        );
    }

    #[test]
    fn repository_metadata_rejects_debian_negative_alternatives() {
        let entry = DebianPackageEntry {
            package: "newpkg".to_string(),
            version: "2".to_string(),
            architecture: "amd64".to_string(),
            multi_arch: None,
            description: None,
            sha256: "a".repeat(64),
            size: "200".to_string(),
            filename: "pool/main/n/newpkg.deb".to_string(),
            depends: None,
            pre_depends: None,
            conflicts: Some("old-a | old-b".to_string()),
            breaks: None,
            replaces: None,
            provides: None,
            homepage: None,
            section: None,
            installed_size: None,
        };
        let parser = parser();

        let error = parser
            .package_from_entry("https://example.test", entry)
            .unwrap_err();

        assert!(error.to_string().contains("do not permit alternatives"));
    }

    #[test]
    fn test_structured_or_deps() {
        let entry = DebianPackageEntry {
            package: "postfix".to_string(),
            version: "3.8-1".to_string(),
            architecture: "amd64".to_string(),
            multi_arch: None,
            description: None,
            sha256: "a".repeat(64),
            size: "300".to_string(),
            filename: "pool/main/p/postfix.deb".to_string(),
            depends: Some("default-mta | mail-transport-agent, libc6".to_string()),
            pre_depends: None,
            conflicts: None,
            breaks: None,
            replaces: None,
            provides: None,
            homepage: None,
            section: None,
            installed_size: None,
        };
        let parser = parser();
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
            multi_arch: None,
            description: None,
            sha256: "a".repeat(64),
            size: "400".to_string(),
            filename: "pool/main/e/exim4.deb".to_string(),
            depends: None,
            pre_depends: None,
            conflicts: None,
            breaks: None,
            replaces: None,
            provides: Some("mail-transport-agent, smtp-server (= 1.0)".to_string()),
            homepage: None,
            section: None,
            installed_size: None,
        };
        let parser = parser();
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
        assert_eq!(
            mta.provenance,
            crate::repository::dependency_model::CapabilityProvenance::SourceDeclared {
                format: crate::repository::dependency_model::SourcePackageFormat::Debian,
                record_index: 0,
            }
        );

        let smtp = pkg
            .provides
            .iter()
            .find(|p| p.name == "smtp-server")
            .expect("versioned virtual provide missing");
        assert_eq!(smtp.kind, RepositoryCapabilityKind::Virtual);
        assert_eq!(smtp.version.as_deref(), Some("1.0"));
        assert_eq!(
            smtp.provenance,
            crate::repository::dependency_model::CapabilityProvenance::SourceDeclared {
                format: crate::repository::dependency_model::SourcePackageFormat::Debian,
                record_index: 1,
            }
        );
    }

    #[test]
    fn structured_provides_preserve_explicit_architecture_qualifiers() {
        let provides = parser()
            .parse_structured_provides("mail-api:any (= 2), helper:arm64")
            .unwrap();
        assert_eq!(provides[0].name, "mail-api");
        assert_eq!(provides[0].version.as_deref(), Some("2"));
        assert_eq!(
            provides[0].architecture_qualifier,
            crate::repository::dependency_model::ProvideArchitectureQualifier::Any
        );
        assert_eq!(provides[1].name, "helper");
        assert_eq!(
            provides[1].architecture_qualifier,
            crate::repository::dependency_model::ProvideArchitectureQualifier::Exact(
                "arm64".to_string()
            )
        );
    }

    #[test]
    fn structured_provides_reject_non_exact_versions_and_empty_atoms() {
        for invalid in ["mail-api (>= 2)", "mail-api,,helper"] {
            let error = parser().parse_structured_provides(invalid).unwrap_err();
            assert!(error.to_string().contains("Provides"), "{error}");
        }
    }

    #[test]
    fn test_structured_pre_depends() {
        let entry = DebianPackageEntry {
            package: "libc6".to_string(),
            version: "2.39-1".to_string(),
            architecture: "amd64".to_string(),
            multi_arch: None,
            description: None,
            sha256: "a".repeat(64),
            size: "500".to_string(),
            filename: "pool/main/g/glibc.deb".to_string(),
            depends: Some("libgcc-s1".to_string()),
            pre_depends: Some("ld-linux-x86-64 (>= 2.39)".to_string()),
            conflicts: None,
            breaks: None,
            replaces: None,
            provides: None,
            homepage: None,
            section: None,
            installed_size: None,
        };
        let parser = parser();
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

    #[test]
    fn snapshot_identity_owns_the_verified_cleartext_release_payload() {
        let release = b"Origin: Example\nSHA256:\n";
        assert_eq!(
            authenticated_release_snapshot(release),
            AuthenticatedSnapshotIdentity::for_bytes(release)
        );
        assert_ne!(
            authenticated_release_snapshot(release),
            AuthenticatedSnapshotIdentity::for_bytes(b"armored InRelease envelope")
        );
    }
}
