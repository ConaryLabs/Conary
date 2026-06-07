// apps/remi/src/server/conversion/safety.rs
//! Critical package and runtime capability refusal guards.

use super::ConversionService;
use anyhow::Result;
use conary_core::db::models::{RepositoryPackage, RepositoryProvide};
use conary_core::packages::common::PackageMetadata;

impl ConversionService {
    pub(super) fn ensure_package_name_not_critical(package_name: &str) -> Result<()> {
        if conary_core::critical_packages::is_critical_package_name(package_name) {
            anyhow::bail!(
                "Refusing to convert critical system package '{}'",
                package_name
            );
        }
        Ok(())
    }

    fn metadata_provides_critical_runtime(metadata: &PackageMetadata) -> Option<&str> {
        metadata
            .provides
            .iter()
            .map(|provide| provide.name.as_str())
            .find(|name| conary_core::critical_packages::is_critical_runtime_capability(name))
    }

    pub(super) fn ensure_metadata_not_critical(metadata: &PackageMetadata) -> Result<()> {
        if let Some(capability) = Self::metadata_provides_critical_runtime(metadata) {
            anyhow::bail!(
                "Refusing to convert critical runtime capability '{}' provided by package '{}'",
                capability,
                metadata.name
            );
        }
        Ok(())
    }

    fn repository_package_provides_critical_runtime(
        conn: &rusqlite::Connection,
        repo_pkg: &RepositoryPackage,
    ) -> Result<Option<String>> {
        let Some(repository_package_id) = repo_pkg.id else {
            return Ok(None);
        };

        let provides = RepositoryProvide::find_by_repository_package(conn, repository_package_id)?;
        Ok(provides
            .into_iter()
            .map(|provide| provide.capability)
            .find(|name| conary_core::critical_packages::is_critical_runtime_capability(name)))
    }

    pub(super) fn ensure_repository_package_not_critical(
        conn: &rusqlite::Connection,
        repo_pkg: &RepositoryPackage,
    ) -> Result<()> {
        if let Some(capability) =
            Self::repository_package_provides_critical_runtime(conn, repo_pkg)?
        {
            anyhow::bail!(
                "Refusing to convert critical runtime capability '{}' provided by package '{}'",
                capability,
                repo_pkg.name
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{create_test_db, insert_package, insert_repo};
    use super::*;
    use conary_core::db::models::RepositoryProvide;
    use conary_core::packages::common::PackageMetadata;
    use conary_core::packages::traits::{Dependency, DependencyType};
    use std::path::PathBuf;

    #[test]
    fn test_critical_packages_blocked() {
        for package_name in [
            "glibc",
            "systemd",
            "openssl-libs",
            "sudo",
            "coreutils",
            "ca-certificates",
        ] {
            assert!(ConversionService::ensure_package_name_not_critical(package_name).is_err());
        }
    }

    #[test]
    fn shared_critical_package_names_are_refused_by_conversion_guard() {
        for package_name in ["bash", "filesystem", "setup", "GLIBC"] {
            let err = ConversionService::ensure_package_name_not_critical(package_name)
                .expect_err("critical package should be refused")
                .to_string();
            assert!(err.contains("Refusing to convert critical system package"));
            assert!(err.contains(package_name));
        }

        ConversionService::ensure_package_name_not_critical("nginx").unwrap();
    }

    #[test]
    fn metadata_provides_critical_runtime_capabilities_are_detected() {
        let mut metadata = PackageMetadata::new(
            PathBuf::from("/tmp/alt-libc.rpm"),
            "alt-libc".to_string(),
            "1.0".to_string(),
        );
        metadata.provides.push(Dependency {
            name: "libc.so.6()(64bit)".to_string(),
            version: None,
            dep_type: DependencyType::Runtime,
            description: None,
        });

        assert_eq!(
            ConversionService::metadata_provides_critical_runtime(&metadata),
            Some("libc.so.6()(64bit)")
        );
    }

    #[test]
    fn repository_provides_guard_blocks_cached_conversion_path() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "alt-libc", "1.0", 1024);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );
        let repo_pkg = service
            .find_package(&conn, "fedora", "alt-libc", None, None)
            .unwrap();
        let repo_pkg_id = repo_pkg.id.unwrap();
        RepositoryProvide::new(
            repo_pkg_id,
            "ld-linux-x86-64.so.2()(64bit)".to_string(),
            None,
            "virtual".to_string(),
            None,
        )
        .insert(&conn)
        .unwrap();

        let err = ConversionService::ensure_repository_package_not_critical(&conn, &repo_pkg)
            .expect_err("critical repository provide should be refused")
            .to_string();
        assert!(err.contains("Refusing to convert critical runtime capability"));
        assert!(err.contains("ld-linux-x86-64.so.2()(64bit)"));
    }

    #[test]
    fn test_normal_packages_not_blocked() {
        for package_name in ["nginx", "tree", "curl", "jq", "vim"] {
            ConversionService::ensure_package_name_not_critical(package_name).unwrap();
        }
    }
}
