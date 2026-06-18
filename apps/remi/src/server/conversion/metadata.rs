// apps/remi/src/server/conversion/metadata.rs
//! Package metadata extraction, safe CCS filenames, and native provide merging.

use super::ConversionService;
use anyhow::{Result, anyhow};
use conary_core::db::models::{RepositoryPackage, RepositoryProvide};
use conary_core::filesystem::path::sanitize_filename;
use conary_core::packages::arch::ArchPackage;
use conary_core::packages::common::PackageMetadata;
use conary_core::packages::deb::DebPackage;
use conary_core::packages::rpm::RpmPackage;
use conary_core::packages::traits::{Dependency, DependencyType, ExtractedFile, PackageFormat};
use conary_core::repository::supported_profiles::ProfilePackageFormat;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

impl ConversionService {
    /// Create a safe CCS filename from package name and version
    ///
    /// Sanitizes both name and version to prevent path traversal attacks
    /// where malicious package metadata could escape the packages directory.
    pub(super) fn safe_ccs_filename(name: &str, version: &str) -> Result<String> {
        Self::safe_ccs_filename_with_arch(name, version, None)
    }

    pub(super) fn safe_ccs_filename_with_arch(
        name: &str,
        version: &str,
        architecture: Option<&str>,
    ) -> Result<String> {
        let safe_name = sanitize_filename(name)
            .map_err(|e| anyhow!("Invalid package name '{}': {}", name, e))?;
        let safe_version = sanitize_filename(version)
            .map_err(|e| anyhow!("Invalid package version '{}': {}", version, e))?;
        if let Some(arch) = architecture {
            let safe_arch = sanitize_filename(arch)
                .map_err(|e| anyhow!("Invalid package architecture '{}': {}", arch, e))?;
            Ok(format!("{}-{}-{}.ccs", safe_name, safe_version, safe_arch))
        } else {
            Ok(format!("{}-{}.ccs", safe_name, safe_version))
        }
    }

    pub(super) fn apply_repository_identity(
        metadata: &mut PackageMetadata,
        repo_pkg: &RepositoryPackage,
    ) {
        metadata.name = repo_pkg.name.clone();
        metadata.version = repo_pkg.version.clone();
        if let Some(architecture) = &repo_pkg.architecture {
            metadata.architecture = Some(architecture.clone());
        }
    }

    /// Parse a downloaded package file
    pub(super) fn parse_package(
        &self,
        path: &Path,
        distro: &str,
    ) -> Result<(PackageMetadata, Vec<ExtractedFile>, &'static str)> {
        let path_str = path.to_str().ok_or_else(|| anyhow!("Invalid path"))?;
        let route = conary_core::repository::supported_profiles::route_by_slug(distro)
            .ok_or_else(|| anyhow!("Unsupported distribution: {}", distro))?;
        let profile_id = route
            .public_profile_ids()
            .first()
            .ok_or_else(|| anyhow!("No public profile for route: {}", distro))?;
        let profile = conary_core::repository::supported_profiles::profile_by_public_id(profile_id)
            .ok_or_else(|| anyhow!("Profile disappeared for route: {}", distro))?;

        match profile.package_format() {
            ProfilePackageFormat::Arch => {
                let pkg = ArchPackage::parse(path_str)
                    .map_err(|e| anyhow!("Failed to parse Arch package: {}", e))?;
                let files = pkg
                    .extract_file_contents()
                    .map_err(|e| anyhow!("Failed to extract Arch package contents: {}", e))?;
                let metadata = Self::build_metadata(&pkg);
                Ok((metadata, files, "arch"))
            }
            ProfilePackageFormat::Rpm => {
                let pkg = RpmPackage::parse(path_str)
                    .map_err(|e| anyhow!("Failed to parse RPM package: {}", e))?;
                let files = pkg
                    .extract_file_contents()
                    .map_err(|e| anyhow!("Failed to extract RPM package contents: {}", e))?;
                let metadata = Self::build_metadata(&pkg);
                Ok((metadata, files, "rpm"))
            }
            ProfilePackageFormat::Deb => {
                let pkg = DebPackage::parse(path_str)
                    .map_err(|e| anyhow!("Failed to parse DEB package: {}", e))?;
                let files = pkg
                    .extract_file_contents()
                    .map_err(|e| anyhow!("Failed to extract DEB package contents: {}", e))?;
                let metadata = Self::build_metadata(&pkg);
                Ok((metadata, files, "deb"))
            }
        }
    }

