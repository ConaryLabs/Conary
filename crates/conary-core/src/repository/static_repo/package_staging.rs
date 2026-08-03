// conary-core/src/repository/static_repo/package_staging.rs

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow, bail};

use crate::ccs::package::CcsPackage;
use crate::ccs::signing::SigningKeyPair;
use crate::ccs::v3::schema::{DependencyKindV3, ProvidedCapabilityV3};
use crate::ccs::verify::{TrustPolicy, verify_package};
use crate::hash;
use crate::packages::traits::PackageFormat;
use crate::repository::dependency_model::{
    ProvideArchitectureQualifier, RepositoryCapabilityKind, RepositoryProvide,
    RepositoryRequirementGroup,
};
use crate::repository::static_repo::publish_gate::AcceptedStaticSignerSet;
use crate::repository::static_repo::{StaticPackageEntry, validate_repo_relative_path};

const ATOMIC_WRITE_TEMP_ATTEMPTS: usize = 1024;
static ATOMIC_WRITE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
pub(crate) struct PendingPackageWrites {
    pub(crate) writes: Vec<PendingPackageWrite>,
    entries: Vec<StaticPackageEntry>,
    committed: bool,
}

pub(crate) struct PendingPackageWrite {
    pub(crate) entry: StaticPackageEntry,
    pub(crate) pending_path: PathBuf,
    pub(crate) final_path: PathBuf,
    pub(crate) promoted: bool,
}

impl PendingPackageWrites {
    pub(crate) fn package_entries(&self) -> Vec<&StaticPackageEntry> {
        self.entries.iter().collect()
    }

    pub(crate) fn target_entry(&self, relative: &str) -> Option<(u64, String)> {
        self.entries
            .iter()
            .find(|entry| entry.path == relative)
            .map(|entry| (entry.size, entry.sha256.clone()))
    }

    pub(crate) fn promote(&mut self) -> Result<()> {
        for write in &mut self.writes {
            if write.final_path.exists() {
                let existing = fs::read(&write.final_path)
                    .with_context(|| format!("read {}", write.final_path.display()))?;
                let pending = fs::read(&write.pending_path)
                    .with_context(|| format!("read {}", write.pending_path.display()))?;
                if existing == pending {
                    fs::remove_file(&write.pending_path)
                        .with_context(|| format!("remove {}", write.pending_path.display()))?;
                    continue;
                }
                bail!(
                    "immutable package artifact {} appeared during publish with different bytes",
                    write.entry.path
                );
            }
            if let Some(parent) = write.final_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create package directory {}", parent.display()))?;
            }
            fs::rename(&write.pending_path, &write.final_path).with_context(|| {
                format!(
                    "promote package {} to {}",
                    write.pending_path.display(),
                    write.final_path.display()
                )
            })?;
            write.promoted = true;
        }
        Ok(())
    }

    pub(crate) fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for PendingPackageWrites {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for write in &self.writes {
            let _ = fs::remove_file(&write.pending_path);
            if write.promoted {
                let _ = fs::remove_file(&write.final_path);
            }
        }
    }
}

pub(crate) fn stage_packages(
    repo_root: &Path,
    package_paths: &[PathBuf],
    accepted_signers: &AcceptedStaticSignerSet,
    publish_key: &SigningKeyPair,
) -> Result<PendingPackageWrites> {
    let trust_policy = TrustPolicy::strict(accepted_signers.trusted_public_keys());
    let mut pending = PendingPackageWrites::default();
    for package_path in package_paths {
        let verified = verify_package(package_path, &trust_policy)
            .with_context(|| format!("verify CCS package {}", package_path.display()))?;
        let package_path_str = package_path.to_str().ok_or_else(|| {
            anyhow!(
                "package path is not valid UTF-8: {}",
                package_path.display()
            )
        })?;
        let package = CcsPackage::from_verified_archive(package_path_str, &verified)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("open verified CCS package {}", package_path.display()))?;
        let signed_bytes = if verified.signature().public_key == publish_key.public_key_base64() {
            fs::read(package_path)
                .with_context(|| format!("read verified CCS package {}", package_path.display()))?
        } else {
            resign_current_package_bytes(&package, &verified, publish_key)
                .with_context(|| format!("sign current CCS package {}", package_path.display()))?
        };
        let relative = package_relative_path(&package)?;
        let entry = package_entry_from_package(&relative, &package, &signed_bytes)?;
        if pending
            .entries
            .iter()
            .any(|staged| staged.path == entry.path)
        {
            bail!("package publish input repeats immutable artifact {relative}");
        }
        pending.entries.push(entry.clone());
        let destination = repo_root.join(&relative);
        if let Some(existing) = read_optional(repo_root, &relative)? {
            if existing != signed_bytes {
                bail!("immutable package artifact {relative} already exists with different bytes");
            }
            continue;
        }
        let pending_path = write_pending_package(&destination, &signed_bytes)?;
        pending.writes.push(PendingPackageWrite {
            entry,
            pending_path,
            final_path: destination,
            promoted: false,
        });
    }

    Ok(pending)
}

