// apps/remi/src/server/conversion/metadata.rs
//! Package metadata extraction, safe CCS filenames, and repository identity checks.

use super::ConversionService;
use anyhow::{Context, Result, anyhow, ensure};
use conary_core::ccs::convert::ForeignConversionInput;
use conary_core::db::models::{RepositoryPackage, RepositoryProvide};
use conary_core::filesystem::path::sanitize_filename;
use conary_core::packages::arch::ArchPackage;
use conary_core::packages::deb::DebPackage;
use conary_core::packages::payload::PackagePayload;
use conary_core::packages::rpm::RpmPackage;
use conary_core::packages::traits::PackageFormat;
use conary_core::repository::supported_profiles::ProfilePackageFormat;
#[cfg(test)]
use conary_core::repository::versioning::VersionScheme;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub(super) struct RepositoryConversionMetadata {
    pub(super) digest: String,
}

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

    pub(super) fn validate_repository_identity(
        metadata: &ForeignConversionInput,
        repo_pkg: &RepositoryPackage,
    ) -> Result<()> {
        ensure!(
            metadata.name() == repo_pkg.name,
            "repository package name contradicts authenticated artifact identity"
        );
        ensure!(
            metadata.version() == repo_pkg.version,
            "repository package version contradicts authenticated artifact identity"
        );
        ensure!(
            metadata.version_scheme() == repo_pkg.version_scheme,
            "repository version scheme contradicts authenticated artifact identity"
        );
        ensure!(
            metadata.architecture() == repo_pkg.architecture.as_deref(),
            "repository architecture contradicts authenticated artifact identity"
        );
        Ok(())
    }

    /// Parse a downloaded package file
    pub(super) fn parse_package(
        &self,
        path: &Path,
        source_profile: &str,
    ) -> Result<(ForeignConversionInput, PackagePayload, &'static str)> {
        let path_str = path.to_str().ok_or_else(|| anyhow!("Invalid path"))?;
        let profile =
            conary_core::repository::supported_profiles::profile_by_public_id(source_profile)
                .ok_or_else(|| anyhow!("unsupported exact source profile '{source_profile}'"))?;

        match profile.package_format() {
            ProfilePackageFormat::Arch => {
                let pkg = ArchPackage::parse(path_str)
                    .map_err(|e| anyhow!("Failed to parse Arch package: {}", e))?;
                let files = pkg
                    .package_payload()
                    .map_err(|e| anyhow!("Failed to open Arch package payload: {}", e))?;
                let metadata = Self::build_metadata(&pkg)?;
                Ok((metadata, files, "arch"))
            }
            ProfilePackageFormat::Rpm => {
                let pkg = RpmPackage::parse(path_str)
                    .map_err(|e| anyhow!("Failed to parse RPM package: {}", e))?;
                let files = pkg
                    .package_payload()
                    .map_err(|e| anyhow!("Failed to open RPM package payload: {}", e))?;
                let metadata = Self::build_metadata(&pkg)?;
                Ok((metadata, files, "rpm"))
            }
            ProfilePackageFormat::Deb => {
                let pkg = DebPackage::parse(path_str)
                    .map_err(|e| anyhow!("Failed to parse DEB package: {}", e))?;
                let files = pkg
                    .package_payload()
                    .map_err(|e| anyhow!("Failed to open DEB package payload: {}", e))?;
                let metadata = Self::build_metadata(&pkg)?;
                Ok((metadata, files, "deb"))
            }
        }
    }

    pub(super) async fn load_repository_conversion_metadata_async(
        &self,
        repo_pkg: &RepositoryPackage,
    ) -> Result<RepositoryConversionMetadata> {
        let db_path = self.db_path.clone();
        let expected_package = repo_pkg.clone();
        let repository_package_id = repo_pkg
            .id
            .ok_or_else(|| anyhow!("repository conversion package has no persisted identity"))?;
        tokio::task::spawn_blocking(move || {
            let mut conn = conary_core::db::open(&db_path)?;
            let tx = conn.transaction()?;
            let current_package = RepositoryPackage::find_by_id(&tx, repository_package_id)?
                .context("repository package metadata changed before conversion")?;
            ensure!(
                repository_package_conversion_inputs_match(&expected_package, &current_package),
                "repository package identity changed before conversion"
            );
            let (_, digest) =
                RepositoryProvide::conversion_capabilities_with_digest(&tx, repository_package_id)?;
            tx.commit()?;
            Ok(RepositoryConversionMetadata { digest })
        })
        .await
        .map_err(|error| anyhow!("repository conversion metadata task panicked: {error}"))?
    }

    fn build_metadata<P: PackageFormat>(pkg: &P) -> Result<ForeignConversionInput> {
        ForeignConversionInput::from_package(PathBuf::new(), pkg).map_err(Into::into)
    }
}

