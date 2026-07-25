// conary-core/src/packages/rpm.rs

//! RPM package format parser

use crate::db::models::Trove;
use crate::error::{Error, Result};
use crate::packages::common::PackageMetadata;
use crate::packages::traits::{
    ConfigFileInfo, Dependency, DependencyType, ExtractedFile, NativeArgumentContract,
    NativeArgumentValue, NativeEnvironmentFact, NativeInvocationContract, NativeLifecyclePath,
    NativeRootExpectation, NativeScriptletBody, NativeScriptletEntry, NativeScriptletFormat,
    NativeScriptletKind, NativeScriptletMetadata, NativeScriptletSupport, NativeStdinContract,
    NativeTransactionOrder, NativeTransactionPosition, PackageFile, PackageFormat,
    RpmHeaderContextMetadata, RpmHeaderFactMetadata, RpmHeaderFactSource, RpmHeaderValueMetadata,
    RpmMacroContextMetadata, RpmMacroDefinitionMetadata, RpmMacroDefinitionSource,
    RpmNativeScriptletMetadata, RpmScriptletCriticality, RpmScriptletFlagsMetadata,
    RpmScriptletProgram, RpmScriptletRuntimeMetadata, RpmScriptletSlot, RpmSysusersDirective,
    RpmSysusersMetadata, RpmTriggerAction, RpmTriggerCondition, RpmTriggerFamily,
    RpmTriggerMetadata,
};
use crate::repository::dependency_model::{RepositoryRequirementGroup, RepositoryRequirementKind};
use crate::repository::package_relation::parse_native_relation;
use crate::repository::versioning::VersionScheme;
use rpm::{DependencyFlags, Package};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use tracing::debug;

mod payload;
/// RPM package representation
mod scriptlets;
mod sysusers;
pub struct RpmPackage {
    /// Common package metadata
    meta: PackageMetadata,
    // RPM-specific provenance information
    source_rpm: Option<String>,
    build_host: Option<String>,
    vendor: Option<String>,
    license: Option<String>,
    url: Option<String>,
    payload: Vec<ExtractedFile>,
}

impl RpmPackage {
    /// Extract config files from RPM package using metadata
    fn extract_config_files(pkg: &Package) -> Result<Vec<ConfigFileInfo>> {
        use rpm::FileFlags;
        let mut config_files = Vec::new();

        for entry in pkg.metadata.get_file_entries().map_err(|error| {
            Error::ParseError(format!("Failed to read RPM config-file metadata: {error}"))
        })? {
            if entry.flags().contains(FileFlags::CONFIG) {
                config_files.push(ConfigFileInfo {
                    path: entry.path().to_string_lossy().to_string(),
                    noreplace: entry.flags().contains(FileFlags::NOREPLACE),
                    ghost: entry.flags().contains(FileFlags::GHOST),
                    remove_on_upgrade: false,
                });
            }
        }

        Ok(config_files)
    }

    /// Parse RPM Requires into the formal Boolean requirement grammar.
    fn extract_requirements(pkg: &Package) -> Result<Vec<RepositoryRequirementGroup>> {
        pkg.metadata
            .get_requires()
            .map_err(|error| Error::ParseError(format!("Failed to read RPM requires: {error}")))?
            .into_iter()
            .filter(|requirement| {
                !requirement
                    .flags
                    .intersects(DependencyFlags::RPMLIB | DependencyFlags::CONFIG)
            })
            .map(|requirement| {
                let native_text = if requirement.name.starts_with('(') {
                    if !requirement.version.is_empty()
                        || requirement.flags.intersects(
                            DependencyFlags::LESS
                                | DependencyFlags::GREATER
                                | DependencyFlags::EQUAL,
                        )
                    {
                        return Err(Error::ParseError(format!(
                            "RPM rich dependency '{}' carries an invalid separate version constraint",
                            requirement.name
                        )));
                    }
                    requirement.name.to_string()
                } else {
                    rpm_relation_text(
                        &requirement.name,
                        &requirement.version,
                        requirement.flags,
                    )?
                };
                crate::repository::requirement::parse_native_requirement(
                    RepositoryRequirementKind::Depends,
                    VersionScheme::Rpm,
                    &native_text,
                )
                .map_err(Error::ParseError)
            })
            .collect()
    }