fn resign_current_package_bytes(
    package: &CcsPackage,
    verified: &crate::ccs::verify::VerifiedCcsArchive,
    publish_key: &SigningKeyPair,
) -> Result<Vec<u8>> {
    use crate::ccs::v3::schema::PackageKindV3;

    let PackageKindV3::Package(_) = &verified.authority().kind else {
        bail!("static repository publication only stages CCS package payloads");
    };

    let output = tempfile::NamedTempFile::new()?;
    let debug_toml = verified
        .debug_toml()
        .map(std::str::from_utf8)
        .transpose()
        .context("decode verified CCS debug projection as UTF-8")?;
    crate::ccs::builder::write_v3_ccs_package_from_sources(
        verified.authority(),
        package.payload().files(),
        output.path(),
        publish_key,
        debug_toml,
        verified.build_attestation(),
        verified.foreign_conversion_boundary(),
    )?;
    verify_package(
        output.path(),
        &TrustPolicy::strict(vec![publish_key.public_key_base64()]),
    )
    .context("verify re-signed static repository package")?;
    fs::read(output.path())
        .with_context(|| format!("read re-signed package {}", output.path().display()))
}

fn write_pending_package(final_path: &Path, bytes: &[u8]) -> Result<PathBuf> {
    if let Some(parent) = final_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create package directory {}", parent.display()))?;
    }
    let (pending_path, mut file) = create_atomic_temp_file(final_path)?;
    if let Err(error) = write_atomic_temp_file(&pending_path, &mut file, bytes) {
        drop(file);
        let _ = fs::remove_file(&pending_path);
        return Err(error);
    }
    drop(file);
    Ok(pending_path)
}

fn package_relative_path(package: &CcsPackage) -> Result<String> {
    let arch = required_package_architecture(package)?;
    let release = required_package_release(package)?;
    Ok(format!(
        "packages/{}/{}-{}-{}-{}.ccs",
        package.name(),
        package.name(),
        package.version(),
        release,
        arch
    ))
}

fn package_entry_from_package(
    relative: &str,
    package: &CcsPackage,
    bytes: &[u8],
) -> Result<StaticPackageEntry> {
    let entry = StaticPackageEntry {
        name: package.name().to_string(),
        version: package.version().to_string(),
        version_scheme: package.manifest().package.version_scheme,
        release: required_package_release(package)?.to_string(),
        arch: required_package_architecture(package)?.to_string(),
        path: relative.to_string(),
        sha256: hash::sha256(bytes),
        size: bytes.len() as u64,
        description: package.description().map(str::to_string),
        provides: static_provides(package)?,
        requirements: static_requirements(package)?,
        relations: package.relations().to_vec(),
    };
    entry.validate()?;
    Ok(entry)
}

fn required_package_architecture(package: &CcsPackage) -> Result<&str> {
    package
        .architecture()
        .filter(|architecture| !architecture.is_empty())
        .ok_or_else(|| {
            anyhow!(
                "CCS package '{}-{}' has no declared architecture; static publication will not invent one",
                package.name(),
                package.version()
            )
        })
}

fn required_package_release(package: &CcsPackage) -> Result<&str> {
    let release = package.manifest().package.release.as_str();
    if release.is_empty() {
        bail!(
            "CCS package '{}-{}' has no declared release",
            package.name(),
            package.version()
        );
    }
    Ok(release)
}

fn static_provides(package: &CcsPackage) -> Result<Vec<RepositoryProvide>> {
    let mut provides = vec![RepositoryProvide::package_name(
        package.name().to_string(),
        Some(package.version().to_string()),
    )];
    let mut seen = std::collections::HashSet::from([(
        RepositoryCapabilityKind::PackageName,
        package.name().to_string(),
        Some(package.version().to_string()),
        Some(crate::repository::dependency_model::ProvideVersionRelation::Equal),
        ProvideArchitectureQualifier::Implicit,
        crate::repository::dependency_model::CapabilityProvenance::ExactIdentity,
    )]);

    let authority = package
        .v3_authority()
        .ok_or_else(|| anyhow!("verified CCS package is missing current v3 authority"))?;
    for provide in &authority.provided_capabilities {
        let provide = project_v3_provide(provide)?;
        let key = (
            provide.kind,
            provide.name.clone(),
            provide.version.clone(),
            provide.version_relation,
            provide.architecture_qualifier.clone(),
            provide.provenance.clone(),
        );
        if seen.insert(key) {
            provides.push(provide);
        }
    }
    Ok(provides)
}