pub(super) fn repository_package_conversion_inputs_match(
    expected: &RepositoryPackage,
    current: &RepositoryPackage,
) -> bool {
    expected.id == current.id
        && expected.repository_id == current.repository_id
        && expected.name == current.name
        && expected.version == current.version
        && expected.package_release == current.package_release
        && expected.architecture == current.architecture
        && expected.debian_multi_arch == current.debian_multi_arch
        && bare_sha256(&expected.checksum) == bare_sha256(&current.checksum)
        && expected.source_profile == current.source_profile
        && expected.version_scheme == current.version_scheme
}

fn bare_sha256(value: &str) -> &str {
    value.strip_prefix("sha256:").unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{create_test_db, insert_repo};
    use super::*;
    use conary_core::ccs::convert::ForeignConversionInput;
    use conary_core::db::models::{RepositoryPackage, RepositoryProvide};
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
    fn repository_identity_cannot_override_authenticated_artifact() {
        let mut metadata = ForeignConversionInput::new(
            PathBuf::from("/tmp/qemu-img.rpm"),
            "qemu-img".to_string(),
            "10.1.0-7.fc44".to_string(),
            VersionScheme::Rpm,
        );
        let conary_core::packages::source_authority::SourcePackageAuthority::Ccs(authority) =
            &mut metadata.source_authority
        else {
            unreachable!()
        };
        authority.architecture = Some("i686".to_string());

        let mut repo_pkg = RepositoryPackage::new(
            42,
            "qemu-img".to_string(),
            "2:10.1.0-7.fc44".to_string(),
            VersionScheme::Rpm,
            "sha256:qemu-img".to_string(),
            4096,
            "https://example.com/qemu-img.rpm".to_string(),
        );
        repo_pkg.architecture = Some("x86_64".to_string());

        let error =
            ConversionService::validate_repository_identity(&metadata, &repo_pkg).unwrap_err();
        assert!(error.to_string().contains("version contradicts"));
        assert_eq!(metadata.version(), "10.1.0-7.fc44");
        assert_eq!(metadata.architecture(), Some("i686"));
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
    fn repository_native_provides_do_not_mutate_artifact_authority() {
        let (_temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");

        let mut repo_pkg = RepositoryPackage::new(
            repo_id,
            "kernel-modules-core".to_string(),
            "6.17.1-300.fc44".to_string(),
            VersionScheme::Rpm,
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
            VersionScheme::Rpm,
        );
        provide.insert(&conn).unwrap();

        let metadata = ForeignConversionInput::new(
            PathBuf::from("/tmp/kernel-modules-core.rpm"),
            "kernel-modules-core".to_string(),
            "6.17.1-300.fc44".to_string(),
            VersionScheme::Rpm,
        );

        let (_, digest) =
            RepositoryProvide::conversion_capabilities_with_digest(&conn, repo_pkg_id).unwrap();
        let repository_metadata = RepositoryConversionMetadata { digest };

        assert!(
            metadata
                .source_authority
                .declared_capabilities()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            repository_metadata.digest,
            RepositoryProvide::conversion_capabilities_digest(&conn, repo_pkg_id).unwrap()
        );
    }
}
