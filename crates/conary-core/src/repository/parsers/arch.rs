// conary-core/src/repository/parsers/arch.rs

//! Arch Linux repository metadata parser
//!
//! Parses Arch Linux .db.tar.gz files which contain package metadata
//! in a custom text format with %FIELD% markers.

use super::{ChecksumType, PackageMetadata, RepositoryParser};
use crate::compression::decompress_auto;
use crate::error::{Error, Result};
use crate::repository::client::RepositoryClient;
use crate::repository::dependency_model::{
    RepositoryDependencyFlavor, RepositoryProvide, RepositoryRequirementGroup,
    RepositoryRequirementKind,
};
use crate::repository::package_relation::{parse_arch_provide, parse_native_relation};
use crate::repository::trust::openpgp::PreparedOpenPgpTrust;
use crate::repository::trust::{ArchSignatureRequirement, RepositoryTrustPolicy, TrustRole};
use crate::repository::versioning::VersionScheme;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use tar::Archive;
use tracing::{debug, info};

use super::common::{self, MAX_PACKAGE_SIZE};

/// Arch Linux repository parser
pub struct ArchParser {
    /// Repository name (e.g., "core", "extra", "community")
    repo_name: String,
    trust: PreparedOpenPgpTrust,
}

impl ArchParser {
    /// Create a new Arch Linux parser for a specific repository
    pub fn new(repo_name: String, trust: PreparedOpenPgpTrust) -> Result<Self> {
        if !matches!(trust.policy(), RepositoryTrustPolicy::Arch { .. }) {
            return Err(Error::ConfigError(
                "Arch parser requires an Arch repository trust policy".to_string(),
            ));
        }
        Ok(Self { repo_name, trust })
    }

    /// Download and decompress the repository database
    ///
    /// Uses RepositoryClient for HTTP and the compression module for auto-decompression.
    async fn download_database(&self, repo_url: &str) -> Result<Vec<u8>> {
        let db_url = format!("{}/{}.db", repo_url.trim_end_matches('/'), self.repo_name);
        debug!("Downloading Arch database from: {}", db_url);

        let client = RepositoryClient::new()?;
        let raw_bytes = client.download_to_bytes(&db_url).await?;
        let signature_url = format!("{db_url}.sig");
        let requirement = match self.trust.policy() {
            RepositoryTrustPolicy::Arch { sig_level, .. } => sig_level.database,
            _ => unreachable!("constructor validates Arch trust"),
        };
        match client.download_to_bytes(&signature_url).await {
            Ok(signature) => {
                self.trust
                    .verify_detached(TrustRole::ArchDatabase, &raw_bytes, &signature)?;
            }
            Err(Error::HttpStatus {
                status: 403 | 404, ..
            }) if requirement == ArchSignatureRequirement::Optional => {}
            Err(Error::HttpStatus {
                status: 403 | 404, ..
            }) => {
                return Err(Error::GpgVerificationFailed(format!(
                    "Arch SigLevel requires repository database signature {signature_url}"
                )));
            }
            Err(error) => return Err(error),
        }
        decompress_auto(&raw_bytes).map_err(|error| {
            Error::ParseError(format!("Failed to decompress {}: {}", db_url, error))
        })
    }

    /// Parse a desc file from the tarball
    fn parse_desc_file(&self, content: &str) -> Result<HashMap<String, Vec<String>>> {
        let mut fields = HashMap::new();
        let mut current_field: Option<String> = None;
        let mut values: Vec<String> = Vec::new();

        for line in content.lines() {
            let trimmed = line.trim();

            if trimmed.starts_with('%') && trimmed.ends_with('%') {
                // Save previous field
                if let Some(field) = current_field.take()
                    && fields
                        .insert(field.clone(), std::mem::take(&mut values))
                        .is_some()
                {
                    return Err(Error::ParseError(format!(
                        "Arch repository metadata repeats %{field}%"
                    )));
                }

                // Start new field
                current_field = Some(trimmed[1..trimmed.len() - 1].to_string());
            } else if !trimmed.is_empty() {
                if current_field.is_none() {
                    return Err(Error::ParseError(format!(
                        "Arch repository metadata has value {trimmed:?} outside a %FIELD% block"
                    )));
                }
                // Add value to current field
                values.push(trimmed.to_string());
            }
        }

        // Save last field
        if let Some(field) = current_field
            && fields.insert(field.clone(), values).is_some()
        {
            return Err(Error::ParseError(format!(
                "Arch repository metadata repeats %{field}%"
            )));
        }

        Ok(fields)
    }