    pub(super) fn merge_repository_provides(
        conn: &rusqlite::Connection,
        repo_pkg: &RepositoryPackage,
        metadata: &mut PackageMetadata,
    ) -> Result<()> {
        let Some(repository_package_id) = repo_pkg.id else {
            return Ok(());
        };

        let repo_provides =
            RepositoryProvide::find_by_repository_package(conn, repository_package_id)?;
        if repo_provides.is_empty() {
            return Ok(());
        }

        let mut seen: HashSet<(String, Option<String>)> = metadata
            .provides
            .iter()
            .map(|provide| (provide.name.clone(), provide.version.clone()))
            .collect();

        for provide in repo_provides {
            if should_skip_repository_provide(&provide, metadata) {
                continue;
            }

            let version = repository_provide_constraint(&provide);
            if !seen.insert((provide.capability.clone(), version.clone())) {
                continue;
            }

            metadata.provides.push(Dependency {
                name: provide.capability,
                version,
                dep_type: DependencyType::Runtime,
                description: None,
            });
        }

        Ok(())
    }

    /// Build PackageMetadata from a parsed package
    fn build_metadata<P: PackageFormat>(pkg: &P) -> PackageMetadata {
        PackageMetadata {
            package_path: PathBuf::new(), // Not needed for conversion
            name: pkg.name().to_string(),
            version: pkg.version().to_string(),
            architecture: pkg.architecture().map(String::from),
            description: pkg.description().map(String::from),
            files: pkg
                .files()
                .iter()
                .map(|f| conary_core::packages::traits::PackageFile {
                    path: f.path.clone(),
                    size: f.size,
                    mode: f.mode,
                    sha256: f.sha256.clone(),
                    symlink_target: f.symlink_target.clone(),
                })
                .collect(),
            dependencies: pkg
                .dependencies()
                .iter()
                .map(|d| conary_core::packages::traits::Dependency {
                    name: d.name.clone(),
                    version: d.version.clone(),
                    dep_type: d.dep_type,
                    description: d.description.clone(),
                })
                .collect(),
            provides: pkg.provides().to_vec(),
            scriptlets: pkg.scriptlets().to_vec(),
            native_scriptlet_abi: pkg.native_scriptlet_abi().to_vec(),
            config_files: pkg.config_files().to_vec(),
        }
    }
}

fn should_skip_repository_provide(provide: &RepositoryProvide, metadata: &PackageMetadata) -> bool {
    provide.capability.is_empty()
        || provide.capability == metadata.name
        || provide.capability.starts_with('/')
        || provide.capability.starts_with("rpmlib(")
        || provide.kind == "file"
}

fn repository_provide_constraint(provide: &RepositoryProvide) -> Option<String> {
    if let Some(raw) = provide.raw.as_deref()
        && let Some(constraint) = constraint_from_raw_provide(raw, &provide.capability)
    {
        return Some(constraint);
    }

    provide
        .version
        .as_deref()
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .map(|version| format!("= {version}"))
}

