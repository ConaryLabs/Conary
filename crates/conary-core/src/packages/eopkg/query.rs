// conary-core/src/packages/eopkg/query.rs

//! Exact installed eopkg authority from retained package XML.

use super::xml;
use crate::error::{Error, Result};
use crate::packages::archive_utils::get_file_metadata;
use crate::packages::query_common::{InstalledFileAbsencePolicy, InstalledFileInfo};
use crate::packages::{InstalledPackageIdentity, InstalledPackageRecord};
use crate::repository::dependency_model::{
    CapabilityProvenance, ProvideArchitectureQualifier, ProvidedCapability,
    RepositoryCapabilityKind, RepositoryRequirementGroup, RepositoryRequirementKind,
};
use crate::repository::versioning::VersionScheme;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct InstalledEopkgInfo {
    pub distribution: String,
    pub distribution_release: String,
    pub package_format: String,
    pub description: Option<String>,
}

struct InstalledEopkgEntry {
    package: InstalledPackageRecord<InstalledEopkgInfo>,
    metadata: xml::Metadata,
    files: Vec<xml::FileRecord>,
}

/// One exact snapshot of eopkg's retained installed-package authority.
///
/// Bulk adoption and ownership transfer load this once. Re-reading and parsing
/// the complete package database for each package field is both unnecessary
/// and quadratic in the number of installed packages.
pub struct InstalledEopkgDatabase {
    entries: BTreeMap<String, InstalledEopkgEntry>,
}

impl InstalledEopkgDatabase {
    pub fn load() -> Result<Self> {
        Self::load_root(Path::new("/var/lib/eopkg/package"))
    }

    pub fn load_root(root: &Path) -> Result<Self> {
        let mut entries = BTreeMap::new();
        if !root.exists() {
            return Ok(Self { entries });
        }
        for directory in fs::read_dir(root)? {
            let directory = directory?;
            if !directory.file_type()?.is_dir() {
                continue;
            }
            let metadata_path = directory.path().join("metadata.xml");
            if !metadata_path.is_file() {
                return Err(Error::ParseError(format!(
                    "installed eopkg record {} has no metadata.xml authority",
                    directory.path().display()
                )));
            }
            let metadata = xml::parse_metadata(&fs::read(&metadata_path)?)?;
            let files_path = directory.path().join("files.xml");
            if !files_path.is_file() {
                return Err(Error::ParseError(format!(
                    "installed eopkg record {} has no files.xml authority",
                    directory.path().display()
                )));
            }
            let files = xml::parse_files(&fs::read(files_path)?)?;
            let identity = InstalledPackageIdentity::eopkg(
                metadata.name.clone(),
                metadata.name.clone(),
                metadata.version.clone(),
                metadata.release,
                metadata.architecture.clone(),
            )?;
            let selector = identity.selector().to_string();
            let package = InstalledPackageRecord {
                identity,
                info: InstalledEopkgInfo {
                    distribution: metadata.distribution.clone(),
                    distribution_release: metadata.distribution_release.clone(),
                    package_format: metadata.package_format.clone(),
                    description: metadata.description.clone().or(metadata.summary.clone()),
                },
            };
            if entries
                .insert(
                    selector.clone(),
                    InstalledEopkgEntry {
                        package,
                        metadata,
                        files,
                    },
                )
                .is_some()
            {
                return Err(Error::ConflictError(format!(
                    "installed eopkg selector {selector:?} has more than one authority record"
                )));
            }
        }
        Ok(Self { entries })
    }

    pub fn packages(&self) -> Vec<InstalledPackageRecord<InstalledEopkgInfo>> {
        self.entries
            .values()
            .map(|entry| entry.package.clone())
            .collect()
    }

    pub fn package(&self, name: &str) -> Result<InstalledPackageRecord<InstalledEopkgInfo>> {
        Ok(self.entry(name)?.package.clone())
    }

    pub fn package_files(&self, name: &str) -> Result<Vec<InstalledFileInfo>> {
        project_installed_files(&self.entry(name)?.files)
    }

    pub fn package_requirement_groups(
        &self,
        name: &str,
    ) -> Result<Vec<RepositoryRequirementGroup>> {
        super::relation_groups(
            self.entry(name)?.metadata.dependencies.clone(),
            RepositoryRequirementKind::Depends,
        )
    }