fn project_v3_provide(entry: &ProvidedCapabilityV3) -> Result<RepositoryProvide> {
    if entry.target.is_some() || entry.component.is_some() {
        bail!(
            "CCS v3 provide '{}' has target/component selectors that the static index contract cannot represent",
            entry.name
        );
    }
    let kind = project_v3_capability(entry.kind);
    Ok(RepositoryProvide {
        name: entry.name.clone(),
        kind,
        version: entry.provider_version.clone(),
        version_relation: entry.version_relation,
        architecture_qualifier: entry.architecture_qualifier.clone(),
        native_text: Some(entry.name.clone()),
        provenance: entry.provenance.clone(),
    })
}

fn static_requirements(package: &CcsPackage) -> Result<Vec<RepositoryRequirementGroup>> {
    let requirements = package.requirements().to_vec();
    for requirement in &requirements {
        crate::repository::requirement::validate_requirement_group(
            requirement,
            package.version_scheme(),
        )
        .map_err(anyhow::Error::msg)?;
    }
    Ok(requirements)
}

const fn project_v3_capability(kind: DependencyKindV3) -> RepositoryCapabilityKind {
    match kind {
        DependencyKindV3::Package => RepositoryCapabilityKind::PackageName,
        DependencyKindV3::Capability => RepositoryCapabilityKind::Virtual,
        DependencyKindV3::File => RepositoryCapabilityKind::File,
        DependencyKindV3::Path => RepositoryCapabilityKind::Path,
        DependencyKindV3::Binary => RepositoryCapabilityKind::Binary,
        DependencyKindV3::Soname => RepositoryCapabilityKind::Soname,
        DependencyKindV3::PkgConfig => RepositoryCapabilityKind::PkgConfig,
    }
}

fn read_optional(root: &Path, relative: &str) -> Result<Option<Vec<u8>>> {
    validate_repo_relative_path(relative)?;
    let path = root.join(relative);
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

fn create_atomic_temp_file(path: &Path) -> Result<(PathBuf, File)> {
    for _ in 0..ATOMIC_WRITE_TEMP_ATTEMPTS {
        let tmp = unique_atomic_temp_path(path);
        match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(file) => return Ok((tmp, file)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("create temp file {}", tmp.display()));
            }
        }
    }

    bail!(
        "failed to create unique temp file next to {} after {} attempts",
        path.display(),
        ATOMIC_WRITE_TEMP_ATTEMPTS
    )
}

fn unique_atomic_temp_path(path: &Path) -> PathBuf {
    let suffix = ATOMIC_WRITE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("atomic-write");
    path.with_file_name(format!(".{filename}.tmp.{}.{}", std::process::id(), suffix))
}

