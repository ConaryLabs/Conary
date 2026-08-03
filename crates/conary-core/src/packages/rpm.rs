// conary-core/src/packages/rpm.rs

//! RPM package format parser

use crate::db::models::Trove;
use crate::error::{Error, Result};
use crate::packages::payload::PackagePayload;
use crate::packages::rpm::authority::{RpmDeclaredCapability, RpmPackageAuthority};
use crate::packages::source_authority::SourcePackageAuthority;
use crate::packages::traits::{
    NativeArgumentContract, NativeArgumentValue, NativeEnvironmentFact, NativeInvocationContract,
    NativeLifecyclePath, NativeRootExpectation, NativeScriptletBody, NativeScriptletEntry,
    NativeScriptletFormat, NativeScriptletKind, NativeScriptletMetadata, NativeScriptletSupport,
    NativeStdinContract, NativeTransactionOrder, NativeTransactionPosition, PackageFile,
    PackageFormat, RpmHeaderContextMetadata, RpmHeaderFactMetadata, RpmHeaderFactSource,
    RpmHeaderValueMetadata, RpmMacroContextMetadata, RpmMacroDefinitionMetadata,
    RpmMacroDefinitionSource, RpmNativeScriptletMetadata, RpmScriptletCriticality,
    RpmScriptletFlagsMetadata, RpmScriptletProgram, RpmScriptletRuntimeMetadata, RpmScriptletSlot,
    RpmSysusersDirective, RpmSysusersMetadata, RpmTriggerAction, RpmTriggerCondition,
    RpmTriggerFamily, RpmTriggerMetadata,
};
use crate::repository::dependency_model::{
    RepositoryCapabilityKind, RepositoryRequirementGroup, RepositoryRequirementKind,
};
use crate::repository::package_relation::parse_native_relation;
use crate::repository::versioning::VersionScheme;
use rpm::{DependencyFlags, Package, PackageMetadata as RpmMetadata};
use std::fs::File;
use std::io::BufReader;
use tracing::debug;

pub mod authority;
mod payload;
/// RPM package representation
mod scriptlets;
mod sysusers;
pub struct RpmPackage {
    authority: RpmPackageAuthority,
    description: Option<String>,
    files: Vec<PackageFile>,
    requirements: Vec<RepositoryRequirementGroup>,
    relations: Vec<RepositoryRequirementGroup>,
    native_scriptlet_abi: Vec<NativeScriptletEntry>,
    // RPM-specific provenance information
    source_rpm: Option<String>,
    build_host: Option<String>,
    vendor: Option<String>,
    license: Option<String>,
    url: Option<String>,
    payload: PackagePayload,
}