    /// Build structured requirement groups from a depends file.
    fn parse_structured_depends(&self, content: &str) -> Result<Vec<RepositoryRequirementGroup>> {
        let fields = self.parse_desc_file(content)?;
        let mut groups = Vec::new();

        if let Some(deps) = fields.get("DEPENDS") {
            for dep in deps {
                groups.push(
                    crate::repository::requirement::parse_native_requirement(
                        RepositoryRequirementKind::Depends,
                        VersionScheme::Arch,
                        dep,
                    )
                    .map_err(|error| {
                        Error::ParseError(format!("invalid Arch %DEPENDS% entry '{dep}': {error}"))
                    })?,
                );
            }
        }

        if let Some(opts) = fields.get("OPTDEPENDS") {
            for opt in opts {
                // libalpm's alpm_dep_from_string() recognizes only ": " as
                // the description boundary so an epoch colon remains part of
                // the version.
                let (native_requirement, description) =
                    if let Some((package, description)) = opt.split_once(": ") {
                        (
                            package.trim(),
                            Some(description.trim().to_string()).filter(|value| !value.is_empty()),
                        )
                    } else {
                        (opt.as_str(), None)
                    };
                let mut group = crate::repository::requirement::parse_native_requirement(
                    RepositoryRequirementKind::Optional,
                    VersionScheme::Arch,
                    native_requirement,
                )
                .map_err(|error| {
                    Error::ParseError(format!("invalid Arch %OPTDEPENDS% entry '{opt}': {error}"))
                })?;
                group.description = description;
                group.native_text = Some(opt.clone());
                groups.push(group);
            }
        }

        Ok(groups)
    }

    fn parse_relation_fields(
        &self,
        field_sets: &[&HashMap<String, Vec<String>>],
    ) -> Result<Vec<RepositoryRequirementGroup>> {
        let mut seen = HashSet::new();
        let mut relations = Vec::new();
        for fields in field_sets {
            for (field, kind) in [
                ("CONFLICTS", RepositoryRequirementKind::Conflict),
                ("REPLACES", RepositoryRequirementKind::Replace),
            ] {
                if let Some(entries) = fields.get(field) {
                    for entry in entries {
                        if !seen.insert((kind, entry.clone())) {
                            continue;
                        }
                        relations.push(
                            parse_native_relation(kind, VersionScheme::Arch, entry).map_err(
                                |error| {
                                    Error::ParseError(format!(
                                        "invalid Arch %{field}% relation '{entry}': {error}"
                                    ))
                                },
                            )?,
                        );
                    }
                }
            }
        }
        Ok(relations)
    }