    /// Extract native provides from RPM package metadata.
    fn extract_provides(pkg: &Package) -> Result<Vec<Dependency>> {
        let mut provides = Vec::new();

        for entry in pkg
            .metadata
            .get_provides()
            .map_err(|error| Error::ParseError(format!("Failed to read RPM provides: {error}")))?
        {
            if entry
                .flags
                .intersects(DependencyFlags::RPMLIB | DependencyFlags::CONFIG)
            {
                continue;
            }

            let version = if !entry.version.is_empty() {
                let operator = flags_to_operator(entry.flags);
                Some(format!("{}{}", operator, entry.version))
            } else {
                None
            };

            provides.push(Dependency {
                name: entry.name.to_string(),
                version,
                dep_type: DependencyType::Runtime,
                description: None,
            });
        }

        Ok(provides)
    }

    fn optional_header_string(
        value: std::result::Result<&str, rpm::Error>,
        field: &str,
    ) -> Result<Option<String>> {
        match value {
            Ok(value) => Ok(Some(value.to_string())),
            Err(rpm::Error::TagNotFound(_)) => Ok(None),
            Err(error) => Err(Error::ParseError(format!(
                "Failed to read optional RPM {field}: {error}"
            ))),
        }
    }

    fn extract_relations(pkg: &Package) -> Result<Vec<RepositoryRequirementGroup>> {
        let mut relations = Vec::new();
        for (kind, entries) in [
            (
                RepositoryRequirementKind::Conflict,
                pkg.metadata.get_conflicts().map_err(|error| {
                    Error::ParseError(format!("Failed to read RPM conflicts: {error}"))
                })?,
            ),
            (
                RepositoryRequirementKind::Obsolete,
                pkg.metadata.get_obsoletes().map_err(|error| {
                    Error::ParseError(format!("Failed to read RPM obsoletes: {error}"))
                })?,
            ),
        ] {
            for entry in entries {
                let native_text = rpm_relation_text(&entry.name, &entry.version, entry.flags)?;
                relations.push(
                    parse_native_relation(kind, VersionScheme::Rpm, &native_text)
                        .map_err(Error::ParseError)?,
                );
            }
        }
        Ok(relations)
    }
}

fn rpm_relation_text(name: &str, version: &str, flags: DependencyFlags) -> Result<String> {
    let comparison =
        flags & (DependencyFlags::LESS | DependencyFlags::GREATER | DependencyFlags::EQUAL);
    if version.is_empty() {
        if !comparison.is_empty() {
            return Err(Error::ParseError(format!(
                "RPM relation '{name}' has comparison flags but no version"
            )));
        }
        return Ok(name.to_string());
    }
    let operator = flags_to_operator(flags);
    if operator.is_empty() {
        return Err(Error::ParseError(format!(
            "RPM relation '{name}' has version '{version}' but no comparison operator"
        )));
    }
    Ok(format!("{name} {operator}{version}"))
}

/// Convert RPM DependencyFlags to constraint operator string
fn flags_to_operator(flags: rpm::DependencyFlags) -> &'static str {
    use rpm::DependencyFlags;

    // Check for combined flags first
    if flags.contains(DependencyFlags::LESS) && flags.contains(DependencyFlags::EQUAL) {
        "<= "
    } else if flags.contains(DependencyFlags::GREATER) && flags.contains(DependencyFlags::EQUAL) {
        ">= "
    } else if flags.contains(DependencyFlags::LESS) {
        "< "
    } else if flags.contains(DependencyFlags::GREATER) {
        "> "
    } else if flags.contains(DependencyFlags::EQUAL) {
        "= "
    } else {
        // No comparison flags (ANY) - return empty
        ""
    }
}