    pub fn package_provides(
        &self,
        identity: &InstalledPackageIdentity,
    ) -> Result<Vec<ProvidedCapability>> {
        project_provides(&self.entry(identity.selector())?.metadata, identity)
    }

    pub fn user_installed(&self) -> Result<std::collections::HashSet<String>> {
        let automatic = fs::read_to_string("/var/lib/eopkg/info/autoinstalled")
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_string)
            .collect::<std::collections::HashSet<_>>();
        Ok(self
            .entries
            .values()
            .map(|entry| entry.package.identity.install_reason_selector())
            .filter(|name| !automatic.contains(name))
            .collect())
    }

    fn entry(&self, name: &str) -> Result<&InstalledEopkgEntry> {
        self.entries
            .get(name)
            .ok_or_else(|| Error::NotFound(format!("Package '{name}' not found in eopkg database")))
    }
}

pub fn query_all_packages() -> Result<Vec<InstalledPackageRecord<InstalledEopkgInfo>>> {
    query_installed_root(Path::new("/var/lib/eopkg/package"))
}

pub fn query_installed_root(
    root: &Path,
) -> Result<Vec<InstalledPackageRecord<InstalledEopkgInfo>>> {
    Ok(InstalledEopkgDatabase::load_root(root)?.packages())
}

pub fn query_package(name: &str) -> Result<InstalledPackageRecord<InstalledEopkgInfo>> {
    InstalledEopkgDatabase::load()?.package(name)
}

pub fn query_package_files(name: &str) -> Result<Vec<InstalledFileInfo>> {
    InstalledEopkgDatabase::load()?.package_files(name)
}

fn project_installed_files(records: &[xml::FileRecord]) -> Result<Vec<InstalledFileInfo>> {
    records
        .iter()
        .map(|record| {
            let path = crate::packages::archive_utils::normalize_path(&record.path)
                .map_err(|error| Error::ParseError(error.to_string()))?;
            let (size, mode) = match get_file_metadata(&path) {
                Ok(metadata) => metadata,
                Err(_error) if record.permanent => (
                    i64::try_from(record.size.unwrap_or(0)).unwrap_or(i64::MAX),
                    i32::try_from(libc::S_IFREG | record.mode.unwrap_or(0o644)).unwrap(),
                ),
                Err(error) => return Err(error),
            };
            let link_target = if mode & 0o170000 == libc::S_IFLNK as i32 {
                Some(
                    fs::read_link(&path)?
                        .into_os_string()
                        .into_string()
                        .map_err(|_| {
                            Error::ParseError(format!("eopkg symlink {path} target is not UTF-8"))
                        })?,
                )
            } else {
                None
            };
            Ok(InstalledFileInfo {
                path,
                size,
                mode,
                digest: record.sha1.clone(),
                user: record.uid.map(|value| value.to_string()),
                group: record.gid.map(|value| value.to_string()),
                link_target,
                mtime: None,
                absence_policy: if record.permanent {
                    InstalledFileAbsencePolicy::EopkgPermanent
                } else {
                    InstalledFileAbsencePolicy::Required
                },
            })
        })
        .collect()
}

pub fn query_package_requirement_groups(name: &str) -> Result<Vec<RepositoryRequirementGroup>> {
    InstalledEopkgDatabase::load()?.package_requirement_groups(name)
}

pub fn query_package_provides(
    identity: &InstalledPackageIdentity,
) -> Result<Vec<ProvidedCapability>> {
    InstalledEopkgDatabase::load()?.package_provides(identity)
}

fn project_provides(
    metadata: &xml::Metadata,
    identity: &InstalledPackageIdentity,
) -> Result<Vec<ProvidedCapability>> {
    let mut provides = vec![ProvidedCapability {
        kind: RepositoryCapabilityKind::PackageName,
        name: metadata.name.clone(),
        version: Some(identity.version()),
        version_relation: Some(crate::repository::dependency_model::ProvideVersionRelation::Equal),
        version_scheme: VersionScheme::Eopkg,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
        provenance: CapabilityProvenance::ExactIdentity,
    }];
    for (kind, names) in [
        (RepositoryCapabilityKind::PkgConfig, &metadata.pkg_config),
        (
            RepositoryCapabilityKind::PkgConfig32,
            &metadata.pkg_config_32,
        ),
        (RepositoryCapabilityKind::Comar, &metadata.comar),
    ] {
        for (index, name) in names.iter().enumerate() {
            provides.push(ProvidedCapability {
                kind,
                name: name.clone(),
                version: None,
                version_relation: None,
                version_scheme: VersionScheme::Eopkg,
                architecture_qualifier: ProvideArchitectureQualifier::Implicit,
                provenance: CapabilityProvenance::SourceDeclared {
                    format: crate::repository::dependency_model::SourcePackageFormat::Eopkg,
                    record_index: u32::try_from(index).map_err(|_| {
                        Error::ParseError("eopkg provide index exceeds u32".to_string())
                    })?,
                },
            });
        }
    }
    Ok(provides)
}

