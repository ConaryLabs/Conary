// apps/remi/src/server/conversion/lookup.rs
//! Repository package lookup and one-shot upstream refresh for conversion.

use super::ConversionService;
use anyhow::{Result, anyhow};
use conary_core::db::models::{Repository, RepositoryPackage};
use conary_core::repository::{DownloadOptions, download_package_verified};
use std::path::{Path, PathBuf};
use tracing::info;

pub(super) struct PackageDownloadRefresh<'a> {
    pub(super) profile: &'a str,
    pub(super) package_name: &'a str,
    pub(super) version: Option<&'a str>,
    pub(super) architecture: Option<&'a str>,
    pub(super) repo_pkg: RepositoryPackage,
    pub(super) dest_dir: &'a Path,
}

impl ConversionService {
    async fn download_trusted_repository_package(
        &self,
        repo_pkg: &RepositoryPackage,
        dest_dir: &Path,
    ) -> Result<PathBuf> {
        let db_path = self.db_path.clone();
        let repository_id = repo_pkg.repository_id;
        let trust = tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open_fast(&db_path)?;
            let repository = Repository::find_by_id(&conn, repository_id)?.ok_or_else(|| {
                conary_core::Error::NotFound(format!(
                    "repository {repository_id} not found for package download"
                ))
            })?;
            let keyring = conary_core::db::paths::keyring_dir(&db_path.display().to_string());
            DownloadOptions::for_repository(&repository, &keyring)
        })
        .await
        .map_err(|error| anyhow!("repository trust lookup task panicked: {error}"))??;
        download_package_verified(repo_pkg, dest_dir, &trust)
            .await
            .map_err(anyhow::Error::from)
    }

    pub(super) async fn find_package_for_conversion_async(
        &self,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
    ) -> Result<RepositoryPackage> {
        let service = self.clone();
        let distro = distro.to_string();
        let package_name = package_name.to_string();
        let version = version.map(ToString::to_string);
        let architecture = architecture.map(ToString::to_string);

        tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&service.db_path)?;
            let repo_pkg = service.find_package(
                &conn,
                &distro,
                &package_name,
                version.as_deref(),
                architecture.as_deref(),
            )?;
            Ok(repo_pkg)
        })
        .await
        .map_err(|e| anyhow!("package lookup task panicked: {e}"))?
    }

    pub(super) fn find_package(
        &self,
        conn: &rusqlite::Connection,
        distro: &str,
        package_name: &str,
        version: Option<&str>,
        architecture: Option<&str>,
    ) -> Result<RepositoryPackage> {
        use conary_core::repository::versioning::compare_repo_versions;

        let profile = conary_core::repository::supported_profiles::profile_by_public_id(distro)
            .ok_or_else(|| anyhow!("unsupported public profile: {}", distro))?;
        let scheme = profile.version_scheme();

        // When a specific version is requested, use a simple exact-match query.
        if let Some(ver) = version {
            if let Some(arch) = architecture {
                let sql = format!(
                    "SELECT {}
                     FROM repository_packages rp
                     JOIN repositories r ON rp.repository_id = r.id
                     WHERE rp.name = ?1
                     AND r.source_profile = ?2
                     AND rp.version = ?3
                     AND rp.architecture = ?4
                     AND rp.size > 0
                     LIMIT 1",
                    RepositoryPackage::COLUMNS_PREFIXED
                );
                let mut stmt = conn.prepare(&sql)?;

                return stmt
                    .query_row(
                        rusqlite::params![package_name, distro, ver, arch],
                        RepositoryPackage::from_row,
                    )
                    .map_err(|error| match error {
                        rusqlite::Error::QueryReturnedNoRows => anyhow!(
                            "Package '{}' version '{}' arch '{}' not found for profile {}. Run repository sync first.",
                            package_name,
                            ver,
                            arch,
                            distro
                        ),
                        other => anyhow!("Database error: {other}"),
                    });
            }

            let sql = format!(
                "SELECT {}
                 FROM repository_packages rp
                 JOIN repositories r ON rp.repository_id = r.id
                 WHERE rp.name = ?1
                 AND r.source_profile = ?2
                 AND rp.version = ?3
                 AND rp.size > 0
                 LIMIT 1",
                RepositoryPackage::COLUMNS_PREFIXED
            );
            let mut stmt = conn.prepare(&sql)?;

            return stmt
                .query_row(
                    rusqlite::params![package_name, distro, ver],
                    RepositoryPackage::from_row,
                )
                .map_err(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => anyhow!(
                        "Package '{}' version '{}' not found for profile {}. Run repository sync first.",
                        package_name,
                        ver,
                        distro
                    ),
                    other => anyhow!("Database error: {other}"),
                });
        }

        // No version specified: fetch all candidates and pick the latest using
        // scheme-aware comparison instead of lexicographic ORDER BY.
        let sql = format!(
            "SELECT {}
             FROM repository_packages rp
             JOIN repositories r ON rp.repository_id = r.id
             WHERE rp.name = ?1
             AND r.source_profile = ?2
             AND (?3 IS NULL OR rp.architecture = ?3)
             AND rp.size > 0",
            RepositoryPackage::COLUMNS_PREFIXED
        );
        let mut stmt = conn.prepare(&sql)?;

        let candidates = stmt
            .query_map(
                rusqlite::params![package_name, distro, architecture],
                RepositoryPackage::from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| anyhow!("Database error: {}", e))?;

        if candidates.is_empty() {
            return Err(anyhow!(
                "Package '{}' not found for profile {}. Run repository sync first.",
                package_name,
                distro
            ));
        }

        // Pick the latest version while preserving parse failures as typed
        // lookup errors rather than hiding them inside an infallible sort.
        let mut candidates = candidates.into_iter();
        let mut latest = candidates
            .next()
            .expect("candidate list was checked as non-empty");
        for candidate in candidates {
            if compare_repo_versions(scheme, &candidate.version, &latest.version)?
                == std::cmp::Ordering::Greater
            {
                latest = candidate;
            }
        }

        Ok(latest)
    }

    pub(super) async fn download_package_with_refresh_async(
        &self,
        request: PackageDownloadRefresh<'_>,
    ) -> Result<(RepositoryPackage, PathBuf)> {
        let PackageDownloadRefresh {
            profile,
            package_name,
            version,
            architecture,
            repo_pkg,
            dest_dir,
        } = request;
        match self
            .download_trusted_repository_package(&repo_pkg, dest_dir)
            .await
        {
            Ok(path) => return Ok((repo_pkg, path)),
            Err(err)
                if !err
                    .downcast_ref::<conary_core::Error>()
                    .is_some_and(Self::is_upstream_not_found) =>
            {
                return Err(err);
            }
            Err(err) => {
                info!(
                    "Download for {}:{} hit upstream 404 ({}), refreshing repo {} once",
                    profile, package_name, err, repo_pkg.repository_id
                );
            }
        }

        let db_path = self.db_path.clone();
        let repo_id = repo_pkg.repository_id;
        let repo = tokio::task::spawn_blocking(move || {
            let conn = conary_core::db::open(&db_path)?;
            conary_core::db::models::Repository::find_by_id(&conn, repo_id)?
                .ok_or_else(|| anyhow!("Repository {} not found during refresh", repo_id))
        })
        .await
        .map_err(|e| anyhow!("repository refresh lookup task panicked: {e}"))??;
        let repo_name = repo.name.clone();
        conary_core::repository::sync_repository_from_db_path(self.db_path.clone(), repo)
            .await
            .map_err(|e| anyhow!("Repository refresh failed for {}: {}", repo_name, e))?;

        let refreshed_pkg = self
            .find_package_for_conversion_async(profile, package_name, version, architecture)
            .await?;
        let path = self
            .download_trusted_repository_package(&refreshed_pkg, dest_dir)
            .await
            .map_err(|e| anyhow!("Retry after refresh failed: {}", e))?;
        Ok((refreshed_pkg, path))
    }

    fn is_upstream_not_found(err: &conary_core::Error) -> bool {
        matches!(err, conary_core::Error::HttpStatus { status: 404, .. })
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{create_test_db, insert_package, insert_repo};
    use super::*;
    use conary_core::db::models::RepositoryPackage;
    use std::path::PathBuf;

    #[test]
    fn test_find_package_found() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora-44");
        insert_package(&conn, repo_id, "nginx", "1.24.0", 1024);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let pkg = service
            .find_package(&conn, "fedora-44", "nginx", None, None)
            .unwrap();
        assert_eq!(pkg.name, "nginx");
        assert_eq!(pkg.version, "1.24.0");
    }

    #[test]
    fn test_find_package_with_specific_version() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora-44");
        insert_package(&conn, repo_id, "nginx", "1.24.0", 1024);
        insert_package(&conn, repo_id, "nginx", "1.25.0", 1100);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let pkg = service
            .find_package(&conn, "fedora-44", "nginx", Some("1.24.0"), None)
            .unwrap();
        assert_eq!(pkg.version, "1.24.0");
    }

    #[test]
    fn test_find_package_with_specific_version_and_architecture() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora-44");

        let mut i686 = RepositoryPackage::new(
            repo_id,
            "glib2".to_string(),
            "2.86.0-2.fc44".to_string(),
            conary_core::repository::versioning::VersionScheme::Rpm,
            "sha256:glib2-i686".to_string(),
            1024,
            "https://example.com/glib2-2.86.0-2.fc44.i686.rpm".to_string(),
        );
        i686.architecture = Some("i686".to_string());
        i686.insert(&conn).unwrap();

        let mut x86_64 = RepositoryPackage::new(
            repo_id,
            "glib2".to_string(),
            "2.86.0-2.fc44".to_string(),
            conary_core::repository::versioning::VersionScheme::Rpm,
            "sha256:glib2-x86_64".to_string(),
            2048,
            "https://example.com/glib2-2.86.0-2.fc44.x86_64.rpm".to_string(),
        );
        x86_64.architecture = Some("x86_64".to_string());
        x86_64.insert(&conn).unwrap();

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let pkg = service
            .find_package(
                &conn,
                "fedora-44",
                "glib2",
                Some("2.86.0-2.fc44"),
                Some("x86_64"),
            )
            .unwrap();
        assert_eq!(pkg.architecture.as_deref(), Some("x86_64"));
        assert!(pkg.download_url.ends_with(".x86_64.rpm"));
    }

    #[test]
    fn test_find_package_not_found() {
        let (temp_file, conn) = create_test_db();
        insert_repo(&conn, "fedora-base", "fedora-44");

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let result = service.find_package(&conn, "fedora-44", "nonexistent", None, None);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not found"));
        assert!(err_msg.contains("repository sync"));
    }

    #[test]
    fn test_find_package_unknown_distro() {
        let (temp_file, conn) = create_test_db();

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let result = service.find_package(&conn, "gentoo", "nginx", None, None);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("unsupported public profile"));
    }

    #[test]
    fn test_find_package_arch_distro() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "arch-core", "arch");
        insert_package(&conn, repo_id, "pacman", "6.0.0", 800);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let pkg = service
            .find_package(&conn, "arch", "pacman", None, None)
            .unwrap();
        assert_eq!(pkg.name, "pacman");
    }

    #[test]
    fn test_find_package_ubuntu_distro() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "ubuntu-main", "ubuntu-26.04");
        insert_package(&conn, repo_id, "libc6", "2.38-1", 2048);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let pkg = service
            .find_package(&conn, "ubuntu-26.04", "libc6", None, None)
            .unwrap();
        assert_eq!(pkg.name, "libc6");
    }

    #[test]
    fn test_find_package_debian_is_not_supported_distro() {
        let (temp_file, conn) = create_test_db();

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let err = service
            .find_package(&conn, "debian", "apt", None, None)
            .expect_err("debian is not a supported Remi distro")
            .to_string();
        assert!(err.contains("unsupported public profile"));
    }

    #[test]
    fn test_find_package_uses_exact_persisted_profile() {
        let (temp_file, conn) = create_test_db();

        let arch_id = insert_repo(&conn, "arch-core", "arch");
        insert_package(&conn, arch_id, "vim", "9.0", 500);

        let fed_id = insert_repo(&conn, "fedora-base", "fedora-44");
        insert_package(&conn, fed_id, "vim", "9.0", 500);

        let ubuntu_id = insert_repo(&conn, "ubuntu-main", "ubuntu-26.04");
        insert_package(&conn, ubuntu_id, "vim", "9.0", 500);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        assert!(
            service
                .find_package(&conn, "arch", "vim", None, None)
                .is_ok()
        );
        assert!(
            service
                .find_package(&conn, "fedora-44", "vim", None, None)
                .is_ok()
        );
        assert!(
            service
                .find_package(&conn, "ubuntu-26.04", "vim", None, None)
                .is_ok()
        );
        assert!(
            service
                .find_package(&conn, "debian", "vim", None, None)
                .is_err()
        );
    }

    #[test]
    fn test_detects_typed_upstream_not_found_error() {
        let err = conary_core::Error::HttpStatus {
            status: 404,
            url: "https://example.com/pkg.rpm".to_string(),
        };
        assert!(ConversionService::is_upstream_not_found(&err));

        let other = conary_core::Error::HttpStatus {
            status: 500,
            url: "https://example.com/pkg.rpm".to_string(),
        };
        assert!(!ConversionService::is_upstream_not_found(&other));
    }
}