impl RpmPackage {
    /// Parse RPM Requires into the formal Boolean requirement grammar.
    fn extract_requirements(pkg: &Package) -> Result<Vec<RepositoryRequirementGroup>> {
        let groups = pkg
            .metadata
            .get_requires()
            .map_err(|error| Error::ParseError(format!("Failed to read RPM requires: {error}")))?
            .into_iter()
            .map(|requirement| {
                decode_rpm_requirement(&requirement.name, &requirement.version, requirement.flags)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(groups.into_iter().flatten().collect())
    }

    /// Extract native provides from RPM package metadata.
    fn extract_declared_capability_records(pkg: &Package) -> Result<Vec<RpmDeclaredCapability>> {
        let mut provides = Vec::new();
        let package_name = pkg
            .metadata
            .get_name()
            .map_err(|error| Error::ParseError(format!("Failed to read RPM name: {error}")))?;

        for (record_index, entry) in pkg
            .metadata
            .get_provides()
            .map_err(|error| Error::ParseError(format!("Failed to read RPM provides: {error}")))?
            .into_iter()
            .enumerate()
        {
            // rpmlib provides are synthesized by the package-manager runtime,
            // not supplied by an installable package:
            // https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/lib/rpmds.cc#L1039-L1063
            let reserved_runtime =
                crate::repository::rpm_runtime::RpmRuntimeFeature::parse_capability(&entry.name)
                    .map_err(|error| Error::ParseError(error.to_string()))?;
            if entry.flags.intersects(DependencyFlags::RPMLIB) || reserved_runtime.is_some() {
                return Err(Error::ParseError(format!(
                    "RPM package header cannot provide package-manager runtime capability '{}'; Conary's typed runtime ledger is the sole authority",
                    entry.name
                )));
            }
            validate_rpm_config_sense(&entry.name, entry.flags)?;

            let version_relation = rpm_provide_relation(&entry.name, &entry.version, entry.flags)?;
            let version = version_relation.map(|_| entry.version.to_string());

            let provide = RpmDeclaredCapability {
                header_index: u32::try_from(record_index).map_err(|_| {
                    Error::ParseError("RPM provide record index exceeds u32".to_string())
                })?,
                kind: if entry.name == package_name {
                    RepositoryCapabilityKind::PackageName
                } else {
                    RepositoryCapabilityKind::Generic
                },
                name: entry.name.to_string(),
                version,
                version_relation,
                architecture_qualifier: Default::default(),
            };
            provides.push(provide);
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

/// Decode one aligned RPM Requires header record into Conary's typed model.
///
/// Artifact parsing and installed-rpmdb adoption share this boundary so query
/// formatting cannot create a second dependency grammar. RPM itself accepts an
/// empty serialized epoch before `:` as epoch zero in `parseEVR()` at pinned
/// upstream commit a8f0192a; Conary canonicalizes that source spelling to an
/// omitted epoch while retaining the stricter canonical EVR grammar elsewhere.
pub(crate) fn decode_rpm_requirement(
    name: &str,
    version: &str,
    flags: DependencyFlags,
) -> Result<Option<RepositoryRequirementGroup>> {
    // RPMSENSE_CONFIG is not disposable annotation. RPM generates paired
    // config(NAME) requires/provides for packages containing %config files:
    // https://github.com/rpm-software-management/rpm/blob/a8f0192aee1c08bd1454ed2ac6ebaf506004b55c/build/rpmfc.cc#L1748-L1762
    // Conary installs those files, so preserve the exact package capability.
    let runtime_feature = crate::repository::rpm_runtime::RpmRuntimeFeature::parse_capability(name)
        .map_err(|error| Error::ParseError(error.to_string()))?;
    if flags.intersects(DependencyFlags::RPMLIB) && runtime_feature.is_none() {
        return Err(Error::ParseError(format!(
            "RPM requirement '{name}' carries RPMLIB sense outside the rpmlib(Feature) grammar"
        )));
    }
    validate_rpm_config_sense(name, flags)?;

    let native_text = if name.starts_with('(') {
        if !version.is_empty()
            || flags.intersects(
                DependencyFlags::LESS | DependencyFlags::GREATER | DependencyFlags::EQUAL,
            )
        {
            return Err(Error::ParseError(format!(
                "RPM rich dependency '{name}' carries an invalid separate version constraint"
            )));
        }
        name.to_string()
    } else {
        let canonical_version = canonicalize_rpm_evr(version);
        rpm_relation_text(name, canonical_version, flags)?
    };
    let group = crate::repository::requirement::parse_native_requirement(
        rpm_requirement_kind(flags),
        VersionScheme::Rpm,
        &native_text,
    )
    .map_err(Error::ParseError)?;
    crate::repository::rpm_runtime::simplify_runtime_requirement_group(group, VersionScheme::Rpm)
        .map_err(|error| Error::ParseError(error.to_string()))
}

/// Project RPM's install-order relation into the shared typed requirement
/// model.
///
/// This mirrors the pinned RPM transaction sorter: `%pre` and `%post`
/// dependencies are strong install edges, while legacy `Prereq` records are
/// mapped to `%pre`. Transaction script, erase, verification, runtime-feature,
/// and meta dependencies do not create strong install edges.
fn rpm_requirement_kind(flags: DependencyFlags) -> RepositoryRequirementKind {
    let non_legacy_context = DependencyFlags::POSTTRANS
        | DependencyFlags::PRETRANS
        | DependencyFlags::INTERP
        | DependencyFlags::SCRIPT_PRE
        | DependencyFlags::SCRIPT_POST
        | DependencyFlags::SCRIPT_PREUN
        | DependencyFlags::SCRIPT_POSTUN
        | DependencyFlags::SCRIPT_VERIFY
        | DependencyFlags::FIND_REQUIRES
        | DependencyFlags::RPMLIB
        | DependencyFlags::KEYRING
        | DependencyFlags::PREUNTRANS
        | DependencyFlags::POSTUNTRANS
        | DependencyFlags::META
        | DependencyFlags::MISSINGOK;
    let legacy_prerequisite =
        flags.intersects(DependencyFlags::PREREQ) && !flags.intersects(non_legacy_context);
    let ordered_install_script = flags
        .intersects(DependencyFlags::SCRIPT_PRE | DependencyFlags::SCRIPT_POST)
        && !flags.intersects(DependencyFlags::MISSINGOK);

    if legacy_prerequisite || ordered_install_script {
        RepositoryRequirementKind::PreDepends
    } else {
        RepositoryRequirementKind::Depends
    }
}

/// Canonicalize RPM's source-defined empty serialized epoch to omitted epoch
/// zero without weakening the repository-wide EVR grammar.
pub(crate) fn canonicalize_rpm_evr(version: &str) -> &str {
    version.strip_prefix(':').unwrap_or(version)
}

fn validate_rpm_config_sense(name: &str, flags: DependencyFlags) -> Result<()> {
    if !flags.intersects(DependencyFlags::CONFIG) {
        return Ok(());
    }
    let Some(inner) = name
        .strip_prefix("config(")
        .and_then(|name| name.strip_suffix(')'))
    else {
        return Err(Error::ParseError(format!(
            "RPM capability '{name}' carries CONFIG sense outside the config(Name) grammar"
        )));
    };
    if inner.is_empty() || inner.contains('(') || inner.contains(')') {
        return Err(Error::ParseError(format!(
            "RPM CONFIG capability '{name}' has an invalid package name"
        )));
    }
    Ok(())
}

fn rpm_provide_relation(
    name: &str,
    version: &str,
    flags: DependencyFlags,
) -> Result<Option<crate::repository::dependency_model::ProvideVersionRelation>> {
    use crate::repository::dependency_model::ProvideVersionRelation;

    let sense = flags & (DependencyFlags::LESS | DependencyFlags::GREATER | DependencyFlags::EQUAL);
    if version.is_empty() {
        if !sense.is_empty() {
            return Err(Error::ParseError(format!(
                "RPM provide '{name}' has comparison flags but no version"
            )));
        }
        return Ok(None);
    }
    let relation = if sense == DependencyFlags::LESS {
        ProvideVersionRelation::LessThan
    } else if sense == (DependencyFlags::LESS | DependencyFlags::EQUAL) {
        ProvideVersionRelation::LessOrEqual
    } else if sense == DependencyFlags::EQUAL {
        ProvideVersionRelation::Equal
    } else if sense == (DependencyFlags::GREATER | DependencyFlags::EQUAL) {
        ProvideVersionRelation::GreaterOrEqual
    } else if sense == DependencyFlags::GREATER {
        ProvideVersionRelation::GreaterThan
    } else {
        return Err(Error::ParseError(format!(
            "RPM provide '{name}' has version '{version}' with unsupported comparison flags {sense:?}"
        )));
    };
    Ok(Some(relation))
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

        let metadata = RpmMetadata::parse(&mut buf_reader)
            .map_err(|e| Error::InitError(format!("Failed to parse RPM: {}", e)))?;
        let pkg = Package {
            metadata,
            payload: Vec::new(),
        };

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

        // Preserve the exact RPM EVR identity. A non-zero epoch participates in
        // RPM ordering and must agree with repository metadata for the same
        // downloaded artifact.
        let release = pkg
            .metadata
            .get_release()
            .map_err(|error| Error::ParseError(format!("Failed to get RPM release: {error}")))?;
        let epoch =
            if pkg
                .metadata
                .header
                .entry_is_present(rpm::IndexTag::RPMTAG_EPOCH)
            {
                Some(pkg.metadata.get_epoch().map_err(|error| {
                    Error::ParseError(format!("Failed to get RPM epoch: {error}"))
                })?)
            } else {
                None
            };
        let evr = if let Some(epoch) = epoch {
            format!("{epoch}:{version}-{release}")
        } else {
            format!("{version}-{release}")
        };

        let architecture = Some(
            pkg.metadata
                .get_arch()
                .map_err(|error| {
                    Error::ParseError(format!("Failed to get RPM architecture: {error}"))
                })?
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

        let payload = payload::parse_stream(&pkg, Box::new(buf_reader))?;
        let files: Vec<PackageFile> = payload
            .files()
            .iter()
            .map(|file| PackageFile {
                path: file.path.clone(),
                node: file.node.clone(),
                content: file.content_authority.clone(),
            })
            .collect();
        let requirements = Self::extract_requirements(&pkg)?;
        let provides = Self::extract_declared_capability_records(&pkg)?;
        let relations = Self::extract_relations(&pkg)?;

        // Extract exact native lifecycle entries and config-file policy.
        let native_scriptlet_abi = Self::extract_native_scriptlet_abi(&pkg)?;
        let config = authority::parse_config_declarations(&pkg)?;

        debug!(
            "Parsed RPM: {} version {} ({} files, {} dependencies, {} native lifecycle entries, {} config files)",
            name,
            version,
            files.len(),
            requirements.len(),
            native_scriptlet_abi.len(),
            config.len()
        );

        let authority = RpmPackageAuthority {
            name,
            epoch,
            version_component: version,
            release: release.to_string(),
            evr,
            architecture: architecture.clone().expect("RPM architecture is required"),
            provides,
            config,
        };

        Ok(Self {
            authority,
            description,
            files,
            requirements,
            relations,
            native_scriptlet_abi,
            source_rpm,
            build_host,
            vendor,
            license,
            url,
            payload,
        })
    }

    fn name(&self) -> &str {
        &self.authority.name
    }

    fn version(&self) -> &str {
        &self.authority.evr
    }

    fn version_scheme(&self) -> VersionScheme {
        VersionScheme::Rpm
    }

    fn architecture(&self) -> Option<&str> {
        Some(&self.authority.architecture)
    }

    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    fn files(&self) -> &[PackageFile] {
        &self.files
    }

    fn requirements(&self) -> &[RepositoryRequirementGroup] {
        &self.requirements
    }

    fn resolution_capabilities(
        &self,
    ) -> Result<Vec<crate::repository::dependency_model::ProvidedCapability>> {
        SourcePackageAuthority::Rpm(self.authority.clone()).resolution_capabilities()
    }

    fn source_authority(&self) -> Result<SourcePackageAuthority> {
        Ok(SourcePackageAuthority::Rpm(self.authority.clone()))
    }

    fn relations(&self) -> &[RepositoryRequirementGroup] {
        &self.relations
    }

    fn package_payload(&self) -> Result<PackagePayload> {
        Ok(self.payload.clone())
    }

    fn to_trove(&self) -> Trove {
        SourcePackageAuthority::Rpm(self.authority.clone()).to_trove(self.description.clone())
    }

    fn native_scriptlet_abi(&self) -> &[NativeScriptletEntry] {
        &self.native_scriptlet_abi
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