pub fn query_user_installed() -> Result<std::collections::HashSet<String>> {
    InstalledEopkgDatabase::load()?.user_installed()
}

#[cfg(test)]
mod tests {
    use super::*;

    const METADATA: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/eopkg/jq-metadata.xml"
    ));
    const FILES: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/eopkg/jq-files.xml"
    ));

    #[test]
    fn authentic_installed_metadata_preserves_exact_identity() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("jq-1.8.2-13");
        fs::create_dir(&package).unwrap();
        fs::write(package.join("metadata.xml"), METADATA).unwrap();
        fs::write(package.join("files.xml"), FILES).unwrap();
        let records = query_installed_root(root.path()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].identity.selector(), "jq");
        assert_eq!(records[0].identity.version(), "1.8.2-13");
        assert_eq!(records[0].identity.architecture(), "x86_64");
        assert_eq!(records[0].info.package_format, "1.2");
    }

    #[test]
    fn installed_database_snapshot_does_not_reread_package_xml() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("jq-1.8.2-13");
        fs::create_dir(&package).unwrap();
        fs::write(package.join("metadata.xml"), METADATA).unwrap();
        fs::write(package.join("files.xml"), FILES).unwrap();

        let database = InstalledEopkgDatabase::load_root(root.path()).unwrap();
        fs::remove_dir_all(package).unwrap();

        let package = database.package("jq").unwrap();
        assert_eq!(package.identity.version(), "1.8.2-13");
        assert_eq!(database.package_requirement_groups("jq").unwrap().len(), 2);
        let provides = database.package_provides(&package.identity).unwrap();
        assert!(provides.iter().any(|provide| provide.name == "jq"));
    }

    #[test]
    fn authentic_files_authority_preserves_symlink_and_config_fields() {
        let files = xml::parse_files(FILES).unwrap();
        assert_eq!(files.len(), 8);
        let symlink = files
            .iter()
            .find(|file| file.path == "usr/lib64/libjq.so.1")
            .expect("jq soname symlink");
        assert_eq!(symlink.size, Some(14));
        assert_eq!(
            symlink.sha1.as_deref(),
            Some("13c7cdb59b2e51b8faf28878311e237b7c372f7e")
        );
        assert!(files.iter().all(|file| !file.path.starts_with('/')));
    }

    #[test]
    fn files_authority_accumulates_xml_character_references_inside_paths() {
        let files = xml::parse_files(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Files><File>
<Path>usr/share/backgrounds/exhibici&#xf3;n &amp; freestyle.jxl</Path>
<Type>data</Type><Size>1</Size><Uid>0</Uid><Gid>0</Gid>
<Mode>0644</Mode><Hash>0000000000000000000000000000000000000000</Hash>
</File></Files>"#,
        )
        .unwrap();

        assert_eq!(
            files[0].path,
            "usr/share/backgrounds/exhibición & freestyle.jxl"
        );
    }

    #[test]
    fn metadata_authority_accumulates_named_and_character_references() {
        let metadata = xml::parse_metadata(
            br#"<PISI><Package><Name>lib&#x66;oo</Name>
<Summary>Tom &amp; Jerry</Summary><RuntimeDependencies><Dependency>glibc&amp;abi</Dependency></RuntimeDependencies>
<History><Update release="2"><Version>1.&#48;</Version></Update></History>
<Distribution>Solus</Distribution><DistributionRelease>4.9</DistributionRelease>
<Architecture>x86_64</Architecture><PackageFormat>1.2</PackageFormat></Package></PISI>"#,
        )
        .unwrap();

        assert_eq!(metadata.name, "libfoo");
        assert_eq!(metadata.summary.as_deref(), Some("Tom & Jerry"));
        assert_eq!(metadata.version, "1.0");
        assert_eq!(metadata.dependencies[0][0].name, "glibc&abi");
    }
}
