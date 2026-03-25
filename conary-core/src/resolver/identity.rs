// conary-core/src/resolver/identity.rs

//! Enriched package identity for resolution.
//!
//! Modeled after libsolv's Solvable: every candidate the resolver considers
//! carries its full provenance (name, version, arch, repo, version_scheme,
//! canonical identity). Loaded from a single join across repository_packages,
//! repositories, and canonical_packages.

use crate::error::Result;
use crate::repository::versioning::VersionScheme;
use rusqlite::{Connection, params};

/// Full package identity for resolution, replacing ConaryPackage and ResolverCandidate.
#[derive(Debug, Clone)]
pub struct PackageIdentity {
    // From repository_packages
    pub repo_package_id: i64,
    pub name: String,
    pub version: String,
    pub architecture: Option<String>,
    pub version_scheme: VersionScheme,

    // From repositories (via join)
    pub repository_id: i64,
    pub repository_name: String,
    pub repository_distro: Option<String>,
    pub repository_priority: i32,

    // From canonical_packages (via canonical_id join, nullable)
    pub canonical_id: Option<i64>,
    pub canonical_name: Option<String>,

    // Installed state (set when matching an installed trove)
    pub installed_trove_id: Option<i64>,
}

impl PackageIdentity {
    /// Load all candidates for a package name across all enabled repos.
    pub fn find_all_by_name(conn: &Connection, name: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT rp.id, rp.name, rp.version, rp.architecture, rp.version_scheme,
                    rp.repository_id, r.name, r.default_strategy_distro, r.priority,
                    rp.canonical_id, cp.name
             FROM repository_packages rp
             JOIN repositories r ON rp.repository_id = r.id
             LEFT JOIN canonical_packages cp ON rp.canonical_id = cp.id
             WHERE rp.name = ?1 AND r.enabled = 1",
        )?;

        let rows = stmt.query_map(params![name], |row| {
            let scheme_str: Option<String> = row.get(4)?;
            let distro_str: Option<String> = row.get(7)?;
            let scheme = parse_version_scheme(scheme_str.as_deref(), distro_str.as_deref());

            Ok(PackageIdentity {
                repo_package_id: row.get(0)?,
                name: row.get(1)?,
                version: row.get(2)?,
                architecture: row.get(3)?,
                version_scheme: scheme,
                repository_id: row.get(5)?,
                repository_name: row.get(6)?,
                repository_distro: row.get(7)?,
                repository_priority: row.get(8)?,
                canonical_id: row.get(9)?,
                canonical_name: row.get(10)?,
                installed_trove_id: None,
            })
        })?;

        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Find all cross-distro equivalent names via canonical_id.
    pub fn find_canonical_equivalents(conn: &Connection, name: &str) -> Result<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT rp2.name FROM repository_packages rp1
             JOIN repository_packages rp2 ON rp1.canonical_id = rp2.canonical_id
             WHERE rp1.name = ?1 AND rp2.name != ?1 AND rp1.canonical_id IS NOT NULL",
        )?;

        let rows = stmt.query_map(params![name], |row| row.get(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}

/// Parse version_scheme string with fallback to distro inference.
fn parse_version_scheme(explicit: Option<&str>, distro: Option<&str>) -> VersionScheme {
    match explicit {
        Some("debian") => VersionScheme::Debian,
        Some("arch") => VersionScheme::Arch,
        Some("rpm") => VersionScheme::Rpm,
        Some(_) => VersionScheme::Rpm,
        None => match distro {
            Some(d) if d.starts_with("debian") || d.starts_with("ubuntu") => VersionScheme::Debian,
            Some(d) if d.starts_with("arch") => VersionScheme::Arch,
            _ => VersionScheme::Rpm,
        },
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
            "INSERT INTO repositories (name, url, enabled, priority, default_strategy_distro)
             VALUES ('fedora-41', 'https://example.com', 1, 10, 'fedora-41')",
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
        assert_eq!(id.repository_name, "fedora-41");
        assert_eq!(id.repository_distro.as_deref(), Some("fedora-41"));
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
             VALUES ('fedora-41', 'https://example.com', 1, 10)",
            [],
        )
        .unwrap();
        let repo_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, canonical_id)
             VALUES (?1, 'nginx', '1.24.0', 'sha256:abc', 1024, 'https://example.com/nginx.rpm', ?2)",
            rusqlite::params![repo_id, canonical_id],
        )
        .unwrap();

        let results = PackageIdentity::find_all_by_name(&conn, "nginx").unwrap();
        assert_eq!(results[0].canonical_id, Some(canonical_id));
        assert_eq!(results[0].canonical_name.as_deref(), Some("nginx-web"));
    }

    #[test]
    fn test_find_all_by_name_version_scheme_inference() {
        let (_temp, conn) = create_test_db();

        // Repo with debian distro, no explicit version_scheme on package
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority, default_strategy_distro)
             VALUES ('debian-bookworm', 'https://example.com', 1, 10, 'debian-bookworm')",
            [],
        )
        .unwrap();
        let repo_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url)
             VALUES (?1, 'nginx', '1.22.1', 'sha256:def', 2048, 'https://example.com/nginx.deb')",
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
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, canonical_id)
             VALUES (?1, 'httpd', '2.4', 'sha256:a', 100, 'https://f.com/httpd', ?2)",
            rusqlite::params![fed_repo, canonical_id],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, canonical_id)
             VALUES (?1, 'apache2', '2.4', 'sha256:b', 100, 'https://d.com/apache2', ?2)",
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
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url)
             VALUES (?1, 'nginx', '1.0', 'sha256:x', 100, 'https://example.com/x')",
            [repo_id],
        )
        .unwrap();

        let results = PackageIdentity::find_all_by_name(&conn, "nginx").unwrap();
        assert!(results.is_empty());
    }
}