    /// Build structured provides from desc fields.
    fn build_structured_provides(
        &self,
        name: &str,
        version: &str,
        desc_fields: &HashMap<String, Vec<String>>,
    ) -> Result<Vec<RepositoryProvide>> {
        let mut provides = vec![RepositoryProvide::package_name(
            name.to_string(),
            Some(version.to_string()),
        )];

        if let Some(prov_list) = desc_fields.get("PROVIDES") {
            for prov in prov_list {
                let parsed = parse_arch_provide(prov, name).map_err(|error| {
                    Error::ParseError(format!("invalid Arch %PROVIDES% entry '{prov}': {error}"))
                })?;

                provides.push(RepositoryProvide {
                    name: parsed.name,
                    kind: parsed.kind,
                    version_relation: parsed.version.as_ref().map(|_| {
                        crate::repository::dependency_model::ProvideVersionRelation::Equal
                    }),
                    version: parsed.version,
                    architecture_qualifier:
                        crate::repository::dependency_model::ProvideArchitectureQualifier::Implicit,
                    native_text: Some(prov.clone()),
                });
            }
        }

        Ok(provides)
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

        let mut requirements = depends_content
            .map(|content| self.parse_structured_depends(content))
            .transpose()?
            .unwrap_or_default();
        let depends_fields = depends_content
            .map(|content| self.parse_desc_file(content))
            .transpose()?;
        let relation_field_sets = depends_fields
            .as_ref()
            .map_or_else(|| vec![desc_fields], |fields| vec![desc_fields, fields]);
        requirements.extend(self.parse_relation_fields(&relation_field_sets)?);

        let structured_provides = self.build_structured_provides(&name, &version, desc_fields)?;

        Ok(PackageMetadata {
            name,
            version,
            architecture,
            debian_multi_arch: None,
            description,
            checksum,
            checksum_type: ChecksumType::Sha256,
            size,
            download_url,
            extra_metadata: serde_json::Value::Object(extra),
            dependency_flavor: RepositoryDependencyFlavor::Arch,
            version_scheme: VersionScheme::Arch,
            requirements,
            provides: structured_provides,
        })
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

            let path_str = path.to_str().ok_or_else(|| {
                Error::ParseError("Arch repository entry path is not valid UTF-8".to_string())
            })?;

            if let Some(dir) = path_str.split('/').next().filter(|dir| !dir.is_empty()) {
                let dir_key = dir.to_string();

                if path_str.ends_with("/desc") {
                    let mut content = String::new();
                    entry.read_to_string(&mut content).map_err(|e| {
                        Error::ParseError(format!("Failed to read desc file: {}", e))
                    })?;
                    if desc_data.insert(dir_key.clone(), content).is_some() {
                        return Err(Error::ParseError(format!(
                            "Arch repository repeats desc metadata for {dir_key}"
                        )));
                    }
                } else if path_str.ends_with("/depends") {
                    let mut content = String::new();
                    entry.read_to_string(&mut content).map_err(|e| {
                        Error::ParseError(format!("Failed to read depends file: {}", e))
                    })?;
                    if depends_data.insert(dir_key.clone(), content).is_some() {
                        return Err(Error::ParseError(format!(
                            "Arch repository repeats depends metadata for {dir_key}"
                        )));
                    }
                }
            }
        }

        if let Some(orphan) = depends_data
            .keys()
            .find(|directory| !desc_data.contains_key(*directory))
        {
            return Err(Error::ParseError(format!(
                "Arch repository has depends metadata without desc metadata for {orphan}"
            )));
        }

        // Build packages from collected data
        let mut packages = Vec::new();
        for (dir_key, desc_content) in &desc_data {
            let desc_fields = self.parse_desc_file(desc_content)?;
            packages.push(self.package_from_fields(
                repo_url,
                &desc_fields,
                depends_data.get(dir_key),
            )?);
        }

