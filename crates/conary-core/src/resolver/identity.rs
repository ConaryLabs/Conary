// conary-core/src/resolver/identity.rs

//! Enriched package identity for resolution.
//!
//! Modeled after libsolv's Solvable: every candidate the resolver considers
//! carries its full provenance (name, version, arch, repo, version_scheme,
//! canonical identity). Loaded from a single join across repository_packages,
//! repositories, and canonical_packages.

use crate::error::Result;
use crate::repository::dependency_model::DebianMultiArch;
pub use crate::repository::dependency_model::ProvidedCapability;
use crate::repository::versioning::VersionScheme;
use rusqlite::{Connection, params};

/// Full package identity for resolution, replacing ConaryPackage and ResolverCandidate.
#[derive(Debug, Clone)]
pub struct PackageIdentity {
    // From repository_packages (None for installed-only troves)
    pub repo_package_id: Option<i64>,
    pub name: String,
    pub version: String,
    pub package_release: Option<String>,
    pub architecture: Option<String>,
    pub debian_multi_arch: Option<DebianMultiArch>,
    pub version_scheme: VersionScheme,

    // From repositories (via join; absent for installed-only troves)
    pub repository_id: Option<i64>,
    pub repository_name: String,
    pub repository_profile: Option<String>,
    pub repository_priority: i32,

    // From canonical_packages (via canonical_id join, nullable)
    pub canonical_id: Option<i64>,
    pub canonical_name: Option<String>,

    // Installed state (set when matching an installed trove)
    pub installed_trove_id: Option<i64>,
    pub installed_pinned: bool,

    // Capabilities this package provides with their exact comparison scheme.
    pub provided_capabilities: Vec<ProvidedCapability>,
}

impl PackageIdentity {
    /// Load all candidates for a package name across all enabled repos.
    pub fn find_all_by_name(conn: &Connection, name: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT rp.id, rp.name, rp.version, rp.package_release, rp.architecture, rp.debian_multi_arch, rp.version_scheme,
                    rp.repository_id, r.name, r.source_profile, r.priority,
                    rp.canonical_id, cp.name
             FROM resolved_repository_packages rp
             JOIN repositories r ON rp.repository_id = r.id
             LEFT JOIN resolved_canonical_packages cp ON rp.canonical_id = cp.id
             WHERE rp.name = ?1 AND r.enabled = 1",
        )?;

        let rows = stmt.query_and_then(params![name], |row| -> Result<PackageIdentity> {
            let package_name: String = row.get(1)?;
            let repository_name: String = row.get(8)?;
            let scheme = crate::db::models::version_scheme_from_row(row, 6)?;
            let package_release: String = row.get(3)?;
            let debian_multi_arch = row
                .get::<_, Option<String>>(5)?
                .map(|value| DebianMultiArch::parse_exact(&value))
                .transpose()
                .map_err(crate::error::Error::ConfigError)?;

            Ok(PackageIdentity {
                repo_package_id: row.get(0)?,
                name: package_name,
                version: row.get(2)?,
                package_release: (!package_release.is_empty()).then_some(package_release),
                architecture: row.get(4)?,
                debian_multi_arch,
                version_scheme: scheme,
                repository_id: Some(row.get(7)?),
                repository_name,
                repository_profile: row.get(9)?,
                repository_priority: row.get(10)?,
                canonical_id: row.get(11)?,
                canonical_name: row.get(12)?,
                installed_trove_id: None,
                installed_pinned: false,
                provided_capabilities: Vec::new(),
            })
        })?;

