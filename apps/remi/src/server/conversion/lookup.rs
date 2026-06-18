// apps/remi/src/server/conversion/lookup.rs
//! Repository package lookup and one-shot upstream refresh for conversion.

use super::ConversionService;
use anyhow::{Result, anyhow};
use conary_core::db::models::RepositoryPackage;
use conary_core::repository::download_package;
use std::path::{Path, PathBuf};
use tracing::info;

fn repository_package_from_lookup_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<RepositoryPackage> {
    Ok(RepositoryPackage {
        id: row.get(0)?,
        repository_id: row.get(1)?,
        name: row.get(2)?,
        version: row.get(3)?,
        package_release: String::new(),
        architecture: row.get(4)?,
        description: row.get(5)?,
        checksum: row.get(6)?,
        size: row.get(7)?,
        download_url: row.get(8)?,
        dependencies: row.get(9)?,
        metadata: row.get(10)?,
        synced_at: row.get(11)?,
        is_security_update: row.get(12)?,
        severity: row.get(13)?,
        cve_ids: row.get(14)?,
        advisory_id: row.get(15)?,
        advisory_url: row.get(16)?,
        distro: None,
        version_scheme: None,
        canonical_id: None,
    })
}

pub(super) struct PackageDownloadRefresh<'a> {
    pub(super) distro: &'a str,
    pub(super) package_name: &'a str,
    pub(super) version: Option<&'a str>,
    pub(super) architecture: Option<&'a str>,
    pub(super) repo_pkg: RepositoryPackage,
    pub(super) dest_dir: &'a Path,
}

impl ConversionService {
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
            Self::ensure_repository_package_not_critical(&conn, &repo_pkg)?;
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

        let route = conary_core::repository::supported_profiles::route_by_slug(distro)
            .ok_or_else(|| anyhow!("Unknown distribution: {}", distro))?;
        let profile_id = route
            .public_profile_ids()
            .first()
            .ok_or_else(|| anyhow!("No public profile for route: {}", distro))?;
        let profile = conary_core::repository::supported_profiles::profile_by_public_id(profile_id)
            .ok_or_else(|| anyhow!("Profile disappeared for route: {}", distro))?;
        let repo_patterns = profile.repository_name_patterns();
        let scheme = profile.version_scheme();

        // When a specific version is requested, use a simple exact-match query.
        if let Some(ver) = version {
            if let Some(arch) = architecture {
                let mut stmt = conn.prepare(
                    "SELECT rp.id, rp.repository_id, rp.name, rp.version, rp.architecture,
                            rp.description, rp.checksum, rp.size, rp.download_url, rp.dependencies,
                            rp.metadata, rp.synced_at, rp.is_security_update, rp.severity,
                            rp.cve_ids, rp.advisory_id, rp.advisory_url
                     FROM repository_packages rp
                     JOIN repositories r ON rp.repository_id = r.id
                     WHERE rp.name = ?1
                     AND r.name LIKE ?2
                     AND rp.version = ?3
                     AND rp.architecture = ?4
                     AND rp.size > 0
                LIMIT 1",
                )?;

                for repo_pattern in repo_patterns {
                    match stmt.query_row(
                        rusqlite::params![package_name, repo_pattern, ver, arch],
                        repository_package_from_lookup_row,
                    ) {
                        Ok(package) => return Ok(package),
                        Err(rusqlite::Error::QueryReturnedNoRows) => {}
                        Err(e) => return Err(anyhow!("Database error: {}", e)),
                    }
                }
                return Err(anyhow!(
                    "Package '{}' version '{}' arch '{}' not found in {} repositories. Run 'conary repo-sync' first.",
                    package_name,
                    ver,
                    arch,
                    distro
                ));
            }

            let mut stmt = conn.prepare(
                "SELECT rp.id, rp.repository_id, rp.name, rp.version, rp.architecture,
                        rp.description, rp.checksum, rp.size, rp.download_url, rp.dependencies,
                        rp.metadata, rp.synced_at, rp.is_security_update, rp.severity,
                        rp.cve_ids, rp.advisory_id, rp.advisory_url
                 FROM repository_packages rp
                 JOIN repositories r ON rp.repository_id = r.id
                 WHERE rp.name = ?1
                 AND r.name LIKE ?2
                 AND rp.version = ?3
                 AND rp.size > 0
                 LIMIT 1",
            )?;

            for repo_pattern in repo_patterns {
                match stmt.query_row(
                    rusqlite::params![package_name, repo_pattern, ver],
                    repository_package_from_lookup_row,
                ) {
                    Ok(package) => return Ok(package),
                    Err(rusqlite::Error::QueryReturnedNoRows) => {}
                    Err(e) => return Err(anyhow!("Database error: {}", e)),
                }
            }
            return Err(anyhow!(
                "Package '{}' version '{}' not found in {} repositories. Run 'conary repo-sync' first.",
                package_name,
                ver,
                distro
            ));
        }

        // No version specified: fetch all candidates and pick the latest using
        // scheme-aware comparison instead of lexicographic ORDER BY.
        let mut stmt = conn.prepare(
            "SELECT rp.id, rp.repository_id, rp.name, rp.version, rp.architecture,
                    rp.description, rp.checksum, rp.size, rp.download_url, rp.dependencies,
                    rp.metadata, rp.synced_at, rp.is_security_update, rp.severity,
                    rp.cve_ids, rp.advisory_id, rp.advisory_url
             FROM repository_packages rp
             JOIN repositories r ON rp.repository_id = r.id
             WHERE rp.name = ?1
             AND r.name LIKE ?2
             AND (?3 IS NULL OR rp.architecture = ?3)
             AND rp.size > 0",
        )?;