fn constraint_from_raw_provide(raw: &str, capability: &str) -> Option<String> {
    let suffix = raw.strip_prefix(capability)?.trim_start();
    if suffix.is_empty() {
        return None;
    }

    for op in ["<=", ">=", "=", "<", ">"] {
        if let Some(version) = suffix.strip_prefix(op) {
            let version = version.trim();
            if !version.is_empty() {
                return Some(format!("{op} {version}"));
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{create_test_db, insert_repo};
    use super::*;
    use conary_core::db::models::{RepositoryPackage, RepositoryProvide};
    use conary_core::packages::common::PackageMetadata;
    use std::path::PathBuf;

    #[test]
    fn test_safe_ccs_filename_normal() {
        let result = ConversionService::safe_ccs_filename("nginx", "1.24.0-1.fc44").unwrap();
        assert_eq!(result, "nginx-1.24.0-1.fc44.ccs");
    }

    #[test]
    fn test_safe_ccs_filename_complex_name() {
        let result = ConversionService::safe_ccs_filename("lib32-glibc-devel", "2.38-1").unwrap();
        assert_eq!(result, "lib32-glibc-devel-2.38-1.ccs");
    }

    #[test]
    fn test_safe_ccs_filename_with_architecture() {
        let result = ConversionService::safe_ccs_filename_with_arch(
            "glib2",
            "2.86.0-2.fc44",
            Some("x86_64"),
        )
        .unwrap();
        assert_eq!(result, "glib2-2.86.0-2.fc44-x86_64.ccs");
    }

    #[test]
    fn test_apply_repository_identity_preserves_epoch_and_architecture() {
        let mut metadata = PackageMetadata::new(
            PathBuf::from("/tmp/qemu-img.rpm"),
            "qemu-img".to_string(),
            "10.1.0-7.fc44".to_string(),
        );
        metadata.architecture = Some("i686".to_string());

        let mut repo_pkg = RepositoryPackage::new(
            42,
            "qemu-img".to_string(),
            "2:10.1.0-7.fc44".to_string(),
            "sha256:qemu-img".to_string(),
            4096,
            "https://example.com/qemu-img.rpm".to_string(),
        );
        repo_pkg.architecture = Some("x86_64".to_string());

        ConversionService::apply_repository_identity(&mut metadata, &repo_pkg);

        assert_eq!(metadata.name, "qemu-img");
        assert_eq!(metadata.version, "2:10.1.0-7.fc44");
        assert_eq!(metadata.architecture.as_deref(), Some("x86_64"));
    }

    #[test]
    fn test_safe_ccs_filename_rejects_path_traversal_in_name() {
        let result = ConversionService::safe_ccs_filename("../../../etc/passwd", "1.0");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Invalid package name"));
    }

    #[test]
    fn test_safe_ccs_filename_rejects_path_traversal_in_version() {
        let result = ConversionService::safe_ccs_filename("nginx", "../../evil");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Invalid package version"));
    }

    #[test]
    fn test_safe_ccs_filename_rejects_slash_in_name() {
        let result = ConversionService::safe_ccs_filename("foo/bar", "1.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_safe_ccs_filename_rejects_empty_name() {
        let result = ConversionService::safe_ccs_filename("", "1.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_safe_ccs_filename_rejects_empty_version() {
        let result = ConversionService::safe_ccs_filename("nginx", "");
        assert!(result.is_err());
    }

    #[test]
    fn repository_native_provides_are_merged_into_conversion_metadata() {
        let (_temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");

        let mut repo_pkg = RepositoryPackage::new(
            repo_id,
            "kernel-modules-core".to_string(),
            "6.17.1-300.fc44".to_string(),
            "sha256:kernel-modules-core".to_string(),
            1024,
            "https://example.com/kernel-modules-core.rpm".to_string(),
        );
        repo_pkg.insert(&conn).unwrap();
        let repo_pkg = repo_pkg;
        let repo_pkg_id = repo_pkg.id.unwrap();

        let mut provide = RepositoryProvide::new(
            repo_pkg_id,
            "kernel-uname-r".to_string(),
            Some("6.17.1-300.fc44.x86_64".to_string()),
            "package".to_string(),
            Some("kernel-uname-r = 6.17.1-300.fc44.x86_64".to_string()),
        );
        provide = provide.with_version_scheme("rpm".to_string());
        provide.insert(&conn).unwrap();

        let mut metadata = PackageMetadata::new(
            PathBuf::from("/tmp/kernel-modules-core.rpm"),
            "kernel-modules-core".to_string(),
            "6.17.1-300.fc44".to_string(),
        );

        ConversionService::merge_repository_provides(&conn, &repo_pkg, &mut metadata).unwrap();

        assert!(metadata.provides.iter().any(|provide| {
            provide.name == "kernel-uname-r"
                && provide.version.as_deref() == Some("= 6.17.1-300.fc44.x86_64")
        }));
    }
}