fn write_atomic_temp_file(path: &Path, file: &mut File, bytes: &[u8]) -> Result<()> {
    file.write_all(bytes)
        .with_context(|| format!("write temp file {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync temp file {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dependency(kind: DependencyKindV3, name: &str) -> ProvidedCapabilityV3 {
        ProvidedCapabilityV3 {
            kind,
            name: name.to_string(),
            provider_version: None,
            version_relation: None,
            version_scheme: crate::repository::versioning::VersionScheme::Conary,
            architecture_qualifier: Default::default(),
            provenance: crate::repository::dependency_model::CapabilityProvenance::AuthorDeclared,
            target: None,
            component: None,
        }
    }

    fn requirement(
        name: &str,
        capability_kind: RepositoryCapabilityKind,
    ) -> RepositoryRequirementGroup {
        RepositoryRequirementGroup::simple(
            crate::repository::dependency_model::RepositoryRequirementKind::Depends,
            crate::repository::dependency_model::RepositoryRequirementClause {
                name: name.to_string(),
                capability_kind: Some(capability_kind),
                version_constraint: None,
                architecture_qualifier: Default::default(),
                native_text: None,
            },
        )
    }

    #[test]
    fn v3_static_projection_uses_declared_kinds_not_name_shape() {
        let mut authority =
            crate::ccs::v3::test_support::package_authority_with_one_file("typed-static");
        authority.provided_capabilities = vec![
            dependency(DependencyKindV3::Capability, "/looks/like/a/path.so"),
            dependency(DependencyKindV3::Soname, "libtyped.so.1"),
            dependency(DependencyKindV3::File, "/usr/lib/libtyped.so"),
        ];
        authority.requirements = vec![
            requirement(
                "/also/looks/like/a/path.so",
                RepositoryCapabilityKind::Virtual,
            ),
            requirement("binary(sh)", RepositoryCapabilityKind::Generic),
        ];
        let package = CcsPackage::from_v3_authority_for_tests(authority, None, None).unwrap();

        let provides = static_provides(&package).unwrap();
        assert!(provides.iter().any(|provide| {
            provide.name == "/looks/like/a/path.so"
                && provide.kind == RepositoryCapabilityKind::Virtual
        }));
        assert!(provides.iter().any(|provide| {
            provide.name == "libtyped.so.1" && provide.kind == RepositoryCapabilityKind::Soname
        }));
        assert!(provides.iter().any(|provide| {
            provide.name == "/usr/lib/libtyped.so" && provide.kind == RepositoryCapabilityKind::File
        }));

        let requirements = static_requirements(&package).unwrap();
        assert!(requirements.iter().any(|group| {
            let clause = &group.alternatives[0];
            clause.name == "/also/looks/like/a/path.so"
                && clause.capability_kind == Some(RepositoryCapabilityKind::Virtual)
        }));
        assert!(requirements.iter().any(|group| {
            let clause = &group.alternatives[0];
            clause.name == "binary(sh)"
                && clause.capability_kind == Some(RepositoryCapabilityKind::Generic)
        }));
    }

    #[test]
    fn static_projection_keeps_identity_and_same_name_source_capability() {
        let mut authority =
            crate::ccs::v3::test_support::package_authority_with_one_file("aspnet-runtime");
        authority.identity.version = "10.0.10.sdk110-1".to_string();
        authority.identity.version_scheme = crate::repository::versioning::VersionScheme::Arch;
        authority.provided_capabilities = vec![ProvidedCapabilityV3 {
            kind: DependencyKindV3::Package,
            name: "aspnet-runtime".to_string(),
            provider_version: Some("10.0".to_string()),
            version_relation: Some(
                crate::repository::dependency_model::ProvideVersionRelation::Equal,
            ),
            version_scheme: crate::repository::versioning::VersionScheme::Arch,
            architecture_qualifier: Default::default(),
            provenance: crate::repository::dependency_model::CapabilityProvenance::SourceDeclared {
                format: crate::repository::dependency_model::SourcePackageFormat::Alpm,
                record_index: 0,
            },
            target: None,
            component: None,
        }];
        let package = CcsPackage::from_v3_authority_for_tests(authority, None, None).unwrap();

        let provides = static_provides(&package).unwrap();

        assert_eq!(provides.len(), 2);
        assert_eq!(
            provides
                .iter()
                .filter(|provide| {
                    provide.provenance
                        == crate::repository::dependency_model::CapabilityProvenance::ExactIdentity
                })
                .count(),
            1
        );
        assert!(provides.iter().any(|provide| {
            provide.name == "aspnet-runtime"
                && provide.version.as_deref() == Some("10.0")
                && matches!(
                    provide.provenance,
                    crate::repository::dependency_model::CapabilityProvenance::SourceDeclared {
                        format: crate::repository::dependency_model::SourcePackageFormat::Alpm,
                        record_index: 0,
                    }
                )
        }));
    }

    #[test]
    fn static_projection_preserves_exact_v3_requirement_constraints() {
        let mut authority =
            crate::ccs::v3::test_support::package_authority_with_one_file("targeted-static");
        let mut requirement = requirement("target-runtime", RepositoryCapabilityKind::PackageName);
        requirement.alternatives[0].version_constraint = Some(">= 2.0.0".to_string());
        requirement.expression =
            crate::repository::dependency_model::RepositoryRequirementExpression::Atom(
                requirement.alternatives[0].clone(),
            );
        authority.requirements.push(requirement);
        let package = CcsPackage::from_v3_authority_for_tests(authority, None, None).unwrap();

        let requirements = static_requirements(&package).unwrap();
        assert_eq!(
            requirements[0].alternatives[0]
                .version_constraint
                .as_deref(),
            Some(">= 2.0.0")
        );
    }

    #[test]
    fn static_publication_rejects_missing_architecture_instead_of_inventing_noarch() {
        let mut authority =
            crate::ccs::v3::test_support::package_authority_with_one_file("missing-arch");
        authority.identity.architecture = None;
        let package = CcsPackage::from_v3_authority_for_tests(authority, None, None).unwrap();

        let error = package_relative_path(&package).unwrap_err().to_string();

        assert!(error.contains("has no declared architecture"), "{error}");
        assert!(error.contains("will not invent one"), "{error}");
    }

    #[test]
    fn static_publication_uses_the_exact_signed_release_identity() {
        let mut authority =
            crate::ccs::v3::test_support::package_authority_with_one_file("exact-release");
        authority.identity.release = "7".to_string();
        let package = CcsPackage::from_v3_authority_for_tests(authority, None, None).unwrap();

        assert_eq!(
            package_relative_path(&package).unwrap(),
            "packages/exact-release/exact-release-1.0.0-7-x86_64.ccs"
        );
    }
}