impl PackageFormat for RpmPackage {
    fn parse(path: &str) -> Result<Self> {
        debug!("Parsing RPM package: {}", path);

        let file = File::open(path)
            .map_err(|e| Error::InitError(format!("Failed to open RPM file: {}", e)))?;

        let mut buf_reader = BufReader::new(file);

        let pkg = Package::parse(&mut buf_reader)
            .map_err(|e| Error::InitError(format!("Failed to parse RPM: {}", e)))?;

        // Extract basic metadata
        let name = pkg
            .metadata
            .get_name()
            .map_err(|e| Error::InitError(format!("Failed to get package name: {}", e)))?
            .to_string();

        let version = pkg
            .metadata
            .get_version()
            .map_err(|e| Error::InitError(format!("Failed to get package version: {}", e)))?
            .to_string();

        // Combine version and release (e.g., "2.2.1" + "2.fc44" -> "2.2.1-2.fc44")
        let release = pkg
            .metadata
            .get_release()
            .map_err(|error| Error::ParseError(format!("Failed to get RPM release: {error}")))?;
        let version = format!("{}-{}", version, release);

        let architecture = Some(
            pkg.metadata
                .get_arch()
                .map_err(|error| {
                    Error::ParseError(format!("Failed to get RPM architecture: {error}"))
                })
                .map(crate::packages::common::normalize_architecture)?
                .to_string(),
        );
        let description =
            Self::optional_header_string(pkg.metadata.get_description(), "description")?;

        // Extract provenance information
        let source_rpm = Self::optional_header_string(pkg.metadata.get_source_rpm(), "source RPM")?;
        let build_host = Self::optional_header_string(pkg.metadata.get_build_host(), "build host")?;
        let vendor = Self::optional_header_string(pkg.metadata.get_vendor(), "vendor")?;
        let license = Self::optional_header_string(pkg.metadata.get_license(), "license")?;
        let url = Self::optional_header_string(pkg.metadata.get_url(), "URL")?;

        let payload = payload::parse(&pkg)?;
        let files: Vec<PackageFile> = payload
            .iter()
            .map(|file| PackageFile {
                path: file.path.clone(),
                node: file.node.clone(),
                content: file.content_authority.clone(),
            })
            .collect();
        let requirements = Self::extract_requirements(&pkg)?;
        let provides = Self::extract_provides(&pkg)?;
        let relations = Self::extract_relations(&pkg)?;

        // Extract exact native lifecycle entries and config-file policy.
        let native_scriptlet_abi = Self::extract_native_scriptlet_abi(&pkg)?;
        let config_files = Self::extract_config_files(&pkg)?;

        debug!(
            "Parsed RPM: {} version {} ({} files, {} dependencies, {} native lifecycle entries, {} config files)",
            name,
            version,
            files.len(),
            requirements.len(),
            native_scriptlet_abi.len(),
            config_files.len()
        );

        let meta = PackageMetadata {
            package_path: PathBuf::from(path),
            name,
            version,
            version_scheme: VersionScheme::Rpm,
            architecture,
            description,
            files,
            requirements,
            provides,
            relations,
            diagnostic_scriptlet_evidence: Vec::new(),
            native_scriptlet_abi,
            config_files,
        };

        Ok(Self {
            meta,
            source_rpm,
            build_host,
            vendor,
            license,
            url,
            payload,
        })
    }

    fn name(&self) -> &str {
        self.meta.name()
    }

    fn version(&self) -> &str {
        self.meta.version()
    }

    fn version_scheme(&self) -> VersionScheme {
        self.meta.version_scheme()
    }

    fn architecture(&self) -> Option<&str> {
        self.meta.architecture()
    }

    fn description(&self) -> Option<&str> {
        self.meta.description()
    }

    fn files(&self) -> &[PackageFile] {
        self.meta.files()
    }

    fn requirements(&self) -> &[RepositoryRequirementGroup] {
        self.meta.requirements()
    }

    fn provides(&self) -> &[Dependency] {
        self.meta.provides()
    }

    fn relations(&self) -> &[RepositoryRequirementGroup] {
        self.meta.relations()
    }

    fn extract_file_contents(&self) -> Result<Vec<ExtractedFile>> {
        Ok(self.payload.clone())
    }

    fn to_trove(&self) -> Trove {
        self.meta.to_trove()
    }

    fn native_scriptlet_abi(&self) -> &[NativeScriptletEntry] {
        self.meta.native_scriptlet_abi()
    }

    fn config_files(&self) -> &[ConfigFileInfo] {
        self.meta.config_files()
    }
}

impl RpmPackage {
    /// Get source RPM name (for provenance tracking)
    pub fn source_rpm(&self) -> Option<&str> {
        self.source_rpm.as_deref()
    }

    /// Get build host (for provenance tracking)
    pub fn build_host(&self) -> Option<&str> {
        self.build_host.as_deref()
    }

    /// Get vendor information
    pub fn vendor(&self) -> Option<&str> {
        self.vendor.as_deref()
    }

    /// Get license information
    pub fn license(&self) -> Option<&str> {
        self.license.as_deref()
    }

    /// Get upstream URL
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }
}

#[cfg(test)]
#[path = "rpm/tests.rs"]
mod tests;