        info!("Parsed {} packages from Arch repository", packages.len());
        Ok(packages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::dependency_model::RepositoryCapabilityKind;

    fn parser() -> ArchParser {
        let trust = PreparedOpenPgpTrust::for_test(RepositoryTrustPolicy::Arch {
            keyring: crate::repository::ArchKeyringTrust {
                url: "https://keys.example.test/arch.gpg".to_string(),
                format: crate::repository::ArchKeyringFormat::OpenPgp,
                master_fingerprints: vec!["A".repeat(40)],
                packager_key_threshold: 1,
            },
            sig_level: crate::repository::ArchSigLevel::distribution_default(),
        });
        ArchParser::new("core".to_string(), trust).unwrap()
    }

    #[test]
    fn test_parse_desc_file() {
        let parser = parser();
        let content =
            "%NAME%\nbash\n\n%VERSION%\n5.2.037-1\n\n%DESC%\nThe GNU Bourne Again shell\n";

        let fields = parser.parse_desc_file(content).unwrap();

        assert_eq!(fields.get("NAME"), Some(&vec!["bash".to_string()]));
        assert_eq!(fields.get("VERSION"), Some(&vec!["5.2.037-1".to_string()]));
        assert_eq!(
            fields.get("DESC"),
            Some(&vec!["The GNU Bourne Again shell".to_string()])
        );
    }

    #[test]
    fn desc_parser_rejects_duplicate_fields_and_unscoped_values() {
        let parser = parser();

        assert!(
            parser
                .parse_desc_file("%NAME%\nfirst\n\n%NAME%\nsecond\n")
                .is_err()
        );
        assert!(parser.parse_desc_file("orphan\n%NAME%\nfixture\n").is_err());
    }

    #[test]
    fn repository_dependency_uses_canonical_arch_parser() {
        let parsed = crate::repository::requirement::parse_native_requirement(
            RepositoryRequirementKind::Depends,
            VersionScheme::Arch,
            "glibc>=2.17",
        )
        .unwrap();
        assert_eq!(parsed.alternatives[0].name, "glibc");
        assert_eq!(
            parsed.alternatives[0].version_constraint.as_deref(),
            Some(">= 2.17")
        );
    }

    #[test]
    fn optional_dependency_preserves_version_epoch_before_description() {
        let parser = parser();

        let groups = parser
            .parse_structured_depends("%OPTDEPENDS%\nruntime>=2:1.0-1: optional runtime\n")
            .unwrap();
        assert_eq!(groups[0].alternatives[0].name, "runtime");
        assert_eq!(
            groups[0].alternatives[0].version_constraint.as_deref(),
            Some(">= 2:1.0-1")
        );
        assert_eq!(groups[0].description.as_deref(), Some("optional runtime"));
    }

    #[test]
    fn test_parse_desc_file_provides_persisted_in_extra_metadata() {
        let parser = parser();
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

        let fields = parser.parse_desc_file(desc).unwrap();
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
        let parser = parser();
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
        let fields = parser.parse_desc_file(desc).unwrap();
        let pkg = parser
            .package_from_fields("https://example.test", &fields, None)
            .unwrap();

        assert_eq!(pkg.dependency_flavor, RepositoryDependencyFlavor::Arch);
        assert_eq!(pkg.version_scheme, VersionScheme::Arch);
    }

    #[test]
    fn test_structured_versioned_depends() {
        let parser = parser();
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
        let fields = parser.parse_desc_file(desc).unwrap();
        let pkg = parser
            .package_from_fields("https://example.test", &fields, Some(&depends.to_string()))
            .unwrap();

        assert_eq!(pkg.requirements.len(), 3);

        let glibc = &pkg.requirements[0];
        assert_eq!(glibc.kind, RepositoryRequirementKind::Depends);
        assert_eq!(glibc.alternatives[0].name, "glibc");
        assert_eq!(
            glibc.alternatives[0].version_constraint.as_deref(),
            Some(">= 2.36")
        );

        let readline = &pkg.requirements[1];
        assert_eq!(readline.alternatives[0].name, "readline");
        assert!(readline.alternatives[0].version_constraint.is_none());
    }

    #[test]
    fn repository_metadata_preserves_arch_negative_relations() {
        let parser = parser();
        let desc = "\
%NAME%
newpkg

%VERSION%
2-1

%FILENAME%
newpkg-2-1-x86_64.pkg.tar.zst

%SHA256SUM%
deadbeef

%CSIZE%
123

%ARCH%
x86_64

%CONFLICTS%
old-conflict<2

%REPLACES%
old-owner=1
";
        let fields = parser.parse_desc_file(desc).unwrap();

        let package = parser
            .package_from_fields("https://example.test", &fields, None)
            .unwrap();

        assert_eq!(
            package
                .requirements
                .iter()
                .map(|relation| relation.kind)
                .collect::<Vec<_>>(),
            vec![
                RepositoryRequirementKind::Conflict,
                RepositoryRequirementKind::Replace,
            ]
        );
        assert_eq!(
            package
                .requirements
                .iter()
                .map(|relation| relation.native_text.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("old-conflict<2"), Some("old-owner=1")]
        );
    }

    #[test]
    fn repository_metadata_rejects_malformed_arch_relation() {
        let parser = parser();
        let desc = "\
%NAME%
newpkg

%VERSION%
2-1

%FILENAME%
newpkg-2-1-x86_64.pkg.tar.zst

%SHA256SUM%
deadbeef

%CSIZE%
123

%ARCH%
x86_64

%CONFLICTS%
oldpkg>=
";
        let fields = parser.parse_desc_file(desc).unwrap();

        let error = parser
            .package_from_fields("https://example.test", &fields, None)
            .unwrap_err();

        assert!(error.to_string().contains("empty version"));
    }

    #[test]
    fn test_structured_versioned_provides() {
        let parser = parser();
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
libwlroots-0.18.so=libwlroots-0.18.so-64
lib:libwayland-client.so.0
";

        let fields = parser.parse_desc_file(desc).unwrap();
        let pkg = parser
            .package_from_fields("https://example.test", &fields, None)
            .unwrap();

        // Self-provide + 4 explicit provides
        assert_eq!(pkg.provides.len(), 5);

        let self_prov = pkg
            .provides
            .iter()
            .find(|p| p.name == "glibc" && p.kind == RepositoryCapabilityKind::PackageName)
            .expect("self-provide missing");
        assert_eq!(self_prov.version.as_deref(), Some("2.40-1"));

        let libm = pkg
            .provides
            .iter()
            .find(|p| p.name == "libm.so=6-64")
            .expect("libm.so provide missing");
        assert_eq!(libm.kind, RepositoryCapabilityKind::Soname);
        assert!(libm.version.is_none());

        let libpthread = pkg
            .provides
            .iter()
            .find(|p| p.name == "libpthread.so")
            .expect("libpthread.so provide missing");
        assert_eq!(libpthread.kind, RepositoryCapabilityKind::Soname);
        assert!(libpthread.version.is_none());

        let wlroots = pkg
            .provides
            .iter()
            .find(|p| p.name == "libwlroots-0.18.so=libwlroots-0.18.so-64")
            .expect("versioned soname v1 provide missing");
        assert_eq!(wlroots.kind, RepositoryCapabilityKind::Soname);
        assert!(wlroots.version.is_none());

        let wayland = pkg
            .provides
            .iter()
            .find(|p| p.name == "lib:libwayland-client.so.0")
            .expect("soname v2 provide missing");
        assert_eq!(wayland.kind, RepositoryCapabilityKind::Soname);
        assert!(wayland.version.is_none());
    }

    #[test]
    fn repository_runtime_sonames_are_atomic_typed_requirements() {
        let parser = parser();
        let requirements = parser
            .parse_structured_depends(
                "%DEPENDS%\n\
                 libwlroots-0.18.so=libwlroots-0.18.so-64\n\
                 lib:libwayland-client.so.0\n",
            )
            .unwrap();

        assert_eq!(requirements.len(), 2);
        for (requirement, identity) in requirements.iter().zip([
            "libwlroots-0.18.so=libwlroots-0.18.so-64",
            "lib:libwayland-client.so.0",
        ]) {
            let clause = &requirement.alternatives[0];
            assert_eq!(clause.name, identity);
            assert_eq!(
                clause.capability_kind,
                Some(RepositoryCapabilityKind::Soname)
            );
            assert!(clause.version_constraint.is_none());
        }
    }

    #[test]
    fn repository_rejects_non_exact_arch_provide_version() {
        let parser = parser();
        let mut fields = HashMap::new();
        fields.insert("PROVIDES".to_string(), vec!["mail-api>=2".to_string()]);

        let error = parser
            .build_structured_provides("mail-provider", "1", &fields)
            .unwrap_err();
        assert!(error.to_string().contains("exact '='"), "{error}");
    }

    #[test]
    fn test_implicit_self_provide_always_present() {
        let parser = parser();
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
        let fields = parser.parse_desc_file(desc).unwrap();
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
    fn repository_rejects_malformed_arch_dependency() {
        let parser = parser();
        let error = parser
            .parse_structured_depends("%DEPENDS%\nopenssl>=\n")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("invalid Arch %DEPENDS% entry 'openssl>='"),
            "{error}"
        );
    }
}