        rows.collect()
    }

    /// Find all cross-distro equivalent names via canonical_id.
    pub fn find_canonical_equivalents(conn: &Connection, name: &str) -> Result<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT rp2.name FROM resolved_repository_packages rp1
             JOIN resolved_repository_packages rp2 ON rp1.canonical_id = rp2.canonical_id
             WHERE rp1.name = ?1 AND rp2.name != ?1 AND rp1.canonical_id IS NOT NULL",
        )?;

        let rows = stmt.query_map(params![name], |row| row.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::create_test_db;

    #[test]
    fn test_find_all_by_name_returns_enriched_identity() {
        let (_temp, conn) = create_test_db();

        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority, source_profile)
             VALUES ('fedora-44', 'https://example.com', 1, 10, 'fedora-44')",
            [],
        )
        .unwrap();
        let repo_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, architecture, checksum, size, download_url, version_scheme)
             VALUES (?1, 'nginx', '1.24.0', 'x86_64', 'sha256:abc', 1024, 'https://example.com/nginx.rpm', 'rpm')",
            [repo_id],
        )
        .unwrap();

        let results = PackageIdentity::find_all_by_name(&conn, "nginx").unwrap();
        assert_eq!(results.len(), 1);

        let id = &results[0];
        assert_eq!(id.name, "nginx");
        assert_eq!(id.version, "1.24.0");
        assert_eq!(id.architecture.as_deref(), Some("x86_64"));
        assert_eq!(id.version_scheme, VersionScheme::Rpm);
        assert_eq!(id.repository_name, "fedora-44");
        assert_eq!(id.repository_profile.as_deref(), Some("fedora-44"));
        assert_eq!(id.repository_priority, 10);
        assert!(id.canonical_id.is_none());
    }

    #[test]
    fn test_find_all_by_name_includes_canonical() {
        let (_temp, conn) = create_test_db();

        conn.execute(
            "INSERT INTO canonical_packages (name, kind) VALUES ('nginx-web', 'package')",
            [],
        )
        .unwrap();
        let canonical_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('fedora-44', 'https://example.com', 1, 10)",
            [],
        )
        .unwrap();
        let repo_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, canonical_id, version_scheme)
             VALUES (?1, 'nginx', '1.24.0', 'sha256:abc', 1024, 'https://example.com/nginx.rpm', ?2, 'rpm')",
            rusqlite::params![repo_id, canonical_id],
        )
        .unwrap();

        let results = PackageIdentity::find_all_by_name(&conn, "nginx").unwrap();
        assert_eq!(results[0].canonical_id, Some(canonical_id));
        assert_eq!(results[0].canonical_name.as_deref(), Some("nginx-web"));
    }

    #[test]
    fn package_identity_loads_repository_package_release() {
        let (_temp, conn) = create_test_db();
        conn.execute(
            "INSERT INTO repositories (name, url, enabled)
             VALUES ('fedora', 'https://example.test', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO repository_packages
             (repository_id, name, version, package_release, checksum, size, download_url, version_scheme)
             VALUES (1, 'hello', '1.0.0', '2', 'sha256:hello', 42, '/v1/chunks/hello', 'rpm')",
            [],
        )
        .unwrap();

        let packages = PackageIdentity::find_all_by_name(&conn, "hello").unwrap();

        assert_eq!(packages[0].package_release.as_deref(), Some("2"));
    }

    #[test]
    fn test_find_all_by_name_uses_persisted_version_scheme() {
        let (_temp, conn) = create_test_db();

        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority, source_profile)
             VALUES ('ubuntu-main', 'https://example.com', 1, 10, 'ubuntu-26.04')",
            [],
        )
        .unwrap();
        let repo_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repository_packages (
                 repository_id, name, version, debian_multi_arch, checksum, size,
                 download_url, version_scheme
             ) VALUES (
                 ?1, 'nginx', '1.22.1', 'no', 'sha256:def', 2048,
                 'https://example.com/nginx.deb', 'debian'
             )",
            [repo_id],
        )
        .unwrap();

        let results = PackageIdentity::find_all_by_name(&conn, "nginx").unwrap();
        assert_eq!(results[0].version_scheme, VersionScheme::Debian);
    }

    #[test]
    fn test_find_canonical_equivalents() {
        let (_temp, conn) = create_test_db();

        conn.execute(
            "INSERT INTO canonical_packages (name, kind) VALUES ('apache-web', 'package')",
            [],
        )
        .unwrap();
        let canonical_id = conn.last_insert_rowid();

        // Two repos, same canonical_id
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('fedora', 'https://f.com', 1, 10)",
            [],
        )
        .unwrap();
        let fed_repo = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('debian', 'https://d.com', 1, 10)",
            [],
        )
        .unwrap();
        let deb_repo = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, canonical_id, version_scheme)
             VALUES (?1, 'httpd', '2.4', 'sha256:a', 100, 'https://f.com/httpd', ?2, 'rpm')",
            rusqlite::params![fed_repo, canonical_id],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO repository_packages (
                 repository_id, name, version, debian_multi_arch, checksum, size,
                 download_url, canonical_id, version_scheme
             ) VALUES (
                 ?1, 'apache2', '2.4', 'no', 'sha256:b', 100,
                 'https://d.com/apache2', ?2, 'debian'
             )",
            rusqlite::params![deb_repo, canonical_id],
        )
        .unwrap();

        let equivs = PackageIdentity::find_canonical_equivalents(&conn, "httpd").unwrap();
        assert_eq!(equivs, vec!["apache2"]);

        let equivs = PackageIdentity::find_canonical_equivalents(&conn, "apache2").unwrap();
        assert_eq!(equivs, vec!["httpd"]);
    }

    #[test]
    fn test_find_all_by_name_excludes_disabled_repos() {
        let (_temp, conn) = create_test_db();

        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('disabled-repo', 'https://example.com', 0, 10)",
            [],
        )
        .unwrap();
        let repo_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, version_scheme)
             VALUES (?1, 'nginx', '1.0', 'sha256:x', 100, 'https://example.com/x', 'rpm')",
            [repo_id],
        )
        .unwrap();

        let results = PackageIdentity::find_all_by_name(&conn, "nginx").unwrap();
        assert!(results.is_empty());
    }
}