        let mut candidates = Vec::new();
        for repo_pattern in repo_patterns {
            let matched = stmt
                .query_map(
                    rusqlite::params![package_name, repo_pattern, architecture],
                    repository_package_from_lookup_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| anyhow!("Database error: {}", e))?;
            candidates.extend(matched);
        }

        if candidates.is_empty() {
            return Err(anyhow!(
                "Package '{}' not found in {} repositories. Run 'conary repo-sync' first.",
                package_name,
                distro
            ));
        }

        // Pick the latest version using scheme-aware comparison.
        // unwrap is safe because we checked candidates is non-empty above.
        let latest = candidates
            .into_iter()
            .max_by(|a, b| {
                compare_repo_versions(scheme, &a.version, &b.version)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();

        Ok(latest)
    }

    pub(super) async fn download_package_with_refresh_async(
        &self,
        request: PackageDownloadRefresh<'_>,
    ) -> Result<(RepositoryPackage, PathBuf)> {
        let PackageDownloadRefresh {
            distro,
            package_name,
            version,
            architecture,
            repo_pkg,
            dest_dir,
        } = request;
        match download_package(&repo_pkg, dest_dir).await {
            Ok(path) => return Ok((repo_pkg, path)),
            Err(err) if !Self::is_upstream_not_found(&err) => return Err(err.into()),
            Err(err) => {
                info!(
                    "Download for {}:{} hit upstream 404 ({}), refreshing repo {} once",
                    distro, package_name, err, repo_pkg.repository_id
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
            .find_package_for_conversion_async(distro, package_name, version, architecture)
            .await?;
        let path = download_package(&refreshed_pkg, dest_dir)
            .await
            .map_err(|e| anyhow!("Retry after refresh failed: {}", e))?;
        Ok((refreshed_pkg, path))
    }

    fn is_upstream_not_found(err: &conary_core::Error) -> bool {
        match err {
            conary_core::Error::DownloadError(message) => {
                message.contains("HTTP 404") || message.contains("404 Not Found")
            }
            _ => false,
        }
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
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "nginx", "1.24.0", 1024);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let pkg = service
            .find_package(&conn, "fedora", "nginx", None, None)
            .unwrap();
        assert_eq!(pkg.name, "nginx");
        assert_eq!(pkg.version, "1.24.0");
    }

    #[test]
    fn test_find_package_with_specific_version() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, repo_id, "nginx", "1.24.0", 1024);
        insert_package(&conn, repo_id, "nginx", "1.25.0", 1100);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let pkg = service
            .find_package(&conn, "fedora", "nginx", Some("1.24.0"), None)
            .unwrap();
        assert_eq!(pkg.version, "1.24.0");
    }

    #[test]
    fn test_find_package_with_specific_version_and_architecture() {
        let (temp_file, conn) = create_test_db();
        let repo_id = insert_repo(&conn, "fedora-base", "fedora");

        let mut i686 = RepositoryPackage::new(
            repo_id,
            "glib2".to_string(),
            "2.86.0-2.fc44".to_string(),
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
                "fedora",
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
        insert_repo(&conn, "fedora-base", "fedora");

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let result = service.find_package(&conn, "fedora", "nonexistent", None, None);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not found"));
        assert!(err_msg.contains("repo-sync"));
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
        assert!(err_msg.contains("Unknown distribution"));
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
        let repo_id = insert_repo(&conn, "ubuntu-main", "ubuntu");
        insert_package(&conn, repo_id, "libc6", "2.38-1", 2048);

        let service = ConversionService::new(
            PathBuf::from("/tmp/chunks"),
            PathBuf::from("/tmp/cache"),
            temp_file.path().to_path_buf(),
            None,
        );

        let pkg = service
            .find_package(&conn, "ubuntu", "libc6", None, None)
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
        assert!(err.contains("Unknown distribution"));
    }

    #[test]
    fn test_find_package_maps_distro_to_repo_pattern() {
        // Verify all supported distros can resolve to their repo patterns.
        // We insert repos with the expected naming pattern and ensure find_package
        // correctly maps distro name -> LIKE pattern.
        let (temp_file, conn) = create_test_db();

        let arch_id = insert_repo(&conn, "arch-core", "arch");
        insert_package(&conn, arch_id, "vim", "9.0", 500);

        let fed_id = insert_repo(&conn, "fedora-base", "fedora");
        insert_package(&conn, fed_id, "vim", "9.0", 500);

        let ubuntu_id = insert_repo(&conn, "ubuntu-main", "ubuntu");
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
                .find_package(&conn, "fedora", "vim", None, None)
                .is_ok()
        );
        assert!(
            service
                .find_package(&conn, "ubuntu", "vim", None, None)
                .is_ok()
        );
        assert!(
            service
                .find_package(&conn, "debian", "vim", None, None)
                .is_err()
        );
    }

    #[test]
    fn test_detects_upstream_not_found_download_error() {
        let err = conary_core::Error::DownloadError(
            "HTTP 404 Not Found from https://example.com/pkg.rpm".to_string(),
        );
        assert!(ConversionService::is_upstream_not_found(&err));

        let other = conary_core::Error::DownloadError("HTTP 500".to_string());
        assert!(!ConversionService::is_upstream_not_found(&other));
    }
}
